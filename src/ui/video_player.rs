//! Reproductor de video embebido para los adjuntos de video de un mensaje
//! (`AttachmentKind::Video`).
//!
//! egui no trae ningún decoder de video — esto usa la crate `egui_video`,
//! que decodifica con `ffmpeg` (necesita las libs de ffmpeg 6/7 instaladas
//! en la máquina donde se compila Y donde corre la app) y reproduce el audio
//! con `cpal`.
//!
//! Usa el fork `egui-video` vendorizado en `vendor/egui-video` (portado a
//! egui 0.36). Su audio va por `cpal` (no por SDL2), así que ya no hace
//! falta la dependencia `sdl2`.
//!
//! API real del fork: `AudioDevice::new() -> AudioDevice` (hace `unwrap`/
//! `panic!` si no hay salida de audio o el formato no es f32, por eso acá se
//! envuelve en `catch_unwind`), `Player::new(ctx, &String) -> Result<Player>`,
//! `player.with_audio(&mut AudioDevice) -> Result<Player>` y
//! `player.ui(ui, Vec2)`.
//!
//! ## Por qué un `thread_local` y no un campo en `App`
//! Cada video adjunto necesita su propio `egui_video::Player`, y ese
//! `Player` tiene que sobrevivir entre frames (si se creara uno nuevo cada
//! frame, la decodificación se reiniciaría constantemente). Meterlo como
//! campo de `App` implicaría pasarlo a mano por toda la cadena de
//! funciones de `ui::chat`/`ui::dm` hasta `ui::media::show_attachments`,
//! que hoy no lo necesitan para nada más. La UI de egui corre siempre en
//! el hilo principal, así que un `thread_local` (sin locks) alcanza y no
//! rompe esa firma.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use egui::{Color32, CornerRadius, Rect, Sense, Ui, UiBuilder, Vec2};

use crate::ui::theme::{self, Icon};

struct VideoState {
    /// Tiene que seguir vivo mientras haya players con audio (si se suelta,
    /// se pierde el sonido).
    audio: Option<egui_video::AudioDevice>,
    /// Si falló crear el dispositivo de audio, no reintentar en cada click.
    audio_failed: bool,
    players: HashMap<String, PlayerSlot>,
    /// Video que está a pantalla completa ahora mismo (a lo sumo uno).
    fullscreen: Option<Fullscreen>,
}

struct Fullscreen {
    url: String,
    /// Si la ventana ya estaba en pantalla completa del SO antes de entrar,
    /// para no sacarla de ahí al salir del modo video.
    restore_os_fullscreen: bool,
}

impl Default for VideoState {
    fn default() -> Self {
        Self { audio: None, audio_failed: false, players: HashMap::new(), fullscreen: None }
    }
}

enum PlayerSlot {
    Playing(egui_video::Player),
    /// Falló crear el player o el audio (sin ffmpeg, códec no soportado,
    /// URL no accesible…): nos quedamos así para no reintentar en cada
    /// frame. La portada vuelve a mostrarse con el botón de "abrir en el
    /// reproductor del sistema", que sigue funcionando siempre.
    Failed,
}

thread_local! {
    static STATE: RefCell<VideoState> = RefCell::new(VideoState::default());
}

/// Inicializa el `AudioDevice` de `egui_video` la primera vez que hace
/// falta, y lo reusa después: toda la app comparte un solo dispositivo de
/// audio, como cualquier reproductor.
///
/// `AudioDevice::new()` no devuelve `Result`: hace `unwrap()` si no hay
/// dispositivo de salida y `panic!` si el formato por defecto no es f32.
/// `catch_unwind` evita que eso tumbe toda la app; en ese caso el video se
/// reproduce igual, solo que sin audio.
fn ensure_audio(state: &mut VideoState) {
    if state.audio.is_some() || state.audio_failed {
        return;
    }
    match std::panic::catch_unwind(egui_video::AudioDevice::new) {
        Ok(audio) => state.audio = Some(audio),
        Err(_) => state.audio_failed = true,
    }
}

