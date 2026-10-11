pub mod friends_panel;
pub mod inbox;

use egui::{Frame, Margin};

use crate::lib::data::Status;
use crate::lib::state::{App, Screen};
use crate::ui::chat;
use crate::ui::shell::extra;
use crate::ui::shell::nav;
use crate::ui::theme::{self, Icon, Palette};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let Screen::Dm(index) = app.screen else {
        return;
    };
    if index >= app.friends.len() {
        // imprimir en consola el índice del amigo
        app.go_home();
        return;
    }

    // Rail de servidores + panel de amigos/DMs + barra de usuario
    // combinada; ver `ui::shell::nav`.
    nav::show(app, ui);
    profile_panel(app, ui, index);
    central_panel(app, ui, index);
}

/// Panel derecho con la tarjeta de perfil del contacto, como en Discord.
/// Con sesión real muestra el perfil completo (banner, decoración, frame,
/// efecto, insignias, bio...); en las pantallas demo, la versión simple.
fn profile_panel(app: &mut App, ui: &mut egui::Ui, index: usize) {
    // DM grupal: no hay un perfil que mostrar, sino la lista de miembros.
    if app.friends[index].is_group {
        app.dm_profile = None;
        group_members_panel(app, ui, index);
        return;
    }
    let palette = app.palette;
    let (uid, name, url, color) = {
        let f = &app.friends[index];
        (f.user_id.clone(), f.name.clone(), f.avatar_url.clone(), f.avatar_color)
    };
    if uid.is_empty() {
        // Amigo de demo (sin id real): que no quede el perfil del DM anterior.
        app.dm_profile = None;
    } else {
        app.ensure_dm_profile(&uid, name, url, color);
    }

    egui::Panel::right("dm_profile")
        .exact_size(340.0)
        .resizable(false)
        .frame(theme::card_frame(&palette, Frame::new().fill(palette.panel), 8, 5, 10))
        .show(ui, |ui| {
            if app.dm_profile.is_some() {
                crate::ui::profile_popup::show_panel(app, ui);
            } else {
                simple_profile(app, ui, index);
            }
        });
}

/// Panel derecho de un DM grupal: "MIEMBROS — N" con avatar y nombre de cada
/// uno (vos incluido); un click abre su perfil.
fn group_members_panel(app: &mut App, ui: &mut egui::Ui, index: usize) {
    let palette = app.palette;
    let channel_id = app.friends[index].dm_channel_id.clone().unwrap_or_default();
    let dm = app.dms.iter().find(|d| d.id == channel_id);
    let owner = dm.and_then(|d| d.owner_id.clone());

    // (id, nombre, avatar, es el dueño del grupo)
    let mut members: Vec<(String, String, Option<String>, bool)> = Vec::new();
    if let Some(me) = app.me.as_ref() {
        let is_owner = owner.as_deref() == Some(me.id.as_str());
        members.push((me.id.clone(), me.display_name().to_string(), me.avatar_url(), is_owner));
    }
    if let Some(dm) = dm {
        for r in &dm.recipients {
            let is_owner = owner.as_deref() == Some(r.id.as_str());
            members.push((r.id.clone(), r.display_name().to_string(), r.avatar_url(), is_owner));
        }
    }

    egui::Panel::right("dm_profile")
        .exact_size(280.0)
        .resizable(false)
        .frame(theme::card_frame(&palette, Frame::new().fill(palette.panel), 8, 5, 10))
        .show(ui, |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                theme::text(
                    ui,
                    format!("MIEMBROS — {}", members.len()),
                    theme::semibold(11.0),
                    palette.dim,
                );
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt("dm_group_members")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (id, name, avatar_url, is_owner) in &members {
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::Vec2::new(ui.available_width(), 44.0),
                            egui::Sense::click(),
                        );
                        if resp.hovered() {
                            ui.painter().rect_filled(
                                rect,
                                egui::CornerRadius::same(8),
                                theme::row_fill(&palette, false, true),
                            );
                        }
                        let color = crate::lib::data::color_from_id(id);
                        let initial = name
                            .chars()
                            .find(|c| c.is_alphanumeric())
                            .map(|c| c.to_uppercase().to_string())
                            .unwrap_or_else(|| "?".to_string());
                        extra::avatar(
                            ui,
                            rect.left_center() + egui::Vec2::new(24.0, 0.0),
                            16.0,
                            avatar_url.as_deref(),
                            color,
                            &initial,
                            &palette,
                        );
                        let name_y = if *is_owner { 7.0 } else { 14.0 };
                        crate::ui::chat::emoji::paint_line_top(
                            ui,
                            rect.left_top() + egui::Vec2::new(50.0, name_y),
                            name,
                            theme::medium(13.5),
                            palette.text,
                            (rect.width() - 58.0).max(0.0),
                        );
                        if *is_owner {
                            ui.painter().text(
                                rect.left_top() + egui::Vec2::new(50.0, 25.0),
                                egui::Align2::LEFT_TOP,
                                "Propietario del grupo",
                                theme::regular(11.0),
                                palette.dim,
                            );
                        }
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            crate::ui::profile_popup::request_open(
                                ui.ctx(),
                                id.clone(),
                                name.clone(),
                                avatar_url.clone(),
                                color,
                            );
                        }
                    }
                });
        });
}

