use egui::{CornerRadius, Frame, Margin, ScrollArea, Stroke, Vec2};

use crate::lib::data::Server;
use crate::lib::state::{App, Screen};
use crate::ui::emoji as twemoji;
use crate::ui::extra;
use crate::ui::nav;
use crate::ui::theme::{self, Icon, Palette};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let Screen::Server(index) = app.screen else {
        return;
    };
    if index >= app.servers.len() {
        app.go_home();
        return;
    }

    // Rail de servidores + lista de canales + barra de usuario combinada;
    // ver `ui::nav` (reemplaza los `rail::show` + `channel_list_panel`
    // separados de antes).
    nav::show(app, ui);
    // Con un hilo abierto, su panel ocupa el lugar de la lista de miembros
    // (como en el cliente real).
    //
    // En un canal de voz no hay lista de miembros (la de activos): los
    // integrantes ya se ven en la propia vista de la llamada, y a la derecha
    // queda solo el chat de voz (`voice_channel_view`).
    let (cat, chan) = app.current_channel;
    let in_voice_channel = app.servers[index]
        .channel(cat, chan)
        .is_some_and(|c| c.is_voice);
    if app.thread_panel.is_some() {
        thread_side_panel(app, ui, index);
    } else if !in_voice_channel {
        member_list_panel(app, ui, index);
    }
    central_panel(app, ui, index);
}

