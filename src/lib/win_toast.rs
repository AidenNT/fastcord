//! Windows: notificación de escritorio directa con WinRT.
//!
//! Reemplaza (en Windows) el camino de `fastframe-notifications`, que antes de
//! mostrar el toast consulta `ToastNotifier::Setting()`. Para una app sin
//! instalar (`cargo run`) esa consulta falla con 0x80070490 («Elemento no
//! encontrado») aunque el toast sí se pueda mostrar, y la librería lo
//! convierte en un genérico «delivery failed». Acá un error de `Setting` no
//! impide mostrar el aviso, y todo fallo se loguea con su HRESULT real.

#![cfg(windows)]

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use windows::{
    Data::Xml::Dom::XmlDocument,
    Foundation::TypedEventHandler,
    UI::Notifications::{NotificationSetting, ToastNotification, ToastNotificationManager},
    Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, REG_SZ, RegCloseKey, RegCreateKeyW, RegSetValueExW},
    core::{HSTRING, IInspectable, Ref},
};

/// Mismo id que usa `lib::notifications`.
pub const AUMID: &str = "dev.ecord.client";

/// Cuántos toasts recientes se mantienen vivos (si se sueltan, se pierde el clic).
const KEEP: usize = 32;
static LIVE: Mutex<VecDeque<ToastNotification>> = Mutex::new(VecDeque::new());

/// Logo de eCord (el mismo PNG del ícono de la ventana).
const LOGO: &[u8] = include_bytes!("../../assets/packaging/icon-256.png");

/// Escribe el logo en la caché (si falta o cambió) y devuelve su ruta.
fn logo_file() -> Option<PathBuf> {
    static FILE: OnceLock<Option<PathBuf>> = OnceLock::new();
    FILE.get_or_init(|| {
        let path = crate::paths::toast_icon_file()?;
        let up_to_date = std::fs::metadata(&path).map(|m| m.len()).ok() == Some(LOGO.len() as u64);
        if !up_to_date {
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::write(&path, LOGO).ok()?;
        }
        Some(path)
    })
    .clone()
}

/// `file:///C:/ruta/con%20espacios.png`: el toast pide un URI, no una ruta.
fn file_uri(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file:///");
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn set_reg_string(key: HKEY, name: &str, value: &str) -> windows::core::Result<()> {
    let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `wide` vive durante la llamada y termina en NUL.
    unsafe {
        let bytes = std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2);
        RegSetValueExW(key, &HSTRING::from(name), None, REG_SZ, Some(bytes)).ok()
    }
}

/// Registra el nombre y el logo del AUMID en `HKCU\Software\Classes\AppUserModelId`.
pub fn register_identity(aumid: &str, display_name: &str) -> windows::core::Result<()> {
    let path = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{aumid}"));
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyW(HKEY_CURRENT_USER, &path, &mut key).ok()?;
    }
    let mut result = set_reg_string(key, "DisplayName", display_name);
    if result.is_ok() {
        if let Some(logo) = logo_file() {
            result = set_reg_string(key, "IconUri", &logo.to_string_lossy());
        }
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Descarga (o reutiliza de la caché) el avatar y devuelve su ruta local.
/// Bloquea: llamar solo desde un hilo propio, nunca desde la UI.
fn avatar_file(url: &str) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    // Un avatar animado (`a_...`) viene como .gif: el toast solo muestra un
    // cuadro, así que se pide directamente como PNG.
    let url = url.replacen(".gif?", ".png?", 1);
    let digest = Sha256::digest(url.as_bytes());
    let mut name = String::new();
    for byte in &digest[..8] {
        let _ = write!(name, "{byte:02x}");
    }
    let dir = crate::paths::toast_avatar_dir()?;
    let path = dir.join(format!("{name}.png"));
    if path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Some(path);
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    let bytes = client.get(&url).send().ok()?.error_for_status().ok()?.bytes().ok()?;
    if bytes.is_empty() {
        return None;
    }
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, &bytes).ok()?;
    prune_avatars(&dir);
    Some(path)
}

/// Borra los avatares con más de 3 días para que la carpeta no crezca sin fin.
fn prune_avatars(dir: &std::path::Path) {
    let limit = std::time::Duration::from_secs(3 * 24 * 3600);
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > limit);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Muestra un toast silencioso (eCord ya reproduce su propio sonido). Un clic
/// trae la ventana al frente. Con `avatar_url` el toast lleva esa foto (recortada
/// en círculo); sin avatar NO lleva imagen (nada de logo como relleno). El
/// avatar se baja en un hilo aparte, así que en ese caso el error de `show`
/// solo queda en el log.
pub fn send(title: &str, body: &str, avatar_url: Option<&str>) -> windows::core::Result<()> {
    let Some(url) = avatar_url else {
        return show(title, body, None);
    };
    let (title, body, url) = (title.to_owned(), body.to_owned(), url.to_owned());
    let _ = std::thread::Builder::new().name("ecord-toast-avatar".into()).spawn(move || {
        let avatar = avatar_file(&url);
        if avatar.is_none() {
            log::debug!("toast: no se pudo bajar el avatar, se muestra sin imagen");
        }
        if let Err(err) = show(&title, &body, avatar.as_deref()) {
            log::warn!(
                "no se pudo mostrar la notificación de escritorio: {err:?} (HRESULT {:#010x})",
                err.code().0 as u32
            );
        }
    });
    Ok(())
}

fn show(title: &str, body: &str, image: Option<&std::path::Path>) -> windows::core::Result<()> {
    if let Err(err) = register_identity(AUMID, "eCord") {
        log::warn!("no se pudo registrar el AUMID en el registro: {err:?}");
    }
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?;
    // Solo informativo: un error acá (0x80070490) NO impide mostrar el toast.
    match notifier.Setting() {
        Ok(NotificationSetting::Enabled) => {}
        Ok(other) => log::warn!("Windows reporta las notificaciones de eCord como {other:?}"),
        Err(err) => log::debug!("ToastNotifier::Setting falló (se ignora): {err:?}"),
    }

    let image = image
        .map(|path| {
            format!(
                "<image placement=\"appLogoOverride\" hint-crop=\"circle\" src=\"{}\"/>",
                escape(&file_uri(path))
            )
        })
        .unwrap_or_default();
    let xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text>{image}</binding></visual><audio silent=\"true\"/></toast>",
        escape(title),
        escape(body)
    );
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    toast.Activated(&TypedEventHandler::new(|_, _: Ref<'_, IInspectable>| {
        crate::app_shell::request_show();
        Ok(())
    }))?;
    notifier.Show(&toast)?;

    let mut live = LIVE.lock().unwrap_or_else(|p| p.into_inner());
    live.push_back(toast);
    while live.len() > KEEP {
        live.pop_front();
    }
    Ok(())
}
