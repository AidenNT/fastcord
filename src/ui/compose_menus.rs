//! Menús que cuelgan del compositor del chat (`ui::chat::composer`):
//!
//! * **Menú de menciones**: al escribir `@` se abre una lista como la de
//!   Discord con los miembros (y roles, `@everyone`, `@here`) que coinciden
//!   con lo tipeado. Flechas para moverse, Enter/Tab o click para elegir,
//!   Esc para cerrar. Elegir una mención escribe `<@id>` en el mensaje.
//! * **Selector de emojis**: el botón de la carita del compositor. Emojis
//!   personalizados de tus servers + un catálogo Unicode con buscador.
//!
//! ## Cómo se reparten el trabajo
//! `ui::chat` no tiene un `&mut App` ni la lista de miembros (mismo problema
//! que `markdown::publish_roles`), así que se hablan por la memoria de egui:
//! 1. el compositor deja la consulta en curso ([`set_active_query`]);
//! 2. `App::ui` la lee ([`active_query`]), arma los candidatos con lo que sabe
//!    del server y los publica ([`publish_results`]);
//! 3. el compositor los dibuja y, si hay pocos, pide más al Gateway
//!    ([`drive_member_search`] → [`take_member_search`] en `App::ui`).
//!
//! Las funciones puras (parseo de la mención, reemplazo de texto, `:nombre:` →
//! `<:nombre:id>`) tienen tests al final del archivo.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{
    Align, Align2, Area, Color32, Context, CornerRadius, Frame, Id, Margin, Order, Pos2, Rect, Response,
    ScrollArea, Sense, Stroke, Ui, Vec2,
};

use crate::discord::models::StickerItem;
use crate::lib::data::{CustomEmoji, EmojiGroup};
use crate::ui::emoji as twemoji;
use crate::ui::emoji_catalog::CATEGORIES;
use crate::ui::extra;
use crate::ui::theme::{self, Icon, Palette};

/// Máximo de miembros / roles que muestra el menú de menciones.
pub const MAX_USERS: usize = 7;
pub const MAX_ROLES: usize = 3;
/// Cuánto tiene que quedarse quieta la consulta antes de preguntarle al
/// Gateway (el Gateway tiene un límite de eventos por minuto).
const SEARCH_DEBOUNCE_SECS: f64 = 0.25;
/// Una "mención" más larga que esto ya no es un nombre: se ignora.
const MAX_QUERY_CHARS: usize = 32;

// ---------------------------------------------------------------------
// Texto: normalizar, ordenar coincidencias
// ---------------------------------------------------------------------

/// Minúsculas y sin tildes (`Ñandú` -> `nandu`), para comparar lo que se tipea
/// con nombres y palabras clave sin que importen mayúsculas ni acentos.
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

/// Qué tan bien coincide `name` con la consulta (ya pasada por [`fold`]):
/// `0` = empieza igual, `1` = una palabra empieza igual, `2` = la contiene,
/// `None` = no coincide. Con la consulta vacía todo coincide.
pub fn match_rank(name: &str, folded_query: &str) -> Option<u8> {
    if folded_query.is_empty() {
        return Some(0);
    }
    let hay = fold(name);
    if hay.starts_with(folded_query) {
        return Some(0);
    }
    if hay.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(folded_query)) {
        return Some(1);
    }
    hay.contains(folded_query).then_some(2)
}

/// Reemplaza el tramo `[start, end)` (en CARACTERES, no bytes) de `text`.
pub fn replace_chars(text: &mut String, start: usize, end: usize, with: &str) {
    let byte_at = |idx: usize| text.char_indices().nth(idx).map(|(b, _)| b).unwrap_or(text.len());
    let from = byte_at(start);
    let to = byte_at(end.max(start));
    text.replace_range(from..to, with);
}

// ---------------------------------------------------------------------
// Detectar la mención en curso
// ---------------------------------------------------------------------

/// Una mención a medio escribir: `@` + lo tipeado hasta el cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveMention {
    /// Índice (en caracteres) del `@`.
    pub start: usize,
    /// Índice (en caracteres) del cursor: donde termina lo tipeado.
    pub end: usize,
    /// Lo tipeado después del `@`.
    pub query: String,
}

/// ¿El cursor (`caret`, índice en caracteres) está justo después de una
/// mención a medio escribir? El `@` tiene que estar al principio del texto o
/// después de un espacio / salto de línea (así `ana@mail.com` no abre el
/// menú), y entre el `@` y el cursor no puede haber espacios.
pub fn active_mention(text: &str, caret: usize) -> Option<ActiveMention> {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    let mut i = caret;
    while i > 0 {
        let c = chars[i - 1];
        if c == '@' {
            let start = i - 1;
            if start > 0 && !chars[start - 1].is_whitespace() {
                return None;
            }
            let query: String = chars[i..caret].iter().collect();
            if query.chars().count() > MAX_QUERY_CHARS {
                return None;
            }
            return Some(ActiveMention { start, end: caret, query });
        }
        if c.is_whitespace() {
            return None;
        }
        i -= 1;
    }
    None
}

// ---------------------------------------------------------------------
// Cursor del campo de texto
// ---------------------------------------------------------------------
// Las dos únicas funciones que tocan el estado del cursor de `egui::TextEdit`:
// si una versión nueva de egui cambia esa API, el arreglo es solo acá.

/// Posición del cursor (índice en caracteres) del `TextEdit` con este id.
pub fn caret_index(ctx: &Context, id: Id) -> Option<usize> {
    let range = egui::text_edit::TextEditState::load(ctx, id)?.cursor.char_range()?;
    char_index_to_usize(&range.primary.index)
}

/// En egui 0.36 el índice del cursor es un `CharIndex` y no un `usize`. Como no
/// sé con certeza qué accesor expone, se lee de su `Debug` (`CharIndex(5)` -> 5),
/// que sí está garantizado. Si preferís, reemplazalo por el accesor real
/// (`.0`, `.get()`, `usize::from(..)`: lo que te dé `cargo doc`).
fn char_index_to_usize(index: &impl std::fmt::Debug) -> Option<usize> {
    let text = format!("{index:?}");
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Mueve el cursor del `TextEdit` con este id (sin selección).
pub fn set_caret(ctx: &Context, id: Id, index: usize) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(index))));
    state.store(ctx, id);
}

// ---------------------------------------------------------------------
// Candidatos del menú de menciones
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MentionKind {
    User,
    Role,
    Everyone,
    Here,
}

#[derive(Clone, Debug)]
pub struct MentionCandidate {
    pub kind: MentionKind,
    /// Id del usuario o del rol (vacío para `@everyone` / `@here`).
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub avatar_color: Color32,
    /// Color del rol (del nombre del miembro o del propio rol).
    pub color: Option<Color32>,
}

impl MentionCandidate {
    /// Lo que se escribe en el mensaje al elegir este candidato.
    pub fn token(&self) -> String {
        match self.kind {
            MentionKind::User => format!("<@{}>", self.id),
            MentionKind::Role => format!("<@&{}>", self.id),
            MentionKind::Everyone => "@everyone".to_string(),
            MentionKind::Here => "@here".to_string(),
        }
    }
}

/// Candidatos ya filtrados y ordenados para UNA consulta concreta.
#[derive(Clone, Debug, Default)]
pub struct MentionResults {
    pub query: String,
    pub items: Vec<MentionCandidate>,
}

fn query_key() -> Id {
    Id::new("ecord_mention_menu_query")
}

fn results_key() -> Id {
    Id::new("ecord_mention_menu_results")
}

fn search_key() -> Id {
    Id::new("ecord_mention_menu_member_search")
}

/// El compositor avisa qué mención se está escribiendo (`None` = ninguna).
pub fn set_active_query(ctx: &Context, query: Option<&str>) {
    ctx.memory_mut(|m| match query {
        Some(q) => {
            m.data.insert_temp(query_key(), q.to_string());
        }
        None => m.data.remove::<String>(query_key()),
    });
}

