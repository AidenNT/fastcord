//! Actualizaciones automáticas de ecord (crate `fastframe-update`, dependencia de git
//! fijada a un commit en `Cargo.toml`).
//!
//! Este módulo no dibuja nada y no conoce a `App`: guarda el estado en un
//! `static` y lo expone con [`view`]. La UI (`ui::settings::updates`) lo lee
//! cada frame y decide qué mostrar. Qué hace cada pieza:
//!
//! * [`intercept`] va PRIMERO en `main`: si ecord se abrió como ayudante de la
//!   actualización (`ecord --apply-update <job>`) instala y sale sin abrir
//!   ventana; si no, saca de la línea de comandos las banderas del updater
//!   (`--update-receipt`, `--update-error`).
//! * [`start`] lanza un hilo que busca una versión nueva al arrancar y una vez
//!   por día, y la descarga y verifica (firma Ed25519 + SHA-256) en segundo
//!   plano. Nunca instala nada solo: eso espera a [`install_and_quit`].
//! * [`install_and_quit`] pasa el relevo al ayudante y pide cerrar la ventana;
//!   el ayudante reemplaza el ejecutable y vuelve a abrir ecord. Si la versión
//!   nueva no arranca, restaura la anterior.
//!
//! * [`elevation`] cubre las copias instaladas en una carpeta protegida (el
//!   .msi instala en Program Files): pide permisos de administrador (UAC)
//!   con un aviso previo, actualiza desde una instancia elevada sin ventana y
//!   reabre eCord sin privilegios.
//!
//! Qué NO hace: actualizar copias instaladas por un gestor de paquetes (.deb,
//! Flatpak, Homebrew, `cargo install`...) ni copias sin el archivo marcador de
//! `packaging/` junto al ejecutable. En esos casos [`view`] devuelve
//! [`View::Manual`] con el motivo y el enlace a la página de la versión.

mod elevation;

use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use fastframe_update::{
    CHECK_INTERVAL, DownloadState, Launch, MacConfig, Receipt, Release, ReqwestTransport, UpdateConfig, Unsupported,
    Updater,
};

/// Repositorio de GitHub (`dueño/nombre`) donde se publican las versiones.
///
/// TODO(antes de publicar): poné el tuyo. Mientras siga con este valor de
/// ejemplo, las actualizaciones quedan desactivadas (ver `UpdateConfig::validate`
/// más abajo y el test `la_configuracion_es_valida`).
pub const REPOSITORY: &str = "AidenNT/fastcord";

/// Cómo se describe ecord al updater. El nombre de comando (`ecord`) define
/// los nombres de los archivos de cada versión (`ecord-v0.1.0-x86_64-unknown-linux-gnu.tar.gz`),
/// el archivo marcador (`ecord-portable.txt`) y lo que imprime `--version`.
pub const UPDATES: UpdateConfig = UpdateConfig {
    macos: MacConfig {
        // El mismo `identifier` de `[package.metadata.packager]` en Cargo.toml.
        bundle_ids: &["dev.ecord.client"],
        executable_names: &[],
        legacy_bundle_names: &[],
    },
    // Clave PÚBLICA Ed25519 (64 caracteres hexadecimales). Cada versión tiene
    // que publicar `checksums.txt` y `checksums.txt.sig` firmado con la clave
    // privada correspondiente; sin firma válida no se descarga nada.
    publisher_key: Some(include_str!("../assets/update-public-key.hex")),
    ..UpdateConfig::new(REPOSITORY, "eCord", "ecord", env!("CARGO_PKG_VERSION"))
};

pub use elevation::{needs_elevation, request_elevated_update, take_error as take_elevation_error};
use elevation::{DEELEVATE_FLAG, ELEVATED_FLAG};

/// Variable de entorno para apagar por completo el chequeo de versiones.
pub const DISABLE_ENV: &str = "ECORD_DISABLE_UPDATES";

/// Lo que la UI tiene que mostrar ahora mismo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    /// Todavía no se buscó nada (o las actualizaciones están apagadas).
    Idle,
    /// Buscando una versión nueva.
    Checking,
    /// Esta es la última versión.
    UpToDate,
    /// No se pudo consultar GitHub (sin red, límite de pedidos...).
    CheckFailed(String),
    /// Hay una versión nueva, pero eCord está en una carpeta protegida
    /// (Program Files): no se baja en segundo plano porque la descarga se
    /// guarda junto al ejecutable. Se baja e instala con permisos de
    /// administrador cuando el usuario lo pide (ver [`elevation`]).
    NeedsElevation { version: String },
    /// Hay una versión nueva pero esta copia no puede instalarla sola.
    Manual { version: String, url: String, reason: String },
    /// Bajando y verificando la versión nueva.
    Downloading { version: String, received: u64, total: u64 },
    /// Lista para instalar: falta reiniciar.
    Ready { version: String },
    /// El ayudante ya arrancó; la ventana está por cerrarse.
    Installing,
    /// Falló la descarga, la verificación o el traspaso.
    Failed(String),
}

