#![allow(dead_code)]

use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, UiBuilder, Vec2};

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
    // "Usar nueva interfaz": Inicio en tarjetas flotantes (ver `modern`).
    if theme::is_modern() {
        modern_central(app, ui);
        return;
    }
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
        .frame(theme::card_frame(
            &palette,
            Frame::new().fill(palette.panel).inner_margin(Margin::same(16)),
            14,
            5,
            10,
        ))
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
pub(crate) fn clipped_line(ui: &egui::Ui, rect: egui::Rect, text: &str, font: egui::FontId, color: egui::Color32) {
    let flat = crate::discord::models::one_line(text);
    ui.painter()
        .with_clip_rect(rect)
        .text(rect.left_top(), egui::Align2::LEFT_TOP, flat, font, color);
}

fn activity_card(ui: &mut egui::Ui, palette: &Palette, card: &crate::lib::data::ActiveNowCard) {
    let activity = &card.activity;
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(theme::radius() + 2))
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
        .frame(theme::card_frame(
            &palette,
            Frame::new().fill(palette.panel).inner_margin(Margin::same(0)),
            6,
            5,
            5,
        ))
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
                    ui.painter().rect_filled(rect, CornerRadius::same(theme::radius_small() + 2), bg);
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
                    if theme::pill_button(ui, &palette, "Añadir amigo", true).clicked() {
                        app.friends_tab = ADD_FRIEND_TAB;
                    }
                });
            });

            // "Añadir amigo" (clásica): reemplaza buscador y lista.
            if app.friends_tab == ADD_FRIEND_TAB {
                ui.add_space(20.0);
                let area = ui.available_rect_before_wrap().shrink2(Vec2::new(16.0, 0.0));
                let send = in_rect(ui, area, |ui| {
                    theme::text(ui, "Añadir amigo", theme::bold(20.0), palette.text);
                    ui.add_space(8.0);
                    add_friend_form(ui, &palette, app)
                });
                if let Some(username) = send {
                    app.send_friend_request(username);
                }
                return;
            }

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

            // "Pendiente" (clásica): solicitudes entrantes y salientes.
            if app.friends_tab == 2 {
                let area = ui.available_rect_before_wrap().shrink2(Vec2::new(16.0, 0.0));
                let click = in_rect(ui, area, |ui| pending_list(ui, &palette, &*app, &app.friends_search));
                if let Some(click) = click {
                    apply_pending_click(app, click);
                }
                return;
            }

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

            if app.friends_tab >= 3 {
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
                app.open_dm(idx);
            }
        });
}

// =====================================================================
// Inicio de la interfaz nueva ("Usar nueva interfaz")
// =====================================================================
//
// Reemplaza al layout clásico (lista de amigos + panel "Activo ahora") por
// un tablero de tarjetas que flotan sobre el fondo, con datos reales:
//
//   ┌ Amigos · N ────────┐ ┌ Activo ahora ───────────────┐
//   │ filas con estado   │ │ tarjetas de juegos / música │
//   │ y actividad        │ │ de tus amigos (en fila)     │
//   │                    │ ├ Canales recientes ──────────┤
//   │                    │ │ lista por server + mensajes │
//   │                    │ │ desde el último leído       │
//   └────────────────────┘ └─────────────────────────────┘
//
// Las notificaciones ya no son una sección del Inicio: viven en la campana
// de la barra superior (`ui::inbox`).
//
// Las secciones se dibujan sobre rectángulos calculados a mano (en vez de
// paneles anidados) para que el reparto de alto sea estable y cada lista
// tenga su propio scroll. Todo lo que cambia el estado de la app (abrir un
// DM, un canal, pedir mensajes) se anota en `HomeAction` y se aplica al
// final, así las secciones solo leen `&App`.

/// Qué hacer cuando se hace click en algo del Inicio.
#[derive(Clone)]
enum HomeAction {
    /// Abre un amigo de `app.friends` (por índice).
    OpenFriend(usize),
    /// Abre un canal: (server, categoría, canal) y, si se sabe, el mensaje al
    /// que hay que ir (`?around=<id>`).
    OpenChannelAt(usize, usize, usize, Option<String>),
    /// La vista previa del canal elegido todavía no tiene mensajes: pedirlos
    /// alrededor del último leído.
    LoadPreview(usize, usize, usize),
    /// Seguir bajando en la vista previa (`?after=<id>`): id del canal.
    LoadNewer(String),
    /// Abre un server de `app.servers` (por índice).
    OpenServer(usize),
}

const GAP: f32 = 14.0;
const HEADING_H: f32 = 44.0;
const ROW_RADIUS: u8 = 16;

/// Pestañas de la fila de píldoras de arriba (interfaz nueva). El índice es
/// `App::friends_tab`; la última (`Canales`) abre los canales recientes.
const MODERN_TABS: [&str; 6] = ["Inicio", "En línea", "Todos", "Pendiente", "Bloqueados", "Canales"];
const HOME_TAB: usize = 0;
const ONLINE_TAB: usize = 1;
const PENDING_TAB: usize = 3;
/// "Añadir amigo": no es una pestaña de la fila; se abre con el botón verde
/// (interfaz nueva) o el botón "Añadir amigo" (clásica).
const ADD_FRIEND_TAB: usize = 6;
const ALL_TAB: usize = 2;
const RECENT_TAB: usize = 5;

/// Alto del banner de color de cada tarjeta de amigo y alto total de la tarjeta.
const BANNER_H: f32 = 84.0;
const TILE_H: f32 = 190.0;
const TILE_MIN_W: f32 = 250.0;
const TILE_GAP: f32 = 16.0;

/// Contenido de la píldora larga de la fila de arriba cuando se está en
/// Inicio: ícono + "Amigos", las pestañas y el botón verde de añadir amigo.
/// Lo llama `ui::topbar::show_row` dentro de un layout izquierda→derecha.
pub(crate) fn row_tabs(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    theme::icon(ui, Icon::Users, 18.0, palette.secondary);
    ui.add_space(8.0);
    theme::text(ui, "Amigos", theme::bold(17.0), palette.text);
    ui.add_space(18.0);

    for (i, tab) in MODERN_TABS.iter().enumerate() {
        let selected = app.friends_tab == i;
        let galley = ui
            .painter()
            .layout_no_wrap(tab.to_string(), theme::medium(14.0), palette.text);
        // "Pendiente" lleva un globito rojo con las solicitudes recibidas.
        let badge = if i == PENDING_TAB { app.pending_incoming_count() } else { 0 };
        let badge_w = if badge > 0 { 24.0 } else { 0.0 };
        let size = Vec2::new(galley.size().x + 22.0 + badge_w, 30.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let bg = if selected {
            palette.surface_active
        } else if resp.hovered() {
            palette.surface_hover
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(8), bg);
        let label_color = if selected { palette.text } else { palette.secondary };
        if badge > 0 {
            ui.painter().text(
                egui::pos2(rect.left() + 11.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                *tab,
                theme::medium(14.0),
                label_color,
            );
            let pill = Rect::from_center_size(
                egui::pos2(rect.right() - 11.0 - 9.0, rect.center().y),
                Vec2::new(18.0, 16.0),
            );
            ui.painter().rect_filled(pill, CornerRadius::same(8), palette.danger);
            ui.painter().text(
                pill.center(),
                egui::Align2::CENTER_CENTER,
                if badge > 9 { "9+".to_string() } else { badge.to_string() },
                theme::bold(10.5),
                Color32::WHITE,
            );
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                *tab,
                theme::medium(14.0),
                label_color,
            );
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            app.friends_tab = i;
        }
        ui.add_space(4.0);
    }

    // Botón verde "Añadir amigo", pegado a la derecha.
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let label = "Añadir amigo";
        let galley = ui
            .painter()
            .layout_no_wrap(label.to_string(), theme::semibold(13.0), Color32::WHITE);
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(galley.size().x + 26.0, 30.0), Sense::click());
        let green = extra::status_color(Status::Online, &palette);
        let selected = app.friends_tab == ADD_FRIEND_TAB;
        let fill = if selected {
            extra::blend(palette.panel, green, 0.18)
        } else if resp.hovered() {
            extra::blend(green, Color32::WHITE, 0.12)
        } else {
            green
        };
        ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            theme::semibold(13.0),
            if selected { green } else { Color32::WHITE },
        );
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            app.friends_tab = ADD_FRIEND_TAB;
        }
    });
}

/// Fondo de una tarjeta grande de Inicio.
fn card_bg(ui: &egui::Ui, palette: &Palette, rect: Rect) {
    let radius = CornerRadius::same(theme::CARD_RADIUS);
    ui.painter().rect_filled(rect, radius, palette.panel);
    ui.painter()
        .rect_stroke(rect, radius, Stroke::new(1.0, palette.outline), StrokeKind::Inside);
}

