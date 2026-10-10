//! Vista central de un canal de voz, al estilo del cliente oficial.
//!
//! Estados:
//!
//! * **Sin integrantes**: degradado con el color de acento del tema, nombre
//!   del canal, "No hay nadie en el chat de voz" y "Unirse a canal de voz".
//! * **Con integrantes, sin stream**: grilla de tiles. Cada transmisión es
//!   un tile aparte, como si fuera otra persona, pegado al de quien transmite:
//!   con la miniatura de fondo y un botón "Ver transmisión" en el centro; el
//!   propio muestra lo que estás transmitiendo.
//! * **Con integrantes, viendo un stream**: dos miradas que se alternan
//!   con un click (sobre el stream o sobre los integrantes):
//!     - *Escenario* (por defecto): el stream grande y los integrantes en una
//!       tira debajo. La tira se puede ocultar con el botón de integrantes.
//!     - *Grilla*: el stream es un tile más, junto a los integrantes.
//! * **Pantalla completa** del stream (botón, `Esc` o doble click para salir).
//!
//! * **Popup**: si seguís viendo un stream pero salís de la vista de la
//!   llamada (otro canal, DMs, inicio…), el stream queda en una ventanita
//!   flotante que se mueve arrastrándola y se redimensiona desde las esquinas
//!   (`show_popup`).
//!
//! Con la ventana en segundo plano el video no se procesa (`App::video_paused`)
//! y el tile muestra "Para ahorrar recursos este video se pausó".
//!
//! Los controles (barra de mic/ensordecer/cámara/transmitir/chat/ajustes/colgar,
//! que reemplaza a la barra inferior de `ui::call_bar` mientras estás acá, y los botones sobre el
//! stream) se desvanecen tras `IDLE_SECS` sin mover el mouse ni tocar teclas.
//!
//! Los clics se juntan en `CallEvents` y se aplican al final del frame, con
//! el mismo patrón que `ui::call_bar`. El estado de la vista (mirada, tira
//! oculta, pantalla completa, última actividad) vive en `ctx.data`.

use std::time::Duration;

use egui::{
    Align, Align2, Color32, CornerRadius, Layout, Pos2, Rect, Sense, Stroke, StrokeKind, UiBuilder,
    Vec2,
};

use crate::discord::{StreamPublishStatus, StreamWatchStatus};
use crate::lib::data::VoiceOccupant;
use crate::lib::state::{App, Screen};
use crate::ui::emoji as twemoji;
use crate::ui::extra;
use crate::ui::theme::{self, Icon, Palette};

const TILE_GAP: f32 = 12.0;
const TILE_RADIUS: u8 = 14;
const TILE_ASPECT: f32 = 16.0 / 9.0;
const OUTER_PAD: f32 = 16.0;
const CONTROLS_H: f32 = 60.0;
const CONTROLS_MARGIN_BOTTOM: f32 = 14.0;
const STRIP_H: f32 = 128.0;

/// Segundos sin actividad antes de esconder los controles.
const IDLE_SECS: f64 = 3.0;
/// Duración del fundido de los controles.
const FADE_SECS: f32 = 0.25;

// Barra de controles.
const BTN_H: f32 = 44.0;
const BAR_PAD: f32 = 8.0;
const BAR_GAP: f32 = 8.0;
const W_ROUND: f32 = 48.0;
const W_LEAVE: f32 = 64.0;
/// Mic, ensordecer, cámara, transmitir, chat de voz y ajustes + desconectar.
const BAR_ROUND_BTNS: f32 = 6.0;
const BAR_W: f32 = W_ROUND * BAR_ROUND_BTNS + W_LEAVE + BAR_GAP * BAR_ROUND_BTNS + BAR_PAD * 2.0;

// ---------------------------------------------------------------------
// Estado de la vista
// ---------------------------------------------------------------------

#[derive(Clone)]
struct ViewState {
    /// `true` = escenario + tira (por defecto); `false` = grilla con el
    /// stream como un tile más.
    focus: bool,
    /// Tira de integrantes oculta en la mirada de escenario.
    strip_hidden: bool,
    fullscreen: bool,
    /// Si la ventana ya estaba en pantalla completa del SO antes de entrar.
    restore_os_fullscreen: bool,
    /// Si la ventana estaba maximizada antes de entrar (hay que
    /// des-maximizarla para que la pantalla completa tape la barra de tareas
    /// y volver a maximizarla al salir).
    restore_maximized: bool,
    last_activity: f64,
    /// Pasada de egui en la que se dibujó por última vez (ver `end_frame`).
    last_seen_pass: u64,
    /// Última pasada en la que el stream se dibujó DENTRO de la vista de la
    /// llamada; si no es la actual, `show_popup` lo muestra flotando.
    stage_seen_pass: u64,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            focus: true,
            strip_hidden: false,
            fullscreen: false,
            restore_os_fullscreen: false,
            restore_maximized: false,
            last_activity: 0.0,
            last_seen_pass: 0,
            stage_seen_pass: 0,
        }
    }
}

fn state_id() -> egui::Id {
    egui::Id::new("ecord_call_view_state")
}

fn load_state(ctx: &egui::Context) -> ViewState {
    ctx.data_mut(|d| d.get_temp::<ViewState>(state_id())).unwrap_or_default()
}

fn store_state(ctx: &egui::Context, vs: ViewState) {
    ctx.data_mut(|d| d.insert_temp(state_id(), vs));
}

fn enter_fullscreen(ctx: &egui::Context, vs: &mut ViewState) {
    if vs.fullscreen {
        return;
    }
    let (fullscreen, maximized) = ctx.input(|i| {
        let vp = i.viewport();
        (vp.fullscreen.unwrap_or(false), vp.maximized.unwrap_or(false))
    });
    vs.restore_os_fullscreen = fullscreen;
    vs.restore_maximized = maximized && !fullscreen;
    vs.fullscreen = true;
    // En Windows, una ventana sin decoraciones que sigue "maximizada" queda
    // limitada al área de trabajo aunque se pida pantalla completa, y la barra
    // de tareas se ve debajo. Des-maximizar primero deja que la pantalla
    // completa ocupe el monitor entero.
    if vs.restore_maximized {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
}

fn exit_fullscreen(ctx: &egui::Context, vs: &mut ViewState) {
    if !vs.fullscreen {
        return;
    }
    vs.fullscreen = false;
    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(vs.restore_os_fullscreen));
    if vs.restore_maximized {
        vs.restore_maximized = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
    }
}

/// **Llamar una vez por frame, al final de `App::ui`** (junto a
/// `video_player::end_frame`). Si quedó en pantalla completa pero esta vista
/// dejó de dibujarse (te sacaron de la llamada, cambiaste de pantalla…), sale
/// de pantalla completa para que la ventana no quede atrapada.
pub fn end_frame(ctx: &egui::Context) {
    let mut vs = load_state(ctx);
    if vs.fullscreen && vs.last_seen_pass != ctx.cumulative_pass_nr() {
        exit_fullscreen(ctx, &mut vs);
        store_state(ctx, vs);
    }
}

// ---------------------------------------------------------------------
// Eventos y datos
// ---------------------------------------------------------------------

/// Todo lo que el usuario pudo pedir en este frame.
#[derive(Default)]
struct CallEvents {
    join: bool,
    leave: bool,
    toggle_mute: bool,
    toggle_deafen: bool,
    /// Transmitir pantalla / cortar la transmisión.
    toggle_share: bool,
    /// Mostrar / ocultar el panel del chat de voz.
    toggle_chat: bool,
    open_settings: bool,
    close_stream: bool,
    /// Alterna entre escenario y grilla.
    toggle_layout: bool,
    toggle_strip: bool,
    enter_fullscreen: bool,
    exit_fullscreen: bool,
    /// (user_id, nombre) de quien se quiere ver.
    watch: Option<(String, String)>,
    /// Menú de clic derecho del stream: (id de quien transmite, volumen en %).
    stream_volume: Option<(String, u16)>,
    stream_muted: Option<(String, bool)>,
    stream_reset: Option<String>,
}

