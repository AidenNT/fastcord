//! Herramientas de desarrollo de eCord: un panel flotante, con la forma de un
//! IDE (barra superior con pestañas, lista a la izquierda, contenido al centro,
//! inspector a la derecha y consola abajo), para ver y probar cómo se comporta
//! el cliente.
//!
//! # Cómo se activa (`use_developer_features`)
//!
//! El panel NO existe para un usuario normal. Solo se habilita si se cumple
//! alguna de estas (se lee una vez al arrancar, ver [`enabled`]):
//!
//! - variable de entorno `USE_DEVELOPER_FEATURES=1` (o `ECORD_USE_DEVELOPER_FEATURES=1`);
//! - argumento de línea de comandos `--use_developer_features` (o `--use-developer-features`);
//! - la opción persistida `use_developer_features` (se guarda desde la pestaña
//!   Cliente → Entorno con "Recordar activado", para no repetir lo anterior).
//!
//! Ya habilitado, se abre/cierra con **Ctrl+Shift+D** o con el botón `DEV` de
//! la esquina inferior derecha (que muestra la cantidad de errores del log).
//!
//! # Dónde se muestra (botones de la barra superior)
//!
//! - **Ventana flotante** (por omisión): se mueve arrastrando la barra superior
//!   (doble clic la vuelve a centrar) y cambia de tamaño desde cualquier borde o esquina.
//! - **Acoplada abajo**: a todo el ancho; el alto se cambia arrastrando su borde de arriba.
//! - **Ventana aparte**: una ventana del sistema operativo, independiente de la de
//!   eCord (se puede poner en otro monitor). Al cerrarla se cierran las devtools.
//!
//! # Pestañas
//!
//! - **Consola** (`console`): todo lo que eCord escribe por `log` (también los
//!   pánicos), con filtros por nivel y origen, búsqueda, inspector de cada línea
//!   y una línea de comandos (`help` para la lista). El registro vive en
//!   `logging::capture`.
//! - **Red**: HTTP, WebSocket y "Todos los profiles" contra cualquier servidor
//!   con una conexión emulada (`wreq` + `wreq-util`), más el editor de profile,
//!   plataforma y headers propios. Estas pruebas salen sin credenciales.
//! - **Discord** (`discord_api`): pedidos REST a la API de Discord con el mismo
//!   cliente que usa la app (cookies, huella TLS, headers del fingerprint) y, si
//!   se quiere, el token de la sesión. Historial, "copiar como curl" y repetir.
//! - **Tráfico** (`traffic`): todo lo que el cliente manda y recibe por la red
//!   (frames de WebSocket con su sentido, pedidos REST/CDN/HTTP), el tipo de
//!   cada conexión, la API a la que va y quién lo pidió. Lo graba
//!   `discord_client_rest::netlog`.
//! - **Cliente** (`client`): resumen del proceso y del sistema, rendimiento,
//!   memoria, seguimiento del Gateway (cuenta regresiva del opcode 41, latidos,
//!   conexión), catálogo de opcodes del Gateway y de voz (`opcodes`), identidad
//!   ante Discord (`X-Super-Properties`, `launch_signature`, IDs), transmisión
//!   (codificador, perfil de NVENC, paquete libx264), actualizaciones (estado y
//!   reinstalación forzada), entorno, y un informe de diagnóstico listo para copiar.
//!
//! Las pruebas de red corren en su propio runtime de tokio (`runtime`), aparte
//! del hilo de Discord, y avisan a la UI con `request_repaint`. Nada de esto
//! toca la sesión de Discord ni el token de la cuenta, salvo la pestaña Discord
//! cuando el usuario marca "Token de la cuenta" a propósito.

mod client;
mod console;
mod discord_api;
mod opcodes;
mod style;
mod traffic;

use std::{
    fmt::Write as _,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use egui_extras::{Size, StripBuilder};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use wreq_util::{Emulation, Platform, Profile};

use crate::logging::capture::{self, Level};
use crate::theme::{self, Palette};
use crate::discord::{
    fingerprint::ClientIdentity,
    launch_signature::{self, LaunchSignatureError, LaunchSignatureReport},
};

const STORAGE_FLAG: &str = "use_developer_features";
const STORAGE_PROFILES: &str = "devtools_custom_profiles";

/// Tope de cuerpo que se muestra de una respuesta (el resto se descarta).
const MAX_BODY_SHOWN: usize = 200 * 1024;
/// Tope de líneas del registro de un WebSocket.
const MAX_WS_LINES: usize = 2000;
/// Pruebas simultáneas de "Todos los profiles".
const MATRIX_CONCURRENCY: usize = 6;

// ---------------------------------------------------------------------------
// Activación
// ---------------------------------------------------------------------------

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

/// `true` si las herramientas de desarrollo están habilitadas (ver el módulo).
/// Se evalúa una sola vez: cambiarlo exige reiniciar.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        env_flag("USE_DEVELOPER_FEATURES")
            || env_flag("ECORD_USE_DEVELOPER_FEATURES")
            || std::env::args().any(|a| a == "--use_developer_features" || a == "--use-developer-features")
            || matches!(
                web_local_storage_api::get_item(STORAGE_FLAG),
                Ok(Some(v)) if v == "true" || v == "1"
            )
    })
}

fn persisted_flag() -> bool {
    matches!(web_local_storage_api::get_item(STORAGE_FLAG), Ok(Some(v)) if v == "true" || v == "1")
}

// ---------------------------------------------------------------------------
// Utilidades
// ---------------------------------------------------------------------------

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runtime propio de las herramientas (no se mezcla con el de Discord).
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ecord-devtools")
            .enable_all()
            .build()
            .expect("no se pudo crear el runtime de las herramientas de desarrollo")
    })
}

fn profile_name(profile: Profile) -> String {
    format!("{profile:?}")
}

fn platform_name(platform: Platform) -> String {
    format!("{platform:?}")
}

/// Familia de navegador de un profile (por el prefijo de su nombre).
fn family_of(name: &str) -> &'static str {
    for family in ["Chrome", "Edge", "Firefox", "Opera", "Safari", "OkHttp"] {
        if name.starts_with(family) {
            return family;
        }
    }
    "Otro"
}

const FAMILIES: [&str; 6] = ["Chrome", "Edge", "Firefox", "Opera", "Safari", "OkHttp"];

// ---------------------------------------------------------------------------
// Configuración de la conexión emulada (compartida por todas las pestañas)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
struct HeaderRow {
    on: bool,
    name: String,
    value: String,
}

impl HeaderRow {
    fn new(name: &str, value: &str) -> Self {
        Self { on: true, name: name.to_owned(), value: value.to_owned() }
    }
}

/// Lo que define una conexión: profile + plataforma + opciones de la emulación
/// + headers propios. `profile` y `platform` se guardan por nombre porque los
/// enums de `wreq-util` no son `Serialize` sin una feature extra.
#[derive(Clone, Serialize, Deserialize)]
struct EmuConfig {
    profile: String,
    platform: String,
    /// Usar el HTTP/2 propio del profile (`Emulation::http2`).
    http2: bool,
    /// Mandar los headers propios del profile (`Emulation::headers`). Apagado,
    /// salen solo los de la tabla de abajo.
    emu_headers: bool,
    headers: Vec<HeaderRow>,
}

impl Default for EmuConfig {
    fn default() -> Self {
        Self {
            profile: profile_name(Profile::Chrome149),
            platform: platform_name(Platform::Windows),
            http2: true,
            emu_headers: true,
            headers: Vec::new(),
        }
    }
}

impl EmuConfig {
    fn resolve(&self) -> Result<(Profile, Platform), String> {
        let profile = Profile::VARIANTS
            .iter()
            .copied()
            .find(|p| profile_name(*p) == self.profile)
            .ok_or_else(|| format!("profile desconocido: {}", self.profile))?;
        let platform = Platform::VARIANTS
            .iter()
            .copied()
            .find(|p| platform_name(*p) == self.platform)
            .ok_or_else(|| format!("plataforma desconocida: {}", self.platform))?;
        Ok((profile, platform))
    }

    fn active_headers(&self) -> impl Iterator<Item = &HeaderRow> {
        self.headers.iter().filter(|h| h.on && !h.name.trim().is_empty())
    }

    fn summary(&self) -> String {
        format!(
            "{} · {} · http2 {} · headers del profile {} · {} headers propios",
            self.profile,
            self.platform,
            if self.http2 { "sí" } else { "no" },
            if self.emu_headers { "sí" } else { "no" },
            self.active_headers().count(),
        )
    }
}

/// Arma el `wreq::Client` con la emulación pedida.
fn build_client(cfg: &EmuConfig) -> Result<wreq::Client, String> {
    let (profile, platform) = cfg.resolve()?;
    let emulation = Emulation::builder()
        .profile(profile)
        .platform(platform)
        .http2(cfg.http2)
        .headers(cfg.emu_headers)
        .build();
    let mut builder = wreq::Client::builder()
        .emulation(emulation)
        .gzip(true)
        .deflate(true)
        .brotli(true)
        .zstd(true);
    // Igual que el resto de la app: con ECORD_INSECURE_TLS=1 se puede
    // interceptar el tráfico con un proxy de depuración.
    if discord_client_rest::insecure_tls_enabled() {
        builder = builder.tls_cert_verification(false);
    }
    builder.build().map_err(|e| format!("{e}\n{e:?}"))
}

