//! Captura en memoria de todo lo que pasa por la crate `log`, para la consola
//! de las herramientas de desarrollo (`ui::devtools`).
//!
//! `install` envuelve al logger de `env_logger` (el que escribe al archivo y a
//! la consola desde la que se lanzó eCord): cada línea se sigue imprimiendo
//! igual que antes y, además, se guarda en un búfer circular de
//! [`CAPACITY`] entradas que la UI lee sin tocar a los hilos que loguean.
//!
//! # Importante para quien lea el búfer
//!
//! [`with`] mantiene tomado un `Mutex` mientras corre el closure. Ese closure
//! tiene que ser corto y NO puede dibujar ni loguear (egui y wreq también usan
//! `log`: si se loguea con el lock tomado, el hilo se bloquea a sí mismo). La UI
//! copia lo que necesita (`Entry: Clone`) y suelta el lock antes de dibujar.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

/// Cuántas líneas se retienen; al llenarse se descartan las más viejas.
pub const CAPACITY: usize = 5000;
/// Tope de caracteres por mensaje (un volcado enorme no debe comerse la RAM).
const MAX_MESSAGE_CHARS: usize = 8000;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
    Trace = 4,
}

impl Level {
    pub const ALL: [Level; 5] = [Level::Error, Level::Warn, Level::Info, Level::Debug, Level::Trace];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Etiqueta corta para copiar/pegar (`ERROR`, `WARN`...).
    pub fn tag(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }

    pub fn parse(text: &str) -> Option<Level> {
        match text.trim().to_ascii_lowercase().as_str() {
            "error" | "err" | "e" => Some(Level::Error),
            "warn" | "warning" | "w" => Some(Level::Warn),
            "info" | "i" => Some(Level::Info),
            "debug" | "d" => Some(Level::Debug),
            "trace" | "t" => Some(Level::Trace),
            _ => None,
        }
    }
}

impl From<log::Level> for Level {
    fn from(level: log::Level) -> Self {
        match level {
            log::Level::Error => Level::Error,
            log::Level::Warn => Level::Warn,
            log::Level::Info => Level::Info,
            log::Level::Debug => Level::Debug,
            log::Level::Trace => Level::Trace,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// Correlativo (nunca se repite ni se reordena; sirve de clave estable).
    pub id: u64,
    /// Milisegundos Unix (UTC). La UI lo pasa a hora local solo al dibujar.
    pub unix_ms: i64,
    pub level: Level,
    pub target: String,
    pub message: String,
}

pub struct Buffer {
    entries: VecDeque<Entry>,
    next_id: u64,
    counts: [u64; 5],
    targets: BTreeMap<String, u64>,
    /// Sube con cada entrada nueva o limpieza.
    generation: u64,
    /// Sube solo al limpiar (invalida las listas filtradas ya calculadas).
    epoch: u64,
}

impl Buffer {
    const fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            next_id: 1,
            counts: [0; 5],
            targets: BTreeMap::new(),
            generation: 0,
            epoch: 0,
        }
    }

    pub fn entries(&self) -> &VecDeque<Entry> {
        &self.entries
    }

    /// Cuántas líneas hubo de cada nivel desde el arranque (o desde el último `clear`).
    pub fn counts(&self) -> [u64; 5] {
        self.counts
    }

    /// Orígenes (`target`) vistos, con su cantidad de líneas.
    pub fn targets(&self) -> &BTreeMap<String, u64> {
        &self.targets
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Id de la entrada más vieja que sigue retenida.
    pub fn first_id(&self) -> u64 {
        self.entries.front().map_or(self.next_id, |entry| entry.id)
    }

    /// Id de la última entrada (0 si todavía no hay ninguna).
    pub fn last_id(&self) -> u64 {
        self.next_id - 1
    }

    pub fn get(&self, id: u64) -> Option<&Entry> {
        let first = self.first_id();
        if id < first {
            return None;
        }
        self.entries.get((id - first) as usize).filter(|entry| entry.id == id)
    }
}

static BUFFER: Mutex<Buffer> = Mutex::new(Buffer::new());
static START: OnceLock<Instant> = OnceLock::new();

fn buffer() -> MutexGuard<'static, Buffer> {
    BUFFER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Instante en que arrancó la captura (≈ el arranque del proceso).
pub fn started_at() -> Instant {
    *START.get_or_init(Instant::now)
}

/// Lee el búfer con el lock tomado. Ver la advertencia del módulo.
pub fn with<R>(read: impl FnOnce(&Buffer) -> R) -> R {
    read(&buffer())
}

/// Cantidad de líneas por nivel (copia, sin retener el lock).
pub fn counts() -> [u64; 5] {
    buffer().counts
}

/// Agrega una línea. Lo usa el logger y también la consola de las devtools
/// para escribir su propia salida (comandos, resultados de pedidos...).
pub fn push(level: Level, target: &str, message: impl Into<String>) {
    let mut message: String = message.into();
    if message.chars().count() > MAX_MESSAGE_CHARS {
        message = message.chars().take(MAX_MESSAGE_CHARS).collect::<String>() + "… [recortado]";
    }
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64);

