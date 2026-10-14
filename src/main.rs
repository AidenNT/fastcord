// En release (Windows) no abre consola negra detrás de la ventana.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]
// El binario no dibuja nada acá: solo arma la ventana y delega
// - la lógica/estado a `lib::state::App`
// - el dibujado a los módulos de `ui`
mod bidi;
mod discord;
mod lib;
mod logging;
mod paths;
mod single_instance;
mod support;
mod system_fonts;
mod theme;
mod ui;
mod updater;

use lib::state::App;

// Ver el comentario de `mimalloc` en Cargo.toml: baja el RSS tras los picos de
// parseo del gateway porque devuelve las páginas libres al sistema. Sigue
// siendo mimalloc; `CountingAlloc` solo lo envuelve para llevar la cuenta de
// los bytes de heap vivos (contador de memoria, ver `support::mem_report`).
#[global_allocator]
static GLOBAL: support::mem_report::CountingAlloc = support::mem_report::CountingAlloc;

/// Ícono de la ventana (PNG embebido en el binario).
fn load_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/packaging/icon-256.png");
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let img = img.into_rgba8();
            let (width, height) = img.dimensions();
            egui::IconData { rgba: img.into_raw(), width, height }
        }
        Err(_) => egui::IconData::default(),
    }
}

fn main() -> eframe::Result<()> {
    // Lo PRIMERO, antes del captcha, del logger y de la instancia única:
    // si este proceso es el ayudante de una actualización (`--apply-update`)
    // instala y sale sin abrir ventana; si no, quita las banderas del updater.
    updater::intercept();
    // `ecord --version`: el ayudante de la versión anterior lo ejecuta para
    // comprobar el binario descargado antes de instalarlo.
    updater::answer_version_probe();
    // Actualización con permisos de administrador (copia en Program Files):
    // `--elevated-update` actualiza sin ventana y `--deelevate` se relanza sin
    // privilegios. Si es una de las dos, no vuelve.
    updater::handle_elevation_flags();
    // Windows/macOS: este mismo ejecutable también es la ventana del captcha
    // de hCaptcha (ver `discord::captcha`): en ese caso no se arranca eframe.
    // En Linux la ventana es el ejecutable aparte `ecord-captcha`, para que
    // `ecord` no dependa de WebKitGTK/GTK.
    #[cfg(not(target_os = "linux"))]
    if std::env::args().any(|arg| arg == discord::captcha::WINDOW_FLAG) {
        discord::captcha::run_window();
    }
    env_logger::init();
    // Una sola ventana de ecord: si ya hay una abierta, se le pide que se
    // muestre y esta instancia se cierra (ver `single_instance`).
    if single_instance::acquire() == single_instance::Role::Secondary {
        return Ok(());
    }
    // La ventana transparente da parpadeo (frames que alternan entre pantallas)
    // en Linux dentro de ChromeOS (Crostini: Wayland vía sommelier + GL virtual).
    // Ahí se pide opaca; también se puede forzar con `ECORD_OPAQUE=1`.
    let opaque = std::env::var_os("ECORD_OPAQUE").is_some()
        || std::env::var_os("SOMMELIER_VERSION").is_some()
        || std::path::Path::new("/dev/.cros_milestone").exists();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 600.0])
            // Sin decoraciones nativas: `ui::shell::topbar` dibuja su propia barra
            // (arrastre + minimizar/maximizar/cerrar), al estilo Fastpotify.
            .with_decorations(false)
            // Necesario para que un tema pueda ser transparente (ver
            // `theme::Backdrop::background_opacity`). No se puede cambiar
            // con la ventana ya creada, así que se pide siempre; con un tema
            // opaco el fondo se pinta al 100 % y no cambia nada. Si el
            // sistema no tiene compositor, la transparencia se ve negra.
            .with_transparent(!opaque)
            // Ícono de la ventana / barra de tareas.
            .with_icon(load_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "eCord",
        options,
        Box::new(|cc| {
            // Fuentes (Inter + emoji) e íconos Lucide se registran una sola
            // vez acá, contra el contexto real de la ventana.
            ui::theme::install(&cc.egui_ctx);
            // Desde acá una segunda instancia puede pedirle foco a esta ventana.
            single_instance::set_context(&cc.egui_ctx);
            Ok(Box::new(App::default()))
        }),
    )
}