/// Parsea texto "Nombre: valor" (uno por línea) a filas de headers. Ignora
/// líneas vacías y la línea de petición (`GET / HTTP/1.1`).
fn parse_raw_headers(raw: &str) -> Vec<HeaderRow> {
    raw.lines()
        .filter_map(|line| {
            let line = line.trim();
            // Vacías, línea de petición/estado y pseudo-headers de HTTP/2
            // (`:authority`), que no se pueden poner a mano.
            if line.is_empty() || line.starts_with(':') || line.starts_with("HTTP/") || line.contains(" HTTP/") {
                return None;
            }
            let (name, value) = line.split_once(':')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some(HeaderRow::new(name, value.trim()))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

const HTTP_PRESETS: [(&str, &str); 4] = [
    ("Discord: gateway (público)", "https://discord.com/api/v10/gateway"),
    ("Discord: canje de ticket QR", "https://discord.com/api/v10/users/@me/remote-auth/login"),
    ("Discord: login web", "https://discord.com/login"),
    ("Eco de huella TLS/HTTP2 (pingly)", "https://pingly.us.kg/api/all"),
];

const WS_PRESETS: [(&str, &str); 4] = [
    ("Discord: Gateway", "wss://gateway.discord.gg/?v=10&encoding=json"),
    ("Discord: Gateway (zlib-stream)", "wss://gateway.discord.gg/?v=10&encoding=json&compress=zlib-stream"),
    ("Discord: login por QR", "wss://remote-auth-gateway.discord.gg/?v=2"),
    ("Eco (postman-echo)", "wss://ws.postman-echo.com/raw"),
];

struct HttpResponseInfo {
    status: String,
    version: String,
    elapsed_ms: u128,
    headers: Vec<(String, String)>,
    body: String,
    body_len: usize,
    truncated: bool,
}

#[derive(Default)]
struct HttpOut {
    running: bool,
    result: Option<Result<HttpResponseInfo, String>>,
}

#[derive(Clone)]
struct HttpRequestSpec {
    method: String,
    url: String,
    body: String,
}

async fn do_http(cfg: &EmuConfig, spec: &HttpRequestSpec) -> Result<HttpResponseInfo, String> {
    let client = build_client(cfg)?;
    let method = wreq::Method::from_bytes(spec.method.as_bytes()).map_err(|e| format!("método inválido: {e}"))?;
    let mut request = client.request(method.clone(), spec.url.trim());
    for row in cfg.active_headers() {
        request = request.header(row.name.trim(), row.value.as_str());
    }
    if !spec.body.is_empty() && method != wreq::Method::GET && method != wreq::Method::HEAD {
        request = request.body(spec.body.clone());
    }

    let started = Instant::now();
    let work = async {
        let response = request.send().await.map_err(|e| format!("{e}\n{e:?}"))?;
        let status = response.status().to_string();
        let version = format!("{:?}", response.version());
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
            .collect::<Vec<_>>();
        let bytes = response.bytes().await.map_err(|e| format!("{e}\n{e:?}"))?;
        Ok::<_, String>((status, version, headers, bytes))
    };
    let (status, version, headers, bytes) = tokio::time::timeout(Duration::from_secs(30), work)
        .await
        .map_err(|_| "tiempo agotado (30 s)".to_owned())??;

    let shown = &bytes[..bytes.len().min(MAX_BODY_SHOWN)];
    Ok(HttpResponseInfo {
        status,
        version,
        elapsed_ms: started.elapsed().as_millis(),
        headers,
        body: String::from_utf8_lossy(shown).into_owned(),
        body_len: bytes.len(),
        truncated: bytes.len() > MAX_BODY_SHOWN,
    })
}

// ---------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Info,
    Out,
    In,
    Error,
}

struct WsLine {
    at_ms: u128,
    kind: LineKind,
    text: String,
}

#[derive(Default)]
struct WsShared {
    lines: Vec<WsLine>,
    connecting: bool,
    connected: bool,
}

enum WsCmd {
    Text(String),
    Close,
}

struct WsSession {
    shared: Arc<Mutex<WsShared>>,
    tx: Option<tokio::sync::mpsc::UnboundedSender<WsCmd>>,
}

impl WsSession {
    fn new() -> Self {
        Self { shared: Arc::new(Mutex::new(WsShared::default())), tx: None }
    }
}

fn ws_log(shared: &Mutex<WsShared>, started: Instant, kind: LineKind, text: impl Into<String>) {
    let mut guard = lock(shared);
    guard.lines.push(WsLine { at_ms: started.elapsed().as_millis(), kind, text: text.into() });
    if guard.lines.len() > MAX_WS_LINES {
        let excess = guard.lines.len() - MAX_WS_LINES;
        guard.lines.drain(..excess);
    }
}

/// Abre el socket con la emulación de `cfg` y los headers propios.
async fn open_ws(cfg: &EmuConfig, url: &str) -> Result<wreq::ws::WebSocket, String> {
    let client = build_client(cfg)?;
    let mut request = client
        .websocket(url.trim())
        .orig_headers(crate::discord::websocket::browser_ws_header_case());
    for row in cfg.active_headers() {
        request = request.header(row.name.trim(), row.value.as_str());
    }
    let response = request.send().await.map_err(|e| format!("{e}\n{e:?}"))?;
    response.into_websocket().await.map_err(|e| format!("{e}\n{e:?}"))
}

async fn ws_task(
    ctx: egui::Context,
    cfg: EmuConfig,
    url: String,
    shared: Arc<Mutex<WsShared>>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<WsCmd>,
) {
    use wreq::ws::message::Message;

    let started = Instant::now();
    ws_log(&shared, started, LineKind::Info, format!("Conectando a {url} con {} ...", cfg.profile));
    ctx.request_repaint();

    let ws = match tokio::time::timeout(Duration::from_secs(20), open_ws(&cfg, &url)).await {
        Ok(Ok(ws)) => ws,
        Ok(Err(error)) => {
            ws_log(&shared, started, LineKind::Error, format!("Handshake rechazado: {error}"));
            lock(&shared).connecting = false;
            ctx.request_repaint();
            return;
        }
        Err(_) => {
            ws_log(&shared, started, LineKind::Error, "Handshake: tiempo agotado (20 s)");
            lock(&shared).connecting = false;
            ctx.request_repaint();
            return;
        }
    };
    {
        let mut guard = lock(&shared);
        guard.connecting = false;
        guard.connected = true;
    }
    ws_log(&shared, started, LineKind::Info, "Conectado (upgrade aceptado)");
    ctx.request_repaint();

    let (mut write, mut read) = ws.split();
    loop {
        tokio::select! {
            command = rx.recv() => match command {
                Some(WsCmd::Text(text)) => {
                    match write.send(Message::Text(text.clone().into())).await {
                        Ok(()) => ws_log(&shared, started, LineKind::Out, text),
                        Err(error) => {
                            ws_log(&shared, started, LineKind::Error, format!("No se pudo enviar: {error:?}"));
                            break;
                        }
                    }
                }
                Some(WsCmd::Close) | None => {
                    let _ = write.close().await;
                    ws_log(&shared, started, LineKind::Info, "Cerrado por el usuario");
                    break;
                }
            },
            incoming = read.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let text: &str = &text;
                    ws_log(&shared, started, LineKind::In, text);
                }
                Some(Ok(Message::Binary(bytes))) => {
                    ws_log(&shared, started, LineKind::In, format!("[binario, {} bytes]", bytes.len()));
                }
                Some(Ok(Message::Close(_))) => {
                    ws_log(&shared, started, LineKind::Info, "El servidor cerró la conexión");
                    break;
                }
                Some(Ok(other)) => {
                    ws_log(&shared, started, LineKind::Info, format!("{other:?}"));
                }
                Some(Err(error)) => {
                    ws_log(&shared, started, LineKind::Error, format!("Error del socket: {error:?}"));
                    break;
                }
                None => {
                    ws_log(&shared, started, LineKind::Info, "Conexión terminada");
                    break;
                }
            },
        }
        ctx.request_repaint();
    }
    lock(&shared).connected = false;
    ctx.request_repaint();
}

// ---------------------------------------------------------------------------
// Todos los profiles
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum MatrixMode {
    Http,
    WebSocket,
}

#[derive(Clone)]
enum RowState {
    Pending,
    Running,
    Ok { detail: String, ms: u128 },
    Fail { detail: String, ms: u128 },
}

#[derive(Clone)]
struct MatrixRow {
    profile: String,
    state: RowState,
}

#[derive(Default)]
struct MatrixShared {
    rows: Vec<MatrixRow>,
    running: bool,
    done: usize,
    total: usize,
    mode_label: String,
}

/// Una prueba con un solo profile. Devuelve `(ok, detalle, ms)`.
async fn probe_one(mode: MatrixMode, cfg: EmuConfig, spec: HttpRequestSpec) -> (bool, String, u128) {
    let started = Instant::now();
    match mode {
        MatrixMode::Http => match do_http(&cfg, &spec).await {
            Ok(info) => {
                let ok = info.status.starts_with('2') || info.status.starts_with('3');
                (ok, format!("{} · {} · {} bytes", info.status, info.version, info.body_len), info.elapsed_ms)
            }
            Err(error) => (false, first_line(&error), started.elapsed().as_millis()),
        },
        MatrixMode::WebSocket => {
            match tokio::time::timeout(Duration::from_secs(15), open_ws(&cfg, &spec.url)).await {
                Ok(Ok(ws)) => {
                    // Si el servidor habla primero (el Gateway manda `HELLO`),
                    // se muestra un pedazo; si no, alcanza con el upgrade.
                    let (_write, mut read) = ws.split();
                    let first = tokio::time::timeout(Duration::from_secs(2), read.next()).await;
                    let detail = match first {
                        Ok(Some(Ok(wreq::ws::message::Message::Text(text)))) => {
                            let text: &str = &text;
                            format!("upgrade OK · 1er mensaje: {}", truncate_chars(text, 90))
                        }
                        Ok(Some(Ok(_))) => "upgrade OK · 1er mensaje no es texto".to_owned(),
                        Ok(Some(Err(error))) => format!("upgrade OK · error al leer: {error}"),
                        Ok(None) => "upgrade OK · cerrado enseguida".to_owned(),
                        Err(_) => "upgrade OK · sin mensajes en 2 s".to_owned(),
                    };
                    drop(_write);
                    (true, detail, started.elapsed().as_millis())
                }
                Ok(Err(error)) => (false, first_line(&error), started.elapsed().as_millis()),
                Err(_) => (false, "tiempo agotado (15 s)".to_owned(), started.elapsed().as_millis()),
            }
        }
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").to_owned()
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}


// ---------------------------------------------------------------------------
// Datos que la app le pasa al panel
// ---------------------------------------------------------------------------

/// Lo que el panel necesita saber de la sesión de Discord. Lo arma `App` cada
/// frame mientras el panel está abierto (ver `DevTools::is_open`).
#[derive(Clone, Default)]
pub struct SessionInfo {
    /// Token de la cuenta, para la pestaña Discord. Nunca se muestra.
    pub token: Option<String>,
    /// Estado de la conexión, en palabras ("Conectado", "Sin sesión"...).
    pub status: String,
    pub username: Option<String>,
    pub user_id: Option<String>,
    pub servers: usize,
    pub dms: usize,
    pub friends: usize,
}

// ---------------------------------------------------------------------------
// Estado de la ventana
// ---------------------------------------------------------------------------

const TOP_H: f32 = 52.0;
const FOOT_H: f32 = 36.0;
/// Tamaño mínimo de la ventana flotante.
const MIN_W: f32 = 760.0;
const MIN_H: f32 = 460.0;
const SIDEBAR_W: f32 = 224.0;
const INSPECTOR_W: f32 = 330.0;
const DOCK_H: f32 = 230.0;
const TOAST_SECS: f32 = 2.2;

const HELP: [&str; 14] = [
    "Comandos de la consola:",
    "  help                 esta lista",
    "  clear                vacía el registro",
    "  info                 informe de diagnóstico del cliente",
    "  mem                  memoria del proceso y caché en disco",
    "  level <nivel>        muestra solo ese nivel y los más graves (error|warn|info|debug|trace)",
    "  copy                 copia al portapapeles las líneas visibles",
    "  tab <nombre>         consola | red | discord | trafico | cliente",
    "  get <ruta>           pedido a la API de Discord con la sesión, p. ej. get /users/@me",
    "  post|put|patch|delete <ruta> [json]   igual, con método y cuerpo opcional",
    "  toast [ps|app|<aumid>]  prueba de notificación de escritorio (Windows)",
    "  op [número|texto]    opcodes del Gateway: detalle, plantilla y (op 41) cuánto falta",
    "Las flechas ↑ ↓ recorren el historial. Doble clic en una línea del registro la copia.",
    "Los pedidos de Discord muestran su respuesta completa en la pestaña Discord.",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum MainTab {
    Console,
    Network,
    Discord,
    Traffic,
    Client,
}

/// Páginas de la pestaña Red.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Http,
    WebSocket,
    Matrix,
    Profile,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Http => "HTTP",
            Tab::WebSocket => "WebSocket",
            Tab::Matrix => "Todos los profiles",
            Tab::Profile => "Perfil / headers",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Tab::Http => "Una petición con la conexión emulada activa.",
            Tab::WebSocket => "Conecta, manda texto y mira lo que llega.",
            Tab::Matrix => "La misma prueba con cada profile, para ver cuáles acepta un servidor.",
            Tab::Profile => "Profile, plataforma, headers propios y perfiles guardados.",
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            Tab::Http => style::BLUE,
            Tab::WebSocket => style::GREEN,
            Tab::Matrix => style::ORANGE,
            Tab::Profile => style::PURPLE,
        }
    }
}

/// Dónde se dibuja el panel.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Ventana flotante dentro de eCord: se mueve y se redimensiona.
    Window,
    /// Acoplado abajo, a todo el ancho (como las devtools de un navegador).
    Dock,
    /// Ventana propia del sistema operativo (viewport de egui), aparte de la de eCord.
    Detached,
}

fn detached_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("ecord_devtools_window")
}

/// Las devtools que guarda la app. Es un `Arc<Mutex<..>>` porque la ventana aparte es un
/// viewport diferido, cuyo callback vive fuera del frame de la ventana principal.
#[derive(Clone, Default)]
pub struct DevToolsHandle(Arc<Mutex<DevTools>>);

impl DevToolsHandle {
    /// ¿Está el panel abierto? La app arma el [`SessionInfo`] solo en ese caso.
    pub fn is_open(&self) -> bool {
        lock(&self.0).is_open()
    }

    pub fn set_session(&self, session: SessionInfo) {
        lock(&self.0).set_session(session);
    }

    /// Se llama una vez por frame. No hace nada si las herramientas no están
    /// habilitadas (ver [`enabled`]).
    pub fn show(&self, ctx: &egui::Context) {
        lock(&self.0).show(ctx, &self.0);
    }
}