/// `App::ui` lee la consulta en curso para armar los candidatos.
pub fn active_query(ctx: &Context) -> Option<String> {
    ctx.memory(|m| m.data.get_temp::<String>(query_key()))
}

/// `App::ui` publica los candidatos de `query`.
pub fn publish_results(ctx: &Context, query: String, items: Vec<MentionCandidate>) {
    let results = Arc::new(MentionResults { query, items });
    ctx.memory_mut(|m| m.data.insert_temp(results_key(), results));
}

/// Los candidatos publicados, solo si son para ESTA consulta (si el usuario
/// siguió tipeando, los publicados son de la anterior y todavía no sirven).
pub fn results_for(ctx: &Context, query: &str) -> Option<Arc<MentionResults>> {
    ctx.memory(|m| m.data.get_temp::<Arc<MentionResults>>(results_key())).filter(|r| r.query == query)
}

/// El compositor pide buscar `query` entre los miembros del server.
pub fn request_member_search(ctx: &Context, query: &str) {
    ctx.memory_mut(|m| m.data.insert_temp(search_key(), query.to_string()));
}

/// `App::ui` saca el pedido de búsqueda (si hay) y se lo manda al Gateway.
pub fn take_member_search(ctx: &Context) -> Option<String> {
    ctx.memory_mut(|m| {
        let query: Option<String> = m.data.get_temp(search_key());
        if query.is_some() {
            m.data.remove::<String>(search_key());
        }
        query
    })
}

#[derive(Clone, Default)]
struct SearchDebounce {
    query: String,
    since: f64,
    sent: bool,
}

/// Si el server tiene más miembros de los que ya conocemos, la lista local no
/// alcanza: cuando la consulta lleva un ratito quieta y hay pocos resultados
/// locales, se le pide al Gateway que busque (una sola vez por consulta).
pub fn drive_member_search(ctx: &Context, query: &str, local_users: usize) {
    if query.is_empty() || local_users >= MAX_USERS {
        return;
    }
    let key = Id::new("ecord_mention_menu_search_debounce");
    let now = ctx.input(|i| i.time);
    let mut state: SearchDebounce = ctx.memory(|m| m.data.get_temp(key)).unwrap_or_default();
    if state.query != query {
        state = SearchDebounce { query: query.to_string(), since: now, sent: false };
    }
    if !state.sent {
        if now - state.since >= SEARCH_DEBOUNCE_SECS {
            request_member_search(ctx, query);
            state.sent = true;
        } else {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }
    ctx.memory_mut(|m| m.data.insert_temp(key, state));
}

/// Estado del menú entre frames (qué fila está resaltada, si se lo cerró con Esc).
#[derive(Clone, Default)]
pub struct MentionMenuState {
    pub selected: usize,
    pub query: String,
    /// Posición del `@` de la mención que se cerró con Esc: no se vuelve a
    /// abrir para esa misma mención.
    pub dismissed_start: Option<usize>,
}

impl MentionMenuState {
    pub fn load(ctx: &Context, id: Id) -> Self {
        ctx.memory(|m| m.data.get_temp(id)).unwrap_or_default()
    }

    pub fn store(self, ctx: &Context, id: Id) {
        ctx.memory_mut(|m| m.data.insert_temp(id, self));
    }
}

// ---------------------------------------------------------------------
// Qué se mencionó / qué emojis se eligieron (para el eco local y el envío)
// ---------------------------------------------------------------------

/// Anota a quién se mencionó con el menú (`user_id`, nombre): el eco local del
/// mensaje necesita el nombre para mostrar `@Nombre` en vez de `<@id>`.
pub fn remember_user(ctx: &Context, id: Id, user_id: &str, name: &str) {
    let mut list: Vec<(String, String)> = ctx.memory(|m| m.data.get_temp(id)).unwrap_or_default();
    if !list.iter().any(|(u, _)| u == user_id) {
        list.push((user_id.to_string(), name.to_string()));
    }
    ctx.memory_mut(|m| m.data.insert_temp(id, list));
}

/// Saca lo anotado con [`remember_user`], quedándose solo con quienes siguen
/// mencionados en `content` (el usuario pudo borrar la mención a mano).
pub fn take_remembered(ctx: &Context, id: Id, content: &str) -> Vec<(String, String)> {
    let list: Vec<(String, String)> = ctx.memory(|m| m.data.get_temp(id)).unwrap_or_default();
    ctx.memory_mut(|m| m.data.remove::<Vec<(String, String)>>(id));
    list.into_iter().filter(|(user_id, _)| content.contains(&format!("<@{user_id}>"))).collect()
}

/// Anota un emoji personalizado elegido en el selector (nombre -> (id, animado)).
pub fn remember_custom(ctx: &Context, id: Id, emoji: &CustomEmoji) {
    let mut map: HashMap<String, (String, bool)> = ctx.memory(|m| m.data.get_temp(id)).unwrap_or_default();
    map.insert(emoji.name.clone(), (emoji.id.clone(), emoji.animated));
    ctx.memory_mut(|m| m.data.insert_temp(id, map));
}

pub fn take_customs(ctx: &Context, id: Id) -> HashMap<String, (String, bool)> {
    let map: HashMap<String, (String, bool)> = ctx.memory(|m| m.data.get_temp(id)).unwrap_or_default();
    ctx.memory_mut(|m| m.data.remove::<HashMap<String, (String, bool)>>(id));
    map
}

/// Convierte los `:nombre:` de un mensaje en emojis personalizados reales
/// (`<:nombre:id>` / `<a:nombre:id>`), que es lo que entiende Discord. Sirve
/// tanto para los que se eligieron en el selector (`picked`) como para los que
/// se tipearon a mano: en ese caso se busca el nombre entre los emojis que la
/// cuenta SÍ puede usar (los bloqueados por falta de Nitro se dejan como texto).
pub fn expand_shortcodes(text: &str, picked: &HashMap<String, (String, bool)>, groups: &[EmojiGroup]) -> String {
    if !text.contains(':') {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ':' && !is_inside_custom_emoji(&chars, i) {
            let mut j = i + 1;
            while j < chars.len() && j - i <= 33 && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            if j < chars.len() && chars[j] == ':' && j - i - 1 >= 2 {
                let name: String = chars[i + 1..j].iter().collect();
                if let Some((id, animated)) = lookup_custom(&name, picked, groups) {
                    let prefix = if animated { "a" } else { "" };
                    out.push_str(&format!("<{prefix}:{name}:{id}>"));
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// ¿El `:` de `chars[i]` ya es parte de un `<:nombre:id>` / `<a:nombre:id>`?
fn is_inside_custom_emoji(chars: &[char], i: usize) -> bool {
    (i >= 1 && chars[i - 1] == '<') || (i >= 2 && chars[i - 1] == 'a' && chars[i - 2] == '<')
}

fn lookup_custom(name: &str, picked: &HashMap<String, (String, bool)>, groups: &[EmojiGroup]) -> Option<(String, bool)> {
    if let Some(found) = picked.get(name) {
        return Some(found.clone());
    }
    // Primero los del server actual, después los de los demás.
    groups
        .iter()
        .filter(|g| g.home)
        .chain(groups.iter().filter(|g| !g.home))
        .find_map(|g| {
            g.emojis
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(name) && !g.is_locked(e))
                .map(|e| (e.id.clone(), e.animated))
        })
}

// ---------------------------------------------------------------------
// Dibujo del menú de menciones
// ---------------------------------------------------------------------

/// Qué pasó con el menú este frame.
pub struct MenuOutcome {
    /// Fila sobre la que está el mouse.
    pub hovered: Option<usize>,
    /// Fila en la que se apretó el botón del mouse este frame. Se elige al
    /// APRETAR (no al soltar): apretar fuera del campo de texto le quita el
    /// foco, y el menú, que solo vive mientras el campo tiene foco, se iría
    /// antes de poder registrar el click.
    pub pressed: Option<usize>,
}

fn section_of(kind: MentionKind) -> u8 {
    match kind {
        MentionKind::User => 0,
        MentionKind::Role => 1,
        MentionKind::Everyone | MentionKind::Here => 2,
    }
}

/// Dibuja el menú arriba de `anchor` (esquina inferior izquierda del menú).
#[allow(clippy::too_many_arguments)]
pub fn show_mention_menu(
    ctx: &Context,
    palette: &Palette,
    id: Id,
    anchor: Pos2,
    width: f32,
    query: &str,
    items: &[MentionCandidate],
    selected: usize,
) -> MenuOutcome {
    let mut outcome = MenuOutcome { hovered: None, pressed: None };
    let pressed_now = ctx.input(|i| i.pointer.primary_pressed());
    Area::new(id)
        .order(Order::Foreground)
        .pivot(Align2::LEFT_BOTTOM)
        .fixed_pos(anchor)
        .show(ctx, |ui| {
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::RADIUS + 2))
                .inner_margin(Margin::same(8))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 20,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let mut last_section: Option<u8> = None;
                    for (index, cand) in items.iter().enumerate() {
                        let section = section_of(cand.kind);
                        if last_section != Some(section) {
                            if last_section.is_some() {
                                ui.add_space(4.0);
                            }
                            let title = match section {
                                0 if query.is_empty() => "MIEMBROS".to_string(),
                                0 => format!("MIEMBROS QUE COINCIDEN CON @{}", query.to_uppercase()),
                                1 => "ROLES".to_string(),
                                _ => "NOTIFICAR A TODOS".to_string(),
                            };
                            theme::text(ui, title, theme::semibold(10.5), palette.dim);
                            last_section = Some(section);
                        }
                        let response = mention_row(ui, palette, cand, index == selected);
                        if response.hovered() {
                            outcome.hovered = Some(index);
                            if pressed_now {
                                outcome.pressed = Some(index);
                            }
                        }
                    }
                });
        });
    outcome
}

fn mention_row(ui: &mut Ui, palette: &Palette, cand: &MentionCandidate, selected: bool) -> Response {
    const ROW_HEIGHT: f32 = 32.0;
    const HINT_WIDTH: f32 = 150.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &cand.name));
    if ui.is_rect_visible(rect) {
        if selected || response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(6), palette.surface_hover);
        }
        let cy = rect.center().y;
        let mut text_x = rect.left() + 8.0;
        match cand.kind {
            MentionKind::User => {
                let initial = cand
                    .name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default();
                extra::avatar(
                    ui,
                    Pos2::new(text_x + 11.0, cy),
                    11.0,
                    cand.avatar_url.as_deref(),
                    cand.avatar_color,
                    &initial,
                    palette,
                );
                text_x += 30.0;
            }
            MentionKind::Role => {
                let dot = cand.color.unwrap_or(palette.dim);
                ui.painter().circle_filled(Pos2::new(text_x + 5.0, cy), 5.0, dot);
                text_x += 18.0;
            }
            MentionKind::Everyone | MentionKind::Here => {}
        }
        let (label, hint) = match cand.kind {
            MentionKind::User => (cand.name.clone(), None),
            MentionKind::Role => (format!("@{}", cand.name), None),
            MentionKind::Everyone => ("@everyone".to_string(), Some("Notifica a todos")),
            MentionKind::Here => ("@here".to_string(), Some("Notifica a los conectados")),
        };
        let reserved = if hint.is_some() { HINT_WIDTH } else { 0.0 };
        let max_width = (rect.right() - text_x - 8.0 - reserved).max(40.0);
        let color = cand.color.unwrap_or(palette.text);
        twemoji::paint_line(ui, Pos2::new(text_x, cy), &label, theme::semibold(13.0), color, max_width);
        if let Some(hint) = hint {
            ui.painter().text(
                Pos2::new(rect.right() - 8.0, cy),
                Align2::RIGHT_CENTER,
                hint,
                theme::regular(11.5),
                palette.dim,
            );
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------
// Selector de emojis
// ---------------------------------------------------------------------

/// Para qué se abrió el selector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickerMode {
    /// Desde el compositor: se puede elegir un emoji, un sticker o un GIF.
    Compose,
    /// Para reaccionar a un mensaje: solo emojis. Las pestañas GIF y
    /// Stickers se ven, pero apagadas (no se puede reaccionar con eso, ni acá
    /// ni en el cliente real).
    React,
}