fn start(ctx: &egui::Context, url: &str, state: &mut VideoState) {
    let result = (|| -> Result<egui_video::Player, String> {
        ensure_audio(state);
        let mut player =
            egui_video::Player::new(ctx, &url.to_string()).map_err(|e| format!("Player::new: {e}"))?;
        if let Some(audio) = state.audio.as_mut() {
            // Un video sin pista de audio no es un error acá: `with_audio`
            // consume el player, así que si falla nos quedamos sin él; se
            // reintenta sin audio abriendo de nuevo.
            player = match player.with_audio(audio) {
                Ok(p) => p,
                Err(_) => egui_video::Player::new(ctx, &url.to_string())
                    .map_err(|e| format!("Player::new: {e}"))?,
            };
        }
        player.start();
        Ok(player)
    })();
    let slot = match result {
        Ok(player) => PlayerSlot::Playing(player),
        Err(_err) => {
            // Sin logger centralizado en este módulo — el fallback visual
            // (portada + "abrir externamente") ya le avisa al usuario que
            // acá no se pudo, no hace falta más que eso.
            PlayerSlot::Failed
        }
    };
    state.players.insert(url.to_string(), slot);
}

fn stop(url: &str, state: &mut VideoState) {
    if let Some(PlayerSlot::Playing(mut player)) = state.players.remove(url) {
        player.stop();
    }
}

/// Qué hay que hacer después de dibujar el player (se decide mientras
/// `player` está prestado y se ejecuta cuando ya no se usa).
enum PlayerAction {
    Close,
    Fullscreen,
}

/// Dibuja el video adjunto en `rect`: si todavía no se lo arrancó (o si
/// arrancarlo falló), la portada con el botón de play que pinta quien
/// llama (`paint_poster`); si ya está reproduciéndose, el player real de
/// `egui_video` (con sus propios controles de play/pause/seek/volumen) más
/// dos botones chicos: cerrarlo (vuelve a la portada) y pantalla completa.
/// Doble click sobre el video también entra a pantalla completa.
///
/// Devuelve `true` mientras se está mostrando la portada (todavía no se
/// arrancó, o arrancarlo falló) — o sea, cuando quien llama puede querer
/// ofrecer además un botón para abrirlo en el reproductor del sistema.
/// `false` mientras el video se está reproduciendo acá adentro (o a
/// pantalla completa).
pub fn show(ui: &mut Ui, url: &str, rect: Rect, paint_poster: impl FnOnce(&mut Ui)) -> bool {
    let ctx = ui.ctx().clone();
    STATE.with_borrow_mut(|state| match state.players.get_mut(url) {
        Some(PlayerSlot::Playing(player)) => {
            let in_fullscreen = state.fullscreen.as_ref().is_some_and(|f| f.url == url);
            let mut action = None;
            if in_fullscreen {
                // Lo dibuja `show_fullscreen` encima de toda la ventana; acá
                // solo queda el hueco negro para no mostrar el video dos
                // veces (un `Player` solo puede procesar su estado una vez
                // por frame).
                ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
            } else {
                // `player.ui()` es de `egui_video` y hace su propio layout
                // (cursor + `ui.add`) en vez de tomar un `Rect` absoluto.
                // Como acá el `rect` ya viene reservado de antes (quien
                // llama ya avanzó el cursor con `allocate_exact_size`), sin
                // este `scope_builder` el player terminaría dibujándose
                // después de ese espacio en vez de adentro.
                let response = ui
                    .scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                        player.ui(ui, Vec2::new(rect.width(), rect.height()))
                    })
                    .inner;
                if response.double_clicked() {
                    action = Some(PlayerAction::Fullscreen);
                }
                // Si el player no pidiera él solo un repaint por cada frame
                // nuevo decodificado, quedaría clavado hasta el próximo
                // evento de la UI. Barato pedirlo de más acá.
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
                if corner_button(ui, rect, Icon::X, ("video_close", url))
                    .on_hover_text("Cerrar")
                    .clicked()
                {
                    action = Some(PlayerAction::Close);
                }
                if corner_button_slot(ui, rect, 1, Icon::Maximize2, ("video_fullscreen", url))
                    .on_hover_text("Pantalla completa")
                    .clicked()
                {
                    action = Some(PlayerAction::Fullscreen);
                }
            }
            // `player` ya no se usa más abajo, así que se puede volver a
            // tomar `state` entero.
            match action {
                Some(PlayerAction::Close) => stop(url, state),
                Some(PlayerAction::Fullscreen) => enter_fullscreen(&ctx, url, state),
                None => {}
            }
            false
        }
        Some(PlayerSlot::Failed) | None => {
            paint_poster(ui);
            let response = ui.interact(rect, ui.id().with(("video_poster", url)), Sense::click());
            if response.clicked() {
                start(&ctx, url, state);
            }
            true
        }
    })
}

