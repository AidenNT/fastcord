//! Slash commands en el compositor del chat.
//!
//! * Al escribir `/` al principio del mensaje se abre un menú con los comandos
//!   disponibles en el canal (flechas, Enter/Tab o click para elegir, Esc para
//!   cerrar). Si lo escrito no coincide con ningún comando, el mensaje se manda
//!   como siempre (`/shrug`, rutas, etc.).
//! * Elegir un comando reemplaza el compositor por un formulario con sus
//!   opciones. Enter (o "Enviar") ejecuta, Esc cancela.
//! * Las opciones NO obligatorias no se dibujan hasta que se eligen: al estar
//!   el cursor en el último campo (o en el cursor invisible tras el comando)
//!   se abre un menú "Opciones" que se recorre con las flechas y se acepta con
//!   Tab (o click). Si el comando no tiene obligatorias y solo una opcional,
//!   esa se agrega sola al elegir el comando, así lo que se escribe ya va ahí.
//! * Los campos de tipo lista (booleanos y opciones con `choices`) también
//!   reciben el foco con Tab: al tenerlo abren un menú con sus valores (↑/↓,
//!   Tab o Enter eligen y pasan al campo siguiente, Retroceso borra el valor).
//! * Los campos de usuario, rol, mencionable y canal se escriben como texto y
//!   buscan mientras se tipea (nombre, con o sin `@` / `#`): el menú usa los
//!   mismos candidatos que las menciones del chat (ver `ui::compose_menus`) y,
//!   para canales, los del server abierto. Elegir uno deja su nombre en el
//!   campo y manda su mención/ID. Un ID o `<@mención>` pegado también sirve.
//! * Cada opción se dibuja como una pastilla con dos partes: el NOMBRE de la
//!   opción y, al lado, su valor (en un recuadro más claro). Un usuario / rol /
//!   canal elegido se marca como una mención: avatar y nombre con el color del
//!   rol. Arriba del formulario va una barra con `/comando`, su descripción y
//!   una X para cancelar.
//! * Después de las opciones se puede seguir escribiendo: ese texto se manda
//!   como valor de la primera opción de texto libre que siga vacía (ver
//!   [`tail_target`]), así `/interact hug @alguien un abrazo` lleva el mensaje.
//!
//! ## Cómo se reparten el trabajo
//! Igual que `compose_menus`: `ui::chat` no tiene `&mut App`, así que se hablan
//! por la memoria de egui.
//! 1. el compositor avisa que hace falta el catálogo ([`set_wanted`]);
//! 2. `App::ui` lo pide a Discord si no lo tiene ([`take_wanted`]) y publica el
//!    del canal abierto ([`publish_catalog`]);
//! 3. al enviar el formulario el compositor deja un [`SlashInvocation`]
//!    ([`request_run`]) que `App::ui` recoge ([`take_invocation`]) y manda.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use egui::{
    Align, Align2, Area, Color32, Context, CornerRadius, Frame, Id, Margin, Order, Pos2, Rect, ScrollArea, Sense, Stroke,
    Ui, Vec2,
};
use serde_json::Value;

use crate::discord::frecency;
use crate::discord::slash::{SlashCatalog, SlashEntry, opt};
use crate::ui::compose_menus::{self as menus, MentionCandidate, MentionKind, fold, match_rank};
use crate::ui::emoji as twemoji;
use crate::ui::extra;
use crate::ui::theme::{self, Icon, Palette};

/// Resultados máximos al buscar.
const MAX_MATCHES: usize = 50;
/// Filas de "usados frecuentemente".
const FRECENT_ROWS: usize = 5;
const ROW_HEIGHT: f32 = 52.0;
const RAIL_WIDTH: f32 = 40.0;
const GAP: f32 = 8.0;
/// Alto máximo de la lista (después se desplaza).
const LIST_HEIGHT: f32 = 380.0;
/// Menú "Opciones" del formulario.
const OPTION_ROW_HEIGHT: f32 = 36.0;
const OPTION_LIST_HEIGHT: f32 = 240.0;

// ---------------------------------------------------------------------
// Puente con `App` (memoria de egui)
// ---------------------------------------------------------------------

fn catalog_key() -> Id {
    Id::new("ecord_slash_catalog")
}

fn wanted_key() -> Id {
    Id::new("ecord_slash_wanted")
}

fn invocation_key() -> Id {
    Id::new("ecord_slash_invocation")
}

/// Catálogo del canal abierto, si ya llegó.
pub fn catalog(ctx: &Context) -> Option<Arc<SlashCatalog>> {
    ctx.memory(|m| m.data.get_temp::<Option<Arc<SlashCatalog>>>(catalog_key())).flatten()
}

pub fn publish_catalog(ctx: &Context, catalog: Option<Arc<SlashCatalog>>) {
    ctx.memory_mut(|m| m.data.insert_temp(catalog_key(), catalog));
}

/// El compositor lo marca cada frame en que está escribiendo un `/...`.
pub fn set_wanted(ctx: &Context, wanted: bool) {
    if wanted {
        ctx.memory_mut(|m| m.data.insert_temp(wanted_key(), true));
    }
}

/// Lo lee `App::ui` (y lo apaga): `true` si hay que tener el catálogo a mano.
pub fn take_wanted(ctx: &Context) -> bool {
    ctx.memory_mut(|m| {
        let wanted = m.data.get_temp::<bool>(wanted_key()).unwrap_or(false);
        if wanted {
            m.data.insert_temp(wanted_key(), false);
        }
        wanted
    })
}

fn channel_query_key() -> Id {
    Id::new("ecord_slash_channel_query")
}

fn channel_results_key() -> Id {
    Id::new("ecord_slash_channel_results")
}

/// Un canal que se puede elegir en una opción de tipo canal.
#[derive(Clone, Debug)]
pub struct ChannelCandidate {
    pub id: String,
    pub name: String,
    /// Categoría en la que está (para la descripción de la fila).
    pub category: String,
}

/// Canales ya filtrados para UNA consulta concreta.
#[derive(Clone, Debug, Default)]
pub struct ChannelResults {
    pub query: String,
    pub items: Vec<ChannelCandidate>,
}

/// El formulario pide canales que coincidan con `query` (hay que repetirlo
/// cada frame mientras haga falta, así no queda un pedido viejo colgado).
pub fn request_channels(ctx: &Context, query: &str) {
    ctx.memory_mut(|m| m.data.insert_temp(channel_query_key(), query.to_string()));
}

/// Lo lee `App::ui` (y lo apaga) para armar los candidatos.
pub fn take_channel_query(ctx: &Context) -> Option<String> {
    ctx.memory_mut(|m| {
        let query: Option<String> = m.data.get_temp(channel_query_key());
        if query.is_some() {
            m.data.remove::<String>(channel_query_key());
        }
        query
    })
}

pub fn publish_channel_results(ctx: &Context, query: String, items: Vec<ChannelCandidate>) {
    let results = Arc::new(ChannelResults { query, items });
    ctx.memory_mut(|m| m.data.insert_temp(channel_results_key(), results));
}

/// Los canales publicados, solo si son para ESTA consulta.
pub fn channel_results_for(ctx: &Context, query: &str) -> Option<Arc<ChannelResults>> {
    ctx.memory(|m| m.data.get_temp::<Arc<ChannelResults>>(channel_results_key())).filter(|r| r.query == query)
}

/// Un comando listo para ejecutar.
#[derive(Clone)]
pub struct SlashInvocation {
    pub channel_id: String,
    pub guild_id: Option<String>,
    pub command: Arc<crate::discord::slash::AppCommand>,
    /// Comando, grupo y subcomando que se eligieron (el último es la hoja).
    pub path: Vec<String>,
    /// `data.options` ya armado (con grupo y subcomando si corresponde).
    pub options: Vec<Value>,
}

pub fn request_run(ctx: &Context, invocation: SlashInvocation) {
    ctx.memory_mut(|m| m.data.insert_temp(invocation_key(), Some(invocation)));
}

pub fn take_invocation(ctx: &Context) -> Option<SlashInvocation> {
    ctx.memory_mut(|m| {
        let pending = m.data.get_temp::<Option<SlashInvocation>>(invocation_key()).flatten();
        if pending.is_some() {
            m.data.insert_temp(invocation_key(), None::<SlashInvocation>);
        }
        pending
    })
}

// ---------------------------------------------------------------------
// Menú de comandos
// ---------------------------------------------------------------------
//
// Mismo diseño que el selector de emojis y que el de comandos del cliente
// oficial: a la izquierda un riel con "usados frecuentemente" y el ícono de
// cada app; a la derecha la lista agrupada por app, con un título por sección
// y filas de dos líneas (nombre + opciones, descripción). Al escribir algo
// después de la `/` el riel se oculta y queda una sola lista con lo que
// coincide.

/// Lo tipeado después de la `/`, si el compositor está "buscando un comando":
/// el texto empieza con `/`, es de una sola línea y no es solo una barra
/// pegada a otra (`//`, `/ `).
pub fn browse_query(text: &str) -> Option<String> {
    let rest = text.strip_prefix('/')?;
    if rest.contains('\n') || rest.starts_with(['/', ' ']) {
        return None;
    }
    // Un comando no llega a tanto: es un mensaje que empieza con barra.
    (rest.chars().count() <= 64).then(|| rest.to_string())
}

/// Qué muestra una sección del menú.
#[derive(Clone)]
pub enum SectionKind {
    /// "Usados frecuentemente" (los de `frecency`).
    Frecent,
    /// Resultado de una búsqueda: una sola sección, sin riel.
    Search,
    /// Los comandos de una app.
    App { icon_url: Option<String> },
}

/// Un tramo de filas del menú con su título.
#[derive(Clone)]
pub struct Section {
    pub kind: SectionKind,
    pub title: String,
    /// Filas de [`MenuState::matches`] que le tocan (`start..end`).
    pub start: usize,
    pub end: usize,
}

/// Estado del menú entre frames.
#[derive(Clone, Default)]
pub struct MenuState {
    pub selected: usize,
    pub dismissed: bool,
    query: String,
    catalog_len: usize,
    /// Índices dentro de `catalog.entries`, en el orden en que se dibujan las
    /// filas (un comando usado seguido aparece en "frecuentes" y en su app).
    pub matches: Vec<usize>,
    pub sections: Vec<Section>,
}

impl MenuState {
    pub fn load(ctx: &Context, id: Id) -> Self {
        ctx.memory(|m| m.data.get_temp(id)).unwrap_or_default()
    }

