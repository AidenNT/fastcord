//! Reproductor de video embebido (adjuntos `AttachmentKind::Video` y GIFs
//! `gifv` de Tenor/Giphy).
//!
//! Usa `egui_video::Engine` (ver `vendor/egui-video/src/engine.rs`): demux y
//! decodificación en hilos propios, reloj de audio como maestro y una cola de
//! cuadros que la UI consume a la hora justa. El `Player` anterior de
//! `egui_video` tironeaba (~cada 0.5 s) por cómo llevaba el reloj; el motivo
//! está explicado arriba de `engine.rs`.
//!
//! ## Estética
//! Los controles copian los de la parte de transmisión (`ui::call_view`,
//! `draw_stream`/`controls_bar`): chip translúcido arriba a la izquierda con
//! el ícono `Video`, botones redondos de 32 px arriba a la derecha
//! (`X`, `Maximize2`/`Shrink`), barra en píldora abajo con play/pausa, tiempo,
//! seek y volumen, y todo se desvanece tras `IDLE_SECS` sin mover el mouse.
//!
//! ## Por qué un `thread_local` y no un campo en `App`
//! Cada video adjunto necesita su propio `Engine`, que tiene que sobrevivir
//! entre frames. Meterlo en `App` obligaría a pasarlo por toda la cadena de
//! `ui::chat`/`ui::dm` hasta `ui::media`. La UI de egui corre siempre en el
//! hilo principal, así que un `thread_local` (sin locks) alcanza.
//!
//! La paleta se guarda en la memoria temporal de egui cada vez que `show` la
//! recibe, para que `show_fullscreen(ctx)` (que solo tiene el `Context`) use
//! los mismos colores sin cambiar su firma.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use egui::load::SizedTexture;
use egui::{Align2, Color32, CornerRadius, Id, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};
use egui_video::Engine;

use crate::ui::extra;
use crate::ui::theme::{self, Icon, Palette};

/// Segundos sin mover el mouse para que los controles se desvanezcan
/// (mismo valor que la vista de llamada).
const IDLE_SECS: f64 = 3.0;
const FADE_SECS: f32 = 0.25;
const INLINE_RADIUS: u8 = 6;
const BTN: f32 = 32.0;
const BTN_STEP: f32 = 40.0;
const BTN_MARGIN: f32 = 28.0;
const BAR_H: f32 = 44.0;
const SEEK_STEP_MS: i64 = 5_000;

fn palette_id() -> Id {
    Id::new("ecord_video_palette")
}

// ---------------------------------------------------------------------
// Estado
// ---------------------------------------------------------------------

struct Slot {
    engine: Engine,
    last_activity: f64,
    /// Fracción 0..1 mientras se arrastra la barra de seek.
    scrub: Option<f32>,
    last_scrub_seek: f64,
    /// Si estaba reproduciéndose al empezar a arrastrar (se pausa mientras
    /// se arrastra para que no suenen saltos).
    resume_after_scrub: bool,
}

enum PlayerSlot {
    Playing(Box<Slot>),
    /// Falló abrir/decodificar (sin ffmpeg, códec no soportado, URL no
    /// accesible…): se queda así para no reintentar en cada frame. La
    /// portada vuelve a mostrarse con el botón de "abrir en el reproductor
    /// del sistema", que sigue funcionando siempre.
    Failed,
}

struct Fullscreen {
    url: String,
    /// Si la ventana ya estaba en pantalla completa del SO antes de entrar,
    /// para no sacarla de ahí al salir del modo video.
    restore_os_fullscreen: bool,
}

struct VideoState {
    /// Tiene que seguir vivo mientras haya engines con audio.
    audio: Option<egui_video::AudioDevice>,
    audio_failed: bool,
    players: HashMap<String, PlayerSlot>,
    fullscreen: Option<Fullscreen>,
    /// Volumen y silencio compartidos por todos los videos.
    volume: f32,
    muted: bool,
}

impl Default for VideoState {
    fn default() -> Self {
        Self {
            audio: None,
            audio_failed: false,
            players: HashMap::new(),
            fullscreen: None,
            volume: 0.6,
            muted: false,
        }
    }
}

thread_local! {
    static STATE: RefCell<VideoState> = RefCell::new(VideoState::default());
}

