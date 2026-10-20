//! Captura de pantalla + codificación H.264 para transmitir (Go Live propio).
//!
//! Todo ocurre en el mismo proceso con `ffmpeg-the-third`, la misma crate que
//! ya usan el decodificador del visor y el reproductor embebido: no hace falta
//! un `ffmpeg` aparte. Un hilo dedicado (`stream-encoder`) hace este ciclo:
//!
//! ```text
//! dispositivo de captura (avdevice)  ->  decoder rawvideo  ->  sws (escala + barras)
//!   gdigrab / x11grab / avfoundation                               |
//!                                                                  v
//!             EncodedFrame  <-  NVENC / AMF / QSV / libopenh264 / libx264 (YUV420P)
//! ```
//!
//! Excepción: una ventana en Windows no pasa por avdevice sino por Windows
//! Graphics Capture (`window_capture.rs`), que entrega cuadros BGRA y se une
//! al mismo tramo `sws -> codificador`. Si WGC no está disponible (Windows
//! anterior a 1903) se usa `gdigrab title=...` como respaldo.
//!
//! Los paquetes del codificador ya son access units en Annex B (con SPS/PPS en
//! cada IDR y AUD), así que salen directo hacia `frames_tx` sin pasar por el
//! `AccessUnitSplitter` (que habría sumado un frame de latencia).
//!
//! Requisitos de las libs de ffmpeg con las que se compila/ejecuta: `avdevice`
//! con el dispositivo de captura de la plataforma y al menos un codificador
//! H.264 (ver `encoder_candidates`). El FFmpeg LGPL de Windows NO trae `libx264`
//! (es GPL): ahí se usa NVENC / AMF / QSV y, por software, `libopenh264`. Los
//! paquetes de Linux y macOS llevan un FFmpeg con `libx264`. En Windows la
//! persona puede instalar aparte el paquete "FFmpeg GPL" (`crate::gpl_pack`),
//! que sí lo trae; en cuanto está cargado, `libx264` pasa delante de OpenH264.
//!
//! Como el codificador vive en el proceso, se puede pedir una IDR en marcha
//! (`KeyframeRequest`): el próximo frame que entra al codificador se marca como
//! `picture::Type::I`. La piden el PLI/FIR de los espectadores
//! (`stream_publish::run_feedback`) y la propia cola de salida cuando descarta
//! frames. Además se mantiene un GOP corto (`KEYFRAME_INTERVAL_SECS`) como red
//! de seguridad.

use std::ffi::CString;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ffmpeg_the_third as ffmpeg;

use ffmpeg::format::{Pixel, context::Input as InputContext};
use ffmpeg::picture;
use ffmpeg::software::scaling::{context::Context as ScalerContext, flag::Flags};
use ffmpeg::{Dictionary, Error as FfmpegError, Packet, frame::Video};

use super::h264::{EncodedFrame, cleaned, describe_sps, encoded_frame_from_access_unit, split_annexb_nals, with_low_delay_vui};
use super::stream_watch::{StreamFrame, StreamFrameSlot};
#[cfg(target_os = "windows")]
use super::window_capture;
use crate::logging;

/// Cada cuántos segundos se fuerza una IDR como red de seguridad. Con ~100 KB
/// por IDR, 1 s hacía que la imagen "latiera"; pero con 2 s, un solo paquete
/// perdido dejaba al espectador congelado hasta ~2 s (el visor oficial casi no
/// manda PLI y los NACK no siempre llegan). Ahora que la IDR se reparte en
/// ~70 ms y pesa menos (`bufsize` de 4 frames), 1 s es un buen compromiso. Si la
/// imagen vuelve a "latir", probar 2.
const KEYFRAME_INTERVAL_SECS: u32 = 1;

/// Se sube cada vez que las devtools cambian algo que se lee al arrancar el codificador
/// (codificador, perfil, slices, SEI/VUI, intervalo de IDR). La transmisión en curso lo
/// vigila (`encoder_settings_generation`) y reinicia el codificador para aplicarlo.
static ENCODER_SETTINGS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Número de versión de los ajustes del codificador: cambia con cada cambio de las devtools.
pub(crate) fn encoder_settings_generation() -> u64 {
    ENCODER_SETTINGS_GENERATION.load(Ordering::Relaxed)
}

fn encoder_settings_changed() {
    ENCODER_SETTINGS_GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Opciones de intervalo entre IDR (ms) que ofrecen las devtools.
pub(crate) const KEYFRAME_INTERVAL_CHOICES_MS: [u32; 7] = [250, 500, 750, 1000, 1500, 2000, 4000];

/// Intervalo entre IDR elegido en las devtools (0 = sin elegir): gana sobre la variable
/// de entorno y se aplica la PRÓXIMA vez que arranca el codificador.
static KEYFRAME_INTERVAL_OVERRIDE_MS: AtomicU32 = AtomicU32::new(0);

pub(crate) fn keyframe_interval_override() -> Option<u32> {
    match KEYFRAME_INTERVAL_OVERRIDE_MS.load(Ordering::Relaxed) {
        0 => None,
        ms => Some(ms),
    }
}

/// `None` vuelve a `ECORD_KEYFRAME_INTERVAL_MS` (o al intervalo por defecto).
pub(crate) fn set_keyframe_interval_override(ms: Option<u32>) {
    KEYFRAME_INTERVAL_OVERRIDE_MS.store(ms.map_or(0, |ms| ms.clamp(250, 4000)), Ordering::Relaxed);
    encoder_settings_changed();
}

/// Intervalo entre IDR en milisegundos: devtools, luego `ECORD_KEYFRAME_INTERVAL_MS`
/// (250 a 4000), luego `KEYFRAME_INTERVAL_SECS`. Si el visor no logra recuperar un
/// paquete perdido, la imagen queda congelada hasta la próxima IDR, así que bajarlo acorta
/// los congelamientos, a costa de ~28 KB extra por IDR.
pub(crate) fn keyframe_interval_ms() -> u32 {
    keyframe_interval_override().unwrap_or_else(|| {
        std::env::var("ECORD_KEYFRAME_INTERVAL_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .map_or(KEYFRAME_INTERVAL_SECS * 1000, |ms| ms.clamp(250, 4000))
    })
}

/// Cuánto se espera al primer frame antes de dar el codificador por colgado.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(8);
/// Separación mínima entre dos IDR forzadas. Una IDR pesa varias veces lo que
/// un frame P: si cada PLI (o varios espectadores a la vez) forzara una, se
/// ahogaría el bitrate. Los pedidos que llegan antes se ignoran; quien los
/// necesite vuelve a pedir.
const MIN_FORCED_KEYFRAME_INTERVAL: Duration = Duration::from_millis(500);

/// Pedido de "mandá una IDR ya". Se clona y se comparte entre quien pide (red,
/// cola de salida) y el hilo del codificador, que lo atiende en el próximo
/// frame. Varios pedidos seguidos valen por uno.
#[derive(Clone, Debug, Default)]
pub(crate) struct KeyframeRequest(Arc<AtomicBool>);

impl KeyframeRequest {
    pub(crate) fn request(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    fn take(&self) -> bool {
        self.0.swap(false, Ordering::Relaxed)
    }
}

/// Tamaño máximo de la vista previa de la propia transmisión (la UI la dibuja
/// en un tile; no hace falta más).
const PREVIEW_MAX: (u32, u32) = (480, 270);
/// Separación mínima entre dos frames de la vista previa (~8 fps).
const PREVIEW_INTERVAL: Duration = Duration::from_millis(125);

struct PreviewScaler {
    source_format: Pixel,
    source_size: (u32, u32),
    context: ScalerContext,
    out: Video,
}

/// Saca de lo que se captura una imagen chica en RGBA para que la UI muestre
/// lo que se está transmitiendo. Corre en el hilo del codificador, a pocos
/// fps, y NO hace nada mientras `slot.is_paused()` (ventana en segundo plano):
/// la transmisión sigue, solo se deja de procesar la vista previa.
struct PreviewRenderer {
    slot: StreamFrameSlot,
    last: Option<Instant>,
    scaler: Option<PreviewScaler>,
    failed: bool,
}

impl PreviewRenderer {
    fn new(slot: StreamFrameSlot) -> Self {
        Self {
            slot,
            last: None,
            scaler: None,
            failed: false,
        }
    }

    fn render(&mut self, source: &Video) {
        if self.failed || self.slot.is_paused() {
            return;
        }
        if self.last.is_some_and(|at| at.elapsed() < PREVIEW_INTERVAL) {
            return;
        }
        self.last = Some(Instant::now());
        if let Err(error) = self.try_render(source) {
            // Si falla una vez (formato raro) no se insiste: la transmisión
            // no depende de la vista previa.
            logging::error("stream", format!("vista previa de la transmisión: {error}"));
            self.failed = true;
        }
    }

    fn try_render(&mut self, source: &Video) -> Result<(), String> {
        let size = (source.width(), source.height());
        if size.0 == 0 || size.1 == 0 {
            return Ok(());
        }
        let stale = self
            .scaler
            .as_ref()
            .is_none_or(|cached| cached.source_format != source.format() || cached.source_size != size);
        if stale {
            let fit = fit_size(size, PREVIEW_MAX);
            let context = ScalerContext::get(
                source.format(),
                size.0,
                size.1,
                Pixel::RGBA,
                fit.0,
                fit.1,
                Flags::FAST_BILINEAR,
            )
            .map_err(|error| ffmpeg_error("no se pudo crear el escalador de la vista previa", error))?;
            self.scaler = Some(PreviewScaler {
                source_format: source.format(),
                source_size: size,
                context,
                out: Video::new(Pixel::RGBA, fit.0, fit.1),
            });
        }
        let Some(scaler) = self.scaler.as_mut() else {
            return Ok(());
        };
        scaler
            .context
            .run(source, &mut scaler.out)
            .map_err(|error| ffmpeg_error("escalando la vista previa", error))?;
        let (width, height) = (scaler.out.width() as usize, scaler.out.height() as usize);
        let stride = scaler.out.stride(0);
        let data = scaler.out.data(0);
        let mut rgba = Vec::with_capacity(width * height * 4);
        for row in 0..height {
            let start = row * stride;
            rgba.extend_from_slice(&data[start..start + width * 4]);
        }
        // La captura de pantalla no trae alfa útil: se fuerza opaco.
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        self.slot.store(StreamFrame {
            width: width as u32,
            height: height as u32,
            rgba,
        });
        Ok(())
    }
}

/// Qué se captura. Lo elige la persona en `ui::chat::share_picker`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum CaptureTarget {
    /// Todo el escritorio principal.
    #[default]
    Desktop,
    /// Una ventana (solo Windows). Se captura por `hwnd` con Windows Graphics
    /// Capture, así que sigue sobre la misma ventana aunque cambie de título
    /// (una pestaña del navegador, la canción del reproductor) y no importa
    /// que haya otra con el mismo título. `title` se usa para el aviso en la
    /// UI y para el respaldo `gdigrab title=...`.
    Window { title: String, hwnd: isize },
    /// Una pantalla puntual (solo Windows): el rectángulo `(x, y, width,
    /// height)` del escritorio virtual, que `gdigrab` recorta con
    /// `offset_x`/`offset_y`/`video_size`. Sirve para elegir un monitor en
    /// instalaciones con varios (`Desktop` los toma todos juntos).
    Monitor {
        label: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
}

impl CaptureTarget {
    /// Texto corto para logs y avisos.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Desktop => "toda la pantalla".to_owned(),
            Self::Window { title, .. } => format!("ventana \"{title}\""),
            Self::Monitor { label, .. } => label.clone(),
        }
    }
}

/// Calidad y origen de la transmisión.
#[derive(Clone, Debug)]
pub(crate) struct ScreenCaptureConfig {
    pub(crate) target: CaptureTarget,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) fps: u32,
    pub(crate) bitrate_kbps: u32,
}