pub struct DevTools {
    open: bool,
    mode: Mode,
    /// Posición y tamaño de la ventana flotante (`None`: centrada con el tamaño por omisión).
    float_rect: Option<egui::Rect>,
    /// Alto del panel acoplado abajo (`None`: automático).
    dock_height: Option<f32>,
    main_tab: MainTab,
    /// Página activa dentro de Red.
    tab: Tab,
    show_left: bool,
    show_right: bool,
    show_dock: bool,
    session: SessionInfo,
    /// Aviso breve ("Copiado") y desde cuándo se muestra.
    toast: Option<(String, Instant)>,
    /// Lo piden los métodos de dibujo; `ui` lo convierte en `toast`.
    pending_toast: Option<&'static str>,
    console: console::ConsoleState,
    discord: discord_api::DiscordState,
    traffic: traffic::TrafficState,
    client: client::ClientState,
    /// Configuración activa, común a las pestañas de red.
    cfg: EmuConfig,

    // HTTP
    method: String,
    http_url: String,
    http_body: String,
    http_body_open: bool,
    http_out: Arc<Mutex<HttpOut>>,
    show_resp_headers: bool,

    // WebSocket
    ws_url: String,
    ws_input: String,
    ws: WsSession,

    // Todos los profiles
    matrix_mode: MatrixMode,
    matrix_family: Option<&'static str>,
    matrix_only_failed: bool,
    matrix: Arc<Mutex<MatrixShared>>,
    matrix_cancel: Arc<AtomicBool>,

    // Perfil / headers
    profile_filter: String,
    raw_headers: String,
    new_profile_name: String,
    saved_profiles: Vec<(String, EmuConfig)>,
    remember_flag: bool,
}

impl Default for DevTools {
    fn default() -> Self {
        let saved_profiles = if enabled() {
            web_local_storage_api::get_item(STORAGE_PROFILES)
                .ok()
                .flatten()
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Self {
            open: false,
            mode: Mode::Window,
            float_rect: None,
            dock_height: None,
            main_tab: MainTab::Console,
            tab: Tab::Http,
            show_left: true,
            show_right: true,
            show_dock: true,
            session: SessionInfo::default(),
            toast: None,
            pending_toast: None,
            console: console::ConsoleState::default(),
            discord: discord_api::DiscordState::default(),
            traffic: traffic::TrafficState::default(),
            client: client::ClientState::default(),
            cfg: EmuConfig::default(),
            method: "GET".to_owned(),
            http_url: HTTP_PRESETS[0].1.to_owned(),
            http_body: String::new(),
            http_body_open: false,
            http_out: Arc::new(Mutex::new(HttpOut::default())),
            show_resp_headers: true,
            ws_url: WS_PRESETS[0].1.to_owned(),
            ws_input: String::new(),
            ws: WsSession::new(),
            matrix_mode: MatrixMode::Http,
            matrix_family: None,
            matrix_only_failed: false,
            matrix: Arc::new(Mutex::new(MatrixShared::default())),
            matrix_cancel: Arc::new(AtomicBool::new(false)),
            profile_filter: String::new(),
            raw_headers: String::new(),
            new_profile_name: String::new(),
            saved_profiles,
            remember_flag: persisted_flag(),
        }
    }
}

/// Estado de la sesión en una palabra, para el chip de la barra superior.
fn status_chip(ui: &mut egui::Ui, palette: &Palette, session: &SessionInfo) {
    let (text, color) = match (&session.username, session.status.as_str()) {
        (Some(name), _) => (name.clone(), style::GREEN),
        (None, "") => ("Sin sesión".to_owned(), palette.dim),
        (None, status) => (status.to_owned(), style::ORANGE),
    };
    let font = theme::medium(12.5);
    let galley = ui.painter().layout_no_wrap(text.clone(), font.clone(), palette.text);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(galley.size().x + 36.0, 26.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 13.0, palette.surface);
    painter.rect_stroke(rect, 13.0, egui::Stroke::new(1.0, palette.outline), egui::StrokeKind::Inside);
    painter.circle_filled(egui::pos2(rect.left() + 14.0, rect.center().y), 4.0, color);
    painter.text(
        egui::pos2(rect.left() + 26.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        palette.text,
    );
    response.on_hover_text(format!(
        "Sesión de Discord: {}",
        if session.status.is_empty() { "sin información" } else { session.status.as_str() }
    ));
}

impl DevTools {
    /// ¿Está el panel abierto? La app arma el [`SessionInfo`] solo en ese caso.
    pub fn is_open(&self) -> bool {
        enabled() && self.open
    }

    pub fn set_session(&mut self, session: SessionInfo) {
        self.session = session;
    }

    /// Se llama una vez por frame. No hace nada si las herramientas no están
    /// habilitadas (ver [`enabled`]).
    fn show(&mut self, ctx: &egui::Context, handle: &Arc<Mutex<DevTools>>) {
        if !enabled() {
            return;
        }

        let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::D);
        if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
            self.open = !self.open;
        }

        if !self.open {
            self.show_pill(ctx);
            return;
        }

        let palette = theme::current(ctx);
        self.client.sample_frame(ctx.input(|i| i.unstable_dt));
        match self.mode {
            Mode::Window | Mode::Dock => self.show_overlay(ctx, &palette),
            Mode::Detached => self.show_detached(ctx, handle),
        }
        self.request_repaints(ctx);
    }

    /// Mientras algo está corriendo, refrescar para mostrar el avance. Se llama desde
    /// la ventana principal y, en modo ventana aparte, desde la propia ventana.
    fn request_repaints(&self, ctx: &egui::Context) {
        let busy = lock(&self.http_out).running
            || lock(&self.ws.shared).connecting
            || lock(&self.ws.shared).connected
            || lock(&self.matrix).running
            || self.discord.busy();
        if busy {
            ctx.request_repaint_after(Duration::from_millis(120));
        } else if self.main_tab == MainTab::Traffic {
            // El tráfico llega desde otros hilos sin despertar a la UI.
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Ventana flotante o acoplada: se dibuja encima de la app, en su misma ventana.
    fn show_overlay(&mut self, ctx: &egui::Context, palette: &Palette) {
        let screen = ctx.viewport_rect();
        let rect = self.overlay_rect(screen);
        egui::Area::new(egui::Id::new("ecord_devtools_overlay_v4"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                self.panel_in(ui, ctx, palette, rect, true);
                // Después del contenido: los bordes ganan el clic sobre lo que haya debajo.
                self.resize_handles(ui, rect);
            });
    }

    /// Ventana propia del sistema operativo. El viewport es "diferido": egui lo dibuja en
    /// su propio frame, sin colgarlo del de la ventana principal. Con uno "inmediato" cada
    /// frame de eCord esperaba además al `swap_buffers` (vsync) de esta ventana, y los fps
    /// de la app caían en picada mientras estaba abierta. Como el callback tiene que ser
    /// `Send + Sync + 'static`, el estado de las devtools va detrás de `handle`.
    fn show_detached(&mut self, ctx: &egui::Context, handle: &Arc<Mutex<DevTools>>) {
        let id = detached_viewport_id();
        let builder = egui::ViewportBuilder::default()
            .with_title("eCord · Developer tools")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([MIN_W, MIN_H]);
        let handle = Arc::clone(handle);
        ctx.show_viewport_deferred(id, builder, move |surface, _class| {
            surface.paint(|ui, vctx| {
                // `try_lock`: si el estado está tomado (p. ej. por el frame de la ventana
                // principal) no se espera —un `lock` anidado en el mismo hilo cuelga la app—,
                // se reintenta enseguida.
                match handle.try_lock() {
                    Ok(mut devtools) => devtools.paint_detached(ui, vctx),
                    Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                        poisoned.into_inner().paint_detached(ui, vctx);
                    }
                    Err(std::sync::TryLockError::WouldBlock) => {
                        vctx.request_repaint_after(Duration::from_millis(16));
                    }
                }
            });
        });
        // Los hilos de fondo despiertan a la ventana principal, no a esta: se repinta
        // tras ella, pero a lo sumo ~30 veces por segundo. Pedirle un frame por cada frame
        // de la principal (140/s) hacía esperar el vsync de esta ventana en cada uno.
        ctx.request_repaint_after_for(Duration::from_millis(33), id);
    }

    /// Contenido de la ventana aparte (corre en el frame de su propio viewport).
    fn paint_detached(&mut self, ui: &mut egui::Ui, vctx: &egui::Context) {
        let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::D);
        if vctx.input(|i| i.viewport().close_requested()) || vctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
            self.open = false;
        }
        let palette = theme::current(vctx);
        let rect = vctx.viewport_rect();
        self.panel_in(ui, vctx, &palette, rect, false);
        self.request_repaints(vctx);
        if !self.open || self.mode != Mode::Detached {
            // La ventana principal es la que deja de pedir este viewport: que lo note ya.
            vctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    /// El panel completo (barra superior, cuerpo y barra inferior) dentro de `rect`.
    fn panel_in(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette, rect: egui::Rect, shadow: bool) {
        ui.set_min_size(rect.size());
        // Se registra antes que el contenido: los widgets del panel
        // ganan el clic, y lo demás no le llega a la app de abajo.
        ui.interact(rect, egui::Id::new("ecord_devtools_block"), egui::Sense::click_and_drag());
        let mut panel = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("ecord_devtools_panel")
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let shadow = if shadow {
            egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: palette.shadow }
        } else {
            egui::epaint::Shadow::NONE
        };
        egui::Frame::new()
            .fill(palette.window_solid)
            .stroke(egui::Stroke::new(1.0, palette.outline))
            .corner_radius(egui::CornerRadius::same(self.corner()))
            .inner_margin(egui::Margin::same(0))
            .shadow(shadow)
            .show(&mut panel, |ui| {
                let inner = rect.size() - egui::vec2(2.0, 2.0);
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                self.ui(ui, ctx, palette);
            });
    }

    /// Radio de las esquinas del panel (acoplado y en ventana aparte van cuadrados).
    fn corner(&self) -> u8 {
        match self.mode {
            Mode::Window => 10,
            Mode::Dock | Mode::Detached => 0,
        }
    }

    fn overlay_rect(&mut self, screen: egui::Rect) -> egui::Rect {
        match self.mode {
            Mode::Window => {
                let default = egui::Rect::from_center_size(
                    screen.center(),
                    egui::vec2(
                        (screen.width() - 48.0).max(MIN_W).min(screen.width()),
                        (screen.height() - 48.0).max(MIN_H).min(screen.height()),
                    ),
                );
                let rect = clamp_floating(self.float_rect.unwrap_or(default), screen);
                self.float_rect = Some(rect);
                rect
            }
            Mode::Dock => {
                let auto = (screen.height() * 0.5).clamp(340.0, 760.0);
                let height = self
                    .dock_height
                    .unwrap_or(auto)
                    .max(240.0)
                    .min(screen.height() - 40.0)
                    .min(screen.height());
                egui::Rect::from_min_size(
                    egui::pos2(screen.left(), screen.bottom() - height),
                    egui::vec2(screen.width(), height),
                )
            }
            // No se usa: la ventana aparte ocupa todo su propio viewport.
            Mode::Detached => screen,
        }
    }

