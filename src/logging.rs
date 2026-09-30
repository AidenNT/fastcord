//! Logging de depuración mínimo, encima de la crate `log` (que ya usa
//! `system_fonts.rs`). Sin un logger inicializado no imprime nada; para
//! verlo agregá `env_logger` (o el que prefieras) y llamalo en `main()`,
//! con `RUST_LOG=debug`.

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