/// Resolución y fps que elige la persona en el selector (engranaje de
/// "Calidad"). La imagen siempre sale en 16:9; si la fuente tiene otra
/// proporción se le agregan barras negras (ver `FrameConverter`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StreamQuality {
    /// Alto en píxeles: 720, 1080 o 1440.
    pub(crate) height: u32,
    pub(crate) fps: u32,
}

impl StreamQuality {
    pub(crate) const HEIGHTS: [u32; 3] = [720, 1080, 1440];
    pub(crate) const FPS: [u32; 3] = [15, 30, 60];

    /// "1440p · 30 fps".
    pub(crate) fn label(self) -> String {
        format!("{}p · {} fps", self.height, self.fps)
    }

    /// Ancho 16:9 par (1280, 1920, 2560...).
    fn width(self) -> u32 {
        (self.height * 16 / 9) & !1
    }

    /// Bitrate a anunciar y pedirle al codificador: crece con la resolución y
    /// con los fps (a 30 fps: 2,5 / 4,5 / 8 Mbps).
    fn bitrate_kbps(self) -> u32 {
        let at_30 = match self.height {
            0..=720 => 2500,
            721..=1080 => 4500,
            _ => 8000,
        };
        match self.fps {
            0..=15 => at_30 * 7 / 10,
            16..=30 => at_30,
            _ => at_30 * 16 / 10,
        }
    }
}

impl Default for StreamQuality {
    /// 720p a 30 fps, el tope de Go Live sin Nitro.
    fn default() -> Self {
        Self { height: 720, fps: 30 }
    }
}

impl ScreenCaptureConfig {
    /// Configuración para `target` con la calidad elegida.
    pub(crate) fn new(target: CaptureTarget, quality: StreamQuality) -> Self {
        Self {
            target,
            width: quality.width(),
            height: quality.height,
            fps: quality.fps,
            bitrate_kbps: quality.bitrate_kbps(),
        }
    }
}

impl Default for ScreenCaptureConfig {
    /// 720p a 30 fps, el tope de Go Live sin Nitro.
    fn default() -> Self {
        Self {
            target: CaptureTarget::Desktop,
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_kbps: 2500,
        }
    }
}