    /// Bordes y esquinas para cambiar el tamaño: los 8 de la ventana flotante o, si
    /// está acoplada abajo, el borde de arriba (cambia el alto).
    fn resize_handles(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        const EDGE: f32 = 6.0;
        const CORNER: f32 = 14.0;
        let half = EDGE / 2.0;
        let (left, right, top, bottom) = (rect.left(), rect.right(), rect.top(), rect.bottom());

        match self.mode {
            Mode::Dock => {
                let strip = egui::Rect::from_min_max(egui::pos2(left, top - half), egui::pos2(right, top + half));
                let response = ui
                    .interact(strip, egui::Id::new("dev_dock_resize"), egui::Sense::drag())
                    .on_hover_cursor(egui::CursorIcon::ResizeVertical);
                if response.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                    self.dock_height = Some(rect.height() - response.drag_delta().y);
                }
            }
            Mode::Window => {
                use egui::CursorIcon::{ResizeHorizontal, ResizeNeSw, ResizeNwSe, ResizeVertical};
                let area = |min: (f32, f32), max: (f32, f32)| {
                    egui::Rect::from_min_max(egui::pos2(min.0, min.1), egui::pos2(max.0, max.1))
                };
                // (nombre, zona, [izquierda, derecha, arriba, abajo] que se mueven, cursor)
                let handles: [(&str, egui::Rect, [bool; 4], egui::CursorIcon); 8] = [
                    ("w", area((left - half, top + CORNER), (left + half, bottom - CORNER)), [true, false, false, false], ResizeHorizontal),
                    ("e", area((right - half, top + CORNER), (right + half, bottom - CORNER)), [false, true, false, false], ResizeHorizontal),
                    ("n", area((left + CORNER, top - half), (right - CORNER, top + half)), [false, false, true, false], ResizeVertical),
                    ("s", area((left + CORNER, bottom - half), (right - CORNER, bottom + half)), [false, false, false, true], ResizeVertical),
                    ("nw", area((left - half, top - half), (left + CORNER, top + CORNER)), [true, false, true, false], ResizeNwSe),
                    ("ne", area((right - CORNER, top - half), (right + half, top + CORNER)), [false, true, true, false], ResizeNeSw),
                    ("sw", area((left - half, bottom - CORNER), (left + CORNER, bottom + half)), [true, false, false, true], ResizeNeSw),
                    ("se", area((right - CORNER, bottom - CORNER), (right + half, bottom + half)), [false, true, false, true], ResizeNwSe),
                ];
                let mut next = rect;
                let mut changed = false;
                for (name, zone, [move_left, move_right, move_top, move_bottom], cursor) in handles {
                    let response = ui
                        .interact(zone, egui::Id::new(("dev_resize", name)), egui::Sense::drag())
                        .on_hover_cursor(cursor);
                    if !response.dragged() {
                        continue;
                    }
                    ui.ctx().set_cursor_icon(cursor);
                    let delta = response.drag_delta();
                    if move_left {
                        next.min.x = (next.min.x + delta.x).min(next.max.x - MIN_W);
                    }
                    if move_right {
                        next.max.x = (next.max.x + delta.x).max(next.min.x + MIN_W);
                    }
                    if move_top {
                        next.min.y = (next.min.y + delta.y).min(next.max.y - MIN_H);
                    }
                    if move_bottom {
                        next.max.y = (next.max.y + delta.y).max(next.min.y + MIN_H);
                    }
                    changed = true;
                }
                if changed {
                    self.float_rect = Some(next);
                }
            }
            Mode::Detached => {}
        }
    }

    /// Botón pequeño `DEV` en la esquina inferior derecha (con los errores del log).
    fn show_pill(&mut self, ctx: &egui::Context) {
        let errors = capture::counts()[Level::Error.index()];
        egui::Area::new(egui::Id::new("ecord_devtools_pill"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-10.0, -10.0])
            .show(ctx, |ui| {
                let text = if errors > 0 { format!("DEV · {errors}") } else { "DEV".to_owned() };
                let mut label = egui::RichText::new(text).small().strong();
                if errors > 0 {
                    label = label.color(style::RED);
                }
                if ui.add(egui::Button::new(label).small()).on_hover_text("Developer tools (Ctrl+Shift+D)").clicked() {
                    self.open = true;
                }
            });
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette) {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

        StripBuilder::new(ui)
            .clip(true)
            .size(Size::exact(TOP_H))
            .size(Size::remainder())
            .size(Size::exact(FOOT_H))
            .vertical(|mut strip| {
                strip.cell(|ui| self.top_bar(ui, ctx, palette));
                strip.cell(|ui| self.body(ui, ctx, palette));
                strip.cell(|ui| self.footer(ui, palette));
            });

        if let Some(text) = self.pending_toast.take() {
            self.toast = Some((text.to_owned(), Instant::now()));
        }
        let expired = self.toast.as_ref().is_some_and(|(_, since)| since.elapsed().as_secs_f32() >= TOAST_SECS);
        if expired {
            self.toast = None;
        }
        if let Some((text, _)) = &self.toast {
            style::paint_toast(ui, palette, ui.max_rect(), text);
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    // -- Barra superior ------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette) {
        let rect = ui.max_rect();
        let radius = self.corner().saturating_sub(1);
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius { nw: radius, ne: radius, sw: 0, se: 0 },
            palette.panel,
        );
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, egui::Stroke::new(1.0, palette.outline));
        let wide = rect.width() >= 980.0;

        // Ventana flotante: toda la barra es el asa para moverla (doble clic la centra).
        // Se registra antes que los botones y las pestañas, que ganan el clic.
        if self.mode == Mode::Window {
            let handle = ui.interact(rect, egui::Id::new("dev_top_drag"), egui::Sense::click_and_drag());
            if handle.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                if let Some(float) = &mut self.float_rect {
                    *float = float.translate(handle.drag_delta());
                }
            } else if handle.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            if handle.double_clicked() {
                self.float_rect = None;
            }
        }

        // Izquierda: marca, título y estado de la sesión.
        let mut left = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("dev_top_left")
                .max_rect(rect.shrink2(egui::vec2(14.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        left.spacing_mut().item_spacing = egui::vec2(10.0, 0.0);
        let (logo, _) = left.allocate_exact_size(egui::vec2(32.0, 26.0), egui::Sense::hover());
        left.painter().rect_filled(logo, egui::CornerRadius::same(8), palette.accent);
        left.painter().text(logo.center(), egui::Align2::CENTER_CENTER, "DEV", theme::bold(10.5), palette.on_accent);
        if wide {
            left.label(egui::RichText::new("Developer tools").font(theme::semibold(15.0)).color(palette.text));
            status_chip(&mut left, palette, &self.session);
        }

        // Centro: pestañas principales.
        let items = [
            (MainTab::Console, "Consola"),
            (MainTab::Network, "Red"),
            (MainTab::Discord, "Discord"),
            (MainTab::Traffic, "Tráfico"),
            (MainTab::Client, "Cliente"),
        ];
        let width = style::segmented_width(ui, &items);
        let mut tabs = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("dev_top_tabs")
                .max_rect(egui::Rect::from_center_size(rect.center(), egui::vec2(width, rect.height())))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        style::segmented(&mut tabs, palette, &mut self.main_tab, &items);

        // Derecha: cerrar, modo, limpiar y copiar informe.
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("dev_top_right")
                .max_rect(rect.shrink2(egui::vec2(10.0, 0.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        right.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
        if style::bar_icon(&mut right, palette, theme::Icon::X, "Cerrar (Ctrl+Shift+D)").clicked() {
            self.open = false;
        }
        // Los dos modos a los que se puede pasar desde el actual.
        let modes = [
            (Mode::Detached, theme::Icon::ExternalLink, "Abrir en una ventana aparte"),
            (Mode::Dock, theme::Icon::Shrink, "Acoplar abajo (el alto se cambia desde el borde de arriba)"),
            (Mode::Window, theme::Icon::Maximize2, "Ventana flotante (se mueve y se redimensiona)"),
        ];
        let mut next_mode = None;
        for (mode, icon, tip) in modes {
            if mode != self.mode && style::bar_icon(&mut right, palette, icon, tip).clicked() {
                next_mode = Some(mode);
            }
        }
        if let Some(mode) = next_mode {
            self.mode = mode;
        }
        if style::bar_icon(&mut right, palette, theme::Icon::Trash, "Vaciar la consola y el tráfico").clicked() {
            capture::clear();
            self.console.selected = None;
            discord_client_rest::netlog::clear();
            self.traffic.clear_selection();
            self.pending_toast = Some("Consola y tráfico vaciados");
        }
        if style::bar_icon(&mut right, palette, theme::Icon::Copy, "Copiar informe de diagnóstico").clicked() {
            ctx.copy_text(client::report(ctx, &self.session, &self.cfg.summary()));
            self.pending_toast = Some("Informe copiado");
        }
    }

    // -- Cuerpo: lista · centro · inspector ---------------------------------------------

    fn body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette) {
        let left = self.show_left;
        let right = self.show_right && ui.available_width() > 880.0;
        let mut builder = StripBuilder::new(ui).clip(true);
        if left {
            builder = builder.size(Size::exact(SIDEBAR_W));
        }
        builder = builder.size(Size::remainder().at_least(300.0));
        if right {
            builder = builder.size(Size::exact(INSPECTOR_W));
        }
        builder.horizontal(|mut strip| {
            if left {
                strip.cell(|ui| {
                    let rect = ui.max_rect();
                    ui.painter().rect_filled(rect, 0.0, palette.panel);
                    ui.painter().vline(rect.right() - 0.5, rect.y_range(), egui::Stroke::new(1.0, palette.outline));
                    self.sidebar(ui, palette);
                });
            }
            strip.cell(|ui| self.center(ui, ctx, palette));
            if right {
                strip.cell(|ui| {
                    let rect = ui.max_rect();
                    ui.painter().rect_filled(rect, 0.0, palette.panel);
                    ui.painter().vline(rect.left() + 0.5, rect.y_range(), egui::Stroke::new(1.0, palette.outline));
                    self.inspector(ui, ctx, palette);
                });
            }
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        match self.main_tab {
            MainTab::Console => self.console.sidebar(ui, palette),
            MainTab::Network => self.net_sidebar(ui, palette),
            MainTab::Discord => self.discord.sidebar(ui, palette, &self.session),
            MainTab::Traffic => self.traffic.sidebar(ui, palette),
            MainTab::Client => self.client.sidebar(ui, palette),
        }
    }

    fn net_sidebar(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        style::panel_title(ui, palette, "Red");
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        style::side_header(ui, palette, "Pruebas");
        for tab in [Tab::Http, Tab::WebSocket, Tab::Matrix] {
            if style::side_item(ui, palette, tab.title(), tab.color(), self.tab == tab, None).clicked() {
                self.tab = tab;
            }
        }
        style::side_header(ui, palette, "Conexión");
        let tab = Tab::Profile;
        if style::side_item(ui, palette, tab.title(), tab.color(), self.tab == tab, None).clicked() {
            self.tab = tab;
        }
    }

    fn center(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette) {
        let dock = self.show_dock
            && self.main_tab != MainTab::Console
            && self.main_tab != MainTab::Traffic
            && ui.available_height() > 380.0;
        let mut builder = StripBuilder::new(ui).clip(true).size(Size::remainder().at_least(160.0));
        if dock {
            builder = builder.size(Size::exact(DOCK_H));
        }
        builder.vertical(|mut strip| {
            strip.cell(|ui| self.page(ui, ctx, palette));
            if dock {
                strip.cell(|ui| {
                    let rect = ui.max_rect();
                    ui.painter().rect_filled(rect, 0.0, palette.panel);
                    ui.painter().hline(rect.x_range(), rect.top() + 0.5, egui::Stroke::new(1.0, palette.outline));
                    self.console.log_table(ui, palette, "dev_console_dock");
                });
            }
        });
    }

    fn page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette) {
        match self.main_tab {
            MainTab::Console => {
                StripBuilder::new(ui).clip(true).size(Size::remainder()).size(Size::exact(46.0)).vertical(|mut strip| {
                    strip.cell(|ui| self.console.log_table(ui, palette, "dev_console_main"));
                    strip.cell(|ui| {
                        if let Some(action) = self.console.input_row(ui, palette) {
                            self.handle_console(ctx, action);
                        }
                    });
                });
            }
            MainTab::Network => {
                egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                    theme::section_title(ui, palette, self.tab.title());
                    theme::text(ui, self.tab.subtitle(), theme::regular(12.0), palette.dim);
                    ui.add_space(4.0);
                    match self.tab {
                        Tab::Http => self.http_tab(ui, ctx, palette),
                        Tab::WebSocket => self.ws_tab(ui, ctx, palette),
                        Tab::Matrix => self.matrix_tab(ui, ctx, palette),
                        Tab::Profile => self.profile_tab(ui),
                    }
                });
            }
            MainTab::Discord => {
                egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                    self.discord.page(ui, ctx, palette, &self.session);
                });
            }
            MainTab::Traffic => self.traffic.page(ui, palette),
            MainTab::Client => {
                egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                    let toast = self.client.page(ui, ctx, palette, &self.session, &mut self.remember_flag);
                    if toast.is_some() {
                        self.pending_toast = toast;
                    }
                });
            }
        }
    }

    // -- Inspector (panel derecho) ---------------------------------------------------------

    fn inspector(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette) {
        let toast = match self.main_tab {
            MainTab::Console => self.console.inspector(ui, palette),
            MainTab::Network => {
                self.net_inspector(ui, palette);
                None
            }
            MainTab::Discord => self.discord.inspector(ui, ctx, palette),
            MainTab::Traffic => self.traffic.inspector(ui, palette),
            MainTab::Client => {
                let report = client::report(ctx, &self.session, &self.cfg.summary());
                self.client.inspector(ui, palette, &report)
            }
        };
        if toast.is_some() {
            self.pending_toast = toast;
        }
    }

    /// Conexión emulada activa: profile, plataforma y opciones (lo que usan
    /// todas las pruebas de Red).
    fn net_inspector(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        style::panel_title(ui, palette, "Conexión activa");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 0)).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            let width = (ui.available_width() - 4.0).max(120.0);

            style::field_label(ui, palette, "Profile");
            ui.add(
                egui::TextEdit::singleline(&mut self.profile_filter).desired_width(width).hint_text("Filtrar profiles…"),
            );
            let filter = self.profile_filter.trim().to_ascii_lowercase();
            egui::ComboBox::from_id_salt("dev_insp_profile").selected_text(self.cfg.profile.clone()).width(width).height(360.0).show_ui(
                ui,
                |ui| {
                    for profile in Profile::VARIANTS {
                        let name = profile_name(*profile);
                        if !filter.is_empty() && !name.to_ascii_lowercase().contains(&filter) {
                            continue;
                        }
                        ui.selectable_value(&mut self.cfg.profile, name.clone(), name);
                    }
                },
            );

            style::field_label(ui, palette, "Plataforma");
            egui::ComboBox::from_id_salt("dev_insp_platform").selected_text(self.cfg.platform.clone()).width(width).show_ui(
                ui,
                |ui| {
                    for platform in Platform::VARIANTS {
                        let name = platform_name(*platform);
                        ui.selectable_value(&mut self.cfg.platform, name.clone(), name);
                    }
                },
            );

            ui.add_space(8.0);
            ui.checkbox(&mut self.cfg.http2, "HTTP/2 propio del profile");
            ui.checkbox(&mut self.cfg.emu_headers, "Headers propios del profile");

            style::field_label(ui, palette, "Headers propios");
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} activos", self.cfg.active_headers().count())).color(palette.dim),
                );
                if ui.small_button("Editar").clicked() {
                    self.tab = Tab::Profile;
                }
            });

            if !self.saved_profiles.is_empty() {
                style::field_label(ui, palette, "Perfiles guardados");
                let mut load: Option<usize> = None;
                egui::ComboBox::from_id_salt("dev_insp_saved").selected_text("Cargar…").width(width).show_ui(ui, |ui| {
                    for (index, (name, _)) in self.saved_profiles.iter().enumerate() {
                        if ui.selectable_label(false, name).clicked() {
                            load = Some(index);
                        }
                    }
                });
                if let Some(index) = load {
                    self.cfg = self.saved_profiles[index].1.clone();
                    self.pending_toast = Some("Perfil cargado");
                }
            }

            ui.add_space(12.0);
            ui.label(egui::RichText::new(self.cfg.summary()).small().color(palette.dim));
        });
    }

    // -- Barra inferior ------------------------------------------------------------------------

    fn footer(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let rect = ui.max_rect();
        let radius = self.corner().saturating_sub(1);
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius { nw: 0, ne: 0, sw: radius, se: radius },
            palette.panel,
        );
        ui.painter().hline(rect.x_range(), rect.top() + 0.5, egui::Stroke::new(1.0, palette.outline));

        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("dev_footer_left")
                .max_rect(rect.shrink2(egui::vec2(12.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        row.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);
        let counts = capture::counts();
        for level in Level::ALL {
            let index = level.index();
            if style::count_chip(&mut row, palette, level, counts[index], self.console.levels[index]).clicked() {
                self.console.levels[index] = !self.console.levels[index];
            }
        }
        row.add_space(10.0);
        row.add(
            egui::TextEdit::singleline(&mut self.console.filter).hint_text("Filtrar logs…").desired_width(220.0),
        );
        row.add_space(10.0);
        style::toggle_switch(&mut row, palette, &mut self.console.follow);
        row.add_space(2.0);
        row.label(egui::RichText::new("Seguir al final").size(12.5).color(palette.secondary));

        let mut icons = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("dev_footer_right")
                .max_rect(rect.shrink2(egui::vec2(12.0, 0.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        icons.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
        if style::panel_toggle(&mut icons, palette, style::PanelSide::Right, self.show_right, "Panel derecho (inspector)")
            .clicked()
        {
            self.show_right = !self.show_right;
        }
        if style::panel_toggle(&mut icons, palette, style::PanelSide::Bottom, self.show_dock, "Consola abajo").clicked() {
            self.show_dock = !self.show_dock;
        }
        if style::panel_toggle(&mut icons, palette, style::PanelSide::Left, self.show_left, "Panel izquierdo (lista)")
            .clicked()
        {
            self.show_left = !self.show_left;
        }
    }

    // -- Comandos de la consola ------------------------------------------------------------------

    fn handle_console(&mut self, ctx: &egui::Context, action: console::ConsoleAction) {
        match action {
            console::ConsoleAction::Run(line) => self.run_command(ctx, &line),
        }
    }

    fn run_command(&mut self, ctx: &egui::Context, line: &str) {
        capture::push(Level::Info, "consola", format!("› {line}"));
        let (command, rest) = match line.split_once(char::is_whitespace) {
            Some((command, rest)) => (command.to_ascii_lowercase(), rest.trim()),
            None => (line.to_ascii_lowercase(), ""),
        };
        match command.as_str() {
            "help" | "?" => {
                for text in HELP {
                    capture::push(Level::Info, "consola", text);
                }
            }
            "clear" | "cls" => {
                capture::clear();
                self.console.selected = None;
            }
            "info" => {
                for text in client::report(ctx, &self.session, &self.cfg.summary()).lines() {
                    capture::push(Level::Info, "consola", text);
                }
            }
            "mem" => {
                let heap = crate::support::mem_report::heap_bytes();
                let resident = crate::support::mem_report::process_memory().map(|memory| memory.resident);
                let textures = crate::support::mem_report::gpu_texture_bytes(ctx);
                capture::push(
                    Level::Info,
                    "consola",
                    format!(
                        "heap {} · RAM {} · texturas {}",
                        crate::support::mem_report::fmt_bytes(heap),
                        resident.map_or("—".to_owned(), crate::support::mem_report::fmt_bytes),
                        crate::support::mem_report::fmt_bytes(textures),
                    ),
                );
            }
            "level" => match Level::parse(rest) {
                Some(minimum) => {
                    for level in Level::ALL {
                        self.console.levels[level.index()] = level <= minimum;
                    }
                    capture::push(Level::Info, "consola", format!("Mostrando {} y más graves.", minimum.tag()));
                }
                None => capture::push(Level::Warn, "consola", "Uso: level error|warn|info|debug|trace"),
            },
            "copy" => {
                let text = self.console.visible_text();
                let lines = text.lines().count();
                ctx.copy_text(text);
                capture::push(Level::Info, "consola", format!("{lines} líneas copiadas."));
                self.pending_toast = Some("Registro copiado");
            }
            "tab" => match rest.to_ascii_lowercase().as_str() {
                "consola" | "console" => self.main_tab = MainTab::Console,
                "red" | "net" | "network" => self.main_tab = MainTab::Network,
                "discord" => self.main_tab = MainTab::Discord,
                "trafico" | "tráfico" | "traffic" => self.main_tab = MainTab::Traffic,
                "cliente" | "client" => self.main_tab = MainTab::Client,
                _ => capture::push(Level::Warn, "consola", "Uso: tab consola|red|discord|trafico|cliente"),
            },
            "get" | "post" | "put" | "patch" | "delete" => {
                let (path, body) = match rest.split_once(char::is_whitespace) {
                    Some((path, body)) => (path, body.trim()),
                    None => (rest, ""),
                };
                if path.is_empty() {
                    capture::push(Level::Warn, "consola", format!("Uso: {command} /ruta [json]"));
                } else {
                    let session = self.session.clone();
                    self.discord.quick(ctx, &session, &command, path, body);
                }
            }
            "op" | "opcode" => {
                for text in opcode_lines(rest) {
                    capture::push(Level::Info, "consola", text);
                }
            }
            "toast" => {
                const ECORD: &str = "dev.ecord.client";
                const POWERSHELL: &str = r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";
                match rest {
                    "app" => crate::lib::notifications::send_desktop(
                        "eCord".into(),
                        "Prueba por el camino normal".into(),
                    ),
                    "ps" => crate::lib::toast_debug::run(POWERSHELL, "eCord (AUMID de PowerShell)", "Prueba de control"),
                    "" => crate::lib::toast_debug::run(ECORD, "eCord", "Prueba directa de toast"),
                    custom => crate::lib::toast_debug::run(custom, "eCord", "Prueba con AUMID propio"),
                }
            }
            other => capture::push(Level::Warn, "consola", format!("Comando desconocido: {other}. Escribe help.")),
        }
    }

    // -- HTTP ---------------------------------------------------------------

    fn http_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette) {
        let running = lock(&self.http_out).running;

        // Barra de petición: método · URL · presets · enviar.
        bar(ui, palette, |ui| {
            egui::ComboBox::from_id_salt("dev_http_method").selected_text(self.method.clone()).width(72.0).show_ui(
                ui,
                |ui| {
                    for method in METHODS {
                        ui.selectable_value(&mut self.method, method.to_owned(), method);
                    }
                },
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.http_url)
                    .frame(egui::Frame::NONE)
                    .desired_width((ui.available_width() - 200.0).max(100.0))
                    .hint_text("https://..."),
            );
            egui::ComboBox::from_id_salt("dev_http_preset").selected_text("Presets").width(70.0).show_ui(ui, |ui| {
                for (label, url) in HTTP_PRESETS {
                    if ui.selectable_label(false, label).clicked() {
                        self.http_url = url.to_owned();
                    }
                }
            });
            if primary_button(ui, palette, "Enviar", !running) {
                self.send_http(ctx);
            }
            if running {
                theme::spinner(ui, 16.0, palette.accent);
            }
        });

        ui.horizontal(|ui| {
            ui.toggle_value(&mut self.http_body_open, "Cuerpo");
            ui.checkbox(&mut self.show_resp_headers, "Headers de la respuesta");
            if !self.http_body.is_empty() {
                ui.weak(format!("{} bytes de cuerpo (se ignora en GET y HEAD)", self.http_body.len()));
            }
        });
        if self.http_body_open {
            ui.add(
                egui::TextEdit::multiline(&mut self.http_body)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .code_editor()
                    .hint_text("(vacío) — se ignora en GET y HEAD"),
            );
        }

        let out = lock(&self.http_out);
        match &out.result {
            None => {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.weak("Sin respuesta todavía. Escribe una URL y pulsa Enviar.");
                });
            }
            Some(Err(error)) => {
                inset_panel(ui, palette, |ui| {
                    ui.horizontal(|ui| badge(ui, "Falló", palette.danger));
                    egui::ScrollArea::vertical().id_salt("dev_http_err").auto_shrink([false, false]).show(ui, |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(error.as_str()).monospace()).selectable(true));
                    });
                });
            }
            Some(Ok(info)) => {
                let good = info.status.starts_with('2') || info.status.starts_with('3');
                ui.horizontal(|ui| {
                    badge(ui, &info.status, if good { theme_ok() } else { palette.danger });
                    ui.label(
                        egui::RichText::new(format!("{} · {} ms · {} bytes", info.version, info.elapsed_ms, info.body_len))
                            .color(palette.secondary),
                    );
                    if info.truncated {
                        ui.weak(format!("(se muestran {} KB)", MAX_BODY_SHOWN / 1024));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("copiar cuerpo").clicked() {
                            ui.ctx().copy_text(info.body.clone());
                        }
                    });
                });
                inset_panel(ui, palette, |ui| {
                    egui::ScrollArea::vertical().id_salt("dev_http_resp").auto_shrink([false, false]).show(ui, |ui| {
                        if self.show_resp_headers {
                            egui::CollapsingHeader::new(format!("Headers ({})", info.headers.len()))
                                .id_salt("dev_http_resp_headers")
                                .default_open(false)
                                .show(ui, |ui| {
                                    egui::Grid::new("dev_http_resp_grid").striped(true).num_columns(2).show(ui, |ui| {
                                        for (name, value) in &info.headers {
                                            ui.label(egui::RichText::new(name).monospace().strong());
                                            ui.add(egui::Label::new(egui::RichText::new(value).monospace()).selectable(true));
                                            ui.end_row();
                                        }
                                    });
                                });
                        }
                        ui.add(egui::Label::new(egui::RichText::new(&info.body).monospace()).selectable(true));
                    });
                });
            }
        }
    }

    fn send_http(&mut self, ctx: &egui::Context) {
        {
            let mut out = lock(&self.http_out);
            out.running = true;
            out.result = None;
        }
        let cfg = self.cfg.clone();
        let spec = HttpRequestSpec {
            method: self.method.clone(),
            url: self.http_url.clone(),
            body: self.http_body.clone(),
        };
        let out = Arc::clone(&self.http_out);
        let ctx = ctx.clone();
        runtime().spawn(async move {
            let result = do_http(&cfg, &spec).await;
            let mut guard = lock(&out);
            guard.running = false;
            guard.result = Some(result);
            drop(guard);
            ctx.request_repaint();
        });
    }

    // -- WebSocket -----------------------------------------------------------

    fn ws_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette) {
        let (connecting, connected) = {
            let guard = lock(&self.ws.shared);
            (guard.connecting, guard.connected)
        };

        bar(ui, palette, |ui| {
            ui.add_enabled(
                !connecting && !connected,
                egui::TextEdit::singleline(&mut self.ws_url)
                    .frame(egui::Frame::NONE)
                    .desired_width((ui.available_width() - 250.0).max(120.0))
                    .hint_text("wss://..."),
            );
            ui.add_enabled_ui(!connecting && !connected, |ui| {
                egui::ComboBox::from_id_salt("dev_ws_preset").selected_text("Presets").width(70.0).show_ui(ui, |ui| {
                    for (label, url) in WS_PRESETS {
                        if ui.selectable_label(false, label).clicked() {
                            self.ws_url = url.to_owned();
                        }
                    }
                });
            });
            if connected || connecting {
                if theme::soft_button(ui, palette, None, "Desconectar", false).clicked() {
                    if let Some(tx) = &self.ws.tx {
                        let _ = tx.send(WsCmd::Close);
                    }
                }
            } else if theme::pill_button(ui, palette, "Conectar", true).clicked() {
                self.connect_ws(ctx);
            }
            if connecting {
                theme::spinner(ui, 16.0, palette.accent);
            }
        });

        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("dev_ws_opcodes")
                .selected_text("Opcodes")
                .width(70.0)
                .height(360.0)
                .show_ui(ui, |ui| {
                    ui.weak("Carga la plantilla en el campo de envío");
                    for op in opcodes::templates() {
                        let label = format!("{:>2} · {}", op.code, op.name);
                        if ui.selectable_label(false, label).on_hover_text(op.note).clicked() {
                            self.ws_input = op.template.unwrap_or_default().to_owned();
                        }
                    }
                });
            let response = ui.add_enabled(
                connected,
                egui::TextEdit::singleline(&mut self.ws_input)
                    .desired_width((ui.available_width() - 270.0).max(120.0))
                    .hint_text("Texto a enviar (JSON, por ejemplo)"),
            );
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.add_enabled(connected, egui::Button::new("Enviar")).clicked() || enter) && connected {
                if let Some(tx) = &self.ws.tx {
                    let _ = tx.send(WsCmd::Text(std::mem::take(&mut self.ws_input)));
                }
                response.request_focus();
            }
            if ui.button("Limpiar").clicked() {
                lock(&self.ws.shared).lines.clear();
            }
            if ui.button("Copiar").on_hover_text("Copiar el registro").clicked() {
                let mut text = String::new();
                for line in &lock(&self.ws.shared).lines {
                    let _ = writeln!(text, "[{:>7} ms] {} {}", line.at_ms, line_tag(line.kind), line.text);
                }
                ui.ctx().copy_text(text);
            }
        });

        inset_panel(ui, palette, |ui| {
            let guard = lock(&self.ws.shared);
            egui::ScrollArea::vertical().id_salt("dev_ws_log").auto_shrink([false, false]).stick_to_bottom(true).show(
                ui,
                |ui| {
                    if guard.lines.is_empty() {
                        ui.weak("Sin actividad. Elige un preset y pulsa Conectar.");
                    }
                    for line in &guard.lines {
                        let color = match line.kind {
                            LineKind::Info => palette.dim,
                            LineKind::Out => palette.accent,
                            LineKind::In => theme_ok(),
                            LineKind::Error => palette.danger,
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(format!("{:>7} ms", line.at_ms)).monospace().color(palette.dim));
                            ui.label(egui::RichText::new(line_tag(line.kind)).monospace().color(color));
                            ui.add(egui::Label::new(egui::RichText::new(&line.text).monospace()).selectable(true));
                        });
                    }
                },
            );
        });
    }

    fn connect_ws(&mut self, ctx: &egui::Context) {
        {
            let mut guard = lock(&self.ws.shared);
            guard.connecting = true;
            guard.connected = false;
        }
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.ws.tx = Some(tx);
        runtime().spawn(ws_task(
            ctx.clone(),
            self.cfg.clone(),
            self.ws_url.clone(),
            Arc::clone(&self.ws.shared),
            rx,
        ));
    }

    // -- Todos los profiles ---------------------------------------------------

    fn matrix_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &Palette) {
        let running = lock(&self.matrix).running;

        bar(ui, palette, |ui| {
            ui.add_enabled_ui(!running, |ui| {
                ui.selectable_value(&mut self.matrix_mode, MatrixMode::Http, "HTTP");
                ui.selectable_value(&mut self.matrix_mode, MatrixMode::WebSocket, "WebSocket");
            });
            ui.separator();
            match self.matrix_mode {
                MatrixMode::Http => {
                    egui::ComboBox::from_id_salt("dev_matrix_method")
                        .selected_text(self.method.clone())
                        .width(72.0)
                        .show_ui(ui, |ui| {
                            for method in METHODS {
                                ui.selectable_value(&mut self.method, method.to_owned(), method);
                            }
                        });
                    ui.add(
                        egui::TextEdit::singleline(&mut self.http_url)
                            .frame(egui::Frame::NONE)
                            .desired_width((ui.available_width() - 10.0).max(120.0)),
                    );
                }
                MatrixMode::WebSocket => {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.ws_url)
                            .frame(egui::Frame::NONE)
                            .desired_width((ui.available_width() - 10.0).max(120.0)),
                    );
                }
            }
        });

        ui.horizontal_wrapped(|ui| {
            if running {
                if theme::soft_button(ui, palette, None, "Cancelar", false).clicked() {
                    self.matrix_cancel.store(true, Ordering::Relaxed);
                }
                theme::spinner(ui, 16.0, palette.accent);
            } else if theme::pill_button(ui, palette, "Probar los profiles", true).clicked() {
                self.start_matrix(ctx);
            }
            ui.label("Familia:");
            egui::ComboBox::from_id_salt("dev_matrix_family")
                .selected_text(self.matrix_family.unwrap_or("Todas"))
                .width(80.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.matrix_family, None, "Todas");
                    for family in FAMILIES {
                        ui.selectable_value(&mut self.matrix_family, Some(family), family);
                    }
                });
            ui.checkbox(&mut self.matrix_only_failed, "Solo fallidos");
            ui.label(egui::RichText::new(format!("Plataforma: {}", self.cfg.platform)).color(palette.dim)).on_hover_text(
                "Usa los headers propios y las opciones de la conexión activa; el profile cambia en cada fila. \
                 La plataforma se cambia en Perfil / headers.",
            );
        });

        let (rows, done, total, label) = {
            let guard = lock(&self.matrix);
            (guard.rows.clone(), guard.done, guard.total, guard.mode_label.clone())
        };

        if total > 0 {
            let ok = rows.iter().filter(|r| matches!(r.state, RowState::Ok { .. })).count();
            let fail = rows.iter().filter(|r| matches!(r.state, RowState::Fail { .. })).count();
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::ProgressBar::new(done as f32 / total as f32).desired_width(160.0).text(format!("{done}/{total}")));
                badge(ui, &format!("{ok} OK"), theme_ok());
                badge(ui, &format!("{fail} fallidos"), palette.danger);
                ui.label(egui::RichText::new(label).small().color(palette.dim));
                if ui.small_button("copiar resultados").clicked() {
                    let mut text = String::new();
                    for row in &rows {
                        let (tag, detail, ms) = match &row.state {
                            RowState::Pending => ("PENDIENTE", String::new(), 0),
                            RowState::Running => ("CORRIENDO", String::new(), 0),
                            RowState::Ok { detail, ms } => ("OK", detail.clone(), *ms),
                            RowState::Fail { detail, ms } => ("FALLO", detail.clone(), *ms),
                        };
                        let _ = writeln!(text, "{}\t{tag}\t{ms} ms\t{detail}", row.profile);
                    }
                    ui.ctx().copy_text(text);
                }
            });
        }

        let mut use_profile: Option<String> = None;
        inset_panel(ui, palette, |ui| {
            egui::ScrollArea::vertical().id_salt("dev_matrix_rows").auto_shrink([false, false]).show(ui, |ui| {
                egui::Grid::new("dev_matrix_grid").striped(true).num_columns(4).spacing([14.0, 5.0]).show(ui, |ui| {
                    for row in &rows {
                        if let Some(family) = self.matrix_family {
                            if family_of(&row.profile) != family {
                                continue;
                            }
                        }
                        if self.matrix_only_failed && !matches!(row.state, RowState::Fail { .. }) {
                            continue;
                        }
                        ui.label(egui::RichText::new(&row.profile).monospace());
                        match &row.state {
                            RowState::Pending => {
                                ui.weak("pendiente");
                                ui.label("");
                                ui.label("");
                            }
                            RowState::Running => {
                                ui.horizontal(|ui| {
                                    theme::spinner(ui, 14.0, palette.accent);
                                    ui.label("probando…");
                                });
                                ui.label("");
                                ui.label("");
                            }
                            RowState::Ok { detail, ms } => {
                                badge(ui, "OK", theme_ok());
                                ui.label(format!("{ms} ms"));
                                ui.horizontal(|ui| {
                                    ui.label(detail);
                                    if ui.small_button("usar").clicked() {
                                        use_profile = Some(row.profile.clone());
                                    }
                                });
                            }
                            RowState::Fail { detail, ms } => {
                                badge(ui, "FALLO", palette.danger);
                                ui.label(format!("{ms} ms"));
                                ui.horizontal(|ui| {
                                    ui.label(detail);
                                    if ui.small_button("usar").clicked() {
                                        use_profile = Some(row.profile.clone());
                                    }
                                });
                            }
                        }
                        ui.end_row();
                    }
                });
                if rows.is_empty() {
                    ui.weak("Todavía no se probó nada.");
                }
            });
        });
        if let Some(profile) = use_profile {
            self.cfg.profile = profile;
        }
    }

    fn start_matrix(&mut self, ctx: &egui::Context) {
        let mode = self.matrix_mode;
        let spec = HttpRequestSpec {
            method: self.method.clone(),
            url: match mode {
                MatrixMode::Http => self.http_url.clone(),
                MatrixMode::WebSocket => self.ws_url.clone(),
            },
            body: self.http_body.clone(),
        };
        let base = self.cfg.clone();
        let names: Vec<String> = Profile::VARIANTS.iter().map(|p| profile_name(*p)).collect();

        {
            let mut guard = lock(&self.matrix);
            guard.rows = names.iter().map(|n| MatrixRow { profile: n.clone(), state: RowState::Pending }).collect();
            guard.running = true;
            guard.done = 0;
            guard.total = names.len();
            guard.mode_label = format!(
                "{} {} · {}",
                match mode {
                    MatrixMode::Http => spec.method.as_str(),
                    MatrixMode::WebSocket => "WS",
                },
                spec.url,
                base.platform
            );
        }
        self.matrix_cancel.store(false, Ordering::Relaxed);

        let shared = Arc::clone(&self.matrix);
        let cancel = Arc::clone(&self.matrix_cancel);
        let ctx = ctx.clone();
        runtime().spawn(async move {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(MATRIX_CONCURRENCY));
            let mut tasks = Vec::with_capacity(names.len());
            for (index, name) in names.into_iter().enumerate() {
                let (shared, cancel, ctx, semaphore) =
                    (Arc::clone(&shared), Arc::clone(&cancel), ctx.clone(), Arc::clone(&semaphore));
                let (mut cfg, spec) = (base.clone(), spec.clone());
                cfg.profile = name;
                tasks.push(tokio::spawn(async move {
                    let Ok(_permit) = semaphore.acquire_owned().await else { return };
                    if cancel.load(Ordering::Relaxed) {
                        let mut guard = lock(&shared);
                        guard.rows[index].state = RowState::Fail { detail: "cancelado".to_owned(), ms: 0 };
                        guard.done += 1;
                        return;
                    }
                    lock(&shared).rows[index].state = RowState::Running;
                    ctx.request_repaint();
                    let (ok, detail, ms) = probe_one(mode, cfg, spec).await;
                    let mut guard = lock(&shared);
                    guard.rows[index].state = if ok { RowState::Ok { detail, ms } } else { RowState::Fail { detail, ms } };
                    guard.done += 1;
                    drop(guard);
                    ctx.request_repaint();
                }));
            }
            for task in tasks {
                let _ = task.await;
            }
            lock(&shared).running = false;
            ctx.request_repaint();
        });
    }

    // -- Perfil / headers -------------------------------------------------------

    fn profile_tab(&mut self, ui: &mut egui::Ui) {
        // Dos columnas en vez de una lista larga: emulación y perfiles a la
        // izquierda, tabla de headers a la derecha.
        StripBuilder::new(ui)
            .size(Size::relative(0.4).at_least(280.0))
            .size(Size::exact(10.0))
            .size(Size::remainder().at_least(300.0))
            .horizontal(|mut strip| {
                strip.cell(|ui| {
                    egui::ScrollArea::vertical().id_salt("dev_profile_left").auto_shrink([false, false]).show(ui, |ui| {
                        self.emulation_ui(ui);
                        ui.add_space(6.0);
                        card(ui, "Perfiles guardados", |ui| self.saved_profiles_ui(ui));
                        ui.add_space(6.0);
                        if ui
                            .checkbox(&mut self.remember_flag, "Recordar activado en próximos arranques")
                            .on_hover_text("Guarda la opción use_developer_features.")
                            .changed()
                        {
                            let _ =
                                web_local_storage_api::set_item(STORAGE_FLAG, if self.remember_flag { "true" } else { "false" });
                        }
                    });
                });
                strip.empty();
                strip.cell(|ui| {
                    egui::ScrollArea::vertical().id_salt("dev_profile_right").auto_shrink([false, false]).show(ui, |ui| {
                        card(ui, "Headers propios", |ui| self.headers_editor(ui));
                    });
                });
            });
    }

    fn emulation_ui(&mut self, ui: &mut egui::Ui) {
        card(ui, "Emulación", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Profile:");
                ui.add(egui::TextEdit::singleline(&mut self.profile_filter).desired_width(90.0).hint_text("filtrar…"));
                let filter = self.profile_filter.trim().to_ascii_lowercase();
                egui::ComboBox::from_id_salt("dev_profile_combo")
                    .selected_text(self.cfg.profile.clone())
                    .width(150.0)
                    .height(360.0)
                    .show_ui(ui, |ui| {
                        for profile in Profile::VARIANTS {
                            let name = profile_name(*profile);
                            if !filter.is_empty() && !name.to_ascii_lowercase().contains(&filter) {
                                continue;
                            }
                            ui.selectable_value(&mut self.cfg.profile, name.clone(), name);
                        }
                    });
                ui.label("Plataforma:");
                egui::ComboBox::from_id_salt("dev_platform_combo")
                    .selected_text(self.cfg.platform.clone())
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        for platform in Platform::VARIANTS {
                            let name = platform_name(*platform);
                            ui.selectable_value(&mut self.cfg.platform, name.clone(), name);
                        }
                    });
            });
            ui.checkbox(&mut self.cfg.http2, "HTTP/2 propio del profile")
                .on_hover_text("SETTINGS, orden de pseudo-headers…");
            ui.checkbox(&mut self.cfg.emu_headers, "Headers propios del profile").on_hover_text(
                "User-Agent, sec-ch-ua, Accept…\n\
                 Apagados, salen SOLO los de la tabla. Prendidos, los de la tabla se agregan \
                 al pedido (para reemplazar uno, apagá los del profile).",
            );
        });
    }

    fn headers_editor(&mut self, ui: &mut egui::Ui) {
        let mut remove: Option<usize> = None;
        let value_width = (ui.available_width() - 150.0 - 90.0).max(140.0);
        egui::Grid::new("dev_headers_grid").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
            for (index, row) in self.cfg.headers.iter_mut().enumerate() {
                ui.checkbox(&mut row.on, "");
                ui.add(egui::TextEdit::singleline(&mut row.name).desired_width(150.0).hint_text("Nombre").font(
                    egui::TextStyle::Monospace,
                ));
                ui.add(egui::TextEdit::singleline(&mut row.value).desired_width(value_width).hint_text("Valor").font(
                    egui::TextStyle::Monospace,
                ));
                if ui.small_button("✕").on_hover_text("Quitar").clicked() {
                    remove = Some(index);
                }
                ui.end_row();
            }
        });
        if let Some(index) = remove {
            self.cfg.headers.remove(index);
        }
        if self.cfg.headers.is_empty() {
            ui.weak("Sin headers propios.");
        }

        ui.horizontal_wrapped(|ui| {
            if ui.button("+ Header").clicked() {
                self.cfg.headers.push(HeaderRow::new("", ""));
            }
            if ui.button("Limpiar").clicked() {
                self.cfg.headers.clear();
            }
            egui::ComboBox::from_id_salt("dev_headers_presets").selected_text("Agregar preset").show_ui(ui, |ui| {
                if ui.selectable_label(false, "Discord web (Origin, Referer…)").clicked() {
                    for (name, value) in [
                        ("Origin", "https://discord.com"),
                        ("Referer", "https://discord.com/channels/@me"),
                        ("Accept-Language", "en-US,en;q=0.9"),
                        ("Cache-Control", "no-cache"),
                        ("Pragma", "no-cache"),
                    ] {
                        self.cfg.headers.push(HeaderRow::new(name, value));
                    }
                }
                if ui.selectable_label(false, "Solo Origin").clicked() {
                    self.cfg.headers.push(HeaderRow::new("Origin", "https://discord.com"));
                }
                if ui.selectable_label(false, "User-Agent propio").clicked() {
                    self.cfg.headers.push(HeaderRow::new("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)"));
                }
                if ui.selectable_label(false, "Authorization (pegá un token a mano)").clicked() {
                    self.cfg.headers.push(HeaderRow::new("Authorization", ""));
                }
            });
        });

        ui.collapsing("Pegar headers en bruto", |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.raw_headers)
                    .desired_rows(5)
                    .desired_width(f32::INFINITY)
                    .code_editor()
                    .hint_text("Nombre: valor — uno por línea (como los copia el navegador o un proxy)"),
            );
            ui.horizontal(|ui| {
                if ui.button("Agregar a la tabla").clicked() {
                    self.cfg.headers.extend(parse_raw_headers(&self.raw_headers));
                    self.raw_headers.clear();
                }
                if ui.button("Reemplazar la tabla").clicked() {
                    self.cfg.headers = parse_raw_headers(&self.raw_headers);
                    self.raw_headers.clear();
                }
            });
        });
    }

    fn saved_profiles_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_profile_name)
                    .desired_width((ui.available_width() - 120.0).max(90.0))
                    .hint_text("Nombre del perfil"),
            );
            let name = self.new_profile_name.trim().to_owned();
            if ui.add_enabled(!name.is_empty(), egui::Button::new("Guardar actual")).clicked() {
                let cfg = self.cfg.clone();
                if let Some(slot) = self.saved_profiles.iter_mut().find(|(n, _)| *n == name) {
                    slot.1 = cfg;
                } else {
                    self.saved_profiles.push((name, cfg));
                }
                self.persist_profiles();
            }
        });

        let mut load: Option<usize> = None;
        let mut delete: Option<usize> = None;
        for (index, (name, cfg)) in self.saved_profiles.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(name).strong()).on_hover_text(cfg.summary());
                if ui.small_button("Cargar").clicked() {
                    load = Some(index);
                }
                if ui.small_button("Borrar").clicked() {
                    delete = Some(index);
                }
            });
        }
        if let Some(index) = load {
            self.cfg = self.saved_profiles[index].1.clone();
        }
        if let Some(index) = delete {
            self.saved_profiles.remove(index);
            self.persist_profiles();
        }
        if self.saved_profiles.is_empty() {
            ui.weak("Todavía no guardaste ninguno.");
        }
    }

    fn persist_profiles(&self) {
        if let Ok(json) = serde_json::to_string(&self.saved_profiles) {
            let _ = web_local_storage_api::set_item(STORAGE_PROFILES, &json);
        }
    }
}

