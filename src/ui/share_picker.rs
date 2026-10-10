//! Selector de "Compartir pantalla": elegir qué transmitir antes de empezar,
//! con el mismo esquema que el de Discord: pestañas (Aplicaciones / Pantalla
//! completa / Dispositivos), una grilla de tarjetas con la miniatura en vivo de
//! cada ventana o pantalla, y abajo la calidad (resolución y fps) con su
//! engranaje.
//!
//! Lo abre el botón de la barra de llamada (`App::toggle_broadcast`) y, al
//! elegir una tarjeta, arranca la transmisión con ese origen
//! (`App::start_broadcast`). Las listas salen de
//! `discord::voice::{list_capture_windows, list_capture_monitors}` y las
//! miniaturas de `discord::voice::spawn_source_thumbnails` (un hilo aparte).
//! Mientras el selector está abierto se vuelve a leer todo cada pocos
//! segundos, así que las vistas previas y la lista se van actualizando solas.
//!
//! Lo que solo vive mientras el selector está abierto (texturas, pestaña,
//! engranaje abierto, canal de las miniaturas) va en la memoria temporal de
//! egui (`ShareThumbs`) y se descarta al cerrarlo. Lo que sí necesita el resto
//! de la app (la calidad elegida) está en `App::share_quality`.
//!
//! Mismo patrón que `ui::clean_mic_popup`: lo que hace falta de `App` se clona
//! antes de dibujar, el closure solo junta lo que tocó la persona y recién
//! después se aplica sobre `app`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Area, Color32, ColorImage, CornerRadius, Frame, Id, Layout, Margin, Order, Rect,
    RichText, ScrollArea, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, UiBuilder, Vec2,
    pos2, vec2,
};

use crate::discord::voice::{
    CaptureMonitor, CaptureTarget, CaptureWindow, StreamQuality, ThumbKey, Thumbnail,
    WINDOW_CAPTURE_SUPPORTED, spawn_source_thumbnails,
};
use crate::lib::state::App;
use crate::ui::theme::{self, Icon, Palette};

const CARD_MAX_WIDTH: f32 = 940.0;
const CARD_PADDING: f32 = 22.0;
/// Ancho mínimo de una tarjeta de la grilla (de ahí salen las columnas).
const TILE_MIN_WIDTH: f32 = 260.0;
const GRID_GAP: f32 = 14.0;
/// Alto de la zona de título/detalle bajo la miniatura.
const TILE_INFO_HEIGHT: f32 = 46.0;
/// Cada cuánto se vuelven a leer las listas y las miniaturas.
const REFRESH_EVERY: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Apps,
    Screens,
    /// Cámaras y capturadoras. Todavía no se pueden transmitir.
    Devices,
}

/// Estado del selector mientras está abierto.
#[derive(Default)]
struct ShareThumbs {
    textures: HashMap<ThumbKey, TextureHandle>,
    rx: Option<Receiver<Thumbnail>>,
    /// `true` mientras el hilo de miniaturas sigue trabajando.
    busy: Arc<AtomicBool>,
    last_run: Option<Instant>,
    tab: Tab,
    settings_open: bool,
}

type Shared = Arc<Mutex<ShareThumbs>>;

enum Action {
    Close,
    Refresh,
    Start(CaptureTarget),
    SetTab(Tab),
    ToggleSettings,
    SetHeight(u32),
    SetFps(u32),
}

/// Una tarjeta de la grilla.
struct Tile {
    key: ThumbKey,
    title: String,
    detail: String,
    icon: Icon,
    enabled: bool,
    target: CaptureTarget,
}

fn shared_state(ctx: &egui::Context) -> Shared {
    let id = Id::new("share_picker_thumbs");
    ctx.data_mut(|data| {
        data.get_temp_mut_or_insert_with(id, || Arc::new(Mutex::new(ShareThumbs::default())))
            .clone()
    })
}

fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, ShareThumbs> {
    shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let seen_id = Id::new("share_picker_seen");
    // Si la llamada terminó (o ya se está transmitiendo) con el selector
    // abierto, no hay nada que elegir.
    if app.share_picker_open && (app.voice_target.is_none() || app.is_broadcasting()) {
        app.share_picker_open = false;
    }
    if !app.share_picker_open {
        ctx.data_mut(|d| {
            d.insert_temp(seen_id, false);
            // Suelta las texturas y el hilo de miniaturas.
            d.remove_temp::<Shared>(Id::new("share_picker_thumbs"));
        });
        return;
    }
    // El clic que abrió el selector cae fuera de la tarjeta (en el botón de la
    // barra): en ese primer frame no cuenta como "clic en el fondo".
    let seen_before = ctx.data(|d| d.get_temp::<bool>(seen_id)).unwrap_or(false);
    ctx.data_mut(|d| d.insert_temp(seen_id, true));

    let shared = shared_state(&ctx);
    if !seen_before {
        *lock(&shared) = ShareThumbs::default();
    }
    tick(app, &ctx, &shared);

    let palette = app.palette;
    let windows = app.share_picker_windows.clone();
    let monitors = app.share_picker_monitors.clone();
    let quality = app.share_quality;
    let (tab, settings_open, textures) = {
        let state = lock(&shared);
        (state.tab, state.settings_open, state.textures.clone())
    };
    let screen_rect = ctx.viewport_rect();
    let mut action: Option<Action> = None;

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = Some(Action::Close);
    }

    Area::new(Id::new("share_picker_scrim"))
        .order(Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(&ctx, |ui| {
            ui.set_width(screen_rect.width());
            ui.set_height(screen_rect.height());
            ui.painter().rect_filled(screen_rect, 0.0, Color32::from_black_alpha(150));
            // Se registra antes que la tarjeta: los widgets de la tarjeta ganan
            // el clic, y un clic en el fondo oscuro cierra el selector.
            let scrim = ui.interact(screen_rect, Id::new("share_picker_scrim_block"), Sense::click());

            let width = CARD_MAX_WIDTH.min(screen_rect.width() - 40.0);
            let top = (screen_rect.top() + screen_rect.height() * 0.08).max(screen_rect.top() + 24.0);
            let card_area = Rect::from_min_max(
                pos2(screen_rect.center().x - width / 2.0, top),
                pos2(screen_rect.center().x + width / 2.0, screen_rect.bottom() - 24.0),
            );
            // Alto de la grilla: lo que queda de la tarjeta sin el encabezado,
            // las pestañas y el pie (con el panel del engranaje si está abierto).
            let chrome = 250.0 + if settings_open { 120.0 } else { 0.0 };
            let grid_max_height = (card_area.height() - chrome).clamp(160.0, 640.0);

            let mut card_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(card_area)
                    .layout(Layout::top_down(Align::Min)),
            );
            let card = Frame::new()
                .fill(palette.overlay)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(theme::radius() + 6))
                .inner_margin(Margin::same(CARD_PADDING as i8))
                .shadow(egui::epaint::Shadow { offset: [0, 16], blur: 40, spread: 0, color: palette.shadow })
                .show(&mut card_ui, |ui| {
                    ui.set_width(width - CARD_PADDING * 2.0);

                    ui.horizontal(|ui| {
                        theme::text(ui, "Compartir pantalla", theme::bold(19.0), palette.text);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if theme::icon_button(ui, Icon::X, 15.0, palette.dim, palette.text, "Cerrar").clicked() {
                                action = Some(Action::Close);
                            }
                            if WINDOW_CAPTURE_SUPPORTED
                                && theme::icon_button(ui, Icon::Refresh, 14.0, palette.dim, palette.text, "Actualizar")
                                    .clicked()
                            {
                                action = Some(Action::Refresh);
                            }
                        });
                    });
                    ui.add_space(12.0);

                    if let Some(picked) = tab_strip(ui, &palette, tab) {
                        action = Some(Action::SetTab(picked));
                    }
                    ui.add_space(14.0);

                    let tiles = match tab {
                        Tab::Apps => window_tiles(&windows),
                        Tab::Screens => screen_tiles(&monitors),
                        Tab::Devices => Vec::new(),
                    };
                    if tiles.is_empty() {
                        empty_hint(ui, &palette, tab);
                    } else {
                        ScrollArea::vertical()
                            .max_height(grid_max_height)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                if let Some(target) = tile_grid(ui, &palette, &tiles, &textures) {
                                    action = Some(Action::Start(target));
                                }
                            });
                    }

                    ui.add_space(16.0);
                    if let Some(next) = quality_footer(ui, &palette, quality, settings_open) {
                        action = Some(next);
                    }
                });

            let card_rect = card.response.rect;
            let clicked_outside = seen_before
                && scrim.clicked()
                && ctx
                    .input(|i| i.pointer.interact_pos())
                    .is_some_and(|pos| !card_rect.contains(pos));
            if clicked_outside {
                action = Some(Action::Close);
            }
        });

    match action {
        Some(Action::Close) => app.share_picker_open = false,
        Some(Action::Refresh) => lock(&shared).last_run = None,
        Some(Action::Start(target)) => app.start_broadcast(target),
        Some(Action::SetTab(next)) => {
            let mut state = lock(&shared);
            state.tab = next;
            // Trae ya las miniaturas de la pestaña nueva.
            state.last_run = None;
        }
        Some(Action::ToggleSettings) => {
            let mut state = lock(&shared);
            state.settings_open = !state.settings_open;
        }
        Some(Action::SetHeight(height)) => app.share_quality.height = height,
        Some(Action::SetFps(fps)) => app.share_quality.fps = fps,
        None => {}
    }
}

