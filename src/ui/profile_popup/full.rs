//! Perfil completo: el modal grande que abre "Ver perfil completo" en el
//! panel del DM, como el de Discord.
//!
//! * Columna izquierda: la tarjeta del usuario (banner, avatar con decoración
//!   y estado, nombre, insignias, botones, bio, "miembro desde", roles y
//!   conexiones), con el profile frame y el efecto de perfil alrededor.
//! * Columna derecha: pestañas "Actividad", amigos en común y servidores en
//!   común.
//!
//! Los datos salen del mismo `ProfilePopup` ya cargado (el del panel del DM o
//! el del popup), así que abrirlo no hace ningún pedido nuevo. Discord no
//! expone por API el historial de actividad ni la lista de deseos: la pestaña
//! de actividad muestra lo que la persona está haciendo AHORA.

use egui::{Align, Layout, UiBuilder};

use super::*;

const MODAL_MAX_W: f32 = 1180.0;
const MODAL_MAX_H: f32 = 900.0;
const MODAL_PAD: f32 = 24.0;
const FULL_BANNER_H: f32 = 175.0;
const FULL_AVATAR_R: f32 = 60.0;
const FULL_PAD: f32 = 24.0;
const MAX_ACTIVITIES: usize = 6;

struct FriendRow {
    name: String,
    handle: String,
    avatar_url: Option<String>,
}

struct GuildRow {
    name: String,
    nick: Option<String>,
    icon_url: Option<String>,
    initial: String,
    color: Color32,
}

/// Todo lo que hace falta para dibujar el modal, sacado de `app` antes de
/// entrar al `Area` (que no puede tener `&mut App` adentro).
struct Full {
    base: View,
    activities: Vec<PresenceActivity>,
    friends: Vec<FriendRow>,
    guilds: Vec<GuildRow>,
}

enum FullAction {
    Close,
    Message,
    EditProfile,
    CopyId,
    Tab(usize),
}

/// El perfil ya cargado de `user_id`: el del panel del DM o el del popup.
fn source<'a>(app: &'a App, user_id: &str) -> Option<&'a ProfilePopup> {
    app.dm_profile
        .as_ref()
        .filter(|p| p.user_id == user_id)
        .or_else(|| app.profile_popup.as_ref().filter(|p| p.user_id == user_id))
}

