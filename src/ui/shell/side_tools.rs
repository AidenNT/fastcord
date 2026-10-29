//! Panel lateral de las herramientas de la píldora superior derecha: buscar
//! mensajes, mensajes fijados y lista de hilos. Ocupa el lugar de la lista de
//! miembros / perfil, y el estado vive en `App::side_tool`
//! (ver `lib::state::SideToolPanel`).

use egui::{CornerRadius, Frame, Margin, ScrollArea, Sense, Stroke, Vec2};

use crate::lib::state::{App, SideTool};
use crate::ui::shell::extra;
use crate::ui::theme::{self, Icon, Palette};

const PANEL_W: f32 = 340.0;
const ROW_RADIUS: u8 = 12;

/// Lo que se eligió en el panel durante el frame; se aplica al terminar de
/// dibujar, cuando ya no hay nada de `app` prestado.
enum Pick {
    Message { channel_id: String, message_id: String },
    Thread { id: String, name: String, owner_id: Option<String> },
}

/// Dibuja el panel. Hay que llamarlo solo si `app.side_tool.is_some()`.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    // Si se cambió de canal, se vuelve a pedir lo que corresponde.
    app.sync_side_tool();
    let Some(panel) = app.side_tool.as_ref() else { return };
    let palette = app.palette;
    let tool = panel.tool;
    let in_server = panel.guild_id.is_some();
    let loading = panel.loading;
    let failed = panel.failed;
    let total = panel.total;
    let submitted = panel.submitted.clone();
    let mut query = panel.query.clone();
    let focus_search = panel.focus_search;

    let (icon, title) = match tool {
        SideTool::Search => (Icon::Search, "Buscar"),
        SideTool::Pins => (Icon::Pin, "Mensajes fijados"),
        SideTool::Threads => (Icon::ListPlus, "Hilos"),
    };
    let hint = if in_server {
        "Buscar en el servidor"
    } else {
        "Buscar en esta conversación"
    };

    let mut close = false;
    let mut submit = false;
    let mut pick: Option<Pick> = None;

    egui::Panel::right("side_tool_panel")
        .exact_size(PANEL_W)
        .resizable(false)
        .frame(theme::card_frame(
            &palette,
            Frame::new().fill(palette.window).inner_margin(Margin::same(0)),
            4,
            5,
            10,
        ))
        .show(ui, |ui| {
            // Línea que separa el panel del chat (la interfaz nueva ya lo
            // separa con el hueco entre tarjetas).
            let rect = ui.max_rect();
            if !theme::is_modern() {
                ui.painter()
                    .vline(rect.left(), rect.y_range(), Stroke::new(1.0, palette.outline));
            }

            Frame::new().inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::icon(ui, icon, 16.0, palette.dim);
                    theme::text(ui, title, theme::semibold(14.5), palette.text);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, Icon::X, 14.0, palette.dim, palette.text, "Cerrar").clicked() {
                            close = true;
                        }
                    });
                });
            });

            if tool == SideTool::Search {
                Frame::new().inner_margin(Margin::symmetric(14, 0)).show(ui, |ui| {
                    let edit = egui::TextEdit::singleline(&mut query)
                        .hint_text(hint)
                        .font(theme::regular(13.0))
                        .desired_width(f32::INFINITY);
                    let response = ui.add(edit);
                    if focus_search {
                        response.request_focus();
                    }
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                });
                ui.add_space(8.0);
            }

            ui.painter().hline(
                ui.max_rect().x_range(),
                ui.cursor().top(),
                Stroke::new(1.0, palette.outline),
            );

            let Some(panel) = app.side_tool.as_ref() else { return };
            Frame::new().inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                let empty = match tool {
                    SideTool::Threads => panel.threads.is_empty(),
                    _ => panel.hits.is_empty(),
                };
                if loading {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                    });
                    return;
                }
                if failed {
                    note(ui, &palette, "No se pudo cargar. Inténtalo de nuevo en un momento.");
                    return;
                }
                if empty {
                    let text = match tool {
                        SideTool::Search if submitted.is_empty() => {
                            "Escribe algo y presiona Enter para buscar.".to_string()
                        }
                        SideTool::Search => format!("Sin resultados para «{submitted}»."),
                        SideTool::Pins => "Este chat no tiene mensajes fijados.".to_string(),
                        SideTool::Threads => "No hay hilos activos en este canal.".to_string(),
                    };
                    note(ui, &palette, &text);
                    return;
                }

                if tool == SideTool::Search {
                    let label = if total == 1 {
                        "1 resultado".to_string()
                    } else {
                        format!("{total} resultados")
                    };
                    theme::text(ui, label, theme::semibold(12.0), palette.dim);
                    ui.add_space(6.0);
                }

                ScrollArea::vertical()
                    .id_salt("side_tool_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let width = (ui.available_width() - 6.0).max(0.0);
                        match tool {
                            SideTool::Threads => {
                                for thread in &panel.threads {
                                    if thread_row(ui, &palette, &thread.name, thread.replies, width) {
                                        pick = Some(Pick::Thread {
                                            id: thread.id.clone(),
                                            name: thread.name.clone(),
                                            owner_id: thread.owner_id.clone(),
                                        });
                                    }
                                    ui.add_space(6.0);
                                }
                            }
                            _ => {
                                for hit in &panel.hits {
                                    if message_row(ui, &palette, hit, width) {
                                        pick = Some(Pick::Message {
                                            channel_id: hit.channel_id.clone(),
                                            message_id: hit.message_id.clone(),
                                        });
                                    }
                                    ui.add_space(6.0);
                                }
                            }
                        }
                    });
            });
        });

    if let Some(panel) = app.side_tool.as_mut() {
        panel.query = query;
        panel.focus_search = false;
    }
    if close {
        app.close_side_tool();
        return;
    }
    if submit {
        app.submit_side_search();
    }
    match pick {
        Some(Pick::Message { channel_id, message_id }) => app.open_side_hit(&channel_id, &message_id),
        Some(Pick::Thread { id, name, owner_id }) => {
            app.close_side_tool();
            app.open_thread_panel(&id, &name, owner_id);
        }
        None => {}
    }
}

