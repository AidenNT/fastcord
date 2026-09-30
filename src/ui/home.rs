use egui::{CornerRadius, Frame, Margin, ScrollArea, Stroke, Vec2};

use crate::lib::data::Status;
use crate::lib::state::App;
use crate::ui::extra;
use crate::ui::nav;
use crate::ui::theme::{self, Icon, Palette};

const TABS: [&str; 4] = ["En línea", "Todos", "Pendiente", "Sugerencias"];

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    // Rail de servidores + panel de amigos/DMs + barra de usuario
    // combinada; ver `ui::nav`.
    nav::show(app, ui);
    active_now_panel(app, ui);
    central_panel(app, ui);
}

// ---------------------------------------------------------------------
// Panel derecho: "Activo ahora"
// ---------------------------------------------------------------------
fn active_now_panel(app: &App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let cards = app.active_now_cards();
    egui::Panel::right("active_now")
        .exact_size(300.0)
        .resizable(false)
        .frame(Frame::new().fill(palette.panel).inner_margin(Margin::same(16)))
        .show(ui, |ui| {
            theme::text(ui, "Activo ahora", theme::bold(16.0), palette.text);
            ui.add_space(12.0);
            if cards.is_empty() {
                theme::text(ui, "Todo tranquilo por ahora", theme::semibold(13.0), palette.text);
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "Cuando un amigo juegue o escuche algo, o vos lo hagas, aparece acá.",
                        )
                        .font(theme::regular(12.0))
                        .color(palette.dim),
                    )
                    .wrap(),
                );
                return;
            }
            ScrollArea::vertical().show(ui, |ui| {
                for card in &cards {
                    activity_card(ui, &palette, card);
                    ui.add_space(8.0);
                }
            });
        });
}