impl Drop for DevTools {
    fn drop(&mut self) {
        if let Some(tx) = &self.ws.tx {
            let _ = tx.send(WsCmd::Close);
        }
        self.matrix_cancel.store(true, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Ventana flotante y ventana aparte
// ---------------------------------------------------------------------------

/// Mantiene la ventana flotante dentro de lo razonable: no más chica que el mínimo,
/// no más grande que la pantalla y con un pedazo de la barra superior siempre a la
/// vista (para poder agarrarla y traerla de vuelta).
fn clamp_floating(rect: egui::Rect, screen: egui::Rect) -> egui::Rect {
    let width = rect.width().max(MIN_W).min(screen.width().max(1.0));
    let height = rect.height().max(MIN_H).min(screen.height().max(1.0));
    let x = rect.min.x.max(screen.left() + 120.0 - width).min(screen.right() - 120.0);
    let y = rect.min.y.max(screen.top()).min(screen.bottom() - TOP_H);
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height))
}

/// Lo que `Context::show_viewport_deferred` le entrega al callback de la ventana aparte
/// (según la versión de egui es un `&Context` o un `&mut Ui`). Este trait deja dibujar el
/// panel igual con cualquiera de los dos: `paint` da siempre un `Ui` que ocupa toda la
/// ventana y su `Context`.
trait ViewportSurface {
    fn paint(self, add: impl FnOnce(&mut egui::Ui, &egui::Context));
}

impl ViewportSurface for &egui::Context {
    fn paint(self, add: impl FnOnce(&mut egui::Ui, &egui::Context)) {
        let rect = self.viewport_rect();
        egui::Area::new(egui::Id::new("ecord_devtools_detached"))
            .order(egui::Order::Background)
            .fixed_pos(rect.min)
            .show(self, |ui| add(ui, self));
    }
}

impl ViewportSurface for &mut egui::Ui {
    fn paint(self, add: impl FnOnce(&mut egui::Ui, &egui::Context)) {
        let ctx = self.ctx().clone();
        add(self, &ctx);
    }
}

// ---------------------------------------------------------------------------
// Colores
// ---------------------------------------------------------------------------

fn line_tag(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Info => "··",
        LineKind::Out => "→ ",
        LineKind::In => "← ",
        LineKind::Error => "!!",
    }
}

