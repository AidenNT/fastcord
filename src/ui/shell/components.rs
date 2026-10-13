//! Componentes interactivos de un mensaje: los botones que un bot pone junto
//! a sus embeds (por ejemplo "Responder" / "Responder anónimo" en un bot de
//! confesiones). Se dibujan debajo del contenido y los embeds, arriba de las
//! reacciones, igual que en el cliente real.
//!
//! Qué se dibuja:
//! * filas de acciones (tipo 1) con botones (tipo 2) de los cinco estilos:
//!   primario, secundario, éxito, peligro y link;
//! * los componentes "nuevos" (contenedor 17, sección 9, texto 10), de forma
//!   sencilla;
//! * los menús desplegables (3 texto, 5 usuarios, 6 roles, 7 usuarios o roles,
//!   8 canales): se abren con un clic. Los de texto muestran sus opciones (con
//!   emoji y descripción); los otros, los miembros, roles y canales del server
//!   abierto (`data::pick_directory`) con un buscador. Si aceptan una sola
//!   opción, elegirla ya manda la interacción; si aceptan varias
//!   (`max_values` > 1) se marcan y se mandan con "Enviar". La elección queda
//!   a la vista mientras el bot no edite el mensaje.
//!   Los usuarios que se pueden elegir son los que ya cargó la lista de
//!   miembros del server (Discord solo manda los de la parte visible).
//!
//! Apretar un botón o elegir en un menú NO hace nada acá: [`show`] devuelve un
//! [`ComponentClick`] y quien lo recibe (ver `App::press_component`) le manda
//! la interacción a la app dueña. Un botón de link sí actúa solo: abre la URL.
//!
//! Mientras la app no contesta, el botón apretado (o el menú, en su flecha)
//! muestra un spinner y no se puede volver a usar. La lista de componentes
//! cargando la publica `App` (`data::loading_buttons`).

use egui::{Color32, CornerRadius, Frame, Margin, Sense, Stroke, Vec2};

use crate::discord::models::{Component, ComponentEmoji};
use crate::lib::data::PickDirectory;
use super::extra;
use crate::lib::data::{ChatMessage, ComponentClick};
use crate::ui::chat::emoji as twemoji;
use crate::ui::chat::markdown::{self, MentionCtx};
use crate::ui::chat::media;
use crate::ui::theme::{self, Icon, Palette};

/// Alto de un botón (el de Discord mide 32 px).
const BUTTON_HEIGHT: f32 = 32.0;
const BUTTON_PAD_X: f32 = 14.0;
const EMOJI_SIZE: f32 = 16.0;
const EMOJI_GAP: f32 = 6.0;
/// Diámetro del spinner que reemplaza el contenido de un botón cargando.
const SPINNER_SIZE: f32 = 18.0;
/// Lado de la miniatura de una sección V2 y ancho máximo de un contenedor.
const THUMB_SIDE: f32 = 80.0;
const CONTAINER_MAX_W: f32 = 480.0;

/// Alto de la caja de un menú desplegable, su ancho máximo y el alto máximo de
/// su lista de opciones.
const SELECT_HEIGHT: f32 = 36.0;
const SELECT_MAX_W: f32 = 320.0;
const SELECT_POPUP_MAX_H: f32 = 280.0;

/// Lo que los componentes de UN mensaje necesitan saber al dibujarse.
struct Env {
    message_id: String,
    /// `custom_id` de los componentes del mensaje que ya mandaron su
    /// interacción y esperan al bot.
    loading: Vec<String>,
}

/// Lo que se tocó este frame: el `custom_id` y, si fue un menú, su tipo y los
/// valores elegidos (`None` = fue un botón).
type Clicked = Option<(String, Option<(u8, Vec<String>)>)>;

/// Bit de `flags` de un mensaje efímero (solo lo ve quien lo provocó).
pub const FLAG_EPHEMERAL: u64 = 1 << 6;