    pub fn store(self, ctx: &Context, id: Id) {
        ctx.memory_mut(|m| m.data.insert_temp(id, self));
    }

    /// Recalcula filas y secciones solo si cambió la consulta o el catálogo
    /// (con cientos de comandos no se arma todo en cada frame).
    pub fn refresh(&mut self, catalog: &SlashCatalog, query: &str) {
        if self.query == query && self.catalog_len == catalog.entries.len() {
            return;
        }
        self.query = query.to_string();
        self.catalog_len = catalog.entries.len();
        let (matches, sections) = layout(catalog, query);
        self.matches = matches;
        self.sections = sections;
        self.selected = 0;
    }
}

/// Entradas que coinciden con `query` (mejores primero, hasta [`MAX_MATCHES`]).
/// Se busca contra el nombre completo (`permisos usuario ver`) y cada palabra
/// tipeada tiene que coincidir con alguna parte de él, así que sirve escribir
/// `/perm ver`.
pub fn filter(catalog: &SlashCatalog, query: &str) -> Vec<usize> {
    let tokens: Vec<String> = query.split_whitespace().map(fold).collect();
    let mut ranked: Vec<(u8, usize)> = catalog
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let mut worst = 0u8;
            for token in &tokens {
                worst = worst.max(match_rank(&entry.display, token)?);
            }
            Some((worst, index))
        })
        .collect();
    // `sort_by_key` es estable: dentro de cada grado queda el orden alfabético.
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().take(MAX_MATCHES).map(|(_, index)| index).collect()
}

/// Los comandos más usados de la cuenta que existen en este catálogo. La clave
/// de `frecency` es el id del comando, con `\0subcomando[:server]` si se usó
/// un subcomando.
fn frecent_entries(catalog: &SlashCatalog) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for key in frecency::top_commands(FRECENT_ROWS * 8) {
        let (id, rest) = key.split_once('\0').unwrap_or((key.as_str(), ""));
        let sub = rest.split(':').next().unwrap_or_default();
        let same_command = |e: &Arc<SlashEntry>| e.command.id == id;
        let found = catalog
            .entries
            .iter()
            .position(|e| {
                same_command(e)
                    && if sub.is_empty() {
                        e.path.len() == 1
                    } else {
                        e.path.len() > 1 && e.path.last().map(String::as_str) == Some(sub)
                    }
            })
            .or_else(|| catalog.entries.iter().position(same_command));
        if let Some(index) = found {
            if !out.contains(&index) {
                out.push(index);
                if out.len() == FRECENT_ROWS {
                    break;
                }
            }
        }
    }
    out
}

/// Arma las filas y secciones del menú para `query`.
pub fn layout(catalog: &SlashCatalog, query: &str) -> (Vec<usize>, Vec<Section>) {
    if !query.trim().is_empty() {
        let matches = filter(catalog, query);
        let end = matches.len();
        let title = format!("Comandos que coinciden con /{}", query.trim());
        return (matches, vec![Section { kind: SectionKind::Search, title, start: 0, end }]);
    }

    let mut matches: Vec<usize> = Vec::new();
    let mut sections: Vec<Section> = Vec::new();

    let frecent = frecent_entries(catalog);
    if !frecent.is_empty() {
        sections.push(Section {
            kind: SectionKind::Frecent,
            title: "Usados frecuentemente".to_string(),
            start: 0,
            end: frecent.len(),
        });
        matches.extend(frecent);
    }

    // Una sección por app: primero las que más se usan, después por nombre.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut group_of: HashMap<&str, usize> = HashMap::new();
    for (index, entry) in catalog.entries.iter().enumerate() {
        let group = *group_of.entry(entry.app_id.as_str()).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[group].push(index);
    }
    let app_rank = frecency::top_applications(64);
    groups.sort_by_cached_key(|items| {
        let first = &catalog.entries[items[0]];
        let rank = app_rank.iter().position(|id| *id == first.app_id).unwrap_or(usize::MAX);
        (rank, fold(&first.app_name))
    });
    for items in groups {
        let first = &catalog.entries[items[0]];
        let title = if first.app_name.is_empty() { "Comandos".to_string() } else { first.app_name.clone() };
        let start = matches.len();
        matches.extend(items.iter().copied());
        sections.push(Section {
            kind: SectionKind::App { icon_url: first.app_icon_url.clone() },
            title,
            start,
            end: matches.len(),
        });
    }
    (matches, sections)
}

/// Qué pasó con el menú este frame (mismo criterio que el de menciones: se
/// elige al APRETAR, porque soltar fuera del campo le quita el foco).
pub struct MenuOutcome {
    pub hovered: Option<usize>,
    pub pressed: Option<usize>,
}

fn menu_frame<R>(ctx: &Context, palette: &Palette, id: Id, anchor: Pos2, width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    Area::new(id)
        .order(Order::Foreground)
        .pivot(Align2::LEFT_BOTTOM)
        .fixed_pos(anchor)
        .show(ctx, |ui| {
            Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::radius() + 6))
                .inner_margin(Margin::same(8))
                .shadow(egui::epaint::Shadow { offset: [0, 12], blur: 32, spread: 0, color: palette.shadow })
                .show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    add(ui)
                })
                .inner
        })
        .inner
}

/// Inicial del nombre de una app (para el círculo cuando no tiene ícono).
fn initial(name: &str) -> String {
    name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()
}

/// Color estable por nombre para el círculo de las apps sin ícono.
fn fallback_color(name: &str) -> Color32 {
    const PALETTE: [Color32; 6] = [
        Color32::from_rgb(0x58, 0x65, 0xf2),
        Color32::from_rgb(0x3b, 0xa5, 0x5d),
        Color32::from_rgb(0xed, 0x42, 0x45),
        Color32::from_rgb(0xeb, 0x45, 0x9e),
        Color32::from_rgb(0xfa, 0xa6, 0x1a),
        Color32::from_rgb(0x2b, 0x9c, 0xc4),
    ];
    let hash = name.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(u32::from(b)));
    PALETTE[hash as usize % PALETTE.len()]
}

pub fn show_slash_menu(
    ctx: &Context,
    palette: &Palette,
    id: Id,
    anchor: Pos2,
    width: f32,
    catalog: &SlashCatalog,
    state: &MenuState,
) -> MenuOutcome {
    let mut outcome = MenuOutcome { hovered: None, pressed: None };
    let pressed_now = ctx.input(|i| i.pointer.primary_pressed());
    // Id propio del menú: el "Buscando comandos…" es un `Area` chiquito y, si
    // compartiera id, egui recordaría ese tamaño y el menú nacería recortado.
    let id = id.with("menu");
    let jump_id = id.with("jump");
    let active_id = id.with("active");
    let last_selected_id = id.with("last_selected");
    let last_selected: usize = ctx.memory(|m| m.data.get_temp(last_selected_id)).unwrap_or(usize::MAX);
    let with_rail = state.sections.first().is_some_and(|s| !matches!(s.kind, SectionKind::Search));

    menu_frame(ctx, palette, id, anchor, width, |ui| {
        let inner_width = ui.available_width();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            let active: usize = ctx.memory(|m| m.data.get_temp(active_id)).unwrap_or(0);

            // ---- Riel: frecuentes + una celda por app.
            if with_rail {
                ui.vertical(|ui| {
                    ScrollArea::vertical()
                        .id_salt(id.with("rail"))
                        .max_height(LIST_HEIGHT)
                        .min_scrolled_height(LIST_HEIGHT)
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.set_width(RAIL_WIDTH);
                            ui.spacing_mut().item_spacing.y = 4.0;
                            for (n, section) in state.sections.iter().enumerate() {
                                let clicked = rail_cell(ui, palette, &section.title, n == active, |ui, rect| {
                                    match &section.kind {
                                        SectionKind::Frecent => {
                                            theme::paint_icon(ui, Icon::Clock, rect, 20.0, palette.text);
                                        }
                                        SectionKind::App { icon_url } => extra::avatar(
                                            ui,
                                            rect.center(),
                                            14.0,
                                            icon_url.as_deref(),
                                            fallback_color(&section.title),
                                            &initial(&section.title),
                                            palette,
                                        ),
                                        SectionKind::Search => {}
                                    }
                                })
                                .clicked();
                                if clicked {
                                    ctx.memory_mut(|m| {
                                        m.data.insert_temp(jump_id, n);
                                        m.data.insert_temp(active_id, n);
                                    });
                                }
                            }
                        });
                });
            }

            // ---- Lista agrupada.
            let list_width = if with_rail { inner_width - RAIL_WIDTH - GAP } else { inner_width };
            ui.vertical(|ui| {
                ui.set_width(list_width);
                ScrollArea::vertical()
                    .id_salt(id.with("list"))
                    .max_height(LIST_HEIGHT)
                    .min_scrolled_height(LIST_HEIGHT)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let jump: Option<usize> = ctx.memory(|m| m.data.get_temp(jump_id));
                        let clip_top = ui.clip_rect().top();
                        // Sección "activa" = la última cuyo título ya llegó arriba.
                        let mut spy = 0usize;
                        for (n, section) in state.sections.iter().enumerate() {
                            let header = section_header(ui, palette, section);
                            if jump == Some(n) {
                                ui.scroll_to_rect(header.rect, Some(Align::TOP));
                            }
                            if header.rect.top() <= clip_top + 8.0 {
                                spy = n;
                            }
                            for row in section.start..section.end {
                                let Some(entry) = state.matches.get(row).and_then(|i| catalog.entries.get(*i))
                                else {
                                    continue;
                                };
                                let response = command_row(ui, palette, entry, row == state.selected);
                                // Con las flechas, la fila elegida tiene que quedar a la vista.
                                if row == state.selected && state.selected != last_selected {
                                    ui.scroll_to_rect(response.rect, None);
                                }
                                if response.hovered() {
                                    outcome.hovered = Some(row);
                                    if pressed_now {
                                        outcome.pressed = Some(row);
                                    }
                                }
                            }
                            ui.add_space(8.0);
                        }
                        if jump.is_some() {
                            // El salto ya se hizo: se consume (el resaltado lo dejó
                            // puesto el click).
                            ctx.memory_mut(|m| m.data.remove::<usize>(jump_id));
                            ctx.request_repaint();
                        } else {
                            ctx.memory_mut(|m| m.data.insert_temp(active_id, spy));
                        }
                    });
            });
        });
    });
    ctx.memory_mut(|m| m.data.insert_temp(last_selected_id, state.selected));
    outcome
}