/// Panel lateral con un hilo abierto: título y quién lo empezó arriba, los
/// mensajes del hilo (mismo `ui::chat::show` que el chat principal, pero en
/// otro "scope" para que no se pisen) y su propio campo para escribir.
fn thread_side_panel(app: &mut App, ui: &mut egui::Ui, server_index: usize) {
    let palette = app.palette;
    let Some((thread_id, title, owner_id)) = app
        .thread_panel
        .as_ref()
        .map(|p| (p.thread_id.clone(), p.title.clone(), p.owner_id.clone()))
    else {
        return;
    };
    // Dónde quedó guardado el canal del hilo (ver `App::open_thread_panel`).
    let Some((cat, chan)) = app.servers[server_index].channel_position_by_id(&thread_id) else {
        app.close_thread_panel();
        return;
    };

    // "Empezado por X": se busca el nombre entre los mensajes ya cargados
    // del propio hilo y de su canal padre.
    let owner_name = owner_id.as_deref().and_then(|owner| {
        let server = &app.servers[server_index];
        let thread = server.channel(cat, chan)?;
        let parent = thread
            .parent_id
            .as_deref()
            .and_then(|id| server.categories.iter().flat_map(|c| c.channels.iter()).find(|c| c.channel_id.as_deref() == Some(id)));
        thread
            .messages
            .iter()
            .chain(parent.into_iter().flat_map(|c| c.messages.iter()))
            .find(|m| m.author_id == owner)
            .map(|m| m.author.clone())
    });

    let own_name = app
        .me
        .as_ref()
        .map(|m| m.display_name().to_string())
        .unwrap_or_else(|| "Aiden".to_string());
    let send_target = app.thread_send_target();
    let channel_names: std::collections::HashMap<String, String> = app.servers[server_index]
        .categories
        .iter()
        .flat_map(|cat| cat.channels.iter())
        .filter_map(|c| c.channel_id.clone().map(|id| (id, c.name.clone())))
        .collect();
    let nitro_missing = app.has_nitro() == Some(false);
    let custom_emojis: Vec<crate::lib::data::EmojiGroup> = {
        let mut groups: Vec<crate::lib::data::EmojiGroup> = Vec::new();
        groups.extend(crate::lib::data::EmojiGroup::from_server(&app.servers[server_index], true, nitro_missing));
        groups.extend(
            app.servers
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != server_index)
                .filter_map(|(_, s)| crate::lib::data::EmojiGroup::from_server(s, false, nitro_missing)),
        );
        groups
    };

    // Un título largo hacía que el placeholder ocupara dos líneas y se
    // saliera de la barra de escribir.
    let short_title: String = if title.chars().count() > 26 {
        title.chars().take(25).collect::<String>() + "…"
    } else {
        title.clone()
    };

    let mut close = false;
    let mut event = crate::ui::chat::ChatEvent::None;

    egui::Panel::right("thread_panel")
        .exact_size(420.0)
        .resizable(false)
        .frame(theme::card_frame(
            &palette,
            Frame::new().fill(palette.window).inner_margin(Margin::same(0)),
            4,
            5,
            10,
        ))
        .show(ui, |ui| {
            // Línea que separa el panel del chat (la interfaz nueva ya lo
            // separa con el hueco entre tarjetas).
            let rect = ui.max_rect();
            if !theme::is_modern() {
                ui.painter()
                    .vline(rect.left(), rect.y_range(), Stroke::new(1.0, palette.outline));
            }

            Frame::new().inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::icon(ui, Icon::ListPlus, 16.0, palette.dim);
                    theme::text(ui, &title, theme::semibold(14.5), palette.text);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, Icon::X, 14.0, palette.dim, palette.text, "Cerrar hilo").clicked() {
                            close = true;
                        }
                    });
                });
            });
            ui.painter()
                .hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, palette.outline));

            Frame::new().inner_margin(Margin { left: 16, right: 16, top: 10, bottom: 4 }).show(ui, |ui| {
                theme::text(ui, &title, theme::bold(20.0), palette.text);
                if let Some(name) = &owner_name {
                    ui.horizontal(|ui| {
                        theme::text(ui, "Empezado por", theme::regular(12.5), palette.dim);
                        theme::text(ui, name, theme::semibold(12.5), palette.text);
                    });
                }
            });

            let Some(panel) = app.thread_panel.as_mut() else { return };
            if let Some(channel) = app.servers[server_index].channel_mut(cat, chan) {
                let loading = channel.loading;
                event = crate::ui::chat::with_scope(1, || {
                    crate::ui::chat::show(
                        ui,
                        &palette,
                        &mut channel.messages,
                        &mut panel.compose_text,
                        &format!("Enviar mensaje a \"{}\"", short_title),
                        &own_name,
                        palette.accent,
                        send_target,
                        Some(&channel_names),
                        &mut panel.reply_target,
                        &custom_emojis,
                        loading,
                        // Los hilos son cortos: no se pide historial más viejo.
                        false,
                        false,
                        // Ni hay ventana "en el medio" que seguir bajando.
                        false,
                        false,
                        &mut panel.scroll_anchor,
                        &mut None,
                        None,
                        false,
                    )
                });
            }
        });

    if close {
        app.close_thread_panel();
        return;
    }
    match event {
        crate::ui::chat::ChatEvent::OpenThread { id, name, owner_id } => app.open_thread_panel(&id, &name, owner_id),
        crate::ui::chat::ChatEvent::Component(click) => app.press_component(click),
        crate::ui::chat::ChatEvent::ForwardRequested => {
            app.push_toast(
                crate::lib::state::ToastKind::Info,
                "Reenviar mensaje",
                "Todavía no se puede elegir un canal o DM destino en esta versión.",
            );
        }
        crate::ui::chat::ChatEvent::NitroRequired => app.open_modal(
            "Necesitás Discord Nitro",
            "Para reaccionar con emojis animados o de otros servidores hace falta Discord Nitro, y tu cuenta no lo tiene. Podés usar los estáticos de este servidor y los Unicode.",
        ),
        crate::ui::chat::ChatEvent::LoadMoreRequested
        | crate::ui::chat::ChatEvent::LoadNewerRequested
        | crate::ui::chat::ChatEvent::JumpToFirstUnread
        | crate::ui::chat::ChatEvent::MarkAsRead
        | crate::ui::chat::ChatEvent::JumpToPresent
        | crate::ui::chat::ChatEvent::MarkUnread { .. }
        | crate::ui::chat::ChatEvent::Unavailable(_)
        | crate::ui::chat::ChatEvent::JumpToMessage { .. }
        | crate::ui::chat::ChatEvent::CreateThread { .. }
        | crate::ui::chat::ChatEvent::None => {}
    }
}

/// Contenido de la lista de canales de un servidor: encabezado con el
/// nombre del servidor + categorías/canales scrolleables. Antes era un
/// `egui::Panel::left` propio con su propia barra de usuario abajo; ahora
/// se dibuja como columna dentro del panel de navegación combinado
/// (`ui::nav`).
pub fn channel_list_content(app: &mut App, ui: &mut egui::Ui, server_index: usize) {
    channel_list_content_ex(app, ui, server_index, None);
}

/// Alto del banner del server (interfaz nueva) cuando el server tiene uno.
pub const BANNER_HEIGHT: f32 = 150.0;
/// Alto del encabezado con solo el nombre, para servers sin banner.
pub const NAME_HEADER_HEIGHT: f32 = 56.0;

