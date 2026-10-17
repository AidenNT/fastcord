//! Pestaña "Tráfico": todo lo que el cliente manda y recibe por la red.
//!
//! - **Tabla**: una línea por pedido HTTP y una por cada conexión WebSocket
//!   (con el tipo de conexión: Gateway, Voz, REST, CDN...; la API a la que va,
//!   un resumen y *quién lo pidió*). Los mensajes de un WebSocket (▼ entra /
//!   ▲ sale) NO se listan solos: hay que hacer clic en la fila de su conexión
//!   para desplegarlos (otro clic los oculta).
//! - **Lista lateral**: filtros por tipo, por sentido y por conexión, más el
//!   estado de cada conexión (abierta/cerrada y sus contadores).
//! - **Inspector**: el contenido completo (JSON indentado), headers, cuerpo,
//!   estado y duración del pedido, y los datos de la conexión.
//!
//! Los datos los graba `discord_client_rest::netlog` (ver ese módulo para qué
//! se oculta y cómo se atribuye el "quién"); acá solo hay estado de la vista.
//! Solo se graba mientras las devtools están habilitadas.

use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use discord_client_rest::netlog::{self, Conn, Detail, Dir, FrameType, Kind, Row};
use egui::{Align2, Color32, Rect, Sense, Shape, Stroke, pos2, vec2};

use super::console::fmt_clock;
use super::style;
use crate::theme::{self, Palette};

const ROW_H: f32 = 26.0;

fn kind_color(kind: Kind, palette: &Palette) -> Color32 {
    match kind {
        Kind::Gateway => style::BLUE,
        Kind::Voice => style::GREEN,
        Kind::StreamWatch => style::TEAL,
        Kind::StreamPublish => style::PURPLE,
        Kind::RemoteAuth => style::PINK,
        Kind::WebSocket => palette.secondary,
        Kind::Rest => style::ORANGE,
        Kind::Cdn => style::YELLOW,
        Kind::Http => palette.text,
        Kind::Udp => style::RED,
    }
}

#[derive(Default)]
struct Cache {
    ids: Vec<u64>,
    key: u64,
    version: u64,
    epoch: u64,
    valid: bool,
}

/// Contenido del inspector ya formateado (para no re-parsear el JSON en cada frame).
struct Formatted {
    id: u64,
    /// Cambia cuando el pedido recibe su respuesta.
    stamp: String,
    payload: String,
    req_body: String,
    resp_body: String,
}

/// Lo justo de una fila para dibujar la tabla (sin copiar los contenidos).
struct Line {
    id: u64,
    kind: Kind,
    dir: Option<Dir>,
    http: bool,
    target: String,
    summary: String,
    who: String,
    unix_ms: i64,
    error: bool,
    pending: bool,
    conn: u64,
    /// Solo en la fila "Conexión abierta": `(mensajes que entran, mensajes que salen, cerrada)`.
    conn_stats: Option<(u64, u64, bool)>,
}

/// Texto con el que `netlog::conn_open` marca la fila que representa a una conexión.
const CONN_OPEN_PREFIX: &str = "Conexión abierta";

/// La fila que representa a una conexión WebSocket/UDP en la tabla (la de "Conexión
/// abierta → url"): al hacer clic se despliegan o se ocultan sus mensajes.
fn is_conn_row(row: &Row) -> bool {
    row.conn != 0
        && matches!(&row.detail, Detail::Frame(frame) if frame.ftype == FrameType::Info)
        && row.summary.starts_with(CONN_OPEN_PREFIX)
}

pub(super) struct TrafficState {
    kinds: [bool; 10],
    show_in: bool,
    show_out: bool,
    only_errors: bool,
    /// Mostrar solo esta conexión.
    conn: Option<u64>,
    filter: String,
    selected: Option<u64>,
    /// Conexiones cuyos mensajes están desplegados. Por omisión ninguna: los
    /// frames de un WebSocket solo aparecen después de abrir su conexión.
    open_conns: BTreeSet<u64>,
    follow: bool,
    cache: Cache,
    formatted: Option<Formatted>,
}

impl Default for TrafficState {
    fn default() -> Self {
        Self {
            kinds: [true; 10],
            show_in: true,
            show_out: true,
            only_errors: false,
            conn: None,
            filter: String::new(),
            selected: None,
            open_conns: BTreeSet::new(),
            follow: true,
            cache: Cache::default(),
            formatted: None,
        }
    }
}