/// Las tres pestañas de arriba del selector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PickerTab {
    Gif,
    Stickers,
    Emojis,
}

/// Qué pasó con el selector este frame.
pub enum PickerResult {
    /// Sigue abierto, sin novedades.
    Pending,
    /// Hay que cerrarlo (Esc o click afuera).
    Closed,
    /// Se eligió un emoji Unicode.
    Unicode(String),
    /// Se eligió un emoji personalizado que se puede usar.
    Custom(CustomEmoji),
    /// Se eligió un emoji o sticker bloqueado (falta Nitro).
    Locked,
    /// Se eligió un sticker que se puede usar (solo en [`PickerMode::Compose`]).
    Sticker(StickerItem),
    /// Se eligió un GIF: el link que hay que mandar como mensaje (solo en
    /// [`PickerMode::Compose`]).
    Gif(String),
}

// ---------------------------------------------------------------------
// GIFs (pestaña "GIF")
// ---------------------------------------------------------------------

/// Cuánto tiene que quedarse quieto el buscador antes de pedir GIFs.
const GIF_DEBOUNCE_SECS: f64 = 0.35;

/// Un GIF de la lista que devuelve Discord (Tenor).
#[derive(Clone)]
struct GifEntry {
    title: String,
    /// El link que se manda como mensaje (Discord lo convierte en el GIF).
    url: String,
    /// Imagen quieta, liviana, para la grilla.
    preview: String,
    /// El GIF animado (se usa solo mientras el mouse está encima).
    gif: String,
    width: f32,
    height: f32,
}

enum GifLoad {
    Loading,
    Ready(Vec<GifEntry>),
    Failed,
}

/// Resultados ya pedidos, por búsqueda (`""` = los del momento). Lo comparten
/// la UI y los hilos que hacen el pedido, así que va en un `Arc<Mutex<..>>`.
#[derive(Default)]
struct GifStore {
    results: HashMap<String, GifLoad>,
}

type SharedGifStore = Arc<Mutex<GifStore>>;

/// Lo que el buscador de GIFs recuerda entre frames.
#[derive(Clone, Default)]
struct GifQueryState {
    /// Lo último tipeado (ya en minúsculas y sin espacios de los bordes).
    typed: String,
    /// Momento (reloj de egui) en que cambió `typed`.
    changed_at: f64,
    /// Búsqueda que ya se pidió.
    requested: Option<String>,
    /// Última búsqueda con resultados listos: es la que se dibuja mientras
    /// llega la nueva, así la grilla no parpadea en blanco al tipear.
    shown: Option<String>,
}

fn gif_store(ctx: &Context) -> SharedGifStore {
    let key = Id::new("ecord_gif_store");
    ctx.memory_mut(|m| m.data.get_temp_mut_or_insert_with(key, SharedGifStore::default).clone())
}

