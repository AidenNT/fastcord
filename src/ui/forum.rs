//! Vista de un canal de foro: en vez de una charla, una lista de posts
//! (tarjetas con título, vista previa del mensaje inicial, etiquetas,
//! respuestas y última actividad), con búsqueda, orden y el formulario de
//! "Nueva publicación". Abrir un post lo muestra como un canal normal (ver
//! `App::open_forum_post`).

use egui::{Color32, CornerRadius, Frame, Margin, RichText, ScrollArea, Sense, Stroke, Vec2};

use crate::discord::models::ForumTag;
use crate::lib::data::{ForumPost, ForumState};
use crate::ui::theme::{self, Icon, Palette};

/// Lo que el usuario pidió en este frame (a lo sumo una cosa).
pub enum ForumEvent {
    None,
    /// Abrir un post.
    Open { id: String, title: String },
    /// Pedir la página siguiente de posts.
    LoadMore,
    /// Cambió el orden: hay que volver a pedir desde el principio.
    Resort,
    /// Publicar el post que se escribió en el formulario.
    Create,
}

/// Lado de la miniatura de una tarjeta.
const THUMB: f32 = 88.0;
/// Pasado este tiempo sin actividad, un post va bajo "Publicaciones antiguas".
const OLD_AFTER_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(PartialEq, Clone, Copy)]
enum Section {
    Start,
    Pinned,
    Recent,
    Old,
}

pub fn show(ui: &mut egui::Ui, palette: &Palette, forum: &mut ForumState, now_ms: i64) -> ForumEvent {
    let mut event = ForumEvent::None;

    ScrollArea::vertical()
        .id_salt("forum_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            Frame::new().inner_margin(Margin::symmetric(20, 16)).show(ui, |ui| {
                // Búsqueda + botón de nueva publicación.
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 180.0).max(120.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut forum.search)
                            .hint_text("Busca o crea una publicación…")
                            .font(theme::regular(14.0))
                            .desired_width(width),
                    );
                    if theme::pill_button(ui, palette, "Nueva publicación", true).clicked() {
                        forum.composing = !forum.composing;
                    }
                });

                // Orden.
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let before = forum.by_creation;
                    let selected = if before { "Fecha de creación" } else { "Actividad reciente" };
                    egui::ComboBox::from_id_salt("forum_sort")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut forum.by_creation, false, "Actividad reciente");
                            ui.selectable_value(&mut forum.by_creation, true, "Fecha de creación");
                        });
                    if forum.by_creation != before {
                        event = ForumEvent::Resort;
                    }
                });

                if forum.composing {
                    ui.add_space(10.0);
                    composer(ui, palette, forum, &mut event);
                }

                ui.add_space(14.0);

                let query = forum.search.trim().to_lowercase();
                let visible: Vec<usize> = forum
                    .posts
                    .iter()
                    .enumerate()
                    .filter(|(_, post)| {
                        query.is_empty()
                            || post.title.to_lowercase().contains(&query)
                            || post.preview.to_lowercase().contains(&query)
                    })
                    .map(|(index, _)| index)
                    .collect();

                if visible.is_empty() {
                    if forum.loading {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            theme::text(ui, "Cargando publicaciones…", theme::regular(13.0), palette.dim);
                        });
                    } else if forum.loaded {
                        let message = if query.is_empty() {
                            "Todavía no hay publicaciones en este foro."
                        } else {
                            "No hay publicaciones que coincidan con la búsqueda."
                        };
                        theme::text(ui, message, theme::regular(13.0), palette.dim);
                    }
                }

                let mut section = Section::Start;
                for &index in &visible {
                    let post = &forum.posts[index];
                    let this = if post.pinned {
                        Section::Pinned
                    } else if now_ms - post.last_activity_ms > OLD_AFTER_MS {
                        Section::Old
                    } else {
                        Section::Recent
                    };
                    if this != section {
                        let header = match this {
                            Section::Pinned => Some("Publicaciones fijadas"),
                            Section::Old => Some("Publicaciones antiguas"),
                            _ => None,
                        };
                        if let Some(header) = header {
                            ui.add_space(6.0);
                            theme::text(ui, header, theme::regular(13.0), palette.secondary);
                            ui.add_space(8.0);
                        }
                        section = this;
                    }
                    if post_card(ui, palette, &forum.tags, post, now_ms) {
                        event = ForumEvent::Open { id: post.id.clone(), title: post.title.clone() };
                    }
                    ui.add_space(10.0);
                }

                if forum.has_more && !visible.is_empty() {
                    ui.add_space(4.0);
                    ui.vertical_centered(|ui| {
                        if forum.loading {
                            theme::text(ui, "Cargando…", theme::regular(13.0), palette.dim);
                        } else if theme::pill_button(ui, palette, "Cargar más publicaciones", false)
                            .clicked()
                        {
                            event = ForumEvent::LoadMore;
                        }
                    });
                }
            });
        });

    event
}

