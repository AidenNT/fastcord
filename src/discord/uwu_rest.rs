//! Reemplazo de `rest.rs` que usa el transporte vendorizado de UwUDev
//! (`vendor/discord_client_rest`, con `discord_client_structs`/`macros`/
//! `utils` como hermanos bajo `vendor/`) en vez de `reqwest` a mano.
//!
//! Deliberadamente NO usamos los structs tipados de `discord_client_structs`
//! (`Channel`, `Message`, `User` de esa crate) para las respuestas: son un
//! sistema de tipos aparte del nuestro (`crate::discord::models`), y
//! reconstruir a mano su `MessageBuilder`/`MessageReferenceBuilder` sin
//! poder compilar acá mismo para probarlo es la parte con más chance de
//! quedar rota. En cambio pedimos las respuestas como JSON crudo
//! (`serde_json::Value`) y las parseamos con `crate::discord::models`, que
//! es exactamente lo que hacía `rest.rs` con `reqwest` -- mismos paths,
//! mismos bodies, mismos modelos. Lo que cambia es el transporte: en vez de
//! `reqwest::Client` normal usamos el `RestClient` de UwUDev, que impersona
//! TLS/HTTP2 de Chrome (`wreq`+`wreq-util`) y trae rate limiting real.
//!
//! El login (obtener el token) sigue siendo cosa de `remote_auth.rs`
//! (QR) / `password_auth.rs`: la librería de UwUDev no tiene endpoint de
//! login con usuario/contraseña ni el protocolo de QR remoto, solo sabe
//! validar un token que ya tenés (`RestClient::connect` le pega a
//! `/experiments` y falla si el token es inválido). Lo que SÍ se mudó acá
//! es ese paso de "validar el token y armar la sesión autenticada": antes
//! no existía explícitamente (se armaba un `reqwest::Client` sin más), y
//! ahora `UwuRest::for_token` hace esa validación real contra Discord antes
//! de dejarte pedir nada.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use discord_client_rest::rest::{RequestProperties, RequestPropertiesBuilder, RestClient as UwuInner};
use discord_client_rest::structs::referer::{DmChannelReferer, GuildChannelReferer, Referer};
use discord_client_structs::structs::client::BuildNumbers;
use discord_client_structs::structs::message::{Message, MessageBuilder, MessageReferenceBuilder};
use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;

use super::fingerprint::{CLIENT_BUILD_NUMBER, ClientFingerprint};
use super::models::{
    Channel, ForumPage, GatewayMessage, ThreadChannel, User, UserProfileResponse,
};

/// Cuántos mensajes se piden por página (igual que en `rest.rs`).
pub const MESSAGES_PAGE_SIZE: u8 = 20;

/// Cuántos posts de foro se piden por página.
pub const FORUM_PAGE_SIZE: u8 = 25;

// --- Fingerprint compartido -------------------------------------------------
//
// Mismo patrón que `rest::set_shared_fingerprint`/`shared_fingerprint`: lo
// carga `discord::init_fingerprint()` una sola vez al arrancar el login y
// de ahí en más cualquier `UwuRest::for_token` lo reusa, para que el build
// number que ve Discord sea siempre el mismo en REST, gateway y QR.
static SHARED_FINGERPRINT: RwLock<Option<Arc<ClientFingerprint>>> = RwLock::new(None);

pub(super) fn set_shared_fingerprint(fingerprint: Arc<ClientFingerprint>) {
    if let Ok(mut guard) = SHARED_FINGERPRINT.write() {
        *guard = Some(fingerprint);
    }
}

fn shared_fingerprint() -> Arc<ClientFingerprint> {
    if let Ok(guard) = SHARED_FINGERPRINT.read() {
        if let Some(fingerprint) = guard.as_ref() {
            return Arc::clone(fingerprint);
        }
    }
    Arc::new(ClientFingerprint::new(CLIENT_BUILD_NUMBER))
}