/// Lee la respuesta de Discord: una lista de GIFs o un objeto con `gifs`.
fn parse_gifs(value: &serde_json::Value) -> Vec<GifEntry> {
    let list: &[serde_json::Value] = match value {
        serde_json::Value::Array(items) => items.as_slice(),
        serde_json::Value::Object(map) => match map.get("gifs").and_then(|g| g.as_array()) {
            Some(items) => items.as_slice(),
            None => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    list.iter()
        .filter_map(|g| {
            let text = |key: &str| g.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let number = |key: &str| g.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let url = text("url");
            let gif = text("gif_src");
            let preview = {
                let p = text("preview");
                if p.is_empty() { gif.clone() } else { p }
            };
            if url.is_empty() || preview.is_empty() {
                return None;
            }
            Some(GifEntry { title: text("title"), url, preview, gif, width: number("width"), height: number("height") })
        })
        .collect()
}

/// Decide si hay que pedir GIFs (con espera si se está tipeando) y, si hace
/// falta, lanza el pedido en un hilo aparte. Devuelve la búsqueda cuyos
/// resultados hay que dibujar (`None` = todavía no hay nada que mostrar).
fn drive_gif_search(ctx: &Context, id: Id, token: &str, typed: &str) -> Option<String> {
    let state_id = id.with("gif_state");
    let mut state: GifQueryState = ctx.memory(|m| m.data.get_temp(state_id)).unwrap_or_default();
    let now = ctx.input(|i| i.time);
    let query = typed.trim().to_lowercase();
    if state.typed != query {
        state.typed = query.clone();
        state.changed_at = now;
    }

    if state.requested.as_deref() != Some(query.as_str()) {
        let waited = now - state.changed_at;
        if query.is_empty() || waited >= GIF_DEBOUNCE_SECS {
            let store = gif_store(ctx);
            let needs_fetch = store
                .lock()
                .map(|s| matches!(s.results.get(&query), None | Some(GifLoad::Failed)))
                .unwrap_or(false);
            if needs_fetch {
                if let Ok(mut s) = store.lock() {
                    s.results.insert(query.clone(), GifLoad::Loading);
                }
                let ctx2 = ctx.clone();
                let key = query.clone();
                crate::discord::spawn_fetch_gifs(token.to_string(), query.clone(), move |res| {
                    let load = match res {
                        Ok(value) => GifLoad::Ready(parse_gifs(&value)),
                        Err(e) => {
                            log::warn!("No se pudieron traer los GIFs: {e}");
                            GifLoad::Failed
                        }
                    };
                    if let Ok(mut s) = store.lock() {
                        s.results.insert(key, load);
                    }
                    ctx2.request_repaint();
                });
            }
            state.requested = Some(query.clone());
        } else {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    // ¿Ya llegaron los de la búsqueda actual? Si sí, es lo que se dibuja.
    let store = gif_store(ctx);
    let ready_now = store
        .lock()
        .map(|s| matches!(s.results.get(&query), Some(GifLoad::Ready(_))))
        .unwrap_or(false);
    if ready_now {
        state.shown = Some(query.clone());
    }
    let to_draw = state.shown.clone().or_else(|| Some(query.clone()));
    ctx.memory_mut(|m| m.data.insert_temp(state_id, state));
    to_draw
}

/// Borra lo que el selector recuerda entre frames (buscador y salto pendiente):
/// se llama al cerrarlo, así la próxima vez abre limpio.
pub fn reset_picker(ctx: &Context, id: Id) {
    ctx.memory_mut(|m| {
        m.data.remove::<String>(id.with("search"));
        m.data.remove::<usize>(id.with("jump"));
        m.data.remove::<usize>(id.with("active"));
        m.data.remove::<PickerTab>(id.with("tab"));
        m.data.remove::<GifQueryState>(id.with("gif_state"));
    });
}

/// Lo que está bajo el mouse en el selector (para la barra de vista previa).
enum Hover {
    Unicode { emoji: &'static str, name: &'static str },
    Custom { emoji: CustomEmoji, locked: bool },
}

/// Selector de emojis flotante. Arriba un buscador con lupa; a la izquierda una
/// barra vertical con un ícono por server y por categoría (resalta la sección
/// que se está viendo y, al clickearla, salta a ella); a la derecha la lista en
/// una grilla grande; y abajo una barra de vista previa que muestra el emoji
/// bajo el mouse en grande con su nombre. `anchor` es la esquina inferior
/// derecha del panel (en [`PickerMode::React`], la superior derecha).
///
/// Arriba del buscador hay tres pestañas: **GIF**, **Stickers** y **Emojis**.
/// En [`PickerMode::Compose`] se puede ir a las tres; en
/// [`PickerMode::React`] (reaccionar a un mensaje) GIF y Stickers quedan
/// apagadas, porque no se puede reaccionar con eso.
pub fn show_emoji_picker(
    ctx: &Context,
    palette: &Palette,
    id: Id,
    anchor: Pos2,
    custom_emojis: &[EmojiGroup],
    mode: PickerMode,
    // Token de la cuenta, para pedir GIFs (pestaña "GIF"). `None` (chat de
    // demo, o modo reacción) = esa pestaña no tiene qué mostrar.
    token: Option<&str>,
) -> PickerResult {
    const WIDTH: f32 = 460.0;
    const RAIL_WIDTH: f32 = 40.0;
    const GAP: f32 = 8.0;
    const LIST_HEIGHT: f32 = 340.0;
    // Alto de la lista en las pestañas GIF y Stickers: lista + vista previa de
    // la de Emojis, así el panel no cambia de tamaño al cambiar de pestaña.
    const BODY_HEIGHT: f32 = LIST_HEIGHT + 8.0 + 52.0;

    let search_id = id.with("search");
    let jump_id = id.with("jump");
    let active_id = id.with("active");
    let tab_id = id.with("tab");
    // Al reaccionar solo existe la pestaña de Emojis.
    let tab: PickerTab = if mode == PickerMode::React {
        PickerTab::Emojis
    } else {
        ctx.memory(|m| m.data.get_temp(tab_id)).unwrap_or(PickerTab::Emojis)
    };
    let mut search: String = ctx.memory(|m| m.data.get_temp(search_id)).unwrap_or_default();
    let needle = fold(search.trim());
    let list_width = WIDTH - 20.0 - RAIL_WIDTH - GAP;

    // Lo que hay para mostrar con el filtro actual, calculado ANTES de
    // dibujar: la barra lateral solo ofrece las secciones que existen.
    let servers: Vec<(&EmojiGroup, Vec<&CustomEmoji>)> = custom_emojis
        .iter()
        .filter_map(|g| {
            let filtered: Vec<&CustomEmoji> = g
                .emojis
                .iter()
                .filter(|e| needle.is_empty() || fold(&e.name).contains(needle.as_str()))
                .collect();
            (!filtered.is_empty()).then_some((g, filtered))
        })
        .collect();
    let categories: Vec<(usize, Vec<(&'static str, &'static str)>)> = CATEGORIES
        .iter()
        .enumerate()
        .filter_map(|(index, (_, _, list))| {
            let filtered: Vec<(&'static str, &'static str)> = list
                .iter()
                .filter(|(emoji, keywords)| {
                    needle.is_empty() || keywords.contains(needle.as_str()) || *emoji == needle.as_str()
                })
                .copied()
                .collect();
            (!filtered.is_empty()).then_some((index, filtered))
        })
        .collect();

    let mut result = PickerResult::Pending;
    let mut hover: Option<Hover> = None;
    let area = Area::new(id)
        .order(Order::Foreground)
        .pivot(if mode == PickerMode::React { Align2::RIGHT_TOP } else { Align2::RIGHT_BOTTOM })
        .fixed_pos(anchor)
        .show(ctx, |ui| {
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::RADIUS + 6))
                .inner_margin(Margin::same(10))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 12],
                    blur: 32,
                    spread: 0,
                    color: palette.shadow,
                })
                .show(ui, |ui| {
                    ui.set_width(WIDTH - 20.0);

                    // ---- Pestañas GIF / Stickers / Emojis. En modo reacción solo
                    // "Emojis" funciona: las otras se ven apagadas.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 16.0;
                        for (candidate, label) in [
                            (PickerTab::Gif, "GIF"),
                            (PickerTab::Stickers, "Stickers"),
                            (PickerTab::Emojis, "Emojis"),
                        ] {
                            let enabled = mode == PickerMode::Compose || candidate == PickerTab::Emojis;
                            let clicked = picker_tab(ui, palette, label, candidate == tab, enabled).clicked();
                            if clicked && enabled && candidate != tab {
                                // Cada pestaña arranca con el buscador vacío.
                                ctx.memory_mut(|m| {
                                    m.data.insert_temp(tab_id, candidate);
                                    m.data.remove::<String>(search_id);
                                    m.data.remove::<usize>(jump_id);
                                    m.data.remove::<usize>(active_id);
                                });
                                ctx.request_repaint();
                            }
                        }
                    });
                    ui.add_space(8.0);

                    // ---- Buscador (se enfoca solo, para tipear de una).
                    Frame::new()
                        .fill(palette.surface)
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(10, 7))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                theme::icon(ui, Icon::Search, 15.0, palette.dim);
                                let search_response = ui.add(
                                    egui::TextEdit::singleline(&mut search)
                                        .hint_text(match tab {
                                            PickerTab::Emojis => "Buscá el emoji perfecto",
                                            PickerTab::Stickers => "Buscá un sticker",
                                            PickerTab::Gif => "Buscá un GIF en Tenor",
                                        })
                                        .frame(egui::Frame::NONE)
                                        .desired_width(f32::INFINITY),
                                );
                                if search_response.changed() {
                                    ctx.memory_mut(|m| m.data.insert_temp(search_id, search.clone()));
                                }
                                if !search_response.has_focus() {
                                    search_response.request_focus();
                                }
                            });
                        });
                    ui.add_space(8.0);

                    match tab {
                        PickerTab::Emojis => {
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = GAP;

                                // ---- Barra lateral de secciones.
                                let active: usize = ctx.memory(|m| m.data.get_temp(active_id)).unwrap_or(0);
                                ui.vertical(|ui| {
                                    ScrollArea::vertical()
                                        .id_salt(id.with("rail"))
                                        .max_height(LIST_HEIGHT)
                                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                                        .show(ui, |ui| {
                                            ui.set_width(RAIL_WIDTH);
                                            ui.spacing_mut().item_spacing.y = 4.0;
                                            let mut section = 0usize;
                                            for (group, _) in &servers {
                                                let clicked = rail_cell(ui, palette, &group.name, section == active, |ui, rect| {
                                                    extra::avatar(
                                                        ui,
                                                        rect.center(),
                                                        12.0,
                                                        group.icon_url.as_deref(),
                                                        group.icon_color,
                                                        &group.icon_initial,
                                                        palette,
                                                    );
                                                })
                                                .clicked();
                                                if clicked {
                                                    ctx.memory_mut(|m| {
                                                        m.data.insert_temp(jump_id, section);
                                                        m.data.insert_temp(active_id, section);
                                                    });
                                                }
                                                section += 1;
                                            }
                                            if !servers.is_empty() && !categories.is_empty() {
                                                ui.add_space(4.0);
                                            }
                                            for (index, _) in &categories {
                                                let (title, icon, _) = CATEGORIES[*index];
                                                let clicked = rail_cell(ui, palette, title, section == active, |ui, rect| {
                                                    let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(22.0));
                                                    if twemoji::paint(ui, icon_rect, icon) == twemoji::State::Failed {
                                                        ui.painter().text(
                                                            rect.center(),
                                                            Align2::CENTER_CENTER,
                                                            icon,
                                                            theme::regular(18.0),
                                                            palette.dim,
                                                        );
                                                    }
                                                })
                                                .clicked();
                                                if clicked {
                                                    ctx.memory_mut(|m| {
                                                        m.data.insert_temp(jump_id, section);
                                                        m.data.insert_temp(active_id, section);
                                                    });
                                                }
                                                section += 1;
                                            }
                                        });
                                });

                                // ---- Lista de emojis.
                                ui.vertical(|ui| {
                                    ui.set_width(list_width);
                                    ScrollArea::vertical()
                                        .id_salt(id.with("list"))
                                        .max_height(LIST_HEIGHT)
                                        .auto_shrink([false, true])
                                        .show(ui, |ui| {
                                            let jump: Option<usize> = ctx.memory(|m| m.data.get_temp(jump_id));
                                            let clip_top = ui.clip_rect().top();
                                            // Sección "activa" = la última cuyo título ya llegó arriba.
                                            let mut spy = 0usize;
                                            let mut section = 0usize;

                                            for (group, emojis) in &servers {
                                                let header = section_header(ui, palette, &group.name.to_uppercase(), emojis.len());
                                                if jump == Some(section) {
                                                    ui.scroll_to_rect(header.rect, Some(Align::TOP));
                                                }
                                                if header.rect.top() <= clip_top + 8.0 {
                                                    spy = section;
                                                }
                                                ui.horizontal_wrapped(|ui| {
                                                    ui.spacing_mut().item_spacing = Vec2::new(2.0, 2.0);
                                                    for emoji in emojis {
                                                        let locked = group.is_locked(emoji);
                                                        let response = custom_cell(ui, palette, emoji, locked);
                                                        if response.hovered() {
                                                            hover = Some(Hover::Custom { emoji: (*emoji).clone(), locked });
                                                        }
                                                        if response.clicked() {
                                                            result = if locked {
                                                                PickerResult::Locked
                                                            } else {
                                                                PickerResult::Custom((*emoji).clone())
                                                            };
                                                        }
                                                    }
                                                });
                                                ui.add_space(10.0);
                                                section += 1;
                                            }

                                            for (index, list) in &categories {
                                                let (title, _, _) = CATEGORIES[*index];
                                                let header = section_header(ui, palette, title, list.len());
                                                if jump == Some(section) {
                                                    ui.scroll_to_rect(header.rect, Some(Align::TOP));
                                                }
                                                if header.rect.top() <= clip_top + 8.0 {
                                                    spy = section;
                                                }
                                                ui.horizontal_wrapped(|ui| {
                                                    ui.spacing_mut().item_spacing = Vec2::new(2.0, 2.0);
                                                    for &(emoji, keywords) in list {
                                                        let name = keywords.split(' ').next().unwrap_or("");
                                                        let response = unicode_cell(ui, palette, emoji, name);
                                                        if response.hovered() {
                                                            hover = Some(Hover::Unicode { emoji, name });
                                                        }
                                                        if response.clicked() {
                                                            result = PickerResult::Unicode(emoji.to_string());
                                                        }
                                                    }
                                                });
                                                ui.add_space(10.0);
                                                section += 1;
                                            }

                                            if jump.is_some() {
                                                // El salto ya se hizo: se consume (el resaltado lo
                                                // dejó puesto el click, no lo pisa el `spy` de este
                                                // frame, que todavía ve la lista sin mover).
                                                ctx.memory_mut(|m| m.data.remove::<usize>(jump_id));
                                                ctx.request_repaint();
                                            } else {
                                                ctx.memory_mut(|m| m.data.insert_temp(active_id, spy));
                                            }
                                            if section == 0 {
                                                ui.add_space(24.0);
                                                ui.vertical_centered(|ui| {
                                                    theme::text(ui, "No se encontró ningún emoji", theme::regular(13.0), palette.dim);
                                                });
                                            }
                                        });
                                });
                            });

                            // ---- Vista previa del emoji bajo el mouse.
                            ui.add_space(8.0);
                            picker_footer(ui, ctx, palette, hover.as_ref());
                        }
                        PickerTab::Stickers => {
                            if let Some(picked) = stickers_body(ui, ctx, palette, id, custom_emojis, &needle, BODY_HEIGHT) {
                                result = picked;
                            }
                        }
                        PickerTab::Gif => {
                            if let Some(picked) = gif_body(ui, ctx, palette, id, token, search.trim(), BODY_HEIGHT) {
                                result = picked;
                            }
                        }
                    }
                });
        });

    if matches!(result, PickerResult::Pending) {
        let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if escape || area.response.clicked_elsewhere() {
            result = PickerResult::Closed;
        }
    }
    result
}

