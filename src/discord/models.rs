//! Formas mínimas de los payloads de la API REST y del Gateway que
//! necesitamos para pantallas de amigos / servidores / mensajes. No están
//! completos (Discord manda muchísimos campos más); si más adelante
//! necesitás algo que no está acá, agregalo al struct correspondiente —
//! `serde` ignora los campos que no declaramos gracias a que no usamos
//! `deny_unknown_fields`.

use serde::Deserialize;
use sha2::digest::consts::U32;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct User {
    #[serde(default)]
    pub id: String,
    /// `#[serde(default)]` en casi todo lo que no sea `id`: Discord manda
    /// objetos de usuario "parciales" en varios lados (webhooks, autores de
    /// mensajes del sistema, usuarios eliminados), y un campo faltante no
    /// debería tirar abajo la deserialización de todo el evento.
    #[serde(default)]
    pub username: String,
    /// Nombre "global" nuevo de Discord (puede no existir en cuentas viejas
    /// o ser `null`).
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub discriminator: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    /// Nivel de Nitro de la cuenta: 0 = ninguno, 1 = Nitro Classic,
    /// 2 = Nitro, 3 = Nitro Basic. Solo viene en el objeto del PROPIO
    /// usuario (el `user` del `READY`); en autores de mensajes y demás
    /// usuarios ajenos falta, y por eso es `Option`.
    #[serde(default)]
    pub premium_type: Option<u8>,
    /// Versión vieja del mismo dato: `true` si la cuenta tiene Nitro.
    #[serde(default)]
    pub premium: Option<bool>,
    /// Hash del banner de perfil. Solo viene en respuestas "completas"
    /// como `GET /users/{id}/profile` — un `User` recortado (autor de un
    /// mensaje, por ejemplo) casi nunca lo trae.
    #[serde(default)]
    pub banner: Option<String>,
    /// Color de acento (`0xRRGGBB`) que Discord usa de fondo cuando no hay
    /// banner. `None` = sin dato (no necesariamente "sin color": puede que
    /// esta respuesta puntual no lo haya incluido).
    #[serde(default)]
    pub accent_color: Option<u32>,
    /// Texto "Sobre mí" del perfil. Igual que `banner`, solo viaja en el
    /// perfil completo.
    #[serde(default)]
    pub bio: Option<String>,
    /// Pronombres del perfil (feature nueva de Discord; puede no venir en
    /// cuentas que no lo configuraron).
    #[serde(default)]
    pub pronouns: Option<String>,
    /// Decoración de avatar (`{ "asset": "a_xxx", "sku_id": "..." }`). Crudo
    /// (`Value`) a propósito: si Discord cambia el formato no debe tumbar la
    /// deserialización de todo el perfil.
    #[serde(default)]
    pub avatar_decoration_data: Option<serde_json::Value>,
    /// Color del banner cuando no hay imagen (`"#RRGGBB"`).
    #[serde(default)]
    pub banner_color: Option<String>,
    /// Etiqueta de servidor ("clan tag"): `{ tag, badge, identity_guild_id,
    /// identity_enabled }`. Discord la llamó `clan` antes de `primary_guild`.
    #[serde(default)]
    pub primary_guild: Option<serde_json::Value>,
    #[serde(default)]
    pub clan: Option<serde_json::Value>,
    /// Estilo del nombre a mostrar (`{ font_id, effect_id, colors: [..] }`).
    #[serde(default)]
    pub display_name_styles: Option<serde_json::Value>,
    /// `true` si la cuenta es un bot. El id de un bot es también el de su
    /// aplicación, que hace falta para contestar a sus botones.
    #[serde(default)]
    pub bot: bool,
    /// Insignias públicas (bits). `1 << 16` = bot verificado (el tilde del
    /// distintivo APP).
    #[serde(default)]
    pub public_flags: u64,
}

/// Etiqueta de servidor de un usuario, lista para dibujar.
#[derive(Debug, Clone)]
pub struct ClanTag {
    pub tag: String,
    pub badge_url: Option<String>,
}

impl User {
    /// ¿La cuenta tiene Nitro? `None` = Discord no informó ninguno de los
    /// dos campos (no sabemos), que NO es lo mismo que `Some(false)`: con
    /// `None` la UI no bloquea nada y deja que Discord decida al enviar.
    /// Cualquier nivel de Nitro (Classic, Nitro o Basic) permite usar
    /// emojis personalizados de otros servidores.
    pub fn has_nitro(&self) -> Option<bool> {
        match (self.premium_type, self.premium) {
            (Some(t), _) => Some(t != 0),
            (None, Some(p)) => Some(p),
            (None, None) => None,
        }
    }

    /// Nombre a mostrar: preferimos `global_name`, si no hay usamos
    /// `username`, y si tampoco vino (usuario parcial) un placeholder para
    /// no mostrar una fila en blanco.
    pub fn display_name(&self) -> &str {
        self.global_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(Some(self.username.as_str()))
            .filter(|s| !s.is_empty())
            .unwrap_or("Usuario desconocido")
    }

    /// URL del avatar en el CDN, o `None` si el usuario no tiene uno
    /// (en ese caso la UI cae al círculo con inicial de siempre).
    pub fn avatar_url(&self) -> Option<String> {
        let hash = self.avatar.as_ref()?;
        let ext = if hash.starts_with("a_") { "gif" } else { "png" };
        Some(format!(
            "https://cdn.discordapp.com/avatars/{}/{}.{}?size=128",
            self.id, hash, ext
        ))
    }

    /// URL de la decoración de avatar (el marco/adorno que va alrededor del
    /// avatar). Se pide con `passthrough=true` para que, si es animada, venga
    /// como APNG (lo anima `ui::anim::source_animated`).
    pub fn avatar_decoration_url(&self) -> Option<String> {
        let asset = self.avatar_decoration_data.as_ref()?.get("asset")?.as_str()?;
        if asset.is_empty() {
            return None;
        }
        Some(format!(
            "https://cdn.discordapp.com/avatar-decoration-presets/{asset}.png?size=256&passthrough=true"
        ))
    }

    /// Color (`0xRRGGBB`) del banner liso, de `banner_color` (`"#RRGGBB"`).
    pub fn banner_color_rgb(&self) -> Option<u32> {
        let hex = self.banner_color.as_deref()?.trim().trim_start_matches('#');
        u32::from_str_radix(hex, 16).ok()
    }

    /// Primer color del estilo del nombre (`display_name_styles.colors[0]`).
    pub fn display_name_color(&self) -> Option<u32> {
        let colors = self.display_name_styles.as_ref()?.get("colors")?.as_array()?;
        let first = colors.first()?;
        first.as_u64().map(|c| c as u32)
    }

    /// Etiqueta de servidor, si la tiene y la muestra (`identity_enabled`).
    pub fn clan_tag(&self) -> Option<ClanTag> {
        let guild = self.primary_guild.as_ref().or(self.clan.as_ref())?;
        if guild.get("identity_enabled").and_then(serde_json::Value::as_bool) == Some(false) {
            return None;
        }
        let tag = guild.get("tag")?.as_str()?.trim().to_string();
        if tag.is_empty() {
            return None;
        }
        let badge_url = match (
            guild.get("identity_guild_id").and_then(serde_json::Value::as_str),
            guild.get("badge").and_then(serde_json::Value::as_str),
        ) {
            (Some(guild_id), Some(badge)) if !badge.is_empty() => Some(format!(
                "https://cdn.discordapp.com/clan-badges/{guild_id}/{badge}.png?size=64"
            )),
            _ => None,
        };
        Some(ClanTag { tag, badge_url })
    }

    /// URL del banner de perfil en el CDN, o `None` si no tiene (la UI cae
    /// a un color liso, ver `ui::profile_popup`).
    pub fn banner_url(&self) -> Option<String> {
        let hash = self.banner.as_ref()?;
        let ext = if hash.starts_with("a_") { "gif" } else { "png" };
        Some(format!(
            "https://cdn.discordapp.com/banners/{}/{}.{}?size=512",
            self.id, hash, ext
        ))
    }
}

/// `GET /users/@me/relationships` — la lista de amigos. `type == 1` es
/// amistad confirmada (2 = bloqueado, 3 = solicitud recibida, 4 = solicitud
/// enviada, 5 = implícita); filtramos por `type == 1` al construir la lista
/// para Home.
#[derive(Debug, Clone, Deserialize)]
pub struct Relationship {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: u8,
    pub user: User,
    /// Si la persona está ignorada ("Ignorar" del menú de clic derecho).
    #[serde(default)]
    pub user_ignored: bool,
}

/// Un servidor tal como llega en `READY`.
///
/// Ojo con `name`: cuando un server está caído (o todavía no terminó de
/// cargar del lado de Discord) el `READY` lo manda "unavailable", es decir
/// solo `{"id": "...", "unavailable": true}`, SIN `name` ni `icon`. Ese es
/// exactamente el caso que hacía fallar todo el `READY` con
/// `missing field "name"`. Por eso `name` e `icon` son opcionales y hay un
/// flag `unavailable` para poder filtrarlos.
#[derive(Debug, Clone, Deserialize)]
pub struct Guild {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    /// Hash del banner del server (el que se ve arriba de la lista de
    /// canales). En el formato nuevo del `READY` puede venir dentro de
    /// `properties`; ver `banner_url`.
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub unavailable: bool,
    /// En el `READY` de una cuenta de usuario (a diferencia del `READY`
    /// recortado que le llega a un bot) Discord ya manda el guild
    /// COMPLETO acá, canales y estados de voz incluidos — no hace falta
    /// esperar un `GUILD_CREATE` aparte ni pedir los canales por REST la
    /// primera vez que se abre el server. Un guild "unavailable" (ver
    /// arriba) no trae nada de esto; para esos sí puede llegar después un
    /// `GUILD_CREATE` con los datos completos.
    #[serde(default)]
    pub channels: Vec<Channel>,
    #[serde(default)]
    pub voice_states: Vec<VoiceState>,
    /// Emojis personalizados del server, ya incluidos acá mismo en el
    /// `READY` (igual que `channels`/`voice_states`) — no hace falta un
    /// pedido de REST aparte para poder ofrecerlos en el picker de
    /// reacciones (`ui::chat::reaction_panel`).
    #[serde(default)]
    pub emojis: Vec<GuildEmoji>,
    /// Stickers personalizados del server (también vienen en el `READY`),
    /// para ofrecerlos en la pestaña "Stickers" del selector.
    #[serde(default)]
    pub stickers: Vec<StickerItem>,
    /// Roles del server (nombre, color, posición, si están "hoisted").
    /// Sirven para ponerle nombre y color a los grupos de la lista de
    /// miembros (`GUILD_MEMBER_LIST_UPDATE` solo manda el id del rol).
    #[serde(default)]
    pub roles: Vec<Role>,
    /// Dueño del server: ignora todos los permisos y overwrites, así que
    /// hace falta para calcular qué canales podés ver (ver
    /// `lib::permissions`). `None` si Discord no lo mandó.
    #[serde(default)]
    pub owner_id: Option<String>,
    /// Formato nuevo del guild en el `READY` de cuentas de usuario: algunos
    /// datos (entre ellos `owner_id`) vienen dentro de `properties`.
    #[serde(default)]
    pub properties: Option<serde_json::Value>,
}

