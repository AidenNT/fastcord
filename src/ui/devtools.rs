//! Herramientas de desarrollo: una ventana flotante para probar, a mano, cómo
//! se comporta la pila de red de eCord (`wreq` + `wreq-util`) contra Discord u
//! otro servidor.
//!
//! # Cómo se activa (`use_developer_features`)
//!
//! La ventana NO existe para un usuario normal. Solo se habilita si se cumple
//! alguna de estas (se lee una vez al arrancar, ver [`enabled`]):
//!
//! - variable de entorno `USE_DEVELOPER_FEATURES=1` (o `ECORD_USE_DEVELOPER_FEATURES=1`);
//! - argumento de línea de comandos `--use_developer_features` (o `--use-developer-features`);
//! - la opción persistida `use_developer_features` (se guarda desde la propia
//!   ventana con "Recordar activado", para no tener que repetir lo anterior).
//!
//! Ya habilitada, se abre/cierra con **Ctrl+Shift+D** o con el botón `DEV` de
//! la esquina inferior derecha.
//!
//! # Qué permite probar
//!
//! - **HTTP**: cualquier método/URL/headers/cuerpo con una conexión emulada.
//! - **WebSocket**: conectar, mandar texto y ver lo que llega (Gateway, QR, eco...).
//! - **Todos los profiles**: la misma prueba (HTTP o WebSocket) lanzada con
//!   CADA profile de `wreq-util` y una tabla de resultados, para ver cuáles
//!   acepta un servidor y cuáles rechaza (por ejemplo, un `403`).
//! - **Perfil / headers**: elegir profile y plataforma, apagar el HTTP/2 o los
//!   headers de la emulación, editar TODOS los headers a mano y guardar
//!   perfiles propios con nombre.
//!
//! Las pruebas corren en su propio runtime de tokio (`runtime`), aparte del
//! hilo de Discord, y avisan a la UI con `request_repaint`. Nada de esto toca
//! la sesión de Discord ni el token de la cuenta: salvo que el usuario ponga a
//! mano un header `Authorization`, los pedidos salen sin credenciales.

use std::{
    fmt::Write as _,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use wreq_util::{Emulation, Platform, Profile};

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
// Estado de la ventana
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Http,
    WebSocket,
    Matrix,
    Profile,
}

pub struct DevTools {
    open: bool,
    tab: Tab,
    /// Configuración activa, común a las cuatro pestañas.
    cfg: EmuConfig,

    // HTTP
    method: String,
    http_url: String,
    http_body: String,
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
            tab: Tab::Http,
            cfg: EmuConfig::default(),
            method: "GET".to_owned(),
            http_url: HTTP_PRESETS[0].1.to_owned(),
            http_body: String::new(),
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

impl DevTools {
    /// Se llama una vez por frame. No hace nada si las herramientas no están
    /// habilitadas (ver [`enabled`]).
    pub fn show(&mut self, ctx: &egui::Context) {
        if !enabled() {
            return;
        }

        let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::D);
        if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
            self.open = !self.open;
        }

        self.show_pill(ctx);
        if !self.open {
            return;
        }

        let mut open = self.open;
        egui::Window::new("Developer tools")
            .id(egui::Id::new("ecord_devtools_window"))
            .open(&mut open)
            .default_size([860.0, 620.0])
            .min_size([560.0, 360.0])
            .resizable(true)
            .collapsible(true)
            .show(ctx, |ui| self.ui(ui, ctx));
        self.open = open;