/// Milisegundos Unix de ahora.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `3:07` / `1:02:03`.
fn clock(total_secs: u64) -> String {
    let (h, m, s) = (total_secs / 3600, (total_secs / 60) % 60, total_secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Tiempo transcurrido en texto corto: "hace un momento", "2 min", "1 h 5 min".
fn elapsed_label(start_ms: u64) -> String {
    let secs = now_ms().saturating_sub(start_ms) / 1000;
    match secs {
        0..=59 => "hace un momento".to_string(),
        60..=3599 => format!("{} min", secs / 60),
        _ => {
            let (h, m) = (secs / 3600, (secs / 60) % 60);
            if m == 0 { format!("{h} h") } else { format!("{h} h {m} min") }
        }
    }
}

/// Encabezado de la tarjeta según el tipo de actividad.
fn activity_heading(activity: &crate::discord::models::PresenceActivity, is_me: bool) -> String {
    let verb = match activity.kind {
        0 => "Jugando",
        1 => "Transmitiendo",
        2 => "Escuchando",
        3 => "Viendo",
        5 => "Compitiendo en",
        _ => "",
    };
    let _ = is_me;
    if verb.is_empty() { activity.name.clone() } else { format!("{verb} {}", activity.name) }
}

/// Dibuja una línea de texto recortada a `rect` (sin `…`, solo clip), en una
/// sola línea aunque el texto original tenga saltos.
fn clipped_line(ui: &egui::Ui, rect: egui::Rect, text: &str, font: egui::FontId, color: egui::Color32) {
    let flat = crate::discord::models::one_line(text);
    ui.painter()
        .with_clip_rect(rect)
        .text(rect.left_top(), egui::Align2::LEFT_TOP, flat, font, color);
}

fn activity_card(ui: &mut egui::Ui, palette: &Palette, card: &crate::lib::data::ActiveNowCard) {
    let activity = &card.activity;
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(theme::RADIUS + 2))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            // Quién: avatar chico + nombre + qué hace.
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::hover());
                let initial = card
                    .name
                    .chars()
                    .find(|c| c.is_alphanumeric())
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_else(|| "?".to_string());
                extra::avatar(ui, rect.center(), 14.0, card.avatar_url.as_deref(), card.avatar_color, &initial, palette);
                extra::status_dot(ui, rect.center(), 14.0, extra::status_color(card.status, palette), palette.surface);
                let text_w = (ui.available_width() - 4.0).max(0.0);
                let (text_rect, _) = ui.allocate_exact_size(Vec2::new(text_w, 30.0), egui::Sense::hover());
                let name = if card.is_me { format!("{} (vos)", card.name) } else { card.name.clone() };
                clipped_line(
                    ui,
                    egui::Rect::from_min_size(text_rect.min, Vec2::new(text_w, 15.0)),
                    &name,
                    theme::semibold(13.0),
                    palette.text,
                );
                clipped_line(
                    ui,
                    egui::Rect::from_min_size(text_rect.min + Vec2::new(0.0, 16.0), Vec2::new(text_w, 14.0)),
                    &activity_heading(activity, card.is_me),
                    theme::regular(11.0),
                    palette.dim,
                );
            });
            ui.add_space(8.0);

            // Qué: carátula/ícono + título, subtítulo y tiempo.
            ui.horizontal(|ui| {
                let art = 56.0;
                let (art_rect, _) = ui.allocate_exact_size(Vec2::splat(art), egui::Sense::hover());
                let mut drew_image = false;
                if let Some(url) = activity.image_url() {
                    let image = egui::Image::new(crate::ui::anim::plain(&url))
                        .corner_radius(CornerRadius::same(6))
                        .fit_to_exact_size(Vec2::splat(art))
                        .show_loading_spinner(false);
                    if let Ok(egui::load::TexturePoll::Ready { .. }) = image.load_for_size(ui.ctx(), Vec2::splat(art)) {
                        ui.put(art_rect, image);
                        drew_image = true;
                    }
                }
                if !drew_image {
                    // Sin imagen (o todavía cargando): cuadrado con la inicial.
                    ui.painter().rect_filled(
                        art_rect,
                        CornerRadius::same(6),
                        extra::blend(palette.surface_hover, palette.accent, 0.15),
                    );
                    let initial = activity
                        .name
                        .chars()
                        .find(|c| c.is_alphanumeric())
                        .map(|c| c.to_uppercase().to_string())
                        .unwrap_or_else(|| "?".to_string());
                    ui.painter().text(
                        art_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        initial,
                        theme::bold(22.0),
                        palette.text,
                    );
                }

                let text_w = (ui.available_width() - 2.0).max(0.0);
                let (col, _) = ui.allocate_exact_size(Vec2::new(text_w, art), egui::Sense::hover());
                let line = |y: f32| egui::Rect::from_min_size(col.min + Vec2::new(0.0, y), Vec2::new(text_w, 15.0));

                // Título: `details` (canción / lo que hace); si no hay, el nombre.
                let title = activity.details.as_deref().filter(|d| !d.trim().is_empty()).unwrap_or(&activity.name);
                clipped_line(ui, line(2.0), title, theme::semibold(12.5), palette.text);

                // Subtítulo: `state` (artistas / detalle). En Spotify los
                // artistas vienen separados por `;`.
                let mut y = 19.0;
                if let Some(state) = activity.state.as_deref().filter(|s| !s.trim().is_empty()) {
                    let state = if activity.is_spotify() { state.replace(';', ",") } else { state.to_string() };
                    clipped_line(ui, line(y), &state, theme::regular(12.0), palette.secondary);
                    y += 17.0;
                }

                // Tiempo: progreso en canciones (inicio y fin), o tiempo
                // jugado en el resto.
                match (activity.start_ms(), activity.end_ms()) {
                    (Some(start), Some(end)) if end > start => {
                        let total = (end - start) / 1000;
                        let played = (now_ms().saturating_sub(start) / 1000).min(total);
                        let bar = egui::Rect::from_min_size(col.min + Vec2::new(0.0, y + 4.0), Vec2::new(text_w, 4.0));
                        ui.painter().rect_filled(bar, 2.0, palette.outline);
                        let done = total.max(1) as f32;
                        let filled = egui::Rect::from_min_size(
                            bar.min,
                            Vec2::new(bar.width() * (played as f32 / done), bar.height()),
                        );
                        ui.painter().rect_filled(filled, 2.0, palette.accent);
                        clipped_line(
                            ui,
                            egui::Rect::from_min_size(col.min + Vec2::new(0.0, y + 10.0), Vec2::new(text_w, 12.0)),
                            &format!("{} / {}", clock(played), clock(total)),
                            theme::regular(10.5),
                            palette.dim,
                        );
                    }
                    (Some(start), _) => {
                        clipped_line(ui, line(y), &elapsed_label(start), theme::regular(11.0), palette.dim);
                    }
                    _ => {}
                }
            });
        });
}

