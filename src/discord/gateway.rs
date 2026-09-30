//! Conexión al Gateway *real* de Discord (`wss://gateway.discord.gg`), la
//! misma que usa el cliente web: acá llegan los eventos en vivo (mensajes
//! nuevos, cambios de presencia, voz, etc.) después de que `remote_auth`
//! (o un token pegado a mano) nos da un token de sesión.
//!
//! Portado (adaptado, no copiado tal cual) del gateway de Concord
//! (`discord/gateway.rs` de ese proyecto) a la arquitectura más simple de
//! `ecord`: acá no hay IDs tipados ni un `AppEventPublisher`/`DiscordState`
//! separados — se sigue mandando todo por el mismo
//! `std::sync::mpsc::Sender<AppEvent>` de siempre. Lo que sí se trajo
//! porque hace a que la conexión ande de verdad y no se corte a la
//! primera:
//! - La URL real, con `compress=zlib-stream`: Discord manda TODO
//!   comprimido en un único stream zlib que vive lo que dura la conexión
//!   (no un mensaje comprimido aparte por frame) — hace falta ir
//!   descomprimiendo con el mismo inflater durante toda la conexión, no
//!   uno nuevo por mensaje.
//! - RESUME (opcode 6): al cortarse la conexión por algo transitorio
//!   (caída de red, por ejemplo), reconectar con el `session_id` +
//!   secuencia guardados en vez de mandar un `IDENTIFY` nuevo — más
//!   rápido y no repite todo el historial de servers/amigos de arriba.
//! - Reconexión con backoff exponencial + jitter en vez de cortar toda la
//!   sesión (lo que hacía antes esta función) ante cualquier corte.
//! - Un `IDENTIFY` con las propiedades reales del fingerprint
//!   (`discord::fingerprint`, ya lo usa `rest.rs` para las llamadas REST)
//!   en vez de un `"eCord"/"linux"` inventado.
//!
//! Igual que con el login por QR: identificarse acá con el token de una
//! cuenta de usuario (en vez de un `Bot `) es el patrón que Discord llama
//! "self-bot" en sus términos de servicio. Funciona porque el Gateway no
//! distingue en el protocolo entre "el cliente oficial" y cualquier otra
//! cosa que mande el mismo `IDENTIFY` — pero el riesgo de que te bloqueen
//! la cuenta si lo detectan es real.

use std::sync::Arc;
use std::time::Duration;

use flate2::{Decompress, FlushDecompress, Status};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use url::Url;



use crate::discord::fingerprint::{self, ClientFingerprint};
use crate::discord::models::{
    Channel, Role, ThreadChannel, GatewayMessage, GatewayPayload, GuildCreatePayload, GuildMemberUpdate, GuildMembersChunk,
    MemberListMember, MemberListUpdate, MessageUpdate, PresenceEvent, ReactionEvent,
    ReadyPayload, VoiceState,
};
use crate::discord::AppEvent;

const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json&compress=zlib-stream";
const MOCK_URL_URL: &str = "ws://localhost:5380";
const USE_MOCK_URL: bool = true;



/// Bitmask que Discord chequea antes de mandar payloads exclusivos de
/// cuenta de usuario, como la presencia en vivo de los amigos
/// (`PRESENCE_UPDATE` por cada uno). Sin estos bits, Discord asume que la
/// sesión es un bot y no manda esos eventos — quedaría la lista de amigos
/// siempre en "desconectado". Portado tal cual de Concord (ver el
/// comentario original ahí para el desglose bit por bit).
const USER_ACCOUNT_CAPABILITIES: u64 = 253;

const RECONNECT_BASE_DELAY: Duration = Duration::from_millis(500);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);
/// Cuántas veces seguidas se puede fallar en RECONECTAR (no en
/// mantenerse conectado — eso reinicia el contador) antes de dejar de
/// intentarlo solo y avisarle al usuario en vez de loopear para siempre
/// en silencio.
const MAX_CONSECUTIVE_RECONNECT_FAILURES: u32 = 6;

/// Un stream zlib comprime TODA la conexión, no mensaje por mensaje: hay
/// que ir alimentándole los bytes de cada frame binario al mismo
/// inflater, y un payload JSON individual puede llegar repartido en más
/// de un frame de WebSocket. Discord marca el final de cada payload con
/// el sufijo de 4 bytes `00 00 ff ff` (un "sync flush" de zlib) — recién
/// ahí hay un JSON completo para parsear.
struct GatewayZlibDecoder {
    inflater: Decompress,
    pending: Vec<u8>,
}

impl Default for GatewayZlibDecoder {
    fn default() -> Self {
        Self { inflater: Decompress::new(true), pending: Vec::new() }
    }
}