/// Una vez por frame: sube las miniaturas que llegaron y, cada
/// `REFRESH_EVERY` (si el hilo anterior ya terminó), vuelve a leer las listas
/// y lanza otra tanda de miniaturas de la pestaña visible.
fn tick(app: &mut App, ctx: &egui::Context, shared: &Shared) {
    let mut state = lock(shared);

    let arrived: Vec<Thumbnail> = state
        .rx
        .as_ref()
        .map(|rx| rx.try_iter().collect())
        .unwrap_or_default();
    for thumbnail in arrived {
        let image = ColorImage::from_rgba_unmultiplied([thumbnail.width, thumbnail.height], &thumbnail.rgba);
        let texture = ctx.load_texture(format!("share_thumb_{:?}", thumbnail.key), image, TextureOptions::LINEAR);
        state.textures.insert(thumbnail.key, texture);
    }

    let idle = !state.busy.load(std::sync::atomic::Ordering::Relaxed);
    if idle && state.last_run.is_none_or(|at| at.elapsed() >= REFRESH_EVERY) {
        app.refresh_share_windows();
        // Texturas de ventanas o pantallas que ya no están.
        let alive: HashSet<ThumbKey> = window_keys(&app.share_picker_windows)
            .into_iter()
            .chain(monitor_keys(&app.share_picker_monitors))
            .collect();
        state.textures.retain(|key, _| alive.contains(key));
        state.last_run = Some(Instant::now());

        let wanted = match state.tab {
            Tab::Apps => window_keys(&app.share_picker_windows),
            Tab::Screens => monitor_keys(&app.share_picker_monitors),
            Tab::Devices => Vec::new(),
        };
        if !wanted.is_empty() {
            let (tx, rx) = mpsc::channel();
            let repaint_ctx = ctx.clone();
            spawn_source_thumbnails(wanted, tx, Arc::clone(&state.busy), move || repaint_ctx.request_repaint());
            state.rx = Some(rx);
        }
    }
    // Para que el refresco llegue aunque la persona no mueva el mouse.
    ctx.request_repaint_after(Duration::from_millis(500));
}