fn enter_fullscreen(ctx: &egui::Context, url: &str, state: &mut VideoState) {
    if state.fullscreen.is_some() {
        return;
    }
    let restore_os_fullscreen = ctx.input(|i| i.viewport().fullscreen).unwrap_or(false);
    state.fullscreen = Some(Fullscreen { url: url.to_string(), restore_os_fullscreen });
    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
}

fn exit_fullscreen(ctx: &egui::Context, state: &mut VideoState) {
    if let Some(fs) = state.fullscreen.take() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fs.restore_os_fullscreen));
    }
}

/// Rect más grande con la proporción de `size` que entra centrado en
/// `container` (letterbox).
fn fit_rect(container: Rect, size: Vec2) -> Rect {
    if size.x <= 0.0 || size.y <= 0.0 {
        return container;
    }
    let scale = (container.width() / size.x).min(container.height() / size.y);
    Rect::from_center_size(container.center(), size * scale)
}

/// Dibuja el video a pantalla completa (encima de todo, con fondo negro) si
/// hay uno en ese modo. **Hay que llamarla una vez por frame, al final de
/// `App::ui`** (después de los demás overlays) — ver `lib/state.rs`. Se
/// sale con la cruz de la esquina (`Icon::Shrink`), doble click en el video
/// o `Esc`.
pub fn show_fullscreen(ctx: &egui::Context) {
    STATE.with_borrow_mut(|state| {
        let Some(url) = state.fullscreen.as_ref().map(|f| f.url.clone()) else {
            return;
        };
        let mut exit = ctx.input(|i| i.key_pressed(egui::Key::Escape));

        match state.players.get_mut(&url) {
            Some(PlayerSlot::Playing(player)) => {
                let screen = ctx.viewport_rect();
                egui::Area::new(egui::Id::new("ecord_video_fullscreen"))
                    .order(egui::Order::Tooltip)
                    .fixed_pos(screen.min)
                    .show(ctx, |ui| {
                        ui.set_width(screen.width());
                        ui.set_height(screen.height());
                        ui.painter().rect_filled(screen, 0.0, Color32::BLACK);
                        // Consume los clicks para que no lleguen a lo que
                        // está atrás (queda por debajo del player y de los
                        // botones, que se agregan después).
                        ui.interact(screen, egui::Id::new("ecord_video_fullscreen_block"), Sense::click_and_drag());

                        let video_rect = fit_rect(screen, player.size);
                        let response = ui
                            .scope_builder(UiBuilder::new().max_rect(video_rect), |ui| {
                                player.ui(ui, video_rect.size())
                            })
                            .inner;
                        if response.double_clicked() {
                            exit = true;
                        }
                        if corner_button(ui, screen, Icon::Shrink, ("video_fullscreen_exit", &url))
                            .on_hover_text("Salir de pantalla completa (Esc)")
                            .clicked()
                        {
                            exit = true;
                        }
                    });
                ctx.request_repaint_after(std::time::Duration::from_millis(33));
            }
            // El player desapareció (se cerró, falló…): no queda nada que
            // mostrar a pantalla completa.
            _ => exit = true,
        }

        if exit {
            exit_fullscreen(ctx, state);
        }
    });
}