/// Inicializa el `AudioDevice` la primera vez que hace falta y lo reusa:
/// toda la app comparte un solo dispositivo de salida.
///
/// `AudioDevice::new()` no devuelve `Result`: hace `unwrap()` si no hay
/// dispositivo y `panic!` si el formato de muestra no es uno de los
/// soportados. `catch_unwind` evita que eso tumbe la app; en ese caso el
/// video se reproduce igual, solo que sin audio.
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
    ensure_audio(state);
    let mut engine = Engine::open(ctx, url, state.audio.as_ref(), false);
    engine.set_volume(state.volume);
    engine.set_muted(state.muted);
    let now = ctx.input(|i| i.time);
    state.players.insert(
        url.to_string(),
        PlayerSlot::Playing(Box::new(Slot {
            engine,
            last_activity: now,
            scrub: None,
            last_scrub_seek: 0.0,
            resume_after_scrub: false,
        })),
    );
}

fn stop(url: &str, state: &mut VideoState) {
    // Soltar el `Engine` detiene sus hilos y desconecta su audio.
    state.players.remove(url);
}

/// Qué hay que hacer después de dibujar el player (se decide mientras el
/// slot está prestado y se ejecuta cuando ya no se usa).
#[derive(Clone, Copy, PartialEq)]
enum Outcome {
    None,
    Start,
    Close,
    Fail,
    EnterFullscreen,
    ExitFullscreen,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Inline,
    Fullscreen,
}

impl Mode {
    fn tag(self) -> &'static str {
        match self {
            Mode::Inline => "inline",
            Mode::Fullscreen => "fs",
        }
    }
}

// ---------------------------------------------------------------------
// API pública (misma forma que antes, salvo que `show` recibe la paleta)
// ---------------------------------------------------------------------

