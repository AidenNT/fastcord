//! Tarjeta de invitación de Discord, debajo del mensaje que trae el link
//! (`discord.gg/<código>`), como en el cliente oficial:
//!
//! * **Válida**: franja gris arriba, ícono del server, nombre (con insignia
//!   de comunidad / verificado / socio), "● N en línea ● N miembros",
//!   "Est. mes año", descripción (2 líneas) y botón verde
//!   ("Unirse" te une desde el propio cliente —si Discord pide captcha se
//!   abre la ventana para resolverlo—; "Ir al servidor" si ya estás, que
//!   abre el server dentro del propio cliente).
//! * **No válida / vencida**: "Has enviado una invitación, pero" +
//!   sobre rojo + "Invitación no válida".
//! * **Cargando**: el mismo marco con barras grises.
//!
//! Los datos los resuelve `discord::invites` (caché + hilo aparte).

use std::sync::Arc;

use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::discord::invites::{self, InviteInfo, InviteState, JoinState};
use crate::ui::extra;
use crate::ui::theme::{self, Icon, Palette};

const CARD_MAX_W: f32 = 400.0;
const CARD_RADIUS: u8 = 12;
const BANNER_H: f32 = 72.0;
const ICON_R: f32 = 32.0;
const PAD: f32 = 16.0;
const BUTTON_H: f32 = 38.0;
const GREEN_BUTTON: Color32 = Color32::from_rgb(0x00, 0x87, 0x46);
const DOT_ONLINE: Color32 = Color32::from_rgb(0x23, 0xa5, 0x5a);
const DOT_MEMBERS: Color32 = Color32::from_rgb(0x80, 0x84, 0x8e);
const BADGE_COMMUNITY: Color32 = Color32::from_rgb(0xb2, 0x7f, 0xe0);

/// Dibuja una tarjeta por cada invitación que aparece en `content`.
/// `is_own`: el mensaje es tuyo (cambia el texto de la tarjeta inválida).
pub fn show(ui: &mut Ui, palette: &Palette, content: &str, is_own: bool) {
    // Barato en el caso normal: sin "discord" en el texto no hay nada que
    // buscar (esto corre por cada mensaje visible, en cada frame).
    if !content.contains("discord") && !content.contains("Discord") && !content.contains("DISCORD") {
        return;
    }
    for code in invites::extract_codes(content) {
        match invites::lookup(ui.ctx(), &code) {
            InviteState::Ready(info) => {
                ui.add_space(4.0);
                valid_card(ui, palette, &info);
            }
            InviteState::Invalid => {
                ui.add_space(4.0);
                invalid_card(ui, palette, is_own);
            }
            InviteState::Loading => {
                ui.add_space(4.0);
                loading_card(ui, palette);
            }
            // Sin red / sin sesión: el link queda como texto, sin tarjeta.
            InviteState::Failed => {}
        }
    }
}

fn card_width(ui: &Ui) -> f32 {
    ui.available_width().min(CARD_MAX_W).max(240.0)
}