/// Versión simple (sin perfil real que pedir): banner liso + datos locales.
fn simple_profile(app: &App, ui: &mut egui::Ui, index: usize) {
    let palette = app.palette;
    let friend = &app.friends[index];
    // Banner con el color del avatar, como fondo de la tarjeta.
    let (banner_rect, _) = ui.allocate_exact_size(egui::Vec2::new(ui.available_width(), 100.0), egui::Sense::hover());
    ui.painter().rect_filled(
        banner_rect,
        0.0,
        extra::blend(friend.avatar_color, palette.window, 0.15),
    );

    ui.add_space(-36.0);
    ui.horizontal(|ui| {
        ui.add_space(20.0);
        let (rect, avatar_response) = ui.allocate_exact_size(egui::Vec2::splat(72.0), egui::Sense::click());
        ui.painter().circle_filled(rect.center(), 38.0, palette.panel);
        extra::avatar(
            ui,
            rect.center(),
            34.0,
            friend.avatar_url.as_deref(),
            friend.avatar_color,
            &friend.initial(),
            &palette,
        );
        if avatar_response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            crate::ui::profile_popup::request_open(
                ui.ctx(),
                &friend.user_id,
                &friend.name,
                friend.avatar_url.clone(),
                friend.avatar_color,
            );
        }
    });

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add_space(20.0);
        ui.vertical(|ui| {
            theme::text(ui, &friend.name, theme::bold(18.0), palette.text);
            theme::text(ui, &friend.handle, theme::regular(12.5), palette.dim);
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::Vec2::splat(8.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), 4.0, extra::status_color(friend.status, &palette));
                let raw = friend.subtitle.as_deref().unwrap_or_else(|| friend.status.label());
                let flat = crate::discord::models::one_line(raw);
                let flat: String = if flat.chars().count() > 34 {
                    flat.chars().take(33).chain(std::iter::once('…')).collect()
                } else {
                    flat
                };
                theme::text(ui, flat, theme::regular(12.0), palette.secondary);
            });
        });
    });

    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.add_space(20.0);
        ui.separator();
    });
    ui.add_space(10.0);

    ui.horizontal(|ui| {
        ui.add_space(20.0);
        theme::icon(ui, Icon::Users, 13.0, palette.dim);
        theme::text(
            ui,
            format!("{} servidores en común", friend.mutual_servers),
            theme::regular(12.5),
            palette.secondary,
        );
    });
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        ui.add_space(20.0);
        theme::text(ui, "MIEMBRO DESDE", theme::semibold(10.5), palette.dim);
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(20.0);
        theme::text(ui, &friend.member_since, theme::regular(13.0), palette.text);
    });

    ui.add_space(20.0);
    ui.horizontal(|ui| {
        ui.add_space(20.0);
        theme::pill_button(ui, &palette, "Ver perfil completo", false);
    });
}