// ---------------------------------------------------------------------
// Panel central: tabs + búsqueda + lista de amigos conectados
// ---------------------------------------------------------------------
fn central_panel(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    egui::CentralPanel::default()
        .frame(Frame::new().fill(palette.panel).inner_margin(Margin::same(0)))
        .show(ui, |ui| {
            top_bar(ui, &palette, "Amigos", Icon::Users);

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                for (i, tab) in TABS.iter().enumerate() {
                    let selected = app.friends_tab == i;
                    let galley = ui.painter().layout_no_wrap(
                        tab.to_string(),
                        theme::medium(13.5),
                        palette.text,
                    );
                    let size = Vec2::new(galley.size().x + 24.0, 30.0);
                    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                    let bg = if selected {
                        palette.surface_hover
                    } else if resp.hovered() {
                        palette.surface
                    } else {
                        palette.panel
                    };
                    ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS_SMALL + 2), bg);
                    let color = if selected { palette.text } else { palette.secondary };
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        *tab,
                        theme::medium(13.5),
                        color,
                    );
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        app.friends_tab = i;
                    }
                    ui.add_space(6.0);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(16.0);
                    theme::pill_button(ui, &palette, "Añadir amigo", true);
                });
            });

            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.add_sized(
                    [ui.available_width() - 16.0, 32.0],
                    egui::TextEdit::singleline(&mut app.friends_search).hint_text("Buscar"),
                );
            });

            ui.add_space(14.0);

            let filtered: Vec<&crate::lib::data::Friend> = app
                .friends
                .iter()
                .filter(|f| {
                    if !f.is_friend {
                        return false;
                    }
                    let matches_search = app.friends_search.is_empty()
                        || f.name
                            .to_lowercase()
                            .contains(&app.friends_search.to_lowercase());
                    let matches_tab = match app.friends_tab {
                        0 => f.status != Status::Offline,
                        _ => true,
                    };
                    matches_search && matches_tab
                })
                .collect();

            ui.horizontal(|ui| {
                ui.add_space(16.0);
                theme::text(
                    ui,
                    format!("Conectado — {}", filtered.len()),
                    theme::semibold(12.5),
                    palette.dim,
                );
            });
            ui.add_space(6.0);

            if app.friends_tab >= 2 {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    theme::text(ui, "No hay elementos en esta vista (demo)", theme::regular(13.0), palette.dim);
                });
                return;
            }

            let mut clicked_index = None;
            ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(4.0);
                for (idx, friend) in app.friends.iter().enumerate() {
                    if !friend.is_friend || !filtered.iter().any(|f| f.name == friend.name) {
                        continue;
                    }
                    if friend_row_full(ui, &palette, friend) {
                        clicked_index = Some(idx);
                    }
                }
                ui.add_space(12.0);
            });

            if let Some(idx) = clicked_index {
                // imprimir en consola el índice del amigo clicado
                println!("Clicked on friend index: {}", idx);
                app.open_dm(idx);
            }
        });
}

