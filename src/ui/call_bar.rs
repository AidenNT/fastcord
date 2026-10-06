//! Barra de llamada persistente: aparece pegada arriba de la barra de
//! usuario (ver `ui::nav::show`) apenas estamos conectados —o pidiendo
//! conectarnos— a un canal de voz o a una llamada de DM, en cualquier
//! pantalla (Home/Dm/Server). Es la única forma de silenciarte, ensordecer
//! o cortar sin tener que estar mirando el canal en el que estás hablando,
//! igual que en el cliente real.

use egui::{Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Sense, Stroke, Vec2};

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
    // Con "Usar nueva interfaz" la barra va abajo, a todo el ancho
    // (`show_bottom`), y esta tarjeta no se dibuja.
    if app.voice_target.is_none() || app.new_call_ui {
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
                    .corner_radius(CornerRadius::same(theme::radius() + 2))
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

// ---------------------------------------------------------------------------
// Barra nueva: a todo el ancho, abajo de la ventana ("Usar nueva interfaz").
// ---------------------------------------------------------------------------

/// Alto de la barra inferior.
pub const BOTTOM_HEIGHT: f32 = 64.0;
const BOTTOM_AVATAR: f32 = 32.0;
const BOTTOM_ICON: f32 = 18.0;
const BOX_BUTTON: Vec2 = Vec2::new(56.0, 36.0);
/// Radio de botones y píldoras: más redondo en la interfaz nueva.
fn button_radius() -> u8 {
    if theme::is_modern() { 14 } else { 10 }
}
/// Máximo de caracteres del nombre del canal en la píldora (con la ventana
/// angosta, un nombre largo empujaría los botones fuera de la barra).
const MAX_CHANNEL_CHARS: usize = 22;

/// Barra de llamada de ancho completo, pegada al borde inferior:
///
/// `[avatar] nombre  [▮▮▮ Conectado | 🔈 canal]  ↗   ···   [compartir][cámara][chat]  mic auriculares ajustes  [Desconectar]`
///
/// Compartir pantalla, cámara y chat de voz todavía no existen en el cliente:
/// se dibujan apagados con un tooltip, para que el diseño quede completo y
/// solo falte conectarles la acción.
///
/// Hay que llamarla ANTES de dibujar las pantallas (reserva el borde inferior
/// con un `Panel::bottom`, igual que `ui::topbar` hace con el superior).
pub fn show_bottom(app: &mut App, ui: &mut egui::Ui) {
    if app.voice_target.is_none() {
        return;
    }
    let palette = app.palette;
    let status = app.voice_connection_status;
    let self_mute = app.voice_target.as_ref().is_some_and(|t| t.self_mute);
    let self_deaf = app.voice_target.as_ref().is_some_and(|t| t.self_deaf);
    let channel = shorten(
        &app.current_voice_channel_label()
            .unwrap_or_else(|| "Llamada de voz".to_string()),
        MAX_CHANNEL_CHARS,
    );
    let (me_name, me_avatar_url) = match app.me.as_ref() {
        Some(me) => (me.display_name().to_string(), me.avatar_url()),
        None => ("Tú".to_string(), None),
    };
    let initial: String = me_name.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();

    let mut mute_clicked = false;
    let mut deafen_clicked = false;
    let mut leave_clicked = false;
    let mut settings_clicked = false;
    let mut goto_clicked = false;

    // Interfaz nueva: la barra flota como una tarjeta redondeada con aire
    // alrededor, hermana de la barra de usuario del panel izquierdo (mismo
    // fondo, mismo borde teñido con el acento).
    let modern = theme::is_modern();
    let (panel_height, panel_frame) = if modern {
        (
            BOTTOM_HEIGHT + 16.0,
            Frame::new()
                .fill(palette.window)
                .inner_margin(Margin::symmetric(26, 8)),
        )
    } else {
        (
            BOTTOM_HEIGHT,
            Frame::new()
                .fill(palette.window)
                .stroke(Stroke::new(1.0, palette.outline))
                .inner_margin(Margin::symmetric(16, 0)),
        )
    };
    egui::Panel::bottom("call_bar_bottom")
        .exact_size(panel_height)
        .resizable(false)
        .frame(panel_frame)
        .show(ui, |ui| {
            if modern {
                let card = ui.max_rect().expand2(Vec2::new(16.0, 0.0));
                ui.painter().rect(
                    card,
                    CornerRadius::same(22),
                    extra::blend(palette.panel, palette.accent, 0.08),
                    Stroke::new(1.0, extra::blend(palette.outline, palette.accent, 0.35)),
                    egui::StrokeKind::Inside,
                );
            }
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                // --- Izquierda: quién soy + estado de la conexión ---
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(BOTTOM_AVATAR), Sense::hover());
                extra::avatar(
                    ui,
                    rect.center(),
                    BOTTOM_AVATAR / 2.0,
                    me_avatar_url.as_deref(),
                    palette.accent,
                    &initial,
                    &palette,
                );
                ui.add_space(2.0);
                theme::text(ui, &me_name, theme::semibold(14.0), palette.text);
                ui.add_space(10.0);

                status_pill(ui, &palette, status, &channel);

                if theme::icon_button(
                    ui,
                    Icon::ExternalLink,
                    BOTTOM_ICON,
                    palette.dim,
                    palette.text,
                    "Ir a la llamada",
                )
                .clicked()
                {
                    goto_clicked = true;
                }

                // --- Derecha. En un layout right_to_left lo primero que se
                // agrega queda más a la derecha, así que va al revés. ---
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if disconnect_button(ui, &palette).clicked() {
                        leave_clicked = true;
                    }
                    ui.add_space(4.0);
                    vertical_separator(ui, &palette);

                    if theme::icon_button(ui, Icon::Settings, BOTTOM_ICON, palette.text, palette.accent, "Ajustes")
                        .clicked()
                    {
                        settings_clicked = true;
                    }

                    let (deafen_icon, deafen_color, deafen_hover, deafen_tip) = if self_deaf {
                        (Icon::VolumeX, palette.danger, palette.danger, "Dejar de ensordecer")
                    } else {
                        (Icon::Headphones, palette.text, palette.accent, "Ensordecer")
                    };
                    if theme::icon_button(ui, deafen_icon, BOTTOM_ICON, deafen_color, deafen_hover, deafen_tip)
                        .clicked()
                    {
                        deafen_clicked = true;
                    }

                    let (mute_icon, mute_color, mute_hover, mute_tip) = if self_mute {
                        (Icon::MicOff, palette.danger, palette.danger, "Dejar de silenciar")
                    } else {
                        (Icon::Mic, palette.text, palette.accent, "Silenciar")
                    };
                    if theme::icon_button(ui, mute_icon, BOTTOM_ICON, mute_color, mute_hover, mute_tip).clicked() {
                        mute_clicked = true;
                    }

                    ui.add_space(4.0);
                    vertical_separator(ui, &palette);
                    ui.add_space(4.0);

                    // Todavía sin función en el cliente: apagados.
                    box_button(ui, &palette, Icon::MessageCircle, "Chat de voz (próximamente)", false);
                    box_button(ui, &palette, Icon::Video, "Cámara (próximamente)", false);
                    box_button(ui, &palette, Icon::SquareArrowUp, "Compartir pantalla (próximamente)", false);
                });
            });
        });

    // Un solo clic a la vez, aplicado después de dibujar (mismo patrón que
    // `show`): arriba `palette`/`channel` salieron de `app`.
    if leave_clicked {
        app.leave_voice();
    } else if deafen_clicked {
        app.toggle_self_deafen();
    } else if mute_clicked {
        app.toggle_self_mute();
    } else if settings_clicked {
        app.settings_open = true;
    } else if goto_clicked {
        app.go_to_voice_call();
    }
}