/// Dibuja los componentes del mensaje. Devuelve el botón apretado este frame,
/// si hubo alguno.
pub fn show(ui: &mut egui::Ui, palette: &Palette, msg: &ChatMessage) -> Option<ComponentClick> {
    if msg.components.is_empty() {
        return None;
    }
    let mut clicked: Clicked = None;
    let ctx = MentionCtx {
        mentions: &msg.mentions,
        channels: None,
    };
    // Componentes de este mensaje que ya mandaron su interacción y esperan al bot.
    let env = Env {
        message_id: msg.id.clone(),
        loading: crate::lib::data::loading_buttons(ui.ctx(), &msg.id),
    };
    ui.add_space(4.0);
    for component in &msg.components {
        draw(ui, palette, component, &ctx, &env, &mut clicked);
    }
    clicked.map(|(custom_id, values)| ComponentClick {
        message_id: msg.id.clone(),
        channel_id: msg.channel_id.clone(),
        application_id: msg.application_id.clone(),
        flags: msg.flags,
        custom_id,
        values,
    })
}

/// Un componente cualquiera (recursivo para filas, contenedores y secciones).
fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    component: &Component,
    ctx: &MentionCtx,
    env: &Env,
    clicked: &mut Clicked,
) {
    match component.kind {
        // Fila de acciones: los botones van uno al lado del otro, y si no
        // entran, siguen abajo.
        1 => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                for child in &component.components {
                    draw(ui, palette, child, ctx, env, clicked);
                }
            });
            ui.add_space(2.0);
        }
        Component::BUTTON => button(ui, palette, component, env, clicked),
        // Menús desplegables: se abren y se puede elegir.
        Component::STRING_SELECT
        | Component::USER_SELECT
        | Component::ROLE_SELECT
        | Component::MENTIONABLE_SELECT
        | Component::CHANNEL_SELECT => select_menu(ui, palette, env, component, clicked),
        // Texto suelto (componentes nuevos): con el markdown de Discord
        // (negrita, cursiva, `-#` subtexto, menciones, emojis...).
        10 => {
            if let Some(content) = component.content.as_deref().filter(|c| !c.is_empty()) {
                markdown::show(ui, palette, content, 13.5, palette.text, ctx);
            }
        }
        // Sección: texto a la izquierda y, si hay, un accesorio a la derecha
        // (una miniatura o un botón).
        9 => {
            let thumb = component.accessory.as_deref().filter(|a| a.kind == 11);
            if let Some(thumb) = thumb {
                let text_w = (ui.available_width() - THUMB_SIDE - 12.0).max(120.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(text_w);
                        for child in &component.components {
                            draw(ui, palette, child, ctx, env, clicked);
                        }
                    });
                    ui.add_space(4.0);
                    draw(ui, palette, thumb, ctx, env, clicked);
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    for child in &component.components {
                        draw(ui, palette, child, ctx, env, clicked);
                    }
                    if let Some(accessory) = component.accessory.as_deref() {
                        draw(ui, palette, accessory, ctx, env, clicked);
                    }
                });
            }
        }
        // Miniatura (accesorio de una sección).
        11 => {
            if let Some(m) = component.media.as_ref() {
                let alt = component.description.as_deref().unwrap_or("");
                media::component_media(ui, palette, m, Some(THUMB_SIDE), component.spoiler, alt);
            }
        }
        // Galería de medios: una imagen/GIF a todo el ancho, o una grilla.
        12 => {
            let n = component.items.len();
            if n == 0 {
                return;
            }
            ui.add_space(2.0);
            if n == 1 {
                let item = &component.items[0];
                let alt = item.description.as_deref().unwrap_or("");
                media::component_media(ui, palette, &item.media, None, item.spoiler, alt);
            } else {
                let side = ((ui.available_width() - 4.0) / 2.0).clamp(80.0, 200.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(4.0);
                    for item in &component.items {
                        let alt = item.description.as_deref().unwrap_or("");
                        media::component_media(ui, palette, &item.media, Some(side), item.spoiler, alt);
                    }
                });
            }
            ui.add_space(2.0);
        }
        // Archivo adjunto: un link con el nombre.
        13 => {
            if let Some(f) = component.file.as_ref().filter(|f| f.has_url()) {
                let url = f.display_url().to_string();
                let label = component.name.clone().unwrap_or_else(|| {
                    let last = f.url.rsplit('/').next().unwrap_or("archivo");
                    last.split(['?', '#']).next().unwrap_or("archivo").to_string()
                });
                let resp = ui.link(egui::RichText::new(label).font(theme::medium(13.5)));
                if resp.clicked() && (url.starts_with("https://") || url.starts_with("http://")) {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            }
        }
        // Separador: espacio y, si `divider`, una línea fina.
        14 => {
            let gap = if component.spacing == Some(2) { 16.0 } else { 8.0 };
            ui.add_space(gap / 2.0);
            if component.divider.unwrap_or(true) {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(rect, CornerRadius::ZERO, palette.outline);
            }
            ui.add_space(gap / 2.0);
        }
        // Contenedor: caja con la barra de color a la izquierda (el
        // `accent_color`), como un embed.
        17 => {
            let radius = theme::radius_small();
            let bar_color = component.accent_color.map(rgb_from_int).unwrap_or(palette.outline);
            let inner = Frame::new()
                .fill(palette.surface)
                .corner_radius(CornerRadius {
                    nw: radius,
                    ne: radius,
                    sw: radius,
                    se: radius,
                })
                .inner_margin(Margin {
                    left: 16,
                    right: 12,
                    top: 8,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(CONTAINER_MAX_W));
                    for child in &component.components {
                        draw(ui, palette, child, ctx, env, clicked);
                    }
                });
            let r = inner.response.rect;
            let bar = egui::Rect::from_min_max(r.min, egui::pos2(r.min.x + 4.0, r.max.y));
            ui.painter().rect_filled(
                bar,
                CornerRadius {
                    nw: radius,
                    ne: 0,
                    sw: radius,
                    se: 0,
                },
                bar_color,
            );
        }
        _ => {}
    }
}

