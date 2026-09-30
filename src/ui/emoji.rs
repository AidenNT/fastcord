//! Emojis Unicode A COLOR, en dos estilos ([`Set`]):
//! * `Set::Twemoji` (el de Discord): mensajes, reacciones, picker y nombres
//!   de personas. Es el que usan por defecto todas las funciones.
//! * `Set::Fluent` (el arte de Windows 11): nombres de canales. Si Fluent no
//!   tiene un emoji (banderas, algunas familias) se cae a Twemoji.
//!
//! ## Por qué existe
//! `epaint` (el motor de texto de egui) dibuja los glifos de una fuente como
//! máscaras de cobertura de UN solo color: no soporta fuentes de emoji a
//! color (COLR/CBDT/sbix). Por eso `NotoEmoji.ttf` sale siempre en
//! blanco y negro, no importa qué tinta le pongas al texto.
//!
//! La solución es la misma que usa Discord: NO dibujar el emoji como texto,
//! sino como una IMAGEN. Este módulo:
//!
//! 1. parte un `&str` en tramos de texto normal y emojis ([`split`]);
//! 2. pide la imagen de cada emoji al CDN (SVG, nítido a cualquier tamaño)
//!    con el mismo loader HTTP de `egui_extras` que ya baja los avatares y
//!    los emojis personalizados: primero Fluent y, si no lo tiene (banderas,
//!    algunas familias, emojis nuevos), Twemoji;
//! 3. ofrece helpers para dibujarlos: en línea dentro de un
//!    `horizontal_wrapped` ([`inline`]), en una línea de texto con
//!    truncado ([`line`], [`paint_line`], [`paint_line_top`]) o en un
//!    rectángulo fijo ([`paint`], para reacciones y el picker).
//!
//! Si ningún set tiene la imagen (o no hay red) se cae a la fuente monocromática de siempre: nunca queda un hueco.
//! Mientras la imagen carga se reserva su lugar exacto, así el texto no
//! "salta" cuando llega.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use egui::load::TexturePoll;
use egui::{Align2, Color32, Context, FontId, Galley, Image, Pos2, Rect, Response, Sense, Ui, Vec2};

/// Fluent Emoji "Color" de Microsoft: es el MISMO arte que muestra Windows 11
/// (Segoe UI Emoji). Repo comunitario que los renombra por codepoint
/// (`1f44b_color.svg`), servido por jsDelivr. Licencia MIT (microsoft/fluentui-emoji).
const FLUENT_BASE: &str = "https://cdn.jsdelivr.net/gh/shuding/fluentui-emoji-unicode/assets";
/// Estilo de Fluent: "color" (el de Windows 11), "flat" o "high-contrast".
const FLUENT_STYLE: &str = "color";
/// Carpeta base de los assets de Twemoji (fork mantenido `jdecked/twemoji`).
/// Se usa como respaldo cuando Fluent no tiene el emoji (banderas, algunas
/// familias ZWJ y los emojis más nuevos).
const TWEMOJI_BASE: &str = "https://cdn.jsdelivr.net/gh/jdecked/twemoji@latest/assets";
/// `true` = SVG de Twemoji (nítido a cualquier escala). `false` = PNG 72x72.
const TWEMOJI_SVG: bool = true;
/// Tamaño del emoji relativo al tamaño de fuente del texto que lo rodea
/// (mismo factor que ya usaban los emojis personalizados en `markdown.rs`).
const EMOJI_SCALE: f32 = 1.35;
const ELLIPSIS: &str = "…";

/// Qué estilo de emoji dibujar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Set {
    /// Twemoji: el look de Discord.
    #[default]
    Twemoji,
    /// Fluent Emoji "color": el look de Windows 11 (con Twemoji de respaldo).
    Fluent,
}

