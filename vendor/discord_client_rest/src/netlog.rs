//! Registro del tráfico de red del cliente, para la pestaña "Tráfico" de las
//! devtools (`ui::devtools::traffic`).
//!
//! Vive en esta crate (y no en la app) porque la capa REST vendorizada y la
//! app necesitan escribir en el mismo registro, y la app ya depende de esta
//! crate. Solo usa `std`.
//!
//! # Qué guarda
//!
//! - **Conexiones** ([`Conn`]): cada WebSocket (Gateway, voz, streams, login por
//!   QR) y cada flujo UDP, con su tipo ([`Kind`]), destino, quién lo abrió,
//!   contadores de entrada/salida y estado.
//! - **Filas** ([`Row`]): una por frame de WebSocket (con su dirección,
//!   [`Dir`]) o por pedido HTTP (REST, CDN u otro) con método, URL, la API y
//!   la ruta "normalizada" (`/channels/{id}/messages`), headers, cuerpos,
//!   estado y duración.
//!
//! # Quién lo pidió
//!
//! Hay dos niveles, ambos en [`Row::who`] / [`Http::via`]:
//!
//! - **Qué función de la app lo disparó**: los hilos de fondo que lanzan los
//!   pedidos (`spawn_*`) declaran su nombre con [`origin`]; ese nombre queda
//!   en el hilo y lo lee [`current_origin`] al registrar el pedido. Los
//!   WebSockets reciben su nombre al abrirse ([`conn_open`]).
//! - **Por qué capa salió**: `via` ("vendor::RestClient", "http_cache"...).
//!
//! # Seguridad
//!
//! Se oculta, antes de guardar nada: `Authorization`, `Cookie`, `Set-Cookie`,
//! claves de captcha, y en los JSON los valores de `token`, `password`,
//! `ticket` y similares (por ejemplo, el `IDENTIFY` del Gateway lleva el token
//! de la cuenta). Ver [`redact_json`], [`redact_header`] y [`redact_url`].
//!
//! # Costo
//!
//! Con el registro apagado ([`set_enabled`]`(false)`, el valor por omisión)
//! cada llamada es una lectura atómica y nada más. La memoria retenida está
//! acotada ([`MAX_ROWS`] filas y [`MAX_HELD_BYTES`] de texto).

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Filas (frames + pedidos) que se retienen; al pasarse se descartan las más viejas.
pub const MAX_ROWS: usize = 5000;
/// Tope de texto retenido entre todas las filas.
pub const MAX_HELD_BYTES: usize = 32 * 1024 * 1024;
/// Tope del contenido que se guarda de un frame de WebSocket.
const MAX_FRAME_PAYLOAD: usize = 16 * 1024;
/// Tope del cuerpo que se guarda de un pedido.
const MAX_REQ_BODY: usize = 32 * 1024;
/// Tope del cuerpo que se guarda de una respuesta.
const MAX_RESP_BODY: usize = 64 * 1024;
/// Conexiones que se recuerdan (se descartan primero las ya cerradas).
const MAX_CONNS: usize = 300;

// ---------------------------------------------------------------------------
// Tipos
// ---------------------------------------------------------------------------

/// Qué clase de conexión es.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// WebSocket del Gateway de Discord (eventos de la cuenta).
    Gateway,
    /// WebSocket de señalización de una llamada de voz.
    Voice,
    /// WebSocket de voz del stream (pantalla/cámara) que se está mirando.
    StreamWatch,
    /// WebSocket de voz del stream que se está emitiendo.
    StreamPublish,
    /// WebSocket del login por código QR (remote auth).
    RemoteAuth,
    /// Cualquier otro WebSocket.
    WebSocket,
    /// Pedido a la API REST de Discord.
    Rest,
    /// Descarga del CDN de Discord (avatares, adjuntos, emojis...).
    Cdn,
    /// Cualquier otro pedido HTTP(S).
    Http,
    /// Flujo UDP de audio/video de una llamada.
    Udp,
}

impl Kind {
    pub const ALL: [Kind; 10] = [
        Kind::Gateway,
        Kind::Voice,
        Kind::StreamWatch,
        Kind::StreamPublish,
        Kind::RemoteAuth,
        Kind::WebSocket,
        Kind::Rest,
        Kind::Cdn,
        Kind::Http,
        Kind::Udp,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }

    /// Nombre corto para la tabla y los filtros.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Gateway => "Gateway",
            Kind::Voice => "Voz",
            Kind::StreamWatch => "Stream (ver)",
            Kind::StreamPublish => "Stream (emitir)",
            Kind::RemoteAuth => "Login QR",
            Kind::WebSocket => "WebSocket",
            Kind::Rest => "REST",
            Kind::Cdn => "CDN",
            Kind::Http => "HTTP",
            Kind::Udp => "UDP",
        }
    }

    /// Explicación de una línea, para el inspector.
    pub fn describe(self) -> &'static str {
        match self {
            Kind::Gateway => "WebSocket del Gateway: eventos en vivo de la cuenta (mensajes, presencia, voz...)",
            Kind::Voice => "WebSocket de señalización de voz: negocia el audio de una llamada",
            Kind::StreamWatch => "WebSocket de voz para mirar el stream de otra persona",
            Kind::StreamPublish => "WebSocket de voz para emitir el stream propio",
            Kind::RemoteAuth => "WebSocket del login por código QR",
            Kind::WebSocket => "WebSocket sin clasificar",
            Kind::Rest => "Pedido a la API REST de Discord",
            Kind::Cdn => "Descarga de archivos del CDN de Discord",
            Kind::Http => "Pedido HTTP(S) a otro servicio",
            Kind::Udp => "Audio/video por UDP (RTP); solo se cuentan los bytes",
        }
    }

    /// ¿Es una conexión persistente (WebSocket/UDP) y no un pedido suelto?
    pub fn is_socket(self) -> bool {
        !matches!(self, Kind::Rest | Kind::Cdn | Kind::Http)
    }
}