fn rgb_from_int(c: u32) -> Color32 {
    Color32::from_rgb(((c >> 16) & 0xff) as u8, ((c >> 8) & 0xff) as u8, (c & 0xff) as u8)
}

/// Colores (fondo, fondo con el mouse encima, texto) de un botón según su estilo.
fn colors(palette: &Palette, style: u8) -> (Color32, Color32, Color32) {
    match style {
        1 => (palette.accent, palette.accent_hover, palette.on_accent),
        3 => (
            Color32::from_rgb(35, 134, 54),
            Color32::from_rgb(26, 110, 42),
            Color32::WHITE,
        ),
        4 => (palette.danger, palette.danger.gamma_multiply(0.85), Color32::WHITE),
        // 2 (secundario) y 5 (link): el gris neutro de la interfaz.
        _ => (palette.surface_hover, palette.surface_active, palette.text),
    }
}

fn button(
    ui: &mut egui::Ui,
    palette: &Palette,
    component: &Component,
    env: &Env,
    clicked: &mut Clicked,
) {
    let (fill, fill_hover, text_color) = colors(palette, component.style);
    let is_link = component.is_link_button();
    let label = component.label.as_deref().unwrap_or("");
    let emoji = component.emoji.as_ref();
    let has_emoji = emoji.is_some_and(|e| e.id.is_some() || e.name.as_deref().is_some_and(|n| !n.is_empty()));

    let galley = (!label.is_empty()).then(|| {
        ui.painter()
            .layout_no_wrap(label.to_string(), theme::medium(13.5), text_color)
    });

    // Ancho = margen + [emoji] + [texto] + [ícono de link] + margen.
    let mut width = BUTTON_PAD_X * 2.0;
    if has_emoji {
        width += EMOJI_SIZE;
    }
    if let Some(galley) = &galley {
        if has_emoji {
            width += EMOJI_GAP;
        }
        width += galley.size().x;
    }
    if is_link {
        width += EMOJI_GAP + 12.0;
    }
    let width = width.max(60.0);

    // Su interacción ya salió y el bot todavía no contestó: spinner adentro y
    // sin reaccionar a más clics. (Un botón de link nunca "carga": solo abre
    // la URL.)
    let is_loading = !is_link
        && component
            .custom_id
            .as_deref()
            .is_some_and(|id| env.loading.iter().any(|l| l == id));
    // Un botón cargando, deshabilitado, o uno sin `custom_id` ni URL (mal
    // formado), no reacciona al mouse.
    let usable =
        !is_loading && !component.disabled && (is_link && component.url.is_some() || component.custom_id.is_some());
    let sense = if usable { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, BUTTON_HEIGHT), sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));

    if ui.is_rect_visible(rect) {
        let mut background = if usable && response.hovered() { fill_hover } else { fill };
        let mut foreground = text_color;
        // Un botón cargando conserva sus colores (no es "deshabilitado").
        if !usable && !is_loading {
            background = background.gamma_multiply(0.5);
            foreground = foreground.gamma_multiply(0.5);
        }
        ui.painter()
            .rect_filled(rect, CornerRadius::same(theme::radius_small()), background);

        // Cargando: el spinner va centrado en lugar del emoji y el texto. El
        // botón no cambia de tamaño (el chat cachea la altura de las filas).
        if is_loading {
            let mut spinner_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
            );
            theme::spinner(&mut spinner_ui, SPINNER_SIZE, foreground);
        }

        let mut x = rect.left() + BUTTON_PAD_X;
        if let Some(emoji) = emoji.filter(|_| has_emoji && !is_loading) {
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(x, rect.center().y - EMOJI_SIZE / 2.0),
                Vec2::splat(EMOJI_SIZE),
            );
            match emoji.id.as_deref() {
                Some(id) => paint_custom_emoji(ui, palette, icon_rect, id, emoji.animated),
                None => {
                    if let Some(name) = emoji.name.as_deref() {
                        twemoji::paint(ui, icon_rect, name);
                    }
                }
            }
            x += EMOJI_SIZE + EMOJI_GAP;
        }
        if let Some(galley) = galley.filter(|_| !is_loading) {
            let pos = egui::pos2(x, rect.center().y - galley.size().y / 2.0);
            let advance = galley.size().x;
            ui.painter().galley(pos, galley, foreground);
            x += advance;
        }
        if is_link {
            let icon_rect =
                egui::Rect::from_min_size(egui::pos2(x + EMOJI_GAP, rect.center().y - 6.0), Vec2::splat(12.0));
            theme::paint_icon(ui, Icon::ExternalLink, icon_rect, 12.0, foreground);
        }
    }

    let response = if usable {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };

    if usable && response.clicked() {
        if is_link {
            if let Some(url) = component.url.as_deref() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        } else if let Some(custom_id) = component.custom_id.as_ref() {
            *clicked = Some((custom_id.clone(), None));
        }
    }
}