/// Una pestaña de la fila GIF / Stickers / Emojis. `enabled = false` la dibuja
/// apagada, sin click ni mano (en modo reacción: GIF y Stickers).
fn picker_tab(ui: &mut Ui, palette: &Palette, label: &str, active: bool, enabled: bool) -> Response {
    let font = if active { theme::semibold(12.5) } else { theme::regular(12.5) };
    let width = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE)
        .size()
        .x;
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width + 4.0, 24.0), sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if ui.is_rect_visible(rect) {
        let color = if active {
            palette.text
        } else if !enabled {
            palette.dim.gamma_multiply(0.45)
        } else if response.hovered() {
            palette.text
        } else {
            palette.dim
        };
        ui.painter().text(
            Pos2::new(rect.left() + 2.0, rect.center().y - 1.0),
            Align2::LEFT_CENTER,
            label,
            font,
            color,
        );
        if active {
            let y = rect.bottom() - 1.0;
            ui.painter().line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                Stroke::new(2.0, palette.accent),
            );
        }
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response.on_hover_text("No se puede reaccionar con esto")
    }
}

/// Un texto centrado que ocupa todo el alto del cuerpo (estados vacíos).
fn centered_note(ui: &mut Ui, palette: &Palette, height: f32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, text, theme::regular(13.0), palette.dim);
}