fn modern_central(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let mut action: Option<HomeAction> = None;
    let mut pending_click: Option<PendingClick> = None;
    let mut send_request: Option<String> = None;
    let gap = theme::GAP as f32;

    egui::CentralPanel::default()
        .frame(Frame::new().inner_margin(Margin {
            left: theme::GAP / 2,
            right: theme::GAP / 2,
            top: 0,
            bottom: theme::GAP,
        }))
        .show(ui, |ui| {
            let full = ui.available_rect_before_wrap();
            if full.width() < 300.0 || full.height() < 160.0 {
                return;
            }

            // Pestaña "Inicio": tablero a todo el ancho (saludo, amigos y
            // servers ordenados por afinidad). Ver `dashboard`.
            if app.friends_tab == HOME_TAB {
                dashboard(app, ui, full, &mut action);
                return;
            }

            // Dos tarjetas: amigos (centro) y "Activo ahora" (derecha, solo si
            // la ventana es lo bastante ancha).
            let show_active = full.width() > 820.0;
            let right_w = if show_active { 340.0 } else { 0.0 };
            let center_w = if show_active { full.width() - right_w - gap } else { full.width() };
            let center = Rect::from_min_size(full.min, Vec2::new(center_w, full.height()));

            // --- Tarjeta central -----------------------------------------
            if app.friends_tab == RECENT_TAB {
                // Los canales recientes dibujan su propio título y tarjeta.
                let app_ref: &App = app;
                channels_section(app_ref, ui, center, &mut action);
            } else if app.friends_tab == ADD_FRIEND_TAB {
                send_request = add_friend_card(app, ui, center, &palette);
            } else {
                card_bg(ui, &palette, center);
                let inner = center.shrink(20.0);

                let tab = app.friends_tab;
                let needle = app.friends_search.trim().to_lowercase();
                let order: Vec<usize> = {
                    let mut v: Vec<usize> = app
                        .friends
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| {
                            f.is_friend
                                && (needle.is_empty() || f.name.to_lowercase().contains(&needle))
                                && (tab != ONLINE_TAB || f.status != Status::Offline)
                        })
                        .map(|(i, _)| i)
                        .collect();
                    // En línea primero; dentro de cada grupo, la mayor afinidad
                    // (Discord) primero. Sin dato, el orden de la lista se mantiene.
                    v.sort_by(|&a, &b| {
                        let (fa, fb) = (&app.friends[a], &app.friends[b]);
                        let pa = app.affinity_users.pct(&fa.user_id).unwrap_or(-1.0);
                        let pb = app.affinity_users.pct(&fb.user_id).unwrap_or(-1.0);
                        (fa.status == Status::Offline)
                            .cmp(&(fb.status == Status::Offline))
                            .then_with(|| pb.total_cmp(&pa))
                    });
                    v
                };

                // Título: "En línea – 26".
                let title = match tab {
                    ONLINE_TAB => "En línea",
                    ALL_TAB => "Todos los amigos",
                    3 => "Pendiente",
                    _ => "Bloqueados",
                };
                let title_galley = ui
                    .painter()
                    .layout_no_wrap(title.to_string(), theme::bold(22.0), palette.text);
                let title_w = title_galley.size().x;
                ui.painter().galley(inner.min, title_galley, palette.text);
                if tab == ONLINE_TAB || tab == ALL_TAB || tab == PENDING_TAB {
                    let count = if tab == PENDING_TAB { app.pending_requests.len() } else { order.len() };
                    ui.painter().text(
                        inner.min + Vec2::new(title_w + 10.0, 3.0),
                        egui::Align2::LEFT_TOP,
                        format!("– {count}"),
                        theme::regular(20.0),
                        palette.dim,
                    );
                }

                // Buscador hundido.
                let search_rect = Rect::from_min_size(
                    inner.min + Vec2::new(0.0, 46.0),
                    Vec2::new(inner.width(), 38.0),
                );
                ui.painter()
                    .rect_filled(search_rect, CornerRadius::same(10), theme::inset(&palette));
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(search_rect.shrink2(Vec2::new(14.0, 0.0)))
                        .layout(Layout::left_to_right(Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        ui.add(
                            egui::TextEdit::singleline(&mut app.friends_search)
                                .hint_text("Buscar")
                                .frame(egui::Frame::NONE)
                                .desired_width((ui.available_width() - 26.0).max(20.0)),
                        );
                        ui.add_space(8.0);
                        theme::icon(ui, Icon::Search, 16.0, palette.secondary);
                    },
                );

                // Cuadrícula de tarjetas.
                let body = Rect::from_min_max(
                    egui::pos2(inner.min.x, search_rect.max.y + 18.0),
                    inner.max,
                );
                let app_ref: &App = app;
                if tab == PENDING_TAB {
                    pending_click = in_rect(ui, body, |ui| pending_list(ui, &palette, app_ref, &needle));
                } else if tab >= 3 {
                    in_rect(ui, body, |ui| {
                        ui.add_space(30.0);
                        ui.vertical_centered(|ui| {
                            theme::text(
                                ui,
                                "No hay elementos en esta vista (demo)",
                                theme::regular(13.0),
                                palette.dim,
                            );
                        });
                    });
                } else if order.is_empty() {
                    in_rect(ui, body, |ui| {
                        empty_card(
                            ui,
                            &palette,
                            "No hay nadie por acá",
                            "Cuando un amigo se conecte, aparece en esta lista.",
                        );
                    });
                } else {
                    in_rect(ui, body, |ui| {
                        ScrollArea::vertical()
                            .id_salt("home_friends_grid")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let avail = (ui.available_width() - 8.0).max(TILE_MIN_W);
                                let cols = (((avail + TILE_GAP) / (TILE_MIN_W + TILE_GAP)).floor() as usize)
                                    .clamp(1, 4);
                                let tile_w = (avail - TILE_GAP * (cols as f32 - 1.0)) / cols as f32;
                                for chunk in order.chunks(cols) {
                                    let (row, _) = ui.allocate_exact_size(
                                        Vec2::new(avail, TILE_H),
                                        Sense::hover(),
                                    );
                                    for (k, &idx) in chunk.iter().enumerate() {
                                        let rect = Rect::from_min_size(
                                            row.min + Vec2::new(k as f32 * (tile_w + TILE_GAP), 0.0),
                                            Vec2::new(tile_w, TILE_H),
                                        );
                                        if friend_tile(ui, &palette, &app_ref.friends[idx], idx, rect) {
                                            action = Some(HomeAction::OpenFriend(idx));
                                        }
                                    }
                                    ui.add_space(TILE_GAP);
                                }
                            });
                    });
                }
            }

            // --- Tarjeta derecha: "Activo ahora" ---------------------------
            if show_active {
                let right = Rect::from_min_max(egui::pos2(center.max.x + gap, full.min.y), full.max);
                card_bg(ui, &palette, right);
                let cards = app.active_now_cards();
                in_rect(ui, right.shrink(20.0), |ui| {
                    theme::text(ui, "Activo ahora", theme::bold(20.0), palette.text);
                    ui.add_space(16.0);
                    if cards.is_empty() {
                        theme::text(ui, "Todo tranquilo por ahora", theme::semibold(13.0), palette.text);
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(
                                    "Cuando un amigo juegue o escuche algo, o vos lo hagas, aparece acá.",
                                )
                                .font(theme::regular(12.0))
                                .color(palette.dim),
                            )
                            .wrap(),
                        );
                        return;
                    }
                    ScrollArea::vertical()
                        .id_salt("home_active_list")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                            for card in &cards {
                                activity_card(ui, &palette, card);
                                ui.add_space(10.0);
                            }
                        });
                });
            }
        });

    match action {
        Some(HomeAction::OpenFriend(i)) => app.open_dm(i),
        Some(HomeAction::OpenChannelAt(server, cat, chan, message_id)) => {
            app.open_channel_at(server, cat, chan, message_id);
        }
        Some(HomeAction::LoadPreview(server, cat, chan)) => app.load_recent_preview(server, cat, chan),
        Some(HomeAction::LoadNewer(channel_id)) => app.load_newer_for_channel(&channel_id),
        Some(HomeAction::OpenServer(i)) => app.open_server(i),
        None => {}
    }
    if let Some(click) = pending_click {
        apply_pending_click(app, click);
    }
    if let Some(username) = send_request {
        app.send_friend_request(username);
    }
}

// ---------------------------------------------------------------------
// "Añadir amigo": mandar una solicitud por nombre de usuario
// ---------------------------------------------------------------------

/// Tarjeta "Añadir amigo" de la interfaz nueva. Devuelve el nombre a enviar
/// cuando se toca el botón (o Enter en el campo).
fn add_friend_card(app: &mut App, ui: &mut egui::Ui, center: Rect, palette: &Palette) -> Option<String> {
    card_bg(ui, palette, center);
    in_rect(ui, center.shrink(20.0), |ui| {
        theme::text(ui, "Añadir amigo", theme::bold(22.0), palette.text);
        ui.add_space(8.0);
        add_friend_form(ui, palette, app)
    })
}

