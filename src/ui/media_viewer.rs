//! Visor multimedia a pantalla completa (como el de Discord) para las
//! imágenes y GIFs de los mensajes.
//!
//! * Fondo oscuro; click fuera de la imagen, `Esc` o la cruz lo cierran.
//! * Barra de opciones arriba a la derecha con el diseño de los controles de
//!   la transmisión (`ui::call_view`): píldora translúcida con borde y
//!   botones redondos, y la cruz en su propio círculo oscuro. Acercar,
//!   compartir (copia el enlace), descargar, abrir en el navegador y un menú
//!   "más opciones".
//! * Rueda del mouse = zoom hacia el cursor, arrastrar = mover, doble click =
//!   alternar zoom, `+`/`-`/`0` desde el teclado.
//! * Si el mensaje trae varias imágenes: flechas a los lados (o `←`/`→`) y
//!   contador "2 / 4".
//! * Descargar guarda el archivo en la carpeta Descargas, en un hilo aparte,
//!   y avisa con un toast.
//!
//! El estado vive en un `thread_local` (la UI de egui corre en un solo hilo),
//! así `ui::media` abre el visor con `open_*` sin pasar nada por la cadena de
//! funciones del chat. `show` se llama una vez por frame al final de
//! `App::ui` (ver `lib/state.rs`).

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use egui::{
    Align2, Color32, Id, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2,
};

use crate::discord::models::Attachment;
use crate::lib::state::{App, ToastKind};
use crate::ui::extra;
use crate::ui::theme::{self, Icon, Palette};

const MAX_ZOOM: f32 = 12.0;
const BTN: f32 = 32.0;
const PILL_H: f32 = 44.0;
const CLOSE_D: f32 = 40.0;
const EDGE: f32 = 16.0;

/// Una imagen para mostrar en el visor.
#[derive(Clone, Debug)]
pub struct Item {
    /// URL original (la del CDN, no la del proxy reducido).
    pub url: String,
    pub name: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// GIF de Tenor/Giphy (`gifv`): el mp4 corto que se reproduce en bucle.
    /// `url` queda como portada mientras carga.
    pub video: Option<String>,
    /// Enlace "humano" (la página del GIF) para copiar / abrir en el navegador.
    pub link: Option<String>,
}

impl Item {
    pub fn new(url: &str, name: &str, width: Option<u32>, height: Option<u32>) -> Self {
        let name = if name.is_empty() { file_name_from_url(url) } else { name.to_string() };
        Self { url: url.to_string(), name, width, height, video: None, link: None }
    }

    /// Lo que se copia / abre en el navegador.
    fn share_url(&self) -> &str {
        self.link.as_deref().or(self.video.as_deref()).unwrap_or(&self.url)
    }

    /// Lo que se descarga (el mp4 en los `gifv`).
    fn download_url(&self) -> &str {
        self.video.as_deref().unwrap_or(&self.url)
    }
}

struct Viewer {
    items: Vec<Item>,
    index: usize,
    /// 1.0 = ajustada a la ventana.
    zoom: f32,
    pan: Vec2,
    menu: bool,
    download: Option<Receiver<Result<PathBuf, String>>>,
    /// Reproductor del `gifv` que se está mostrando (índice del item + motor).
    engine: Option<(usize, egui_video::Engine)>,
}

thread_local! {
    static VIEWER: RefCell<Option<Viewer>> = const { RefCell::new(None) };
}

// ---------------------------------------------------------------------
// Abrir / cerrar
// ---------------------------------------------------------------------

/// Abre el visor con `items`, mostrando el de posición `index`.
pub fn open(ctx: &egui::Context, items: Vec<Item>, index: usize) {
    if items.is_empty() {
        return;
    }
    let index = index.min(items.len() - 1);
    // Que el foco no se quede en la caja de texto del chat (el visor usa
    // flechas y +/-).
    ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
    VIEWER.with_borrow_mut(|v| {
        *v = Some(Viewer { items, index, zoom: 1.0, pan: Vec2::ZERO, menu: false, download: None, engine: None });
    });
    ctx.request_repaint();
}

/// Abre una sola imagen.
pub fn open_single(ctx: &egui::Context, url: &str, name: &str, width: Option<u32>, height: Option<u32>) {
    open(ctx, vec![Item::new(url, name, width, height)], 0);
}

