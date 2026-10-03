//! Dónde guarda ecord su estado en disco (cookies y identificadores de
//! sesión de Discord). Vive en el directorio de estado del usuario:
//!
//! * Linux: `$XDG_STATE_HOME/ecord` (normalmente `~/.local/state/ecord`)
//! * Windows / macOS: no existe un "state dir" propio, así que cae al de
//!   datos locales (`%LOCALAPPDATA%\ecord\data`, `~/Library/Application Support/ecord`).
//!
//! Devuelven `Option` porque `directories` no puede resolver la carpeta si
//! no hay un HOME válido; quien llama trata `None` como "no persistir".

use std::path::PathBuf;

use directories::ProjectDirs;

const APP_NAME: &str = "ecord";

fn state_dir() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("", "", APP_NAME)?;
    Some(
        dirs.state_dir()
            .unwrap_or_else(|| dirs.data_local_dir())
            .to_path_buf(),
    )
}

/// Carpeta de caché (borrable sin perder nada): imágenes y JSON bajados.
fn cache_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", APP_NAME).map(|dirs| dirs.cache_dir().to_path_buf())
}

/// Videos cortos (GIFs de Tenor/Giphy en mp4) ya bajados, listos para que
/// ffmpeg los abra desde disco (`ui::video_player::local_video`).
pub fn video_cache_dir() -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join("video"))
}

/// Imágenes/bytes bajados por HTTP (`support::http_cache`).
pub fn http_cache_dir() -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join("http"))
}

/// JSON chicos cacheados (catálogo de efectos, productos de coleccionables).
pub fn json_cache_dir() -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join("json"))
}

/// Audios de las alertas sonoras volcados desde el binario para que ffmpeg los
/// pueda abrir por ruta (`support::sound_alerts`). Se regeneran solos.
pub fn sound_cache_dir() -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join("sounds"))
}

/// Cookie jar de Discord (JSON de `cookie_store`).
pub fn discord_cookie_file() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("discord_cookies.json"))
}

/// Cuentas guardadas (JSON con los tokens de cada cuenta, ver
/// `lib::accounts`). Es un archivo privado: se escribe con `write_private_file`.
pub fn accounts_file() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("accounts.json"))
}

/// Identificadores de sesión del "browser" de Discord (TOML): fingerprint
/// anónimo e installation id.
pub fn discord_browser_file() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("discord_browser.toml"))
}