// ---------------------------------------------------------------------
// GIFs de Tenor/Giphy (embeds `gifv`): autoplay, en bucle, mudos
// ---------------------------------------------------------------------
//
// Discord no manda un `.gif` para esos embeds sino un mp4 corto (`video`) y
// una portada estática (`thumbnail`). Se reproducen con el mismo
// `egui_video::Player`, pero:
//   * arrancan solos (sin click), en bucle y sin audio (no se le pasa
//     `AudioDevice`), sin barra de controles;
//   * `Player::new` es BLOQUEANTE (abre la URL con ffmpeg y decodifica el
//     primer cuadro), así que se crea en un hilo aparte y la UI sigue
//     mostrando la portada hasta que llega;
//   * solo arrancan si llevan un ratito a la vista (para no lanzar una
//     descarga por cada GIF que pasa volando al scrollear), hay un tope de
//     `MAX_LOOPING` a la vez, y se sueltan al dejar de dibujarse
//     (`end_frame`), porque cada player tiene sus propios hilos y pide un
//     repaint por cuadro.

/// Reproductores de GIF simultáneos (el resto se queda con la portada).
const MAX_LOOPING: usize = 6;
/// Segundos a la vista antes de arrancar la descarga/decodificación.
const LOOP_START_DELAY: f64 = 0.3;
/// Segundos sin dibujarse para soltar el player.
const LOOP_IDLE_DROP: f64 = 1.0;

enum LoopKind {
    /// Visible desde hace poco: todavía no vale la pena arrancarlo.
    Waiting,
    /// El hilo está creando el `Player`.
    Loading(Receiver<Result<egui_video::Player, String>>),
    Playing(Box<egui_video::Player>),
    /// No se pudo crear (sin ffmpeg, códec no soportado, red…): se queda la
    /// portada y no se reintenta hasta que el GIF salga de pantalla.
    Failed,
}

struct LoopSlot {
    kind: LoopKind,
    first_seen: f64,
    last_seen: f64,
}

#[derive(Default)]
struct LoopState {
    slots: HashMap<String, LoopSlot>,
}

thread_local! {
    static LOOPS: RefCell<LoopState> = RefCell::new(LoopState::default());
}

