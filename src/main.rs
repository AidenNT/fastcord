//#![windows_subsystem = "windows"]
// El binario no dibuja nada acá: solo arma la ventana y delega
// - la lógica/estado a `lib::state::App`
// - el dibujado a los módulos de `ui`
mod bidi;
mod discord;
mod lib;
mod logging;
mod paths;
mod support;
mod system_fonts;
mod theme;
mod ui;

use lib::state::App;

fn main() -> eframe::Result<()> {
    env_logger::init();
    println!("Hello, world!");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 600.0])
            // Sin decoraciones nativas: `ui::topbar` dibuja su propia barra
            // (arrastre + minimizar/maximizar/cerrar), al estilo Fastpotify.
            .with_decorations(false)
            // Necesario para que un tema pueda ser transparente (ver
            // `theme::Backdrop::background_opacity`). No se puede cambiar
            // con la ventana ya creada, así que se pide siempre; con un tema
            // opaco el fondo se pinta al 100 % y no cambia nada. Si el
            // sistema no tiene compositor, la transparencia se ve negra.
            .with_transparent(true),
        ..Default::default()
    };

    eframe::run_native(
        "eCord",
        options,
        Box::new(|cc| {
            // Fuentes (Inter + emoji) e íconos Lucide se registran una sola
            // vez acá, contra el contexto real de la ventana.
            ui::theme::install(&cc.egui_ctx);
            Ok(Box::new(App::default()))
        }),
    )
}
