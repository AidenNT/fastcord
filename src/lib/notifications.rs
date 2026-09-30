//! Lógica (sin UI) del sistema de notificaciones:
//!
//! - **Política** (`policy`): qué superficies están permitidas ahora mismo
//!   según el `PreloadedUserSettings` de la cuenta (`notifications` y
//!   `status`, ver `discord::user_settings`).
//! - **Silenciados** (`MuteRules`): servers/canales/DMs silenciados y nivel
//!   de mensajes. Eso NO viaja en el proto de settings sino en el `READY`
//!   (`user_guild_settings`), así que se lee de ahí.
//! - **Texto** (`preview`, `author_name`): lo que se muestra en la
//!   notificación, con `<@id>`/`<#id>`/emojis ya convertidos a algo legible.
//! - **Escritorio** (`send_desktop`): notificación nativa del SO.
//!
//! Quién llama a esto y qué hace con el resultado (contadores, toast dentro
//! de la app, notificación de escritorio) vive en `lib::state::App`.

use std::collections::HashMap;

use crate::discord::models::{GatewayMessage, PrivateChannel};
use crate::discord::user_settings::PreloadedUserSettings;

// `message_notifications` de Discord.
const LEVEL_ALL: i64 = 0;
const LEVEL_NOTHING: i64 = 2;
const LEVEL_INHERIT: i64 = 3;

/// Máximo de caracteres del cuerpo de una notificación.
const PREVIEW_CHARS: usize = 140;

// ---------------------------------------------------------------------
// Política según los ajustes de la cuenta
// ---------------------------------------------------------------------

/// Qué superficies de notificación se pueden usar ahora mismo. El contador
/// de menciones NO depende de esto: se cuenta siempre (como el cliente
/// oficial, que muestra el badge aunque estés en "No molestar").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Tarjeta dentro de la app (cuando la ventana tiene el foco).
    pub in_app: bool,
    /// Notificación del escritorio (cuando la ventana NO tiene el foco).
    pub desktop: bool,
}

