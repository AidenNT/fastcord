use egui::{CornerRadius, Frame, Margin, ScrollArea, Vec2, Color32};

use crate::lib::data::{Friend, Status};
use crate::discord::models::PrivateChannel;
use crate::lib::state::{App, Screen};
use crate::ui::extra;
use crate::ui::theme::{self, Icon, Palette};

/// Ancho fijo que ocupa este contenido dentro de `ui::nav`.
pub const WIDTH: f32 = 260.0;

/// Contenido del panel de amigos/DMs: buscador + navegación + lista de
/// conversaciones. Antes era un `egui::Panel::left` propio con su propia
/// barra de usuario abajo; ahora se dibuja como una columna dentro del
/// panel de navegación combinado (`ui::nav`), que es quien pone la barra
/// de usuario abajo ocupando el ancho de esta columna MÁS el rail de
/// servidores de al lado.
pub fn content(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    // `Screen::Dm(i)` indexa `app.friends`, no `app.dms` (son dos listas
    // distintas — ver `App::open_dm_from_channel`), así que para saber
    // cuál fila de la lista de DMs de acá abajo pintar como "seleccionada"
    // hay que ir por `user_id`, no comparar el índice directo.
    let selected_dm = match app.screen {
        Screen::Dm(i) => app.friends.get(i).and_then(|f| {
            app.dms
                .iter()
                .position(|dm| dm.recipients.first().map(|r| r.id.as_str()) == Some(f.user_id.as_str()))
        }),
        _ => None,
    };
    let on_home = matches!(app.screen, Screen::Home);

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.add_sized(
            [ui.available_width() - 8.0, 32.0],
            egui::TextEdit::singleline(&mut app.friends_search)
                .hint_text("Busca o inicia una conversación"),
        );
    });
    ui.add_space(10.0);

    ScrollArea::vertical()
        .id_salt("nav_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if nav_row(ui, &palette, Icon::Users, "Amigos", Some(app.friends.iter().filter(|f| f.is_friend).count()), on_home) {
                app.go_home();
            }
            // TODO: no hay ícono de "sobre/inbox" en el set de Lucide
            // que trae tu theme; CircleAlert es el más parecido.
            nav_row(ui, &palette, Icon::CircleAlert, "Solicitudes de mensajes", Some(8), false);
            nav_row(ui, &palette, Icon::Sparkles, "Inicio de Nitro", None, false);
            // TODO: no hay ícono de "tienda"; Bookmark como placeholder.
            nav_row(ui, &palette, Icon::Bookmark, "Tienda", None, false);
            nav_row(ui, &palette, Icon::Compass, "Misiones", None, false);

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                theme::text(ui, "MENSAJES DIRECTOS", theme::semibold(11.0), palette.dim);
            });
            ui.add_space(4.0);

            let mut clicked_dm = None;
            for (i, dm) in app.dms.iter().enumerate() {
                let mentions = app.unread_mentions.get(&dm.id).copied().unwrap_or(0);
                let (status, activity) = app.dm_presence(dm);
                if dm_row_compact(ui, &palette, dm, selected_dm == Some(i), mentions, status, activity.as_deref()) {
                    clicked_dm = Some(i);
                }
            }
            if let Some(i) = clicked_dm {
                app.open_dm_from_channel(i);
            }
        });
}

/// Devuelve `true` si se hizo click en la fila.
fn nav_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Icon,
    label: &str,
    badge: Option<usize>,
    active: bool,
) -> bool {
    let desired = Vec2::new(ui.available_width() - 12.0, 34.0);
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
        let bg = if active || resp.hovered() {
            palette.surface_hover
        } else {
            palette.window
        };
        ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS_SMALL + 2), bg);

        let text_color = if active { palette.text } else { palette.secondary };
        theme::paint_icon(
            ui,
            icon,
            egui::Rect::from_center_size(rect.left_center() + Vec2::new(16.0, 0.0), Vec2::splat(16.0)),
            15.0,
            text_color,
        );
        ui.painter().text(
            rect.left_center() + Vec2::new(38.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            theme::medium(14.0),
            text_color,
        );
        if let Some(n) = badge {
            ui.painter().circle_filled(
                rect.right_center() - Vec2::new(16.0, 0.0),
                9.0,
                palette.danger,
            );
            ui.painter().text(
                rect.right_center() - Vec2::new(16.0, 0.0),
                egui::Align2::CENTER_CENTER,
                n.to_string(),
                theme::semibold(10.5),
                palette.on_accent,
            );
        }
        clicked = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    });
    clicked
}