/// JSON indentado si lo es; si no, el texto tal cual.
fn pretty(text: &str) -> String {
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
            if let Ok(out) = serde_json::to_string_pretty(&value) {
                return out;
            }
        }
    }
    text.to_owned()
}

fn dir_label(dir: Dir) -> &'static str {
    match dir {
        Dir::In => "Entrada (llega al cliente)",
        Dir::Out => "Salida (sale del cliente)",
    }
}

struct Columns {
    kind_x: f32,
    target_x: f32,
    target_w: f32,
    summary_x: f32,
    summary_right: f32,
    who_x: f32,
    time_right: f32,
}

impl Columns {
    fn new(rect: Rect) -> Self {
        let kind_x = rect.left() + 36.0;
        let target_x = kind_x + 108.0;
        let target_w = (rect.width() * 0.2).clamp(110.0, 240.0);
        let summary_x = target_x + target_w + 8.0;
        let who_w = (rect.width() * 0.17).clamp(100.0, 220.0);
        let who_x = (rect.right() - 104.0 - who_w).max(summary_x + 60.0);
        Self {
            kind_x,
            target_x,
            target_w,
            summary_x,
            summary_right: (who_x - 8.0).max(summary_x + 40.0),
            who_x,
            time_right: rect.right() - 14.0,
        }
    }
}