/// Lee `notifications` y `status` de los ajustes de la cuenta:
///
/// - `notifications.show_in_app_notifications` (default: activado) apaga la
///   tarjeta dentro de la app.
/// - `notifications.quiet_mode` y `notifications.focus_mode_expires_at_ms`
///   (modo concentración vigente) apagan las dos.
/// - Estado "No molestar" (`status.status == "dnd"`) apaga las dos.
///
/// Sin settings cargados todavía (falló el pedido, o es una pantalla demo)
/// se permite todo: mejor avisar de más que perder una mención.
pub fn policy(settings: Option<&PreloadedUserSettings>) -> Policy {
    let Some(settings) = settings else {
        return Policy { in_app: true, desktop: true };
    };

    let mut show_in_app = true;
    let mut quiet = false;
    if let Some(n) = settings.notifications.as_ref() {
        show_in_app = n.show_in_app_notifications.unwrap_or(true);
        quiet = n.quiet_mode.unwrap_or(false) || n.focus_mode_expires_at_ms > now_ms();
    }
    let dnd = settings
        .status
        .as_ref()
        .and_then(|s| s.status.as_deref())
        == Some("dnd");

    let suppressed = quiet || dnd;
    Policy {
        in_app: show_in_app && !suppressed,
        desktop: !suppressed,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------
// Silenciados / nivel de notificación (viene en el READY)
// ---------------------------------------------------------------------

/// Qué tipo de mención tiene un mensaje para esta cuenta.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mention {
    /// `<@mi_id>`, o una respuesta a un mensaje mío (Discord me mete en
    /// `mentions` cuando la respuesta pinga).
    pub direct: bool,
    /// `@everyone` / `@here`.
    pub everyone: bool,
    /// Un rol que yo tengo en ese server.
    pub role: bool,
}

#[derive(Clone, Debug, Default)]
struct ChannelRule {
    muted: bool,
    level: Option<i64>,
}

#[derive(Clone, Debug, Default)]
struct GuildRule {
    muted: bool,
    level: Option<i64>,
    suppress_everyone: bool,
    suppress_roles: bool,
    channels: HashMap<String, ChannelRule>,
}

/// Reglas de silenciado de la cuenta. Los DMs silenciados viajan en la
/// entrada con `guild_id: null`, que acá se guarda bajo la clave `""`.
#[derive(Clone, Debug, Default)]
pub struct MuteRules {
    guilds: HashMap<String, GuildRule>,
}

impl MuteRules {
    /// Interpreta `READY.user_guild_settings`. Tolerante a propósito: lo que
    /// no entienda lo ignora (queda sin reglas = notifica), nunca falla.
    pub fn from_ready(value: &serde_json::Value) -> Self {
        // Las cuentas de usuario lo reciben como `{ "entries": [...] }`; por
        // las dudas también acepto un array pelado.
        let entries = value
            .get("entries")
            .and_then(|v| v.as_array())
            .or_else(|| value.as_array());

        let mut guilds = HashMap::new();
        for entry in entries.into_iter().flatten() {
            let key = entry
                .get("guild_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let mut rule = GuildRule {
                muted: muted_now(entry),
                level: level_of(entry),
                suppress_everyone: entry
                    .get("suppress_everyone")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                suppress_roles: entry
                    .get("suppress_roles")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                channels: HashMap::new(),
            };

            let overrides = entry.get("channel_overrides").and_then(|v| v.as_array());
            for over in overrides.into_iter().flatten() {
                let Some(channel_id) = over.get("channel_id").and_then(|v| v.as_str()) else {
                    continue;
                };
                rule.channels.insert(
                    channel_id.to_string(),
                    ChannelRule {
                        muted: muted_now(over),
                        level: level_of(over),
                    },
                );
            }
            guilds.insert(key, rule);
        }
        Self { guilds }
    }

    /// `true` si un mensaje debe contar como mención y notificar, según lo
    /// que la cuenta tiene silenciado / el nivel elegido.
    ///
    /// - DM (`guild_id == None`): siempre, salvo que ese DM esté silenciado.
    /// - Server: solo con mención (directa, `@everyone` o un rol propio),
    ///   salvo que el nivel del canal/server sea "todos los mensajes". Un
    ///   canal o server silenciado (o en nivel "nada") nunca cuenta.
    pub fn passes(&self, guild_id: Option<&str>, channel_id: &str, mention: Mention) -> bool {
        let default_rule = GuildRule::default();
        let rule = self
            .guilds
            .get(guild_id.unwrap_or(""))
            .unwrap_or(&default_rule);
        let channel = rule.channels.get(channel_id);

        if channel.is_some_and(|c| c.muted) {
            return false;
        }
        // Un DM no tiene "server": solo cuenta la regla del propio canal.
        if guild_id.is_none() {
            return channel.and_then(|c| c.level) != Some(LEVEL_NOTHING);
        }
        if rule.muted {
            return false;
        }

        // El nivel del canal (si no hereda) pisa al del server.
        let level = channel
            .and_then(|c| c.level)
            .filter(|l| *l != LEVEL_INHERIT)
            .or(rule.level.filter(|l| *l != LEVEL_INHERIT));
        match level {
            Some(LEVEL_NOTHING) => return false,
            Some(LEVEL_ALL) => return true,
            _ => {}
        }

        let everyone = mention.everyone && !rule.suppress_everyone;
        let role = mention.role && !rule.suppress_roles;
        mention.direct || everyone || role
    }
}

/// `muted` de una entrada/override, respetando el silencio temporal
/// (`mute_config.end_time`): si ya venció, no cuenta como silenciado.
fn muted_now(obj: &serde_json::Value) -> bool {
    if !obj.get("muted").and_then(|v| v.as_bool()).unwrap_or(false) {
        return false;
    }
    let end = obj
        .get("mute_config")
        .and_then(|c| c.get("end_time"))
        .and_then(|v| v.as_str());
    match end {
        None => true,
        Some(text) => chrono::DateTime::parse_from_rfc3339(text)
            .map(|t| t > chrono::Utc::now())
            .unwrap_or(true),
    }
}

fn level_of(obj: &serde_json::Value) -> Option<i64> {
    obj.get("message_notifications").and_then(|v| v.as_i64())
}

// ---------------------------------------------------------------------
// Orden de la lista de DMs
// ---------------------------------------------------------------------

/// Snowflake de Discord como número (los más nuevos son más grandes). Un id
/// inválido vale 0.
pub fn snowflake(id: &str) -> u64 {
    id.parse().unwrap_or(0)
}

/// Clave para ordenar la lista de DMs como el cliente oficial: por el id del
/// último mensaje; si el DM nunca tuvo mensajes, por el id del canal (o sea,
/// cuándo se creó).
pub fn dm_sort_key(dm: &PrivateChannel) -> u64 {
    dm.last_message_id
        .as_deref()
        .map(snowflake)
        .filter(|v| *v != 0)
        .unwrap_or_else(|| snowflake(&dm.id))
}

// ---------------------------------------------------------------------
// Texto de la notificación
// ---------------------------------------------------------------------

/// Nombre del autor: el apodo en el server si lo hay, si no el nombre global.
pub fn author_name(msg: &GatewayMessage) -> String {
    msg.member
        .as_ref()
        .and_then(|m| m.nick.as_deref())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| msg.author.display_name())
        .to_string()
}

