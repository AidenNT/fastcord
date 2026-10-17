//! Piezas visuales propias de las herramientas de desarrollo: pestañas
//! segmentadas, lista lateral con cuadrito de color, chips de nivel, interruptor
//! y los tres botones de paneles de la barra inferior.
//!
//! Todo se dibuja con el `Painter` sobre rectángulos reservados con
//! `allocate_exact_size`, igual que `theme::pill_button`, para que el aspecto
//! dependa solo de la [`Palette`] activa.

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::logging::capture::Level;
use crate::theme::{self, Palette};

pub(super) const RED: Color32 = Color32::from_rgb(0xE5, 0x5B, 0x5B);
pub(super) const ORANGE: Color32 = Color32::from_rgb(0xF2, 0x9A, 0x38);
pub(super) const YELLOW: Color32 = Color32::from_rgb(0xE5, 0xC0, 0x4B);
pub(super) const GREEN: Color32 = Color32::from_rgb(0x57, 0xC2, 0x7D);
pub(super) const BLUE: Color32 = Color32::from_rgb(0x4D, 0xA3, 0xFF);
pub(super) const PURPLE: Color32 = Color32::from_rgb(0xA7, 0x8B, 0xFA);
pub(super) const PINK: Color32 = Color32::from_rgb(0xF4, 0x72, 0xB6);
pub(super) const TEAL: Color32 = Color32::from_rgb(0x2D, 0xD4, 0xBF);

const TAB_PAD: f32 = 22.0;
const TAB_HEIGHT: f32 = 32.0;

pub(super) fn level_color(level: Level) -> Color32 {
    match level {
        Level::Error => RED,
        Level::Warn => ORANGE,
        Level::Info => BLUE,
        Level::Debug => GREEN,
        Level::Trace => PURPLE,
    }
}

fn level_letter(level: Level) -> &'static str {
    match level {
        Level::Error => "E",
        Level::Warn => "W",
        Level::Info => "I",
        Level::Debug => "D",
        Level::Trace => "T",
    }
}

/// Nombre largo de un nivel para la lista lateral.
pub(super) fn level_label(level: Level) -> &'static str {
    match level {
        Level::Error => "Errores",
        Level::Warn => "Advertencias",
        Level::Info => "Información",
        Level::Debug => "Depuración",
        Level::Trace => "Traza",
    }
}

pub(super) fn method_color(method: &str) -> Color32 {
    match method {
        "GET" => GREEN,
        "POST" => BLUE,
        "PUT" => ORANGE,
        "PATCH" => PURPLE,
        "DELETE" => RED,
        _ => TEAL,
    }
}

pub(super) fn status_color(code: u16) -> Color32 {
    match code {
        200..=299 => GREEN,
        300..=399 => BLUE,
        400..=499 => ORANGE,
        _ => RED,
    }
}

/// Color estable para un origen de log (el mismo texto da siempre el mismo color).
pub(super) fn target_color(target: &str) -> Color32 {
    const COLORS: [Color32; 7] = [BLUE, GREEN, ORANGE, PURPLE, PINK, TEAL, YELLOW];
    let hash = target.bytes().fold(5381u32, |acc, byte| acc.wrapping_mul(33).wrapping_add(u32::from(byte)));
    COLORS[(hash as usize) % COLORS.len()]
}

/// Círculo con la inicial del nivel (el equivalente a los íconos de severidad).
pub(super) fn level_dot(painter: &egui::Painter, center: egui::Pos2, radius: f32, level: Level, dim: bool) {
    let mut color = level_color(level);
    if dim {
        color = color.gamma_multiply(0.35);
    }
    painter.circle_filled(center, radius, color.gamma_multiply(0.22));
    painter.circle_stroke(center, radius, Stroke::new(1.2, color));
    painter.text(center, Align2::CENTER_CENTER, level_letter(level), theme::bold(radius * 1.15), color);
}

// ---------------------------------------------------------------------------
// Pestañas
// ---------------------------------------------------------------------------

/// Ancho que ocupa [`segmented`] (para poder centrarlo).
pub(super) fn segmented_width<T>(ui: &egui::Ui, items: &[(T, &str)]) -> f32 {
    let font = theme::semibold(13.0);
    let labels: f32 = items
        .iter()
        .map(|(_, label)| {
            ui.painter().layout_no_wrap((*label).to_owned(), font.clone(), Color32::WHITE).size().x + TAB_PAD * 2.0
        })
        .sum();
    labels + 8.0
}

