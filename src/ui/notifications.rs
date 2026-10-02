//! UI de notificaciones: el badge rojo con el contador de menciones (DMs y
//! servers) y las tarjetas flotantes que se ven DENTRO de la app cuando llega
//! una mención con la ventana en foco (y los avisos genéricos de `App::toasts`).
//! Con la ventana sin foco no se dibuja nada acá: se manda una notificación
//! del escritorio (ver `lib::notifications` y `App::notify_incoming`).
//!
//! Diseño tomado del overlay de Nullscape: las tarjetas caen desde arriba de la
//! pantalla con fade-in (ease-out cúbico), se apilan en una esquina superior
//! (izquierda o derecha, ver `NotificationSide` / `App::notification_side`) y
//! llevan un fondo oscuro teñido con borde de 2px del color del tipo de aviso
//! (azul = mención/info, verde = éxito, dorado = advertencia). Las de mención y
//! DM muestran el avatar de quien escribió a la izquierda del texto.

use std::collections::HashMap;
use std::time::Duration;

use egui::{
    pos2, Align2, Area, Color32, CornerRadius, Frame, Margin, Order, Pos2, Rect, Sense, Stroke,
    UiBuilder, Vec2,
};

use crate::lib::state::{App, NotificationSide, ToastKind};
use crate::ui::extra;
use crate::ui::theme::{self, Icon, Palette};
use crate::ui::topbar;

const CARD_WIDTH: f32 = 320.0;
/// Cuánto dura una tarjeta de mención / un aviso genérico en pantalla.
const LIFETIME_SECS: f32 = 6.0;
const TOAST_LIFETIME_SECS: f32 = 5.0;
/// Duración de la animación de entrada (caída + fade-in).
const ANIM_MS: f32 = 320.0;
/// Fade-out al final de la vida de la tarjeta.
const FADE_OUT_SECS: f32 = 0.3;
/// Distancia al borde (izquierdo o derecho, según el lado elegido) de la
/// ventana.
const MARGIN_SIDE: f32 = 16.0;
/// Diámetro del avatar de las tarjetas de mención/DM.
const AVATAR_SIZE: f32 = 40.0;
/// Ancho reservado a la X de cerrar, a la derecha.
const CLOSE_SLOT: f32 = 30.0;
/// Distancia al borde de arriba: deja libre la barra superior (con los
/// botones de minimizar/cerrar de la ventana sin marco).
const MARGIN_TOP: f32 = topbar::HEIGHT + 12.0;
/// Separación entre tarjetas apiladas.
const GAP: f32 = 10.0;
/// Y (fuera de la pantalla) desde donde arranca la caída.
const START_Y: f32 = -160.0;
const CARD_DIM: Color32 = Color32::from_rgb(160, 160, 185);

/// Círculo rojo con el número de menciones (`99+` si pasa de 99), centrado en
/// `center`. `ring` es el color del fondo sobre el que se dibuja: se usa para
/// el aro que despega el badge del ícono de atrás. Con `count == 0` no dibuja
/// nada.
pub fn paint_badge(painter: &egui::Painter, center: Pos2, count: u32, palette: &Palette, ring: Color32) {
    if count == 0 {
        return;
    }
    let label = if count > 99 { "99+".to_string() } else { count.to_string() };
    let width = match label.len() {
        1 => 18.0,
        2 => 22.0,
        _ => 28.0,
    };
    let rect = Rect::from_center_size(center, Vec2::new(width, 18.0));
    painter.rect_filled(rect.expand(2.5), CornerRadius::same(12), ring);
    painter.rect_filled(rect, CornerRadius::same(9), palette.danger);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::semibold(10.5),
        palette.on_accent,
    );
}

// ---------------------------------------------------------------------
// Tarjetas animadas
// ---------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Tone {
    Mention,
    Info,
    Success,
    Warning,
}

struct ToneColors {
    fill: Color32,
    stroke: Color32,
    title: Color32,
}

impl Tone {
    fn colors(self) -> ToneColors {
        match self {
            Tone::Mention | Tone::Info => ToneColors {
                fill: Color32::from_rgba_unmultiplied(14, 20, 46, 236),
                stroke: Color32::from_rgb(90, 150, 255),
                title: Color32::from_rgb(140, 190, 255),
            },
            Tone::Success => ToneColors {
                fill: Color32::from_rgba_unmultiplied(16, 30, 22, 238),
                stroke: Color32::from_rgb(110, 230, 140),
                title: Color32::from_rgb(110, 230, 140),
            },
            Tone::Warning => ToneColors {
                fill: Color32::from_rgba_unmultiplied(50, 12, 22, 238),
                stroke: Color32::from_rgb(240, 200, 60),
                title: Color32::from_rgb(240, 200, 60),
            },
        }
    }
}