impl Full {
    fn build(app: &App, popup: &ProfilePopup) -> Option<Self> {
        let base = View::build(app, popup)?;
        let profile = popup.profile.as_ref();
        let activities = app
            .activities_of(&popup.user_id)
            .into_iter()
            .filter(|a| a.kind != 4 && !a.name.is_empty())
            .take(MAX_ACTIVITIES)
            .collect();
        let friends = profile
            .map(|p| {
                p.mutual_friends
                    .iter()
                    .map(|u| FriendRow {
                        name: u.display_name().to_string(),
                        handle: u.username.clone(),
                        avatar_url: u.avatar_url(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Discord solo manda el id de cada server en común: nombre e ícono
        // salen de los servers que ya tenemos cargados.
        let guilds = profile
            .map(|p| {
                p.mutual_guilds
                    .iter()
                    .filter_map(|g| {
                        let s = app.servers.iter().find(|s| s.guild_id == g.id)?;
                        Some(GuildRow {
                            name: s.name.clone(),
                            nick: g.nick.clone().filter(|n| !n.is_empty()),
                            icon_url: s.icon_url.clone(),
                            initial: s.icon_initial.clone(),
                            color: s.icon_color,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self { base, activities, friends, guilds })
    }
}

pub fn show_full(app: &mut App, ui: &mut egui::Ui) {
    let Some(state) = app.profile_full.as_ref() else { return };
    let (user_id, tab) = (state.user_id.clone(), state.tab);
    let full = source(app, &user_id).and_then(|p| Full::build(app, p));
    let Some(full) = full else {
        app.profile_full = None;
        return;
    };
    let palette = app.palette;
    let tone = Tone::for_view(&full.base, &palette);
    let ctx = ui.ctx().clone();
    let screen = ctx.viewport_rect();
    let mut action: Option<FullAction> = None;

    let modal_w = (screen.width() - 48.0).clamp(320.0, MODAL_MAX_W);
    let modal_h = (screen.height() - 48.0).clamp(320.0, MODAL_MAX_H);
    let modal = Rect::from_center_size(screen.center(), vec2(modal_w, modal_h));
    let left_w = (modal_w * 0.42).clamp(280.0, 500.0);
    // El frame sobresale de la tarjeta: se le deja ese espacio alrededor.
    let (pad_top, pad_bottom, pad_side) = match &full.base.frame {
        Some(f) => frame_padding(f, left_w),
        None => (0.0, 0.0, 0.0),
    };
    let l_rect = Rect::from_min_max(
        pos2(modal.left() + MODAL_PAD + pad_side, modal.top() + MODAL_PAD + pad_top),
        pos2(modal.left() + MODAL_PAD + pad_side + left_w, modal.bottom() - MODAL_PAD - pad_bottom),
    );
    let r_rect = Rect::from_min_max(
        pos2(l_rect.right() + pad_side + 32.0, modal.top() + MODAL_PAD + 8.0),
        pos2((modal.right() - 72.0).max(l_rect.right() + 160.0), modal.bottom() - MODAL_PAD),
    );
    let radius: u8 = 18;

    let area = Area::new(Id::new("profile_full_modal"))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .show(&ctx, |ui| {
            ui.set_width(screen.width());
            ui.set_height(screen.height());
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(170));
            // Consume los clicks para que no lleguen a lo de atrás; los de
            // afuera del modal lo cierran.
            let scrim = ui.interact(screen, Id::new("profile_full_scrim"), Sense::click());
            if scrim.clicked() && scrim.interact_pointer_pos().is_some_and(|p| !modal.contains(p)) {
                action = Some(FullAction::Close);
            }
            let bg = extra::blend(tone.bottom, Color32::BLACK, 0.35);
            ui.painter().rect_filled(modal, CornerRadius::same(radius), bg);

            // Lugar reservado para las capas `back` del frame (debajo de la tarjeta).
            let wide = ui.painter().with_clip_rect(modal);
            let back_idx = wide.add(Shape::Noop);

            // -- Tarjeta izquierda --
            ui.painter().rect_filled(l_rect, CornerRadius::same(radius), tone.bottom);
            let gradient = Rect::from_min_max(
                pos2(l_rect.left(), l_rect.top() + FULL_BANNER_H),
                pos2(
                    l_rect.right(),
                    (l_rect.bottom() - f32::from(radius)).max(l_rect.top() + FULL_BANNER_H + 1.0),
                ),
            );
            ui.painter().add(gradient_mesh(gradient, tone.top, tone.bottom));
            let mut l_ui = ui.new_child(
                UiBuilder::new().max_rect(l_rect).layout(Layout::top_down(Align::Min)),
            );
            l_ui.set_clip_rect(l_rect);
            egui::ScrollArea::vertical()
                .id_salt("profile_full_left")
                .auto_shrink([false, false])
                .show(&mut l_ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                    ui.set_width(left_w);
                    draw_left(ui, &full, &tone, &palette, left_w, radius, &mut action);
                });

            // -- Columna derecha --
            let mut r_ui = ui.new_child(
                UiBuilder::new().max_rect(r_rect).layout(Layout::top_down(Align::Min)),
            );
            draw_right(&mut r_ui, &full, &tone, &palette, tab, r_rect.width(), &mut action);

            // Frame: `back` debajo de la tarjeta, `front` encima.
            if let Some(frame) = &full.base.frame {
                let (back, front) = frame_shapes(ui.ctx(), frame, l_rect);
                wide.set(back_idx, Shape::Vec(back));
                wide.extend(front);
            }

            // Cerrar.
            let close = round_button(
                ui,
                Rect::from_center_size(pos2(modal.right() - 38.0, modal.top() + 38.0), Vec2::splat(36.0)),
                Icon::X,
                "Cerrar",
                Id::new("profile_full_close"),
            );
            if close.clicked() {
                action = Some(FullAction::Close);
            }
        });

    // El efecto va por encima de la tarjeta, recortado al modal.
    let effect_key = format!("full_{user_id}");
    if let Some(effect) = &full.base.effect {
        draw_effect(&ctx, area.response.layer_id, l_rect, effect, &effect_key, modal);
    }

    if action.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = Some(FullAction::Close);
    }
    let close_full = |app: &mut App| {
        // Que el efecto arranque de cero la próxima vez.
        ctx.memory_mut(|m| m.data.remove::<f64>(effect_start_id(&effect_key)));
        app.profile_full = None;
    };
    match action {
        Some(FullAction::Close) => close_full(app),
        Some(FullAction::Tab(i)) => {
            if let Some(state) = &mut app.profile_full {
                state.tab = i;
            }
        }
        Some(FullAction::CopyId) => ctx.copy_text(user_id.clone()),
        Some(FullAction::EditProfile) => {
            app.settings_tab = crate::ui::settings::SettingsTab::Account;
            app.settings_open = true;
            close_full(app);
        }
        Some(FullAction::Message) => {
            app.message_user_from_profile(
                &user_id,
                &full.base.display_name,
                full.base.avatar_url.clone(),
                full.base.avatar_color,
                None,
            );
            close_full(app);
        }
        None => {}
    }
}

// ---------------------------------------------------------------------
// Columna izquierda
// ---------------------------------------------------------------------

fn draw_left(
    ui: &mut egui::Ui,
    full: &Full,
    tone: &Tone,
    palette: &Palette,
    w: f32,
    radius: u8,
    action: &mut Option<FullAction>,
) {
    let v = &full.base;

    // -- Banner y avatar --
    let banner = Rect::from_min_size(ui.cursor().min, vec2(w, FULL_BANNER_H));
    paint_banner(ui, v, palette, banner, radius);
    ui.allocate_rect(banner, Sense::hover());
    let center = pos2(banner.left() + FULL_PAD + FULL_AVATAR_R + 6.0, banner.bottom());
    ui.painter().circle_filled(center, FULL_AVATAR_R + 8.0, tone.top);
    let initial = v.display_name.chars().next().unwrap_or('?').to_uppercase().to_string();
    extra::avatar(ui, center, FULL_AVATAR_R, v.avatar_url.as_deref(), v.avatar_color, &initial, palette);
    if let Some(url) = v.decoration_url.as_ref().filter(|_| !anim::hide_profile_images()) {
        let size = Vec2::splat(FULL_AVATAR_R * 2.0 * DECORATION_SCALE);
        let source = anim::source_animated(ui.ctx(), url, 160, None);
        egui::Image::new(source)
            .show_loading_spinner(false)
            .paint_at(ui, Rect::from_center_size(center, size));
    }
    if let Some(status) = v.status {
        paint_status_badge(ui, center, FULL_AVATAR_R, status, tone);
    }
    ui.add_space(FULL_AVATAR_R + 14.0);

    Frame::new().inner_margin(Margin::symmetric(FULL_PAD as i8, 0)).show(ui, |ui| {
        ui.set_width(w - FULL_PAD * 2.0);

        if v.loading {
            ui.horizontal(|ui| {
                theme::spinner(ui, 14.0, tone.dim);
                theme::text(ui, "Cargando perfil...", theme::regular(13.0), tone.dim);
            });
            ui.add_space(8.0);
        }

        // -- Nombre, usuario, pronombres, etiqueta e insignias --
        let name_color = v
            .name_color
            .map(|c| readable_on(c, tone.top, tone.text, 4.5))
            .unwrap_or(tone.text);
        theme::text(ui, &v.display_name, theme::bold(28.0), name_color);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            if let Some(username) = &v.username_line {
                theme::text(ui, username, theme::regular(15.0), tone.text);
            }
            if let Some(pronouns) = &v.pronouns {
                if v.username_line.is_some() {
                    theme::text(ui, "•", theme::regular(15.0), tone.dim);
                }
                theme::text(ui, pronouns, theme::regular(15.0), tone.text);
            }
            if let Some(clan) = &v.clan {
                clan_chip(ui, tone, clan);
            }
            if !v.badges.is_empty() {
                badges_pill(ui, tone, &v.badges);
            }
        });

        // -- Botones --
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if v.is_me {
                if solid_button(ui, tone, "Editar perfil", 130.0).clicked() {
                    *action = Some(FullAction::EditProfile);
                }
            } else if solid_button(ui, tone, "Mensaje", 110.0).clicked() {
                *action = Some(FullAction::Message);
            }
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(38.0), Sense::click());
            let fill = if response.hovered() { tone.stroke } else { tone.panel };
            ui.painter().rect_filled(rect, 8.0, fill);
            theme::paint_icon(ui, Icon::Ellipsis, rect, 16.0, tone.text);
            let response = response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Copiar ID de usuario");
            if response.clicked() {
                *action = Some(FullAction::CopyId);
            }
        });

        // -- Bio --
        if let Some(bio) = &v.bio {
            ui.add_space(18.0);
            markdown::show(ui, palette, bio, 14.5, tone.text, &MentionCtx::none());
        }

        // -- Miembro desde --
        if let Some(since) = &v.member_since {
            ui.add_space(18.0);
            theme::text(ui, "Miembro desde", theme::semibold(13.5), tone.text);
            ui.add_space(2.0);
            theme::text(ui, since, theme::regular(15.0), tone.text);
        }

        // -- Roles --
        if !v.roles.is_empty() {
            ui.add_space(18.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for role in &v.roles {
                    role_chip(ui, tone, role);
                }
            });
        }

        // -- Conexiones --
        if !v.connections.is_empty() {
            ui.add_space(18.0);
            panel(ui, tone, |ui| {
                theme::text(ui, "CONEXIONES", theme::semibold(10.5), tone.dim);
                ui.add_space(4.0);
                for connection in &v.connections {
                    ui.horizontal(|ui| {
                        theme::icon(ui, Icon::Globe, 14.0, tone.dim);
                        theme::text(ui, connection.name.clone(), theme::medium(13.0), tone.text);
                        theme::text(ui, format!("· {}", connection.kind), theme::regular(12.0), tone.dim);
                        if connection.verified {
                            theme::icon(ui, Icon::BadgeCheck, 13.0, STATUS_ONLINE);
                        }
                    });
                }
            });
        }
    });
    ui.add_space(FULL_PAD);
}

/// Botón relleno con el color de acento del perfil.
fn solid_button(ui: &mut egui::Ui, tone: &Tone, label: &str, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 38.0), Sense::click());
    let fill = if response.hovered() {
        extra::blend(tone.accent, tone.on_accent, 0.12)
    } else {
        tone.accent
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        theme::semibold(14.0),
        tone.on_accent,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------
// Columna derecha
// ---------------------------------------------------------------------

fn draw_right(
    ui: &mut egui::Ui,
    full: &Full,
    tone: &Tone,
    palette: &Palette,
    tab: usize,
    width: f32,
    action: &mut Option<FullAction>,
) {
    let v = &full.base;
    let plural = |n: usize, one: &str, many: &str| if n == 1 { one.to_string() } else { many.to_string() };
    let friends_label = match v.mutual_friends {
        Some(0) => "Sin amigos en común".to_string(),
        Some(n) => format!("{n} {}", plural(n, "amigo en común", "amigos en común")),
        None => "Amigos en común".to_string(),
    };
    let guilds_label = match v.mutual_guilds {
        Some(0) => "Sin servidores en común".to_string(),
        Some(n) => format!("{n} {}", plural(n, "servidor en común", "servidores en común")),
        None => "Servidores en común".to_string(),
    };
    let labels = ["Actividad".to_string(), friends_label, guilds_label];

    ui.spacing_mut().item_spacing = vec2(22.0, 8.0);
    ui.horizontal_wrapped(|ui| {
        for (i, label) in labels.iter().enumerate() {
            if tab_button(ui, tone, label, tab == i).clicked() {
                *action = Some(FullAction::Tab(i));
            }
        }
    });
    ui.add_space(14.0);

    let inner_w = (width - 14.0).max(120.0);
    egui::ScrollArea::vertical()
        .id_salt(("profile_full_right", tab))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(inner_w);
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            match tab {
                0 => activity_tab(ui, full, tone, inner_w),
                1 => friends_tab(ui, full, tone, palette, inner_w),
                _ => guilds_tab(ui, full, tone, inner_w),
            }
        });
}

