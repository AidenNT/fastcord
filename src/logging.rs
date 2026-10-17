//! Logging de depuración mínimo, encima de la crate `log` (que ya usa
//! `system_fonts.rs`). Sin un logger inicializado no imprime nada; para
//! verlo agregá `env_logger` (o el que prefieras) y llamalo en `main()`,
//! con `RUST_LOG=debug`.

use std::fs::{File, OpenOptions};
use std::io::Write;

/// Captura en memoria de los logs para la consola de las devtools.
pub mod capture;

/// Escribe cada línea al archivo y, si hay, a la consola.
struct Tee {
    file: Option<File>,
    console: Option<File>,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(c) = self.console.as_mut() {
            let _ = c.write_all(buf);
        }
        match self.file.as_mut() {
            Some(f) => f.write(buf),
            None => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(f) = self.file.as_mut() {
            f.flush()?;
        }
        Ok(())
    }
}

/// Si eCord se lanzó desde cmd/PowerShell, se engancha a esa consola SIN
/// abrir una nueva. Si se lanzó con doble click, no hay a qué engancharse
/// y no aparece ninguna ventana.
#[cfg(windows)]
fn parent_console() -> Option<File> {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: llamada simple a la API de Windows, sin punteros.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return None;
    }
    OpenOptions::new().write(true).open("CONOUT$").ok()
}
#[cfg(not(windows))]
fn parent_console() -> Option<File> {
    None
}

/// Reemplaza a `env_logger::init()`: log a archivo y, si hay, a la consola
/// desde la que se lanzó.
pub fn init() {
    let file = crate::paths::log_file().and_then(|path| {
        std::fs::create_dir_all(path.parent()?).ok()?;
        // Si el log anterior pesa mucho, se empieza de cero.
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 5_000_000) {
            let _ = std::fs::remove_file(&path);
        }
        OpenOptions::new().create(true).append(true).open(path).ok()
    });
    let console = parent_console();

    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    if file.is_some() || console.is_some() {
        builder.target(env_logger::Target::Pipe(Box::new(Tee { file, console })));
    }
    // En vez de `builder.init()`: el logger de `env_logger` queda envuelto por
    // el de captura (ver `capture`), que además guarda cada línea en memoria.
    capture::install(builder.build(), crate::ui::devtools::enabled());
    // Registro de tráfico de red (pestaña Tráfico): solo con las devtools activas.
    discord_client_rest::netlog::set_enabled(crate::ui::devtools::enabled());
}

/// Un mensaje de error. Igual que `debug`, pero con nivel `error` — lo usa
/// el subsistema de voz (`discord::voice`, portado de Concord) para fallas
/// que sí importan aunque no corten la conexión (por ejemplo, no poder
/// abrir el dispositivo de audio pedido).
pub fn error(target: &str, message: impl std::fmt::Display) {
    log::error!(target: target, "{message}");
}

/// Un mensaje de depuración. `target` agrupa por área (`"fingerprint"`,
/// `"http"`).
pub fn debug(target: &str, message: impl std::fmt::Display) {
    log::debug!(target: target, "{message}");
}

/// ¿Hay algún logger escuchando el nivel `debug`? Sirve para no armar
/// mensajes caros (p. ej. listas de cookies) que nadie va a leer.
pub fn debug_logging_enabled() -> bool {
    log::log_enabled!(log::Level::Debug)
}