// ---------------------------------------------------------------------
// Partir un texto en tramos "texto" / "emoji"
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seg<'a> {
    Text(&'a str),
    /// Un emoji COMPLETO (bandera, familia ZWJ, tono de piel, keycap...).
    Emoji(&'a str),
}

/// ¿`text` tiene al menos un emoji? (rápido: los textos ASCII salen al toque)
pub fn contains_emoji(text: &str) -> bool {
    !text.is_ascii() && split(text).iter().any(|s| matches!(s, Seg::Emoji(_)))
}

/// Parte `text` en tramos de texto y emojis, en orden y sin perder nada:
/// concatenar todos los tramos devuelve exactamente `text`.
pub fn split(text: &str) -> Vec<Seg<'_>> {
    let mut out = Vec::new();
    if text.is_empty() {
        return out;
    }
    if text.is_ascii() {
        out.push(Seg::Text(text));
        return out;
    }
    let cs: Vec<(usize, char)> = text.char_indices().collect();
    let byte_at = |i: usize| cs.get(i).map(|&(b, _)| b).unwrap_or(text.len());

    let mut i = 0;
    let mut text_from = 0;
    while i < cs.len() {
        if let Some(end) = emoji_end(&cs, i) {
            if text_from < i {
                out.push(Seg::Text(&text[byte_at(text_from)..byte_at(i)]));
            }
            out.push(Seg::Emoji(&text[byte_at(i)..byte_at(end)]));
            i = end;
            text_from = end;
        } else {
            i += 1;
        }
    }
    if text_from < cs.len() {
        out.push(Seg::Text(&text[byte_at(text_from)..]));
    }
    out
}

/// Si en `start` empieza un emoji, devuelve el índice (de carácter) donde
/// termina. Cubre: emojis simples, con selector de variación (FE0F), con
/// tono de piel, secuencias ZWJ (👨‍👩‍👧), banderas (dos indicadores
/// regionales), banderas de subregión (tags) y keycaps (1️⃣).
fn emoji_end(cs: &[(usize, char)], start: usize) -> Option<usize> {
    let at = |k: usize| cs.get(k).map(|&(_, ch)| ch);
    let c = cs[start].1;

    let mut i;
    if is_regional_indicator(c) {
        i = start + 1;
        if at(i).is_some_and(is_regional_indicator) {
            i += 1;
        }
    } else if matches!(c, '0'..='9' | '#' | '*') {
        // Keycap: dígito/#/* + (FE0F) + 20E3. Un dígito suelto NO es emoji.
        let mut k = start + 1;
        if at(k) == Some('\u{FE0F}') {
            k += 1;
        }
        return if at(k) == Some('\u{20E3}') { Some(k + 1) } else { None };
    } else if is_emoji_presentation(c as u32)
        || (at(start + 1) == Some('\u{FE0F}') && is_text_default_emoji(c as u32))
        || c == '\u{2764}'
    {
        i = start + 1;
    } else {
        return None;
    }

    loop {
        match at(i) {
            // selector de variación, tono de piel, tags de banderas de subregión
            Some('\u{FE0F}') | Some('\u{1F3FB}'..='\u{1F3FF}') | Some('\u{E0020}'..='\u{E007F}') => i += 1,
            // ZWJ + otro emoji: sigue la misma secuencia
            Some('\u{200D}') => match at(i + 1) {
                Some(n) if is_emoji_presentation(n as u32) || is_text_default_emoji(n as u32) => i += 2,
                _ => break,
            },
            _ => break,
        }
    }
    Some(i)
}

fn is_regional_indicator(c: char) -> bool {
    matches!(c, '\u{1F1E6}'..='\u{1F1FF}')
}

