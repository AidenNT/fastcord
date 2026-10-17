//! Paquete opcional "FFmpeg GPL" (libx264) para transmitir pantalla en Windows.
//!
//! eCord se distribuye bajo MIT y en Windows lleva un FFmpeg LGPL, que NO trae
//! `libx264` (GPL). Para que quien quiera pueda transmitir con x264 sin que el
//! instalador de eCord lo incluya, el release publica aparte
//! `ecord-vX-ffmpeg-gpl-x86_64-pc-windows-msvc.zip`: las mismas DLL de FFmpeg
//! (misma versión, mismos nombres) pero compiladas con `--enable-gpl
//! --enable-libx264`. Este módulo:
//!
//! 1. decide si hay que avisar ([`should_ask`]): solo en Windows, si el FFmpeg
//!    cargado no tiene `libx264` y la persona nunca contestó. Se pregunta UNA
//!    sola vez: la respuesta (aceptar / rechazar) queda guardada;
//! 2. baja ese zip del release de la MISMA versión de eCord, comprueba su
//!    SHA-256 contra `checksums.txt` y la firma Ed25519 de `checksums.txt.sig`
//!    (la misma clave pública que usa el actualizador) y copia las DLL junto
//!    al .exe ([`start_download`]);
//! 3. deja las DLL LGPL originales como `*.lgpl-bak` para poder volver atrás
//!    ([`revert`]).
//!
//! Windows no deja pisar una DLL en uso pero sí RENOMBRARLA, así que la
//! instalación funciona con eCord abierto; las DLL nuevas se cargan al
//! reiniciar. Hasta entonces se sigue transmitiendo con OpenH264.
//!
//! Si el FFmpeg ya trae x264 (Linux, macOS, un FFmpeg propio) nada de esto se
//! activa: la detección es [`crate::discord::voice::x264_available`], no un
//! archivo marcador.
//!
//! Actualizaciones de eCord: el actualizador puede reponer las DLL LGPL. Si la
//! persona ya aceptó, [`maintain`] (una vez por arranque) lo nota —las libs
//! cargadas no traen x264— y baja e instala SIN preguntar el paquete de la
//! versión nueva. Surte efecto en el siguiente reinicio; si el actualizador
//! dejó las DLL GPL intactas no hace nada.

#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Clave de `web_local_storage_api` donde se guarda la decisión de la persona.
const PREF_KEY: &str = "x264_gpl_pref";

/// Qué eligió la persona la última vez que se le preguntó.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pref {
    /// Todavía no contestó: se le pregunta (una sola vez).
    Ask,
    /// Aceptó la licencia: el paquete se instala y se repone solo tras cada
    /// actualización de eCord, sin volver a preguntar.
    Accepted,
    /// Rechazó: se transmite con GPU / OpenH264 y no se vuelve a preguntar.
    Declined,
}

pub fn pref() -> Pref {
    match web_local_storage_api::get_item(PREF_KEY).ok().flatten().as_deref() {
        Some("accepted") => Pref::Accepted,
        Some("declined") => Pref::Declined,
        _ => Pref::Ask,
    }
}

pub fn set_pref(pref: Pref) {
    let value = match pref {
        Pref::Ask => "ask",
        Pref::Accepted => "accepted",
        Pref::Declined => "declined",
    };
    let _ = web_local_storage_api::set_item(PREF_KEY, value);
}

/// Estado de la descarga para la UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Downloading { received: u64, total: u64 },
    /// Instalado: falta reiniciar eCord para cargar las DLL nuevas.
    InstalledRestartNeeded,
    Failed(String),
}

static STATUS: Mutex<Status> = Mutex::new(Status::Idle);
/// Cambio de estado terminal (instalado / falló) que la UI todavía no mostró.
static FINISHED: AtomicU8 = AtomicU8::new(0);
const FINISHED_NONE: u8 = 0;
const FINISHED_PENDING: u8 = 1;

fn status_slot() -> MutexGuard<'static, Status> {
    STATUS.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn status() -> Status {
    status_slot().clone()
}

/// Una sola vez por resultado: `Some` cuando la descarga terminó (bien o mal)
/// y la UI todavía no avisó. Se llama cada frame.
pub fn take_finished() -> Option<Status> {
    if FINISHED
        .compare_exchange(FINISHED_PENDING, FINISHED_NONE, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return None;
    }
    Some(status())
}

/// `true` si hay que mostrar el aviso de la licencia antes de transmitir.
pub fn should_ask() -> bool {
    cfg!(windows)
        && !crate::discord::voice::x264_available()
        && pref() == Pref::Ask
        && matches!(status(), Status::Idle | Status::Failed(_))
}