/// Texto de ayuda, campo con el botón "Enviar solicitud de amistad" a la
/// derecha y, debajo, el resultado del último envío (verde si salió bien,
/// rojo si falló). Devuelve el nombre a enviar si se confirmó.
fn add_friend_form(ui: &mut egui::Ui, palette: &Palette, app: &mut App) -> Option<String> {
    ui.add(
        egui::Label::new(
            RichText::new("Puedes añadir amigos con su nombre de usuario de Discord.")
                .font(theme::regular(13.5))
                .color(palette.secondary),
        )
        .wrap(),
    );
    ui.add_space(14.0);

    let (bar, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), Sense::hover());
    let label = "Enviar solicitud de amistad";
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::semibold(13.0), Color32::WHITE);
    let btn_w = galley.size().x + 28.0;
    let btn = Rect::from_center_size(
        egui::pos2(bar.right() - 10.0 - btn_w / 2.0, bar.center().y),
        Vec2::new(btn_w, 36.0),
    );
    let text_rect = Rect::from_min_max(
        bar.min + Vec2::new(16.0, 0.0),
        egui::pos2((btn.left() - 12.0).max(bar.min.x + 40.0), bar.max.y),
    );

    ui.painter()
        .rect_filled(bar, CornerRadius::same(10), theme::inset(palette));
    let edit = ui
        .scope_builder(
            UiBuilder::new()
                .max_rect(text_rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut app.add_friend_input)
                        .hint_text("Escribe un nombre de usuario")
                        .frame(egui::Frame::NONE)
                        .desired_width(text_rect.width()),
                )
            },
        )
        .inner;
    if edit.has_focus() {
        ui.painter().rect_stroke(
            bar,
            CornerRadius::same(10),
            Stroke::new(1.0, palette.accent),
            StrokeKind::Inside,
        );
    }

    let trimmed = app.add_friend_input.trim().to_string();
    let enabled = !trimmed.is_empty() && !app.add_friend_busy;
    let resp = ui.interact(
        btn,
        ui.id().with("add_friend_send"),
        if enabled { Sense::click() } else { Sense::hover() },
    );
    let fill = if !enabled {
        extra::blend(palette.accent, palette.panel, 0.55)
    } else if resp.hovered() {
        palette.accent_hover
    } else {
        palette.accent
    };
    ui.painter().rect_filled(btn, CornerRadius::same(8), fill);
    ui.painter().text(
        btn.center(),
        egui::Align2::CENTER_CENTER,
        label,
        theme::semibold(13.0),
        palette.on_accent,
    );
    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let mut send = None;
    if enabled && (resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() || enter) {
        send = Some(trimmed);
    }

    ui.add_space(14.0);
    if app.add_friend_busy {
        theme::text(ui, "Enviando solicitud…", theme::regular(13.0), palette.dim);
    } else if let Some((ok, message)) = &app.add_friend_status {
        let color = if *ok { extra::status_color(Status::Online, palette) } else { palette.danger };
        ui.add(
            egui::Label::new(RichText::new(message.as_str()).font(theme::regular(13.0)).color(color)).wrap(),
        );
    }
    send
}

// ---------------------------------------------------------------------
// Pestaña "Pendiente": solicitudes de amistad entrantes y salientes
// ---------------------------------------------------------------------

/// Alto de cada fila de solicitud.
const PENDING_ROW_H: f32 = 64.0;

/// Qué se tocó en la lista de solicitudes (se aplica al final del frame, así
/// la lista solo lee `&App`).
#[derive(Clone)]
enum PendingClick {
    /// Aceptar la solicitud entrante de esta persona (id).
    Accept(String),
    /// Rechazar la entrante o cancelar la saliente (id).
    Remove(String),
    /// Abrir el perfil de la persona (id).
    Profile(String),
}

fn apply_pending_click(app: &mut App, click: PendingClick) {
    use crate::discord::UserAction;
    match click {
        // Primero sin `confirm_stranger_request`; si Discord contesta 80013 sale
        // el popup de confirmación (ver `App::confirm_accept_friend`).
        PendingClick::Accept(id) => app.run_user_action(id, UserAction::AcceptFriend { confirm: false }),
        PendingClick::Remove(id) => app.run_user_action(id, UserAction::RemoveFriend),
        PendingClick::Profile(id) => {
            let found = app
                .pending_requests
                .iter()
                .find(|p| p.user.id == id)
                .map(|p| (p.name.clone(), p.avatar_url.clone(), p.avatar_color));
            if let Some((name, avatar_url, color)) = found {
                app.open_user_profile(id, name, avatar_url, color);
            }
        }
    }
}

/// Lista de solicitudes pendientes en dos secciones (entrantes primero, después
/// las enviadas), filtrada por `needle` (minúsculas) sobre nombre y usuario.
fn pending_list(ui: &mut egui::Ui, palette: &Palette, app: &App, needle: &str) -> Option<PendingClick> {
    let needle = needle.trim().to_lowercase();
    let matches = |p: &crate::lib::data::PendingRequest| {
        needle.is_empty()
            || p.name.to_lowercase().contains(&needle)
            || p.username.to_lowercase().contains(&needle)
    };
    let incoming: Vec<&crate::lib::data::PendingRequest> =
        app.pending_requests.iter().filter(|p| p.incoming && matches(p)).collect();
    let outgoing: Vec<&crate::lib::data::PendingRequest> =
        app.pending_requests.iter().filter(|p| !p.incoming && matches(p)).collect();

    if incoming.is_empty() && outgoing.is_empty() {
        if app.pending_requests.is_empty() {
            empty_card(
                ui,
                palette,
                "No hay solicitudes pendientes",
                "Cuando alguien te mande una solicitud de amistad, o mandes una tú, aparece aquí.",
            );
        } else {
            empty_card(ui, palette, "Sin resultados", "Ninguna solicitud coincide con la búsqueda.");
        }
        return None;
    }

    let mut click = None;
    ScrollArea::vertical()
        .id_salt("home_pending_list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (title, list) in [("Solicitudes entrantes", &incoming), ("Solicitudes enviadas", &outgoing)] {
                if list.is_empty() {
                    continue;
                }
                theme::text(ui, format!("{title} — {}", list.len()), theme::semibold(12.5), palette.dim);
                ui.add_space(6.0);
                for request in list.iter() {
                    if let Some(c) = pending_row(ui, palette, request) {
                        click = Some(c);
                    }
                    ui.add_space(2.0);
                }
                ui.add_space(14.0);
            }
        });
    click
}

/// Una solicitud: avatar, nombre, "Solicitud de amistad entrante/enviada" y a
/// la derecha los botones redondos (✓ aceptar solo en las entrantes, ✕ rechazar
/// o cancelar). Click en el resto de la fila: abre el perfil.
fn pending_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    request: &crate::lib::data::PendingRequest,
) -> Option<PendingClick> {
    let mut click = None;
    let (rect, row) = ui.allocate_exact_size(Vec2::new(ui.available_width(), PENDING_ROW_H), Sense::click());
    let hovered = ui.rect_contains_pointer(rect);
    if hovered {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(theme::radius() + 2), palette.surface);
    }

    let avatar_center = rect.left_center() + Vec2::new(28.0, 0.0);
    extra::avatar(
        ui,
        avatar_center,
        20.0,
        request.avatar_url.as_deref(),
        request.avatar_color,
        &request.initial(),
        palette,
    );

    // Botones, de derecha a izquierda.
    let btn = 36.0;
    let mut right = rect.right() - 12.0;
    let remove_rect = Rect::from_center_size(egui::pos2(right - btn / 2.0, rect.center().y), Vec2::splat(btn));
    right -= btn + 8.0;
    let accept_rect = request
        .incoming
        .then(|| Rect::from_center_size(egui::pos2(right - btn / 2.0, rect.center().y), Vec2::splat(btn)));
    let buttons_w = if request.incoming { 2.0 * btn + 8.0 + 12.0 } else { btn + 12.0 };

    // Texto.
    let text_x = 64.0;
    let max_w = (rect.width() - text_x - buttons_w - 12.0).max(40.0);
    crate::ui::emoji::paint_line_top(
        ui,
        rect.left_top() + Vec2::new(text_x, 13.0),
        &request.name,
        theme::medium(14.5),
        palette.text,
        max_w,
    );
    let label = if request.incoming {
        "Solicitud de amistad entrante"
    } else {
        "Solicitud de amistad enviada"
    };
    let sub = if request.username.is_empty() || request.username == request.name {
        label.to_string()
    } else {
        format!("{} · {label}", request.username)
    };
    let sub = clip_line(&sub, ((max_w / 6.0).max(8.0)) as usize);
    ui.painter().text(
        rect.left_top() + Vec2::new(text_x, 35.0),
        egui::Align2::LEFT_TOP,
        sub,
        theme::regular(12.0),
        palette.dim,
    );

    let base = if hovered { palette.surface_hover } else { palette.surface };
    let id = ui.id().with(("pending_row", &request.user.id));
    if let Some(accept_rect) = accept_rect {
        let green = extra::status_color(Status::Online, palette);
        if round_icon_button(ui, palette, base, accept_rect, id.with("accept"), Icon::Check, green, "Aceptar") {
            click = Some(PendingClick::Accept(request.user.id.clone()));
        }
    }
    let tip = if request.incoming { "Rechazar" } else { "Cancelar solicitud" };
    if round_icon_button(ui, palette, base, remove_rect, id.with("remove"), Icon::X, palette.danger, tip) {
        click = Some(PendingClick::Remove(request.user.id.clone()));
    }

    if click.is_none() && row.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        click = Some(PendingClick::Profile(request.user.id.clone()));
    }
    click
}

/// Botón circular con un ícono; el ícono toma `hover_color` al pasar el mouse.
#[allow(clippy::too_many_arguments)]
fn round_icon_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    base: Color32,
    rect: Rect,
    id: egui::Id,
    icon: Icon,
    hover_color: Color32,
    tip: &str,
) -> bool {
    let resp = ui.interact(rect, id, Sense::click());
    let hovered = resp.hovered();
    let bg = if hovered { palette.surface_active } else { base };
    ui.painter().circle_filled(rect.center(), rect.width() / 2.0, bg);
    theme::paint_icon(ui, icon, rect, 18.0, if hovered { hover_color } else { palette.secondary });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip).clicked()
}