/// Sentido de un mensaje, visto desde el cliente.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Llega al cliente.
    In,
    /// Sale del cliente.
    Out,
}

/// Clase de un frame de WebSocket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameType {
    Text,
    Binary,
    /// Texto ya descomprimido de un Gateway `zlib-stream` (en el cable viaja
    /// como binario comprimido; ver [`conn_open`]).
    Decoded,
    Ping,
    Pong,
    Close,
    /// Evento de la conexión (se abrió, falló, se cerró), no un mensaje.
    Info,
}

impl FrameType {
    pub fn label(self) -> &'static str {
        match self {
            FrameType::Text => "texto",
            FrameType::Binary => "binario",
            FrameType::Decoded => "texto (zlib)",
            FrameType::Ping => "ping",
            FrameType::Pong => "pong",
            FrameType::Close => "cierre",
            FrameType::Info => "evento",
        }
    }
}

/// Estado de una conexión persistente.
#[derive(Clone, Debug)]
pub struct Conn {
    pub id: u64,
    pub kind: Kind,
    /// URL sin secretos.
    pub url: String,
    pub host: String,
    /// Quién la abrió.
    pub who: String,
    pub opened_ms: i64,
    pub closed_ms: Option<i64>,
    pub close_info: Option<String>,
    pub frames_in: u64,
    pub frames_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// El tráfico binario es `zlib-stream` y se registra ya descomprimido.
    pub compressed: bool,
}

/// Un frame de WebSocket.
#[derive(Clone, Debug)]
pub struct Frame {
    pub dir: Dir,
    pub ftype: FrameType,
    pub payload: String,
    pub truncated: bool,
}

/// Un pedido HTTP, en curso o terminado.
#[derive(Clone, Debug, Default)]
pub struct Http {
    pub method: String,
    pub url: String,
    /// Nombre legible de la API ("Discord REST API v10", "Discord CDN"...).
    pub api: String,
    /// Ruta con los ids reemplazados (`/channels/{id}/messages`).
    pub route: String,
    /// Por qué capa del cliente salió.
    pub via: String,
    pub status: Option<u16>,
    pub version: String,
    pub error: Option<String>,
    pub elapsed_ms: Option<u64>,
    pub req_headers: Vec<(String, String)>,
    pub req_body: Option<String>,
    pub req_len: usize,
    pub resp_headers: Vec<(String, String)>,
    pub resp_body: Option<String>,
    pub resp_len: Option<usize>,
    pub resp_type: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub enum Detail {
    Frame(Frame),
    Http(Http),
}

/// Una línea del registro.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: u64,
    pub unix_ms: i64,
    pub kind: Kind,
    /// Conexión a la que pertenece (0 en los pedidos HTTP).
    pub conn: u64,
    /// Destino legible: host, o API + ruta.
    pub target: String,
    /// Quién lo pidió o abrió.
    pub who: String,
    pub summary: String,
    /// Tamaño del mensaje o de la respuesta, en bytes, si se conoce.
    pub size: Option<usize>,
    pub detail: Detail,
    /// Texto en minúsculas para buscar.
    pub search: String,
}

impl Row {
    pub fn dir(&self) -> Option<Dir> {
        match &self.detail {
            Detail::Frame(frame) if frame.ftype != FrameType::Info => Some(frame.dir),
            _ => None,
        }
    }

    /// Pedido con error de red o con estado 4xx/5xx, o conexión que falló.
    pub fn is_error(&self) -> bool {
        match &self.detail {
            Detail::Http(http) => http.error.is_some() || http.status.is_some_and(|s| s >= 400),
            Detail::Frame(frame) => frame.ftype == FrameType::Info && self.summary.starts_with("Error"),
        }
    }

    /// Pedido todavía sin respuesta.
    pub fn is_pending(&self) -> bool {
        matches!(&self.detail, Detail::Http(http) if http.status.is_none() && http.error.is_none())
    }