/// Datos del stream que se está viendo en este canal (ya clonados).
struct StageData {
    /// Id de usuario de quien transmite (clave del volumen del stream).
    owner_id: String,
    /// Volumen/silencio local del audio de este stream.
    playback: crate::discord::VoiceParticipantPlaybackSettings,
    owner_name: String,
    status: StreamWatchStatus,
    message: Option<String>,
    frame_size: [usize; 2],
    texture: Option<egui::TextureHandle>,
    /// Ventana en segundo plano: el video está pausado.
    paused: bool,
}

/// Lo que se ve en el tile de NUESTRA transmisión (vista previa local).
struct OwnStream {
    status: StreamPublishStatus,
    texture: Option<egui::TextureHandle>,
    size: [usize; 2],
}

/// Contexto común a todos los tiles de integrantes.
struct TileCtx<'a> {
    my_id: Option<&'a str>,
    /// Los tiles son clickeables (en la tira: pasan a la grilla).
    clickable: bool,
    /// Volumen/silencio local de cada integrante (por user_id), para el menú
    /// de clic derecho.
    playback: &'a std::collections::HashMap<String, crate::discord::VoiceParticipantPlaybackSettings>,
}

/// Qué va en cada casillero de la grilla / la tira.
#[derive(Clone, Copy)]
enum Item {
    /// Tile de una persona (índice en `members`).
    Member(usize),
    /// Tile de la transmisión de esa persona (miniatura + "Ver transmisión",
    /// o la vista previa propia).
    Stream(usize),
    /// La transmisión que se está viendo, como un tile más (mirada de grilla).
    Watched,
}

/// Todo lo que hace falta para dibujar los casilleros.
struct Scene<'a> {
    members: &'a [VoiceOccupant],
    speaking: &'a [bool],
    stage: Option<&'a StageData>,
    own: Option<&'a OwnStream>,
    /// Miniatura (URL) de la transmisión de cada persona, por user_id.
    previews: &'a std::collections::HashMap<String, Option<String>>,
    connected_here: bool,
    /// Ventana en segundo plano: no se procesa video.
    video_paused: bool,
    tc: &'a TileCtx<'a>,
}

/// Arma los casilleros: cada integrante y, pegada a él, su transmisión como
/// otro tile. `hide_watched` saca la que se está viendo (en el escenario ya
/// ocupa el lugar grande).
fn build_items(
    members: &[VoiceOccupant],
    my_id: Option<&str>,
    watching_user: Option<&str>,
    own_stream: bool,
    has_stage: bool,
    hide_watched: bool,
) -> Vec<Item> {
    let mut items = Vec::with_capacity(members.len() * 2);
    let mut watched_placed = false;
    for (i, m) in members.iter().enumerate() {
        items.push(Item::Member(i));
        let is_me = my_id == Some(m.user_id.as_str());
        if !(m.streaming || (is_me && own_stream)) {
            continue;
        }
        if !is_me && watching_user == Some(m.user_id.as_str()) {
            watched_placed = true;
            if !hide_watched {
                items.push(Item::Watched);
            }
        } else {
            items.push(Item::Stream(i));
        }
    }
    // Se mira un stream cuyo dueño ya no figura entre los integrantes.
    if has_stage && !watched_placed && !hide_watched {
        items.insert(0, Item::Watched);
    }
    items
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamMode {
    Stage,
    GridTile,
    Fullscreen,
}

impl StreamMode {
    fn tag(self) -> &'static str {
        match self {
            StreamMode::Stage => "stage",
            StreamMode::GridTile => "grid",
            StreamMode::Fullscreen => "fullscreen",
        }
    }
}

// ---------------------------------------------------------------------
// Entrada
// ---------------------------------------------------------------------