/// Igual que [`channel_list_content`]. Con `header_pad = Some(alto)` el
/// encabezado (nombre del server) NO se dibuja acá: lo pinta quien llama
/// encima de la lista (el banner de `ui::nav`) y la lista deja ese alto libre
/// arriba, así al scrollear las filas pasan por debajo del banner.
pub fn channel_list_content_ex(
    app: &mut App,
    ui: &mut egui::Ui,
    server_index: usize,
    header_pad: Option<f32>,
) {
    let palette = app.palette;
    let current_channel = app.current_channel;

    if header_pad.is_none() {
        let server = &app.servers[server_index];
        Frame::new()
            .stroke(theme::header_stroke(&palette))
            .inner_margin(Margin::symmetric(12, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::text(ui, &server.name, theme::semibold(14.5), palette.text);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::icon(ui, Icon::ChevronDown, 13.0, palette.dim);
                    });
                });
            });
    }

    let guild_id = app.servers[server_index].guild_id.clone();
    // Si se está viendo un post de foro, en la barra lateral se resalta su
    // foro (los posts no tienen fila propia).
    let open_forum_id: Option<String> = app.servers[server_index]
        .channel(current_channel.0, current_channel.1)
        .filter(|c| c.is_thread)
        .and_then(|c| c.parent_id.clone());
    let mut clicked_channel = None;
    // Además del `(cat, chan)` de `open_channel` (para navegar el panel
    // central), un canal de voz clickeado necesita su `channel_id` real
    // de Discord para `App::join_voice_channel` — que es `None` en los
    // canales de demo (sin id real, no hay a qué unirse de verdad).
    let mut clicked_voice_channel_id = None;
    ScrollArea::vertical()
        .id_salt("channel_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(header_pad.unwrap_or(0.0) + 8.0);
            let server = &app.servers[server_index];
            for (cat_idx, category) in server.categories.iter().enumerate() {
                // Categoría cuyos canales no se pueden ver (con tu rol
                // actual): no se muestra ni el encabezado.
                if !category.channels.is_empty()
                    && !category.channels.iter().any(|c| c.access.can_view)
                {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    twemoji::text_in(ui, &category.name, theme::semibold(11.0), palette.dim, twemoji::Set::Fluent);
                });
                ui.add_space(2.0);
                for (chan_idx, channel) in category.channels.iter().enumerate() {
                    // Canal que tu rol no puede ver: no se dibuja. (Se
                    // salta en vez de sacarlo de la lista para que los
                    // índices `(cat, chan)` sigan siendo válidos.)
                    if !channel.access.can_view {
                        continue;
                    }
                    let selected = current_channel == (cat_idx, chan_idx)
                        || (open_forum_id.is_some() && open_forum_id == channel.channel_id);
                    // Candado: canal privado que sí podés ver, o canal de
                    // voz al que no te podés conectar. Solo es un
                    // indicador: el clic funciona igual que siempre.
                    let locked = channel.access.show_lock();
                    if channel.is_voice {
                        if voice_channel_row(ui, &palette, &channel.name, selected, locked) {
                            clicked_channel = Some((cat_idx, chan_idx));
                            clicked_voice_channel_id = channel.channel_id.clone();
                        }
                        for occupant in &channel.voice_members {
                            let speaking =
                                app.user_voice_speaking_in_guild_str(&guild_id, &occupant.user_id);
                            let local = app.voice_participant_playback(&occupant.user_id);
                            let is_me = app.me.as_ref().is_some_and(|me| me.id == occupant.user_id);
                            voice_member_row(ui, &palette, occupant, speaking, local, is_me);
                        }
                    } else if channel_row(ui, &palette, &channel.name, selected, locked) {
                        clicked_channel = Some((cat_idx, chan_idx));
                    }
                }
                ui.add_space(8.0);
            }
        });
    if let Some((c, ch)) = clicked_channel {
        app.open_channel(c, ch);
        if let Some(channel_id) = clicked_voice_channel_id {
            app.join_voice_channel(&guild_id, &channel_id);
        }
    }
}

