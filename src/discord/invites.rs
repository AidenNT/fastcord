//! Invitaciones de Discord dentro del chat.
//!
//! Discord NO manda un embed para un link de invitación: el cliente oficial
//! detecta `discord.gg/<código>` en el texto, consulta `GET /invites/<código>`
//! y dibuja él mismo la tarjeta (servidor, en línea, miembros, botón...) o,
//! si la invitación venció o no existe, la tarjeta "Invitación no válida".
//!
//! Este módulo hace la parte de datos (la parte visual vive en
//! `ui::invite_card`):
//!
//! * [`extract_codes`] saca los códigos de invitación de un texto.
//! * [`lookup`] devuelve el estado de un código desde una caché global y,
//!   si todavía no se pidió, lanza el pedido en un hilo aparte (sin bloquear
//!   el frame) y pide un repintado cuando llega la respuesta.
//!
//! El token y la lista de servers donde ya estás se publican con
//! [`set_token`] / [`set_joined_guilds`] porque el render de mensajes no
//! recibe el estado de la app (y se dibuja muchas veces por segundo).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

/// Pedidos de invitación simultáneos como máximo (un canal con muchos links
/// no debe disparar decenas de requests juntos).
const MAX_IN_FLIGHT: usize = 4;
/// Un error de red (no "invitación inexistente") se reintenta pasado este
/// tiempo.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Tarjetas máximas por mensaje (Discord también acota).
const MAX_CODES_PER_MESSAGE: usize = 3;

/// Lo que se muestra de una invitación válida.
#[derive(Clone, Debug)]
pub struct InviteInfo {
    pub code: String,
    pub guild_id: String,
    /// Canal al que apunta la invitación (hace falta para unirse).
    pub channel_id: String,
    pub channel_type: u8,
    pub name: String,
    /// Ícono del server (`.gif` si es animado), ya como URL del CDN.
    pub icon_url: Option<String>,
    pub description: Option<String>,
    /// `approximate_presence_count`.
    pub online: Option<u64>,
    /// `approximate_member_count`.
    pub members: Option<u64>,
    pub community: bool,
    pub verified: bool,
    pub partnered: bool,
}

#[derive(Clone, Debug)]
pub enum InviteState {
    /// Pedido en curso.
    Loading,
    Ready(InviteInfo),
    /// Discord contestó que no existe / venció (404, código 10006).
    Invalid,
    /// No se pudo consultar (red, rate limit...). Se reintenta solo.
    Failed,
}

struct Entry {
    state: InviteState,
    at: Instant,
}

static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
static TOKEN: Mutex<Option<String>> = Mutex::new(None);
static JOINED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static SESSION_ID: Mutex<Option<String>> = Mutex::new(None);
static JOINS: OnceLock<Mutex<HashMap<String, JoinState>>> = OnceLock::new();

/// Estado del botón de la tarjeta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinState {
    Idle,
    /// Pedido en curso (incluye la espera a que se resuelva un captcha).
    Joining,
    /// Falló (se puede reintentar).
    Failed,
}

fn joins() -> std::sync::MutexGuard<'static, HashMap<String, JoinState>> {
    JOINS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Publica el `session_id` del gateway (lo pide `POST /invites/{code}`).
pub fn set_session_id(session_id: &str) {
    *SESSION_ID.lock().unwrap_or_else(|p| p.into_inner()) = Some(session_id.to_string());
}

pub fn join_state(code: &str) -> JoinState {
    joins().get(code).copied().unwrap_or(JoinState::Idle)
}

