//! Dónde te quedaste: el último canal de texto abierto en cada server y la
//! posición del scroll de cada canal (qué mensaje quedó arriba de todo y
//! cuántos píxeles de él se veían), guardados en disco para retomar justo
//! ahí después de cerrar y volver a abrir el cliente.
//!
//! * `ui::chat::show` llama a [`record_position`] cada frame con el mensaje
//!   que está arriba del todo de la vista (o `None` si el scroll está
//!   pegado al final: en ese caso no se guarda nada y el canal abre en lo
//!   más nuevo, como siempre).
//! * `App::open_channel` / `App::open_server` leen [`position`] /
//!   [`last_channel`] y, si hay posición, piden la ventana de mensajes
//!   alrededor de ese mensaje (`jump_to_message`) y arman la restauración
//!   con [`arm_restore`]; el chat la consume con [`take_restore`] y deja el
//!   scroll exactamente donde estaba (en vez de centrar el mensaje).
//!
//! Se escribe a `last_view.json` (carpeta de estado) como mucho cada
//! ~1.5 s, y al cerrar (`eframe::App::save`) con [`flush`].

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Mientras hay una restauración armada y todavía no se aplicó, no se
/// guarda la posición de ese canal (el scroll aún no está donde estaba y
/// se pisaría lo guardado). Pasado este tiempo se da por perdida (el
/// mensaje pudo haberse borrado).
const RESTORE_TIMEOUT: Duration = Duration::from_secs(8);
const WRITE_EVERY: Duration = Duration::from_millis(1500);
/// Tope de posiciones guardadas (las más viejas se descartan al pasarse).
const MAX_POSITIONS: usize = 400;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Position {
    message_id: String,
    /// Píxeles del mensaje que quedaron por encima del borde superior.
    offset: f32,
    /// Para descartar las más viejas (segundos desde el epoch).
    #[serde(default)]
    at: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    /// guild_id -> último canal de texto abierto.
    #[serde(default)]
    channels: HashMap<String, String>,
    /// channel_id -> posición del scroll.
    #[serde(default)]
    positions: HashMap<String, Position>,
}

struct Pending {
    channel_id: String,
    message_id: String,
    offset: f32,
    since: Instant,
}

struct Runtime {
    store: Store,
    dirty: bool,
    last_write: Instant,
    pending: Option<Pending>,
}

static RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();

fn runtime() -> std::sync::MutexGuard<'static, Runtime> {
    RUNTIME
        .get_or_init(|| {
            Mutex::new(Runtime {
                store: load().unwrap_or_default(),
                dirty: false,
                last_write: Instant::now(),
                pending: None,
            })
        })
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn load() -> Option<Store> {
    let path = crate::paths::last_view_file()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write(store: &Store) {
    let Some(path) = crate::paths::last_view_file() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(text) = serde_json::to_string(store) else { return };
    // Se escribe a un temporal y se renombra: un cierre a mitad de la
    // escritura no deja el archivo cortado.
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn maybe_write(rt: &mut Runtime) {
    if rt.dirty && rt.last_write.elapsed() >= WRITE_EVERY {
        write(&rt.store);
        rt.dirty = false;
        rt.last_write = Instant::now();
    }
}

/// Escribe a disco lo pendiente (se llama al cerrar la app).
pub fn flush() {
    let mut rt = runtime();
    if rt.dirty {
        write(&rt.store);
        rt.dirty = false;
        rt.last_write = Instant::now();
    }
}

/// Último canal de texto abierto en `guild_id`.
pub fn last_channel(guild_id: &str) -> Option<String> {
    runtime().store.channels.get(guild_id).cloned()
}

/// Recuerda que `channel_id` es el último canal abierto de `guild_id`.
pub fn record_channel(guild_id: &str, channel_id: &str) {
    if guild_id.is_empty() || channel_id.is_empty() {
        return;
    }
    let mut rt = runtime();
    if rt.store.channels.get(guild_id).map(String::as_str) != Some(channel_id) {
        rt.store.channels.insert(guild_id.to_string(), channel_id.to_string());
        rt.dirty = true;
        // Un cambio de canal se guarda enseguida.
        write(&rt.store);
        rt.dirty = false;
        rt.last_write = Instant::now();
    }
}

/// (mensaje, píxeles) guardados para `channel_id`.
pub fn position(channel_id: &str) -> Option<(String, f32)> {
    runtime()
        .store
        .positions
        .get(channel_id)
        .map(|p| (p.message_id.clone(), p.offset))
}

/// Guarda la posición actual de `channel_id`: `Some((mensaje, píxeles))`
/// o `None` si el scroll está al final (se borra lo guardado).
pub fn record_position(channel_id: &str, pos: Option<(String, f32)>) {
    if channel_id.is_empty() {
        return;
    }
    let mut rt = runtime();
    // Restauración en curso para este canal: no pisar lo guardado.
    let expired = rt.pending.as_ref().map(|p| p.since.elapsed() >= RESTORE_TIMEOUT);
    if expired == Some(true) {
        rt.pending = None;
    } else if rt.pending.as_ref().is_some_and(|p| p.channel_id == channel_id) {
        return;
    }
    match pos {
        Some((message_id, offset)) => {
            let offset = offset.max(0.0);
            let same = rt
                .store
                .positions
                .get(channel_id)
                .is_some_and(|p| p.message_id == message_id && (p.offset - offset).abs() < 1.0);
            if same {
                return;
            }
            rt.store.positions.insert(
                channel_id.to_string(),
                Position { message_id, offset, at: now_secs() },
            );
            if rt.store.positions.len() > MAX_POSITIONS {
                // Se descartan las más viejas.
                let mut by_age: Vec<(String, u64)> =
                    rt.store.positions.iter().map(|(k, p)| (k.clone(), p.at)).collect();
                by_age.sort_by_key(|(_, at)| *at);
                let excess = rt.store.positions.len() - MAX_POSITIONS;
                for (key, _) in by_age.into_iter().take(excess) {
                    rt.store.positions.remove(&key);
                }
            }
            rt.dirty = true;
        }
        None => {
            if rt.store.positions.remove(channel_id).is_some() {
                rt.dirty = true;
            }
        }
    }
    maybe_write(&mut rt);
}

/// Arma la restauración: cuando el chat vaya a ese mensaje, tiene que
/// dejarlo con `offset` píxeles por encima del borde (ver [`take_restore`]).
pub fn arm_restore(channel_id: &str, message_id: &str, offset: f32) {
    runtime().pending = Some(Pending {
        channel_id: channel_id.to_string(),
        message_id: message_id.to_string(),
        offset,
        since: Instant::now(),
    });
}

/// El mensaje guardado ya no existe: la restauración pasa a apuntar al más
/// cercano (sin desplazamiento).
pub fn retarget_restore(channel_id: &str, message_id: &str) {
    let mut rt = runtime();
    if let Some(p) = rt.pending.as_mut() {
        if p.channel_id == channel_id {
            p.message_id = message_id.to_string();
            p.offset = 0.0;
        }
    }
}

/// Si hay una restauración armada para ese canal y mensaje, la consume y
/// devuelve los píxeles.
pub fn take_restore(channel_id: &str, message_id: &str) -> Option<f32> {
    let mut rt = runtime();
    let matches = rt
        .pending
        .as_ref()
        .is_some_and(|p| p.channel_id == channel_id && p.message_id == message_id);
    if matches {
        rt.pending.take().map(|p| p.offset)
    } else {
        None
    }
}
