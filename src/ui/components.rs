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
//! * los menús desplegables (3, 5, 6, 7, 8) solo como una caja apagada — todavía
//!   no se pueden usar, y se avisa en el tooltip.
//!
//! Apretar un botón normal NO hace nada acá: [`show`] devuelve un
//! [`ComponentClick`] y quien lo recibe (ver `App::press_component`) le manda
//! la interacción a la app dueña. Un botón de link sí actúa solo: abre la URL.
//!
//! Mientras la app no contesta, el botón apretado muestra un spinner en lugar
//! de su contenido (mismo tamaño que antes) y no se puede volver a apretar.
//! La lista de botones cargando la publica `App` (`data::loading_buttons`).

use egui::{Color32, CornerRadius, Frame, Margin, Sense, Stroke, Vec2};

use crate::discord::models::Component;
use crate::lib::data::{ChatMessage, ComponentClick};
use crate::ui::emoji as twemoji;
use crate::ui::markdown::{self, MentionCtx};
use crate::ui::media;
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

/// Bit de `flags` de un mensaje efímero (solo lo ve quien lo provocó).
pub const FLAG_EPHEMERAL: u64 = 1 << 6;

/// Dibuja los componentes del mensaje. Devuelve el botón apretado este frame,
/// si hubo alguno.
pub fn show(ui: &mut egui::Ui, palette: &Palette, msg: &ChatMessage) -> Option<ComponentClick> {
    if msg.components.is_empty() {
        return None;
    }
    let mut clicked: Option<String> = None;
    let ctx = MentionCtx { mentions: &msg.mentions, channels: None };
    // Botones de este mensaje que ya mandaron su interacción y esperan al bot.
    let loading = crate::lib::data::loading_buttons(ui.ctx(), &msg.id);
    ui.add_space(4.0);
    for component in &msg.components {
        draw(ui, palette, component, &ctx, &loading, &mut clicked);
    }
    clicked.map(|custom_id| ComponentClick {
        message_id: msg.id.clone(),
        channel_id: msg.channel_id.clone(),
        application_id: msg.application_id.clone(),
        flags: msg.flags,
        custom_id,
    })
}

/// Un componente cualquiera (recursivo para filas, contenedores y secciones).
fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    component: &Component,
    ctx: &MentionCtx,
    loading: &[String],
    clicked: &mut Option<String>,
) {
    match component.kind {
        // Fila de acciones: los botones van uno al lado del otro, y si no
        // entran, siguen abajo.
        1 => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                for child in &component.components {
                    draw(ui, palette, child, ctx, loading, clicked);
                }
            });
            ui.add_space(2.0);
        }
        Component::BUTTON => button(ui, palette, component, loading, clicked),
        // Menús desplegables: caja apagada con el texto de ayuda.
        3 | 5 | 6 | 7 | 8 => select_placeholder(ui, palette, component),
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
                            draw(ui, palette, child, ctx, loading, clicked);
                        }
                    });
                    ui.add_space(4.0);
                    draw(ui, palette, thumb, ctx, loading, clicked);
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    for child in &component.components {
                        draw(ui, palette, child, ctx, loading, clicked);
                    }
                    if let Some(accessory) = component.accessory.as_deref() {
                        draw(ui, palette, accessory, ctx, loading, clicked);
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
                .corner_radius(CornerRadius { nw: radius, ne: radius, sw: radius, se: radius })
                .inner_margin(Margin { left: 16, right: 12, top: 8, bottom: 8 })
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(CONTAINER_MAX_W));
                    for child in &component.components {
                        draw(ui, palette, child, ctx, loading, clicked);
                    }
                });
            let r = inner.response.rect;
            let bar = egui::Rect::from_min_max(r.min, egui::pos2(r.min.x + 4.0, r.max.y));
            ui.painter().rect_filled(
                bar,
                CornerRadius { nw: radius, ne: 0, sw: radius, se: 0 },
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
        3 => (Color32::from_rgb(35, 134, 54), Color32::from_rgb(26, 110, 42), Color32::WHITE),
        4 => (palette.danger, palette.danger.gamma_multiply(0.85), Color32::WHITE),
        // 2 (secundario) y 5 (link): el gris neutro de la interfaz.
        _ => (palette.surface_hover, palette.surface_active, palette.text),
    }
}

fn button(
    ui: &mut egui::Ui,
    palette: &Palette,
    component: &Component,
    loading: &[String],
    clicked: &mut Option<String>,
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
            .is_some_and(|id| loading.iter().any(|l| l == id));
    // Un botón cargando, deshabilitado, o uno sin `custom_id` ni URL (mal
    // formado), no reacciona al mouse.
    let usable = !is_loading
        && !component.disabled
        && (is_link && component.url.is_some() || component.custom_id.is_some());
    let sense = if usable { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, BUTTON_HEIGHT), sense);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });

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
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(x + EMOJI_GAP, rect.center().y - 6.0),
                Vec2::splat(12.0),
            );
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
            *clicked = Some(custom_id.clone());
        }
    }
}

/// Emoji personalizado de un botón: mismo criterio que las reacciones
/// (`.gif` si es animado, `.png` si no).
fn paint_custom_emoji(ui: &egui::Ui, palette: &Palette, rect: egui::Rect, id: &str, animated: bool) {
    let ext = if animated { "gif" } else { "png" };
    let url = format!("https://cdn.discordapp.com/emojis/{id}.{ext}?size=48");
    let url = if animated { crate::ui::media::gif_safe_url(&url) } else { url };
    let image = egui::Image::new(crate::ui::anim::source(ui.ctx(), &url))
        .fit_to_exact_size(rect.size())
        .show_loading_spinner(false);
    match image.load_for_size(ui.ctx(), rect.size()) {
        Ok(egui::load::TexturePoll::Ready { .. }) => image.paint_at(ui, rect),
        _ => {
            ui.painter().rect_filled(rect, 3.0, palette.surface_active);
        }
    }
}

/// Un menú desplegable, dibujado apagado (todavía no se puede usar).
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
        .on_hover_text("Los menús desplegables de los bots todavía no se pueden usar en ecord.");
}