// --- Cliente conectado, cacheado por token ----------------------------------
//
// `RestClient::connect` de UwUDev hace una llamada de red (bootstrap +
// validación del token) cada vez que se construye. Conectar de nuevo en
// cada `spawn_fetch_*` sería un pedido extra a Discord por cada click de
// la UI, así que se cachea una sola instancia por token.
static SHARED_CLIENT: OnceLock<AsyncMutex<Option<(String, Arc<UwuInner>)>>> = OnceLock::new();

async fn connected_client(token: &str) -> anyhow::Result<Arc<UwuInner>> {
    let cell = SHARED_CLIENT.get_or_init(|| AsyncMutex::new(None));
    let mut guard = cell.lock().await;

    if let Some((cached_token, client)) = guard.as_ref() {
        if cached_token == token {
            return Ok(Arc::clone(client));
        }
    }

    let fingerprint = shared_fingerprint();
    let build_numbers = BuildNumbers::new(fingerprint.client_build_number as u32, None);

    let inner = UwuInner::connect(
        token.to_string(),
        None,                 // custom_api_version: que lo detecte solo
        Some(build_numbers),  // ← mismo build number que fingerprint/gateway
        None,                 // client_session: que genere uno propio
        None,                 // proxy
    )
    .await
    .map_err(|e| anyhow::anyhow!("no se pudo validar el token con el REST de uwudev: {e}"))?;

    let client = Arc::new(inner);
    *guard = Some((token.to_string(), Arc::clone(&client)));
    Ok(client)
}

