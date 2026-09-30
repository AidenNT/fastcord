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

use egui::{Color32, CornerRadius, Frame, Margin, Sense, Stroke, Vec2};

use crate::discord::models::Component;
use crate::lib::data::{ChatMessage, ComponentClick};
use crate::ui::emoji as twemoji;
use crate::ui::theme::{self, Icon, Palette};

/// Alto de un botón (el de Discord mide 32 px).
const BUTTON_HEIGHT: f32 = 32.0;
const BUTTON_PAD_X: f32 = 14.0;
const EMOJI_SIZE: f32 = 16.0;
const EMOJI_GAP: f32 = 6.0;

/// Bit de `flags` de un mensaje efímero (solo lo ve quien lo provocó).
pub const FLAG_EPHEMERAL: u64 = 1 << 6;

/// Dibuja los componentes del mensaje. Devuelve el botón apretado este frame,
/// si hubo alguno.
pub fn show(ui: &mut egui::Ui, palette: &Palette, msg: &ChatMessage) -> Option<ComponentClick> {
    if msg.components.is_empty() {
        return None;
    }
    let mut clicked: Option<String> = None;
    ui.add_space(4.0);
    for component in &msg.components {
        draw(ui, palette, component, &mut clicked);
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
fn draw(ui: &mut egui::Ui, palette: &Palette, component: &Component, clicked: &mut Option<String>) {
    match component.kind {
        // Fila de acciones: los botones van uno al lado del otro, y si no
        // entran, siguen abajo.
        1 => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                for child in &component.components {
                    draw(ui, palette, child, clicked);
                }
            });
            ui.add_space(2.0);
        }
        Component::BUTTON => button(ui, palette, component, clicked),
        // Menús desplegables: caja apagada con el texto de ayuda.
        3 | 5 | 6 | 7 | 8 => select_placeholder(ui, palette, component),
        // Texto suelto (componentes nuevos).
        10 => {
            if let Some(content) = component.content.as_deref().filter(|c| !c.is_empty()) {
                ui.add(
                    egui::Label::new(egui::RichText::new(content).font(theme::regular(13.5)).color(palette.text))
                        .wrap(),
                );
            }
        }
        // Sección: texto a la izquierda y, si hay, un botón (accesorio) a la derecha.
        9 => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                for child in &component.components {
                    draw(ui, palette, child, clicked);
                }
                if let Some(accessory) = component.accessory.as_deref() {
                    draw(ui, palette, accessory, clicked);
                }
            });
        }
        // Contenedor: caja con borde a la izquierda, como un embed.
        17 => {
            Frame::new()
                .fill(palette.surface)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::RADIUS_SMALL))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    for child in &component.components {
                        draw(ui, palette, child, clicked);
                    }
                });
        }
        _ => {}
    }
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

fn button(ui: &mut egui::Ui, palette: &Palette, component: &Component, clicked: &mut Option<String>) {
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

    // Un botón deshabilitado, o uno sin `custom_id` ni URL (mal formado), no
    // reacciona al mouse.
    let usable = !component.disabled && (is_link && component.url.is_some() || component.custom_id.is_some());
    let sense = if usable { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, BUTTON_HEIGHT), sense);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });

    if ui.is_rect_visible(rect) {
        let mut background = if usable && response.hovered() { fill_hover } else { fill };
        let mut foreground = text_color;
        if !usable {
            background = background.gamma_multiply(0.5);
            foreground = foreground.gamma_multiply(0.5);
        }
        ui.painter()
            .rect_filled(rect, CornerRadius::same(theme::RADIUS_SMALL), background);

        let mut x = rect.left() + BUTTON_PAD_X;
        if let Some(emoji) = emoji.filter(|_| has_emoji) {
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
        if let Some(galley) = galley {
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
        .corner_radius(CornerRadius::same(theme::RADIUS_SMALL))
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