fn note(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::regular(13.0)).color(palette.dim)).wrap());
    });
}

/// Una fila con un mensaje: avatar, autor, hora y hasta dos líneas de texto.
/// Devuelve `true` si se hizo click.
fn message_row(ui: &mut egui::Ui, palette: &Palette, hit: &crate::lib::state::SideHit, width: f32) -> bool {
    let text_x = 46.0;
    let text_w = (width - text_x - 10.0).max(20.0);
    let mut job = egui::text::LayoutJob::simple(hit.content.clone(), theme::regular(13.0), palette.secondary, text_w);
    job.wrap.max_rows = 3;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    let galley = ui.painter().layout_job(job);

    let height = 32.0 + galley.size().y + 8.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let fill = if resp.hovered() {
        palette.surface_hover
    } else {
        palette.surface
    };
    ui.painter().rect_filled(rect, CornerRadius::same(ROW_RADIUS), fill);

    let initial = hit
        .author
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    extra::avatar(
        ui,
        rect.left_top() + Vec2::new(24.0, 24.0),
        14.0,
        hit.avatar_url.as_deref(),
        palette.accent,
        &initial,
        palette,
    );

    let line = |y: f32, h: f32, w: f32| egui::Rect::from_min_size(rect.left_top() + Vec2::new(text_x, y), Vec2::new(w, h));
    let time_w = 96.0_f32.min(text_w * 0.5);
    crate::ui::home::clipped_line(
        ui,
        line(9.0, 18.0, text_w - time_w),
        &hit.author,
        theme::semibold(13.5),
        palette.text,
    );
    let time_rect = line(10.0, 16.0, time_w);
    ui.painter().with_clip_rect(time_rect).text(
        time_rect.right_top(),
        egui::Align2::RIGHT_TOP,
        &hit.time,
        theme::regular(11.5),
        palette.dim,
    );
    ui.painter()
        .galley(rect.left_top() + Vec2::new(text_x, 30.0), galley, palette.secondary);

    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Una fila de la lista de hilos: nombre y cantidad de respuestas.
fn thread_row(ui: &mut egui::Ui, palette: &Palette, name: &str, replies: u32, width: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 52.0), Sense::click());
    let fill = if resp.hovered() {
        palette.surface_hover
    } else {
        palette.surface
    };
    ui.painter().rect_filled(rect, CornerRadius::same(ROW_RADIUS), fill);
    theme::paint_icon(
        ui,
        Icon::ListPlus,
        egui::Rect::from_center_size(rect.left_top() + Vec2::new(22.0, 26.0), Vec2::splat(16.0)),
        15.0,
        palette.dim,
    );
    let text_w = (width - 44.0 - 10.0).max(20.0);
    let line = |y: f32, h: f32| egui::Rect::from_min_size(rect.left_top() + Vec2::new(44.0, y), Vec2::new(text_w, h));
    crate::ui::home::clipped_line(ui, line(9.0, 18.0), name, theme::semibold(13.5), palette.text);
    let summary = match replies {
        0 => "Sin respuestas".to_string(),
        1 => "1 respuesta".to_string(),
        n => format!("{n} respuestas"),
    };
    crate::ui::home::clipped_line(ui, line(28.0, 16.0), &summary, theme::regular(12.0), palette.dim);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}
