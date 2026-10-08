//! Barra superior propia, al estilo de la imagen de referencia de
//! Fastpotify: ocupa TODO el ancho de la ventana (por encima del rail, los
//! paneles y el panel central), con flechas de navegación + un buscador al
//! medio + información de estado + controles de ventana a la derecha.
//!
//! La ventana se crea sin decoraciones nativas (`main.rs`), así que esta
//! barra también hace de titlebar: se puede arrastrar, y tiene sus propios
//! botones de minimizar/maximizar/cerrar.

use egui::{Align, CornerRadius, Frame, Layout, Margin, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2, ViewportCommand};

use crate::lib::state::{App, Screen, ToastKind};
use crate::ui::emoji as twemoji;
use crate::ui::extra;
use crate::ui::theme::{self, Icon, Palette};

pub const HEIGHT: f32 = 52.0;

/// Alto de la barra de título fina de la interfaz nueva.
pub const TITLEBAR_HEIGHT: f32 = 32.0;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    if theme::is_modern() {
        show_titlebar(app, ui);
        return;
    }
    let palette = app.palette;
    let reconnecting = app.gateway_disconnected;
    // Dónde quedó la campana de notificaciones (para anclarle el panel).
    let mut bell_rect: Option<egui::Rect> = None;

    egui::Panel::top("app_topbar")
        .exact_size(HEIGHT)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(palette.window)
                .stroke(theme::header_stroke(&palette))
                .inner_margin(Margin::symmetric(12, 0)),
        )
        .show(ui, |ui| {
            let bar_rect = ui.max_rect();

            // Toda la barra sirve para arrastrar la ventana, salvo donde
            // haya controles encima (se dibujan después y ganan el
            // hit-test al estar en una capa más "reciente").
            let drag = ui.interact(bar_rect, ui.id().with("drag"), Sense::click_and_drag());
            if drag.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                toggle_maximize(ui);
            }

            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                // --- Navegación ---
                theme::icon_button(ui, Icon::ArrowLeft, 14.0, palette.dim, palette.text, "Atrás");
                theme::icon_button(ui, Icon::ArrowRight, 14.0, palette.dim, palette.text, "Adelante");
                ui.add_space(10.0);

                // --- Buscador central ---
                Frame::new()
                    .fill(palette.surface)
                    .stroke(egui::Stroke::new(1.0, palette.outline))
                    .corner_radius(CornerRadius::same(18))
                    .inner_margin(Margin::symmetric(14, 7))
                    .show(ui, |ui| {
                        ui.set_width(340.0);
                        ui.horizontal(|ui| {
                            theme::icon(ui, Icon::Search, 14.0, palette.dim);
                            ui.add(
                                egui::TextEdit::singleline(&mut app.top_search)
                                    .hint_text("¿Qué querés buscar?")
                                    .frame(egui::Frame::NONE)
                                    .desired_width(ui.available_width()),
                            );
                        });
                    });

                // --- Estado, acciones y controles de ventana, a la derecha ---
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // El orden acá importa: en un layout right_to_left, lo
                    // primero que se agrega queda más a la derecha, así que
                    // cerrar/maximizar/minimizar van en ese orden.
                    if theme::icon_button(ui, Icon::X, 13.0, palette.dim, palette.text, "Cerrar")
                        .clicked()
                    {
                        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                    }

                    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                    let (restore_icon, restore_tip) = if maximized {
                        (Icon::Shrink, "Restaurar")
                    } else {
                        (Icon::Maximize2, "Maximizar")
                    };
                    if theme::icon_button(ui, restore_icon, 13.0, palette.dim, palette.text, restore_tip)
                        .clicked()
                    {
                        toggle_maximize(ui);
                    }

                    if theme::icon_button(ui, Icon::Minus, 13.0, palette.dim, palette.text, "Minimizar")
                        .clicked()
                    {
                        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
                    }

                    ui.add_space(6.0);

                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
                    let (me_name, me_avatar_url) = match &app.me {
                        Some(me) => (me.display_name().to_string(), me.avatar_url()),
                        None => ("Aiden".to_string(), None),
                    };
                    extra::avatar(ui, rect.center(), 13.0, me_avatar_url.as_deref(), palette.accent, "A", &palette);
                    if resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(&me_name)
                        .clicked()
                    {
                        app.open_modal(
                            &me_name,
                            "Los ajustes de la cuenta todavía no están disponibles en esta demo.",
                        );
                    }

                    // Notificaciones: campana con el contador de pendientes; el
                    // panel se dibuja abajo, ya fuera de la barra.
                    bell_rect = Some(crate::ui::inbox::bell_button(ui, app));

                    if theme::icon_button(ui, Icon::Settings, 14.0, palette.dim, palette.text, "Ajustes")
                        .clicked()
                    {
                        app.settings_open = true;
                    }

                    if theme::icon_button(
                        ui,
                        Icon::Refresh,
                        14.0,
                        palette.dim,
                        palette.text,
                        "Buscar actualizaciones",
                    )
                    .clicked()
                    {
                        app.push_toast(
                            ToastKind::Info,
                            "Buscando actualizaciones",
                            "eCord ya está en la última versión.",
                        );
                    }

                    // Entre las píldoras y los íconos: gira mientras hay
                    // requests a la API de Discord en vuelo.
                    ui.add_space(4.0);
                    activity_spinner(ui, &palette, reconnecting);
                    ui.add_space(4.0);
                    status_pill(ui, &palette, Icon::Zap, &format!("Conectado como {}", me_name));
                    ui.add_space(6.0);
                    status_pill(
                        ui,
                        &palette,
                        Icon::Sparkles,
                        &format!("eCord {}", env!("CARGO_PKG_VERSION")),
                    );

                    // Contador de RAM: un click abre Ajustes → Memoria con el
                    // detalle. Solo se dibuja si entra sin pisar el buscador
                    // (con la ventana angosta no hay lugar). No fuerza
                    // repintados: se actualiza cuando la UI se repinta sola.
                    if ui.available_width() > 150.0 {
                        ui.add_space(6.0);
                        let label = format!(
                            "RAM {}",
                            crate::support::mem_report::fmt_bytes(
                                crate::support::mem_report::resident_now()
                            )
                        );
                        let pill = status_pill_impl(ui, &palette, Icon::Laptop, &label, true)
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Memoria RAM que usa eCord. Click para ver el detalle.");
                        if pill.clicked() {
                            app.settings_tab = crate::ui::settings::SettingsTab::Memory;
                            app.settings_open = true;
                        }
                    }
                });
            });
        });

    if let Some(rect) = bell_rect {
        crate::ui::inbox::show_panel(app, ui.ctx(), rect);
    }
}