/// Pestañas en una píldora; la activa va con el color de acento.
pub(super) fn segmented<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    palette: &Palette,
    current: &mut T,
    items: &[(T, &str)],
) {
    let font = theme::semibold(13.0);
    let total = segmented_width(ui, items);
    let (outer, _) = ui.allocate_exact_size(vec2(total, TAB_HEIGHT + 8.0), Sense::hover());
    ui.painter().rect_filled(outer, CornerRadius::same(12), palette.surface);
    ui.painter().rect_stroke(outer, CornerRadius::same(12), Stroke::new(1.0, palette.outline), StrokeKind::Inside);

    let mut x = outer.left() + 4.0;
    for (value, label) in items {
        let width =
            ui.painter().layout_no_wrap((*label).to_owned(), font.clone(), Color32::WHITE).size().x + TAB_PAD * 2.0;
        let rect = Rect::from_min_size(pos2(x, outer.top() + 4.0), vec2(width, TAB_HEIGHT));
        let response = ui
            .interact(rect, ui.id().with(("dev_segmented", *label)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let active = *current == *value;
        if active {
            ui.painter().rect_filled(rect, CornerRadius::same(9), palette.accent);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(9), palette.surface_hover);
        }
        let color = if active { palette.on_accent } else { palette.secondary };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, *label, font.clone(), color);
        if response.clicked() {
            *current = *value;
        }
        x += width;
    }
}

// ---------------------------------------------------------------------------
// Paneles laterales
// ---------------------------------------------------------------------------

/// Título grande y apagado del tope de un panel (como "AWS Components").
pub(super) fn panel_title(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
    ui.painter().with_clip_rect(rect).text(
        pos2(rect.left() + 18.0, rect.center().y + 2.0),
        Align2::LEFT_CENTER,
        text,
        theme::bold(16.0),
        palette.dim,
    );
}

/// Subtítulo de grupo dentro de una lista lateral.
pub(super) fn side_header(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    ui.painter().text(
        pos2(rect.left() + 18.0, rect.bottom() - 6.0),
        Align2::LEFT_BOTTOM,
        text.to_uppercase(),
        theme::semibold(10.5),
        palette.dim,
    );
}

/// Fila de una lista lateral: texto a la izquierda, cantidad opcional y un
/// cuadrito de color a la derecha. Un `dot` transparente no dibuja el cuadrito.
pub(super) fn side_item(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    dot: Color32,
    selected: bool,
    count: Option<usize>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let inner = rect.shrink2(vec2(6.0, 2.0));
        if selected {
            painter.rect_filled(inner, CornerRadius::same(8), palette.surface_active);
        } else if response.hovered() {
            painter.rect_filled(inner, CornerRadius::same(8), palette.surface_hover);
        }
        let square_x = rect.right() - 22.0;
        if dot != Color32::TRANSPARENT {
            let square = Rect::from_center_size(pos2(square_x, rect.center().y), vec2(10.0, 10.0));
            let tint = if selected || response.hovered() { dot } else { dot.gamma_multiply(0.8) };
            painter.rect_filled(square, CornerRadius::same(3), tint);
        }
        let mut text_right = square_x - 14.0;
        if let Some(count) = count {
            let used = painter.text(
                pos2(text_right, rect.center().y),
                Align2::RIGHT_CENTER,
                count.to_string(),
                theme::regular(11.5),
                palette.dim,
            );
            text_right = used.left() - 8.0;
        }
        let clip = Rect::from_min_max(rect.left_top(), pos2(text_right, rect.bottom()));
        painter.with_clip_rect(clip).text(
            pos2(rect.left() + 18.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            theme::medium(13.5),
            if selected { palette.text } else { palette.secondary },
        );
        painter.hline(
            rect.left() + 14.0..=rect.right() - 14.0,
            rect.bottom() - 0.5,
            Stroke::new(1.0, palette.outline.gamma_multiply(0.7)),
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Etiqueta de un campo del inspector.
pub(super) fn field_label(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new(text).font(theme::regular(13.0)).color(palette.secondary));
    ui.add_space(3.0);
}

// ---------------------------------------------------------------------------
// Barra inferior
// ---------------------------------------------------------------------------

/// Chip con la cantidad de líneas de un nivel; pulsarlo lo muestra/oculta.
pub(super) fn count_chip(
    ui: &mut egui::Ui,
    palette: &Palette,
    level: Level,
    count: u64,
    active: bool,
) -> egui::Response {
    let text = if count >= 100_000 { "99k+".to_owned() } else { count.to_string() };
    let font = theme::medium(13.0);
    let galley = ui.painter().layout_no_wrap(text.clone(), font.clone(), palette.text);
    let width = 34.0 + galley.size().x;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, CornerRadius::same(8), palette.surface_hover);
        }
        level_dot(painter, pos2(rect.left() + 14.0, rect.center().y), 8.0, level, !active);
        painter.text(
            pos2(rect.left() + 28.0, rect.center().y),
            Align2::LEFT_CENTER,
            text,
            font,
            if active { palette.text } else { palette.dim },
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("{} — clic para {}", level_label(level), if active { "ocultar" } else { "mostrar" }))
}