impl Guild {
    /// Dueño del server, de `owner_id` o (formato nuevo) de
    /// `properties.owner_id`. Sin esto el dueño se filtra como un miembro
    /// cualquiera y se le esconden canales que sí ve.
    pub fn owner(&self) -> Option<String> {
        self.owner_id.clone().or_else(|| {
            self.properties
                .as_ref()
                .and_then(|p| p.get("owner_id"))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
    }
}

/// Un bitfield de permisos de Discord llega como string (`"1071698660929"`)
/// en la API actual y como número en formatos viejos: aceptamos los dos.
pub(crate) fn value_to_bits(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::String(s) => s.trim().parse().ok(),
        serde_json::Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

fn de_optional_bits<'de, D>(d: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = <Option<serde_json::Value> as Deserialize>::deserialize(d)?;
    Ok(v.as_ref().and_then(value_to_bits))
}

/// Un permission overwrite de un canal (`channel.permission_overwrites[]`):
/// a un rol (`kind == 0`) o a un miembro (`kind == 1`) se le permite
/// (`allow`) o se le niega (`deny`) algún permiso puntual en ESE canal.
#[derive(Debug, Clone, Default)]
pub struct PermissionOverwrite {
    pub id: String,
    /// `0` = rol, `1` = miembro.
    pub kind: u8,
    pub allow: u64,
    pub deny: u64,
}

impl<'de> Deserialize<'de> for PermissionOverwrite {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            id: String,
            #[serde(default, rename = "type")]
            kind: serde_json::Value,
            #[serde(default)]
            allow: serde_json::Value,
            #[serde(default)]
            deny: serde_json::Value,
        }
        let raw = Raw::deserialize(d)?;
        // El tipo es 0/1 en la API actual y "role"/"member" en formatos viejos.
        let kind = match &raw.kind {
            serde_json::Value::Number(n) => n.as_u64().unwrap_or(0) as u8,
            serde_json::Value::String(s) if s == "member" => 1,
            _ => 0,
        };
        Ok(Self {
            id: raw.id,
            kind,
            allow: value_to_bits(&raw.allow).unwrap_or(0),
            deny: value_to_bits(&raw.deny).unwrap_or(0),
        })
    }
}

/// Un rol de un server (`guild.roles[]`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Role {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Color RGB como entero (`0xRRGGBB`). `0` = el rol no tiene color y
    /// los nombres se dibujan con el color de texto normal.
    #[serde(default)]
    pub color: u32,
    /// Más alto = más arriba en la jerarquía. Un miembro se pinta con el
    /// color de su rol con color de mayor posición.
    #[serde(default)]
    pub position: i64,
    /// `true` = el rol tiene su propio grupo en la lista de miembros.
    #[serde(default)]
    pub hoist: bool,
    /// Bitfield de permisos del rol. `None` = Discord no lo mandó (en ese
    /// caso NO se filtra ningún canal: mejor mostrar de más que esconder
    /// canales que sí se pueden ver).
    #[serde(default, deserialize_with = "de_optional_bits")]
    pub permissions: Option<u64>,
    /// `true` = cualquiera puede mencionar este rol (`<@&id>`), aunque no
    /// tenga el permiso de mencionar a todos. Lo usa el menú de menciones
    /// del compositor para decidir qué roles ofrecer.
    #[serde(default)]
    pub mentionable: bool,
}

/// Un emoji personalizado tal como aparece en `guild.emojis`. A
/// diferencia de [`Emoji`] (que es el shape recortado que trae una
/// reacción puntual), acá siempre hay `id` — Discord no lista los emojis
/// unicode en esta lista, solo los subidos al server — y además viene
/// `animated`, necesario para saber si hay que pedirlo como `.gif` o
/// `.png` al CDN.
#[derive(Debug, Clone, Deserialize)]
pub struct GuildEmoji {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub animated: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrivateChannel {
    #[serde(rename = "type")]
    pub channel_type: u8,

    #[serde(default)]
    pub safety_warnings: Vec<serde_json::Value>,

    #[serde(default)]
    pub recipients: Vec<User>,

    // Los DMs grupales (y algunos formatos de `READY`) no lo traen.
    #[serde(default)]
    pub recipient_flags: u64,

    #[serde(default)]
    pub last_message_id: Option<String>,

    #[serde(default)]
    pub is_spam: bool,

    #[serde(default)]
    pub is_message_request_timestamp: Option<String>,

    #[serde(default)]
    pub is_message_request: bool,

    pub id: String,

    #[serde(default)]
    pub flags: u64,

    #[serde(default)]
    pub username: String,

    #[serde(default)]
    pub avatar: Option<String>,

    #[serde(default)]
    pub avatar_url: Option<String>,
}


impl Guild {
    /// Nombre a mostrar en el rail. Un server "unavailable" no trae nombre,
    /// así que ponemos un placeholder en vez de una entrada vacía.
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("Servidor no disponible")
    }

    pub fn icon_url(&self) -> Option<String> {
        let hash = self.icon.as_ref()?;
        let ext = if hash.starts_with("a_") { "gif" } else { "png" };
        Some(format!(
            "https://cdn.discordapp.com/icons/{}/{}.{}?size=128",
            self.id, hash, ext
        ))
    }

    /// URL del banner del server en el CDN (siempre `.png`: para los animados
    /// es el primer cuadro, que no cuesta memoria), o `None` si no tiene.
    pub fn banner_url(&self) -> Option<String> {
        let hash = self
            .banner
            .clone()
            .or_else(|| {
                self.properties
                    .as_ref()
                    .and_then(|p| p.get("banner"))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            })
            .filter(|h| !h.is_empty())?;
        Some(format!(
            "https://cdn.discordapp.com/banners/{}/{}.png?size=480",
            self.id, hash
        ))
    }
}

/// Tipos de canal: 0 = texto, 5 = anuncios, 15 = foro (todos "de texto"
/// para nosotros); 2 = voz, 13 = stage (los tratamos igual, como "de
/// voz"); 4 = categoría, se maneja aparte en `Server::apply_channels`.
#[derive(Debug, Clone, Deserialize)]
pub struct Channel {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Overwrites de permisos de ESTE canal (ya incluyen lo heredado de la
    /// categoría si está sincronizado: Discord manda los efectivos).
    #[serde(default)]
    pub permission_overwrites: Vec<PermissionOverwrite>,
    /// Solo foros (tipo 15/16): las etiquetas que se le pueden poner a un
    /// post (`available_tags`).
    #[serde(default)]
    pub available_tags: Vec<ForumTag>,
}

/// Etiqueta de un canal de foro (`available_tags`). Los posts guardan solo
/// los ids de las que tienen puestas (`ThreadChannel::applied_tags`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ForumTag {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Emoji unicode de la etiqueta, si tiene uno (los personalizados
    /// vienen como `emoji_id` y no se dibujan por ahora).
    #[serde(default)]
    pub emoji_name: Option<String>,
}

/// Un post de foro = un hilo (`type` 11) cuyo `parent_id` es el foro. Es lo
/// que devuelve `GET /channels/{id}/threads/search` y lo que llega en los
/// eventos `THREAD_CREATE`/`THREAD_UPDATE`.
#[derive(Debug, Clone, Deserialize)]
pub struct ThreadChannel {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub owner_id: Option<String>,
    /// Cantidad de respuestas (sin contar el mensaje inicial).
    #[serde(default)]
    pub message_count: Option<u32>,
    #[serde(default)]
    pub applied_tags: Vec<String>,
    /// Bit 1 (`1 << 1`) = post fijado.
    #[serde(default)]
    pub flags: u64,
    #[serde(default)]
    pub last_message_id: Option<String>,
    #[serde(default)]
    pub thread_metadata: Option<ThreadMetadata>,
}

impl ThreadChannel {
    pub const FLAG_PINNED: u64 = 1 << 1;

    pub fn is_pinned(&self) -> bool {
        self.flags & Self::FLAG_PINNED != 0
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ThreadMetadata {
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub locked: bool,
}

/// Una página de posts de un foro, ya parseada (ver
/// `UwuRest::forum_threads`): los hilos y el primer mensaje de cada uno
/// (`first_messages`, donde `id` == id del hilo).
#[derive(Debug, Clone, Default)]
pub struct ForumPage {
    pub threads: Vec<ThreadChannel>,
    pub first_messages: Vec<GatewayMessage>,
    pub has_more: bool,
}

/// Miembro de guild "parcial", tal como viaja embebido dentro de un
/// `VoiceState` (`GUILD_CREATE`/`VOICE_STATE_UPDATE`). Alcanza con
/// `nick` + `user` para mostrar quién está en un canal de voz sin tener
/// que pedir la lista completa de miembros del server.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartialMember {
    #[serde(default)]
    pub nick: Option<String>,
    #[serde(default)]
    pub user: Option<User>,
    /// Ids de los roles del miembro. En un `MESSAGE_CREATE` de server
    /// viene junto con `nick` y de acá sale el color con el que se pinta
    /// el nombre del autor en el chat.
    #[serde(default)]
    pub roles: Vec<String>,
}

/// Un server en común, tal como viene en `mutual_guilds` de
/// `GET /users/{id}/profile`. Discord solo manda el id acá — el
/// nombre/ícono para mostrarlo lo sacamos de `App::servers` cruzando por
/// este id (ver `ui::profile_popup`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MutualGuild {
    pub id: String,
    /// Apodo del usuario en ESE server, si tiene. No lo usamos todavía,
    /// pero Discord lo manda así que lo guardamos por si hace falta.
    #[serde(default)]
    pub nick: Option<String>,
}