/// Banner del server arriba de la lista de canales (interfaz nueva): la
/// imagen del banner recortada para cubrir el rect, con el nombre del server
/// y la flechita encima. Sin banner queda un encabezado liso del color del
/// server. Se pinta DESPUÉS de la lista, que pasa por debajo.
pub fn paint_server_banner(ui: &mut egui::Ui, palette: &Palette, server: &Server, rect: egui::Rect) {
    let radius = CornerRadius::same(theme::CARD_RADIUS);
    let fill = theme::mix(server.icon_color, palette.panel, 0.55);
    ui.painter().rect_filled(rect, radius, fill);

    let has_image = server.banner_url.is_some();
    if let Some(url) = server.banner_url.as_ref() {
        let image = egui::Image::new(crate::ui::anim::plain(url))
            .corner_radius(radius)
            .show_loading_spinner(false);
        // Hasta que la imagen carga no se sabe su proporción: se ve el color liso.
        let uv = match image.load_for_size(ui.ctx(), rect.size()) {
            Ok(egui::load::TexturePoll::Ready { texture }) => Some(cover_uv(texture.size, rect.size())),
            _ => None,
        };
        if let Some(uv) = uv {
            image.uv(uv).paint_at(ui, rect);
        }
    }
    ui.painter()
        .rect_stroke(rect, radius, Stroke::new(1.0, palette.outline), egui::StrokeKind::Inside);

    // Nombre + flechita: sobre la imagen van en una cinta oscura para que se
    // lean con cualquier banner.
    let chip_h = 30.0;
    let chip_y = if has_image { 10.0 } else { ((rect.height() - chip_h) / 2.0).max(0.0) };
    let chip = egui::Rect::from_min_size(
        rect.min + Vec2::new(10.0, chip_y),
        Vec2::new((rect.width() - 20.0).max(40.0), chip_h),
    );
    let text_color = if has_image { egui::Color32::WHITE } else { palette.text };
    if has_image {
        ui.painter().rect_filled(chip, CornerRadius::same(12), egui::Color32::from_black_alpha(120));
    }
    let name = if server.name.chars().count() > 26 {
        server.name.chars().take(25).collect::<String>() + "…"
    } else {
        server.name.clone()
    };
    ui.painter().text(
        chip.left_center() + Vec2::new(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        name,
        theme::bold(15.0),
        text_color,
    );
    let chevron = egui::Rect::from_center_size(chip.right_center() - Vec2::new(16.0, 0.0), Vec2::splat(13.0));
    theme::paint_icon(ui, Icon::ChevronDown, chevron, 13.0, text_color);
}

/// Región (en UV) de una textura de tamaño `tex` que hay que mostrar para que
/// llene `target` sin deformarse, recortando parejo lo que sobra.
fn cover_uv(tex: Vec2, target: Vec2) -> egui::Rect {
    let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if tex.x <= 0.0 || tex.y <= 0.0 || target.x <= 0.0 || target.y <= 0.0 {
        return full;
    }
    let tex_ratio = tex.x / tex.y;
    let target_ratio = target.x / target.y;
    if tex_ratio > target_ratio {
        let w = target_ratio / tex_ratio;
        let x0 = (1.0 - w) / 2.0;
        egui::Rect::from_min_max(egui::pos2(x0, 0.0), egui::pos2(x0 + w, 1.0))
    } else {
        let h = tex_ratio / target_ratio;
        let y0 = (1.0 - h) / 2.0;
        egui::Rect::from_min_max(egui::pos2(0.0, y0), egui::pos2(1.0, y0 + h))
    }
}

/// Fila de un canal de voz: mismo look que `channel_row` pero con un
/// parlante en vez del `#` de texto (como en el cliente real).
fn voice_channel_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    name: &str,
    selected: bool,
    locked: bool,
) -> bool {
    let desired = Vec2::new(ui.available_width() - 16.0, 30.0);
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
        let bg = theme::row_fill(palette, selected, resp.hovered());
        ui.painter().rect_filled(rect, CornerRadius::same(theme::radius_small() + 2), bg);
        let icon_color = if selected { palette.text } else { palette.dim };
        let icon_rect = egui::Rect::from_center_size(rect.left_center() + Vec2::new(18.0, 0.0), Vec2::splat(14.0));
        theme::paint_icon(ui, Icon::Volume2, icon_rect, 14.0, icon_color);
        let color = if selected { palette.text } else { palette.secondary };
        twemoji::paint_line_in(
            ui,
            rect.left_center() + Vec2::new(32.0, 0.0),
            name,
            theme::medium(13.0),
            color,
            (rect.width() - 32.0 - 8.0 - lock_reserved(locked)).max(0.0),
            twemoji::Set::Fluent,
        );
        paint_lock(ui, palette, rect, locked);
        clicked = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    });
    clicked
}

/// Fila chiquita, indentada debajo de un canal de voz, por cada usuario
/// conectado ahí (avatar + nombre) — así se ve, sin entrar al canal,
/// quién está hablando ahora mismo, igual que en el cliente real.
/// `speaking` viene de `App::user_voice_speaking_in_guild_str` (el dato
/// en sí vive en `discord::voice::VoiceCache`, no en este `VoiceOccupant`
/// — ver el comentario en `channel_list_content`).
fn voice_member_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    occupant: &crate::lib::data::VoiceOccupant,
    speaking: bool,
    local: crate::discord::VoiceParticipantPlaybackSettings,
    is_me: bool,
) {
    let row = ui.horizontal(|ui| {
        ui.add_space(34.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), egui::Sense::hover());
        if speaking {
            ui.painter().circle_stroke(rect.center(), 11.0, Stroke::new(1.5, palette.accent));
        }
        extra::avatar(
            ui,
            rect.center(),
            10.0,
            occupant.avatar_url.as_deref(),
            occupant.avatar_color,
            &occupant.initial(),
            palette,
        );
        let name_color = if speaking { palette.text } else { palette.secondary };
        theme::text(ui, &occupant.name, theme::regular(12.5), name_color);
        // Mic/audífonos tachados junto al nombre cuando la persona se
        // silenció o ensordeció ella misma — igual que el cliente real,
        // que muestra esto acá aunque no estés vos en el canal.
        if occupant.self_deaf {
            ui.add_space(4.0);
            theme::icon(ui, Icon::VolumeX, 11.0, palette.danger);
        } else if occupant.self_mute {
            ui.add_space(4.0);
            theme::icon(ui, Icon::MicOff, 11.0, palette.danger);
        }
        if occupant.streaming {
            ui.add_space(4.0);
            live_pill(ui, palette);
        }
    });
    // Clic derecho sobre la fila: volumen de esa persona.
    let row_resp = ui.interact(
        row.response.rect,
        egui::Id::new(("voice_member_row_menu", occupant.user_id.as_str())),
        egui::Sense::click(),
    );
    crate::ui::audio_menu::show(&row_resp, &occupant.name, &occupant.user_id, local, is_me);
    ui.add_space(2.0);
}