/// Tarjeta de amigo: banner de color arriba, avatar grande que lo pisa con su
/// punto de presencia, y nombre + estado/actividad debajo. Devuelve `true` si
/// se hizo click.
fn friend_tile(
    ui: &mut egui::Ui,
    palette: &Palette,
    friend: &crate::lib::data::Friend,
    idx: usize,
    rect: Rect,
) -> bool {
    let resp = ui.interact(rect, ui.id().with(("friend_tile", idx)), Sense::click());
    let painter = ui.painter().clone();
    let radius = CornerRadius::same(14);
    let body = if resp.hovered() { palette.surface } else { palette.panel };

    painter.rect_filled(rect, radius, body);
    // Banner: solo redondeado arriba.
    let banner = Rect::from_min_size(rect.min, Vec2::new(rect.width(), BANNER_H));
    painter.rect_filled(
        banner,
        CornerRadius { nw: 14, ne: 14, sw: 0, se: 0 },
        palette.accent,
    );
    painter.rect_stroke(rect, radius, Stroke::new(1.0, palette.outline), StrokeKind::Inside);

    // Avatar grande pisando el banner, con un aro del color del cuerpo.
    let av_r = 34.0;
    let center = egui::pos2(rect.min.x + 20.0 + av_r, banner.max.y);
    painter.circle_filled(center, av_r + 5.0, body);
    extra::avatar(
        ui,
        center,
        av_r,
        friend.avatar_url.as_deref(),
        friend.avatar_color,
        &friend.initial(),
        palette,
    );
    extra::status_dot(ui, center, av_r, extra::status_color(friend.status, palette), body);

    // Nombre y estado.
    let text_x = rect.min.x + 18.0;
    let text_w = (rect.width() - 36.0).max(0.0);
    crate::ui::emoji::paint_line_top(
        ui,
        egui::pos2(text_x, banner.max.y + av_r + 12.0),
        &friend.name,
        theme::semibold(16.0),
        palette.text,
        text_w,
    );
    let subtitle = friend.subtitle.clone().unwrap_or_else(|| friend.status.label().to_string());
    let max_chars = ((text_w / 6.4).max(8.0)) as usize;
    painter.text(
        egui::pos2(text_x, banner.max.y + av_r + 36.0),
        egui::Align2::LEFT_TOP,
        clip_line(&subtitle, max_chars),
        theme::regular(12.5),
        palette.secondary,
    );

    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Corre `add` dentro de `rect`, recortando lo que se pase.
fn in_rect<R>(ui: &mut egui::Ui, rect: Rect, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope_builder(
        UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            add(ui)
        },
    )
    .inner
}

/// Lo que va a la derecha del título de una sección.
enum Tag {
    None,
    /// Globito rojo con un número (no leídos).
    Count(u32),
    /// Punto de estado + número (amigos en línea).
    Online(usize),
}

fn heading_row(ui: &mut egui::Ui, palette: &Palette, title: &str, tag: Tag) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.set_height(HEADING_H - 8.0);
        ui.add(
            egui::Label::new(RichText::new(title).font(theme::bold(24.0)).color(palette.text))
                .selectable(false),
        );
        match tag {
            Tag::None => {}
            Tag::Count(0) => {}
            Tag::Count(n) => {
                let label = if n > 99 { "99+".to_string() } else { n.to_string() };
                let w = if label.len() > 1 { 26.0 } else { 20.0 };
                let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 20.0), Sense::hover());
                ui.painter().rect_filled(rect, CornerRadius::same(10), palette.danger);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    theme::semibold(11.0),
                    palette.on_accent,
                );
            }
            Tag::Online(n) => {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                ui.painter().circle_filled(
                    rect.center(),
                    5.0,
                    extra::status_color(Status::Online, palette),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(n.to_string()).font(theme::regular(16.0)).color(palette.secondary),
                    )
                    .selectable(false),
                );
            }
        }
    });
    ui.add_space(8.0);
}

/// Tarjeta chica con un mensaje (listas vacías).
fn empty_card(ui: &mut egui::Ui, palette: &Palette, title: &str, hint: &str) {
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(ROW_RADIUS))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::text(ui, title, theme::semibold(14.0), palette.text);
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(RichText::new(hint).font(theme::regular(12.5)).color(palette.dim))
                    .wrap(),
            );
        });
}

/// Texto plano de un mensaje: menciones `<@id>` como `@Nombre`, en una sola
/// línea. Si no tiene texto, describe el adjunto.
pub(crate) fn plain_text(msg: &crate::lib::data::ChatMessage) -> String {
    let mut text = msg.content.clone();
    for (id, name) in &msg.mentions {
        text = text.replace(&format!("<@{id}>"), &format!("@{name}"));
        text = text.replace(&format!("<@!{id}>"), &format!("@{name}"));
    }
    let flat = crate::discord::models::one_line(&text);
    if !flat.is_empty() {
        flat
    } else if !msg.attachments.is_empty() {
        "Archivo adjunto".to_string()
    } else if !msg.embeds.is_empty() {
        "Enlace o contenido incrustado".to_string()
    } else {
        String::new()
    }
}

// ---------------------------------------------------------------------
// Amigos
// ---------------------------------------------------------------------

fn friends_section(app: &App, ui: &mut egui::Ui, rect: Rect, action: &mut Option<HomeAction>) {
    let palette = app.palette;
    // En línea primero; el orden de la lista se mantiene dentro de cada grupo.
    let mut order: Vec<usize> = app
        .friends
        .iter()
        .enumerate()
        .filter(|(_, f)| f.is_friend)
        .map(|(i, _)| i)
        .collect();
    order.sort_by_key(|&i| app.friends[i].status == Status::Offline);
    let online = order.iter().filter(|&&i| app.friends[i].status != Status::Offline).count();

    in_rect(ui, rect, |ui| {
        heading_row(ui, &palette, "Amigos", Tag::Online(online));
        if order.is_empty() {
            empty_card(ui, &palette, "Todavía no hay amigos", "Cuando agregues a alguien, aparece acá.");
            return;
        }
        ScrollArea::vertical()
            .id_salt("home_friends")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = (ui.available_width() - 6.0).max(0.0);
                for &i in &order {
                    if friend_card(ui, &palette, &app.friends[i], width) {
                        *action = Some(HomeAction::OpenFriend(i));
                    }
                    ui.add_space(8.0);
                }
            });
    });
}

fn friend_card(ui: &mut egui::Ui, palette: &Palette, friend: &crate::lib::data::Friend, width: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 58.0), Sense::click());
    let fill = if resp.hovered() { palette.surface_hover } else { palette.surface };
    ui.painter().rect_filled(rect, CornerRadius::same(ROW_RADIUS), fill);

    let center = rect.left_center() + Vec2::new(32.0, 0.0);
    extra::avatar(ui, center, 19.0, friend.avatar_url.as_deref(), friend.avatar_color, &friend.initial(), palette);
    extra::status_dot(ui, center, 19.0, extra::status_color(friend.status, palette), fill);

    let text_x = 64.0;
    crate::ui::emoji::paint_line_top(
        ui,
        rect.left_top() + Vec2::new(text_x, 11.0),
        &friend.name,
        theme::semibold(15.0),
        palette.text,
        (width - text_x - 12.0).max(0.0),
    );
    let subtitle = friend.subtitle.clone().unwrap_or_else(|| friend.status.label().to_string());
    let max_chars = (((width - text_x - 16.0) / 6.2).max(8.0)) as usize;
    ui.painter().text(
        rect.left_top() + Vec2::new(text_x, 33.0),
        egui::Align2::LEFT_TOP,
        clip_line(&subtitle, max_chars),
        theme::regular(12.5),
        palette.dim,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

// ---------------------------------------------------------------------
// Activo ahora
// ---------------------------------------------------------------------

fn active_section(app: &App, ui: &mut egui::Ui, rect: Rect) {
    let palette = app.palette;
    let cards = app.active_now_cards();
    in_rect(ui, rect, |ui| {
        heading_row(ui, &palette, "Activo ahora", Tag::None);
        if cards.is_empty() {
            empty_card(
                ui,
                &palette,
                "Todo tranquilo por ahora",
                "Cuando un amigo juegue o escuche algo, o vos lo hagas, aparece acá.",
            );
            return;
        }
        ScrollArea::horizontal()
            .id_salt("home_active")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    for card in &cards {
                        ui.allocate_ui_with_layout(
                            Vec2::new(300.0, 150.0),
                            Layout::top_down(Align::Min),
                            |ui| activity_card(ui, &palette, card),
                        );
                    }
                });
            });
    });
}

// ---------------------------------------------------------------------
// Canales recientes
// ---------------------------------------------------------------------
//
// Lista de canales (agrupada por server) y, a la derecha, los mensajes del
// canal elegido. La vista previa NO muestra "lo último que llegó" sino lo
// que hay alrededor del último mensaje leído: se piden con
// `?limit=30&around=<último leído>` (`App::load_recent_preview`), se marca
// dónde empiezan los sin leer y, al llegar abajo del todo, se siguen
// trayendo con `?after=<id>` (`App::load_newer_for_channel`) mientras la
// ventana no llegue al mensaje más nuevo del canal.

struct RecentChannel {
    server: usize,
    cat: usize,
    chan: usize,
    /// Id real de Discord (vacío en los canales de demo).
    channel_id: String,
    server_name: String,
    icon_url: Option<String>,
    icon_color: Color32,
    icon_initial: String,
    name: String,
    mentions: u32,
    last: u64,
}

impl RecentChannel {
    fn key(&self) -> (usize, usize, usize) {
        (self.server, self.cat, self.chan)
    }
}