static MAINTAINED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Llamar cada frame: una sola vez por arranque, si la persona ya aceptó la
/// licencia y las libs cargadas no traen x264 (primer arranque tras una
/// actualización de eCord que repuso las DLL LGPL), baja e instala el paquete
/// de esta versión sin preguntar. No hace nada en el resto de los casos.
pub fn maintain(repaint: impl Fn() + Send + Sync + 'static) {
    if !cfg!(windows) || MAINTAINED.swap(true, Ordering::AcqRel) {
        return;
    }
    if pref() == Pref::Accepted && !crate::discord::voice::x264_available() {
        log::info!("paquete FFmpeg GPL: x264 no está cargado y ya se aceptó la licencia; se repone");
        start_download(repaint);
    }
}

/// Nombre del zip en el release (lo fija `.github/workflows/release.yml`).
pub fn asset_name() -> String {
    format!("ecord-v{}-ffmpeg-gpl-x86_64-pc-windows-msvc.zip", env!("CARGO_PKG_VERSION"))
}

/// Página del release, para que la persona vea los archivos y el código fuente.
pub fn release_page() -> String {
    format!(
        "https://github.com/{}/releases/tag/v{}",
        crate::updater::REPOSITORY,
        env!("CARGO_PKG_VERSION")
    )
}

/// Baja, verifica e instala el paquete en un hilo. `repaint` despierta la UI.
#[cfg(windows)]
pub fn start_download(repaint: impl Fn() + Send + Sync + 'static) {
    {
        let mut status = status_slot();
        if matches!(*status, Status::Downloading { .. } | Status::InstalledRestartNeeded) {
            return;
        }
        *status = Status::Downloading { received: 0, total: 0 };
    }
    let spawned = std::thread::Builder::new().name("ecord-gpl-pack".into()).spawn(move || {
        let result = windows_impl::download_and_install(&|received, total| {
            *status_slot() = Status::Downloading { received, total };
            repaint();
        });
        let next = match result {
            Ok(()) => Status::InstalledRestartNeeded,
            Err(error) => {
                log::error!("paquete FFmpeg GPL: {error:#}");
                Status::Failed(format!("{error:#}"))
            }
        };
        *status_slot() = next;
        FINISHED.store(FINISHED_PENDING, Ordering::Release);
        repaint();
    });
    if let Err(error) = spawned {
        *status_slot() = Status::Failed(error.to_string());
        FINISHED.store(FINISHED_PENDING, Ordering::Release);
    }
}

#[cfg(not(windows))]
pub fn start_download(_repaint: impl Fn() + Send + Sync + 'static) {
    *status_slot() = Status::Failed("el paquete GPL solo existe para Windows".to_owned());
    FINISHED.store(FINISHED_PENDING, Ordering::Release);
}

/// Vuelve a las DLL LGPL (las `*.lgpl-bak` que dejó la instalación). Falta
/// reiniciar para que surta efecto.
#[cfg(windows)]
#[allow(dead_code)]
pub fn revert() -> Result<usize, String> {
    windows_impl::revert().map_err(|error| format!("{error:#}"))
}