// ---------------------------------------------------------------------
// Interfaz nueva (estilo CreArts)
// ---------------------------------------------------------------------
//
// La barra de arriba se parte en dos: una barra de título fina a todo el
// ancho (`show_titlebar`: logo, título centrado, controles de ventana) y, más
// abajo, una fila de píldoras flotantes (`show_row`: pestañas o título de la
// pantalla a la izquierda, íconos a la derecha). El buscador de la fila vive
// dentro de `ui::nav` porque va encima de la tarjeta de mensajes directos.

/// Barra mínima para la pantalla de Login: sin buscador ni estado, solo
/// arrastre de ventana y los controles minimizar/maximizar/cerrar.
///
/// La ventana se crea sin decoraciones nativas (`main.rs`), así que sin esta
/// barra el login no se podría mover, minimizar ni cerrar (en Linux, sobre
/// todo en Wayland, tampoco hay ningún atajo que lo suplante).
pub fn show_login(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;

    egui::Panel::top("app_topbar")
        .exact_size(TITLEBAR_HEIGHT)
        .resizable(false)
        .frame(Frame::new().fill(palette.window).inner_margin(Margin::symmetric(12, 0)))
        .show(ui, |ui| {
            let bar_rect = ui.max_rect();

            let drag = ui.interact(bar_rect, ui.id().with("drag"), Sense::click_and_drag());
            if drag.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                toggle_maximize(ui);
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::icon_button(ui, Icon::X, 12.0, palette.dim, palette.text, "Cerrar").clicked() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                }

                let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                let (restore_icon, restore_tip) = if maximized {
                    (Icon::Shrink, "Restaurar")
                } else {
                    (Icon::Maximize2, "Maximizar")
                };
                if theme::icon_button(ui, restore_icon, 12.0, palette.dim, palette.text, restore_tip).clicked() {
                    toggle_maximize(ui);
                }

                if theme::icon_button(ui, Icon::Minus, 12.0, palette.dim, palette.text, "Minimizar").clicked() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
                }
            });
        });
}