/// Canales de texto con menciones sin leer o con mensajes ya cargados (o
/// sea, donde estuviste hace poco), los más recientes primero.
fn collect_recent_channels(app: &App) -> Vec<RecentChannel> {
    use crate::lib::notifications::snowflake;
    let mut out = Vec::new();
    for (si, server) in app.servers.iter().enumerate() {
        for (ci, category) in server.categories.iter().enumerate() {
            for (hi, ch) in category.channels.iter().enumerate() {
                if ch.is_voice || ch.is_forum || ch.is_thread || !server.channel_visible(ci, hi) {
                    continue;
                }
                let mentions = ch
                    .channel_id
                    .as_deref()
                    .and_then(|id| app.unread_mentions.get(id).copied())
                    .unwrap_or(0);
                if mentions == 0 && ch.messages.is_empty() {
                    continue;
                }
                out.push(RecentChannel {
                    server: si,
                    cat: ci,
                    chan: hi,
                    channel_id: ch.channel_id.clone().unwrap_or_default(),
                    server_name: server.name.clone(),
                    icon_url: server.icon_url.clone(),
                    icon_color: server.icon_color,
                    icon_initial: server.icon_initial.clone(),
                    name: ch.name.clone(),
                    mentions,
                    last: ch.messages.last().map(|m| snowflake(&m.id)).unwrap_or(0),
                });
            }
        }
    }
    out.sort_by(|a, b| (b.mentions > 0, b.last).cmp(&(a.mentions > 0, a.last)));
    out.truncate(8);
    out
}

struct PreviewMessage {
    id: String,
    author: String,
    time: String,
    text: String,
    avatar_url: Option<String>,
    color: Color32,
}

/// Lo que se ve del canal elegido en el panel de la derecha.
struct Preview {
    messages: Vec<PreviewMessage>,
    /// Pedido inicial en curso.
    loading: bool,
    /// Todavía no se pidió nada de este canal: hay que cargar la ventana.
    needs_load: bool,
    /// Índice (en `messages`) del primer mensaje sin leer, si se conoce el
    /// último leído y hay algo posterior.
    first_unread: Option<usize>,
    /// La ventana no llega al mensaje más nuevo: abajo del todo se sigue con
    /// `after`.
    has_newer: bool,
    loading_newer: bool,
}

fn preview_messages(app: &App, r: &RecentChannel) -> Preview {
    use crate::lib::notifications::snowflake;
    let Some(ch) = app.servers.get(r.server).and_then(|s| s.channel(r.cat, r.chan)) else {
        return Preview {
            messages: Vec::new(),
            loading: false,
            needs_load: false,
            first_unread: None,
            has_newer: false,
            loading_newer: false,
        };
    };
    let last_read = app.last_read_message(&r.channel_id).map(snowflake).unwrap_or(0);
    let first_unread = if last_read == 0 {
        None
    } else {
        ch.messages.iter().position(|m| snowflake(&m.id) > last_read)
    };
    let messages = ch
        .messages
        .iter()
        .map(|m| PreviewMessage {
            id: m.id.clone(),
            author: m.author.clone(),
            time: m.time.clone(),
            text: plain_text(m),
            avatar_url: m.avatar_url.clone(),
            color: m.avatar_color,
        })
        .collect();
    Preview {
        messages,
        loading: ch.loading,
        needs_load: !ch.loaded && !ch.loading && !ch.is_forum && ch.channel_id.is_some(),
        first_unread,
        has_newer: ch.has_newer,
        loading_newer: ch.loading_newer,
    }
}

const PREVIEW_ROW_H: f32 = 44.0;

/// Mensajes de la vista previa, en una lista con scroll. Arranca parada en
/// el último mensaje leído (o abajo del todo si no hay sin leer); click en
/// un mensaje abre el canal justo ahí.
fn preview_list(
    ui: &mut egui::Ui,
    palette: &Palette,
    rect: Rect,
    current: &RecentChannel,
    preview: &Preview,
    action: &mut Option<HomeAction>,
) {
    in_rect(ui, rect, |ui| {
        let init_id = egui::Id::new(("home_recent_scrolled", current.server, current.cat, current.chan));
        let initialized = ui.ctx().data(|d| d.get_temp::<bool>(init_id)).unwrap_or(false);
        let mut area = ScrollArea::vertical()
            .id_salt(("home_recent_msgs", current.server, current.cat, current.chan))
            .auto_shrink([false, false]);
        match preview.first_unread {
            // Primera vez: un mensaje leído arriba de los sin leer, de contexto.
            Some(i) if !initialized => {
                area = area.vertical_scroll_offset(i.saturating_sub(1) as f32 * PREVIEW_ROW_H);
                ui.ctx().data_mut(|d| d.insert_temp(init_id, true));
            }
            // Nada sin leer: se queda pegado a lo más nuevo.
            None => area = area.stick_to_bottom(true),
            Some(_) => {}
        }
        area.show(ui, |ui| {
            let width = (ui.available_width() - 6.0).max(0.0);
            for (i, m) in preview.messages.iter().enumerate() {
                let (row, resp) = ui.allocate_exact_size(Vec2::new(width, PREVIEW_ROW_H), Sense::click());
                if !ui.is_rect_visible(row) {
                    continue;
                }
                if resp.hovered() {
                    ui.painter().rect_filled(row, CornerRadius::same(10), palette.surface_hover);
                }
                if preview.first_unread == Some(i) {
                    // Línea roja donde empiezan los mensajes sin leer.
                    ui.painter()
                        .hline(row.x_range(), row.top() + 1.0, Stroke::new(1.5, palette.danger));
                    ui.painter().text(
                        row.right_top() + Vec2::new(-8.0, 4.0),
                        egui::Align2::RIGHT_TOP,
                        "NUEVOS",
                        theme::semibold(9.5),
                        palette.danger,
                    );
                }
                let top = row.min + Vec2::new(6.0, 3.0);
                extra::avatar(
                    ui,
                    top + Vec2::new(16.0, 16.0),
                    15.0,
                    m.avatar_url.as_deref(),
                    m.color,
                    &m.author
                        .chars()
                        .find(|c| c.is_alphanumeric())
                        .map(|c| c.to_uppercase().to_string())
                        .unwrap_or_else(|| "?".to_string()),
                    palette,
                );
                let name_galley = ui
                    .painter()
                    .layout_no_wrap(m.author.clone(), theme::semibold(13.0), palette.accent);
                let name_w = name_galley.size().x.min((width - 130.0).max(40.0));
                let painter = ui.painter().with_clip_rect(row);
                painter.text(
                    top + Vec2::new(40.0, 0.0),
                    egui::Align2::LEFT_TOP,
                    clip_line(&m.author, 28),
                    theme::semibold(13.0),
                    palette.accent,
                );
                painter.text(
                    top + Vec2::new(40.0 + name_w + 8.0, 2.0),
                    egui::Align2::LEFT_TOP,
                    &m.time,
                    theme::regular(11.0),
                    palette.dim,
                );
                clipped_line(
                    ui,
                    Rect::from_min_size(top + Vec2::new(40.0, 19.0), Vec2::new((width - 52.0).max(0.0), 16.0)),
                    &m.text,
                    theme::regular(13.0),
                    palette.text,
                );
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !m.id.is_empty() {
                    *action = Some(HomeAction::OpenChannelAt(
                        current.server,
                        current.cat,
                        current.chan,
                        Some(m.id.clone()),
                    ));
                }
            }

            // La ventana no llega al último mensaje: al llegar acá abajo se
            // piden los siguientes (`after`).
            if preview.has_newer {
                let (end, _) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::hover());
                ui.painter().text(
                    end.center(),
                    egui::Align2::CENTER_CENTER,
                    if preview.loading_newer { "Cargando más mensajes…" } else { "Seguí bajando para ver más" },
                    theme::regular(11.5),
                    palette.dim,
                );
                if !preview.loading_newer && ui.is_rect_visible(end) && action.is_none() {
                    *action = Some(HomeAction::LoadNewer(current.channel_id.clone()));
                }
            }
        });
    });
}

