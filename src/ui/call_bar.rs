//! Barra de llamada persistente: aparece pegada arriba de la barra de
//! usuario (ver `ui::nav::show`) apenas estamos conectados —o pidiendo
//! conectarnos— a un canal de voz o a una llamada de DM, en cualquier
//! pantalla (Home/Dm/Server). Es la única forma de silenciarte, ensordecer
//! o cortar sin tener que estar mirando el canal en el que estás hablando,
//! igual que en el cliente real.

use egui::{CornerRadius, Frame, Margin, Stroke, Vec2};

use crate::discord::VoiceConnectionStatus;
use crate::lib::state::App;
use crate::ui::extra;
use crate::ui::theme::{self, Icon};

/// Alto fijo reservado para toda la barra, incluido el aire que la separa
/// de la lista de arriba y de la barra de usuario de abajo — para que
/// `ui::nav` le calcule espacio al bloque de arriba (rail + lista) igual
/// que ya hace con `FOOTER_HEIGHT` para la barra de usuario.
///
/// Antes la tarjeta ocupaba el alto entero sin margen propio, pegada
/// borde a borde contra la barra de usuario de abajo (mismo color de
/// fondo en algunos temas) — de ahí que a simple vista pareciera que
/// "tapaba" al usuario en vez de ser dos bloques separados. Ahora deja
/// `CARD_MARGIN` de aire arriba y abajo, como un chip flotando.
pub const HEIGHT: f32 = 64.0;
const CARD_MARGIN: f32 = 6.0;
const CARD_HEIGHT: f32 = HEIGHT - CARD_MARGIN * 2.0;
const SIDE_MARGIN: f32 = 8.0;
const ICON_SIZE: f32 = 14.0;

/// Dibuja la barra si (y solo si) hay una llamada pedida ahora mismo
/// (`app.voice_target`, la fuente de verdad optimista — ver su doc en
/// `lib::state`). `width` es el ancho combinado del panel de nav (rail +
/// lista/canales), igual que la barra de usuario de `ui::friends_panel`.
pub fn show(app: &mut App, ui: &mut egui::Ui, width: f32) {
    if app.voice_target.is_none() {
        return;
    }
    let palette = app.palette;
    let status = app.voice_connection_status;
    let connected = matches!(status, Some(VoiceConnectionStatus::Connected));
    let self_mute = app.voice_target.as_ref().is_some_and(|t| t.self_mute);
    let self_deaf = app.voice_target.as_ref().is_some_and(|t| t.self_deaf);
    let label = app
        .current_voice_channel_label()
        .unwrap_or_else(|| "Llamada de voz".to_string());

    let mut mute_clicked = false;
    let mut deafen_clicked = false;
    let mut leave_clicked = false;

    ui.allocate_ui_with_layout(
        Vec2::new(width, HEIGHT),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_size(Vec2::new(width, HEIGHT));
            // Fondo continuo detrás de la tarjeta (mismo tono que el rail
            // de servidores), para que el margen de arriba/abajo no deje
            // ver un hueco de otro color — mismo truco que ya usa
            // `ui::nav` para la barra de usuario de abajo.
            ui.painter().rect_filled(ui.max_rect(), 0.0, palette.panel);

            ui.add_space(CARD_MARGIN);
            ui.horizontal(|ui| {
                ui.add_space(SIDE_MARGIN);
                Frame::new()
                    .fill(extra::blend(palette.surface, palette.accent, 0.10))
                    .stroke(Stroke::new(1.0, extra::blend(palette.outline, palette.accent, 0.3)))
                    .corner_radius(CornerRadius::same(theme::RADIUS + 2))
                    .inner_margin(Margin::symmetric(10, 0))
                    .show(ui, |ui| {
                        ui.set_width((width - SIDE_MARGIN * 2.0).max(0.0));
                        ui.set_min_height(CARD_HEIGHT);
                        ui.horizontal_centered(|ui| {
                            // Insignia circular en vez del ícono suelto de
                            // antes: mismo lenguaje visual que el avatar
                            // de `user_bar`, así ambas barras se leen
                            // como parte de la misma familia.
                            let (badge_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::hover());
                            let badge_color = if connected { palette.accent } else { palette.dim };
                            ui.painter().circle_filled(badge_rect.center(), 13.0, badge_color);
                            theme::paint_icon(ui, Icon::Phone, badge_rect, ICON_SIZE, palette.on_accent);

                            ui.add_space(8.0);
                            ui.vertical(|ui| {
                                ui.add_space(1.0);
                                theme::text(ui, status_text(status), theme::semibold(12.0), palette.accent);
                                theme::text(ui, &label, theme::regular(11.0), palette.dim);
                            });

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                // El panel de nav pone `item_spacing = ZERO`
                                // para el resto de su contenido (ver
                                // `ui::nav::show`); sin restaurarlo acá los
                                // tres botones de este grupo quedan pegados
                                // uno con otro en vez de separados.
                                ui.spacing_mut().item_spacing.x = 6.0;

                                if theme::icon_button(
                                    ui,
                                    Icon::PhoneOff,
                                    ICON_SIZE,
                                    palette.danger,
                                    palette.text,
                                    "Salir de la llamada",
                                )
                                .clicked()
                                {
                                    leave_clicked = true;
                                }
                                let deafen_icon = if self_deaf { Icon::VolumeX } else { Icon::Headphones };
                                let deafen_color = if self_deaf { palette.danger } else { palette.dim };
                                let deafen_tooltip =
                                    if self_deaf { "Dejar de ensordecer" } else { "Ensordecer" };
                                if theme::icon_button(
                                    ui,
                                    deafen_icon,
                                    ICON_SIZE,
                                    deafen_color,
                                    palette.text,
                                    deafen_tooltip,
                                )
                                .clicked()
                                {
                                    deafen_clicked = true;
                                }
                                let mute_icon = if self_mute { Icon::MicOff } else { Icon::Mic };
                                let mute_color = if self_mute { palette.danger } else { palette.dim };
                                let mute_tooltip = if self_mute { "Dejar de silenciar" } else { "Silenciar" };
                                if theme::icon_button(
                                    ui,
                                    mute_icon,
                                    ICON_SIZE,
                                    mute_color,
                                    palette.text,
                                    mute_tooltip,
                                )
                                .clicked()
                                {
                                    mute_clicked = true;
                                }
                            });
                        });
                    });
            });
            ui.add_space(CARD_MARGIN);
        },
    );

    // Un solo clic a la vez, aplicado después de dibujar — mismo patrón
    // que `ui::settings`: mientras el bloque de arriba tiene tomado
    // `palette`/`label` (que salieron de `app`), no queremos también
    // tener a `app` prestado mutable para estos toggles.
    if leave_clicked {
        app.leave_voice();
    } else if deafen_clicked {
        app.toggle_self_deafen();
    } else if mute_clicked {
        app.toggle_self_mute();
    }
}

fn status_text(status: Option<VoiceConnectionStatus>) -> &'static str {
    match status {
        Some(VoiceConnectionStatus::Connecting) | None => "Conectando…",
        Some(VoiceConnectionStatus::Connected) => "Voz conectada",
        Some(VoiceConnectionStatus::Disconnected) => "Desconectado",
        Some(VoiceConnectionStatus::Failed) => "No se pudo conectar",
    }
}