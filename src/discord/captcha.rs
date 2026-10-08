//! hCaptcha resuelto por la persona, para cualquier request que Discord
//! rechace pidiendo un captcha (login, mandar mensajes, unirse a un server,
//! abrir un DM...).
//!
//! ## Cómo encaja
//!
//! ```text
//!   request ──► Discord: 400 + captcha_sitekey/rqdata/rqtoken
//!      ▲                         │
//!      │                         ▼
//!      │              captcha::solve(challenge)   (hilo de fondo, async)
//!      │                         │  encola el pedido y espera
//!      │                         ▼
//!      │        App::poll → CaptchaController::poll   (hilo de la UI)
//!      │                         │  abre la ventana (proceso hijo + WebView)
//!      │                         ▼
//!      │              la persona resuelve el hCaptcha
//!      │                         │  token por stdout → captcha::resolve
//!      └── reintento con X-Captcha-Key / X-Captcha-Rqtoken ◄──┘
//! ```
//!
//! * Los reintentos viven en un solo lugar por transporte:
//!   `uwu_rest::CaptchaClient` (todas las llamadas de `UwuRest` pasan por
//!   ahí) y `password_auth` (login/2FA, que usa `reqwest`). Ningún caller
//!   tiene que saber que existe el captcha: si la persona lo resuelve, la
//!   request simplemente termina bien; si cierra la ventana, falla con
//!   [`CaptchaCancelled`].
//! * El broker es global (como el fingerprint compartido de `uwu_rest`):
//!   así no hay que pasar un `Sender<AppEvent>` por cada `UwuRest`, y
//!   sobrevive a que el canal de eventos se recree en cada login.
//! * Un captcha a la vez: si llegan varios pedidos juntos se atienden en
//!   fila, cada uno con su ventana (el `rqtoken` es por request).
//!
//! ## Por qué un proceso hijo
//!
//! `wry`/`tao` necesitan crear su propio event loop, y `eframe` ya tiene el
//! suyo en el hilo principal (en macOS/Linux dos event loops en un proceso
//! directamente no conviven). En vez de pelear con eso, la ventana corre en
//! el mismo ejecutable lanzado con `--captcha-window`: `main()` detecta la
//! bandera, llama a [`run_window`] y nunca arranca eframe. El token vuelve
//! al proceso padre por stdout, en una línea `ECORD_CAPTCHA_TOKEN:<token>`.
//!
//! ## Cómo se muestra hCaptcha
//!
//! La sitekey de Discord solo funciona en páginas cuyo host sea de Discord,
//! así que no sirve un HTML local. La ventana abre `https://discord.com/login`
//! (cuya CSP ya permite hCaptcha) y un script de inicialización frena la
//! carga de la app de Discord y reemplaza el documento por el widget. El
//! script solo actúa en el frame principal de discord.com: los iframes de
//! hCaptcha (otro origen) no se tocan.
//!
//! ⚠️ No se pudo probar contra Discord real desde donde se escribió esto.
//! Si el widget no carga, la propia ventana muestra un aviso a los pocos
//! segundos (puede ser CSP o que Discord haya cambiado la página).

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use discord_client_rest::captcha::{CaptchaRequiredError, SolvedCaptcha};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::oneshot;

/// Bandera con la que `main()` reconoce que este proceso es la ventana del
/// captcha y no la app.
pub const WINDOW_FLAG: &str = "--captcha-window";

/// Cuántos captchas seguidos se le piden a la persona por una misma request
/// antes de darse por vencido (si Discord los rechaza todos, no tiene
/// sentido seguir abriendo ventanas).
pub const MAX_SOLVES_PER_REQUEST: usize = 3;

const TOKEN_PREFIX: &str = "ECORD_CAPTCHA_TOKEN:";
const ENV_SITEKEY: &str = "ECORD_CAPTCHA_SITEKEY";
const ENV_RQDATA: &str = "ECORD_CAPTCHA_RQDATA";
const ENV_USER_AGENT: &str = "ECORD_CAPTCHA_UA";
const HOST_PAGE: &str = "https://discord.com/login";