fn window_keys(windows: &[CaptureWindow]) -> Vec<ThumbKey> {
    windows
        .iter()
        // Una ventana minimizada no tiene contenido que mostrar.
        .filter(|window| !window.minimized)
        .map(|window| ThumbKey::Window(window.hwnd))
        .collect()
}

fn monitor_keys(monitors: &[CaptureMonitor]) -> Vec<ThumbKey> {
    monitors.iter().map(monitor_key).collect()
}

fn monitor_key(monitor: &CaptureMonitor) -> ThumbKey {
    ThumbKey::Monitor { x: monitor.x, y: monitor.y, width: monitor.width, height: monitor.height }
}

fn window_tiles(windows: &[CaptureWindow]) -> Vec<Tile> {
    windows
        .iter()
        .map(|window| Tile {
            key: ThumbKey::Window(window.hwnd),
            title: window.title.clone(),
            detail: if window.minimized {
                "Minimizada: restaurala para transmitirla".to_owned()
            } else {
                window.app.clone().unwrap_or_else(|| "Ventana".to_owned())
            },
            icon: Icon::Laptop,
            enabled: !window.minimized,
            target: CaptureTarget::Window { title: window.title.clone(), hwnd: window.hwnd },
        })
        .collect()
}

fn screen_tiles(monitors: &[CaptureMonitor]) -> Vec<Tile> {
    if monitors.is_empty() {
        // Sin lista de pantallas (Linux/macOS): una sola opción, el escritorio.
        return vec![Tile {
            key: ThumbKey::Monitor { x: 0, y: 0, width: 0, height: 0 },
            title: "Toda la pantalla".to_owned(),
            detail: "Escritorio principal".to_owned(),
            icon: Icon::Monitor,
            enabled: true,
            target: CaptureTarget::Desktop,
        }];
    }
    monitors
        .iter()
        .map(|monitor| Tile {
            key: monitor_key(monitor),
            title: monitor.label.clone(),
            detail: if monitor.primary {
                format!("{}×{} · Principal", monitor.width, monitor.height)
            } else {
                format!("{}×{}", monitor.width, monitor.height)
            },
            icon: Icon::Monitor,
            enabled: true,
            target: CaptureTarget::Monitor {
                label: monitor.label.clone(),
                x: monitor.x,
                y: monitor.y,
                width: monitor.width,
                height: monitor.height,
            },
        })
        .collect()
}

fn empty_hint(ui: &mut egui::Ui, palette: &Palette, tab: Tab) {
    let text = match tab {
        Tab::Apps if !WINDOW_CAPTURE_SUPPORTED => "Transmitir una sola ventana todavía solo funciona en Windows.",
        Tab::Apps => "No se encontró ninguna ventana abierta.",
        Tab::Screens => "No se encontró ninguna pantalla.",
        Tab::Devices => "Transmitir una cámara o capturadora todavía no está disponible.",
    };
    ui.add_space(10.0);
    hint(ui, palette, text);
    ui.add_space(10.0);
}

fn hint(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.label(RichText::new(text).font(theme::regular(12.0)).color(palette.dim));
}

