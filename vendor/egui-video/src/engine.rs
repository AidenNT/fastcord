//! Motor de reproducción nuevo, pensado para que el video NO tironee.
//!
//! ## Por qué el `Player` viejo tironeaba
//!
//! * El reloj de todo era el `dts` del último paquete que leía el demuxer, no
//!   un reloj real. En un mp4 los paquetes de audio y video vienen
//!   intercalados en bloques de ~0.5 s, así que ese "reloj" avanzaba a
//!   saltos de ~0.5 s y disparaba el bucle de "recuperar desincronización"
//!   (saltar paquetes de golpe).
//! * Los cuadros se pedían con la crate `timer` a `1/fps`. En Windows esa
//!   espera tiene granularidad de ~15.6 ms, así que un video de 30 fps se
//!   entregaba a 31/47 ms alternados.
//! * Cada cuadro se leía, decodificaba, convertía y subía a la textura DENTRO
//!   de ese tick, sin ninguna cola: cualquier espera de red o de CPU se veía.
//! * El audio abría la URL por segunda vez (dos descargas del mismo archivo)
//!   y su decodificación dependía del reloj del video, no al revés.
//! * Se creaba un `swscale` nuevo por cada cuadro.
//!
//! ## Cómo funciona este motor
//!
//! ```text
//!  [demux] --paquetes--> [decode video] --cuadros RGBA--> cola --> UI (elige el cuadro que toca)
//!     |
//!     +-----paquetes--> [decode audio + resample] --> AudioFeed --> callback de cpal
//!                                                        ^              |
//!                                                        +-- reloj <----+  (muestras reproducidas)
//! ```
//!
//! * UNA sola apertura de la URL; el demuxer lee por adelantado.
//! * El reloj maestro es el audio (muestras realmente entregadas a la placa de
//!   sonido). Sin audio, un reloj monotónico (`Instant`).
//! * La UI, en cada frame, toma el último cuadro cuyo `pts` ya venció, descarta
//!   los atrasados y vuelve a dormirse justo hasta el próximo. Si algo se
//!   atrasa, se saltan cuadros de a uno en vez de congelar la imagen.
//! * El `swscale` se crea una vez y se reutiliza.

#![allow(missing_docs)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, ColorImage, TextureHandle, TextureId, TextureOptions, Vec2};
use ffmpeg::format::Pixel;
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context as ScalerContext, flag::Flags};
use parking_lot::Mutex;

use crate::AudioDevice;

// ---------------------------------------------------------------------
// Parámetros
// ---------------------------------------------------------------------

/// Paquetes de video en cola a partir de los cuales el demuxer deja de leer
/// (si el audio tampoco necesita nada). ~10 s a 30 fps.
const V_CAP: usize = 300;
/// Tope duro para que un archivo raro no llene la memoria.
const V_HARD: usize = 1500;
/// Ídem para audio (~17 s con paquetes de ~21 ms).
const A_CAP: usize = 800;
const A_HARD: usize = 4000;
/// Cuadros ya decodificados esperando en el canal hacia la UI.
const FRAME_QUEUE: usize = 3;
/// Cuadros que la UI guarda a la espera de su hora.
const PENDING_MAX: usize = 3;
/// Audio mínimo en el buffer antes de empezar a sonar.
const AUDIO_PREROLL_MS: usize = 200;
/// Audio máximo decodificado por adelantado.
const AUDIO_MAX_BUFFER_MS: usize = 1000;
/// Tras un seek, los cuadros anteriores a `objetivo - esto` ni se convierten.
const SKIP_BEFORE_TARGET_MS: i64 = 300;
/// Tolerancia al decidir si un cuadro "ya toca".
const FRAME_SLACK_MS: i64 = 4;
/// Máximo de bytes de paquetes que se guardan para repetir un video en bucle
/// sin volver a la red.
const LOOP_CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Tope blando de bytes de paquetes (comprimidos) en cola, sumando TODOS los
/// reproductores. Los paquetes los reserva ffmpeg con su propio malloc (no el
/// de Rust), así que a 4K / bitrates altos 300 paquetes pueden ser decenas de MB.
const PKT_BYTES_SOFT: usize = 48 * 1024 * 1024;
/// Aunque se pase del tope, se sigue leyendo hasta tener al menos esto en cola
/// de video (~2 s), para no dejar sin datos al decoder ni al audio.
const V_MIN_BUFFERED: usize = 60;
/// Hilos máximos del decoder. Con `thread_count = 0` (automático) ffmpeg usa
/// uno por núcleo (hasta 16) y CADA hilo de frame-threading retiene un cuadro
/// YUV completo en vuelo: en 1080p son ~3 MB por hilo, en 4K ~12 MB.
const MAX_DECODE_THREADS: usize = 3;
/// Alto máximo del cuadro ya convertido (si el video es más grande se reduce
/// al convertirlo).
const MAX_FRAME_HEIGHT: u32 = 1080;

const UNSET: i64 = i64::MIN;

// ---------------------------------------------------------------------
// Medición de memoria (para Ajustes → Memoria)
// ---------------------------------------------------------------------

/// Bytes de paquetes (comprimidos) esperando a ser decodificados.
static PKT_BYTES: AtomicUsize = AtomicUsize::new(0);
/// Paquetes guardados para repetir un GIF en bucle sin volver a la red.
static LOOP_BYTES: AtomicUsize = AtomicUsize::new(0);
/// Cuadros RGBA ya convertidos que todavía no llegaron a la textura.
static FRAME_BYTES: AtomicUsize = AtomicUsize::new(0);
/// Estimación de lo que ffmpeg reserva por su cuenta (fuera del heap de Rust).
static NATIVE_EST: AtomicUsize = AtomicUsize::new(0);
static ENGINES: AtomicUsize = AtomicUsize::new(0);

