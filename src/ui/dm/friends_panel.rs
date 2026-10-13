use egui::{Color32, CornerRadius, Frame, Margin, ScrollArea, Vec2};

use crate::discord::models::PrivateChannel;
use crate::lib::data::{Friend, Status};
use crate::lib::state::{App, Screen};
use crate::ui::shell::extra;
use crate::ui::theme::{self, Icon, Palette};

/// Ancho fijo que ocupa este contenido dentro de `ui::shell::nav`.
pub const WIDTH: f32 = 260.0;

/// Ancho de la columna de DMs en la interfaz nueva (ver `ui::shell::nav::show_modern`).
pub const MODERN_WIDTH: f32 = 288.0;

/// Contenido del panel de amigos/DMs: buscador + navegación + lista de
/// conversaciones. Antes era un `egui::Panel::left` propio con su propia
/// barra de usuario abajo; ahora se dibuja como una columna dentro del
/// panel de navegación combinado (`ui::shell::nav`), que es quien pone la barra
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

    let modern = theme::is_modern();
    if modern {
        // Interfaz nueva: el buscador vive en la píldora de arriba
        // (`ui::shell::nav::show_modern`) y filtra esta lista.
        ui.add_space(12.0);
    } else {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.add_sized(
                [ui.available_width() - 8.0, 32.0],
                egui::TextEdit::singleline(&mut app.friends_search).hint_text("Busca o inicia una conversación"),
            );
        });
        ui.add_space(10.0);
    }
    let dm_filter = if modern {
        app.top_search.trim().to_lowercase()
    } else {
        String::new()
    };

    ScrollArea::vertical()
        .id_salt("nav_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if nav_row(
                ui,
                &palette,
                Icon::Users,
                "Amigos",
                Some(app.friends.iter().filter(|f| f.is_friend).count()),
                on_home,
            ) {
                app.go_home();
            }
            // TODO: no hay ícono de "sobre/inbox" en el set de Lucide
            // que trae tu theme; CircleAlert es el más parecido.
            nav_row(
                ui,
                &palette,
                Icon::CircleAlert,
                "Solicitudes de mensajes",
                Some(8),
                false,
            );
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
                if !dm_filter.is_empty() && !dm.username.to_lowercase().contains(&dm_filter) {
                    continue;
                }
                let mentions = app.unread_mentions.get(&dm.id).copied().unwrap_or(0);
                let (status, activity) = app.dm_presence(dm);
                if dm_row_compact(
                    ui,
                    &palette,
                    dm,
                    selected_dm == Some(i),
                    mentions,
                    status,
                    activity.as_deref(),
                    app.dm_call_active(&dm.id),
                ) {
                    clicked_dm = Some(i);
                }
            }
            if let Some(i) = clicked_dm {
                app.open_dm_from_channel(i);
            }
        });
}