/// La persona cerró la ventana sin resolver el captcha (o no se pudo abrir).
#[derive(Debug)]
pub struct CaptchaCancelled;

impl std::fmt::Display for CaptchaCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Captcha cancelado: no se completó la verificación")
    }
}

impl std::error::Error for CaptchaCancelled {}

/// Desafío que pide Discord (campos `captcha_*` de la respuesta 400).
#[derive(Clone, Debug)]
pub struct CaptchaChallenge {
    pub sitekey: String,
    /// Datos extra de hCaptcha Enterprise; se pasan al widget con `setData`.
    pub rqdata: Option<String>,
    /// Identifica el desafío; hay que devolverlo en `X-Captcha-Rqtoken`.
    pub rqtoken: Option<String>,
    /// `captcha_session_id`; si viene, hay que devolverlo en
    /// `X-Captcha-Session-Id`.
    pub session_id: Option<String>,
}

impl CaptchaChallenge {
    /// Desde el error que devuelve el cliente vendorizado. `None` si el
    /// servicio no es hCaptcha (no hay widget que mostrar).
    pub fn from_vendor(error: &CaptchaRequiredError) -> Option<Self> {
        if error.captcha_sitekey.is_empty() {
            return None;
        }
        if !error.captcha_service.is_empty() && error.captcha_service != "hcaptcha" {
            return None;
        }
        Some(Self {
            sitekey: error.captcha_sitekey.clone(),
            rqdata: Some(error.captcha_rqdata.clone()).filter(|data| !data.is_empty()),
            rqtoken: Some(error.captcha_rqtoken.clone()).filter(|token| !token.is_empty()),
            session_id: Some(error.captcha_session_id.clone()).filter(|id| !id.is_empty()),
        })
    }
}

/// Respuesta del captcha ya resuelto, lista para reenviar la request.
#[derive(Clone, Debug)]
pub struct CaptchaSolution {
    pub token: String,
    pub rqtoken: Option<String>,
    pub session_id: Option<String>,
}

impl CaptchaSolution {
    /// En el formato que espera `RequestProperties::with_solved_captcha`
    /// (agrega `X-Captcha-Key` y `X-Captcha-Rqtoken`).
    pub fn to_vendor(&self) -> SolvedCaptcha {
        SolvedCaptcha::new(self.token.clone(), self.rqtoken.clone().unwrap_or_default())
            .with_session_id(self.session_id.clone().unwrap_or_default())
    }
}

/// Lee los campos `captcha_*` de un cuerpo de error de Discord (para los
/// pedidos hechos con `reqwest`, como el login). Según la doc, un captcha se
/// identifica por el 400 + la presencia de `captcha_key` (sin mirar su
/// contenido). `None` si no es un desafío de hCaptcha que se pueda mostrar.
pub(super) fn parse_challenge(body: &str) -> Option<CaptchaChallenge> {
    #[derive(Deserialize)]
    struct Raw {
        captcha_key: Option<Value>,
        captcha_sitekey: Option<String>,
        captcha_service: Option<String>,
        captcha_session_id: Option<String>,
        captcha_rqdata: Option<String>,
        captcha_rqtoken: Option<String>,
    }

    let raw: Raw = serde_json::from_str(body).ok()?;
    raw.captcha_key.as_ref()?;
    let sitekey = raw.captcha_sitekey.filter(|key| !key.is_empty())?;
    if raw.captcha_service.as_deref().is_some_and(|service| service != "hcaptcha") {
        return None;
    }
    Some(CaptchaChallenge {
        sitekey,
        rqdata: raw.captcha_rqdata.filter(|data| !data.is_empty()),
        rqtoken: raw.captcha_rqtoken.filter(|token| !token.is_empty()),
        session_id: raw.captcha_session_id.filter(|id| !id.is_empty()),
    })
}

// --- Broker global -----------------------------------------------------------