#[cfg(windows)]
mod windows_impl {
    use std::fs;
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result, anyhow, bail};
    use sha2::{Digest, Sha256};

    use super::{asset_name, release_page};

    const BACKUP_SUFFIX: &str = "lgpl-bak";
    /// Tope de descarga: las DLL de FFmpeg GPL pesan decenas de MB.
    const MAX_ZIP_BYTES: u64 = 400 * 1024 * 1024;
    const MAX_TEXT_BYTES: u64 = 1024 * 1024;

    fn release_url(file: &str) -> String {
        format!(
            "https://github.com/{}/releases/download/v{}/{file}",
            crate::updater::REPOSITORY,
            env!("CARGO_PKG_VERSION")
        )
    }

    fn client() -> Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .user_agent(concat!("ecord/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .context("no se pudo crear el cliente HTTP")
    }

    fn fetch_text(client: &reqwest::blocking::Client, file: &str) -> Result<Vec<u8>> {
        let response = client
            .get(release_url(file))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .with_context(|| format!("no se pudo bajar {file} (¿esta versión ya publicó el paquete? {})", release_page()))?;
        let mut bytes = Vec::new();
        response.take(MAX_TEXT_BYTES).read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn hex_decode(text: &str) -> Option<Vec<u8>> {
        let text = text.trim();
        if text.len() % 2 != 0 {
            return None;
        }
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
            .collect()
    }

    /// Comprueba la firma Ed25519 de `checksums.txt` con la clave pública que
    /// lleva la app (la misma del actualizador) y devuelve el SHA-256 que
    /// figura ahí para `name`.
    fn signed_sha256(client: &reqwest::blocking::Client, name: &str) -> Result<String> {
        let key_hex = crate::updater::UPDATES
            .publisher_key
            .ok_or_else(|| anyhow!("esta copia de eCord no tiene clave pública para verificar descargas"))?;
        let key = hex_decode(key_hex).ok_or_else(|| anyhow!("la clave pública de eCord no es hexadecimal válida"))?;
        let checksums = fetch_text(client, "checksums.txt")?;
        let signature = fetch_text(client, "checksums.txt.sig")?;
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &key)
            .verify(&checksums, &signature)
            .map_err(|_| anyhow!("la firma de checksums.txt no es válida: no se instala nada"))?;
        let text = String::from_utf8(checksums).context("checksums.txt no es texto")?;
        text.lines()
            .filter_map(|line| {
                let (hash, file) = line.split_once(char::is_whitespace)?;
                let file = file.trim().trim_start_matches('*');
                (file == name).then(|| hash.to_ascii_lowercase())
            })
            .next()
            .ok_or_else(|| anyhow!("{name} no figura en checksums.txt"))
    }

    fn exe_dir() -> Result<PathBuf> {
        let exe = std::env::current_exe().context("no se pudo ubicar ecord.exe")?;
        exe.parent().map(Path::to_path_buf).ok_or_else(|| anyhow!("ecord.exe no tiene carpeta"))
    }

    pub fn download_and_install(progress: &(dyn Fn(u64, u64) + Send + Sync)) -> Result<()> {
        let name = asset_name();
        let client = client()?;
        // Primero lo firmado: si no hay hash válido ni se baja el zip.
        let expected = signed_sha256(&client, &name)?;

        let mut response = client
            .get(release_url(&name))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .with_context(|| format!("no se pudo bajar {name}"))?;
        let total = response.content_length().unwrap_or(0);
        if total > MAX_ZIP_BYTES {
            bail!("{name} pesa demasiado ({total} bytes)");
        }
        let mut hasher = Sha256::new();
        let mut data = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = response.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            data.extend_from_slice(&buffer[..read]);
            if data.len() as u64 > MAX_ZIP_BYTES {
                bail!("{name} pesa demasiado");
            }
            progress(data.len() as u64, total);
        }
        let actual: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
        if actual != expected {
            bail!("el SHA-256 de {name} no coincide con el firmado: no se instala nada");
        }
        install(&data)
    }

    /// Solo `*.dll` y los textos de licencia, planos (sin subcarpetas ni `..`).
    fn wanted(name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        !name.is_empty()
            && !name.contains(['/', '\\'])
            && (lower.ends_with(".dll") || lower.ends_with(".txt"))
    }

    fn install(zip_bytes: &[u8]) -> Result<()> {
        let dir = exe_dir()?;
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).context("el zip está dañado")?;

        // Todo a archivos temporales primero: si algo falla no queda mezclado.
        let mut staged: Vec<(String, PathBuf)> = Vec::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            if entry.is_dir() {
                continue;
            }
            // El zip del release es plano; se ignora cualquier ruta rara.
            let Some(file_name) = entry.enclosed_name().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            else {
                continue;
            };
            if !wanted(&file_name) {
                continue;
            }
            let temp = dir.join(format!("{file_name}.gpl-new"));
            let mut out = fs::File::create(&temp).with_context(|| {
                format!("no se pudo escribir en {} (¿carpeta protegida? mové eCord a una carpeta tuya)", dir.display())
            })?;
            std::io::copy(&mut entry, &mut out)?;
            out.flush()?;
            staged.push((file_name, temp));
        }
        if !staged.iter().any(|(name, _)| name.to_ascii_lowercase().starts_with("avcodec")) {
            for (_, temp) in &staged {
                let _ = fs::remove_file(temp);
            }
            bail!("el paquete no trae avcodec: se descarta");
        }

        let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
        let result = (|| -> Result<()> {
            for (file_name, temp) in &staged {
                let target = dir.join(file_name);
                let mut backup = None;
                if target.exists() {
                    let bak = dir.join(format!("{file_name}.{BACKUP_SUFFIX}"));
                    let _ = fs::remove_file(&bak);
                    // Renombrar sí se puede aunque la DLL esté cargada.
                    fs::rename(&target, &bak).with_context(|| format!("no se pudo apartar {file_name}"))?;
                    backup = Some(bak);
                }
                if let Err(error) = fs::rename(temp, &target) {
                    if let Some(bak) = &backup {
                        let _ = fs::rename(bak, &target);
                    }
                    return Err(error).with_context(|| format!("no se pudo instalar {file_name}"));
                }
                done.push((target, backup));
            }
            Ok(())
        })();

        if let Err(error) = result {
            // Deshacer lo ya copiado.
            for (target, backup) in done.into_iter().rev() {
                let _ = fs::remove_file(&target);
                if let Some(bak) = backup {
                    let _ = fs::rename(bak, target);
                }
            }
            for (_, temp) in &staged {
                let _ = fs::remove_file(temp);
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn revert() -> Result<usize> {
        let dir = exe_dir()?;
        let mut restored = 0;
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let Some(original) = file_name.strip_suffix(&format!(".{BACKUP_SUFFIX}")) else { continue };
            let target = dir.join(original);
            let _ = fs::remove_file(&target);
            if fs::rename(&path, &target).is_err() {
                // La DLL en uso no se borra: se aparta y se repone.
                let moved = dir.join(format!("{original}.gpl-old"));
                let _ = fs::remove_file(&moved);
                fs::rename(&target, &moved)?;
                fs::rename(&path, &target)?;
            }
            restored += 1;
        }
        Ok(restored)
    }
}