/// Dibuja el video adjunto en `rect`: si todavía no se lo arrancó (o si
/// arrancarlo falló), la portada con el botón de play que pinta quien llama
/// (`paint_poster`); si ya está reproduciéndose, el player con sus controles.
/// Un click sobre el video alterna play/pausa y doble click entra a pantalla
/// completa.
///
/// Devuelve `true` mientras se está mostrando la portada (todavía no se
/// arrancó, o arrancarlo falló) — o sea, cuando quien llama puede querer
/// ofrecer además un botón para abrirlo en el reproductor del sistema.
pub fn show(ui: &mut Ui, palette: &Palette, url: &str, rect: Rect, paint_poster: impl FnOnce(&mut Ui)) -> bool {
    let ctx = ui.ctx().clone();
    ctx.data_mut(|d| d.insert_temp(palette_id(), *palette));
    STATE.with_borrow_mut(|st| {
        let in_fullscreen = st.fullscreen.as_ref().is_some_and(|f| f.url == url);
        let mut outcome = Outcome::None;
        let showing_poster = match st.players.get_mut(url) {
            Some(PlayerSlot::Playing(slot)) => {
                if in_fullscreen {
                    // Lo dibuja `show_fullscreen` encima de toda la ventana;
                    // acá solo queda el hueco negro (un `Engine` solo debe
                    // actualizarse una vez por frame).
                    ui.painter().rect_filled(rect, CornerRadius::same(INLINE_RADIUS), Color32::BLACK);
                } else {
                    slot.engine.update(&ctx);
                    if slot.engine.failed() {
                        outcome = Outcome::Fail;
                    } else {
                        if !slot.engine.has_frame() {
                            paint_poster(ui);
                        }
                        outcome = draw_player(ui, palette, slot, rect, Mode::Inline, url, &mut st.volume, &mut st.muted);
                    }
                }
                false
            }
            Some(PlayerSlot::Failed) | None => {
                paint_poster(ui);
                let response = ui.interact(rect, ui.id().with(("video_poster", url)), Sense::click());
                if response.clicked() {
                    outcome = Outcome::Start;
                }
                true
            }
        };
        match outcome {
            Outcome::Start => start(&ctx, url, st),
            Outcome::Close => stop(url, st),
            Outcome::Fail => {
                st.players.insert(url.to_string(), PlayerSlot::Failed);
            }
            Outcome::EnterFullscreen => enter_fullscreen(&ctx, url, st),
            Outcome::ExitFullscreen | Outcome::None => {}
        }
        showing_poster
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
/// `App::ui`** (después de los demás overlays) — ver `lib/state.rs`. Se sale
/// con la cruz de la esquina (`Icon::Shrink`), doble click o `Esc`.
/// Atajos mientras está a pantalla completa: `Espacio` play/pausa, `←`/`→`
/// ∓5 s, `↑`/`↓` volumen, `M` silencio.
pub fn show_fullscreen(ctx: &egui::Context) {
    STATE.with_borrow_mut(|st| {
        let Some(url) = st.fullscreen.as_ref().map(|f| f.url.clone()) else {
            return;
        };
        let palette = ctx.data(|d| d.get_temp::<Palette>(palette_id())).unwrap_or_else(Palette::dark);
        let mut exit = ctx.input(|i| i.key_pressed(egui::Key::Escape));

        match st.players.get_mut(&url) {
            Some(PlayerSlot::Playing(slot)) => {
                slot.engine.update(ctx);
                let screen = ctx.viewport_rect();
                egui::Area::new(Id::new("ecord_video_fullscreen"))
                    .order(egui::Order::Tooltip)
                    .fixed_pos(screen.min)
                    .show(ctx, |ui| {
                        ui.set_width(screen.width());
                        ui.set_height(screen.height());
                        ui.painter().rect_filled(screen, 0.0, Color32::BLACK);
                        // Consume los clicks para que no lleguen a lo que
                        // está atrás (queda por debajo del player y de los
                        // botones, que se registran después).
                        ui.interact(screen, Id::new("ecord_video_fullscreen_block"), Sense::click_and_drag());
                        let outcome =
                            draw_player(ui, &palette, slot, screen, Mode::Fullscreen, &url, &mut st.volume, &mut st.muted);
                        if outcome == Outcome::ExitFullscreen {
                            exit = true;
                        }
                    });
            }
            // El player desapareció (se cerró, falló…): no queda nada que
            // mostrar a pantalla completa.
            _ => exit = true,
        }

        if exit {
            exit_fullscreen(ctx, st);
        }
    });
}

// ---------------------------------------------------------------------
// Dibujo del player
// ---------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn draw_player(
    ui: &mut Ui,
    palette: &Palette,
    slot: &mut Slot,
    rect: Rect,
    mode: Mode,
    url: &str,
    volume: &mut f32,
    muted: &mut bool,
) -> Outcome {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let base = Id::new(("ecord_video", url, mode.tag()));
    let radius = if mode == Mode::Fullscreen { CornerRadius::ZERO } else { CornerRadius::same(INLINE_RADIUS) };
    let mut outcome = Outcome::None;

    // --- imagen -----------------------------------------------------
    let has_frame = slot.engine.has_frame();
    if has_frame {
        ui.painter().rect_filled(rect, radius, Color32::BLACK);
        if let Some(size) = slot.engine.size() {
            let video = fit_rect(rect, size);
            let fills = (video.width() - rect.width()).abs() < 1.0 && (video.height() - rect.height()).abs() < 1.0;
            egui::Image::new(SizedTexture::new(slot.engine.texture_id(), video.size()))
                .corner_radius(if fills { radius } else { CornerRadius::ZERO })
                .paint_at(ui, video);
        }
    } else {
        // Todavía no llegó el primer cuadro: se oscurece la portada que
        // pintó quien llama.
        ui.painter().rect_filled(rect, radius, Color32::from_black_alpha(110));
    }

    // --- superficie y visibilidad de los controles -------------------
    let surface = ui.interact(rect, base.with("surface"), Sense::click());

    let hovered = ui.rect_contains_pointer(rect);
    let moved = ctx.input(|i| i.pointer.delta() != Vec2::ZERO || i.pointer.any_pressed());
    if hovered && moved {
        slot.last_activity = now;
    }
    let paused_like = slot.engine.is_paused() || slot.engine.has_ended();
    let recently_active = hovered && now - slot.last_activity <= IDLE_SECS;
    let visible = slot.scrub.is_some() || recently_active || paused_like;
    let alpha = ctx.animate_bool_with_time(base.with("alpha"), visible, FADE_SECS);
    if recently_active && !paused_like {
        let remaining = (IDLE_SECS - (now - slot.last_activity)).max(0.0) + 0.05;
        ctx.request_repaint_after(Duration::from_secs_f64(remaining));
    }
    if mode == Mode::Fullscreen && !visible {
        ctx.set_cursor_icon(egui::CursorIcon::None);
    }

    // --- estado central: cargando / pausa ----------------------------
    let center = rect.center();
    if !paused_like && (!has_frame || slot.engine.buffering()) {
        egui::Spinner::new()
            .size(30.0)
            .color(Color32::WHITE)
            .paint_at(ui, Rect::from_center_size(center, Vec2::splat(30.0)));
    }
    if paused_like && has_frame {
        let big = Rect::from_center_size(center, Vec2::splat(56.0));
        let icon = if slot.engine.has_ended() { Icon::Refresh } else { Icon::PlayFilled };
        let resp = round_button_resp(
            ui,
            base.with("center"),
            big,
            icon,
            26.0,
            Color32::from_black_alpha(150),
            Color32::from_black_alpha(210),
            Color32::WHITE,
            1.0,
        );
        if resp.on_hover_text(if slot.engine.has_ended() { "Repetir" } else { "Reproducir" }).clicked() {
            slot.engine.toggle_pause();
        }
    }

    // --- chip + botones de arriba (como `draw_stream`) ----------------
    if alpha > 0.02 {
        if rect.width() >= 240.0 {
            let font = theme::semibold(12.5);
            let label = file_label(url);
            let max_title = (rect.width() - 12.0 - 40.0 - 12.0 - 3.0 * BTN_STEP).max(40.0);
            let (title, w) = fit_text(ui, &label, &font, max_title);
            let chip = Rect::from_min_size(rect.left_top() + Vec2::new(12.0, 12.0), Vec2::new(w + 40.0, 28.0));
            ui.painter().rect_filled(chip, 14.0, Color32::from_black_alpha(150).gamma_multiply(alpha));
            theme::paint_icon(
                ui,
                Icon::Video,
                Rect::from_center_size(Pos2::new(chip.left() + 16.0, chip.center().y), Vec2::splat(14.0)),
                14.0,
                palette.accent.gamma_multiply(alpha),
            );
            ui.painter().text(
                Pos2::new(chip.left() + 30.0, chip.center().y),
                Align2::LEFT_CENTER,
                title,
                font,
                Color32::WHITE.gamma_multiply(alpha),
            );
        }

        let dark = Color32::from_black_alpha(150);
        let darker = Color32::from_black_alpha(210);
        let slot_rect =
            |n: f32| Rect::from_center_size(rect.right_top() + Vec2::new(-BTN_MARGIN - n * BTN_STEP, BTN_MARGIN), Vec2::splat(BTN));
        match mode {
            Mode::Fullscreen => {
                if top_button(ui, base.with("exit_fs"), slot_rect(0.0), Icon::Shrink, "Salir de pantalla completa (Esc)", dark, darker, alpha) {
                    outcome = Outcome::ExitFullscreen;
                }
            }
            Mode::Inline => {
                if top_button(ui, base.with("close"), slot_rect(0.0), Icon::X, "Cerrar", dark, darker, alpha) {
                    outcome = Outcome::Close;
                }
                if top_button(ui, base.with("fs"), slot_rect(1.0), Icon::Maximize2, "Pantalla completa", dark, darker, alpha) {
                    outcome = Outcome::EnterFullscreen;
                }
            }
        }
    }

    // --- barra de abajo (como `controls_bar`) -------------------------
    if alpha > 0.02 && rect.height() >= 120.0 && rect.width() >= 160.0 {
        let bar = match mode {
            Mode::Inline => Rect::from_min_max(
                Pos2::new(rect.left() + 12.0, rect.bottom() - 12.0 - BAR_H),
                Pos2::new(rect.right() - 12.0, rect.bottom() - 12.0),
            ),
            Mode::Fullscreen => {
                let w = (rect.width() - 24.0).min(760.0);
                Rect::from_center_size(Pos2::new(rect.center().x, rect.bottom() - 24.0 - BAR_H / 2.0), Vec2::new(w, BAR_H))
            }
        };
        draw_bar(ui, palette, slot, bar, alpha, base, now, volume, muted);
    }

    // --- clicks sobre el video + atajos -------------------------------
    // Un click alterna play/pausa; si es doble click, el segundo click lo
    // vuelve a alternar y queda igual, y encima entra/sale de pantalla
    // completa.
    if surface.clicked() {
        slot.engine.toggle_pause();
    }
    if surface.double_clicked() {
        outcome = if mode == Mode::Fullscreen { Outcome::ExitFullscreen } else { Outcome::EnterFullscreen };
    }

    if mode == Mode::Fullscreen && ctx.memory(|m| m.focused()).is_none() {
        let (space, left, right, up, down, mute) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::M),
            )
        });
        if space {
            slot.engine.toggle_pause();
        }
        if left {
            slot.engine.seek_ms(slot.engine.position_ms() - SEEK_STEP_MS);
        }
        if right {
            slot.engine.seek_ms(slot.engine.position_ms() + SEEK_STEP_MS);
        }
        if up || down {
            *volume = (*volume + if up { 0.1 } else { -0.1 }).clamp(0.0, 1.0);
            *muted = *volume <= 0.001;
            slot.engine.set_volume(*volume);
            slot.engine.set_muted(*muted);
            slot.last_activity = now;
        }
        if mute {
            *muted = !*muted;
            slot.engine.set_muted(*muted);
        }
    }

    outcome
}