/// Menú mientras el catálogo todavía se está pidiendo.
pub fn show_slash_loading(ctx: &Context, palette: &Palette, id: Id, anchor: Pos2, width: f32) {
    menu_frame(ctx, palette, id.with("loading"), anchor, width, |ui| {
        ui.horizontal(|ui| {
            theme::spinner(ui, 14.0, palette.dim);
            theme::text(ui, "Buscando comandos…", theme::regular(12.5), palette.dim);
        });
    });
}

/// Una celda del riel. `active` = es la sección que se está viendo.
fn rail_cell(ui: &mut Ui, palette: &Palette, tooltip: &str, active: bool, paint: impl FnOnce(&mut Ui, Rect)) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(RAIL_WIDTH, 38.0), Sense::click());
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

/// Título de sección: ícono (reloj o ícono de la app) y nombre en negrita.
fn section_header(ui: &mut Ui, palette: &Palette, section: &Section) -> egui::Response {
    ui.horizontal(|ui| {
        ui.set_min_height(32.0);
        ui.add_space(8.0);
        match &section.kind {
            SectionKind::Frecent => {
                theme::icon(ui, Icon::Clock, 16.0, palette.text);
            }
            SectionKind::App { icon_url } => {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                extra::avatar(
                    ui,
                    rect.center(),
                    10.0,
                    icon_url.as_deref(),
                    fallback_color(&section.title),
                    &initial(&section.title),
                    palette,
                );
            }
            SectionKind::Search => {}
        }
        let color = if matches!(section.kind, SectionKind::Search) { palette.secondary } else { palette.text };
        theme::text(ui, section.title.clone(), theme::semibold(14.0), color);
    })
    .response
}

fn command_row(ui: &mut Ui, palette: &Palette, entry: &SlashEntry, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
    let label = format!("/{}", entry.display);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &label));
    if ui.is_rect_visible(rect) {
        if selected || response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(8), palette.surface_hover);
        }
        let painter = ui.painter().clone();

        // Ícono de la app a la izquierda.
        let avatar_radius = 18.0;
        let avatar_center = Pos2::new(rect.left() + 10.0 + avatar_radius, rect.center().y);
        extra::avatar(
            ui,
            avatar_center,
            avatar_radius,
            entry.app_icon_url.as_deref(),
            fallback_color(&entry.app_name),
            &initial(&entry.app_name),
            palette,
        );

        let text_left = avatar_center.x + avatar_radius + 12.0;
        let right = rect.right() - 12.0;
        let app_width = if entry.app_name.is_empty() { 0.0 } else { (rect.width() * 0.22).clamp(60.0, 120.0) };
        let text_right = right - app_width - if app_width > 0.0 { 10.0 } else { 0.0 };

        // Nombre de la app a la derecha, centrado en la fila.
        if app_width > 0.0 {
            let mut job = egui::text::LayoutJob::simple(entry.app_name.clone(), theme::regular(12.5), palette.dim, app_width);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let galley = painter.layout_job(job);
            let pos = Pos2::new(right - galley.size().x, rect.center().y - galley.size().y / 2.0);
            painter.galley(pos, galley, palette.dim);
        }

        // Línea 1: `/comando`, opciones obligatorias como etiquetas y `+N opcional`.
        let y1 = rect.top() + 18.0;
        let name = painter.layout_no_wrap(label.clone(), theme::semibold(14.0), palette.text);
        let name_width = name.size().x.min((text_right - text_left).max(40.0));
        let name_clip = Rect::from_min_max(Pos2::new(text_left, rect.top()), Pos2::new(text_left + name_width, rect.bottom()));
        painter
            .with_clip_rect(name_clip)
            .galley(Pos2::new(text_left, y1 - name.size().y / 2.0), name, palette.text);
        let mut x = text_left + name_width + 8.0;
        for arg in entry.args.iter().filter(|a| a.required).take(3) {
            let galley = painter.layout_no_wrap(arg.name.clone(), theme::regular(11.5), palette.secondary);
            let chip_width = galley.size().x + 12.0;
            if x + chip_width > text_right {
                break;
            }
            let chip = Rect::from_min_size(Pos2::new(x, y1 - 9.0), Vec2::new(chip_width, 18.0));
            painter.rect_filled(chip, CornerRadius::same(4), palette.surface_active);
            painter.galley(Pos2::new(chip.left() + 6.0, y1 - galley.size().y / 2.0), galley, palette.secondary);
            x += chip_width + 4.0;
        }
        let optional = entry.args.iter().filter(|a| !a.required).count();
        if optional > 0 {
            let text = if optional == 1 { "+1 opcional".to_string() } else { format!("+{optional} opcionales") };
            let galley = painter.layout_no_wrap(text, theme::regular(12.0), palette.dim);
            if x + 12.0 + galley.size().x <= text_right {
                painter.line_segment(
                    [Pos2::new(x + 4.0, y1 - 7.0), Pos2::new(x + 4.0, y1 + 7.0)],
                    Stroke::new(1.0, palette.outline),
                );
                painter.galley(Pos2::new(x + 12.0, y1 - galley.size().y / 2.0), galley, palette.dim);
            }
        }

        // Línea 2: descripción (con emojis dibujados como imagen).
        if !entry.description.is_empty() && text_right > text_left + 20.0 {
            twemoji::paint_line(
                ui,
                Pos2::new(text_left, rect.top() + 37.0),
                &entry.description,
                theme::regular(12.5),
                palette.dim,
                text_right - text_left,
            );
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------
// Formulario de opciones
// ---------------------------------------------------------------------

/// Comando elegido y lo que se va completando. `values[i]` es el texto de
/// `entry.args[i]` (para opciones con `choices`, el valor de la elegida).
#[derive(Clone)]
pub struct ActiveSlash {
    pub entry: Arc<SlashEntry>,
    pub values: Vec<String>,
    pub error: Option<String>,
    /// Índices (de `entry.args`) de las opciones NO obligatorias que ya se
    /// agregaron al formulario, en el orden en que se eligieron.
    added: Vec<usize>,
    /// Fila resaltada del menú "Opciones".
    popup_selected: usize,
    /// Se usaron las flechas: recién ahí Enter elige la opción en vez de enviar.
    popup_navigated: bool,
    /// Esc cerró el menú "Opciones" (vuelve al tipear o con "+N más").
    popup_dismissed: bool,
    /// Último filtro del menú (lo tipeado en el campo invisible).
    popup_filter: String,
    /// Fila resaltada del menú de valores (booleanos y `choices`).
    value_selected: usize,
    /// Esc cerró el menú de valores del campo actual.
    value_dismissed: bool,
    /// Último campo que tuvo el foco (al llegar a otro se reabren sus menús).
    last_focused: Option<usize>,
    /// Por opción de usuario/rol/canal: lo elegido en el menú como
    /// `(texto que se ve en el campo, mención que se manda)`. Vale mientras el
    /// campo siga con ese texto; si se edita, se manda lo que haya escrito.
    picked: Vec<Option<(String, String)>>,
    /// Cómo se dibuja lo elegido en `picked` (avatar y color de la mención).
    look: Vec<Option<PickedLook>>,
    /// Campo al que hay que darle el foco (se repite cada frame hasta que lo
    /// tiene).
    focus: Option<usize>,
    /// Primer frame del formulario: el foco va al primer campo de texto.
    focus_pending: bool,
    /// Campo invisible que sigue al comando cuando no hay ningún campo de
    /// texto (`/leaderboard`): da dónde tener el cursor para poder borrar el
    /// comando con Retroceso. Lo que se escriba ahí se descarta.
    tail: String,
    focus_pending_tail: bool,
}

/// Cómo se ve un usuario / rol / canal elegido dentro de su pastilla.
#[derive(Clone, Debug)]
struct PickedLook {
    /// Los canales no llevan avatar.
    show_avatar: bool,
    avatar_url: Option<String>,
    avatar_color: Color32,
    initial: String,
    /// Color del rol (si no tiene, se usa el de acento).
    color: Option<Color32>,
}

/// Opciones que se muestran apenas se elige el comando: ninguna. Las
/// obligatorias se dibujan siempre y las opcionales solo aparecen en el menú
/// "Opciones" (se agregan con Tab, Enter tras las flechas o click).
fn initial_added(_entry: &SlashEntry) -> Vec<usize> {
    Vec::new()
}

pub fn activate(ctx: &Context, id: Id, entry: Arc<SlashEntry>) {
    let values = vec![String::new(); entry.args.len()];
    let added = initial_added(&entry);
    let picked = vec![None; entry.args.len()];
    let look = vec![None; entry.args.len()];
    // Sin campos visibles, el foco inicial va al cursor invisible.
    let no_fields = field_layout(&entry, &added).1.is_empty();
    store_active(
        ctx,
        id,
        Some(ActiveSlash {
            entry,
            values,
            error: None,
            added,
            popup_selected: 0,
            popup_navigated: false,
            popup_dismissed: false,
            popup_filter: String::new(),
            value_selected: 0,
            value_dismissed: false,
            last_focused: None,
            picked,
            look,
            focus: None,
            focus_pending: true,
            tail: String::new(),
            focus_pending_tail: no_fields,
        }),
    );
}

impl ActiveSlash {
    /// El comando convertido otra vez en texto (`/permisos usuario ver`) para
    /// devolverlo al compositor. El texto de al lado (que al enviar iría a una
    /// opción) se descarta: ya no hay campos donde ponerlo.
    pub fn command_text(&mut self) -> String {
        self.tail.clear();
        format!("/{}", self.entry.display)
    }
}

pub fn load_active(ctx: &Context, id: Id) -> Option<ActiveSlash> {
    ctx.memory(|m| m.data.get_temp::<Option<ActiveSlash>>(id)).flatten()
}

pub fn store_active(ctx: &Context, id: Id, active: Option<ActiveSlash>) {
    ctx.memory_mut(|m| m.data.insert_temp(id, active));
}

pub enum FormOutcome {
    None,
    /// Esc: se descarta el comando.
    Cancel,
    /// Retroceso sobre el comando: pasa de interacción a TEXTO (`/comando`
    /// en el compositor, con las sugerencias otra vez). Ver
    /// [`ActiveSlash::command_text`].
    ToText,
    Submit(Vec<Value>),
}

/// ¿La opción se escribe con un campo de texto? (las demás usan un desplegable).
fn is_text_option(option: &crate::discord::slash::CommandOption) -> bool {
    option.choices.is_empty() && option.kind != opt::BOOLEAN && option.kind != opt::ATTACHMENT
}

/// Texto tenue de un campo vacío (el nombre de la opción ya se ve en su
/// pastilla, así que acá va qué se espera escribir).
fn hint_for(option: &crate::discord::slash::CommandOption) -> String {
    match option.kind {
        opt::USER => "usuario",
        opt::ROLE => "rol",
        opt::MENTIONABLE => "usuario o rol",
        opt::CHANNEL => "canal",
        opt::INTEGER | opt::NUMBER => "número",
        _ => "texto",
    }
    .to_string()
}

/// Opción que recibe lo escrito DESPUÉS de las opciones (el texto de al lado):
/// la primera de texto libre (sin lista de valores) que siga vacía.
fn tail_target(entry: &SlashEntry, values: &[String]) -> Option<usize> {
    entry.args.iter().enumerate().find_map(|(i, option)| {
        let empty = values.get(i).is_none_or(|v| v.trim().is_empty());
        (option.kind == opt::STRING && option.choices.is_empty() && empty).then_some(i)
    })
}

/// ¿El campo se elige de una lista (booleano u opción con `choices`)? Se
/// maneja con el teclado igual que uno de texto, pero con un menú de valores.
fn is_pick_option(option: &crate::discord::slash::CommandOption) -> bool {
    !is_text_option(option) && option.kind != opt::ATTACHMENT
}

/// Usuario, rol o mencionable: se buscan entre los candidatos de las menciones.
fn is_mention_option(option: &crate::discord::slash::CommandOption) -> bool {
    matches!(option.kind, opt::USER | opt::ROLE | opt::MENTIONABLE)
}

/// Usuario, rol, mencionable o canal: campos de texto con menú de búsqueda.
fn is_entity_option(option: &crate::discord::slash::CommandOption) -> bool {
    is_mention_option(option) || option.kind == opt::CHANNEL
}

/// Qué se busca según lo escrito en un campo de entidad, y si hay que buscar:
/// no si ya es lo elegido en el menú, ni si es un ID / mención pegada.
fn entity_search(raw: &str, picked: Option<&(String, String)>) -> (String, bool) {
    let trimmed = raw.trim();
    let query = trimmed.trim_start_matches(['@', '#']).to_string();
    let resolved = picked.is_some_and(|(label, _)| label == raw);
    let pasted_id = trimmed.starts_with('<') || (!trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()));
    (query, !resolved && !pasted_id)
}