fn channels_section(app: &App, ui: &mut egui::Ui, rect: Rect, action: &mut Option<HomeAction>) {
    let palette = app.palette;
    let recent = collect_recent_channels(app);
    let total_mentions: u32 = recent.iter().map(|r| r.mentions).sum();
    let sel_id = egui::Id::new("home_recent_channel");

    in_rect(ui, rect, |ui| {
        heading_row(ui, &palette, "Canales recientes", Tag::Count(total_mentions));

        let card = ui.available_rect_before_wrap();
        if card.height() < 80.0 {
            return;
        }
        ui.allocate_rect(card, Sense::hover());
        ui.painter().rect_filled(card, CornerRadius::same(theme::CARD_RADIUS), palette.panel);
        ui.painter().rect_stroke(
            card,
            CornerRadius::same(theme::CARD_RADIUS),
            Stroke::new(1.0, palette.outline),
            StrokeKind::Inside,
        );

        if recent.is_empty() {
            ui.painter().text(
                card.center(),
                egui::Align2::CENTER_CENTER,
                "Todavía no hay canales recientes. Abrí un servidor y aparecen acá.",
                theme::regular(13.0),
                palette.dim,
            );
            return;
        }

        // Canal elegido: el guardado si sigue en la lista, si no el primero.
        let stored = ui.ctx().data_mut(|d| d.get_temp::<(usize, usize, usize)>(sel_id));
        let selected = stored
            .filter(|s| recent.iter().any(|r| r.key() == *s))
            .unwrap_or_else(|| recent[0].key());

        let list_w = (card.width() * 0.32).clamp(190.0, 260.0).min(card.width() * 0.6);
        let list_rect = Rect::from_min_size(card.min, Vec2::new(list_w, card.height())).shrink(10.0);
        let pane_rect = Rect::from_min_max(card.min + Vec2::new(list_w, 0.0), card.max).shrink2(Vec2::new(14.0, 12.0));
        ui.painter().line_segment(
            [
                egui::pos2(card.min.x + list_w, card.min.y + 14.0),
                egui::pos2(card.min.x + list_w, card.max.y - 14.0),
            ],
            Stroke::new(1.0, palette.outline),
        );

        // ---- Lista agrupada por server ----
        let mut new_selection = None;
        in_rect(ui, list_rect, |ui| {
            let mut servers: Vec<usize> = Vec::new();
            for r in &recent {
                if !servers.contains(&r.server) {
                    servers.push(r.server);
                }
            }
            ScrollArea::vertical()
                .id_salt("home_recent_list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let width = (ui.available_width() - 6.0).max(0.0);
                    for server in servers {
                        let name = recent
                            .iter()
                            .find(|r| r.server == server)
                            .map(|r| r.server_name.to_uppercase())
                            .unwrap_or_default();
                        ui.add_space(6.0);
                        let (head, _) = ui.allocate_exact_size(Vec2::new(width, 18.0), Sense::hover());
                        clipped_line(
                            ui,
                            Rect::from_min_size(head.min + Vec2::new(8.0, 2.0), Vec2::new(width - 16.0, 14.0)),
                            &name,
                            theme::semibold(11.0),
                            palette.dim,
                        );
                        for r in recent.iter().filter(|r| r.server == server) {
                            if recent_row(ui, &palette, r, r.key() == selected, width) {
                                new_selection = Some(r.key());
                            }
                            ui.add_space(2.0);
                        }
                    }
                });
        });
        if let Some(sel) = new_selection {
            ui.ctx().data_mut(|d| d.insert_temp(sel_id, sel));
        }

        // ---- Vista previa del canal elegido ----
        let Some(current) = recent.iter().find(|r| r.key() == selected) else { return };
        let preview = preview_messages(app, current);
        // Todavía sin mensajes: se piden alrededor del último leído.
        if preview.needs_load && action.is_none() {
            *action = Some(HomeAction::LoadPreview(current.server, current.cat, current.chan));
        }
        // Al abrir el canal se va al primer mensaje sin leer (si lo hay).
        let unread_id = preview
            .first_unread
            .and_then(|i| preview.messages.get(i))
            .map(|m| m.id.clone())
            .filter(|id| !id.is_empty());
        let mut open = false;

        // Encabezado: "# canal" + server, y botón "Abrir" a la derecha.
        let button = Rect::from_min_size(
            egui::pos2(pane_rect.right() - 76.0, pane_rect.top()),
            Vec2::new(76.0, 28.0),
        );
        let button_resp = ui.interact(button, egui::Id::new("home_recent_open"), Sense::click());
        ui.painter().rect_filled(
            button,
            CornerRadius::same(14),
            if button_resp.hovered() { palette.surface_hover } else { palette.surface },
        );
        ui.painter().text(
            button.center(),
            egui::Align2::CENTER_CENTER,
            "Abrir",
            theme::medium(12.5),
            palette.text,
        );
        if button_resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            open = true;
        }
        ui.painter().text(
            pane_rect.left_top() + Vec2::new(2.0, 2.0),
            egui::Align2::LEFT_TOP,
            "#",
            theme::medium(17.0),
            palette.dim,
        );
        clipped_line(
            ui,
            Rect::from_min_size(
                pane_rect.left_top() + Vec2::new(20.0, 3.0),
                Vec2::new((pane_rect.width() - 20.0 - 88.0).max(0.0), 20.0),
            ),
            &format!("{}  ·  {}", current.name, current.server_name),
            theme::semibold(14.5),
            palette.text,
        );

        // Compositor (decorativo: al hacer click abre el canal).
        let composer = Rect::from_min_max(
            egui::pos2(pane_rect.left(), pane_rect.bottom() - 44.0),
            pane_rect.max,
        );
        let composer_resp = ui.interact(composer, egui::Id::new("home_recent_composer"), Sense::click());
        ui.painter().rect_filled(
            composer,
            CornerRadius::same(14),
            if composer_resp.hovered() { palette.surface_hover } else { palette.surface },
        );
        ui.painter().text(
            composer.left_center() + Vec2::new(16.0, 0.0),
            egui::Align2::LEFT_CENTER,
            format!("Mensaje #{}", current.name),
            theme::regular(13.0),
            palette.dim,
        );
        if composer_resp.on_hover_cursor(egui::CursorIcon::Text).clicked() {
            open = true;
        }

        // Mensajes: alrededor del último leído, con scroll.
        let msgs_rect = Rect::from_min_max(
            egui::pos2(pane_rect.left(), pane_rect.top() + 40.0),
            egui::pos2(pane_rect.right(), composer.top() - 10.0),
        );
        if preview.messages.is_empty() {
            ui.painter().with_clip_rect(msgs_rect).text(
                msgs_rect.center(),
                egui::Align2::CENTER_CENTER,
                if preview.loading || preview.needs_load { "Cargando mensajes…" } else { "Todavía no hay mensajes cargados." },
                theme::regular(13.0),
                palette.dim,
            );
        } else if msgs_rect.height() > 20.0 {
            preview_list(ui, &palette, msgs_rect, current, &preview, action);
        }

        if open {
            *action = Some(HomeAction::OpenChannelAt(current.server, current.cat, current.chan, unread_id));
        }
    });
}

/// Fila de un canal en la lista de recientes. `true` si se eligió.
fn recent_row(ui: &mut egui::Ui, palette: &Palette, r: &RecentChannel, selected: bool, width: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::click());
    let fill = theme::row_fill(palette, selected, resp.hovered());
    ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
    extra::avatar(
        ui,
        rect.left_center() + Vec2::new(17.0, 0.0),
        10.0,
        r.icon_url.as_deref(),
        r.icon_color,
        &r.icon_initial,
        palette,
    );
    ui.painter().text(
        rect.left_center() + Vec2::new(36.0, 0.0),
        egui::Align2::LEFT_CENTER,
        "#",
        theme::medium(15.0),
        palette.dim,
    );
    let name_w = (width - 52.0 - if r.mentions > 0 { 36.0 } else { 8.0 }).max(0.0);
    clipped_line(
        ui,
        Rect::from_min_size(rect.left_top() + Vec2::new(50.0, 9.0), Vec2::new(name_w, 18.0)),
        &r.name,
        theme::medium(13.5),
        if selected { palette.text } else { palette.secondary },
    );
    crate::ui::notifications::paint_badge(
        ui.painter(),
        rect.right_center() - Vec2::new(20.0, 0.0),
        r.mentions,
        palette,
        fill,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Barra de arriba compartida por Home/Dm/Server: flechas + título + íconos.
pub fn top_bar(ui: &mut egui::Ui, palette: &Palette, title: &str, title_icon: Icon) {
    Frame::new()
        .stroke(theme::header_stroke(palette))
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
                .rect_filled(rect, CornerRadius::same(theme::radius() + 2), palette.surface);
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

// =====================================================================
// Inicio nuevo: tablero con saludo, amigos y servers por afinidad
// =====================================================================
//
// Pestaña "Inicio" de la interfaz nueva. De arriba hacia abajo:
//
//   1. Tarjeta de perfil: banner con degradado, avatar, "Buenos días, X".
//   2. Amigos: carrusel horizontal de avatares con aro de estado. Orden:
//      primero los que están haciendo algo (juego, música...), después los
//      conectados y al final los desconectados; dentro de cada grupo, mayor
//      afinidad (`GET /users/@me/affinities/users`) primero.
//   3. Servidores frecuentes: carrusel ordenado por la afinidad con cada
//      server (`GET /users/@me/affinities/guilds`).
//
// Todo se dibuja sobre rectángulos absolutos dentro de un único scroll
// vertical (ver el comentario de `dashboard`), y los carruseles solo
// dibujan los ítems visibles para no pedir avatares que no se ven.

const DASH_HEADER_H: f32 = 170.0;
const DASH_BANNER_H: f32 = 96.0;
const DASH_HEAD_H: f32 = 44.0;
const DASH_PAD: f32 = 20.0;
const DASH_ITEM_W: f32 = 128.0;
const DASH_ITEM_GAP: f32 = 10.0;
/// Espacio que se deja abajo de cada carrusel para la barra de scroll.
const DASH_SCROLLBAR_ROOM: f32 = 16.0;
const FRIEND_AV_R: f32 = 50.0;
const FRIEND_ROW_H: f32 = 160.0;
const SERVER_ICON_R: f32 = 44.0;
const SERVER_ROW_H: f32 = 140.0;

/// Saludo según la hora local.
fn greeting() -> &'static str {
    use chrono::Timelike;
    match chrono::Local::now().hour() {
        5..=11 => "Buenos días",
        12..=19 => "Buenas tardes",
        _ => "Buenas noches",
    }
}

/// Texto de una sola línea centrado en `center_x` (con soporte de emojis).
fn centered_line(ui: &egui::Ui, center_x: f32, top: f32, text: &str, font: egui::FontId, color: Color32, max_w: f32) {
    let w = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), color)
        .size()
        .x
        .min(max_w);
    crate::ui::emoji::paint_line_top(ui, egui::pos2(center_x - w / 2.0, top), text, font, color, max_w);
}