/// Un pedido de captcha esperando que la UI lo atienda.
pub struct CaptchaRequest {
    pub id: u64,
    pub challenge: CaptchaChallenge,
}

#[derive(Default)]
struct Broker {
    next_id: u64,
    /// Pedidos nuevos que la UI todavía no levantó.
    inbox: VecDeque<CaptchaRequest>,
    /// Quién espera la respuesta de cada pedido.
    waiting: HashMap<u64, oneshot::Sender<Option<CaptchaSolution>>>,
    /// Para despertar a la UI cuando llega algo (egui no repinta solo).
    repaint: Option<egui::Context>,
}

static BROKER: LazyLock<Mutex<Broker>> = LazyLock::new(|| Mutex::new(Broker::default()));

fn broker() -> std::sync::MutexGuard<'static, Broker> {
    BROKER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// La UI registra su contexto una vez, para que un pedido de captcha
/// (que llega desde un hilo de fondo) la despierte aunque esté quieta.
pub fn set_repaint_context(ctx: &egui::Context) {
    let mut broker = broker();
    if broker.repaint.is_none() {
        broker.repaint = Some(ctx.clone());
    }
}

fn wake_ui() {
    let ctx = broker().repaint.clone();
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
}

/// Pide que la persona resuelva `challenge` y espera la respuesta. Se llama
/// desde los hilos de fondo. `None` si cerró la ventana sin resolverlo.
pub async fn solve(challenge: CaptchaChallenge) -> Option<CaptchaSolution> {
    let (tx, rx) = oneshot::channel();
    {
        let mut broker = broker();
        broker.next_id += 1;
        let id = broker.next_id;
        broker.waiting.insert(id, tx);
        broker.inbox.push_back(CaptchaRequest { id, challenge });
    }
    wake_ui();
    rx.await.ok().flatten()
}

/// Entrega la respuesta de un pedido (o `None` si se canceló). Idempotente:
/// si ya se resolvió, no hace nada.
pub fn resolve(id: u64, solution: Option<CaptchaSolution>) {
    let waiter = broker().waiting.remove(&id);
    if let Some(waiter) = waiter {
        let _ = waiter.send(solution);
    }
}

fn take_requests() -> Vec<CaptchaRequest> {
    broker().inbox.drain(..).collect()
}

// --- Ventana (lado app) -------------------------------------------------------

const RUNNING: u8 = 0;
const SOLVED: u8 = 1;
const CLOSED: u8 = 2;

/// Handle de la ventana abierta. Soltarlo (o `close`) la cierra.
pub struct CaptchaWindow {
    child: Arc<Mutex<Child>>,
    outcome: Arc<AtomicU8>,
}

impl CaptchaWindow {
    pub fn close(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }

    /// Ya terminó (resuelta o cerrada) y su respuesta ya se entregó.
    pub fn is_finished(&self) -> bool {
        self.outcome.load(Ordering::Acquire) != RUNNING
    }
}

impl Drop for CaptchaWindow {
    fn drop(&mut self) {
        self.close();
    }
}