        // Mientras algo está corriendo, refrescar para mostrar el avance.
        let busy = lock(&self.http_out).running
            || lock(&self.ws.shared).connecting
            || lock(&self.ws.shared).connected
            || lock(&self.matrix).running;
        if busy {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    /// Botón pequeño `DEV` en la esquina inferior derecha.
    fn show_pill(&mut self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("ecord_devtools_pill"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-10.0, -10.0])
            .show(ctx, |ui| {
                let label = egui::RichText::new("DEV").small().strong();
                if ui.add(egui::Button::new(label).small()).on_hover_text("Developer tools (Ctrl+Shift+D)").clicked() {
                    self.open = !self.open;
                }
            });
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Http, "HTTP");
            ui.selectable_value(&mut self.tab, Tab::WebSocket, "WebSocket");
            ui.selectable_value(&mut self.tab, Tab::Matrix, "Todos los profiles");
            ui.selectable_value(&mut self.tab, Tab::Profile, "Perfil / headers");
        });
        ui.separator();

        if self.tab != Tab::Profile {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("Conexión:").strong());
                ui.label(self.cfg.summary());
                if ui.small_button("editar").clicked() {
                    self.tab = Tab::Profile;
                }
            });
            ui.add_space(4.0);
        }

        match self.tab {
            Tab::Http => self.http_tab(ui, ctx),
            Tab::WebSocket => self.ws_tab(ui, ctx),
            Tab::Matrix => self.matrix_tab(ui, ctx),
            Tab::Profile => self.profile_tab(ui),
        }
    }

    // -- HTTP ---------------------------------------------------------------

    fn http_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("dev_http_method").selected_text(self.method.clone()).width(90.0).show_ui(
                ui,
                |ui| {
                    for method in METHODS {
                        ui.selectable_value(&mut self.method, method.to_owned(), method);
                    }
                },
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.http_url)
                    .desired_width((ui.available_width() - 150.0).max(120.0))
                    .hint_text("https://..."),
            );
            egui::ComboBox::from_id_salt("dev_http_preset").selected_text("Presets").width(110.0).show_ui(ui, |ui| {
                for (label, url) in HTTP_PRESETS {
                    if ui.selectable_label(false, label).clicked() {
                        self.http_url = url.to_owned();
                    }
                }
            });
        });

        ui.collapsing("Cuerpo", |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.http_body)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .code_editor()
                    .hint_text("(vacío) — se ignora en GET y HEAD"),
            );
        });

        let running = lock(&self.http_out).running;
        ui.horizontal(|ui| {
            if ui.add_enabled(!running, egui::Button::new("Enviar")).clicked() {
                self.send_http(ctx);
            }
            if running {
                ui.spinner();
            }
            ui.checkbox(&mut self.show_resp_headers, "Mostrar headers de la respuesta");
        });
        ui.separator();

        let out = lock(&self.http_out);
        match &out.result {
            None => {
                ui.weak("Sin respuesta todavía.");
            }
            Some(Err(error)) => {
                ui.colored_label(theme_error(), "Falló:");
                egui::ScrollArea::vertical().id_salt("dev_http_err").max_height(f32::INFINITY).show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(error.as_str()).monospace()).selectable(true));
                });
            }
            Some(Ok(info)) => {
                let good = info.status.starts_with('2') || info.status.starts_with('3');
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        if good { theme_ok() } else { theme_error() },
                        egui::RichText::new(&info.status).strong(),
                    );
                    ui.label(format!("{} · {} ms · {} bytes", info.version, info.elapsed_ms, info.body_len));
                    if info.truncated {
                        ui.weak(format!("(se muestran {} KB)", MAX_BODY_SHOWN / 1024));
                    }
                    if ui.small_button("copiar cuerpo").clicked() {
                        ui.ctx().copy_text(info.body.clone());
                    }
                });
                egui::ScrollArea::vertical().id_salt("dev_http_resp").auto_shrink([false, false]).show(ui, |ui| {
                    if self.show_resp_headers {
                        egui::CollapsingHeader::new(format!("Headers ({})", info.headers.len()))
                            .id_salt("dev_http_resp_headers")
                            .default_open(true)
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

    fn ws_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (connecting, connected) = {
            let guard = lock(&self.ws.shared);
            (guard.connecting, guard.connected)
        };

        ui.horizontal(|ui| {
            ui.add_enabled(
                !connecting && !connected,
                egui::TextEdit::singleline(&mut self.ws_url)
                    .desired_width((ui.available_width() - 250.0).max(120.0))
                    .hint_text("wss://..."),
            );
            ui.add_enabled_ui(!connecting && !connected, |ui| {
                egui::ComboBox::from_id_salt("dev_ws_preset").selected_text("Presets").width(110.0).show_ui(ui, |ui| {
                    for (label, url) in WS_PRESETS {
                        if ui.selectable_label(false, label).clicked() {
                            self.ws_url = url.to_owned();
                        }
                    }
                });
            });
            if connected || connecting {
                if ui.button("Desconectar").clicked() {
                    if let Some(tx) = &self.ws.tx {
                        let _ = tx.send(WsCmd::Close);
                    }
                }
            } else if ui.button("Conectar").clicked() {
                self.connect_ws(ctx);
            }
            if connecting {
                ui.spinner();
            }
        });

        ui.horizontal(|ui| {
            let response = ui.add_enabled(
                connected,
                egui::TextEdit::singleline(&mut self.ws_input)
                    .desired_width((ui.available_width() - 150.0).max(120.0))
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
            if ui.button("Copiar registro").clicked() {
                let mut text = String::new();
                for line in &lock(&self.ws.shared).lines {
                    let _ = writeln!(text, "[{:>7} ms] {} {}", line.at_ms, line_tag(line.kind), line.text);
                }
                ui.ctx().copy_text(text);
            }
        });
        ui.separator();

        let guard = lock(&self.ws.shared);
        egui::ScrollArea::vertical().id_salt("dev_ws_log").auto_shrink([false, false]).stick_to_bottom(true).show(
            ui,
            |ui| {
                if guard.lines.is_empty() {
                    ui.weak("Sin actividad. Elegí un preset y tocá Conectar.");
                }
                for line in &guard.lines {
                    let color = match line.kind {
                        LineKind::Info => ui.visuals().weak_text_color(),
                        LineKind::Out => theme_out(),
                        LineKind::In => theme_ok(),
                        LineKind::Error => theme_error(),
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(format!("{:>7} ms", line.at_ms)).monospace().weak());
                        ui.label(egui::RichText::new(line_tag(line.kind)).monospace().color(color));
                        ui.add(egui::Label::new(egui::RichText::new(&line.text).monospace()).selectable(true));
                    });
                }
            },
        );
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

    fn matrix_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let running = lock(&self.matrix).running;

        ui.horizontal(|ui| {
            ui.label("Prueba:");
            ui.add_enabled_ui(!running, |ui| {
                ui.selectable_value(&mut self.matrix_mode, MatrixMode::Http, "HTTP");
                ui.selectable_value(&mut self.matrix_mode, MatrixMode::WebSocket, "WebSocket");
            });
            ui.separator();
            ui.label("Plataforma:");
            ui.label(egui::RichText::new(&self.cfg.platform).strong());
            ui.weak("(se cambia en Perfil / headers)");
        });

        match self.matrix_mode {
            MatrixMode::Http => {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("dev_matrix_method")
                        .selected_text(self.method.clone())
                        .width(90.0)
                        .show_ui(ui, |ui| {
                            for method in METHODS {
                                ui.selectable_value(&mut self.method, method.to_owned(), method);
                            }
                        });
                    ui.add(
                        egui::TextEdit::singleline(&mut self.http_url)
                            .desired_width((ui.available_width() - 20.0).max(120.0)),
                    );
                });
            }
            MatrixMode::WebSocket => {
                ui.add(egui::TextEdit::singleline(&mut self.ws_url).desired_width(f32::INFINITY));
            }
        }
        ui.weak("Usa los headers propios y las opciones de la conexión activa; el profile cambia en cada fila.");

        ui.horizontal(|ui| {
            if running {
                if ui.button("Cancelar").clicked() {
                    self.matrix_cancel.store(true, Ordering::Relaxed);
                }
                ui.spinner();
            } else if ui.button("Probar los profiles").clicked() {
                self.start_matrix(ctx);
            }

            ui.separator();
            ui.label("Familia:");
            egui::ComboBox::from_id_salt("dev_matrix_family")
                .selected_text(self.matrix_family.unwrap_or("Todas"))
                .width(90.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.matrix_family, None, "Todas");
                    for family in FAMILIES {
                        ui.selectable_value(&mut self.matrix_family, Some(family), family);
                    }
                });
            ui.checkbox(&mut self.matrix_only_failed, "Solo fallidos");
        });

        let (rows, done, total, label) = {
            let guard = lock(&self.matrix);
            (guard.rows.clone(), guard.done, guard.total, guard.mode_label.clone())
        };

        if total > 0 {
            let ok = rows.iter().filter(|r| matches!(r.state, RowState::Ok { .. })).count();
            let fail = rows.iter().filter(|r| matches!(r.state, RowState::Fail { .. })).count();
            ui.horizontal(|ui| {
                ui.add(egui::ProgressBar::new(done as f32 / total as f32).desired_width(180.0).text(format!("{done}/{total}")));
                ui.colored_label(theme_ok(), format!("{ok} OK"));
                ui.colored_label(theme_error(), format!("{fail} fallidos"));
                ui.weak(label);
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
        ui.separator();

        let mut use_profile: Option<String> = None;
        egui::ScrollArea::vertical().id_salt("dev_matrix_rows").auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("dev_matrix_grid").striped(true).num_columns(4).spacing([14.0, 4.0]).show(ui, |ui| {
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
                                ui.spinner();
                                ui.label("probando…");
                            });
                            ui.label("");
                            ui.label("");
                        }
                        RowState::Ok { detail, ms } => {
                            ui.colored_label(theme_ok(), "OK");
                            ui.label(format!("{ms} ms"));
                            ui.horizontal(|ui| {
                                ui.label(detail);
                                if ui.small_button("usar").clicked() {
                                    use_profile = Some(row.profile.clone());
                                }
                            });
                        }
                        RowState::Fail { detail, ms } => {
                            ui.colored_label(theme_error(), "FALLO");
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
        egui::ScrollArea::vertical().id_salt("dev_profile_scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.heading("Emulación");
            ui.horizontal_wrapped(|ui| {
                ui.label("Profile:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.profile_filter).desired_width(110.0).hint_text("filtrar…"),
                );
                let filter = self.profile_filter.trim().to_ascii_lowercase();
                egui::ComboBox::from_id_salt("dev_profile_combo")
                    .selected_text(self.cfg.profile.clone())
                    .width(190.0)
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
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for platform in Platform::VARIANTS {
                            let name = platform_name(*platform);
                            ui.selectable_value(&mut self.cfg.platform, name.clone(), name);
                        }
                    });
            });
            ui.checkbox(&mut self.cfg.http2, "Usar el HTTP/2 propio del profile (SETTINGS, orden de pseudo-headers…)");
            ui.checkbox(
                &mut self.cfg.emu_headers,
                "Mandar los headers propios del profile (User-Agent, sec-ch-ua, Accept…)",
            );
            ui.weak(
                "Con los headers del profile apagados salen SOLO los de la tabla. Con ellos prendidos, \
                 los de la tabla se agregan al pedido (para reemplazar uno, apagá los del profile).",
            );

            ui.add_space(8.0);
            ui.heading("Headers propios");
            self.headers_editor(ui);

            ui.add_space(8.0);
            ui.heading("Perfiles guardados");
            self.saved_profiles_ui(ui);

            ui.add_space(8.0);
            ui.separator();
            if ui
                .checkbox(&mut self.remember_flag, "Recordar activado (use_developer_features) para próximos arranques")
                .changed()
            {
                let _ = web_local_storage_api::set_item(STORAGE_FLAG, if self.remember_flag { "true" } else { "false" });
            }
        });
    }

    fn headers_editor(&mut self, ui: &mut egui::Ui) {
        let mut remove: Option<usize> = None;
        egui::Grid::new("dev_headers_grid").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
            for (index, row) in self.cfg.headers.iter_mut().enumerate() {
                ui.checkbox(&mut row.on, "");
                ui.add(egui::TextEdit::singleline(&mut row.name).desired_width(170.0).hint_text("Nombre").font(
                    egui::TextStyle::Monospace,
                ));
                ui.add(egui::TextEdit::singleline(&mut row.value).desired_width(360.0).hint_text("Valor").font(
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
            ui.add(egui::TextEdit::singleline(&mut self.new_profile_name).desired_width(200.0).hint_text("Nombre del perfil"));
            let name = self.new_profile_name.trim().to_owned();
            if ui.add_enabled(!name.is_empty(), egui::Button::new("Guardar el actual")).clicked() {
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
                ui.label(egui::RichText::new(name).strong());
                ui.weak(cfg.summary());
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

fn theme_out() -> egui::Color32 {
    egui::Color32::from_rgb(0x6C, 0xA8, 0xF0)
}