/// Abre un GIF de Tenor/Giphy (`gifv`): `poster` es la portada estática,
/// `video` el mp4 que se repite en bucle y `link` la página original.
pub fn open_video_gif(
    ctx: &egui::Context,
    poster: &str,
    video: &str,
    link: &str,
    width: Option<u32>,
    height: Option<u32>,
) {
    let mut item = Item::new(poster, &file_name_from_url(video), width, height);
    item.video = Some(video.to_string());
    if link.starts_with("http") {
        item.link = Some(link.to_string());
    }
    open(ctx, vec![item], 0);
}

/// Abre los adjuntos de imagen de un mensaje en la posición `index`.
pub fn open_attachments(ctx: &egui::Context, images: &[&Attachment], index: usize) {
    let items = images
        .iter()
        .map(|a| Item::new(&a.url, &a.filename, a.width, a.height))
        .collect();
    open(ctx, items, index);
}

pub fn is_open() -> bool {
    VIEWER.with_borrow(|v| v.is_some())
}

pub fn close() {
    VIEWER.with_borrow_mut(|v| *v = None);
}

// ---------------------------------------------------------------------
// Dibujo
// ---------------------------------------------------------------------

enum Act {
    Close,
    Prev,
    Next,
    ToggleZoom,
    ZoomBy(f32),
    ResetZoom,
    CopyLink,
    CopyName,
    Download,
    OpenBrowser,
    ToggleMenu,
}

/// Dibuja el visor si hay uno abierto. Una vez por frame, al final de
/// `App::ui` (antes de `video_player::show_fullscreen`).
pub fn show(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let palette = app.palette;
    let mut toasts: Vec<(ToastKind, String, String)> = Vec::new();

    VIEWER.with_borrow_mut(|slot| {
        let Some(v) = slot.as_mut() else { return };

        // Resultado de la descarga en curso.
        if let Some(rx) = &v.download {
            match rx.try_recv() {
                Ok(Ok(path)) => {
                    toasts.push((ToastKind::Success, "Descarga completa".into(), path.display().to_string()));
                    v.download = None;
                }
                Ok(Err(e)) => {
                    toasts.push((ToastKind::Warning, "No se pudo descargar".into(), e));
                    v.download = None;
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
                Err(TryRecvError::Disconnected) => v.download = None,
            }
        }

        let mut acts: Vec<Act> = Vec::new();
        let screen = ctx.viewport_rect();
        let appear = ctx.animate_bool_with_time(Id::new("ecord_viewer_appear"), true, 0.12);

        egui::Area::new(Id::new("ecord_media_viewer"))
            .order(Order::Tooltip)
            .fixed_pos(screen.min)
            .show(&ctx, |ui| {
                ui.set_width(screen.width());
                ui.set_height(screen.height());
                draw(ui, &palette, v, screen, appear, &mut acts);
            });

        // Esc: primero el menú, después el visor.
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if esc {
            if v.menu {
                v.menu = false;
            } else {
                acts.push(Act::Close);
            }
        }
        if ctx.memory(|m| m.focused()).is_none() {
            let (left, right, plus, minus, zero) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowLeft),
                    i.key_pressed(egui::Key::ArrowRight),
                    i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals),
                    i.key_pressed(egui::Key::Minus),
                    i.key_pressed(egui::Key::Num0),
                )
            });
            if left {
                acts.push(Act::Prev);
            }
            if right {
                acts.push(Act::Next);
            }
            if plus {
                acts.push(Act::ZoomBy(1.25));
            }
            if minus {
                acts.push(Act::ZoomBy(0.8));
            }
            if zero {
                acts.push(Act::ResetZoom);
            }
        }

        let mut close_now = false;
        for act in acts {
            match act {
                Act::Close => close_now = true,
                Act::Prev => go(v, -1),
                Act::Next => go(v, 1),
                Act::ToggleZoom => {
                    if v.zoom > 1.01 {
                        v.zoom = 1.0;
                        v.pan = Vec2::ZERO;
                    } else {
                        v.zoom = 2.5;
                    }
                }
                Act::ZoomBy(f) => {
                    v.zoom = (v.zoom * f).clamp(1.0, MAX_ZOOM);
                    if v.zoom <= 1.0 {
                        v.pan = Vec2::ZERO;
                    }
                }
                Act::ResetZoom => {
                    v.zoom = 1.0;
                    v.pan = Vec2::ZERO;
                    v.menu = false;
                }
                Act::CopyLink => {
                    ctx.copy_text(v.items[v.index].share_url().to_string());
                    toasts.push((ToastKind::Info, "Enlace copiado".into(), String::new()));
                    v.menu = false;
                }
                Act::CopyName => {
                    ctx.copy_text(v.items[v.index].name.clone());
                    toasts.push((ToastKind::Info, "Nombre copiado".into(), String::new()));
                    v.menu = false;
                }
                Act::Download => {
                    if v.download.is_none() {
                        let item = v.items[v.index].clone();
                        v.download = Some(spawn_download(&ctx, item));
                        toasts.push((ToastKind::Info, "Descargando…".into(), v.items[v.index].name.clone()));
                    }
                    v.menu = false;
                }
                Act::OpenBrowser => {
                    let url = v.items[v.index].share_url();
                    if url.starts_with("https://") || url.starts_with("http://") {
                        ctx.open_url(egui::OpenUrl::new_tab(url));
                    }
                    v.menu = false;
                }
                Act::ToggleMenu => v.menu = !v.menu,
            }
        }
        if close_now {
            *slot = None;
            ctx.request_repaint();
        }
    });

    for (kind, title, msg) in toasts {
        app.push_toast(kind, title, msg);
    }
}

