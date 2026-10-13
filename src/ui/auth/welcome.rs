//! Pantalla de bienvenida de la primera vez: elegir entre la interfaz clásica
//! y la nueva (rediseño). Se muestra justo después de iniciar sesión, solo a
//! quien abre la app por primera vez (`App::show_welcome`); la elección se aplica con
//! `App::choose_interface` y se puede cambiar luego en Ajustes → Apariencia.

use egui::{Align2, CornerRadius, Frame, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::lib::state::App;
use crate::ui::shell::extra;
use crate::ui::theme::{self, Palette};

const CARD_H: f32 = 300.0;
const CARD_GAP: f32 = 20.0;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let mut chosen: Option<bool> = None;

    egui::CentralPanel::default()
        .frame(Frame::new().fill(palette.window))
        .show(ui, |ui| {
            let rect = ui.max_rect();

            // Mismo degradado del login, solo con fondo sólido (si el tema trae
            // degradado/imagen, se deja ver ese).
            if app.backdrop.kind == crate::theme::BackdropKind::Solid {
                let top = extra::blend(palette.window, palette.accent, 0.14);
                extra::paint_vertical_gradient(ui, rect, top, palette.window);
            }

            let wide = rect.width() >= 780.0;
            let card_w = if wide { 340.0 } else { (rect.width() - 48.0).clamp(240.0, 420.0) };
            let content_h = if wide { 150.0 + CARD_H } else { 150.0 + 2.0 * CARD_H + CARD_GAP };

            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(((rect.height() - content_h - 40.0) / 2.0).max(24.0));
                    theme::text(ui, "Te damos la bienvenida a eCord", theme::bold(28.0), palette.text);
                    ui.add_space(8.0);
                    theme::subtle(
                        ui,
                        &palette,
                        "Elige cómo quieres ver la app. Es solo una preferencia: puedes cambiarla \
                         cuando quieras en Ajustes → Apariencia.",
                    );
                    ui.add_space(24.0);

                    if wide {
                        let total = card_w * 2.0 + CARD_GAP;
                        ui.horizontal(|ui| {
                            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
                            ui.spacing_mut().item_spacing.x = CARD_GAP;
                            if interface_card(ui, &palette, card_w, false).clicked() {
                                chosen = Some(false);
                            }
                            if interface_card(ui, &palette, card_w, true).clicked() {
                                chosen = Some(true);
                            }
                        });
                    } else {
                        if interface_card(ui, &palette, card_w, false).clicked() {
                            chosen = Some(false);
                        }
                        ui.add_space(CARD_GAP);
                        if interface_card(ui, &palette, card_w, true).clicked() {
                            chosen = Some(true);
                        }
                    }

                    ui.add_space(22.0);
                    theme::subtle(
                        ui,
                        &palette,
                        "La supresión de ruido del micrófono ya viene activada. \
                         Puedes ajustarla en Ajustes → Voz y audio.",
                    );
                    ui.add_space(24.0);
                });
            });
        });

    if let Some(modern) = chosen {
        app.choose_interface(modern);
    }
}

/// Tarjeta clicable de una interfaz: miniatura, nombre, descripción y botón.
fn interface_card(ui: &mut egui::Ui, palette: &Palette, w: f32, modern: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, CARD_H), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = resp.hovered();
    let painter = ui.painter();

    let radius = CornerRadius::same(theme::CARD_RADIUS + 2);
    let fill = if hovered {
        extra::blend(palette.panel, palette.accent, 0.10)
    } else {
        palette.panel
    };
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(
            if hovered { 2.0 } else { 1.0 },
            if hovered { palette.accent } else { palette.outline },
        ),
        StrokeKind::Inside,
    );

    let preview = Rect::from_min_size(rect.min + Vec2::splat(14.0), Vec2::new(w - 28.0, 128.0));
    if modern {
        paint_modern_preview(painter, palette, preview);
    } else {
        paint_classic_preview(painter, palette, preview);
    }

    let x = rect.left() + 18.0;
    let (title, desc) = if modern {
        (
            "Interfaz nueva",
            "Rediseño completo: tarjetas redondeadas que flotan sobre el fondo, barra de \
             llamada a todo el ancho e Inicio en tablero.",
        )
    } else {
        (
            "Interfaz clásica",
            "El diseño de siempre: paneles planos y compactos, con la barra de llamada en el \
             panel izquierdo.",
        )
    };
    painter.text(
        Pos2::new(x, preview.bottom() + 16.0),
        Align2::LEFT_TOP,
        title,
        theme::semibold(17.0),
        palette.text,
    );
    let galley = painter.layout(desc.to_owned(), theme::regular(12.5), palette.secondary, w - 36.0);
    painter.galley(Pos2::new(x, preview.bottom() + 42.0), galley, palette.secondary);

    let button = Rect::from_min_size(
        Pos2::new(x, rect.bottom() - 18.0 - 32.0),
        Vec2::new(w - 36.0, 32.0),
    );
    painter.rect_filled(
        button,
        CornerRadius::same(10),
        if hovered { palette.accent_hover } else { palette.accent },
    );
    painter.text(
        button.center(),
        Align2::CENTER_CENTER,
        "Usar esta interfaz",
        theme::semibold(13.5),
        palette.on_accent,
    );

    resp
}