/// Insignia roja "EN VIVO" para quien está transmitiendo (Go Live).
fn live_pill(ui: &mut egui::Ui, palette: &Palette) {
    Frame::new()
        .fill(palette.danger)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(5, 1))
        .show(ui, |ui| {
            theme::text(ui, "EN VIVO", theme::semibold(9.5), palette.on_accent);
        });
}

/// Espacio que se le quita al nombre para que no pise el candado.
fn lock_reserved(locked: bool) -> f32 {
    if locked { 22.0 } else { 0.0 }
}

/// Candado chico a la derecha de la fila de un canal (privado, o de voz sin
/// permiso para conectarse). No hace nada si `locked` es `false`.
fn paint_lock(ui: &egui::Ui, palette: &Palette, row: egui::Rect, locked: bool) {
    if !locked {
        return;
    }
    let rect = egui::Rect::from_center_size(row.right_center() - Vec2::new(14.0, 0.0), Vec2::splat(12.0));
    theme::paint_icon(ui, Icon::Lock, rect, 12.0, palette.dim);
}

fn channel_row(ui: &mut egui::Ui, palette: &Palette, name: &str, selected: bool, locked: bool) -> bool {
    let desired = Vec2::new(ui.available_width() - 16.0, 30.0);
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
        let bg = theme::row_fill(palette, selected, resp.hovered());
        ui.painter().rect_filled(rect, CornerRadius::same(theme::radius_small() + 2), bg);
        let color = if selected { palette.text } else { palette.secondary };
        ui.painter().text(
            rect.left_center() + Vec2::new(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "#",
            theme::medium(13.0),
            palette.dim,
        );
        // Con emojis (p. ej. "⌈🗣️general🗣️⌉") se dibujan a color; sin emojis
        // es el mismo `painter.text` de siempre.
        twemoji::paint_line_in(
            ui,
            rect.left_center() + Vec2::new(24.0, 0.0),
            name,
            theme::medium(13.0),
            color,
            (rect.width() - 24.0 - 8.0 - lock_reserved(locked)).max(0.0),
            twemoji::Set::Fluent,
        );
        paint_lock(ui, palette, rect, locked);
        clicked = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    });
    clicked
}

