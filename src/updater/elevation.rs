//! Actualizar una copia instalada en una carpeta protegida (típicamente
//! `C:\Program Files\eCord`, donde instala el .msi).
//!
//! El ayudante de `fastframe-update` corre con los permisos de quien lo
//! lanzó. Si eCord se abrió sin ser administrador, reemplazar `ecord.exe` en
//! Program Files falla con "acceso denegado". Este módulo resuelve eso así:
//!
//! 1. [`needs_elevation`]: la UI pregunta, antes de instalar, si la carpeta del
//!    ejecutable es escribible. Si no lo es, muestra un aviso propio.
//! 2. Con el visto bueno del usuario, [`request_elevated_update`] vuelve a
//!    lanzar este mismo ejecutable con el verbo `runas` (el aviso UAC de
//!    Windows) y la bandera [`ELEVATED_FLAG`], y la ventana se cierra.
//! 3. La instancia elevada ([`run_if_requested`]) no abre ventana: busca la
//!    versión, la descarga y le pasa el relevo al ayudante. Como el ayudante
//!    nace de un proceso elevado, hereda los permisos y puede reemplazar el
//!    ejecutable.
//! 4. El ayudante reabre eCord con [`DEELEVATE_FLAG`]. Esa copia
//!    ([`finish_deelevate_if_requested`]) confirma la actualización, se
//!    vuelve a lanzar a través de `explorer.exe` (que la abre SIN privilegios)
//!    y sale, para no dejar a eCord corriendo como administrador.
//!
//! Solo hace algo en Windows; en el resto de sistemas `needs_elevation` es
//! siempre `false`.

use std::sync::{Mutex, MutexGuard, PoisonError};

use fastframe_update::{ReqwestTransport, Updater};

use super::{UPDATES, launch_slot, relaunch_arguments};

/// Bandera con la que se relanza eCord, elevado, para que solo actualice.
pub const ELEVATED_FLAG: &str = "--elevated-update";
/// Bandera con la que el ayudante reabre eCord tras una actualización
/// elevada: la copia que la recibe se relanza sin privilegios y sale.
pub const DEELEVATE_FLAG: &str = "--deelevate";

/// Error de un intento de elevar (UAC cancelado, `ShellExecuteW` falló...)
/// que la UI muestra como aviso en el próximo frame.
static ERROR: Mutex<Option<String>> = Mutex::new(None);

fn error_slot() -> MutexGuard<'static, Option<String>> {
    ERROR.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Último error al pedir permisos (una sola vez).
pub fn take_error() -> Option<String> {
    error_slot().take()
}

fn has_flag(flag: &str) -> bool {
    std::env::args_os().skip(1).any(|arg| arg == flag)
}

/// `true` si este proceso es la instancia elevada que solo actualiza.
pub fn is_elevated_run() -> bool {
    has_flag(ELEVATED_FLAG)
}