fn central_panel(app: &mut App, ui: &mut egui::Ui, index: usize) {
    let palette = app.palette;
    egui::CentralPanel::default()
        .frame(theme::card_frame(
            &palette,
            Frame::new().fill(palette.window).inner_margin(Margin::same(0)),
            6,
            5,
            5,
        ))
        .show(ui, |ui| {
            let already_connected = app
                .friends
                .get(index)
                .and_then(|f| f.dm_channel_id.clone())
                .is_some_and(|id| app.is_connected_to_voice_dm_str(&id));
            // En un grupo, el subtítulo de la barra es la cantidad de miembros.
            let group_subtitle: Option<String> = {
                let f = &app.friends[index];
                f.is_group.then(|| {
                    let n = app
                        .dms
                        .iter()
                        .find(|d| Some(d.id.as_str()) == f.dm_channel_id.as_deref())
                        .map(|d| d.recipients.len() + 1)
                        .unwrap_or(1);
                    format!("{n} miembros")
                })
            };
            let call_clicked = {
                let friend = &app.friends[index];
                dm_top_bar(
                    ui,
                    &palette,
                    &friend.name,
                    friend.avatar_url.as_deref(),
                    friend.avatar_color,
                    &friend.initial(),
                    friend.status,
                    group_subtitle.as_deref().or(friend.subtitle.as_deref()),
                    already_connected,
                    friend.is_group,
                )
            };
            if call_clicked && already_connected {
                app.leave_voice();
            } else if call_clicked {
                match app.friends[index].dm_channel_id.clone() {
                    Some(channel_id) => app.start_dm_call(&channel_id),
                    // Todavía no se abrió el canal de DM de verdad (por
                    // REST) — pasa con amigos con los que nunca hablaste
                    // en esta sesión; `open_dm` ya lo está pidiendo en
                    // segundo plano apenas se entra a la conversación.
                    None => app.push_toast(
                        crate::lib::state::ToastKind::Info,
                        "Todavía estamos abriendo esta conversación",
                        "Probá de nuevo en un segundo.",
                    ),
                }
            }

            let friend_name = app.friends[index].name.clone();
            // Un DM 1:1 dice "@usuario"; un grupo, el nombre a secas.
            let placeholder = if app.friends[index].is_group {
                format!("Enviar mensaje a {friend_name}")
            } else {
                format!("Enviar mensaje a @{friend_name}")
            };
            let own_name = app
                .me
                .as_ref()
                .map(|m| m.display_name().to_string())
                .unwrap_or_else(|| "Aiden".to_string());
            let send_target = app.current_send_target();
            // Copiamos estos tres antes de pedir el préstamo mutable de
            // `app.friends[index].messages` de acá abajo — son `bool`
            // (Copy), así que no hace falta mantener viva ninguna
            // referencia a `app.friends[index]` mientras dura el `show`.
            let (loading, loading_more, has_more, has_newer, loading_newer) = {
                let friend = &app.friends[index];
                (friend.loading, friend.loading_more, friend.has_more, friend.has_newer, friend.loading_newer)
            };
            // El DM abierto, para saber a qué canal pertenece un salto.
            let dm_channel_id = app.friends[index].dm_channel_id.clone().unwrap_or_default();
            // En un DM se ofrecen los emojis personalizados de TODOS los
            // servers en los que estás (Discord no restringe esto a
            // servers "mutuos" con el destinatario) — ver `chat::show`.
            // En un DM no hay "server actual": TODOS los emojis personalizados
            // son externos y hacen falta Nitro. Sin él quedan bloqueados.
            let nitro_missing = app.has_nitro() == Some(false);
            let custom_emojis: Vec<crate::lib::data::EmojiGroup> = app
                .servers
                .iter()
                .filter_map(|s| crate::lib::data::EmojiGroup::from_server(s, false, nitro_missing))
                .collect();
            // "X está escribiendo…" en este DM.
            let typing = app.typing_line(ui.ctx(), &dm_channel_id);
            chat::set_typing_line(ui.ctx(), typing);
            // Límite de tamaño por archivo según el Nitro de la cuenta.
            chat::set_next_limits(
                ui.ctx(),
                chat::ChatLimits { max_upload_bytes: app.upload_limit_bytes(false), ..Default::default() },
            );
            let event = chat::show(
                ui,
                &palette,
                &mut app.friends[index].messages,
                &mut app.compose_text,
                &placeholder,
                &own_name,
                palette.accent,
                send_target,
                None, // sin lista de canales acá — es un DM, no un server
                &mut app.reply_target,
                &custom_emojis,
                loading,
                loading_more,
                has_more,
                has_newer,
                loading_newer,
                &mut app.pending_scroll_anchor,
                &mut app.pending_jump,
                app.unread_markers.get_mut(&dm_channel_id),
                false,
            );
            match event {
                chat::ChatEvent::ForwardRequested => {
                    app.push_toast(
                        crate::lib::state::ToastKind::Info,
                        "Reenviar mensaje",
                        "Todavía no se puede elegir un canal o DM destino en esta versión.",
                    );
                }
                chat::ChatEvent::LoadMoreRequested => app.load_more_messages(),
                chat::ChatEvent::LoadNewerRequested => app.load_newer_messages(),
                chat::ChatEvent::JumpToFirstUnread => app.jump_to_first_unread(),
                chat::ChatEvent::MarkAsRead => app.mark_viewed_read(),
                chat::ChatEvent::JumpToPresent => app.jump_to_present(),
                chat::ChatEvent::MarkUnread { message_id } => app.mark_message_unread(&message_id),
                chat::ChatEvent::Unavailable(what) => app.push_toast(
                    crate::lib::state::ToastKind::Info,
                    what,
                    "Todavía no está disponible en esta versión.",
                ),
                chat::ChatEvent::JumpToMessage { message_id } => app.jump_to_message(&dm_channel_id, &message_id),
                chat::ChatEvent::NitroRequired => app.open_modal(
                    "Necesitás Discord Nitro",
                    "En mensajes directos, los emojis personalizados requieren Discord Nitro, y tu cuenta no lo tiene. Podés usar los emojis Unicode.",
                ),
                chat::ChatEvent::Component(click) => app.press_component(click),
                // Los DMs no tienen hilos.
                chat::ChatEvent::OpenThread { .. } | chat::ChatEvent::CreateThread { .. } => {}
                chat::ChatEvent::None => {}
            }
        });
}