impl GatewayZlibDecoder {
    /// Devuelve `Some(json)` si `chunk` completó un payload; `None` si
    /// hace falta esperar más frames.
    fn decode(&mut self, chunk: &[u8]) -> anyhow::Result<Option<String>> {
        self.pending.extend_from_slice(chunk);
        const SUFFIX: [u8; 4] = [0x00, 0x00, 0xff, 0xff];
        if !self.pending.ends_with(&SUFFIX) {
            return Ok(None);
        }

        let compressed = std::mem::take(&mut self.pending);
        // `total_in()`/`total_out()` del inflater son acumulados de TODA
        // la conexión (vive lo que dura el socket), no de este payload —
        // hay que trackear el offset DENTRO de `compressed` a mano con la
        // diferencia antes/después de cada `decompress`, no con el total
        // acumulado directamente (eso hacía que, pasado el primer
        // payload, se intentara indexar `compressed` con un offset de
        // decenas de miles de bytes en un buffer de un puñado de bytes).
        let mut input_offset = 0usize;
        let mut output = Vec::new();
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let input_before = self.inflater.total_in();
            let output_before = self.inflater.total_out();
            let status =
                self.inflater
                    .decompress(&compressed[input_offset..], &mut buffer, FlushDecompress::Sync)?;
            let consumed = (self.inflater.total_in() - input_before) as usize;
            let produced = (self.inflater.total_out() - output_before) as usize;
            input_offset += consumed;
            output.extend_from_slice(&buffer[..produced]);

            if matches!(status, Status::StreamEnd) {
                // No debería pasar nunca en un stream zlib de conexión
                // (no tiene bloque final hasta que se cierra el socket),
                // pero por las dudas: reiniciar el inflater en vez de
                // seguir alimentándolo con datos de otro stream.
                self.inflater = Decompress::new(true);
                break;
            }
            if input_offset >= compressed.len() && produced < buffer.len() {
                break;
            }
            if consumed == 0 && produced == 0 {
                anyhow::bail!("el decoder zlib del Gateway no avanzó");
            }
        }
        Ok(Some(String::from_utf8(output)?))
    }
}

/// Pedidos que la UI le puede hacer al Gateway (ver
/// `AppEvent::GatewayCommands`, que le entrega el extremo de envío a `App`).
#[derive(Debug, Clone)]
pub enum GatewayCommand {
    /// Suscribirse a la lista de miembros de `channel_id` dentro de
    /// `guild_id`. Sin esto Discord no manda ni la lista de miembros de un
    /// server ni las presencias de sus miembros: para las cuentas de usuario
    /// esos datos son "perezosos" y hay que pedirlos (opcode 37).
    SubscribeMemberList { guild_id: String, channel_id: String },
    /// Pedir el apodo y los roles de estos usuarios en `guild_id`
    /// (opcode 8). El historial de mensajes por REST no trae `member`, así
    /// que es la forma de saber cómo se llama cada autor en el server y de
    /// qué color es su rol. La respuesta llega como `GUILD_MEMBERS_CHUNK`.
    RequestGuildMembers { guild_id: String, user_ids: Vec<String> },
    /// Opcode 4: unirse/salir de un canal de voz (`channel_id: None` para
    /// salir), o cambiar mute/deaf mientras se está adentro.
    UpdateVoiceState {
        guild_id: Option<String>,
        channel_id: Option<String>,
        self_mute: bool,
        self_deaf: bool,
    },
}

/// La suscripción a la lista de miembros vigente. Se guarda para volver a
/// mandarla si la sesión se re-identifica (una sesión nueva no hereda las
/// suscripciones de la anterior).
#[derive(Debug, Clone, PartialEq, Eq)]
struct MemberListSubscription {
    guild_id: String,
    channel_id: String,
}

/// Opcode 37: "quiero la lista de miembros de este canal". Se piden los
/// primeros 100 lugares (`[0, 99]`): como Discord ordena la lista por rol y
/// después en línea / desconectados, eso cubre a todos los que están
/// conectados en casi cualquier server.
fn member_list_subscribe_payload(subscription: &MemberListSubscription) -> String {
    let mut channels = serde_json::Map::new();
    channels.insert(subscription.channel_id.clone(), serde_json::json!([[0, 99]]));
    let mut guild = serde_json::json!({
        "typing": true,
        "activities": true,
        "threads": true,
        "member_updates": true,
        "members": [],
    });
    guild["channels"] = Value::Object(channels);
    let mut subscriptions = serde_json::Map::new();
    subscriptions.insert(subscription.guild_id.clone(), guild);
    serde_json::json!({ "op": 37, "d": { "subscriptions": Value::Object(subscriptions) } }).to_string()
}

/// Opcode 8: "dame estos miembros". `guild_id` va como lista porque así lo
/// manda el cliente oficial en cuentas de usuario. Sin presencias: solo
/// hacen falta apodo y roles.
fn request_members_payload(guild_id: &str, user_ids: &[String]) -> String {
    serde_json::json!({
        "op": 8,
        "d": { "guild_id": [guild_id], "user_ids": user_ids, "presences": false },
    })
    .to_string()
}

/// Opcode 4: `guild_id` va en null para una llamada de DM/grupo (Discord
/// indexa esas por `channel_id` en el propio voice state).
fn update_voice_state_payload(
    guild_id: Option<&str>,
    channel_id: Option<&str>,
    self_mute: bool,
    self_deaf: bool,
) -> String {
    serde_json::json!({
        "op": 4,
        "d": {
            "guild_id": guild_id,
            "channel_id": channel_id,
            "self_mute": self_mute,
            "self_deaf": self_deaf,
        },
    })
    .to_string()
}

#[derive(Serialize)]
struct IdentifyProperties<'a> {
    os: &'a str,
    browser: &'static str,
    device: &'static str,
    system_locale: &'a str,
    browser_user_agent: &'a str,
    browser_version: &'static str,
    os_version: &'a str,
    referrer: &'static str,
    referring_domain: &'static str,
    referrer_current: &'static str,
    referring_domain_current: &'static str,
    release_channel: &'static str,
    client_build_number: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    installation_id: Option<String>,
}