/// Emojis que se muestran como emoji por sí solos (Emoji_Presentation=Yes).
fn is_emoji_presentation(u: u32) -> bool {
    matches!(u,
        0x1F300..=0x1F64F
        | 0x1F680..=0x1F6FF
        | 0x1F7E0..=0x1F7EB
        | 0x1F7F0
        | 0x1F90C..=0x1F9FF
        | 0x1FA70..=0x1FAFF
        | 0x1F004
        | 0x1F0CF
        | 0x1F18E
        | 0x1F191..=0x1F19A
        | 0x1F201
        | 0x1F21A
        | 0x1F22F
        | 0x1F232..=0x1F236
        | 0x1F238..=0x1F23A
        | 0x1F250..=0x1F251
        | 0x231A..=0x231B
        | 0x23E9..=0x23EC
        | 0x23F0
        | 0x23F3
        | 0x25FD..=0x25FE
        | 0x2614..=0x2615
        | 0x2648..=0x2653
        | 0x267F
        | 0x2693
        | 0x26A1
        | 0x26AA..=0x26AB
        | 0x26BD..=0x26BE
        | 0x26C4..=0x26C5
        | 0x26CE
        | 0x26D4
        | 0x26EA
        | 0x26F2..=0x26F3
        | 0x26F5
        | 0x26FA
        | 0x26FD
        | 0x2705
        | 0x270A..=0x270B
        | 0x2728
        | 0x274C
        | 0x274E
        | 0x2753..=0x2755
        | 0x2757
        | 0x2795..=0x2797
        | 0x27B0
        | 0x27BF
        | 0x2B1B..=0x2B1C
        | 0x2B50
        | 0x2B55
    )
}

/// Emojis "de texto por defecto": solo cuentan como emoji si les sigue el
/// selector FE0F (como ❤️, ☀️, ✈️, ©️...), igual que en Discord.
fn is_text_default_emoji(u: u32) -> bool {
    matches!(u,
        0xA9 | 0xAE | 0x203C | 0x2049 | 0x2122 | 0x2139
        | 0x2194..=0x2199
        | 0x21A9..=0x21AA
        | 0x2328 | 0x23CF
        | 0x23ED..=0x23EF
        | 0x23F1..=0x23F2
        | 0x23F8..=0x23FA
        | 0x24C2
        | 0x25AA..=0x25AB
        | 0x25B6 | 0x25C0
        | 0x25FB..=0x25FC
        | 0x2600..=0x2604
        | 0x260E | 0x2611 | 0x2618 | 0x261D | 0x2620
        | 0x2622..=0x2623
        | 0x2626 | 0x262A
        | 0x262E..=0x262F
        | 0x2638..=0x263A
        | 0x2640 | 0x2642
        | 0x265F..=0x2660
        | 0x2663
        | 0x2665..=0x2666
        | 0x2668 | 0x267B | 0x267E | 0x2692
        | 0x2694..=0x2697
        | 0x2699
        | 0x269B..=0x269C
        | 0x26A0 | 0x26A7
        | 0x26B0..=0x26B1
        | 0x26C8 | 0x26CF | 0x26D1 | 0x26D3 | 0x26E9
        | 0x26F0..=0x26F1
        | 0x26F4
        | 0x26F7..=0x26F9
        | 0x2702
        | 0x2708..=0x2709
        | 0x270C..=0x270D
        | 0x270F | 0x2712 | 0x2714 | 0x2716 | 0x271D | 0x2721
        | 0x2733..=0x2734
        | 0x2744 | 0x2747
        | 0x2763..=0x2764
        | 0x27A1
        | 0x2934..=0x2935
        | 0x2B05..=0x2B07
        | 0x3030 | 0x303D | 0x3297 | 0x3299
        | 0x1F170..=0x1F171
        | 0x1F17E..=0x1F17F
        | 0x1F202 | 0x1F237
    )
}

// ---------------------------------------------------------------------
// URL de la imagen
// ---------------------------------------------------------------------

fn hex_join(chars: impl Iterator<Item = char>) -> String {
    chars.map(|c| format!("{:x}", c as u32)).collect::<Vec<_>>().join("-")
}

/// Nombre de archivo de Twemoji: codepoints en hexa minúscula unidos con
/// `-`. Twemoji saca el FE0F salvo que la secuencia tenga un ZWJ.
fn twemoji_name(cluster: &str) -> String {
    let has_zwj = cluster.contains('\u{200D}');
    hex_join(cluster.chars().filter(|&c| has_zwj || c != '\u{FE0F}'))
}