struct Shared {
    release: Option<Release>,
    state: DownloadState,
    unsupported: Option<String>,
    checking: bool,
    checked: bool,
    check_error: Option<String>,
    /// La versión nueva está disponible pero hay que bajarla con permisos de
    /// administrador (la carpeta del ejecutable no es escribible).
    needs_elevation: bool,
    /// Pedido de "buscar ahora" que despierta al hilo antes del próximo día.
    wake: bool,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    release: None,
    state: DownloadState::Idle,
    unsupported: None,
    checking: false,
    checked: false,
    check_error: None,
    needs_elevation: false,
    wake: false,
});
static WAKE: Condvar = Condvar::new();
static UPDATER: OnceLock<Updater> = OnceLock::new();
static STARTED: AtomicBool = AtomicBool::new(false);

/// Lo que dejó `intercept` para que lo use el resto del arranque.
struct LaunchInfo {
    arguments: Vec<OsString>,
    receipt: Option<Receipt>,
    error: Option<String>,
}

static LAUNCH: Mutex<Option<LaunchInfo>> = Mutex::new(None);

fn shared() -> MutexGuard<'static, Shared> {
    // Un pánico en el hilo del updater no tiene que tumbar la UI.
    SHARED.lock().unwrap_or_else(PoisonError::into_inner)
}

fn launch_slot() -> MutexGuard<'static, Option<LaunchInfo>> {
    LAUNCH.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Llamar primero en `main`, antes de la ventana del captcha, del logger y de
/// `single_instance`. Si este proceso es el ayudante, no vuelve.
pub fn intercept() {
    let Launch { arguments, receipt, error } = fastframe_update::intercept(&UPDATES);
    *launch_slot() = Some(LaunchInfo { arguments, receipt, error });
}

/// Atiende las banderas de la actualización con permisos elevados
/// (`--elevated-update` y `--deelevate`, ver [`elevation`]). Llamar justo
/// después de [`answer_version_probe`], antes de abrir cualquier ventana y de
/// `single_instance`. Si esta ejecución era una de esas dos, no vuelve.
pub fn handle_elevation_flags() {
    elevation::run_if_requested();
    elevation::finish_deelevate_if_requested();
}

/// `ecord --version` imprime `ecord <versión>` y sale. El ayudante de la
/// versión anterior ejecuta así el ejecutable descargado antes de instalarlo,
/// y rechaza la actualización si no responde exactamente eso. Tiene que
/// resolverse antes de abrir cualquier ventana.
pub fn answer_version_probe() {
    if std::env::args_os().skip(1).any(|arg| arg == "--version") {
        #[allow(clippy::print_stdout)]
        {
            println!("{} {}", UPDATES.slug, UPDATES.current_version);
        }
        std::process::exit(0);
    }
}

/// Argumentos con los que el ayudante vuelve a abrir ecord (sin el nombre del
/// programa ni las banderas del updater).
fn relaunch_arguments() -> Vec<String> {
    let mut arguments: Vec<String> = launch_slot()
        .as_ref()
        .map(|launch| {
            launch
                .arguments
                .iter()
                .skip(1)
                .map(|arg| arg.to_string_lossy().into_owned())
                .filter(|arg| arg != ELEVATED_FLAG && arg != DEELEVATE_FLAG)
                .collect()
        })
        .unwrap_or_default();
    // Si esta ejecución es la instancia elevada, lo que se reabra después de
    // actualizar también lo estaría: `--deelevate` hace que se relance sin
    // privilegios.
    if elevation::is_elevated_run() {
        arguments.push(DEELEVATE_FLAG.to_owned());
    }
    arguments
}

/// Mensaje del ayudante si tuvo que restaurar la versión anterior (una sola
/// vez).
pub fn take_restore_error() -> Option<String> {
    launch_slot().as_mut().and_then(|launch| launch.error.take())
}

/// Si esta ejecución es la primera de una versión recién instalada, avisa al
/// ayudante (en un hilo aparte) de que arrancó bien, para que no restaure la
/// anterior. Llamar una vez dibujado el primer frame. `true` si había recibo.
pub fn acknowledge_receipt() -> bool {
    let Some(receipt) = launch_slot().as_mut().and_then(|launch| launch.receipt.take()) else {
        return false;
    };
    let spawned = std::thread::Builder::new().name("ecord-update-ack".into()).spawn(move || {
        if let Err(error) = receipt.acknowledge() {
            log::error!("no se pudo confirmar la actualización al ayudante: {error:#}");
        }
    });
    if let Err(error) = spawned {
        log::error!("no se pudo lanzar el hilo de confirmación de la actualización: {error}");
    }
    true
}

