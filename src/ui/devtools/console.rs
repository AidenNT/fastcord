//! Consola de las herramientas de desarrollo: tabla de logs (la misma que se
//! muestra en el panel inferior de las otras pestañas), filtros por nivel y por
//! origen, línea de comandos e inspector de la línea seleccionada.
//!
//! Los datos viven en `logging::capture`; acá solo hay estado de la vista (qué
//! niveles se ven, qué línea está elegida, caché de la lista filtrada).

use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

use egui::{Align2, Rect, Sense, Stroke, pos2, vec2};

use super::style;
use crate::logging::capture::{self, Entry, Level};
use crate::theme::{self, Palette};

const ROW_H: f32 = 28.0;
const HISTORY_MAX: usize = 100;

#[derive(Default)]
struct Cache {
    /// Ids de las entradas que pasan el filtro, en orden.
    ids: Vec<u64>,
    key: u64,
    epoch: u64,
    last_id: u64,
    valid: bool,
}

pub(super) struct ConsoleState {
    /// Qué niveles se muestran (índice = `Level::index`).
    pub levels: [bool; 5],
    pub hidden_targets: BTreeSet<String>,
    /// Texto que tiene que aparecer en el mensaje o el origen.
    pub filter: String,
    pub selected: Option<u64>,
    /// Pegarse al final cuando llegan líneas nuevas.
    pub follow: bool,
    input: String,
    history: Vec<String>,
    history_pos: Option<usize>,
    cache: Cache,
}

impl Default for ConsoleState {
    fn default() -> Self {
        Self {
            levels: [true; 5],
            hidden_targets: BTreeSet::new(),
            filter: String::new(),
            selected: None,
            follow: true,
            input: String::new(),
            history: Vec::new(),
            history_pos: None,
            cache: Cache::default(),
        }
    }
}

/// Hora local `HH:MM:SS.mmm` de un timestamp en milisegundos Unix.
pub(super) fn fmt_clock(unix_ms: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(unix_ms) {
        Some(utc) => utc.with_timezone(&chrono::Local).format("%H:%M:%S%.3f").to_string(),
        None => "--:--:--".to_owned(),
    }
}

/// Una línea lista para copiar al portapapeles.
pub(super) fn fmt_line(entry: &Entry) -> String {
    format!("{} {:<5} {}: {}", fmt_clock(entry.unix_ms), entry.level.tag(), entry.target, entry.message)
}

fn contains_ci(haystack: &str, needle_lower: &str) -> bool {
    haystack.to_lowercase().contains(needle_lower)
}

/// Primera línea de un mensaje (los multilínea se resumen con `…`).
fn one_line(message: &str) -> String {
    let mut lines = message.lines();
    let first = lines.next().unwrap_or("").replace('\t', " ");
    if lines.next().is_some() { format!("{first} …") } else { first }
}

/// Posiciones de las columnas de la tabla para un ancho dado.
struct Columns {
    target_x: f32,
    target_w: f32,
    message_x: f32,
    message_right: f32,
    time_right: f32,
}

impl Columns {
    fn new(rect: Rect) -> Self {
        let target_w = (rect.width() * 0.2).clamp(110.0, 220.0);
        let target_x = rect.left() + 58.0;
        let message_x = target_x + target_w + 10.0;
        Self {
            target_x,
            target_w,
            message_x,
            message_right: (rect.right() - 112.0).max(message_x + 40.0),
            time_right: rect.right() - 14.0,
        }
    }
}

/// Qué pidió el usuario en la línea de comandos.
pub(super) enum ConsoleAction {
    Run(String),
}

