//! Cliente REST muy chico: lo justo para completar lo que no viene ya en
//! `READY` (por ejemplo los canales de un servidor cuando el usuario lo
//! abre, o el historial de mensajes de un canal/DM).
//!
//! Nota: pegarle a la API de Discord con el token de una cuenta de usuario
//! normal (no un bot, sin pasar por `Authorization: Bot ...`) es justo lo
//! que Discord llama "unofficial client" en sus términos de servicio. Ver
//! el aviso que te dí en el chat antes de generar este código.
//!
//! Este cliente comparte el fingerprint web (user agent, `X-Super-Properties`,
//! `X-Discord-Locale`, `Sec-CH-UA`, cookies de sesión, etc.) con el resto
//! del stack de auth — así Discord ve un solo "browser" consistente y no
//! un cliente REST aparte. Lo ideal es pasarle el mismo `reqwest::Client`
//! que usa `DiscordAuthSession` vía `RestClient::with_http` para que el
//! cookie jar sea compartido.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::discord::fingerprint::{
    CLIENT_BUILD_NUMBER, ClientFingerprint, discord_http_client_with_cookie_path,
    discord_rest_headers,
};
use crate::discord::models::{Channel, GatewayMessage, User, UserProfileResponse};
use crate::paths;

//const API_BASE: &str = "http://localhost:8870";
const API_BASE: &str = "https://discord.com/api/v10";

/// Cuántos mensajes se piden por página, tanto en la carga inicial
/// (`channel_messages`) como en "cargar más" al llegar arriba del todo
/// del historial (`channel_messages_before`). Mismo número en los dos
/// lados para poder usarlo como pista de "¿probablemente queda más
/// historial?": si una página vuelve con menos de esto, ya no hay nada
/// más viejo que pedir (ver `has_more` en `lib/state.rs`).
pub const MESSAGES_PAGE_SIZE: u8 = 20;

/// Fingerprint compartido por todos los `RestClient` del proceso. Arranca
/// vacío (se usa el build number de respaldo) y `discord::init_fingerprint`
/// lo reemplaza por el que se leyó de Discord Web al iniciar sesión.
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

#[derive(Clone)]
pub struct RestClient {
    http: reqwest::Client,
    fingerprint: Arc<ClientFingerprint>,
    token: String,
}

impl RestClient {
    /// El constructor de todos los días: el fingerprint compartido del
    /// proceso y el cookie jar persistido en la carpeta de estado del
    /// usuario (`paths::discord_cookie_file`). Es lo que usan los
    /// `spawn_*` de `discord/mod.rs`, que solo tienen el token a mano.
    pub fn for_token(token: String) -> Self {
        Self::new(token, shared_fingerprint(), paths::discord_cookie_file())
    }

    /// Construye un `RestClient` con su propio `reqwest::Client` (con
    /// cookie jar persistido en `cookie_path`, si se pasa). Útil para
    /// tests o si no te importa compartir cookies con `DiscordAuthSession`.
    ///
    /// Si vas a tener los dos vivos al mismo tiempo, preferí
    /// `RestClient::with_http` y pasale el mismo cliente que usa la
    /// sesión de auth — así las cookies que setea `/experiments` están
    /// disponibles para el REST sin esperar al debounce de persistencia.
    pub fn new(
        token: String,
        fingerprint: Arc<ClientFingerprint>,
        cookie_path: Option<PathBuf>,
    ) -> Self {
        let http = discord_http_client_with_cookie_path(&fingerprint, cookie_path);
        Self::with_http(token, fingerprint, http)
    }

    /// Igual que `new`, pero reusando un `reqwest::Client` ya armado
    /// (típicamente el de `DiscordAuthSession::http()`), para que ambos
    /// compartan el mismo cookie jar.
    pub fn with_http(
        token: String,
        fingerprint: Arc<ClientFingerprint>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            http,
            fingerprint,
            token,
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{API_BASE}{path}"))
            // Headers web del fingerprint: user agent, `X-Super-Properties`,
            // `X-Discord-Locale`, `X-Discord-Timezone`, `Sec-CH-UA`,
            // `Sec-Fetch-*`, `Priority`, `Referer` a /channels, etc.
            // `discord_rest_headers` ya decide qué va y qué no según la
            // build; no hace falta tocar nada a mano acá.
            .headers(discord_rest_headers(&self.fingerprint))
            // Un cliente de verdad (no un bot de la Developer Portal) manda
            // el token pelado en `Authorization`, sin el prefijo `Bot `.
            .header("Authorization", &self.token)
    }

    pub async fn guild_channels(&self, guild_id: &str) -> reqwest::Result<Vec<Channel>> {
        self.request(reqwest::Method::GET, &format!("/guilds/{guild_id}/channels"))
            .send()
            .await?
            .json()
            .await
    }