/// Emoji personalizado de un botón: mismo criterio que las reacciones
/// (`.gif` si es animado, `.png` si no).
fn paint_custom_emoji(ui: &egui::Ui, palette: &Palette, rect: egui::Rect, id: &str, animated: bool) {
    let ext = if animated { "gif" } else { "png" };
    let url = format!("https://cdn.discordapp.com/emojis/{id}.{ext}?size=48");
    let url = if animated {
        crate::ui::chat::media::gif_safe_url(&url)
    } else {
        url
    };
    let image = egui::Image::new(crate::ui::chat::anim::source(ui.ctx(), &url))
        .fit_to_exact_size(rect.size())
        .show_loading_spinner(false);
    match image.load_for_size(ui.ctx(), rect.size()) {
        Ok(egui::load::TexturePoll::Ready { .. }) => image.paint_at(ui, rect),
        _ => {
            ui.painter().rect_filled(rect, 3.0, palette.surface_active);
        }
    }
}

/// Un menú desplegable mal formado (sin `custom_id`, o de texto sin opciones),
/// dibujado apagado.
fn select_placeholder(ui: &mut egui::Ui, palette: &Palette, component: &Component) {
    let text = component
        .placeholder
        .as_deref()
        .filter(|t| !t.is_empty())
        .unwrap_or("Seleccioná una opción");
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::radius_small()))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(220.0);
            ui.horizontal(|ui| {
                theme::text(ui, text, theme::regular(13.0), palette.dim);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::icon(ui, Icon::ChevronDown, 12.0, palette.dim);
                });
            });
        })
        .response
        .on_hover_text("Este menú viene mal formado (sin identificador o sin opciones) y no se puede usar.");
}

/// Dibuja el emoji de un componente (unicode o personalizado) en `rect`.
fn paint_component_emoji(ui: &egui::Ui, palette: &Palette, rect: egui::Rect, emoji: &ComponentEmoji) {
    match emoji.id.as_deref() {
        Some(id) => paint_custom_emoji(ui, palette, rect, id, emoji.animated),
        None => {
            if let Some(name) = emoji.name.as_deref() {
                twemoji::paint(ui, rect, name);
            }
        }
    }
}

fn has_emoji(emoji: Option<&ComponentEmoji>) -> bool {
    emoji.is_some_and(|e| e.id.is_some() || e.name.as_deref().is_some_and(|n| !n.is_empty()))
}

/// Lo que se dibuja a la izquierda del nombre en una fila de la lista.
enum Lead {
    None,
    Avatar { url: Option<String>, color: Color32, initial: String },
    Dot(Color32),
    Icon(Icon),
}