/// Une a la cuenta al server de `info` sin bloquear el frame. Si Discord
/// pide captcha, `UwuRest` abre la ventana y espera a la persona; al
/// terminar bien, `is_joined` pasa a `true` y la tarjeta dice "Ir al
/// servidor".
pub fn start_join(ctx: &egui::Context, info: &InviteInfo) {
    if join_state(&info.code) == JoinState::Joining {
        return;
    }
    let token = TOKEN.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let session_id = SESSION_ID.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let (Some(token), Some(session_id)) = (token, session_id.filter(|s| !s.is_empty())) else {
        log::warn!("No se puede unir a {}: falta la sesión del gateway", info.code);
        joins().insert(info.code.clone(), JoinState::Failed);
        return;
    };
    let (Ok(guild_id), Ok(channel_id)) = (info.guild_id.parse::<u64>(), info.channel_id.parse::<u64>()) else {
        log::warn!("La invitación {} no trae canal; no se puede unir", info.code);
        joins().insert(info.code.clone(), JoinState::Failed);
        return;
    };

    joins().insert(info.code.clone(), JoinState::Joining);
    let info = info.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let result: anyhow::Result<()> = (|| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            rt.block_on(async {
                let rest = super::uwu_rest::UwuRest::for_token(token).await?;
                rest.join_invite(&info.code, &session_id, guild_id, channel_id, info.channel_type)
                    .await
                    .map(|_| ())
            })
        })();
        match result {
            Ok(()) => {
                let cell = JOINED.get_or_init(|| Mutex::new(HashSet::new()));
                cell.lock().unwrap_or_else(|p| p.into_inner()).insert(info.guild_id.clone());
                joins().remove(&info.code);
            }
            Err(e) => {
                // Cerrar la ventana del captcha no es un error: vuelve "Unirse".
                if e.to_string() == super::captcha::CaptchaCancelled.to_string() {
                    joins().remove(&info.code);
                } else {
                    log::warn!("No se pudo unir a la invitación {}: {e}", info.code);
                    joins().insert(info.code.clone(), JoinState::Failed);
                }
            }
        }
        ctx.request_repaint();
    });
}

fn cache() -> std::sync::MutexGuard<'static, HashMap<String, Entry>> {
    CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Publica el token de la cuenta activa (lo llama `ui::chat::show` en cada
/// frame; solo escribe si cambió).
pub fn set_token(token: &str) {
    let mut guard = TOKEN.lock().unwrap_or_else(|p| p.into_inner());
    if guard.as_deref() != Some(token) {
        *guard = Some(token.to_string());
    }
}

/// Publica los ids de los servers en los que ya estás: con eso la tarjeta
/// dice "Ir al servidor" en vez de "Unirse".
pub fn set_joined_guilds(ids: impl IntoIterator<Item = String>) {
    let set: HashSet<String> = ids.into_iter().filter(|id| !id.is_empty()).collect();
    let cell = JOINED.get_or_init(|| Mutex::new(HashSet::new()));
    *cell.lock().unwrap_or_else(|p| p.into_inner()) = set;
}

pub fn is_joined(guild_id: &str) -> bool {
    JOINED
        .get()
        .map(|cell| cell.lock().unwrap_or_else(|p| p.into_inner()).contains(guild_id))
        .unwrap_or(false)
}

/// Códigos de invitación que aparecen en `content`
/// (`discord.gg/x`, `discord.com/invite/x`, `discordapp.com/invite/x`), en
/// orden de aparición y sin repetir.
pub fn extract_codes(content: &str) -> Vec<String> {
    const MARKERS: [&str; 3] = ["discord.gg/", "discord.com/invite/", "discordapp.com/invite/"];
    // `to_ascii_lowercase` no cambia el largo en bytes: los índices del
    // texto en minúsculas valen para el original (los códigos distinguen
    // mayúsculas).
    let lower = content.to_ascii_lowercase();
    let mut found: Vec<(usize, String)> = Vec::new();
    for marker in MARKERS {
        for (pos, _) in lower.match_indices(marker) {
            // "xdiscord.gg/..." o "foo.discord.gg/..." no son invitaciones.
            if let Some(prev) = lower[..pos].chars().next_back() {
                if prev.is_ascii_alphanumeric() || prev == '.' || prev == '-' {
                    continue;
                }
            }
            let start = pos + marker.len();
            let code: String = content[start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if (2..=32).contains(&code.len()) {
                found.push((pos, code));
            }
        }
    }
    found.sort_by_key(|(pos, _)| *pos);
    let mut out: Vec<String> = Vec::new();
    for (_, code) in found {
        if !out.contains(&code) {
            out.push(code);
        }
        if out.len() >= MAX_CODES_PER_MESSAGE {
            break;
        }
    }
    out
}

/// Estado de `code`. La primera vez devuelve `Loading` y lanza el pedido;
/// cuando llega, repinta (`ctx.request_repaint`) y las llamadas siguientes
/// devuelven el resultado cacheado.
pub fn lookup(ctx: &egui::Context, code: &str) -> InviteState {
    {
        let guard = cache();
        if let Some(entry) = guard.get(code) {
            let stale = matches!(entry.state, InviteState::Failed) && entry.at.elapsed() >= RETRY_AFTER;
            if !stale {
                return entry.state.clone();
            }
        }
    }

    let token = TOKEN.lock().unwrap_or_else(|p| p.into_inner()).clone();
    // Sin sesión (modo demo) no hay a quién preguntarle.
    let Some(token) = token else {
        return InviteState::Failed;
    };

    if IN_FLIGHT.load(Ordering::Relaxed) >= MAX_IN_FLIGHT {
        ctx.request_repaint_after(Duration::from_millis(250));
        return InviteState::Loading;
    }

    cache().insert(
        code.to_string(),
        Entry { state: InviteState::Loading, at: Instant::now() },
    );
    IN_FLIGHT.fetch_add(1, Ordering::Relaxed);

    let code = code.to_string();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let state = fetch(token, &code);
        cache().insert(code, Entry { state, at: Instant::now() });
        IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
        ctx.request_repaint();
    });
    InviteState::Loading
}