fn one_line(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_w: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font, color, max_w.max(20.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    painter.layout_job(job)
}

/// Fondo + borde + franja gris (con degradé hacia el fondo) de las
/// tarjetas con banner.
fn paint_card_frame(ui: &mut Ui, palette: &Palette, rect: Rect, with_banner: bool) {
    let painter = ui.painter().clone();
    let radius = CornerRadius::same(CARD_RADIUS);
    painter.rect_filled(rect, radius, palette.surface);
    if with_banner {
        let gray = extra::blend(palette.surface, Color32::WHITE, 0.16);
        let banner = Rect::from_min_size(rect.min, Vec2::new(rect.width(), BANNER_H));
        painter.rect_filled(
            banner,
            CornerRadius { nw: CARD_RADIUS, ne: CARD_RADIUS, sw: 0, se: 0 },
            gray,
        );
        // La mitad de abajo se desvanece hacia el color de la tarjeta.
        let fade = Rect::from_min_max(
            Pos2::new(banner.left(), banner.center().y),
            Pos2::new(banner.right(), banner.bottom()),
        );
        extra::paint_vertical_gradient(ui, fade, gray, palette.surface);
    }
    painter.rect_stroke(rect, radius, Stroke::new(1.0, palette.outline), StrokeKind::Inside);
}

// ---------------------------------------------------------------------
// Válida
// ---------------------------------------------------------------------

fn valid_card(ui: &mut Ui, palette: &Palette, info: &InviteInfo) {
    let width = card_width(ui);
    let text_w = width - PAD * 2.0;
    let painter = ui.painter().clone();

    // Se mide todo ANTES de reservar el rect: el alto depende de la
    // descripción (0, 1 o 2 líneas).
    let desc = info.description.as_deref().map(|d| {
        let mut job = egui::text::LayoutJob::simple(
            d.replace('\n', " "),
            theme::regular(13.5),
            palette.secondary,
            text_w,
        );
        job.wrap.max_rows = 2;
        job.wrap.overflow_character = Some('…');
        painter.layout_job(job)
    });

    let name_y = BANNER_H + ICON_R + 12.0;
    let stats_y = name_y + 26.0;
    let est_y = stats_y + 22.0;
    let desc_y = est_y + 26.0;
    let btn_y = match &desc {
        Some(g) => desc_y + g.size().y + 12.0,
        None => desc_y,
    };
    let total_h = btn_y + BUTTON_H + PAD;

    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, total_h), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    paint_card_frame(ui, palette, rect, true);

    // Ícono: una placa del color de la tarjeta recorta la franja gris.
    let icon_center = Pos2::new(rect.left() + PAD + 4.0 + ICON_R, rect.top() + BANNER_H);
    painter.rect_filled(
        Rect::from_center_size(icon_center, Vec2::splat(ICON_R * 2.0 + 8.0)),
        CornerRadius::same(18),
        palette.surface,
    );
    let initial = info.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    extra::avatar_rounded(
        ui,
        icon_center,
        ICON_R,
        14.0,
        info.icon_url.as_deref(),
        palette.surface_active,
        &initial,
        palette,
    );

    // Nombre + insignia.
    let has_badge = info.community || info.verified || info.partnered;
    let badge_room = if has_badge { 24.0 } else { 0.0 };
    let name_galley = one_line(&painter, &info.name, theme::semibold(16.0), palette.text, text_w - badge_room);
    let name_w = name_galley.size().x;
    painter.galley(rect.min + Vec2::new(PAD, name_y), name_galley, palette.text);
    if has_badge {
        let badge = Rect::from_min_size(
            rect.min + Vec2::new(PAD + name_w + 6.0, name_y + 1.0),
            Vec2::splat(18.0),
        );
        if info.verified || info.partnered {
            let color = if info.partnered { Color32::from_rgb(0x58, 0x65, 0xf2) } else { DOT_ONLINE };
            theme::paint_icon(ui, Icon::BadgeCheck, badge, 18.0, color);
        } else {
            painter.rect_filled(badge, CornerRadius::same(6), BADGE_COMMUNITY);
            theme::paint_icon(ui, Icon::House, badge, 11.0, Color32::WHITE);
        }
    }

    // "● N en línea  ● N miembros".
    let mut x = rect.left() + PAD;
    let stats_mid = rect.top() + stats_y + 9.0;
    let stat = |online: bool, text: String, x: &mut f32| {
        let color = if online { DOT_ONLINE } else { DOT_MEMBERS };
        painter.circle_filled(Pos2::new(*x + 4.0, stats_mid), 4.0, color);
        let galley = painter.layout_no_wrap(text, theme::regular(13.5), palette.secondary);
        let w = galley.size().x;
        painter.galley(Pos2::new(*x + 14.0, rect.top() + stats_y), galley, palette.secondary);
        *x += 14.0 + w + 12.0;
    };
    if let Some(n) = info.online {
        stat(true, format!("{} en línea", invites::format_count(n)), &mut x);
    }
    if let Some(n) = info.members {
        stat(false, format!("{} miembros", invites::format_count(n)), &mut x);
    }

    if let Some(est) = invites::established_label(&info.guild_id) {
        let galley = painter.layout_no_wrap(est, theme::regular(13.5), palette.dim);
        painter.galley(rect.min + Vec2::new(PAD, est_y), galley, palette.dim);
    }
    if let Some(galley) = desc {
        painter.galley(rect.min + Vec2::new(PAD, desc_y), galley, palette.secondary);
    }

    // Botón: "Ir al servidor" si ya estás; si no, "Unirse" (te une desde acá,
    // y si Discord pide captcha se abre la ventana para resolverlo).
    let joined = invites::is_joined(&info.guild_id);
    let join_state = invites::join_state(&info.code);
    let label = if joined {
        "Ir al servidor"
    } else {
        match join_state {
            JoinState::Idle => "Unirse",
            JoinState::Joining => "Uniéndose…",
            JoinState::Failed => "No se pudo unir. Reintentar",
        }
    };
    let btn_rect = Rect::from_min_size(rect.min + Vec2::new(PAD, btn_y), Vec2::new(text_w, BUTTON_H));
    let button = egui::Button::new(
        RichText::new(label).font(theme::semibold(14.0)).color(Color32::WHITE),
    )
    .fill(GREEN_BUTTON)
    .stroke(Stroke::NONE)
    .corner_radius(CornerRadius::same(8));
    let busy = !joined && join_state == JoinState::Joining;
    let response = ui.put(btn_rect, button).on_hover_cursor(if busy {
        egui::CursorIcon::Default
    } else {
        egui::CursorIcon::PointingHand
    });
    if response.clicked() && !busy {
        if joined {
            // Abre el server dentro del cliente (ver `App::open_server`).
            invites::request_goto(ui.ctx(), &info.guild_id);
        } else {
            invites::start_join(ui.ctx(), info);
        }
    }
}