/// Devuelve `true` si se hizo click en la fila.
fn nav_row(ui: &mut egui::Ui, palette: &Palette, icon: Icon, label: &str, badge: Option<usize>, active: bool) -> bool {
    let modern = theme::is_modern();
    let desired = Vec2::new(ui.available_width() - 12.0, if modern { 40.0 } else { 34.0 });
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
        let bg = theme::row_fill(palette, active, resp.hovered());
        let row_radius = if modern { 10 } else { theme::radius_small() + 2 };
        ui.painter().rect_filled(rect, CornerRadius::same(row_radius), bg);

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
            ui.painter()
                .circle_filled(rect.right_center() - Vec2::new(16.0, 0.0), 9.0, palette.danger);
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

fn dm_row_compact(
    ui: &mut egui::Ui,
    palette: &Palette,
    dm: &PrivateChannel,
    selected: bool,
    mentions: u32,
    status: Status,
    activity: Option<&str>,
    in_call: bool,
) -> bool {
    let modern = theme::is_modern();
    let desired = Vec2::new(ui.available_width() - 12.0, if modern { 52.0 } else { 46.0 });
    let (av_x, av_r, text_x, name_y, sub_y) = if modern {
        (24.0, 18.0, 54.0, 11.0, 30.0)
    } else {
        (18.0, 16.0, 44.0, 10.0, 26.0)
    };

    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
        if selected || resp.hovered() {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(if modern { 10 } else { theme::radius_small() + 2 }),
                theme::row_fill(palette, selected, resp.hovered()),
            );
        }
        let avatar_center = rect.left_center() + Vec2::new(av_x, 0.0);
        extra::avatar(
            ui,
            avatar_center,
            av_r,
            dm.avatar_url.as_deref(),
            Color32::from_rgb(90, 170, 210),
            &dm_initial(&dm.username),
            palette,
        );
        // Un grupo no tiene "estado": sin puntito.
        if !dm.is_group() {
            extra::status_dot(
                ui,
                avatar_center,
                av_r,
                extra::status_color(status, palette),
                theme::row_bg(palette),
            );
        }

        crate::ui::chat::emoji::paint_line_top(
            ui,
            rect.left_top() + Vec2::new(text_x, name_y),
            &dm.username,
            theme::medium(13.5),
            palette.text,
            // Con menciones sin leer, deja lugar al badge de la derecha.
            (rect.width() - text_x - if mentions > 0 { 40.0 } else { 8.0 }).max(0.0),
        );
        // Estado real (`App::dm_presence`): la actividad o estado
        // personalizado si hay, y si no la etiqueta del estado.
        // En un grupo, la cantidad de miembros (`recipients` no te incluye).
        let members_label = format!("{} miembros", dm.recipients.len() + 1);
        let subtitle = if dm.is_group() {
            members_label.as_str()
        } else {
            activity.unwrap_or_else(|| status.label())
        };
        let subtitle = crate::discord::models::one_line(subtitle);
        let subtitle = subtitle.as_str();
        if in_call {
            // Con una llamada en curso en ese DM/grupo: teléfono + "En llamada"
            // en lugar del estado, así se ve desde afuera del chat.
            let green = Color32::from_rgb(0x3b, 0xa5, 0x5d);
            let icon_size = 12.0;
            let icon_rect = egui::Rect::from_min_size(
                rect.left_top() + Vec2::new(text_x, sub_y + 1.0),
                Vec2::splat(icon_size),
            );
            theme::paint_icon(ui, Icon::Phone, icon_rect, icon_size, green);
            ui.painter().text(
                rect.left_top() + Vec2::new(text_x + icon_size + 4.0, sub_y),
                egui::Align2::LEFT_TOP,
                "En llamada",
                theme::medium(11.0),
                green,
            );
        } else {
            ui.painter().text(
                rect.left_top() + Vec2::new(text_x, sub_y),
                egui::Align2::LEFT_TOP,
                truncate(subtitle, 26),
                theme::regular(11.0),
                palette.dim,
            );
        }
        // Contador de menciones sin leer (todo mensaje de un DM cuenta).
        let ring = theme::row_fill(palette, selected, resp.hovered());
        crate::ui::shell::notifications::paint_badge(
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

pub(crate) fn user_bar(ui: &mut egui::Ui, app: &mut App) {
    let palette = app.palette;
    let self_mute = app.voice_target.as_ref().map_or(app.self_mute, |t| t.self_mute);
    let self_deaf = app.voice_target.as_ref().map_or(app.self_deaf, |t| t.self_deaf);

    let mut settings_clicked = false;
    let mut deafen_clicked = false;
    let mut mute_clicked = false;

    // Margen vertical para que la tarjeta llene TODA la barra del `nav`
    // (`FOOTER_HEIGHT` menos 2px de borde): fila de 32px (el avatar) + 2 × pad.
    // Así el contenido queda centrado y no sobra una franja vacía abajo.
    let modern = theme::is_modern();
    let footer_inner = if modern {
        crate::ui::shell::nav::FOOTER_HEIGHT - 12.0
    } else {
        crate::ui::shell::nav::FOOTER_HEIGHT - 2.0
    };
    // La tarjeta nueva lleva borde de 1px (2px de alto): se descuentan.
    let border = if modern { 2.0 } else { 0.0 };
    let pad_y = ((footer_inner - 32.0 - border) / 2.0).round() as i8;
    let bar_frame = Frame::new()
        .fill(palette.surface)
        .inner_margin(Margin::symmetric(8, pad_y));
    let bar_frame = if modern {
        bar_frame
            .stroke(egui::Stroke::new(
                1.0,
                extra::blend(palette.outline, palette.accent, 0.35),
            ))
            .corner_radius(egui::CornerRadius::same(22))
    } else {
        bar_frame
    };
    bar_frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            // `nav::show` pone `item_spacing = ZERO` para todo su
            // contenido: sin esto el nombre queda pegado al avatar.
            ui.spacing_mut().item_spacing.x = 10.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::hover());
            let (me_name, me_avatar_url) = match app.me.as_ref() {
                Some(me) => (me.display_name().to_string(), me.avatar_url()),
                None => ("Aiden".to_string(), None),
            };
            extra::avatar(
                ui,
                rect.center(),
                16.0,
                me_avatar_url.as_deref(),
                palette.accent,
                "A",
                &palette,
            );
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                theme::text(ui, &me_name, theme::semibold(12.5), palette.text);
                theme::text(ui, "En línea", theme::regular(10.5), palette.dim);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // El panel de nav pone `item_spacing = ZERO` para el
                // resto de su contenido (ver `ui::shell::nav::show`); sin
                // restaurarlo acá estos tres botones quedan pegados
                // uno con otro.
                ui.spacing_mut().item_spacing.x = 4.0;

                if theme::icon_button(ui, Icon::Settings, 14.0, palette.dim, palette.text, "Ajustes").clicked() {
                    settings_clicked = true;
                }

                let deafen_icon = if self_deaf { Icon::VolumeX } else { Icon::Headphones };
                let deafen_color = if self_deaf { palette.danger } else { palette.dim };
                let deafen_tooltip = if self_deaf { "Dejar de ensordecer" } else { "Ensordecer" };
                if theme::icon_button(ui, deafen_icon, 14.0, deafen_color, palette.text, deafen_tooltip).clicked() {
                    deafen_clicked = true;
                }

                let mute_icon = if self_mute { Icon::MicOff } else { Icon::Mic };
                let mute_color = if self_mute { palette.danger } else { palette.dim };
                let mute_tooltip = if self_mute { "Dejar de silenciar" } else { "Silenciar" };
                if theme::icon_button(ui, mute_icon, 14.0, mute_color, palette.text, mute_tooltip).clicked() {
                    mute_clicked = true;
                }
            });
        });
    });

    // Mismo patrón que `ui::call::call_bar`: aplicar después de dibujar, ya que
    // arriba tomamos `palette`/`self_mute`/`self_deaf` prestados de `app`.
    if settings_clicked {
        app.settings_open = true;
    } else if deafen_clicked {
        app.toggle_self_deafen();
    } else if mute_clicked {
        app.toggle_self_mute();
    }
}