fn build_identify_payload(token: &str, fp: &ClientFingerprint) -> String {
    let properties = IdentifyProperties {
        os: fp.os,
        browser: fingerprint::CLIENT_BROWSER,
        device: "",
        system_locale: &fp.system_locale,
        browser_user_agent: &fp.user_agent,
        browser_version: fingerprint::CLIENT_BROWSER_VERSION,
        os_version: &fp.os_version,
        referrer: "",
        referring_domain: "",
        referrer_current: fingerprint::DISCORD_REFERRER_CURRENT,
        referring_domain_current: fingerprint::DISCORD_REFERRING_DOMAIN_CURRENT,
        release_channel: "stable",
        client_build_number: fp.client_build_number,
        installation_id: fp.installation_id(),
    };
    serde_json::json!({
        "op": 2,
        "d": {
            "token": token,
            "capabilities": USER_ACCOUNT_CAPABILITIES,
            "properties": properties,
            "presence": { "since": 0, "activities": [], "status": "online", "afk": false },
            // `zlib-stream` ya va en la URL; este flag es un modo de
            // compresión de IDENTIFY aparte que el cliente web no usa.
            "compress": false,
            // Ningún campo de "versión de caché" real: como no guardamos
            // el estado entre reinicios de la app (a diferencia del
            // navegador, que sí lo hace), un `IDENTIFY` siempre arranca
            // de cero. Es exactamente lo mismo que ya hacía este cliente
            // antes de este cambio, no una regresión.
            "client_state": {
                "guild_versions": {},
                "highest_last_message_id": "0",
                "read_state_version": 0,
                "user_guild_settings_version": -1,
                "user_settings_version": -1,
                "private_channels_version": "0",
                "api_code_version": 0,
            },
        },
    })
    .to_string()
}

fn build_resume_payload(token: &str, session_id: &str, sequence: u64) -> String {
    serde_json::json!({ "op": 6, "d": { "token": token, "session_id": session_id, "seq": sequence } })
        .to_string()
}

/// Qué hacer después de que una conexión termina (se cortó el socket, o
/// Discord pidió reconectar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Reconectar reusando `session_id`/secuencia (RESUME).
    Resume,
    /// Tirar la sesión guardada y mandar un `IDENTIFY` nuevo.
    Reidentify,
    /// No tiene sentido reintentar (token inválido, versión de protocolo
    /// rechazada, etc.) — hay que avisarle al usuario y dejar de intentar.
    Fatal,
}

/// Códigos de cierre que Discord manda con distinto significado cada uno.
/// Reintentar un `IDENTIFY` idéntico tras un 4004 (token inválido) lo
/// único que logra es loopear para siempre mostrando "Conectando..." sin
/// avisarle nunca al usuario que el problema es el token.
fn outcome_for_close_code(code: u16) -> Outcome {
    match code {
        4004 | 4010..=4014 => Outcome::Fatal,
        4003 | 4007 | 4009 => Outcome::Reidentify,
        _ => Outcome::Resume,
    }
}

/// Sesión guardada entre reconexiones, para poder mandar un RESUME en vez
/// de un IDENTIFY nuevo tras un corte transitorio.
#[derive(Default)]
struct Session {
    session_id: Option<String>,
    resume_url: Option<String>,
    sequence: Option<u64>,
}

impl Session {
    fn can_resume(&self) -> bool {
        self.session_id.is_some() && self.sequence.is_some()
    }

    fn clear(&mut self) {
        self.session_id = None;
        self.resume_url = None;
        self.sequence = None;
    }
}

pub async fn run(
    token: String,
    fingerprint: Arc<ClientFingerprint>,
    tx: std::sync::mpsc::Sender<AppEvent>,
    mut commands: UnboundedReceiver<GatewayCommand>,
) -> anyhow::Result<()> {
    let mut session = Session::default();
    // Sobrevive a las reconexiones: si la sesión se re-identifica hay que
    // volver a suscribirse a la lista de miembros que se estaba viendo.
    let mut member_list: Option<MemberListSubscription> = None;
    let mut backoff = RECONNECT_BASE_DELAY;
    // Se resetea cada vez que una conexión llega a establecerse de
    // verdad (READY/RESUMED) — así "5 cortes en 3 horas, cada uno
    // reconectando bien" no cuenta como fallas seguidas, pero "5
    // intentos de reconexión seguidos sin lograrlo ni una vez" sí.
    let mut consecutive_reconnect_failures: u32 = 0;
    // Para no mandar un toast de "se cortó" por cada frame perdido: solo
    // uno al cortarse, y uno al reconectar — no uno por cada reintento
    // fallido mientras tanto.
    let mut disconnected_notified = false;

    loop {
        let mut established = false;
        let outcome = connect_and_run(
            &token,
            &fingerprint,
            &tx,
            &mut session,
            &mut established,
            &mut commands,
            &mut member_list,
        )
        .await;

        if established {
            // Esta conexión llegó a andar de verdad (haya durado lo que
            // haya durado) — lo que pasó después no es "no pudimos
            // reconectar", es un corte nuevo. Reiniciar todo, backoff
            // incluido (antes de calcular la espera de abajo: si no,
            // el primer reintento de un corte fresco heredaba el
            // backoff ya crecido de una racha de fallas anterior).
            consecutive_reconnect_failures = 0;
            backoff = RECONNECT_BASE_DELAY;
            if disconnected_notified {
                let _ = tx.send(AppEvent::GatewayReconnected);
                disconnected_notified = false;
            }
        } else if !disconnected_notified {
            // Primera vez que esta racha de reconexión falla: avisar. Los
            // siguientes reintentos de la misma racha no vuelven a
            // avisar (ver el comentario de arriba).
            let reason = match &outcome {
                Ok(Outcome::Fatal) => "Discord rechazó la conexión".to_owned(),
                Ok(_) => "se cortó la conexión con Discord".to_owned(),
                Err(e) => format!("se cortó la conexión con Discord ({e})"),
            };
            let _ = tx.send(AppEvent::GatewayDisconnected(reason));
            disconnected_notified = true;
        }

        match outcome {
            Ok(Outcome::Resume) => {}
            Ok(Outcome::Reidentify) => session.clear(),
            Ok(Outcome::Fatal) => {
                anyhow::bail!("Discord rechazó la conexión (sesión no recuperable)");
            }
            Err(e) => log::warn!("Gateway: {e}"),
        }

        if !established {
            consecutive_reconnect_failures += 1;
            if consecutive_reconnect_failures >= MAX_CONSECUTIVE_RECONNECT_FAILURES {
                anyhow::bail!(
                    "no se pudo reconectar con Discord después de {consecutive_reconnect_failures} intentos"
                );
            }
        }

        let jitter_ms = rand::thread_rng().gen_range(0..=backoff.as_millis() as u64);
        tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
        backoff = (backoff * 2).min(RECONNECT_MAX_DELAY);
    }
}