pub fn show(
    app: &mut App,
    ui: &mut egui::Ui,
    palette: &Palette,
    server_index: usize,
    cat: usize,
    chan: usize,
    channel_name: &str,
) {
    let server = &app.servers[server_index];
    let guild_id = server.guild_id.clone();
    let channel = server.channel(cat, chan);
    let members: Vec<VoiceOccupant> = channel.map(|c| c.voice_members.clone()).unwrap_or_default();
    let channel_id = channel.and_then(|c| c.channel_id.clone());
    let connected_here = channel_id
        .as_deref()
        .is_some_and(|id| app.is_connected_to_voice_channel_str(&guild_id, id));
    let my_id: Option<String> = app.me.as_ref().map(|me| me.id.clone());
    let speaking: Vec<bool> = members
        .iter()
        .map(|m| app.user_voice_speaking_in_guild_str(&guild_id, &m.user_id))
        .collect();
    let playback: std::collections::HashMap<String, crate::discord::VoiceParticipantPlaybackSettings> =
        members
            .iter()
            .map(|m| (m.user_id.clone(), app.voice_participant_playback(&m.user_id)))
            .collect();
    let (self_mute, self_deaf) = app
        .voice_target
        .as_ref()
        .map(|t| (t.self_mute, t.self_deaf))
        .unwrap_or((app.self_mute, app.self_deaf));

    // Mi transmisión, si sale de ESTE canal: el tile propio muestra la vista
    // previa local.
    let own_stream: Option<OwnStream> = app
        .broadcasting_stream
        .as_ref()
        .filter(|b| Some(&b.channel_id) == channel_id.as_ref())
        .map(|b| OwnStream {
            status: b.status,
            texture: b.preview_texture.clone(),
            size: b.preview_size,
        });
    let video_paused = app.video_paused;
    // Transmitir solo tiene sentido con la llamada ya conectada; si ya hay una
    // transmisión en curso el botón sirve para cortarla en cualquier estado.
    let broadcasting = app.is_broadcasting();
    let share_enabled = broadcasting || app.voice_fully_connected();
    // Mientras no hay nadie en pantalla se muestra en qué fase va la conexión.
    let connect_label =
        crate::ui::call_bar::connection_label(app.voice_connection_status, app.voice_connect_phase);
    // Miniaturas de las transmisiones de los demás (se piden a Discord en
    // segundo plano y se refrescan cada tanto).
    let previews: std::collections::HashMap<String, Option<String>> = match channel_id.as_deref() {
        Some(cid) => members
            .iter()
            .filter(|m| m.streaming && my_id.as_deref() != Some(m.user_id.as_str()))
            .map(|m| {
                let key = format!("guild:{guild_id}:{cid}:{}", m.user_id);
                (m.user_id.clone(), app.stream_preview_url(&key))
            })
            .collect(),
        None => std::collections::HashMap::new(),
    };

    // Stream que se está viendo, solo si es de ESTE canal.
    let watched_here = app
        .watching_stream
        .as_ref()
        .filter(|w| Some(&w.channel_id) == channel_id.as_ref());
    let stage: Option<StageData> = watched_here.map(|w| StageData {
        owner_id: w.stream_key.rsplit(':').next().unwrap_or_default().to_owned(),
        playback: app.voice_stream_playback(w.stream_key.rsplit(':').next().unwrap_or_default()),
        owner_name: w.owner_name.clone(),
        status: w.status,
        message: w.message.clone(),
        frame_size: w.frame_size,
        texture: w.texture.clone(),
        paused: w.paused,
    });
    let watching_user: Option<String> =
        watched_here.and_then(|w| w.stream_key.rsplit(':').next().map(str::to_owned));

    let ctx = ui.ctx().clone();
    let mut vs = load_state(&ctx);
    let now = ctx.input(|i| i.time);
    vs.last_seen_pass = ctx.cumulative_pass_nr();
    if vs.last_activity == 0.0 {
        vs.last_activity = now;
    }
    if stage.is_some() {
        vs.stage_seen_pass = ctx.cumulative_pass_nr();
    }
    if stage.is_none() {
        // Sin stream no hay nada que mantener: la próxima vez arranca en la
        // mirada por defecto.
        exit_fullscreen(&ctx, &mut vs);
        vs.focus = true;
        vs.strip_hidden = false;
    }

    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());

    let mut ev = CallEvents::default();
    if vs.fullscreen && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        ev.exit_fullscreen = true;
    }

    // --- Inactividad -> fundido de los controles ---
    let region = if vs.fullscreen { ctx.viewport_rect() } else { full };
    let bar_center = if vs.fullscreen {
        Pos2::new(region.center().x, region.bottom() - 40.0 - CONTROLS_H / 2.0)
    } else {
        Pos2::new(full.center().x, full.bottom() - CONTROLS_MARGIN_BOTTOM - CONTROLS_H / 2.0)
    };
    let bar = Rect::from_center_size(bar_center, Vec2::new(BAR_W, CONTROLS_H));
    let active = ctx.input(|i| {
        let pos = i.pointer.hover_pos();
        let inside = pos.is_some_and(|p| region.contains(p));
        let moved = i.pointer.delta() != Vec2::ZERO
            || i.pointer.any_pressed()
            || i.events
                .iter()
                .any(|e| matches!(e, egui::Event::Key { pressed: true, .. }));
        (inside && moved) || pos.is_some_and(|p| bar.contains(p))
    });
    if active {
        vs.last_activity = now;
    }
    let idle = now - vs.last_activity > IDLE_SECS;
    let alpha = ctx.animate_bool_with_time(egui::Id::new("call_controls_visible"), !idle, FADE_SECS);
    if !idle {
        let remaining = (IDLE_SECS - (now - vs.last_activity)).max(0.0) + 0.05;
        ctx.request_repaint_after(Duration::from_secs_f64(remaining));
    }

    if members.is_empty() && !connected_here && stage.is_none() {
        empty_state(ui, palette, full, channel_name, channel_id.is_some(), &mut ev);
    } else {
        ui.painter().rect_filled(
            full,
            0.0,
            extra::blend(palette.window_solid, Color32::BLACK, 0.18),
        );

        let content = Rect::from_min_max(
            full.min + Vec2::splat(OUTER_PAD),
            Pos2::new(
                full.max.x - OUTER_PAD,
                full.max.y - OUTER_PAD - CONTROLS_H - CONTROLS_MARGIN_BOTTOM,
            ),
        );

        if members.is_empty() && stage.is_none() {
            ui.painter().text(
                content.center(),
                Align2::CENTER_CENTER,
                connect_label,
                theme::medium(14.0),
                palette.dim,
            );
        } else {
            // En la mirada de escenario el stream que se ve ocupa el lugar
            // grande y el resto va en la tira de abajo.
            let stage_mode = stage.is_some() && vs.focus;
            let items = build_items(
                &members,
                my_id.as_deref(),
                watching_user.as_deref(),
                own_stream.is_some(),
                stage.is_some(),
                stage_mode,
            );
            let tc = TileCtx {
                my_id: my_id.as_deref(),
                clickable: stage_mode,
                playback: &playback,
            };
            let scene = Scene {
                members: &members,
                speaking: &speaking,
                stage: stage.as_ref(),
                own: own_stream.as_ref(),
                previews: &previews,
                connected_here,
                video_paused,
                tc: &tc,
            };
            match stage.as_ref().filter(|_| stage_mode) {
                Some(stage) => {
                    // Escenario + tira (ocultable).
                    let strip = if vs.strip_hidden || items.is_empty() {
                        Vec::new()
                    } else {
                        strip_layout(content, items.len())
                    };
                    let stage_bottom = strip
                        .first()
                        .map(|r| r.top() - TILE_GAP)
                        .unwrap_or(content.bottom())
                        .max(content.top() + 80.0);
                    let stage_rect =
                        Rect::from_min_max(content.min, Pos2::new(content.max.x, stage_bottom));
                    draw_stream(ui, palette, stage_rect, stage, StreamMode::Stage, vs.strip_hidden, alpha, &mut ev);
                    for (item, rect) in items.iter().zip(strip.iter()) {
                        draw_item(ui, palette, *rect, *item, &scene, alpha, &mut ev);
                    }
                }
                None => {
                    // Grilla: cada integrante, su transmisión como otro tile
                    // y, si se está viendo, el stream en vivo en su lugar.
                    for (item, rect) in items.iter().zip(grid_layout(content, items.len()).iter()) {
                        draw_item(ui, palette, *rect, *item, &scene, alpha, &mut ev);
                    }
                }
            }
        }

        // Barra inferior (en pantalla completa la dibuja el overlay).
        if !vs.fullscreen {
            if connected_here {
                controls_bar(ui, palette, bar, self_mute, self_deaf, broadcasting, share_enabled, alpha, "base", &mut ev);
            } else if channel_id.is_some() {
                // "Unirse" es la acción principal: no se desvanece.
                let rect = Rect::from_center_size(bar.center(), Vec2::new(260.0, CONTROLS_H));
                ui.scope_builder(
                    UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Center)),
                    |ui| {
                        ui.add_space((CONTROLS_H - 34.0) / 2.0);
                        if theme::pill_button(ui, palette, "Unirse a la llamada", true).clicked() {
                            ev.join = true;
                        }
                    },
                );
            }
        }
    }

    // --- Pantalla completa: overlay encima de toda la ventana ---
    if vs.fullscreen {
        if let Some(stage) = stage.as_ref() {
            let screen = ctx.viewport_rect();
            egui::Area::new(egui::Id::new("ecord_call_fullscreen"))
                .order(egui::Order::Tooltip)
                .fixed_pos(screen.min)
                .show(&ctx, |ui| {
                    ui.set_width(screen.width());
                    ui.set_height(screen.height());
                    ui.painter().rect_filled(screen, 0.0, Color32::BLACK);
                    draw_stream(ui, palette, screen, stage, StreamMode::Fullscreen, false, alpha, &mut ev);
                    if connected_here {
                        controls_bar(ui, palette, bar, self_mute, self_deaf, broadcasting, share_enabled, alpha, "fs", &mut ev);
                    }
                });
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    // --- Aplicar eventos (ya sin nada prestado) ---
    if ev.exit_fullscreen {
        exit_fullscreen(&ctx, &mut vs);
    } else if ev.enter_fullscreen && stage.is_some() {
        enter_fullscreen(&ctx, &mut vs);
    }
    if ev.toggle_layout {
        vs.focus = !vs.focus;
    }
    if ev.toggle_strip {
        vs.strip_hidden = !vs.strip_hidden;
    }
    if vs.fullscreen && idle {
        ctx.set_cursor_icon(egui::CursorIcon::None);
    }
    store_state(&ctx, vs);

    if ev.close_stream {
        app.stop_watching_stream();
    }
    if let (Some((owner_id, owner_name)), Some(channel_id)) = (ev.watch.as_ref(), channel_id.as_deref()) {
        app.watch_stream(&guild_id, channel_id, owner_id, owner_name);
    }
    if let Some((owner_id, percent)) = ev.stream_volume.as_ref() {
        app.set_voice_stream_volume(owner_id, *percent);
    }
    if let Some((owner_id, muted)) = ev.stream_muted.as_ref() {
        app.set_voice_stream_muted(owner_id, *muted);
    }
    if let Some(owner_id) = ev.stream_reset.as_ref() {
        app.reset_voice_stream_audio(owner_id);
    }
    if ev.toggle_chat {
        // El panel del chat de voz lo dibuja `ui::server` (abierto por defecto).
        let chat_id = egui::Id::new("voice_chat_open");
        ctx.data_mut(|d| {
            let open = d.get_temp::<bool>(chat_id).unwrap_or(true);
            d.insert_temp(chat_id, !open);
        });
        ctx.request_repaint();
    }
    if ev.open_settings {
        app.settings_open = true;
    }
    if ev.leave {
        app.leave_voice();
    } else if ev.toggle_deafen {
        app.toggle_self_deafen();
    } else if ev.toggle_mute {
        app.toggle_self_mute();
    } else if ev.toggle_share {
        app.toggle_broadcast();
    }
    if ev.join {
        if let Some(channel_id) = channel_id {
            app.join_voice_channel(&guild_id, &channel_id);
        }
    }
}

// ---------------------------------------------------------------------
// Popup flotante del stream (cuando no estás en la vista de la llamada)
// ---------------------------------------------------------------------

const POPUP_DEFAULT_W: f32 = 360.0;
const POPUP_MIN_W: f32 = 220.0;
const POPUP_MARGIN: f32 = 16.0;
const POPUP_HANDLE: f32 = 16.0;
const POPUP_BAR_H: f32 = 40.0;

/// Posición y ancho del popup; el alto sale del ancho y la proporción del
/// video. Se conserva mientras corre la app.
#[derive(Clone, Default)]
struct PopupState {
    pos: Option<Pos2>,
    width: f32,
}

fn popup_state_id() -> egui::Id {
    egui::Id::new("ecord_stream_popup_state")
}

/// Lleva a la vista del canal de voz donde está el stream.
fn go_to_call(app: &mut App, stream_key: &str, channel_id: &str) {
    // `guild:<guild>:<canal>:<usuario>`
    let Some(guild_id) = stream_key.split(':').nth(1) else { return };
    let Some(index) = app.servers.iter().position(|s| s.guild_id == guild_id) else { return };
    if !matches!(app.screen, Screen::Server(i) if i == index) {
        app.open_server(index);
    }
    let Some((cat, chan)) = app.servers[index].channel_position_by_id(channel_id) else { return };
    app.open_channel(cat, chan);
}

/// **Llamar una vez por frame, al final de `App::ui`** (antes de los modales).
/// Si hay un stream que se está viendo y no se dibujó dentro de la vista de la
/// llamada este frame, lo muestra en una ventanita flotante:
/// * arrastrar el video la mueve;
/// * arrastrar cualquier esquina la redimensiona (manteniendo la proporción);
/// * arriba, al pasar el mouse: volver a la llamada y dejar de ver;
/// * doble click también vuelve a la llamada.
pub fn show_popup(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let Some(watched) = app.watching_stream.as_ref() else { return };
    let vs = load_state(&ctx);
    if vs.stage_seen_pass == ctx.cumulative_pass_nr() {
        return;
    }
    let stage = StageData {
        owner_id: watched.stream_key.rsplit(':').next().unwrap_or_default().to_owned(),
        playback: app
            .voice_stream_playback(watched.stream_key.rsplit(':').next().unwrap_or_default()),
        owner_name: watched.owner_name.clone(),
        status: watched.status,
        message: watched.message.clone(),
        frame_size: watched.frame_size,
        texture: watched.texture.clone(),
        paused: watched.paused,
    };
    let stream_key = watched.stream_key.clone();
    let channel_id = watched.channel_id.clone();
    let palette = app.palette;

    let screen = ctx.viewport_rect();
    let aspect = if stage.frame_size[0] > 0 && stage.frame_size[1] > 0 {
        stage.frame_size[0] as f32 / stage.frame_size[1] as f32
    } else {
        TILE_ASPECT
    };
    let mut ps = ctx.data_mut(|d| d.get_temp::<PopupState>(popup_state_id())).unwrap_or_default();
    if ps.width <= 0.0 {
        ps.width = POPUP_DEFAULT_W;
    }
    // Que entre en la ventana (ancho y alto).
    let max_w = (screen.width() - POPUP_MARGIN * 2.0)
        .min((screen.height() - POPUP_MARGIN * 2.0) * aspect)
        .max(80.0);
    let min_w = POPUP_MIN_W.min(max_w);
    ps.width = ps.width.clamp(min_w, max_w);
    let size = Vec2::new(ps.width, ps.width / aspect);
    let mut pos = ps
        .pos
        .unwrap_or(Pos2::new(screen.right() - size.x - POPUP_MARGIN, screen.bottom() - size.y - POPUP_MARGIN));
    let clamp_pos = |p: Pos2, size: Vec2| {
        Pos2::new(
            p.x.clamp(screen.left(), (screen.right() - size.x).max(screen.left())),
            p.y.clamp(screen.top(), (screen.bottom() - size.y).max(screen.top())),
        )
    };
    pos = clamp_pos(pos, size);

    let mut go_back = false;
    let mut close = false;

    egui::Area::new(egui::Id::new("ecord_stream_popup"))
        .order(egui::Order::Middle)
        .fixed_pos(pos)
        .interactable(true)
        .show(&ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let radius = CornerRadius::same(10);

            // Cuerpo: arrastrar mueve, doble click vuelve a la llamada.
            let body = ui
                .interact(rect, egui::Id::new("popup_body"), Sense::click_and_drag())
                .on_hover_cursor(egui::CursorIcon::Grab);
            if body.dragged() {
                pos += body.drag_delta();
            }
            if body.double_clicked() {
                go_back = true;
            }

            // Video.
            ui.painter().rect_filled(rect, radius, Color32::BLACK);
            match stage.texture.as_ref() {
                Some(texture) if stage.frame_size[0] > 0 && stage.frame_size[1] > 0 => {
                    egui::Image::new(texture).corner_radius(radius).paint_at(ui, rect);
                }
                _ => {
                    let (text, color) = match stage.status {
                        StreamWatchStatus::Failed => (
                            stage
                                .message
                                .clone()
                                .unwrap_or_else(|| "No se pudo conectar al stream".to_string()),
                            palette.danger,
                        ),
                        _ => ("Conectando al stream…".to_string(), palette.dim),
                    };
                    ui.painter()
                        .text(rect.center(), Align2::CENTER_CENTER, text, theme::regular(12.0), color);
                }
            }
            if stage.paused {
                paused_notice(ui, rect, radius, &palette);
            }

            // Barra superior que aparece al pasar el mouse.
            let hovered = ctx
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|p| rect.contains(p));
            let alpha = ctx.animate_bool_with_time(
                egui::Id::new("popup_controls_visible"),
                hovered || body.dragged(),
                0.15,
            );
            if alpha > 0.02 {
                let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), POPUP_BAR_H));
                ui.painter().rect_filled(
                    bar,
                    CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 },
                    Color32::from_black_alpha(150).gamma_multiply(alpha),
                );
                let font = theme::semibold(12.0);
                let title = format!("Stream de {}", stage.owner_name);
                let max_title = (rect.width() - 18.0 - 76.0 - 8.0).max(30.0);
                let (title, _) = fit_text(ui, &title, &font, max_title);
                ui.painter().text(
                    Pos2::new(rect.left() + 18.0, bar.center().y),
                    Align2::LEFT_CENTER,
                    title,
                    font,
                    Color32::WHITE.gamma_multiply(alpha),
                );
            }
            let btn = |n: f32| {
                Rect::from_center_size(
                    Pos2::new(rect.right() - 32.0 - n * 30.0, rect.top() + POPUP_BAR_H / 2.0),
                    Vec2::splat(24.0),
                )
            };
            let dark = Color32::from_black_alpha(120);
            let darker = Color32::from_black_alpha(210);
            if round_button(
                ui, "popup_close", btn(0.0), Icon::X, 14.0, dark, darker, Color32::WHITE,
                "Dejar de ver el stream", alpha,
            ) {
                close = true;
            }
            if round_button(
                ui, "popup_back", btn(1.0), Icon::Volume2, 14.0, dark, darker, Color32::WHITE,
                "Volver a la llamada", alpha,
            ) {
                go_back = true;
            }

            // Esquinas para redimensionar (se registran al final: ganan el
            // click sobre el cuerpo).
            for (i, (fx, fy)) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].into_iter().enumerate() {
                let corner = Pos2::new(rect.left() + fx * rect.width(), rect.top() + fy * rect.height());
                let inward = Vec2::new(1.0 - 2.0 * fx, 1.0 - 2.0 * fy) * (POPUP_HANDLE / 2.0);
                let handle = Rect::from_center_size(corner + inward, Vec2::splat(POPUP_HANDLE));
                let cursor = if fx == fy {
                    egui::CursorIcon::ResizeNwSe
                } else {
                    egui::CursorIcon::ResizeNeSw
                };
                let resp = ui
                    .interact(handle, egui::Id::new(("popup_resize", i)), Sense::drag())
                    .on_hover_cursor(cursor);
                if alpha > 0.02 && fy == 1.0 {
                    // Marca de agarre (dos rayitas en diagonal) en las esquinas de abajo.
                    let sx = if fx == 1.0 { -1.0 } else { 1.0 };
                    let color = Color32::from_white_alpha((alpha * 160.0) as u8);
                    for off in [7.5f32, 11.0] {
                        ui.painter().line_segment(
                            [
                                Pos2::new(corner.x + sx * off, corner.y - 3.0),
                                Pos2::new(corner.x + sx * 3.0, corner.y - off),
                            ],
                            Stroke::new(1.5, color),
                        );
                    }
                }
                if resp.dragged() {
                    let d = resp.drag_delta();
                    let dw_x = if fx == 1.0 { d.x } else { -d.x };
                    let dh_y = if fy == 1.0 { d.y } else { -d.y };
                    // Los dos ejes aportan: se promedian en ancho.
                    let dw = (dw_x + dh_y * aspect) / 2.0;
                    let new_w = (ps.width + dw).clamp(min_w, max_w);
                    let applied = new_w - ps.width;
                    ps.width = new_w;
                    if fx == 0.0 {
                        pos.x -= applied;
                    }
                    if fy == 0.0 {
                        pos.y -= applied / aspect;
                    }
                }
            }
        });

    ps.pos = Some(clamp_pos(pos, Vec2::new(ps.width, ps.width / aspect)));
    ctx.data_mut(|d| d.insert_temp(popup_state_id(), ps));

    if close {
        app.stop_watching_stream();
    } else if go_back {
        go_to_call(app, &stream_key, &channel_id);
    }
}