/// Respuesta completa de `GET /users/{id}/profile?with_mutual_guilds=true
/// &with_mutual_friends=true`: el usuario, en qué servers coincidimos, qué
/// amigos tenemos en común y (si se pasó `guild_id`) su apodo/roles en ESE
/// server puntual. Es lo que arma la tarjeta de perfil al clickear a
/// alguien (`ui::profile_popup`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserProfileResponse {
    pub user: User,
    #[serde(default)]
    pub mutual_guilds: Vec<MutualGuild>,
    /// Lista de amigos en común. Ojo: Discord solo la llena si se pidió
    /// `with_mutual_friends=true` Y hay `guild_id`/contexto suficiente;
    /// en algunos casos manda solo el conteo (ver `mutual_friends_count`
    /// más abajo) y esta lista queda vacía.
    #[serde(default)]
    pub mutual_friends: Vec<User>,
    /// Conteo de amigos en común, presente incluso cuando `mutual_friends`
    /// viene vacía.
    #[serde(default)]
    pub mutual_friends_count: Option<u32>,
    /// Apodo + roles del usuario en el server desde el que se abrió el
    /// perfil (pasando `guild_id` en el pedido). `None` si el perfil se
    /// abrió desde un DM, o si el pedido no incluyó `guild_id`.
    #[serde(default)]
    pub guild_member: Option<PartialMember>,
    /// Bio, pronombres, colores del tema y efecto del perfil. OJO: en la API
    /// real la bio viaja acá (`user_profile.bio`), no siempre en `user.bio`.
    #[serde(default)]
    pub user_profile: Option<UserProfileMeta>,
    /// Lo mismo, pero el perfil específico del server desde el que se abrió
    /// (si el usuario configuró uno). Tiene prioridad sobre `user_profile`.
    #[serde(default)]
    pub guild_member_profile: Option<UserProfileMeta>,
    /// Insignias (Nitro, Active Developer, HypeSquad...).
    #[serde(default)]
    pub badges: Vec<ProfileBadge>,
    /// Cuentas conectadas (Steam, GitHub, Spotify...).
    #[serde(default)]
    pub connected_accounts: Vec<ConnectedAccount>,
    /// Efecto de perfil ya resuelto (capas + animaciones). No viene en este
    /// JSON: lo completa `discord::spawn_fetch_user_profile` con una segunda
    /// llamada a `/user-profile-effects`.
    #[serde(skip)]
    pub effect: Option<ProfileEffect>,
    /// Profile frame equipado, ya resuelto con `GET /collectibles-products/{sku}`
    /// (lo completa `discord::spawn_fetch_user_profile`).
    #[serde(skip)]
    pub frame: Option<ProfileFrame>,
}

impl UserProfileResponse {
    /// Id del efecto de perfil equipado (el del server tiene prioridad).
    pub fn profile_effect_id(&self) -> Option<String> {
        let from = |meta: &Option<UserProfileMeta>| -> Option<String> {
            let effect = meta.as_ref()?.profile_effect.as_ref()?;
            let id = effect.get("id")?;
            id.as_str().map(str::to_string).or_else(|| id.as_u64().map(|n| n.to_string()))
        };
        // Formato viejo: `profile_effect.id`. Si no viene, el efecto llega como
        // coleccionable `type` 1 con su `sku_id`; `ProfileEffect::from_configs`
        // acepta tanto el `id` como el `sku_id` del catálogo.
        let from_collectibles = |meta: &Option<UserProfileMeta>| -> Option<String> {
            meta.as_ref()?.collectible_sku(COLLECTIBLE_PROFILE_EFFECT)
        };
        from(&self.guild_member_profile)
            .or_else(|| from(&self.user_profile))
            .or_else(|| from_collectibles(&self.guild_member_profile))
            .or_else(|| from_collectibles(&self.user_profile))
    }

    /// `sku_id` del efecto de perfil equipado, si viene como coleccionable
    /// `type` 1 (el del server tiene prioridad).
    pub fn profile_effect_sku(&self) -> Option<String> {
        let from = |meta: &Option<UserProfileMeta>| -> Option<String> {
            meta.as_ref()?.collectible_sku(COLLECTIBLE_PROFILE_EFFECT)
        };
        from(&self.guild_member_profile).or_else(|| from(&self.user_profile))
    }

    /// `sku_id` del profile frame equipado (el del server tiene prioridad).
    pub fn profile_frame_sku(&self) -> Option<String> {
        let from = |meta: &Option<UserProfileMeta>| -> Option<String> {
            meta.as_ref()?.collectible_sku(COLLECTIBLE_PROFILE_FRAME)
        };
        from(&self.guild_member_profile).or_else(|| from(&self.user_profile))
    }
}

/// Datos de perfil "de estilo" que viven en `user_profile`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserProfileMeta {
    #[serde(default)]
    pub bio: Option<String>,
    #[serde(default)]
    pub pronouns: Option<String>,
    #[serde(default)]
    pub accent_color: Option<u32>,
    /// `[color_primario, color_secundario]` (`0xRRGGBB`): el degradado de
    /// fondo de la tarjeta.
    #[serde(default)]
    pub theme_colors: Option<Vec<u32>>,
    /// `{ "id": "...", "expires_at": ... }`, crudo por la misma razón que
    /// `User::avatar_decoration_data`.
    #[serde(default)]
    pub profile_effect: Option<serde_json::Value>,
    /// `[{ "sku_id": "...", "type": N, "expires_at": null }]`. `type` 1 = efecto
    /// de perfil, `type` 3 = profile frame. Crudo (`Value`) para que un cambio
    /// de formato no rompa la deserialización de todo el perfil.
    #[serde(default)]
    pub collectibles: Option<serde_json::Value>,
}

/// `type` de `user_profile.collectibles[]` para un efecto de perfil.
pub const COLLECTIBLE_PROFILE_EFFECT: u64 = 1;
/// `type` de `user_profile.collectibles[]` para un profile frame.
pub const COLLECTIBLE_PROFILE_FRAME: u64 = 3;

impl UserProfileMeta {
    /// `sku_id` del coleccionable equipado de ese `type` (ver las constantes
    /// `COLLECTIBLE_*`), ignorando los que ya vencieron (`expires_at`).
    pub fn collectible_sku(&self, kind: u64) -> Option<String> {
        let list = self.collectibles.as_ref()?.as_array()?;
        list.iter().find_map(|item| {
            if item.get("type")?.as_u64()? != kind {
                return None;
            }
            let expired = item
                .get("expires_at")
                .and_then(serde_json::Value::as_str)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .is_some_and(|t| t.with_timezone(&chrono::Utc) < chrono::Utc::now());
            if expired {
                return None;
            }
            let sku = item.get("sku_id")?;
            sku.as_str().map(str::to_string).or_else(|| sku.as_u64().map(|n| n.to_string()))
        })
    }
}

/// Una insignia del perfil (`badges[]`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProfileBadge {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Hash del ícono en el CDN (`/badge-icons/{icon}.png`).
    #[serde(default)]
    pub icon: Option<String>,
}

impl ProfileBadge {
    pub fn icon_url(&self) -> Option<String> {
        let icon = self.icon.as_deref().filter(|i| !i.is_empty())?;
        Some(format!("https://cdn.discordapp.com/badge-icons/{icon}.png"))
    }
}

/// Una cuenta conectada (`connected_accounts[]`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConnectedAccount {
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub verified: Option<bool>,
}

/// Una capa de un efecto de perfil (cada una es un PNG animado).
#[derive(Debug, Clone, Default)]
pub struct EffectLayer {
    pub src: String,
    /// `false` = se reproduce una sola vez (intro); `true` = en bucle.
    pub looping: bool,
    /// Tamaño y posición en el espacio de referencia del efecto (450 px de
    /// ancho); la UI lo escala al ancho real de la tarjeta.
    pub width: f32,
    pub height: f32,
    pub x: f32,
    pub y: f32,
    /// Milisegundos desde que se abre la tarjeta hasta que arranca la capa.
    pub start_ms: f32,
    /// Duración de la capa (para cortar las que no son bucle).
    pub duration_ms: f32,
    pub z: i32,
}

/// Efecto de perfil: las capas a superponer sobre la tarjeta.
#[derive(Debug, Clone, Default)]
pub struct ProfileEffect {
    pub layers: Vec<EffectLayer>,
}

impl ProfileEffect {
    /// Busca `effect_id` en la respuesta de `GET /user-profile-effects` y
    /// arma sus capas. Tolera `snake_case` y `camelCase`.
    pub fn from_configs(configs: &serde_json::Value, effect_id: &str) -> Option<Self> {
        use serde_json::Value;
        let list = configs
            .get("profile_effect_configs")
            .or_else(|| configs.get("profileEffectConfigs"))?
            .as_array()?;
        // `effect_id` puede ser el `id` del efecto o el `sku_id` del catálogo.
        let matches = |config: &Value, key: &str| {
            config.get(key).is_some_and(|v| {
                v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string()))
                    == Some(effect_id.to_string())
            })
        };
        let entry = list
            .iter()
            .find(|config| matches(config, "id") || matches(config, "sku_id") || matches(config, "skuId"))?;
        Self::from_entry(entry)
    }

    /// Arma las capas desde algo que trae `effects[]`: una entrada del catálogo
    /// (`/user-profile-effects`) o un item `type` 1 de
    /// `GET /collectibles-products/{sku}` (`items[]`). Mismo formato en ambos.
    pub fn from_entry(entry: &serde_json::Value) -> Option<Self> {
        use serde_json::Value;
        let effects = entry.get("effects")?.as_array()?;

        let num = |v: &Value, keys: &[&str]| -> f32 {
            keys.iter()
                .find_map(|key| v.get(*key).and_then(Value::as_f64))
                .unwrap_or(0.0) as f32
        };

        let layers: Vec<EffectLayer> = effects
            .iter()
            .filter_map(|layer| {
                // Algunas capas no traen `src` fijo sino `randomizedSources`
                // (se elige una al azar); se toma la primera.
                let src = layer
                    .get("src")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        layer.get("randomizedSources")?.as_array()?.first()?.get("src")?.as_str().map(str::to_string)
                    })?;
                let position = layer.get("position");
                Some(EffectLayer {
                    src,
                    looping: layer.get("loop").and_then(Value::as_bool).unwrap_or(false),
                    width: num(layer, &["width"]),
                    height: num(layer, &["height"]),
                    x: position.map(|p| num(p, &["x"])).unwrap_or(0.0),
                    y: position.map(|p| num(p, &["y"])).unwrap_or(0.0),
                    start_ms: num(layer, &["start"]),
                    duration_ms: num(layer, &["duration"]),
                    z: num(layer, &["z_index", "zIndex"]) as i32,
                })
            })
            .collect();
        (!layers.is_empty()).then_some(Self { layers })
    }
}

