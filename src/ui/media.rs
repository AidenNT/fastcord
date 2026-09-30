//! Adjuntos y embeds de un mensaje, dibujados debajo de su texto:
//!
//! * **Imágenes** (`png/jpg/gif/webp/bmp`): se muestran inline, escaladas a
//!   una caja de 400×300 (o una grilla de 200×200 si son varias). Un `.gif`
//!   se anima. Si el archivo se llama `SPOILER_*` queda tapado hasta que se
//!   clickea.
//! * **Videos**: egui no trae ningún reproductor, así que se dibuja el
//!   fotograma de portada con un botón de "play"; clickearlo abre el video
//!   en el navegador / reproductor del sistema.
//! * **Audio y otros archivos**: tarjeta con nombre + tamaño que abre el
//!   link al clickear.
//! * **Stickers**: imagen de 160×160 sin tarjeta (GIF animado; PNG/APNG
//!   estático). Los Lottie se piden como PNG y, si no hay, queda un
//!   recuadro con el nombre.
//! * **Embeds**: tarjeta con barra de color, autor, título (link), descripción
//!   (con el mismo markdown que los mensajes), campos en columnas, imagen,
//!   miniatura y pie — más los casos especiales de Discord: `image` (link
//!   directo a una imagen: se ve la imagen sola, sin tarjeta), `gifv`
//!   (Tenor/Giphy: portada con la insignia GIF) y `video` (YouTube, etc.:
//!   portada grande con "play").
//!
//! Todas las imágenes reservan su tamaño ANTES de cargar (Discord manda
//! `width`/`height`), así el alto de la fila no cambia cuando llega la
//! textura — importante porque `ui::chat` virtualiza la lista y cachea el
//! alto medido de cada mensaje.

use egui::{Color32, CornerRadius, Frame, Margin, Rect, RichText, Sense, Stroke, Ui, Vec2};

use crate::discord::models::{
    Attachment, AttachmentKind, Embed, EmbedField, EmbedMedia, StickerItem,
};
use crate::ui::markdown::MentionCtx;
use crate::ui::theme::{self, Icon, Palette};
use crate::ui::video_player;

/// Caja máxima de una imagen/video suelto (mismo tope que el cliente real).
const MEDIA_MAX_W: f32 = 400.0;
const MEDIA_MAX_H: f32 = 300.0;
/// Lado máximo de cada imagen cuando un mensaje trae varias.
const GRID_MAX: f32 = 200.0;
const MEDIA_RADIUS: u8 = 6;
const EMBED_MAX_W: f32 = 432.0;
/// Margen izquierdo del embed: deja lugar a la barra de color de 4px.
const EMBED_PAD_LEFT: f32 = 16.0;
const EMBED_PAD_RIGHT: f32 = 12.0;
const THUMB_SIDE: f32 = 80.0;
const FILE_CARD_W: f32 = 320.0;

/// Truco viejo: sufijar `#.gif` a las URLs de GIF para que el loader animado
/// de `egui_extras` las reconozca (detecta un GIF por el final de la URI, y
/// las de Discord terminan en `.gif?ex=…&hm=…`). Ya no se usa: los GIF los
/// anima `ui::anim` (decodifica los cuadros y elige el actual según el
/// reloj) y, mientras tanto, la URL va tal cual a `egui`, que dibuja el
/// primer cuadro. Con `true` vuelve el truco, pero entonces el loader de
/// `egui` decodificaría cada GIF completo por su cuenta, además de `anim`.
const ANIMATE_GIFS: bool = false;

/// Sufija `#.gif` a cualquier URL cuyo *path* termine en `.gif` pero que la
/// URL completa no termine ahí (por un `?query` de por medio, típicamente
/// `?size=…` en avatares/emojis o `?ex=…&hm=…` en adjuntos firmados): el
/// loader animado de `egui_extras` detecta un GIF mirando el final de la
/// URI tal cual, así que ese query de por medio lo deja "congelado" en el
/// primer cuadro si no se lo movemos a un fragmento `#` (que no se manda
/// al servidor, y queda al final de la URI para el loader).
///
/// Pública porque no es solo para adjuntos: también la usan los avatares
/// e íconos de server (`ui::extra::avatar`), los emojis personalizados en
/// el texto de un mensaje (`ui::markdown::custom_emoji`) y las reacciones
/// ya puestas (`ui::chat::reaction_pill_custom`). Ver `ANIMATE_GIFS`.
pub fn gif_safe_url(url: &str) -> String {
    if !ANIMATE_GIFS {
        return url.to_string();
    }
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    if path.ends_with(".gif") && !url.ends_with(".gif") {
        format!("{url}#.gif")
    } else {
        url.to_string()
    }
}