/// Formulario de "Nueva publicación": título, mensaje y etiquetas.
fn composer(ui: &mut egui::Ui, palette: &Palette, forum: &mut ForumState, event: &mut ForumEvent) {
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS + 2))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::text(ui, "Nueva publicación", theme::semibold(15.0), palette.text);
            ui.add_space(8.0);
            ui.add(
                egui::TextEdit::singleline(&mut forum.draft_title)
                    .hint_text("Título de la publicación")
                    .char_limit(100)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::multiline(&mut forum.draft_body)
                    .hint_text("Escribí tu mensaje…")
                    .desired_rows(4)
                    .desired_width(f32::INFINITY),
            );
            if !forum.tags.is_empty() {
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    for tag in &forum.tags {
                        let selected = forum.draft_tags.contains(&tag.id);
                        let label = match &tag.emoji_name {
                            Some(emoji) if !emoji.is_empty() => format!("{emoji} {}", tag.name),
                            _ => tag.name.clone(),
                        };
                        if ui.selectable_label(selected, label).clicked() {
                            if selected {
                                forum.draft_tags.retain(|id| id != &tag.id);
                            } else if forum.draft_tags.len() < 5 {
                                // Discord permite hasta 5 etiquetas por post.
                                forum.draft_tags.push(tag.id.clone());
                            }
                        }
                    }
                });
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let creating = forum.creating;
                let label = if creating { "Publicando…" } else { "Publicar" };
                ui.add_enabled_ui(!creating, |ui| {
                    if theme::pill_button(ui, palette, label, true).clicked() {
                        *event = ForumEvent::Create;
                    }
                });
                if theme::pill_button(ui, palette, "Cancelar", false).clicked() {
                    forum.composing = false;
                }
            });
        });
}

/// Una tarjeta de post. Devuelve `true` si se hizo clic en ella.
fn post_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    tags: &[ForumTag],
    post: &ForumPost,
    now_ms: i64,
) -> bool {
    let radius = CornerRadius::same(theme::RADIUS + 2);
    let card = Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(radius)
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let reserved = if post.thumbnail.is_some() { THUMB + 12.0 } else { 0.0 };
                let text_width = (ui.available_width() - reserved).max(120.0);
                ui.vertical(|ui| {
                    ui.set_width(text_width);
                    theme::text(ui, post.title.clone(), theme::semibold(17.0), palette.text);

                    if !post.author.is_empty() || !post.preview.is_empty() {
                        ui.add_space(2.0);
                        let mut job = egui::text::LayoutJob::default();
                        if !post.author.is_empty() {
                            job.append(
                                &post.author,
                                0.0,
                                egui::TextFormat {
                                    font_id: theme::semibold(13.0),
                                    color: post.author_color.unwrap_or(palette.accent),
                                    ..Default::default()
                                },
                            );
                            job.append(
                                ": ",
                                0.0,
                                egui::TextFormat {
                                    font_id: theme::regular(13.0),
                                    color: palette.secondary,
                                    ..Default::default()
                                },
                            );
                        }
                        job.append(
                            &post.preview,
                            0.0,
                            egui::TextFormat {
                                font_id: theme::regular(13.0),
                                color: palette.secondary,
                                ..Default::default()
                            },
                        );
                        ui.add(egui::Label::new(job).truncate());
                    }

                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if post.pinned {
                            theme::icon(ui, Icon::Pin, 13.0, palette.dim);
                        }
                        for tag_id in &post.tag_ids {
                            if let Some(tag) = tags.iter().find(|t| &t.id == tag_id) {
                                tag_pill(ui, palette, tag);
                            }
                        }
                        if let Some((emoji, count)) = &post.reaction {
                            theme::text(
                                ui,
                                format!("{emoji} {count}"),
                                theme::medium(13.0),
                                palette.secondary,
                            );
                        }
                        theme::text(
                            ui,
                            format!("💬 {}", post.replies),
                            theme::medium(13.0),
                            palette.secondary,
                        );
                        theme::text(ui, ago(now_ms, post.last_activity_ms), theme::regular(13.0), palette.dim);
                    });
                });

                if let Some(url) = &post.thumbnail {
                    ui.add_space(12.0);
                    ui.add(
                        egui::Image::new(crate::ui::anim::plain(url))
                            .fit_to_exact_size(Vec2::splat(THUMB))
                            .corner_radius(CornerRadius::same(theme::RADIUS))
                            .show_loading_spinner(false),
                    );
                }
            });
        });

    // Toda la tarjeta es clickeable.
    let rect = card.response.rect;
    let response = ui.interact(rect, ui.id().with(("forum_post", &post.id)), Sense::click());
    if response.hovered() {
        let tint = if palette.dark { Color32::from_white_alpha(6) } else { Color32::from_black_alpha(8) };
        ui.painter().rect_filled(rect, radius, tint);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Etiqueta de un post ("NUEVO", "Resuelto"...).
fn tag_pill(ui: &mut egui::Ui, palette: &Palette, tag: &ForumTag) {
    let label = match &tag.emoji_name {
        Some(emoji) if !emoji.is_empty() => format!("{emoji} {}", tag.name),
        _ => tag.name.clone(),
    };
    Frame::new()
        .fill(palette.accent.gamma_multiply(0.22))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.add(egui::Label::new(
                RichText::new(label).font(theme::semibold(11.5)).color(palette.accent),
            ));
        });
}

/// "hace 5 min", "hace 3 h", "hace 8 d", "hace >30 días".
fn ago(now_ms: i64, then_ms: i64) -> String {
    if then_ms <= 0 {
        return String::new();
    }
    let seconds = ((now_ms - then_ms) / 1000).max(0);
    if seconds < 60 {
        "ahora".to_string()
    } else if seconds < 60 * 60 {
        format!("hace {} min", seconds / 60)
    } else if seconds < 24 * 60 * 60 {
        format!("hace {} h", seconds / 3600)
    } else if seconds < 30 * 24 * 60 * 60 {
        format!("hace {} d", seconds / 86_400)
    } else {
        "hace >30 días".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn relative_time_matches_the_official_client_format() {
        let now = 1_000_000_000_000;
        assert_eq!(ago(now, now - 5_000), "ahora");
        assert_eq!(ago(now, now - 5 * 60_000), "hace 5 min");
        assert_eq!(ago(now, now - 3 * 3_600_000), "hace 3 h");
        assert_eq!(ago(now, now - 8 * 86_400_000), "hace 8 d");
        assert_eq!(ago(now, now - 45 * 86_400_000), "hace >30 días");
        assert_eq!(ago(now, 0), "");
    }
}