/// Item `type` `t` dentro de un producto de `GET /collectibles-products/{sku}`
/// (`items[]`), p. ej. `COLLECTIBLE_PROFILE_FRAME`.
pub fn collectible_item(product: &serde_json::Value, kind: u64) -> Option<&serde_json::Value> {
    product
        .get("items")?
        .as_array()?
        .iter()
        .find(|item| item.get("type").and_then(serde_json::Value::as_u64) == Some(kind))
}

/// Qué es una capa de un profile frame (`layers[].type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameLayerKind {
    /// El marco que rodea la tarjeta (`"border"`).
    Border,
    /// Adorno suelto pegado a un borde (`"staple"`): salpicaduras, conejos...
    Staple,
    #[default]
    Other,
}

/// Si la capa va detrás o delante de la tarjeta (`layers[].order`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameLayerOrder {
    Back,
    #[default]
    Front,
}

/// A qué borde del frame se pega la capa (`layers[].anchor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameLayerAnchor {
    Top,
    Bottom,
    #[default]
    Center,
}

/// Una capa de un profile frame. Discord solo manda el `id` del asset (el
/// PNG se baja de `ProfileFrame::layer_url`); `type`, `order` y `anchor`
/// llegan como TEXTO (`"border"`, `"front"`, `"top"`...), no como números.
#[derive(Debug, Clone, Default)]
pub struct FrameLayer {
    pub id: String,
    pub kind: FrameLayerKind,
    pub order: FrameLayerOrder,
    pub anchor: FrameLayerAnchor,
    /// `true` = la capa se estira al ancho del frame en vez de mantener su
    /// tamaño natural (escalado con la tarjeta).
    pub responsive: bool,
}

/// Profile frame: borde decorativo que envuelve la tarjeta y sobresale de
/// ella (`overflow_*`). `inner_width` es el ancho de la tarjeta para el que
/// se diseñó el arte; todos los `overflow_*` están en esa misma escala.
#[derive(Debug, Clone, Default)]
pub struct ProfileFrame {
    pub sku_id: String,
    /// Descripción accesible del arte (texto alternativo).
    #[allow(dead_code)]
    pub label: String,
    pub inner_width: f32,
    pub overflow_top: f32,
    pub overflow_bottom: f32,
    pub overflow_horizontal: f32,
    /// En el orden en que las manda Discord (dentro de cada `order`, la
    /// primera se dibuja debajo de la siguiente).
    pub layers: Vec<FrameLayer>,
}

impl ProfileFrame {
    /// Desde un item `type` 3 de `GET /collectibles-products/{sku}`.
    pub fn from_item(item: &serde_json::Value) -> Option<Self> {
        use serde_json::Value;
        let snowflake = |v: &Value| {
            v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string()))
        };
        let num = |key: &str| item.get(key).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        let text = |layer: &Value, key: &str| {
            layer.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_ascii_lowercase()
        };
        let layers: Vec<FrameLayer> = item
            .get("layers")?
            .as_array()?
            .iter()
            .filter_map(|layer| {
                Some(FrameLayer {
                    id: snowflake(layer.get("id")?)?,
                    kind: match text(layer, "type").as_str() {
                        "border" => FrameLayerKind::Border,
                        "staple" => FrameLayerKind::Staple,
                        _ => FrameLayerKind::Other,
                    },
                    order: match text(layer, "order").as_str() {
                        "back" => FrameLayerOrder::Back,
                        _ => FrameLayerOrder::Front,
                    },
                    anchor: match text(layer, "anchor").as_str() {
                        "top" => FrameLayerAnchor::Top,
                        "bottom" => FrameLayerAnchor::Bottom,
                        _ => FrameLayerAnchor::Center,
                    },
                    responsive: layer.get("responsive").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect();
        if layers.is_empty() {
            return None;
        }
        Some(Self {
            sku_id: item.get("sku_id").and_then(snowflake).unwrap_or_default(),
            label: item.get("label").and_then(Value::as_str).unwrap_or_default().to_string(),
            inner_width: num("inner_width"),
            overflow_top: num("overflow_top"),
            overflow_bottom: num("overflow_bottom"),
            overflow_horizontal: num("overflow_horizontal"),
            layers,
        })
    }

    /// URL del PNG de una capa en el CDN de Discord (versión estática).
    pub fn layer_url(&self, layer: &FrameLayer) -> String {
        format!(
            "https://cdn.discordapp.com/media/v1/collectibles-shop/{}/{}/static",
            self.sku_id, layer.id
        )
    }
}

/// Estado de voz de un usuario: en qué canal está conectado (si está en
/// alguno) dentro de un guild. Llega embebido en `GUILD_CREATE` (foto
/// inicial, un array por guild) y en vivo por `VOICE_STATE_UPDATE` (uno
/// por cambio: se conectó, cambió de canal o se desconectó).
#[derive(Debug, Clone, Deserialize)]
pub struct VoiceState {
    /// Ausente en `GUILD_CREATE` (viaja aparte, a nivel del guild); sí
    /// viene en `VOICE_STATE_UPDATE`.
    #[serde(default)]
    pub guild_id: Option<String>,
    /// `None` significa que el usuario se desconectó de voz.
    #[serde(default)]
    pub channel_id: Option<String>,
    pub user_id: String,
    /// Necesario para el `IDENTIFY` (opcode 0) que abre la conexión de voz.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub member: Option<PartialMember>,
    #[serde(default)]
    pub deaf: bool,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub self_deaf: bool,
    #[serde(default)]
    pub self_mute: bool,
    /// `true` mientras la persona transmite su pantalla (Go Live) desde este
    /// canal. Es lo que hace aparecer la insignia "EN VIVO" y el botón de
    /// ver el stream en la lista de conectados.
    #[serde(default)]
    pub self_stream: bool,
    /// Cámara prendida. Se guarda por completitud; la UI todavía no la usa.
    #[serde(default)]
    pub self_video: bool,
}

impl VoiceState {
    /// Nombre a mostrar en la lista de "conectados" de un canal de voz:
    /// preferimos el apodo del server, después el nombre global/usuario.
    pub fn display_name(&self) -> String {
        if let Some(nick) = self
            .member
            .as_ref()
            .and_then(|m| m.nick.as_deref())
            .filter(|n| !n.is_empty())
        {
            return nick.to_string();
        }
        self.member
            .as_ref()
            .and_then(|m| m.user.as_ref())
            .map(|u| u.display_name().to_string())
            .unwrap_or_else(|| "Usuario desconocido".to_string())
    }

    pub fn avatar_url(&self) -> Option<String> {
        self.member.as_ref()?.user.as_ref()?.avatar_url()
    }

    /// `false` cuando no hay ni apodo ni `member.user` embebido y
    /// `display_name()` tuvo que caer al placeholder "Usuario
    /// desconocido" — señal para pedir los datos del usuario por REST
    /// (`spawn_fetch_user`). Si al menos hay apodo, ya se puede mostrar
    /// algo real y no hace falta pedir nada de más.
    pub fn known(&self) -> bool {
        let has_nick = self
            .member
            .as_ref()
            .and_then(|m| m.nick.as_deref())
            .is_some_and(|n| !n.is_empty());
        has_nick || self.member.as_ref().is_some_and(|m| m.user.is_some())
    }
}

/// `GUILD_CREATE`: Discord lo manda por cada guild después del `READY`
/// (los servers grandes/"unavailable" en el `READY` llegan completos acá).
/// Solo nos interesan los `voice_states` para saber quién está conectado a
/// qué canal de voz apenas se abre la app; el resto del guild ya lo
/// tenemos de `READY` + `GuildChannels`.
#[derive(Debug, Clone, Deserialize)]
pub struct GuildCreatePayload {
    pub id: String,
    #[serde(default)]
    pub voice_states: Vec<VoiceState>,
    #[serde(default)]
    pub roles: Vec<Role>,
    /// Miembros embebidos (en cuentas de usuario suele venir al menos el
    /// propio). Se leen uno por uno: un miembro raro se descarta en vez de
    /// tirar el evento entero.
    #[serde(default, deserialize_with = "lenient_members")]
    pub members: Vec<MemberListMember>,
}

/// `members` de un `GUILD_CREATE`, descartando los que no se puedan leer.
/// Cada miembro se guarda como texto crudo y se parsea aparte, así un error
/// en uno no invalida a los demás ni hace falta un árbol `Value`.
fn lenient_members<'de, D>(deserializer: D) -> Result<Vec<MemberListMember>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<Vec<Box<serde_json::value::RawValue>>> = Option::deserialize(deserializer)?;
    Ok(raw
        .unwrap_or_default()
        .iter()
        .filter_map(|member| serde_json::from_str(member.get()).ok())
        .collect())
}

/// Emoji de una reacción (o de un `MESSAGE_REACTION_ADD/REMOVE`). Discord
/// manda el mismo shape para emoji unicode (solo `name`, `id: null`) y
/// para emojis personalizados del server (`id` presente).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Emoji {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// Si el emoji personalizado es animado (Discord manda este campo en
    /// el shape de una reacción igual que en `guild.emojis`); decide si el
    /// ícono se pide como `.gif` o `.png` al CDN. No existe para emoji
    /// unicode, de ahí el `default` (queda en `false`, sin usarse).
    #[serde(default)]
    pub animated: bool,
}

impl Emoji {
    /// A qué [`ReactionKind`] corresponde este emoji. Para unicode (sin
    /// `id`) el `name` YA es el carácter en sí (p.ej. "👍"); para un emoji
    /// personalizado del server guardamos `id` + `name` + `animated` para
    /// poder pedir el ícono real al CDN (`ui::chat::reaction_pill`, en el
    /// formato correcto según sea o no animado) y mandar el formato que
    /// espera la API (`nombre:id`) en vez del texto ":nombre:" que se
    /// mostraba antes.
    pub fn kind(&self) -> crate::lib::data::ReactionKind {
        use crate::lib::data::ReactionKind;
        match (&self.id, self.name.as_deref()) {
            (Some(id), Some(name)) if !name.is_empty() => {
                ReactionKind::Custom { id: id.clone(), name: name.to_string(), animated: self.animated }
            }
            (None, Some(name)) if !name.is_empty() => ReactionKind::Unicode(name.to_string()),
            _ => ReactionKind::Unicode("❓".to_string()),
        }
    }
}