enum CardEvent {
    None,
    Dismiss,
    Open,
}

/// Foto de quien escribió: la imagen si hay URL, y si no (o mientras carga)
/// un círculo con la inicial.
struct CardAvatar<'a> {
    url: Option<&'a str>,
    initial: &'a str,
}

struct Card<'a> {
    id: egui::Id,
    tone: Tone,
    title: &'a str,
    body: &'a str,
    /// `None` en los avisos genéricos (no hay a quién mostrarle la foto).
    avatar: Option<CardAvatar<'a>>,
    /// Línea chica extra debajo del cuerpo (contador de mensajes).
    extra: Option<String>,
    /// 0 = recién aparece (arriba, transparente), 1 = asentada en su lugar.
    progress: f32,
    /// Multiplicador de opacidad del fade-out final (1 = sin desvanecer).
    fade: f32,
}

/// `t` en [0,1] → progreso con ease-out cúbico (arranca rápido, frena suave).
fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn progress_from_secs(elapsed: f32) -> f32 {
    ease_out_cubic(elapsed * 1_000.0 / ANIM_MS)
}

fn fade_out(elapsed: f32, lifetime: f32) -> f32 {
    ((lifetime - elapsed) / FADE_OUT_SECS).clamp(0.0, 1.0)
}

/// Dibuja una tarjeta con la posición animada y devuelve la próxima Y libre
/// (para apilar la siguiente debajo sin superponerse) y lo que hizo el usuario.
fn draw_card(
    ctx: &egui::Context,
    palette: &Palette,
    card: &Card,
    top_y: f32,
    side: NotificationSide,
) -> (f32, CardEvent) {
    let colors = card.tone.colors();
    let a = (card.progress * card.fade).clamp(0.0, 1.0);
    let screen = ctx.viewport_rect();
    let x = match side {
        NotificationSide::Left => screen.left() + MARGIN_SIDE,
        NotificationSide::Right => (screen.right() - CARD_WIDTH - MARGIN_SIDE).max(0.0),
    };
    let y = START_Y + (top_y - START_Y) * card.progress;

    let mut event = CardEvent::None;
    let resp = Area::new(card.id)
        .fixed_pos(pos2(x, y))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            Frame::new()
                .fill(colors.fill.gamma_multiply(a))
                .stroke(Stroke::new(2.0, colors.stroke.gamma_multiply(a)))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(12))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 6],
                    blur: 20,
                    spread: 0,
                    color: palette.shadow.gamma_multiply(a),
                })
                .show(ui, |ui| {
                    ui.set_width(CARD_WIDTH - 24.0);
                    ui.horizontal_top(|ui| {
                        let mut text_width = CARD_WIDTH - 24.0 - CLOSE_SLOT;
                        if let Some(avatar) = &card.avatar {
                            let (rect, _) = ui.allocate_exact_size(Vec2::splat(AVATAR_SIZE), Sense::hover());
                            // Ui hijo sin reservar lugar extra: así la foto sigue
                            // el fade-in/out de la tarjeta (la imagen no usa
                            // `gamma_multiply`).
                            let mut avatar_ui = ui.new_child(UiBuilder::new().max_rect(rect));
                            avatar_ui.set_opacity(a);
                            extra::avatar(
                                &mut avatar_ui,
                                rect.center(),
                                AVATAR_SIZE / 2.0,
                                avatar.url,
                                colors.stroke.gamma_multiply(0.55),
                                avatar.initial,
                                palette,
                            );
                            text_width -= AVATAR_SIZE + ui.spacing().item_spacing.x;
                        }
                        ui.vertical(|ui| {
                            // Deja lugar a la X de la derecha (y al avatar).
                            ui.set_max_width(text_width);
                            let title = ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(card.title)
                                            .font(theme::bold(16.0))
                                            .color(colors.title.gamma_multiply(a)),
                                    )
                                    .wrap()
                                    .sense(Sense::click()),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            let body = ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(card.body)
                                            .font(theme::regular(13.0))
                                            .color(Color32::WHITE.gamma_multiply(a)),
                                    )
                                    .wrap()
                                    .sense(Sense::click()),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if let Some(extra) = &card.extra {
                                ui.add(egui::Label::new(
                                    egui::RichText::new(extra)
                                        .font(theme::medium(11.0))
                                        .color(CARD_DIM.gamma_multiply(a)),
                                ));
                            }
                            if title.clicked() || body.clicked() {
                                event = CardEvent::Open;
                            }
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            if theme::icon_button(
                                ui,
                                Icon::X,
                                11.0,
                                CARD_DIM.gamma_multiply(a),
                                Color32::WHITE,
                                "Cerrar",
                            )
                            .clicked()
                            {
                                event = CardEvent::Dismiss;
                            }
                        });
                    });
                });
        });
    (top_y + resp.response.rect.height() + GAP, event)
}

