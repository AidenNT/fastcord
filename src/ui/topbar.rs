//! Barra superior propia, al estilo de la imagen de referencia de
//! Fastpotify: ocupa TODO el ancho de la ventana (por encima del rail, los
//! paneles y el panel central), con flechas de navegación + un buscador al
//! medio + información de estado + controles de ventana a la derecha.
//!
//! La ventana se crea sin decoraciones nativas (`main.rs`), así que esta
//! barra también hace de titlebar: se puede arrastrar, y tiene sus propios
//! botones de minimizar/maximizar/cerrar.

use egui::{CornerRadius, Frame, Margin, Sense, Stroke, Vec2, ViewportCommand};

use crate::lib::state::{App, ToastKind};
use crate::ui::extra;
use crate::ui::theme::{self, Icon, Palette};

pub const HEIGHT: f32 = 52.0;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;

    egui::Panel::top("app_topbar")
        .exact_size(HEIGHT)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(palette.window)
                .stroke(Stroke::new(1.0, palette.outline))
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
                    .corner_radius(CornerRadius::same(16))
                    .inner_margin(Margin::symmetric(12, 6))
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

                    ui.add_space(8.0);
                    status_pill(ui, &palette, Icon::Zap, &format!("Conectado como {}", me_name));
                    ui.add_space(6.0);
                    status_pill(
                        ui,
                        &palette,
                        Icon::Sparkles,
                        &format!("eCord {}", env!("CARGO_PKG_VERSION")),
                    );
                });
            });
        });
}

fn toggle_maximize(ui: &mut egui::Ui) {
    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
    ui.ctx()
        .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
}

/// Una "píldora" de estado, como "Update to 0.8.0" / "Playing on
/// DESKTOP-x" en la barra de referencia.
fn status_pill(ui: &mut egui::Ui, palette: &Palette, icon: Icon, label: &str) {
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
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, rect.height() / 2.0, palette.surface);
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
}
