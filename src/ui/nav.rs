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

use egui::{Align, CornerRadius, Frame, Layout, Margin, Rect, Stroke, StrokeKind, UiBuilder, Vec2};

use crate::lib::state::{App, Screen};
use crate::ui::{call_bar, friends_panel, rail, server};

/// Alto de la barra de usuario combinada de abajo.
pub(crate) const FOOTER_HEIGHT: f32 = 64.0;

/// Dibuja el rail de servidores + panel de DMs/canales + barra de usuario,
/// como un único panel izquierdo.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    if crate::ui::theme::is_modern() {
        show_modern(app, ui);
        // La fila de píldoras de arriba (pestañas / título / íconos) va a la
        // derecha de este panel, así que se declara justo después de él.
        crate::ui::topbar::show_row(app, ui);
        return;
    }
    let palette = app.palette;
    let total_width = rail::WIDTH + friends_panel::WIDTH;
    // La barra de llamada (`ui::call_bar`) se cuela entre el bloque de
    // arriba y la barra de usuario solo mientras estás en una llamada —
    // hay que restarle su alto acá también, si no el contenido de arriba
    // queda tapado por ella en vez de encogerse para hacerle lugar.
    let call_bar_height =
        if app.voice_target.is_some() && !app.new_call_ui { call_bar::HEIGHT } else { 0.0 };

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
                    // El Frame suma 1px de borde arriba y 1px abajo: se
                    // los restamos para que la barra mida EXACTAMENTE
                    // `FOOTER_HEIGHT` y no se pase 2px del panel.
                    ui.set_min_height(FOOTER_HEIGHT - 2.0);
                    friends_panel::user_bar(ui, app);
                });
        });
}