fn member_list_panel(app: &App, ui: &mut egui::Ui, server_index: usize) {
    let palette = app.palette;
    let server = &app.servers[server_index];

    egui::Panel::right("member_list")
        .exact_size(260.0)
        .resizable(false)
        .frame(theme::card_frame(
            &palette,
            Frame::new().fill(palette.panel).inner_margin(Margin::same(16)),
            14,
            5,
            10,
        ))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::icon(ui, Icon::Users, 13.0, palette.accent);
                theme::text(
                    ui,
                    format!("{}  ●  {}", server.online_count, server.total_count),
                    theme::semibold(12.5),
                    palette.text,
                );
            });
            ui.add_space(12.0);

            ScrollArea::vertical().show(ui, |ui| {
                // Un server real sin lista todavía: la suscripción al
                // Gateway está en camino (ver `App::subscribe_member_list`).
                if server.member_groups.is_empty() && !server.guild_id.is_empty() {
                    theme::text(ui, "Cargando miembros…", theme::regular(12.0), palette.dim);
                }
                for group in &server.member_groups {
                    theme::text(ui, &group.name, theme::semibold(11.0), palette.dim);
                    ui.add_space(4.0);
                    for m in &group.members {
                        let offline = m.status == crate::lib::data::Status::Offline;
                        let row = ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::hover());
                            extra::avatar(ui, rect.center(), 14.0, m.avatar_url.as_deref(), m.avatar_color, &member_initial(&m.name), &palette);
                            if offline {
                                // Como el cliente real: los desconectados van
                                // apagados y sin punto de estado.
                                ui.painter().circle_filled(rect.center(), 14.0, palette.panel.gamma_multiply(0.55));
                            } else {
                                extra::status_dot(ui, rect.center(), 14.0, extra::status_color(m.status, &palette), palette.panel);
                            }
                            // Color del rol más alto que tenga color; sin
                            // rol con color, el de texto normal.
                            let normal = if offline { palette.dim } else { palette.secondary };
                            let name_color = match m.name_color {
                                Some(color) if offline => color.gamma_multiply(0.5),
                                Some(color) => color,
                                None => normal,
                            };
                            ui.vertical(|ui| {
                                theme::text(ui, &m.name, theme::regular(13.0), name_color);
                                if let Some(subtitle) = &m.subtitle {
                                    theme::text(ui, subtitle, theme::regular(11.0), palette.dim);
                                }
                            });
                        });
                        // Toda la fila (avatar + nombre) es clickeable para
                        // abrir la tarjeta de perfil — el `horizontal` de
                        // arriba ya sigue sin sentir clicks (sus hijos son
                        // `Sense::hover()`), así que la interceptamos acá
                        // por encima, sobre el rect entero de la fila ya
                        // dibujada.
                        let row_response = ui
                            .interact(
                                row.response.rect,
                                egui::Id::new(("member_row_click", server_index, m.user_id.as_str())),
                                egui::Sense::click(),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        crate::ui::audio_menu::show(
                            &row_response,
                            &m.name,
                            &m.user_id,
                            app.voice_participant_playback(&m.user_id),
                            app.me.as_ref().is_some_and(|me| me.id == m.user_id),
                        );
                        if row_response.clicked() {
                            crate::ui::profile_popup::request_open(
                                ui.ctx(),
                                &m.user_id,
                                &m.name,
                                m.avatar_url.clone(),
                                m.avatar_color,
                            );
                        }
                        ui.add_space(4.0);
                    }
                    ui.add_space(10.0);
                }
            });
        });
}

fn member_initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

fn central_panel(app: &mut App, ui: &mut egui::Ui, server_index: usize) {
    let palette = app.palette;
    // A la derecha del chat hay otra tarjeta (miembros o hilo) salvo en un
    // canal de voz sin hilo abierto: ahí el borde de la ventana queda a 10.
    let (cat0, chan0) = app.current_channel;
    let in_voice = app.servers[server_index].channel(cat0, chan0).is_some_and(|c| c.is_voice);
    let right_gap = if app.thread_panel.is_some() || !in_voice { 5 } else { 10 };
    egui::CentralPanel::default()
        .frame(theme::card_frame(
            &palette,
            Frame::new().fill(palette.window).inner_margin(Margin::same(0)),
            6,
            5,
            right_gap,
        ))
        .show(ui, |ui| {
            let (cat, chan) = app.current_channel;
            let channel_name = current_channel_name(&app.servers[server_index], cat, chan);
            let (is_voice, is_forum, is_thread) = app.servers[server_index]
                .channel(cat, chan)
                .map(|c| (c.is_voice, c.is_forum, c.is_thread))
                .unwrap_or((false, false, false));
            if !theme::is_modern() {
                server_top_bar(ui, &palette, &channel_name, &app.servers[server_index].topic, is_voice);
            }

            if is_voice {
                voice_channel_view(app, ui, &palette, server_index, cat, chan, &channel_name);
                return;
            }

            // Foro: en vez de una charla, la lista de posts.
            if is_forum {
                forum_channel_view(app, ui, &palette, server_index, cat, chan);
                return;
            }

            // Post de foro abierto: se ve como un canal, con una barra para
            // volver a la lista de posts.
            if is_thread {
                let mut back = false;
                Frame::new().inner_margin(Margin::symmetric(12, 6)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if theme::pill_button(ui, &palette, "Volver al foro", false).clicked() {
                            back = true;
                        }
                    });
                });
                if back {
                    app.leave_thread();
                    return;
                }
            }

            channel_chat(app, ui, &palette, server_index, cat, chan, &channel_name);
        });
}

/// Vista central de un canal de foro (ver `ui::forum`).
fn forum_channel_view(
    app: &mut App,
    ui: &mut egui::Ui,
    palette: &Palette,
    server_index: usize,
    cat: usize,
    chan: usize,
) {
    let now_ms = crate::lib::data::now_ms();
    let event = {
        let Some(channel) = app.servers[server_index].channel_mut(cat, chan) else { return };
        crate::ui::forum::show(ui, palette, &mut channel.forum, now_ms)
    };
    match event {
        crate::ui::forum::ForumEvent::None => {}
        crate::ui::forum::ForumEvent::Open { id, title } => app.open_forum_post(&id, &title),
        crate::ui::forum::ForumEvent::LoadMore => app.load_forum_posts(false),
        crate::ui::forum::ForumEvent::Resort => app.resort_forum(),
        crate::ui::forum::ForumEvent::Create => app.create_forum_post(),
    }
}

