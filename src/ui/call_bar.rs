//! Barra de llamada persistente: aparece pegada arriba de la barra de
//! usuario (ver `ui::nav::show`) apenas estamos conectados —o pidiendo
//! conectarnos— a un canal de voz o a una llamada de DM, en cualquier
//! pantalla (Home/Dm/Server). Es la única forma de silenciarte, ensordecer
//! o cortar sin tener que estar mirando el canal en el que estás hablando,
//! igual que en el cliente real.

use egui::{Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Sense, Stroke, UiBuilder, Vec2};

use crate::discord::{VoiceConnectPhase, VoiceConnectionStatus};
use crate::lib::state::{App, Screen};
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
    let phase = app.voice_connect_phase;
    let connected = app.voice_fully_connected();
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
                                theme::text(ui, connection_label(status, phase), theme::semibold(12.0), palette.accent);
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

/// Texto de estado de la conexión. Mientras la conexión no esté completa
/// muestra la fase en la que va ("Esperando servidor RTC", "Autenticando",
/// "Encriptando"…); sin ninguna fase todavía, se está esperando al servidor
/// de voz.
pub fn connection_label(
    status: Option<VoiceConnectionStatus>,
    phase: Option<VoiceConnectPhase>,
) -> &'static str {
    match status {
        Some(VoiceConnectionStatus::Disconnected) => "Desconectado",
        Some(VoiceConnectionStatus::Failed) => "Sin conexión",
        Some(VoiceConnectionStatus::Connecting) | Some(VoiceConnectionStatus::Connected) | None => {
            match phase {
                Some(phase) => phase.label(),
                None if status == Some(VoiceConnectionStatus::Connected) => "Conectado",
                None => VoiceConnectPhase::WaitingVoiceServer.label(),
            }
        }
    }
}

/// `true` mientras la llamada todavía está arrancando (alguna fase antes de
/// `Ready`): la píldora usa el color de aviso y barritas a medias.
fn connection_pending(status: Option<VoiceConnectionStatus>, phase: Option<VoiceConnectPhase>) -> bool {
    match status {
        Some(VoiceConnectionStatus::Disconnected) | Some(VoiceConnectionStatus::Failed) => false,
        Some(VoiceConnectionStatus::Connected) => {
            !matches!(phase, None | Some(VoiceConnectPhase::Ready))
        }
        Some(VoiceConnectionStatus::Connecting) | None => true,
    }
}