/// Pestañas (una franja con tres segmentos). Devuelve la que se tocó.
fn tab_strip(ui: &mut egui::Ui, palette: &Palette, current: Tab) -> Option<Tab> {
    let tabs = [
        (Tab::Apps, Icon::Laptop, "Aplicaciones", true),
        (Tab::Screens, Icon::Monitor, "Pantalla completa", true),
        (Tab::Devices, Icon::Tv, "Dispositivos", false),
    ];
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::hover());
    ui.painter().rect_filled(strip, CornerRadius::same(theme::radius()), palette.surface);

    let segment_width = (strip.width() - 8.0) / tabs.len() as f32;
    let mut picked = None;
    for (index, (tab, icon, label, enabled)) in tabs.into_iter().enumerate() {
        let segment = Rect::from_min_size(
            pos2(strip.left() + 4.0 + index as f32 * segment_width, strip.top() + 4.0),
            vec2(segment_width, strip.height() - 8.0),
        );
        let sense = if enabled { Sense::click() } else { Sense::hover() };
        let response = ui.interact(segment, Id::new(("share_picker_tab", index)), sense);
        if tab == current {
            ui.painter().rect_filled(segment, CornerRadius::same(theme::radius()), palette.surface_active);
        } else if enabled && response.hovered() {
            ui.painter().rect_filled(segment, CornerRadius::same(theme::radius()), palette.surface_hover);
        }

        let color = if enabled { palette.text } else { palette.dim };
        let galley = ui.painter().layout_no_wrap(label.to_owned(), theme::semibold(14.0), color);
        let total = 18.0 + 8.0 + galley.size().x;
        let start = segment.center().x - total / 2.0;
        let icon_rect = Rect::from_center_size(pos2(start + 9.0, segment.center().y), Vec2::splat(18.0));
        theme::paint_icon(ui, icon, icon_rect, 17.0, color);
        ui.painter().galley(pos2(start + 26.0, segment.center().y - galley.size().y / 2.0), galley, color);

        if enabled {
            if response.clicked() {
                picked = Some(tab);
            }
        } else {
            response.on_hover_text("Todavía no disponible");
        }
    }
    picked
}

/// Grilla de tarjetas. Devuelve el origen de la que se tocó.
fn tile_grid(
    ui: &mut egui::Ui,
    palette: &Palette,
    tiles: &[Tile],
    textures: &HashMap<ThumbKey, TextureHandle>,
) -> Option<CaptureTarget> {
    let available = ui.available_width();
    let columns = (((available + GRID_GAP) / (TILE_MIN_WIDTH + GRID_GAP)).floor() as usize).clamp(1, 4);
    let tile_width = (available - GRID_GAP * (columns - 1) as f32) / columns as f32;
    let mut chosen = None;
    for row in tiles.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GRID_GAP;
            for tile in row {
                if tile_ui(ui, palette, tile, tile_width, textures.get(&tile.key)) {
                    chosen = Some(tile.target.clone());
                }
            }
        });
        ui.add_space(GRID_GAP);
    }
    chosen
}

/// Una tarjeta: miniatura 16:9 arriba y, debajo, ícono + título + detalle.
/// `true` si se la tocó.
fn tile_ui(
    ui: &mut egui::Ui,
    palette: &Palette,
    tile: &Tile,
    width: f32,
    texture: Option<&TextureHandle>,
) -> bool {
    let thumb_height = (width * 9.0 / 16.0).round();
    let sense = if tile.enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(width, thumb_height + TILE_INFO_HEIGHT), sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, tile.enabled, &tile.title));
    if !ui.is_rect_visible(rect) {
        return false;
    }

    let radius = CornerRadius::same(8);
    let thumb = Rect::from_min_size(rect.min, vec2(width, thumb_height));
    let painter = ui.painter();
    painter.rect_filled(thumb, radius, palette.surface);
    match texture {
        Some(texture) => {
            // Entra completa en el 16:9 aunque la ventana tenga otra proporción.
            let size = texture.size_vec2();
            let scale = (thumb.width() / size.x).min(thumb.height() / size.y);
            let fitted = Rect::from_center_size(thumb.center(), size * scale);
            let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            painter.image(texture.id(), fitted, whole, Color32::WHITE);
        }
        None => {
            let icon_rect = Rect::from_center_size(thumb.center(), Vec2::splat(34.0));
            theme::paint_icon(ui, tile.icon, icon_rect, 30.0, palette.dim);
        }
    }
    let painter = ui.painter();
    if !tile.enabled {
        painter.rect_filled(thumb, radius, Color32::from_black_alpha(140));
    } else if response.hovered() {
        painter.rect_stroke(thumb, radius, Stroke::new(2.0, palette.accent), StrokeKind::Inside);
    }

    let color = if tile.enabled { palette.text } else { palette.dim };
    let icon_rect = Rect::from_center_size(pos2(rect.left() + 11.0, thumb.bottom() + 16.0), Vec2::splat(18.0));
    theme::paint_icon(ui, tile.icon, icon_rect, 16.0, color);

    let text_x = rect.left() + 28.0;
    let max_width = (rect.right() - text_x - 4.0).max(20.0);
    let title_font = theme::semibold(13.0);
    let detail_font = theme::regular(12.0);
    let title = fit_width(ui, &tile.title, &title_font, max_width);
    let detail = fit_width(ui, &tile.detail, &detail_font, max_width);
    let painter = ui.painter();
    painter.text(pos2(text_x, thumb.bottom() + 16.0), Align2::LEFT_CENTER, title, title_font, color);
    painter.text(pos2(text_x, thumb.bottom() + 33.0), Align2::LEFT_CENTER, detail, detail_font, palette.dim);

    if tile.enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    } else {
        false
    }
}