/// Rectángulo con las esquinas de arriba redondeadas y un degradado vertical
/// (malla con un color por vértice: como el color depende solo de `y`, el
/// degradado queda exacto aunque el polígono sea un abanico de triángulos).
fn banner_gradient(painter: &egui::Painter, rect: Rect, radius: f32, top: Color32, bottom: Color32) {
    const SEG: usize = 8;
    let quarter = std::f32::consts::FRAC_PI_2;
    let mut pts: Vec<egui::Pos2> = Vec::with_capacity(2 * (SEG + 1) + 2);
    let left = egui::pos2(rect.min.x + radius, rect.min.y + radius);
    for i in 0..=SEG {
        let a = std::f32::consts::PI + quarter * i as f32 / SEG as f32;
        pts.push(left + Vec2::new(a.cos(), a.sin()) * radius);
    }
    let right = egui::pos2(rect.max.x - radius, rect.min.y + radius);
    for i in 0..=SEG {
        let a = std::f32::consts::PI * 1.5 + quarter * i as f32 / SEG as f32;
        pts.push(right + Vec2::new(a.cos(), a.sin()) * radius);
    }
    pts.push(rect.right_bottom());
    pts.push(rect.left_bottom());

    let mut mesh = egui::Mesh::default();
    for p in &pts {
        let t = ((p.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0);
        mesh.colored_vertex(*p, extra::blend(top, bottom, t));
    }
    for i in 1..(pts.len() as u32 - 1) {
        mesh.add_triangle(0, i, i + 1);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Tarjeta de arriba: banner, avatar y saludo.
fn dash_header(
    ui: &mut egui::Ui,
    palette: &Palette,
    rect: Rect,
    me: Option<&(String, String, Option<String>, Option<u32>)>,
) {
    card_bg(ui, palette, rect);
    let (name, username, avatar_url, accent_rgb) = match me {
        Some((n, u, a, c)) => (n.as_str(), u.as_str(), a.as_deref(), *c),
        None => ("Aiden", "", None, None),
    };
    let accent = accent_rgb
        .map(|c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8))
        .unwrap_or(palette.accent);

    // Banner con degradado (del acento hacia el color de la tarjeta).
    let banner = Rect::from_min_size(rect.min, Vec2::new(rect.width(), DASH_BANNER_H));
    banner_gradient(
        ui.painter(),
        banner,
        theme::CARD_RADIUS as f32,
        extra::blend(palette.panel, accent, 0.65),
        extra::blend(palette.panel, accent, 0.10),
    );
    // El borde se vuelve a pasar por encima del banner.
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(theme::CARD_RADIUS),
        Stroke::new(1.0, palette.outline),
        StrokeKind::Inside,
    );

    // Avatar grande a caballo entre el banner y el cuerpo.
    let av_r = 48.0;
    let center = egui::pos2(rect.min.x + 28.0 + av_r, banner.max.y);
    ui.painter().circle_filled(center, av_r + 6.0, palette.panel);
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    extra::avatar(ui, center, av_r, avatar_url, palette.accent, &initial, palette);

    // "Buenos días, Nombre" + @usuario.
    let text_x = center.x + av_r + 22.0;
    let top = banner.max.y + 14.0;
    let font = theme::bold(26.0);
    let hello = ui
        .painter()
        .layout_no_wrap(format!("{}, ", greeting()), font.clone(), palette.text);
    let hello_w = hello.size().x;
    ui.painter().galley(egui::pos2(text_x, top), hello, palette.text);
    let max_name_w = (rect.max.x - text_x - hello_w - 20.0).max(40.0);
    crate::ui::emoji::paint_line_top(
        ui,
        egui::pos2(text_x + hello_w, top),
        name,
        font,
        palette.accent,
        max_name_w,
    );
    if !username.is_empty() {
        ui.painter().text(
            egui::pos2(text_x, top + 40.0),
            egui::Align2::LEFT_TOP,
            format!("@{username}"),
            theme::regular(14.0),
            palette.secondary,
        );
    }
}

/// Botón cuadrado con una flecha (`‹` / `›`). `true` si se hizo click.
fn dash_arrow(ui: &mut egui::Ui, palette: &Palette, rect: Rect, icon: Icon, enabled: bool, id: egui::Id) -> bool {
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(8), palette.surface_hover);
    }
    let color = if enabled { palette.secondary } else { extra::blend(palette.dim, palette.panel, 0.55) };
    ui.put(Rect::from_center_size(rect.center(), Vec2::splat(18.0)), icon.image(color, 18.0));
    enabled && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Lo que va a la derecha del título de un carrusel: "Ver todos" (opcional)
/// y las flechas. Devuelve `(ver_todos, dirección)` con dirección -1 / 0 / 1.
fn dash_controls(
    ui: &mut egui::Ui,
    palette: &Palette,
    head: Rect,
    salt: &'static str,
    see_all: bool,
    can_left: bool,
    can_right: bool,
) -> (bool, i32) {
    let btn = 30.0;
    let mut dir = 0;
    let right_rect = Rect::from_min_size(egui::pos2(head.max.x - btn, head.min.y), Vec2::splat(btn));
    let left_rect = right_rect.translate(Vec2::new(-btn - 4.0, 0.0));
    // Los ids se arman antes: `ui` va prestado como `&mut` en cada llamada.
    let left_id = ui.id().with((salt, "left"));
    let right_id = ui.id().with((salt, "right"));
    if dash_arrow(ui, palette, left_rect, Icon::ChevronLeft, can_left, left_id) {
        dir = -1;
    }
    if dash_arrow(ui, palette, right_rect, Icon::ChevronRight, can_right, right_id) {
        dir = 1;
    }

    let mut see = false;
    if see_all {
        let label = "Ver todos";
        let galley = ui
            .painter()
            .layout_no_wrap(label.to_string(), theme::semibold(13.0), palette.accent);
        let w = galley.size().x;
        let r = Rect::from_min_size(
            egui::pos2(left_rect.min.x - 16.0 - w, head.min.y + 5.0),
            Vec2::new(w, 20.0),
        );
        let resp = ui.interact(r, ui.id().with((salt, "see_all")), Sense::click());
        let color = if resp.hovered() { extra::blend(palette.accent, Color32::WHITE, 0.2) } else { palette.accent };
        ui.painter().text(
            r.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            theme::semibold(13.0),
            color,
        );
        see = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    }
    (see, dir)
}

/// Fila horizontal con scroll: dibuja solo los ítems visibles. `req` mueve la
/// fila a ese offset (lo usan las flechas). Devuelve `(offset, offset_máximo)`.
fn dash_carousel(
    ui: &mut egui::Ui,
    id: &'static str,
    rect: Rect,
    count: usize,
    req: Option<f32>,
    mut draw: impl FnMut(&mut egui::Ui, usize, Rect),
) -> (f32, f32) {
    let stride = DASH_ITEM_W + DASH_ITEM_GAP;
    let total_w = (count as f32 * stride - DASH_ITEM_GAP).max(1.0);
    let row_h = (rect.height() - DASH_SCROLLBAR_ROOM).max(1.0);
    let out = in_rect(ui, rect, |ui| {
        let mut area = ScrollArea::horizontal().id_salt(id).auto_shrink([false, false]);
        if let Some(offset) = req {
            area = area.horizontal_scroll_offset(offset);
        }
        area.show_viewport(ui, |ui, viewport| {
            let (row, _) = ui.allocate_exact_size(Vec2::new(total_w, row_h), Sense::hover());
            let first = (viewport.min.x / stride).floor().max(0.0) as usize;
            let last = (((viewport.max.x / stride).ceil() as usize) + 1).min(count);
            for i in first..last {
                let item = Rect::from_min_size(
                    egui::pos2(row.min.x + i as f32 * stride, row.min.y),
                    Vec2::new(DASH_ITEM_W, row_h),
                );
                draw(ui, i, item);
            }
        })
    });
    (out.state.offset.x, (out.content_size.x - out.inner_rect.width()).max(0.0))
}

/// Título de un carrusel (`Amigos · 283 · 3 en línea`).
fn dash_title(ui: &egui::Ui, palette: &Palette, pos: egui::Pos2, title: &str, detail: &str) {
    let galley = ui
        .painter()
        .layout_no_wrap(title.to_string(), theme::bold(22.0), palette.text);
    let w = galley.size().x;
    ui.painter().galley(pos, galley, palette.text);
    if !detail.is_empty() {
        ui.painter().text(
            pos + Vec2::new(w + 10.0, 5.0),
            egui::Align2::LEFT_TOP,
            detail,
            theme::regular(14.0),
            palette.dim,
        );
    }
}

/// Color del aro del avatar de un amigo: verde si está haciendo algo, el
/// acento si está conectado, y el color de su estado en ausente / no molestar.
fn friend_ring(palette: &Palette, f: &crate::lib::data::HomeFriend) -> Color32 {
    if f.activity.is_some() {
        return extra::status_color(Status::Online, palette);
    }
    match f.status {
        Status::Online => palette.accent,
        Status::Idle => palette.warning,
        Status::Dnd => palette.danger,
        Status::Offline => palette.outline,
    }
}

/// Un amigo del carrusel: avatar con aro, nombre y qué está haciendo.
fn dash_friend_item(
    ui: &mut egui::Ui,
    palette: &Palette,
    f: &crate::lib::data::HomeFriend,
    item: Rect,
) -> bool {
    let resp = ui.interact(item, ui.id().with(("home_friend", f.index)), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(item, CornerRadius::same(14), palette.surface);
    }
    let center = egui::pos2(item.center().x, item.min.y + 8.0 + FRIEND_AV_R);
    let ring = friend_ring(palette, f);
    extra::avatar(ui, center, FRIEND_AV_R - 3.0, f.avatar_url.as_deref(), f.avatar_color, &f.initial, palette);
    ui.painter()
        .circle_stroke(center, FRIEND_AV_R - 1.0, Stroke::new(3.0, ring));

    let max_w = item.width() - 12.0;
    centered_line(ui, item.center().x, center.y + FRIEND_AV_R + 10.0, &f.name, theme::semibold(14.0), palette.text, max_w);
    let (sub, sub_color) = match (&f.activity, f.status) {
        (Some(activity), _) => (activity.clone(), ring),
        (None, Status::Offline) => (Status::Offline.label().to_string(), palette.dim),
        (None, status) => (status.label().to_string(), ring),
    };
    centered_line(ui, item.center().x, center.y + FRIEND_AV_R + 31.0, &sub, theme::medium(12.5), sub_color, max_w);

    let clicked = resp.clicked();
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let ring = friend_ring(palette, f);
    resp.on_hover_ui(|ui| friend_hover_card(ui, palette, f, ring));
    clicked
}