/// `true` si, para reemplazar el ejecutable, hacen falta permisos de
/// administrador: la carpeta del `.exe` existe pero no se puede escribir en
/// ella. Prueba creando (y borrando) un archivo temporal. Ante cualquier otro
/// error devuelve `false`, para no cambiar el comportamiento de siempre.
pub fn needs_elevation() -> bool {
    #[cfg(windows)]
    {
        let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
        else {
            return false;
        };
        let probe = dir.join(format!(".ecord-write-test-{}", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(file) => {
                drop(file);
                let _ = std::fs::remove_file(&probe);
                false
            }
            Err(error) => error.kind() == std::io::ErrorKind::PermissionDenied,
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Pide permisos de administrador (aviso UAC) y relanza eCord como
/// actualizador. Corre en un hilo, porque `ShellExecuteW` bloquea hasta que el
/// usuario contesta el aviso y no hay que congelar la UI. Si sale bien llama a
/// `quit` (cierra la ventana); si el usuario cancela o falla, deja el motivo
/// para [`take_error`] y llama a `repaint`. La actualización descargada sigue
/// lista: se puede volver a intentar.
pub fn request_elevated_update(repaint: impl Fn() + Send + 'static, quit: impl FnOnce() + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("ecord-update-elevate".into())
        .spawn(move || match launch_elevated() {
            Ok(()) => quit(),
            Err(message) => {
                log::warn!("updater: no se pudo elevar: {message}");
                *error_slot() = Some(message);
                repaint();
            }
        });
    if let Err(error) = spawned {
        log::error!("no se pudo lanzar el hilo de elevación: {error}");
        *error_slot() = Some(error.to_string());
    }
}

#[cfg(windows)]
fn launch_elevated() -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    let exe = std::env::current_exe().map_err(|error| format!("no se encontró el ejecutable: {error}"))?;
    // Primero la bandera; después, los argumentos con los que había que
    // reabrir eCord (sin las banderas del updater).
    let parameters = std::iter::once(ELEVATED_FLAG.to_owned())
        .chain(relaunch_arguments())
        .map(|arg| quote_argument(&arg))
        .collect::<Vec<_>>()
        .join(" ");

    let verb = wide("runas".as_ref());
    let file = wide(exe.as_os_str());
    let parameters = wide(parameters.as_ref());
    // SAFETY: los tres búferes terminan en NUL y viven hasta que vuelve la
    // llamada; el resto de los punteros son nulos, que `ShellExecuteW` admite.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_HIDE,
        )
    };
    // Más de 32 = éxito; hasta 32 es un código de error.
    let code = result as isize;
    if code > 32 {
        return Ok(());
    }
    // 5 (SE_ERR_ACCESSDENIED) es lo que devuelve cuando se cancela el UAC.
    if code == 5 {
        Err("Se canceló el permiso de administrador, así que no se instaló la actualización.".into())
    } else {
        Err(format!("Windows no pudo pedir el permiso de administrador (código {code})."))
    }
}

#[cfg(not(windows))]
fn launch_elevated() -> Result<(), String> {
    Err("Elevar permisos solo hace falta en Windows.".into())
}

/// Comillas para la línea de comandos de Windows.
#[cfg_attr(not(windows), allow(dead_code))]
fn quote_argument(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    // Las barras invertidas del final se duplican para no "escapar" la
    // comilla de cierre.
    let trailing = arg.chars().rev().take_while(|c| *c == '\\').count();
    let body = arg.replace('"', "\\\"");
    format!("\"{body}{}\"", "\\".repeat(trailing))
}

/// Si este proceso es la instancia elevada (`--elevated-update`), actualiza
/// sin abrir ventana y sale; si no, no hace nada. Llamar justo después de
/// `intercept`/`answer_version_probe` y antes de abrir cualquier ventana.
pub fn run_if_requested() {
    if !is_elevated_run() {
        return;
    }
    // No hay ventana: una notificación evita que parezca que eCord se cerró
    // sin motivo mientras se descarga.
    crate::lib::notifications::send_desktop(
        "eCord se está actualizando".into(),
        "Se está descargando la versión nueva. eCord vuelve a abrirse solo.".into(),
    );
    let code = match run_elevated_update() {
        Ok(()) => 0,
        Err(message) => {
            log::error!("updater (elevado): {message}");
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("eCord")
                .set_description(format!("No se pudo actualizar eCord.\n\n{message}"))
                .show();
            // La actualización falló: que al menos vuelva a abrirse eCord.
            relaunch_deelevated();
            1
        }
    };
    std::process::exit(code);
}

fn run_elevated_update() -> Result<(), String> {
    let transport = ReqwestTransport::new(reqwest::blocking::Client::builder())
        .map_err(|error| format!("no se pudo crear el cliente HTTP: {error:#}"))?;
    let updater = Updater::new(UPDATES, transport);
    let release = updater
        .check()
        .map_err(|error| format!("no se pudo consultar la última versión: {error:#}"))?
        .ok_or_else(|| "Ya tenés la última versión.".to_owned())?;
    let prepared = updater
        .download(&release, |_received, _total| {})
        .map_err(|error| format!("falló la descarga de {}: {error:#}", release.version))?;
    // `relaunch_arguments` agrega `--deelevate` porque esta ejecución es la
    // elevada. Cuando `handoff` vuelve, el ayudante ya vigila este proceso y
    // espera a que salga para reemplazar el ejecutable.
    updater
        .handoff(prepared, relaunch_arguments())
        .map_err(|error| format!("no se pudo iniciar la actualización: {error:#}"))
}

/// Si eCord se abrió con `--deelevate` (lo hace el ayudante después de una
/// actualización elevada): confirma al ayudante que la versión nueva arrancó,
/// se relanza sin privilegios y sale. Llamar antes de `single_instance`.
pub fn finish_deelevate_if_requested() {
    if !has_flag(DEELEVATE_FLAG) {
        return;
    }
    // Esta copia es la primera de la versión nueva: si no se confirma acá, el
    // ayudante creería que no arrancó y restauraría la anterior.
    acknowledge_receipt_now();
    relaunch_deelevated();
    std::process::exit(0);
}

/// Como `acknowledge_receipt`, pero sin hilo: el proceso sale enseguida y el
/// hilo podría no llegar a correr.
fn acknowledge_receipt_now() {
    let receipt = launch_slot().as_mut().and_then(|launch| launch.receipt.take());
    if let Some(receipt) = receipt {
        if let Err(error) = receipt.acknowledge() {
            log::error!("no se pudo confirmar la actualización al ayudante: {error:#}");
        }
    }
}

/// Abre eCord otra vez con los permisos normales del usuario. `explorer.exe`
/// lanza el programa con su propio token (sin elevar), que es el truco
/// habitual para "bajar" de administrador. Si falla, abre eCord directo (aunque
/// quede elevado): es mejor que dejar al usuario sin la app.
fn relaunch_deelevated() {
    #[cfg(windows)]
    {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        if let Err(error) = std::process::Command::new("explorer.exe").arg(&exe).spawn() {
            log::error!("no se pudo reabrir eCord con explorer.exe: {error}");
            if let Err(error) = std::process::Command::new(&exe).spawn() {
                log::error!("no se pudo reabrir eCord: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn los_argumentos_simples_van_sin_comillas() {
        assert_eq!(quote_argument("--flag"), "--flag");
    }

    #[test]
    fn los_argumentos_con_espacios_o_comillas_se_entrecomillan() {
        assert_eq!(quote_argument("a b"), "\"a b\"");
        assert_eq!(quote_argument("di\"ce"), "\"di\\\"ce\"");
        assert_eq!(quote_argument(""), "\"\"");
        assert_eq!(quote_argument("C:\\mi carpeta\\"), "\"C:\\mi carpeta\\\\\"");
    }
}