// ---------------------------------------------------------------------
// API pública
// ---------------------------------------------------------------------

/// Dibuja los adjuntos de un mensaje: primero las imágenes, después los
/// videos y por último los archivos/audio.
pub fn show_attachments(ui: &mut Ui, palette: &Palette, attachments: &[Attachment]) {
    if attachments.is_empty() {
        return;
    }
    let max_w = ui.available_width().min(MEDIA_MAX_W).max(80.0);

    let images: Vec<&Attachment> = attachments
        .iter()
        .filter(|a| a.kind() == AttachmentKind::Image)
        .collect();
    match images.len() {
        0 => {}
        1 => {
            ui.add_space(4.0);
            attachment_image(ui, palette, images[0], Vec2::new(max_w, MEDIA_MAX_H));
        }
        _ => {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(4.0);
                for att in &images {
                    attachment_image(ui, palette, att, Vec2::splat(GRID_MAX.min(max_w)));
                }
            });
        }
    }

    for att in attachments {
        match att.kind() {
            AttachmentKind::Image => {}
            AttachmentKind::Video => {
                ui.add_space(4.0);
                attachment_video(ui, palette, att, Vec2::new(max_w, MEDIA_MAX_H));
            }
            AttachmentKind::Audio => {
                ui.add_space(4.0);
                file_card(ui, palette, att, Icon::Music);
            }
            AttachmentKind::File => {
                ui.add_space(4.0);
                file_card(ui, palette, att, Icon::ExternalLink);
            }
        }
    }
}

/// Dibuja los stickers de un mensaje (normalmente uno solo), cada uno en
/// una caja fija de 160×160 como en el cliente real. El tamaño se reserva
/// antes de que cargue la imagen, así el alto de la fila no cambia.
pub fn show_stickers(ui: &mut Ui, palette: &Palette, stickers: &[StickerItem]) {
    if stickers.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(4.0);
        for sticker in stickers {
            if sticker.id.is_empty() {
                continue;
            }
            let side = StickerItem::DISPLAY_SIDE;
            let url = sticker.url();
            let response = media_box(
                ui,
                palette,
                MediaSpec {
                    url: &url,
                    // Los stickers son cuadrados: con las dimensiones
                    // conocidas la caja no salta cuando llega la textura.
                    width: Some(side as u32),
                    height: Some(side as u32),
                    max: Vec2::splat(side),
                    radius: 0,
                    alt: sticker.name.as_str(),
                    format: "png",
                    play: false,
                    badge: None,
                },
            );
            if !sticker.name.is_empty() {
                let _ = response.on_hover_text(sticker.name.as_str());
            }
        }
    });
}

/// Dibuja los embeds de un mensaje, uno debajo del otro.
pub fn show_embeds(ui: &mut Ui, palette: &Palette, embeds: &[Embed]) {
    for embed in embeds {
        match embed.kind.as_deref().unwrap_or("rich") {
            "image" => embed_image(ui, palette, embed),
            "gifv" => embed_gifv(ui, palette, embed),
            _ => {
                if !embed_is_empty(embed) {
                    ui.add_space(4.0);
                    embed_card(ui, palette, embed);
                }
            }
        }
    }
}

/// Discord no muestra el texto de un mensaje cuando es SOLO el link de una
/// imagen/GIF que ya se ve como embed (sería la URL repetida arriba de la
/// imagen). Con cualquier otro tipo de embed (link, video...) el texto
/// se queda, como en el cliente real.
pub fn hides_content(content: &str, embeds: &[Embed]) -> bool {
    let c = content.trim();
    if c.is_empty()
        || c.contains(char::is_whitespace)
        || !(c.starts_with("http://") || c.starts_with("https://"))
    {
        return false;
    }
    !embeds.is_empty()
        && embeds
            .iter()
            .all(|e| matches!(e.kind.as_deref(), Some("image") | Some("gifv")))
        && embeds.iter().any(|e| e.url.as_deref() == Some(c))
}

// ---------------------------------------------------------------------
// Adjuntos
// ---------------------------------------------------------------------