/// Cuántas de las 3 barritas de señal se llenan según el avance.
fn connection_bars(status: Option<VoiceConnectionStatus>, phase: Option<VoiceConnectPhase>) -> usize {
    match status {
        Some(VoiceConnectionStatus::Disconnected) | Some(VoiceConnectionStatus::Failed) => 1,
        _ if !connection_pending(status, phase) => 3,
        _ => match phase {
            None | Some(VoiceConnectPhase::WaitingVoiceServer) | Some(VoiceConnectPhase::ConnectingRtc) => 1,
            Some(_) => 2,
        },
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

/// Duración de la animación con la que la barra se esconde / reaparece al
/// entrar / salir de la vista de la llamada.
const SLIDE_SECS: f32 = 0.30;

/// Segundos que la barra queda a la vista tras llegar al estado "Conectado"
/// antes de bajar sola.
const AUTO_HIDE_SECS: f64 = 2.0;

/// Alto (px) de la franja pegada al borde inferior de la ventana que sube la
/// barra cuando está escondida en la vista de la llamada. Fina y por debajo de
/// la barra de controles de la llamada (que queda ~14 px sobre el borde) para
/// no dispararse al ir a tocar un botón de ahí, como Ajustes.
const REVEAL_ZONE: f32 = 10.0;

/// `true` si ahora mismo se está mirando la vista de la llamada a la que
/// estamos conectados (el canal de voz abierto es el de `voice_target`).
/// Ahí los mismos controles viven dentro de la propia vista
/// (`ui::call_view::controls_bar`), así que la barra se esconde.
pub fn viewing_call(app: &App) -> bool {
    let Screen::Server(index) = app.screen else { return false };
    let Some(server) = app.servers.get(index) else { return false };
    let (cat, chan) = app.current_channel;
    let Some(channel) = server.channel(cat, chan) else { return false };
    if !channel.is_voice {
        return false;
    }
    channel
        .channel_id
        .as_deref()
        .is_some_and(|id| app.is_connected_to_voice_channel_str(&server.guild_id, id))
}

/// Barra de llamada de ancho completo, pegada al borde inferior:
///
/// `[avatar] [Desconectar]  [▮▮▮ Conectado | 🔈 canal]  ↗   ···   ajustes | [compartir][cámara][chat]`
///
/// Compartir pantalla abre el selector de qué transmitir (`ui::share_picker`) y,
/// con una transmisión en curso, la corta (`App::toggle_broadcast`). Cámara y chat de
/// voz todavía no existen en el cliente: se dibujan apagados con un tooltip,
/// para que el diseño quede completo y solo falte conectarles la acción.
///
/// Hay que llamarla ANTES de dibujar las pantallas (reserva el borde inferior
/// con un `Panel::bottom`, igual que `ui::topbar` hace con el superior).
pub fn show_bottom(app: &mut App, ui: &mut egui::Ui) {
    if app.voice_target.is_none() {
        return;
    }
    // Animación de esconderse / salir: 1.0 = barra a la vista, 0.0 = escondida
    // (deslizada hacia abajo y desvanecida).
    let phase = app.voice_connect_phase;

    // Auto-ocultado, SOLO dentro de la vista de la llamada (ahí los controles
    // viven en la propia vista; en cualquier otro canal o pantalla la barra se
    // queda siempre visible, y mientras la conexión arranca se ve en todos
    // lados para poder seguir las fases).
    //
    // `hold_until`: hasta cuándo se mantiene a la vista. Arranca AUTO_HIDE_SECS
    // después de quedar completamente conectado. Con la barra escondida, poner
    // el puntero en la franja pegada al borde inferior (REVEAL_ZONE, más abajo
    // que la barra de controles de la llamada para no dispararse al tocarla) la
    // sube; y mientras el puntero esté sobre ella se sigue renovando, así que
    // baja AUTO_HIDE_SECS después de que lo sacás.
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let hold_id = egui::Id::new("call_bar_hold_until");
    let vis_id = egui::Id::new("call_bar_was_visible");
    let mut hold_until = ctx.data(|d| d.get_temp::<f64>(hold_id)).unwrap_or(-1.0);
    let was_visible = ctx.data(|d| d.get_temp::<bool>(vis_id)).unwrap_or(true);
    let fully_connected = app.voice_fully_connected();
    let in_call_view = viewing_call(app);
    if !fully_connected {
        hold_until = -1.0;
    } else if hold_until < 0.0 {
        hold_until = now + AUTO_HIDE_SECS;
    }
    if fully_connected && in_call_view {
        let full_h = if theme::is_modern() { BOTTOM_HEIGHT + 16.0 } else { BOTTOM_HEIGHT };
        // Con la barra a la vista, "sobre ella" es todo su alto; escondida,
        // solo la franja fina del borde.
        let zone = if was_visible { full_h } else { REVEAL_ZONE };
        let pointer_in_zone = ctx
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|pos| pos.y >= ctx.viewport_rect().bottom() - zone);
        if pointer_in_zone {
            hold_until = now + AUTO_HIDE_SECS;
        }
    }
    ctx.data_mut(|d| d.insert_temp(hold_id, hold_until));
    let settled = fully_connected && now >= hold_until;
    if fully_connected && !settled {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((hold_until - now).max(0.0) + 0.02));
    }
    let shown = !settled || !in_call_view;
    ctx.data_mut(|d| d.insert_temp(vis_id, shown));
    let t = ui.ctx().animate_bool_with_time_and_easing(
        egui::Id::new("call_bar_bottom_slide"),
        shown,
        SLIDE_SECS,
        egui::emath::easing::cubic_out,
    );
    if t < 0.005 {
        return;
    }
    let palette = app.palette;
    let status = app.voice_connection_status;
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

    let mut leave_clicked = false;
    let mut settings_clicked = false;
    let mut goto_clicked = false;
    let mut share_clicked = false;
    // Transmitir solo tiene sentido con la llamada ya conectada; si ya hay una
    // transmisión en curso el botón sirve para cortarla en cualquier estado.
    let broadcasting = app.is_broadcasting();
    let share_enabled = broadcasting || app.voice_fully_connected();

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
        .exact_size(panel_height * t)
        .resizable(false)
        .frame(panel_frame)
        .show(ui, |ui| {
            // El contenido se dibuja siempre a tamaño completo, anclado al
            // borde superior del panel: al encogerse el panel (animación) la
            // barra baja y se corta contra el borde de la ventana.
            let inner_h = if modern { panel_height - 16.0 } else { panel_height };
            let full_rect = egui::Rect::from_min_size(
                ui.max_rect().min,
                Vec2::new(ui.max_rect().width(), inner_h),
            );
            ui.scope_builder(UiBuilder::new().max_rect(full_rect), |ui| {
            ui.set_opacity(t);
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
                if disconnect_button(ui, &palette).clicked() {
                    leave_clicked = true;
                }
                ui.add_space(10.0);

                status_pill(ui, &palette, status, phase, &channel);

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
                    // Lo que antes estaba junto a "Desconectar" ahora ocupa
                    // el extremo derecho (visualmente: compartir, cámara, chat).
                    // Todavía sin función en el cliente: cámara y chat apagados.
                    box_button(ui, &palette, Icon::MessageCircle, "Chat de voz (próximamente)", false);
                    box_button(ui, &palette, Icon::Video, "Cámara (próximamente)", false);
                    if share_button(ui, &palette, broadcasting, share_enabled).clicked() {
                        share_clicked = true;
                    }

                    ui.add_space(4.0);
                    vertical_separator(ui, &palette);
                    ui.add_space(4.0);

                    if theme::icon_button(ui, Icon::Settings, BOTTOM_ICON, palette.text, palette.accent, "Ajustes")
                        .clicked()
                    {
                        settings_clicked = true;
                    }
                });
            });
            });
        });

    // Un solo clic a la vez, aplicado después de dibujar (mismo patrón que
    // `show`): arriba `palette`/`channel` salieron de `app`.
    if leave_clicked {
        app.leave_voice();
    } else if settings_clicked {
        app.settings_open = true;
    } else if goto_clicked {
        app.go_to_voice_call();
    } else if share_clicked {
        app.toggle_broadcast();
    }
}