impl ScreenCaptureConfig {
    /// Tamaño siempre par (lo exige yuv420p).
    fn even_size(&self) -> (u32, u32) {
        ((self.width & !1).max(2), (self.height & !1).max(2))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Encoder {
    X264,
    /// Software, de Cisco (BSD). Está en el FFmpeg LGPL de Windows, que no trae x264.
    OpenH264,
    Nvenc,
    Amf,
    Qsv,
}

impl Encoder {
    fn name(self) -> &'static str {
        match self {
            Self::X264 => "libx264",
            Self::OpenH264 => "libopenh264",
            Self::Nvenc => "h264_nvenc",
            Self::Amf => "h264_amf",
            Self::Qsv => "h264_qsv",
        }
    }
}

/// Valores que acepta `ECORD_STREAM_ENCODER` (y el selector de las devtools).
pub(crate) const ENCODER_CHOICES: [&str; 6] = ["auto", "x264", "openh264", "nvenc", "amf", "qsv"];
/// Perfiles H.264 que acepta `ECORD_NVENC_PROFILE` (y el selector de las devtools).
pub(crate) const NVENC_PROFILES: [&str; 3] = ["baseline", "main", "high"];

/// Lo que eligen las devtools en esta sesión: gana sobre las variables de
/// entorno y se aplica a la PRÓXIMA transmisión.
static ENCODER_OVERRIDE: Mutex<Option<&'static str>> = Mutex::new(None);
static NVENC_PROFILE_OVERRIDE: Mutex<Option<&'static str>> = Mutex::new(None);
/// Último codificador que arrancó (para mostrarlo en las devtools).
static LAST_ENCODER: Mutex<Option<&'static str>> = Mutex::new(None);
/// Quitar los SEI de lo que saca NVENC antes de mandarlo (ver `cleaned`).
/// Por omisión sí; `ECORD_KEEP_SEI=1` o las devtools lo apagan.
static STRIP_SEI: AtomicBool = AtomicBool::new(true);

/// ¿Se quitan los SEI de NVENC en la próxima transmisión?
pub(crate) fn strip_sei() -> bool {
    STRIP_SEI.load(Ordering::Relaxed) && std::env::var_os("ECORD_KEEP_SEI").is_none_or(|value| value.is_empty() || value == "0")
}

pub(crate) fn set_strip_sei(strip: bool) {
    STRIP_SEI.store(strip, Ordering::Relaxed);
    encoder_settings_changed();
}

/// Slices por frame que se le piden a NVENC (los mismos que x264 usa en la
/// práctica con `zerolatency`).
const NVENC_SLICES: &str = "6";

/// Partir cada frame de NVENC en varios slices, como x264. Por omisión sí;
/// `ECORD_NVENC_SINGLE_SLICE=1` o las devtools lo apagan.
static NVENC_MULTI_SLICE: AtomicBool = AtomicBool::new(true);

/// ¿Se le piden varios slices por frame a NVENC en la próxima transmisión?
pub(crate) fn nvenc_multi_slice() -> bool {
    NVENC_MULTI_SLICE.load(Ordering::Relaxed)
        && std::env::var_os("ECORD_NVENC_SINGLE_SLICE").is_none_or(|value| value.is_empty() || value == "0")
}

pub(crate) fn set_nvenc_multi_slice(enabled: bool) {
    NVENC_MULTI_SLICE.store(enabled, Ordering::Relaxed);
    encoder_settings_changed();
}

/// Pedirle a NVENC el mismo nivel y número de referencias que a x264. Por
/// omisión sí; `ECORD_NVENC_AUTO_LEVEL=1` o las devtools lo apagan.
static NVENC_MATCH_X264: AtomicBool = AtomicBool::new(true);

/// ¿Se le fuerza a NVENC el nivel y 1 referencia en la próxima transmisión?
pub(crate) fn nvenc_match_x264() -> bool {
    NVENC_MATCH_X264.load(Ordering::Relaxed)
        && std::env::var_os("ECORD_NVENC_AUTO_LEVEL").is_none_or(|value| value.is_empty() || value == "0")
}

pub(crate) fn set_nvenc_match_x264(enabled: bool) {
    NVENC_MATCH_X264.store(enabled, Ordering::Relaxed);
    encoder_settings_changed();
}

/// Reescribir el VUI del SPS de NVENC: sin timing ni HRD y con `bitstream_restriction`
/// (sin reordenamiento), como el de x264. Por omisión sí; `ECORD_KEEP_VUI=1` o las
/// devtools lo apagan y el SPS sale tal cual lo escribe NVENC.
static STRIP_VUI: AtomicBool = AtomicBool::new(true);

/// ¿Se reescribe el VUI del SPS de NVENC en la próxima transmisión?
pub(crate) fn strip_vui() -> bool {
    STRIP_VUI.load(Ordering::Relaxed) && std::env::var_os("ECORD_KEEP_VUI").is_none_or(|value| value.is_empty() || value == "0")
}

pub(crate) fn set_strip_vui(strip: bool) {
    STRIP_VUI.store(strip, Ordering::Relaxed);
    encoder_settings_changed();
}

fn read_slot(slot: &Mutex<Option<&'static str>>) -> Option<&'static str> {
    *slot.lock().unwrap_or_else(PoisonError::into_inner)
}

fn write_slot(slot: &Mutex<Option<&'static str>>, value: Option<&'static str>) {
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = value;
}

/// Codificador forzado desde las devtools, si lo hay.
pub(crate) fn encoder_override() -> Option<&'static str> {
    read_slot(&ENCODER_OVERRIDE)
}

/// `None` vuelve a lo que diga `ECORD_STREAM_ENCODER` (o al orden por omisión).
pub(crate) fn set_encoder_override(choice: Option<&'static str>) {
    write_slot(&ENCODER_OVERRIDE, choice);
    encoder_settings_changed();
}

/// Perfil de NVENC forzado desde las devtools, si lo hay.
pub(crate) fn nvenc_profile_override() -> Option<&'static str> {
    read_slot(&NVENC_PROFILE_OVERRIDE)
}

pub(crate) fn set_nvenc_profile_override(profile: Option<&'static str>) {
    write_slot(&NVENC_PROFILE_OVERRIDE, profile);
    encoder_settings_changed();
}

/// Nombre del último codificador que arrancó, si ya hubo una transmisión.
pub(crate) fn last_encoder() -> Option<&'static str> {
    read_slot(&LAST_ENCODER)
}

/// Qué codificador se pide: devtools, luego `ECORD_STREAM_ENCODER`; vacío si ninguno.
fn encoder_choice() -> String {
    encoder_override().map_or_else(
        || {
            std::env::var("ECORD_STREAM_ENCODER")
                .unwrap_or_default()
                .to_ascii_lowercase()
        },
        str::to_owned,
    )
}

/// Perfil con el que se abre NVENC: devtools, luego `ECORD_NVENC_PROFILE`, luego `baseline`.
pub(crate) fn nvenc_profile() -> String {
    nvenc_profile_override().map_or_else(
        || {
            std::env::var("ECORD_NVENC_PROFILE")
                .ok()
                .map(|value| value.to_ascii_lowercase())
                .filter(|value| NVENC_PROFILES.contains(&value.as_str()))
                .unwrap_or_else(|| "baseline".to_owned())
        },
        str::to_owned,
    )
}

/// Encoders a probar, en orden: el primero que arranque se usa.
///
/// * Windows: GPU primero (NVENC, AMF, QSV; si no hay esa GPU fallan al
///   abrirse, al instante) y después software: `libx264` si el FFmpeg cargado
///   lo trae (paquete GPL opcional o FFmpeg propio) y, si no, `libopenh264`
///   (el FFmpeg LGPL de serie no trae x264, así que ese intento falla al
///   instante y se pasa al siguiente).
/// * Linux / macOS (FFmpeg con `libx264`): `libx264` y, si no está, `libopenh264`.
///
/// `ECORD_STREAM_ENCODER` lo fuerza: `x264`, `openh264`, `nvenc`, `amf`, `qsv` o
/// `auto` (hardware primero y, si no arranca, software). Un encoder de
/// hardware elegido a mano cae a software si no arranca.
fn encoder_candidates() -> Vec<Encoder> {
    let choice = encoder_choice();
    let mut candidates = candidates_for(&choice, cfg!(target_os = "windows"));
    // Sin libx264 en las libs (FFmpeg LGPL de Windows) no se gasta un intento:
    // abrir la captura solo para descubrir que falta el codificador es tiempo
    // perdido antes de caer a OpenH264. `ECORD_STREAM_ENCODER=x264` lo deja
    // para que el error diga claramente qué falta.
    if choice != "x264" && !x264_available() {
        candidates.retain(|encoder| *encoder != Encoder::X264);
    }
    candidates
}

fn candidates_for(choice: &str, windows: bool) -> Vec<Encoder> {
    use Encoder::{Amf, Nvenc, OpenH264, Qsv, X264};
    match choice {
        "x264" => vec![X264],
        "openh264" => vec![OpenH264],
        "nvenc" => vec![Nvenc, X264, OpenH264],
        "amf" => vec![Amf, X264, OpenH264],
        "qsv" => vec![Qsv, X264, OpenH264],
        "auto" => vec![Nvenc, Amf, Qsv, X264, OpenH264],
        _ if windows => vec![Nvenc, Amf, Qsv, X264, OpenH264],
        _ => vec![X264, OpenH264],
    }
}

/// `true` si las libs de FFmpeg cargadas traen `libx264` (GPL). Decide si hay
/// que ofrecer el paquete opcional en Windows (`crate::gpl_pack`).
pub(crate) fn x264_available() -> bool {
    ffmpeg::encoder::find_by_name(Encoder::X264.name()).is_some()
}