fn go(v: &mut Viewer, delta: i32) {
    let next = v.index as i32 + delta;
    if next >= 0 && (next as usize) < v.items.len() {
        v.index = next as usize;
        v.zoom = 1.0;
        v.pan = Vec2::ZERO;
        v.menu = false;
    }
}

fn draw(ui: &mut Ui, palette: &Palette, v: &mut Viewer, screen: Rect, appear: f32, acts: &mut Vec<Act>) {
    let ctx = ui.ctx().clone();
    let item = v.items[v.index].clone();

    // Fondo + click fuera de la imagen para cerrar (se registra primero:
    // lo que se registra después, encima, le gana el click).
    ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(238).gamma_multiply(appear));
    let backdrop = ui.interact(screen, Id::new("ecord_viewer_backdrop"), Sense::click_and_drag());
    if backdrop.clicked() {
        if v.menu {
            v.menu = false;
        } else {
            acts.push(Act::Close);
        }
    }

    // --- imagen -----------------------------------------------------
    let multi = v.items.len() > 1;
    let side_pad = if multi { 76.0 } else { 28.0 };
    let avail = Rect::from_min_max(
        Pos2::new(screen.left() + side_pad, screen.top() + 76.0),
        Pos2::new(screen.right() - side_pad, screen.bottom() - 64.0),
    );
    // GIF de Tenor/Giphy: se reproduce el mp4 en bucle (sin audio); mientras
    // no llega el primer cuadro se ve la portada estática.
    if let Some(video) = item.video.as_deref() {
        if v.engine.as_ref().map(|(i, _)| *i) != Some(v.index) {
            // Desde el archivo local: si el GIF ya se vio en el chat no hay
            // pedido nuevo a la red (ver `video_player::local_video`).
            if let Some(path) = crate::ui::video_player::local_video(&ctx, video) {
                v.engine = Some((v.index, egui_video::Engine::open(&ctx, &path, None, true)));
            }
        }
    } else {
        v.engine = None;
    }
    let video_tex = match v.engine.as_mut() {
        Some((_, e)) => {
            e.update(&ctx);
            if !e.failed() && e.has_frame() { e.size().map(|s| (e.texture_id(), s)) } else { None }
        }
        None => None,
    };

    let (image, poll): (egui::Image<'static>, egui::load::Result<egui::load::TexturePoll>) =
        if let Some((tid, size)) = video_tex {
            let tex = egui::load::SizedTexture::new(tid, size);
            (
                egui::Image::new(tex).show_loading_spinner(false),
                Ok(egui::load::TexturePoll::Ready { texture: tex }),
            )
        } else {
            let source = crate::ui::anim::source_sized(&ctx, &item.url, 1600);
            let image = egui::Image::new(source).show_loading_spinner(false);
            let poll = image.load_for_size(&ctx, avail.size());
            (image, poll)
        };

    let natural = match (video_tex, item.width, item.height) {
        (Some((_, s)), _, _) => Some(s),
        (None, Some(w), Some(h)) if w > 0 && h > 0 => Some(Vec2::new(w as f32, h as f32)),
        _ => match &poll {
            Ok(egui::load::TexturePoll::Ready { texture }) => Some(texture.size),
            _ => None,
        },
    };

    match (&poll, natural) {
        (Ok(egui::load::TexturePoll::Ready { .. }), Some(nat)) => {
            let fit = (avail.width() / nat.x).min(avail.height() / nat.y).min(1.0).max(0.01);
            let size = nat * fit * v.zoom;

            // Mover: la imagen no puede salirse de la zona visible.
            let max_pan = Vec2::new(((size.x - avail.width()) / 2.0).max(0.0), ((size.y - avail.height()) / 2.0).max(0.0));
            v.pan = Vec2::new(v.pan.x.clamp(-max_pan.x, max_pan.x), v.pan.y.clamp(-max_pan.y, max_pan.y));
            let rect = Rect::from_center_size(avail.center() + v.pan, size);

            // La zona interactiva es la imagen recortada a lo visible, para
            // que el click en el fondo (aunque la imagen esté ampliada
            // detrás de la barra) siga cerrando.
            let hit = rect.intersect(screen);
            let resp = ui.interact(hit, Id::new("ecord_viewer_image"), Sense::click_and_drag());
            if v.zoom > 1.01 {
                ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
            }
            if resp.dragged() {
                v.pan += resp.drag_delta();
            }
            if resp.double_clicked() {
                acts.push(Act::ToggleZoom);
            }

            // Rueda: zoom hacia el cursor.
            if let Some(p) = ctx.pointer_hover_pos() {
                if hit.contains(p) && ctx.layer_id_at(p).is_some_and(|l| l == ui.layer_id()) {
                    let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
                    if scroll.abs() > 0.1 {
                        let old = v.zoom;
                        let new = (old * (scroll * 0.0018).exp()).clamp(1.0, MAX_ZOOM);
                        if (new - old).abs() > f32::EPSILON {
                            let rel = p - (avail.center() + v.pan);
                            v.pan += rel - rel * (new / old);
                            v.zoom = new;
                            if v.zoom <= 1.001 {
                                v.pan = Vec2::ZERO;
                            }
                        }
                    }
                }
            }

            let clip = screen;
            ui.scope_builder(UiBuilder::new().max_rect(screen), |ui| {
                ui.set_clip_rect(clip);
                image.tint(Color32::WHITE.gamma_multiply(appear)).paint_at(ui, rect);
            });
        }
        (Ok(_), _) => {
            // Cargando.
            egui::Spinner::new()
                .size(34.0)
                .color(Color32::WHITE)
                .paint_at(ui, Rect::from_center_size(avail.center(), Vec2::splat(34.0)));
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        (Err(_), _) => {
            ui.painter().text(
                avail.center() - Vec2::new(0.0, 14.0),
                Align2::CENTER_CENTER,
                "No se pudo cargar la imagen",
                theme::regular(14.0),
                palette.secondary,
            );
            let btn = Rect::from_center_size(avail.center() + Vec2::new(0.0, 22.0), Vec2::new(190.0, 32.0));
            if pill_text_button(ui, Id::new("viewer_err_open"), btn, "Abrir en el navegador", palette) {
                acts.push(Act::OpenBrowser);
            }
        }
    }

    // --- flechas de la galería ----------------------------------------
    if multi {
        let dark = Color32::from_black_alpha(150);
        let darker = Color32::from_black_alpha(210);
        let cy = screen.center().y;
        if v.index > 0 {
            let r = Rect::from_center_size(Pos2::new(screen.left() + 38.0, cy), Vec2::splat(44.0));
            if round_button(ui, Id::new("viewer_prev"), r, Icon::ChevronLeft, 22.0, dark, darker, Color32::WHITE, "Anterior") {
                acts.push(Act::Prev);
            }
        }
        if v.index + 1 < v.items.len() {
            let r = Rect::from_center_size(Pos2::new(screen.right() - 38.0, cy), Vec2::splat(44.0));
            if round_button(ui, Id::new("viewer_next"), r, Icon::ChevronRight, 22.0, dark, darker, Color32::WHITE, "Siguiente") {
                acts.push(Act::Next);
            }
        }
    }

    // --- pie: nombre del archivo, tamaño y contador -------------------
    {
        let mut label = item.name.clone();
        if item.video.is_some() {
            label.push_str("  ·  GIF");
        }
        if let (Some(w), Some(h)) = (item.width, item.height) {
            label.push_str(&format!("  ·  {w}×{h}"));
        }
        if multi {
            label.push_str(&format!("  ·  {} / {}", v.index + 1, v.items.len()));
        }
        let font = theme::semibold(12.5);
        let max_w = (screen.width() - 2.0 * (side_pad + 24.0)).max(80.0);
        let (text, w) = fit_text(ui, &label, &font, max_w);
        let chip = Rect::from_center_size(Pos2::new(screen.center().x, screen.bottom() - 32.0), Vec2::new(w + 28.0, 28.0));
        ui.painter().rect_filled(chip, 14.0, Color32::from_black_alpha(150));
        ui.painter().text(chip.center(), Align2::CENTER_CENTER, text, font, Color32::WHITE);
    }

    // --- barra de opciones (arriba a la derecha) ----------------------
    let close_c = Pos2::new(screen.right() - EDGE - CLOSE_D / 2.0, screen.top() + EDGE + CLOSE_D / 2.0);
    let close_r = Rect::from_center_size(close_c, Vec2::splat(CLOSE_D));
    if round_button(
        ui,
        Id::new("viewer_close"),
        close_r,
        Icon::X,
        18.0,
        Color32::from_black_alpha(150),
        Color32::from_black_alpha(210),
        Color32::WHITE,
        "Cerrar (Esc)",
    ) {
        acts.push(Act::Close);
    }

    let downloading = v.download.is_some();
    let zoomed = v.zoom > 1.01;
    // (id, icono, tooltip, acción)
    let buttons: [(&str, Icon, &str, Act); 5] = [
        (
            "zoom",
            if zoomed { Icon::ZoomOut } else { Icon::ZoomIn },
            if zoomed { "Alejar" } else { "Acercar" },
            Act::ToggleZoom,
        ),
        ("share", Icon::Forward, "Compartir (copiar enlace)", Act::CopyLink),
        ("download", Icon::Download, "Descargar", Act::Download),
        ("browser", Icon::ExternalLink, "Abrir en el navegador", Act::OpenBrowser),
        ("more", Icon::Ellipsis, "Más opciones", Act::ToggleMenu),
    ];
    let pad = 6.0;
    let gap = 4.0;
    let pill_w = pad * 2.0 + buttons.len() as f32 * BTN + (buttons.len() as f32 - 1.0) * gap;
    let pill = Rect::from_min_size(
        Pos2::new(close_r.left() - 10.0 - pill_w, close_c.y - PILL_H / 2.0),
        Vec2::new(pill_w, PILL_H),
    );
    ui.painter().rect_filled(pill, PILL_H / 2.0, extra::blend(palette.panel, Color32::BLACK, 0.25));
    ui.painter()
        .rect_stroke(pill, PILL_H / 2.0, Stroke::new(1.0, palette.outline), StrokeKind::Inside);

    let mut more_rect = Rect::NOTHING;
    let mut x = pill.left() + pad;
    for (id, icon, tip, act) in buttons {
        let r = Rect::from_min_size(Pos2::new(x, pill.center().y - BTN / 2.0), Vec2::splat(BTN));
        x += BTN + gap;
        if id == "more" {
            more_rect = r;
        }
        if id == "download" && downloading {
            egui::Spinner::new()
                .size(16.0)
                .color(palette.text)
                .paint_at(ui, Rect::from_center_size(r.center(), Vec2::splat(16.0)));
            continue;
        }
        let active = id == "more" && v.menu;
        let fill = if active { palette.surface_active } else { Color32::TRANSPARENT };
        if round_button(ui, Id::new(("viewer_btn", id)), r, icon, 18.0, fill, palette.surface_hover, palette.text, tip) {
            acts.push(act);
        }
    }

    // --- menú "más opciones" -------------------------------------------
    if v.menu {
        let rows: [(&str, Icon, Act, bool); 4] = [
            ("Copiar enlace", Icon::Copy, Act::CopyLink, true),
            ("Copiar nombre del archivo", Icon::Copy, Act::CopyName, true),
            ("Abrir en el navegador", Icon::ExternalLink, Act::OpenBrowser, true),
            ("Restablecer zoom", Icon::ZoomOut, Act::ResetZoom, zoomed),
        ];
        let row_h = 34.0;
        let w = 232.0;
        let h = rows.len() as f32 * row_h + 12.0;
        let menu = Rect::from_min_size(Pos2::new((more_rect.right() - w).max(8.0), pill.bottom() + 8.0), Vec2::new(w, h));
        ui.painter().rect_filled(menu, 10.0, extra::blend(palette.panel, Color32::BLACK, 0.15));
        ui.painter().rect_stroke(menu, 10.0, Stroke::new(1.0, palette.outline), StrokeKind::Inside);
        // Registrado antes que las filas: absorbe los clicks dentro del menú.
        let _ = ui.interact(menu, Id::new("viewer_menu_block"), Sense::click());
        for (i, (label, icon, act, enabled)) in rows.into_iter().enumerate() {
            let r = Rect::from_min_size(
                Pos2::new(menu.left() + 6.0, menu.top() + 6.0 + i as f32 * row_h),
                Vec2::new(w - 12.0, row_h),
            );
            let resp = ui.interact(r, Id::new(("viewer_menu_row", i)), if enabled { Sense::click() } else { Sense::hover() });
            if enabled && resp.hovered() {
                ui.painter().rect_filled(r, 6.0, palette.surface_hover);
            }
            let color = if enabled { palette.text } else { palette.dim };
            theme::paint_icon(
                ui,
                icon,
                Rect::from_center_size(Pos2::new(r.left() + 18.0, r.center().y), Vec2::splat(16.0)),
                16.0,
                color,
            );
            ui.painter().text(
                Pos2::new(r.left() + 36.0, r.center().y),
                Align2::LEFT_CENTER,
                label,
                theme::regular(13.0),
                color,
            );
            if enabled && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                acts.push(act);
            }
        }
    }
}