/// Posibles nombres de archivo en Fluent, en orden de probabilidad: tal cual
/// viene el texto (Fluent conserva el FE0F de ❤️, ☀️...), sin FE0F, y para un
/// emoji de un solo codepoint escrito sin selector, con FE0F.
fn fluent_names(cluster: &str) -> Vec<String> {
    let typed = hex_join(cluster.chars());
    let stripped = hex_join(cluster.chars().filter(|&c| c != '\u{FE0F}'));
    let mut names = vec![typed];
    if !names.contains(&stripped) {
        names.push(stripped.clone());
    }
    if !stripped.contains('-') {
        let with = format!("{stripped}-fe0f");
        if !names.contains(&with) {
            names.push(with);
        }
    }
    names
}

/// Todas las URLs a probar para `cluster`, en orden (la primera que cargue gana).
fn candidate_urls(cluster: &str, set: Set) -> Vec<String> {
    let mut urls = Vec::new();
    if set == Set::Fluent {
        for n in fluent_names(cluster) {
            urls.push(format!("{FLUENT_BASE}/{n}_{FLUENT_STYLE}.svg"));
        }
    }
    let (dir, ext) = if TWEMOJI_SVG { ("svg", "svg") } else { ("72x72", "png") };
    urls.push(format!("{TWEMOJI_BASE}/{dir}/{}.{ext}", twemoji_name(cluster)));
    urls
}

thread_local! {
    /// `candidate_urls` por emoji: se llama en cada frame, no vale la pena
    /// rearmar los `String` cada vez.
    static URL_CACHE: RefCell<HashMap<(Set, String), Rc<Vec<String>>>> = RefCell::new(HashMap::new());
}

fn urls_for(cluster: &str, set: Set) -> Rc<Vec<String>> {
    URL_CACHE.with(|c| {
        c.borrow_mut()
            .entry((set, cluster.to_string()))
            .or_insert_with(|| Rc::new(candidate_urls(cluster, set)))
            .clone()
    })
}

// ---------------------------------------------------------------------
// Carga y pintado de la imagen
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Imagen lista (o recién pintada).
    Ready,
    /// Todavía bajándose / decodificándose: reservar el lugar y esperar.
    Pending,
    /// No se pudo cargar: caer a la fuente monocromática.
    Failed,
}

fn image(url: &str, size: Vec2) -> Image<'static> {
    Image::new(crate::ui::anim::plain(url))
        .fit_to_exact_size(size)
        .show_loading_spinner(false)
}

/// Recorre las URLs candidatas: la primera lista gana; si una todavía está
/// bajándose se espera a ella; si falló (p. ej. 404 porque Fluent no tiene
/// ese emoji) se prueba la siguiente. Los errores los cachea el loader de
/// `egui`, así que un 404 se paga una sola vez por emoji.
fn resolve(ctx: &Context, cluster: &str, size: Vec2, set: Set) -> (Option<Image<'static>>, State) {
    for url in urls_for(cluster, set).iter() {
        let img = image(url, size);
        match img.load_for_size(ctx, size) {
            Ok(TexturePoll::Ready { .. }) => return (Some(img), State::Ready),
            Ok(_) => return (None, State::Pending),
            Err(_) => continue,
        }
    }
    (None, State::Failed)
}

/// Estado de carga del emoji a `px` de lado (dispara la descarga si hace falta).
pub fn probe(ctx: &Context, cluster: &str, px: f32) -> State {
    probe_in(ctx, cluster, px, Set::Twemoji)
}

pub fn probe_in(ctx: &Context, cluster: &str, px: f32, set: Set) -> State {
    resolve(ctx, cluster, Vec2::splat(px), set).1
}

/// Pinta el emoji en `rect` si ya está listo. No reserva espacio en `ui`
/// (usa `Image::paint_at`), así que sirve dentro de píldoras y botones
/// pintados a mano.
pub fn paint(ui: &Ui, rect: Rect, cluster: &str) -> State {
    paint_in(ui, rect, cluster, Set::Twemoji)
}