// ---------------------------------------------------------------------
// Estado "sin integrantes"
// ---------------------------------------------------------------------

fn empty_state(
    ui: &mut egui::Ui,
    palette: &Palette,
    full: Rect,
    channel_name: &str,
    can_join: bool,
    ev: &mut CallEvents,
) {
    let base = palette.window_solid;
    let top = extra::blend(base, palette.accent, 0.04);
    let bottom = extra::blend(base, palette.accent, 0.38);
    ui.painter().add(gradient_mesh(full, top, bottom));

    // Resplandor abajo al centro: círculos concéntricos con alfa bajo,
    // recortados al rectángulo de la vista.
    let painter = ui.painter_at(full);
    let glow_center = Pos2::new(full.center().x, full.bottom() + 30.0);
    let (r, g, b) = (palette.accent.r(), palette.accent.g(), palette.accent.b());
    for i in 0..14u32 {
        let radius = 70.0 + i as f32 * 34.0;
        let alpha = (14 - i) as u8 * 2;
        painter.circle_filled(glow_center, radius, Color32::from_rgba_unmultiplied(r, g, b, alpha));
    }

    let block = Rect::from_center_size(
        full.center() - Vec2::new(0.0, 12.0),
        Vec2::new((full.width() - 40.0).clamp(120.0, 640.0), 170.0),
    );
    ui.scope_builder(
        UiBuilder::new().max_rect(block).layout(Layout::top_down(Align::Center)),
        |ui| {
            twemoji::text_in(ui, channel_name, theme::semibold(28.0), palette.text, twemoji::Set::Fluent);
            ui.add_space(4.0);
            theme::text(ui, "No hay nadie en el chat de voz", theme::regular(14.0), palette.secondary);
            ui.add_space(20.0);
            if can_join && theme::pill_button(ui, palette, "Unirse a canal de voz", true).clicked() {
                ev.join = true;
            }
        },
    );
}

