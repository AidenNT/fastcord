//! Caché en disco para todo lo que la UI baja por HTTP (avatares, emojis,
//! iconos de servers, insignias, capas de efectos/frames...), más un par de
//! helpers para guardar JSON chico (catálogos que casi no cambian).
//!
//! ## Por qué
//! El loader HTTP de `egui_extras` solo cachea en memoria: cada vez que se
//! abre el cliente se vuelve a bajar TODO. Acá se registra un `BytesLoader`
//! propio que `egui` prueba ANTES que el de `egui_extras` (los más nuevos van
//! primero) y que:
//!
//! 1. responde de memoria si ya lo tiene en esta sesión;
//! 2. si no, lo busca en disco (`<cache dir>/ecord/http/<sha256(url)>`), sin
//!    tocar la red mientras el archivo tenga menos de `DISK_TTL`;
//! 3. si tampoco está, lo baja (en un hilo aparte, con tope de descargas
//!    simultáneas) y lo guarda para la próxima vez.
//!
//! Todo el trabajo pesado (disco y red) corre fuera del hilo de la UI: mientras
//! tanto `load` devuelve `Pending`. Las URLs de Discord llevan el hash del
//! asset (`/avatars/{id}/{hash}.png`, `/emojis/{id}.webp`), así que un cambio
//! de avatar es una URL nueva y nunca se sirve uno viejo.
//!
//! Al arrancar se poda la carpeta (archivos vencidos y tope de tamaño total).

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use egui::load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError};
use sha2::{Digest, Sha256};

/// Cuánto vive un archivo en disco antes de volver a bajarlo.
const DISK_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// Tope de tamaño de toda la carpeta de imágenes.
const DISK_MAX_TOTAL: u64 = 512 * 1024 * 1024;
/// Archivos más grandes que esto no se guardan en disco.
const MAX_ITEM: usize = 32 * 1024 * 1024;
/// Bytes en RAM (además de lo que decodifica `egui`).
const MEM_BUDGET: usize = 40 * 1024 * 1024;
/// Descargas simultáneas.
const MAX_INFLIGHT: usize = 8;
/// Cuánto esperar antes de reintentar una URL que falló.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Vida en disco de los JSON chicos que no se piden seguido.
const JSON_PRUNE_TTL: Duration = Duration::from_secs(30 * 24 * 3600);

static INFLIGHT: AtomicUsize = AtomicUsize::new(0);

enum Entry {
    Pending,
    Ready { bytes: Arc<[u8]>, mime: Option<String> },
    /// `permanent` = el servidor dijo 404/410: ese archivo no existe (p. ej.
    /// Fluent no tiene ese emoji), así que no se vuelve a pedir en la sesión.
    Failed { at: Instant, msg: String, permanent: bool },
}

#[derive(Default)]
struct Inner {
    map: HashMap<String, Entry>,
    /// Orden de llegada de las entradas `Ready` (para desalojar las más viejas).
    order: VecDeque<String>,
    mem: usize,
}

pub struct DiskCachedHttpLoader {
    inner: Arc<Mutex<Inner>>,
    dir: Option<PathBuf>,
    client: reqwest::blocking::Client,
}

/// Registra el loader en `ctx`. Llamar DESPUÉS de `install_image_loaders`, para
/// que se pruebe primero.
pub fn install(ctx: &egui::Context) {
    let dir = crate::paths::http_cache_dir();
    let client = match reqwest::blocking::Client::builder()
        .user_agent("ecord")
        .timeout(Duration::from_secs(30))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            log::warn!("caché HTTP desactivada (no se pudo crear el cliente): {e}");
            return;
        }
    };
    if let Some(dir) = dir.clone() {
        std::thread::spawn(move || prune(&dir, DISK_TTL, DISK_MAX_TOTAL));
    }
    if let Some(dir) = crate::paths::json_cache_dir() {
        std::thread::spawn(move || prune(&dir, JSON_PRUNE_TTL, 64 * 1024 * 1024));
    }
    ctx.add_bytes_loader(Arc::new(DiskCachedHttpLoader {
        inner: Arc::new(Mutex::new(Inner::default())),
        dir,
        client,
    }));
}