fn attachment_image(ui: &mut Ui, palette: &Palette, att: &Attachment, max: Vec2) {
    // Spoiler: mientras no se lo clickee, solo se dibuja una tapa del
    // mismo tamaño que tendría la imagen (no se pide nada a la red).
    if att.is_spoiler() {
        let key = egui::Id::new(("ecord_spoiler", att.url.as_str()));
        let revealed = ui
            .ctx()
            .memory(|m| m.data.get_temp::<bool>(key).unwrap_or(false));
        if !revealed {
            let size = box_size(att.width, att.height, max);
            if spoiler_cover(ui, palette, size).clicked() {
                ui.ctx().memory_mut(|m| m.data.insert_temp(key, true));
            }
            return;
        }
    }

    let response = media_box(
        ui,
        palette,
        MediaSpec {
            url: att.display_url(),
            width: att.width,
            height: att.height,
            max,
            radius: MEDIA_RADIUS,
            alt: "No se pudo cargar la imagen",
            format: "webp",
            play: false,
            badge: None,
        },
    );
    if response.clicked() {
        open(ui, &att.url);
    }
}

fn attachment_video(ui: &mut Ui, palette: &Palette, att: &Attachment, max: Vec2) {
    let url = att.display_url();
    let size = box_size(att.width, att.height, max);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    // El proxy de Discord devuelve un fotograma del video si se le pide
    // como imagen (`format=jpeg`): esa es la portada mientras el video
    // todavía no se arrancó. Si el proxy no lo soporta, la carga falla y
    // queda el recuadro con el nombre del archivo — el click para
    // reproducir acá sigue funcionando igual, con o sin portada.
    let poster_url = resolve_url(url, Some(size), "jpeg");
    let showing_poster = video_player::show(ui, url, rect, |ui| {
        paint_remote_image(ui, palette, rect, &poster_url, MEDIA_RADIUS, att.filename.as_str());
        paint_play(ui, rect);
    });
    // Mientras se muestra la portada (todavía no se arrancó el player, o
    // arrancarlo falló), dejamos además un botón para abrirlo en el
    // reproductor del sistema (video grande, subtítulos del SO, lo que
    // sea) sin tener que reproducirlo acá adentro. Reproduciendo, el
    // botón de esa misma esquina ya lo dibuja `video_player` (es el de
    // "cerrar").
    if showing_poster {
        if video_player::corner_button(ui, rect, Icon::ExternalLink, ("video_ext", url))
            .on_hover_text(format!("{} — abrir en el reproductor del sistema", att.filename))
            .clicked()
        {
            open(ui, &att.url);
        }
    }
}

fn file_card(ui: &mut Ui, palette: &Palette, att: &Attachment, icon: Icon) {
    let width = ui.available_width().min(FILE_CARD_W);
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS_SMALL))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width((width - 22.0).max(60.0));
            ui.horizontal(|ui| {
                theme::icon(ui, icon, 18.0, palette.dim);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let name = if att.filename.is_empty() { "archivo" } else { att.filename.as_str() };
                    if theme::link(ui, name, theme::semibold(12.5), palette.accent).clicked() {
                        open(ui, &att.url);
                    }
                    theme::text(ui, human_size(att.size), theme::regular(11.0), palette.dim);
                });
            });
        });
}

/// Tapa de un adjunto `SPOILER_*` todavía sin revelar.
fn spoiler_cover(ui: &mut Ui, palette: &Palette, size: Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(MEDIA_RADIUS), palette.surface_active);
        let galley = ui
            .painter()
            .layout_no_wrap("SPOILER".to_string(), theme::semibold(11.0), Color32::WHITE);
        let pad = Vec2::new(10.0, 6.0);
        let pill = Rect::from_center_size(rect.center(), galley.size() + pad * 2.0);
        ui.painter()
            .rect_filled(pill, CornerRadius::same(12), Color32::from_black_alpha(190));
        ui.painter().galley(pill.min + pad, galley, Color32::WHITE);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------
// Embeds
// ---------------------------------------------------------------------

/// `type: "image"` — el mensaje era un link directo a una imagen. Discord
/// guarda la imagen en `thumbnail` (a veces en `image`) y el cliente la
/// muestra sola, sin tarjeta.
fn embed_image(ui: &mut Ui, palette: &Palette, e: &Embed) {
    let Some(m) = e
        .thumbnail
        .as_ref()
        .filter(|m| m.has_url())
        .or(e.image.as_ref().filter(|m| m.has_url()))
    else {
        return;
    };
    ui.add_space(4.0);
    let max = Vec2::new(ui.available_width().min(MEDIA_MAX_W).max(80.0), MEDIA_MAX_H);
    let response = media_box(
        ui,
        palette,
        MediaSpec {
            url: m.display_url(),
            width: m.width,
            height: m.height,
            max,
            radius: MEDIA_RADIUS,
            alt: "No se pudo cargar la imagen",
            format: "webp",
            play: false,
            badge: None,
        },
    );
    if response.clicked() {
        open(ui, e.url.as_deref().unwrap_or(m.url.as_str()));
    }
}