/// Tarjeta "escuchando / jugando" de la propia cuenta (primera actividad de
/// `App::own_activities`), como la de arriba de la barra de usuario en la
/// referencia. Devuelve `false` (y no dibuja nada) si no hay actividad.
pub(crate) fn now_playing_card(ui: &mut egui::Ui, app: &App) -> bool {
    let palette = app.palette;
    let Some(activity) = app.own_activity() else {
        return false;
    };
    let title = activity
        .details
        .as_deref()
        .filter(|d| !d.trim().is_empty())
        .unwrap_or(&activity.name)
        .to_string();
    let subtitle = activity
        .state
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            if activity.is_spotify() {
                s.replace(';', ",")
            } else {
                s.to_string()
            }
        })
        .unwrap_or_default();

    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::symmetric(10, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.horizontal(|ui| {
                let art = 40.0;
                let (art_rect, _) = ui.allocate_exact_size(Vec2::splat(art), egui::Sense::hover());
                let mut drew_image = false;
                if let Some(url) = activity.image_url() {
                    let image = egui::Image::new(crate::ui::chat::anim::plain(&url))
                        .corner_radius(CornerRadius::same(8))
                        .fit_to_exact_size(Vec2::splat(art))
                        .show_loading_spinner(false);
                    if let Ok(egui::load::TexturePoll::Ready { .. }) = image.load_for_size(ui.ctx(), Vec2::splat(art)) {
                        ui.put(art_rect, image);
                        drew_image = true;
                    }
                }
                if !drew_image {
                    ui.painter()
                        .rect_filled(art_rect, CornerRadius::same(8), palette.surface_hover);
                    theme::paint_icon(ui, Icon::Music, art_rect, 18.0, palette.secondary);
                }
                ui.add_space(10.0);
                let text_w = (ui.available_width() - 2.0).max(0.0);
                let (col, _) = ui.allocate_exact_size(Vec2::new(text_w, art), egui::Sense::hover());
                crate::ui::home::clipped_line(
                    ui,
                    egui::Rect::from_min_size(col.min + Vec2::new(0.0, 4.0), Vec2::new(text_w, 15.0)),
                    &title,
                    theme::semibold(13.0),
                    palette.text,
                );
                crate::ui::home::clipped_line(
                    ui,
                    egui::Rect::from_min_size(col.min + Vec2::new(0.0, 21.0), Vec2::new(text_w, 14.0)),
                    &subtitle,
                    theme::regular(11.0),
                    palette.dim,
                );
            });
        });
    true
}

/// Tarjeta de usuario de la interfaz nueva: avatar con punto de presencia,
/// nombre y estado. Los botones de silenciar / ensordecer / ajustes viven
/// ahora en el rail (`ui::shell::rail::content_modern`).
pub(crate) fn user_card(ui: &mut egui::Ui, app: &App) {
    let palette = app.palette;
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(38.0), egui::Sense::hover());
                let (me_name, me_avatar_url) = match app.me.as_ref() {
                    Some(me) => (me.display_name().to_string(), me.avatar_url()),
                    None => ("Aiden".to_string(), None),
                };
                extra::avatar(
                    ui,
                    rect.center(),
                    19.0,
                    me_avatar_url.as_deref(),
                    palette.accent,
                    "A",
                    &palette,
                );
                extra::status_dot(
                    ui,
                    rect.center(),
                    19.0,
                    extra::status_color(Status::Online, &palette),
                    palette.surface,
                );
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.add_space(3.0);
                    theme::text(ui, &me_name, theme::semibold(14.0), palette.text);
                    ui.add_space(1.0);
                    theme::text(ui, "En línea", theme::regular(11.5), palette.dim);
                });
            });
        });
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