/// Barra de título fina: arrastra la ventana y lleva los controles.
fn show_titlebar(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let reconnecting = app.gateway_disconnected;
    let bg = theme::titlebar_color(&palette);
    let me_name = match &app.me {
        Some(me) => me.display_name().to_string(),
        None => "Aiden".to_string(),
    };

    egui::Panel::top("app_topbar")
        .exact_size(TITLEBAR_HEIGHT)
        .resizable(false)
        .frame(Frame::new().fill(bg).inner_margin(Margin::symmetric(12, 0)))
        .show(ui, |ui| {
            let bar_rect = ui.max_rect();

            let drag = ui.interact(bar_rect, ui.id().with("drag"), Sense::click_and_drag());
            if drag.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                toggle_maximize(ui);
            }

            // Título centrado, como en la referencia.
            ui.painter().text(
                bar_rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("eCord v{} - Conectado como {}", env!("CARGO_PKG_VERSION"), me_name),
                theme::regular(12.0),
                palette.secondary,
            );

            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                // --- Logo + nombre ---
                let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                theme::logo(ui, logo_rect.center(), 18.0, palette.accent, palette.on_accent);
                ui.add_space(8.0);
                theme::text(ui, "ECORD", theme::bold(11.0), palette.secondary);

                // --- Controles de ventana y estado, a la derecha ---
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_button(ui, Icon::X, 12.0, palette.dim, palette.text, "Cerrar")
                        .clicked()
                    {
                        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                    }

                    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                    let (restore_icon, restore_tip) = if maximized {
                        (Icon::Shrink, "Restaurar")
                    } else {
                        (Icon::Maximize2, "Maximizar")
                    };
                    if theme::icon_button(ui, restore_icon, 12.0, palette.dim, palette.text, restore_tip)
                        .clicked()
                    {
                        toggle_maximize(ui);
                    }

                    if theme::icon_button(ui, Icon::Minus, 12.0, palette.dim, palette.text, "Minimizar")
                        .clicked()
                    {
                        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
                    }

                    ui.add_space(6.0);
                    activity_spinner(ui, &palette, reconnecting);

                    // Contador de RAM (solo si entra sin pisar el título).
                    if ui.available_width() > 420.0 {
                        ui.add_space(6.0);
                        let label = format!(
                            "RAM {}",
                            crate::support::mem_report::fmt_bytes(
                                crate::support::mem_report::resident_now()
                            )
                        );
                        let pill = status_pill_impl(ui, &palette, Icon::Laptop, &label, true)
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Memoria RAM que usa eCord. Click para ver el detalle.");
                        if pill.clicked() {
                            app.settings_tab = crate::ui::settings::SettingsTab::Memory;
                            app.settings_open = true;
                        }
                    }
                });
            });
        });
}