/// Lo que ocupan ahora todos los reproductores, por partes.
#[derive(Clone, Copy, Debug, Default)]
pub struct EngineMem {
    /// Reproductores abiertos (videos y GIFs en bucle).
    pub engines: usize,
    /// Paquetes comprimidos en cola (heap de Rust).
    pub packets: usize,
    /// Caché de bucle de los GIFs (heap de Rust).
    pub loop_cache: usize,
    /// Cuadros convertidos en cola, ~8 MB cada uno en 1080p (heap de Rust).
    pub frames: usize,
    /// Memoria propia de ffmpeg (decoder, hilos, scaler): ESTIMADA, no
    /// pasa por el contador del heap de Rust.
    pub native_estimate: usize,
}

impl EngineMem {
    /// Lo medido dentro del heap de Rust.
    pub fn rust_heap(&self) -> usize {
        self.packets + self.loop_cache + self.frames
    }
}

pub fn mem_stats() -> EngineMem {
    EngineMem {
        engines: ENGINES.load(Ordering::Relaxed),
        packets: PKT_BYTES.load(Ordering::Relaxed),
        loop_cache: LOOP_BYTES.load(Ordering::Relaxed),
        frames: FRAME_BYTES.load(Ordering::Relaxed),
        native_estimate: NATIVE_EST.load(Ordering::Relaxed),
    }
}

/// Un paquete que se cuenta mientras está vivo (también si se descarta sin
/// llegar al decoder porque el reproductor se cerró).
struct Counted {
    pkt: ffmpeg::Packet,
    bytes: usize,
}

impl Counted {
    fn new(pkt: ffmpeg::Packet) -> Self {
        let bytes = pkt.size();
        PKT_BYTES.fetch_add(bytes, Ordering::Relaxed);
        Self { pkt, bytes }
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        PKT_BYTES.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

/// Un cuadro RGBA que se cuenta mientras está vivo.
struct Tracked {
    image: ColorImage,
    bytes: usize,
}

impl Tracked {
    fn new(image: ColorImage) -> Self {
        let bytes = image.pixels.len() * 4;
        FRAME_BYTES.fetch_add(bytes, Ordering::Relaxed);
        Self { image, bytes }
    }

    /// Entrega la imagen (el conteo se descuenta al soltar `self`).
    fn into_image(mut self) -> ColorImage {
        std::mem::take(&mut self.image)
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        FRAME_BYTES.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------
// Tipos compartidos entre hilos
// ---------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Info {
    duration_ms: i64,
    width: u32,
    height: u32,
    has_audio: bool,
}

enum OpenState {
    Opening,
    Ready(Info),
    Failed(String),
}

struct Shared {
    stop: AtomicBool,
    /// Se incrementa en cada seek; todo lo que lleve otra época se descarta.
    epoch: AtomicU64,
    /// Instante absoluto (ms) al que apunta el último seek (`UNSET` si no hay).
    seek_target_ms: AtomicI64,
    /// `pts` (ms) del primer cuadro del video: el "0:00" que ve el usuario.
    origin_ms: AtomicI64,
    v_q: AtomicUsize,
    a_q: AtomicUsize,
    open: Mutex<OpenState>,
}

enum Pkt {
    Data(Counted, u64),
    Flush(u64),
    Eof(u64),
}

enum VMsg {
    Frame { epoch: u64, pts_ms: i64, image: Tracked },
    Eof(u64),
}

enum Cmd {
    Seek { ms: i64, epoch: u64, restart: bool },
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum DemuxState {
    Reading,
    Replay(usize),
    Eof,
}

fn rational_secs(r: ffmpeg::Rational) -> f64 {
    r.numerator() as f64 / r.denominator().max(1) as f64
}

fn ticks_to_ms(ticks: i64, tb_secs: f64) -> i64 {
    (ticks as f64 * tb_secs * 1000.0).round() as i64
}

/// En Windows, `thread::sleep`/`WaitUntil` redondean a ~15.6 ms salvo que se
/// pida más resolución al sistema. Se hace una sola vez por proceso.
#[cfg(windows)]
fn raise_timer_resolution() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    #[link(name = "winmm")]
    extern "system" {
        fn timeBeginPeriod(period: u32) -> u32;
    }
    ONCE.call_once(|| unsafe {
        timeBeginPeriod(1);
    });
}

#[cfg(not(windows))]
fn raise_timer_resolution() {}

// ---------------------------------------------------------------------
// Audio: buffer entre el hilo de decodificación y el callback de cpal
// ---------------------------------------------------------------------

/// Cola de muestras `f32` estéreo intercaladas, a la frecuencia del
/// dispositivo. El callback de audio consume de acá y, al hacerlo, cuenta
/// cuántas muestras se reprodujeron de verdad: eso ES el reloj del video.
pub struct AudioFeed {
    inner: Mutex<FeedInner>,
    pub(crate) rate: u32,
    base_ms: AtomicI64,
    played_frames: AtomicU64,
    started: AtomicBool,
    paused: AtomicBool,
    has_base: AtomicBool,
    eof: AtomicBool,
    gain: AtomicU32,
}

struct FeedInner {
    buf: VecDeque<f32>,
    epoch: u64,
}

impl AudioFeed {
    fn new(rate: u32) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(FeedInner { buf: VecDeque::new(), epoch: 0 }),
            rate: rate.max(1),
            base_ms: AtomicI64::new(0),
            played_frames: AtomicU64::new(0),
            started: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            has_base: AtomicBool::new(false),
            eof: AtomicBool::new(false),
            gain: AtomicU32::new(0.0f32.to_bits()),
        })
    }

    /// Vacía todo y arranca de nuevo (seek o reinicio).
    fn reset(&self, epoch: u64, base_ms: i64) {
        let mut g = self.inner.lock();
        g.buf.clear();
        g.epoch = epoch;
        self.played_frames.store(0, Ordering::Relaxed);
        self.base_ms.store(base_ms, Ordering::Relaxed);
        self.started.store(false, Ordering::Relaxed);
        self.has_base.store(false, Ordering::Relaxed);
        self.eof.store(false, Ordering::Relaxed);
    }