impl BytesLoader for DiskCachedHttpLoader {
    fn id(&self) -> &str {
        egui::generate_loader_id!(DiskCachedHttpLoader)
    }

    fn load(&self, ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        if !(uri.starts_with("http://") || uri.starts_with("https://")) {
            return Err(LoadError::NotSupported);
        }
        let Ok(mut inner) = self.inner.lock() else {
            return Err(LoadError::Loading("caché bloqueada".into()));
        };
        match inner.map.get(uri) {
            Some(Entry::Ready { bytes, mime }) => {
                return Ok(BytesPoll::Ready {
                    size: None,
                    bytes: Bytes::Shared(bytes.clone()),
                    mime: mime.clone(),
                });
            }
            Some(Entry::Pending) => return Ok(BytesPoll::Pending { size: None }),
            Some(Entry::Failed { at, msg, permanent }) if *permanent || at.elapsed() < RETRY_AFTER => {
                return Err(LoadError::Loading(msg.clone()));
            }
            _ => {}
        }
        if INFLIGHT.load(Ordering::Relaxed) >= MAX_INFLIGHT {
            // Cola llena: se reintenta en el próximo repaint.
            ctx.request_repaint_after(Duration::from_millis(60));
            return Ok(BytesPoll::Pending { size: None });
        }
        inner.map.insert(uri.to_string(), Entry::Pending);
        drop(inner);

        INFLIGHT.fetch_add(1, Ordering::Relaxed);
        let (shared, dir, client) = (self.inner.clone(), self.dir.clone(), self.client.clone());
        let (ctx, url) = (ctx.clone(), uri.to_string());
        std::thread::spawn(move || {
            let result = fetch(&client, dir.as_deref(), &url);
            if let Ok(mut inner) = shared.lock() {
                match result {
                    Ok((bytes, mime)) => {
                        inner.mem += bytes.len();
                        inner.order.push_back(url.clone());
                        inner.map.insert(url.clone(), Entry::Ready { bytes, mime });
                        evict(&mut inner, &url);
                    }
                    Err((msg, permanent)) => {
                        log::debug!("no se pudo bajar {url}: {msg}");
                        inner.map.insert(url, Entry::Failed { at: Instant::now(), msg, permanent });
                    }
                }
            }
            INFLIGHT.fetch_sub(1, Ordering::Relaxed);
            ctx.request_repaint();
        });
        Ok(BytesPoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(Entry::Ready { bytes, .. }) = inner.map.remove(uri) {
                inner.mem = inner.mem.saturating_sub(bytes.len());
                inner.order.retain(|k| k != uri);
            }
        }
    }

    fn forget_all(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            *inner = Inner::default();
        }
    }

    fn byte_size(&self) -> usize {
        self.inner.lock().map(|i| i.mem).unwrap_or(0)
    }
}

/// Saca de RAM las entradas más viejas hasta entrar en `MEM_BUDGET` (siguen
/// en disco: volver a pedirlas cuesta una lectura, no una descarga).
fn evict(inner: &mut Inner, keep: &str) {
    while inner.mem > MEM_BUDGET {
        let Some(old) = inner.order.pop_front() else { break };
        if old == keep {
            inner.order.push_back(old);
            if inner.order.len() == 1 {
                break;
            }
            continue;
        }
        if let Some(Entry::Ready { bytes, .. }) = inner.map.remove(&old) {
            inner.mem = inner.mem.saturating_sub(bytes.len());
        }
    }
}