    /// Últimos mensajes de un canal o de un DM (`channel_id` es el mismo
    /// concepto para los dos casos en la API de Discord).
    pub async fn channel_messages(
        &self,
        channel_id: &str,
        limit: u8,
    ) -> reqwest::Result<Vec<GatewayMessage>> {
        self.request(
            reqwest::Method::GET,
            &format!("/channels/{channel_id}/messages?limit={limit}"),
        )
        .send()
        .await?
        .json()
        .await
    }

    /// Como `channel_messages`, pero pidiendo la página anterior a
    /// `before_message_id` (mismo parámetro `before` que usa la API de
    /// Discord para paginar) — se usa para "cargar más" cuando el usuario
    /// llega arriba del todo de lo que ya se cargó (ver `ui::chat` y
    /// `App::load_more_messages`).
    pub async fn channel_messages_before(
        &self,
        channel_id: &str,
        limit: u8,
        before_message_id: &str,
    ) -> reqwest::Result<Vec<GatewayMessage>> {
        self.request(
            reqwest::Method::GET,
            &format!("/channels/{channel_id}/messages?limit={limit}&before={before_message_id}"),
        )
        .send()
        .await?
        .json()
        .await
    }

    /// Abre (o recupera, si ya existe) el DM con `user_id`. Discord no
    /// tiene un "id de DM" fijo por amigo: hay que pedirlo así antes de
    /// poder mandar/leer mensajes.
    pub async fn open_dm(&self, user_id: &str) -> reqwest::Result<String> {
        #[derive(serde::Deserialize)]
        struct DmChannel {
            id: String,
        }
        let body = serde_json::json!({ "recipient_id": user_id });
        let dm: DmChannel = self
            .request(reqwest::Method::POST, "/users/@me/channels")
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        Ok(dm.id)
    }

    /// Manda un mensaje. `reply_to`, si trae un id, se agrega como
    /// `message_reference` — es lo que hace que el mensaje aparezca en el
    /// cliente real como "respondiendo a" ese otro (ver el botón
    /// "Responder" de la barra al pasar el mouse, en `ui::chat`).
    pub async fn send_message(
        &self,
        channel_id: &str,
        content: &str,
        reply_to: Option<&str>,
    ) -> reqwest::Result<()> {
        let mut body = serde_json::json!({ "content": content });
        if let Some(message_id) = reply_to {
            body["message_reference"] = serde_json::json!({ "message_id": message_id });
        }
        self.request(
            reqwest::Method::POST,
            &format!("/channels/{channel_id}/messages"),
        )
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    /// Datos básicos de un usuario por id. Se usa como respaldo para los
    /// estados de voz que llegan sin `member`/`user` embebido (la foto
    /// inicial del `READY`, para guilds grandes, a veces trae eso recortado)
    /// — así no se queda mostrando "Usuario desconocido" para siempre.
    pub async fn get_user(&self, user_id: &str) -> reqwest::Result<User> {
        self.request(reqwest::Method::GET, &format!("/users/{user_id}"))
            .send()
            .await?
            .json()
            .await
    }

    /// Perfil completo de un usuario para la tarjeta que se abre al
    /// clickearlo (`ui::profile_popup`): bio/banner/pronombres, servers y
    /// amigos en común, y — si se pasa `guild_id` — su apodo y roles en
    /// ESE server puntual (Discord no los manda si no le decís desde
    /// dónde se está mirando el perfil).
    pub async fn user_profile(
        &self,
        user_id: &str,
        guild_id: Option<&str>,
    ) -> reqwest::Result<UserProfileResponse> {
        let mut path = format!(
            "/users/{user_id}/profile?with_mutual_guilds=true&with_mutual_friends=true"
        );
        if let Some(guild_id) = guild_id {
            path.push_str(&format!("&guild_id={guild_id}"));
        }
        self.request(reqwest::Method::GET, &path)
            .send()
            .await?
            .json()
            .await
    }

    /// Pone la reacción propia con `emoji` en un mensaje
    /// (`PUT .../reactions/{emoji}/@me`). `emoji` es el emoji unicode tal
    /// cual (p.ej. "👍"); para uno personalizado del server Discord espera
    /// `nombre:id`.
    pub async fn add_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> reqwest::Result<()> {
        let encoded = percent_encode_emoji(emoji);
        self.request(
            reqwest::Method::PUT,
            &format!("/channels/{channel_id}/messages/{message_id}/reactions/{encoded}/@me"),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }

    /// Saca la reacción propia con `emoji` de un mensaje.
    pub async fn remove_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> reqwest::Result<()> {
        let encoded = percent_encode_emoji(emoji);
        self.request(
            reqwest::Method::DELETE,
            &format!("/channels/{channel_id}/messages/{message_id}/reactions/{encoded}/@me"),
        )
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }
}

/// Percent-encoding mínimo para meter un emoji (UTF-8 de varios bytes) en
/// un path de la URL. No hay ninguna crate de percent-encoding en las
/// dependencias del proyecto, así que lo hacemos a mano: los caracteres
/// "no reservados" de la RFC 3986 (letras/dígitos ASCII y `-_.~`) pasan
/// tal cual, todo lo demás se escapa byte a byte como `%XX`.
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