fn spawn_loader(ctx: &egui::Context, url: &str) -> Receiver<Result<egui_video::Player, String>> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    let url = url.to_string();
    std::thread::spawn(move || {
        let result = (|| -> Result<egui_video::Player, String> {
            let mut player = egui_video::Player::new(&ctx, &url).map_err(|e| e.to_string())?;
            player.options.looping = true;
            player.options.audio_volume.set(0.0);
            player.start();
            Ok(player)
        })();
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

/// Dibuja el video `url` (mp4 de un embed `gifv`) en `rect`, en bucle y sin
/// audio, con las esquinas redondeadas `radius`. Devuelve `true` si lo
/// está mostrando (quien llama debería repintar encima lo que quiera, p. ej.
/// la insignia "GIF"), o `false` mientras no hay video (esperando, cargando,
/// falló o no hay lugar): en ese caso queda lo que quien llama ya pintó, o
/// sea la portada.
pub fn show_looping(ui: &mut Ui, url: &str, rect: Rect, radius: u8) -> bool {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    LOOPS.with_borrow_mut(|st| {
        let active = st
            .slots
            .values()
            .filter(|s| matches!(s.kind, LoopKind::Loading(_) | LoopKind::Playing(_)))
            .count();
        let slot = st.slots.entry(url.to_string()).or_insert_with(|| LoopSlot {
            kind: LoopKind::Waiting,
            first_seen: now,
            last_seen: now,
        });
        slot.last_seen = now;

        if matches!(slot.kind, LoopKind::Waiting) {
            if now - slot.first_seen < LOOP_START_DELAY {
                ctx.request_repaint_after(Duration::from_millis(100));
                return false;
            }
            if active >= MAX_LOOPING {
                ctx.request_repaint_after(Duration::from_millis(500));
                return false;
            }
            slot.kind = LoopKind::Loading(spawn_loader(&ctx, url));
        }

        let arrived = if let LoopKind::Loading(rx) = &slot.kind { Some(rx.try_recv()) } else { None };
        match arrived {
            Some(Ok(Ok(player))) => slot.kind = LoopKind::Playing(Box::new(player)),
            Some(Ok(Err(_))) | Some(Err(TryRecvError::Disconnected)) => slot.kind = LoopKind::Failed,
            Some(Err(TryRecvError::Empty)) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                return false;
            }
            None => {}
        }

        match &mut slot.kind {
            LoopKind::Playing(player) => {
                // Sin `Player::ui`: no queremos la barra de controles. Pero
                // `process_state` es lo que reinicia el video al llegar al
                // final (`options.looping`), así que va en cada frame.
                player.process_state();
                egui::Image::new(egui::load::SizedTexture::new(player.texture_handle.id(), rect.size()))
                    .corner_radius(CornerRadius::same(radius))
                    .paint_at(ui, rect);
                true
            }
            _ => false,
        }
    })
}

/// Suelta los players de GIF que dejaron de dibujarse. **Hay que llamarla
/// una vez por frame, al final de `App::ui`** (junto a `show_fullscreen`):
/// si un GIF sale de pantalla ya nadie lo dibuja, y sin esto su player
/// seguiría decodificando y pidiendo repaints para siempre.
pub fn end_frame(ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    LOOPS.with_borrow_mut(|st| {
        st.slots.retain(|_, s| now - s.last_seen <= LOOP_IDLE_DROP);
    });
}

/// Botón redondo chico pegado a la esquina superior derecha de `rect` —
/// "cerrar" el player (`Icon::X`, acá mismo) y "abrir en el reproductor
/// del sistema" sobre la portada (`Icon::ExternalLink`, usado por
/// `ui::media`) comparten el mismo estilo. `id_salt` distingue el botón
/// cuando puede haber más de uno con el mismo `rect` en el mismo frame.
pub fn corner_button(ui: &mut Ui, rect: Rect, icon: Icon, id_salt: impl std::hash::Hash + std::fmt::Debug) -> egui::Response {
    corner_button_slot(ui, rect, 0, icon, id_salt)
}

/// Igual que `corner_button`, pero corrido `slot` lugares hacia la izquierda
/// (0 = pegado a la esquina, 1 = el de al lado, …) para poder poner varios.
pub fn corner_button_slot(
    ui: &mut Ui,
    rect: Rect,
    slot: usize,
    icon: Icon,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
) -> egui::Response {
    let side = 22.0;
    let center = rect.right_top() + Vec2::new(-14.0 - slot as f32 * (side + 4.0), 14.0);
    let btn_rect = Rect::from_center_size(center, Vec2::splat(side));
    let response = ui.interact(btn_rect, ui.id().with(id_salt), Sense::click());
    if ui.is_rect_visible(btn_rect) {
        let bg = if response.hovered() {
            Color32::from_black_alpha(220)
        } else {
            Color32::from_black_alpha(160)
        };
        ui.painter().circle_filled(btn_rect.center(), side / 2.0, bg);
        theme::paint_icon(
            ui,
            icon,
            Rect::from_center_size(btn_rect.center(), Vec2::splat(side * 0.55)),
            side * 0.28,
            Color32::WHITE,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