/// `type: "gifv"` (Tenor, Giphy...). Discord manda un mp4 corto en `video`
/// y una portada estática en `thumbnail`. Se dibuja primero la portada con
/// la insignia "GIF" (que es lo que se ve mientras carga, si el video no se
/// puede reproducir, o si ya hay demasiados GIF reproduciéndose) y encima el
/// video en bucle y mudo (`video_player::show_looping`). Al clickear se abre
/// el link original.
fn embed_gifv(ui: &mut Ui, palette: &Palette, e: &Embed) {
    let Some(m) = e
        .thumbnail
        .as_ref()
        .filter(|m| m.has_url())
        .or(e.image.as_ref().filter(|m| m.has_url()))
    else {
        return;
    };
    ui.add_space(4.0);
    let max = Vec2::new(ui.available_width().min(MEDIA_MAX_W).max(80.0), MEDIA_MAX_H);
    let response = media_box(
        ui,
        palette,
        MediaSpec {
            url: m.display_url(),
            width: m.width,
            height: m.height,
            max,
            radius: MEDIA_RADIUS,
            alt: "GIF",
            format: "webp",
            play: false,
            badge: Some("GIF"),
        },
    );
    if let Some(video) = e.video.as_ref().filter(|v| v.has_url())
        && ui.is_rect_visible(response.rect)
        && video_player::show_looping(ui, video.display_url(), response.rect, MEDIA_RADIUS)
    {
        // El video tapó la insignia que pintó `media_box`: se repinta.
        paint_badge(ui, response.rect, "GIF");
    }
    if response.clicked() {
        open(ui, e.url.as_deref().unwrap_or(m.url.as_str()));
    }
}

/// Tarjeta de un embed `rich` / `article` / `link` / `video`.
fn embed_card(ui: &mut Ui, palette: &Palette, e: &Embed) {
    let kind = e.kind.as_deref().unwrap_or("rich");
    let bar = e.color.map(rgb_from_int).unwrap_or(palette.outline);
    let width = ui.available_width().min(EMBED_MAX_W);
    let inner_w = (width - EMBED_PAD_LEFT - EMBED_PAD_RIGHT - 2.0).max(80.0);

    // Imagen grande: la `image` del embed o, en los de tipo `video`
    // (YouTube...), su portada. Si no hay, una miniatura chica a la
    // derecha del texto (`thumbnail` de un link/article/rich).
    let large: Option<&EmbedMedia> = e.image.as_ref().filter(|m| m.has_url()).or_else(|| {
        if kind == "video" {
            e.thumbnail.as_ref().filter(|m| m.has_url())
        } else {
            None
        }
    });
    let small: Option<&EmbedMedia> = if large.is_none() && kind != "video" {
        e.thumbnail.as_ref().filter(|m| m.has_url())
    } else {
        None
    };

    let out = Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS_SMALL))
        .inner_margin(Margin {
            left: EMBED_PAD_LEFT as i8,
            right: EMBED_PAD_RIGHT as i8,
            top: 10,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.set_width(inner_w);
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);

            if let Some(thumb) = small {
                let text_w = (inner_w - THUMB_SIDE - 12.0).max(60.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    ui.vertical(|ui| {
                        ui.set_width(text_w);
                        card_text(ui, palette, e, kind, text_w);
                    });
                    let response = media_box(
                        ui,
                        palette,
                        MediaSpec {
                            url: thumb.display_url(),
                            width: thumb.width,
                            height: thumb.height,
                            max: Vec2::splat(THUMB_SIDE),
                            radius: 4,
                            alt: "",
                            format: "webp",
                            play: false,
                            badge: None,
                        },
                    );
                    if response.clicked() {
                        open(ui, e.url.as_deref().unwrap_or(thumb.url.as_str()));
                    }
                });
            } else {
                card_text(ui, palette, e, kind, inner_w);
            }

            if let Some(img) = large {
                ui.add_space(4.0);
                let response = media_box(
                    ui,
                    palette,
                    MediaSpec {
                        url: img.display_url(),
                        width: img.width,
                        height: img.height,
                        max: Vec2::new(inner_w, MEDIA_MAX_H),
                        radius: 4,
                        alt: "",
                        format: "webp",
                        play: kind == "video",
                        badge: None,
                    },
                );
                if response.clicked() {
                    // En un video la portada abre la PÁGINA (YouTube...);
                    // en el resto, la imagen a tamaño completo.
                    let target = if kind == "video" {
                        e.url.as_deref().unwrap_or(img.url.as_str())
                    } else {
                        img.url.as_str()
                    };
                    open(ui, target);
                }
            }

            card_footer(ui, palette, e);
        });

    // Barra de color pegada al borde izquierdo, pintada DESPUÉS de la
    // tarjeta para conocer su alto final.
    let r = out.response.rect;
    ui.painter().rect_filled(
        Rect::from_min_size(r.min, Vec2::new(4.0, r.height())),
        CornerRadius {
            nw: theme::RADIUS_SMALL,
            sw: theme::RADIUS_SMALL,
            ne: 0,
            se: 0,
        },
        bar,
    );
}