#[allow(clippy::too_many_arguments)]
fn draw_bar(
    ui: &mut Ui,
    palette: &Palette,
    slot: &mut Slot,
    bar: Rect,
    alpha: f32,
    base: Id,
    now: f64,
    volume: &mut f32,
    muted: &mut bool,
) {
    let h = bar.height();
    ui.painter().rect_filled(
        bar,
        h / 2.0,
        extra::blend(palette.panel, Color32::BLACK, 0.25).gamma_multiply(alpha),
    );
    ui.painter().rect_stroke(
        bar,
        h / 2.0,
        Stroke::new(1.0, palette.outline.gamma_multiply(alpha)),
        StrokeKind::Inside,
    );

    let pad = 6.0;
    let gap = 8.0;
    let cy = bar.center().y;
    let wide = bar.width() >= 300.0;
    let font = theme::regular(12.0);

    let dur = slot.engine.duration_ms();
    let pos = slot.engine.position_ms();
    let ended = slot.engine.has_ended();
    let paused = slot.engine.is_paused();

    // Izquierda: play/pausa.
    let mut x = bar.left() + pad;
    let play_rect = Rect::from_min_size(Pos2::new(x, cy - BTN / 2.0), Vec2::splat(BTN));
    x += BTN + gap;
    let (icon, tip) = if ended {
        (Icon::Refresh, "Repetir")
    } else if paused {
        (Icon::PlayFilled, "Reproducir")
    } else {
        (Icon::PauseFilled, "Pausar")
    };
    if round_button_resp(
        ui,
        base.with("play"),
        play_rect,
        icon,
        18.0,
        palette.surface,
        palette.surface_hover,
        palette.text,
        alpha,
    )
    .on_hover_text(tip)
    .clicked()
    {
        slot.engine.toggle_pause();
    }

    // Derecha: volumen (solo si el video tiene audio).
    let mut right = bar.right() - pad;
    if slot.engine.has_audio() {
        let vol_btn = Rect::from_min_size(Pos2::new(right - BTN, cy - BTN / 2.0), Vec2::splat(BTN));
        right -= BTN + gap;
        let silent = *muted || *volume <= 0.001;
        let muted_fill = extra::blend(palette.surface, palette.danger, 0.35);
        let muted_fill_hover = extra::blend(palette.surface_hover, palette.danger, 0.45);
        let (icon, fill, hover, fg, tip) = if silent {
            (Icon::VolumeX, muted_fill, muted_fill_hover, palette.danger, "Activar sonido")
        } else if *volume < 0.4 {
            (Icon::Volume1, palette.surface, palette.surface_hover, palette.text, "Silenciar")
        } else {
            (Icon::Volume2, palette.surface, palette.surface_hover, palette.text, "Silenciar")
        };
        if round_button_resp(ui, base.with("vol_btn"), vol_btn, icon, 18.0, fill, hover, fg, alpha)
            .on_hover_text(tip)
            .clicked()
        {
            if silent {
                *muted = false;
                if *volume < 0.05 {
                    *volume = 0.5;
                }
            } else {
                *muted = true;
            }
            slot.engine.set_volume(*volume);
            slot.engine.set_muted(*muted);
        }

        if bar.width() >= 480.0 {
            let w = 64.0;
            let vr = Rect::from_min_size(Pos2::new(right - w, cy - 6.0), Vec2::new(w, 12.0));
            right -= w + gap;
            let shown = if *muted { 0.0 } else { *volume };
            let out = slider(ui, base.with("vol_slider"), vr, shown, palette, alpha, true);
            if let Some(v) = out.value {
                *volume = v;
                *muted = v <= 0.001;
                slot.engine.set_volume(*volume);
                slot.engine.set_muted(*muted);
                slot.last_activity = now;
            }
        }
    }

    // Tiempos y barra de seek.
    let shown_pos = match slot.scrub {
        Some(f) if dur > 0 => (f * dur as f32) as i64,
        _ => pos,
    };
    let (left_txt, right_txt) = (fmt_time(shown_pos), fmt_time(dur.max(shown_pos)));
    let tw = {
        let widest = if right_txt.len() >= left_txt.len() { &right_txt } else { &left_txt };
        ui.painter().layout_no_wrap(widest.clone(), font.clone(), Color32::WHITE).size().x
    };
    if wide {
        ui.painter().text(
            Pos2::new(x, cy),
            Align2::LEFT_CENTER,
            &left_txt,
            font.clone(),
            palette.secondary.gamma_multiply(alpha),
        );
        x += tw + gap;
        if dur > 0 {
            ui.painter().text(
                Pos2::new(right, cy),
                Align2::RIGHT_CENTER,
                &right_txt,
                font.clone(),
                palette.secondary.gamma_multiply(alpha),
            );
            right -= tw + gap;
        }
    }

    if right - x > 24.0 {
        let seek = Rect::from_min_max(Pos2::new(x, cy - 6.0), Pos2::new(right, cy + 6.0));
        let frac = match slot.scrub {
            Some(f) => f,
            None if dur > 0 => (pos as f32 / dur as f32).clamp(0.0, 1.0),
            None => 0.0,
        };
        let out = slider(ui, base.with("seek"), seek, frac, palette, alpha, dur > 0);
        if out.started {
            slot.resume_after_scrub = !slot.engine.is_paused() && !slot.engine.has_ended();
            if slot.resume_after_scrub {
                slot.engine.pause();
            }
        }
        if let Some(v) = out.value {
            slot.scrub = Some(v);
            slot.last_activity = now;
            if now - slot.last_scrub_seek > 0.12 || out.stopped {
                slot.engine.seek_ms((v * dur as f32) as i64);
                slot.last_scrub_seek = now;
            }
        }
        if out.stopped {
            if out.value.is_none() {
                if let Some(v) = slot.scrub {
                    slot.engine.seek_ms((v * dur as f32) as i64);
                }
            }
            slot.scrub = None;
            if slot.resume_after_scrub {
                slot.engine.play();
                slot.resume_after_scrub = false;
            }
        }
    }
}