async fn connect_and_run(
    token: &str,
    fingerprint: &ClientFingerprint,
    tx: &std::sync::mpsc::Sender<AppEvent>,
    session: &mut Session,
    established: &mut bool,
    commands: &mut UnboundedReceiver<GatewayCommand>,
    member_list: &mut Option<MemberListSubscription>,
) -> anyhow::Result<Outcome> {
    let url = if session.can_resume() {
        session.resume_url.clone().unwrap_or_else(|| GATEWAY_URL.to_string())
    } else {
        GATEWAY_URL.to_string()
    };
    // Un resume_gateway_url de Discord no trae los query params de
    // versión/encoding/compresión — hay que volver a pegárselos.
    let mut url = if url.contains('?') { url } else { format!("{url}/?v=10&encoding=json&compress=zlib-stream") };
    if USE_MOCK_URL {
        let mut u = Url::parse(MOCK_URL_URL).unwrap();
        u.query_pairs_mut().append_pair("url", &url);
        url = u.to_string();
    }

    let mut request = url.into_client_request()?;
    request.headers_mut().extend(fingerprint::discord_gateway_headers(fingerprint));
    let (ws_stream, _) = tokio_tungstenite::connect_async(request).await?;
    let (mut write, mut read) = ws_stream.split();
    let mut zlib = GatewayZlibDecoder::default();

    let mut heartbeat = tokio::time::interval(Duration::from_secs(3600));
    heartbeat.tick().await;
    let mut awaiting_ack = false;

    loop {
        let msg = tokio::select! {
            msg = read.next() => match msg {
                Some(msg) => msg?,
                None => return Ok(Outcome::Resume),
            },
            _ = heartbeat.tick() => {
                if awaiting_ack {
                    // Discord no contestó el heartbeat anterior: la
                    // conexión quedó zombie (no se cortó pero tampoco
                    // responde). Reconectar es más confiable que seguir
                    // esperando un ACK que ya no va a llegar.
                    return Ok(Outcome::Resume);
                }
                let hb = serde_json::json!({ "op": 1, "d": session.sequence });
                write.send(Message::Text(hb.to_string().into())).await?;
                awaiting_ack = true;
                continue;
            }
            // Un pedido de la UI. Si el canal de comandos se cierra,
            // `recv()` devuelve `None`, el patrón no coincide y esta rama
            // simplemente queda deshabilitada (no gira en vacío).
            Some(command) = commands.recv() => {
                match command {
                    GatewayCommand::SubscribeMemberList { guild_id, channel_id } => {
                        let wanted = MemberListSubscription { guild_id, channel_id };
                        if member_list.as_ref() != Some(&wanted) {
                            let payload = member_list_subscribe_payload(&wanted);
                            *member_list = Some(wanted);
                            // Antes del READY no hay sesión a la cual
                            // suscribirse: queda guardada y se manda apenas
                            // llegue (ver más abajo).
                            if *established {
                                write.send(Message::Text(payload.into())).await?;
                            }
                        }
                    }
                    GatewayCommand::RequestGuildMembers { guild_id, user_ids } => {
                        // Antes del READY no hay sesión: el pedido se
                        // descarta (la UI vuelve a pedir lo que le falte
                        // la próxima vez que cargue mensajes).
                        if *established && !user_ids.is_empty() {
                            let payload = request_members_payload(&guild_id, &user_ids);
                            write.send(Message::Text(payload.into())).await?;
                        }
                    }
                    GatewayCommand::UpdateVoiceState {
                        guild_id,
                        channel_id,
                        self_mute,
                        self_deaf,
                    } => {
                        // A diferencia de los otros comandos, este sí hace
                        // falta mandarlo aunque todavía no haya `establecido`
                        // sesión propia: es lo que dispara el join en sí.
                        let payload = update_voice_state_payload(
                            guild_id.as_deref(),
                            channel_id.as_deref(),
                            self_mute,
                            self_deaf,
                        );
                        write.send(Message::Text(payload.into())).await?;
                    }
                }
                continue;
            }
        };

        let text = match msg {
            Message::Text(text) => text.to_string(),
            Message::Binary(bytes) => match zlib.decode(&bytes)? {
                Some(text) => text,
                None => continue,
            },
            Message::Close(frame) => {
                let code: u16 = frame.map(|f| f.code.into()).unwrap_or(1000);
                log::warn!("Gateway cerrado por Discord (código {code})");
                return Ok(outcome_for_close_code(code));
            }
            _ => continue,
        };

        let payload: GatewayPayload = serde_json::from_str(&text)?;
        if let Some(s) = payload.s {
            session.sequence = Some(s);
        }

        match payload.op {
            // Hello: acá arranca todo. Ajustamos el intervalo real de
            // heartbeat (con jitter en el primero, como recomienda
            // Discord) y mandamos RESUME o IDENTIFY según haya o no
            // sesión guardada.
            10 => {
                let interval_ms = payload.d["heartbeat_interval"].as_u64().unwrap_or(41250);
                let jitter = rand::thread_rng().gen_range(0..=interval_ms);
                heartbeat = tokio::time::interval(Duration::from_millis(interval_ms));
                tokio::time::sleep(Duration::from_millis(jitter)).await;
                heartbeat.reset();

                let hello_payload = if session.can_resume() {
                    build_resume_payload(token, session.session_id.as_deref().unwrap_or_default(), session.sequence.unwrap_or_default())
                } else {
                    build_identify_payload(token, fingerprint)
                };
                write.send(Message::Text(hello_payload.into())).await?;
            }
            // Heartbeat ACK.
            11 => awaiting_ack = false,
            // Discord puede pedir un heartbeat inmediato (fuera del
            // intervalo normal) mandando este opcode.
            1 => {
                let hb = serde_json::json!({ "op": 1, "d": session.sequence });
                write.send(Message::Text(hb.to_string().into())).await?;
                awaiting_ack = true;
            }
            // Dispatch: acá viajan `READY`, `MESSAGE_CREATE`, etc.
            //
            // Un evento que no podamos deserializar NO debe tirar abajo la
            // conexión entera: Discord manda muchas variantes de cada
            // payload (mensajes de sistema, usuarios parciales, servers
            // caídos) y nuestros structs solo cubren lo que usamos.
            0 => {
                let Some(event_name) = payload.t.as_deref() else { continue };
                if event_name == "RESUMED" {
                    // A un RESUME exitoso Discord no le manda un READY de
                    // vuelta (por algo es más rápido que un IDENTIFY) —
                    // este dispatch, sin más datos que el nombre, es la
                    // única confirmación de que la sesión quedó viva.
                    *established = true;
                }
                if event_name == "READY" {
                    // Guardamos `session_id`/`resume_gateway_url` ANTES de
                    // mover `payload.d` a `handle_dispatch` — son justo
                    // los dos campos que hacen falta para poder mandar un
                    // RESUME la próxima vez que se corte la conexión.
                    session.session_id = payload.d.get("session_id").and_then(Value::as_str).map(str::to_owned);
                    session.resume_url =
                        payload.d.get("resume_gateway_url").and_then(Value::as_str).map(str::to_owned);
                }
                // Diagnóstico ANTES de mover `payload.d` a `handle_dispatch`
                // (que lo consume): si falla el parseo, mostrar qué claves
                // de primer nivel trajo realmente el payload es la forma
                // más rápida de saber qué cambió, en vez de adivinar. Solo
                // se arma el string si hace falta (mismo motivo que el
                // chequeo de `event_name == "READY"` para el clon de
                // session_id/resume_url más arriba: no vale la pena para
                // cada `MESSAGE_CREATE`).
                let debug_keys = (event_name == "READY")
                    .then(|| payload.d.as_object())
                    .flatten()
                    .map(|obj| obj.keys().cloned().collect::<Vec<_>>().join(", "));
                if let Err(e) = handle_dispatch(event_name, payload.d, tx) {
                    log::warn!("No se pudo procesar el evento {event_name}: {e}");
                    if event_name == "READY" {
                        let keys = debug_keys.unwrap_or_else(|| "(no era un objeto JSON)".to_owned());
                        let _ = tx.send(AppEvent::Error(format!(
                            "No se pudo leer el READY de Discord: {e} — claves recibidas: [{keys}]"
                        )));
                        return Ok(Outcome::Reidentify);
                    }
                } else if event_name == "READY" {
                    // Recién acá, después de que el parseo salió bien:
                    // antes (mientras se debuggeaba el "falta el campo
                    // `user`") esto se hubiera marcado como "conexión
                    // establecida" con un READY que en realidad no se
                    // pudo leer, escondiendo el problema en vez de
                    // mostrarlo.
                    *established = true;
                    // Una sesión nueva arranca sin suscripciones: volver a
                    // pedir la lista de miembros que se estaba viendo. (Un
                    // RESUME conserva las de la sesión anterior.)
                    if let Some(subscription) = member_list.as_ref() {
                        let payload = member_list_subscribe_payload(subscription);
                        write.send(Message::Text(payload.into())).await?;
                    }
                }
            }
            // Discord pide reconectar (mantenimiento del lado de ellos,
            // por lo general) — RESUME apenas se pueda reconectar.
            7 => return Ok(Outcome::Resume),
            // La sesión ya no es válida; `d` dice si vale la pena
            // reintentar con RESUME o si hay que arrancar de cero.
            9 => {
                let resumable = payload.d.as_bool().unwrap_or(false);
                return Ok(if resumable { Outcome::Resume } else { Outcome::Reidentify });
            }
            _ => {}
        }
    }
}

