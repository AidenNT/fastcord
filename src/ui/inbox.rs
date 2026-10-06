//! Notificaciones en la barra superior: botón de campana con el contador de
//! menciones sin leer y, al apretarlo, un panel desplegable con los DMs y
//! canales que tienen algo pendiente.
//!
//! Antes esto vivía como una sección más del Inicio (`ui::home`); ahora el
//! Inicio muestra los canales recientes y las notificaciones quedaron a un
//! click desde cualquier pantalla.

use egui::{
    Align2, Area, Color32, CornerRadius, Frame, Id, Margin, Order, Rect, ScrollArea, Sense, Stroke, Vec2,
};

use crate::lib::state::App;
use crate::ui::extra;
use crate::ui::home::{clipped_line, plain_text};
use crate::ui::theme::{self, Icon, Palette};

const PANEL_W: f32 = 392.0;
const PANEL_MAX_H: f32 = 520.0;
const CARD_RADIUS: u8 = 14;

/// A dónde lleva el click en una notificación.
#[derive(Clone, Copy)]
enum Target {
    /// Un DM de `app.dms` (por índice).
    Dm(usize),
    /// Un canal de server: (server, categoría, canal).
    Channel(usize, usize, usize),
}

struct Entry {
    title: String,
    avatar_url: Option<String>,
    avatar_color: Color32,
    initial: String,
    count: u32,
    /// "3 mensajes nuevos" / "2 menciones".
    summary: String,
    preview: Option<String>,
    /// Para ordenar: lo más reciente primero.
    sort: u64,
    target: Target,
}

/// DMs con mensajes sin leer y canales de servers con menciones, lo más
/// reciente primero.
fn collect(app: &App) -> Vec<Entry> {
    use crate::lib::notifications::{dm_sort_key, snowflake};
    let mut out = Vec::new();

    // DMs: cada mensaje sin leer cuenta.
    for (i, dm) in app.dms.iter().enumerate() {
        let count = app.unread_mentions.get(&dm.id).copied().unwrap_or(0);
        if count == 0 {
            continue;
        }
        let user_id = dm.recipients.first().map(|r| r.id.as_str()).unwrap_or("");
        let preview = app
            .friends
            .iter()
            .find(|f| !user_id.is_empty() && f.user_id == user_id)
            .and_then(|f| f.messages.iter().rev().find(|m| !m.is_own))
            .map(plain_text)
            .filter(|t| !t.is_empty());
        out.push(Entry {
            title: dm.username.clone(),
            avatar_url: dm.avatar_url.clone(),
            avatar_color: Color32::from_rgb(90, 170, 210),
            initial: dm
                .username
                .chars()
                .find(|c| c.is_alphanumeric())
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_string()),
            count,
            summary: if count == 1 { "1 mensaje nuevo".to_string() } else { format!("{count} mensajes nuevos") },
            preview,
            sort: dm_sort_key(dm),
            target: Target::Dm(i),
        });
    }

    // Canales de servers: solo cuentan las menciones.
    for (si, server) in app.servers.iter().enumerate() {
        for (ci, category) in server.categories.iter().enumerate() {
            for (hi, ch) in category.channels.iter().enumerate() {
                if ch.is_thread || !server.channel_visible(ci, hi) {
                    continue;
                }
                let Some(id) = ch.channel_id.as_deref() else { continue };
                let count = app.unread_mentions.get(id).copied().unwrap_or(0);
                if count == 0 {
                    continue;
                }
                let preview = ch
                    .messages
                    .iter()
                    .rev()
                    .find(|m| !m.is_own)
                    .map(|m| format!("{}: {}", m.author, plain_text(m)))
                    .filter(|t| !t.trim_end_matches(": ").is_empty());
                out.push(Entry {
                    title: format!("#{} · {}", ch.name, server.name),
                    avatar_url: server.icon_url.clone(),
                    avatar_color: server.icon_color,
                    initial: server.icon_initial.clone(),
                    count,
                    summary: if count == 1 { "1 mención".to_string() } else { format!("{count} menciones") },
                    preview,
                    sort: ch.messages.last().map(|m| snowflake(&m.id)).unwrap_or(0),
                    target: Target::Channel(si, ci, hi),
                });
            }
        }
    }

    out.sort_by(|a, b| b.sort.cmp(&a.sort));
    out
}

/// Botón de campana de la barra superior, con el globito rojo de pendientes.
/// Devuelve su `Rect` para anclar el panel (`show_panel`).
pub fn bell_button(ui: &mut egui::Ui, app: &mut App) -> Rect {
    let palette = app.palette;
    let open = app.inbox_open;
    let color = if open { palette.text } else { palette.dim };
    let response = theme::icon_button(ui, Icon::Bell, 15.0, color, palette.text, "Notificaciones");
    crate::ui::notifications::paint_badge(
        ui.painter(),
        response.rect.right_top() + Vec2::new(-4.0, 7.0),
        app.total_mentions(),
        &palette,
        palette.window,
    );
    if response.clicked() {
        app.inbox_open = !app.inbox_open;
    }
    response.rect
}