/// Cuerpo de la notificación: el contenido con las menciones/emojis
/// convertidos a texto legible, en una sola línea y recortado. Si el mensaje
/// no tiene texto (solo adjunto/sticker) dice qué mandó.
pub fn preview(msg: &GatewayMessage) -> String {
    let cleaned = clean_content(msg);
    let one_line = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        let fallback = if !msg.attachments.is_empty() {
            "Envió un archivo adjunto"
        } else if !msg.sticker_items.is_empty() {
            "Envió un sticker"
        } else {
            "Envió un mensaje"
        };
        return fallback.to_string();
    }
    truncate_chars(&one_line, PREVIEW_CHARS)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Reemplaza `<@id>`, `<@!id>`, `<@&id>`, `<#id>` y `<:emoji:id>` por texto.
fn clean_content(msg: &GatewayMessage) -> String {
    let mut out = String::with_capacity(msg.content.len());
    let mut rest = msg.content.as_str();
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        match after.find('>') {
            Some(end) => {
                match render_token(&after[1..end], msg) {
                    Some(text) => out.push_str(&text),
                    None => out.push_str(&after[..=end]),
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_snowflake_text(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_digit())
}

fn render_token(token: &str, msg: &GatewayMessage) -> Option<String> {
    if let Some(id) = token.strip_prefix("@&") {
        return is_snowflake_text(id).then(|| "@rol".to_string());
    }
    if let Some(id) = token.strip_prefix("@!").or_else(|| token.strip_prefix('@')) {
        if !is_snowflake_text(id) {
            return None;
        }
        let name = msg
            .mentions
            .iter()
            .find(|u| u.id == id)
            .map(|u| u.display_name().to_string())
            .unwrap_or_else(|| "usuario".to_string());
        return Some(format!("@{name}"));
    }
    if let Some(id) = token.strip_prefix('#') {
        return is_snowflake_text(id).then(|| "#canal".to_string());
    }
    // Emoji personalizado: `<:nombre:id>` o animado `<a:nombre:id>`.
    let emoji = token.strip_prefix("a:").or_else(|| token.strip_prefix(':'))?;
    let name = emoji.split(':').next()?;
    if name.is_empty() {
        return None;
    }
    Some(format!(":{name}:"))
}

// ---------------------------------------------------------------------
// Notificación del escritorio
// ---------------------------------------------------------------------

/// Manda una notificación nativa del SO. Va en un hilo aparte porque
/// `notify-rust` puede bloquear (D-Bus en Linux) y esto se llama desde el
/// hilo de la UI. Si falla (sin servicio de notificaciones, etc.) solo lo
/// deja en el log: una notificación perdida no justifica molestar al usuario.
pub fn send_desktop(title: String, body: String) {
    std::thread::spawn(move || {
        let result = notify_rust::Notification::new()
            .appname("eCord")
            .summary(&title)
            .body(&body)
            .show();
        if let Err(err) = result {
            log::warn!("no se pudo mostrar la notificación de escritorio: {err}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(value: serde_json::Value) -> GatewayMessage {
        serde_json::from_value(value).expect("mensaje válido")
    }

    fn dm(id: &str, last: Option<&str>) -> PrivateChannel {
        serde_json::from_value(json!({ "type": 1, "id": id, "last_message_id": last }))
            .expect("dm válido")
    }

    fn direct() -> Mention {
        Mention { direct: true, ..Default::default() }
    }

    #[test]
    fn dms_sort_by_last_message_then_channel_creation() {
        let mut dms = vec![dm("1", Some("50")), dm("2", Some("90")), dm("70", None)];
        dms.sort_by_key(|d| std::cmp::Reverse(dm_sort_key(d)));
        let ids: Vec<&str> = dms.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["2", "70", "1"]);
    }

    #[test]
    fn server_without_rules_only_counts_mentions() {
        let rules = MuteRules::default();
        assert!(rules.passes(Some("g"), "c", direct()));
        assert!(rules.passes(Some("g"), "c", Mention { everyone: true, ..Default::default() }));
        assert!(!rules.passes(Some("g"), "c", Mention::default()));
        // Un DM siempre cuenta.
        assert!(rules.passes(None, "c", Mention::default()));
    }

    #[test]
    fn muted_server_and_channel_are_ignored() {
        let rules = MuteRules::from_ready(&json!({ "entries": [
            { "guild_id": "g1", "muted": true, "channel_overrides": [] },
            { "guild_id": "g2", "muted": false, "suppress_everyone": true,
              "channel_overrides": [ { "channel_id": "c9", "muted": true } ] },
            { "guild_id": null, "channel_overrides": [ { "channel_id": "dm1", "muted": true } ] },
        ]}));
        assert!(!rules.passes(Some("g1"), "c", direct()));
        assert!(!rules.passes(Some("g2"), "c9", direct()));
        assert!(rules.passes(Some("g2"), "c", direct()));
        assert!(!rules.passes(Some("g2"), "c", Mention { everyone: true, ..Default::default() }));
        assert!(!rules.passes(None, "dm1", Mention::default()));
        assert!(rules.passes(None, "dm2", Mention::default()));
    }

    #[test]
    fn expired_mute_does_not_count_as_muted() {
        let rules = MuteRules::from_ready(&json!({ "entries": [
            { "guild_id": "g", "muted": true,
              "mute_config": { "end_time": "2020-01-01T00:00:00+00:00" } },
        ]}));
        assert!(rules.passes(Some("g"), "c", direct()));
    }

    #[test]
    fn channel_level_all_notifies_without_mention() {
        let rules = MuteRules::from_ready(&json!({ "entries": [
            { "guild_id": "g", "message_notifications": 1,
              "channel_overrides": [ { "channel_id": "c", "message_notifications": 0 } ] },
        ]}));
        assert!(rules.passes(Some("g"), "c", Mention::default()));
        assert!(!rules.passes(Some("g"), "otro", Mention::default()));
    }

    #[test]
    fn preview_resolves_mentions_and_emojis() {
        let m = msg(json!({
            "id": "1", "channel_id": "2",
            "content": "hola <@123>, mirá <#456> <:risa:789>\nchau",
            "author": { "id": "9", "username": "ana" },
            "mentions": [ { "id": "123", "username": "beto", "global_name": "Beto" } ],
        }));
        assert_eq!(preview(&m), "hola @Beto, mirá #canal :risa: chau");
    }

    #[test]
    fn preview_of_empty_message_says_what_was_sent() {
        let m = msg(json!({
            "id": "1", "channel_id": "2", "content": "",
            "author": { "id": "9", "username": "ana" },
            "attachments": [ { "id": "5", "filename": "a.png", "size": 1, "url": "u", "proxy_url": "u" } ],
        }));
        assert_eq!(preview(&m), "Envió un archivo adjunto");
    }
}
