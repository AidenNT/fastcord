//! Overlays flotantes, al estilo Fastpotify: notificaciones apiladas en una
//! esquina ("Spotify is taking a while...") y un popup modal centrado con
//! fondo semi-transparente ("This account cannot play music here").
//!
//! Se dibujan al final de `App::ui`, después de la pantalla activa, para
//! quedar siempre por encima de todo lo demás.

use egui::{Align2, Area, Color32, CornerRadius, Frame, Margin, Order, Stroke, UiBuilder, Vec2};

use crate::lib::state::App;
use crate::ui::theme;

/// Los avisos flotantes (`App::toasts`) ahora se dibujan junto con las
/// tarjetas de mención, en una sola pila con el diseño del overlay de
/// Nullscape: ver `ui::notifications::show_in_app`. Se deja esta función
/// (vacía) para no tocar el punto de llamada en `App::ui`.
pub fn show_toasts(_app: &mut App, _ui: &mut egui::Ui) {}

/// Popup modal centrado, con fondo semi-transparente detrás, al estilo del
/// aviso "This account cannot play music here" de Fastpotify. Bloquea los
/// clicks al resto de la ventana mientras está abierto.
pub fn show_modal(app: &mut App, ui: &mut egui::Ui) {
    let Some(modal) = &app.modal else {
        return;
    };
    let palette = app.palette;
    let title = modal.title.clone();
    let message = modal.message.clone();
    let confirm_label = modal.confirm_label.clone();

    let screen_rect = ui.ctx().viewport_rect();
    let mut close = false;

    Area::new(egui::Id::new("app_modal_scrim"))
        .order(Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(ui.ctx(), |ui| {
            ui.set_width(screen_rect.width());
            ui.set_height(screen_rect.height());
            ui.painter()
                .rect_filled(screen_rect, 0.0, Color32::from_black_alpha(150));
            // Consume los clicks para que no lleguen a lo que está atrás.
            ui.interact(screen_rect, egui::Id::new("app_modal_block"), egui::Sense::click());

            let card_size = Vec2::new(420.0, 190.0);
            let card_rect = egui::Rect::from_center_size(screen_rect.center(), card_size);
            let mut card_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(card_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::RADIUS + 6))
                .inner_margin(Margin::same(22))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 12],
                    blur: 32,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(&mut card_ui, |ui| {
                    ui.set_width(card_size.x - 44.0);
                    theme::text(ui, &title, theme::bold(17.0), palette.text);
                    ui.add_space(10.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&message)
                                .font(theme::regular(13.5))
                                .color(palette.secondary),
                        )
                        .wrap(),
                    );
                    ui.add_space(20.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::pill_button(ui, &palette, &confirm_label, true).clicked() {
                            close = true;
                        }
                    });
                });
        });

    if close {
        app.close_modal();
    }
}

/// Formulario que pidió un bot tras apretar uno de sus botones (por ejemplo
/// "Responder anónimo" pide el texto de la respuesta). Mismo estilo que
/// `show_modal`, pero con campos de texto y dos botones: Cancelar / Enviar.
/// Lo manda `App::submit_component_modal`.
pub fn show_component_modal(app: &mut App, ui: &mut egui::Ui) {
    if app.component_modal.is_none() {
        return;
    }
    let palette = app.palette;
    let screen_rect = ui.ctx().viewport_rect();
    let mut submit = false;
    let mut cancel = ui.ctx().input(|i| i.key_pressed(egui::Key::Escape));

    // Fondo oscuro que también se come los clicks.
    Area::new(egui::Id::new("component_modal_scrim"))
        .order(Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(ui.ctx(), |ui| {
            ui.set_width(screen_rect.width());
            ui.set_height(screen_rect.height());
            ui.painter()
                .rect_filled(screen_rect, 0.0, Color32::from_black_alpha(150));
            ui.interact(screen_rect, egui::Id::new("component_modal_block"), egui::Sense::click());
        });

    Area::new(egui::Id::new("component_modal_card"))
        .order(Order::Foreground)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            let Some(modal) = app.component_modal.as_mut() else { return };
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::RADIUS + 6))
                .inner_margin(Margin::same(22))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 12],
                    blur: 32,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(ui, |ui| {
                    ui.set_width(440.0);
                    theme::text(ui, &modal.request.title, theme::bold(17.0), palette.text);
                    ui.add_space(12.0);
                    for (index, field) in modal.fields.iter_mut().enumerate() {
                        if !field.label.is_empty() {
                            theme::text(ui, &field.label, theme::semibold(12.0), palette.secondary);
                            ui.add_space(4.0);
                        }
                        let edit = if field.paragraph {
                            egui::TextEdit::multiline(&mut field.value).desired_rows(5)
                        } else {
                            egui::TextEdit::singleline(&mut field.value)
                        };
                        let response = ui.add(
                            edit.id(egui::Id::new(("component_modal_field", index)))
                                .hint_text(field.placeholder.as_str())
                                .char_limit(field.max_length)
                                .font(theme::regular(13.5))
                                .desired_width(f32::INFINITY),
                        );
                        // Que el primer campo ya tenga el foco al abrir.
                        if index == 0 && !response.has_focus() && !ui.ctx().memory(|m| m.focused().is_some()) {
                            response.request_focus();
                        }
                        ui.add_space(10.0);
                    }
                    if modal.fields.is_empty() {
                        theme::text(ui, "Este formulario no tiene campos que ecord pueda mostrar.", theme::regular(13.0), palette.dim);
                        ui.add_space(10.0);
                    }
                    if let Some(error) = &modal.error {
                        theme::text(ui, error, theme::regular(12.5), palette.danger);
                        ui.add_space(8.0);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::pill_button(ui, &palette, "Enviar", true).clicked() {
                            submit = true;
                        }
                        if theme::pill_button(ui, &palette, "Cancelar", false).clicked() {
                            cancel = true;
                        }
                    });
                });
        });

    if cancel {
        app.component_modal = None;
    } else if submit {
        app.submit_component_modal();
    }
}