/// Panel desplegable debajo de la campana. Se cierra con click afuera, con
/// Escape o al elegir una notificación.
pub fn show_panel(app: &mut App, ctx: &egui::Context, bell: Rect) {
    if !app.inbox_open {
        return;
    }
    let palette = app.palette;
    let entries = collect(app);
    let total: u32 = entries.iter().map(|e| e.count).sum();
    let screen = ctx.viewport_rect();

    let x = (bell.right() - PANEL_W).max(screen.left() + 8.0);
    let pos = egui::pos2(x, bell.bottom() + 6.0);
    let mut chosen: Option<Target> = None;

    let inner = Area::new(Id::new("topbar_inbox_panel"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::radius() + 4))
                .inner_margin(Margin::same(12))
                .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: palette.shadow })
                .show(ui, |ui| {
                    ui.set_width(PANEL_W - 24.0);
                    ui.horizontal(|ui| {
                        theme::text(ui, "Notificaciones", theme::bold(16.0), palette.text);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if total > 0 {
                                let label = if total > 99 { "99+".to_string() } else { total.to_string() };
                                let w = if label.len() > 1 { 26.0 } else { 20.0 };
                                let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 20.0), Sense::hover());
                                ui.painter().rect_filled(rect, CornerRadius::same(10), palette.danger);
                                ui.painter().text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    label,
                                    theme::semibold(11.0),
                                    palette.on_accent,
                                );
                            }
                        });
                    });
                    ui.add_space(8.0);

                    if entries.is_empty() {
                        ui.add_space(6.0);
                        theme::text(ui, "Estás al día", theme::semibold(14.0), palette.text);
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("No tenés mensajes ni menciones sin leer.")
                                    .font(theme::regular(12.5))
                                    .color(palette.dim),
                            )
                            .wrap(),
                        );
                        ui.add_space(6.0);
                        return;
                    }

                    ScrollArea::vertical()
                        .id_salt("topbar_inbox_scroll")
                        .max_height(PANEL_MAX_H - 70.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            let width = (ui.available_width() - 6.0).max(0.0);
                            for entry in &entries {
                                if card(ui, &palette, entry, width) {
                                    chosen = Some(entry.target);
                                }
                                ui.add_space(8.0);
                            }
                        });
                });
        });

    let panel_rect = inner.response.rect;
    let clicked_outside = ctx.input(|i| {
        i.pointer.any_click()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !panel_rect.contains(p) && !bell.contains(p))
    });
    if clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.inbox_open = false;
    }

    if let Some(target) = chosen {
        app.inbox_open = false;
        match target {
            Target::Dm(i) => app.open_dm_from_channel(i),
            Target::Channel(server, category, channel) => app.open_channel_at(server, category, channel, None),
        }
    }
}

/// Devuelve `true` si se hizo click en la tarjeta.
fn card(ui: &mut egui::Ui, palette: &Palette, entry: &Entry, width: f32) -> bool {
    let height = if entry.preview.is_some() { 80.0 } else { 60.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let fill = if resp.hovered() { palette.surface_hover } else { palette.surface };
    ui.painter().rect_filled(rect, CornerRadius::same(CARD_RADIUS), fill);

    let avatar_center = rect.left_top() + Vec2::new(32.0, 30.0);
    extra::avatar(
        ui,
        avatar_center,
        17.0,
        entry.avatar_url.as_deref(),
        entry.avatar_color,
        &entry.initial,
        palette,
    );

    let text_x = 62.0;
    let text_w = (width - text_x - 52.0).max(0.0);
    let line = |y: f32, h: f32| Rect::from_min_size(rect.left_top() + Vec2::new(text_x, y), Vec2::new(text_w, h));
    clipped_line(ui, line(11.0, 18.0), &entry.title, theme::semibold(15.0), palette.text);

    theme::paint_icon(
        ui,
        Icon::MessageCircle,
        Rect::from_center_size(rect.left_top() + Vec2::new(text_x + 7.0, 40.0), Vec2::splat(14.0)),
        13.0,
        palette.secondary,
    );
    clipped_line(
        ui,
        Rect::from_min_size(
            rect.left_top() + Vec2::new(text_x + 20.0, 33.0),
            Vec2::new((text_w - 20.0).max(0.0), 16.0),
        ),
        &entry.summary,
        theme::regular(12.5),
        palette.secondary,
    );
    if let Some(preview) = &entry.preview {
        clipped_line(ui, line(55.0, 16.0), preview, theme::regular(12.5), palette.dim);
    }

    crate::ui::notifications::paint_badge(
        ui.painter(),
        rect.right_top() + Vec2::new(-26.0, 28.0),
        entry.count,
        palette,
        fill,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}