// ---------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------

/// Botón redondo con un ícono centrado (mismo estilo que
/// `call_view::round_button`). Devuelve `true` si hubo click.
#[allow(clippy::too_many_arguments)]
fn round_button(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    icon: Icon,
    icon_size: f32,
    fill: Color32,
    fill_hover: Color32,
    fg: Color32,
    tooltip: &str,
) -> bool {
    let resp = ui.interact(rect, id, Sense::click());
    let bg = if resp.hovered() { fill_hover } else { fill };
    ui.painter().rect_filled(rect, rect.height() / 2.0, bg);
    let size = if resp.is_pointer_button_down_on() { icon_size * 0.92 } else { icon_size };
    theme::paint_icon(ui, icon, rect, size, fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip).clicked()
}

fn pill_text_button(ui: &mut Ui, id: Id, rect: Rect, text: &str, palette: &Palette) -> bool {
    let resp = ui.interact(rect, id, Sense::click());
    let bg = if resp.hovered() { palette.surface_hover } else { palette.surface };
    ui.painter().rect_filled(rect, rect.height() / 2.0, bg);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, theme::semibold(12.5), palette.text);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Recorta `text` con "…" hasta que entre en `max_w`. Devuelve el texto y su ancho.
fn fit_text(ui: &Ui, text: &str, font: &egui::FontId, max_w: f32) -> (String, f32) {
    let measure = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x;
    let full = measure(text);
    if full <= max_w {
        return (text.to_string(), full);
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.len() > 1 {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "…";
        let w = measure(&candidate);
        if w <= max_w {
            return (candidate, w);
        }
    }
    ("…".to_string(), measure("…"))
}

// ---------------------------------------------------------------------
// Nombres y descarga
// ---------------------------------------------------------------------

/// Nombre del archivo a partir de la URL (sin query, con `%xx` decodificado).
fn file_name_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or(path);
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let decoded = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(b) = decoded {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let s = String::from_utf8_lossy(&out).into_owned();
    if s.is_empty() { "imagen".to_string() } else { s }
}

/// Quita los caracteres que Windows no admite en un nombre de archivo.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() { "imagen".to_string() } else { cleaned }
}

fn downloads_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(|p| p.to_path_buf()))
        .or_else(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
}

/// `nombre.ext` -> `nombre (1).ext`, `nombre (2).ext`... hasta que no exista.
fn unique_path(dir: &std::path::Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 1..1000 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

fn spawn_download(ctx: &egui::Context, item: Item) -> Receiver<Result<PathBuf, String>> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(180))
                .user_agent("Mozilla/5.0")
                .build()
                .map_err(|e| e.to_string())?;
            let resp = client.get(item.download_url()).send().map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(format!("El servidor respondió {}", resp.status()));
            }
            let bytes = resp.bytes().map_err(|e| e.to_string())?;
            let mut name = sanitize(&item.name);
            if !name.contains('.') {
                let from_url = file_name_from_url(item.download_url());
                if let Some((_, ext)) = from_url.rsplit_once('.') {
                    name = format!("{name}.{ext}");
                }
            }
            let dir = downloads_dir();
            let _ = std::fs::create_dir_all(&dir);
            let path = unique_path(&dir, &name);
            std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
            Ok(path)
        })();
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}