/// Píldora con borde: `▮▮▮ Conectado | 🔈 canal`.
fn status_pill(
    ui: &mut egui::Ui,
    palette: &crate::ui::theme::Palette,
    status: Option<VoiceConnectionStatus>,
    phase: Option<VoiceConnectPhase>,
    channel: &str,
) {
    let color = match status {
        Some(VoiceConnectionStatus::Disconnected) | Some(VoiceConnectionStatus::Failed) => palette.danger,
        _ if connection_pending(status, phase) => palette.warning,
        _ => palette.accent,
    };
    Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(button_radius()))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.horizontal(|ui| {
                signal_bars(ui, color, connection_bars(status, phase));
                theme::text(ui, connection_label(status, phase), theme::semibold(12.5), color);
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
fn signal_bars(ui: &mut egui::Ui, color: Color32, filled: usize) {
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

/// Botón "Compartir pantalla". Con la transmisión en curso se pinta de rojo y
/// sirve para cortarla; con `enabled = false` (llamada todavía sin conectar)
/// se ve apagado y no responde.
fn share_button(
    ui: &mut egui::Ui,
    palette: &crate::ui::theme::Palette,
    live: bool,
    enabled: bool,
) -> egui::Response {
    let tooltip = if live { "Dejar de compartir pantalla" } else { "Compartir pantalla" };
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(BOX_BUTTON, sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tooltip));
    if ui.is_rect_visible(rect) {
        let fill = if live {
            palette.danger
        } else if enabled && response.hovered() {
            palette.surface_hover
        } else {
            palette.surface
        };
        let tint = if live {
            Color32::WHITE
        } else if enabled {
            palette.text
        } else {
            palette.dim
        };
        let icon = if live { Icon::Monitor } else { Icon::SquareArrowUp };
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