fn theme_ok() -> egui::Color32 {
    egui::Color32::from_rgb(0x57, 0xC2, 0x7D)
}

fn theme_error() -> egui::Color32 {
    egui::Color32::from_rgb(0xE5, 0x5B, 0x5B)
}

fn theme_warn() -> egui::Color32 {
    egui::Color32::from_rgb(0xE5, 0xB8, 0x4B)
}

// ---------------------------------------------------------------------------
// Piezas de interfaz compartidas
// ---------------------------------------------------------------------------

/// Texto del comando `op`: sin argumentos, un resumen; con un número, el
/// detalle de ese opcode del Gateway; con texto, los que lo contengan.
fn opcode_lines(query: &str) -> Vec<String> {
    use crate::discord::gateway_tracking;

    fn describe(op: &opcodes::Op) -> Vec<String> {
        let mut lines = vec![format!(
            "op {} · {} · {}{}",
            op.code,
            op.name,
            op.dir.label(),
            if op.deprecated { " · obsoleto" } else { "" }
        )];
        lines.push(format!("  en eCord: {}", op.ecord.unwrap_or("todavía no implementado")));
        if !op.note.is_empty() {
            lines.push(format!("  {}", op.note));
        }
        if let Some(template) = op.template {
            lines.push(format!("  plantilla: {template}"));
        }
        lines
    }

    let query = query.trim();
    if query.is_empty() {
        let missing = opcodes::missing(&opcodes::GATEWAY);
        return vec![
            format!(
                "Gateway: {} opcodes, {} implementados en eCord, {missing} sin implementar.",
                opcodes::GATEWAY.len(),
                opcodes::GATEWAY.len() - missing
            ),
            "Uso: op <número> (p. ej. op 41) u op <texto> (p. ej. op stream). Más detalle en Cliente → Opcodes.".to_owned(),
        ];
    }

    let matches: Vec<&opcodes::Op> = match query.parse::<u8>() {
        Ok(code) => opcodes::GATEWAY.iter().filter(|op| op.code == code).collect(),
        Err(_) => {
            let needle = query.to_lowercase();
            opcodes::GATEWAY.iter().filter(|op| op.name.to_lowercase().contains(&needle)).collect()
        }
    };
    if matches.is_empty() {
        return vec![format!("Ningún opcode del Gateway coincide con \"{query}\".")];
    }

    let mut lines = Vec::new();
    for op in matches {
        lines.extend(describe(op));
        if op.code == 41 {
            let snap = gateway_tracking::snapshot();
            lines.push(match (snap.connected, snap.time_spent_next_at) {
                (true, Some(next)) => format!(
                    "  próximo tick en {} · {} enviados, {} omitidos",
                    client::fmt_duration(next.saturating_duration_since(Instant::now())),
                    snap.time_spent_sent,
                    snap.time_spent_skipped
                ),
                _ => "  sin conexión con el Gateway: no hay cuenta regresiva.".to_owned(),
            });
        }
    }
    lines
}