/// Arranca el hilo que busca y baja versiones nuevas. `repaint` despierta la
/// UI cuando cambia algo. Se puede llamar más de una vez: solo la primera vale.
pub fn start(repaint: impl Fn() + Send + Sync + 'static) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    if std::env::var_os(DISABLE_ENV).is_some_and(|value| value != "0" && !value.is_empty()) {
        log::info!("actualizaciones apagadas por {DISABLE_ENV}");
        return;
    }
    if let Err(error) = UPDATES.validate() {
        log::warn!("actualizaciones desactivadas: la configuración no es válida: {error:#}");
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("ecord-updater".into())
        .spawn(move || worker(&repaint));
    if let Err(error) = spawned {
        log::error!("no se pudo lanzar el hilo del updater: {error}");
    }
}

/// Estado actual para la UI.
pub fn view() -> View {
    let shared = shared();
    match &shared.state {
        DownloadState::Downloading { received, total } => {
            return View::Downloading {
                version: shared.release.as_ref().map(|r| r.version.clone()).unwrap_or_default(),
                received: *received,
                total: *total,
            };
        }
        DownloadState::Ready(prepared) => {
            return View::Ready { version: prepared.version().to_owned() };
        }
        DownloadState::Installing => return View::Installing,
        DownloadState::Failed(message) => return View::Failed(message.clone()),
        DownloadState::Idle => {}
    }
    if shared.checking {
        return View::Checking;
    }
    if let (Some(release), Some(reason)) = (&shared.release, &shared.unsupported) {
        return View::Manual {
            version: release.version.clone(),
            url: release.url.clone(),
            reason: reason.clone(),
        };
    }
    if let (Some(release), true) = (&shared.release, shared.needs_elevation) {
        return View::NeedsElevation { version: release.version.clone() };
    }
    if let Some(error) = &shared.check_error {
        return View::CheckFailed(error.clone());
    }
    if shared.checked { View::UpToDate } else { View::Idle }
}

/// "Buscar actualizaciones" desde Ajustes: despierta al hilo ya, sin esperar
/// al próximo chequeo diario. No hace nada si ya hay una lista para instalar.
pub fn check_now() {
    let mut shared = shared();
    if matches!(
        shared.state,
        DownloadState::Downloading { .. } | DownloadState::Ready(_) | DownloadState::Installing
    ) || shared.checking
    {
        return;
    }
    shared.wake = true;
    WAKE.notify_all();
}

/// Pasa el relevo al ayudante y, cuando este ya vigila el proceso, llama a
/// `quit` para cerrar la ventana. Corre en un hilo: el traspaso copia el
/// ejecutable a la carpeta de trabajo y puede tardar un momento.
pub fn install_and_quit(repaint: impl Fn() + Send + 'static, quit: impl FnOnce() + Send + 'static) {
    let Some(updater) = UPDATER.get().cloned() else {
        return;
    };
    let prepared = {
        let mut shared = shared();
        match std::mem::replace(&mut shared.state, DownloadState::Installing) {
            DownloadState::Ready(prepared) => *prepared,
            other => {
                shared.state = other;
                return;
            }
        }
    };
    repaint();
    let arguments = relaunch_arguments();
    let spawned = std::thread::Builder::new().name("ecord-update-handoff".into()).spawn(move || {
        match updater.handoff(prepared, arguments) {
            Ok(()) => quit(),
            Err(error) => {
                log::error!("no se pudo iniciar la actualización: {error:#}");
                shared().state = DownloadState::Failed(format!("{error:#}"));
                repaint();
            }
        }
    });
    if let Err(error) = spawned {
        log::error!("no se pudo lanzar el hilo del traspaso: {error}");
        shared().state = DownloadState::Failed(error.to_string());
    }
}

fn worker(repaint: &(dyn Fn() + Send + Sync)) {
    let transport = match ReqwestTransport::new(reqwest::blocking::Client::builder()) {
        Ok(transport) => transport,
        Err(error) => {
            log::error!("updater: no se pudo crear el cliente HTTP: {error}");
            return;
        }
    };
    let updater = UPDATER.get_or_init(|| Updater::new(UPDATES, transport)).clone();
    loop {
        run_cycle(&updater, repaint);
        wait_for_next_cycle();
    }
}

/// Duerme hasta el próximo chequeo diario o hasta que alguien pida uno ya.
fn wait_for_next_cycle() {
    let deadline = Instant::now() + CHECK_INTERVAL;
    let mut guard = shared();
    while !guard.wake {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        guard = WAKE.wait_timeout(guard, remaining).unwrap_or_else(PoisonError::into_inner).0;
    }
    guard.wake = false;
}