/// Una fila de la lista de un menú: de una opción de texto, o de un usuario,
/// rol o canal.
struct Item<'a> {
    value: &'a str,
    label: String,
    description: Option<&'a str>,
    emoji: Option<&'a ComponentEmoji>,
    lead: Lead,
}

/// Las filas de un menú de usuarios, roles, menciones o canales que coinciden
/// con la búsqueda `query`.
fn entity_items<'a>(component: &Component, dir: &'a PickDirectory, query: &str) -> Vec<Item<'a>> {
    let query = query.trim().to_lowercase();
    let hit = |name: &str| query.is_empty() || name.to_lowercase().contains(&query);
    let mut out = Vec::new();
    let kind = component.kind;

    if matches!(kind, Component::USER_SELECT | Component::MENTIONABLE_SELECT) {
        for user in dir.users.iter().filter(|u| hit(&u.name)) {
            out.push(Item {
                value: &user.id,
                label: user.name.clone(),
                description: None,
                emoji: None,
                lead: Lead::Avatar {
                    url: user.avatar_url.clone(),
                    color: user.color,
                    initial: user.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(),
                },
            });
        }
    }
    if matches!(kind, Component::ROLE_SELECT | Component::MENTIONABLE_SELECT) {
        for role in dir.roles.iter().filter(|r| hit(&r.name)) {
            out.push(Item {
                value: &role.id,
                label: role.name.clone(),
                description: None,
                emoji: None,
                lead: Lead::Dot(role.color.unwrap_or(Color32::from_gray(0x99))),
            });
        }
    }
    if kind == Component::CHANNEL_SELECT {
        let types = &component.channel_types;
        let allowed = |voice: bool, forum: bool| {
            types.is_empty()
                || if voice {
                    types.iter().any(|t| matches!(t, 2 | 13))
                } else if forum {
                    types.iter().any(|t| matches!(t, 15 | 16))
                } else {
                    types.iter().any(|t| matches!(t, 0 | 5 | 10 | 11 | 12))
                }
        };
        for channel in dir.channels.iter().filter(|c| allowed(c.voice, c.forum) && hit(&c.name)) {
            out.push(Item {
                value: &channel.id,
                label: channel.name.clone(),
                description: None,
                emoji: None,
                lead: Lead::Icon(if channel.voice { Icon::Volume2 } else { Icon::Hash }),
            });
        }
    }
    out
}

/// Nombre con el que se muestra, en la caja cerrada, un valor elegido de un
/// menú de usuarios, roles, menciones o canales. Si no se conoce (por ejemplo
/// un usuario que la lista de miembros todavía no cargó), el id.
fn entity_label(kind: u8, dir: &PickDirectory, id: &str) -> String {
    let user = || dir.users.iter().find(|u| u.id == id).map(|u| u.name.clone());
    let role = || dir.roles.iter().find(|r| r.id == id).map(|r| r.name.clone());
    let channel = || dir.channels.iter().find(|c| c.id == id).map(|c| format!("#{}", c.name));
    match kind {
        Component::USER_SELECT => user(),
        Component::ROLE_SELECT => role(),
        Component::MENTIONABLE_SELECT => user().or_else(role),
        _ => channel(),
    }
    .unwrap_or_else(|| id.to_string())
}

/// Aplica el clic en una fila: en un menú de una sola opción la elige y manda;
/// en uno de varias, la marca o la desmarca (hasta `max`).
fn apply_pick(
    value: &str,
    multi: bool,
    max: usize,
    selected: &mut Vec<String>,
    send: &mut Option<Vec<String>>,
    close: &mut bool,
) {
    if !multi {
        *selected = vec![value.to_string()];
        *send = Some(selected.clone());
        *close = true;
    } else if let Some(at) = selected.iter().position(|v| v == value) {
        selected.remove(at);
    } else if selected.len() < max {
        selected.push(value.to_string());
    }
}