struct SliderOut {
    value: Option<f32>,
    started: bool,
    stopped: bool,
}

/// Barra horizontal (seek / volumen): pista fina que crece al pasar el mouse,
/// relleno con el color de acento y perilla.
fn slider(ui: &mut Ui, id: Id, rect: Rect, frac: f32, palette: &Palette, alpha: f32, enabled: bool) -> SliderOut {
    let hit = rect.expand2(Vec2::new(6.0, 4.0));
    let resp = ui.interact(hit, id, if enabled { Sense::click_and_drag() } else { Sense::hover() });
    let engaged = enabled && (resp.hovered() || resp.dragged());
    let thickness = if engaged { 5.0 } else { 3.0 };
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), thickness));
    let track_color = extra::blend(palette.surface, Color32::WHITE, 0.14);
    ui.painter().rect_filled(track, thickness / 2.0, track_color.gamma_multiply(alpha));
    let fill_w = rect.width() * frac.clamp(0.0, 1.0);
    if fill_w > 0.0 {
        ui.painter().rect_filled(
            Rect::from_min_size(track.min, Vec2::new(fill_w, thickness)),
            thickness / 2.0,
            palette.accent.gamma_multiply(alpha),
        );
    }
    if engaged {
        ui.painter()
            .circle_filled(Pos2::new(rect.left() + fill_w, rect.center().y), 6.0, palette.accent.gamma_multiply(alpha));
    }

    let mut out = SliderOut { value: None, started: false, stopped: false };
    if enabled {
        if resp.dragged() || resp.drag_started() || resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                out.value = Some(((p.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0));
            }
        }
        out.started = resp.drag_started();
        out.stopped = resp.drag_stopped() || resp.clicked();
        if engaged {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    out
}