/// De dónde sale la imagen: formato de entrada de avdevice, "URL" y opciones.
struct InputSpec {
    format: &'static str,
    url: String,
    options: Vec<(&'static str, String)>,
}

fn input_spec(config: &ScreenCaptureConfig) -> Result<InputSpec, String> {
    let fps = config.fps.max(1).to_string();
    #[cfg(target_os = "windows")]
    {
        let mut options = vec![("framerate", fps), ("draw_mouse", "1".to_owned())];
        let url = match &config.target {
            CaptureTarget::Desktop => "desktop".to_owned(),
            CaptureTarget::Window { title, .. } => format!("title={title}"),
            CaptureTarget::Monitor {
                x, y, width, height, ..
            } => {
                options.push(("offset_x", x.to_string()));
                options.push(("offset_y", y.to_string()));
                options.push(("video_size", format!("{width}x{height}")));
                "desktop".to_owned()
            }
        };
        Ok(InputSpec {
            format: "gdigrab",
            url,
            options,
        })
    }
    #[cfg(target_os = "linux")]
    {
        if !matches!(config.target, CaptureTarget::Desktop) {
            return Err("elegir una ventana o una pantalla puntual todavía solo funciona en Windows".to_owned());
        }
        let display = match std::env::var("DISPLAY") {
            Ok(display) if !display.is_empty() => display,
            _ if std::env::var_os("WAYLAND_DISPLAY").is_some() => {
                return Err("en Wayland puro no se puede capturar la pantalla con x11grab \
                     (hace falta XWayland o una captura por PipeWire)"
                    .to_owned());
            }
            _ => ":0.0".to_owned(),
        };
        Ok(InputSpec {
            format: "x11grab",
            url: display,
            options: vec![("framerate", fps), ("draw_mouse", "1".to_owned())],
        })
    }
    #[cfg(target_os = "macos")]
    {
        if !matches!(config.target, CaptureTarget::Desktop) {
            return Err("elegir una ventana o una pantalla puntual todavía solo funciona en Windows".to_owned());
        }
        Ok(InputSpec {
            format: "avfoundation",
            url: "Capture screen 0:none".to_owned(),
            options: vec![("framerate", fps), ("capture_cursor", "1".to_owned())],
        })
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        let _ = (config, fps);
        Err("no hay captura de pantalla para este sistema".to_owned())
    }
}

/// Nivel H.264 mínimo que admite `size` a `fps` (macrobloques de 16x16: tope
/// por frame y por segundo de cada nivel). 720p30 sigue siendo 4.1; 1440p
/// necesita 5.0 o 5.1 y un decodificador estricto rechazaría un SPS que se
/// quede corto.
fn h264_level(size: (u32, u32), fps: u32) -> &'static str {
    let frame_mbs = u64::from(size.0.div_ceil(16)) * u64::from(size.1.div_ceil(16));
    let rate = frame_mbs * u64::from(fps);
    // (nivel, máx. macrobloques por frame, máx. macrobloques por segundo)
    const LEVELS: [(&str, u64, u64); 4] = [
        ("4.1", 8192, 245_760),
        ("4.2", 8192, 522_240),
        ("5.0", 22_080, 589_824),
        ("5.1", 36_864, 983_040),
    ];
    LEVELS
        .iter()
        .find(|(_, max_frame, max_rate)| frame_mbs <= *max_frame && rate <= *max_rate)
        .map_or("5.2", |&(level, _, _)| level)
}

/// Opciones del codificador (las mismas que antes se pasaban por la línea de
/// comandos). Todos escriben H.264 Annex B con SPS/PPS repetidos en cada IDR,
/// sin B-frames (latencia mínima).
fn encoder_options(config: &ScreenCaptureConfig, encoder: Encoder) -> Vec<(&'static str, String)> {
    let fps = config.fps.max(1);
    let level = h264_level(config.even_size(), fps);
    let gop = (fps * keyframe_interval_ms() / 1000).max(1).to_string();
    let bitrate = config.bitrate_kbps.max(100);
    let mut options: Vec<(&'static str, String)> = vec![
        ("b", format!("{bitrate}k")),
        ("maxrate", format!("{bitrate}k")),
        // Buffer de rate control de ~4 frames. Con el de 0,5 s de antes una IDR
        // podía pesar ~105 KB (94 paquetes en ráfaga); el visor mostraba saltos
        // de ~40 paquetes perdidos de golpe y cada pérdida congelaba la imagen
        // hasta la próxima IDR. Con 4 frames la IDR sale en ~35 paquetes.
        ("bufsize", format!("{}k", (bitrate / fps).max(1) * 4)),
        ("g", gop.clone()),
        ("bf", "0".to_owned()),
    ];
    let mut add = |items: &[(&'static str, &str)]| {
        options.extend(items.iter().map(|(key, value)| (*key, (*value).to_owned())));
    };
    match encoder {
        Encoder::X264 => add(&[
            ("preset", "veryfast"), //veryfast
            ("tune", "zerolatency"),
            ("profile", "baseline"),
            ("level", level),
            ("keyint_min", gop.as_str()),
            ("sc_threshold", "0"),
            ("x264-params", "aud=1:repeat-headers=1"),
        ]),
        // libopenh264 escribe SPS/PPS en cada IDR por sí solo (no se pide
        // `global_header`). Opciones comprobadas contra el avcodec de BtbN
        // (FFmpeg 8.1): `profile`, `rc_mode` y `allow_skip_frames`; no hay
        // `slice_mode` ni presets. Sin saltar frames para mantener el ritmo.
        Encoder::OpenH264 => add(&[
            ("profile", "constrained_baseline"),
            ("rc_mode", "bitrate"),
            ("allow_skip_frames", "0"),
        ]),
        Encoder::Nvenc => {
            // `ECORD_NVENC_PROFILE` o las devtools (baseline, main o high) para probar
            // otro perfil sin recompilar: NVENC no es un codificador baseline "de verdad" y
            // algunos decodificadores de los espectadores lo rechazan.
            let profile = nvenc_profile();
            add(&[
                ("preset", "p1"),
                ("tune", "ll"),
                ("rc", "cbr"),
                ("zerolatency", "1"),
                ("profile", profile.as_str()),
                ("forced-idr", "1"),
                ("aud", "1"),
            ]);
            // x264 declara el nivel que calcula `h264_level` (mínimo 4.1) y usa una
            // sola referencia. NVENC, por su cuenta, declara 3.1 a 720p30 y abre 3
            // referencias: con x264 el espectador decodifica y con NVENC no, y esa
            // es la diferencia de SPS/PPS que queda una vez quitados SEI y VUI.
            if nvenc_match_x264() {
                add(&[("level", level), ("refs", "1")]);
            }
            // x264 con `zerolatency` parte cada frame en 6 slices (uno por hilo) y el
            // espectador lo decodifica; NVENC manda 1 solo slice de 40 a 110 KB y no.
            // Es la diferencia estructural que queda entre los dos flujos.
            if nvenc_multi_slice() {
                add(&[("slices", NVENC_SLICES)]);
            }
        }
        Encoder::Amf => add(&[
            ("usage", "ultralowlatency"),
            ("rc", "cbr"),
            ("profile", "constrained_baseline"),
            ("header_insertion_mode", "gop"),
            ("aud", "1"),
        ]),
        Encoder::Qsv => add(&[
            ("preset", "veryfast"),
            ("look_ahead", "0"),
            ("profile", "baseline"),
            ("forced_idr", "1"),
            ("aud", "1"),
        ]),
    }
    options
}

/// Tamaño al que se escala una fuente de `source` para entrar en `output`
/// manteniendo la proporción (el resto se rellena con barras negras). Siempre
/// par y nunca mayor que `output`.
fn fit_size(source: (u32, u32), output: (u32, u32)) -> (u32, u32) {
    let (source_w, source_h) = (u64::from(source.0), u64::from(source.1));
    let (output_w, output_h) = (u64::from(output.0), u64::from(output.1));
    if source_w == 0 || source_h == 0 {
        return output;
    }
    let (width, height) = if source_w * output_h >= source_h * output_w {
        // Más ancha (o igual) que la salida: manda el ancho.
        (output_w, output_w * source_h / source_w)
    } else {
        (output_h * source_w / source_h, output_h)
    };
    let even = |value: u64| (value as u32 & !1).clamp(2, u32::MAX);
    (even(width).min(output.0), even(height).min(output.1))
}

/// Proceso de captura + codificación en marcha. Al soltarlo se detiene el hilo.
pub(crate) struct ScreenEncoder {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    encoder: Encoder,
}

impl ScreenEncoder {
    pub(crate) fn encoder_name(&self) -> &'static str {
        self.encoder.name()
    }
}

impl Drop for ScreenEncoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // El hilo mira `stop` entre frame y frame: tarda, como mucho, un
            // intervalo de captura.
            let _ = thread.join();
        }
    }
}