/// Chat de texto de un canal del server (lista de mensajes + barra para
/// escribir). Lo usan los canales de texto y el panel de chat de la vista
/// de llamada de los canales de voz.
fn channel_chat(
    app: &mut App,
    ui: &mut egui::Ui,
    palette: &Palette,
    server_index: usize,
    cat: usize,
    chan: usize,
    channel_name: &str,
) {
    let own_name = app
        .me
        .as_ref()
        .map(|m| m.display_name().to_string())
        .unwrap_or_else(|| "Aiden".to_string());
    let send_target = app.current_send_target();
    // Para resolver menciones de canal (`<#id>`) dentro de los
    // mensajes de este server. Se arma una sola vez por frame acá
    // (no por mensaje) porque es la misma lista para todo el
    // canal.
    let channel_names: std::collections::HashMap<String, String> = app.servers[server_index]
        .categories
        .iter()
        .flat_map(|cat| cat.channels.iter())
        .filter_map(|c| c.channel_id.clone().map(|id| (id, c.name.clone())))
        .collect();
    // Emojis personalizados para el picker de reacciones: primero
    // los de ESTE server y después los de todos los demás en los
    // que estás (una sección por server, con su ícono en la barra
    // lateral del picker, como el cliente real). Se copian antes
    // del préstamo mutable de `channel_mut` de acá abajo.
    // Los de OTROS servers y los animados hacen falta Nitro: si
    // sabemos que la cuenta no lo tiene, quedan bloqueados (ver
    // `EmojiGroup::is_locked`).
    let nitro_missing = app.has_nitro() == Some(false);
    let custom_emojis: Vec<crate::lib::data::EmojiGroup> = {
        let mut groups: Vec<crate::lib::data::EmojiGroup> = Vec::new();
        groups.extend(crate::lib::data::EmojiGroup::from_server(&app.servers[server_index], true, nitro_missing));
        groups.extend(
            app.servers
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != server_index)
                .filter_map(|(_, s)| crate::lib::data::EmojiGroup::from_server(s, false, nitro_missing)),
        );
        groups
    };

    // ¿Puede borrar mensajes ajenos / sacar reacciones de otros acá?
    let can_manage_messages = {
        let server = &app.servers[server_index];
        let overwrites = server
            .channel(cat, chan)
            .map(|c| c.overwrites.as_slice())
            .unwrap_or(&[]);
        crate::lib::permissions::can_manage_messages(&server.guild_id, &server.access_ctx, &server.roles, overwrites)
    };

    // Permisos y modo lento de este canal para el chat (ver `chat::ChatLimits`).
    let limits = app.servers[server_index]
        .channel(cat, chan)
        .map(crate::ui::chat::ChatLimits::from_channel)
        .unwrap_or_default();
    let limits = crate::ui::chat::ChatLimits { max_upload_bytes: app.upload_limit_bytes(true), ..limits };
    crate::ui::chat::set_next_limits(ui.ctx(), limits);

    if let Some(channel) = app.servers[server_index].channel_mut(cat, chan) {
        let loading = channel.loading;
        let loading_more = channel.loading_more;
        let has_more = channel.has_more;
        let has_newer = channel.has_newer;
        let loading_newer = channel.loading_newer;
        let viewed_channel_id = channel.channel_id.clone().unwrap_or_default();
        let event = crate::ui::chat::show(
            ui,
            palette,
            &mut channel.messages,
            &mut app.compose_text,
            &format!("Enviar mensaje a #{}", channel_name),
            &own_name,
            palette.accent,
            send_target,
            Some(&channel_names),
            &mut app.reply_target,
            &custom_emojis,
            loading,
            loading_more,
            has_more,
            has_newer,
            loading_newer,
            &mut app.pending_scroll_anchor,
            &mut app.pending_jump,
            app.unread_markers.get_mut(&viewed_channel_id),
            can_manage_messages,
        );
        match event {
            crate::ui::chat::ChatEvent::ForwardRequested => {
                app.push_toast(
                    crate::lib::state::ToastKind::Info,
                    "Reenviar mensaje",
                    "Todavía no se puede elegir un canal o DM destino en esta versión.",
                );
            }
            crate::ui::chat::ChatEvent::LoadMoreRequested => app.load_more_messages(),
            crate::ui::chat::ChatEvent::LoadNewerRequested => app.load_newer_messages(),
            crate::ui::chat::ChatEvent::JumpToFirstUnread => app.jump_to_first_unread(),
            crate::ui::chat::ChatEvent::MarkAsRead => app.mark_viewed_read(),
            crate::ui::chat::ChatEvent::JumpToPresent => app.jump_to_present(),
            crate::ui::chat::ChatEvent::MarkUnread { message_id } => app.mark_message_unread(&message_id),
            crate::ui::chat::ChatEvent::Unavailable(what) => app.push_toast(
                crate::lib::state::ToastKind::Info,
                what,
                "Todavía no está disponible en esta versión.",
            ),
            crate::ui::chat::ChatEvent::JumpToMessage { message_id } => {
                app.jump_to_message(&viewed_channel_id, &message_id)
            }
            crate::ui::chat::ChatEvent::OpenThread { id, name, owner_id } => {
                app.open_thread_panel(&id, &name, owner_id)
            }
            crate::ui::chat::ChatEvent::CreateThread { channel_id, message_id, name } => {
                app.create_thread_from_message(&channel_id, &message_id, &name)
            }
            crate::ui::chat::ChatEvent::Component(click) => app.press_component(click),
            crate::ui::chat::ChatEvent::NitroRequired => app.open_modal(
                "Necesitás Discord Nitro",
                "Para reaccionar con emojis animados o de otros servidores hace falta Discord Nitro, y tu cuenta no lo tiene. Podés usar los estáticos de este servidor y los Unicode.",
            ),
            crate::ui::chat::ChatEvent::None => {}
        }
    }
}