/// `(texto del campo, mención)` de la fila `row` del menú de entidades.
fn entity_choice(items: &[MentionCandidate], channels: &[ChannelCandidate], row: usize) -> Option<(String, String)> {
    if !items.is_empty() {
        items.get(row).map(|c| (format!("@{}", c.name), c.token()))
    } else {
        channels.get(row).map(|c| (format!("#{}", c.name), format!("<#{}>", c.id)))
    }
}

/// Elige un usuario / rol / canal: deja su nombre en el campo, recuerda su
/// mención y pasa al campo siguiente.
fn pick_entity(
    active: &mut ActiveSlash,
    index: usize,
    label: String,
    token: String,
    look: Option<PickedLook>,
    focusable: &[usize],
) {
    pick_value(active, index, label.clone(), focusable);
    if let Some(slot) = active.picked.get_mut(index) {
        *slot = Some((label, token));
    }
    if let Some(slot) = active.look.get_mut(index) {
        *slot = look;
    }
}

/// Cómo dibujar la fila `row` del menú de entidades una vez elegida.
fn entity_look(items: &[MentionCandidate], channels: &[ChannelCandidate], row: usize, dim: Color32) -> Option<PickedLook> {
    if !items.is_empty() {
        items.get(row).map(|c| PickedLook {
            show_avatar: true,
            avatar_url: c.avatar_url.clone(),
            avatar_color: c.avatar_color,
            initial: initial(&c.name),
            color: c.color,
        })
    } else {
        channels.get(row).map(|_| PickedLook {
            show_avatar: false,
            avatar_url: None,
            avatar_color: dim,
            initial: String::new(),
            color: None,
        })
    }
}

/// Lo escrito en cada opción, con lo elegido en el menú convertido a su
/// mención (lo que espera la API).
fn resolved_values(active: &ActiveSlash) -> Vec<String> {
    let mut values: Vec<String> = active
        .values
        .iter()
        .enumerate()
        .map(|(i, value)| match active.picked.get(i).and_then(|p| p.as_ref()) {
            Some((label, token)) if value == label => token.clone(),
            _ => value.clone(),
        })
        .collect();
    // Lo escrito a continuación de las opciones va a la primera de texto libre
    // que siga vacía.
    let text = active.tail.trim();
    if !text.is_empty() {
        if let Some(i) = tail_target(&active.entry, &values) {
            values[i] = text.to_string();
        }
    }
    values
}

/// Valores que ofrece un campo de lista: `(nombre mostrado, valor)`.
fn value_choices(option: &crate::discord::slash::CommandOption) -> Vec<(String, String)> {
    if !option.choices.is_empty() {
        option.choices.iter().map(|c| (c.name.clone(), c.value_text())).collect()
    } else if option.kind == opt::BOOLEAN {
        vec![("Verdadero".to_string(), "true".to_string()), ("Falso".to_string(), "false".to_string())]
    } else {
        Vec::new()
    }
}

/// Consume UNA pulsación de Retroceso (no la repetición, sin modificadores).
fn take_backspace(ctx: &Context) -> bool {
    ctx.input_mut(|i| {
        let found = i.events.iter().position(|e| {
            matches!(e, egui::Event::Key { key: egui::Key::Backspace, pressed: true, repeat: false, modifiers, .. }
                if modifiers.is_none())
        });
        if let Some(pos) = found {
            i.events.remove(pos);
        }
        found.is_some()
    })
}

/// Elige un valor en un campo de lista y pasa al campo siguiente (si hay).
fn pick_value(active: &mut ActiveSlash, index: usize, value: String, focusable: &[usize]) {
    active.values[index] = value;
    active.value_dismissed = true;
    active.value_selected = 0;
    if let Some(pos) = focusable.iter().position(|&f| f == index) {
        if let Some(next) = focusable.get(pos + 1) {
            active.focus = Some(*next);
        } else {
            // Último campo: se sigue escribiendo a continuación.
            active.focus_pending_tail = true;
        }
    }
}

/// ¿Se puede agregar desde el menú "Opciones"? (las adjuntas no están
/// soportadas, así que no se ofrecen).
fn is_addable(option: &crate::discord::slash::CommandOption) -> bool {
    !option.required && option.kind != opt::ATTACHMENT
}

/// Opciones no obligatorias que todavía no están en el formulario, filtradas
/// por `filter` (mejores primero; con el filtro vacío, en el orden del comando).
fn remaining_options(entry: &SlashEntry, added: &[usize], filter: &str) -> Vec<usize> {
    let query = fold(filter.trim());
    let mut ranked: Vec<(u8, usize)> = (0..entry.args.len())
        .filter(|&i| is_addable(&entry.args[i]) && !added.contains(&i))
        .filter_map(|i| match_rank(&entry.args[i].name, &query).map(|rank| (rank, i)))
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, i)| i).collect()
}

/// Campos que se dibujan (obligatorios + agregados) y, de ellos, los que pueden
/// tener el foco.
fn field_layout(entry: &SlashEntry, added: &[usize]) -> (Vec<usize>, Vec<usize>) {
    let mut visible: Vec<usize> = (0..entry.args.len()).filter(|&i| entry.args[i].required).collect();
    visible.extend(added.iter().copied().filter(|&i| i < entry.args.len() && !entry.args[i].required));
    // Todos reciben el foco (texto y listas); solo las adjuntas quedan afuera.
    let focusable = visible.iter().copied().filter(|&i| entry.args[i].kind != opt::ATTACHMENT).collect();
    (visible, focusable)
}

/// Agrega una opcional al formulario y le da el foco (si es de texto).
fn add_option(active: &mut ActiveSlash, entry: &SlashEntry, arg: usize) {
    if !active.added.contains(&arg) {
        active.added.push(arg);
    }
    active.tail.clear();
    active.popup_filter.clear();
    active.popup_selected = 0;
    active.popup_navigated = false;
    active.popup_dismissed = false;
    active.focus = Some(arg);
}

struct ListOutcome {
    hovered: Option<usize>,
    pressed: Option<usize>,
}

/// Menú flotante sobre el formulario (para "Opciones" y para los valores de un
/// campo de lista): título y filas `(nombre, descripción)`, con el nombre a la
/// izquierda y la descripción a la derecha. Como el de menciones, se elige al
/// APRETAR (soltar fuera del campo le quitaría el foco).
#[allow(clippy::too_many_arguments)]
fn show_popup_list(
    ctx: &Context,
    palette: &Palette,
    id: Id,
    anchor: Pos2,
    width: f32,
    title: &str,
    rows: &[(String, String)],
    selected: usize,
) -> ListOutcome {
    let mut outcome = ListOutcome { hovered: None, pressed: None };
    let pressed_now = ctx.input(|i| i.pointer.primary_pressed());
    let moved = ctx.input(|i| i.pointer.delta() != Vec2::ZERO);
    let last_id = id.with("last_selected");
    let last_selected: usize = ctx.memory(|m| m.data.get_temp(last_id)).unwrap_or(usize::MAX);

    menu_frame(ctx, palette, id, anchor, width, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.horizontal(|ui| {
            ui.set_min_height(26.0);
            ui.add_space(8.0);
            theme::text(ui, title.to_string(), theme::semibold(13.0), palette.dim);
        });
        ScrollArea::vertical()
            .id_salt(id.with("scroll"))
            .max_height(OPTION_LIST_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (row, (name_text, description)) in rows.iter().enumerate() {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(ui.available_width(), OPTION_ROW_HEIGHT), Sense::click());
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name_text)
                    });
                    if ui.is_rect_visible(rect) {
                        if row == selected || response.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(8), palette.surface_hover);
                        }
                        let painter = ui.painter().clone();
                        let left = rect.left() + 10.0;
                        let right = rect.right() - 12.0;
                        let name = painter.layout_no_wrap(name_text.clone(), theme::semibold(14.0), palette.text);
                        let name_width = name.size().x;
                        painter.galley(Pos2::new(left, rect.center().y - name.size().y / 2.0), name, palette.text);
                        let max_description = right - left - name_width - 16.0;
                        if !description.is_empty() && max_description > 40.0 {
                            let mut job = egui::text::LayoutJob::simple(
                                description.clone(),
                                theme::regular(12.5),
                                palette.dim,
                                max_description,
                            );
                            job.wrap.max_rows = 1;
                            job.wrap.break_anywhere = true;
                            job.wrap.overflow_character = Some('…');
                            let galley = painter.layout_job(job);
                            let pos = Pos2::new(right - galley.size().x, rect.center().y - galley.size().y / 2.0);
                            painter.galley(pos, galley, palette.dim);
                        }
                    }
                    // Con las flechas, la fila elegida tiene que quedar a la vista.
                    if row == selected && selected != last_selected {
                        ui.scroll_to_rect(rect, None);
                    }
                    if response.hovered() {
                        if moved {
                            outcome.hovered = Some(row);
                        }
                        if pressed_now {
                            outcome.pressed = Some(row);
                        }
                    }
                }
            });
    });
    ctx.memory_mut(|m| m.data.insert_temp(last_id, selected));
    outcome
}