// ---------------------------------------------------------------------
// No válida
// ---------------------------------------------------------------------

fn invalid_card(ui: &mut Ui, palette: &Palette, is_own: bool) {
    let width = card_width(ui);
    let total_h = 112.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, total_h), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    paint_card_frame(ui, palette, rect, false);
    let painter = ui.painter().clone();

    let header = if is_own { "Has enviado una invitación, pero" } else { "Has recibido una invitación, pero" };
    let sub = if is_own { "Intenta enviar una nueva invitación" } else { "Pídele a quien la envió una nueva invitación" };
    painter.galley(
        rect.min + Vec2::new(PAD, 16.0),
        painter.layout_no_wrap(header.to_string(), theme::regular(13.5), palette.secondary),
        palette.secondary,
    );

    // Cuadro rojo oscuro con el sobre.
    let box_rect = Rect::from_min_size(rect.min + Vec2::new(PAD, 44.0), Vec2::splat(52.0));
    let red = palette.danger;
    painter.rect_filled(box_rect, CornerRadius::same(10), extra::blend(palette.surface, red, 0.18));
    let env = Rect::from_center_size(box_rect.center(), Vec2::new(24.0, 17.0));
    painter.rect_filled(env, CornerRadius::same(3), red);
    let dark = extra::blend(palette.surface, red, 0.18);
    let stroke = Stroke::new(1.6, dark);
    let mid = Pos2::new(env.center().x, env.top() + env.height() * 0.58);
    painter.line_segment([Pos2::new(env.left() + 2.0, env.top() + 2.5), mid], stroke);
    painter.line_segment([Pos2::new(env.right() - 2.0, env.top() + 2.5), mid], stroke);

    let text_x = box_rect.right() + 14.0;
    let max_w = rect.right() - PAD - text_x;
    let title = one_line(&painter, "Invitación no válida", theme::semibold(15.5), red, max_w);
    painter.galley(Pos2::new(text_x, box_rect.top() + 6.0), title, red);
    let subtitle = one_line(&painter, sub, theme::regular(13.5), palette.secondary, max_w);
    painter.galley(Pos2::new(text_x, box_rect.top() + 28.0), subtitle, palette.secondary);
}

// ---------------------------------------------------------------------
// Cargando
// ---------------------------------------------------------------------

fn loading_card(ui: &mut Ui, palette: &Palette) {
    let width = card_width(ui);
    let total_h = BANNER_H + ICON_R + 12.0 + 26.0 + 22.0 + 26.0 + BUTTON_H + PAD;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, total_h), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    paint_card_frame(ui, palette, rect, true);
    let painter = ui.painter().clone();
    let icon_center = Pos2::new(rect.left() + PAD + 4.0 + ICON_R, rect.top() + BANNER_H);
    painter.rect_filled(
        Rect::from_center_size(icon_center, Vec2::splat(ICON_R * 2.0 + 8.0)),
        CornerRadius::same(18),
        palette.surface,
    );
    painter.rect_filled(
        Rect::from_center_size(icon_center, Vec2::splat(ICON_R * 2.0)),
        CornerRadius::same(14),
        palette.surface_active,
    );
    let bar = |y: f32, w: f32| {
        painter.rect_filled(
            Rect::from_min_size(
                rect.min + Vec2::new(PAD, BANNER_H + ICON_R + 12.0 + y),
                Vec2::new(w.min(width - PAD * 2.0), 12.0),
            ),
            CornerRadius::same(6),
            palette.surface_active,
        );
    };
    bar(2.0, 160.0);
    bar(28.0, 220.0);
    bar(50.0, 90.0);
}