pub fn paint_in(ui: &Ui, rect: Rect, cluster: &str, set: Set) -> State {
    match resolve(ui.ctx(), cluster, rect.size(), set) {
        (Some(img), State::Ready) => {
            img.paint_at(ui, rect);
            State::Ready
        }
        (_, state) => state,
    }
}

/// Lado (px) del emoji para un texto de este tamaño de fuente.
pub fn px_for(font: &FontId) -> f32 {
    (font.size * EMOJI_SCALE).round().max(14.0)
}

/// Emoji "en línea" para usar como un widget más dentro de un
/// `horizontal_wrapped` (mensajes). Reserva un cuadrado del tamaño justo
/// aunque la imagen no haya llegado; si falla, dibuja el glifo de la fuente.
pub fn inline(ui: &mut Ui, cluster: &str, font: &FontId, color: Color32) {
    let px = px_for(font);
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(px), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    if paint(ui, rect, cluster) == State::Failed {
        ui.painter()
            .text(rect.center(), Align2::CENTER_CENTER, cluster, font.clone(), color);
    }
}

// ---------------------------------------------------------------------
// Una línea de texto con emojis (con truncado con "…")
// ---------------------------------------------------------------------

enum Item<'a> {
    Text { src: &'a str, galley: Arc<Galley> },
    Emoji(&'a str),
}

struct Laid<'a> {
    items: Vec<Item<'a>>,
    total: f32,
    px: f32,
    row_h: f32,
    set: Set,
}

fn layout<'a>(ui: &Ui, text: &'a str, font: &FontId, color: Color32, set: Set) -> Laid<'a> {
    let px = px_for(font);
    let row_h = ui
        .painter()
        .layout_no_wrap("M".to_string(), font.clone(), color)
        .size()
        .y;
    let mut items = Vec::new();
    let mut total = 0.0_f32;
    for seg in split(text) {
        match seg {
            Seg::Text(t) => {
                let galley = ui.painter().layout_no_wrap(t.to_string(), font.clone(), color);
                total += galley.size().x;
                items.push(Item::Text { src: t, galley });
            }
            Seg::Emoji(e) => {
                if probe_in(ui.ctx(), e, px, set) == State::Failed {
                    // Sin imagen: el glifo monocromático de siempre.
                    let galley = ui.painter().layout_no_wrap(e.to_string(), font.clone(), color);
                    total += galley.size().x;
                    items.push(Item::Text { src: e, galley });
                } else {
                    total += px;
                    items.push(Item::Emoji(e));
                }
            }
        }
    }
    Laid { items, total, px, row_h, set }
}