/// Bloque con título sobre la superficie de la paleta: agrupa una sección.
fn card<R>(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let palette = theme::current(ui.ctx());
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.outline))
        .corner_radius(egui::CornerRadius::same(theme::radius_small() + 2))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::text(ui, title, theme::semibold(12.5), palette.secondary);
            ui.add_space(4.0);
            add_contents(ui)
        })
        .inner
}

/// Barra redondeada para una fila de controles (método · URL · acciones).
fn bar<R>(ui: &mut egui::Ui, palette: &Palette, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.outline))
        .corner_radius(egui::CornerRadius::same(theme::radius()))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| add_contents(ui)).inner
        })
        .inner
}

/// Panel hundido para respuestas, logs y resultados: ocupa el alto que sobra.
fn inset_panel<R>(ui: &mut egui::Ui, palette: &Palette, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::inset(palette))
        .corner_radius(egui::CornerRadius::same(theme::radius_small() + 2))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(ui.available_height().max(60.0));
            add_contents(ui)
        })
        .inner
}

/// Etiqueta de color con fondo tenue (estado, veredicto, contadores).
fn badge(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(color).strong().small());
        });
}

/// Botón principal de la paleta; `true` si se pulsó (y estaba habilitado).
fn primary_button(ui: &mut egui::Ui, palette: &Palette, label: &str, enabled: bool) -> bool {
    ui.add_enabled_ui(enabled, |ui| theme::pill_button(ui, palette, label, true)).inner.clicked()
}