/// Deshace la deduplicación de usuarios del `READY`.
///
/// Con `DEDUPE_USER_OBJECTS` Discord manda cada usuario una sola vez en el
/// array `users` de nivel superior y en el resto pone solo el id:
///
/// * `relationships[]` trae `user_id` en vez de `user`;
/// * `private_channels[]` trae `recipient_ids` en vez de `recipients`.
///
/// Acá se vuelven a armar esos campos, así `Relationship` y `PrivateChannel`
/// se deserializan igual que con el formato viejo. Si el payload ya venía con
/// los objetos completos (o no trae `users`), no toca nada.
fn hydrate_deduped_users(ready: &mut serde_json::Value) {
    use serde_json::Value;

    let Some(root) = ready.as_object_mut() else {
        return;
    };
    let users: std::collections::HashMap<String, Value> = root
        .get("users")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|user| Some((user.get("id")?.as_str()?.to_owned(), user.clone())))
                .collect()
        })
        .unwrap_or_default();
    if users.is_empty() {
        return;
    }

    if let Some(relationships) = root.get_mut("relationships").and_then(Value::as_array_mut) {
        for relationship in relationships.iter_mut() {
            let Some(object) = relationship.as_object_mut() else {
                continue;
            };
            if object.contains_key("user") {
                continue;
            }
            let user = object
                .get("user_id")
                .or_else(|| object.get("id"))
                .and_then(Value::as_str)
                .and_then(|id| users.get(id))
                .cloned();
            if let Some(user) = user {
                object.insert("user".to_owned(), user);
            }
        }
        // Una relación cuyo usuario no vino en `users` no se puede mostrar:
        // mejor descartarla que perder el `READY` entero por ella.
        relationships.retain(|relationship| relationship.get("user").is_some());
    }

    if let Some(channels) = root.get_mut("private_channels").and_then(Value::as_array_mut) {
        for channel in channels.iter_mut() {
            let Some(object) = channel.as_object_mut() else {
                continue;
            };
            let already_has_recipients = object
                .get("recipients")
                .and_then(Value::as_array)
                .is_some_and(|recipients| !recipients.is_empty());
            if already_has_recipients {
                continue;
            }
            let recipients: Vec<Value> = object
                .get("recipient_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|id| users.get(id).cloned())
                .collect();
            object.insert("recipients".to_owned(), Value::Array(recipients));
        }
    }
}