fn run_cycle(updater: &Updater, repaint: &(dyn Fn() + Send + Sync)) {
    {
        let mut shared = shared();
        match shared.state {
            // Ya hay una versión lista (o instalándose): no hay nada más que buscar.
            DownloadState::Ready(_) | DownloadState::Installing | DownloadState::Downloading { .. } => return,
            // Un fallo anterior no bloquea el reintento.
            DownloadState::Failed(_) => shared.state = DownloadState::Idle,
            DownloadState::Idle => {}
        }
        shared.checking = true;
        shared.check_error = None;
        shared.needs_elevation = false;
    }
    repaint();

    let outcome = updater.check();
    match outcome {
        Err(error) => {
            log::warn!("updater: no se pudo consultar la última versión: {error:#}");
            let mut shared = shared();
            shared.checking = false;
            shared.checked = true;
            shared.check_error = Some(format!("{error:#}"));
        }
        Ok(None) => {
            let mut shared = shared();
            shared.checking = false;
            shared.checked = true;
            shared.release = None;
            shared.unsupported = None;
            shared.needs_elevation = false;
        }
        Ok(Some(release)) => {
            log::info!("updater: hay una versión nueva: {}", release.version);
            let unsupported = updater.installation().err().map(|reason| explain(&reason));
            // La descarga crea una carpeta temporal junto al ejecutable. Si
            // está en una carpeta protegida, hacerlo desde acá falla con
            // "acceso denegado (os error 5)": se deja para la instancia
            // elevada, que baja e instala todo con permisos de administrador.
            let protected = unsupported.is_none() && needs_elevation();
            {
                let mut shared = shared();
                shared.checking = false;
                shared.checked = true;
                shared.release = Some(release.clone());
                shared.unsupported = unsupported.clone();
                shared.needs_elevation = protected;
                if unsupported.is_none() && !protected {
                    shared.state = DownloadState::Downloading { received: 0, total: 0 };
                }
            }
            repaint();
            if unsupported.is_none() && !protected {
                download(updater, &release, repaint);
            }
        }
    }
    repaint();
}

fn download(updater: &Updater, release: &Release, repaint: &(dyn Fn() + Send + Sync)) {
    let mut last_repaint = Instant::now();
    let result = updater.download(release, |received, total| {
        shared().state = DownloadState::Downloading { received, total };
        if last_repaint.elapsed() >= Duration::from_millis(100) {
            last_repaint = Instant::now();
            repaint();
        }
    });
    let mut shared = shared();
    match result {
        Ok(prepared) => {
            log::info!("updater: {} descargada y verificada", prepared.version());
            shared.state = DownloadState::Ready(Box::new(prepared));
        }
        Err(error) => {
            log::error!("updater: falló la descarga de {}: {error:#}", release.version);
            shared.state = DownloadState::Failed(format!("{error:#}"));
        }
    }
}

/// Motivo en castellano por el que esta copia no se actualiza sola.
fn explain(reason: &Unsupported) -> String {
    match reason {
        Unsupported::Flatpak => "Se instaló con Flatpak: actualizala desde tu centro de software.".into(),
        Unsupported::Snap => "Se instaló con Snap: actualizala con snap refresh.".into(),
        Unsupported::Cargo => "Se instaló con cargo install: volvé a correr cargo install.".into(),
        Unsupported::Nix => "Se instaló con Nix: actualizala con Nix.".into(),
        Unsupported::Homebrew => "Se instaló con Homebrew: actualizala con brew upgrade.".into(),
        Unsupported::SystemPackage(manager) => format!(
            "La instaló el gestor de paquetes ({}): actualizala desde ahí o desde tu centro de software.",
            format!("{manager:?}").to_lowercase()
        ),
        Unsupported::SystemDirectory => {
            "Está en una carpeta del sistema: actualizala con tu gestor de paquetes o bajá la nueva versión.".into()
        }
        Unsupported::NotPortable => {
            "Esta copia no viene de una descarga con actualización automática: bajá la nueva versión.".into()
        }
        Unsupported::MoveToApplications => "Mové eCord a Aplicaciones y abrilo desde ahí para actualizar.".into(),
        Unsupported::ForeignBundle => "Esta app no es una versión publicada de eCord.".into(),
        Unsupported::Platform => "No hay descarga para este sistema o procesador.".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_configuracion_es_valida() {
        // Falla mientras `REPOSITORY` siga con el valor de ejemplo si el
        // formato no es `dueño/nombre`, y mientras `assets/update-public-key.hex`
        // no tenga una clave de 64 caracteres hexadecimales.
        UPDATES.validate().unwrap();
    }

    #[test]
    fn el_nombre_de_comando_es_el_que_espera_el_ayudante() {
        assert_eq!(UPDATES.slug, "ecord");
        assert_eq!(UPDATES.current_version, env!("CARGO_PKG_VERSION"));
    }
}