    /// Fija el instante del primer sample (una sola vez tras cada `reset`).
    fn begin(&self, epoch: u64, pts_ms: i64) -> bool {
        let g = self.inner.lock();
        if g.epoch != epoch {
            return false;
        }
        if !self.has_base.load(Ordering::Relaxed) {
            self.base_ms.store(pts_ms, Ordering::Relaxed);
            self.played_frames.store(0, Ordering::Relaxed);
            self.has_base.store(true, Ordering::Relaxed);
        }
        true
    }

    fn push(&self, epoch: u64, samples: &[f32]) -> bool {
        let mut g = self.inner.lock();
        if g.epoch != epoch {
            return false;
        }
        g.buf.extend(samples.iter().copied());
        true
    }

    fn buffered_frames(&self) -> usize {
        self.inner.lock().buf.len() / 2
    }

    fn maybe_start(&self) {
        if !self.started.load(Ordering::Relaxed)
            && self.buffered_frames() >= self.rate as usize * AUDIO_PREROLL_MS / 1000
        {
            self.started.store(true, Ordering::Relaxed);
        }
    }

    fn mark_eof(&self, epoch: u64) {
        let g = self.inner.lock();
        if g.epoch == epoch {
            self.eof.store(true, Ordering::Relaxed);
            self.started.store(true, Ordering::Relaxed);
        }
    }

    fn has_base(&self) -> bool {
        self.has_base.load(Ordering::Relaxed)
    }

    fn clock_ms(&self) -> i64 {
        let played = self.played_frames.load(Ordering::Relaxed);
        self.base_ms.load(Ordering::Relaxed) + (played * 1000 / self.rate as u64) as i64
    }

    /// El audio terminó y ya no queda nada por sonar.
    fn drained(&self) -> bool {
        self.eof.load(Ordering::Relaxed) && self.inner.lock().buf.is_empty()
    }

    fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    fn set_gain(&self, gain: f32) {
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// Lo llama el callback de cpal: SUMA en `out` (estéreo intercalado) lo
    /// que haya en el buffer. Si está en pausa o todavía no arrancó, no
    /// consume nada (y por lo tanto el reloj no avanza).
    pub(crate) fn mix_into(&self, out: &mut [f32]) {
        if !self.started.load(Ordering::Relaxed) || self.paused.load(Ordering::Relaxed) {
            return;
        }
        let gain = f32::from_bits(self.gain.load(Ordering::Relaxed));
        let mut g = self.inner.lock();
        let n = out.len().min(g.buf.len()) & !1;
        if n == 0 {
            return;
        }
        for (o, s) in out.iter_mut().zip(g.buf.drain(..n)) {
            *o += s * gain;
        }
        self.played_frames.fetch_add((n / 2) as u64, Ordering::Relaxed);
    }
}

impl AudioDevice {
    /// Conecta un `AudioFeed` a la mezcla del dispositivo.
    pub(crate) fn attach_feed(&self, feed: Arc<AudioFeed>) {
        self.callback.lock().feeds.push(feed);
        self.play();
    }
}

// ---------------------------------------------------------------------
// Conversión de cuadros
// ---------------------------------------------------------------------

struct Scaler {
    ctx: Option<ScalerContext>,
    key: (Pixel, u32, u32),
    out: ffmpeg::frame::Video,
}

impl Scaler {
    fn new() -> Self {
        Self { ctx: None, key: (Pixel::None, 0, 0), out: ffmpeg::frame::Video::empty() }
    }

    fn convert(&mut self, frame: &ffmpeg::frame::Video, max_h: u32) -> Option<ColorImage> {
        let key = (frame.format(), frame.width(), frame.height());
        if key.1 == 0 || key.2 == 0 {
            return None;
        }
        if self.ctx.is_none() || self.key != key {
            let (dw, dh) = fit_dims(key.1, key.2, max_h);
            self.ctx = ScalerContext::get(key.0, key.1, key.2, Pixel::RGBA, dw, dh, Flags::BILINEAR).ok();
            self.key = key;
            self.out = ffmpeg::frame::Video::empty();
        }
        let ctx = self.ctx.as_mut()?;
        ctx.run(frame, &mut self.out).ok()?;
        Some(rgba_to_image(&self.out))
    }
}

fn fit_dims(w: u32, h: u32, max_h: u32) -> (u32, u32) {
    if h <= max_h {
        return (w, h);
    }
    let dh = max_h & !1;
    let dw = ((w as u64 * dh as u64 / h as u64) as u32) & !1;
    (dw.max(2), dh.max(2))
}

/// Copia un cuadro RGBA (con su `stride`) a un `ColorImage` compacto.
fn rgba_to_image(frame: &ffmpeg::frame::Video) -> ColorImage {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let data = frame.data(0);
    let stride = frame.stride(0);
    let row_bytes = width * 4;
    let mut pixels: Vec<Color32> = Vec::with_capacity(width * height);
    for line in 0..height {
        let begin = line * stride;
        let row: &[Color32] = bytemuck::cast_slice(&data[begin..begin + row_bytes]);
        pixels.extend_from_slice(row);
    }
    ColorImage {
        size: [width, height],
        source_size: Vec2::new(width as f32, height as f32),
        pixels,
    }
}

// ---------------------------------------------------------------------
// Apertura del archivo
// ---------------------------------------------------------------------

struct AudioSetup {
    index: usize,
    tb: f64,
    in_rate: u32,
    dec: ffmpeg::decoder::Audio,
    resampler: ffmpeg::software::resampling::Context,
}

struct Setup {
    ictx: ffmpeg::format::context::Input,
    v_index: usize,
    v_tb: f64,
    v_dec: ffmpeg::decoder::Video,
    audio: Option<AudioSetup>,
    duration_ms: i64,
    width: u32,
    height: u32,
}

/// Hilos que usa el decoder de video cuando se pide multihilo.
fn decode_threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, MAX_DECODE_THREADS)
}