/// Fila de píldoras flotantes de la interfaz nueva. Se declara DESPUÉS del
/// panel de navegación (`ui::nav::show`), así ocupa solo el ancho que queda a
/// su derecha: a la izquierda una píldora larga con las pestañas de Amigos
/// (en Inicio) o el título de la pantalla, y a la derecha una chica con
/// actualizaciones y notificaciones.
pub fn show_row(app: &mut App, ui: &mut egui::Ui) {
    if !theme::is_modern() {
        return;
    }
    let palette = app.palette;
    let gap = theme::GAP;
    let pill_h = theme::PILL_H;
    let mut bell_rect: Option<Rect> = None;

    egui::Panel::top("app_row")
        .exact_size(pill_h + (gap as f32) * 2.0)
        .resizable(false)
        .frame(Frame::new().inner_margin(Margin { left: gap / 2, right: 0, top: gap, bottom: gap }))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let full = ui.available_rect_before_wrap();
            let right_w = 116.0;
            let left_w = (full.width() - right_w - gap as f32).max(120.0);
            let left_rect = Rect::from_min_size(full.min, Vec2::new(left_w, pill_h));
            let right_rect = Rect::from_min_size(
                egui::pos2(full.max.x - right_w, full.min.y),
                Vec2::new(right_w, pill_h),
            );

            for rect in [left_rect, right_rect] {
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(theme::PILL_RADIUS), palette.panel);
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same(theme::PILL_RADIUS),
                    Stroke::new(1.0, palette.outline),
                    StrokeKind::Inside,
                );
            }

            // Izquierda: pestañas (Inicio) o título de la pantalla.
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(left_rect.shrink2(Vec2::new(16.0, 0.0)))
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    match app.screen {
                        Screen::Home => crate::ui::home::row_tabs(app, ui),
                        Screen::Dm(i) => {
                            let name = app.friends.get(i).map(|f| f.name.clone()).unwrap_or_default();
                            theme::icon(ui, Icon::MessageCircle, 18.0, palette.secondary);
                            ui.add_space(8.0);
                            theme::text(ui, name, theme::bold(16.0), palette.text);
                        }
                        Screen::Server(i) => {
                            // Como en la referencia: el canal abierto (# o
                            // parlante + nombre) y su descripción, en vez del
                            // nombre del server (ese va en el banner).
                            let (cat, chan) = app.current_channel;
                            let (name, topic, is_voice) = match app.servers.get(i) {
                                Some(s) => {
                                    let channel = s.channel(cat, chan);
                                    (
                                        channel.map(|c| c.name.clone()).unwrap_or_else(|| s.name.clone()),
                                        s.topic.clone(),
                                        channel.is_some_and(|c| c.is_voice),
                                    )
                                }
                                None => (String::new(), String::new(), false),
                            };
                            if is_voice {
                                theme::icon(ui, Icon::Volume2, 18.0, palette.secondary);
                            } else {
                                theme::text(ui, "#", theme::bold(18.0), palette.secondary);
                            }
                            ui.add_space(8.0);
                            twemoji::text_in(ui, &name, theme::bold(16.0), palette.text, twemoji::Set::Fluent);
                            if !topic.trim().is_empty() {
                                let topic: String = if topic.chars().count() > 70 {
                                    topic.chars().take(69).collect::<String>() + "…"
                                } else {
                                    topic
                                };
                                ui.add_space(12.0);
                                theme::text(ui, topic, theme::regular(12.5), palette.dim);
                            }
                        }
                        _ => {}
                    }
                },
            );

            // Derecha: actualizaciones + notificaciones.
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(right_rect.shrink2(Vec2::new(10.0, 0.0)))
                    .layout(Layout::right_to_left(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    // Notificaciones: campana con el contador de pendientes; el
                    // panel se dibuja abajo, ya fuera de la fila.
                    bell_rect = Some(crate::ui::inbox::bell_button(ui, app));
                    ui.add_space(4.0);
                    if theme::icon_button(
                        ui,
                        Icon::Refresh,
                        15.0,
                        palette.dim,
                        palette.text,
                        "Buscar actualizaciones",
                    )
                    .clicked()
                    {
                        app.push_toast(
                            ToastKind::Info,
                            "Buscando actualizaciones",
                            "eCord ya está en la última versión.",
                        );
                    }
                },
            );
        });

    if let Some(rect) = bell_rect {
        crate::ui::inbox::show_panel(app, ui.ctx(), rect);
    }
}