/// Pila de tarjetas de la esquina superior (izquierda o derecha según
/// `App::notification_side`): primero las menciones / DMs (`App::in_app_notifications`, click = abrir la conversación) y debajo
/// los avisos genéricos (`App::toasts`). Se cierran solas a los pocos
/// segundos, con la X, o al abrir la conversación.
pub fn show_in_app(app: &mut App, ui: &mut egui::Ui) {
    app.in_app_notifications
        .retain(|n| n.created.elapsed().as_secs_f32() < LIFETIME_SECS);
    app.toasts
        .retain(|t| t.created.elapsed().as_secs_f32() < TOAST_LIFETIME_SECS);

    let ctx = ui.ctx().clone();
    let anim_id = egui::Id::new("in_app_notification_anim");
    if app.in_app_notifications.is_empty() && app.toasts.is_empty() {
        // Sin tarjetas: olvida los relojes de animación, así una mención
        // futura de la misma conversación vuelve a caer desde arriba.
        ctx.data_mut(|d| d.remove::<HashMap<String, f64>>(anim_id));
        return;
    }

    let palette = app.palette;
    let side = app.notification_side;
    let now = ctx.input(|i| i.time);
    // Instante (reloj de egui) en que cada conversación empezó a mostrarse.
    // Se guarda aparte de `InAppNotification::created` porque ese se
    // reinicia cada vez que llega otro mensaje a la misma tarjeta, y no
    // queremos que la tarjeta vuelva a caer cada vez.
    let mut starts: HashMap<String, f64> = ctx.data_mut(|d| d.get_temp(anim_id)).unwrap_or_default();
    starts.retain(|k, _| {
        app.in_app_notifications
            .iter()
            .any(|n| n.target.channel_id() == k.as_str())
    });

    let mut animating = false;
    let mut y = MARGIN_TOP;
    let mut dismiss_notification: Option<usize> = None;
    let mut open_notification: Option<usize> = None;
    let mut dismiss_toast: Option<usize> = None;

    for (i, n) in app.in_app_notifications.iter().enumerate() {
        let channel_id = n.target.channel_id();
        let start = *starts.entry(channel_id.to_string()).or_insert(now);
        let progress = progress_from_secs((now - start) as f32);
        let fade = fade_out(n.created.elapsed().as_secs_f32(), LIFETIME_SECS);
        animating |= progress < 1.0 || fade < 1.0;
        let card = Card {
            id: egui::Id::new(("in_app_notification", channel_id)),
            tone: Tone::Mention,
            title: &n.title,
            body: &n.body,
            avatar: Some(CardAvatar { url: n.avatar_url.as_deref(), initial: &n.initial }),
            extra: (n.count > 1).then(|| format!("{} mensajes nuevos", n.count)),
            progress,
            fade,
        };
        let (next_y, event) = draw_card(&ctx, &palette, &card, y, side);
        y = next_y;
        match event {
            CardEvent::Dismiss => dismiss_notification = Some(i),
            CardEvent::Open => open_notification = Some(i),
            CardEvent::None => {}
        }
    }

    for (i, t) in app.toasts.iter().enumerate() {
        let elapsed = t.created.elapsed().as_secs_f32();
        let progress = progress_from_secs(elapsed);
        let fade = fade_out(elapsed, TOAST_LIFETIME_SECS);
        animating |= progress < 1.0 || fade < 1.0;
        let card = Card {
            id: egui::Id::new(("toast", t.created)),
            tone: match t.kind {
                ToastKind::Info => Tone::Info,
                ToastKind::Success => Tone::Success,
                ToastKind::Warning => Tone::Warning,
            },
            title: &t.title,
            body: &t.message,
            avatar: None,
            extra: None,
            progress,
            fade,
        };
        let (next_y, event) = draw_card(&ctx, &palette, &card, y, side);
        y = next_y;
        if matches!(event, CardEvent::Dismiss) {
            dismiss_toast = Some(i);
        }
    }

    ctx.data_mut(|d| d.insert_temp(anim_id, starts));
    // Mientras anima, un frame tras otro; el resto del tiempo alcanza con
    // despertar de a ratos para que las tarjetas se retiren solas.
    if animating {
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    if let Some(i) = dismiss_toast {
        if i < app.toasts.len() {
            app.toasts.remove(i);
        }
    }
    if let Some(i) = dismiss_notification {
        if i < app.in_app_notifications.len() {
            app.in_app_notifications.remove(i);
        }
    } else if let Some(i) = open_notification {
        if i < app.in_app_notifications.len() {
            let notification = app.in_app_notifications.remove(i);
            app.open_notification_target(&notification.target);
        }
    }
}