/// Una reacción tal como viaja embebida en un mensaje (`GET
/// /channels/{id}/messages`, `MESSAGE_CREATE`). `me` ya viene resuelto por
/// Discord respecto a la cuenta del token usado, así que no hace falta
/// comparar contra ningún id nosotros.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiReaction {
    pub emoji: Emoji,
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub me: bool,
}

/// `MESSAGE_REACTION_ADD` / `MESSAGE_REACTION_REMOVE`: alguien puso o sacó
/// una reacción en vivo. A diferencia de `ApiReaction` (que viaja dentro
/// del mensaje), acá viene un evento por reacción/usuario, con
/// `channel_id`/`message_id`/`user_id` sueltos en vez de embebidos.
#[derive(Debug, Clone, Deserialize)]
pub struct ReactionEvent {
    pub channel_id: String,
    pub message_id: String,
    pub user_id: String,
    pub emoji: Emoji,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayMessage {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub author: User,
    /// Apodo y roles del autor DENTRO del server. Discord lo manda en los
    /// `MESSAGE_CREATE` de canales de server (nunca en DMs). El historial
    /// por REST no lo trae: para eso se piden los miembros por Gateway
    /// (ver `GatewayCommand::RequestGuildMembers`).
    #[serde(default)]
    pub member: Option<PartialMember>,
    /// Vacío en mensajes que son solo un embed/adjunto/mensaje de sistema.
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: String,
    /// Usuarios mencionados, con su objeto completo ya embebido — Discord
    /// los manda así en el propio mensaje para no obligar al cliente a
    /// pedirlos aparte solo para poder mostrar `<@id>` como un nombre.
    #[serde(default)]
    pub mentions: Vec<User>,
    /// Server del mensaje. Discord lo manda en los `MESSAGE_CREATE` de
    /// canales de server (nunca en DMs).
    #[serde(default)]
    pub guild_id: Option<String>,
    /// `true` si el mensaje pinga a `@everyone` o `@here`.
    #[serde(default)]
    pub mention_everyone: bool,
    /// Ids de los roles mencionados (`<@&id>`).
    #[serde(default)]
    pub mention_roles: Vec<String>,
    /// Reacciones ya puestas en el mensaje (historial/`MESSAGE_CREATE`).
    /// Vacío si nadie reaccionó todavía.
    #[serde(default)]
    pub reactions: Vec<ApiReaction>,
    /// Archivos subidos con el mensaje (imágenes, videos, audio, etc.).
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Embeds del mensaje: los que arma un bot/webhook a mano (`rich`) y
    /// los que Discord genera solo a partir de links (`image`, `gifv`,
    /// `video`, `article`, `link`). Ojo: los de links suelen llegar UN
    /// RATO DESPUÉS del `MESSAGE_CREATE`, en un `MESSAGE_UPDATE` (ver
    /// `MessageUpdate`).
    #[serde(default)]
    pub embeds: Vec<Embed>,
    /// Stickers mandados con el mensaje (`sticker_items[]`, la versión
    /// "mini" que trae solo id, nombre y formato). Un mensaje con sticker
    /// suele llegar con `content` vacío.
    #[serde(default)]
    pub sticker_items: Vec<StickerItem>,
    /// Presente cuando este mensaje ES una respuesta a otro — trae el id
    /// del original. Discord lo manda SIEMPRE que hay respuesta, incluso
    /// cuando el original ya no se puede mostrar (ver `referenced_message`
    /// abajo), así que sirve para saber "esto es una respuesta" aunque no
    /// tengamos con qué completar el banner.
    #[serde(default)]
    pub message_reference: Option<MessageReference>,
    /// El mensaje citado, embebido completo (autor, contenido, adjuntos)
    /// cuando este mensaje es una respuesta — es lo que Discord manda para
    /// no obligar al cliente a pedirlo aparte por REST. Puede faltar
    /// (`None`) aunque `message_reference` esté presente: pasa cuando el
    /// original se borró o es demasiado viejo para que Discord lo resuelva.
    /// `Box` porque es, ella misma, otro `GatewayMessage` completo.
    #[serde(default)]
    pub referenced_message: Option<Box<GatewayMessage>>,
    /// Copias de los mensajes reenviados (`message_snapshots`). Un mensaje
    /// reenviado llega con `content` vacío, `message_reference.type == 1` y
    /// acá el contenido real del original (texto, adjuntos, embeds...).
    /// Lenient: un snapshot con una forma rara se descarta en vez de tirar
    /// abajo el mensaje entero (ver [`GatewayMessage::is_forward`]).
    #[serde(default, deserialize_with = "lenient_snapshots")]
    pub message_snapshots: Vec<MessageSnapshot>,
    /// Tipo de mensaje de Discord: 0 = normal, 19 = respuesta, 18 = "X
    /// empezó un hilo" (mensaje de sistema, su `content` es el nombre del
    /// hilo) y 21 = mensaje inicial de un hilo (su `referenced_message` es
    /// el mensaje del canal padre que originó el hilo).
    #[serde(rename = "type", default)]
    pub kind: u8,
    /// Componentes interactivos del mensaje (botones, selects...). Un bot
    /// los arma junto a sus embeds; ver [`Component`].
    #[serde(default, deserialize_with = "lenient_components")]
    pub components: Vec<Component>,
    /// Comando (slash) que provocó este mensaje, formato viejo: `{ name,
    /// user, .. }`. El encabezado "X ha utilizado /comando".
    #[serde(default, deserialize_with = "lenient_interaction")]
    pub interaction: Option<MessageInteraction>,
    /// Lo mismo, formato nuevo (`interaction_metadata`): trae `user` y, en
    /// versiones recientes, también `name`.
    #[serde(default, deserialize_with = "lenient_interaction")]
    pub interaction_metadata: Option<MessageInteraction>,
    /// Hilo que nació de ESTE mensaje, si tiene uno (Discord lo embebe en
    /// el mensaje; su `id` es el mismo que el del mensaje).
    #[serde(default)]
    pub thread: Option<ThreadChannel>,
    /// App (bot) dueña del mensaje. Hace falta para responderle a sus
    /// botones (`POST /interactions`).
    #[serde(default)]
    pub application_id: Option<String>,
    /// Bits de `flags` del mensaje (`1 << 6` = efímero).
    #[serde(default)]
    pub flags: u64,
}

/// Solo el id del mensaje citado — la parte de `message_reference` que
/// nos importa para saber que un mensaje es una respuesta (ver
/// `GatewayMessage::message_reference`).
#[derive(Debug, Clone, Deserialize)]
pub struct MessageReference {
    #[serde(default)]
    pub message_id: Option<String>,
    /// Canal del mensaje citado. En el mensaje inicial de un hilo (tipo 21)
    /// es el canal PADRE, no el hilo.
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub guild_id: Option<String>,
    /// Tipo de referencia: 0 = respuesta (o cita), [`MESSAGE_REFERENCE_FORWARD`]
    /// (1) = reenvío.
    #[serde(rename = "type", default)]
    pub kind: u8,
}

/// `message_reference.type` de un mensaje reenviado: `message_id`/`channel_id`
/// apuntan al ORIGINAL, pero Discord no manda `referenced_message` — el
/// contenido viaja en `message_snapshots`.
pub const MESSAGE_REFERENCE_FORWARD: u8 = 1;

impl GatewayMessage {
    /// ¿Es un mensaje reenviado? Cualquiera de tres señales alcanza (así un
    /// payload al que le falte una sigue mostrándose como reenvío): el tipo
    /// de la referencia, el flag `HAS_SNAPSHOT` (`1 << 14`) o que traiga
    /// snapshots.
    pub fn is_forward(&self) -> bool {
        const HAS_SNAPSHOT: u64 = 1 << 14;
        self.message_reference
            .as_ref()
            .is_some_and(|reference| reference.kind == MESSAGE_REFERENCE_FORWARD)
            || self.flags & HAS_SNAPSHOT != 0
            || !self.message_snapshots.is_empty()
    }
}

/// Un elemento de `message_snapshots`: `{ "message": { ... } }`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MessageSnapshot {
    #[serde(default)]
    pub message: SnapshotMessage,
}

/// El mensaje original tal como era al reenviarlo (subconjunto de los campos
/// de un mensaje normal; sin autor: Discord no lo incluye).
#[derive(Debug, Clone, Default)]
pub struct SnapshotMessage {
    pub content: String,
    pub timestamp: String,
    pub edited_timestamp: Option<String>,
    pub mentions: Vec<User>,
    pub attachments: Vec<Attachment>,
    pub embeds: Vec<Embed>,
    pub sticker_items: Vec<StickerItem>,
}

/// Campo por campo y sin quejarse: un embed o un usuario con una forma rara
/// se descarta solo a él, en vez de perder el snapshot entero (y con él el
/// texto del reenvío).
impl<'de> Deserialize<'de> for SnapshotMessage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        fn list<T: serde::de::DeserializeOwned>(value: &serde_json::Value, key: &str) -> Vec<T> {
            value
                .get(key)
                .and_then(|v| v.as_array())
                .map(|items| items.iter().filter_map(|item| serde_json::from_value(item.clone()).ok()).collect())
                .unwrap_or_default()
        }
        let value = serde_json::Value::deserialize(deserializer)?;
        let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        Ok(Self {
            content: text("content").unwrap_or_default(),
            timestamp: text("timestamp").unwrap_or_default(),
            edited_timestamp: text("edited_timestamp"),
            mentions: list(&value, "mentions"),
            attachments: list(&value, "attachments"),
            embeds: list(&value, "embeds"),
            sticker_items: list(&value, "sticker_items"),
        })
    }
}

/// Como [`lenient_components`], para `message_snapshots`.
fn lenient_snapshots<'de, D>(deserializer: D) -> Result<Vec<MessageSnapshot>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<Vec<serde_json::Value>> = Option::deserialize(deserializer)?;
    Ok(raw
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect())
}

/// `MESSAGE_UPDATE`: mensaje editado o, MUY a menudo, Discord terminó de
/// resolver los embeds de un link recién posteado. Es un payload
/// "parcial": solo `id` y `channel_id` están siempre; el resto viene
/// únicamente si cambió, por eso todo es `Option` (`None` = no tocar lo
/// que ya tenemos, `Some(vec![])` = ahora está vacío de verdad).
#[derive(Debug, Clone, Deserialize)]
pub struct MessageUpdate {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub mentions: Option<Vec<User>>,
    #[serde(default)]
    pub attachments: Option<Vec<Attachment>>,
    #[serde(default)]
    pub embeds: Option<Vec<Embed>>,
    #[serde(default)]
    pub sticker_items: Option<Vec<StickerItem>>,
    /// Botones/selects nuevos (un bot que edita su mensaje los cambia o los
    /// deshabilita). `None` = no vinieron, se conservan los que había.
    #[serde(default, deserialize_with = "lenient_components_opt")]
    pub components: Option<Vec<Component>>,
}