    let mut guard = buffer();
    let id = guard.next_id;
    guard.next_id += 1;
    guard.generation += 1;
    guard.counts[level.index()] += 1;
    match guard.targets.get_mut(target) {
        Some(count) => *count += 1,
        None => {
            // Tope defensivo por si algún origen se genera dinámicamente.
            if guard.targets.len() < 500 {
                guard.targets.insert(target.to_owned(), 1);
            }
        }
    }
    guard.entries.push_back(Entry { id, unix_ms, level, target: target.to_owned(), message });
    while guard.entries.len() > CAPACITY {
        guard.entries.pop_front();
    }
}

/// Vacía el búfer y los contadores.
pub fn clear() {
    let mut guard = buffer();
    guard.entries.clear();
    guard.counts = [0; 5];
    guard.targets.clear();
    guard.generation += 1;
    guard.epoch += 1;
}

// ---------------------------------------------------------------------------
// Logger
// ---------------------------------------------------------------------------

/// Crates de terceros muy habladoras: sus mensajes `debug`/`trace` no se
/// capturan (solo `info` o más), para que no tapen los de eCord.
const NOISY_TARGETS: [&str; 18] = [
    "hyper", "hyper_util", "rustls", "reqwest", "h2", "wreq", "tokio", "tokio_util", "tungstenite",
    "tokio_tungstenite", "btls", "boring", "cookie_store", "egui", "eframe", "winit", "wgpu", "naga",
];

fn is_noisy(target: &str) -> bool {
    NOISY_TARGETS.iter().any(|prefix| {
        target == *prefix || target.strip_prefix(prefix).is_some_and(|rest| rest.starts_with("::"))
    })
}

struct CaptureLogger {
    inner: env_logger::Logger,
    /// Con las devtools activas se captura hasta `debug` aunque `RUST_LOG`
    /// pida menos (lo que no pasa por el filtro no se imprime, solo se guarda).
    capture_debug: bool,
}

impl CaptureLogger {
    fn captures(&self, metadata: &log::Metadata) -> bool {
        if !self.capture_debug {
            return false;
        }
        match metadata.level() {
            log::Level::Error | log::Level::Warn | log::Level::Info => true,
            log::Level::Debug => !is_noisy(metadata.target()),
            log::Level::Trace => false,
        }
    }
}

impl log::Log for CaptureLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata) || self.captures(metadata)
    }

    fn log(&self, record: &log::Record) {
        let printed = self.inner.enabled(record.metadata());
        if printed {
            self.inner.log(record);
        }
        if printed || self.captures(record.metadata()) {
            push(record.level().into(), record.target(), record.args().to_string());
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Reemplaza a `env_logger::Builder::init()`: instala el logger envuelto y un
/// hook de pánicos que deja el mensaje en la consola antes de seguir con el
/// hook anterior.
pub fn install(inner: env_logger::Logger, capture_debug: bool) {
    started_at();
    let mut max = inner.filter();
    if capture_debug && max < log::LevelFilter::Debug {
        max = log::LevelFilter::Debug;
    }
    if log::set_boxed_logger(Box::new(CaptureLogger { inner, capture_debug })).is_ok() {
        log::set_max_level(max);
    }

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        push(Level::Error, "panic", info.to_string());
        previous(info);
    }));
}
