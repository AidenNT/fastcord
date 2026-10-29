//! Catálogo de opcodes del Gateway y del Gateway de voz, con su dirección,
//! si eCord los usa y una plantilla de payload para probarlos.
//!
//! Tabla tomada de la documentación de Userdoccers (`docs.discord.food`,
//! "Opcodes and Close Codes" y "Gateway Events"). Las plantillas usan IDs de
//! ejemplo (`<...>` marca lo que hay que reemplazar). Solo hay plantilla donde
//! la documentación describe la estructura; el resto queda sin ella a
//! propósito, en vez de inventar campos.
//!
//! Se usa en dos lugares: la sección Cliente → Opcodes (referencia con
//! búsqueda) y el selector "Opcodes" de Red → WebSocket, que carga la
//! plantilla en el campo de envío.

/// Quién manda el opcode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Dir {
    Send,
    Receive,
    Both,
}

impl Dir {
    pub(super) fn label(self) -> &'static str {
        match self {
            Dir::Send => "envía el cliente",
            Dir::Receive => "recibe el cliente",
            Dir::Both => "ambos",
        }
    }

    pub(super) fn arrow(self) -> &'static str {
        match self {
            Dir::Send => "↑",
            Dir::Receive => "↓",
            Dir::Both => "↕",
        }
    }
}

pub(super) struct Op {
    pub code: u8,
    pub name: &'static str,
    pub dir: Dir,
    /// Marcado como obsoleto en la documentación.
    pub deprecated: bool,
    /// Dónde lo usa eCord; `None` si todavía no lo implementa.
    pub ecord: Option<&'static str>,
    /// Payload de ejemplo (el JSON completo, con `op` y `d`).
    pub template: Option<&'static str>,
    pub note: &'static str,
}

const fn op(
    code: u8,
    name: &'static str,
    dir: Dir,
    ecord: Option<&'static str>,
    template: Option<&'static str>,
    note: &'static str,
) -> Op {
    Op { code, name, dir, deprecated: false, ecord, template, note }
}

const fn old(
    code: u8,
    name: &'static str,
    dir: Dir,
    ecord: Option<&'static str>,
    template: Option<&'static str>,
    note: &'static str,
) -> Op {
    Op { code, name, dir, deprecated: true, ecord, template, note }
}

use Dir::{Both, Receive, Send};