fn bar(p: &Painter, r: Rect, color: egui::Color32) {
    p.rect_filled(r, CornerRadius::same(2), color);
}

/// Miniatura de la interfaz clásica: columnas planas pegadas, con una línea fina
/// de separación.
fn paint_classic_preview(p: &Painter, pal: &Palette, r: Rect) {
    p.rect_filled(r, CornerRadius::same(10), pal.window);

    // Rail de servidores.
    let rail = Rect::from_min_size(r.min, Vec2::new(26.0, r.height()));
    p.rect_filled(rail, CornerRadius { nw: 10, ne: 0, sw: 10, se: 0 }, pal.surface);
    for i in 0..4 {
        p.circle_filled(
            Pos2::new(rail.center().x, rail.top() + 16.0 + i as f32 * 26.0),
            7.0,
            if i == 0 { pal.accent } else { pal.surface_active },
        );
    }

    // Lista de canales.
    let list = Rect::from_min_size(Pos2::new(r.left() + 26.0, r.top()), Vec2::new(64.0, r.height()));
    p.rect_filled(list, 0.0, pal.panel);
    for i in 0..5 {
        let y = list.top() + 14.0 + i as f32 * 20.0;
        let color = if i == 1 { extra::blend(pal.panel, pal.accent, 0.35) } else { pal.surface_active };
        bar(p, Rect::from_min_size(Pos2::new(list.left() + 8.0, y), Vec2::new(48.0, 11.0)), color);
    }
    for x in [rail.right(), list.right()] {
        p.line_segment(
            [Pos2::new(x, r.top()), Pos2::new(x, r.bottom())],
            Stroke::new(1.0, pal.outline),
        );
    }

    // Chat.
    let chat_left = list.right() + 12.0;
    let chat_w = r.right() - chat_left - 12.0;
    for (i, frac) in [0.8_f32, 0.55, 0.9, 0.4].iter().enumerate() {
        bar(
            p,
            Rect::from_min_size(
                Pos2::new(chat_left, r.top() + 14.0 + i as f32 * 20.0),
                Vec2::new(chat_w * frac, 9.0),
            ),
            pal.surface_active,
        );
    }
    p.rect_filled(
        Rect::from_min_size(Pos2::new(chat_left, r.bottom() - 26.0), Vec2::new(chat_w, 16.0)),
        CornerRadius::same(6),
        pal.surface,
    );
}

/// Miniatura de la interfaz nueva: tarjetas redondeadas con huecos entre sí y
/// una barra de llamada flotante abajo.
fn paint_modern_preview(p: &Painter, pal: &Palette, r: Rect) {
    p.rect_filled(r, CornerRadius::same(10), pal.window);

    let gap = 5.0;
    let call_h = 14.0;
    let top = r.top() + gap;
    let bottom = r.bottom() - gap - call_h - gap;
    let card = |x0: f32, x1: f32| {
        let c = Rect::from_min_max(Pos2::new(x0, top), Pos2::new(x1, bottom));
        p.rect_filled(c, CornerRadius::same(8), pal.panel);
        p.rect_stroke(c, CornerRadius::same(8), Stroke::new(1.0, pal.outline), StrokeKind::Inside);
        c
    };

    // Rail, canales y chat, cada uno su tarjeta.
    let rail = card(r.left() + gap, r.left() + gap + 24.0);
    let list = card(rail.right() + gap, rail.right() + gap + 58.0);
    let chat = card(list.right() + gap, r.right() - gap);

    for i in 0..3 {
        p.circle_filled(
            Pos2::new(rail.center().x, rail.top() + 13.0 + i as f32 * 24.0),
            7.0,
            if i == 0 { pal.accent } else { pal.surface_active },
        );
    }
    for i in 0..4 {
        let color = if i == 1 { extra::blend(pal.panel, pal.accent, 0.35) } else { pal.surface_active };
        p.rect_filled(
            Rect::from_min_size(
                Pos2::new(list.left() + 6.0, list.top() + 10.0 + i as f32 * 19.0),
                Vec2::new(list.width() - 12.0, 11.0),
            ),
            CornerRadius::same(5),
            color,
        );
    }
    for (i, frac) in [0.8_f32, 0.55, 0.9].iter().enumerate() {
        bar(
            p,
            Rect::from_min_size(
                Pos2::new(chat.left() + 10.0, chat.top() + 12.0 + i as f32 * 18.0),
                Vec2::new((chat.width() - 20.0) * frac, 9.0),
            ),
            pal.surface_active,
        );
    }

    // Barra de llamada a todo el ancho.
    let call = Rect::from_min_max(
        Pos2::new(r.left() + gap, r.bottom() - gap - call_h),
        Pos2::new(r.right() - gap, r.bottom() - gap),
    );
    p.rect_filled(call, CornerRadius::same(7), extra::blend(pal.panel, pal.accent, 0.25));
}