/// Pestaña con subrayado cuando está seleccionada.
fn tab_button(ui: &mut egui::Ui, tone: &Tone, label: &str, selected: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), theme::semibold(15.0), tone.text);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x, 30.0), Sense::click());
    let color = if selected || response.hovered() { tone.text } else { tone.dim };
    ui.painter().galley(pos2(rect.left(), rect.top() + 2.0), galley, color);
    if selected {
        ui.painter().rect_filled(
            Rect::from_min_size(pos2(rect.left(), rect.bottom() - 2.0), vec2(rect.width(), 2.0)),
            1.0,
            tone.text,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Fila/tarjeta con el fondo translúcido de las secciones.
fn card<R>(ui: &mut egui::Ui, tone: &Tone, width: f32, margin: i8, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    Frame::new()
        .fill(tone.panel)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(margin))
        .show(ui, |ui| {
            ui.set_width(width - f32::from(margin) * 2.0);
            add(ui)
        })
        .inner
}

fn empty_note(ui: &mut egui::Ui, tone: &Tone, text: &str) {
    theme::text(ui, text, theme::regular(14.0), tone.dim);
}

fn activity_tab(ui: &mut egui::Ui, full: &Full, tone: &Tone, width: f32) {
    theme::text(ui, "Actividad reciente", theme::semibold(13.0), tone.text);
    ui.add_space(2.0);
    if full.activities.is_empty() {
        empty_note(ui, tone, "No está haciendo nada en este momento.");
        return;
    }
    for activity in &full.activities {
        card(ui, tone, width, 16, |ui| activity_block(ui, tone, activity));
    }
}

fn friends_tab(ui: &mut egui::Ui, full: &Full, tone: &Tone, palette: &Palette, width: f32) {
    if full.friends.is_empty() {
        let text = match full.base.mutual_friends {
            Some(n) if n > 0 => "Discord no compartió la lista de amigos en común.",
            _ => "Sin amigos en común.",
        };
        empty_note(ui, tone, text);
        return;
    }
    let placeholder = extra::blend(tone.dim, tone.bottom, 0.6);
    for friend in &full.friends {
        card(ui, tone, width, 10, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                let initial = friend.name.chars().next().unwrap_or('?').to_uppercase().to_string();
                extra::avatar(ui, rect.center(), 20.0, friend.avatar_url.as_deref(), placeholder, &initial, palette);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    theme::text(ui, &friend.name, theme::semibold(14.5), tone.text);
                    theme::text(ui, &friend.handle, theme::regular(12.5), tone.dim);
                });
            });
        });
    }
}

fn guilds_tab(ui: &mut egui::Ui, full: &Full, tone: &Tone, width: f32) {
    if full.guilds.is_empty() {
        let text = match full.base.mutual_guilds {
            Some(n) if n > 0 => "No se pudieron leer los servidores en común.",
            _ => "Sin servidores en común.",
        };
        empty_note(ui, tone, text);
        return;
    }
    for guild in &full.guilds {
        card(ui, tone, width, 10, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                let corner = CornerRadius::same(12);
                match &guild.icon_url {
                    Some(url) => {
                        ui.painter().rect_filled(rect, corner, guild.color);
                        egui::Image::new(anim::plain(url))
                            .corner_radius(corner)
                            .show_loading_spinner(false)
                            .paint_at(ui, rect);
                    }
                    None => {
                        ui.painter().rect_filled(rect, corner, guild.color);
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            &guild.initial,
                            theme::bold(16.0),
                            Color32::WHITE,
                        );
                    }
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    theme::text(ui, &guild.name, theme::semibold(14.5), tone.text);
                    if let Some(nick) = &guild.nick {
                        theme::text(ui, format!("Apodo: {nick}"), theme::regular(12.5), tone.dim);
                    }
                });
            });
        });
    }
}