/// Píldora con borde: `▮▮▮ Conectado | 🔈 canal`.
fn status_pill(
    ui: &mut egui::Ui,
    palette: &crate::ui::theme::Palette,
    status: Option<VoiceConnectionStatus>,
    channel: &str,
) {
    let color = match status {
        Some(VoiceConnectionStatus::Connected) => palette.accent,
        Some(VoiceConnectionStatus::Disconnected) | Some(VoiceConnectionStatus::Failed) => palette.danger,
        Some(VoiceConnectionStatus::Connecting) | None => palette.warning,
    };
    Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(button_radius()))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.horizontal(|ui| {
                signal_bars(ui, color, status);
                theme::text(ui, short_status_text(status), theme::semibold(12.5), color);
                ui.add_space(2.0);
                vertical_separator(ui, palette);
                ui.add_space(2.0);
                theme::icon(ui, Icon::Volume2, 15.0, palette.secondary);
                theme::text(ui, channel, theme::semibold(12.5), palette.text);
            });
        });
}

/// Tres barritas crecientes (señal). Completas al estar conectado; con la
/// conexión a medias o caída solo se llenan las primeras.
fn signal_bars(ui: &mut egui::Ui, color: Color32, status: Option<VoiceConnectionStatus>) {
    let filled = match status {
        Some(VoiceConnectionStatus::Connected) => 3,
        Some(VoiceConnectionStatus::Connecting) | None => 2,
        Some(VoiceConnectionStatus::Disconnected) | Some(VoiceConnectionStatus::Failed) => 1,
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), Sense::hover());
    let heights = [6.0_f32, 10.0, 14.0];
    let faded = color.gamma_multiply(0.3);
    for (index, height) in heights.iter().enumerate() {
        let x = rect.left() + index as f32 * 5.0;
        let bar = egui::Rect::from_min_max(
            egui::pos2(x, rect.bottom() - height),
            egui::pos2(x + 3.0, rect.bottom()),
        );
        let fill = if index < filled { color } else { faded };
        ui.painter().rect_filled(bar, CornerRadius::same(1), fill);
    }
}