/// Como `home::top_bar` pero con el avatar del contacto en vez de un ícono,
/// y acciones típicas de una conversación (llamada, video, fijados, perfil).
/// Devuelve si se clickeó "Llamar" — quien llama decide qué hacer con eso
/// (`central_panel`, porque necesita el `dm_channel_id`, que acá no
/// tenemos: solo mostramos si ya estás en la llamada o no).
fn dm_top_bar(
    ui: &mut egui::Ui,
    palette: &Palette,
    name: &str,
    avatar_url: Option<&str>,
    avatar_color: egui::Color32,
    initial: &str,
    status: Status,
    activity: Option<&str>,
    already_connected: bool,
    is_group: bool,
) -> bool {
    let mut call_clicked = false;
    Frame::new()
        .stroke(theme::header_stroke(palette))
        .inner_margin(Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(24.0), egui::Sense::hover());
                extra::avatar(ui, rect.center(), 12.0, avatar_url, avatar_color, initial, palette);
                // Un grupo no tiene "estado": sin puntito.
                if !is_group {
                    extra::status_dot(
                        ui,
                        rect.center(),
                        12.0,
                        extra::status_color(status, palette),
                        theme::row_bg(palette),
                    );
                }
                ui.add_space(6.0);
                theme::text(ui, name, theme::semibold(14.5), palette.text);
                ui.add_space(6.0);
                // Estado (o actividad/estado personalizado) al lado del nombre.
                let status_text = activity.unwrap_or_else(|| status.label());
                let status_text = crate::discord::models::one_line(status_text);
                let status_text: String = if status_text.chars().count() > 40 {
                    status_text.chars().take(39).chain(std::iter::once('…')).collect()
                } else {
                    status_text.to_string()
                };
                theme::text(ui, status_text, theme::regular(12.0), palette.dim);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::icon_button(ui, Icon::User, 15.0, palette.dim, palette.text, "Ver perfil");
                    theme::icon_button(
                        ui,
                        Icon::Search,
                        15.0,
                        palette.dim,
                        palette.text,
                        "Buscar en la conversación",
                    );
                    theme::icon_button(ui, Icon::Pin, 15.0, palette.dim, palette.text, "Mensajes fijados");
                    let (icon, color, tooltip) = if already_connected {
                        (Icon::PhoneOff, palette.danger, "En llamada — cortar")
                    } else {
                        (Icon::Phone, palette.dim, "Llamar")
                    };
                    if theme::icon_button(ui, icon, 15.0, color, palette.text, tooltip).clicked() {
                        call_clicked = true;
                    }
                });
            });
        });
    call_clicked
}