/// Proveedor, autor, título, descripción y campos de un embed.
fn card_text(ui: &mut Ui, palette: &Palette, e: &Embed, kind: &str, width: f32) {
    if kind != "rich" {
        if let Some(name) = e
            .provider
            .as_ref()
            .and_then(|p| p.name.as_deref())
            .filter(|n| !n.is_empty())
        {
            theme::text(ui, name, theme::regular(11.0), palette.dim);
        }
    }

    if let Some(author) = e.author.as_ref().filter(|a| !a.name.is_empty()) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if let Some(icon) = author
                .proxy_icon_url
                .as_deref()
                .or(author.icon_url.as_deref())
                .filter(|u| !u.is_empty())
            {
                small_icon(ui, palette, icon, 20.0);
            }
            match author.url.as_deref().filter(|u| !u.is_empty()) {
                Some(url) => {
                    if theme::link(ui, author.name.as_str(), theme::semibold(12.0), palette.text).clicked() {
                        open(ui, url);
                    }
                }
                None => {
                    theme::text(ui, author.name.as_str(), theme::semibold(12.0), palette.text);
                }
            }
        });
    }

    if let Some(title) = e.title.as_deref().filter(|t| !t.is_empty()) {
        let link = e.url.as_deref().filter(|u| !u.is_empty());
        let color = if link.is_some() { palette.accent } else { palette.text };
        // Los títulos pueden ser largos: a diferencia de `theme::link`
        // (una sola línea con "…"), acá el texto se parte en varias líneas.
        let label = egui::Label::new(RichText::new(title).font(theme::semibold(14.0)).color(color))
            .wrap_mode(egui::TextWrapMode::Wrap)
            .selectable(false)
            .sense(if link.is_some() { Sense::click() } else { Sense::hover() });
        let response = ui.add(label);
        if let Some(url) = link {
            if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                open(ui, url);
            }
        }
    }

    if let Some(desc) = e.description.as_deref().filter(|d| !d.trim().is_empty()) {
        crate::ui::markdown::show(ui, palette, desc, 12.5, palette.text, &MentionCtx::none());
    }

    embed_fields(ui, palette, &e.fields, width);
}

/// Campos del embed. Los `inline` consecutivos se acomodan en columnas
/// (hasta 3 por fila, como en Discord); los demás ocupan una fila entera.
fn embed_fields(ui: &mut Ui, palette: &Palette, fields: &[EmbedField], width: f32) {
    let mut i = 0;
    while i < fields.len() {
        if fields[i].inline {
            let mut j = i;
            while j < fields.len() && fields[j].inline && j - i < 3 {
                j += 1;
            }
            let row = &fields[i..j];
            let gap = 8.0;
            let n = row.len() as f32;
            let col_w = ((width - gap * (n - 1.0)) / n).max(40.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for field in row {
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        embed_field(ui, palette, field);
                    });
                }
            });
            i = j;
        } else {
            embed_field(ui, palette, &fields[i]);
            i += 1;
        }
    }
}

fn embed_field(ui: &mut Ui, palette: &Palette, field: &EmbedField) {
    ui.spacing_mut().item_spacing.y = 1.0;
    if !field.name.is_empty() {
        theme::text(ui, field.name.as_str(), theme::semibold(12.0), palette.text);
    }
    if !field.value.trim().is_empty() {
        crate::ui::markdown::show(ui, palette, &field.value, 12.0, palette.text, &MentionCtx::none());
    }
}