/// Menú desplegable (tipos 3, 5, 6, 7 y 8). La caja muestra lo elegido (o el
/// texto de ayuda); al hacerle clic se abre la lista debajo.
///
/// * una sola opción (`max_values` 1): elegirla manda la interacción;
/// * varias: se marcan hasta `max_values` y "Enviar" (habilitado desde
///   `min_values`) manda las elegidas.
///
/// Los de usuarios/roles/canales traen un buscador y una lista virtualizada
/// (puede haber miles de miembros).
///
/// Lo elegido y si la lista está abierta viven en la memoria temporal de egui,
/// por mensaje y `custom_id`. Lo elegido solo vale mientras las opciones (o los
/// valores por defecto) del mensaje no cambien: si el bot lo edita, vuelve a
/// valer lo que diga él.
fn select_menu(ui: &mut egui::Ui, palette: &Palette, env: &Env, component: &Component, clicked: &mut Clicked) {
    let kind = component.kind;
    let entity = kind != Component::STRING_SELECT;
    let Some(custom_id) = component
        .custom_id
        .as_deref()
        .filter(|_| entity || !component.options.is_empty())
    else {
        // Mal formado (sin `custom_id` o sin opciones): nada que elegir.
        select_placeholder(ui, palette, component);
        return;
    };
    let egui_ctx = ui.ctx().clone();
    let dir = entity.then(|| crate::lib::data::pick_directory(&egui_ctx));
    // Discord permite hasta 25 valores en cualquier menú.
    let limit = if entity { 25 } else { component.options.len().min(25) };
    let max = (component.max_values.unwrap_or(1) as usize).clamp(1, limit.max(1));
    let min = (component.min_values.unwrap_or(1) as usize).min(max);
    let multi = max > 1;
    let is_loading = env.loading.iter().any(|l| l == custom_id);
    let usable = !component.disabled && !is_loading;

    let key = (env.message_id.as_str(), custom_id);
    let open_id = egui::Id::new(("ecord_select_open", key));
    let value_id = egui::Id::new(("ecord_select_value", key));
    let popup_id = egui::Id::new(("ecord_select_popup", key));
    let query_id = egui::Id::new(("ecord_select_query", key));
    let focus_id = egui::Id::new(("ecord_select_focus", key));

    // Lo elegido: lo que dejó la persona si lo que trae el mensaje sigue igual;
    // si no, lo que el mensaje trae marcado.
    let defaults: Vec<String> = if entity {
        component.default_values.iter().map(|d| d.id.clone()).collect()
    } else {
        component
            .options
            .iter()
            .filter(|o| o.is_default)
            .map(|o| o.value.clone())
            .collect()
    };
    let signature: String = if entity {
        defaults.join("\u{1}")
    } else {
        component
            .options
            .iter()
            .map(|o| format!("{}\u{1}{}\u{2}", o.value, u8::from(o.is_default)))
            .collect()
    };
    let mut selected: Vec<String> = egui_ctx
        .data(|d| d.get_temp::<(String, Vec<String>)>(value_id))
        .filter(|(sig, _)| *sig == signature)
        .map(|(_, values)| values)
        .unwrap_or(defaults);
    let open = egui_ctx.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);

    // Texto de la caja.
    let chosen: Vec<String> = match &dir {
        Some(dir) => selected.iter().map(|id| entity_label(kind, dir, id)).collect(),
        None => component
            .options
            .iter()
            .filter(|o| selected.contains(&o.value))
            .map(|o| o.label.clone())
            .collect(),
    };
    let (text, text_color) = if chosen.is_empty() {
        let placeholder = component.placeholder.as_deref().filter(|t| !t.is_empty());
        (placeholder.unwrap_or("Elige una opción").to_string(), palette.dim)
    } else {
        (chosen.join(", "), palette.text)
    };

    let width = ui.available_width().clamp(200.0, SELECT_MAX_W);
    let sense = if usable { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, SELECT_HEIGHT), sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, ui.is_enabled(), &text));

    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same(theme::radius_small());
        let background = if usable && (response.hovered() || open) {
            palette.surface_hover
        } else {
            palette.surface
        };
        ui.painter().rect_filled(rect, radius, background);
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, if open { palette.accent } else { palette.outline }),
            egui::StrokeKind::Inside,
        );
        // Un menú deshabilitado se apaga; uno cargando conserva sus colores.
        let fade = if usable || is_loading { 1.0 } else { 0.5 };
        let galley = ui
            .painter()
            .layout_no_wrap(text.clone(), theme::regular(13.5), text_color.gamma_multiply(fade));
        let clip = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 10.0, rect.top()),
            egui::pos2(rect.right() - 32.0, rect.bottom()),
        );
        let pos = egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0);
        ui.painter().with_clip_rect(clip).galley(pos, galley, text_color);

        let arrow = egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.center().y), Vec2::splat(14.0));
        if is_loading {
            let mut spinner_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(arrow)
                    .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
            );
            theme::spinner(&mut spinner_ui, 14.0, palette.dim);
        } else {
            theme::paint_icon(ui, Icon::ChevronDown, arrow, 14.0, palette.dim.gamma_multiply(fade));
        }
    }

    let response = if usable {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    if usable && response.clicked() {
        egui_ctx.data_mut(|d| {
            d.insert_temp(open_id, !open);
            // Al abrir, el buscador arranca vacío y con el foco.
            d.insert_temp(query_id, String::new());
            d.insert_temp(focus_id, true);
        });
        egui_ctx.request_repaint();
    }

    if !open {
        return;
    }
    if !usable {
        // Se deshabilitó (o quedó cargando) con la lista abierta: se cierra.
        egui_ctx.data_mut(|d| d.insert_temp(open_id, false));
        return;
    }

    // ---- Lista, flotando debajo de la caja ----
    let mut query: String = egui_ctx.data(|d| d.get_temp::<String>(query_id)).unwrap_or_default();
    let mut want_focus = egui_ctx.data(|d| d.get_temp::<bool>(focus_id)).unwrap_or(false);
    let mut close = false;
    let mut send: Option<Vec<String>> = None;
    let area = egui::Area::new(popup_id)
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(rect.left(), rect.bottom() + 4.0))
        .show(&egui_ctx, |ui| {
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::radius() + 2))
                .inner_margin(Margin::same(6))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 20,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(ui, |ui| {
                    ui.set_width(width.max(240.0) - 12.0);
                    ui.spacing_mut().item_spacing.y = 2.0;

                    if let Some(dir) = dir.as_deref() {
                        // Usuarios, roles y canales: buscador + lista virtualizada.
                        let search = ui.add(
                            egui::TextEdit::singleline(&mut query)
                                .hint_text("Buscar")
                                .font(theme::regular(13.0))
                                .desired_width(f32::INFINITY),
                        );
                        if want_focus {
                            search.request_focus();
                            want_focus = false;
                        }
                        ui.add_space(4.0);
                        let items = entity_items(component, dir, &query);
                        if items.is_empty() {
                            theme::text(ui, "No hay resultados", theme::regular(12.5), palette.dim);
                        } else {
                            egui::ScrollArea::vertical()
                                .max_height(SELECT_POPUP_MAX_H)
                                .auto_shrink([false, true])
                                .show_rows(ui, ITEM_HEIGHT, items.len(), |ui, range| {
                                    for item in &items[range] {
                                        let is_selected = selected.iter().any(|v| v == item.value);
                                        if item_row(ui, palette, item, is_selected, multi).clicked() {
                                            apply_pick(item.value, multi, max, &mut selected, &mut send, &mut close);
                                        }
                                    }
                                });
                        }
                    } else {
                        // Menú de texto: pocas opciones (hasta 25), filas de alto variable.
                        egui::ScrollArea::vertical()
                            .max_height(SELECT_POPUP_MAX_H)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for option in &component.options {
                                    let item = Item {
                                        value: &option.value,
                                        label: option.label.clone(),
                                        description: option.description.as_deref().filter(|d| !d.is_empty()),
                                        emoji: option.emoji.as_ref(),
                                        lead: Lead::None,
                                    };
                                    let is_selected = selected.contains(&option.value);
                                    if item_row(ui, palette, &item, is_selected, multi).clicked() {
                                        apply_pick(item.value, multi, max, &mut selected, &mut send, &mut close);
                                    }
                                }
                            });
                    }

                    if multi {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let counter = format!("{} de {}", selected.len(), max);
                            theme::text(ui, counter, theme::regular(12.0), palette.dim);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let ready = selected.len() >= min;
                                let sense = if ready { Sense::click() } else { Sense::hover() };
                                let (r, resp) = ui.allocate_exact_size(Vec2::new(76.0, 28.0), sense);
                                let fill = if !ready {
                                    palette.surface_active
                                } else if resp.hovered() {
                                    palette.accent_hover
                                } else {
                                    palette.accent
                                };
                                ui.painter().rect_filled(r, CornerRadius::same(theme::radius_small()), fill);
                                ui.painter().text(
                                    r.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "Enviar",
                                    theme::semibold(13.0),
                                    if ready { palette.on_accent } else { palette.dim },
                                );
                                if ready && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    send = Some(selected.clone());
                                    close = true;
                                }
                            });
                        });
                    }
                });
        });

    egui_ctx.data_mut(|d| {
        d.insert_temp(value_id, (signature, selected));
        d.insert_temp(query_id, query);
        d.insert_temp(focus_id, want_focus);
    });

    // Se cierra al apretar afuera (ni en la caja ni en la lista) o con Escape.
    let outside = egui_ctx.input(|i| i.pointer.any_pressed())
        && egui_ctx
            .pointer_interact_pos()
            .is_some_and(|p| !area.response.rect.contains(p) && !rect.contains(p));
    if outside || egui_ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if close {
        egui_ctx.data_mut(|d| d.insert_temp(open_id, false));
        egui_ctx.request_repaint();
    }
    if let Some(values) = send {
        *clicked = Some((custom_id.to_string(), Some((kind, values))));
    }
}