fn copy_button(ui: &mut egui::Ui, text: &str) {
    let button = egui::Button::new("copiar").small().wrap_mode(egui::TextWrapMode::Extend);
    if ui.add(button).on_hover_text("Copiar al portapapeles").clicked() {
        ui.ctx().copy_text(text.to_owned());
    }
}

/// Fila de una tabla de 3 columnas: nombre · copiar · valor. El valor va al
/// final para poder ocupar (y partirse en) el ancho que sobra.
fn kv_row(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    // Nombre y botón nunca se truncan ni se parten (el valor, a la derecha, es el que se ajusta).
    ui.add(egui::Label::new(egui::RichText::new(label).weak()).extend());
    match value {
        Some(value) => {
            copy_button(ui, value);
            // Se parte en varias líneas solo si no entra: nunca se corta con "…".
            ui.add(egui::Label::new(egui::RichText::new(value).monospace()).wrap().selectable(true));
        }
        None => {
            ui.label("");
            ui.colored_label(theme_warn(), "— sin valor —");
        }
    }
    ui.end_row();
}

fn session_rows(ui: &mut egui::Ui, identity: &ClientIdentity) {
    egui::Grid::new("dev_ident_session").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
        kv_row(ui, "Installation ID", identity.installation_id.as_deref());
        kv_row(ui, "X-Fingerprint", identity.anonymous_fingerprint.as_deref());
        kv_row(ui, "Build number", Some(&identity.client_build_number.to_string()));
        kv_row(ui, "client_launch_id", Some(&identity.client_launch_id));
        kv_row(ui, "heartbeat_session_id", Some(&identity.client_heartbeat_session_id));
        kv_row(ui, "User-Agent", Some(&identity.user_agent));
    });
    ui.weak("El Installation ID viaja en el header X-Installation-ID y en el IDENTIFY del Gateway.");
}

/// Campos de `X-Super-Properties` en el orden en que se envían (un `serde_json::Value`
/// los reordenaría alfabéticamente).
struct OrderedFields(Vec<(String, serde_json::Value)>);

impl<'de> Deserialize<'de> for OrderedFields {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldsVisitor;

        impl<'de> serde::de::Visitor<'de> for FieldsVisitor {
            type Value = OrderedFields;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("un objeto JSON")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, serde_json::Value>()? {
                    fields.push((key, value));
                }
                Ok(OrderedFields(fields))
            }
        }

        deserializer.deserialize_map(FieldsVisitor)
    }
}

fn parse_super_properties(json: &str) -> Result<Vec<(String, serde_json::Value)>, String> {
    serde_json::from_str::<OrderedFields>(json).map(|fields| fields.0).map_err(|error| error.to_string())
}

fn super_properties_section(
    ui: &mut egui::Ui,
    identity: &ClientIdentity,
    fields: &Result<Vec<(String, serde_json::Value)>, String>,
) {
    ui.horizontal_wrapped(|ui| {
        if ui.button("Copiar base64").clicked() {
            ui.ctx().copy_text(identity.super_properties_b64.clone());
        }
        if ui.button("Copiar JSON").clicked() {
            ui.ctx().copy_text(identity.super_properties_json.clone());
        }
        let count = fields.as_ref().map_or(0, |fields| fields.len());
        ui.weak(format!("{count} campos · {} bytes de JSON", identity.super_properties_json.len()));
    });
    ui.add_space(2.0);

    match fields {
        Err(error) => {
            ui.colored_label(theme_error(), format!("No se pudo decodificar: {error}"));
        }
        Ok(fields) => {
            egui::Grid::new("dev_ident_props").striped(true).num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
                for (key, value) in fields {
                    ui.label(egui::RichText::new(key).monospace().strong());
                    prop_value(ui, key, value);
                    ui.end_row();
                }
            });
        }
    }

    ui.add_space(2.0);
    egui::CollapsingHeader::new("Base64 (valor del header)").id_salt("dev_ident_b64").show(ui, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(&identity.super_properties_b64).monospace()).wrap().selectable(true));
    });
}

fn prop_value(ui: &mut egui::Ui, key: &str, value: &serde_json::Value) {
    match value {
        serde_json::Value::Null => {
            ui.weak("null");
        }
        serde_json::Value::Bool(flag) if key == "has_client_mods" => {
            ui.colored_label(if *flag { theme_error() } else { theme_ok() }, flag.to_string());
        }
        serde_json::Value::String(text) if text.is_empty() => {
            ui.weak("\"\"");
        }
        serde_json::Value::String(text) => {
            ui.add(egui::Label::new(egui::RichText::new(text).monospace()).wrap().selectable(true));
        }
        other => {
            ui.add(egui::Label::new(egui::RichText::new(other.to_string()).monospace()).wrap().selectable(true));
        }
    }
}

fn signature_section(
    ui: &mut egui::Ui,
    identity: &ClientIdentity,
    report: &Result<LaunchSignatureReport, LaunchSignatureError>,
    declares_mods: Option<bool>,
    other: &mut String,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("En uso:").weak());
        ui.add(egui::Label::new(egui::RichText::new(&identity.launch_signature).monospace()).selectable(true));
        copy_button(ui, &identity.launch_signature);
    });

    match report {
        Err(error) => {
            ui.colored_label(theme_error(), format!("Firma inválida: {error}"));
        }
        Ok(report) => {
            report_ui(ui, report, "dev_sig_live");
            // `has_client_mods` del super-properties tiene que coincidir con lo que dice la firma.
            if let Some(declared) = declares_mods {
                let consistent = declared == !report.is_clean();
                let text = format!(
                    "has_client_mods = {declared} · firma {} → {}",
                    if report.is_clean() { "limpia" } else { "con mods" },
                    if consistent { "coherente" } else { "INCOHERENTE" },
                );
                ui.colored_label(if consistent { theme_ok() } else { theme_error() }, text);
            }
        }
    }

    ui.add_space(2.0);
    egui::CollapsingHeader::new("Analizar otra firma").id_salt("dev_sig_other").show(ui, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut *other)
                .desired_width(f32::INFINITY)
                .font(egui::TextStyle::Monospace)
                .hint_text("xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"),
        );
        if !other.trim().is_empty() {
            match launch_signature::analyze(other.as_str()) {
                Ok(report) => report_ui(ui, &report, "dev_sig_other_report"),
                Err(error) => {
                    ui.colored_label(theme_error(), format!("Firma inválida: {error}"));
                }
            }
        }
    });
}

/// Veredicto, mods detectados y detalle de bytes/bits de una firma ya analizada.
fn report_ui(ui: &mut egui::Ui, report: &LaunchSignatureReport, salt: &str) {
    if report.is_clean() {
        badge(ui, "CLIENTE LIMPIO", theme_ok());
    } else {
        badge(ui, "CLIENTE CON MODS", theme_error());
    }

    if report.detected.is_empty() {
        ui.weak("Mods detectados: ninguno");
    } else {
        ui.label(format!("Mods detectados ({}):", report.detected.len()));
        for client_mod in &report.detected {
            ui.colored_label(theme_error(), format!("✗ {} (bit {})", client_mod.name, client_mod.id));
        }
    }
    if !report.reserved_set.is_empty() {
        let ids = report.reserved_set.iter().map(u8::to_string).collect::<Vec<_>>().join(", ");
        ui.colored_label(theme_warn(), format!("Bits reservados encendidos: {ids} (no corresponden a un cliente web limpio)"));
    }

    egui::CollapsingHeader::new(format!("Detalle ({} mods comprobados)", launch_signature::CLIENT_MODS.len()))
        .id_salt(salt)
        .show(ui, |ui| {
            let mono = |text: String| egui::Label::new(egui::RichText::new(text).monospace()).wrap().selectable(true);
            ui.add(mono(format!("Bytes:        {}", report.bytes_hex())));
            ui.add(mono(format!("Bytes 0–7:    0x{:016x}  (u64 LE)", report.low_bitfield())));
            ui.add(mono(format!("Bytes 8–15:   0x{:016x}  (u64 LE)", report.high_bitfield())));
            ui.weak("Los mods con id ≥ 64 (Legcord en adelante) viven en los bytes 8–15.");
            ui.add_space(2.0);
            egui::Grid::new(format!("{salt}_grid")).striped(true).num_columns(3).spacing([12.0, 2.0]).show(ui, |ui| {
                for client_mod in &launch_signature::CLIENT_MODS {
                    let present = report.detected.contains(client_mod);
                    ui.label(client_mod.name);
                    ui.label(egui::RichText::new(format!("bit {}", client_mod.id)).monospace().weak());
                    if present {
                        ui.colored_label(theme_error(), "detectado");
                    } else {
                        ui.colored_label(theme_ok(), "ausente");
                    }
                    ui.end_row();
                }
            });
        });
}