fn card_footer(ui: &mut Ui, palette: &Palette, e: &Embed) {
    let footer_text = e
        .footer
        .as_ref()
        .map(|f| f.text.as_str())
        .filter(|t| !t.is_empty());
    let timestamp = e
        .timestamp
        .as_deref()
        .map(format_embed_timestamp)
        .filter(|t| !t.is_empty());
    let icon = e
        .footer
        .as_ref()
        .and_then(|f| f.proxy_icon_url.as_deref().or(f.icon_url.as_deref()))
        .filter(|u| !u.is_empty());

    let text = match (footer_text, timestamp) {
        (Some(f), Some(t)) => format!("{f} • {t}"),
        (Some(f), None) => f.to_string(),
        (None, Some(t)) => t,
        (None, None) => return,
    };
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if let Some(icon) = icon {
            small_icon(ui, palette, icon, 16.0);
        }
        theme::text(ui, text, theme::regular(11.0), palette.dim);
    });
}

/// Un embed sin nada visible (algunos bots mandan `{}`): no se dibuja.
fn embed_is_empty(e: &Embed) -> bool {
    e.title.as_deref().unwrap_or("").is_empty()
        && e.description.as_deref().unwrap_or("").trim().is_empty()
        && e.author.is_none()
        && e.fields.is_empty()
        && e.footer.is_none()
        && e.timestamp.is_none()
        && !e.image.as_ref().is_some_and(|m| m.has_url())
        && !e.thumbnail.as_ref().is_some_and(|m| m.has_url())
}

// ---------------------------------------------------------------------
// Imágenes remotas
// ---------------------------------------------------------------------

/// Todo lo que hace falta para dibujar una imagen/portada remota.
struct MediaSpec<'a> {
    url: &'a str,
    /// Dimensiones originales si la API las mandó (casi siempre): con
    /// ellas el tamaño en pantalla se conoce antes de cargar la imagen.
    width: Option<u32>,
    height: Option<u32>,
    /// Caja máxima; la imagen se achica para entrar, sin deformarse.
    max: Vec2,
    radius: u8,
    /// Texto del recuadro si la imagen no carga ("" = ninguno).
    alt: &'a str,
    /// Formato pedido al proxy de Discord al redimensionar.
    format: &'a str,
    /// Botón de "play" encima (videos).
    play: bool,
    /// Insignia abajo a la izquierda (p. ej. "GIF").
    badge: Option<&'a str>,
}

/// Reserva el rect (con el tamaño ya calculado), dibuja la imagen adentro y
/// devuelve la `Response` para que quien llama decida qué hace el click.
fn media_box(ui: &mut Ui, palette: &Palette, spec: MediaSpec) -> egui::Response {
    let dims = match (spec.width, spec.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some((w as f32, h as f32)),
        _ => None,
    };
    let size = match dims {
        Some((w, h)) => fit_size(w, h, spec.max, false),
        None => {
            // Sin dimensiones de la API: si la imagen ya está cargada
            // usamos su tamaño real; si no, una caja 16:9 provisoria.
            let probe = resolve_url(spec.url, None, spec.format);
            let approx = Rect::from_min_size(ui.cursor().min, spec.max);
            let natural = if ui.is_rect_visible(approx) {
                natural_size(ui.ctx(), &probe, spec.max)
            } else {
                None
            };
            match natural {
                Some(n) => fit_size(n.x, n.y, spec.max, false),
                None => fit_size(16.0, 9.0, spec.max, true),
            }
        }
    };
    // Solo se pide redimensionada si conocemos el tamaño de destino.
    let url = resolve_url(spec.url, dims.map(|_| size), spec.format);

    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        paint_remote_image(ui, palette, rect, &url, spec.radius, spec.alt);
        if let Some(text) = spec.badge {
            paint_badge(ui, rect, text);
        }
        if spec.play {
            paint_play(ui, rect);
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Pinta la imagen en `rect` (que ya tiene la misma proporción que ella).
/// Mientras carga —o si falla— queda un recuadro liso con el texto `alt`.
fn paint_remote_image(ui: &mut Ui, palette: &Palette, rect: Rect, url: &str, radius: u8, alt: &str) {
    let corner = CornerRadius::same(radius);
    // `anim::source` devuelve el cuadro actual si es un GIF ya decodificado
    // (y pide el repaint del siguiente); si no, la URL, como siempre.
    let image = egui::Image::new(crate::ui::anim::source(ui.ctx(), url))
        .corner_radius(corner)
        .fit_to_exact_size(rect.size())
        .show_loading_spinner(false);
    match image.load_for_size(ui.ctx(), rect.size()) {
        Ok(egui::load::TexturePoll::Ready { .. }) => image.paint_at(ui, rect),
        Ok(_) => {
            ui.painter().rect_filled(rect, corner, palette.surface_hover);
        }
        Err(_) => {
            ui.painter().rect_filled(rect, corner, palette.surface_hover);
            if !alt.is_empty() && rect.width() > 60.0 && rect.height() > 24.0 {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    alt,
                    theme::regular(11.5),
                    palette.dim,
                );
            }
        }
    }
}

/// Ícono chico y redondo (avatar de autor/pie de un embed).
fn small_icon(ui: &mut Ui, palette: &Palette, url: &str, side: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    if ui.is_rect_visible(rect) {
        let url = resolve_url(url, Some(Vec2::splat(side)), "webp");
        paint_remote_image(ui, palette, rect, &url, (side / 2.0) as u8, "");
    }
}

fn natural_size(ctx: &egui::Context, url: &str, hint: Vec2) -> Option<Vec2> {
    match egui::Image::new(url.to_owned()).load_for_size(ctx, hint) {
        Ok(egui::load::TexturePoll::Ready { texture }) => Some(texture.size),
        _ => None,
    }
}

fn paint_play(ui: &mut Ui, rect: Rect) {
    let center = rect.center();
    let radius = (rect.width().min(rect.height()) / 3.0).min(22.0);
    ui.painter()
        .circle_filled(center, radius, Color32::from_black_alpha(160));
    theme::paint_icon(
        ui,
        Icon::PlayFilled,
        Rect::from_center_size(center, Vec2::splat(radius * 2.0)),
        radius,
        Color32::WHITE,
    );
}

fn paint_badge(ui: &mut Ui, rect: Rect, text: &str) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), theme::semibold(10.0), Color32::WHITE);
    let pad = Vec2::new(6.0, 3.0);
    let size = galley.size() + pad * 2.0;
    let badge = Rect::from_min_size(
        egui::pos2(rect.left() + 6.0, rect.bottom() - 6.0 - size.y),
        size,
    );
    ui.painter()
        .rect_filled(badge, CornerRadius::same(4), Color32::from_black_alpha(170));
    ui.painter().galley(badge.min + pad, galley, Color32::WHITE);
}