/// Arranca la captura y entrega los frames codificados por `frames_tx`.
/// Bloquea hasta que sale el primer frame (o falla): llamarla desde
/// `spawn_blocking`. Si un codificador de hardware no arranca, prueba el
/// siguiente de la lista.
///
/// Si `frames_tx` se llena (la red no da abasto) se descartan frames y se
/// espera a la próxima IDR para volver a mandar: mandar un frame P sin su
/// referencia solo mostraría basura.
pub(crate) fn start_screen_encoder(
    config: &ScreenCaptureConfig,
    frames_tx: tokio::sync::mpsc::Sender<EncodedFrame>,
    keyframe: KeyframeRequest,
    preview: StreamFrameSlot,
) -> Result<ScreenEncoder, String> {
    let mut last_error = String::from("no hay codificadores disponibles");
    let mut candidates = encoder_candidates().into_iter().peekable();
    while let Some(encoder) = candidates.next() {
        match start_with_encoder(config, encoder, frames_tx.clone(), keyframe.clone(), preview.clone()) {
            Ok(started) => {
                write_slot(&LAST_ENCODER, Some(encoder.name()));
                logging::debug(
                    "stream",
                    format!("screen encoder started: {} ({})", encoder.name(), config.target.label()),
                );
                return Ok(started);
            }
            Err(error) => {
                // Con varias opciones, que falle una (p. ej. NVENC sin GPU NVIDIA) es lo
                // normal y se pasa a la siguiente; solo es un error si no queda ninguna.
                let message = format!("screen encoder {} failed: {error}", encoder.name());
                if candidates.peek().is_some() {
                    logging::debug("stream", message);
                } else {
                    logging::error("stream", message);
                }
                last_error = error;
            }
        }
    }
    Err(last_error)
}

fn start_with_encoder(
    config: &ScreenCaptureConfig,
    encoder: Encoder,
    frames_tx: tokio::sync::mpsc::Sender<EncodedFrame>,
    keyframe: KeyframeRequest,
    preview: StreamFrameSlot,
) -> Result<ScreenEncoder, String> {
    let stop = Arc::new(AtomicBool::new(false));
    // El hilo avisa una sola vez: `Ok` con el primer frame codificado o `Err`
    // con el motivo si algo falla antes. Si se suelta sin avisar es que terminó
    // (o lo pararon) antes del primer frame.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

    let thread = {
        let config = config.clone();
        let stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("stream-encoder".to_owned())
            .spawn(move || {
                super::stream_publish::win_perf::boost_current_thread();
                run_capture(&config, encoder, frames_tx, keyframe, preview, &stop, ready_tx)
            })
            .map_err(|error| format!("no se pudo iniciar el hilo del codificador: {error}"))?
    };

    match ready_rx.recv_timeout(FIRST_FRAME_TIMEOUT) {
        Ok(Ok(())) => Ok(ScreenEncoder {
            stop,
            thread: Some(thread),
            encoder,
        }),
        Ok(Err(error)) => {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
            Err(error)
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // No se hace `join`: el hilo podría estar colgado dentro del driver
            // de captura. Con `stop` sale solo en cuanto vuelva.
            stop.store(true, Ordering::Relaxed);
            Err("la captura no produjo video a tiempo".to_owned())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            let _ = thread.join();
            Err("la captura terminó antes de producir video".to_owned())
        }
    }
}

/// Cuerpo del hilo: corre la captura y reporta cómo terminó.
fn run_capture(
    config: &ScreenCaptureConfig,
    encoder: Encoder,
    frames_tx: tokio::sync::mpsc::Sender<EncodedFrame>,
    keyframe: KeyframeRequest,
    preview: StreamFrameSlot,
    stop: &AtomicBool,
    ready_tx: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let mut sink = FrameSink {
        frames_tx,
        keyframe: keyframe.clone(),
        ready_tx: Some(ready_tx),
        waiting_for_keyframe: false,
        dropped: 0,
        logged_packets: 0,
        strip_sei: encoder == Encoder::Nvenc && strip_sei(),
        strip_vui: encoder == Encoder::Nvenc && strip_vui(),
    };
    if let Err(error) = capture_loop(config, encoder, &keyframe, preview, stop, &mut sink) {
        match sink.ready_tx.take() {
            // Falló antes del primer frame: lo recibe `start_with_encoder`.
            Some(ready_tx) => {
                let _ = ready_tx.send(Err(error));
            }
            // Falló en plena transmisión: al soltarse `frames_tx` la sesión se
            // entera porque el canal se cierra.
            None => logging::error("stream", format!("screen capture stopped: {error}")),
        }
    }
}

/// Destino de los frames codificados: aplica la política de "si la cola se
/// llena, descartar y esperar a la próxima IDR".
struct FrameSink {
    frames_tx: tokio::sync::mpsc::Sender<EncodedFrame>,
    keyframe: KeyframeRequest,
    ready_tx: Option<std::sync::mpsc::Sender<Result<(), String>>>,
    /// Tras descartar un frame hay que esperar a una IDR para seguir.
    waiting_for_keyframe: bool,
    dropped: u64,
    /// Paquetes del codificador ya registrados en el log (solo los primeros).
    logged_packets: u32,
    /// Quitar los SEI de cada frame antes de encolarlo (NVENC).
    strip_sei: bool,
    /// Reescribir el VUI del SPS (con `bitstream_restriction`) antes de encolar la IDR (NVENC).
    strip_vui: bool,
}

