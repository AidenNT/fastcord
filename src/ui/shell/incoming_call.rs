//! Tarjeta de llamada entrante: aparece arriba, al centro, mientras suena el
//! ring de un DM/grupo (`App::incoming_ring`, ver `lib::state::dm_calls`), con
//! el avatar, quién llama y los botones de Aceptar / Rechazar.
//!
//! No aparece (ni suena nada) con la cuenta en "No molestar": eso lo decide
//! `App::update_incoming_ring`, esta UI solo dibuja lo que haya.

use std::time::Duration;

use egui::{Align, Align2, Area, Color32, CornerRadius, Frame, Id, Layout, Margin, Order, Sense, Stroke, Vec2};

use crate::lib::state::App;
use crate::ui::shell::extra;
use crate::ui::shell::topbar;
use crate::ui::theme::{self, Icon};

const CARD_WIDTH: f32 = 340.0;
const AVATAR: f32 = 48.0;
const BUTTON: f32 = 40.0;
/// Verde de "atender" (el mismo de Discord).
const ACCEPT_GREEN: Color32 = Color32::from_rgb(0x3b, 0xa5, 0x5d);

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let Some(ring) = app.incoming_ring.clone() else {
        return;
    };
    let ctx = ui.ctx().clone();
    let palette = app.palette;

    // Entrada: baja desde arriba con fade-in.
    let enter = ctx.animate_bool_with_time_and_easing(
        Id::new("incoming_call_enter"),
        true,
        0.28,
        egui::emath::easing::cubic_out,
    );
    let time = ctx.input(|i| i.time) as f32;
    // Latido del aro del avatar (ring).
    let pulse = 0.5 + 0.5 * (time * 4.0).sin();

    let mut accept = false;
    let mut decline = false;
    Area::new(Id::new("incoming_call_card"))
        .order(Order::Foreground)
        .anchor(
            Align2::CENTER_TOP,
            Vec2::new(0.0, topbar::HEIGHT + 12.0 - 24.0 * (1.0 - enter)),
        )
        .show(&ctx, |ui| {
            ui.set_opacity(enter);
            Frame::new()
                .fill(extra::blend(palette.panel, palette.accent, 0.10))
                .stroke(Stroke::new(1.5, extra::blend(palette.outline, palette.accent, 0.45)))
                .corner_radius(CornerRadius::same(16))
                .inner_margin(Margin::symmetric(14, 12))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 6],
                    blur: 24,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(ui, |ui| {
                    ui.set_width(CARD_WIDTH - 28.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(AVATAR + 8.0), Sense::hover());
                        ui.painter().circle_stroke(
                            rect.center(),
                            AVATAR / 2.0 + 2.0 + 3.0 * pulse,
                            Stroke::new(2.0, ACCEPT_GREEN.gamma_multiply(0.35 + 0.5 * (1.0 - pulse))),
                        );
                        extra::avatar(
                            ui,
                            rect.center(),
                            AVATAR / 2.0,
                            ring.avatar_url.as_deref(),
                            palette.accent,
                            &ring.initial,
                            &palette,
                        );

                        ui.vertical(|ui| {
                            ui.set_max_width(CARD_WIDTH - 28.0 - AVATAR - 8.0 - BUTTON * 2.0 - 36.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&ring.title)
                                        .font(theme::bold(15.0))
                                        .color(palette.text),
                                )
                                .truncate(),
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&ring.body)
                                        .font(theme::regular(12.5))
                                        .color(palette.secondary),
                                )
                                .truncate(),
                            );
                        });

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if circle_button(ui, "incoming_call_decline", Icon::PhoneOff, palette.danger, "Rechazar") {
                                decline = true;
                            }
                            if circle_button(ui, "incoming_call_accept", Icon::Phone, ACCEPT_GREEN, "Aceptar") {
                                accept = true;
                            }
                        });
                    });
                });
        });

    // Mientras suena, un frame cada tanto para el latido y la animación.
    ctx.request_repaint_after(Duration::from_millis(33));

    if accept {
        app.accept_incoming_call();
    } else if decline {
        app.decline_incoming_call();
    }
}

/// Botón circular con un ícono blanco. `true` si se hizo clic.
fn circle_button(ui: &mut egui::Ui, id_salt: &'static str, icon: Icon, fill: Color32, tooltip: &str) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::hover());
    let response = ui.interact(rect, Id::new(id_salt), Sense::click());
    let bg = if response.hovered() {
        extra::blend(fill, Color32::WHITE, 0.15)
    } else {
        fill
    };
    ui.painter().circle_filled(rect.center(), BUTTON / 2.0, bg);
    let size = if response.is_pointer_button_down_on() { 17.0 } else { 19.0 };
    theme::paint_icon(ui, icon, rect, size, Color32::WHITE);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
        .clicked()
}