/// Vista central de un canal de voz (con o sin integrantes). El dibujo vive
/// en `ui::call_view`; a la derecha va el chat de texto del canal (se puede
/// ocultar y volver a mostrar).
fn voice_channel_view(
    app: &mut App,
    ui: &mut egui::Ui,
    palette: &Palette,
    server_index: usize,
    cat: usize,
    chan: usize,
    channel_name: &str,
) {
    let real = !app.servers[server_index].guild_id.is_empty();
    let chat_id = egui::Id::new("voice_chat_open");
    let mut open = ui.ctx().data(|d| d.get_temp::<bool>(chat_id)).unwrap_or(true);

    if real && open {
        let mut close = false;
        egui::Panel::right("voice_chat_panel")
            .exact_size(380.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(if theme::is_modern() { palette.panel } else { palette.window })
                    .inner_margin(Margin::same(0)),
            )
            .show(ui, |ui| {
                let rect = ui.max_rect();
                ui.painter()
                    .vline(rect.left(), rect.y_range(), Stroke::new(1.0, palette.outline));
                Frame::new().inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        theme::text(ui, "Chat de voz", theme::semibold(14.0), palette.text);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(ui, Icon::X, 14.0, palette.dim, palette.text, "Ocultar chat")
                                .clicked()
                            {
                                close = true;
                            }
                        });
                    });
                });
                ui.painter()
                    .hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, palette.outline));
                channel_chat(app, ui, palette, server_index, cat, chan, channel_name);
            });
        if close {
            open = false;
        }
    }

    crate::ui::call_view::show(app, ui, palette, server_index, cat, chan, channel_name);

    if real && !open {
        let rect = ui.max_rect();
        egui::Area::new(egui::Id::new("voice_chat_show"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(rect.right() - 140.0, rect.top() + 10.0))
            .show(ui.ctx(), |ui| {
                if theme::pill_button(ui, palette, "Mostrar chat", false).clicked() {
                    open = true;
                }
            });
    }
    ui.ctx().data_mut(|d| d.insert_temp(chat_id, open));
}

fn current_channel_name(server: &Server, cat: usize, chan: usize) -> String {
    server
        .channel(cat, chan)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "canal".to_string())
}

fn server_top_bar(ui: &mut egui::Ui, palette: &Palette, channel_name: &str, topic: &str, is_voice: bool) {
    Frame::new()
        .stroke(theme::header_stroke(palette))
        .inner_margin(Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if is_voice {
                    theme::icon(ui, Icon::Volume2, 15.0, palette.dim);
                } else {
                    theme::text(ui, "#", theme::semibold(15.0), palette.dim);
                }
                twemoji::text_in(ui, channel_name, theme::semibold(14.5), palette.text, twemoji::Set::Fluent);
                ui.add_space(8.0);
                theme::text(ui, topic, theme::regular(12.0), palette.dim);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::icon_button(ui, Icon::Users, 15.0, palette.dim, palette.text, "Miembros");
                    theme::icon_button(ui, Icon::Pin, 15.0, palette.dim, palette.text, "Mensajes fijados");
                    theme::icon_button(ui, Icon::Search, 15.0, palette.dim, palette.text, "Buscar");
                });
            });
        });
}