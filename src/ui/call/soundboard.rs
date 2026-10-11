//! Popup del soundboard: la lista de sonidos para tocar en el canal de voz.
//!
//! Se abre con el botón de la barra de llamada (`ui::call::call_bar`) y se dibuja
//! encima de él. Un clic en un sonido lo manda (`App::send_soundboard_sound`);
//! el popup queda abierto para tocar varios, como en el cliente oficial. Se
//! cierra con Escape, con un clic afuera o al salir de la llamada.

use egui::{Align2, Area, CornerRadius, Frame, Id, Margin, Order, ScrollArea, Sense, Stroke, Vec2};

use crate::discord::soundboard::SoundboardSound;
use crate::lib::state::App;
use crate::ui::theme::{self, Icon};

const W: f32 = 340.0;
const CELL_H: f32 = 36.0;
const GAP: f32 = 6.0;

pub fn show_popup(app: &mut App, ui: &mut egui::Ui) {
    if !app.soundboard_open {
        return;
    }
    // Sin llamada no hay a dónde tocar.
    if app.voice_target.is_none() {
        app.soundboard_open = false;
        return;
    }
    let palette = app.palette;
    let ctx = ui.ctx().clone();
    let screen = ctx.viewport_rect();
    let button = app
        .soundboard_button
        .unwrap_or_else(|| egui::Rect::from_center_size(screen.center_bottom(), Vec2::ZERO));

    let guild_id = app.voice_guild_id();
    let guild_sounds: Vec<SoundboardSound> = guild_id
        .as_ref()
        .and_then(|id| app.soundboard_guild_sounds.get(id))
        .cloned()
        .unwrap_or_default();
    let defaults = app.soundboard_defaults.clone();
    let loading = app.soundboard_loading;

    let x = button.center().x.clamp(
        screen.min.x + W / 2.0 + 12.0,
        (screen.max.x - W / 2.0 - 12.0).max(screen.min.x + W / 2.0 + 12.0),
    );
    let mut picked: Option<SoundboardSound> = None;

    let inner = Area::new(Id::new("soundboard_popup"))
        .order(Order::Foreground)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(x, button.min.y - 10.0))
        .show(&ctx, |ui| {
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::radius() + 2))
                .inner_margin(Margin::same(12))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(ui, |ui| {
                    ui.set_width(W - 24.0);
                    theme::text(ui, "Soundboard", theme::semibold(14.0), palette.text);
                    ui.add_space(8.0);
                    if guild_sounds.is_empty() && defaults.is_empty() {
                        let message = if loading {
                            "Cargando sonidos…"
                        } else {
                            "No hay sonidos disponibles."
                        };
                        theme::text(ui, message, theme::regular(12.5), palette.dim);
                        return;
                    }
                    ScrollArea::vertical()
                        .max_height(320.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            if !guild_sounds.is_empty() {
                                theme::text(ui, "Sonidos del servidor", theme::semibold(12.0), palette.dim);
                                ui.add_space(6.0);
                                sound_grid(ui, &palette, &guild_sounds, &mut picked);
                                ui.add_space(12.0);
                            }
                            if !defaults.is_empty() {
                                theme::text(ui, "Sonidos predeterminados", theme::semibold(12.0), palette.dim);
                                ui.add_space(6.0);
                                sound_grid(ui, &palette, &defaults, &mut picked);
                            }
                        });
                });
        });

    if let Some(sound) = picked {
        app.send_soundboard_sound(&sound);
    }

    let popup_rect = inner.response.rect;
    let pressed_outside = ctx.input(|i| i.pointer.any_pressed())
        && ctx
            .pointer_interact_pos()
            .is_some_and(|p| !popup_rect.contains(p) && !button.contains(p));
    if pressed_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.soundboard_open = false;
    }
}

/// Cuadrícula de dos columnas con un botón por sonido.
fn sound_grid(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    sounds: &[SoundboardSound],
    picked: &mut Option<SoundboardSound>,
) {
    let cell_w = (ui.available_width() - GAP) / 2.0;
    for pair in sounds.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for sound in pair {
                let sense = if sound.available {
                    Sense::click()
                } else {
                    Sense::hover()
                };
                let (rect, response) = ui.allocate_exact_size(Vec2::new(cell_w, CELL_H), sense);
                let hovered = sound.available && response.hovered();
                let fill = if hovered {
                    palette.surface_hover
                } else {
                    palette.surface
                };
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(theme::radius()), fill);
                let tint = if sound.available {
                    palette.secondary
                } else {
                    palette.dim
                };
                theme::paint_icon(
                    ui,
                    Icon::Volume2,
                    egui::Rect::from_center_size(rect.left_center() + Vec2::new(16.0, 0.0), Vec2::splat(16.0)),
                    16.0,
                    tint,
                );
                crate::ui::chat::emoji::paint_line_top(
                    ui,
                    rect.left_top() + Vec2::new(32.0, 9.0),
                    &sound.name,
                    theme::medium(13.0),
                    if sound.available { palette.text } else { palette.dim },
                    cell_w - 40.0,
                );
                let response = if sound.available {
                    response
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(&sound.name)
                } else {
                    response.on_hover_text("Este sonido no está disponible")
                };
                if sound.available && response.clicked() {
                    *picked = Some(sound.clone());
                }
            }
        });
        ui.add_space(GAP);
    }
}