/// Tarjeta flotante al pasar el mouse sobre un amigo: actividad, identidad y
/// todos sus puestos de afinidad.
fn friend_hover_card(ui: &mut egui::Ui, palette: &Palette, f: &crate::lib::data::HomeFriend, ring: Color32) {
    const W: f32 = 300.0;
    ui.set_width(W);
    ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);

    // Actividad (como el banner de la referencia).
    if let Some(title) = &f.activity_title {
        Frame::new()
            .fill(palette.surface)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(W - 20.0);
                theme::text(ui, title.clone(), theme::semibold(14.0), extra::status_color(Status::Online, palette));
                if let Some(detail) = &f.activity_detail {
                    ui.add(egui::Label::new(RichText::new(detail).font(theme::regular(12.5)).color(palette.secondary)).wrap());
                }
            });
        ui.add_space(8.0);
    }

    // Identidad.
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(52.0), Sense::hover());
        extra::avatar(ui, rect.center(), 24.0, f.avatar_url.as_deref(), f.avatar_color, &f.initial, palette);
        ui.painter().circle_stroke(rect.center(), 25.5, Stroke::new(2.5, ring));
        ui.vertical(|ui| {
            theme::text(ui, f.name.clone(), theme::bold(16.0), palette.text);
            theme::text(ui, f.handle.clone(), theme::regular(12.5), palette.secondary);
            theme::text(ui, f.status.label(), theme::medium(12.0), ring);
        });
    });
    ui.add_space(8.0);

    // Afinidad.
    match f.affinity {
        Some(pct) => {
            theme::text(ui, "Afinidad", theme::semibold(13.0), palette.text);
            theme::text(ui, format!("{pct:.0}%"), theme::bold(22.0), palette.accent);
            // Barra de progreso.
            let (bar, _) = ui.allocate_exact_size(Vec2::new(W, 6.0), Sense::hover());
            ui.painter().rect_filled(bar, CornerRadius::same(3), theme::inset(palette));
            let fill = Rect::from_min_size(bar.min, Vec2::new(bar.width() * (pct / 100.0).clamp(0.0, 1.0), 6.0));
            ui.painter().rect_filled(fill, CornerRadius::same(3), palette.accent);
            ui.add_space(6.0);
            for (label, rank, total) in &f.ranks {
                ui.horizontal(|ui| {
                    theme::text(ui, *label, theme::regular(12.5), palette.secondary);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        theme::text(ui, format!("#{rank} de {total}"), theme::semibold(12.5), palette.text);
                    });
                });
            }
        }
        None => {
            theme::text(ui, "Sin datos de afinidad para este amigo", theme::regular(12.5), palette.dim);
        }
    }
    ui.add_space(4.0);
    theme::text(ui, "Click para abrir el chat", theme::regular(11.5), palette.dim);
}

/// Un server del carrusel: ícono, nombre y cuántos hay en línea.
fn dash_server_item(
    ui: &mut egui::Ui,
    palette: &Palette,
    s: &crate::lib::data::HomeServer,
    item: Rect,
) -> bool {
    let resp = ui.interact(item, ui.id().with(("home_server", s.index)), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(item, CornerRadius::same(14), palette.surface);
    }
    let center = egui::pos2(item.center().x, item.min.y + 8.0 + SERVER_ICON_R);
    extra::avatar_rounded(ui, center, SERVER_ICON_R, 20.0, s.icon_url.as_deref(), s.icon_color, &s.initial, palette);

    let max_w = item.width() - 12.0;
    centered_line(ui, item.center().x, center.y + SERVER_ICON_R + 10.0, &s.name, theme::semibold(14.0), palette.text, max_w);
    if s.online_count > 0 {
        let sub = format!("{} en línea", s.online_count);
        centered_line(ui, item.center().x, center.y + SERVER_ICON_R + 31.0, &sub, theme::medium(12.5), palette.secondary, max_w);
    }

    let clicked = resp.clicked();
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if let Some(pct) = s.affinity {
        resp.on_hover_text(format!("Afinidad: {pct:.0}%"));
    }
    clicked
}

/// Calcula el offset al que llevar un carrusel al tocar una flecha (avanza
/// tres ítems por click).
fn dash_step(offset: f32, max_offset: f32, dir: i32) -> f32 {
    let step = (DASH_ITEM_W + DASH_ITEM_GAP) * 3.0;
    (offset + step * dir as f32).clamp(0.0, max_offset)
}

/// El tablero. Se dibuja sobre rectángulos absolutos: se reserva UNA vez el
/// alto total dentro de un scroll vertical y cada tarjeta se ubica a mano. Así
/// los widgets que se posicionan a mano (flechas, scrolls horizontales) no
/// mueven el cursor del layout y las tarjetas no se pisan.
fn dashboard(app: &mut App, ui: &mut egui::Ui, full: Rect, action: &mut Option<HomeAction>) {
    let palette = app.palette;
    let gap = theme::GAP as f32;
    let friends = app.home_friends_sorted();
    let servers = app.home_servers_sorted();
    let online = friends.iter().filter(|f| f.status != Status::Offline).count();
    let me = app
        .me
        .as_ref()
        .map(|m| (m.display_name().to_string(), m.username.clone(), m.avatar_url(), m.accent_color));
    let servers_ranked = !app.affinity_guilds.is_empty();

    let friends_h = DASH_PAD * 2.0 + DASH_HEAD_H + FRIEND_ROW_H + DASH_SCROLLBAR_ROOM;
    let servers_h = DASH_PAD * 2.0 + DASH_HEAD_H + SERVER_ROW_H + DASH_SCROLLBAR_ROOM;
    let total_h = DASH_HEADER_H + gap + friends_h + gap + servers_h;

    in_rect(ui, full, |ui| {
        ScrollArea::vertical()
            .id_salt("home_dashboard")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let (content, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), total_h), Sense::hover());
                let w = content.width();

                // --- 1. Perfil ---------------------------------------------
                let header = Rect::from_min_size(content.min, Vec2::new(w, DASH_HEADER_H));
                dash_header(ui, &palette, header, me.as_ref());

                // --- 2. Amigos ---------------------------------------------
                let card = Rect::from_min_size(
                    egui::pos2(content.min.x, header.max.y + gap),
                    Vec2::new(w, friends_h),
                );
                card_bg(ui, &palette, card);
                let inner = card.shrink(DASH_PAD);
                dash_title(
                    ui,
                    &palette,
                    inner.min,
                    "Amigos",
                    &format!("· {} · {} en línea", friends.len(), online),
                );
                let head = Rect::from_min_size(inner.min, Vec2::new(inner.width(), 30.0));
                let row = Rect::from_min_max(egui::pos2(inner.min.x, inner.min.y + DASH_HEAD_H), inner.max);
                if friends.is_empty() {
                    in_rect(ui, row, |ui| {
                        empty_card(ui, &palette, "Todavía no hay amigos por acá", "Cuando agregues amigos, aparecen en esta fila.");
                    });
                } else {
                    let req = app.home_scroll_req[0].take();
                    let (offset, max_offset) = dash_carousel(ui, "home_friends_row", row, friends.len(), req, |ui, i, item| {
                        if dash_friend_item(ui, &palette, &friends[i], item) {
                            *action = Some(HomeAction::OpenFriend(friends[i].index));
                        }
                    });
                    let (see_all, dir) =
                        dash_controls(ui, &palette, head, "friends", true, offset > 1.0, offset < max_offset - 1.0);
                    if see_all {
                        app.friends_tab = ALL_TAB;
                    }
                    if dir != 0 {
                        app.home_scroll_req[0] = Some(dash_step(offset, max_offset, dir));
                        ui.ctx().request_repaint();
                    }
                }

                // --- 3. Servidores frecuentes --------------------------------
                let card = Rect::from_min_size(
                    egui::pos2(content.min.x, card.max.y + gap),
                    Vec2::new(w, servers_h),
                );
                card_bg(ui, &palette, card);
                let inner = card.shrink(DASH_PAD);
                dash_title(
                    ui,
                    &palette,
                    inner.min,
                    if servers_ranked { "Servidores frecuentes" } else { "Tus servidores" },
                    "",
                );
                let head = Rect::from_min_size(inner.min, Vec2::new(inner.width(), 30.0));
                let row = Rect::from_min_max(egui::pos2(inner.min.x, inner.min.y + DASH_HEAD_H), inner.max);
                if servers.is_empty() {
                    in_rect(ui, row, |ui| {
                        empty_card(ui, &palette, "Todavía no estás en ningún servidor", "Cuando te unas a uno, aparece en esta fila.");
                    });
                } else {
                    let req = app.home_scroll_req[1].take();
                    let (offset, max_offset) = dash_carousel(ui, "home_servers_row", row, servers.len(), req, |ui, i, item| {
                        if dash_server_item(ui, &palette, &servers[i], item) {
                            *action = Some(HomeAction::OpenServer(servers[i].index));
                        }
                    });
                    let (_, dir) =
                        dash_controls(ui, &palette, head, "servers", false, offset > 1.0, offset < max_offset - 1.0);
                    if dir != 0 {
                        app.home_scroll_req[1] = Some(dash_step(offset, max_offset, dir));
                        ui.ctx().request_repaint();
                    }
                }
            });
    });
}