/// Degradado vertical como malla (dos triángulos).
fn gradient_mesh(rect: Rect, top: Color32, bottom: Color32) -> egui::Shape {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    egui::Shape::mesh(mesh)
}

// ---------------------------------------------------------------------
// Layouts
// ---------------------------------------------------------------------

/// Grilla 16:9 que maximiza el tamaño de los tiles para `n` elementos.
/// La última fila (si queda incompleta) se centra.
fn grid_layout(area: Rect, n: usize) -> Vec<Rect> {
    if n == 0 || area.width() < 40.0 || area.height() < 40.0 {
        return Vec::new();
    }
    let mut best: Option<(f32, usize, f32, f32)> = None; // (área, cols, tw, th)
    for cols in 1..=n {
        let rows = n.div_ceil(cols);
        let w = (area.width() - TILE_GAP * (cols as f32 - 1.0)) / cols as f32;
        let h = (area.height() - TILE_GAP * (rows as f32 - 1.0)) / rows as f32;
        let tw = w.min(h * TILE_ASPECT);
        let th = tw / TILE_ASPECT;
        if tw <= 0.0 {
            continue;
        }
        if best.is_none_or(|(a, ..)| tw * th > a) {
            best = Some((tw * th, cols, tw, th));
        }
    }
    let Some((_, cols, tw, th)) = best else { return Vec::new() };
    // Con pocos elementos no hace falta llenar toda la pantalla.
    let (tw, th) = if tw > 720.0 { (720.0, 720.0 / TILE_ASPECT) } else { (tw, th) };
    let rows = n.div_ceil(cols);
    let total_h = th * rows as f32 + TILE_GAP * (rows as f32 - 1.0);
    let top = area.center().y - total_h / 2.0;

    let mut out = Vec::with_capacity(n);
    for row in 0..rows {
        let in_row = if row == rows - 1 { n - row * cols } else { cols };
        let row_w = tw * in_row as f32 + TILE_GAP * (in_row as f32 - 1.0);
        let left = area.center().x - row_w / 2.0;
        for col in 0..in_row {
            let min = Pos2::new(
                left + col as f32 * (tw + TILE_GAP),
                top + row as f32 * (th + TILE_GAP),
            );
            out.push(Rect::from_min_size(min, Vec2::new(tw, th)));
        }
    }
    out
}