/// Devuelve `true` si se hizo click en la fila.
fn friend_row_compact(ui: &mut egui::Ui, palette: &Palette, friend: &Friend, selected: bool) -> bool {
    let desired = Vec2::new(ui.available_width() - 12.0, 46.0);
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
        if selected || resp.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(theme::RADIUS_SMALL + 2), palette.surface_hover);
        }
        let avatar_center = rect.left_center() + Vec2::new(18.0, 0.0);
        extra::avatar(ui, avatar_center, 16.0, friend.avatar_url.as_deref(), friend.avatar_color, &friend.initial(), palette);
        extra::status_dot(
            ui,
            avatar_center,
            16.0,
            extra::status_color(friend.status, palette),
            palette.window,
        );

        let text_x = 44.0;
        crate::ui::emoji::paint_line_top(
            ui,
            rect.left_top() + Vec2::new(text_x, 10.0),
            &friend.name,
            theme::medium(13.5),
            palette.text,
            (rect.width() - text_x - 8.0).max(0.0),
        );
        let subtitle = friend
            .subtitle
            .clone()
            .unwrap_or_else(|| friend.status.label().to_string());
        ui.painter().text(
            rect.left_top() + Vec2::new(text_x, 26.0),
            egui::Align2::LEFT_TOP,
            truncate(&subtitle, 26),
            theme::regular(11.0),
            palette.dim,
        );
        clicked = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    });
    clicked
}

fn dm_row_compact(
    ui: &mut egui::Ui,
    palette: &Palette,
    dm: &PrivateChannel,
    selected: bool,
    mentions: u32,
    status: Status,
    activity: Option<&str>,
) -> bool {
    let desired = Vec2::new(ui.available_width() - 12.0, 46.0);

    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
        if selected || resp.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(theme::RADIUS_SMALL + 2), palette.surface_hover);
        }
        let avatar_center = rect.left_center() + Vec2::new(18.0, 0.0);
        extra::avatar(ui, avatar_center, 16.0, dm.avatar_url.as_deref(), Color32::from_rgb(90, 170, 210), &dm_initial(&dm.username), palette);
        extra::status_dot(
            ui,
            avatar_center,
            16.0,
            extra::status_color(status, palette),
            palette.window,
        );

        let text_x = 44.0;
        crate::ui::emoji::paint_line_top(
            ui,
            rect.left_top() + Vec2::new(text_x, 10.0),
            &dm.username,
            theme::medium(13.5),
            palette.text,
            // Con menciones sin leer, deja lugar al badge de la derecha.
            (rect.width() - text_x - if mentions > 0 { 40.0 } else { 8.0 }).max(0.0),
        );
        // Estado real (`App::dm_presence`): la actividad o estado
        // personalizado si hay, y si no la etiqueta del estado.
        let subtitle = activity.unwrap_or_else(|| status.label());
        let subtitle = crate::discord::models::one_line(subtitle);
        let subtitle = subtitle.as_str();
        ui.painter().text(
            rect.left_top() + Vec2::new(text_x, 26.0),
            egui::Align2::LEFT_TOP,
            truncate(subtitle, 26),
            theme::regular(11.0),
            palette.dim,
        );
        // Contador de menciones sin leer (todo mensaje de un DM cuenta).
        let ring = if selected || resp.hovered() { palette.surface_hover } else { palette.window };
        crate::ui::notifications::paint_badge(
            ui.painter(),
            rect.right_center() - Vec2::new(20.0, 0.0),
            mentions,
            palette,
            ring,
        );
        clicked = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    });
    clicked
}