/// Pie: calidad actual a la izquierda y el engranaje que despliega la
/// resolución y los fps. Devuelve lo que la persona tocó.
fn quality_footer(ui: &mut egui::Ui, palette: &Palette, quality: StreamQuality, open: bool) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            theme::text(ui, "Calidad", theme::bold(16.0), palette.text);
            ui.label(RichText::new(quality.label()).font(theme::regular(13.0)).color(palette.dim));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let color = if open { palette.accent } else { palette.text };
            if theme::icon_button(ui, Icon::Settings, 18.0, color, palette.accent_hover, "Ajustes de calidad").clicked() {
                action = Some(Action::ToggleSettings);
            }
        });
    });
    if open {
        ui.add_space(10.0);
        Frame::new()
            .fill(palette.surface)
            .corner_radius(CornerRadius::same(theme::radius()))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.add_sized(vec2(96.0, 28.0), egui::Label::new(
                        RichText::new("Resolución").font(theme::semibold(13.0)).color(palette.secondary),
                    ));
                    for height in StreamQuality::HEIGHTS {
                        if chip(ui, palette, &format!("{height}p"), quality.height == height).clicked() {
                            action = Some(Action::SetHeight(height));
                        }
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_sized(vec2(96.0, 28.0), egui::Label::new(
                        RichText::new("Fotogramas").font(theme::semibold(13.0)).color(palette.secondary),
                    ));
                    for fps in StreamQuality::FPS {
                        if chip(ui, palette, &format!("{fps} fps"), quality.fps == fps).clicked() {
                            action = Some(Action::SetFps(fps));
                        }
                    }
                });
                ui.add_space(8.0);
                hint(
                    ui,
                    palette,
                    "Más resolución y fps piden más del equipo, y Discord puede limitarlos según tu cuenta.",
                );
            });
    }
    action
}

/// Botón de opción: relleno de acento si es la elegida.
fn chip(ui: &mut egui::Ui, palette: &Palette, label: &str, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(74.0, 28.0), Sense::click());
    let (fill, text) = if selected {
        (palette.accent, palette.on_accent)
    } else if response.hovered() {
        (palette.surface_hover, palette.text)
    } else {
        (palette.surface_active, palette.text)
    };
    ui.painter().rect_filled(rect, CornerRadius::same(theme::radius()), fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, theme::semibold(13.0), text);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Recorta `text` con "…" hasta que entre en `max_width` (búsqueda binaria
/// sobre la cantidad de caracteres: un título largo no cuesta decenas de
/// medidas por frame).
fn fit_width(ui: &egui::Ui, text: &str, font: &egui::FontId, max_width: f32) -> String {
    let measure = |candidate: &str| {
        ui.painter()
            .layout_no_wrap(candidate.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x
    };
    if measure(text) <= max_width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    // Mayor cantidad de caracteres que, con "…" al final, todavía entra.
    let (mut low, mut high) = (0usize, chars.len());
    while low < high {
        let mid = (low + high).div_ceil(2);
        let mut candidate: String = chars[..mid].iter().collect();
        candidate.push('…');
        if measure(&candidate) <= max_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let mut out: String = chars[..low].iter().collect();
    out.push('…');
    out
}