/// Tira de tiles chicos pegada abajo del área, centrada.
fn strip_layout(area: Rect, n: usize) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let max_w = (area.width() - TILE_GAP * (n as f32 - 1.0)) / n as f32;
    let tw = max_w.min(STRIP_H * TILE_ASPECT).max(60.0);
    let th = tw / TILE_ASPECT;
    let total_w = tw * n as f32 + TILE_GAP * (n as f32 - 1.0);
    let left = area.center().x - total_w / 2.0;
    let top = area.bottom() - th;
    (0..n)
        .map(|i| {
            Rect::from_min_size(
                Pos2::new(left + i as f32 * (tw + TILE_GAP), top),
                Vec2::new(tw, th),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------
// Tiles de integrantes
// ---------------------------------------------------------------------

fn draw_tile(
    ui: &mut egui::Ui,
    palette: &Palette,
    rect: Rect,
    occ: &VoiceOccupant,
    speaking: bool,
    tc: &TileCtx,
    ev: &mut CallEvents,
) {
    let radius = CornerRadius::same(TILE_RADIUS);

    // El tile entero, debajo de todo lo demás (los botones se registran
    // después y ganan el click).
    // Un solo widget para el tile: clic izquierdo (cambia la mirada, si es
    // clickeable) y clic derecho (volumen/silencio de esa persona).
    let tile_resp = ui.interact(rect, egui::Id::new(("call_tile", &occ.user_id)), Sense::click());
    if tc.clickable {
        if tile_resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            ev.toggle_layout = true;
        }
    }
    let is_me_tile = tc.my_id == Some(occ.user_id.as_str());
    let local = tc.playback.get(&occ.user_id).copied().unwrap_or_default();
    crate::ui::audio_menu::show(&tile_resp, &occ.name, &occ.user_id, local, is_me_tile);

    let fill = extra::blend(palette.surface, occ.avatar_color, 0.22);
    ui.painter().rect_filled(rect, radius, fill);

    let avatar_r = (rect.height() * 0.22).clamp(18.0, 68.0);
    extra::avatar(
        ui,
        rect.center() - Vec2::new(0.0, 4.0),
        avatar_r,
        occ.avatar_url.as_deref(),
        occ.avatar_color,
        &occ.initial(),
        palette,
    );

    // La transmisión de esta persona es un tile aparte (`draw_stream_card`):
    // acá solo va la persona.
    let is_me = tc.my_id == Some(occ.user_id.as_str());
    let small = rect.width() < 190.0;

    // Chip con el nombre (+ ícono de mute/ensordecido) abajo a la izquierda.
    let font = theme::medium(if small { 11.5 } else { 13.0 });
    let has_badge = occ.self_deaf || occ.self_mute;
    let badge_w = if has_badge { 18.0 } else { 0.0 };
    let max_text_w = (rect.width() - 20.0 - 16.0 - badge_w).max(30.0);
    let (name, text_w) = fit_text(ui, &occ.name, &font, max_text_w);
    let chip_h = if small { 20.0 } else { 24.0 };
    let chip = Rect::from_min_size(
        Pos2::new(rect.left() + 10.0, rect.bottom() - 10.0 - chip_h),
        Vec2::new(text_w + 16.0 + badge_w, chip_h),
    );
    ui.painter().rect_filled(chip, chip_h / 2.0, Color32::from_black_alpha(150));
    let mut text_x = chip.left() + 8.0;
    if has_badge {
        let icon = if occ.self_deaf { Icon::VolumeX } else { Icon::MicOff };
        let icon_rect =
            Rect::from_center_size(Pos2::new(chip.left() + 8.0 + 7.0, chip.center().y), Vec2::splat(13.0));
        theme::paint_icon(ui, icon, icon_rect, 13.0, palette.danger);
        text_x += badge_w;
    }
    ui.painter().text(
        Pos2::new(text_x, chip.center().y),
        Align2::LEFT_CENTER,
        name,
        font,
        Color32::WHITE,
    );

    // Silenciado (o a 0 %) solo para vos: se nota sin abrir el menú.
    if !is_me && (local.muted || local.volume.value() == 0) {
        let badge = Rect::from_center_size(
            Pos2::new(rect.right() - 22.0, rect.top() + 22.0),
            Vec2::splat(26.0),
        );
        ui.painter().circle_filled(badge.center(), 13.0, Color32::from_black_alpha(170));
        theme::paint_icon(
            ui,
            Icon::VolumeX,
            Rect::from_center_size(badge.center(), Vec2::splat(14.0)),
            14.0,
            palette.danger,
        );
    }

    if speaking {
        ui.painter()
            .rect_stroke(rect, radius, Stroke::new(3.0, palette.accent), StrokeKind::Inside);
    }
}

/// Dibuja un casillero de la grilla o de la tira.
fn draw_item(
    ui: &mut egui::Ui,
    palette: &Palette,
    rect: Rect,
    item: Item,
    sc: &Scene,
    alpha: f32,
    ev: &mut CallEvents,
) {
    match item {
        Item::Member(i) => draw_tile(ui, palette, rect, &sc.members[i], sc.speaking[i], sc.tc, ev),
        Item::Stream(i) => draw_stream_card(ui, palette, rect, &sc.members[i], sc, ev),
        Item::Watched => {
            if let Some(stage) = sc.stage {
                draw_stream(ui, palette, rect, stage, StreamMode::GridTile, false, alpha, ev);
            }
        }
    }
}

/// Tile de la transmisión de `owner`, separado del de la persona.
///
/// * De otra persona (todavía sin verla): la miniatura de fondo, si Discord la
///   tiene, y el botón "Ver transmisión" en el centro.
/// * Propia: lo que estás transmitiendo (vista previa local). Con la ventana
///   en segundo plano no se procesa y se avisa.
fn draw_stream_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    rect: Rect,
    owner: &VoiceOccupant,
    sc: &Scene,
    ev: &mut CallEvents,
) {
    let radius = CornerRadius::same(TILE_RADIUS);
    let is_me = sc.tc.my_id == Some(owner.user_id.as_str());
    let small = rect.width() < 190.0;

    ui.painter().rect_filled(
        rect,
        radius,
        extra::blend(palette.window_solid, Color32::BLACK, 0.6),
    );

    if is_me {
        match sc.own {
            _ if sc.video_paused => paused_notice(ui, rect, radius, palette),
            Some(own) => match own.texture.as_ref() {
                Some(texture) if own.size[0] > 0 && own.size[1] > 0 => {
                    let (fw, fh) = (own.size[0] as f32, own.size[1] as f32);
                    let scale = (rect.width() / fw).min(rect.height() / fh);
                    let video = Rect::from_center_size(rect.center(), Vec2::new(fw * scale, fh * scale));
                    let fills = (video.width() - rect.width()).abs() < 1.0
                        && (video.height() - rect.height()).abs() < 1.0;
                    egui::Image::new(texture)
                        .corner_radius(if fills { radius } else { CornerRadius::ZERO })
                        .paint_at(ui, video);
                }
                _ => centered_text(ui, rect, "Iniciando la transmisión…", 13.0, palette.dim),
            },
            None => centered_text(ui, rect, "Iniciando la transmisión…", 13.0, palette.dim),
        }
    } else {
        // Fondo: la miniatura (si hay) o el color de la persona, oscurecido
        // para que el botón se lea.
        ui.painter()
            .rect_filled(rect, radius, extra::blend(palette.surface, owner.avatar_color, 0.30));
        if let Some(url) = sc.previews.get(&owner.user_id).and_then(|url| url.as_deref()) {
            paint_cover_image(ui, rect, radius, url);
        }
        ui.painter().rect_filled(rect, radius, Color32::from_black_alpha(120));

        if sc.connected_here {
            // Acción principal: no se desvanece con la inactividad.
            let size = if small { Vec2::new(122.0, 28.0) } else { Vec2::new(156.0, 36.0) };
            let btn = Rect::from_center_size(rect.center(), size);
            let resp = ui.interact(btn, egui::Id::new(("call_watch", &owner.user_id)), Sense::click());
            let bg = if resp.hovered() { palette.accent_hover } else { palette.accent };
            ui.painter().rect_filled(btn, size.y / 2.0, bg);
            ui.painter().text(
                btn.center(),
                Align2::CENTER_CENTER,
                "Ver transmisión",
                theme::semibold(if small { 11.5 } else { 13.0 }),
                palette.on_accent,
            );
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                ev.watch = Some((owner.user_id.clone(), owner.name.clone()));
            }
        } else {
            centered_text(
                ui,
                rect,
                "Únete a la llamada para ver la transmisión",
                if small { 11.5 } else { 13.0 },
                Color32::WHITE,
            );
        }
    }

    // Título + "EN DIRECTO" arriba a la izquierda.
    let font = theme::semibold(if small { 11.0 } else { 12.5 });
    let title = if is_me {
        "Tu transmisión".to_string()
    } else {
        format!("Stream de {}", owner.name)
    };
    let live = !is_me || sc.own.is_some_and(|own| own.status == StreamPublishStatus::Live);
    let pill_w = if small { 0.0 } else { 74.0 };
    let max_title = (rect.width() - 12.0 - 40.0 - 8.0 - pill_w - 12.0).max(40.0);
    let (title, w) = fit_text(ui, &title, &font, max_title);
    let chip_h = if small { 22.0 } else { 28.0 };
    let chip = Rect::from_min_size(rect.left_top() + Vec2::new(10.0, 10.0), Vec2::new(w + 40.0, chip_h));
    ui.painter().rect_filled(chip, chip_h / 2.0, Color32::from_black_alpha(150));
    theme::paint_icon(
        ui,
        Icon::Monitor,
        Rect::from_center_size(Pos2::new(chip.left() + 16.0, chip.center().y), Vec2::splat(14.0)),
        14.0,
        palette.accent,
    );
    ui.painter().text(
        Pos2::new(chip.left() + 30.0, chip.center().y),
        Align2::LEFT_CENTER,
        title,
        font,
        Color32::WHITE,
    );
    if !small {
        let pill = Rect::from_min_size(
            Pos2::new(chip.right() + 8.0, chip.center().y - 10.0),
            Vec2::new(pill_w, 20.0),
        );
        let (label, fill) = if live {
            ("EN DIRECTO", palette.danger)
        } else {
            ("CONECTANDO", Color32::from_black_alpha(170))
        };
        ui.painter().rect_filled(pill, 5.0, fill);
        ui.painter().text(
            pill.center(),
            Align2::CENTER_CENTER,
            label,
            theme::semibold(10.0),
            palette.on_accent,
        );
    }
}