fn open_all(url: &String, audio_rate: Option<u32>, frame_threads: bool) -> anyhow::Result<Setup> {
    let ictx = ffmpeg::format::input(url)?;

    let (v_index, v_tb, mut v_ctx) = {
        let s = ictx.streams().best(Type::Video).ok_or(ffmpeg::Error::StreamNotFound)?;
        (
            s.index(),
            rational_secs(s.time_base()),
            ffmpeg::codec::context::Context::from_parameters(s.parameters())?,
        )
    };
    if frame_threads {
        // Decodificación multihilo (cuadros + rebanadas), acotada a
        // `MAX_DECODE_THREADS` para no multiplicar los cuadros en vuelo.
        // Va ANTES de abrir el decoder. Para los GIF/embeds cortos no se usa.
        unsafe {
            let p = v_ctx.as_mut_ptr();
            (*p).thread_count = decode_threads() as i32;
            (*p).thread_type = 3; // FF_THREAD_FRAME | FF_THREAD_SLICE
        }
    }
    let v_dec = v_ctx.decoder().video()?;
    let (width, height) = (v_dec.width(), v_dec.height());

    let audio = audio_rate.and_then(|rate| {
        (|| -> anyhow::Result<AudioSetup> {
            let s = ictx.streams().best(Type::Audio).ok_or(ffmpeg::Error::StreamNotFound)?;
            let index = s.index();
            let tb = rational_secs(s.time_base());
            let dec = ffmpeg::codec::context::Context::from_parameters(s.parameters())?
                .decoder()
                .audio()?;
            let in_rate = dec.rate();
            let resampler = ffmpeg::software::resampling::context::Context::get2(
                dec.format(),
                dec.ch_layout(),
                dec.rate(),
                // Siempre f32 empaquetado; el callback de cpal convierte.
                ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
                ffmpeg::ChannelLayout::STEREO,
                rate,
            )?;
            Ok(AudioSetup { index, tb, in_rate, dec, resampler })
        })()
        .ok()
    });

    let d = ictx.duration();
    let duration_ms = if d > 0 { d / 1000 } else { 0 };

    Ok(Setup { ictx, v_index, v_tb, v_dec, audio, duration_ms, width, height })
}

// ---------------------------------------------------------------------
// Hilo de demux
// ---------------------------------------------------------------------

#[derive(Default)]
struct LoopCache {
    pkts: Vec<(bool, ffmpeg::Packet)>,
    bytes: usize,
    /// Lo que este caché suma a `LOOP_BYTES` (se descuenta al soltarlo).
    counted: usize,
    complete: bool,
    overflow: bool,
}

impl Drop for LoopCache {
    fn drop(&mut self) {
        LOOP_BYTES.fetch_sub(self.counted, Ordering::Relaxed);
    }
}

fn route(
    pkt: ffmpeg::Packet,
    is_video: bool,
    epoch: u64,
    vp_tx: &Sender<Pkt>,
    ap_tx: &Sender<Pkt>,
    shared: &Shared,
) -> bool {
    if is_video {
        shared.v_q.fetch_add(1, Ordering::Relaxed);
        vp_tx.send(Pkt::Data(Counted::new(pkt), epoch)).is_ok()
    } else {
        shared.a_q.fetch_add(1, Ordering::Relaxed);
        ap_tx.send(Pkt::Data(Counted::new(pkt), epoch)).is_ok()
    }
}

#[allow(clippy::too_many_arguments)]
fn demux_main(
    url: String,
    shared: Arc<Shared>,
    cmd_rx: Receiver<Cmd>,
    vtx: SyncSender<VMsg>,
    feed: Option<Arc<AudioFeed>>,
    looping: bool,
    ctx: egui::Context,
) {
    let audio_rate = feed.as_ref().map(|f| f.rate);
    let setup = match open_all(&url, audio_rate, !looping) {
        Ok(s) => s,
        Err(e) => {
            *shared.open.lock() = OpenState::Failed(e.to_string());
            ctx.request_repaint();
            return;
        }
    };
    let Setup { mut ictx, v_index, v_tb, v_dec, audio, duration_ms, width, height } = setup;

    let has_audio = audio.is_some();
    *shared.open.lock() = OpenState::Ready(Info { duration_ms, width, height, has_audio });
    ctx.request_repaint();

    let (vp_tx, vp_rx) = mpsc::channel::<Pkt>();
    let (ap_tx, ap_rx) = mpsc::channel::<Pkt>();

    {
        let shared = shared.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("ecord-video-decode".into())
            .spawn(move || video_thread(v_dec, v_tb, vp_rx, vtx, shared, ctx));
    }

    let mut a_index = None;
    match (audio, feed) {
        (Some(a), Some(feed)) => {
            a_index = Some(a.index);
            let shared = shared.clone();
            let _ = std::thread::Builder::new()
                .name("ecord-audio-decode".into())
                .spawn(move || audio_thread(a.dec, a.resampler, a.tb, a.in_rate, ap_rx, feed, shared));
        }
        _ => drop(ap_rx),
    }

    let mut epoch: u64 = 0;
    let mut state = DemuxState::Reading;
    let mut cache = LoopCache::default();
    let mut cache_on = looping;

    loop {
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }

        let cmd = if state == DemuxState::Eof {
            match cmd_rx.recv_timeout(Duration::from_millis(20)) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match cmd_rx.try_recv() {
                Ok(c) => Some(c),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => break,
            }
        };

        if let Some(Cmd::Seek { ms, epoch: e, restart }) = cmd {
            epoch = e;
            if restart && cache.complete {
                // Bucle de un video corto: se repiten los paquetes que ya
                // están en memoria, sin volver a tocar la red.
                state = DemuxState::Replay(0);
            } else {
                let ts = ms.saturating_mul(1000); // AV_TIME_BASE = microsegundos
                let _ = ictx.seek(ts, ..=ts);
                state = DemuxState::Reading;
                cache_on = false;
            }
            let _ = vp_tx.send(Pkt::Flush(e));
            if a_index.is_some() {
                let _ = ap_tx.send(Pkt::Flush(e));
            }
            continue;
        }

        if state == DemuxState::Eof {
            continue;
        }

        // Freno: no leer más si lo que ya hay en cola alcanza.
        let vq = shared.v_q.load(Ordering::Relaxed);
        let aq = shared.a_q.load(Ordering::Relaxed);
        let v_full = vq >= V_CAP;
        let a_full = a_index.is_none() || aq >= A_CAP;
        let over_budget = vq >= V_MIN_BUFFERED && PKT_BYTES.load(Ordering::Relaxed) >= PKT_BYTES_SOFT;
        if (v_full && a_full) || vq >= V_HARD || aq >= A_HARD || over_budget {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }

        let mut hit_eof = false;

        if let DemuxState::Replay(i) = state {
            if i >= cache.pkts.len() {
                hit_eof = true;
            } else {
                let (is_v, pkt) = &cache.pkts[i];
                if !route(pkt.clone(), *is_v, epoch, &vp_tx, &ap_tx, &shared) && *is_v {
                    break;
                }
                state = DemuxState::Replay(i + 1);
            }
        } else {
            match ictx.packets().next() {
                Some(Ok((stream, packet))) => {
                    let idx = stream.index();
                    let is_v = idx == v_index;
                    let is_a = Some(idx) == a_index;
                    if is_v || is_a {
                        if cache_on {
                            cache.bytes += packet.size();
                            if cache.bytes > LOOP_CACHE_MAX_BYTES {
                                cache.pkts.clear();
                                cache.pkts.shrink_to_fit();
                                LOOP_BYTES.fetch_sub(cache.counted, Ordering::Relaxed);
                                cache.counted = 0;
                                cache.overflow = true;
                                cache_on = false;
                            } else {
                                LOOP_BYTES.fetch_add(packet.size(), Ordering::Relaxed);
                                cache.counted += packet.size();
                                cache.pkts.push((is_v, packet.clone()));
                            }
                        }
                        if !route(packet, is_v, epoch, &vp_tx, &ap_tx, &shared) && is_v {
                            break;
                        }
                    }
                }
                Some(Err(e)) => {
                    if matches!(e, ffmpeg::Error::Other { errno } if errno == libc::EAGAIN) {
                        std::thread::sleep(Duration::from_millis(2));
                    } else {
                        hit_eof = true;
                    }
                }
                None => hit_eof = true,
            }
        }

        if hit_eof {
            if cache_on {
                cache.complete = !cache.overflow;
                cache_on = false;
            }
            let _ = vp_tx.send(Pkt::Eof(epoch));
            if a_index.is_some() {
                let _ = ap_tx.send(Pkt::Eof(epoch));
            }
            state = DemuxState::Eof;
        }
    }
}

