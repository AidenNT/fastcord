//! Navegación izquierda combinada: rail de servidores + panel de DMs (o de
//! canales, si hay un servidor abierto).
//!
//! Antes `rail::show` y `friends_panel::show`/`channel_list_panel` eran dos
//! (o tres) `egui::Panel::left` independientes, cada uno con su propia
//! barra de usuario ("Aiden") pegada abajo, así que esa barra solo ocupaba
//! el ancho del panel de DMs/canales. Acá se unifican en un solo panel: el
//! rail y el panel secundario se dibujan como dos columnas adentro, y la
//! barra de usuario queda una sola vez, abajo de las dos, ocupando TODO el
//! ancho combinado (rail + panel) — el pedido explícito de la consigna.

use egui::{Frame, Margin, Vec2};

use crate::lib::state::{App, Screen};
use crate::ui::{call_bar, friends_panel, rail, server};

/// Alto de la barra de usuario combinada de abajo.
const FOOTER_HEIGHT: f32 = 64.0;

/// Dibuja el rail de servidores + panel de DMs/canales + barra de usuario,
/// como un único panel izquierdo.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let total_width = rail::WIDTH + friends_panel::WIDTH;
    // La barra de llamada (`ui::call_bar`) se cuela entre el bloque de
    // arriba y la barra de usuario solo mientras estás en una llamada —
    // hay que restarle su alto acá también, si no el contenido de arriba
    // queda tapado por ella en vez de encogerse para hacerle lugar.
    let call_bar_height = if app.voice_target.is_some() { call_bar::HEIGHT } else { 0.0 };

    egui::Panel::left("nav_panel")
        .exact_size(total_width)
        .resizable(false)
        .frame(Frame::new().inner_margin(Margin::same(0)))
        .show(ui, |ui| {
            // Sin esto, egui mete su `item_spacing` por defecto entre el
            // bloque de arriba (rail + lista) y la barra de usuario de
            // abajo, y entre el rail y la lista mismos — un hueco de un
            // color que no es ni el del rail ni el de la lista de al lado.
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let top_height =
                (ui.available_height() - FOOTER_HEIGHT - call_bar_height).max(120.0);

            // Rail (servidores) + panel secundario (DMs o canales), lado a
            // lado, cada uno con un ancho fijo.
            ui.allocate_ui_with_layout(
                Vec2::new(total_width, top_height),
                egui::Layout::left_to_right(egui::Align::Min),
                |ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(rail::WIDTH, top_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_size(Vec2::new(rail::WIDTH, top_height));
                            Frame::new().fill(palette.panel).show(ui, |ui| {
                                ui.set_min_size(Vec2::new(rail::WIDTH, top_height));
                                rail::content(app, ui);
                            });
                        },
                    );

                    ui.allocate_ui_with_layout(
                        Vec2::new(friends_panel::WIDTH, top_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_size(Vec2::new(friends_panel::WIDTH, top_height));
                            Frame::new().fill(palette.window).show(ui, |ui| {
                                ui.set_min_size(Vec2::new(friends_panel::WIDTH, top_height));
                                match app.screen {
                                    Screen::Server(index) => {
                                        server::channel_list_content(app, ui, index);
                                    }
                                    _ => {
                                        friends_panel::content(app, ui);
                                    }
                                }
                            });
                        },
                    );
                },
            );

            call_bar::show(app, ui, total_width);

            // Barra de usuario ("Aiden"): ocupa TODO el ancho combinado de
            // arriba (rail + panel), no solo el del panel secundario.
            Frame::new()
                .fill(palette.panel)
                .stroke(egui::Stroke::new(1.0, palette.outline))
                .show(ui, |ui| {
                    ui.set_width(total_width);
                    // Sin esto, el Frame se pinta del alto justo que
                    // necesita `user_bar` (~48px) en vez de los 64px que
                    // `FOOTER_HEIGHT` le reservó arriba al calcular
                    // `top_height` — la diferencia quedaba sin pintar,
                    // mostrando el negro de fondo debajo de la barra.
                    ui.set_min_height(FOOTER_HEIGHT);
                    friends_panel::user_bar(ui, app);
                });
        });
}