/// Barra de arriba compartida por Home/Dm/Server: flechas + título + íconos.
pub fn top_bar(ui: &mut egui::Ui, palette: &Palette, title: &str, title_icon: Icon) {
    Frame::new()
        .stroke(Stroke::new(1.0, palette.outline))
        .inner_margin(Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::icon_button(ui, Icon::ArrowLeft, 14.0, palette.dim, palette.text, "Atrás");
                theme::icon_button(ui, Icon::ArrowRight, 14.0, palette.dim, palette.text, "Adelante");
                ui.add_space(6.0);
                theme::icon(ui, title_icon, 16.0, palette.text);
                ui.add_space(4.0);
                theme::text(ui, title, theme::semibold(14.5), palette.text);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::icon_button(ui, Icon::Settings, 15.0, palette.dim, palette.text, "Herramientas");
                    theme::icon_button(ui, Icon::Info, 15.0, palette.dim, palette.text, "Ayuda");
                    // TODO: no hay ícono de "inbox/bandeja" en el set de Lucide
                    // que trae tu theme; Bookmark como placeholder.
                    theme::icon_button(ui, Icon::Bookmark, 15.0, palette.dim, palette.text, "Bandeja");
                });
            });
        });
}

/// Devuelve `true` si se hizo click en la fila (para abrir el DM).
fn friend_row_full(ui: &mut egui::Ui, palette: &Palette, friend: &crate::lib::data::Friend) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let desired = Vec2::new(ui.available_width() - 32.0, 56.0);
        let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
        if resp.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(theme::RADIUS + 2), palette.surface);
        }
        let avatar_center = rect.left_center() + Vec2::new(20.0, 0.0);
        extra::avatar(ui, avatar_center, 20.0, friend.avatar_url.as_deref(), friend.avatar_color, &friend.initial(), palette);
        extra::status_dot(ui, avatar_center, 20.0, extra::status_color(friend.status, palette), palette.panel);

        let text_x = 52.0;
        crate::ui::emoji::paint_line_top(
            ui,
            rect.left_top() + Vec2::new(text_x, 14.0),
            &friend.name,
            theme::medium(14.5),
            palette.text,
            (rect.width() - text_x - 8.0).max(0.0),
        );
        let subtitle = friend
            .subtitle
            .clone()
            .unwrap_or_else(|| friend.status.label().to_string());
        // Una línea y recortada al ancho de la fila (con hover, dejando
        // lugar a los íconos de la derecha).
        let max_chars = (((rect.width() - text_x - 90.0) / 6.0).max(8.0)) as usize;
        let subtitle = clip_line(&subtitle, max_chars);
        ui.painter().text(
            rect.left_top() + Vec2::new(text_x, 33.0),
            egui::Align2::LEFT_TOP,
            &subtitle,
            theme::regular(12.0),
            palette.dim,
        );

        if resp.hovered() {
            theme::paint_icon(
                ui,
                Icon::SquarePen,
                egui::Rect::from_center_size(rect.right_center() - Vec2::new(50.0, 0.0), Vec2::splat(16.0)),
                15.0,
                palette.dim,
            );
            theme::paint_icon(
                ui,
                Icon::Ellipsis,
                egui::Rect::from_center_size(rect.right_center() - Vec2::new(16.0, 0.0), Vec2::splat(16.0)),
                16.0,
                palette.dim,
            );
        }
        clicked = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0 + 52.0);
        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width() - 32.0, 1.0),
            egui::Sense::hover(),
        );
        ui.painter().rect_filled(rect, 0.0, palette.outline);
    });
    clicked
}

/// Texto en una sola línea, cortado con `…` si pasa de `max_chars`.
fn clip_line(text: &str, max_chars: usize) -> String {
    let flat = crate::discord::models::one_line(text);
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let mut out: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}