/// Interruptor tipo "pastilla" (el de "Hide Threats on Diagram").
pub(super) fn toggle_switch(ui: &mut egui::Ui, palette: &Palette, on: &mut bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    if response.clicked() {
        *on = !*on;
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let track = if *on { palette.accent } else { palette.surface_active };
        painter.rect_filled(rect, CornerRadius::same(10), track);
        let knob_x = if *on { rect.right() - 10.0 } else { rect.left() + 10.0 };
        painter.circle_filled(pos2(knob_x, rect.center().y), 7.0, if *on { palette.on_accent } else { palette.secondary });
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[derive(Clone, Copy)]
pub(super) enum PanelSide {
    Left,
    Bottom,
    Right,
}

/// Botón que muestra/oculta un panel, con el dibujo de "ventana con ese lado relleno".
pub(super) fn panel_toggle(
    ui: &mut egui::Ui,
    palette: &Palette,
    side: PanelSide,
    on: bool,
    tooltip: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(32.0, 26.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let color = if on {
            palette.accent
        } else if response.hovered() {
            palette.text
        } else {
            palette.dim
        };
        let frame = Rect::from_center_size(rect.center(), vec2(20.0, 16.0));
        painter.rect_stroke(frame, CornerRadius::same(3), Stroke::new(1.6, color), StrokeKind::Inside);
        let filled = match side {
            PanelSide::Left => Rect::from_min_max(frame.min, pos2(frame.left() + 7.0, frame.bottom())),
            PanelSide::Right => Rect::from_min_max(pos2(frame.right() - 7.0, frame.top()), frame.max),
            PanelSide::Bottom => Rect::from_min_max(pos2(frame.left(), frame.bottom() - 6.0), frame.max),
        };
        painter.rect_filled(filled.shrink(1.5), CornerRadius::same(1), color.gamma_multiply(if on { 1.0 } else { 0.55 }));
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip)
}

/// Botón de ícono de la barra superior (cerrar, limpiar...).
pub(super) fn bar_icon(ui: &mut egui::Ui, palette: &Palette, icon: theme::Icon, tooltip: &str) -> egui::Response {
    theme::icon_button(ui, icon, 17.0, palette.secondary, palette.text, tooltip)
}

/// Aviso breve ("Copiado") pintado sobre el borde inferior de `area`.
pub(super) fn paint_toast(ui: &egui::Ui, palette: &Palette, area: Rect, text: &str) {
    let font = theme::medium(13.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), palette.text);
    let size = galley.size() + vec2(32.0, 16.0);
    let rect = Rect::from_center_size(pos2(area.center().x, area.bottom() - 28.0 - size.y / 2.0), size);
    let painter = ui.painter();
    painter.rect_filled(rect.translate(vec2(0.0, 2.0)), CornerRadius::same(12), Color32::from_black_alpha(90));
    painter.rect_filled(rect, CornerRadius::same(12), palette.surface_active);
    painter.rect_stroke(rect, CornerRadius::same(12), Stroke::new(1.0, palette.outline), StrokeKind::Inside);
    painter.text(rect.center(), Align2::CENTER_CENTER, text, font, palette.text);
}