/// Botón cuadrado con ícono (compartir, cámara, chat). Con `enabled = false`
/// se ve apagado, no responde al clic y deja el tooltip.
fn box_button(
    ui: &mut egui::Ui,
    palette: &crate::ui::theme::Palette,
    icon: Icon,
    tooltip: &str,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(BOX_BUTTON, sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tooltip));
    if ui.is_rect_visible(rect) {
        let fill = if enabled && response.hovered() { palette.surface_hover } else { palette.surface };
        let tint = if enabled { palette.text } else { palette.dim };
        ui.painter().rect_filled(rect, CornerRadius::same(button_radius()), fill);
        theme::paint_icon(ui, icon, rect, BOTTOM_ICON, tint);
    }
    let response = if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    response.on_hover_text(tooltip)
}

/// Botón rojo "Desconectar" con el ícono de teléfono cortado.
fn disconnect_button(ui: &mut egui::Ui, palette: &crate::ui::theme::Palette) -> egui::Response {
    const LABEL: &str = "Desconectar";
    let font = theme::semibold(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(LABEL.to_owned(), font, Color32::WHITE);
    let icon_size = 16.0;
    let pad_x = 14.0;
    let gap = 8.0;
    let size = Vec2::new(pad_x * 2.0 + icon_size + gap + galley.size().x, BOX_BUTTON.y);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Salir de la llamada"));
    if ui.is_rect_visible(rect) {
        let fill = if response.hovered() {
            extra::blend(palette.danger, Color32::WHITE, 0.12)
        } else {
            palette.danger
        };
        ui.painter().rect_filled(rect, CornerRadius::same(button_radius()), fill);
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + pad_x + icon_size / 2.0, rect.center().y),
            Vec2::splat(icon_size),
        );
        theme::paint_icon(ui, Icon::PhoneOff, icon_rect, icon_size, Color32::WHITE);
        ui.painter().text(
            egui::pos2(rect.left() + pad_x + icon_size + gap, rect.center().y),
            Align2::LEFT_CENTER,
            LABEL,
            theme::semibold(13.0),
            Color32::WHITE,
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Salir de la llamada")
}

/// Línea vertical fina que separa grupos.
fn vertical_separator(ui: &mut egui::Ui, palette: &crate::ui::theme::Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, 22.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, palette.outline);
}

fn short_status_text(status: Option<VoiceConnectionStatus>) -> &'static str {
    match status {
        Some(VoiceConnectionStatus::Connecting) | None => "Conectando…",
        Some(VoiceConnectionStatus::Connected) => "Conectado",
        Some(VoiceConnectionStatus::Disconnected) => "Desconectado",
        Some(VoiceConnectionStatus::Failed) => "Sin conexión",
    }
}

/// Recorta `text` a `max` caracteres añadiendo `…` (sin partir un carácter
/// multibyte).
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