impl FrameSink {
    /// `false` si el receptor ya no existe (la sesión terminó).
    fn push(&mut self, packet: &Packet) -> bool {
        // Diagnóstico: qué NAL trae cada uno de los primeros paquetes (7 = SPS,
        // 8 = PPS, 5 = IDR, 1 = P, 9 = AUD, 6 = SEI). Si un codificador (p. ej.
        // NVENC) nunca saca una IDR (5), el emisor descarta todo en silencio
        // esperándola y el espectador solo pide keyframes.
        if self.logged_packets < 5 {
            self.logged_packets += 1;
            let nals: Vec<u8> = packet
                .data()
                .map(|data| split_annexb_nals(data).iter().map(|nal| nal[0] & 0x1F).collect())
                .unwrap_or_default();
            logging::debug(
                "stream",
                format!(
                    "encoder packet #{}: bytes={} key={} nal_types={nals:?}",
                    self.logged_packets,
                    packet.size(),
                    packet.is_key(),
                ),
            );
            // El SPS y el PPS de la primera IDR: perfil, nivel y bytes crudos,
            // para ver qué le llega de verdad al espectador.
            if let Some(data) = packet.data() {
                for nal in split_annexb_nals(data) {
                    match nal[0] & 0x1F {
                        7 => {
                            logging::debug("stream", format!("sps: {}", describe_sps(nal)));
                            if self.strip_vui {
                                match with_low_delay_vui(nal) {
                                    Some(rewritten) => {
                                        logging::debug(
                                            "stream",
                                            format!("sps reescrito (VUI con bitstream_restriction): {}", describe_sps(&rewritten)),
                                        );
                                    }
                                    None => logging::debug("stream", "sps: no se pudo reescribir el VUI (se envía igual)".to_owned()),
                                }
                            }
                        }
                        8 => logging::debug(
                            "stream",
                            format!(
                                "pps: bytes={} hex={}",
                                nal.len(),
                                nal.iter().take(16).map(|byte| format!("{byte:02x}")).collect::<String>()
                            ),
                        ),
                        // Cabecera de slice (1 = P, 5 = IDR): nal_ref_idc, first_mb, slice_type,
                        // pps_id, frame_num, idr_pic_id... para comparar NVENC con x264.
                        1 | 5 => logging::debug(
                            "stream",
                            format!(
                                "slice: nal_type={} bytes={} head={}",
                                nal[0] & 0x1F,
                                nal.len(),
                                nal.iter().take(12).map(|byte| format!("{byte:02x}")).collect::<String>()
                            ),
                        ),
                        _ => {}
                    }
                }
            }
        }
        let Some(mut frame) = packet.data().and_then(encoded_frame_from_access_unit) else {
            return true;
        };
        if self.strip_sei || self.strip_vui {
            frame.data = cleaned(&frame.data, self.strip_sei, self.strip_vui);
        }
        frame.pts = packet.pts();
        if let Some(ready_tx) = self.ready_tx.take() {
            let _ = ready_tx.send(Ok(()));
        }
        if self.waiting_for_keyframe && !frame.keyframe {
            return true;
        }
        match self.frames_tx.try_send(frame) {
            Ok(()) => self.waiting_for_keyframe = false,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.waiting_for_keyframe = true;
                // No esperar al próximo GOP: pedir ya la IDR con la que retomar.
                self.keyframe.request();
                self.dropped += 1;
                if self.dropped == 1 || self.dropped % 100 == 0 {
                    logging::debug(
                        "stream",
                        format!("stream encoder queue full: {} frames dropped", self.dropped),
                    );
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return false,
        }
        true
    }
}

fn ffmpeg_error(context: &str, error: FfmpegError) -> String {
    format!("{context}: {error}")
}

/// Abre el dispositivo de captura. La crate no deja elegir el formato de
/// entrada (`x11grab`, `gdigrab`...) al abrir, así que se llama directo a
/// `avformat_open_input`.
fn open_capture(spec: &InputSpec) -> Result<InputContext, String> {
    use ffmpeg::ffi::{av_find_input_format, avformat_close_input, avformat_find_stream_info, avformat_open_input};

    let format_name = CString::new(spec.format).map_err(|error| error.to_string())?;
    let url = CString::new(spec.url.as_str()).map_err(|error| error.to_string())?;
    let mut options = Dictionary::new();
    for (key, value) in &spec.options {
        options.set(key, value);
    }

    // SAFETY: llamadas directas a libavformat con punteros válidos; el
    // contexto abierto se entrega a `InputContext`, que lo cierra al soltarse.
    unsafe {
        let input_format = av_find_input_format(format_name.as_ptr());
        if input_format.is_null() {
            return Err(format!(
                "estas libs de ffmpeg no incluyen el dispositivo de captura `{}` \
                 (hace falta compilar con avdevice)",
                spec.format
            ));
        }
        let mut context = ptr::null_mut();
        let result = avformat_open_input(&mut context, url.as_ptr(), input_format, options.as_mut_ptr());
        if result < 0 {
            return Err(ffmpeg_error(
                &format!("no se pudo abrir la captura ({})", spec.format),
                FfmpegError::from(result),
            ));
        }
        let result = avformat_find_stream_info(context, ptr::null_mut());
        if result < 0 {
            avformat_close_input(&mut context);
            return Err(ffmpeg_error(
                "no se pudo leer el formato de la captura",
                FfmpegError::from(result),
            ));
        }
        Ok(InputContext::wrap(context))
    }
}

fn open_encoder(config: &ScreenCaptureConfig, encoder: Encoder) -> Result<ffmpeg::encoder::Video, String> {
    let (width, height) = config.even_size();
    let fps = i32::try_from(config.fps.max(1)).unwrap_or(30);
    let codec = ffmpeg::encoder::find_by_name(encoder.name())
        .ok_or_else(|| format!("estas libs de ffmpeg no incluyen el codificador {}", encoder.name()))?;
    let mut video = ffmpeg::codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .map_err(|error| ffmpeg_error("codificador de video", error))?;
    video.set_width(width);
    video.set_height(height);
    video.set_format(Pixel::YUV420P);
    video.set_time_base((1, fps));
    video.set_frame_rate(Some((fps, 1)));

    let mut options = Dictionary::new();
    for (key, value) in encoder_options(config, encoder) {
        options.set(key, &value);
    }
    video
        .open_as_with(codec, options)
        .map_err(|error| ffmpeg_error(&format!("no se pudo abrir {}", encoder.name()), error))
}

/// Del frame capturado al paquete H.264: escala (con barras), lo numera, fuerza
/// la IDR cuando se pidió y saca lo que el codificador tenga listo.
struct Pipeline {
    encoder: ffmpeg::encoder::Video,
    converter: FrameConverter,
    keyframe: KeyframeRequest,
    last_forced: Option<Instant>,
    preview: PreviewRenderer,
    stats: EncodeStats,
    fps: u32,
}

/// Cada 5 s deja en el log cuántos frames salen de verdad y cuánto tarda cada
/// uno (escalar + codificar): si el promedio supera 1000/fps ms el emisor no
/// llega a los fps anunciados y el visor muestra el error `stream-view-low-fps`.
struct EncodeStats {
    since: Instant,
    frames: u32,
    total: Duration,
    max: Duration,
}

impl EncodeStats {
    fn new() -> Self {
        Self {
            since: Instant::now(),
            frames: 0,
            total: Duration::ZERO,
            max: Duration::ZERO,
        }
    }

    fn record(&mut self, took: Duration, source: (u32, u32), output: (u32, u32), fps: u32) {
        self.frames += 1;
        self.total += took;
        self.max = self.max.max(took);
        let elapsed = self.since.elapsed();
        if elapsed < Duration::from_secs(5) {
            return;
        }
        let seconds = elapsed.as_secs_f64();
        logging::debug(
            "stream",
            format!(
                "encode stats: {:.1} fps (pedido {fps}), por frame avg={:.1} ms max={} ms, captura {}x{} -> {}x{}",
                f64::from(self.frames) / seconds,
                self.total.as_secs_f64() * 1000.0 / f64::from(self.frames.max(1)),
                self.max.as_millis(),
                source.0,
                source.1,
                output.0,
                output.1,
            ),
        );
        *self = Self::new();
    }
}

impl Pipeline {
    fn new(
        config: &ScreenCaptureConfig,
        encoder_kind: Encoder,
        keyframe: KeyframeRequest,
        preview: StreamFrameSlot,
    ) -> Result<Self, String> {
        Ok(Self {
            encoder: open_encoder(config, encoder_kind)?,
            converter: FrameConverter::new(config.even_size()),
            keyframe,
            last_forced: None,
            preview: PreviewRenderer::new(preview),
            stats: EncodeStats::new(),
            fps: config.fps.max(1),
        })
    }

    /// Codifica `source` como el frame número `pts` (en unidades de 1/fps).
    /// `false` si hay que parar (la sesión terminó).
    fn encode(&mut self, source: &Video, pts: i64, sink: &mut FrameSink) -> Result<bool, String> {
        let started = Instant::now();
        let result = self.encode_inner(source, pts, sink);
        let output = self.converter.output;
        self.stats.record(
            started.elapsed(),
            (source.width(), source.height()),
            output,
            self.fps,
        );
        result
    }

    fn encode_inner(&mut self, source: &Video, pts: i64, sink: &mut FrameSink) -> Result<bool, String> {
        // Vista previa para la UI (a pocos fps y solo con la ventana al frente).
        self.preview.render(source);
        let frame = self.converter.convert(source)?;
        frame.set_pts(Some(pts));
        // El lienzo se reutiliza: el tipo de imagen se fija en cada frame
        // para que una IDR forzada no se arrastre a los siguientes.
        let force = self.keyframe.take()
            && self
                .last_forced
                .is_none_or(|at| at.elapsed() >= MIN_FORCED_KEYFRAME_INTERVAL);
        if force {
            self.last_forced = Some(Instant::now());
        }
        frame.set_kind(if force { picture::Type::I } else { picture::Type::None });
        self.encoder
            .send_frame(frame)
            .map_err(|error| ffmpeg_error("codificando", error))?;
        drain_packets(&mut self.encoder, sink)
    }
}

fn capture_loop(
    config: &ScreenCaptureConfig,
    encoder_kind: Encoder,
    keyframe: &KeyframeRequest,
    preview: StreamFrameSlot,
    stop: &AtomicBool,
    sink: &mut FrameSink,
) -> Result<(), String> {
    ffmpeg::init().map_err(|error| ffmpeg_error("ffmpeg init", error))?;

    // Una ventana en Windows: Windows Graphics Capture. Si no se puede (Windows
    // viejo, ventana protegida) se sigue con `gdigrab title=...`.
    #[cfg(target_os = "windows")]
    if let CaptureTarget::Window { hwnd, .. } = &config.target {
        match window_capture::WindowCapture::new(*hwnd) {
            Ok(capture) => {
                return window_capture_loop(config, encoder_kind, keyframe, preview, stop, sink, capture);
            }
            Err(error) => logging::error(
                "stream",
                format!("Windows Graphics Capture no disponible, uso gdigrab: {error}"),
            ),
        }
    }

    let mut input = open_capture(&input_spec(config)?)?;
    let (video_index, mut decoder) = {
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| "la captura no entrega video".to_owned())?;
        let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .and_then(|context| context.decoder().video())
            .map_err(|error| ffmpeg_error("decoder de la captura", error))?;
        (stream.index(), decoder)
    };