/// Menú de clic derecho sobre el video del stream: volumen del AUDIO DEL STREAM
/// (0–200 %), silenciarlo y restablecer. Es aparte del volumen de esa persona en
/// la llamada, igual que en el cliente oficial; se guarda en la cuenta
/// (`audio_context_settings.stream`).
fn stream_audio_menu(resp: &egui::Response, stage: &StageData, ev: &mut CallEvents) {
    if stage.owner_id.is_empty() {
        return;
    }
    resp.clone().context_menu(|ui| {
        ui.set_min_width(220.0);
        ui.label(egui::RichText::new(format!("Stream de {}", stage.owner_name)).strong());
        ui.separator();

        ui.label("Volumen del stream");
        let mut percent = f32::from(stage.playback.volume.value());
        let slider = egui::Slider::new(
            &mut percent,
            0.0..=f32::from(crate::discord::VoiceParticipantVolumePercent::maximum()),
        )
        .integer()
        .suffix(" %");
        if ui.add(slider).changed() {
            ev.stream_volume = Some((stage.owner_id.clone(), percent.round() as u16));
        }

        let mut muted = stage.playback.muted;
        if ui.checkbox(&mut muted, "Silenciar stream").changed() {
            ev.stream_muted = Some((stage.owner_id.clone(), muted));
        }

        ui.separator();
        let modified = stage.playback != crate::discord::VoiceParticipantPlaybackSettings::default();
        if ui.add_enabled(modified, egui::Button::new("Restablecer")).clicked() {
            ev.stream_reset = Some(stage.owner_id.clone());
            ui.close();
        }
    });
}

/// Recorta `text` con "…" hasta que entre en `max_w`. Devuelve el texto y su ancho.
const PAUSED_TEXT: &str = "Para ahorrar recursos este video se pausó";

/// Texto centrado en `rect`, partido en renglones si no entra.
fn centered_text(ui: &egui::Ui, rect: Rect, text: &str, size: f32, color: Color32) {
    let wrap = (rect.width() - 32.0).max(40.0);
    let galley = ui
        .painter()
        .layout(text.to_string(), theme::medium(size), color, wrap);
    let pos = Pos2::new(
        rect.center().x - galley.size().x / 2.0,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(pos, galley, color);
}

/// Aviso de "video pausado" (ventana en segundo plano) encima de `rect`.
fn paused_notice(ui: &egui::Ui, rect: Rect, radius: CornerRadius, palette: &Palette) {
    ui.painter()
        .rect_filled(rect, radius, Color32::from_black_alpha(190));
    let size = if rect.width() < 260.0 { 11.5 } else { 13.5 };
    centered_text(ui, rect, PAUSED_TEXT, size, palette.text);
}

/// Pinta la imagen de `url` cubriendo `rect` (recortada, sin deformarla).
/// Mientras carga, o si falla, no pinta nada.
fn paint_cover_image(ui: &mut egui::Ui, rect: Rect, radius: CornerRadius, url: &str) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let image = egui::Image::new(crate::ui::anim::plain(url)).show_loading_spinner(false);
    if let Ok(egui::load::TexturePoll::Ready { texture }) = image.load_for_size(ui.ctx(), rect.size()) {
        let tex = texture.size;
        if tex.x <= 0.0 || tex.y <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        let tex_aspect = tex.x / tex.y;
        let rect_aspect = rect.width() / rect.height();
        let uv = if tex_aspect > rect_aspect {
            let w = rect_aspect / tex_aspect;
            Rect::from_min_max(Pos2::new((1.0 - w) / 2.0, 0.0), Pos2::new((1.0 + w) / 2.0, 1.0))
        } else {
            let h = tex_aspect / rect_aspect;
            Rect::from_min_max(Pos2::new(0.0, (1.0 - h) / 2.0), Pos2::new(1.0, (1.0 + h) / 2.0))
        };
        image.uv(uv).corner_radius(radius).paint_at(ui, rect);
    }
}

fn fit_text(ui: &egui::Ui, text: &str, font: &egui::FontId, max_w: f32) -> (String, f32) {
    let measure = |s: &str| {
        ui.painter()
            .layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE)
            .size()
            .x
    };
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
// Stream (escenario, tile de grilla y pantalla completa)
// ---------------------------------------------------------------------

/// Dibuja el stream en `rect`. Un click sobre el video alterna la mirada
/// (escenario <-> grilla); en pantalla completa, doble click sale.
#[allow(clippy::too_many_arguments)]
fn draw_stream(
    ui: &mut egui::Ui,
    palette: &Palette,
    rect: Rect,
    stage: &StageData,
    mode: StreamMode,
    strip_hidden: bool,
    alpha: f32,
    ev: &mut CallEvents,
) {
    let radius = if mode == StreamMode::Fullscreen {
        CornerRadius::ZERO
    } else {
        CornerRadius::same(TILE_RADIUS)
    };
    let tag = mode.tag();

    // Superficie clickeable debajo de los botones (que se registran después).
    let surface = ui.interact(rect, egui::Id::new(("call_stream_surface", tag)), Sense::click());
    stream_audio_menu(&surface, stage, ev);

    ui.painter()
        .rect_filled(rect, radius, extra::blend(palette.window_solid, Color32::BLACK, 0.6));

    match stage.texture.as_ref() {
        Some(texture) if stage.frame_size[0] > 0 && stage.frame_size[1] > 0 => {
            let (fw, fh) = (stage.frame_size[0] as f32, stage.frame_size[1] as f32);
            let scale = (rect.width() / fw).min(rect.height() / fh);
            let video = Rect::from_center_size(rect.center(), Vec2::new(fw * scale, fh * scale));
            let fills = (video.width() - rect.width()).abs() < 1.0
                && (video.height() - rect.height()).abs() < 1.0;
            egui::Image::new(texture)
                .corner_radius(if fills { radius } else { CornerRadius::ZERO })
                .paint_at(ui, video);
        }
        _ => {
            let (text, color) = match stage.status {
                StreamWatchStatus::Failed => (
                    stage
                        .message
                        .clone()
                        .unwrap_or_else(|| "No se pudo conectar al stream".to_string()),
                    palette.danger,
                ),
                StreamWatchStatus::Connected => ("Esperando el primer frame…".to_string(), palette.dim),
                StreamWatchStatus::Receiving => {
                    ("Recibiendo video, mostrando el primer frame…".to_string(), palette.dim)
                }
                _ => ("Conectando al stream…".to_string(), palette.dim),
            };
            ui.painter()
                .text(rect.center(), Align2::CENTER_CENTER, text, theme::regular(13.0), color);
        }
    }

    // Ventana en segundo plano: el video no se procesa, se avisa.
    if stage.paused {
        paused_notice(ui, rect, radius, palette);
    }

    // Título + "EN DIRECTO" arriba a la izquierda (siempre visibles).
    let button_slots = match mode {
        StreamMode::Stage => 3.0,
        StreamMode::GridTile => 2.0,
        StreamMode::Fullscreen => 1.0,
    };
    let font = theme::semibold(12.5);
    let title = format!("Stream de {}", stage.owner_name);
    let max_title = (rect.width() - 12.0 - 40.0 - 8.0 - 74.0 - button_slots * 40.0 - 12.0).max(40.0);
    let (title, w) = fit_text(ui, &title, &font, max_title);
    let chip = Rect::from_min_size(rect.left_top() + Vec2::new(12.0, 12.0), Vec2::new(w + 40.0, 28.0));
    ui.painter().rect_filled(chip, 14.0, Color32::from_black_alpha(150));
    theme::paint_icon(
        ui,
        Icon::Monitor,
        Rect::from_center_size(Pos2::new(chip.left() + 16.0, chip.center().y), Vec2::splat(14.0)),
        14.0,
        palette.accent,
    );
    ui.painter().text(
        Pos2::new(chip.left() + 30.0, chip.center().y),
        Align2::LEFT_CENTER,
        title,
        font,
        Color32::WHITE,
    );
    let live = Rect::from_min_size(
        Pos2::new(chip.right() + 8.0, chip.center().y - 10.0),
        Vec2::new(74.0, 20.0),
    );
    ui.painter().rect_filled(live, 5.0, palette.danger);
    ui.painter().text(
        live.center(),
        Align2::CENTER_CENTER,
        "EN DIRECTO",
        theme::semibold(10.0),
        palette.on_accent,
    );

    // Botones arriba a la derecha (se desvanecen con la inactividad).
    let slot = |n: f32| Rect::from_center_size(rect.right_top() + Vec2::new(-28.0 - n * 40.0, 28.0), Vec2::splat(32.0));
    let dark = Color32::from_black_alpha(150);
    let darker = Color32::from_black_alpha(210);
    match mode {
        StreamMode::Fullscreen => {
            if round_button(
                ui, ("stream_exit_fs", tag), slot(0.0), Icon::Shrink, 16.0, dark, darker, Color32::WHITE,
                "Salir de pantalla completa (Esc)", alpha,
            ) {
                ev.exit_fullscreen = true;
            }
        }
        StreamMode::Stage | StreamMode::GridTile => {
            if round_button(
                ui, ("stream_close", tag), slot(0.0), Icon::X, 16.0, dark, darker, Color32::WHITE,
                "Dejar de ver el stream", alpha,
            ) {
                ev.close_stream = true;
            }
            if round_button(
                ui, ("stream_fs", tag), slot(1.0), Icon::Maximize2, 16.0, dark, darker, Color32::WHITE,
                "Pantalla completa", alpha,
            ) {
                ev.enter_fullscreen = true;
            }
            if mode == StreamMode::Stage {
                let (fg, tip) = if strip_hidden {
                    (palette.dim, "Mostrar integrantes")
                } else {
                    (Color32::WHITE, "Ocultar integrantes")
                };
                if round_button(
                    ui, ("stream_members", tag), slot(2.0), Icon::Users, 16.0, dark, darker, fg, tip, alpha,
                ) {
                    ev.toggle_strip = true;
                }
            }
        }
    }

    match mode {
        StreamMode::Fullscreen => {
            if surface.double_clicked() {
                ev.exit_fullscreen = true;
            }
        }
        _ => {
            if surface.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                ev.toggle_layout = true;
            }
        }
    }
}

