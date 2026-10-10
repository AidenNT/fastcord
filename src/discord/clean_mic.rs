//! Parámetros de la cadena de limpieza de micrófono ("Clean Mic") y su medidor.
//!
//! Viven en un almacén global (no en `VoiceCaptureGate`, que es `Copy + Eq` y
//! viaja por varios canales) para que el popup de `ui::clean_mic_popup` pueda
//! cambiarlos en caliente: `voice/noise.rs` mira `version()` en cada frame y,
//! si cambió, relee `current()` y reconfigura filtros y EQ sin cortar el audio.
//!
//! Este módulo no depende de la feature `voice-playback`, así la UI compila
//! igual aunque no haya captura real de audio.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

pub const EQ_BANDS: usize = 5;
pub const EQ_BAND_NAMES: [&str; EQ_BANDS] = ["Graves", "Calidez", "Medios", "Presencia", "Agudos"];
const STORAGE_KEY: &str = "clean_mic_settings";

/// Una banda del EQ. La 0 es un shelf de graves, la última un shelf de agudos y
/// las del medio son campanas.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqBand {
    pub freq: f32,
    pub gain: f32,
    pub q: f32,
}

impl Default for EqBand {
    fn default() -> Self {
        Self { freq: 1000.0, gain: 0.0, q: 1.0 }
    }
}

/// Frecuencia y Q fijos de cada banda al elegir un estilo de voz.
const EQ_BASE: [(f32, f32); EQ_BANDS] =
    [(100.0, 0.7), (300.0, 1.0), (1000.0, 1.0), (3500.0, 0.8), (8000.0, 0.7)];

/// (clave, nombre, descripción, ganancias en dB por banda)
pub const PRESETS: [(&str, &str, &str, [f32; EQ_BANDS]); 6] = [
    ("natural", "Natural", "Tu voz, solo que más limpia.", [0.0, -2.0, 0.0, 2.5, 1.5]),
    ("warm", "Cálida", "Más llena y suave, para charlar.", [3.0, 0.5, -1.0, 1.0, -1.0]),
    ("crisp", "Nítida", "Brillante y clara, ideal para micrófonos baratos.", [-1.5, -3.0, 0.0, 4.0, 4.0]),
    ("radio", "Radio", "Sonido grande y pulido de podcast.", [4.0, -3.0, 1.0, 4.0, 2.5]),
    ("deep", "Grave", "Más peso y graves en la voz.", [6.0, 1.5, -1.0, 1.0, 0.0]),
    ("flat", "Plana", "Sin cambio de tono, solo limpieza.", [0.0, 0.0, 0.0, 0.0, 0.0]),
];

pub fn bands_for(preset: &str) -> [EqBand; EQ_BANDS] {
    let gains = PRESETS
        .iter()
        .find(|(key, ..)| *key == preset)
        .map_or(PRESETS[0].3, |(.., gains)| *gains);
    std::array::from_fn(|i| EqBand { freq: EQ_BASE[i].0, gain: gains[i], q: EQ_BASE[i].1 })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanMicSettings {
    /// Cancelación de eco (AEC3) usando lo que suena en tus parlantes.
    pub echo_cancel: bool,

    /// Filtro pasa-altos contra retumbos.
    pub high_pass: bool,
    pub high_pass_hz: f32,

    /// 0..1: cuánto se baja el fondo entre palabras (profundidad de la compuerta).
    pub strength: f32,

    /// Compuerta de voz.
    pub gate: bool,
    /// Cuántos dB más fuerte que el fondo tiene que ser para abrir.
    pub gate_margin_db: f32,
    /// Probabilidad de voz de RNNoise (0..1) a partir de la cual abre.
    pub gate_vad: f32,
    /// Cuánto se queda abierta después de hablar.
    pub gate_hold_ms: f32,

    /// Volumen automático (AGC).
    pub agc: bool,
    /// 0..1: nivel al que queda la voz (0.5 = normal).
    pub loudness: f32,

    /// Ecualizador.
    pub eq: bool,
    pub eq_preset: String,
    pub eq_bands: [EqBand; EQ_BANDS],

    pub compressor: bool,
    pub compressor_threshold_db: f32,
    pub compressor_ratio: f32,

    pub limiter: bool,
    pub limiter_ceiling_db: f32,
}

impl Default for CleanMicSettings {
    fn default() -> Self {
        Self {
            echo_cancel: true,
            high_pass: true,
            high_pass_hz: 80.0,
            strength: 0.7,
            gate: true,
            gate_margin_db: 8.0,
            gate_vad: 0.45,
            gate_hold_ms: 250.0,
            agc: true,
            loudness: 0.5,
            eq: true,
            eq_preset: "natural".to_owned(),
            eq_bands: bands_for("natural"),
            compressor: true,
            compressor_threshold_db: -18.0,
            compressor_ratio: 3.0,
            limiter: true,
            limiter_ceiling_db: -1.0,
        }
    }
}

struct Store {
    settings: RwLock<CleanMicSettings>,
    version: AtomicU64,
}

fn store() -> &'static Store {
    static STORE: OnceLock<Store> = OnceLock::new();
    STORE.get_or_init(|| Store { settings: RwLock::new(CleanMicSettings::default()), version: AtomicU64::new(1) })
}