/// Pestaña "Stickers": igual que la de emojis (barra lateral con un ícono por
/// server + secciones en una grilla), pero con los stickers de cada server.
/// Devuelve lo que se eligió, si algo.
fn stickers_body(
    ui: &mut Ui,
    ctx: &Context,
    palette: &Palette,
    id: Id,
    groups: &[EmojiGroup],
    needle: &str,
    height: f32,
) -> Option<PickerResult> {
    const WIDTH: f32 = 460.0;
    const RAIL_WIDTH: f32 = 40.0;
    const GAP: f32 = 8.0;
    let list_width = WIDTH - 20.0 - RAIL_WIDTH - GAP;
    let jump_id = id.with("jump");
    let active_id = id.with("active");

    let sections: Vec<(&EmojiGroup, Vec<&StickerItem>)> = groups
        .iter()
        .filter_map(|g| {
            let filtered: Vec<&StickerItem> = g
                .stickers
                .iter()
                .filter(|s| needle.is_empty() || fold(&s.name).contains(needle))
                .collect();
            (!filtered.is_empty()).then_some((g, filtered))
        })
        .collect();

    if sections.is_empty() {
        let message = if needle.is_empty() {
            "Tus servidores no tienen stickers"
        } else {
            "No se encontró ningún sticker"
        };
        centered_note(ui, palette, height, message);
        return None;
    }

    let mut picked: Option<PickerResult> = None;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;

        // ---- Barra lateral: un ícono por server.
        let active: usize = ctx.memory(|m| m.data.get_temp(active_id)).unwrap_or(0);
        ui.vertical(|ui| {
            ScrollArea::vertical()
                .id_salt(id.with("sticker_rail"))
                .max_height(height)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.set_width(RAIL_WIDTH);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for (section, (group, _)) in sections.iter().enumerate() {
                        let clicked = rail_cell(ui, palette, &group.name, section == active, |ui, rect| {
                            extra::avatar(
                                ui,
                                rect.center(),
                                12.0,
                                group.icon_url.as_deref(),
                                group.icon_color,
                                &group.icon_initial,
                                palette,
                            );
                        })
                        .clicked();
                        if clicked {
                            ctx.memory_mut(|m| {
                                m.data.insert_temp(jump_id, section);
                                m.data.insert_temp(active_id, section);
                            });
                        }
                    }
                });
        });

        // ---- Lista: una sección por server.
        ui.vertical(|ui| {
            ui.set_width(list_width);
            ScrollArea::vertical()
                .id_salt(id.with("sticker_list"))
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let jump: Option<usize> = ctx.memory(|m| m.data.get_temp(jump_id));
                    let clip_top = ui.clip_rect().top();
                    let mut spy = 0usize;
                    for (section, (group, stickers)) in sections.iter().enumerate() {
                        let header = section_header(ui, palette, &group.name.to_uppercase(), stickers.len());
                        if jump == Some(section) {
                            ui.scroll_to_rect(header.rect, Some(Align::TOP));
                        }
                        if header.rect.top() <= clip_top + 8.0 {
                            spy = section;
                        }
                        let locked = group.is_sticker_locked();
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::splat(4.0);
                            for sticker in stickers {
                                if sticker_cell(ui, palette, sticker, locked).clicked() {
                                    picked = Some(if locked {
                                        PickerResult::Locked
                                    } else {
                                        PickerResult::Sticker((*sticker).clone())
                                    });
                                }
                            }
                        });
                        ui.add_space(10.0);
                    }
                    if jump.is_some() {
                        ctx.memory_mut(|m| m.data.remove::<usize>(jump_id));
                        ctx.request_repaint();
                    } else {
                        ctx.memory_mut(|m| m.data.insert_temp(active_id, spy));
                    }
                });
        });
    });
    picked
}

/// Una celda de sticker. Solo pide la imagen si está a la vista. Atenuada y
/// con candadito si hace falta Nitro y no se tiene.
fn sticker_cell(ui: &mut Ui, palette: &Palette, sticker: &StickerItem, locked: bool) -> Response {
    const SIDE: f32 = 74.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SIDE), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &sticker.name));
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(9), palette.surface_hover);
        }
        let image_rect = rect.shrink(5.0);
        let mut image = egui::Image::new(crate::ui::anim::source(ui.ctx(), &sticker.thumb_url()))
            .fit_to_exact_size(image_rect.size())
            .show_loading_spinner(false);
        if locked {
            image = image.tint(Color32::from_white_alpha(90));
        }
        match image.load_for_size(ui.ctx(), image_rect.size()) {
            Ok(egui::load::TexturePoll::Ready { .. }) => image.paint_at(ui, image_rect),
            _ => {
                ui.painter().rect_filled(image_rect, 8.0, palette.surface_hover);
            }
        }
        if locked {
            let badge = Rect::from_center_size(rect.right_bottom() - Vec2::splat(10.0), Vec2::splat(13.0));
            ui.painter().circle_filled(badge.center(), 7.5, palette.overlay);
            theme::paint_icon(ui, Icon::Lock, badge, 10.0, palette.text);
        }
    }
    let hint = if locked {
        format!("{} — requiere Nitro", sticker.name)
    } else {
        sticker.name.clone()
    };
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(hint)
}

/// Pestaña "GIF": GIFs del momento o los de la búsqueda, en dos columnas.
/// Elegir uno devuelve su link (se manda como mensaje).
fn gif_body(
    ui: &mut Ui,
    ctx: &Context,
    palette: &Palette,
    id: Id,
    token: Option<&str>,
    typed: &str,
    height: f32,
) -> Option<PickerResult> {
    enum View {
        Loading,
        Failed,
        Entries(Vec<GifEntry>),
    }

    let Some(token) = token else {
        centered_note(ui, palette, height, "No hay GIFs disponibles en este chat");
        return None;
    };

    let to_draw = drive_gif_search(ctx, id, token, typed);
    let store = gif_store(ctx);
    let view = {
        let found = store.lock().ok().map(|s| match to_draw.as_ref().and_then(|q| s.results.get(q)) {
            Some(GifLoad::Ready(list)) => View::Entries(list.clone()),
            Some(GifLoad::Failed) => View::Failed,
            _ => View::Loading,
        });
        found.unwrap_or(View::Failed)
    };

    let mut picked: Option<String> = None;
    match view {
        View::Loading => centered_note(ui, palette, height, "Cargando GIFs…"),
        View::Failed => centered_note(ui, palette, height, "No se pudieron cargar los GIFs"),
        View::Entries(list) if list.is_empty() => centered_note(ui, palette, height, "No se encontró ningún GIF"),
        View::Entries(list) => {
            ScrollArea::vertical()
                .id_salt(id.with("gif_list"))
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    picked = gif_grid(ui, ctx, palette, &list);
                });
        }
    }
    picked.map(PickerResult::Gif)
}