    fn weight(&self) -> usize {
        let base = 192 + self.target.len() + self.who.len() + self.summary.len() + self.search.len();
        base + match &self.detail {
            Detail::Frame(frame) => frame.payload.len(),
            Detail::Http(http) => {
                http.url.len()
                    + http.route.len()
                    + http.req_body.as_ref().map_or(0, String::len)
                    + http.resp_body.as_ref().map_or(0, String::len)
                    + http
                        .req_headers
                        .iter()
                        .chain(http.resp_headers.iter())
                        .map(|(k, v)| k.len() + v.len())
                        .sum::<usize>()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Almacén global
// ---------------------------------------------------------------------------

pub struct Store {
    rows: VecDeque<Row>,
    conns: Vec<Conn>,
    next_id: u64,
    next_conn: u64,
    held: usize,
    epoch: u64,
    version: u64,
}

static STORE: Mutex<Store> = Mutex::new(Store {
    rows: VecDeque::new(),
    conns: Vec::new(),
    next_id: 1,
    next_conn: 1,
    held: 0,
    epoch: 0,
    version: 0,
});

static ENABLED: AtomicBool = AtomicBool::new(false);

fn lock() -> MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Enciende o apaga el registro. Lo decide la app al arrancar según si las
/// devtools están habilitadas; apagado, no se guarda nada.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Lee el almacén. No dibujes la UI con el lock tomado: copiá lo que
/// necesites y soltalo (mismo criterio que `logging::capture`).
pub fn with<R>(f: impl FnOnce(&Store) -> R) -> R {
    f(&lock())
}

/// Vacía las filas y las conexiones ya cerradas (las abiertas siguen y
/// conservan sus contadores).
pub fn clear() {
    let mut store = lock();
    store.rows.clear();
    store.held = 0;
    store.conns.retain(|conn| conn.closed_ms.is_none());
    store.epoch += 1;
    store.version += 1;
}

impl Store {
    pub fn rows(&self) -> &VecDeque<Row> {
        &self.rows
    }

    pub fn conns(&self) -> &[Conn] {
        &self.conns
    }

    pub fn conn(&self, id: u64) -> Option<&Conn> {
        self.conns.iter().rev().find(|conn| conn.id == id)
    }

    /// Cambia con cada escritura (para saber si hay que rehacer un filtro).
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Cambia cuando se vacía el registro.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn held_bytes(&self) -> usize {
        self.held
    }

    pub fn get(&self, id: u64) -> Option<&Row> {
        let first = self.rows.front()?.id;
        if id < first {
            return None;
        }
        self.rows.get((id - first) as usize).filter(|row| row.id == id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Row> {
        let first = self.rows.front()?.id;
        if id < first {
            return None;
        }
        self.rows.get_mut((id - first) as usize).filter(|row| row.id == id)
    }

    fn conn_mut(&mut self, id: u64) -> Option<&mut Conn> {
        self.conns.iter_mut().rev().find(|conn| conn.id == id)
    }

    fn push(&mut self, mut row: Row) -> u64 {
        row.id = self.next_id;
        self.next_id += 1;
        let id = row.id;
        self.held += row.weight();
        self.rows.push_back(row);
        self.version += 1;
        while self.rows.len() > MAX_ROWS || self.held > MAX_HELD_BYTES {
            match self.rows.pop_front() {
                Some(old) => self.held = self.held.saturating_sub(old.weight()),
                None => break,
            }
        }
        id
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Quién lo pidió (hilo actual)
// ---------------------------------------------------------------------------

thread_local! {
    static ORIGIN: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// Devuelve la marca anterior al soltarse.
#[must_use = "la marca vuelve a la anterior apenas se destruye el guard"]
pub struct OriginGuard(Option<&'static str>);

/// Declara, para este hilo, qué función de la app está pidiendo cosas a la red
/// (`let _who = netlog::origin("fetch_channel_messages");`). Todo pedido que
/// salga desde este hilo mientras viva el guard queda atribuido a ese nombre.
///
/// Sirve para los hilos de fondo con un runtime `current_thread` (el patrón de
/// los `spawn_*` de la app), donde toda la tarea corre en el mismo hilo. En un
/// runtime multihilo no es confiable y hay que pasar el nombre a mano.
pub fn origin(name: &'static str) -> OriginGuard {
    OriginGuard(ORIGIN.with(|cell| cell.replace(Some(name))))
}

impl Drop for OriginGuard {
    fn drop(&mut self) {
        ORIGIN.with(|cell| cell.set(self.0));
    }
}

/// Nombre declarado con [`origin`] y, si no hay, el del hilo.
pub fn current_origin() -> String {
    if let Some(name) = ORIGIN.with(Cell::get) {
        return name.to_owned();
    }
    let thread = std::thread::current();
    match thread.name() {
        Some(name) => format!("hilo «{name}»"),
        None => "hilo sin nombre".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Conexiones (WebSocket / UDP)
// ---------------------------------------------------------------------------

/// Registra una conexión nueva y devuelve su id (`0` si el registro está
/// apagado: todas las demás funciones ignoran el id 0).
///
/// Si la URL pide `compress=zlib-stream` (el Gateway), los frames binarios de
/// entrada son bytes comprimidos sin sentido para una persona: solo se cuentan,
/// y el contenido se registra con [`frame_decoded`] una vez descomprimido.
pub fn conn_open(kind: Kind, url: &str, who: &str) -> u64 {
    if !enabled() {
        return 0;
    }
    let url = redact_url(url);
    let (_, host, _) = split_url(&url);
    let host = host.to_owned();
    let compressed = url.contains("compress=zlib-stream");
    let now = now_ms();
    let mut store = lock();
    let id = store.next_conn;
    store.next_conn += 1;
    store.conns.push(Conn {
        id,
        kind,
        url: url.clone(),
        host: host.clone(),
        who: who.to_owned(),
        opened_ms: now,
        closed_ms: None,
        close_info: None,
        frames_in: 0,
        frames_out: 0,
        bytes_in: 0,
        bytes_out: 0,
        compressed,
    });
    if store.conns.len() > MAX_CONNS {
        if let Some(pos) = store.conns.iter().position(|conn| conn.closed_ms.is_some()) {
            store.conns.remove(pos);
        } else {
            store.conns.remove(0);
        }
    }
    let summary = format!("Conexión abierta → {url}");
    let row = info_row(kind, id, &host, who, summary.clone(), summary);
    store.push(row);
    id
}

/// Cierra la conexión (`conn_close`) cuando se suelta; para tareas con varios `return`.
pub struct ConnGuard(pub u64);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        conn_close(self.0, "flujo terminado");
    }
}

/// Marca una conexión como cerrada.
pub fn conn_close(id: u64, info: &str) {
    if id == 0 || !enabled() {
        return;
    }
    let now = now_ms();
    let mut store = lock();
    let Some(conn) = store.conn_mut(id) else {
        return;
    };
    if conn.closed_ms.is_some() {
        return;
    }
    conn.closed_ms = Some(now);
    conn.close_info = Some(info.to_owned());
    let (kind, host, who) = (conn.kind, conn.host.clone(), conn.who.clone());
    let summary = format!("Conexión cerrada · {info}");
    let row = info_row(kind, id, &host, &who, summary.clone(), summary);
    store.push(row);
}

/// Un evento de la conexión que no es un mensaje ("handshake falló", etc.).
/// Si empieza con `Error` se marca como error en la tabla.
pub fn conn_note(id: u64, text: &str) {
    if id == 0 || !enabled() {
        return;
    }
    let mut store = lock();
    let Some(conn) = store.conn_mut(id) else {
        return;
    };
    let (kind, host, who) = (conn.kind, conn.host.clone(), conn.who.clone());
    let row = info_row(kind, id, &host, &who, text.to_owned(), text.to_owned());
    store.push(row);
}

/// Registra un intento de conexión que falló antes de abrirse (no hay id).
pub fn conn_failed(kind: Kind, url: &str, who: &str, error: &str) {
    if !enabled() {
        return;
    }
    let url = redact_url(url);
    let (_, host, _) = split_url(&url);
    let host = host.to_owned();
    let summary = format!("Error al conectar con {url}: {error}");
    let row = info_row(kind, 0, &host, who, summary, format!("{url}\n\n{error}"));
    lock().push(row);
}

fn info_row(kind: Kind, conn: u64, host: &str, who: &str, summary: String, payload: String) -> Row {
    let search = format!("{} {} {} {}", kind.label(), host, who, summary).to_lowercase();
    Row {
        id: 0,
        unix_ms: now_ms(),
        kind,
        conn,
        target: host.to_owned(),
        who: who.to_owned(),
        summary,
        size: None,
        detail: Detail::Frame(Frame {
            dir: Dir::Out,
            ftype: FrameType::Info,
            payload,
            truncated: false,
        }),
        search,
    }
}

/// Cuenta bytes de una conexión sin crear filas (UDP: son miles de paquetes).
pub fn conn_add_bytes(id: u64, dir: Dir, bytes: usize) {
    if id == 0 || !enabled() {
        return;
    }
    let mut store = lock();
    if let Some(conn) = store.conn_mut(id) {
        match dir {
            Dir::In => {
                conn.frames_in += 1;
                conn.bytes_in += bytes as u64;
            }
            Dir::Out => {
                conn.frames_out += 1;
                conn.bytes_out += bytes as u64;
            }
        }
        store.version += 1;
    }
}

/// Un mensaje de texto de WebSocket.
pub fn frame_text(id: u64, dir: Dir, text: &str) {
    push_frame(id, dir, FrameType::Text, text.len(), Some(text), None, "");
}

/// Un mensaje binario de WebSocket.
pub fn frame_binary(id: u64, dir: Dir, bytes: &[u8]) {
    push_frame(id, dir, FrameType::Binary, bytes.len(), None, Some(bytes), "");
}

/// El texto de un frame de Gateway `zlib-stream` ya descomprimido (entrada).
pub fn frame_decoded(id: u64, text: &str) {
    push_frame(id, Dir::In, FrameType::Decoded, text.len(), Some(text), None, "");
}

/// Ping, pong o cierre. `detail` es lo que se muestra (p. ej. el código de cierre).
pub fn frame_control(id: u64, dir: Dir, ftype: FrameType, size: usize, detail: &str) {
    push_frame(id, dir, ftype, size, None, None, detail);
}

fn push_frame(
    id: u64,
    dir: Dir,
    ftype: FrameType,
    size: usize,
    text: Option<&str>,
    bytes: Option<&[u8]>,
    detail: &str,
) {
    if id == 0 || !enabled() {
        return;
    }
    let mut store = lock();
    let Some(conn) = store.conn_mut(id) else {
        return;
    };
    match dir {
        Dir::In => {
            conn.frames_in += 1;
            conn.bytes_in += size as u64;
        }
        Dir::Out => {
            conn.frames_out += 1;
            conn.bytes_out += size as u64;
        }
    }
    let (kind, host, who, compressed) = (conn.kind, conn.host.clone(), conn.who.clone(), conn.compressed);
    // Gateway comprimido: el binario de entrada solo se cuenta.
    if compressed && ftype == FrameType::Binary && dir == Dir::In {
        store.version += 1;
        return;
    }
    let (payload, truncated, summary) = match (text, bytes) {
        (Some(text), _) => {
            let summary = summarize(kind, text, size);
            let (clean, truncated) = truncate_utf8(text, MAX_FRAME_PAYLOAD);
            (redact_json(&clean), truncated, summary)
        }
        (None, Some(bytes)) => {
            let shown = &bytes[..bytes.len().min(512)];
            let summary = format!("binario · {}", human_size(size));
            (hex_dump(shown), bytes.len() > shown.len(), summary)
        }
        (None, None) => {
            let summary = if detail.is_empty() {
                ftype.label().to_owned()
            } else {
                format!("{} · {detail}", ftype.label())
            };
            (detail.to_owned(), false, summary)
        }
    };
    let search = format!("{} {} {} {} {}", kind.label(), host, who, summary, ftype.label()).to_lowercase();
    store.push(Row {
        id: 0,
        unix_ms: now_ms(),
        kind,
        conn: id,
        target: host,
        who,
        summary,
        size: Some(size),
        detail: Detail::Frame(Frame { dir, ftype, payload, truncated }),
        search,
    });
}

// ---------------------------------------------------------------------------
// Pedidos HTTP
// ---------------------------------------------------------------------------

/// Un pedido en curso. Con el registro apagado `id` es 0 y todo se ignora.
pub struct HttpTrace {
    id: u64,
    started: Instant,
}

impl HttpTrace {
    /// Un trace que no registra nada.
    pub fn none() -> Self {
        Self { id: 0, started: Instant::now() }
    }

    pub fn is_active(&self) -> bool {
        self.id != 0
    }

    /// Cierra el pedido con su respuesta. `body` es lo que se tiene del
    /// cuerpo (puede ser solo el comienzo) y `total_len` su tamaño real; si el
    /// cuerpo no es texto (imágenes, audio...) no se guarda, solo el tamaño.
    pub fn complete(
        &self,
        status: u16,
        version: &str,
        headers: Vec<(String, String)>,
        body: &[u8],
        total_len: usize,
    ) {
        if self.id == 0 {
            return;
        }
        let elapsed = self.started.elapsed().as_millis() as u64;
        let content_type = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.clone());
        let textual = is_textual(content_type.as_deref(), body);
        let (shown, truncated) = if textual && !body.is_empty() {
            let text = String::from_utf8_lossy(body);
            let (clipped, cut) = truncate_utf8(&text, MAX_RESP_BODY);
            (Some(redact_json(&clipped)), cut || total_len > body.len())
        } else {
            (None, false)
        };
        let headers = redact_headers(headers);
        let mut store = lock();
        let Some(row) = store.get_mut(self.id) else {
            return;
        };
        let before = row.weight();
        if let Detail::Http(http) = &mut row.detail {
            http.status = Some(status);
            http.version = version.to_owned();
            http.elapsed_ms = Some(elapsed);
            http.resp_headers = headers;
            http.resp_body = shown;
            http.resp_len = Some(total_len);
            http.resp_type = content_type;
            http.truncated = truncated;
            row.size = Some(total_len);
            let route = http.route.clone();
            row.summary = format!("{} {} → {}", http.method, route, status);
            row.search = format!("{} {} {}", row.search, status, http.version).to_lowercase();
        }
        let after = row.weight();
        store.held = (store.held + after).saturating_sub(before);
        store.version += 1;
    }

    /// Cierra el pedido con un error de red (sin respuesta).
    pub fn fail(&self, error: &str) {
        if self.id == 0 {
            return;
        }
        let elapsed = self.started.elapsed().as_millis() as u64;
        let mut store = lock();
        let Some(row) = store.get_mut(self.id) else {
            return;
        };
        let before = row.weight();
        if let Detail::Http(http) = &mut row.detail {
            http.error = Some(error.to_owned());
            http.elapsed_ms = Some(elapsed);
            row.summary = format!("{} {} → error: {}", http.method, http.route, first_line(error));
        }
        let after = row.weight();
        store.held = (store.held + after).saturating_sub(before);
        store.version += 1;
    }
}

/// Registra un pedido que sale. `who` es quien lo pide (si es `None` se usa
/// [`current_origin`]) y `via` la capa del cliente por la que sale.
pub fn http_begin(
    method: &str,
    url: &str,
    headers: Vec<(String, String)>,
    body: Option<&[u8]>,
    who: Option<&str>,
    via: &str,
) -> HttpTrace {
    if !enabled() {
        return HttpTrace::none();
    }
    let who = who.map(str::to_owned).unwrap_or_else(current_origin);
    let classified = classify(url);
    let clean_url = redact_url(url);
    let (req_body, req_len, cut) = match body {
        Some(bytes) if !bytes.is_empty() => {
            if is_textual(None, bytes) {
                let text = String::from_utf8_lossy(bytes);
                let (clipped, cut) = truncate_utf8(&text, MAX_REQ_BODY);
                (Some(redact_json(&clipped)), bytes.len(), cut)
            } else {
                (None, bytes.len(), false)
            }
        }
        _ => (None, 0, false),
    };
    let http = Http {
        method: method.to_owned(),
        url: clean_url,
        api: classified.api.clone(),
        route: classified.route.clone(),
        via: via.to_owned(),
        req_headers: redact_headers(headers),
        req_body,
        req_len,
        truncated: cut,
        ..Http::default()
    };
    let summary = format!("{method} {} …", classified.route);
    let search = format!(
        "{} {} {} {} {} {}",
        classified.kind.label(),
        classified.host,
        classified.api,
        who,
        via,
        summary
    )
    .to_lowercase();
    let row = Row {
        id: 0,
        unix_ms: now_ms(),
        kind: classified.kind,
        conn: 0,
        target: format!("{} · {}", classified.api, classified.host),
        who,
        summary,
        size: None,
        detail: Detail::Http(http),
        search,
    };
    let id = lock().push(row);
    HttpTrace { id, started: Instant::now() }
}

/// Convierte pares (nombre, valor en bytes) de la librería HTTP que sea en
/// texto, listos para [`http_begin`] / [`HttpTrace::complete`].
pub fn header_pairs<'a, I>(iter: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (&'a str, &'a [u8])>,
{
    iter.into_iter()
        .map(|(name, value)| (name.to_owned(), String::from_utf8_lossy(value).into_owned()))
        .collect()
}

// ---------------------------------------------------------------------------
// Clasificación
// ---------------------------------------------------------------------------

/// Qué es una URL: tipo, host, API y ruta normalizada.
pub struct Classified {
    pub kind: Kind,
    pub host: String,
    pub api: String,
    pub route: String,
}

/// Separa `esquema`, `host` (sin usuario ni puerto) y `ruta+query`.
fn split_url(url: &str) -> (&str, &str, &str) {
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => ("", url),
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let path = &rest[end..];
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = host_port.split(':').next().unwrap_or(host_port);
    (scheme, host, path)
}

/// Reemplaza por `{id}`, `{emoji}`... los segmentos que cambian de pedido a
/// pedido, para poder agrupar (`/channels/{id}/messages`).
fn normalize_segments(segs: &[&str]) -> String {
    let mut out = String::new();
    let mut prev = "";
    for &seg in segs {
        let norm: String = if prev == "reactions" {
            "{emoji}".to_owned()
        } else if prev == "invites" {
            "{code}".to_owned()
        } else if seg.len() >= 15 && seg.bytes().all(|b| b.is_ascii_digit()) {
            "{id}".to_owned()
        } else if seg.len() >= 24 && seg.bytes().all(|b| b.is_ascii_hexdigit()) {
            "{hash}".to_owned()
        } else {
            seg.to_owned()
        };
        out.push('/');
        out.push_str(&norm);
        prev = seg;
    }
    if out.is_empty() {
        "/".to_owned()
    } else {
        out
    }
}

/// Tipo, API y ruta de una URL de pedido HTTP.
pub fn classify(url: &str) -> Classified {
    let (_, host, path) = split_url(url);
    let host = host.to_ascii_lowercase();
    let h = host.as_str();
    let path_only = path.split(['?', '#']).next().unwrap_or("");
    let segs: Vec<&str> = path_only.split('/').filter(|s| !s.is_empty()).collect();

    let is_discord = h == "discord.com"
        || h.ends_with(".discord.com")
        || h == "discordapp.com"
        || h.ends_with(".discordapp.com");

    if is_discord && segs.first() == Some(&"api") {
        let mut start = 1;
        let mut version = String::new();
        if let Some(v) = segs.get(1) {
            if v.len() > 1 && v.starts_with('v') && v[1..].bytes().all(|b| b.is_ascii_digit()) {
                version = format!(" {v}");
                start = 2;
            }
        }
        return Classified {
            kind: Kind::Rest,
            host,
            api: format!("Discord REST API{version}"),
            route: normalize_segments(&segs[start.min(segs.len())..]),
        };
    }
    if h.ends_with("discordapp.com") || h.ends_with("discordapp.net") || h == "cdn.discord.com" {
        return Classified {
            kind: Kind::Cdn,
            host,
            api: "Discord CDN".to_owned(),
            route: normalize_segments(&segs),
        };
    }
    let api = if h == "discord.com" || h.ends_with(".discord.com") {
        "Discord Web"
    } else if h == "discord.gg" || h.ends_with(".discord.gg") {
        "Discord (invitaciones)"
    } else if h.ends_with("tenor.com") {
        "Tenor (GIFs)"
    } else if h.ends_with("giphy.com") {
        "Giphy (GIFs)"
    } else if h.ends_with("hcaptcha.com") {
        "hCaptcha"
    } else if h == "github.com" || h.ends_with(".github.com") || h.ends_with("githubusercontent.com") {
        "GitHub (actualizaciones / modelos)"
    } else {
        ""
    };
    let api = if api.is_empty() { host.clone() } else { api.to_owned() };
    Classified {
        kind: Kind::Http,
        host,
        api,
        route: normalize_segments(&segs),
    }
}

// ---------------------------------------------------------------------------
// Resúmenes de frames
// ---------------------------------------------------------------------------

/// Una línea para la tabla: el opcode (y el evento, si es un dispatch) más el tamaño.
fn summarize(kind: Kind, text: &str, size: usize) -> String {
    let head = prefix(text, 320);
    let op = field_after(head, "\"op\":");
    let event = field_after(head, "\"t\":");
    let name = match (&op, &event) {
        (Some(op), Some(event)) if !event.is_empty() => format!("{} · {event}", op_name(kind, op)),
        (Some(op), _) => op_name(kind, op),
        (None, _) => {
            let first = first_line(text);
            return format!("{} · {}", truncate_chars(&first, 60), human_size(size));
        }
    };
    format!("{name} · {}", human_size(size))
}

fn op_name(kind: Kind, op: &str) -> String {
    let Ok(n) = op.parse::<u32>() else {
        // Login por QR: el opcode es un texto ("hello", "nonce_proof"...).
        return op.to_owned();
    };
    let name = match kind {
        Kind::Gateway => match n {
            0 => "DISPATCH",
            1 => "HEARTBEAT",
            2 => "IDENTIFY",
            3 => "PRESENCE_UPDATE",
            4 => "VOICE_STATE_UPDATE",
            6 => "RESUME",
            7 => "RECONNECT",
            8 => "REQUEST_GUILD_MEMBERS",
            9 => "INVALID_SESSION",
            10 => "HELLO",
            11 => "HEARTBEAT_ACK",
            14 => "GUILD_SUBSCRIPTIONS",
            37 => "GUILD_SUBSCRIPTIONS_BULK",
            _ => "",
        },
        Kind::Voice | Kind::StreamWatch | Kind::StreamPublish => match n {
            0 => "IDENTIFY",
            1 => "SELECT_PROTOCOL",
            2 => "READY",
            3 => "HEARTBEAT",
            4 => "SESSION_DESCRIPTION",
            5 => "SPEAKING",
            6 => "HEARTBEAT_ACK",
            7 => "RESUME",
            8 => "HELLO",
            9 => "RESUMED",
            11 => "CLIENTS_CONNECT",
            12 => "VIDEO",
            13 => "CLIENT_DISCONNECT",
            21 => "DAVE_PREPARE_TRANSITION",
            22 => "DAVE_EXECUTE_TRANSITION",
            23 => "DAVE_TRANSITION_READY",
            24 => "DAVE_PREPARE_EPOCH",
            25 => "MLS_EXTERNAL_SENDER",
            26 => "MLS_KEY_PACKAGE",
            27 => "MLS_PROPOSALS",
            28 => "MLS_COMMIT_WELCOME",
            29 => "MLS_ANNOUNCE_COMMIT_TRANSITION",
            30 => "MLS_WELCOME",
            31 => "MLS_INVALID_COMMIT_WELCOME",
            _ => "",
        },
        _ => "",
    };
    if name.is_empty() {
        format!("op {n}")
    } else {
        format!("{name} (op {n})")
    }
}

/// Primer `limit` de bytes de `text` sin cortar un carácter.
fn prefix(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// El valor que sigue a `key` (un texto entre comillas o un número).
fn field_after(head: &str, key: &str) -> Option<String> {
    let start = head.find(key)? + key.len();
    let rest = head[start..].trim_start();
    if let Some(quoted) = rest.strip_prefix('"') {
        let end = quoted.find('"')?;
        return Some(quoted[..end].to_owned());
    }
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '-'))
        .unwrap_or(rest.len());
    if end == 0 { None } else { Some(rest[..end].to_owned()) }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_owned()
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let cut: String = text.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// `1.2 KB`, `3.4 MB`...
pub fn human_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / (KB * KB))
    }
}

// ---------------------------------------------------------------------------
// Contenido: truncar, volcar en hex y decidir si es texto
// ---------------------------------------------------------------------------

/// Recorta a `max` bytes sin partir un carácter. Devuelve si recortó.
fn truncate_utf8(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_owned(), false);
    }
    (prefix(text, max).to_owned(), true)
}

/// Volcado hexadecimal clásico (16 bytes por línea, con columna ASCII).
fn hex_dump(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (line, chunk) in bytes.chunks(16).enumerate() {
        out.push_str(&format!("{:04x}  ", line * 16));
        for i in 0..16 {
            match chunk.get(i) {
                Some(b) => out.push_str(&format!("{b:02x} ")),
                None => out.push_str("   "),
            }
        }
        out.push(' ');
        for b in chunk {
            out.push(if (0x20..0x7f).contains(b) { *b as char } else { '.' });
        }
        out.push('\n');
    }
    out
}

/// ¿Vale la pena guardar este cuerpo como texto?
fn is_textual(content_type: Option<&str>, body: &[u8]) -> bool {
    if let Some(content_type) = content_type {
        let t = content_type.to_ascii_lowercase();
        if t.contains("json")
            || t.starts_with("text/")
            || t.contains("xml")
            || t.contains("javascript")
            || t.contains("x-www-form-urlencoded")
        {
            return true;
        }
        if t.starts_with("image/") || t.starts_with("audio/") || t.starts_with("video/") || t.contains("octet-stream")
        {
            return false;
        }
    }
    // Sin tipo (o desconocido): texto si el comienzo es UTF-8 válido y sin controles raros.
    let head = &body[..body.len().min(512)];
    let text = match std::str::from_utf8(head) {
        Ok(text) => text,
        // Un carácter partido justo en el límite de los 512 bytes no es binario.
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&head[..error.valid_up_to()]).unwrap_or("")
        }
        Err(_) => return false,
    };
    !text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}

// ---------------------------------------------------------------------------
// Secretos
// ---------------------------------------------------------------------------

/// Claves JSON / parámetros de URL cuyo valor no se guarda.
const SECRET_KEYS: &[&str] = &[
    "token",
    "password",
    "new_password",
    "authorization",
    "captcha_key",
    "captcha_rqtoken",
    "mfa_token",
    "ticket",
    "auth_token",
    "analytics_token",
    "access_token",
    "refresh_token",
    "login_token",
    "encrypted_token",
    "secret",
];

const SECRET_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-captcha-key",
    "x-captcha-rqtoken",
];

fn is_secret_key(key: &str) -> bool {
    SECRET_KEYS.iter().any(|secret| key.eq_ignore_ascii_case(secret))
}

/// `«oculto · 72 caracteres»` en lugar de un valor sensible.
fn hidden(value: &str) -> String {
    format!("«oculto · {} caracteres»", value.chars().count())
}

/// Oculta el valor de los headers sensibles.
pub fn redact_header(name: &str, value: &str) -> String {
    if SECRET_HEADERS.iter().any(|secret| name.eq_ignore_ascii_case(secret)) {
        hidden(value)
    } else {
        value.to_owned()
    }
}

fn redact_headers(headers: Vec<(String, String)>) -> Vec<(String, String)> {
    headers
        .into_iter()
        .map(|(name, value)| {
            let value = redact_header(&name, &value);
            (name, value)
        })
        .collect()
}

/// Oculta los parámetros sensibles de la query de una URL.
pub fn redact_url(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_owned();
    };
    let (query, fragment) = match query.split_once('#') {
        Some((query, fragment)) => (query, Some(fragment)),
        None => (query, None),
    };
    let cleaned: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) if is_secret_key(key) => format!("{key}=•••"),
            _ => pair.to_owned(),
        })
        .collect();
    let mut out = format!("{base}?{}", cleaned.join("&"));
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