/// Qué tipo de archivo es un [`Attachment`], a los fines de cómo se dibuja.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// Formatos que el decodificador del cliente (crate `image`, ver
    /// `Cargo.toml`) sabe leer: png, jpg, gif, webp, bmp.
    Image,
    Video,
    Audio,
    File,
}

/// Un sticker dentro de un mensaje (`sticker_items[]`).
///
/// `format_type` según la API: 1 = PNG, 2 = APNG (PNG animado), 3 = Lottie,
/// 4 = GIF.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StickerItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub format_type: u8,
}

impl StickerItem {
    /// Lado (en px) con el que el cliente real dibuja un sticker en el chat.
    pub const DISPLAY_SIDE: f32 = 160.0;

    /// URL de la imagen del sticker en el CDN de Discord.
    ///
    /// * GIF (4): el `.gif` — se anima igual que cualquier otro GIF.
    /// * PNG (1) / APNG (2): el `.png`. El decodificador del cliente
    ///   (crate `image`) lee solo el primer cuadro de un APNG, así que
    ///   los APNG se ven estáticos.
    /// * Lottie (3): el CDN sirve el `.json` (animación vectorial), que
    ///   acá no se puede dibujar. Se pide el `.png` a modo de vista
    ///   previa; si el CDN no lo tiene, `ui::media` deja un recuadro con
    ///   el nombre del sticker.
    ///
    /// Se usa `cdn.discordapp.com` (no `media.discordapp.net`) a propósito:
    /// `media::resolve_url` le agrega `format=/width=/height=` a las URL de
    /// `discordapp.net`, y eso no aplica a los stickers.
    pub fn url(&self) -> String {
        let ext = if self.format_type == 4 { "gif" } else { "png" };
        format!(
            "https://cdn.discordapp.com/stickers/{}.{}?size=320",
            self.id, ext
        )
    }

    /// Como [`StickerItem::url`] pero chica (128 px), para las celdas del
    /// selector de stickers: no hace falta bajar ni decodificar 320 px para
    /// dibujar una miniatura.
    pub fn thumb_url(&self) -> String {
        let ext = if self.format_type == 4 { "gif" } else { "png" };
        format!(
            "https://cdn.discordapp.com/stickers/{}.{}?size=128",
            self.id, ext
        )
    }
}

/// Un adjunto de mensaje (`attachments[]`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Attachment {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size: u64,
    /// URL directa del CDN (`cdn.discordapp.com`).
    #[serde(default)]
    pub url: String,
    /// Misma imagen a través del proxy de medios (`media.discordapp.net`),
    /// que además permite pedirla redimensionada (`?width=..&height=..`).
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

impl Attachment {
    /// Discord marca como spoiler a los adjuntos cuyo nombre empieza con
    /// `SPOILER_` (es lo que hace el cliente oficial al subirlos).
    pub fn is_spoiler(&self) -> bool {
        self.filename.starts_with("SPOILER_")
    }

    pub fn kind(&self) -> AttachmentKind {
        let ext = self
            .filename
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => return AttachmentKind::Image,
            "mp4" | "webm" | "mov" | "m4v" | "mkv" => return AttachmentKind::Video,
            "mp3" | "ogg" | "oga" | "opus" | "wav" | "flac" | "m4a" => return AttachmentKind::Audio,
            _ => {}
        }
        // Sin extensión útil: caemos al content-type.
        let ct = self.content_type.as_deref().unwrap_or("").to_ascii_lowercase();
        match ct.as_str() {
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp" => AttachmentKind::Image,
            c if c.starts_with("video/") => AttachmentKind::Video,
            c if c.starts_with("audio/") => AttachmentKind::Audio,
            _ => AttachmentKind::File,
        }
    }

    /// URL preferida para mostrar: el proxy si vino, si no la directa.
    pub fn display_url(&self) -> &str {
        self.proxy_url
            .as_deref()
            .filter(|u| !u.is_empty())
            .unwrap_or(&self.url)
    }
}

/// `image` / `thumbnail` / `video` de un [`Embed`] — mismo shape para los tres.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedMedia {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

impl EmbedMedia {
    /// Para links externos, `proxy_url` es la copia que Discord ya tiene
    /// cacheada: cargarla de ahí evita pegarle a un sitio ajeno (y sus
    /// bloqueos de hotlink) desde el cliente.
    pub fn display_url(&self) -> &str {
        self.proxy_url
            .as_deref()
            .filter(|u| !u.is_empty())
            .unwrap_or(&self.url)
    }

    pub fn has_url(&self) -> bool {
        !self.display_url().is_empty()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedFooter {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub proxy_icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedAuthor {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub proxy_icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedProvider {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedField {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub inline: bool,
}

/// Un embed (`embeds[]`). `kind` es `rich` (armado a mano por un
/// bot/webhook), `image`, `gifv`, `video`, `article` o `link`; si no viene
/// se asume `rich`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Embed {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    /// Color de la barra lateral, como entero 0xRRGGBB.
    #[serde(default)]
    pub color: Option<u32>,
    #[serde(default)]
    pub footer: Option<EmbedFooter>,
    #[serde(default)]
    pub image: Option<EmbedMedia>,
    #[serde(default)]
    pub thumbnail: Option<EmbedMedia>,
    #[serde(default)]
    pub video: Option<EmbedMedia>,
    #[serde(default)]
    pub provider: Option<EmbedProvider>,
    #[serde(default)]
    pub author: Option<EmbedAuthor>,
    #[serde(default)]
    pub fields: Vec<EmbedField>,
}

/// Quién usó qué comando para provocar un mensaje de bot
/// (`interaction` / `interaction_metadata`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MessageInteraction {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub user: Option<User>,
}

/// Lee una interacción sin que una forma rara tire abajo el mensaje.
fn lenient_interaction<'de, D>(deserializer: D) -> Result<Option<MessageInteraction>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    Ok(raw.and_then(|v| serde_json::from_value(v).ok()))
}

/// Lee `components` sin que un componente raro (un tipo nuevo de Discord, un
/// campo con otra forma) tire abajo el mensaje entero: los que no se pueden
/// leer se descartan y el resto se queda.
fn lenient_components<'de, D>(deserializer: D) -> Result<Vec<Component>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<Vec<serde_json::Value>> = Option::deserialize(deserializer)?;
    Ok(raw
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect())
}

/// Igual que [`lenient_components`], pero conserva la diferencia entre
/// "no vino" (`None`, no tocar lo que hay) y "vino vacío" (`Some(vec![])`).
fn lenient_components_opt<'de, D>(deserializer: D) -> Result<Option<Vec<Component>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<Vec<serde_json::Value>> = Option::deserialize(deserializer)?;
    Ok(raw.map(|list| {
        list.into_iter()
            .filter_map(|value| serde_json::from_value(value).ok())
            .collect()
    }))
}

/// Emoji de un botón (`emoji` de un [`Component`]): unicode (`name` sin
/// `id`) o personalizado (`id` + `name`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ComponentEmoji {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub animated: bool,
}

/// Un componente de mensaje o de modal. Es un único struct "ancho" para no
/// tener que modelar cada variante de Discord: `kind` dice cuál es y solo
/// se rellenan los campos que esa variante usa.
///
/// * 1 = fila de acciones (`components`), 2 = botón, 3/5/6/7/8 = selects.
/// * 4 = campo de texto (solo dentro de un modal).
/// * 9 = sección (`components` + `accessory`), 10 = texto suelto (`content`),
///   17 = contenedor (`components`), 18 = etiqueta de modal (`component`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Component {
    #[serde(rename = "type", default)]
    pub kind: u8,
    /// Id numérico del componente dentro del mensaje/modal. Los modales
    /// nuevos lo traen en cada componente y el cliente oficial lo devuelve
    /// tal cual en el submit. Crudo (`Value`) por si algún día viene como texto.
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    #[serde(default)]
    pub custom_id: Option<String>,
    /// Botón: 1 primario, 2 secundario, 3 éxito, 4 peligro, 5 link.
    /// Campo de texto: 1 corto, 2 párrafo.
    #[serde(default)]
    pub style: u8,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub emoji: Option<ComponentEmoji>,
    /// Solo botones de estilo 5 (link): no mandan interacción, abren la URL.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub components: Vec<Component>,
    #[serde(default)]
    pub accessory: Option<Box<Component>>,
    /// Modales nuevos: el campo va dentro de una "etiqueta" (`kind` 18).
    #[serde(default)]
    pub component: Option<Box<Component>>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub required: Option<bool>,
    #[serde(default)]
    pub min_length: Option<u32>,
    #[serde(default)]
    pub max_length: Option<u32>,
    /// Componentes V2: galería de medios (12), sus `items`.
    #[serde(default)]
    pub items: Vec<GalleryItem>,
    /// Componentes V2: miniatura (11), `media` con la imagen.
    #[serde(default)]
    pub media: Option<EmbedMedia>,
    /// Componentes V2: archivo (13), `file` con la URL (`attachment://...`
    /// o la del CDN), más `name` y `size` que a veces trae el componente.
    #[serde(default)]
    pub file: Option<EmbedMedia>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    /// Contenedor (17): color de la barra lateral (`0xRRGGBB`).
    #[serde(default)]
    pub accent_color: Option<u32>,
    /// Contenedor / miniatura / archivo: tapado como spoiler.
    #[serde(default)]
    pub spoiler: bool,
    /// Separador (14): `divider` dibuja una línea; `spacing` 1 chico, 2 grande.
    #[serde(default)]
    pub divider: Option<bool>,
    #[serde(default)]
    pub spacing: Option<u8>,
}

/// Un elemento de una galería de medios (componente V2 tipo 12).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GalleryItem {
    #[serde(default)]
    pub media: EmbedMedia,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub spoiler: bool,
}

impl Component {
    pub const BUTTON: u8 = 2;
    pub const TEXT_INPUT: u8 = 4;
    pub const STYLE_LINK: u8 = 5;

    /// ¿Es un botón de link (abre una URL en vez de mandar una interacción)?
    pub fn is_link_button(&self) -> bool {
        self.kind == Self::BUTTON && self.style == Self::STYLE_LINK
    }