/// La grilla de GIFs en dos columnas: cada GIF va a la columna más baja, con
/// el alto que le toca por su proporción (acotado), como en el cliente real.
fn gif_grid(ui: &mut Ui, ctx: &Context, palette: &Palette, entries: &[GifEntry]) -> Option<String> {
    const GAP: f32 = 6.0;
    let column_width = ((ui.available_width() - GAP) / 2.0).floor().max(60.0);

    let mut columns: [Vec<(&GifEntry, f32)>; 2] = [Vec::new(), Vec::new()];
    let mut heights = [0.0_f32; 2];
    for gif in entries {
        let ratio = if gif.width > 0.0 && gif.height > 0.0 { gif.height / gif.width } else { 0.75 };
        let h = (column_width * ratio).clamp(60.0, 240.0);
        let target = if heights[0] <= heights[1] { 0 } else { 1 };
        columns[target].push((gif, h));
        heights[target] += h + GAP;
    }

    let mut picked: Option<String> = None;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(GAP, GAP);
        for column in &columns {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = GAP;
                for (gif, h) in column {
                    if gif_tile(ui, ctx, palette, gif, Vec2::new(column_width, *h)).clicked() {
                        picked = Some(gif.url.clone());
                    }
                }
            });
        }
    });
    picked
}

/// Un GIF de la grilla: la imagen quieta (liviana) y, solo mientras el mouse
/// está encima, el GIF animado.
fn gif_tile(ui: &mut Ui, ctx: &Context, palette: &Palette, gif: &GifEntry, size: Vec2) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &gif.title));
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, CornerRadius::same(8), palette.surface);
        let still = egui::Image::new(crate::ui::anim::plain(&gif.preview))
            .fit_to_exact_size(size)
            .show_loading_spinner(false);
        if let Ok(egui::load::TexturePoll::Ready { .. }) = still.load_for_size(ctx, size) {
            still.paint_at(ui, rect);
        }
        if response.hovered() && !gif.gif.is_empty() {
            let animated = egui::Image::new(crate::ui::anim::source(ctx, &gif.gif))
                .fit_to_exact_size(size)
                .show_loading_spinner(false);
            if let Ok(egui::load::TexturePoll::Ready { .. }) = animated.load_for_size(ctx, size) {
                animated.paint_at(ui, rect);
            }
            ui.painter()
                .rect_stroke(rect, 8.0, Stroke::new(2.0, palette.accent), egui::StrokeKind::Inside);
        }
    }
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if gif.title.is_empty() {
        response
    } else {
        response.on_hover_text(gif.title.as_str())
    }
}

/// Título de sección con la cantidad de emojis a la derecha.
fn section_header(ui: &mut Ui, palette: &Palette, title: &str, count: usize) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().text(
            Pos2::new(rect.left() + 2.0, rect.center().y),
            Align2::LEFT_CENTER,
            title,
            theme::semibold(11.0),
            palette.secondary,
        );
        ui.painter().text(
            Pos2::new(rect.right() - 4.0, rect.center().y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            theme::regular(10.5),
            palette.dim,
        );
    }
    response
}

/// Barra de abajo: el emoji bajo el mouse en grande, con su nombre.
fn picker_footer(ui: &mut Ui, ctx: &Context, palette: &Palette, hover: Option<&Hover>) {
    const HEIGHT: f32 = 52.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HEIGHT), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(10), palette.surface);
    let cy = rect.center().y;
    let icon_rect = Rect::from_center_size(Pos2::new(rect.left() + 14.0 + 20.0, cy), Vec2::splat(36.0));
    let text_x = icon_rect.right() + 12.0;
    let painter = ui.painter().clone();
    let two_lines = |title: &str, sub: &str| {
        painter.text(
            Pos2::new(text_x, cy - 8.0),
            Align2::LEFT_CENTER,
            title,
            theme::semibold(13.5),
            palette.text,
        );
        painter.text(Pos2::new(text_x, cy + 10.0), Align2::LEFT_CENTER, sub, theme::regular(11.0), palette.dim);
    };
    match hover {
        None => {
            painter.text(
                Pos2::new(rect.left() + 16.0, cy),
                Align2::LEFT_CENTER,
                "Elegí un emoji",
                theme::regular(13.0),
                palette.dim,
            );
        }
        Some(Hover::Unicode { emoji, name }) => {
            if twemoji::paint(ui, icon_rect, emoji) == twemoji::State::Failed {
                painter.text(icon_rect.center(), Align2::CENTER_CENTER, *emoji, theme::regular(28.0), palette.text);
            }
            two_lines(&format!(":{name}:"), "Click para agregar al mensaje");
        }
        Some(Hover::Custom { emoji, locked }) => {
            let mut image = egui::Image::new(crate::ui::anim::source(ctx, &emoji.url()))
                .fit_to_exact_size(Vec2::splat(36.0))
                .show_loading_spinner(false);
            if *locked {
                image = image.tint(Color32::from_white_alpha(90));
            }
            if let Ok(egui::load::TexturePoll::Ready { .. }) = image.load_for_size(ctx, Vec2::splat(36.0)) {
                image.paint_at(ui, icon_rect);
            }
            let sub = if *locked { "Requiere Nitro" } else { "Click para agregar al mensaje" };
            two_lines(&format!(":{}:", emoji.name), sub);
        }
    }
}

/// Una celda de la barra lateral. `active` = es la sección que se está viendo.
fn rail_cell(ui: &mut Ui, palette: &Palette, tooltip: &str, active: bool, paint: impl FnOnce(&mut Ui, Rect)) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(40.0, 38.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tooltip));
    if ui.is_rect_visible(rect) {
        if active || response.hovered() {
            let fill = if active { palette.surface_active } else { palette.surface_hover };
            ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
        }
        if active {
            let bar = Rect::from_center_size(Pos2::new(rect.left() + 1.5, rect.center().y), Vec2::new(3.0, 16.0));
            ui.painter().rect_filled(bar, CornerRadius::same(2), palette.accent);
        }
        paint(ui, rect);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip)
}

