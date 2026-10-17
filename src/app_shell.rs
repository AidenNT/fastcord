//! Pegamento entre eCord y `fastframe-shell` / `fastframe-tray`.
//!
//! eframe ata el loop de eventos a la ventana. Para que eCord siga conectado
//! (mensajes, llamadas, notificaciones) con la ventana cerrada, `main` corre
//! `eframe::run_native` dentro de un `fastframe_shell::Shell`: al cerrar la
//! ventana con la bandeja disponible, la ventana se destruye, el `App` queda
//! guardado y un loop sin ventana lo sigue llamando hasta que algo pide la
//! ventana de nuevo (bandeja, clic en una notificación, segunda instancia).
//!
//! Como cada ventana tiene su propio `egui::Context` (y sin ventana no hay
//! ninguno), los hilos de fondo NO guardan un contexto: despiertan a la UI
//! con [`wake`], que repinta la ventana si existe o corta la espera del loop
//! sin ventana si no.

use std::sync::OnceLock;
use std::time::Instant;
use std::sync::atomic::{AtomicBool, Ordering};

use fastframe_shell::{Held, StallWatch, Waker};
use fastframe_tray::{Config, MenuItem, Tray};

use crate::lib::state::App;

/// Ids de las entradas del menú de la bandeja (los que devuelve `Event::Menu`).
pub const MENU_SHOW: &str = "show";
pub const MENU_QUIT: &str = "quit";

static WAKER: OnceLock<Waker> = OnceLock::new();
/// El usuario pidió salir de verdad (menú "Salir", actualización): cerrar la
/// ventana entonces NO manda eCord a la bandeja.
static QUIT: AtomicBool = AtomicBool::new(false);
/// Alguien (segunda instancia, clic en una notificación) pidió ver la ventana.
static SHOW: AtomicBool = AtomicBool::new(false);

/// El `Waker` único de la app. Se clona para el `Shell` y para la bandeja.
pub fn waker() -> &'static Waker {
    WAKER.get_or_init(Waker::default)
}

/// Segundos desde que arrancó el proceso. Reloj propio (y no el de egui) para
/// los temporizadores que también corren sin ventana: cada ventana nueva
/// tiene un contexto de egui cuyo tiempo vuelve a empezar de cero.
pub fn uptime_secs() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Despierta a la UI desde cualquier hilo (ventana abierta o no).
pub fn wake() {
    waker().wake();
}

/// Pide salir del todo (no esconder a la bandeja) y despierta a la UI.
pub fn request_quit() {
    QUIT.store(true, Ordering::SeqCst);
    wake();
}

pub fn quit_requested() -> bool {
    QUIT.load(Ordering::SeqCst)
}

/// Pide que se vea la ventana: la restaura si existe, la crea si no.
pub fn request_show() {
    SHOW.store(true, Ordering::SeqCst);
    wake();
}

/// `true` una sola vez por pedido de [`request_show`].
pub fn take_show_request() -> bool {
    SHOW.swap(false, Ordering::SeqCst)
}

// ---------------------------------------------------------------------
// Bandeja
// ---------------------------------------------------------------------

/// Crea el ícono de bandeja (o la barra de menú en macOS). `None` si el
/// sistema no tiene (GNOME sin extensión, sin D-Bus...): entonces cerrar la
/// ventana cierra eCord, como antes.
pub fn spawn_tray() -> Option<Tray> {
    Tray::spawn(
        Config {
            id: "ecord",
            title: "eCord".into(),
            icon: tray_icon_rgba,
            template_icon: None,
            themed_icon: true,
            menu: vec![
                MenuItem::action(MENU_SHOW, "Mostrar u ocultar eCord"),
                MenuItem::Separator,
                MenuItem::action(MENU_QUIT, "Salir de eCord"),
            ],
        },
        wake,
    )
}

/// Ícono de la app (el mismo PNG de la ventana) a `size` x `size`, RGBA.
fn tray_icon_rgba(size: usize) -> Vec<u8> {
    static BASE: OnceLock<image::RgbaImage> = OnceLock::new();
    let base = BASE.get_or_init(|| {
        image::load_from_memory(include_bytes!("../assets/packaging/icon-256.png"))
            .map(image::DynamicImage::into_rgba8)
            .unwrap_or_else(|_| image::RgbaImage::new(1, 1))
    });
    let side = size.max(1) as u32;
    image::imageops::resize(base, side, side, image::imageops::FilterType::Lanczos3).into_raw()
}

// ---------------------------------------------------------------------
// La ventana
// ---------------------------------------------------------------------

/// El `eframe::App` de UNA ventana. Guarda el `App` prestado por el `Shell`
/// (`Held`): al destruirse la ventana, el `App` vuelve al `Shell`.
pub struct Window {
    app: Held<App>,
    stalls: StallWatch,
    recovery_checked: bool,
}

impl Window {
    pub fn new(app: Held<App>) -> Self {
        Self {
            app,
            stalls: StallWatch::default(),
            recovery_checked: false,
        }
    }
}

impl eframe::App for Window {
    /// Corre aun con la ventana minimizada u oculta por el compositor (eframe
    /// salta `ui` en esos casos): acá se decide "esconder a la bandeja".
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !std::mem::replace(&mut self.recovery_checked, true) {
            fastframe_shell::window::recover_offscreen(ctx, frame);
        }
        self.stalls.logic(|| self.app.window_logic(ctx));
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.stalls.ui(|| eframe::App::ui(&mut *self.app, ui, frame));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::App::save(&mut *self.app, storage);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        eframe::App::clear_color(&*self.app, visuals)
    }
}