    let mut pipeline = Pipeline::new(config, encoder_kind, keyframe.clone(), preview)?;
    let fps = config.fps.max(1);

    let started = Instant::now();
    // Número de frame (en unidades de 1/fps) del último que se codificó.
    let mut last_pts: i64 = -1;
    let mut decoded = Video::empty();

    while !stop.load(Ordering::Relaxed) {
        let mut packet = Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => {}
            Err(FfmpegError::Eof) => return Err("la captura se cerró".to_owned()),
            Err(FfmpegError::Other { errno }) if errno == libc::EAGAIN => {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(error) => return Err(ffmpeg_error("leyendo la captura", error)),
        }
        if packet.stream() != video_index {
            continue;
        }
        decoder
            .send_packet(&packet)
            .map_err(|error| ffmpeg_error("decodificando la captura", error))?;

        while decoder.receive_frame(&mut decoded).is_ok() {
            // El número de frame sale del reloj, no de los timestamps del
            // dispositivo: así el ritmo es el pedido aunque el dispositivo vaya
            // más rápido (se descarta) o se atrase (el número salta).
            let slot = started.elapsed().as_secs_f64() * f64::from(fps);
            if slot < last_pts as f64 + 0.5 {
                continue;
            }
            let pts = (slot.round() as i64).max(last_pts + 1);
            last_pts = pts;
            if !pipeline.encode(&decoded, pts, sink)? {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Bucle de una ventana capturada con WGC. WGC solo entrega un cuadro cuando
/// el contenido cambia, así que acá el ritmo lo marca el reloj: cada `1/fps`
/// se codifica el último cuadro recibido (repetido si la ventana está quieta).
#[cfg(target_os = "windows")]
fn window_capture_loop(
    config: &ScreenCaptureConfig,
    encoder_kind: Encoder,
    keyframe: &KeyframeRequest,
    preview: StreamFrameSlot,
    stop: &AtomicBool,
    sink: &mut FrameSink,
    mut capture: window_capture::WindowCapture,
) -> Result<(), String> {
    let mut pipeline = Pipeline::new(config, encoder_kind, keyframe.clone(), preview)?;
    let fps = config.fps.max(1);
    let tick = Duration::from_secs_f64(1.0 / f64::from(fps));
    let started = Instant::now();
    let mut next_tick = started;
    let mut last_pts: i64 = -1;

    let mut last_fresh = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        // Si WGC deja de entregar cuadros durante más de 1 s, el visor ve la
        // imagen congelada aunque el codificador siga a 30 fps (reenvía el
        // último cuadro). Esto lo deja en el log para distinguirlo de la red.
        if capture.poll()? {
            let gap = last_fresh.elapsed();
            if gap >= Duration::from_secs(1) {
                logging::debug(
                    "stream",
                    format!(
                        "captura: la ventana no entregó cuadros nuevos durante {} ms \
                         (quieta, minimizada o sin actualizar)",
                        gap.as_millis()
                    ),
                );
            }
            last_fresh = Instant::now();
        }
        // Hasta que no llegue el primer cuadro no hay nada que codificar (si
        // tarda demasiado, `start_with_encoder` da el intento por fallido).
        if let Some(frame) = capture.latest() {
            let slot = started.elapsed().as_secs_f64() * f64::from(fps);
            let pts = (slot.round() as i64).max(last_pts + 1);
            last_pts = pts;
            if !pipeline.encode(frame, pts, sink)? {
                return Ok(());
            }
        }
        next_tick += tick;
        let now = Instant::now();
        if next_tick > now {
            std::thread::sleep(next_tick - now);
        } else {
            // Nos atrasamos (codificar tardó de más): no acumular deuda.
            next_tick = now;
        }
    }
    Ok(())
}

/// Saca del codificador todos los paquetes listos. `false` si hay que parar.
fn drain_packets(encoder: &mut ffmpeg::encoder::Video, sink: &mut FrameSink) -> Result<bool, String> {
    loop {
        let mut packet = Packet::empty();
        match encoder.receive_packet(&mut packet) {
            Ok(()) => {
                if !sink.push(&packet) {
                    return Ok(false);
                }
            }
            Err(FfmpegError::Eof) => return Ok(true),
            Err(FfmpegError::Other { errno }) if errno == libc::EAGAIN => return Ok(true),
            Err(error) => return Err(ffmpeg_error("leyendo del codificador", error)),
        }
    }
}

struct CachedScaler {
    source_format: Pixel,
    source_size: (u32, u32),
    fit: (u32, u32),
    context: ScalerContext,
}

/// Convierte lo capturado a YUV420P del tamaño fijo que se anuncia a Discord:
/// escala manteniendo la proporción y centra sobre un lienzo negro.
struct FrameConverter {
    output: (u32, u32),
    /// Lo que se le entrega al codificador. Las barras negras se pintan una
    /// sola vez y no se tocan más.
    canvas: Video,
    /// Imagen ya escalada, cuando no ocupa todo el lienzo.
    scaled: Option<Video>,
    scaler: Option<CachedScaler>,
}

impl FrameConverter {
    fn new(output: (u32, u32)) -> Self {
        let mut canvas = Video::new(Pixel::YUV420P, output.0, output.1);
        // Negro en YUV de rango limitado: Y=16, U=V=128.
        canvas.data_mut(0).fill(16);
        canvas.data_mut(1).fill(128);
        canvas.data_mut(2).fill(128);
        Self {
            output,
            canvas,
            scaled: None,
            scaler: None,
        }
    }

    fn convert(&mut self, source: &Video) -> Result<&mut Video, String> {
        let source_size = (source.width(), source.height());
        if source_size.0 == 0 || source_size.1 == 0 {
            return Err("la captura entregó un frame sin tamaño".to_owned());
        }
        let stale = self
            .scaler
            .as_ref()
            .is_none_or(|cached| cached.source_format != source.format() || cached.source_size != source_size);
        if stale {
            // Primer frame, o cambió la resolución / el formato (por ejemplo,
            // al cambiar la resolución del monitor).
            let fit = fit_size(source_size, self.output);
            let context = ScalerContext::get(
                source.format(),
                source_size.0,
                source_size.1,
                Pixel::YUV420P,
                fit.0,
                fit.1,
                // FAST_BILINEAR al reducir (1440p/4K -> 720p) toma muy pocas
                // muestras por píxel de salida: el texto sale dentado y
                // tembloroso con cualquier codificador. AREA promedia todos los
                // píxeles de origen (filtro de caja), nítido y barato.
                Flags::AREA,
            )
            .map_err(|error| ffmpeg_error("no se pudo crear el escalador", error))?;
            self.scaled = (fit != self.output).then(|| Video::new(Pixel::YUV420P, fit.0, fit.1));
            self.scaler = Some(CachedScaler {
                source_format: source.format(),
                source_size,
                fit,
                context,
            });
        }
        let Some(scaler) = self.scaler.as_mut() else {
            return Err("escalador de video ausente".to_owned());
        };

        // Si el codificador todavía retiene el lienzo anterior (los de
        // hardware encolan frames), esto lo copia en lugar de pisarlo; el
        // contenido, barras negras incluidas, se conserva.
        // SAFETY: `canvas` es un AVFrame propio y válido.
        let status = unsafe { ffmpeg::ffi::av_frame_make_writable(self.canvas.as_mut_ptr()) };
        if status < 0 {
            return Err(ffmpeg_error("no se pudo preparar el frame", FfmpegError::from(status)));
        }

        match self.scaled.as_mut() {
            None => scaler
                .context
                .run(source, &mut self.canvas)
                .map_err(|error| ffmpeg_error("escalando", error))?,
            Some(scaled) => {
                scaler
                    .context
                    .run(source, scaled)
                    .map_err(|error| ffmpeg_error("escalando", error))?;
                // Offsets pares: el croma de YUV420P va a media resolución.
                let x = ((self.output.0 - scaler.fit.0) / 2) & !1;
                let y = ((self.output.1 - scaler.fit.1) / 2) & !1;
                blit_yuv420p(&mut self.canvas, scaled, x, y);
            }
        }
        Ok(&mut self.canvas)
    }
}

/// Copia `source` dentro de `target` (ambos YUV420P) con su esquina superior
/// izquierda en `(x, y)`. `x` e `y` tienen que ser pares y `source` entrar.
fn blit_yuv420p(target: &mut Video, source: &Video, x: u32, y: u32) {
    for plane in 0..3 {
        let shift = u32::from(plane != 0);
        let rows = source.plane_height(plane) as usize;
        let width = source.plane_width(plane) as usize;
        let source_stride = source.stride(plane);
        let target_stride = target.stride(plane);
        let (offset_x, offset_y) = ((x >> shift) as usize, (y >> shift) as usize);
        let source_data = source.data(plane);
        let target_data = target.data_mut(plane);
        for row in 0..rows {
            let from = row * source_stride;
            let to = (offset_y + row) * target_stride + offset_x;
            target_data[to..to + width].copy_from_slice(&source_data[from..from + width]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(options: &[(&'static str, String)]) -> String {
        options
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn window_target_falls_back_to_gdigrab_by_title() {
        let config = ScreenCaptureConfig {
            target: CaptureTarget::Window {
                title: "Mi app - Documento".to_owned(),
                hwnd: 0,
            },
            ..Default::default()
        };
        let spec = input_spec(&config).unwrap();
        assert_eq!(spec.format, "gdigrab");
        assert_eq!(spec.url, "title=Mi app - Documento");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn monitor_target_crops_the_desktop_with_offsets() {
        let config = ScreenCaptureConfig {
            target: CaptureTarget::Monitor {
                label: "Pantalla 2".to_owned(),
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
            },
            ..Default::default()
        };
        let spec = input_spec(&config).unwrap();
        assert_eq!(spec.format, "gdigrab");
        assert_eq!(spec.url, "desktop");
        assert!(spec.options.contains(&("offset_x", "-1920".to_owned())));
        assert!(spec.options.contains(&("offset_y", "0".to_owned())));
        assert!(spec.options.contains(&("video_size", "1920x1080".to_owned())));
    }

    #[test]
    fn default_quality_matches_the_default_config() {
        let config = ScreenCaptureConfig::new(CaptureTarget::Desktop, StreamQuality::default());
        let default = ScreenCaptureConfig::default();
        assert_eq!(
            (config.width, config.height, config.fps, config.bitrate_kbps),
            (default.width, default.height, default.fps, default.bitrate_kbps)
        );
    }

    #[test]
    fn quality_scales_resolution_and_bitrate() {
        let q = |height, fps| ScreenCaptureConfig::new(CaptureTarget::Desktop, StreamQuality { height, fps });
        let high = q(1440, 60);
        assert_eq!((high.width, high.height, high.fps), (2560, 1440, 60));
        assert_eq!(high.bitrate_kbps, 12_800);
        assert_eq!(q(1080, 30).width, 1920);
        assert!(q(720, 15).bitrate_kbps < q(720, 30).bitrate_kbps);
        assert!(q(1080, 30).bitrate_kbps < q(1440, 30).bitrate_kbps);
        assert_eq!(StreamQuality { height: 1440, fps: 30 }.label(), "1440p · 30 fps");
    }

    #[test]
    fn h264_level_grows_with_resolution_and_fps() {
        assert_eq!(h264_level((1280, 720), 30), "4.1");
        assert_eq!(h264_level((1920, 1080), 30), "4.1");
        assert_eq!(h264_level((1920, 1080), 60), "4.2");
        assert_eq!(h264_level((2560, 1440), 30), "5.0");
        assert_eq!(h264_level((2560, 1440), 60), "5.1");
    }

    #[test]
    fn target_label_names_the_window() {
        assert_eq!(CaptureTarget::Desktop.label(), "toda la pantalla");
        assert_eq!(
            CaptureTarget::Window {
                title: "Notas".to_owned(),
                hwnd: 0
            }
            .label(),
            "ventana \"Notas\""
        );
    }

    #[test]
    fn size_is_forced_even() {
        let config = ScreenCaptureConfig {
            width: 1281,
            height: 721,
            ..Default::default()
        };
        assert_eq!(config.even_size(), (1280, 720));
    }

    #[test]
    fn x264_options_have_short_gop_and_repeated_headers() {
        let options = encoder_options(&ScreenCaptureConfig::default(), Encoder::X264);
        let text = joined(&options);
        assert!(text.contains("g=30"), "{text}");
        assert!(text.contains("keyint_min=30"), "{text}");
        assert!(text.contains("bf=0"), "{text}");
        assert!(text.contains("b=2500k maxrate=2500k bufsize=332k"), "{text}");
        assert!(text.contains("x264-params=aud=1:repeat-headers=1"), "{text}");
    }

    #[test]
    fn nvenc_declares_the_same_level_and_references_as_x264() {
        let config = ScreenCaptureConfig::default();
        let text = joined(&encoder_options(&config, Encoder::Nvenc));
        let x264 = joined(&encoder_options(&config, Encoder::X264));
        let level = h264_level(config.even_size(), config.fps.max(1));
        assert!(text.contains(&format!("level={level}")), "{text}");
        assert!(x264.contains(&format!("level={level}")), "{x264}");
        assert!(text.contains("refs=1"), "{text}");
        assert!(text.contains("slices=6"), "{text}");
        assert!(text.contains("forced-idr=1") && text.contains("aud=1"), "{text}");
    }

    #[test]
    fn openh264_options_have_short_gop_and_no_b_frames() {
        let options = encoder_options(&ScreenCaptureConfig::default(), Encoder::OpenH264);
        let text = joined(&options);
        assert!(text.contains("g=30"), "{text}");
        assert!(text.contains("bf=0"), "{text}");
        assert!(text.contains("b=2500k"), "{text}");
        assert!(text.contains("rc_mode=bitrate"), "{text}");
        assert!(!text.contains("x264-params") && !text.contains("slice_mode"), "{text}");
    }

    #[test]
    fn windows_default_is_gpu_then_x264_then_openh264() {
        use Encoder::{Amf, Nvenc, OpenH264, Qsv, X264};
        let order = candidates_for("", true);
        assert_eq!(order, vec![Nvenc, Amf, Qsv, X264, OpenH264]);
        // Un valor desconocido se comporta como el valor por defecto.
        assert_eq!(candidates_for("nada", true), order);
        // libx264 (paquete GPL opcional) va antes que libopenh264; sin él
        // `encoder_candidates` lo descarta y queda OpenH264.
        let openh264 = order.iter().position(|e| *e == OpenH264).unwrap();
        let x264 = order.iter().position(|e| *e == X264).unwrap();
        assert!(x264 < openh264);
    }

    #[test]
    fn linux_and_macos_default_keeps_x264_first() {
        use Encoder::{OpenH264, X264};
        assert_eq!(candidates_for("", false), vec![X264, OpenH264]);
    }

    #[test]
    fn forced_encoders_fall_back_to_software() {
        use Encoder::{Nvenc, OpenH264, X264};
        assert_eq!(candidates_for("x264", true), vec![X264]);
        assert_eq!(candidates_for("openh264", false), vec![OpenH264]);
        assert_eq!(candidates_for("nvenc", true), vec![Nvenc, X264, OpenH264]);
        assert!(candidates_for("auto", false).ends_with(&[X264, OpenH264]));
    }

    #[test]
    fn fit_keeps_aspect_ratio_and_stays_even() {
        // 16:9 -> sin barras.
        assert_eq!(fit_size((1920, 1080), (1280, 720)), (1280, 720));
        // Más cuadrada que la salida -> barras a los costados.
        assert_eq!(fit_size((2560, 1600), (1280, 720)), (1152, 720));
        // Ultrawide -> barras arriba y abajo.
        assert_eq!(fit_size((3440, 1440), (1280, 720)), (1280, 534));
        // Fuente más chica que la salida: se agranda hasta entrar.
        assert_eq!(fit_size((640, 480), (1280, 720)), (960, 720));
        for source in [(1366, 768), (1, 1), (5000, 7), (7, 5000)] {
            let (width, height) = fit_size(source, (1280, 720));
            assert!(width % 2 == 0 && height % 2 == 0, "{source:?}");
            assert!((2..=1280).contains(&width) && (2..=720).contains(&height));
        }
    }
}