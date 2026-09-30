//! Popup con las personas que tienen un rol, al clickear una mención `@rol`
//! en el chat. Solo se pueden listar los miembros que el cliente ya conoce
//! (`Server::member_info`, que se llena con la lista de miembros, los
//! mensajes vistos y los chunks del Gateway): Discord no permite pedir "todos
//! los miembros de un rol" sin permisos de moderación.

use egui::{Align2, Area, Color32, CornerRadius, Frame, Id, Margin, Order, ScrollArea, Sense, Stroke, Vec2};

use crate::lib::data::NameResolver;
use crate::lib::state::{App, Screen};
use crate::ui::theme;

const CARD_W: f32 = 250.0;
const CARD_EST_H: f32 = 340.0;
const ROW_H: f32 = 28.0;

struct Member {
    id: String,
    name: String,
    color: Option<Color32>,
    avatar_url: Option<String>,
}

enum Action {
    Close,
    OpenProfile(Member),
}

fn members_with_role(app: &App, role_id: &str) -> Vec<Member> {
    let Screen::Server(index) = app.screen else { return Vec::new() };
    let Some(server) = app.servers.get(index) else { return Vec::new() };
    let resolver = NameResolver::new(&server.member_info, &server.roles);
    let mut out: Vec<Member> = server
        .member_info
        .iter()
        .filter(|(_, info)| info.roles.iter().any(|r| r == role_id))
        .map(|(id, _)| {
            let known = server.known_users.get(id);
            let base = known.map(|k| k.name.clone()).unwrap_or_else(|| {
                let tail: String = id.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
                format!("Usuario {tail}")
            });
            let (name, color) = resolver.identity(id, &base);
            Member { id: id.clone(), name, color, avatar_url: known.and_then(|k| k.avatar_url.clone()) }
        })
        .collect();
    out.sort_by_key(|m| m.name.to_lowercase());
    out
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    if app.role_popup.is_none() {
        return;
    }
    // Si se salió del server, el popup no tiene sentido.
    if !matches!(app.screen, Screen::Server(_)) {
        app.role_popup = None;
        return;
    }
    let (role_id, role_name, role_color, anchor) = {
        let p = app.role_popup.as_ref().unwrap();
        (p.role_id.clone(), p.name.clone(), p.color, p.anchor)
    };
    let members = members_with_role(app, &role_id);
    let palette = app.palette;
    let ctx = ui.ctx().clone();
    let screen = ctx.viewport_rect();

    let title_color = if role_color != 0 {
        Color32::from_rgb((role_color >> 16) as u8, (role_color >> 8) as u8, role_color as u8)
    } else {
        palette.accent
    };

    let mut pos = anchor.map(|a| a + Vec2::new(12.0, 12.0)).unwrap_or_else(|| screen.center() - Vec2::new(CARD_W / 2.0, CARD_EST_H / 2.0));
    pos.x = pos.x.min(screen.max.x - CARD_W - 12.0).max(screen.min.x + 12.0);
    pos.y = pos.y.min(screen.max.y - CARD_EST_H - 12.0).max(screen.min.y + 12.0);

    let mut action: Option<Action> = None;

    let inner = Area::new(Id::new("role_members_popout"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(&ctx, |ui| {
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::RADIUS + 2))
                .inner_margin(Margin::same(12))
                .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: palette.shadow })
                .show(ui, |ui| {
                    ui.set_width(CARD_W - 24.0);
                    ui.horizontal(|ui| {
                        theme::text(ui, &format!("@{role_name}"), theme::semibold(14.0), title_color);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            theme::text(ui, &members.len().to_string(), theme::regular(12.0), palette.dim);
                        });
                    });
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    if members.is_empty() {
                        theme::text(
                            ui,
                            "Todavía no hay miembros cargados con este rol.",
                            theme::regular(12.5),
                            palette.dim,
                        );
                        return;
                    }
                    ScrollArea::vertical().max_height(CARD_EST_H - 90.0).auto_shrink([false, true]).show(ui, |ui| {
                        for m in &members {
                            let (rect, resp) =
                                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
                            if resp.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(4), palette.surface_hover);
                            }
                            ui.painter().text(
                                rect.left_center() + Vec2::new(8.0, 0.0),
                                Align2::LEFT_CENTER,
                                &m.name,
                                theme::medium(13.0),
                                m.color.unwrap_or(palette.text),
                            );
                            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                action = Some(Action::OpenProfile(Member {
                                    id: m.id.clone(),
                                    name: m.name.clone(),
                                    color: m.color,
                                    avatar_url: m.avatar_url.clone(),
                                }));
                            }
                        }
                    });
                });
        });

    let card_rect = inner.response.rect;
    let clicked_outside = ctx.input(|i| {
        i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|p| !card_rect.contains(p))
    });
    if clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = Some(Action::Close);
    }

    match action {
        Some(Action::Close) => app.role_popup = None,
        Some(Action::OpenProfile(m)) => {
            app.role_popup = None;
            // El pedido lo levanta `App::ui` en el próximo frame.
            crate::ui::profile_popup::request_open(&ctx, m.id, m.name, m.avatar_url, palette.accent);
        }
        None => {}
    }
}