/// Disco primero, red después (y guarda el resultado).
fn fetch(
    client: &reqwest::blocking::Client,
    dir: Option<&Path>,
    url: &str,
) -> Result<(Arc<[u8]>, Option<String>), (String, bool)> {
    let path = dir.map(|d| file_for(d, url));
    if let Some((body, mime)) = path.as_deref().and_then(|p| read_disk(p, DISK_TTL)) {
        return Ok((body, mime));
    }
    let resp = client.get(url).send().map_err(|e| (e.to_string(), false))?;
    let status = resp.status();
    if !status.is_success() {
        // 404/410: no existe, no tiene sentido reintentar cada 30 s.
        let permanent = status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE;
        return Err((format!("HTTP {status}"), permanent));
    }
    let header_mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).trim().to_ascii_lowercase());
    let data = resp.bytes().map_err(|e| (e.to_string(), false))?;
    let mime = pick_mime(header_mime, &data);
    let body: Arc<[u8]> = Arc::from(&data[..]);
    if let Some(path) = path {
        if body.len() <= MAX_ITEM {
            write_disk(&path, mime.as_deref().unwrap_or(""), &body);
        }
    }
    Ok((body, mime))
}

/// El `Content-Type` manda si es de imagen; si falta o es genérico, se mira la
/// firma del archivo (las URLs de `collectibles-shop/.../static` no tienen
/// extensión, así que `egui` necesita el mime para saber cómo decodificar).
fn pick_mime(header: Option<String>, data: &[u8]) -> Option<String> {
    if let Some(h) = header.as_deref() {
        if h.starts_with("image/") {
            return header;
        }
    }
    let sniffed = if data.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF8") {
        Some("image/gif")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else if data.starts_with(b"<svg") || data.starts_with(b"<?xml") {
        Some("image/svg+xml")
    } else {
        None
    };
    sniffed.map(str::to_string).or(header)
}

fn file_for(dir: &Path, key: &str) -> PathBuf {
    let digest = Sha256::digest(key.as_bytes());
    let mut name = String::with_capacity(64);
    for b in digest.iter() {
        let _ = write!(name, "{b:02x}");
    }
    dir.join(name)
}

/// Formato: `mime\n` + bytes. `None` si no existe, venció o está corrupto.
fn read_disk(path: &Path, ttl: Duration) -> Option<(Arc<[u8]>, Option<String>)> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    if SystemTime::now().duration_since(modified).unwrap_or_default() > ttl {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    let nl = data.iter().position(|b| *b == b'\n')?;
    let mime = std::str::from_utf8(&data[..nl]).ok()?.trim().to_string();
    let body = &data[nl + 1..];
    if body.is_empty() {
        return None;
    }
    Some((Arc::from(body), (!mime.is_empty()).then_some(mime)))
}

/// Escribe a un temporal y renombra, así nunca queda un archivo a medias.
fn write_disk(path: &Path, mime: &str, body: &[u8]) {
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let mut data = Vec::with_capacity(mime.len() + 1 + body.len());
    data.extend_from_slice(mime.as_bytes());
    data.push(b'\n');
    data.extend_from_slice(body);
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, data).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Borra lo vencido y, si todavía pasa de `max_total`, lo más viejo primero.
fn prune(dir: &Path, ttl: Duration, max_total: u64) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    let mut files: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let modified = meta.modified().unwrap_or(now);
        let expired = now.duration_since(modified).unwrap_or_default() > ttl;
        if expired || path.extension().is_some_and(|e| e == "tmp") {
            let _ = std::fs::remove_file(&path);
        } else {
            files.push((path, modified, meta.len()));
        }
    }
    let mut total: u64 = files.iter().map(|f| f.2).sum();
    if total <= max_total {
        return;
    }
    files.sort_by_key(|f| f.1);
    for (path, _, len) in files {
        if total <= max_total {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

// ---------------------------------------------------------------------
// JSON chico en disco (catálogos)
// ---------------------------------------------------------------------

/// Lee un JSON guardado con `json_write` si tiene menos de `ttl`.
pub fn json_read(key: &str, ttl: Duration) -> Option<serde_json::Value> {
    let path = file_for(&crate::paths::json_cache_dir()?, key);
    let (body, _) = read_disk(&path, ttl)?;
    serde_json::from_slice(&body).ok()
}

pub fn json_write(key: &str, value: &serde_json::Value) {
    let Some(dir) = crate::paths::json_cache_dir() else { return };
    if let Ok(body) = serde_json::to_vec(value) {
        write_disk(&file_for(&dir, key), "", &body);
    }
}