/// Navegación de la interfaz nueva (estilo CreArts): una columna con tres
/// píldoras flotantes (`rail::content_modern`) y, a su lado, la columna de
/// mensajes directos / canales: buscador en píldora, tarjeta con la lista y,
/// abajo, otra tarjeta con la actividad propia (si hay) y el usuario. La barra
/// de llamada vive abajo, a todo el ancho (`call_bar::show_bottom`), así que
/// acá no hace falta hacerle lugar.
fn show_modern(app: &mut App, ui: &mut egui::Ui) {
    use crate::ui::theme::{self, GAP, PILL_H, PILL_RADIUS};

    let palette = app.palette;
    let gap = GAP as f32;
    let rail_w = rail::MODERN_WIDTH;
    let card_w = friends_panel::MODERN_WIDTH;
    // Margen izquierdo GAP + rail + hueco + columna + GAP/2 (el resto del
    // hueco lo pone la tarjeta de al lado).
    let total_width = gap + rail_w + gap + card_w + gap / 2.0;

    egui::Panel::left("nav_panel")
        .exact_size(total_width)
        .resizable(false)
        .frame(Frame::new().inner_margin(Margin { left: GAP, right: GAP / 2, top: GAP, bottom: GAP }))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let full = ui.available_rect_before_wrap();
            let height = full.height();

            // --- Rail: tres píldoras ---------------------------------------
            let rail_rect = Rect::from_min_size(full.min, Vec2::new(rail_w, height));
            ui.scope_builder(
                UiBuilder::new().max_rect(rail_rect).layout(Layout::top_down(Align::Min)),
                |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    rail::content_modern(app, ui);
                },
            );

            // --- Columna de DMs / canales -----------------------------------
            let col_x = full.min.x + rail_w + gap;

            // En un server, arriba va el banner (se pinta al final, encima de
            // la lista); en el resto, el buscador en píldora.
            let server_screen = match app.screen {
                Screen::Server(i) if i < app.servers.len() => Some(i),
                _ => None,
            };
            let header_h = match server_screen {
                Some(i) if app.servers[i].banner_url.is_some() => server::BANNER_HEIGHT,
                Some(_) => server::NAME_HEADER_HEIGHT,
                None => PILL_H,
            };
            let search_rect =
                Rect::from_min_size(egui::pos2(col_x, full.min.y), Vec2::new(card_w, header_h));
            if server_screen.is_none() {
                ui.painter()
                    .rect_filled(search_rect, CornerRadius::same(PILL_RADIUS), palette.panel);
                ui.painter().rect_stroke(
                    search_rect,
                    CornerRadius::same(PILL_RADIUS),
                    Stroke::new(1.0, palette.outline),
                    StrokeKind::Inside,
                );
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(search_rect.shrink2(Vec2::new(16.0, 0.0)))
                        .layout(Layout::left_to_right(Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        theme::icon(ui, theme::Icon::Search, 15.0, palette.dim);
                        ui.add_space(8.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut app.top_search)
                                .hint_text("Busca o inicia una conversación")
                                .frame(egui::Frame::NONE)
                                .desired_width(ui.available_width()),
                        );
                    },
                );
            }

            // Tarjeta de abajo: actividad propia (opcional) + usuario.
            let has_activity = app.own_activity().is_some();
            let bottom_h = 12.0 * 2.0 + 56.0 + if has_activity { 58.0 + 8.0 } else { 0.0 };
            let bottom_rect = Rect::from_min_size(
                egui::pos2(col_x, full.max.y - bottom_h),
                Vec2::new(card_w, bottom_h),
            );

            // Tarjeta de la lista: entre el buscador y la tarjeta de abajo.
            // En un server la tarjeta arranca DEBAJO del banner (asoma por
            // abajo): las filas pasan por detrás al scrollear.
            let banner_overlap = 40.0;
            let list_top = if server_screen.is_some() {
                search_rect.min.y + banner_overlap
            } else {
                search_rect.max.y + gap
            };
            let list_bottom = (bottom_rect.min.y - gap).max(list_top + 120.0);
            let list_rect = Rect::from_min_max(
                egui::pos2(col_x, list_top),
                egui::pos2(col_x + card_w, list_bottom),
            );
            ui.scope_builder(
                UiBuilder::new().max_rect(list_rect).layout(Layout::top_down(Align::Min)),
                |ui| {
                    // El borde de 1px suma 2px al marco: se restan.
                    let inner = list_rect.size() - Vec2::splat(2.0);
                    Frame::new()
                        .fill(palette.panel)
                        .stroke(Stroke::new(1.0, palette.outline))
                        .corner_radius(CornerRadius::same(theme::CARD_RADIUS))
                        .show(ui, |ui| {
                            ui.set_min_size(inner);
                            ui.set_max_size(inner);
                            ui.spacing_mut().item_spacing = Vec2::ZERO;
                            match app.screen {
                                Screen::Server(index) => {
                                    // El banner lo pinta el bloque de abajo; acá
                                    // solo se deja libre su alto.
                                    let pad = (header_h - banner_overlap + 6.0).max(0.0);
                                    server::channel_list_content_ex(app, ui, index, Some(pad));
                                }
                                _ => friends_panel::content(app, ui),
                            }
                        });
                },
            );

            // Banner del server, encima de la lista. Absorbe los clics para que
            // no lleguen a las filas que quedaron por detrás.
            if let Some(i) = server_screen {
                server::paint_server_banner(ui, &palette, &app.servers[i], search_rect);
                let _ = ui.interact(search_rect, egui::Id::new("server_banner_hit"), egui::Sense::click());
            }

            ui.scope_builder(
                UiBuilder::new().max_rect(bottom_rect).layout(Layout::top_down(Align::Min)),
                |ui| {
                    let inner = bottom_rect.size() - Vec2::splat(2.0 + 24.0);
                    Frame::new()
                        .fill(palette.panel)
                        .stroke(Stroke::new(1.0, palette.outline))
                        .corner_radius(CornerRadius::same(theme::CARD_RADIUS))
                        .inner_margin(Margin::same(12))
                        .show(ui, |ui| {
                            ui.set_min_size(inner);
                            ui.set_max_size(inner);
                            ui.spacing_mut().item_spacing = Vec2::ZERO;
                            if friends_panel::now_playing_card(ui, app) {
                                ui.add_space(8.0);
                            }
                            friends_panel::user_card(ui, app);
                        });
                },
            );
        });
}