// ---------------------------------------------------------------------
// Botones redondos (mismo estilo que `call_view::round_button`)
// ---------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn round_button_resp(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    icon: Icon,
    icon_size: f32,
    fill: Color32,
    fill_hover: Color32,
    fg: Color32,
    alpha: f32,
) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    let bg = if resp.hovered() { fill_hover } else { fill };
    ui.painter().rect_filled(rect, rect.height() / 2.0, bg.gamma_multiply(alpha));
    let size = if resp.is_pointer_button_down_on() { icon_size * 0.92 } else { icon_size };
    theme::paint_icon(ui, icon, rect, size, fg.gamma_multiply(alpha));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[allow(clippy::too_many_arguments)]
fn top_button(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    icon: Icon,
    tooltip: &str,
    dark: Color32,
    darker: Color32,
    alpha: f32,
) -> bool {
    round_button_resp(ui, id, rect, icon, 16.0, dark, darker, Color32::WHITE, alpha)
        .on_hover_text(tooltip)
        .clicked()
}

/// Botón redondo chico pegado a la esquina superior derecha de `rect` —
/// "cerrar"/"pantalla completa" del player y "abrir en el reproductor del
/// sistema" sobre la portada (`Icon::ExternalLink`, usado por `ui::media`)
/// comparten el estilo de los botones de la transmisión. `id_salt` distingue
/// el botón cuando puede haber más de uno con el mismo `rect` en el mismo
/// frame.
pub fn corner_button(ui: &mut Ui, rect: Rect, icon: Icon, id_salt: impl std::hash::Hash + std::fmt::Debug) -> Response {
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
) -> Response {
    let center = rect.right_top() + Vec2::new(-BTN_MARGIN - slot as f32 * BTN_STEP, BTN_MARGIN);
    let btn_rect = Rect::from_center_size(center, Vec2::splat(BTN));
    let id = ui.id().with(id_salt);
    if ui.is_rect_visible(btn_rect) {
        round_button_resp(
            ui,
            id,
            btn_rect,
            icon,
            16.0,
            Color32::from_black_alpha(150),
            Color32::from_black_alpha(210),
            Color32::WHITE,
            1.0,
        )
    } else {
        ui.interact(btn_rect, id, Sense::click())
    }
}