impl TrafficState {
    fn filter_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.kinds.hash(&mut hasher);
        self.show_in.hash(&mut hasher);
        self.show_out.hash(&mut hasher);
        self.only_errors.hash(&mut hasher);
        self.conn.hash(&mut hasher);
        self.open_conns.hash(&mut hasher);
        self.filter.trim().to_lowercase().hash(&mut hasher);
        hasher.finish()
    }

    /// Pone al día la lista de filas que pasan el filtro. Solo trabaja si
    /// cambió el filtro o llegó algo nuevo al registro.
    fn refresh(&mut self) {
        let key = self.filter_key();
        let needle = self.filter.trim().to_lowercase();
        let (kinds, show_in, show_out, only_errors, conn) =
            (self.kinds, self.show_in, self.show_out, self.only_errors, self.conn);
        let open_conns = self.open_conns.clone();
        let cache_fits = self.cache.valid && self.cache.key == key;
        let (cached_version, cached_epoch) = (self.cache.version, self.cache.epoch);

        let fresh = netlog::with(|store| {
            if cache_fits && store.version() == cached_version && store.epoch() == cached_epoch {
                return None;
            }
            let ids: Vec<u64> = store
                .rows()
                .iter()
                .filter(|row| {
                    kinds[row.kind.index()]
                        && (conn.is_none() || Some(row.conn) == conn)
                        // Los mensajes de un WebSocket solo se ven con su conexión desplegada;
                        // los eventos (abrió, cerró, falló) y los pedidos HTTP, siempre.
                        && match &row.detail {
                            Detail::Frame(frame) if frame.ftype != FrameType::Info => open_conns.contains(&row.conn),
                            _ => true,
                        }
                        && match row.dir() {
                            Some(Dir::In) => show_in,
                            Some(Dir::Out) => show_out,
                            None => true,
                        }
                        && (!only_errors || row.is_error())
                        && (needle.is_empty() || row.search.contains(&needle))
                })
                .map(|row| row.id)
                .collect();
            Some((ids, store.version(), store.epoch()))
        });
        if let Some((ids, version, epoch)) = fresh {
            self.cache = Cache { ids, key, version, epoch, valid: true };
        }
    }

    pub(super) fn clear_selection(&mut self) {
        self.selected = None;
        self.open_conns.clear();
        self.formatted = None;
    }

    pub(super) fn show_all(&mut self) {
        self.kinds = [true; 10];
        self.show_in = true;
        self.show_out = true;
        self.only_errors = false;
        self.conn = None;
        self.filter.clear();
    }

    // -- Página: barra de filtro + tabla ------------------------------------------------

    pub(super) fn page(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        self.refresh();
        let total = netlog::with(|store| store.rows().len());
        let shown = self.cache.ids.len();

        egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 6)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("Filtrar por tipo, API, ruta, quién o evento…")
                        .desired_width(320.0),
                );
                ui.add_space(10.0);
                style::toggle_switch(ui, palette, &mut self.follow);
                ui.label(egui::RichText::new("Seguir").color(palette.secondary));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{shown} de {total} filas")).small().color(palette.dim));
                    if !netlog::enabled() {
                        ui.label(egui::RichText::new("registro apagado").small().color(style::RED));
                    }
                });
            });
        });
        let (rule, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().hline(rule.x_range(), rule.center().y, Stroke::new(1.0, palette.outline));

        self.table(ui, palette);
    }

    fn table(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let (header, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
        {
            let columns = Columns::new(header);
            let painter = ui.painter();
            let font = theme::medium(12.5);
            let y = header.center().y;
            painter.text(pos2(columns.kind_x, y), Align2::LEFT_CENTER, "Tipo", font.clone(), palette.secondary);
            painter.text(pos2(columns.target_x, y), Align2::LEFT_CENTER, "API / destino", font.clone(), palette.secondary);
            painter.text(pos2(columns.summary_x, y), Align2::LEFT_CENTER, "Resumen", font.clone(), palette.secondary);
            painter.text(pos2(columns.who_x, y), Align2::LEFT_CENTER, "Quién lo pidió", font.clone(), palette.secondary);
            painter.text(pos2(columns.time_right, y), Align2::RIGHT_CENTER, "Hora", font, palette.secondary);
            painter.hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, palette.outline));
        }

        let ids = std::mem::take(&mut self.cache.ids);
        let total = ids.len();
        if total == 0 {
            ui.add_space(26.0);
            ui.vertical_centered(|ui| {
                let text = if netlog::enabled() {
                    "Todavía no hay tráfico que mostrar con estos filtros."
                } else {
                    "El registro de tráfico está apagado (las devtools no estaban habilitadas al arrancar)."
                };
                ui.label(egui::RichText::new(text).color(palette.dim));
            });
            self.cache.ids = ids;
            return;
        }

        let mut selected = self.selected;
        let mut open_conns = std::mem::take(&mut self.open_conns);
        egui::ScrollArea::vertical()
            .id_salt("dev_traffic_table")
            .auto_shrink([false, false])
            .stick_to_bottom(self.follow)
            .show_rows(ui, ROW_H, total, |ui, range| {
                let start = range.start;
                // Se copia lo justo de las filas visibles y se suelta el lock
                // ANTES de dibujar.
                let lines: Vec<Line> = netlog::with(|store| {
                    range
                        .filter_map(|index| ids.get(index).and_then(|id| store.get(*id)))
                        .map(|row| Line {
                            id: row.id,
                            kind: row.kind,
                            dir: row.dir(),
                            http: matches!(row.detail, Detail::Http(_)),
                            target: row.target.clone(),
                            summary: row.summary.clone(),
                            who: row.who.clone(),
                            unix_ms: row.unix_ms,
                            error: row.is_error(),
                            pending: row.is_pending(),
                            conn: row.conn,
                            conn_stats: if is_conn_row(row) {
                                store.conn(row.conn).map(|c| (c.frames_in, c.frames_out, c.closed_ms.is_some()))
                            } else {
                                None
                            },
                        })
                        .collect()
                });
                for (offset, line) in lines.iter().enumerate() {
                    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                    let is_selected = selected == Some(line.id);
                    let columns = Columns::new(rect);
                    let painter = ui.painter();
                    if is_selected {
                        painter.rect_filled(rect, 0.0, palette.surface_active);
                    } else if response.hovered() {
                        painter.rect_filled(rect, 0.0, palette.surface_hover);
                    } else if (start + offset) % 2 == 1 {
                        painter.rect_filled(rect, 0.0, palette.surface.gamma_multiply(0.35));
                    }

                    // Sentido: ▼ entra, ▲ sale, círculo = pedido HTTP, punto = evento;
                    // la fila de una conexión lleva un chevron (> cerrada, v desplegada).
                    let center = pos2(rect.left() + 18.0, rect.center().y);
                    let is_open = open_conns.contains(&line.conn);
                    if line.conn_stats.is_some() {
                        let stroke = Stroke::new(1.8, palette.text);
                        let (a, b, c) = if is_open {
                            (center + vec2(-4.5, -2.5), center + vec2(0.0, 2.5), center + vec2(4.5, -2.5))
                        } else {
                            (center + vec2(-2.5, -4.5), center + vec2(2.5, 0.0), center + vec2(-2.5, 4.5))
                        };
                        painter.line_segment([a, b], stroke);
                        painter.line_segment([b, c], stroke);
                    } else {
                        match line.dir {
                            Some(Dir::In) => painter.add(Shape::convex_polygon(
                                vec![center + vec2(-5.0, -3.5), center + vec2(5.0, -3.5), center + vec2(0.0, 4.5)],
                                style::GREEN,
                                Stroke::NONE,
                            )),
                            Some(Dir::Out) => painter.add(Shape::convex_polygon(
                                vec![center + vec2(-5.0, 3.5), center + vec2(5.0, 3.5), center + vec2(0.0, -4.5)],
                                style::BLUE,
                                Stroke::NONE,
                            )),
                            None if line.http => painter.add(Shape::circle_stroke(
                                center,
                                4.5,
                                Stroke::new(1.5, if line.error { style::RED } else { palette.secondary }),
                            )),
                            None => painter.add(Shape::circle_filled(center, 2.5, palette.dim)),
                        };
                    }

                    let y = rect.center().y;
                    painter.text(
                        pos2(columns.kind_x, y),
                        Align2::LEFT_CENTER,
                        line.kind.label(),
                        theme::semibold(12.5),
                        kind_color(line.kind, palette),
                    );
                    let target_clip = Rect::from_min_max(
                        pos2(columns.target_x, rect.top()),
                        pos2(columns.target_x + columns.target_w, rect.bottom()),
                    );
                    painter.with_clip_rect(target_clip).text(
                        pos2(columns.target_x, y),
                        Align2::LEFT_CENTER,
                        &line.target,
                        theme::regular(12.5),
                        palette.secondary,
                    );
                    let summary_color = if line.error {
                        style::RED
                    } else if line.pending {
                        style::ORANGE
                    } else if line.conn_stats.is_some() {
                        palette.text
                    } else if line.dir.is_none() && !line.http {
                        palette.dim
                    } else {
                        palette.text
                    };
                    let summary_clip = Rect::from_min_max(
                        pos2(columns.summary_x, rect.top()),
                        pos2(columns.summary_right, rect.bottom()),
                    );
                    let summary_text = match line.conn_stats {
                        Some((frames_in, frames_out, closed)) => format!(
                            "{} mensajes (↓ {frames_in} ↑ {frames_out}) · {}{} · {}",
                            frames_in + frames_out,
                            if closed { "cerrada · " } else { "" },
                            if is_open { "clic para ocultarlos" } else { "clic para verlos" },
                            line.summary,
                        ),
                        None => line.summary.clone(),
                    };
                    painter.with_clip_rect(summary_clip).text(
                        pos2(columns.summary_x, y),
                        Align2::LEFT_CENTER,
                        &summary_text,
                        theme::regular(13.0),
                        summary_color,
                    );
                    let who_clip = Rect::from_min_max(
                        pos2(columns.who_x, rect.top()),
                        pos2(columns.who_x + (columns.time_right - columns.who_x - 96.0).max(40.0), rect.bottom()),
                    );
                    painter.with_clip_rect(who_clip).text(
                        pos2(columns.who_x, y),
                        Align2::LEFT_CENTER,
                        &line.who,
                        theme::regular(12.5),
                        palette.secondary,
                    );
                    painter.text(
                        pos2(columns.time_right, y),
                        Align2::RIGHT_CENTER,
                        fmt_clock(line.unix_ms),
                        theme::regular(12.0),
                        palette.dim,
                    );
                    if line.conn_stats.is_some() {
                        // Clic en una conexión: despliega (o esconde) sus mensajes y la selecciona.
                        if response.clicked() {
                            if !open_conns.remove(&line.conn) {
                                open_conns.insert(line.conn);
                            }
                            selected = Some(line.id);
                            ui.ctx().request_repaint();
                        }
                    } else if response.clicked() {
                        selected = if is_selected { None } else { Some(line.id) };
                    }
                }
            });
        self.selected = selected;
        self.open_conns = open_conns;
        self.cache.ids = ids;
    }

    // -- Lista lateral -------------------------------------------------------------------

    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        style::panel_title(ui, palette, "Tráfico");
        let mut counts = [0usize; 10];
        netlog::with(|store| {
            for row in store.rows() {
                counts[row.kind.index()] += 1;
            }
        });
        let conns: Vec<Conn> = netlog::with(|store| store.conns().iter().rev().take(40).cloned().collect());

        egui::ScrollArea::vertical().id_salt("dev_traffic_side").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

            style::side_header(ui, palette, "Tipo de conexión");
            for kind in Kind::ALL {
                let index = kind.index();
                let item = style::side_item(
                    ui,
                    palette,
                    kind.label(),
                    kind_color(kind, palette),
                    self.kinds[index],
                    Some(counts[index]),
                )
                .on_hover_text(kind.describe());
                if item.clicked() {
                    self.kinds[index] = !self.kinds[index];
                }
            }

            style::side_header(ui, palette, "Sentido (WebSocket)");
            if style::side_item(ui, palette, "Entrada ▼", style::GREEN, self.show_in, None).clicked() {
                self.show_in = !self.show_in;
            }
            if style::side_item(ui, palette, "Salida ▲", style::BLUE, self.show_out, None).clicked() {
                self.show_out = !self.show_out;
            }
            if style::side_item(ui, palette, "Solo errores", style::RED, self.only_errors, None).clicked() {
                self.only_errors = !self.only_errors;
            }

            style::side_header(ui, palette, "Conexiones");
            if conns.is_empty() {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("  Ninguna todavía.").color(palette.dim));
            }
            for conn in &conns {
                let open = conn.closed_ms.is_none();
                let label = format!(
                    "#{} {} · {}{}",
                    conn.id,
                    conn.kind.label(),
                    conn.host,
                    if open { "" } else { " (cerrada)" }
                );
                let dot = if open { kind_color(conn.kind, palette) } else { palette.dim };
                let item = style::side_item(
                    ui,
                    palette,
                    &label,
                    dot,
                    self.conn == Some(conn.id),
                    Some((conn.frames_in + conn.frames_out) as usize),
                )
                .on_hover_text(format!(
                    "{}\nAbre: {}\n↓ {} ({}) · ↑ {} ({})",
                    conn.url,
                    conn.who,
                    conn.frames_in,
                    netlog::human_size(conn.bytes_in as usize),
                    conn.frames_out,
                    netlog::human_size(conn.bytes_out as usize),
                ));
                if item.clicked() {
                    // Filtra por esa conexión y despliega sus mensajes (o deshace ambas cosas).
                    if self.conn == Some(conn.id) {
                        self.conn = None;
                        self.open_conns.remove(&conn.id);
                    } else {
                        self.conn = Some(conn.id);
                        self.open_conns.insert(conn.id);
                    }
                }
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                if theme::soft_button(ui, palette, None, "Mostrar todo", false).clicked() {
                    self.show_all();
                }
                if theme::soft_button(ui, palette, None, "Vaciar", false).clicked() {
                    netlog::clear();
                    self.clear_selection();
                }
            });
            ui.add_space(10.0);
        });
    }

    // -- Inspector -----------------------------------------------------------------------

    fn ensure_formatted(&mut self, row: &Row) {
        let stamp = row.summary.clone();
        if self.formatted.as_ref().is_some_and(|f| f.id == row.id && f.stamp == stamp) {
            return;
        }
        let (payload, req_body, resp_body) = match &row.detail {
            Detail::Frame(frame) => (pretty(&frame.payload), String::new(), String::new()),
            Detail::Http(http) => (
                String::new(),
                http.req_body.as_deref().map(pretty).unwrap_or_default(),
                http.resp_body.as_deref().map(pretty).unwrap_or_default(),
            ),
        };
        self.formatted = Some(Formatted { id: row.id, stamp, payload, req_body, resp_body });
    }

    /// Devuelve un texto para avisar ("Copiado").
    pub(super) fn inspector(&mut self, ui: &mut egui::Ui, palette: &Palette) -> Option<&'static str> {
        let row: Option<Row> = self.selected.and_then(|id| netlog::with(|store| store.get(id).cloned()));
        let Some(row) = row else {
            style::panel_title(ui, palette, "Tráfico");
            egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 0)).show(ui, |ui| {
                ui.label(
                    egui::RichText::new("Elige una línea para ver el contenido completo, a qué API fue y quién lo pidió.")
                        .color(palette.dim),
                );
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(
                        "▼ llega al cliente · ▲ sale del cliente · ○ pedido HTTP (pedido y respuesta en una sola línea).",
                    )
                    .small()
                    .color(palette.dim),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "Los tokens, contraseñas y cookies se ocultan antes de guardarse. El audio por UDP solo se \
                         cuenta (bytes que entran). Las descargas que hace el actualizador salen por una librería \
                         externa y no aparecen.",
                    )
                    .small()
                    .color(palette.dim),
                );
                ui.add_space(8.0);
                let held = netlog::with(|store| store.held_bytes());
                ui.label(
                    egui::RichText::new(format!(
                        "Se retienen hasta {} filas ({} usados).",
                        netlog::MAX_ROWS,
                        netlog::human_size(held)
                    ))
                    .small()
                    .color(palette.dim),
                );
            });
            return None;
        };
        self.ensure_formatted(&row);
        let conn: Option<Conn> = if row.conn != 0 { netlog::with(|store| store.conn(row.conn).cloned()) } else { None };
        let formatted = self.formatted.as_ref();
        let mut toast = None;
        let mut only_conn: Option<u64> = None;
        let mut toggle_conn: Option<u64> = None;
        let conn_is_open = self.open_conns.contains(&row.conn);

        style::panel_title(ui, palette, if matches!(row.detail, Detail::Http(_)) { "Pedido" } else { "Mensaje" });
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 0)).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("dev_traffic_detail").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                ui.horizontal_wrapped(|ui| {
                    super::badge(ui, row.kind.label(), kind_color(row.kind, palette));
                    match row.dir() {
                        Some(Dir::In) => super::badge(ui, "ENTRADA", style::GREEN),
                        Some(Dir::Out) => super::badge(ui, "SALIDA", style::BLUE),
                        None => {}
                    }
                    if let Detail::Http(http) = &row.detail {
                        match (http.status, &http.error) {
                            (Some(code), _) => super::badge(ui, &code.to_string(), style::status_color(code)),
                            (None, Some(_)) => super::badge(ui, "ERROR", style::RED),
                            (None, None) => super::badge(ui, "EN CURSO", style::ORANGE),
                        }
                    }
                });
                ui.label(egui::RichText::new(fmt_clock(row.unix_ms)).small().color(palette.dim));
                ui.label(egui::RichText::new(row.kind.describe()).small().color(palette.secondary));

                field(ui, palette, "Quién lo pidió", &row.who);

                match &row.detail {
                    Detail::Http(http) => {
                        field(ui, palette, "API", &http.api);
                        field(ui, palette, "Ruta", &format!("{} {}", http.method, http.route));
                        field(ui, palette, "URL", &http.url);
                        field(ui, palette, "Salió por", &http.via);
                        if let Some(code) = http.status {
                            let mut line = format!("{code} {}", http.version);
                            if let Some(ms) = http.elapsed_ms {
                                line.push_str(&format!(" · {ms} ms"));
                            }
                            if let Some(len) = http.resp_len {
                                line.push_str(&format!(" · {}", netlog::human_size(len)));
                            }
                            field(ui, palette, "Respuesta", &line);
                        }
                        if let Some(error) = &http.error {
                            field(ui, palette, "Error de red", error);
                        }
                        headers_section(ui, palette, "Headers del pedido", &http.req_headers);
                        if let Some(f) = formatted {
                            if !f.req_body.is_empty() {
                                body_section(ui, palette, "Cuerpo del pedido", &f.req_body, http.req_len);
                            }
                            headers_section(ui, palette, "Headers de la respuesta", &http.resp_headers);
                            if !f.resp_body.is_empty() {
                                body_section(
                                    ui,
                                    palette,
                                    "Cuerpo de la respuesta",
                                    &f.resp_body,
                                    http.resp_len.unwrap_or(0),
                                );
                                if http.truncated {
                                    ui.label(
                                        egui::RichText::new("Cuerpo recortado: se guarda solo el comienzo.")
                                            .small()
                                            .color(style::ORANGE),
                                    );
                                }
                            } else if http.resp_len.is_some() {
                                ui.label(
                                    egui::RichText::new("Cuerpo no guardado (no es texto o no se leyó).")
                                        .small()
                                        .color(palette.dim),
                                );
                            }
                        }
                    }
                    Detail::Frame(frame) => {
                        if frame.ftype != FrameType::Info {
                            if let Some(dir) = row.dir() {
                                field(ui, palette, "Sentido", dir_label(dir));
                            }
                            field(ui, palette, "Clase", frame.ftype.label());
                        }
                        if let Some(size) = row.size {
                            field(ui, palette, "Tamaño", &netlog::human_size(size));
                        }
                        // Si el frame está en la tabla es porque su conexión está desplegada.
                        if let Some(f) = formatted {
                            if !f.payload.is_empty() {
                                body_section(ui, palette, "Contenido", &f.payload, row.size.unwrap_or(0));
                                if frame.truncated {
                                    ui.label(
                                        egui::RichText::new("Contenido recortado: se guarda solo el comienzo.")
                                            .small()
                                            .color(style::ORANGE),
                                    );
                                }
                            }
                        }
                    }
                }

                if let Some(conn) = &conn {
                    ui.add_space(8.0);
                    style::field_label(ui, palette, "Conexión");
                    let state = match (&conn.closed_ms, &conn.close_info) {
                        (None, _) => "abierta".to_owned(),
                        (Some(_), Some(info)) => format!("cerrada · {info}"),
                        (Some(_), None) => "cerrada".to_owned(),
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!(
                                "#{} · {}\n{}\nAbrió: {}\n↓ {} msgs ({})  ↑ {} msgs ({})",
                                conn.id,
                                state,
                                conn.url,
                                conn.who,
                                conn.frames_in,
                                netlog::human_size(conn.bytes_in as usize),
                                conn.frames_out,
                                netlog::human_size(conn.bytes_out as usize),
                            ))
                            .monospace()
                            .size(12.0),
                        )
                        .wrap()
                        .selectable(true),
                    );
                    ui.horizontal_wrapped(|ui| {
                        let label = if conn_is_open { "Ocultar mensajes" } else { "Ver mensajes" };
                        if ui.small_button(label).clicked() {
                            toggle_conn = Some(conn.id);
                        }
                        if ui.small_button("Ver solo esta conexión").clicked() {
                            only_conn = Some(conn.id);
                        }
                    });
                }

                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Copiar contenido").clicked() {
                        let text = match (&row.detail, formatted) {
                            (Detail::Frame(_), Some(f)) => f.payload.clone(),
                            (Detail::Http(_), Some(f)) if !f.resp_body.is_empty() => f.resp_body.clone(),
                            _ => row.summary.clone(),
                        };
                        ui.ctx().copy_text(text);
                        toast = Some("Contenido copiado");
                    }
                    if let Detail::Http(http) = &row.detail {
                        if ui.small_button("Copiar URL").clicked() {
                            ui.ctx().copy_text(http.url.clone());
                            toast = Some("URL copiada");
                        }
                    }
                });
                ui.add_space(14.0);
            });
        });
        if let Some(id) = only_conn {
            self.conn = Some(id);
            self.open_conns.insert(id);
        }
        if let Some(id) = toggle_conn {
            if !self.open_conns.remove(&id) {
                self.open_conns.insert(id);
            }
        }
        toast
    }
}

/// Una etiqueta con su valor seleccionable.
fn field(ui: &mut egui::Ui, palette: &Palette, label: &str, value: &str) {
    style::field_label(ui, palette, label);
    ui.add(egui::Label::new(egui::RichText::new(value).monospace().size(12.5)).wrap().selectable(true));
}

fn headers_section(ui: &mut egui::Ui, palette: &Palette, title: &str, headers: &[(String, String)]) {
    if headers.is_empty() {
        return;
    }
    ui.add_space(6.0);
    egui::CollapsingHeader::new(format!("{title} ({})", headers.len()))
        .id_salt(title)
        .default_open(false)
        .show(ui, |ui| {
            let text = headers.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n");
            ui.add(
                egui::Label::new(egui::RichText::new(text).monospace().size(12.0).color(palette.secondary))
                    .wrap()
                    .selectable(true),
            );
        });
}

fn body_section(ui: &mut egui::Ui, palette: &Palette, title: &str, text: &str, total_len: usize) {
    ui.add_space(6.0);
    style::field_label(ui, palette, &format!("{title} · {}", netlog::human_size(total_len)));
    super::inset_panel(ui, palette, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(text).monospace().size(12.0)).wrap().selectable(true));
    });
}