    /// Arma este componente de modal tal como lo manda el cliente oficial en
    /// el `MODAL_SUBMIT` (`POST /interactions`, tipo 5): MISMA estructura que
    /// el modal que abrió el bot, con el `value` puesto en cada campo de
    /// texto (`values`: custom_id del campo -> texto).
    ///
    /// * fila de acciones (1, modales viejos): `{type:1, components:[...]}`
    /// * etiqueta (18, modales nuevos): `{type:18, component:{...}}`
    /// * campo de texto (4): `{type:4, custom_id, value}`
    ///
    /// Mandar un modal nuevo (con etiquetas) envuelto en filas de acciones
    /// hace que Discord conteste `500: Internal Server Error`. `None` = este
    /// componente no se manda (texto decorativo, o tipos que la UI todavía no
    /// sabe llenar).
    pub fn to_modal_submit(&self, values: &std::collections::HashMap<String, String>) -> Option<serde_json::Value> {
        use serde_json::json;
        let mut out = match self.kind {
            1 => {
                let children: Vec<serde_json::Value> = self
                    .components
                    .iter()
                    .filter_map(|c| c.to_modal_submit(values))
                    .collect();
                if children.is_empty() {
                    return None;
                }
                json!({ "type": 1, "components": children })
            }
            Self::TEXT_INPUT => {
                let custom_id = self.custom_id.as_deref()?;
                let value = values.get(custom_id).cloned().unwrap_or_default();
                json!({ "type": Self::TEXT_INPUT, "custom_id": custom_id, "value": value })
            }
            18 => {
                let inner = self.component.as_deref()?.to_modal_submit(values)?;
                json!({ "type": 18, "component": inner })
            }
            _ => return None,
        };
        if let Some(id) = &self.id {
            out["id"] = id.clone();
        }
        Some(out)
    }

    /// Junta los campos de texto de un modal, sin importar si vienen en
    /// filas de acciones (formato viejo) o dentro de etiquetas (nuevo).
    pub fn collect_text_inputs<'a>(&'a self, out: &mut Vec<(&'a Component, Option<&'a str>)>) {
        match self.kind {
            Self::TEXT_INPUT => out.push((self, self.label.as_deref())),
            18 => {
                if let Some(inner) = self.component.as_deref() {
                    if inner.kind == Self::TEXT_INPUT {
                        out.push((inner, self.label.as_deref()));
                    }
                }
            }
            _ => {
                for child in &self.components {
                    child.collect_text_inputs(out);
                }
            }
        }
    }
}

/// `INTERACTION_MODAL_CREATE`: el bot respondió a un botón pidiendo un
/// formulario (por ejemplo "Responder anónimo" pide el texto). Se muestra
/// como un popup y se contesta con `POST /interactions` (tipo 5).
#[derive(Debug, Clone, Default)]
pub struct ModalRequest {
    /// Id de la interacción que abrió el modal (va en la respuesta).
    pub interaction_id: String,
    pub application_id: String,
    pub channel_id: String,
    pub guild_id: Option<String>,
    pub title: String,
    pub custom_id: String,
    pub components: Vec<Component>,
    /// Nombre de la aplicación (bot) que pidió el formulario, si vino en el
    /// evento. La UI lo muestra en el encabezado del formulario.
    pub app_name: Option<String>,
    /// URL de la foto de la aplicación (ícono de app o, si no tiene, el
    /// avatar de su usuario bot).
    pub app_icon_url: Option<String>,
}

impl ModalRequest {
    /// Arma el pedido desde el `d` crudo del evento. Tolerante: los campos
    /// que falten quedan vacíos en vez de tirar abajo el evento.
    pub fn from_value(value: &serde_json::Value) -> Option<Self> {
        let text = |v: Option<&serde_json::Value>| v.and_then(serde_json::Value::as_str).map(str::to_owned);
        let interaction_id = text(value.get("id"))?;
        let application_id = text(value.get("application_id"))
            .or_else(|| text(value.get("application").and_then(|a| a.get("id"))))
            .unwrap_or_default();
        let components = value
            .get("components")
            .and_then(serde_json::Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|c| serde_json::from_value::<Component>(c.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        // Nombre y foto de la aplicación: viajan en `application` (ícono de
        // la app) y/o en `application.bot` (usuario bot). Todo opcional.
        let application = value.get("application");
        let non_empty = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
        let bot = application.and_then(|a| a.get("bot"));
        let app_name = non_empty(text(application.and_then(|a| a.get("name"))))
            .or_else(|| non_empty(text(bot.and_then(|b| b.get("global_name")))))
            .or_else(|| non_empty(text(bot.and_then(|b| b.get("username")))));
        let app_icon_url = non_empty(text(application.and_then(|a| a.get("icon"))))
            .map(|hash| {
                format!("https://cdn.discordapp.com/app-icons/{application_id}/{hash}.png?size=128")
            })
            .or_else(|| {
                let bot_id = non_empty(text(bot.and_then(|b| b.get("id"))))?;
                let hash = non_empty(text(bot.and_then(|b| b.get("avatar"))))?;
                let ext = if hash.starts_with("a_") { "gif" } else { "png" };
                Some(format!("https://cdn.discordapp.com/avatars/{bot_id}/{hash}.{ext}?size=128"))
            });
        Some(Self {
            interaction_id,
            application_id,
            app_name,
            app_icon_url,
            channel_id: text(value.get("channel_id")).unwrap_or_default(),
            guild_id: text(value.get("guild_id")),
            title: text(value.get("title")).unwrap_or_else(|| "Formulario".to_string()),
            custom_id: text(value.get("custom_id")).unwrap_or_default(),
            components,
        })
    }
}

/// Payload completo de `READY`: llega una sola vez apenas se completa el
/// `IDENTIFY` y trae de entrada casi todo lo que necesitamos (amigos y
/// servidores incluidos), sin tener que pedirlo aparte por REST.
#[derive(Debug, Clone, Deserialize)]
pub struct ReadyPayload {
    pub user: User,
    #[serde(default)]
    pub guilds: Vec<Guild>,
    #[serde(default)]
    pub relationships: Vec<Relationship>,
    #[serde(default)]
    pub private_channels: Vec<PrivateChannel>,
    /// Token de resume, por si más adelante agregás reconexión con
    /// `RESUME` en vez de un `IDENTIFY` nuevo tras un corte de red.
    #[serde(default)]
    pub resume_gateway_url: Option<String>,
    #[serde(default)]
    pub session_id: String,
    /// Ajustes de notificación por server/canal/DM de la cuenta (silenciados,
    /// nivel de mensajes, `@everyone` suprimido...). NO están en el proto de
    /// `settings-proto/1`: viajan en el `READY` como
    /// `{ "entries": [...], "version": N }`. Se guarda crudo y lo interpreta
    /// `lib::notifications::MuteRules::from_ready`, así un campo que cambie
    /// no rompe la deserialización del `READY` entero.
    #[serde(default)]
    pub user_guild_settings: serde_json::Value,
    /// Estado de lectura de la cuenta (último mensaje leído y menciones sin
    /// leer por canal), guardado por Discord en el servidor: es lo que hace
    /// que los no leídos sobrevivan a un reinicio del cliente. Se guarda
    /// crudo (`{ "entries": [...], "version": N }` con la capability
    /// `VERSIONED_READ_STATES`, o un array suelto en formatos viejos) y se
    /// interpreta con [`parse_read_state`], igual que `user_guild_settings`.
    #[serde(default)]
    pub read_state: serde_json::Value,
    /// Sesiones propias abiertas (cada una con su estado y actividades).
    /// Ver [`parse_session_activities`].
    #[serde(default)]
    pub sessions: serde_json::Value,
}

/// Una entrada de `READY.read_state`: qué tan leído está un canal/DM.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadStateEntry {
    pub channel_id: String,
    /// Último mensaje que Discord considera leído por esta cuenta.
    pub last_message_id: Option<String>,
    /// Menciones sin leer (en un DM cuenta cada mensaje ajeno).
    pub mention_count: u32,
}

/// Saca las entradas de canal de `READY.read_state`. Ignora las que no son
/// de un canal (`read_state_type` != 0: eventos de server, centro de
/// notificaciones, solicitudes de mensajes...) y cualquier entrada rara, así
/// un cambio de formato no rompe el `READY` entero.
pub fn parse_read_state(raw: &serde_json::Value) -> Vec<ReadStateEntry> {
    let list = raw
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .or_else(|| raw.as_array());
    let Some(list) = list else { return Vec::new() };
    list.iter()
        .filter_map(|entry| {
            let kind = entry
                .get("read_state_type")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            if kind != 0 {
                return None;
            }
            let channel_id = entry.get("id")?.as_str()?.to_owned();
            Some(ReadStateEntry {
                channel_id,
                last_message_id: entry
                    .get("last_message_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                mention_count: entry
                    .get("mention_count")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as u32,
            })
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct GatewayPayload {
    pub op: u8,
    /// El `d` del payload como texto JSON sin parsear: los eventos pesados
    /// (`GUILD_CREATE`) se leen directo a structs tipados desde acá, sin
    /// pasar por un árbol `Value` que ocupa varias veces el tamaño del JSON.
    /// Para el resto, ver [`GatewayPayload::data`].
    #[serde(default)]
    pub d: Option<Box<serde_json::value::RawValue>>,
    #[serde(default)]
    pub t: Option<String>,
    #[serde(default)]
    pub s: Option<u64>,
}

impl GatewayPayload {
    /// `d` como árbol `Value` (`Null` si falta o no es JSON válido). Solo
    /// para payloads chicos o que de todos modos hay que recorrer a mano.
    pub fn data(&self) -> serde_json::Value {
        self.d
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw.get()).ok())
            .unwrap_or(serde_json::Value::Null)
    }
}

// ---------------------------------------------------------------------
// Presencia
// ---------------------------------------------------------------------

/// Una actividad de `presence.activities[]`. Solo lo que se muestra.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PresenceActivity {
    #[serde(default)]
    pub name: String,
    /// 0 jugando, 1 transmitiendo, 2 escuchando, 3 viendo, 4 estado
    /// personalizado, 5 compitiendo.
    #[serde(default, rename = "type")]
    pub kind: u8,
    /// Texto del estado personalizado (`kind == 4`), o el "estado" de la
    /// rich presence de un juego/app.
    #[serde(default)]
    pub state: Option<String>,
    /// Primera línea de la rich presence: en Spotify el título de la
    /// canción, en un juego lo que está haciendo.
    #[serde(default)]
    pub details: Option<String>,
    #[serde(default)]
    pub application_id: Option<String>,
    /// `{ "start": ms, "end": ms }`. Crudo (`Value`) para tolerar números o
    /// textos: un formato raro no debe tumbar el `PRESENCE_UPDATE` entero.
    #[serde(default)]
    pub timestamps: serde_json::Value,
    /// `{ "large_image": "...", "large_text": "...", ... }`, también crudo.
    #[serde(default)]
    pub assets: serde_json::Value,
}

impl PresenceActivity {
    fn timestamp(&self, key: &str) -> Option<u64> {
        let value = self.timestamps.get(key)?;
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
    }

    /// Cuándo empezó (milisegundos Unix), si la actividad lo informa.
    pub fn start_ms(&self) -> Option<u64> {
        self.timestamp("start").filter(|t| *t > 0)
    }

    /// Cuándo termina (milisegundos Unix): en Spotify, el fin de la canción.
    pub fn end_ms(&self) -> Option<u64> {
        self.timestamp("end").filter(|t| *t > 0)
    }

    pub fn is_spotify(&self) -> bool {
        self.kind == 2 && self.name.eq_ignore_ascii_case("spotify")
    }

    /// URL de la imagen grande (carátula del disco, ícono del juego...).
    /// `None` si no trae, o si es de un servicio que no sabemos resolver.
    pub fn image_url(&self) -> Option<String> {
        let image = self.assets.get("large_image")?.as_str()?;
        if let Some(id) = image.strip_prefix("spotify:") {
            return Some(format!("https://i.scdn.co/image/{id}"));
        }
        if let Some(path) = image.strip_prefix("mp:external/") {
            return Some(format!("https://media.discordapp.net/external/{path}"));
        }
        if let Some(path) = image.strip_prefix("mp:") {
            return Some(format!("https://media.discordapp.net/{path}"));
        }
        if image.contains(':') {
            // `youtube:`, `twitch:`, etc.: no hay CDN conocido.
            return None;
        }
        let app = self.application_id.as_deref().filter(|a| !a.is_empty())?;
        Some(format!("https://cdn.discordapp.com/app-assets/{app}/{image}.png"))
    }
}

/// Junta las actividades de las sesiones propias (`READY.sessions` y
/// `SESSIONS_REPLACE`): lo que jugás o escuchás en OTRO cliente tuyo (la
/// app oficial de Discord en tu PC, el celular...) llega por acá, no por
/// `PRESENCE_UPDATE`. Se descartan los estados personalizados (no son una
/// actividad) y las repetidas entre sesiones.
pub fn parse_session_activities(raw: &serde_json::Value) -> Vec<PresenceActivity> {
    let Some(sessions) = raw.as_array() else { return Vec::new() };
    let mut out: Vec<PresenceActivity> = Vec::new();
    for session in sessions {
        let status = session.get("status").and_then(serde_json::Value::as_str).unwrap_or("");
        if status == "offline" || status == "invisible" {
            continue;
        }
        let Some(list) = session.get("activities").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for entry in list {
            let Ok(activity) = serde_json::from_value::<PresenceActivity>(entry.clone()) else {
                continue;
            };
            if activity.kind == 4 || activity.name.is_empty() {
                continue;
            }
            if out.iter().any(|a| a.name == activity.name && a.kind == activity.kind) {
                continue;
            }
            out.push(activity);
        }
    }
    out
}

/// El estado en línea de un usuario tal como viaja dentro de un miembro de
/// la lista de miembros.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Presence {
    /// `online`, `idle`, `dnd`, `offline` (o `invisible`).
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub activities: Vec<PresenceActivity>,
}

impl Presence {
    /// Texto secundario para mostrar bajo el nombre (ver
    /// [`activities_subtitle`]).
    pub fn subtitle(&self) -> Option<String> {
        activities_subtitle(&self.activities)
    }
}

/// Qué mostrar bajo el nombre de alguien según sus actividades: el estado
/// personalizado si tiene uno, o "Jugando a X" / "Escuchando X" / etc.
pub fn activities_subtitle(activities: &[PresenceActivity]) -> Option<String> {
    activities_subtitle_raw(activities).map(|text| one_line(&text))
}

/// Junta saltos de línea y espacios repetidos en un solo espacio. Un estado
/// personalizado puede traer varias líneas (letras de canciones, por
/// ejemplo) y el texto de una fila se dibuja en una sola línea: con `\n`
/// se desbordaba de la fila y se metía en las de abajo.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn activities_subtitle_raw(activities: &[PresenceActivity]) -> Option<String> {
    if let Some(custom) = activities.iter().find(|a| a.kind == 4) {
        if let Some(state) = custom.state.as_deref().filter(|s| !s.trim().is_empty()) {
            return Some(state.to_owned());
        }
    }
    let activity = activities
        .iter()
        .find(|a| a.kind != 4 && !a.name.is_empty())?;
    Some(match activity.kind {
        0 => format!("Jugando a {}", activity.name),
        1 => format!("Transmitiendo {}", activity.name),
        2 => format!("Escuchando {}", activity.name),
        3 => format!("Viendo {}", activity.name),
        5 => format!("Compitiendo en {}", activity.name),
        _ => activity.name.clone(),
    })
}