// ---------------------------------------------------------------------
// Utilidades de texto
// ---------------------------------------------------------------------

fn fmt_time(ms: i64) -> String {
    let total = (ms.max(0) / 1000) as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Nombre del archivo a partir de la URL (sin query, con `%xx` decodificado).
fn file_label(url: &str) -> String {
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
            if let Some(v) = decoded {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let s = String::from_utf8_lossy(&out).into_owned();
    if s.is_empty() {
        "Video".to_string()
    } else {
        s
    }
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
// GIFs de Tenor/Giphy (embeds `gifv`): autoplay, en bucle, mudos
// ---------------------------------------------------------------------
//
// Discord no manda un `.gif` para esos embeds sino un mp4 corto (`video`) y
// una portada estática (`thumbnail`). Se reproducen con el mismo `Engine`,
// pero:
//   * arrancan solos (sin click), en bucle y sin audio (no se le pasa
//     `AudioDevice`), sin barra de controles;
//   * `Engine::open` no bloquea (abre en un hilo), así que la UI sigue
//     mostrando la portada hasta que llega el primer cuadro;
//   * al repetirse usan los paquetes que ya quedaron en memoria, sin volver
//     a la red (ver `LoopCache` en `engine.rs`);
//   * solo arrancan si llevan un ratito a la vista (para no lanzar una
//     descarga por cada GIF que pasa volando al scrollear), hay un tope de
//     `MAX_LOOPING` a la vez, y se sueltan al dejar de dibujarse
//     (`end_frame`), porque cada engine tiene sus propios hilos.

// --- caché local de los mp4 de GIF --------------------------------------
//
// Antes cada GIF en mp4 se abría con ffmpeg DIRECTO sobre la URL: cada vez que
// el GIF salía de pantalla (o se pasaba el mouse por encima de nuevo en el
// selector de GIFs) el player se soltaba y, al volver, ffmpeg lo bajaba otra
// vez de Discord/Tenor (y ffmpeg suele hacer varios pedidos por archivo). Ahora
// el mp4 se baja UNA vez con el loader HTTP de la app (`support::http_cache`:
// RAM + disco, con un solo pedido por URL), se deja en una carpeta de caché y
// ffmpeg lo abre desde ahí: repetir, volver a mostrar o reabrir ya no toca la
// red.

enum Fetch {
    /// Escribiendo el archivo local en un hilo.
    Writing(Receiver<Option<String>>),
    Ready(String),
    /// No se pudo bajar/escribir; no se reintenta hasta pasado un rato.
    Failed(f64),
}

const FETCH_RETRY_SECS: f64 = 60.0;

thread_local! {
    static FETCHES: RefCell<HashMap<String, Fetch>> = RefCell::new(HashMap::new());
}

fn local_file_for(url: &str) -> Option<std::path::PathBuf> {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    static PRUNE: std::sync::Once = std::sync::Once::new();
    let dir = crate::paths::video_cache_dir()?;
    PRUNE.call_once(|| {
        let dir = dir.clone();
        std::thread::spawn(move || {
            crate::support::http_cache::prune(&dir, Duration::from_secs(7 * 24 * 3600), 256 * 1024 * 1024)
        });
    });
    let digest = Sha256::digest(url.as_bytes());
    let mut name = String::with_capacity(68);
    for b in digest.iter() {
        let _ = write!(name, "{b:02x}");
    }
    name.push_str(".mp4");
    Some(dir.join(name))
}

/// Ruta local del mp4 `url` cuando ya está bajado (`None` = todavía se está
/// bajando o no se pudo). Hay que llamarla en cada frame hasta que devuelva
/// `Some`; el pedido de red es uno solo por URL sin importar cuántas veces se
/// llame.
pub fn local_video(ctx: &egui::Context, url: &str) -> Option<String> {
    let now = ctx.input(|i| i.time);
    FETCHES.with_borrow_mut(|map| {
        match map.get(url) {
            Some(Fetch::Ready(p)) => return Some(p.clone()),
            Some(Fetch::Failed(at)) if now - *at < FETCH_RETRY_SECS => return None,
            Some(Fetch::Writing(rx)) => {
                let next = match rx.try_recv() {
                    Ok(Some(path)) => Fetch::Ready(path),
                    Ok(None) | Err(TryRecvError::Disconnected) => Fetch::Failed(now),
                    Err(TryRecvError::Empty) => {
                        ctx.request_repaint_after(Duration::from_millis(80));
                        return None;
                    }
                };
                let out = if let Fetch::Ready(p) = &next { Some(p.clone()) } else { None };
                map.insert(url.to_string(), next);
                return out;
            }
            _ => {}
        }
        match ctx.try_load_bytes(url) {
            Ok(egui::load::BytesPoll::Ready { bytes, .. }) => {
                let Some(path) = local_file_for(url) else {
                    map.insert(url.to_string(), Fetch::Failed(now));
                    return None;
                };
                let (tx, rx) = mpsc::channel();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let data: &[u8] = bytes.as_ref();
                    let same = std::fs::metadata(&path).map(|m| m.len() == data.len() as u64).unwrap_or(false);
                    let ok = same || {
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let tmp = path.with_extension("tmp");
                        std::fs::write(&tmp, data).is_ok() && std::fs::rename(&tmp, &path).is_ok()
                    };
                    let _ = tx.send(ok.then(|| path.to_string_lossy().into_owned()));
                    ctx2.request_repaint();
                });
                map.insert(url.to_string(), Fetch::Writing(rx));
                None
            }
            // El loader HTTP pide un repaint solo cuando termina.
            Ok(egui::load::BytesPoll::Pending { .. }) => None,
            Err(_) => {
                map.insert(url.to_string(), Fetch::Failed(now));
                None
            }
        }
    })
}

/// Reproductores de GIF simultáneos (el resto se queda con la portada).
const MAX_LOOPING: usize = 6;
/// Segundos a la vista antes de arrancar la descarga/decodificación.
const LOOP_START_DELAY: f64 = 0.3;
/// Segundos sin dibujarse para soltar el player.
const LOOP_IDLE_DROP: f64 = 1.0;

enum LoopKind {
    /// Visible desde hace poco: todavía no vale la pena arrancarlo.
    Waiting,
    Running(Box<Engine>),
    /// No se pudo abrir (sin ffmpeg, códec no soportado, red…): se queda la
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

/// Dibuja el video `url` (mp4 de un embed `gifv`) en `rect`, en bucle y sin
/// audio, con las esquinas redondeadas `radius`. Devuelve `true` si lo está
/// mostrando (quien llama debería repintar encima lo que quiera, p. ej. la
/// insignia "GIF"), o `false` mientras no hay video (esperando, cargando,
/// falló o no hay lugar): en ese caso queda lo que quien llama ya pintó, o
/// sea la portada.
pub fn show_looping(ui: &mut Ui, url: &str, rect: Rect, radius: u8) -> bool {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    LOOPS.with_borrow_mut(|st| {
        let active = st.slots.values().filter(|s| matches!(s.kind, LoopKind::Running(_))).count();
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
            // Se abre desde el archivo local (ver `local_video`): ni al repetir
            // ni al volver a mostrarlo se vuelve a pedir nada a la red.
            let Some(path) = local_video(&ctx, url) else {
                return false;
            };
            slot.kind = LoopKind::Running(Box::new(Engine::open(&ctx, &path, None, true)));
        }

        let mut failed = false;
        let shown = match &mut slot.kind {
            LoopKind::Running(engine) => {
                engine.update(&ctx);
                if engine.failed() {
                    failed = true;
                    false
                } else if engine.has_frame() {
                    egui::Image::new(SizedTexture::new(engine.texture_id(), rect.size()))
                        .corner_radius(CornerRadius::same(radius))
                        .paint_at(ui, rect);
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if failed {
            slot.kind = LoopKind::Failed;
        }
        shown
    })
}

/// Suelta los players de GIF que dejaron de dibujarse. **Hay que llamarla
/// una vez por frame, al final de `App::ui`** (junto a `show_fullscreen`): si
/// un GIF sale de pantalla ya nadie lo dibuja, y sin esto su engine seguiría
/// decodificando para siempre.
pub fn end_frame(ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    LOOPS.with_borrow_mut(|st| {
        st.slots.retain(|_, s| now - s.last_seen <= LOOP_IDLE_DROP);
    });
}