/// Cuánto sigue girando el spinner después de la última request (segundos).
/// Sin esto, una request de unos 50 ms sería un parpadeo apenas visible.
const ACTIVITY_LINGER: f64 = 0.45;

/// Spinner de la barra: gira mientras haya requests a la API de Discord en
/// vuelo (`discord::activity`). Siempre reserva su lugar (la caja de un
/// `icon_button` de 14 px) para que las píldoras de al lado no se corran
/// cuando aparece o desaparece.
///
/// Si el Gateway está cortado (`reconnecting`), el spinner pasa a amarillo con
/// un "!" en el centro y gira sin parar hasta que se reconecta, haya o no
/// requests en vuelo.
fn activity_spinner(ui: &mut egui::Ui, palette: &Palette, reconnecting: bool) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());

    if reconnecting {
        if ui.is_rect_visible(rect) {
            let mut spinner_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
            );
            theme::spinner(&mut spinner_ui, 18.0, palette.warning);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "!",
                theme::bold(10.0),
                palette.warning,
            );
        }
        let _ = response.on_hover_text("Sin conexión con Discord. Reconectando…");
        return;
    }

    let now = ui.input(|i| i.time);
    let id = egui::Id::new("topbar_activity_spinner_until");
    let mut visible_until = ui.ctx().data(|d| d.get_temp::<f64>(id)).unwrap_or(0.0);
    if crate::discord::activity::is_busy() {
        visible_until = now + ACTIVITY_LINGER;
        ui.ctx().data_mut(|d| d.insert_temp(id, visible_until));
    }
    if now >= visible_until || !ui.is_rect_visible(rect) {
        return;
    }

    let mut spinner_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
    );
    theme::spinner(&mut spinner_ui, 16.0, palette.dim);
    let _ = response.on_hover_text("Comunicándose con Discord…");
}

fn toggle_maximize(ui: &mut egui::Ui) {
    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
    ui.ctx()
        .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
}

/// Una "píldora" de estado, como "Update to 0.8.0" / "Playing on
/// DESKTOP-x" en la barra de referencia.
fn status_pill(ui: &mut egui::Ui, palette: &Palette, icon: Icon, label: &str) {
    let _ = status_pill_impl(ui, palette, icon, label, false);
}

/// Lo mismo que [`status_pill`] pero devuelve la `Response` y, con
/// `clickable`, reacciona al hover y al click (la píldora de RAM).
fn status_pill_impl(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Icon,
    label: &str,
    clickable: bool,
) -> egui::Response {
    let font = theme::medium(11.5);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, palette.secondary);
    let padding = Vec2::new(8.0, 5.0);
    let icon_w = 14.0;
    let size = Vec2::new(
        galley.size().x + icon_w + padding.x * 2.0 + 4.0,
        galley.size().y + padding.y * 2.0,
    );
    let sense = if clickable { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let fill = if clickable && response.hovered() { palette.surface_hover } else { palette.surface };
    ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
    let icon_rect = egui::Rect::from_center_size(
        rect.left_center() + Vec2::new(padding.x + icon_w / 2.0, 0.0),
        Vec2::splat(icon_w),
    );
    icon.image(palette.accent, icon_w).paint_at(ui, icon_rect);
    ui.painter().galley(
        rect.left_center() + Vec2::new(padding.x + icon_w + 4.0, -galley.size().y / 2.0),
        galley,
        palette.secondary,
    );
    response
}