/// Alto de una fila sin descripción (las de usuarios, roles y canales).
const ITEM_HEIGHT: f32 = 32.0;

/// Una fila de la lista de un menú: guía (avatar, punto de color o ícono),
/// emoji, nombre, descripción y, a la derecha, la marca de elegida (un tilde,
/// o una casilla si se puede elegir más de una).
fn item_row(ui: &mut egui::Ui, palette: &Palette, item: &Item, selected: bool, multi: bool) -> egui::Response {
    let height = if item.description.is_some() { 46.0 } else { ITEM_HEIGHT };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &item.label));
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same(6);
        if response.hovered() {
            ui.painter().rect_filled(rect, radius, palette.surface_hover);
        }
        if selected {
            ui.painter().rect_filled(rect, radius, palette.accent.gamma_multiply(0.18));
        }

        let cy = rect.center().y;
        let mut x = rect.left() + 8.0;
        match &item.lead {
            Lead::None => {}
            Lead::Avatar { url, color, initial } => {
                extra::avatar(ui, egui::pos2(x + 10.0, cy), 10.0, url.as_deref(), *color, initial, palette);
                x += 28.0;
            }
            Lead::Dot(color) => {
                ui.painter().circle_filled(egui::pos2(x + 6.0, cy), 5.0, *color);
                x += 18.0;
            }
            Lead::Icon(icon) => {
                let icon_rect = egui::Rect::from_center_size(egui::pos2(x + 8.0, cy), Vec2::splat(16.0));
                theme::paint_icon(ui, *icon, icon_rect, 16.0, palette.dim);
                x += 24.0;
            }
        }
        if has_emoji(item.emoji) {
            if let Some(emoji) = item.emoji {
                let icon_rect =
                    egui::Rect::from_min_size(egui::pos2(x, cy - EMOJI_SIZE / 2.0), Vec2::splat(EMOJI_SIZE));
                paint_component_emoji(ui, palette, icon_rect, emoji);
            }
            x += EMOJI_SIZE + EMOJI_GAP;
        }

        // El texto no pisa la marca de la derecha.
        let text_clip = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right() - 32.0, rect.bottom()));
        let painter = ui.painter().with_clip_rect(text_clip);
        match item.description {
            Some(description) => {
                painter.text(
                    egui::pos2(x, rect.top() + 7.0),
                    egui::Align2::LEFT_TOP,
                    &item.label,
                    theme::medium(13.5),
                    palette.text,
                );
                painter.text(
                    egui::pos2(x, rect.top() + 25.0),
                    egui::Align2::LEFT_TOP,
                    description,
                    theme::regular(11.5),
                    palette.dim,
                );
            }
            None => {
                painter.text(
                    egui::pos2(x, cy),
                    egui::Align2::LEFT_CENTER,
                    &item.label,
                    theme::medium(13.5),
                    palette.text,
                );
            }
        }

        let mark = egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, cy), Vec2::splat(16.0));
        if multi {
            if selected {
                ui.painter().rect_filled(mark, CornerRadius::same(4), palette.accent);
                theme::paint_icon(ui, Icon::Check, mark, 12.0, palette.on_accent);
            } else {
                ui.painter().rect_stroke(
                    mark,
                    CornerRadius::same(4),
                    Stroke::new(1.5, palette.dim),
                    egui::StrokeKind::Inside,
                );
            }
        } else if selected {
            theme::paint_icon(ui, Icon::Check, mark, 14.0, palette.accent);
        }
    }
    response
}