pub(crate) fn mini_player(ui: &mut egui::Ui, palette: &Palette) {
    Frame::new()
        .fill(extra::blend(palette.window, palette.accent, 0.06))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::text(ui, "Promises, Promises", theme::semibold(12.5), palette.text);
            theme::text(ui, "Naked Eyes", theme::regular(11.0), palette.dim);
            ui.add_space(4.0);
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 4.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, palette.outline);
            let filled =
                egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * 0.4, rect.height()));
            ui.painter().rect_filled(filled, 2.0, palette.accent);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for icon in [Icon::Shuffle, Icon::SkipBack, Icon::PauseFilled, Icon::SkipForward, Icon::Repeat] {
                    theme::icon(ui, icon, 14.0, palette.dim);
                    ui.add_space(4.0);
                }
            });
        });
}

pub(crate) fn user_bar(ui: &mut egui::Ui, app: &mut App) {
    let palette = app.palette;
    let self_mute = app.voice_target.as_ref().is_some_and(|t| t.self_mute);
    let self_deaf = app.voice_target.as_ref().is_some_and(|t| t.self_deaf);

    let mut settings_clicked = false;
    let mut deafen_clicked = false;
    let mut mute_clicked = false;

    Frame::new()
        .fill(palette.surface)
        .inner_margin(Margin::symmetric(8, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::hover());
                let (me_name, me_avatar_url) = match app.me.as_ref() {
                    Some(me) => (me.display_name().to_string(), me.avatar_url()),
                    None => ("Aiden".to_string(), None),
                };
                extra::avatar(ui, rect.center(), 16.0, me_avatar_url.as_deref(), palette.accent, "A", &palette);
                ui.vertical(|ui| {
                    theme::text(ui, &me_name, theme::semibold(12.5), palette.text);
                    theme::text(ui, "En línea", theme::regular(10.5), palette.dim);
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // El panel de nav pone `item_spacing = ZERO` para el
                    // resto de su contenido (ver `ui::nav::show`); sin
                    // restaurarlo acá estos tres botones quedan pegados
                    // uno con otro.
                    ui.spacing_mut().item_spacing.x = 4.0;

                    if theme::icon_button(ui, Icon::Settings, 14.0, palette.dim, palette.text, "Ajustes")
                        .clicked()
                    {
                        settings_clicked = true;
                    }

                    let deafen_icon = if self_deaf { Icon::VolumeX } else { Icon::Headphones };
                    let deafen_color = if self_deaf { palette.danger } else { palette.dim };
                    let deafen_tooltip = if self_deaf { "Dejar de ensordecer" } else { "Ensordecer" };
                    if theme::icon_button(ui, deafen_icon, 14.0, deafen_color, palette.text, deafen_tooltip)
                        .clicked()
                    {
                        deafen_clicked = true;
                    }

                    let mute_icon = if self_mute { Icon::MicOff } else { Icon::Mic };
                    let mute_color = if self_mute { palette.danger } else { palette.dim };
                    let mute_tooltip = if self_mute { "Dejar de silenciar" } else { "Silenciar" };
                    if theme::icon_button(ui, mute_icon, 14.0, mute_color, palette.text, mute_tooltip)
                        .clicked()
                    {
                        mute_clicked = true;
                    }
                });
            });
        });

    // Mismo patrón que `ui::call_bar`: aplicar después de dibujar, ya que
    // arriba tomamos `palette`/`self_mute`/`self_deaf` prestados de `app`.
    if settings_clicked {
        app.settings_open = true;
    } else if deafen_clicked {
        app.toggle_self_deafen();
    } else if mute_clicked {
        app.toggle_self_mute();
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Inicial para el círculo placeholder de un DM (mismo criterio que
/// `Friend::initial`/`server::member_initial`). Pasarle el `username`
/// completo a `extra::avatar` en vez de esto pintaba el string entero
/// centrado en el avatar — con nombres largos como
/// `deleted_user_390bbbae089d`, eso se salía del panel entero.
fn dm_initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}