fn fetch(token: String, code: &str) -> InviteState {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return InviteState::Failed;
    };
    let result: anyhow::Result<Value> = rt.block_on(async {
        let rest = super::uwu_rest::UwuRest::for_token(token).await?;
        rest.get_invite(code).await
    });
    match result {
        Ok(value) => parse_invite(&value).map(InviteState::Ready).unwrap_or(InviteState::Invalid),
        Err(e) => {
            if is_unknown_invite(&e) {
                InviteState::Invalid
            } else {
                log::warn!("No se pudo consultar la invitación {code}: {e}");
                InviteState::Failed
            }
        }
    }
}

/// El cliente vendorizado deja el status y el cuerpo en el texto del error
/// (`... failed with code 404: {"message": "Unknown Invite", "code": 10006}`).
fn is_unknown_invite(e: &anyhow::Error) -> bool {
    let text = e.to_string();
    text.contains("10006") || text.contains("Unknown Invite") || text.contains("code 404")
}

fn parse_invite(v: &Value) -> Option<InviteInfo> {
    // Invitaciones a un grupo de DM (sin `guild`): no hay tarjeta de server.
    let guild = v.get("guild")?;
    let guild_id = guild.get("id")?.as_str()?.to_string();
    let name = guild.get("name")?.as_str()?.to_string();
    let icon_url = guild
        .get("icon")
        .and_then(Value::as_str)
        .filter(|h| !h.is_empty())
        .map(|hash| {
            let ext = if hash.starts_with("a_") { "gif" } else { "png" };
            format!("https://cdn.discordapp.com/icons/{guild_id}/{hash}.{ext}?size=128")
        });
    let has_feature = |feature: &str| {
        guild
            .get("features")
            .and_then(Value::as_array)
            .is_some_and(|f| f.iter().any(|x| x.as_str() == Some(feature)))
    };
    Some(InviteInfo {
        code: v.get("code").and_then(Value::as_str).unwrap_or_default().to_string(),
        community: has_feature("COMMUNITY"),
        verified: has_feature("VERIFIED"),
        partnered: has_feature("PARTNERED"),
        description: guild
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|d| !d.trim().is_empty()),
        online: v.get("approximate_presence_count").and_then(Value::as_u64),
        members: v.get("approximate_member_count").and_then(Value::as_u64),
        channel_id: v
            .pointer("/channel/id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        channel_type: v.pointer("/channel/type").and_then(Value::as_u64).unwrap_or(0) as u8,
        guild_id,
        name,
        icon_url,
    })
}

/// "Est. ene 2025": mes y año de creación del server, sacados del snowflake
/// de su id (los 42 bits altos son los ms desde 2015-01-01).
pub fn established_label(guild_id: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
    ];
    let id: u64 = guild_id.parse().ok()?;
    let ms = (id >> 22) + 1_420_070_400_000;
    let days = (ms / 86_400_000) as i64;
    let (year, month) = civil_from_days(days);
    Some(format!("Est. {} {}", MONTHS[(month - 1) as usize], year))
}

/// Días desde 1970-01-01 -> (año, mes) del calendario gregoriano.
fn civil_from_days(z: i64) -> (i64, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if month <= 2 { year + 1 } else { year }, month)
}

/// 1234 -> "1.234" (separador de miles como en es).
pub fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(ch);
    }
    out
}