impl ConsoleState {
    fn filter_key(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.levels.hash(&mut hasher);
        self.filter.trim().to_lowercase().hash(&mut hasher);
        for target in &self.hidden_targets {
            target.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Pone al día la lista filtrada. Si solo llegaron líneas nuevas se filtran
    /// únicamente esas; si cambió el filtro o se limpió el búfer, se rehace.
    pub(super) fn refresh(&mut self) {
        let key = self.filter_key();
        let levels = self.levels;
        let needle = self.filter.trim().to_lowercase();
        let hidden = self.hidden_targets.clone();
        let mut ids = std::mem::take(&mut self.cache.ids);
        let (prev_key, prev_epoch, prev_last, prev_valid) =
            (self.cache.key, self.cache.epoch, self.cache.last_id, self.cache.valid);

        let (epoch, last_id) = capture::with(|buffer| {
            let matches = |entry: &Entry| -> bool {
                levels[entry.level.index()]
                    && !hidden.contains(&entry.target)
                    && (needle.is_empty() || contains_ci(&entry.message, &needle) || contains_ci(&entry.target, &needle))
            };
            let incremental = prev_valid && key == prev_key && buffer.epoch() == prev_epoch;
            if incremental {
                // Quitar lo que el búfer circular ya descartó y sumar lo nuevo.
                let first = buffer.first_id();
                let evicted = ids.partition_point(|id| *id < first);
                ids.drain(..evicted);
                let mut fresh: Vec<u64> = Vec::new();
                for entry in buffer.entries().iter().rev() {
                    if entry.id <= prev_last {
                        break;
                    }
                    if matches(entry) {
                        fresh.push(entry.id);
                    }
                }
                ids.extend(fresh.into_iter().rev());
            } else {
                ids.clear();
                ids.extend(buffer.entries().iter().filter(|entry| matches(entry)).map(|entry| entry.id));
            }
            (buffer.epoch(), buffer.last_id())
        });
        self.cache = Cache { ids, key, epoch, last_id, valid: true };
    }

    /// Todas las líneas visibles (con el filtro actual) como texto.
    pub(super) fn visible_text(&mut self) -> String {
        self.refresh();
        let ids = &self.cache.ids;
        capture::with(|buffer| {
            ids.iter().filter_map(|id| buffer.get(*id)).map(fmt_line).collect::<Vec<_>>().join("\n")
        })
    }

    pub(super) fn visible_count(&self) -> usize {
        self.cache.ids.len()
    }

    pub(super) fn show_all(&mut self) {
        self.levels = [true; 5];
        self.hidden_targets.clear();
        self.filter.clear();
    }

    // -- Tabla ------------------------------------------------------------------

    /// Cabecera + filas. Se usa tanto en la pestaña Consola como en el panel
    /// inferior de las demás (`salt` distingue el estado de scroll de cada una).
    pub(super) fn log_table(&mut self, ui: &mut egui::Ui, palette: &Palette, salt: &str) {
        self.refresh();

        let (header, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
        {
            let columns = Columns::new(header);
            let painter = ui.painter();
            let font = theme::medium(12.5);
            let y = header.center().y;
            painter.text(pos2(header.left() + 18.0, y), Align2::LEFT_CENTER, "Nivel", font.clone(), palette.secondary);
            painter.text(pos2(columns.target_x, y), Align2::LEFT_CENTER, "Origen", font.clone(), palette.secondary);
            painter.text(pos2(columns.message_x, y), Align2::LEFT_CENTER, "Mensaje", font.clone(), palette.secondary);
            painter.text(pos2(columns.time_right, y), Align2::RIGHT_CENTER, "Hora", font, palette.secondary);
            painter.hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, palette.outline));
        }

        let ids = std::mem::take(&mut self.cache.ids);
        let total = ids.len();
        if total == 0 {
            ui.add_space(26.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Sin registros para mostrar.").color(palette.dim));
            });
            self.cache.ids = ids;
            return;
        }

        let mut selected = self.selected;
        let mut copy: Option<String> = None;
        egui::ScrollArea::vertical()
            .id_salt(salt)
            .auto_shrink([false, false])
            .stick_to_bottom(self.follow)
            .show_rows(ui, ROW_H, total, |ui, range| {
                let start = range.start;
                // Se copian las filas visibles y se suelta el lock ANTES de
                // dibujar (ver la advertencia de `logging::capture`).
                let rows: Vec<Entry> = capture::with(|buffer| {
                    range.filter_map(|index| ids.get(index).and_then(|id| buffer.get(*id)).cloned()).collect()
                });
                for (offset, entry) in rows.iter().enumerate() {
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                    let is_selected = selected == Some(entry.id);
                    {
                        let columns = Columns::new(rect);
                        let painter = ui.painter();
                        if is_selected {
                            painter.rect_filled(rect, 0.0, palette.surface_active);
                        } else if response.hovered() {
                            painter.rect_filled(rect, 0.0, palette.surface_hover);
                        } else if (start + offset) % 2 == 1 {
                            painter.rect_filled(rect, 0.0, palette.surface.gamma_multiply(0.35));
                        }
                        style::level_dot(painter, pos2(rect.left() + 28.0, rect.center().y), 8.0, entry.level, false);

                        let y = rect.center().y;
                        let target_clip = Rect::from_min_max(
                            pos2(columns.target_x, rect.top()),
                            pos2(columns.target_x + columns.target_w, rect.bottom()),
                        );
                        painter.with_clip_rect(target_clip).text(
                            pos2(columns.target_x, y),
                            Align2::LEFT_CENTER,
                            &entry.target,
                            theme::medium(12.5),
                            style::target_color(&entry.target),
                        );

                        let message_color = match entry.level {
                            Level::Error => style::RED,
                            Level::Warn => style::ORANGE,
                            Level::Trace | Level::Debug => palette.secondary,
                            Level::Info => palette.text,
                        };
                        let message_clip = Rect::from_min_max(
                            pos2(columns.message_x, rect.top()),
                            pos2(columns.message_right, rect.bottom()),
                        );
                        painter.with_clip_rect(message_clip).text(
                            pos2(columns.message_x, y),
                            Align2::LEFT_CENTER,
                            one_line(&entry.message),
                            theme::regular(13.0),
                            message_color,
                        );
                        painter.text(
                            pos2(columns.time_right, y),
                            Align2::RIGHT_CENTER,
                            fmt_clock(entry.unix_ms),
                            theme::regular(12.0),
                            palette.dim,
                        );
                    }
                    if response.clicked() {
                        selected = if is_selected { None } else { Some(entry.id) };
                    }
                    if response.double_clicked() {
                        copy = Some(fmt_line(entry));
                    }
                }
            });
        self.selected = selected;
        self.cache.ids = ids;
        if let Some(text) = copy {
            ui.ctx().copy_text(text);
        }
    }