/// Dibuja el formulario en el lugar del compositor, en una sola fila (con
/// salto de línea si no entra): ícono de la app, `/comando` y un campo por
/// opción, sin botones ni textos de ayuda (como el cliente oficial). `id`
/// distingue el del chat principal del panel de un hilo.
///
/// Las opciones no obligatorias se agregan desde el menú "Opciones", que
/// aparece con el cursor en el último campo (o en el cursor invisible si no hay
/// campos de texto).
///
/// Teclas: Enter envía, Esc cancela (si el menú "Opciones" está abierto, Esc
/// primero lo cierra), Tab / Shift+Tab cambian de campo, y con el menú abierto
/// ↑/↓ recorren las opciones y Tab (o Enter, después de usar las flechas) elige
/// la resaltada. Retroceso en un campo vacío quita la opción agregada y vuelve
/// al campo anterior (y, en el primero, borra el comando). Se puede
/// seguir escribiendo después del comando (el texto de al lado): al enviar va
/// como valor de la primera opción de texto libre que siga vacía. Retroceso
/// sobre el comando lo vuelve texto (`/comando`).
pub fn show_command_form(ui: &mut Ui, palette: &Palette, active: &mut ActiveSlash, id: Id) -> FormOutcome {
    let ctx = ui.ctx().clone();
    let entry = active.entry.clone();
    let field_id = |index: usize| Id::new((id, "slash_field", index));
    let tail_id = Id::new((id, "slash_tail"));

    let (visible, focusable) = field_layout(&entry, &active.added);
    let mut focused = ctx.memory(|m| m.focused());

    // ---- El foco NUNCA debe quedar fuera del formulario mientras se usa el
    // teclado. egui mueve el foco con Tab por su cuenta (antes de que las
    // teclas se consuman acá) y un campo de lista (booleano / choices) no
    // recibe texto, así que pasaba que: con Tab el foco se iba del
    // formulario (había que clickear para poder enviar) y, sin foco, lo que se
    // tipeaba no iba a ningún lado. Abajo se bloquea Tab en los widgets del
    // formulario; esto es la red de seguridad por si igual se pierde.
    let in_form = |f: Id| f == tail_id || (0..entry.args.len()).any(|i| f == field_id(i));
    let last_focus_key = id.with("slash_last_form_focus");
    let last_tab_key = id.with("slash_last_tab");
    let prev_focus: Option<Id> = ctx.memory(|m| m.data.get_temp(last_focus_key)).filter(|f: &Id| in_form(*f));
    let prev_tab: bool = ctx.memory(|m| m.data.get_temp(last_tab_key)).unwrap_or(false);
    let (typed, nav_key, tab_now) = ctx.input(|i| {
        let (mut typed, mut nav, mut tab) = (false, false, false);
        for event in &i.events {
            match event {
                egui::Event::Text(t) | egui::Event::Paste(t) if !t.is_empty() => typed = true,
                egui::Event::Key { key, pressed: true, .. } => {
                    tab |= *key == egui::Key::Tab;
                    nav |= matches!(
                        key,
                        egui::Key::Tab
                            | egui::Key::Enter
                            | egui::Key::Backspace
                            | egui::Key::Delete
                            | egui::Key::ArrowLeft
                            | egui::Key::ArrowRight
                            | egui::Key::ArrowUp
                            | egui::Key::ArrowDown
                    );
                }
                _ => {}
            }
        }
        (typed, nav, tab)
    });
    // Otro campo de texto con el foco (fuera del formulario) se queda con el teclado.
    let text_edit_elsewhere =
        focused.is_some_and(|f| !in_form(f) && egui::text_edit::TextEditState::load(&ctx, f).is_some());
    let allowed = crate::ui::chat::type_to_focus_allowed(&ctx);
    let first_field = focusable.first().copied().map(field_id);
    let last_field = focusable.last().copied().map(field_id);

    let mut target: Option<Id> = None;
    if allowed && !text_edit_elsewhere {
        if !focused.is_some_and(in_form) {
            if prev_tab {
                // Venía de un Tab y el foco se fue: seguía el campo siguiente;
                // desde el último, el texto de al lado.
                target = Some(match prev_focus {
                    Some(p) if Some(p) != last_field && p != tail_id => p,
                    _ => tail_id,
                });
            } else if typed || nav_key {
                // Sin foco y se usa el teclado: vuelve al último campo.
                target = Some(prev_focus.or(first_field).unwrap_or(tail_id));
            }
        } else if typed {
            // Un campo de lista no recibe texto: lo tipeado va al texto de al lado.
            let on_pick = focusable
                .iter()
                .copied()
                .find(|&i| focused == Some(field_id(i)))
                .is_some_and(|i| is_pick_option(&entry.args[i]));
            if on_pick {
                target = Some(tail_id);
            }
        }
    }
    if let Some(t) = target {
        if t == tail_id {
            active.focus_pending_tail = true;
        } else if let Some(&index) = focusable.iter().find(|&&i| field_id(i) == t) {
            active.focus = Some(index);
        }
        // Se pide ya, antes de dibujar, para que este mismo frame las teclas
        // lleguen al campo (si no, se perdería la primera letra).
        ctx.memory_mut(|m| m.request_focus(t));
        ctx.request_repaint();
        focused = ctx.memory(|m| m.focused());
    }
    ctx.memory_mut(|m| {
        if let Some(f) = focused.filter(|f| in_form(*f)) {
            m.data.insert_temp(last_focus_key, f);
        }
        m.data.insert_temp(last_tab_key, tab_now);
    });

    let focused_idx = focusable.iter().copied().find(|&i| focused == Some(field_id(i)));
    let tail_focused = focused == Some(tail_id);

    if active.focus_pending {
        active.focus = focusable.first().copied();
        active.focus_pending = false;
    }

    // ---- Campo de entidad (usuario / rol / canal) con el foco: lo tipeado es
    // la búsqueda del menú.
    let entity_focused = focused_idx.filter(|&n| is_entity_option(&entry.args[n]));
    let (entity_query, entity_searchable) = match entity_focused {
        Some(n) => entity_search(&active.values[n], active.picked[n].as_ref()),
        None => (String::new(), false),
    };
    let tail_input = tail_focused || focusable.is_empty();
    let options_filter = if tail_input { active.tail.clone() } else { String::new() };
    let popup_key = if tail_input { options_filter.clone() } else { entity_query.clone() };
    if popup_key != active.popup_filter {
        active.popup_filter = popup_key;
        active.popup_selected = 0;
        active.popup_navigated = false;
        active.popup_dismissed = false;
        active.value_selected = 0;
        active.value_dismissed = false;
    }

    // Al llegar a OTRO campo se reabren sus menús (si el foco se pierde un
    // instante, p. ej. por un click en un menú, y vuelve al mismo, no).
    if let Some(n) = focused_idx {
        if active.last_focused != Some(n) {
            let selected = value_choices(&entry.args[n])
                .iter()
                .position(|(_, value)| *value == active.values[n])
                .unwrap_or(0);
            active.last_focused = Some(n);
            active.popup_dismissed = false;
            active.popup_navigated = false;
            active.value_dismissed = false;
            active.value_selected = selected;
        }
    }

    // ---- Menú de valores: el campo de lista (booleano / choices) con el foco.
    let pick_focused = focused_idx.filter(|&n| is_pick_option(&entry.args[n]));
    let value_rows = pick_focused.map(|n| value_choices(&entry.args[n])).unwrap_or_default();
    let value_popup_open = pick_focused.is_some() && !active.value_dismissed && !value_rows.is_empty();
    let mut value_accepted = false;

    // ---- Menú de entidades: candidatos de las menciones (usuarios / roles) o
    // canales del server. Los arma `App::ui` un frame después de pedirlos.
    let mut entity_items: Vec<MentionCandidate> = Vec::new();
    let mut entity_channels: Vec<ChannelCandidate> = Vec::new();
    let mut entity_loading = false;
    let want_entities = entity_focused.is_some() && entity_searchable && !active.value_dismissed;
    let mut mention_query: Option<&str> = None;
    if let (Some(n), true) = (entity_focused, want_entities) {
        let kind = entry.args[n].kind;
        if kind == opt::CHANNEL {
            request_channels(&ctx, &entity_query);
            match channel_results_for(&ctx, &entity_query) {
                Some(results) => entity_channels = results.items.clone(),
                None => entity_loading = true,
            }
        } else {
            mention_query = Some(entity_query.as_str());
            match menus::results_for(&ctx, &entity_query) {
                Some(results) => {
                    if kind != opt::ROLE {
                        let local_users = results.items.iter().filter(|c| c.kind == MentionKind::User).count();
                        menus::drive_member_search(&ctx, &entity_query, local_users);
                    }
                    entity_items = results
                        .items
                        .iter()
                        .filter(|c| match kind {
                            opt::USER => c.kind == MentionKind::User,
                            opt::ROLE => c.kind == MentionKind::Role,
                            _ => matches!(c.kind, MentionKind::User | MentionKind::Role),
                        })
                        .cloned()
                        .collect();
                }
                None => entity_loading = true,
            }
        }
    }
    // Sin campo de mención con el foco se apaga la consulta (el compositor la
    // vuelve a manejar cuando regrese).
    menus::set_active_query(&ctx, mention_query);
    if entity_loading {
        ctx.request_repaint_after(Duration::from_millis(50));
    }
    let entity_len = entity_items.len() + entity_channels.len();
    let entity_popup_open = want_entities && entity_len > 0;
    let selectable = if entity_focused.is_some() { entity_len } else { value_rows.len() };
    if active.value_selected >= selectable {
        active.value_selected = 0;
    }
    let mut entity_accepted = false;

    // ---- Menú "Opciones": se abre con el cursor en el último campo (o en el
    // cursor invisible). Lo tipeado en el cursor invisible lo filtra.
    let on_last = match focused_idx {
        Some(n) => focusable.last() == Some(&n),
        None => tail_focused,
    };
    let options = remaining_options(&entry, &active.added, &options_filter);
    if active.popup_selected >= options.len() {
        active.popup_selected = 0;
    }
    let popup_open =
        on_last && !value_popup_open && !entity_popup_open && !active.popup_dismissed && !options.is_empty();

    // ---- Teclas (ANTES de dibujar, para que los campos no se las lleven).
    if let Some(n) = pick_focused {
        // Un campo de lista no usa las flechas: si no las consumimos, egui las
        // usaría para saltar de widget en widget.
        let down = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
        let up = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));
        if down || up {
            if !value_popup_open {
                active.value_dismissed = false;
            } else {
                let count = value_rows.len();
                active.value_selected = if down {
                    (active.value_selected + 1) % count
                } else {
                    (active.value_selected + count - 1) % count
                };
            }
        }
        let pos = focusable.iter().position(|&f| f == n).unwrap_or(0);
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)) {
            if let Some(prev) = pos.checked_sub(1) {
                active.focus = Some(focusable[prev]);
            }
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)) {
            if let Some(next) = focusable.get(pos + 1) {
                active.focus = Some(*next);
            } else {
                active.focus_pending_tail = true;
            }
        }
        if value_popup_open {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                active.value_dismissed = true;
            }
            let tab = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
            let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            if (tab || enter) && !value_rows.is_empty() {
                let value = value_rows[active.value_selected].1.clone();
                pick_value(active, n, value, &focusable);
                value_accepted = true;
            }
        }
        // Retroceso con un valor ya elegido: lo borra (y vuelve a abrir el menú).
        if !active.values[n].is_empty() && take_backspace(&ctx) {
            active.values[n].clear();
            active.value_dismissed = false;
            active.value_selected = 0;
        }
    }

    if entity_popup_open {
        let count = entity_len;
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
            active.value_selected = (active.value_selected + 1) % count;
            active.popup_navigated = true;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
            active.value_selected = (active.value_selected + count - 1) % count;
            active.popup_navigated = true;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            active.value_dismissed = true;
        }
        // Enter elige si ya se escribió algo o se recorrió la lista; con el
        // campo vacío y sin flechas sigue siendo "enviar".
        let tab = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
        let enter = (active.popup_navigated || !entity_query.is_empty())
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        if tab || enter {
            if let (Some(n), Some((label, token))) =
                (entity_focused, entity_choice(&entity_items, &entity_channels, active.value_selected))
            {
                let look = entity_look(&entity_items, &entity_channels, active.value_selected, palette.dim);
                pick_entity(active, n, label, token, look, &focusable);
                entity_accepted = true;
            }
        }
    }

    if tail_focused && !focusable.is_empty() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab)) {
        active.focus = focusable.last().copied();
    }

    let mut picked: Option<usize> = None;
    if popup_open {
        let count = options.len();
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
            active.popup_selected = (active.popup_selected + 1) % count;
            active.popup_navigated = true;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
            active.popup_selected = (active.popup_selected + count - 1) % count;
            active.popup_navigated = true;
        }
        // Esc cierra primero el menú (antes de que cancele el comando).
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            active.popup_dismissed = true;
        }
        let tab = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
        let enter = active.popup_navigated && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        if tab || enter {
            picked = options.get(active.popup_selected).copied();
        }
    } else if tail_focused {
        // Sin campos, Tab no tiene a dónde ir: que no se lleve el foco.
        let _ = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
    }
    if let Some(arg) = picked {
        add_option(active, &entry, arg);
        ctx.request_repaint();
    }

    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return FormOutcome::Cancel;
    }
    let mut submit = false;
    if focused_idx.is_some() || tail_focused || focusable.is_empty() {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            submit = true;
        }
    }
    // Retroceso (solo la pulsación, no la repetición) con el campo ya vacío.
    let backspace_on_empty = match focused_idx {
        Some(n) => active.values[n].is_empty(),
        // En el texto de al lado basta con estar al INICIO: el texto escrito
        // no se pierde.
        None => {
            (focusable.is_empty() || tail_focused)
                && (active.tail.is_empty() || (tail_focused && menus::caret_index(&ctx, tail_id) == Some(0)))
        }
    };
    if backspace_on_empty && take_backspace(&ctx) {
        if focused_idx.is_none() && !focusable.is_empty() {
            // Del texto de al lado se vuelve al último campo.
            active.focus = focusable.last().copied();
        } else {
            let pos = focused_idx.and_then(|n| focusable.iter().position(|&f| f == n));
            match pos {
                Some(pos) if pos > 0 => {
                    let current = focusable[pos];
                    // Una opcional agregada se quita (como en Discord).
                    if active.added.contains(&current) {
                        active.added.retain(|&a| a != current);
                        active.values[current].clear();
                        active.picked[current] = None;
                        active.look[current] = None;
                    }
                    active.focus = Some(focusable[pos - 1]);
                }
                // Primer campo (o cursor invisible): la interacción vuelve a
                // ser texto (`/comando`) y reaparecen las sugerencias.
                _ => return FormOutcome::ToText,
            }
        }
    }
    if let Some(n) = focused_idx {
        let pos = focusable.iter().position(|&f| f == n).unwrap_or(0);
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab)) {
            if let Some(prev) = pos.checked_sub(1) {
                active.focus = Some(focusable[prev]);
            }
        } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)) {
            if let Some(next) = focusable.get(pos + 1) {
                active.focus = Some(*next);
            } else {
                active.focus_pending_tail = true;
            }
        }
    }

    // El layout puede haber cambiado (se agregó o quitó una opción).
    let (visible, focusable) = field_layout(&entry, &active.added);

    let mut reopen = false;
    let mut pick_rect: Option<Rect> = None;
    let hidden = remaining_options(&entry, &active.added, "").len();

    ui.add_space(6.0);

    // ---- Barra superior: `/comando`, su descripción y una X para cancelar.
    let mut cancel_clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        Frame::new()
            .fill(palette.surface_hover)
            .corner_radius(CornerRadius::same(theme::radius() + 2))
            .inner_margin(Margin::symmetric(12, 2))
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - 16.0);
                let room = ui.available_width();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let title = ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("/{}", entry.display))
                                .font(theme::semibold(13.5))
                                .color(palette.text),
                        )
                        .selectable(false),
                    );
                    let description = entry.description.trim();
                    if !description.is_empty() {
                        let max = (room - title.rect.width() - 56.0).max(40.0);
                        let mut job = egui::text::LayoutJob::simple(
                            description.to_string(),
                            theme::regular(12.5),
                            palette.dim,
                            max,
                        );
                        job.wrap.max_rows = 1;
                        job.wrap.break_anywhere = true;
                        job.wrap.overflow_character = Some('…');
                        ui.add(egui::Label::new(job).selectable(false));
                    }
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if theme::icon_button(ui, Icon::X, 14.0, palette.dim, palette.text, "Cancelar").clicked() {
                            cancel_clicked = true;
                        }
                    });
                });
            });
    });
    ui.add_space(2.0);

    let form_rect = ui
        .horizontal(|ui| {
            ui.add_space(16.0);
            Frame::new()
                .fill(palette.surface)
                .corner_radius(CornerRadius::same(theme::radius() + 2))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width() - 16.0);
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);

                    ui.horizontal_wrapped(|ui| {
                        // ---- Como el compositor normal (+), y el comando como una
                        // "pastilla" con el ícono de la app.
                        theme::icon(ui, Icon::CirclePlus, 16.0, palette.dim);
                        Frame::new()
                            .fill(palette.window_solid)
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(Margin::symmetric(8, 3))
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.horizontal(|ui| {
                                    let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                                    extra::avatar(
                                        ui,
                                        icon_rect.center(),
                                        10.0,
                                        entry.app_icon_url.as_deref(),
                                        fallback_color(&entry.app_name),
                                        &initial(&entry.app_name),
                                        palette,
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!("/{}", entry.display))
                                                .font(theme::semibold(14.0))
                                                .color(palette.text),
                                        )
                                        .selectable(false),
                                    );
                                });
                            });

                        // ---- Una pastilla por opción visible: el nombre de la
                        // opción y, al lado, su valor en un recuadro más claro.
                        for &index in &visible {
                            let option = entry.args[index].clone();
                            let missing =
                                active.error.is_some() && option.required && active.values[index].trim().is_empty();
                            let is_focused = focused_idx == Some(index);
                            let stroke = if missing {
                                Stroke::new(1.0, palette.danger)
                            } else if is_focused {
                                Stroke::new(1.0, palette.accent)
                            } else {
                                Stroke::NONE
                            };
                            // Usuario / rol / canal ya elegido en el menú (y sin
                            // editar): se marca como una mención.
                            let look: Option<PickedLook> = match active.picked.get(index).and_then(|p| p.as_ref()) {
                                Some((label, _)) if *label == active.values[index] => {
                                    active.look.get(index).cloned().flatten()
                                }
                                _ => None,
                            };
                            let mention_color = look.as_ref().map(|l| l.color.unwrap_or(palette.accent));
                            let value = &mut active.values[index];
                            Frame::new()
                                .fill(palette.window_solid)
                                .stroke(stroke)
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(Margin::symmetric(4, 3))
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    ui.horizontal(|ui| {
                                        ui.add_space(4.0);
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(option.name.clone())
                                                    .font(theme::semibold(12.5))
                                                    .color(palette.secondary),
                                            )
                                            .selectable(false),
                                        );
                                        if option.kind == opt::ATTACHMENT {
                                            theme::text(ui, "(adjuntos no soportados)", theme::regular(12.0), palette.dim);
                                            return;
                                        }
                                        Frame::new()
                                            .fill(palette.surface_hover)
                                            .corner_radius(CornerRadius::same(4))
                                            .inner_margin(Margin::symmetric(6, 1))
                                            .show(ui, |ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                ui.horizontal(|ui| {
                                                    if let Some(look) = look.as_ref().filter(|l| l.show_avatar) {
                                                        let (r, _) =
                                                            ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                                                        extra::avatar(
                                                            ui,
                                                            r.center(),
                                                            8.0,
                                                            look.avatar_url.as_deref(),
                                                            look.avatar_color,
                                                            &look.initial,
                                                            palette,
                                                        );
                                                    }
                                                    if is_pick_option(&option) {
                                                        let chosen = value_choices(&option)
                                                            .into_iter()
                                                            .find(|(_, v)| *v == *value)
                                                            .map(|(name, _)| name);
                                                        let (text, color) = match chosen {
                                                            Some(name) => (name, palette.text),
                                                            None => ("elegir".to_string(), palette.dim),
                                                        };
                                                        let label = ui.add(
                                                            egui::Label::new(
                                                                egui::RichText::new(text)
                                                                    .font(theme::regular(13.0))
                                                                    .color(color),
                                                            )
                                                            .selectable(false),
                                                        );
                                                        // Misma interacción que un campo de texto: recibe
                                                        // el foco (Tab / click) y abre su menú de valores.
                                                        let response = ui
                                                            .interact(
                                                                label.rect.expand2(Vec2::new(4.0, 2.0)),
                                                                field_id(index),
                                                                Sense::click(),
                                                            )
                                                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                                                        if response.clicked() {
                                                            active.focus = Some(index);
                                                            active.value_dismissed = false;
                                                        }
                                                        if focused_idx == Some(index) {
                                                            pick_rect = Some(response.rect);
                                                        }
                                                        if active.focus == Some(index) {
                                                            if response.has_focus() {
                                                                active.focus = None;
                                                            } else {
                                                                response.request_focus();
                                                            }
                                                        }
                                                    } else {
                                                        let font = theme::regular(13.0);
                                                        let hint = hint_for(&option);
                                                        let measure = |text: &str| {
                                                            ui.painter()
                                                                .layout_no_wrap(text.to_string(), font.clone(), palette.text)
                                                                .size()
                                                                .x
                                                        };
                                                        let width = (measure(&hint).max(measure(value)) + 12.0).clamp(60.0, 280.0);
                                                        let mut edit = egui::TextEdit::singleline(value)
                                                            .id(field_id(index))
                                                            .hint_text(hint)
                                                            .font(font.clone())
                                                            .frame(egui::Frame::NONE)
                                                            .desired_width(width);
                                                        if let Some(color) = mention_color {
                                                            edit = edit.text_color(color);
                                                        }
                                                        let response = ui.add(edit);
                                                        if focused_idx == Some(index) {
                                                            pick_rect = Some(response.rect);
                                                        }
                                                        if active.focus == Some(index) {
                                                            if response.has_focus() {
                                                                active.focus = None;
                                                            } else {
                                                                response.request_focus();
                                                            }
                                                        }
                                                    }
                                                });
                                            });
                                    });
                                });
                        }

                        // ---- "+N más": abre el menú "Opciones".
                        if hidden > 0 {
                            let label =
                                egui::RichText::new(format!("+{hidden} más")).font(theme::regular(12.5)).color(palette.dim);
                            if ui
                                .add(egui::Label::new(label).sense(Sense::click()))
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                reopen = true;
                            }
                        }

                        // ---- Texto de al lado: siempre se puede seguir
                        // escribiendo, aunque el comando ya esté completo (como
                        // en Discord). Lo escrito va como valor de la primera
                        // opción de texto libre que siga vacía (`tail_target`);
                        // su nombre se ve como pista. Además filtra el menú
                        // "Opciones", y con Retroceso al inicio el comando vuelve
                        // a ser texto. Sin campos se queda con el foco.
                        {
                            let hint = tail_target(&entry, &active.values)
                                .map(|i| entry.args[i].name.clone())
                                .unwrap_or_else(|| "escribir…".to_string());
                            // Ancho = lo que queda de la fila MEDIDO desde el cursor
                            // (`available_width` en un layout con saltos de línea no es
                            // fiable: con el ancho de más el campo caía a una segunda
                            // fila, fuera de vista). Si no entran 120px, salta de fila.
                            let room = ui.max_rect().right() - ui.cursor().min.x - 12.0;
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut active.tail)
                                    .id(tail_id)
                                    .hint_text(hint)
                                    .frame(egui::Frame::NONE)
                                    .desired_width(room.max(120.0))
                                    .min_size(Vec2::new(120.0, 22.0)),
                            );
                            let wants_focus = if focusable.is_empty() {
                                focused.is_none() || active.focus_pending_tail
                            } else {
                                active.focus_pending_tail
                            };
                            if !response.has_focus() && wants_focus {
                                response.request_focus();
                            }
                            if response.has_focus() {
                                active.focus_pending_tail = false;
                            }
                        }
                    });

                    // Solo si algo salió mal (falta un dato obligatorio, etc.).
                    if let Some(error) = &active.error {
                        theme::text(ui, error.clone(), theme::regular(12.0), palette.danger);
                    }
                });
        })
        .response
        .rect;

    if cancel_clicked {
        return FormOutcome::Cancel;
    }

    // Tab y las flechas los manejamos nosotros: que egui no use Tab para saltar
    // a otro widget (ver arriba). Va DESPUÉS de dibujar los campos para que
    // gane sobre el filtro que fija cada `TextEdit`.
    if let Some(f) = ctx.memory(|m| m.focused()).filter(|f| in_form(*f)) {
        ctx.memory_mut(|m| {
            m.set_focus_lock_filter(
                f,
                egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false },
            )
        });
    }

    // ---- Menús flotantes (se eligen con Tab / Enter / click; las flechas ya se
    // manejaron arriba).
    if value_popup_open && !value_accepted {
        if let Some(n) = pick_focused {
            let rows: Vec<(String, String)> = value_rows.iter().map(|(name, _)| (name.clone(), String::new())).collect();
            let rect = pick_rect.unwrap_or(form_rect);
            let outcome = show_popup_list(
                &ctx,
                palette,
                id.with("slash_values"),
                Pos2::new(rect.left() - 10.0, rect.top() - 10.0),
                240.0,
                &entry.args[n].name,
                &rows,
                active.value_selected,
            );
            if let Some(row) = outcome.hovered {
                active.value_selected = row;
            }
            if let Some(row) = outcome.pressed {
                if let Some((_, value)) = value_rows.get(row) {
                    pick_value(active, n, value.clone(), &focusable);
                    ctx.request_repaint();
                }
            }
        }
    } else if entity_popup_open && !entity_accepted {
        if let Some(n) = entity_focused {
            let rect = pick_rect.unwrap_or(form_rect);
            let anchor = Pos2::new(rect.left() - 10.0, rect.top() - 10.0);
            let (hovered, pressed) = if !entity_items.is_empty() {
                let outcome = menus::show_mention_menu(
                    &ctx,
                    palette,
                    id.with("slash_mentions"),
                    anchor,
                    320.0,
                    &entity_query,
                    &entity_items,
                    active.value_selected,
                );
                (outcome.hovered, outcome.pressed)
            } else {
                let rows: Vec<(String, String)> =
                    entity_channels.iter().map(|c| (format!("#{}", c.name), c.category.clone())).collect();
                let outcome =
                    show_popup_list(&ctx, palette, id.with("slash_channels"), anchor, 320.0, "Canales", &rows, active.value_selected);
                (outcome.hovered, outcome.pressed)
            };
            // El resaltado sigue al mouse solo cuando se mueve (no pelea con las flechas).
            if let Some(row) = hovered {
                if ctx.input(|i| i.pointer.delta() != Vec2::ZERO) {
                    active.value_selected = row;
                }
            }
            if let Some((row, (label, token))) =
                pressed.and_then(|row| entity_choice(&entity_items, &entity_channels, row).map(|c| (row, c)))
            {
                let look = entity_look(&entity_items, &entity_channels, row, palette.dim);
                pick_entity(active, n, label, token, look, &focusable);
                ctx.request_repaint();
            }
        }
    } else if popup_open && picked.is_none() {
        let anchor = Pos2::new(form_rect.left() + 16.0, form_rect.top() - 4.0);
        let width = (form_rect.width() - 32.0).max(260.0);
        let rows: Vec<(String, String)> = options
            .iter()
            .map(|&i| (entry.args[i].name.clone(), entry.args[i].description.clone()))
            .collect();
        let outcome = show_popup_list(
            &ctx,
            palette,
            id.with("slash_options"),
            anchor,
            width,
            "Opciones",
            &rows,
            active.popup_selected,
        );
        if let Some(row) = outcome.hovered {
            active.popup_selected = row;
        }
        if let Some(arg) = outcome.pressed.and_then(|row| options.get(row).copied()) {
            add_option(active, &entry, arg);
            ctx.request_repaint();
        }
    }

    if reopen {
        active.popup_dismissed = false;
        active.focus = focusable.last().copied();
        if active.focus.is_none() {
            active.focus_pending_tail = true;
        }
        ctx.request_repaint();
    }
    if active.focus.is_some() {
        // Seguir pidiendo el foco hasta que el campo lo tenga.
        ctx.request_repaint();
    }
    if submit {
        match crate::discord::slash::build_options(&active.entry, &resolved_values(active)) {
            Ok(options) => return FormOutcome::Submit(options),
            Err(message) => active.error = Some(message),
        }
    }
    FormOutcome::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::slash::{build_catalog, parse_index};
    use serde_json::json;

    #[test]
    fn browse_query_only_for_a_leading_slash() {
        assert_eq!(browse_query("/ping"), Some("ping".to_string()));
        assert_eq!(browse_query("/"), Some(String::new()));
        assert_eq!(browse_query("/permisos ver"), Some("permisos ver".to_string()));
        assert_eq!(browse_query("hola /ping"), None);
        assert_eq!(browse_query("// comentario"), None);
        assert_eq!(browse_query("/ ping"), None);
        assert_eq!(browse_query("/a\nb"), None);
    }

    #[test]
    fn layout_groups_by_app_and_search_is_one_section() {
        let catalog = build_catalog(parse_index(&json!({
            "applications": [{ "id": "1", "name": "Zeta" }, { "id": "2", "name": "Alfa", "icon": "abc" }],
            "application_commands": [
                { "id": "1", "application_id": "1", "version": "1", "name": "ping", "description": "" },
                { "id": "2", "application_id": "2", "version": "1", "name": "hug", "description": "" },
                { "id": "3", "application_id": "2", "version": "1", "name": "pat", "description": "" },
            ]
        })));
        let (matches, sections) = layout(&catalog, "");
        assert_eq!(matches.len(), 3);
        // Sin frecuentes: una sección por app, por nombre (Alfa antes que Zeta).
        let titles: Vec<&str> = sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["Alfa", "Zeta"]);
        assert_eq!((sections[0].start, sections[0].end), (0, 2));
        assert!(matches!(&sections[0].kind, SectionKind::App { icon_url: Some(url) } if url.contains("app-icons/2/abc")));
        let (found, sections) = layout(&catalog, "pa");
        assert_eq!(found.len(), 1);
        assert_eq!(sections.len(), 1);
        assert!(matches!(sections[0].kind, SectionKind::Search));
    }

    #[test]
    fn filter_prefers_prefix_and_matches_subcommand_paths() {
        let catalog = build_catalog(parse_index(&json!({
            "applications": [{ "id": "1", "name": "Bot" }],
            "application_commands": [
                { "id": "1", "application_id": "1", "version": "1", "name": "ping", "description": "" },
                { "id": "2", "application_id": "1", "version": "1", "name": "mapping", "description": "" },
                { "id": "3", "application_id": "1", "version": "1", "name": "permisos", "description": "",
                  "options": [{ "type": 1, "name": "ver", "description": "" }] },
            ]
        })));
        let names = |q: &str| -> Vec<String> {
            filter(&catalog, q).into_iter().map(|i| catalog.entries[i].display.clone()).collect()
        };
        assert_eq!(names("pin"), vec!["ping", "mapping"]);
        assert_eq!(names("perm ver"), vec!["permisos ver"]);
        assert_eq!(names("").len(), 3);
        assert!(names("zzz").is_empty());
    }

    fn options_catalog() -> SlashCatalog {
        build_catalog(parse_index(&json!({
            "applications": [{ "id": "1", "name": "Bot" }],
            "application_commands": [
                { "id": "1", "application_id": "1", "version": "1", "name": "purge", "description": "",
                  "options": [
                      { "type": 4, "name": "amount", "description": "", "required": true },
                      { "type": 3, "name": "contains", "description": "" },
                      { "type": 5, "name": "bots", "description": "" },
                      { "type": 11, "name": "file", "description": "" },
                  ] },
                { "id": "2", "application_id": "1", "version": "1", "name": "clear", "description": "",
                  "options": [{ "type": 3, "name": "reason", "description": "" }] },
                { "id": "3", "application_id": "1", "version": "1", "name": "multi", "description": "",
                  "options": [
                      { "type": 3, "name": "alpha", "description": "" },
                      { "type": 3, "name": "beta", "description": "" },
                  ] },
            ]
        })))
    }

    fn entry_named(catalog: &SlashCatalog, name: &str) -> Arc<SlashEntry> {
        catalog.entries.iter().find(|e| e.display == name).cloned().expect("entry")
    }

    #[test]
    fn optional_options_are_never_added_automatically() {
        let catalog = options_catalog();
        // Ni con una sola opcional, ni con varias, ni con obligatorias.
        assert!(initial_added(&entry_named(&catalog, "clear")).is_empty());
        assert!(initial_added(&entry_named(&catalog, "purge")).is_empty());
        assert!(initial_added(&entry_named(&catalog, "multi")).is_empty());
        // Siguen ofrecidas en el menú "Opciones".
        let clear = entry_named(&catalog, "clear");
        assert_eq!(remaining_options(&clear, &[], ""), vec![0]);
    }

    fn blank_active(entry: Arc<SlashEntry>) -> ActiveSlash {
        ActiveSlash {
            values: vec![String::new(); entry.args.len()],
            picked: vec![None; entry.args.len()],
            look: vec![None; entry.args.len()],
            entry,
            error: None,
            added: Vec::new(),
            popup_selected: 0,
            popup_navigated: false,
            popup_dismissed: false,
            popup_filter: String::new(),
            value_selected: 0,
            value_dismissed: false,
            last_focused: None,
            focus: None,
            focus_pending: false,
            tail: String::new(),
            focus_pending_tail: false,
        }
    }

    #[test]
    fn deleting_the_interaction_turns_it_back_into_text() {
        let catalog = options_catalog();
        let mut active = blank_active(entry_named(&catalog, "clear"));
        active.tail = " hola ".to_string();
        // Vuelve como `/clear`; lo escrito fuera de los campos desaparece.
        assert_eq!(active.command_text(), "/clear");
        assert!(active.tail.is_empty());
        // Y como texto, vuelve a disparar el menú de sugerencias.
        assert_eq!(browse_query("/clear").as_deref(), Some("clear"));
    }

    #[test]
    fn options_menu_lists_only_missing_addable_options() {
        let catalog = options_catalog();
        let purge = entry_named(&catalog, "purge");
        let names = |added: &[usize], filter: &str| -> Vec<String> {
            remaining_options(&purge, added, filter).into_iter().map(|i| purge.args[i].name.clone()).collect()
        };
        // La obligatoria y la adjunta no se ofrecen.
        assert_eq!(names(&[], ""), vec!["contains", "bots"]);
        assert_eq!(names(&[], "bo"), vec!["bots"]);
        // Las ya agregadas salen del menú.
        let contains = purge.args.iter().position(|a| a.name == "contains").unwrap();
        assert_eq!(names(&[contains], ""), vec!["bots"]);
    }

    #[test]
    fn field_layout_shows_required_then_added_in_pick_order() {
        let catalog = options_catalog();
        let multi = entry_named(&catalog, "multi");
        let (visible, focusable) = field_layout(&multi, &[1, 0]);
        assert_eq!(visible, vec![1, 0]);
        assert_eq!(focusable, vec![1, 0]);
        let purge = entry_named(&catalog, "purge");
        // El booleano (`bots`) también se alcanza con Tab.
        let (visible, focusable) = field_layout(&purge, &[2]);
        assert_eq!(visible, vec![0, 2]);
        assert_eq!(focusable, vec![0, 2]);
        assert!(is_pick_option(&purge.args[2]));
        assert!(!is_pick_option(&purge.args[1]));
    }

    #[test]
    fn pick_fields_offer_their_values_and_advance_on_pick() {
        let catalog = options_catalog();
        let purge = entry_named(&catalog, "purge");
        let bots = &purge.args[2];
        let values: Vec<String> = value_choices(bots).into_iter().map(|(_, v)| v).collect();
        assert_eq!(values, vec!["true", "false"]);
        assert!(value_choices(&purge.args[1]).is_empty());

        let mut active = ActiveSlash {
            entry: purge.clone(),
            values: vec![String::new(); purge.args.len()],
            error: None,
            added: vec![1, 2],
            popup_selected: 0,
            popup_navigated: false,
            popup_dismissed: false,
            popup_filter: String::new(),
            value_selected: 0,
            value_dismissed: false,
            last_focused: None,
            picked: vec![None; purge.args.len()],
            look: vec![None; purge.args.len()],
            focus: None,
            focus_pending: false,
            tail: String::new(),
            focus_pending_tail: false,
        };
        // Elegir un valor pasa al campo siguiente (si hay).
        pick_value(&mut active, 0, "5".to_string(), &[0, 1, 2]);
        assert_eq!(active.values[0], "5");
        assert_eq!(active.focus, Some(1));
        pick_value(&mut active, 2, "true".to_string(), &[0, 1, 2]);
        assert_eq!(active.values[2], "true");
        // Es el último: se queda donde está.
        assert_eq!(active.focus, Some(1));
    }

    #[test]
    fn entity_search_ignores_prefix_and_pasted_ids() {
        assert_eq!(entity_search("@ana", None), ("ana".to_string(), true));
        assert_eq!(entity_search("#gen", None), ("gen".to_string(), true));
        assert_eq!(entity_search("", None), (String::new(), true));
        // Un ID o una mención pegada no se busca.
        assert!(!entity_search("123456", None).1);
        assert!(!entity_search("<@123>", None).1);
        // Tampoco lo que ya se eligió en el menú.
        let picked = ("@Ana".to_string(), "<@1>".to_string());
        assert!(!entity_search("@Ana", Some(&picked)).1);
        // ...pero si se edita, se vuelve a buscar.
        assert!(entity_search("@An", Some(&picked)).1);
    }

    #[test]
    fn picked_entities_are_sent_as_mentions_until_edited() {
        let catalog = options_catalog();
        let purge = entry_named(&catalog, "purge");
        let mut active = ActiveSlash {
            entry: purge.clone(),
            values: vec![String::new(); purge.args.len()],
            error: None,
            added: vec![],
            popup_selected: 0,
            popup_navigated: false,
            popup_dismissed: false,
            popup_filter: String::new(),
            value_selected: 0,
            value_dismissed: false,
            last_focused: None,
            picked: vec![None; purge.args.len()],
            look: vec![None; purge.args.len()],
            focus: None,
            focus_pending: false,
            tail: String::new(),
            focus_pending_tail: false,
        };
        pick_entity(&mut active, 1, "@Ana".to_string(), "<@42>".to_string(), None, &[0, 1]);
        assert_eq!(resolved_values(&active)[1], "<@42>");
        active.values[1] = "@Ana B".to_string();
        assert_eq!(resolved_values(&active)[1], "@Ana B");
    }

    #[test]
    fn trailing_text_goes_to_the_first_free_text_option_left_empty() {
        let catalog = options_catalog();
        let purge = entry_named(&catalog, "purge");
        // `amount` es entero y `bots` booleano: el texto libre es `contains`.
        let mut active = blank_active(purge.clone());
        active.values[0] = "5".to_string();
        active.tail = "  hola  ".to_string();
        assert_eq!(tail_target(&purge, &active.values), Some(1));
        assert_eq!(resolved_values(&active)[1], "hola");
        // Si esa opción ya tiene valor, el texto de al lado no la pisa.
        active.values[1] = "x".to_string();
        assert_eq!(tail_target(&purge, &active.values), None);
        assert_eq!(resolved_values(&active)[1], "x");
        // Sin texto, no se manda nada.
        active.values[1].clear();
        active.tail = "   ".to_string();
        assert_eq!(resolved_values(&active)[1], "");
    }

    #[test]
    fn trailing_text_is_sent_in_the_interaction() {
        let catalog = options_catalog();
        let clear = entry_named(&catalog, "clear");
        let mut active = blank_active(clear.clone());
        active.tail = "spam".to_string();
        let options = crate::discord::slash::build_options(&clear, &resolved_values(&active)).unwrap();
        assert_eq!(options, vec![json!({ "type": 3, "name": "reason", "value": "spam" })]);
        // Sin texto la opcional no viaja.
        active.tail.clear();
        assert!(crate::discord::slash::build_options(&clear, &resolved_values(&active)).unwrap().is_empty());
    }

    #[test]
    fn entity_choice_builds_label_and_mention() {
        let channels = vec![ChannelCandidate { id: "9".into(), name: "general".into(), category: "Texto".into() }];
        assert_eq!(
            entity_choice(&[], &channels, 0),
            Some(("#general".to_string(), "<#9>".to_string()))
        );
        assert_eq!(entity_choice(&[], &channels, 1), None);
    }
}