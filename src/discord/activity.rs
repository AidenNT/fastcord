//! Cuántas requests a la API REST de Discord hay "en vuelo" ahora mismo.
//!
//! Lo usa la barra superior (`ui::topbar`) para mostrar un spinner mientras
//! se le está hablando a Discord. Las requests nacen en hilos de fondo
//! (cada `spawn_*` tiene el suyo), así que el contador es global y atómico:
//!
//! * `uwu_rest::CaptchaClient::run` (por donde pasa TODO el REST) y
//!   `uwu_rest::connected_client` toman un [`RequestGuard`] con [`begin`]
//!   alrededor de la llamada de red;
//! * el guard resta solo cuando se destruye, así que cubre también los
//!   `return`/`?` tempranos y los errores;
//! * la UI lo lee con [`is_busy`] una vez por frame.
//!
//! eframe no repinta si no pasa nada, y el hilo de Discord no despierta a la
//! UI al llegar un evento. Por eso, al empezar y al terminar una request se
//! pide un repintado con el `Context` que registra
//! [`set_repaint_context`]: sin eso el spinner aparecería (y se iría) recién
//! en el siguiente repintado de respaldo, hasta medio segundo después.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static REPAINT: OnceLock<egui::Context> = OnceLock::new();

/// Registra el `Context` al que hay que despertar cuando cambia el contador.
/// Se llama una sola vez (`App::poll_discord_events`); las siguientes no
/// hacen nada.
pub fn set_repaint_context(ctx: &egui::Context) {
    let _ = REPAINT.set(ctx.clone());
}

fn wake_ui() {
    if let Some(ctx) = REPAINT.get() {
        ctx.request_repaint();
    }
}

/// Marca una request como en vuelo mientras viva este valor.
#[must_use = "la request deja de contarse apenas se destruye el guard"]
pub struct RequestGuard(());

/// Empieza a contar una request. Guardá el resultado en una variable
/// (`let _busy = activity::begin();`, NO `let _ = ...`: ese descarta el
/// guard enseguida) y soltalo cuando la respuesta ya llegó.
pub fn begin() -> RequestGuard {
    IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    wake_ui();
    RequestGuard(())
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
        wake_ui();
    }
}

/// Cantidad de requests en vuelo.
pub fn in_flight() -> usize {
    IN_FLIGHT.load(Ordering::Relaxed)
}

/// `true` si hay al menos una request en vuelo.
pub fn is_busy() -> bool {
    in_flight() > 0
}