#[cfg(test)]
mod hydrate_tests {
    use super::hydrate_deduped_users;
    use serde_json::json;

    #[test]
    fn rebuilds_relationships_and_dm_recipients_from_users() {
        let mut ready = json!({
            "users": [
                { "id": "1", "username": "ana" },
                { "id": "2", "username": "beto" }
            ],
            "relationships": [
                { "id": "1", "type": 1, "user_id": "1" },
                { "id": "9", "type": 1, "user_id": "9" }
            ],
            "private_channels": [
                { "id": "50", "type": 1, "recipient_ids": ["2"] }
            ]
        });
        hydrate_deduped_users(&mut ready);

        let relationships = ready["relationships"].as_array().unwrap();
        assert_eq!(relationships.len(), 1, "la relación sin usuario se descarta");
        assert_eq!(relationships[0]["user"]["username"], "ana");
        assert_eq!(ready["private_channels"][0]["recipients"][0]["username"], "beto");
    }

    #[test]
    fn leaves_full_objects_untouched() {
        let mut ready = json!({
            "relationships": [
                { "id": "1", "type": 1, "user": { "id": "1", "username": "ana" } }
            ]
        });
        let before = ready.clone();
        hydrate_deduped_users(&mut ready);
        assert_eq!(ready, before);
    }
}

/// Roles de la propia cuenta en cada server, de `merged_members`: un array
/// (uno por guild, en el MISMO orden que `guilds`) de arrays de miembros —
/// en la práctica, un solo miembro: el nuestro. Cada entrada se lee por
/// separado y tolerando formas raras: una mala no descarta a las demás.
/// Si un guild no trae su entrada, simplemente no aparece acá (y sus
/// canales quedan sin filtrar).
fn merged_member_roles(data: &serde_json::Value) -> Vec<(String, Vec<String>)> {
    use serde_json::Value;

    let (Some(guilds), Some(merged)) = (
        data.get("guilds").and_then(Value::as_array),
        data.get("merged_members").and_then(Value::as_array),
    ) else {
        return Vec::new();
    };
    guilds
        .iter()
        .zip(merged)
        .filter_map(|(guild, members)| {
            let guild_id = guild.get("id")?.as_str()?.to_owned();
            // Normalmente `[ {miembro} ]`; por si viniera el miembro suelto.
            let member = match members {
                Value::Array(list) => list.first()?,
                other => other,
            };
            let roles = member
                .get("roles")?
                .as_array()?
                .iter()
                .filter_map(|r| r.as_str().map(str::to_owned))
                .collect();
            Some((guild_id, roles))
        })
        .collect()
}