pub fn current() -> CleanMicSettings {
    store().settings.read().map(|s| s.clone()).unwrap_or_default()
}

/// Sube cada vez que cambian los ajustes; el hilo de audio lo compara para saber
/// si tiene que releerlos.
pub fn version() -> u64 {
    store().version.load(Ordering::Acquire)
}

pub fn set(settings: CleanMicSettings) {
    if let Ok(mut guard) = store().settings.write() {
        *guard = settings;
    }
    store().version.fetch_add(1, Ordering::AcqRel);
}

/// Lee los ajustes guardados (al arrancar la app).
pub fn load_from_storage() {
    if let Ok(Some(json)) = web_local_storage_api::get_item(STORAGE_KEY) {
        if let Ok(settings) = serde_json::from_str::<CleanMicSettings>(&json) {
            set(settings);
        }
    }
}

pub fn save_to_storage() {
    if let Ok(json) = serde_json::to_string(&current()) {
        let _ = web_local_storage_api::set_item(STORAGE_KEY, &json);
    }
}

// ----------------------------------------------------------------- medidor

/// Lo último que midió la cadena, para el medidor del popup.
#[derive(Clone, Copy, Debug, Default)]
pub struct MeterSnapshot {
    /// Nivel de entrada en dBFS (tras el pasa-altos).
    pub level_db: f32,
    /// Probabilidad de voz de RNNoise (0..1).
    pub voice: f32,
    pub gate_open: bool,
    /// Si pasó audio por el procesador hace poco (hay llamada y micrófono transmitiendo).
    pub active: bool,
}

struct Meter {
    level_db: AtomicU32,
    voice: AtomicU32,
    gate_open: AtomicBool,
    last_update_ms: AtomicU64,
}

fn meter_store() -> &'static Meter {
    static METER: OnceLock<Meter> = OnceLock::new();
    METER.get_or_init(|| Meter {
        level_db: AtomicU32::new((-100.0f32).to_bits()),
        voice: AtomicU32::new(0.0f32.to_bits()),
        gate_open: AtomicBool::new(false),
        last_update_ms: AtomicU64::new(0),
    })
}

fn now_ms() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1
}

pub fn publish_meter(level_db: f32, voice: f32, gate_open: bool) {
    let meter = meter_store();
    meter.level_db.store(level_db.to_bits(), Ordering::Relaxed);
    meter.voice.store(voice.to_bits(), Ordering::Relaxed);
    meter.gate_open.store(gate_open, Ordering::Relaxed);
    meter.last_update_ms.store(now_ms(), Ordering::Relaxed);
}

pub fn meter() -> MeterSnapshot {
    let meter = meter_store();
    let last = meter.last_update_ms.load(Ordering::Relaxed);
    MeterSnapshot {
        level_db: f32::from_bits(meter.level_db.load(Ordering::Relaxed)),
        voice: f32::from_bits(meter.voice.load(Ordering::Relaxed)),
        gate_open: meter.gate_open.load(Ordering::Relaxed),
        active: last != 0 && now_ms().saturating_sub(last) < 500,
    }
}