/// Percent-encoding mínimo para meter un emoji en un path de la URL
/// (idéntico al de `rest.rs`, copiado acá para no depender de ese módulo).
fn percent_encode_emoji(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Debug: con `ECORD_DUMP_PROFILE=1` guarda la respuesta cruda de
/// `GET /users/{id}/profile` en `profile_dump_<id>.json` (carpeta desde la que
/// se corre el cliente). Sirve para ver qué campos manda Discord (efectos,
/// frames, decoraciones) sin tener que interceptar el tráfico.
fn dump_profile_if_requested(user_id: &str, value: &Value) {
    dump_json_if_requested("profile_dump", user_id, value);
}

/// Igual, para cualquier respuesta: guarda `<tag>_<id>.json`.
fn dump_json_if_requested(tag: &str, id: &str, value: &Value) {
    if std::env::var("ECORD_DUMP_PROFILE").as_deref() != Ok("1") {
        return;
    }
    let safe_id: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let name = format!("{tag}_{safe_id}.json");
    match serde_json::to_string_pretty(value) {
        Ok(text) => match std::fs::write(&name, text) {
            Ok(()) => log::warn!("ECORD_DUMP_PROFILE=1: respuesta cruda guardada en {name}"),
            Err(e) => log::warn!("ECORD_DUMP_PROFILE=1: no se pudo escribir {name}: {e}"),
        },
        Err(e) => log::warn!("ECORD_DUMP_PROFILE=1: no se pudo serializar el perfil: {e}"),
    }
}

fn err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// Reemplazo de `rest::RestClient`. Mismos métodos, misma pinta de uso;
/// la diferencia es que construirlo valida el token contra Discord (por
/// eso `for_token` es async y falible, a diferencia del viejo que era
/// sync e infalible).
pub struct UwuRest {
    client: Arc<UwuInner>,
}

impl UwuRest {
    pub async fn for_token(token: String) -> anyhow::Result<Self> {
        let client = connected_client(&token).await?;
        Ok(Self { client })
    }

    fn home() -> RequestProperties {
        RequestProperties::home()
    }

    pub async fn guild_channels(&self, guild_id: &str) -> anyhow::Result<Vec<Channel>> {
        let path = format!("guilds/{guild_id}/channels");
        let value: Value = self
            .client
            .get(&path, None, Some(Self::home()))
            .await
            .map_err(err)?;
        serde_json::from_value(value).map_err(err)
    }

    pub async fn channel_messages(
        &self,
        channel_id: &str,
        limit: u8,
    ) -> anyhow::Result<Vec<GatewayMessage>> {
        let mut query = HashMap::new();
        query.insert("limit".to_string(), limit.to_string());
        let path = format!("channels/{channel_id}/messages");
        let value: Value = self
            .client
            .get(&path, Some(query), Some(Self::home()))
            .await
            .map_err(err)?;
        serde_json::from_value(value).map_err(err)
    }

    pub async fn channel_messages_before(
        &self,
        channel_id: &str,
        limit: u8,
        before_message_id: &str,
    ) -> anyhow::Result<Vec<GatewayMessage>> {
        let mut query = HashMap::new();
        query.insert("limit".to_string(), limit.to_string());
        query.insert("before".to_string(), before_message_id.to_string());
        let path = format!("channels/{channel_id}/messages");
        let value: Value = self
            .client
            .get(&path, Some(query), Some(Self::home()))
            .await
            .map_err(err)?;
        serde_json::from_value(value).map_err(err)
    }

    /// Una página de posts de un foro (`GET /channels/{id}/threads/search`,
    /// el mismo endpoint que usa el cliente oficial). `by_creation` ordena
    /// por fecha de creación; si no, por actividad reciente. Cada hilo y
    /// cada primer mensaje se parsean por separado, así uno raro no tira
    /// abajo toda la página.
    pub async fn forum_threads(
        &self,
        channel_id: &str,
        offset: u32,
        by_creation: bool,
    ) -> anyhow::Result<ForumPage> {
        let mut query = HashMap::new();
        let sort_by = if by_creation { "creation_time" } else { "last_message_time" };
        query.insert("sort_by".to_string(), sort_by.to_string());
        query.insert("sort_order".to_string(), "desc".to_string());
        query.insert("limit".to_string(), FORUM_PAGE_SIZE.to_string());
        query.insert("offset".to_string(), offset.to_string());
        query.insert("tag_setting".to_string(), "match_some".to_string());
        let path = format!("channels/{channel_id}/threads/search");
        let value: Value = self
            .client
            .get(&path, Some(query), Some(Self::home()))
            .await
            .map_err(err)?;
        let list = |key: &str| -> Vec<Value> {
            value.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
        };
        Ok(ForumPage {
            threads: list("threads")
                .into_iter()
                .filter_map(|t| serde_json::from_value(t).ok())
                .collect(),
            first_messages: list("first_messages")
                .into_iter()
                .filter_map(|m| serde_json::from_value(m).ok())
                .collect(),
            has_more: value.get("has_more").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// Crea un post en un foro (`POST /channels/{id}/threads`). Devuelve el
    /// hilo creado.
    pub async fn create_forum_post(
        &self,
        forum_id: &str,
        title: &str,
        content: &str,
        tag_ids: &[String],
    ) -> anyhow::Result<ThreadChannel> {
        let body = json!({
            "name": title,
            "auto_archive_duration": 4320,
            "applied_tags": tag_ids,
            "message": { "content": content },
        });
        let path = format!("channels/{forum_id}/threads");
        let value: Value = self
            .client
            .post(&path, Some(body), Some(Self::home()))
            .await
            .map_err(err)?;
        serde_json::from_value(value).map_err(err)
    }

    /// Crea un hilo a partir de un mensaje
    /// (`POST /channels/{id}/messages/{id}/threads`). El hilo que nace de un
    /// mensaje tiene el mismo id que el mensaje.
    pub async fn create_thread_from_message(
        &self,
        channel_id: &str,
        message_id: &str,
        name: &str,
    ) -> anyhow::Result<ThreadChannel> {
        let body = json!({ "name": name, "auto_archive_duration": 4320 });
        let path = format!("channels/{channel_id}/messages/{message_id}/threads");
        let value: Value = self
            .client
            .post(&path, Some(body), Some(Self::home()))
            .await
            .map_err(err)?;
        serde_json::from_value(value).map_err(err)
    }

    /// `nonce` de una interacción: un snowflake "de mentira" armado con la
    /// hora actual, como hace el cliente oficial.
    fn interaction_nonce() -> String {
        const DISCORD_EPOCH_MS: u128 = 1_420_070_400_000;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(DISCORD_EPOCH_MS);
        ((now.saturating_sub(DISCORD_EPOCH_MS)) << 22).to_string()
    }

    /// Aprieta un botón de un mensaje (`POST /interactions`, tipo 3).
    pub async fn press_button(
        &self,
        ctx: &super::InteractionContext,
        message_id: &str,
        message_flags: u64,
        custom_id: &str,
    ) -> anyhow::Result<()> {
        let mut body = json!({
            "type": 3,
            "nonce": Self::interaction_nonce(),
            "channel_id": ctx.channel_id,
            "message_id": message_id,
            "message_flags": message_flags,
            "application_id": ctx.application_id,
            "session_id": ctx.session_id,
            "data": { "component_type": 2, "custom_id": custom_id },
        });
        if let Some(guild_id) = &ctx.guild_id {
            body["guild_id"] = json!(guild_id);
        }
        let _: Value = self
            .client
            .post("interactions", Some(body), Some(Self::home()))
            .await
            .map_err(err)?;
        Ok(())
    }

    /// Manda un modal completado (`POST /interactions`, tipo 5).
    ///
    /// `components` son los del modal que abrió el bot (`ModalRequest`): el
    /// submit replica su estructura (filas de acciones en los modales viejos,
    /// etiquetas en los nuevos) con el `value` de cada campo, que es lo que
    /// hace el cliente oficial. Envolver siempre cada campo en una fila de
    /// acciones rompe los modales con etiquetas (Discord da un 500).
    pub async fn submit_modal(
        &self,
        ctx: &super::InteractionContext,
        modal_interaction_id: &str,
        modal_custom_id: &str,
        components: &[super::models::Component],
        fields: &[(String, String)],
    ) -> anyhow::Result<()> {
        let values: HashMap<String, String> = fields.iter().cloned().collect();
        let mut rows: Vec<Value> = components
            .iter()
            .filter_map(|c| c.to_modal_submit(&values))
            .collect();
        if rows.is_empty() {
            // Sin estructura conocida: formato viejo, un campo por fila.
            rows = fields
                .iter()
                .map(|(custom_id, value)| {
                    json!({
                        "type": 1,
                        "components": [{ "type": 4, "custom_id": custom_id, "value": value }],
                    })
                })
                .collect();
        }
        let mut body = json!({
            "type": 5,
            "nonce": Self::interaction_nonce(),
            "channel_id": ctx.channel_id,
            "application_id": ctx.application_id,
            "session_id": ctx.session_id,
            "data": {
                "id": modal_interaction_id,
                "custom_id": modal_custom_id,
                "components": rows,
            },
        });
        if let Some(guild_id) = &ctx.guild_id {
            body["guild_id"] = json!(guild_id);
        }
        let _: Value = self
            .client
            .post("interactions", Some(body), Some(Self::home()))
            .await
            .map_err(err)?;
        Ok(())
    }

    pub async fn open_dm(&self, user_id: &str) -> anyhow::Result<String> {
        let body = json!({ "recipient_id": user_id });
        let value: Value = self
            .client
            .post("users/@me/channels", Some(body), Some(Self::home()))
            .await
            .map_err(err)?;
        value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("Discord no devolvió el id del canal de DM"))
    }

    /// `POST /channels/{id}/messages` armando el cuerpo con `MessageBuilder`
    /// (y `MessageReferenceBuilder` para las respuestas) y mandando el
    /// `Referer` del canal desde el que se escribe, como el cliente oficial:
    /// `/channels/{guild}/{canal}` en un server, `/channels/@me/{canal}` en un
    /// DM (`guild_id` = `None`). Devuelve el mensaje que creó Discord.
    pub async fn send_message(
        &self,
        channel_id: &str,
        guild_id: Option<&str>,
        content: &str,
        reply_to: Option<&str>,
    ) -> anyhow::Result<Message> {
        let channel_id_num: u64 = channel_id.parse().map_err(err)?;
        let guild_id_num: Option<u64> = guild_id
            .filter(|g| !g.is_empty())
            .map(str::parse::<u64>)
            .transpose()
            .map_err(err)?;

        let path = format!("channels/{channel_id}/messages");

        // Siempre mandamos un referer.
        let referer: Referer = match guild_id_num {
            Some(guild_id) => GuildChannelReferer {
                guild_id,
                channel_id: channel_id_num,
            }
            .into(),
            None => DmChannelReferer {
                channel_id: channel_id_num,
            }
            .into(),
        };

        let props = RequestPropertiesBuilder::default()
            .referer::<Referer>(referer)
            // .context(...)
            // .solved_captcha(...)
            .build()
            .map_err(err)?;

        let mut builder = MessageBuilder::default();
        builder.content(content).nonce(Self::interaction_nonce());
        if let Some(message_id) = reply_to {
            let reference = MessageReferenceBuilder::default()
                .message_id(message_id.parse::<u64>().map_err(err)?)
                .channel_id(channel_id_num)
                .guild_id(guild_id_num)
                .build()
                .map_err(err)?;
            builder.message_reference(reference);
        }
        let message = builder.build().map_err(err)?;

        let resp: Message = self
            .client
            .post::<Message, Message>(&path, Some(message), Some(props))
            .await
            .map_err(err)?;
        Ok(resp)
    }

    /// `GET /users/@me/settings-proto/1`: la configuración real de la
    /// cuenta (tema, locale, status, notificaciones, privacidad,
    /// favoritos...) tal cual la ve/sincroniza el cliente oficial,
    /// codificada como protobuf y mandada en base64 dentro de un JSON
    /// (`{"settings": "<base64>"}`). Ver `discord::user_settings` para el
    /// mensaje generado y los helpers de encode/decode.
    pub async fn get_user_settings(
        &self,
    ) -> anyhow::Result<crate::discord::user_settings::PreloadedUserSettings> {
        #[derive(serde::Deserialize)]
        struct SettingsProtoResponse {
            settings: String,
        }

        let value: Value = self
            .client
            .get("users/@me/settings-proto/1", None, Some(Self::home()))
            .await
            .map_err(err)?;
        let resp: SettingsProtoResponse = serde_json::from_value(value).map_err(err)?;
        crate::discord::user_settings::decode_base64(&resp.settings)
    }

    /// `PATCH /users/@me/settings-proto/1`: manda un `PreloadedUserSettings`
    /// (típicamente parcial — ver el comentario de
    /// `user_settings::encode`) codificado con el mismo shape que
    /// `get_user_settings`. Discord mergea los campos presentes contra lo
    /// que ya tenía guardado; no hace falta (ni conviene) mandar el blob
    /// completo de vuelta.
    pub async fn patch_user_settings(
        &self,
        settings: &crate::discord::user_settings::PreloadedUserSettings,
    ) -> anyhow::Result<()> {
        let body = json!({ "settings": crate::discord::user_settings::encode_base64(settings) });
        let _: Value = self
            .client
            .patch("users/@me/settings-proto/1", Some(body), Some(Self::home()))
            .await
            .map_err(err)?;
        Ok(())
    }

    pub async fn get_user(&self, user_id: &str) -> anyhow::Result<User> {
        /*let path = format!("users/{user_id}/profile?with_mutual_guilds=false&with_mutual_friends=false&with_mutual_friends_count=false");
        let value: Value = self
            .client
            .get(&path, None, Some(Self::home()))
            .await
            .map_err(err)?;*/
        /*{
  "message": "internal network error",
  "code": 40333
}*/
        //let value: Value = json!({ "code": 40333, "message": "internal network error" });
        // trigger error response
        let value: Value = json!({ "code": 40333, "message": "internal network error" });
        serde_json::from_value(value).map_err(err)
    }

    /// `GET /user-profile-effects`: catálogo de efectos de perfil (capas PNG
    /// animadas + tiempos). Devuelve el JSON crudo; `ProfileEffect::from_configs`
    /// saca de ahí el efecto puntual que tiene equipado alguien.
    pub async fn profile_effects(&self) -> anyhow::Result<Value> {
        let value: Value = self
            .client
            .get("user-profile-effects", None, Some(Self::home()))
            .await
            .map_err(err)?;
        Ok(value)
    }

    /// `GET /collectibles-products/{sku_id}`: el producto de un coleccionable
    /// (efecto de perfil, frame...). `items[]` trae la estructura según el
    /// `type` (ver `models::collectible_item`).
    pub async fn collectible_product(&self, sku_id: &str) -> anyhow::Result<Value> {
        let mut query = HashMap::new();
        query.insert("locale".to_string(), "es-ES".to_string());
        let path = format!("collectibles-products/{sku_id}");
        let value: Value = self
            .client
            .get(&path, Some(query), Some(Self::home()))
            .await
            .map_err(err)?;
        dump_json_if_requested("product_dump", sku_id, &value);
        Ok(value)
    }

    pub async fn user_profile(
        &self,
        user_id: &str,
        guild_id: Option<&str>,
    ) -> anyhow::Result<UserProfileResponse> {
        let mut query = HashMap::new();
        query.insert("with_mutual_guilds".to_string(), "true".to_string());
        query.insert("with_mutual_friends".to_string(), "true".to_string());
        if let Some(guild_id) = guild_id {
            query.insert("guild_id".to_string(), guild_id.to_string());
        }
        let path = format!("users/{user_id}/profile");
        let value: Value = self
            .client
            .get(&path, Some(query), Some(Self::home()))
            .await
            .map_err(err)?;
        dump_profile_if_requested(user_id, &value);
        serde_json::from_value(value).map_err(err)
    }

    /// `POST /channels/{channel}/messages/{message}/ack`: marca el canal como
    /// leído hasta ese mensaje (lo mismo que hace el cliente oficial al
    /// abrir una conversación). Con esto Discord baja las menciones sin leer
    /// y el estado queda guardado en el servidor: sobrevive a reiniciar el
    /// cliente y se sincroniza con tus otros dispositivos.
    ///
    /// `last_viewed` es el día en que se vio el canal, contado en días desde
    /// el epoch de Discord (2015-01-01 UTC), como lo manda el cliente web.
    pub async fn ack_message(&self, channel_id: &str, message_id: &str) -> anyhow::Result<()> {
        const DISCORD_EPOCH_SECS: u64 = 1_420_070_400;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(DISCORD_EPOCH_SECS);
        let last_viewed = now.saturating_sub(DISCORD_EPOCH_SECS) / 86_400;
        let body = json!({ "token": Value::Null, "last_viewed": last_viewed });
        let path = format!("channels/{channel_id}/messages/{message_id}/ack");
        let _: Value = self
            .client
            .post(&path, Some(body), Some(Self::home()))
            .await
            .map_err(err)?;
        Ok(())
    }

    pub async fn add_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        let encoded = percent_encode_emoji(emoji);
        let path = format!("channels/{channel_id}/messages/{message_id}/reactions/{encoded}/@me");
        let _: Value = self
            .client
            .put(&path, None::<()>, Some(Self::home()))
            .await
            .map_err(err)?;
        Ok(())
    }

    pub async fn remove_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        let encoded = percent_encode_emoji(emoji);
        let path = format!("channels/{channel_id}/messages/{message_id}/reactions/{encoded}/@me");
        let _: Value = self
            .client
            .delete(&path, None::<()>, Some(Self::home()))
            .await
            .map_err(err)?;
        Ok(())
    }

    /// El `wreq::Client` interno, por si necesitás algo que este wrapper
    /// todavía no cubre (cualquiera de los ~270 métodos en
    /// `vendor/discord_client_rest/src/api/*.rs`, por ejemplo).
    pub fn get_http_client(&self) -> &wreq::Client {
        self.client.get_http_client()
    }
}