fn handle_dispatch(
    event_name: &str,
    data: serde_json::Value,
    tx: &std::sync::mpsc::Sender<AppEvent>,
) -> anyhow::Result<()> {
    match event_name {
        "READY" => {
            // Con la capability `DEDUPE_USER_OBJECTS` (ver
            // `USER_ACCOUNT_CAPABILITIES`) el `READY` no repite el objeto de
            // usuario en cada lado: lo manda UNA vez en `users` y el resto
            // apunta por id. Nuestros modelos esperan los objetos completos.
            let mut data = data;
            hydrate_deduped_users(&mut data);
            // Roles propios por server, ANTES de que `data` se consuma. Se
            // mandan después del `Ready` (que es el que crea los `Server`).
            let self_roles = merged_member_roles(&data);
            let ready: ReadyPayload = serde_json::from_value(data)?;
            let _ = tx.send(AppEvent::Ready(Box::new(ready)));
            for (guild_id, roles) in self_roles {
                let _ = tx.send(AppEvent::SelfGuildRoles { guild_id, roles });
            }
        }
        "MESSAGE_CREATE" => {
            let message: GatewayMessage = serde_json::from_value(data)?;
            let _ = tx.send(AppEvent::MessageCreate(Box::new(message)));
        }
        // Mensaje editado, o Discord terminó de resolver los embeds de un
        // link (lo normal: `MESSAGE_CREATE` llega sin embeds y unos
        // segundos después este evento trae `embeds` completos). Es un
        // payload parcial, ver `MessageUpdate`.
        "MESSAGE_UPDATE" => {
            let update: MessageUpdate = serde_json::from_value(data)?;
            let _ = tx.send(AppEvent::MessageUpdate(Box::new(update)));
        }
        // Discord manda un `GUILD_CREATE` por cada guild después del
        // `READY` (es como se completan los servers que en el `READY`
        // venían "unavailable", y de paso trae la foto inicial de voz).
        // Ignoramos todo lo que no sea `voice_states`: el resto del guild
        // (canales, nombre, ícono) ya lo tenemos de `READY` o lo pedimos
        // por REST al abrirlo.
        "GUILD_CREATE" => {
            let guild: GuildCreatePayload = serde_json::from_value(data)?;
            // Los roles se toman de acá también (no solo del `READY`) por si
            // el guild llegó "unavailable" en el `READY`, sin roles.
            if !guild.roles.is_empty() {
                let _ = tx.send(AppEvent::GuildRoles {
                    guild_id: guild.id.clone(),
                    roles: guild.roles,
                });
            }
            let _ = tx.send(AppEvent::GuildVoiceStates { guild_id: guild.id, states: guild.voice_states });
        }
        // Alguien cambió su estado (en línea / ausente / no molestar /
        // desconectado) o su actividad. Con `guild_id` es la presencia de un
        // miembro de ese server; sin él, la de un amigo.
        "PRESENCE_UPDATE" => {
            let event: PresenceEvent = serde_json::from_value(data)?;
            let _ = tx.send(AppEvent::PresenceUpdate(Box::new(event)));
        }
        // Con la capability `PRIORITIZED_READY_PAYLOAD` las presencias
        // iniciales NO viajan en el `READY` sino en este segundo payload:
        // `merged_presences.friends` es la lista de amigos conectados.
        // (Los que no aparecen están desconectados.)
        "READY_SUPPLEMENTAL" => {
            // Con `PRIORITIZED_READY_PAYLOAD` los roles propios pueden venir
            // acá en vez de en el `READY`: se leen de los dos lados.
            for (guild_id, roles) in merged_member_roles(&data) {
                let _ = tx.send(AppEvent::SelfGuildRoles { guild_id, roles });
            }
            let friends: Vec<PresenceEvent> = data
                .get("merged_presences")
                .and_then(|merged| merged.get("friends"))
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            if !friends.is_empty() {
                let _ = tx.send(AppEvent::PresenceSnapshot(friends));
            }
            // Además de las presencias, este payload trae por cada guild su
            // `id` y sus `voice_states`: la foto de quién está conectado a
            // qué canal de voz justo al conectar. Cada guild se parsea por
            // separado (y cada estado también) para que uno raro no tire
            // abajo a los demás. Si la clave `voice_states` no está, el
            // guild no informó nada y no se toca lo que ya hay; si viene
            // vacía, significa "nadie en voz" y sí se aplica.
            if let Some(guilds) = data.get("guilds").and_then(Value::as_array) {
                for guild in guilds {
                    let Some(guild_id) = guild.get("id").and_then(Value::as_str) else { continue };
                    let Some(list) = guild.get("voice_states").and_then(Value::as_array) else { continue };
                    let states: Vec<VoiceState> = list
                        .iter()
                        .filter_map(|state| serde_json::from_value(state.clone()).ok())
                        .collect();
                    let _ = tx.send(AppEvent::GuildVoiceStates { guild_id: guild_id.to_string(), states });
                }
            }
        }
        // La lista de miembros de un canal al que nos suscribimos (opcode
        // 37), ya ordenada por Discord: grupos por rol, después en línea y
        // desconectados. Ver `models::MemberListOp` para cómo se aplica.
        "GUILD_MEMBER_LIST_UPDATE" => {
            let update: MemberListUpdate = serde_json::from_value(data)?;
            let _ = tx.send(AppEvent::MemberListUpdate(Box::new(update)));
        }
        // Respuesta a `RequestGuildMembers` (opcode 8): apodo y roles de
        // los usuarios pedidos.
        "GUILD_MEMBERS_CHUNK" => {
            let chunk: GuildMembersChunk = serde_json::from_value(data)?;
            let _ = tx.send(AppEvent::GuildMembers { guild_id: chunk.guild_id, members: chunk.members });
        }
        // Cambió alguna de nuestras sesiones: es como llega lo que jugás o
        // escuchás en otro cliente tuyo (app oficial, celular).
        "SESSIONS_REPLACE" => {
            let activities = crate::discord::models::parse_session_activities(&data);
            let _ = tx.send(AppEvent::OwnActivities(activities));
        }
        // Un canal se marcó como leído (desde otro dispositivo, o el eco de
        // nuestro propio ack). Trae lo que queda sin leer en `mention_count`.
        "MESSAGE_ACK" => {
            let channel_id = data.get("channel_id").and_then(Value::as_str);
            let message_id = data.get("message_id").and_then(Value::as_str);
            if let (Some(channel_id), Some(message_id)) = (channel_id, message_id) {
                let mention_count =
                    data.get("mention_count").and_then(Value::as_u64).unwrap_or(0) as u32;
                let _ = tx.send(AppEvent::MessageAck {
                    channel_id: channel_id.to_owned(),
                    message_id: message_id.to_owned(),
                    mention_count,
                });
            }
        }
        // Alguien cambió su apodo o sus roles: se reusa el mismo evento que
        // el chunk, con un único miembro.
        "GUILD_MEMBER_UPDATE" => {
            let update: GuildMemberUpdate = serde_json::from_value(data)?;
            let member = MemberListMember {
                user: update.user,
                nick: update.nick,
                roles: update.roles,
                presence: None,
            };
            let _ = tx.send(AppEvent::GuildMembers { guild_id: update.guild_id, members: vec![member] });
        }
        // Alguien se conectó/desconectó/cambió de canal de voz. A
        // diferencia del array embebido en `GUILD_CREATE`, acá sí viene
        // `guild_id` en el propio payload.
        "VOICE_STATE_UPDATE" => {
            let state: VoiceState = serde_json::from_value(data)?;
            let _ = tx.send(AppEvent::VoiceStateUpdate(state));
        }
        // El endpoint/token de voz para la conexión que acabamos de pedir
        // con `GatewayCommand::UpdateVoiceState` (opcode 4). Llega después
        // del `VOICE_STATE_UPDATE` propio.
        "VOICE_SERVER_UPDATE" => {
            let server: super::VoiceServerUpdate = serde_json::from_value(data)?;
            let _ = tx.send(AppEvent::VoiceServerUpdate(server));
        }
        // Alguien puso una reacción (en cualquier mensaje al que tengamos
        // acceso, no solo los nuestros). Un evento por cada
        // reacción/usuario — si tres personas ponen 👍 llegan tres
        // `MESSAGE_REACTION_ADD` separados, no uno con contador 3.
        "MESSAGE_REACTION_ADD" => {
            let ev: ReactionEvent = serde_json::from_value(data)?;
            let _ = tx.send(AppEvent::ReactionAdd {
                channel_id: ev.channel_id,
                message_id: ev.message_id,
                user_id: ev.user_id,
                emoji: ev.emoji,
            });
        }
        "MESSAGE_REACTION_REMOVE" => {
            let ev: ReactionEvent = serde_json::from_value(data)?;
            let _ = tx.send(AppEvent::ReactionRemove {
                channel_id: ev.channel_id,
                message_id: ev.message_id,
                user_id: ev.user_id,
                emoji: ev.emoji,
            });
        }
        // Un admin creó o modificó un canal (nombre, orden, categoría,
        // permisos). Los DMs también pasan por acá pero no traen
        // `guild_id`: se ignoran.
        "CHANNEL_CREATE" | "CHANNEL_UPDATE" => {
            let Some(guild_id) = data.get("guild_id").and_then(Value::as_str).map(str::to_owned)
            else {
                return Ok(());
            };
            let channel: Channel = serde_json::from_value(data)?;
            let _ = tx.send(AppEvent::ChannelUpsert { guild_id, channel });
        }
        "CHANNEL_DELETE" => {
            let (Some(guild_id), Some(channel_id)) = (
                data.get("guild_id").and_then(Value::as_str).map(str::to_owned),
                data.get("id").and_then(Value::as_str).map(str::to_owned),
            ) else {
                return Ok(());
            };
            let _ = tx.send(AppEvent::ChannelDelete { guild_id, channel_id });
        }
        // Rol nuevo o editado: `{ guild_id, role: { ... } }`.
        "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => {
            let Some(guild_id) = data.get("guild_id").and_then(Value::as_str).map(str::to_owned)
            else {
                return Ok(());
            };
            let Some(role) = data.get("role").cloned() else { return Ok(()) };
            let role: Role = serde_json::from_value(role)?;
            let _ = tx.send(AppEvent::GuildRoleUpsert { guild_id, role });
        }
        "GUILD_ROLE_DELETE" => {
            let (Some(guild_id), Some(role_id)) = (
                data.get("guild_id").and_then(Value::as_str).map(str::to_owned),
                data.get("role_id").and_then(Value::as_str).map(str::to_owned),
            ) else {
                return Ok(());
            };
            let _ = tx.send(AppEvent::GuildRoleDelete { guild_id, role_id });
        }
        // Posts de foro (un post es un hilo). Los hilos de canales de
        // texto normales también llegan acá; el estado decide si le
        // importan (solo si su `parent_id` es un foro).
        "THREAD_CREATE" | "THREAD_UPDATE" => {
            let thread: ThreadChannel = serde_json::from_value(data)?;
            let _ = tx.send(AppEvent::ThreadUpsert(thread));
        }
        "THREAD_DELETE" => {
            let (Some(thread_id), Some(parent_id)) = (
                data.get("id").and_then(Value::as_str).map(str::to_owned),
                data.get("parent_id").and_then(Value::as_str).map(str::to_owned),
            ) else {
                return Ok(());
            };
            let _ = tx.send(AppEvent::ThreadDelete { parent_id, thread_id });
        }
        // Un bot contestó a un botón con un formulario.
        "INTERACTION_MODAL_CREATE" => {
            if let Some(modal) = super::models::ModalRequest::from_value(&data) {
                let _ = tx.send(AppEvent::ModalCreate(Box::new(modal)));
            }
        }
        // La app no contestó a tiempo (o rechazó la interacción).
        "INTERACTION_FAILURE" => {
            let _ = tx.send(AppEvent::InteractionFailed {
                message: "La aplicación no respondió a esa interacción.".to_string(),
            });
        }
        // Muchísimos otros eventos existen (TYPING_START, ...). Se agregan
        // acá con el mismo patrón: deserializar y mandar un nuevo
        // `AppEvent`.
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod merged_members_tests {
    use super::merged_member_roles;
    use serde_json::json;

    #[test]
    fn reads_own_roles_aligned_with_guilds() {
        let data = json!({
            "guilds": [ { "id": "g1" }, { "id": "g2" }, { "id": "g3" } ],
            "merged_members": [
                [ { "user_id": "me", "roles": ["r1", "r2"] } ],
                [ { "user_id": "me", "roles": [] } ],
                []
            ]
        });
        assert_eq!(
            merged_member_roles(&data),
            vec![
                ("g1".to_string(), vec!["r1".to_string(), "r2".to_string()]),
                ("g2".to_string(), vec![]),
            ]
        );
    }

    #[test]
    fn missing_or_odd_shapes_are_skipped() {
        assert!(merged_member_roles(&json!({ "guilds": [{ "id": "g" }] })).is_empty());
        let data = json!({
            "guilds": [ { "id": "g1" }, { "id": "g2" } ],
            "merged_members": [ [ { "user_id": "me" } ], [ { "roles": ["x"] } ] ]
        });
        assert_eq!(
            merged_member_roles(&data),
            vec![("g2".to_string(), vec!["x".to_string()])]
        );
    }
}