    // -- Línea de comandos -------------------------------------------------------------

    /// Campo de comandos al pie de la consola. Devuelve la acción pedida, si hay.
    pub(super) fn input_row(&mut self, ui: &mut egui::Ui, palette: &Palette) -> Option<ConsoleAction> {
        let mut action = None;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
        ui.painter().hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, palette.outline));

        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("dev_console_input_row")
                .max_rect(rect.shrink2(vec2(14.0, 7.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        row.label(egui::RichText::new("›").font(theme::bold(18.0)).color(palette.accent));
        row.add_space(6.0);
        let response = row.add(
            egui::TextEdit::singleline(&mut self.input)
                .id(egui::Id::new("dev_console_input"))
                .font(egui::TextStyle::Monospace)
                .frame(egui::Frame::NONE)
                .desired_width((row.available_width() - 6.0).max(80.0))
                .hint_text("Escribe un comando — help para ver la lista"),
        );

        if response.has_focus() {
            let up = row.input(|input| input.key_pressed(egui::Key::ArrowUp));
            let down = row.input(|input| input.key_pressed(egui::Key::ArrowDown));
            if up && !self.history.is_empty() {
                let next = match self.history_pos {
                    None => self.history.len() - 1,
                    Some(position) => position.saturating_sub(1),
                };
                self.history_pos = Some(next);
                self.input = self.history[next].clone();
            } else if down {
                match self.history_pos {
                    Some(position) if position + 1 < self.history.len() => {
                        self.history_pos = Some(position + 1);
                        self.input = self.history[position + 1].clone();
                    }
                    Some(_) => {
                        self.history_pos = None;
                        self.input.clear();
                    }
                    None => {}
                }
            }
        }

        let submitted = response.lost_focus() && row.input(|input| input.key_pressed(egui::Key::Enter));
        if submitted {
            let line = self.input.trim().to_owned();
            self.input.clear();
            self.history_pos = None;
            if !line.is_empty() {
                if self.history.last() != Some(&line) {
                    self.history.push(line.clone());
                    if self.history.len() > HISTORY_MAX {
                        self.history.remove(0);
                    }
                }
                action = Some(ConsoleAction::Run(line));
            }
            response.request_focus();
        }
        action
    }

    // -- Paneles laterales --------------------------------------------------------------

    /// Lista lateral de la consola: niveles y orígenes con su cantidad.
    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        style::panel_title(ui, palette, "Consola");
        egui::ScrollArea::vertical().id_salt("dev_console_side").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            style::side_header(ui, palette, "Niveles");
            let counts = capture::counts();
            for level in Level::ALL {
                let index = level.index();
                let selected = self.levels[index];
                let item = style::side_item(
                    ui,
                    palette,
                    style::level_label(level),
                    style::level_color(level),
                    selected,
                    Some(counts[index].min(usize::MAX as u64) as usize),
                );
                if item.clicked() {
                    self.levels[index] = !self.levels[index];
                }
            }

            style::side_header(ui, palette, "Orígenes");
            let mut targets: Vec<(String, u64)> =
                capture::with(|buffer| buffer.targets().iter().map(|(name, count)| (name.clone(), *count)).collect());
            targets.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            if targets.is_empty() {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("  Todavía no hay registros.").color(palette.dim));
            }
            for (name, count) in targets.into_iter().take(60) {
                let visible = !self.hidden_targets.contains(&name);
                let item = style::side_item(ui, palette, &name, style::target_color(&name), visible, Some(count as usize));
                if item.clicked() {
                    if visible {
                        self.hidden_targets.insert(name);
                    } else {
                        self.hidden_targets.remove(&name);
                    }
                }
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                if theme::soft_button(ui, palette, None, "Mostrar todo", false).clicked() {
                    self.show_all();
                }
            });
            ui.add_space(10.0);
        });
    }

    /// Inspector de la línea elegida. Devuelve un texto para avisar ("Copiado").
    pub(super) fn inspector(&mut self, ui: &mut egui::Ui, palette: &Palette) -> Option<&'static str> {
        let mut toast = None;
        let selected: Option<Entry> = self.selected.and_then(|id| capture::with(|buffer| buffer.get(id).cloned()));
        let counts = capture::counts();

        let Some(entry) = selected else {
            style::panel_title(ui, palette, "Registro");
            egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 0)).show(ui, |ui| {
                ui.label(
                    egui::RichText::new("Elige una línea de la tabla para ver el mensaje completo.").color(palette.dim),
                );
                ui.add_space(14.0);
                for level in Level::ALL {
                    ui.horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                        style::level_dot(ui.painter(), dot.center(), 8.0, level, false);
                        ui.label(egui::RichText::new(style::level_label(level)).color(palette.secondary));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(counts[level.index()].to_string()).monospace());
                        });
                    });
                }
                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new(format!("{} líneas visibles de {} retenidas.", self.visible_count(), capture::CAPACITY))
                        .small()
                        .color(palette.dim),
                );
            });
            return None;
        };

        style::panel_title(ui, palette, "Registro seleccionado");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 0)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                super::badge(ui, entry.level.tag(), style::level_color(entry.level));
                ui.label(egui::RichText::new(&entry.target).monospace().color(style::target_color(&entry.target)));
            });
            ui.add_space(2.0);
            ui.label(egui::RichText::new(fmt_clock(entry.unix_ms)).small().color(palette.dim));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("Copiar mensaje").clicked() {
                    ui.ctx().copy_text(entry.message.clone());
                    toast = Some("Mensaje copiado");
                }
                if ui.small_button("Copiar línea").clicked() {
                    ui.ctx().copy_text(fmt_line(&entry));
                    toast = Some("Línea copiada");
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("Ocultar este origen").clicked() {
                    self.hidden_targets.insert(entry.target.clone());
                    self.selected = None;
                }
                if ui.small_button("Solo este origen").clicked() {
                    let others: Vec<String> = capture::with(|buffer| buffer.targets().keys().cloned().collect());
                    self.hidden_targets = others.into_iter().filter(|name| *name != entry.target).collect();
                }
            });
            ui.add_space(8.0);
            super::inset_panel(ui, palette, |ui| {
                egui::ScrollArea::vertical().id_salt("dev_console_detail").auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&entry.message).monospace().size(12.5))
                            .wrap()
                            .selectable(true),
                    );
                });
            });
        });
        toast
    }
}