// ---------------------------------------------------------------------
// Barra de controles
// ---------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn controls_bar(
    ui: &mut egui::Ui,
    palette: &Palette,
    bar: Rect,
    self_mute: bool,
    self_deaf: bool,
    broadcasting: bool,
    share_enabled: bool,
    alpha: f32,
    salt: &str,
    ev: &mut CallEvents,
) {
    if alpha < 0.02 {
        return;
    }
    ui.painter().rect_filled(
        bar,
        bar.height() / 2.0,
        extra::blend(palette.panel, Color32::BLACK, 0.25).gamma_multiply(alpha),
    );
    ui.painter().rect_stroke(
        bar,
        bar.height() / 2.0,
        Stroke::new(1.0, palette.outline.gamma_multiply(alpha)),
        StrokeKind::Inside,
    );

    let mut x = bar.left() + BAR_PAD;
    let y = bar.center().y;
    let mut next = |w: f32| {
        let r = Rect::from_min_size(Pos2::new(x, y - BTN_H / 2.0), Vec2::new(w, BTN_H));
        x += w + BAR_GAP;
        r
    };

    let muted_fill = extra::blend(palette.surface, palette.danger, 0.35);
    let muted_fill_hover = extra::blend(palette.surface_hover, palette.danger, 0.45);

    let (mic_icon, mic_fill, mic_hover, mic_fg, mic_tip) = if self_mute {
        (Icon::MicOff, muted_fill, muted_fill_hover, palette.danger, "Dejar de silenciar")
    } else {
        (Icon::Mic, palette.surface, palette.surface_hover, palette.text, "Silenciar")
    };
    if round_button(ui, ("mic", salt), next(W_ROUND), mic_icon, 20.0, mic_fill, mic_hover, mic_fg, mic_tip, alpha) {
        ev.toggle_mute = true;
    }

    let (deaf_icon, deaf_fill, deaf_hover, deaf_fg, deaf_tip) = if self_deaf {
        (Icon::VolumeX, muted_fill, muted_fill_hover, palette.danger, "Dejar de ensordecer")
    } else {
        (Icon::Headphones, palette.surface, palette.surface_hover, palette.text, "Ensordecer")
    };
    if round_button(ui, ("deaf", salt), next(W_ROUND), deaf_icon, 20.0, deaf_fill, deaf_hover, deaf_fg, deaf_tip, alpha) {
        ev.toggle_deafen = true;
    }

    // Cámara: todavía sin función en el cliente, se dibuja apagada.
    round_button(
        ui,
        ("camera", salt),
        next(W_ROUND),
        Icon::Video,
        20.0,
        palette.surface,
        palette.surface,
        palette.dim,
        "Cámara (próximamente)",
        alpha,
    );

    // Transmitir: con una transmisión en curso se pinta de rojo y la corta.
    let (share_icon, share_fill, share_hover, share_fg, share_tip) = if broadcasting {
        (
            Icon::Monitor,
            palette.danger,
            extra::blend(palette.danger, Color32::WHITE, 0.15),
            Color32::WHITE,
            "Dejar de compartir pantalla",
        )
    } else if share_enabled {
        (Icon::SquareArrowUp, palette.surface, palette.surface_hover, palette.text, "Compartir pantalla")
    } else {
        (Icon::SquareArrowUp, palette.surface, palette.surface, palette.dim, "Compartir pantalla")
    };
    let share_clicked =
        round_button(ui, ("share", salt), next(W_ROUND), share_icon, 20.0, share_fill, share_hover, share_fg, share_tip, alpha);
    if share_clicked && share_enabled {
        ev.toggle_share = true;
    }

    if round_button(
        ui,
        ("chat", salt),
        next(W_ROUND),
        Icon::MessageCircle,
        20.0,
        palette.surface,
        palette.surface_hover,
        palette.text,
        "Chat de voz",
        alpha,
    ) {
        ev.toggle_chat = true;
    }

    if round_button(
        ui,
        ("settings", salt),
        next(W_ROUND),
        Icon::Settings,
        20.0,
        palette.surface,
        palette.surface_hover,
        palette.text,
        "Ajustes",
        alpha,
    ) {
        ev.open_settings = true;
    }

    if round_button(
        ui,
        ("leave", salt),
        next(W_LEAVE),
        Icon::PhoneOff,
        20.0,
        palette.danger,
        extra::blend(palette.danger, Color32::WHITE, 0.15),
        Color32::WHITE,
        "Salir de la llamada",
        alpha,
    ) {
        ev.leave = true;
    }
}

/// Botón redondeado/píldora con un ícono centrado. Con `alpha` bajo se
/// dibuja translúcido y, por debajo de un umbral, ni se dibuja ni recibe
/// clics (así un botón invisible no puede activarse por accidente).
/// Devuelve `true` si hubo clic.
#[allow(clippy::too_many_arguments)]
fn round_button(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    rect: Rect,
    icon: Icon,
    icon_size: f32,
    fill: Color32,
    fill_hover: Color32,
    fg: Color32,
    tooltip: &str,
    alpha: f32,
) -> bool {
    if alpha < 0.02 {
        return false;
    }
    let resp = ui.interact(rect, egui::Id::new(("call_btn", id_salt)), Sense::click());
    let bg = if resp.hovered() { fill_hover } else { fill };
    ui.painter().rect_filled(rect, rect.height() / 2.0, bg.gamma_multiply(alpha));
    let size = if resp.is_pointer_button_down_on() { icon_size * 0.92 } else { icon_size };
    theme::paint_icon(ui, icon, rect, size, fg.gamma_multiply(alpha));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
        .clicked()
}