/// Opcodes del Gateway principal (0–43).
pub(super) static GATEWAY: [Op; 44] = [
    op(0, "Dispatch", Receive, Some("gateway.rs · handle_dispatch"), None, "Un evento (READY, MESSAGE_CREATE…). Lleva `t` y `s`."),
    op(1, "Heartbeat", Both, Some("gateway.rs · latido de respaldo"), Some(r#"{"op":1,"d":null}"#), "`d` es el último `s` recibido (null si no hay). Discord también lo puede pedir."),
    op(2, "Identify", Send, Some("gateway.rs · build_identify_payload"), Some(r#"{"op":2,"d":{"token":"<TOKEN>","properties":{"os":"linux","browser":"disco","device":"disco"},"compress":false,"capabilities":1734653,"client_state":{"guild_versions":{},"api_code_version":0}}}"#), "Handshake inicial. Mandarlo dos veces cierra con 4005."),
    op(3, "Presence Update", Send, None, Some(r#"{"op":3,"d":{"since":0,"activities":[],"status":"online","afk":false}}"#), "Máximo 5 cada 20 s."),
    op(4, "Voice State Update", Send, Some("gateway.rs · update_voice_state_payload"), Some(r#"{"op":4,"d":{"guild_id":"<GUILD_ID>","channel_id":"<CHANNEL_ID>","self_mute":false,"self_deaf":false}}"#), "Entrar, moverse o salir (`channel_id: null`) de un canal de voz o llamada."),
    op(5, "Voice Server Ping", Send, None, Some(r#"{"op":5,"d":null}"#), "Pide revisar un servidor de voz que anda mal."),
    op(6, "Resume", Send, Some("gateway.rs · build_resume_payload"), Some(r#"{"op":6,"d":{"token":"<TOKEN>","session_id":"<SESSION_ID>","seq":1337}}"#), "Reanuda una sesión cortada."),
    op(7, "Reconnect", Receive, Some("gateway.rs · Outcome::Resume"), None, "Discord pide reconectar y reanudar."),
    op(8, "Request Guild Members", Send, Some("gateway.rs · request_members_payload"), Some(r#"{"op":8,"d":{"guild_id":["<GUILD_ID>"],"query":"","limit":0}}"#), "Por `query` (máx. 100) o por `user_ids` (máx. 100)."),
    op(9, "Invalid Session", Receive, Some("gateway.rs · Outcome::Resume/Reidentify"), None, "`d` indica si se puede reanudar."),
    op(10, "Hello", Receive, Some("gateway.rs · arranque de latidos"), None, "Trae `heartbeat_interval`."),
    op(11, "Heartbeat ACK", Receive, Some("gateway.rs · awaiting_ack"), None, "Respuesta a los opcodes 1 y 40."),
    op(12, "Guild Sync", Send, None, None, "Pedía miembros y presencias de los servers. Sin estructura documentada."),
    op(13, "Call Connect", Send, None, Some(r#"{"op":13,"d":{"channel_id":"<DM_CHANNEL_ID>"}}"#), "Pide los datos de una llamada que ya existía. Con la capability `AUTO_CALL_CONNECT` no hace falta."),
    old(14, "Guild Subscriptions", Send, None, None, "Reemplazado por el opcode 37. Sin estructura documentada."),
    op(15, "Lobby Connect", Send, None, None, "Unirse a un lobby. Sin estructura documentada."),
    op(16, "Lobby Disconnect", Send, None, None, "Salir de un lobby. Sin estructura documentada."),
    op(17, "Lobby Voice States", Send, None, Some(r#"{"op":17,"d":[{"lobby_id":"<LOBBY_ID>","self_mute":false,"self_deaf":false}]}"#), "`d` es una lista, una entrada por lobby."),
    op(18, "Stream Create", Send, Some("gateway.rs · stream_create_payload"), Some(r#"{"op":18,"d":{"type":"call","guild_id":null,"channel_id":"<CHANNEL_ID>","preferred_region":null}}"#), "Empezar a transmitir (Go Live). `type`: `guild` o `call`."),
    op(19, "Stream Delete", Send, Some("gateway.rs · stream_delete_payload"), Some(r#"{"op":19,"d":{"stream_key":"call:<CHANNEL_ID>:<USER_ID>"}}"#), "Dejar de ver, o cortar el propio stream."),
    op(20, "Stream Watch", Send, Some("gateway.rs · stream_watch_payload"), Some(r#"{"op":20,"d":{"stream_key":"call:<CHANNEL_ID>:<USER_ID>"}}"#), "Pedir ver un stream."),
    op(21, "Stream Ping", Send, None, Some(r#"{"op":21,"d":{"stream_key":"call:<CHANNEL_ID>:<USER_ID>"}}"#), "Pide revisar un servidor de stream que anda mal."),
    op(22, "Stream Set Paused", Send, Some("gateway.rs · stream_set_paused_payload"), Some(r#"{"op":22,"d":{"stream_key":"call:<CHANNEL_ID>:<USER_ID>","paused":true}}"#), "Solo el dueño del stream."),
    op(23, "LFG Subscriptions", Send, None, None, "Sin estructura documentada."),
    op(24, "Request Guild Application Commands", Send, None, None, "Sin estructura documentada."),
    op(25, "Embedded Activity Create", Send, None, None, "Lanzar una actividad embebida. Sin estructura documentada."),
    op(26, "Embedded Activity Delete", Send, None, None, "Sin estructura documentada."),
    op(27, "Embedded Activity Update", Send, None, None, "Sin estructura documentada."),
    op(28, "Request Forum Unreads", Send, None, None, "Sin estructura documentada."),
    op(29, "Remote Command", Send, None, Some(r#"{"op":29,"d":{"target_session_id":"<SESSION_ID>","payload":{}}}"#), "El `payload` no está estandarizado ni se valida."),
    op(30, "Request Deleted Entity IDs", Send, None, Some(r#"{"op":30,"d":{"guild_id":"<GUILD_ID>","channel_ids_hash":"<HASH>","role_ids_hash":"<HASH>","emoji_ids_hash":"<HASH>","sticker_ids_hash":"<HASH>"}}"#), "Hashes Murmur3 de 32 bits de los IDs ordenados y unidos por `,`."),
    op(31, "Request Soundboard Sounds", Send, None, Some(r#"{"op":31,"d":{"guild_ids":["<GUILD_ID>"]}}"#), "Responde con un SOUNDBOARD_SOUNDS por server."),
    old(32, "Speed Test Create", Send, None, Some(r#"{"op":32,"d":{"preferred_region":null}}"#), "Crea o actualiza un speed test de RTC."),
    old(33, "Speed Test Delete", Send, None, Some(r#"{"op":33,"d":null}"#), "Borra el speed test del cliente."),
    op(34, "Request Last Messages", Send, None, Some(r#"{"op":34,"d":{"guild_id":"<GUILD_ID>","channel_ids":["<CHANNEL_ID>"]}}"#), "Hasta 100 canales."),
    op(35, "Search Recent Members", Send, None, Some(r#"{"op":35,"d":{"guild_id":"<GUILD_ID>","query":"","continuation_token":null}}"#), "Busca entre los 10 000 miembros que entraron más recién."),
    old(36, "Request Channel Statuses", Send, None, Some(r#"{"op":36,"d":{"guild_id":"<GUILD_ID>"}}"#), "Reemplazado por el opcode 43."),
    op(37, "Guild Subscriptions Bulk", Send, Some("gateway.rs · member_list_subscribe_payload"), Some(r#"{"op":37,"d":{"subscriptions":{"<GUILD_ID>":{"typing":true,"activities":true,"threads":true,"member_updates":true,"members":[],"channels":{"<CHANNEL_ID>":[[0,99]]}}}}}"#), "Suscripciones de varios servers; así se pide la lista de miembros."),
    op(38, "Guild Channels Resync", Send, None, Some(r#"{"op":38,"d":{"guild_id":"<GUILD_ID>","obfuscated_channel_ids":["<CHANNEL_ID>"]}}"#), "Responde con CHANNEL_SYNC."),
    op(39, "Request Channel Member Count", Send, None, Some(r#"{"op":39,"d":{"guild_id":"<GUILD_ID>","channel_id":"<CHANNEL_ID>"}}"#), "Miembros que pueden ver el canal y cuántos están en línea."),
    op(40, "QoS Heartbeat", Send, Some("gateway.rs · build_qos_heartbeat_payload"), Some(r#"{"op":40,"d":{"seq":1337,"qos":{"ver":31,"active":true,"reasons":["foregrounded"]}}}"#), "El latido normal del cliente oficial. Motivos: `foregrounded`, `rtc_connected`."),
    op(41, "Update Time Spent Session ID", Send, Some("gateway.rs · build_time_spent_payload"), Some(r#"{"op":41,"d":{"initialization_timestamp":1753221861697,"session_id":"<HEARTBEAT_SESSION_ID>","client_launch_id":"<CLIENT_LAUNCH_ID>"}}"#), "Al conectar y cada 30 min (con foco o en llamada), siempre junto a un QoS heartbeat. Cuenta regresiva en Cliente → Seguimiento."),
    op(42, "Lobby Voice Server Ping", Send, None, Some(r#"{"op":42,"d":{"lobby_id":"<LOBBY_ID>"}}"#), "Pide revisar el servidor de voz de un lobby."),
    op(43, "Request Channel Info", Send, None, Some(r#"{"op":43,"d":{"guild_id":"<GUILD_ID>","fields":["status","voice_start_time"]}}"#), "Campos extra de los canales de voz. Responde con CHANNEL_INFO."),
];

/// Opcodes del Gateway de voz (0–32). Los binarios (MLS) no se pueden mandar
/// desde el campo de texto, así que no llevan plantilla.
pub(super) static VOICE: [Op; 34] = [
    op(0, "Identify", Send, Some("voice/gateway.rs"), None, "Abre la conexión de voz."),
    op(1, "Select Protocol", Send, Some("voice/gateway.rs"), None, "Elige el protocolo y el modo de cifrado."),
    op(2, "Ready", Receive, Some("voice.rs · VOICE_OP_READY"), None, "Completa el handshake."),
    op(3, "Heartbeat", Both, Some("voice.rs · VOICE_OP_HEARTBEAT"), None, ""),
    op(4, "Session Description", Receive, Some("voice.rs · VOICE_OP_SESSION_DESCRIPTION"), None, "Trae la clave de la sesión."),
    op(5, "Speaking", Both, Some("voice.rs · VOICE_OP_SPEAKING"), None, "Quién está hablando."),
    op(6, "Heartbeat ACK", Receive, Some("voice.rs · VOICE_OP_HEARTBEAT_ACK"), None, ""),
    op(7, "Resume", Send, Some("voice.rs · VOICE_OP_RESUME"), None, ""),
    op(8, "Hello", Receive, Some("voice.rs · VOICE_OP_HELLO"), None, "Trae `heartbeat_interval`."),
    op(9, "Resumed", Receive, Some("voice.rs · VOICE_OP_RESUMED"), None, ""),
    op(10, "Signal", Both, None, None, "Señalización WebRTC para conexiones P2P."),
    op(11, "Reset", Both, None, None, "Reinicia la conexión de voz. (Comparte número con Clients Connect.)"),
    op(11, "Clients Connect", Receive, Some("voice.rs · VOICE_OP_CLIENTS_CONNECT"), None, "Entraron clientes al canal."),
    op(12, "Video", Both, Some("voice.rs · VOICE_OP_VIDEO"), None, "Describe la sesión de video."),
    op(13, "Client Disconnect", Receive, Some("voice.rs · VOICE_OP_CLIENT_DISCONNECT"), None, ""),
    op(14, "Session Update", Both, Some("voice.rs · VOICE_OP_SESSION_UPDATE"), None, ""),
    op(15, "Media Sink Wants", Both, Some("voice.rs · VOICE_OP_MEDIA_SINK_WANTS"), None, "Streams que se quieren recibir (simulcast)."),
    op(16, "Voice Backend Version", Both, None, None, "Versión del backend de voz."),
    op(17, "Channel Options Update", Receive, None, None, "Cambió alguna propiedad de la conexión de voz."),
    op(18, "Client Flags", Receive, Some("voice.rs · VOICE_OP_CLIENT_FLAGS"), None, ""),
    op(19, "Speed Test", Receive, None, None, "Resultados de un speed test."),
    op(20, "Client Platform", Receive, Some("voice.rs · VOICE_OP_CLIENT_PLATFORM"), None, ""),
    op(21, "DAVE Protocol Prepare Transition", Receive, Some("voice.rs · VOICE_OP_DAVE_PREPARE_TRANSITION"), None, ""),
    op(22, "DAVE Protocol Execute Transition", Receive, Some("voice.rs · VOICE_OP_DAVE_EXECUTE_TRANSITION"), None, ""),
    op(23, "DAVE Protocol Transition Ready", Send, Some("voice.rs · VOICE_OP_DAVE_TRANSITION_READY"), None, ""),
    op(24, "DAVE Protocol Prepare Epoch", Receive, Some("voice.rs · VOICE_OP_DAVE_PREPARE_EPOCH"), None, ""),
    op(25, "MLS External Sender Package", Receive, Some("voice.rs · VOICE_OP_DAVE_MLS_EXTERNAL_SENDER"), None, "Binario."),
    op(26, "MLS Key Package", Send, Some("voice.rs · VOICE_OP_DAVE_MLS_KEY_PACKAGE"), None, "Binario."),
    op(27, "MLS Proposals", Receive, Some("voice.rs · VOICE_OP_DAVE_MLS_PROPOSALS"), None, "Binario."),
    op(28, "MLS Commit Welcome", Send, Some("voice.rs · VOICE_OP_DAVE_MLS_COMMIT_WELCOME"), None, "Binario."),
    op(29, "MLS Announce Commit Transition", Receive, Some("voice.rs · VOICE_OP_DAVE_MLS_ANNOUNCE_COMMIT_TRANSITION"), None, ""),
    op(30, "MLS Welcome", Receive, Some("voice.rs · VOICE_OP_DAVE_MLS_WELCOME"), None, "Binario."),
    op(31, "MLS Invalid Commit Welcome", Send, Some("voice.rs · VOICE_OP_DAVE_MLS_INVALID_COMMIT_WELCOME"), None, ""),
    op(32, "No Route", Send, None, None, "Avisa que no hubo ruta RTC disponible."),
];

/// Cuántos opcodes de `list` eCord todavía no implementa.
pub(super) fn missing(list: &[Op]) -> usize {
    list.iter().filter(|op| op.ecord.is_none()).count()
}

/// Los opcodes (del Gateway principal) que tienen plantilla para mandar.
pub(super) fn templates() -> impl Iterator<Item = &'static Op> {
    GATEWAY.iter().filter(|op| op.template.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_codes_are_contiguous_and_unique() {
        for (index, op) in GATEWAY.iter().enumerate() {
            assert_eq!(op.code as usize, index, "{} fuera de lugar", op.name);
        }
    }

    #[test]
    fn voice_codes_are_ascending() {
        let mut previous = 0;
        for op in &VOICE {
            assert!(op.code >= previous, "{} fuera de orden", op.name);
            previous = op.code;
        }
    }

    #[test]
    fn templates_are_valid_json_with_matching_op() {
        for op in templates() {
            let value: serde_json::Value =
                serde_json::from_str(op.template.unwrap()).unwrap_or_else(|e| panic!("{}: {e}", op.name));
            assert_eq!(value["op"], op.code, "{}", op.name);
        }
    }

    #[test]
    fn op_41_is_marked_as_used_by_ecord() {
        let op = &GATEWAY[41];
        assert_eq!(op.name, "Update Time Spent Session ID");
        assert!(op.ecord.is_some());
    }
}