/// Abre la ventana para el pedido `id`. Cuando termina entrega la respuesta
/// con [`resolve`] y despierta a la UI.
fn spawn_window(id: u64, challenge: &CaptchaChallenge) -> Result<CaptchaWindow, String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("No se pudo ubicar el ejecutable para abrir el captcha: {error}"))?;

    let mut command = Command::new(exe);
    command
        .arg(WINDOW_FLAG)
        .env(ENV_SITEKEY, &challenge.sitekey)
        .env(ENV_USER_AGENT, super::uwu_rest::shared_user_agent())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Los errores de la ventana (WebView2 ausente, etc.) salen por la
        // consola de la app, útil al probar con `cargo run`.
        .stderr(Stdio::inherit());
    if let Some(rqdata) = &challenge.rqdata {
        command.env(ENV_RQDATA, rqdata);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: que no aparezca una consola extra.
        command.creation_flags(0x0800_0000);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("No se pudo abrir la ventana del captcha: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "No se pudo leer la respuesta de la ventana del captcha".to_owned())?;
    let child = Arc::new(Mutex::new(child));
    let outcome = Arc::new(AtomicU8::new(RUNNING));

    let watcher = Arc::clone(&child);
    let watcher_outcome = Arc::clone(&outcome);
    let rqtoken = challenge.rqtoken.clone();
    let session_id = challenge.session_id.clone();
    std::thread::spawn(move || {
        let token = BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .find_map(|line| line.strip_prefix(TOKEN_PREFIX).map(str::to_owned))
            .filter(|token| !token.is_empty());

        // Esperar a que el proceso termine sin retener el candado (si no,
        // `CaptchaWindow::close` se quedaría esperando).
        loop {
            let finished = match watcher.lock() {
                Ok(mut child) => !matches!(child.try_wait(), Ok(None)),
                Err(_) => true,
            };
            if finished {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let solved = token.is_some();
        resolve(id, token.map(|token| CaptchaSolution { token, rqtoken, session_id }));
        watcher_outcome.store(if solved { SOLVED } else { CLOSED }, Ordering::Release);
        wake_ui();
    });

    Ok(CaptchaWindow { child, outcome })
}

/// Avisos de [`CaptchaController::poll`] para que `App` los muestre.
pub enum CaptchaNotice {
    /// Se abrió la ventana de un captcha.
    Opened,
    /// No se pudo abrir la ventana (el pedido se canceló).
    OpenFailed(String),
}

/// Lado UI del broker: levanta los pedidos, abre una ventana por vez y
/// limpia cuando terminan. `App` lo llama una vez por frame.
#[derive(Default)]
pub struct CaptchaController {
    queue: VecDeque<CaptchaRequest>,
    active: Option<(u64, CaptchaWindow)>,
}

impl CaptchaController {
    pub fn poll(&mut self) -> Vec<CaptchaNotice> {
        let mut notices = Vec::new();
        self.queue.extend(take_requests());

        if self.active.as_ref().is_some_and(|(_, window)| window.is_finished()) {
            self.active = None;
        }
        while self.active.is_none() {
            let Some(request) = self.queue.pop_front() else { break };
            match spawn_window(request.id, &request.challenge) {
                Ok(window) => {
                    self.active = Some((request.id, window));
                    notices.push(CaptchaNotice::Opened);
                }
                Err(message) => {
                    resolve(request.id, None);
                    notices.push(CaptchaNotice::OpenFailed(message));
                }
            }
        }
        notices
    }

    /// Hay un captcha abierto o en fila esperando a la persona.
    pub fn is_waiting(&self) -> bool {
        self.active.is_some() || !self.queue.is_empty()
    }

    /// Cancela todo lo pendiente (cierra la ventana; cada request falla con
    /// [`CaptchaCancelled`]).
    pub fn cancel_all(&mut self) {
        for request in self.queue.drain(..) {
            resolve(request.id, None);
        }
        if let Some((id, window)) = self.active.take() {
            resolve(id, None);
            drop(window);
        }
    }
}

// --- Ventana (proceso hijo) ---------------------------------------------------

/// Cuerpo del proceso hijo: abre la ventana, espera el token, lo imprime
/// por stdout y termina. Nunca vuelve.
pub fn run_window() -> ! {
    use std::io::Write;
    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;
    use wry::WebViewBuilder;

    let sitekey = std::env::var(ENV_SITEKEY).unwrap_or_default();
    let rqdata = std::env::var(ENV_RQDATA).ok();
    let user_agent = std::env::var(ENV_USER_AGENT).ok();

    let event_loop = EventLoopBuilder::<String>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("eCord — Verificación")
        .with_inner_size(LogicalSize::new(500.0, 680.0))
        .build(&event_loop)
        .expect("no se pudo crear la ventana del captcha");

    let script = init_script(&sitekey, rqdata.as_deref());
    let mut builder = WebViewBuilder::new()
        .with_url(HOST_PAGE)
        .with_initialization_script(&script)
        .with_ipc_handler(move |request: wry::http::Request<String>| {
            let _ = proxy.send_event(request.body().clone());
        });
    if let Some(user_agent) = &user_agent {
        builder = builder.with_user_agent(user_agent);
    }
    let webview = builder
        .build(&window)
        .expect("no se pudo crear el WebView del captcha (¿falta WebView2/webkit2gtk?)");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        // El WebView tiene que vivir mientras dure el event loop.
        let _keep_alive = &webview;
        match event {
            Event::UserEvent(token) => {
                println!("{TOKEN_PREFIX}{token}");
                let _ = std::io::stdout().flush();
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    })
}

/// Script que corre al crear cada documento: en la página de discord.com
/// (solo frame principal) frena la carga de la app, arma una página mínima
/// con el widget de hCaptcha y manda el token por `window.ipc`.
fn init_script(sitekey: &str, rqdata: Option<&str>) -> String {
    let sitekey = serde_json::to_string(sitekey).unwrap_or_else(|_| "\"\"".to_owned());
    let rqdata = match rqdata {
        Some(data) => serde_json::to_string(data).unwrap_or_else(|_| "null".to_owned()),
        None => "null".to_owned(),
    };
    format!(
        r#"(function () {{
  if (window.top !== window || location.hostname !== 'discord.com') return;
  var SITEKEY = {sitekey};
  var RQDATA = {rqdata};

  function boot() {{
    if (window.__ecordBooted) return;
    window.__ecordBooted = true;
    try {{ window.stop(); }} catch (e) {{}}
    document.open();
    document.write('<!doctype html><html><head><meta charset="utf-8"><title>eCord</title></head><body></body></html>');
    document.close();

    var style = document.createElement('style');
    style.textContent = 'html,body{{margin:0;height:100%;background:#1e1f22;color:#dbdee1;font-family:system-ui,Segoe UI,sans-serif}}' +
      'body{{display:flex;flex-direction:column;align-items:center;justify-content:center;gap:16px;text-align:center}}' +
      'p{{margin:0;padding:0 24px;font-size:14px}}';
    document.head.appendChild(style);

    var msg = document.createElement('p');
    msg.textContent = 'Necesitamos verificar que eres un humano.';
    document.body.appendChild(msg);
    var box = document.createElement('div');
    box.id = 'ecord-hcaptcha';
    document.body.appendChild(box);

    var widgetId = null;
    window.ecordCaptchaReady = function () {{
      try {{
        widgetId = hcaptcha.render('ecord-hcaptcha', {{
          sitekey: SITEKEY,
          theme: 'dark',
          callback: function (token) {{
            msg.textContent = 'Listo, volviendo a eCord…';
            window.ipc.postMessage(token);
          }},
          'expired-callback': function () {{ try {{ hcaptcha.reset(widgetId); }} catch (e) {{}} }},
          'error-callback': function (err) {{ msg.textContent = 'Error del captcha: ' + err; }}
        }});
        if (RQDATA) {{ hcaptcha.setData(widgetId, {{ rqdata: RQDATA }}); }}
      }} catch (e) {{
        msg.textContent = 'No se pudo mostrar el captcha: ' + e;
      }}
    }};

    var script = document.createElement('script');
    script.src = 'https://js.hcaptcha.com/1/api.js?render=explicit&onload=ecordCaptchaReady';
    script.async = true;
    script.onerror = function () {{ msg.textContent = 'No se pudo cargar hCaptcha (revisá tu conexión).'; }};
    document.head.appendChild(script);

    setTimeout(function () {{
      if (typeof hcaptcha === 'undefined') {{
        msg.textContent = 'hCaptcha no cargó. Cerrá esta ventana y probá de nuevo o usá el QR.';
      }}
    }}, 15000);
  }}

  if (document.readyState === 'loading') {{
    document.addEventListener('DOMContentLoaded', boot);
  }} else {{
    boot();
  }}
}})();"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hcaptcha_challenge() {
        let body = r#"{"captcha_key":["captcha-required"],"captcha_sitekey":"site","captcha_service":"hcaptcha","captcha_session_id":"sess","captcha_rqdata":"data","captcha_rqtoken":"token"}"#;
        let challenge = parse_challenge(body).expect("challenge should parse");
        assert_eq!(challenge.sitekey, "site");
        assert_eq!(challenge.rqdata.as_deref(), Some("data"));
        assert_eq!(challenge.rqtoken.as_deref(), Some("token"));
        assert_eq!(challenge.session_id.as_deref(), Some("sess"));
    }

    #[test]
    fn ignores_errors_that_are_not_a_showable_captcha() {
        assert!(parse_challenge(r#"{"code":50035,"message":"Invalid Form Body"}"#).is_none());
        assert!(parse_challenge(r#"{"captcha_key":["captcha-required"]}"#).is_none());
        assert!(
            parse_challenge(r#"{"captcha_key":["x"],"captcha_sitekey":"s","captcha_service":"recaptcha"}"#)
                .is_none()
        );
        assert!(parse_challenge("not json").is_none());
    }

    #[test]
    fn vendor_error_converts_and_tolerates_missing_fields() {
        let error: CaptchaRequiredError =
            serde_json::from_str(r#"{"captcha_key":["captcha-required"],"captcha_sitekey":"site"}"#)
                .expect("missing rqdata/rqtoken/service should still deserialize");
        let challenge = CaptchaChallenge::from_vendor(&error).expect("challenge");
        assert_eq!(challenge.sitekey, "site");
        assert!(challenge.rqdata.is_none() && challenge.rqtoken.is_none() && challenge.session_id.is_none());

        // `captcha_sitekey` es `?string`: un null no debe romper la deserialización.
        let nullable: CaptchaRequiredError =
            serde_json::from_str(r#"{"captcha_key":["x"],"captcha_sitekey":null,"captcha_service":"recaptcha_enterprise"}"#)
                .expect("null sitekey should deserialize");
        assert!(nullable.captcha_sitekey.is_empty());

        let recaptcha: CaptchaRequiredError = serde_json::from_str(
            r#"{"captcha_key":["x"],"captcha_sitekey":"s","captcha_service":"recaptcha"}"#,
        )
        .unwrap();
        assert!(CaptchaChallenge::from_vendor(&recaptcha).is_none());
    }

    #[test]
    fn broker_round_trip_and_cancel() {
        fn challenge(sitekey: &str) -> CaptchaChallenge {
            CaptchaChallenge { sitekey: sitekey.to_owned(), rqdata: None, rqtoken: Some("rq".to_owned()), session_id: None }
        }
        fn run_solve(challenge: CaptchaChallenge) -> std::thread::JoinHandle<Option<CaptchaSolution>> {
            std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap()
                    .block_on(solve(challenge))
            })
        }
        fn wait_for_request() -> CaptchaRequest {
            loop {
                if let Some(request) = take_requests().into_iter().next() {
                    return request;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        // Resuelto por la persona.
        let solver = run_solve(challenge("a"));
        let request = wait_for_request();
        assert_eq!(request.challenge.sitekey, "a");
        resolve(request.id, Some(CaptchaSolution { token: "tok".into(), rqtoken: Some("rq".into()), session_id: Some("sess".into()) }));
        let solution = solver.join().unwrap().expect("solved");
        assert_eq!(solution.token, "tok");
        assert_eq!(solution.to_vendor().rqtoken, "rq");
        assert_eq!(solution.to_vendor().session_id, "sess");

        // Cancelado.
        let waiter = run_solve(challenge("b"));
        let request = wait_for_request();
        assert_eq!(request.challenge.sitekey, "b");
        resolve(request.id, None);
        assert!(waiter.join().unwrap().is_none());

        // Resolver dos veces no rompe nada.
        resolve(request.id, None);
    }
}