/// Una celda de emoji Unicode. Solo pide la imagen si la celda está a la
/// vista: el catálogo tiene cientos de emojis y no tiene sentido bajarse
/// los de las secciones que ni se ven.
fn unicode_cell(ui: &mut Ui, palette: &Palette, emoji: &str, name: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(38.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name));
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(9), palette.surface_hover);
        }
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(28.0));
        if twemoji::paint(ui, icon_rect, emoji) == twemoji::State::Failed {
            ui.painter()
                .text(rect.center(), Align2::CENTER_CENTER, emoji, theme::regular(22.0), palette.text);
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Igual que [`unicode_cell`] pero para un emoji personalizado de un server:
/// atenuado y con candadito si hace falta Nitro y no se tiene.
fn custom_cell(ui: &mut Ui, palette: &Palette, emoji: &CustomEmoji, locked: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(38.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &emoji.name));
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(9), palette.surface_hover);
        }
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(28.0));
        let mut image = egui::Image::new(crate::ui::anim::source(ui.ctx(), &emoji.url()))
            .fit_to_exact_size(Vec2::splat(28.0))
            .show_loading_spinner(false);
        if locked {
            image = image.tint(Color32::from_white_alpha(90));
        }
        match image.load_for_size(ui.ctx(), Vec2::splat(28.0)) {
            Ok(egui::load::TexturePoll::Ready { .. }) => image.paint_at(ui, icon_rect),
            _ => {
                ui.painter().rect_filled(icon_rect, 5.0, palette.surface_hover);
            }
        }
        if locked {
            let badge = Rect::from_center_size(rect.right_bottom() - Vec2::splat(8.0), Vec2::splat(13.0));
            ui.painter().circle_filled(badge.center(), 7.5, palette.overlay);
            theme::paint_icon(ui, Icon::Lock, badge, 10.0, palette.text);
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------
// Tests de la parte pura
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn mention(text: &str, caret: usize) -> Option<(usize, usize, String)> {
        active_mention(text, caret).map(|m| (m.start, m.end, m.query))
    }

    #[test]
    fn opens_on_a_bare_at_sign() {
        assert_eq!(mention("@", 1), Some((0, 1, String::new())));
        assert_eq!(mention("hola @", 6), Some((5, 6, String::new())));
    }

    #[test]
    fn query_is_what_sits_between_the_at_sign_and_the_caret() {
        assert_eq!(mention("hola @ana", 9), Some((5, 9, "ana".into())));
        // El cursor en medio: solo cuenta hasta el cursor.
        assert_eq!(mention("hola @ana", 7), Some((5, 7, "a".into())));
        assert_eq!(mention("a\n@pe", 5), Some((2, 5, "pe".into())));
    }

    #[test]
    fn does_not_open_inside_words_or_after_a_space() {
        assert_eq!(mention("ana@mail.com", 12), None);
        assert_eq!(mention("@ana hola", 9), None);
        assert_eq!(mention("sin arroba", 10), None);
        assert_eq!(mention("", 0), None);
    }

    #[test]
    fn caret_past_the_end_is_clamped() {
        assert_eq!(mention("@ab", 99), Some((0, 3, "ab".into())));
    }

    #[test]
    fn works_with_multibyte_text() {
        // `ñ` y emojis ocupan varios bytes pero un solo carácter.
        assert_eq!(mention("¡hola! @ñandú", 13), Some((7, 13, "ñandú".into())));
        assert_eq!(mention("😀 @a", 4), Some((2, 4, "a".into())));
    }

    #[test]
    fn replaces_by_character_index() {
        let mut s = "hola @an y más".to_string();
        replace_chars(&mut s, 5, 8, "<@1> ");
        assert_eq!(s, "hola <@1>  y más");
        let mut s = "😀@a".to_string();
        replace_chars(&mut s, 1, 3, "X");
        assert_eq!(s, "😀X");
        // Insertar sin borrar.
        let mut s = "ab".to_string();
        replace_chars(&mut s, 1, 1, "-");
        assert_eq!(s, "a-b");
    }

    #[test]
    fn char_index_is_read_from_debug_output() {
        #[derive(Debug)]
        struct CharIndex(usize);
        assert_eq!(char_index_to_usize(&CharIndex(5)), Some(5));
        assert_eq!(char_index_to_usize(&7usize), Some(7));
    }

    #[test]
    fn fold_ignores_case_and_accents() {
        assert_eq!(fold("Ñandú"), "nandu");
        assert_eq!(fold("CUMPLEAÑOS"), "cumpleanos");
    }

    #[test]
    fn match_rank_prefers_prefixes() {
        let q = fold("an");
        assert_eq!(match_rank("Ana", &q), Some(0));
        assert_eq!(match_rank("Juan Ana", &q), Some(1));
        assert_eq!(match_rank("Mariano", &q), Some(2));
        assert_eq!(match_rank("Pedro", &q), None);
        assert_eq!(match_rank("Pedro", ""), Some(0));
    }

    #[test]
    fn mention_tokens() {
        let user = MentionCandidate {
            kind: MentionKind::User,
            id: "42".into(),
            name: "Ana".into(),
            avatar_url: None,
            avatar_color: Color32::WHITE,
            color: None,
        };
        assert_eq!(user.token(), "<@42>");
        assert_eq!(MentionCandidate { kind: MentionKind::Role, ..user.clone() }.token(), "<@&42>");
        assert_eq!(MentionCandidate { kind: MentionKind::Everyone, ..user.clone() }.token(), "@everyone");
        assert_eq!(MentionCandidate { kind: MentionKind::Here, ..user }.token(), "@here");
    }

    fn group(home: bool, no_nitro: bool, emojis: &[(&str, &str, bool)]) -> EmojiGroup {
        EmojiGroup {
            name: "srv".into(),
            icon_url: None,
            icon_initial: "S".into(),
            icon_color: Color32::WHITE,
            emojis: emojis
                .iter()
                .map(|(id, name, animated)| CustomEmoji { id: (*id).into(), name: (*name).into(), animated: *animated })
                .collect(),
            stickers: Vec::new(),
            home,
            no_nitro,
        }
    }

    #[test]
    fn expands_picked_and_typed_shortcodes() {
        let groups = [group(true, false, &[("111", "pepe", false), ("222", "baile", true)])];
        let none = HashMap::new();
        assert_eq!(expand_shortcodes("hola :pepe:", &none, &groups), "hola <:pepe:111>");
        assert_eq!(expand_shortcodes(":baile: y :pepe:", &none, &groups), "<a:baile:222> y <:pepe:111>");
        // Los elegidos en el selector ganan aunque no estén en `groups`.
        let mut picked = HashMap::new();
        picked.insert("otro".to_string(), ("999".to_string(), false));
        assert_eq!(expand_shortcodes(":otro:", &picked, &[]), "<:otro:999>");
    }

    #[test]
    fn leaves_everything_else_alone() {
        let groups = [group(true, false, &[("111", "pepe", false)])];
        let none = HashMap::new();
        // No existe, ya está expandido, hora, un solo carácter.
        assert_eq!(expand_shortcodes(":nada:", &none, &groups), ":nada:");
        assert_eq!(expand_shortcodes("<:pepe:111>", &none, &groups), "<:pepe:111>");
        assert_eq!(expand_shortcodes("<a:pepe:111>", &none, &groups), "<a:pepe:111>");
        assert_eq!(expand_shortcodes("a las 10:30:45 hs", &none, &groups), "a las 10:30:45 hs");
        assert_eq!(expand_shortcodes("sin emojis", &none, &groups), "sin emojis");
    }

    #[test]
    fn locked_custom_emojis_are_not_expanded() {
        // Otro server (no `home`) y la cuenta sin Nitro: se ve pero no se usa.
        let groups = [group(false, true, &[("111", "pepe", false)])];
        assert_eq!(expand_shortcodes(":pepe:", &HashMap::new(), &groups), ":pepe:");
        // Con Nitro sí.
        let groups = [group(false, false, &[("111", "pepe", false)])];
        assert_eq!(expand_shortcodes(":pepe:", &HashMap::new(), &groups), "<:pepe:111>");
    }

    #[test]
    fn home_server_wins_when_names_collide() {
        let groups = [
            group(false, false, &[("1", "pepe", false)]),
            group(true, false, &[("2", "pepe", false)]),
        ];
        assert_eq!(expand_shortcodes(":pepe:", &HashMap::new(), &groups), "<:pepe:2>");
    }

    #[test]
    fn catalog_has_no_empty_sections() {
        assert!(!CATEGORIES.is_empty());
        for (title, icon, list) in CATEGORIES {
            assert!(!title.is_empty() && !icon.is_empty() && !list.is_empty());
            for (emoji, keywords) in *list {
                assert!(!emoji.is_empty() && !keywords.is_empty());
                // Las palabras clave se comparan contra `fold(consulta)`.
                assert_eq!(fold(keywords), *keywords, "palabras clave sin normalizar: {keywords}");
            }
        }
    }
}