/// `{ "id": "..." }`: el usuario "recortado" que trae un `PRESENCE_UPDATE`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartialUserId {
    #[serde(default)]
    pub id: String,
}

/// Un cambio de presencia: el dispatch `PRESENCE_UPDATE` y también cada
/// entrada de `READY_SUPPLEMENTAL.merged_presences.friends` (que en vez de
/// `user.id` trae `user_id`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PresenceEvent {
    #[serde(default)]
    pub user: Option<PartialUserId>,
    #[serde(default)]
    pub user_id: Option<String>,
    /// Presente cuando el cambio es de un miembro de ese server.
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub activities: Vec<PresenceActivity>,
}

impl PresenceEvent {
    pub fn user_id(&self) -> Option<&str> {
        self.user
            .as_ref()
            .map(|u| u.id.as_str())
            .filter(|id| !id.is_empty())
            .or(self.user_id.as_deref())
    }

    pub fn subtitle(&self) -> Option<String> {
        activities_subtitle(&self.activities)
    }
}

// ---------------------------------------------------------------------
// Lista de miembros (`GUILD_MEMBER_LIST_UPDATE`)
// ---------------------------------------------------------------------

/// Encabezado de grupo de la lista: `id` es el id de un rol, o `online` /
/// `offline`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MemberListGroup {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub count: u64,
}

/// Un miembro dentro de la lista, con su presencia y sus roles.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MemberListMember {
    #[serde(default)]
    pub user: User,
    #[serde(default)]
    pub nick: Option<String>,
    /// Ids de TODOS los roles del miembro (no solo los "hoisted").
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub presence: Option<Presence>,
}

/// Cada casilla de la lista: o es el encabezado de un grupo o es un
/// miembro (nunca las dos a la vez).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MemberListItem {
    #[serde(default)]
    pub group: Option<MemberListGroup>,
    #[serde(default)]
    pub member: Option<MemberListMember>,
}

/// Una operación sobre la lista PLANA que mantiene el cliente. Discord no
/// manda la lista entera: manda operaciones sobre índices.
///
/// * `SYNC`: reemplaza el rango `range` por `items`.
/// * `INSERT` / `UPDATE`: pone `item` en `index` (insertando o pisando).
/// * `DELETE`: saca lo que hay en `index`.
/// * `INVALIDATE`: el rango `range` ya no es válido.
#[derive(Debug, Clone, Deserialize)]
pub struct MemberListOp {
    pub op: String,
    #[serde(default)]
    pub range: Option<(usize, usize)>,
    #[serde(default)]
    pub index: Option<usize>,
    #[serde(default)]
    pub item: Option<MemberListItem>,
    #[serde(default)]
    pub items: Vec<MemberListItem>,
}

/// `GUILD_MEMBER_LIST_UPDATE`. Solo llega para los canales a los que la
/// sesión se suscribió con el opcode 37 (ver
/// `gateway::GatewayCommand::SubscribeMemberList`).
#[derive(Debug, Clone, Deserialize)]
pub struct MemberListUpdate {
    pub guild_id: String,
    /// Identifica QUÉ lista es: `everyone` o un hash de permisos. Canales
    /// con distintos permisos pueden tener listas distintas.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub member_count: u64,
    #[serde(default)]
    pub online_count: u64,
    #[serde(default)]
    pub groups: Vec<MemberListGroup>,
    #[serde(default)]
    pub ops: Vec<MemberListOp>,
}

// ---------------------------------------------------------------------
// Miembros sueltos (`GUILD_MEMBERS_CHUNK`, `GUILD_MEMBER_UPDATE`)
// ---------------------------------------------------------------------

/// `GUILD_MEMBERS_CHUNK`: la respuesta a un `REQUEST_GUILD_MEMBERS`
/// (opcode 8). Cada miembro trae `user`, `nick` y `roles`, que es lo mismo
/// que necesita [`MemberListMember`] — por eso se reutiliza (`presence`
/// queda en `None`: las presencias no se piden).
#[derive(Debug, Clone, Deserialize)]
pub struct GuildMembersChunk {
    pub guild_id: String,
    #[serde(default)]
    pub members: Vec<MemberListMember>,
}

/// `GUILD_MEMBER_UPDATE`: alguien cambió su apodo o sus roles.
#[derive(Debug, Clone, Deserialize)]
pub struct GuildMemberUpdate {
    pub guild_id: String,
    #[serde(default)]
    pub user: User,
    #[serde(default)]
    pub nick: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}


#[cfg(test)]
mod read_state_tests {
    use super::*;

    #[test]
    fn parses_versioned_entries_and_skips_non_channel_ones() {
        let raw = serde_json::json!({
            "version": 3,
            "partial": false,
            "entries": [
                { "id": "111", "last_message_id": "900", "mention_count": 4 },
                { "id": "222", "mention_count": 0 },
                { "id": "333", "read_state_type": 2, "mention_count": 9 },
                { "last_message_id": "1" }
            ]
        });
        let entries = parse_read_state(&raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].channel_id, "111");
        assert_eq!(entries[0].last_message_id.as_deref(), Some("900"));
        assert_eq!(entries[0].mention_count, 4);
        assert_eq!(entries[1].channel_id, "222");
        assert_eq!(entries[1].last_message_id, None);
    }

    #[test]
    fn accepts_the_old_bare_array_and_missing_data() {
        let raw = serde_json::json!([{ "id": "5", "last_message_id": "7", "mention_count": 1 }]);
        assert_eq!(parse_read_state(&raw).len(), 1);
        assert!(parse_read_state(&serde_json::Value::Null).is_empty());
    }
}