// ---------------------------------------------------------------------
// Hilo de decodificación de video
// ---------------------------------------------------------------------

/// Manda `msg` a la UI esperando si la cola está llena, pero sin colgarse si
/// mientras tanto hay un seek o se cierra el motor.
fn push_msg(tx: &SyncSender<VMsg>, mut msg: VMsg, shared: &Shared, epoch: u64) -> bool {
    loop {
        if shared.stop.load(Ordering::Relaxed) || shared.epoch.load(Ordering::Acquire) != epoch {
            return false;
        }
        match tx.try_send(msg) {
            Ok(()) => return true,
            Err(TrySendError::Full(m)) => {
                msg = m;
                std::thread::sleep(Duration::from_millis(3));
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

fn video_thread(
    mut dec: ffmpeg::decoder::Video,
    tb: f64,
    rx: Receiver<Pkt>,
    tx: SyncSender<VMsg>,
    shared: Arc<Shared>,
    ctx: egui::Context,
) {
    let mut scaler = Scaler::new();
    while let Ok(msg) = rx.recv() {
        match msg {
            Pkt::Data(pkt, epoch) => {
                shared.v_q.fetch_sub(1, Ordering::Relaxed);
                if epoch != shared.epoch.load(Ordering::Acquire) {
                    continue;
                }
                if dec.0.send_packet(&pkt.pkt).is_err() {
                    continue;
                }
                drain_video(&mut dec, tb, epoch, &tx, &shared, &ctx, &mut scaler);
            }
            Pkt::Flush(_) => {
                dec.0.flush();
            }
            Pkt::Eof(epoch) => {
                if epoch != shared.epoch.load(Ordering::Acquire) {
                    continue;
                }
                let _ = dec.0.send_eof();
                if drain_video(&mut dec, tb, epoch, &tx, &shared, &ctx, &mut scaler) {
                    let _ = push_msg(&tx, VMsg::Eof(epoch), &shared, epoch);
                    ctx.request_repaint();
                }
            }
        }
    }
}

/// Saca del decoder todos los cuadros disponibles. Devuelve `false` si se
/// abortó (seek nuevo o cierre).
fn drain_video(
    dec: &mut ffmpeg::decoder::Video,
    tb: f64,
    epoch: u64,
    tx: &SyncSender<VMsg>,
    shared: &Shared,
    ctx: &egui::Context,
    scaler: &mut Scaler,
) -> bool {
    loop {
        let mut frame = ffmpeg::frame::Video::empty();
        if dec.receive_frame(&mut frame).is_err() {
            return true; // necesita más paquetes, o terminó
        }
        if shared.epoch.load(Ordering::Acquire) != epoch {
            return false;
        }
        let pts_ms = match frame.timestamp().or(frame.pts()) {
            Some(p) => ticks_to_ms(p, tb),
            None => continue,
        };
        let _ = shared.origin_ms.compare_exchange(UNSET, pts_ms, Ordering::Relaxed, Ordering::Relaxed);

        // Tras un seek, el decoder arranca en el keyframe anterior: los
        // cuadros que quedan muy atrás del objetivo no valen la conversión.
        let target = shared.seek_target_ms.load(Ordering::Relaxed);
        if target != UNSET && pts_ms < target - SKIP_BEFORE_TARGET_MS {
            continue;
        }

        let Some(image) = scaler.convert(&frame, MAX_FRAME_HEIGHT) else { continue };
        if !push_msg(tx, VMsg::Frame { epoch, pts_ms, image: Tracked::new(image) }, shared, epoch) {
            return false;
        }
        ctx.request_repaint();
    }
}

// ---------------------------------------------------------------------
// Hilo de decodificación de audio
// ---------------------------------------------------------------------

fn audio_thread(
    mut dec: ffmpeg::decoder::Audio,
    mut resampler: ffmpeg::software::resampling::Context,
    tb: f64,
    in_rate: u32,
    rx: Receiver<Pkt>,
    feed: Arc<AudioFeed>,
    shared: Arc<Shared>,
) {
    let mut next_pts_ms: Option<i64> = None;
    while let Ok(msg) = rx.recv() {
        match msg {
            Pkt::Data(pkt, epoch) => {
                shared.a_q.fetch_sub(1, Ordering::Relaxed);
                if epoch != shared.epoch.load(Ordering::Acquire) {
                    continue;
                }
                if dec.0.send_packet(&pkt.pkt).is_err() {
                    continue;
                }
                drain_audio(&mut dec, &mut resampler, tb, in_rate, epoch, &feed, &shared, &mut next_pts_ms);
            }
            Pkt::Flush(_) => {
                dec.0.flush();
                next_pts_ms = None;
            }
            Pkt::Eof(epoch) => {
                if epoch != shared.epoch.load(Ordering::Acquire) {
                    continue;
                }
                let _ = dec.0.send_eof();
                if drain_audio(&mut dec, &mut resampler, tb, in_rate, epoch, &feed, &shared, &mut next_pts_ms) {
                    feed.mark_eof(epoch);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn drain_audio(
    dec: &mut ffmpeg::decoder::Audio,
    resampler: &mut ffmpeg::software::resampling::Context,
    tb: f64,
    in_rate: u32,
    epoch: u64,
    feed: &AudioFeed,
    shared: &Shared,
    next_pts_ms: &mut Option<i64>,
) -> bool {
    loop {
        let mut frame = ffmpeg::frame::Audio::empty();
        if dec.receive_frame(&mut frame).is_err() {
            return true;
        }
        if shared.epoch.load(Ordering::Acquire) != epoch {
            return false;
        }
        let n_in = frame.samples() as i64;
        let dur_ms = n_in * 1000 / in_rate.max(1) as i64;
        let pts_ms = match frame.timestamp().or(frame.pts()) {
            Some(p) => ticks_to_ms(p, tb),
            None => next_pts_ms.unwrap_or(0),
        };
        *next_pts_ms = Some(pts_ms + dur_ms);

        // Tras un seek, el audio anterior al objetivo se descarta.
        let target = shared.seek_target_ms.load(Ordering::Relaxed);
        if target != UNSET && pts_ms + dur_ms <= target {
            continue;
        }

        let mut out = ffmpeg::frame::Audio::empty();
        if resampler.run(&frame, &mut out).is_err() || out.samples() == 0 {
            continue;
        }
        let samples: &[f32] = crate::packed::<f32>(&out);

        // No decodificar mucho más adelante de lo que suena.
        let max_frames = feed.rate as usize * AUDIO_MAX_BUFFER_MS / 1000;
        while feed.buffered_frames() > max_frames {
            if shared.stop.load(Ordering::Relaxed) || shared.epoch.load(Ordering::Acquire) != epoch {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        if !feed.begin(epoch, pts_ms) || !feed.push(epoch, samples) {
            return false;
        }
        feed.maybe_start();
    }
}

// ---------------------------------------------------------------------
// Reloj sin audio
// ---------------------------------------------------------------------

#[derive(Default)]
struct SysClock {
    base_ms: Option<i64>,
    since: Option<Instant>,
}

impl SysClock {
    fn now(&self) -> Option<i64> {
        self.base_ms.map(|b| b + self.since.map_or(0, |s| s.elapsed().as_millis() as i64))
    }
    fn set(&mut self, ms: i64) {
        self.base_ms = Some(ms);
        if self.since.is_some() {
            self.since = Some(Instant::now());
        }
    }
    fn run(&mut self) {
        if self.since.is_none() && self.base_ms.is_some() {
            self.since = Some(Instant::now());
        }
    }
    fn freeze(&mut self) {
        if let Some(n) = self.now() {
            self.base_ms = Some(n);
        }
        self.since = None;
    }
}

// ---------------------------------------------------------------------
// API para la UI
// ---------------------------------------------------------------------

/// Estado de apertura del motor.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineStatus {
    /// Abriendo el archivo / red.
    Opening,
    /// Listo (puede que todavía no haya llegado el primer cuadro: ver
    /// [`Engine::has_frame`]).
    Ready,
    /// No se pudo abrir o decodificar.
    Failed(String),
}

/// Un video en reproducción. Se crea con [`Engine::open`] (no bloquea) y hay
/// que llamar a [`Engine::update`] una vez por frame de UI mientras se dibuja.
pub struct Engine {
    shared: Arc<Shared>,
    cmd_tx: Sender<Cmd>,
    video_rx: Receiver<VMsg>,
    feed: Option<Arc<AudioFeed>>,
    texture: TextureHandle,
    tex_opts: TextureOptions,
    info: Option<Info>,
    failed: Option<String>,
    pending: VecDeque<(i64, Tracked)>,
    /// Lo que se estimó de memoria nativa de ffmpeg para este reproductor.
    native_est: usize,
    sys: SysClock,
    use_audio_clock: bool,
    audio_capable: bool,
    paused: bool,
    ended: bool,
    video_eof: bool,
    has_frame: bool,
    awaiting_frame: bool,
    frame_size: [usize; 2],
    looping: bool,
    volume: f32,
    muted: bool,
    waiting_since: Option<Instant>,
    stall_since: Option<Instant>,
    /// Cuándo fue el último seek (o la apertura): acota el sondeo mientras se
    /// espera el primer cuadro con el video en pausa.
    seek_at: Instant,
    epoch: u64,
}

impl Engine {
    /// Empieza a abrir `url` en segundo plano.
    ///
    /// * `audio`: dispositivo de salida; `None` = video mudo (GIFs).
    /// * `looping`: al terminar vuelve a empezar solo (usa un caché en
    ///   memoria si el video es corto, así no vuelve a la red).
    pub fn open(ctx: &egui::Context, url: &str, audio: Option<&AudioDevice>, looping: bool) -> Engine {
        raise_timer_resolution();
        ENGINES.fetch_add(1, Ordering::Relaxed);

        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            seek_target_ms: AtomicI64::new(UNSET),
            origin_ms: AtomicI64::new(UNSET),
            v_q: AtomicUsize::new(0),
            a_q: AtomicUsize::new(0),
            open: Mutex::new(OpenState::Opening),
        });
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (vtx, vrx) = mpsc::sync_channel(FRAME_QUEUE);

        let feed = audio.map(|dev| {
            let f = AudioFeed::new(dev.get_sample_rate());
            dev.attach_feed(f.clone());
            f
        });

        {
            let shared_t = shared.clone();
            let feed_t = feed.clone();
            let ctx_t = ctx.clone();
            let url_t = url.to_string();
            let spawned = std::thread::Builder::new()
                .name("ecord-video-demux".into())
                .spawn(move || demux_main(url_t, shared_t, cmd_rx, vtx, feed_t, looping, ctx_t));
            if let Err(e) = spawned {
                *shared.open.lock() = OpenState::Failed(e.to_string());
            }
        }

        let tex_opts = TextureOptions::LINEAR;
        let texture = ctx.load_texture("ecord_video", ColorImage::filled([1, 1], Color32::BLACK), tex_opts);

        Engine {
            shared,
            cmd_tx,
            video_rx: vrx,
            feed,
            texture,
            tex_opts,
            info: None,
            failed: None,
            pending: VecDeque::new(),
            native_est: 0,
            sys: SysClock::default(),
            use_audio_clock: false,
            audio_capable: false,
            paused: false,
            ended: false,
            video_eof: false,
            has_frame: false,
            awaiting_frame: true,
            frame_size: [0, 0],
            looping,
            volume: 0.6,
            muted: false,
            waiting_since: None,
            stall_since: None,
            seek_at: Instant::now(),
            epoch: 0,
        }
    }

    // ------------------------------ consultas ------------------------------

    pub fn status(&self) -> EngineStatus {
        if let Some(m) = &self.failed {
            return EngineStatus::Failed(m.clone());
        }
        if self.info.is_some() {
            EngineStatus::Ready
        } else {
            EngineStatus::Opening
        }
    }

    pub fn failed(&self) -> bool {
        self.failed.is_some()
    }

    /// ¿Ya llegó (y se pintó) al menos un cuadro?
    pub fn has_frame(&self) -> bool {
        self.has_frame
    }

    pub fn texture_id(&self) -> TextureId {
        self.texture.id()
    }

    /// Tamaño original del video (para respetar la proporción).
    pub fn size(&self) -> Option<Vec2> {
        self.info.as_ref().filter(|i| i.width > 0 && i.height > 0).map(|i| Vec2::new(i.width as f32, i.height as f32))
    }

    pub fn duration_ms(&self) -> i64 {
        self.info.as_ref().map_or(0, |i| i.duration_ms)
    }

    pub fn has_audio(&self) -> bool {
        self.audio_capable
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn has_ended(&self) -> bool {
        self.ended
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Hay que esperar a que lleguen datos (red lenta o CPU al límite).
    pub fn buffering(&self) -> bool {
        (self.info.is_some() && !self.has_frame)
            || self.stall_since.map_or(false, |t| t.elapsed() > Duration::from_millis(250))
    }

    /// Posición actual en ms desde el principio del video.
    pub fn position_ms(&self) -> i64 {
        if self.ended {
            return self.duration_ms();
        }
        let origin = self.shared.origin_ms.load(Ordering::Relaxed);
        if origin == UNSET {
            return 0;
        }
        let pos = self.clock_ms().map_or(0, |c| (c - origin).max(0));
        let dur = self.duration_ms();
        if dur > 0 {
            pos.min(dur)
        } else {
            pos
        }
    }

    // ------------------------------- control -------------------------------

    pub fn play(&mut self) {
        if self.failed.is_some() {
            return;
        }
        if self.ended {
            self.restart();
        }
        self.paused = false;
        if let Some(f) = &self.feed {
            f.set_paused(false);
        }
        if !self.use_audio_clock {
            self.sys.run();
        }
    }

    pub fn pause(&mut self) {
        self.paused = true;
        if let Some(f) = &self.feed {
            f.set_paused(true);
        }
        self.sys.freeze();
    }

    pub fn toggle_pause(&mut self) {
        if self.paused || self.ended {
            self.play();
        } else {
            self.pause();
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.apply_gain();
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        self.apply_gain();
    }

    fn apply_gain(&self) {
        if let Some(f) = &self.feed {
            let v = if self.muted { 0.0 } else { self.volume };
            // Curva cuadrática: el volumen se "siente" más parejo.
            f.set_gain(v * v);
        }
    }

    /// Salta a `ms` (desde el principio del video).
    pub fn seek_ms(&mut self, ms: i64) {
        let origin = self.shared.origin_ms.load(Ordering::Relaxed);
        if self.info.is_none() || origin == UNSET {
            return;
        }
        let dur = self.duration_ms();
        let rel = if dur > 0 { ms.clamp(0, dur) } else { ms.max(0) };
        if self.ended {
            self.ended = false;
            self.paused = false;
            if let Some(f) = &self.feed {
                f.set_paused(false);
            }
        }
        self.seek_abs(origin + rel, rel <= 50);
    }

    fn restart(&mut self) {
        let origin = self.shared.origin_ms.load(Ordering::Relaxed);
        if origin == UNSET {
            return;
        }
        self.ended = false;
        self.seek_abs(origin, true);
    }

    fn seek_abs(&mut self, abs_ms: i64, restart: bool) {
        self.epoch += 1;
        let epoch = self.epoch;
        self.shared.epoch.store(epoch, Ordering::Release);
        self.shared.seek_target_ms.store(if restart { UNSET } else { abs_ms }, Ordering::Relaxed);

        self.pending.clear();
        while self.video_rx.try_recv().is_ok() {}
        self.video_eof = false;
        self.awaiting_frame = true;
        self.waiting_since = None;
        self.stall_since = None;
        self.seek_at = Instant::now();

        if let Some(f) = &self.feed {
            f.reset(epoch, abs_ms);
        }
        if self.audio_capable {
            self.use_audio_clock = true;
        }
        self.sys.set(abs_ms);
        if self.use_audio_clock {
            self.sys.freeze();
        } else if !self.paused {
            self.sys.run();
        }
        let _ = self.cmd_tx.send(Cmd::Seek { ms: abs_ms, epoch, restart });
    }

    // ------------------------------ por frame ------------------------------

    fn clock_ms(&self) -> Option<i64> {
        if self.use_audio_clock {
            let f = self.feed.as_ref()?;
            if f.has_base() {
                Some(f.clock_ms())
            } else {
                None
            }
        } else {
            self.sys.now()
        }
    }

    fn present(&mut self, image: ColorImage) {
        self.frame_size = image.size;
        self.texture.set(image, self.tex_opts);
        self.has_frame = true;
        self.awaiting_frame = false;
        self.stall_since = None;
        self.waiting_since = None;
    }

    /// Hay que llamarla una vez por frame de UI mientras el video se dibuja.
    pub fn update(&mut self, ctx: &egui::Context) {
        if self.failed.is_some() {
            return;
        }

        // 1. ¿Terminó de abrirse?
        if self.info.is_none() {
            let opened = match &*self.shared.open.lock() {
                OpenState::Opening => None,
                OpenState::Failed(m) => Some(Err(m.clone())),
                OpenState::Ready(i) => Some(Ok(i.clone())),
            };
            match opened {
                None => {
                    ctx.request_repaint_after(Duration::from_millis(40));
                    return;
                }
                Some(Err(m)) => {
                    self.failed = Some(m);
                    return;
                }
                Some(Ok(info)) => {
                    self.audio_capable = info.has_audio && self.feed.is_some();
                    self.use_audio_clock = self.audio_capable;
                    // Estimación de la memoria que ffmpeg reserva por su cuenta:
                    // los cuadros YUV 4:2:0 (1.5 B/px) que el decoder tiene en
                    // vuelo (uno por hilo de decodificación + referencias) y
                    // el cuadro RGBA de salida del scaler.
                    let threads = if self.looping { 1 } else { decode_threads() };
                    let px = (info.width as usize) * (info.height as usize);
                    let out_px = {
                        let (w, h) = fit_dims(info.width.max(2), info.height.max(2), MAX_FRAME_HEIGHT);
                        (w as usize) * (h as usize)
                    };
                    self.native_est = px * 3 / 2 * (threads + 6) + out_px * 4;
                    NATIVE_EST.fetch_add(self.native_est, Ordering::Relaxed);
                    self.info = Some(info);
                    self.apply_gain();
                    if !self.paused {
                        self.sys.run();
                    }
                }
            }
        }

        // 2. Cuadros nuevos.
        let epoch = self.epoch;
        while self.pending.len() < PENDING_MAX {
            match self.video_rx.try_recv() {
                Ok(VMsg::Frame { epoch: e, pts_ms, image }) => {
                    if e == epoch {
                        self.pending.push_back((pts_ms, image));
                    }
                }
                Ok(VMsg::Eof(e)) => {
                    if e == epoch {
                        self.video_eof = true;
                    }
                }
                Err(_) => break,
            }
        }

        // 3. Si el audio se acabó antes que el video, seguir con reloj propio.
        if self.use_audio_clock {
            let drained = self.feed.as_ref().map_or(false, |f| f.drained());
            if drained {
                let now = self.feed.as_ref().map_or(0, |f| f.clock_ms());
                self.use_audio_clock = false;
                self.sys.set(now);
                if !self.paused {
                    self.sys.run();
                }
            }
        }

        // 4. Sin reloj todavía (el audio no arrancó o no hay audio).
        if self.clock_ms().is_none() {
            if let Some(pts) = self.pending.front().map(|(p, _)| *p) {
                if !self.use_audio_clock {
                    self.sys.set(pts);
                    if !self.paused {
                        self.sys.run();
                    }
                } else {
                    let since = *self.waiting_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_millis(1500) {
                        // Hay audio "en el papel" pero no llega: no dejar el
                        // video congelado.
                        self.use_audio_clock = false;
                        self.sys.set(pts);
                        if !self.paused {
                            self.sys.run();
                        }
                    } else if !self.has_frame {
                        // Mientras tanto, mostrar el primer cuadro como portada.
                        if let Some((_, img)) = self.pending.pop_front() {
                            self.present(img.into_image());
                        }
                    }
                }
            }
        }

        // 5. Elegir el cuadro que toca: el último cuyo `pts` ya venció.
        if let Some(now) = self.clock_ms() {
            let mut chosen = None;
            while self.pending.front().map_or(false, |(p, _)| *p <= now + FRAME_SLACK_MS) {
                chosen = self.pending.pop_front();
            }
            if let Some((_, img)) = chosen {
                self.present(img.into_image());
            }
        }

        // 6. ¿Se terminó?
        if self.video_eof && self.pending.is_empty() && !self.ended {
            if self.looping {
                self.restart();
            } else {
                self.ended = true;
                self.pause();
            }
        }

        // 7. Detectar falta de datos y programar el próximo repintado.
        let playing = !self.paused && !self.ended;
        if playing && self.pending.is_empty() && !self.video_eof && self.has_frame {
            self.stall_since.get_or_insert_with(Instant::now);
        } else if !self.pending.is_empty() {
            self.stall_since = None;
        }

        if playing || (self.awaiting_frame && self.seek_at.elapsed() < Duration::from_secs(3)) {
            let wait = match (self.clock_ms(), self.pending.front()) {
                (Some(now), Some((pts, _))) => (*pts - now).clamp(1, 16),
                _ => 8,
            };
            ctx.request_repaint_after(Duration::from_millis(wait as u64));
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        ENGINES.fetch_sub(1, Ordering::Relaxed);
        NATIVE_EST.fetch_sub(self.native_est, Ordering::Relaxed);
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(f) = &self.feed {
            f.set_paused(true);
        }
    }
}