/// Fin (exclusivo) del texto entre comillas que empieza en `start` (donde hay
/// una `"`), o `bytes.len()` si no se cierra.
fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// Reemplaza por `"•••"` el valor de las claves sensibles de un JSON (también
/// si está cortado o es solo un fragmento). Lo que no parece JSON queda igual.
pub fn redact_json(text: &str) -> String {
    if !text.contains('"') {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let n = bytes.len();
    let is_space = |b: u8| matches!(b, b' ' | b'\n' | b'\t' | b'\r');
    let mut out = String::with_capacity(n);
    let mut i = 0;
    while i < n {
        if bytes[i] != b'"' {
            let start = i;
            while i < n && bytes[i] != b'"' {
                i += 1;
            }
            out.push_str(&text[start..i]);
            continue;
        }
        let end = string_end(bytes, i).min(n);
        let literal = &text[i..end];
        out.push_str(literal);
        let mut j = end;
        while j < n && is_space(bytes[j]) {
            j += 1;
        }
        let is_key = j < n && bytes[j] == b':';
        let inner = literal.get(1..literal.len().saturating_sub(1)).unwrap_or("");
        if is_key && is_secret_key(inner) {
            let mut k = j + 1;
            while k < n && is_space(bytes[k]) {
                k += 1;
            }
            out.push_str(&text[end..k]);
            if k < n && bytes[k] == b'"' {
                let value_end = string_end(bytes, k).min(n);
                out.push_str("\"•••\"");
                i = value_end;
            } else {
                i = k;
            }
            continue;
        }
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_rest_route() {
        let c = classify("https://discord.com/api/v10/channels/123456789012345678/messages?limit=20");
        assert_eq!(c.kind, Kind::Rest);
        assert_eq!(c.api, "Discord REST API v10");
        assert_eq!(c.route, "/channels/{id}/messages");
    }

    #[test]
    fn classify_cdn_and_other() {
        assert_eq!(classify("https://cdn.discordapp.com/avatars/123456789012345678/abc.png").kind, Kind::Cdn);
        assert_eq!(classify("https://example.org/x").kind, Kind::Http);
    }

    #[test]
    fn redacts_json_secrets() {
        let out = redact_json(r#"{"op":2,"d":{"token":"abc.def","x":1}}"#);
        assert!(!out.contains("abc.def"));
        assert!(out.contains("\"token\":\"•••\""));
        assert!(out.contains("\"x\":1"));
    }

    #[test]
    fn redacts_url_and_headers() {
        assert_eq!(redact_url("wss://h/?v=2&token=xyz"), "wss://h/?v=2&token=•••");
        assert!(redact_header("Authorization", "secret").starts_with("«oculto"));
        assert_eq!(redact_header("Accept", "*/*"), "*/*");
    }

    #[test]
    fn summarizes_gateway_dispatch() {
        let s = summarize(Kind::Gateway, r#"{"t":"MESSAGE_CREATE","s":5,"op":0,"d":{}}"#, 40);
        assert!(s.starts_with("DISPATCH (op 0) · MESSAGE_CREATE"));
    }
}