/// El prefijo más largo de `s` que entra en `room` píxeles (búsqueda binaria
/// sobre los límites de carácter, así un texto largo no cuesta N layouts).
fn fit_prefix(ui: &Ui, s: &str, font: &FontId, color: Color32, room: f32) -> Option<Arc<Galley>> {
    if room <= 0.0 {
        return None;
    }
    let starts: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
    let (mut lo, mut hi) = (0usize, starts.len().saturating_sub(1));
    let mut best = None;
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let end = starts.get(mid).copied().unwrap_or(s.len());
        let galley = ui.painter().layout_no_wrap(s[..end].to_string(), font.clone(), color);
        if galley.size().x <= room {
            best = Some(galley);
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    best
}

/// Pinta un [`Laid`] con el borde izquierdo-centro en `left_center`, sin
/// pasarse de `max_width` (si no entra, corta y agrega "…"). Devuelve el
/// ancho realmente usado.
fn paint_laid(ui: &Ui, laid: &Laid, left_center: Pos2, max_width: f32, font: &FontId, color: Color32) -> f32 {
    let truncate = laid.total > max_width + 0.5;
    let ellipsis = truncate.then(|| ui.painter().layout_no_wrap(ELLIPSIS.to_string(), font.clone(), color));
    let limit = match &ellipsis {
        Some(e) => max_width - e.size().x,
        None => max_width,
    };

    let painter = ui.painter();
    let mut x = 0.0_f32;
    for item in &laid.items {
        match item {
            Item::Text { src, galley } => {
                let w = galley.size().x;
                if !truncate || x + w <= limit {
                    let pos = Pos2::new(left_center.x + x, left_center.y - galley.size().y / 2.0);
                    painter.galley(pos, galley.clone(), color);
                    x += w;
                } else {
                    if let Some(g) = fit_prefix(ui, src, font, color, limit - x) {
                        let pos = Pos2::new(left_center.x + x, left_center.y - g.size().y / 2.0);
                        let gw = g.size().x;
                        painter.galley(pos, g, color);
                        x += gw;
                    }
                    break;
                }
            }
            Item::Emoji(e) => {
                if truncate && x + laid.px > limit {
                    break;
                }
                let rect = Rect::from_min_size(
                    Pos2::new(left_center.x + x, left_center.y - laid.px / 2.0),
                    Vec2::splat(laid.px),
                );
                paint_in(ui, rect, e, laid.set);
                x += laid.px;
            }
        }
    }
    if let Some(e) = ellipsis {
        let pos = Pos2::new(left_center.x + x, left_center.y - e.size().y / 2.0);
        let ew = e.size().x;
        painter.galley(pos, e, color);
        x += ew;
    }
    x
}

/// Equivalente a `theme::text` para un texto CON emojis: una línea, sin
/// selección, truncada con "…" al ancho disponible. Estilo Twemoji.
pub fn line(ui: &mut Ui, text: &str, font: FontId, color: Color32) -> Response {
    line_in(ui, text, font, color, Set::Twemoji)
}

pub fn line_in(ui: &mut Ui, text: &str, font: FontId, color: Color32, set: Set) -> Response {
    let laid = layout(ui, text, &font, color, set);
    let width = laid.total.min(ui.available_width()).max(0.0);
    let height = laid.row_h.max(laid.px);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_laid(ui, &laid, rect.left_center(), width, &font, color);
    }
    response
}

/// Como `theme::text` pero eligiendo el estilo de emoji: sin emojis es
/// exactamente `theme::text` (con su truncado y soporte RTL).
pub fn text_in(ui: &mut Ui, text: &str, font: FontId, color: Color32, set: Set) -> Response {
    if contains_emoji(text) {
        line_in(ui, text, font, color, set)
    } else {
        crate::theme::text(ui, text, font, color)
    }
}

/// Como `ui.painter().text(pos, Align2::LEFT_CENTER, ...)` pero con emojis a
/// color. Sin emojis usa exactamente el `painter.text` de siempre (y NO
/// trunca); con emojis trunca a `max_width` con "…". Estilo Twemoji.
pub fn paint_line(ui: &Ui, left_center: Pos2, text: &str, font: FontId, color: Color32, max_width: f32) {
    paint_line_in(ui, left_center, text, font, color, max_width, Set::Twemoji);
}

pub fn paint_line_in(ui: &Ui, left_center: Pos2, text: &str, font: FontId, color: Color32, max_width: f32, set: Set) {
    if !contains_emoji(text) {
        ui.painter().text(left_center, Align2::LEFT_CENTER, text, font, color);
        return;
    }
    let laid = layout(ui, text, &font, color, set);
    paint_laid(ui, &laid, left_center, max_width, &font, color);
}

/// Igual que [`paint_line`] pero anclado arriba-izquierda (`Align2::LEFT_TOP`).
pub fn paint_line_top(ui: &Ui, left_top: Pos2, text: &str, font: FontId, color: Color32, max_width: f32) {
    if !contains_emoji(text) {
        ui.painter().text(left_top, Align2::LEFT_TOP, text, font, color);
        return;
    }
    let laid = layout(ui, text, &font, color, Set::Twemoji);
    let h = laid.row_h.max(laid.px);
    let center = Pos2::new(left_top.x, left_top.y + h / 2.0);
    paint_laid(ui, &laid, center, max_width, &font, color);
}