// ---------------------------------------------------------------------
// Helpers puros
// ---------------------------------------------------------------------

/// Abre un link en el navegador del sistema. Solo `http(s)`: los datos
/// vienen de mensajes ajenos, no corresponde lanzar `file:` ni esquemas
/// raros a partir de un click.
fn open(ui: &Ui, url: &str) {
    if url.starts_with("https://") || url.starts_with("http://") {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

/// Escala `(w, h)` para que entre en `max` manteniendo la proporción.
/// Con `upscale == false` nunca agranda (una imagen chica queda chica).
fn fit_size(w: f32, h: f32, max: Vec2, upscale: bool) -> Vec2 {
    let mut scale = (max.x / w).min(max.y / h);
    if !upscale {
        scale = scale.min(1.0);
    }
    Vec2::new((w * scale).floor().max(1.0), (h * scale).floor().max(1.0))
}

/// Tamaño de la caja de un adjunto: con dimensiones, escalado; sin ellas,
/// una caja 16:9.
fn box_size(width: Option<u32>, height: Option<u32>, max: Vec2) -> Vec2 {
    match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => fit_size(w as f32, h as f32, max, false),
        _ => fit_size(16.0, 9.0, max, true),
    }
}

/// URL final con la que se le pide la imagen a egui:
///
/// * GIF: sin tocar (redimensionar mataría la animación); ver `ANIMATE_GIFS`.
/// * URL de `*.discordapp.net` y se conoce el tamaño de destino: se pide
///   redimensionada (x2 por pantallas HiDPI) al proxy de medios, en vez
///   de bajar y decodificar un PNG de varios MB para mostrarlo a 400px.
/// * Cualquier otra: tal cual.
fn resolve_url(url: &str, size: Option<Vec2>, format: &str) -> String {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    if path.ends_with(".gif") {
        return gif_safe_url(url);
    }
    match size {
        Some(size) if url.contains("discordapp.net") && !url.contains("width=") => {
            let sep = if url.contains('?') { '&' } else { '?' };
            let w = (size.x * 2.0).ceil().max(1.0) as u32;
            let h = (size.y * 2.0).ceil().max(1.0) as u32;
            format!("{url}{sep}format={format}&width={w}&height={h}")
        }
        _ => url.to_string(),
    }
}

fn rgb_from_int(c: u32) -> Color32 {
    Color32::from_rgb(((c >> 16) & 0xff) as u8, ((c >> 8) & 0xff) as u8, (c & 0xff) as u8)
}

fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

/// `2026-09-19T04:41:00.000000+00:00` -> `2026-09-19 04:41` (hora del
/// timestamp tal cual viene, sin convertir a zona local).
fn format_embed_timestamp(ts: &str) -> String {
    match (ts.get(0..10), ts.get(11..16)) {
        (Some(date), Some(time)) => format!("{date} {time}"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_size_keeps_aspect_and_never_upscales() {
        let max = Vec2::new(400.0, 300.0);
        assert_eq!(fit_size(800.0, 600.0, max, false), Vec2::new(400.0, 300.0));
        assert_eq!(fit_size(1000.0, 250.0, max, false), Vec2::new(400.0, 100.0));
        assert_eq!(fit_size(100.0, 50.0, max, false), Vec2::new(100.0, 50.0));
        assert_eq!(fit_size(16.0, 9.0, max, true), Vec2::new(400.0, 225.0));
    }

    #[test]
    fn resolve_url_resizes_only_discord_proxy_non_gif() {
        let size = Some(Vec2::new(400.0, 300.0));
        let u = "https://media.discordapp.net/attachments/1/2/a.png?ex=1&hm=2";
        assert_eq!(
            resolve_url(u, size, "webp"),
            format!("{u}&format=webp&width=800&height=600")
        );
        // sin query
        assert_eq!(
            resolve_url("https://media.discordapp.net/x/a.png", size, "webp"),
            "https://media.discordapp.net/x/a.png?format=webp&width=800&height=600"
        );
        // ya trae width=: no se toca
        let w = "https://images-ext-1.discordapp.net/external/h/https/x.com/a.png?width=100&height=50";
        assert_eq!(resolve_url(w, size, "webp"), w);
        // otro host: no se toca
        assert_eq!(resolve_url("https://example.com/a.png", size, "webp"), "https://example.com/a.png");
        // sin tamaño de destino: no se toca
        assert_eq!(resolve_url(u, None, "webp"), u);
    }

    #[test]
    fn resolve_url_gif_keeps_original_url() {
        let size = Some(Vec2::new(400.0, 300.0));
        let g = "https://media.discordapp.net/attachments/1/2/a.gif?ex=1&hm=2";
        assert_eq!(resolve_url(g, size, "webp"), g);
        assert_eq!(
            resolve_url("https://media.tenor.com/x/a.gif", size, "webp"),
            "https://media.tenor.com/x/a.gif"
        );
    }

    #[test]
    fn hides_content_only_for_bare_image_links() {
        let mk = |kind: &str, url: &str| Embed {
            kind: Some(kind.to_string()),
            url: Some(url.to_string()),
            ..Default::default()
        };
        let u = "https://i.imgur.com/a.png";
        assert!(hides_content(u, &[mk("image", u)]));
        assert!(hides_content(&format!("  {u}\n"), &[mk("gifv", u)]));
        assert!(!hides_content(u, &[mk("video", u)]));
        assert!(!hides_content(&format!("mirá {u}"), &[mk("image", u)]));
        assert!(!hides_content(u, &[]));
    }

    #[test]
    fn gif_safe_url_is_identity_while_animate_gifs_is_off() {
        // `ANIMATE_GIFS == false`: las URLs no se tocan (ver la constante).
        assert_eq!(
            gif_safe_url("https://cdn.discordapp.com/avatars/1/a_hash.gif?size=128"),
            "https://cdn.discordapp.com/avatars/1/a_hash.gif?size=128"
        );
        assert_eq!(
            gif_safe_url("https://cdn.discordapp.com/emojis/1.gif?size=48"),
            "https://cdn.discordapp.com/emojis/1.gif?size=48"
        );
        // Ya termina en `.gif` sin nada después.
        assert_eq!(
            gif_safe_url("https://media.tenor.com/x/a.gif"),
            "https://media.tenor.com/x/a.gif"
        );
        // No es un gif: no se toca.
        assert_eq!(
            gif_safe_url("https://cdn.discordapp.com/avatars/1/hash.png?size=128"),
            "https://cdn.discordapp.com/avatars/1/hash.png?size=128"
        );
    }

    #[test]
    fn small_helpers() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(rgb_from_int(0xff8000), Color32::from_rgb(255, 128, 0));
        assert_eq!(format_embed_timestamp("2026-09-19T04:41:00.000000+00:00"), "2026-09-19 04:41");
        assert_eq!(format_embed_timestamp("basura"), "");
    }
}
