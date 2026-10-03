use egui::Color32;

use crate::theme::Icon::User;
use crate::discord::models::{Attachment, Component, Embed, MessageUpdate, PrivateChannel, StickerItem};
use crate::ui::media::gif_safe_url;


#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Online,
    Idle,
    Dnd,
    Offline,
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Online => "En línea",
            Status::Idle => "Ausente",
            Status::Dnd => "No molestar",
            Status::Offline => "Desconectado",
        }
    }
}

/// Identidad de un emoji usado en una reacción: unicode tal cual, o un
/// emoji personalizado del server (con `id`, para poder pedir el ícono
/// real al CDN — ver `reaction_pill` en `ui::chat`). Separado de un
/// `String` plano porque necesitamos tanto el formato que espera la API
/// de Discord (`nombre:id` para uno personalizado, ver `api_format`) como
/// los datos sueltos para armar la URL del ícono.
#[derive(Clone, PartialEq)]
pub enum ReactionKind {
    Unicode(String),
    /// `animated` viene de `Emoji::animated` (el emoji trae ese dato en
    /// `guild.emojis` y en el shape recortado de una reacción puntual) y
    /// decide si el ícono se pide como `.gif` o `.png` al CDN — ver
    /// `ui::chat::reaction_pill_custom`.
    Custom { id: String, name: String, animated: bool },
}

impl ReactionKind {
    /// Texto tal como lo espera la API de Discord en la URL de
    /// PUT/DELETE reacción (`.../reactions/{emoji}/@me`): el emoji unicode
    /// tal cual, o "nombre:id" para uno personalizado.
    pub fn api_format(&self) -> String {
        match self {
            ReactionKind::Unicode(s) => s.clone(),
            ReactionKind::Custom { id, name, .. } => format!("{name}:{id}"),
        }
    }
}

/// Emoji personalizado de un server (`guild.emojis`, ver
/// `discord::models::GuildEmoji`), para poder ofrecerlo en el picker de
/// reacciones (`ui::chat::reaction_panel`) además del set fijo de emoji
/// unicode.
#[derive(Clone)]
pub struct CustomEmoji {
    pub id: String,
    pub name: String,
    pub animated: bool,
}

impl CustomEmoji {
    /// URL al CDN de Discord para pintar el ícono en el picker — `.gif`
    /// para los animados, `.png` para el resto (mismo criterio que
    /// `ui::chat::reaction_pill_custom` ya usa para las reacciones ya
    /// puestas).
    pub fn url(&self) -> String {
        let ext = if self.animated { "gif" } else { "png" };
        let url = format!("https://cdn.discordapp.com/emojis/{}.{ext}?size=48", self.id);
        if self.animated {
            gif_safe_url(&url)
        } else {
            url
        }
    }
}

/// Los emojis personalizados de UN server, con lo necesario para dibujar su
/// ícono en la barra lateral del picker de reacciones
/// (`ui::chat::reaction_panel`), donde cada server es una sección a la que
/// se puede saltar.
#[derive(Clone)]
pub struct EmojiGroup {
    pub name: String,
    pub icon_url: Option<String>,
    pub icon_initial: String,
    pub icon_color: Color32,
    pub emojis: Vec<CustomEmoji>,
    /// Stickers personalizados del server, para la pestaña "Stickers" del
    /// selector (ver `ui::compose_menus::show_emoji_picker`).
    pub stickers: Vec<StickerItem>,
    /// Es el server del canal que se está mirando (no un server "externo").
    /// En un DM ningún grupo es `home`.
    pub home: bool,
    /// La cuenta NO tiene Nitro (se sabe con certeza, ver
    /// `User::has_nitro`; si no se sabe, esto queda en `false` y no se
    /// bloquea nada). Junto con `home` decide qué emojis se pueden usar:
    /// ver [`EmojiGroup::is_locked`].
    pub no_nitro: bool,
}

impl EmojiGroup {
    /// `None` si el server no tiene ningún emoji ni sticker personalizado (no
    /// tiene sentido darle una sección ni un ícono en el picker).
    pub fn from_server(server: &Server, home: bool, no_nitro: bool) -> Option<Self> {
        if server.custom_emojis.is_empty() && server.custom_stickers.is_empty() {
            return None;
        }
        Some(Self {
            name: server.name.clone(),
            icon_url: server.icon_url.clone(),
            icon_initial: server.icon_initial.clone(),
            icon_color: server.icon_color,
            emojis: server.custom_emojis.clone(),
            stickers: server.custom_stickers.clone(),
            home,
            no_nitro,
        })
    }

    /// ¿Este emoji se ve pero NO se puede usar porque hace falta Nitro y la
    /// cuenta no lo tiene? Como en el cliente real, piden Nitro:
    /// * los emojis de OTRO server (o cualquiera, en un DM), y
    /// * los emojis ANIMADOS, incluso los del propio server.
    ///
    /// El picker los atenúa y, al clickearlos, avisa en vez de reaccionar.
    pub fn is_locked(&self, emoji: &CustomEmoji) -> bool {
        self.no_nitro && (!self.home || emoji.animated)
    }

    /// ¿Este sticker pide Nitro y la cuenta no lo tiene? Los stickers de
    /// OTRO server (o cualquiera, en un DM) necesitan Nitro; los del propio
    /// server no.
    pub fn is_sticker_locked(&self) -> bool {
        self.no_nitro && !self.home
    }
}

/// Una reacción con emoji sobre un [`ChatMessage`], como las cuentitas
/// "👍 3" que aparecen debajo de un mensaje en el cliente real.
#[derive(Clone)]
pub struct Reaction {
    pub emoji: ReactionKind,
    pub count: u32,
    /// Si la cuenta logueada es una de las que puso esta reacción — se
    /// pinta distinto (fondo con el acento) para que sea obvio con cuál
    /// ya reaccionaste, igual que en el cliente real.
    pub reacted_by_me: bool,
}

/// Mensaje al que se está respondiendo desde el composer (ver
/// `ui::chat::composer`): se muestra como un banner arriba del input,
/// con el autor original y una vista previa recortada del contenido,
/// hasta que se manda la respuesta o se cancela con la X.
#[derive(Clone)]
pub struct ReplyTarget {
    /// Id real de Discord del mensaje citado. Vacío para mensajes de
    /// demo/eco local — en ese caso el banner se muestra igual, pero al
    /// mandar no se agrega `message_reference` (no hay nada real a qué
    /// referenciar).
    pub message_id: String,
    pub author: String,
    pub preview: String,
}

/// Vista previa del mensaje citado cuando un [`ChatMessage`] es una
/// respuesta a otro — el textito con el autor y un resumen del contenido
/// que se muestra ARRIBA del mensaje, con la rayita en L hacia el avatar,
/// igual que en el cliente real (no confundir con [`ReplyTarget`], que es
/// lo mismo pero mientras todavía se está redactando la respuesta).
#[derive(Clone, Default)]
pub struct RepliedMessage {
    /// Nombre a mostrar del autor citado: el apodo que tiene en el server
    /// si lo conocemos, si no su nombre global.
    pub author: String,
    /// Id del autor citado y su nombre "base" (sin apodo). Se guardan para
    /// poder volver a resolver el nombre cuando llega el apodo real (ver
    /// `ChatMessage::reresolve_names`). Vacíos en los ecos locales.
    pub author_id: String,
    pub author_base: String,
    /// Color del rol más alto del autor citado. `None` = sin rol con color.
    pub author_color: Option<Color32>,
    /// Resumen corto del contenido citado, ya recortado (ver
    /// `ui::chat::preview_text`). Vacío si el original no tenía texto
    /// (por ejemplo, un mensaje que era solo una imagen o un sticker).
    pub preview: String,
    /// `true` cuando Discord marcó el mensaje como respuesta pero no
    /// mandó el original embebido (se borró, o es muy viejo) — en ese
    /// caso se avisa en vez de mostrar un autor/preview que no tenemos.
    pub deleted: bool,
}

/// Copia de UN mensaje reenviado (un `message_snapshots[]` de Discord): lo
/// que se dibuja dentro del bloque "Reenviado" de un [`ChatMessage`].
#[derive(Clone, Default)]
pub struct ForwardedMessage {
    pub content: String,
    /// Hora del mensaje original (mismo formato que `ChatMessage::time`).
    pub time: String,
    pub edited: bool,
    /// (user_id, nombre) de los mencionados, para resolver `<@id>`.
    pub mentions: Vec<(String, String)>,
    pub attachments: Vec<Attachment>,
    pub embeds: Vec<Embed>,
    pub stickers: Vec<StickerItem>,
}

/// Un mensaje reenviado: lo que trae `message_snapshots` más de dónde venía
/// (`message_reference`). Ver `ui::chat::forward_block`.
#[derive(Clone, Default)]
pub struct Forward {
    /// Vacío si Discord no mandó el contenido (o no se pudo leer): el bloque
    /// muestra un aviso en vez de quedar en blanco.
    pub snapshots: Vec<ForwardedMessage>,
    /// Canal del mensaje original, para mostrar "#canal" si lo conocemos.
    pub source_channel_id: Option<String>,
    /// Server del mensaje original (`message_reference.guild_id`): de ahí
    /// salen el nombre y el ícono del pie del bloque, como en el cliente
    /// real. `None` si el reenvío viene de un DM.
    pub source_guild_id: Option<String>,
}

/// Lo mínimo de un server para rotular de dónde viene un reenvío (ver
/// [`publish_guild_directory`]).
#[derive(Clone)]
pub struct GuildBrief {
    pub name: String,
    pub icon_url: Option<String>,
    pub initial: String,
    pub color: Color32,
}

fn guild_directory_id() -> egui::Id {
    egui::Id::new("ecord_guild_directory")
}

/// Deja en la memoria de egui un índice `guild_id → nombre/ícono` de los
/// servers de la cuenta, para que `ui::chat` rotule de dónde viene un
/// reenvío sin tener que recibir la lista de servers por parámetro en cada
/// llamada. Se reconstruye si cambia la cantidad de servers o cada 3 s
/// (un cambio de nombre/ícono es raro), así que el costo por frame es casi
/// nulo.
pub fn publish_guild_directory(ctx: &egui::Context, servers: &[Server]) {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let stamp_id = egui::Id::new("ecord_guild_directory_stamp");
    let fresh = ctx
        .data(|d| d.get_temp::<(usize, Instant)>(stamp_id))
        .is_some_and(|(len, at)| len == servers.len() && at.elapsed() < Duration::from_secs(3));
    if fresh {
        return;
    }
    let directory: std::collections::HashMap<String, GuildBrief> = servers
        .iter()
        .filter(|s| !s.guild_id.is_empty())
        .map(|s| {
            (
                s.guild_id.clone(),
                GuildBrief {
                    name: s.name.clone(),
                    icon_url: s.icon_url.clone(),
                    initial: s.icon_initial.clone(),
                    color: s.icon_color,
                },
            )
        })
        .collect();
    ctx.data_mut(|d| {
        d.insert_temp(guild_directory_id(), Arc::new(directory));
        d.insert_temp(stamp_id, (servers.len(), Instant::now()));
    });
}

/// Nombre e ícono de un server de la cuenta, si se conoce (ver
/// [`publish_guild_directory`]). `None` si la cuenta no está en ese server.
pub fn guild_brief(ctx: &egui::Context, guild_id: &str) -> Option<GuildBrief> {
    use std::sync::Arc;
    ctx.data(|d| d.get_temp::<Arc<std::collections::HashMap<String, GuildBrief>>>(guild_directory_id()))
        .and_then(|directory| directory.get(guild_id).cloned())
}

impl Forward {
    /// `None` si el mensaje no es un reenvío.
    pub fn from_discord(msg: &crate::discord::models::GatewayMessage) -> Option<Self> {
        if !msg.is_forward() {
            return None;
        }
        if msg.message_snapshots.is_empty() {
            // Señal para diagnosticar: Discord marcó el mensaje como reenvío
            // pero no llegó (o no se pudo leer) el contenido.
            log::warn!(
                "Reenvío {} sin snapshots legibles (flags={}, tipo de referencia={:?})",
                msg.id,
                msg.flags,
                msg.message_reference.as_ref().map(|r| r.kind)
            );
        }
        Some(Self {
            snapshots: msg
                .message_snapshots
                .iter()
                .map(|snapshot| {
                    let original = &snapshot.message;
                    ForwardedMessage {
                        content: original.content.clone(),
                        time: format_timestamp(&original.timestamp),
                        edited: original.edited_timestamp.is_some(),
                        mentions: original
                            .mentions
                            .iter()
                            .map(|u| (u.id.clone(), u.display_name().to_string()))
                            .collect(),
                        attachments: original.attachments.clone(),
                        embeds: original.embeds.clone(),
                        stickers: original.sticker_items.clone(),
                    }
                })
                .collect(),
            source_channel_id: msg.message_reference.as_ref().and_then(|r| r.channel_id.clone()),
            source_guild_id: msg.message_reference.as_ref().and_then(|r| r.guild_id.clone()),
        })
    }
}

/// Un mensaje de chat, usado tanto en DMs como en canales de servidor.
#[derive(Clone)]
pub struct ChatMessage {
    /// Id real de Discord del mensaje. Vacío en los mensajes de demo (no
    /// hace falta: nunca se les manda una reacción por REST).
    pub id: String,
    /// Nombre a mostrar del autor: el apodo que tiene en ESTE server si lo
    /// conocemos, si no su nombre global (o el username).
    pub author: String,
    /// Id de Discord del autor. Vacío en mensajes de demo/eco local.
    pub author_id: String,
    /// Nombre global/username del autor, sin apodo: es el que se usa
    /// mientras no sepamos el apodo, y el que se vuelve a mirar cuando el
    /// apodo cambia (ver `reresolve_names`).
    pub author_base: String,
    /// Color del rol más alto (con color) del autor en este server.
    /// `None` = sin rol con color, o todavía no conocemos sus roles: en ese
    /// caso el nombre se dibuja con el color de siempre.
    pub author_color: Option<Color32>,
    pub time: String,
    pub content: String,
    pub is_own: bool,
    pub avatar_color: Color32,
    /// URL real del avatar del autor (CDN de Discord). `None` en los
    /// mensajes de demo, donde se usa el círculo de color + inicial.
    pub avatar_url: Option<String>,
    /// (user_id, nombre a mostrar) de cada usuario mencionado en este
    /// mensaje — Discord los manda embebidos en el propio mensaje
    /// (`mentions`), así que no hace falta ir a buscarlos aparte para
    /// poder mostrar `<@id>` como "@Nombre" en vez del id pelado.
    pub mentions: Vec<(String, String)>,
    pub reactions: Vec<Reaction>,
    /// Archivos adjuntos (imágenes/videos/audio/otros) — ver `ui::media`.
    pub attachments: Vec<Attachment>,
    /// Embeds del mensaje (de bots/webhooks o generados por Discord a
    /// partir de links) — ver `ui::media`.
    pub embeds: Vec<Embed>,
    /// Stickers del mensaje — ver `ui::media::show_stickers`.
    pub stickers: Vec<StickerItem>,
    /// Si este mensaje es una respuesta a otro, la vista previa de ese
    /// original para mostrar arriba (ver `ui::chat::message_row`). `None`
    /// cuando el mensaje no responde a nada.
    pub replied_to: Option<RepliedMessage>,
    /// Si este mensaje es un REENVÍO (`message_snapshots`): el contenido del
    /// original, que se dibuja en un bloque "Reenviado" (el `content` propio
    /// del mensaje viene vacío). `None` en un mensaje normal.
    pub forwarded: Option<Forward>,
    /// Tipo de mensaje de Discord (`GatewayMessage::kind`): 0 normal, 18 =
    /// "X empezó un hilo" (mensaje de sistema). Los mensajes de demo/eco
    /// local son 0.
    pub kind: u8,
    /// Botones y demás componentes que un bot le puso al mensaje.
    pub components: Vec<Component>,
    /// Hilo que nació de este mensaje (tarjeta "Hilo de respuestas · N
    /// mensajes" debajo del mensaje).
    pub thread: Option<ThreadCard>,
    /// Canal REAL del mensaje. Casi siempre es el del chat donde se ve;
    /// difiere en el mensaje inicial de un hilo, que vive en el canal
    /// padre. Vacío en mensajes de demo/eco local.
    pub channel_id: String,
    /// App dueña del mensaje (para contestarle a sus botones).
    pub application_id: Option<String>,
    /// `flags` del mensaje de Discord (`1 << 6` = efímero).
    pub flags: u64,
    /// El autor es un bot/app: se dibuja la insignia APP junto al nombre.
    pub is_bot: bool,
    /// Bot verificado (tilde dentro de la insignia APP).
    pub bot_verified: bool,
    /// Si el mensaje es la respuesta a un comando: quién lo usó y cuál.
    pub interaction: Option<InteractionLine>,
}

/// Encabezado "X ha utilizado /comando" de la respuesta de un bot.
#[derive(Debug, Clone, Default)]
pub struct InteractionLine {
    pub user_id: String,
    pub user: String,
    pub avatar_url: Option<String>,
    pub avatar_color: Color32,
    pub command: String,
}

/// Un botón apretado en un mensaje (`ui::components` → `App::press_component`).
#[derive(Clone, Debug)]
pub struct ComponentClick {
    pub message_id: String,
    /// Canal real del mensaje; vacío = el canal abierto.
    pub channel_id: String,
    pub application_id: Option<String>,
    pub flags: u64,
    pub custom_id: String,
}

/// Un botón de bot al que se le mandó la interacción y que todavía no
/// contestó: se dibuja con un spinner adentro (`ui::components`) hasta que
/// llega la respuesta, falla, o vence el respaldo de `App::tick_pending_buttons`.
#[derive(Clone, Debug)]
pub struct PendingButton {
    pub message_id: String,
    pub custom_id: String,
    pub since: std::time::Instant,
}

fn pending_buttons_id() -> egui::Id {
    egui::Id::new("ecord_pending_buttons")
}

/// Deja la lista de botones cargando donde la UI de los mensajes la pueda
/// leer sin pasarla por toda la cadena de funciones del chat (mismo patrón
/// que `publish_guild_directory`). `App` la publica una vez por frame.
pub fn publish_pending_buttons(ctx: &egui::Context, pending: &[PendingButton]) {
    let keys: Vec<(String, String)> = pending
        .iter()
        .map(|p| (p.message_id.clone(), p.custom_id.clone()))
        .collect();
    ctx.data_mut(|d| d.insert_temp(pending_buttons_id(), std::sync::Arc::new(keys)));
}

/// `custom_id` de los botones de `message_id` que están cargando ahora.
pub fn loading_buttons(ctx: &egui::Context, message_id: &str) -> Vec<String> {
    ctx.data(|d| d.get_temp::<std::sync::Arc<Vec<(String, String)>>>(pending_buttons_id()))
        .map(|keys| {
            keys.iter()
                .filter(|(message, _)| message == message_id)
                .map(|(_, custom_id)| custom_id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Hilo que nació de un mensaje, tal como se resume debajo de él. El id del
/// hilo es el mismo que el del mensaje.
#[derive(Clone, Debug, Default)]
pub struct ThreadCard {
    pub id: String,
    pub name: String,
    pub message_count: u32,
    pub last_message_id: Option<String>,
    pub owner_id: Option<String>,
}

impl ThreadCard {
    pub fn from_thread(thread: &crate::discord::models::ThreadChannel) -> Self {
        Self {
            id: thread.id.clone(),
            name: thread.name.clone().unwrap_or_else(|| "Hilo".to_string()),
            message_count: thread.message_count.unwrap_or(0),
            last_message_id: thread.last_message_id.clone(),
            owner_id: thread.owner_id.clone(),
        }
    }

    /// Momento (ms desde epoch) del último mensaje del hilo, sacado del
    /// snowflake de `last_message_id`. `None` si el hilo no tiene mensajes.
    pub fn last_activity_ms(&self) -> Option<i64> {
        const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;
        let id: u64 = self.last_message_id.as_deref()?.parse().ok()?;
        Some((id >> 22) as i64 + DISCORD_EPOCH_MS)
    }
}

impl ChatMessage {
    pub fn them(author: &str, time: &str, content: &str, avatar_color: Color32) -> Self {
        Self {
            id: String::new(),
            author: author.to_string(),
            author_id: String::new(),
            author_base: author.to_string(),
            author_color: None,
            time: time.to_string(),
            content: content.to_string(),
            is_own: false,
            avatar_color,
            avatar_url: None,
            mentions: Vec::new(),
            reactions: Vec::new(),
            attachments: Vec::new(),
            embeds: Vec::new(),
            stickers: Vec::new(),
            replied_to: None,
            forwarded: None,
            kind: 0,
            components: Vec::new(),
            thread: None,
            channel_id: String::new(),
            application_id: None,
            flags: 0,
            is_bot: false,
            bot_verified: false,
            interaction: None,
        }
    }

    pub fn own(author: &str, time: &str, content: &str, avatar_color: Color32) -> Self {
        Self {
            id: String::new(),
            author: author.to_string(),
            author_id: String::new(),
            author_base: author.to_string(),
            author_color: None,
            time: time.to_string(),
            content: content.to_string(),
            is_own: true,
            avatar_color,
            avatar_url: None,
            mentions: Vec::new(),
            reactions: Vec::new(),
            attachments: Vec::new(),
            embeds: Vec::new(),
            stickers: Vec::new(),
            replied_to: None,
            forwarded: None,
            kind: 0,
            components: Vec::new(),
            thread: None,
            channel_id: String::new(),
            application_id: None,
            flags: 0,
            is_bot: false,
            bot_verified: false,
            interaction: None,
        }
    }

    /// Mensaje real recibido del Gateway/REST (`MESSAGE_CREATE`, historial
    /// de canal). `my_user_id` es el id del usuario logueado, para decidir
    /// si el mensaje se pinta como "propio".
    pub fn from_discord(msg: &crate::discord::models::GatewayMessage, my_user_id: &str) -> Self {
        Self::from_discord_in(msg, my_user_id, None)
    }

    /// Igual que [`Self::from_discord`], pero para un mensaje de un canal de
    /// server: con `names` el autor sale con el apodo que tiene en ese
    /// server y el color de su rol más alto, en vez del nombre global.
    pub fn from_discord_in(
        msg: &crate::discord::models::GatewayMessage,
        my_user_id: &str,
        names: Option<NameResolver<'_>>,
    ) -> Self {
        // Mensaje inicial de un hilo (tipo 21): no tiene contenido propio,
        // es una referencia al mensaje del canal padre. Como el cliente
        // real, se muestra ESE mensaje (con sus embeds y botones).
        if msg.kind == 21 {
            if let Some(original) = msg.referenced_message.as_deref() {
                return Self::from_discord_in(original, my_user_id, names);
            }
        }
        let author_base = msg.author.display_name().to_string();
        let (author, author_color) = match names {
            Some(names) => names.identity(&msg.author.id, &author_base),
            None => (author_base.clone(), None),
        };
        // Un bot que responde a una interacción no siempre manda
        // `application_id`; su propio id de usuario es el de la app.
        let application_id = msg
            .application_id
            .clone()
            .or_else(|| msg.author.bot.then(|| msg.author.id.clone()));
        Self {
            id: msg.id.clone(),
            author,
            author_id: msg.author.id.clone(),
            author_base,
            author_color,
            time: format_timestamp(&msg.timestamp),
            content: msg.content.clone(),
            is_own: msg.author.id == my_user_id,
            avatar_color: color_from_id(&msg.author.id),
            avatar_url: msg.author.avatar_url(),
            mentions: msg
                .mentions
                .iter()
                .map(|u| (u.id.clone(), u.display_name().to_string()))
                .collect(),
            reactions: msg
                .reactions
                .iter()
                .map(|r| Reaction {
                    emoji: r.emoji.kind(),
                    count: r.count,
                    reacted_by_me: r.me,
                })
                .collect(),
            attachments: msg.attachments.clone(),
            embeds: msg.embeds.clone(),
            stickers: msg.sticker_items.clone(),
            replied_to: Self::replied_to_from_discord(msg, names),
            forwarded: Forward::from_discord(msg),
            kind: msg.kind,
            components: msg.components.clone(),
            thread: msg.thread.as_ref().map(ThreadCard::from_thread).or_else(|| {
                // "X empezó un hilo" (tipo 18): el hilo es el canal al que
                // apunta la referencia del mensaje, y su nombre es el
                // contenido.
                if msg.kind != 18 {
                    return None;
                }
                let id = msg.message_reference.as_ref()?.channel_id.clone()?;
                Some(ThreadCard { id, name: msg.content.clone(), ..Default::default() })
            }),
            channel_id: msg.channel_id.clone(),
            application_id,
            flags: msg.flags,
            is_bot: msg.author.bot,
            bot_verified: msg.author.public_flags & (1 << 16) != 0,
            interaction: Self::interaction_from_discord(msg, names),
        }
    }

    /// Arma el encabezado "X ha utilizado /comando". Discord manda quién y
    /// cuál en `interaction` (viejo) y/o `interaction_metadata` (nuevo, el
    /// nombre del comando solo viene en versiones recientes): se juntan los
    /// dos. Sin usuario o sin nombre no hay nada que mostrar.
    fn interaction_from_discord(
        msg: &crate::discord::models::GatewayMessage,
        names: Option<NameResolver<'_>>,
    ) -> Option<InteractionLine> {
        let user = msg
            .interaction
            .as_ref()
            .and_then(|i| i.user.as_ref())
            .or_else(|| msg.interaction_metadata.as_ref().and_then(|i| i.user.as_ref()))?;
        let command = msg
            .interaction
            .as_ref()
            .and_then(|i| i.name.clone())
            .or_else(|| msg.interaction_metadata.as_ref().and_then(|i| i.name.clone()))
            .filter(|n| !n.is_empty())?;
        let base = user.display_name().to_string();
        let name = match names {
            Some(names) => names.identity(&user.id, &base).0,
            None => base,
        };
        Some(InteractionLine {
            user_id: user.id.clone(),
            user: name,
            avatar_url: user.avatar_url(),
            avatar_color: color_from_id(&user.id),
            command,
        })
    }

    /// ¿Es el mismo autor que `other`, a efectos de agrupar mensajes
    /// seguidos bajo un único encabezado? Compara por id cuando lo hay (dos
    /// personas pueden tener el mismo apodo) y por nombre si no (demo/eco).
    pub fn same_author(&self, other: &Self) -> bool {
        self.is_own == other.is_own
            && if !self.author_id.is_empty() && !other.author_id.is_empty() {
                self.author_id == other.author_id
            } else {
                self.author == other.author
            }
    }

    /// Vuelve a calcular nombre y color del autor (y del autor citado, si
    /// es una respuesta) con lo que `names` sabe ahora. Se usa cuando llega
    /// el apodo/rol de alguien después de que su mensaje ya se dibujó.
    pub fn reresolve_names(&mut self, names: NameResolver<'_>) {
        if !self.author_id.is_empty() {
            let (author, color) = names.identity(&self.author_id, &self.author_base);
            self.author = author;
            self.author_color = color;
        }
        if let Some(replied) = self.replied_to.as_mut() {
            if !replied.author_id.is_empty() {
                let (author, color) = names.identity(&replied.author_id, &replied.author_base);
                replied.author = author;
                replied.author_color = color;
            }
        }
    }

    /// Arma el `RepliedMessage` de este mensaje, si corresponde. Discord
    /// puede mandar tres casos distintos y hay que distinguirlos:
    /// 1. No es una respuesta → `message_reference` viene vacío → `None`.
    /// 2. Es una respuesta y Discord mandó el original embebido →
    ///    `referenced_message` trae autor/contenido → banner normal.
    /// 3. Es una respuesta pero el original ya no está (se borró, o es
    ///    muy viejo) → `message_reference` está pero `referenced_message`
    ///    no → banner de "mensaje eliminado", como en el cliente real.
    fn replied_to_from_discord(
        msg: &crate::discord::models::GatewayMessage,
        names: Option<NameResolver<'_>>,
    ) -> Option<RepliedMessage> {
        // Un reenvío también trae `message_reference` (con el id del
        // original) pero nunca `referenced_message`: no es una respuesta, y
        // sin esta guarda caería en el caso 3 y mostraría "Mensaje original
        // eliminado". Su contenido se dibuja aparte (`Forward`).
        if msg.is_forward() {
            return None;
        }
        if let Some(original) = &msg.referenced_message {
            let author_base = original.author.display_name().to_string();
            let (author, author_color) = match names {
                Some(names) => names.identity(&original.author.id, &author_base),
                None => (author_base.clone(), None),
            };
            return Some(RepliedMessage {
                author,
                author_id: original.author.id.clone(),
                author_base,
                author_color,
                preview: reply_preview(original),
                deleted: false,
            });
        }
        msg.message_reference.as_ref()?.message_id.as_ref()?;
        Some(RepliedMessage { deleted: true, ..Default::default() })
    }

    /// Aplica un `MESSAGE_UPDATE` (payload parcial): solo pisa lo que el
    /// evento trae. El caso típico es que lleguen `embeds` de un link
    /// que Discord recién terminó de resolver.
    pub fn apply_update(&mut self, update: &MessageUpdate) {
        if let Some(content) = &update.content {
            self.content = content.clone();
        }
        if let Some(mentions) = &update.mentions {
            self.mentions = mentions
                .iter()
                .map(|u| (u.id.clone(), u.display_name().to_string()))
                .collect();
        }
        if let Some(attachments) = &update.attachments {
            self.attachments = attachments.clone();
        }
        if let Some(embeds) = &update.embeds {
            self.embeds = embeds.clone();
        }
        if let Some(stickers) = &update.sticker_items {
            self.stickers = stickers.clone();
        }
        if let Some(components) = &update.components {
            self.components = components.clone();
        }
    }

    pub fn initial(&self) -> String {
        self.author
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string())
    }

    /// Prende/apaga la reacción propia para `emoji` en este mensaje,
    /// actualizando el contador — igual que clickear una reacción (o el
    /// botón de "agregar reacción") en el cliente real. Devuelve `true` si
    /// quedó puesta (había que avisarle a Discord con un PUT) o `false` si
    /// se sacó (DELETE). Si el contador llega a 0 la reacción desaparece
    /// de la lista en vez de quedar mostrando "emoji 0".
    pub fn toggle_reaction(&mut self, emoji: &ReactionKind) -> bool {
        if let Some(existing) = self.reactions.iter_mut().find(|r| &r.emoji == emoji) {
            if existing.reacted_by_me {
                existing.reacted_by_me = false;
                existing.count = existing.count.saturating_sub(1);
                if existing.count == 0 {
                    self.reactions.retain(|r| &r.emoji != emoji);
                }
                false
            } else {
                existing.reacted_by_me = true;
                existing.count += 1;
                true
            }
        } else {
            self.reactions.push(Reaction {
                emoji: emoji.clone(),
                count: 1,
                reacted_by_me: true,
            });
            true
        }
    }

    /// Builder chico para dejar reacciones ya puestas en un mensaje de
    /// demo (`ChatMessage::them(...).with_reactions(...)`). `reacted_by_me`
    /// en `true` simula que la cuenta logueada ya reaccionó así. Solo
    /// unicode — la demo no tiene emojis personalizados de ningún server.
    pub fn with_reactions(mut self, reactions: Vec<(&str, u32, bool)>) -> Self {
        self.reactions = reactions
            .into_iter()
            .map(|(emoji, count, reacted_by_me)| Reaction {
                emoji: ReactionKind::Unicode(emoji.to_string()),
                count,
                reacted_by_me,
            })
            .collect();
        self
    }
}

#[derive(Clone)]
pub struct Friend {
    pub name: String,
    pub status: Status,
    /// Actividad/estado personalizado que se muestra debajo del nombre.
    pub subtitle: Option<String>,
    /// Color de fondo del avatar placeholder, usado como fallback si
    /// `avatar_url` es `None` o la imagen todavía no cargó.
    pub avatar_color: Color32,
    /// URL real del avatar (CDN de Discord). `None` en los amigos de demo.
    pub avatar_url: Option<String>,
    /// Id real de Discord del usuario (para abrir el DM, pedir mensajes,
    /// etc.). Vacío en los amigos de demo.
    pub user_id: String,
    /// Id del canal de DM con este amigo, si ya se pidió con
    /// `RestClient::open_dm`. Se guarda para no volver a pedirlo cada vez
    /// que se abre la conversación.
    pub dm_channel_id: Option<String>,
    /// Datos de demo para la vista de perfil / DM.
    pub handle: String,
    pub member_since: String,
    pub mutual_servers: usize,
    pub messages: Vec<ChatMessage>,
    /// Si ya se pidió el historial real de este DM (para no repetir el
    /// pedido cada vez que se reabre la conversación en la misma sesión).
    /// `true` de entrada en los amigos de demo, que ya traen mensajes
    /// canned.
    pub loaded: bool,
    /// Pedido de historial inicial en curso (mientras `loaded` ya está en
    /// `true` de forma optimista pero la respuesta todavía no llegó) —
    /// controla el placeholder tipo Discord de `ui::chat::show` mientras
    /// no hay ni un mensaje para mostrar todavía.
    pub loading: bool,
    /// Pedido de "cargar más" (página más vieja) en curso — controla el
    /// spinner que aparece arriba del todo de la lista mientras se espera
    /// esa respuesta (ver `App::load_more_messages`).
    pub loading_more: bool,
    /// Si probablemente queda historial más viejo por cargar. `false` en
    /// los amigos de demo (no hay más nada atrás de los mensajes canned)
    /// y mientras no se sepa todavía; se actualiza con cada página que
    /// llega según si vino completa o no (ver `MESSAGES_PAGE_SIZE`).
    pub has_more: bool,
    /// `true` solo para amistades confirmadas (`READY.relationships` con
    /// tipo 1, o los amigos de demo). Un `Friend` con `false` existe solo
    /// para poder abrir el DM con alguien que no es amigo (desde la lista
    /// de DMs o desde un perfil): no se muestra en la lista de amigos ni
    /// cuenta en el contador de "Amigos".
    pub is_friend: bool,
}

const AIDEN: &str = "Aiden";
const AIDEN_COLOR: Color32 = Color32::from_rgb(30, 200, 110);

impl Friend {
    fn new(name: &str, status: Status, subtitle: Option<&str>, avatar_color: Color32) -> Self {
        let handle = format!(
            "{}{:02}",
            name.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>(),
            (name.len() * 7) % 90 + 10
        );
        let messages = vec![
            ChatMessage::them(name, "ayer a las 20:08", "Hola! ¿todo bien por ahí?", avatar_color),
            ChatMessage::own(AIDEN, "ayer a las 20:09", "Sí, todo tranqui por acá 🙂", AIDEN_COLOR),
            ChatMessage::them(
                name,
                "ayer a las 20:10",
                "Dale, cualquier cosa avisame",
                avatar_color,
            ),
        ];
        Self {
            name: name.to_string(),
            status,
            subtitle: subtitle.map(|s| s.to_string()),
            avatar_color,
            avatar_url: None,
            user_id: String::new(),
            dm_channel_id: None,
            handle,
            member_since: "14 may 2023".to_string(),
            mutual_servers: 2,
            messages,
            loaded: true,
            loading: false,
            loading_more: false,
            has_more: false,
            is_friend: true,
        }
    }

    pub fn initial(&self) -> String {
        self.name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string())
    }

    /// Amigo real construido a partir de una `Relationship` del Gateway
    /// (`READY.relationships`, `type == 1`). El estado en línea/DND/etc. no
    /// viaja en `READY` de forma directa para cuentas de usuario en todas
    /// las versiones del Gateway — si no te llega, esto cae a `Offline` y
    /// se actualiza solo cuando entre un evento `PRESENCE_UPDATE` (falta
    /// cablear ese evento en `discord::gateway`, ver el comentario ahí).
    pub fn from_relationship(rel: &crate::discord::models::Relationship) -> Self {
        let user = &rel.user;
        Self {
            name: user.display_name().to_string(),
            status: Status::Offline,
            subtitle: None,
            avatar_color: color_from_id(&user.id),
            avatar_url: user.avatar_url(),
            user_id: user.id.clone(),
            dm_channel_id: None,
            handle: format!(
                "{}#{}",
                user.username,
                user.discriminator.as_deref().unwrap_or("0000")
            ),
            member_since: String::new(),
            mutual_servers: 0,
            messages: Vec::new(),
            loaded: false,
            loading: false,
            loading_more: false,
            has_more: true,
            is_friend: true,
        }
    }

    /// A partir de una conversación de la lista de DMs recientes
    /// (`App::dms`, viene de `READY.private_channels`). A diferencia de
    /// `from_relationship`, acá el otro usuario puede no ser "amigo"
    /// confirmado (podés tener un DM abierto con alguien sin ser mutuals)
    /// — por eso esto vive aparte y se usa como fallback cuando el
    /// `user_id` del DM no matchea ningún `Friend` ya cargado.
    pub fn from_dm_channel(dm: &crate::discord::models::PrivateChannel) -> Option<Self> {
        let recipient = dm.recipients.first()?;
        Some(Self {
            name: if dm.username.is_empty() {
                recipient.display_name().to_string()
            } else {
                dm.username.clone()
            },
            status: Status::Offline,
            subtitle: None,
            avatar_color: color_from_id(&recipient.id),
            avatar_url: dm.avatar_url.clone().or_else(|| recipient.avatar_url()),
            user_id: recipient.id.clone(),
            dm_channel_id: Some(dm.id.clone()),
            handle: format!(
                "{}#{}",
                recipient.username,
                recipient.discriminator.as_deref().unwrap_or("0000")
            ),
            member_since: String::new(),
            mutual_servers: 0,
            messages: Vec::new(),
            loaded: false,
            loading: false,
            loading_more: false,
            has_more: true,
            is_friend: false,
        })
    }
}

/// Color determinístico a partir de un id de Discord (snowflake), para
/// usar como fondo del avatar placeholder mientras no hay `avatar_url` o
/// la imagen todavía no terminó de cargar.
fn color_from_id(id: &str) -> Color32 {
    let hash: u64 = id.parse().unwrap_or_else(|_| {
        id.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
    });
    let hue = (hash % 360) as f32;
    let (r, g, b) = hsl_to_rgb(hue, 0.55, 0.55);
    Color32::from_rgb(r, g, b)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
    )
}

/// Formatea un timestamp ISO-8601 (como los que manda Discord, ej.
/// `"2026-09-15T14:32:00.123000+00:00"`) a `"HH:MM"`. Best-effort: si el
/// formato no matchea devuelve el string tal cual llegó en vez de
/// explotar.
/// Resumen del mensaje citado para el banner de una respuesta. Si el original
/// no tenía texto (solo una imagen, un sticker...) se dice qué era en vez de
/// dejar el banner con el autor solo.
fn reply_preview(original: &crate::discord::models::GatewayMessage) -> String {
    if !original.content.trim().is_empty() {
        return crate::ui::chat::preview_text(&original.content);
    }
    if original.is_forward() {
        "Mensaje reenviado".to_string()
    } else if !original.attachments.is_empty() {
        "Adjunto".to_string()
    } else if !original.sticker_items.is_empty() {
        "Sticker".to_string()
    } else if !original.embeds.is_empty() {
        "Embed".to_string()
    } else {
        String::new()
    }
}

fn format_timestamp(ts: &str) -> String {
    ts.split('T')
        .nth(1)
        .and_then(|time_part| time_part.get(0..5))
        .map(|s| s.to_string())
        .unwrap_or_else(|| ts.to_string())
}

/// Lista de amigos de demostración (nombres inventados, no son datos reales).
pub fn demo_friends() -> Vec<Friend> {
    vec![
        Friend::new(
            "Nightowl",
            Status::Idle,
            Some("un poco cansado pero de buen humor"),
            Color32::from_rgb(120, 100, 200),
        ),
        Friend::new(
            "Riverside",
            Status::Dnd,
            Some("League of Legends · jugando una partida"),
            Color32::from_rgb(210, 90, 90),
        ),
        Friend::new(
            "Marbleflux",
            Status::Online,
            Some("En línea"),
            Color32::from_rgb(90, 170, 210),
        ),
        Friend::new(
            "Bin A Lenda",
            Status::Online,
            Some("En línea"),
            Color32::from_rgb(200, 170, 60),
        ),
        Friend::new(
            "BotsitoLex",
            Status::Idle,
            Some("Ausente"),
            Color32::from_rgb(90, 200, 140),
        ),
        Friend::new(
            "Briiith",
            Status::Idle,
            Some("Ausente"),
            Color32::from_rgb(220, 140, 180),
        ),
        Friend::new(
            "Camikase",
            Status::Online,
            Some("En línea"),
            Color32::from_rgb(150, 120, 210),
        ),
        Friend::new(
            "Chunny",
            Status::Dnd,
            Some("No molestar"),
            Color32::from_rgb(80, 150, 200),
        ),
        Friend::new(
            "Cole-sterol",
            Status::Online,
            Some("ROBLOX"),
            Color32::from_rgb(190, 90, 60),
        ),
        Friend::new(
            "Dare",
            Status::Dnd,
            Some("kick.com/demo"),
            Color32::from_rgb(60, 160, 160),
        ),
    ]
}

/// Una tarjeta del panel "Activo ahora": quién es y qué está haciendo.
pub struct ActiveNowCard {
    pub user_id: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub avatar_color: Color32,
    pub status: Status,
    /// `true` si es la actividad de la propia cuenta.
    pub is_me: bool,
    pub activity: crate::discord::models::PresenceActivity,
}

pub struct ActivityCard {
    pub name: String,
    pub detail: String,
    pub time: String,
    pub avatar_color: Color32,
    /// Si es Some, se muestra un botón "Conecta..." (p. ej. juegos co-op).
    pub connect_label: Option<String>,
}

impl ActivityCard {
    fn new(
        name: &str,
        detail: &str,
        time: &str,
        avatar_color: Color32,
        connect_label: Option<&str>,
    ) -> Self {
        Self {
            name: name.to_string(),
            detail: detail.to_string(),
            time: time.to_string(),
            avatar_color,
            connect_label: connect_label.map(|s| s.to_string()),
        }
    }

    pub fn initial(&self) -> String {
        self.name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string())
    }
}

/// Panel "Activo ahora" de demostración.
pub fn demo_activity() -> Vec<ActivityCard> {
    vec![
        ActivityCard::new(
            "Riverside",
            "League of Legends · 2 min",
            "",
            Color32::from_rgb(210, 90, 90),
            Some("Conecta..."),
        ),
        ActivityCard::new(
            "Nightsky",
            "Genshin Impact · 2 h",
            "",
            Color32::from_rgb(120, 100, 200),
            None,
        ),
        ActivityCard::new(
            "Willstar",
            "Lv.58 · America",
            "",
            Color32::from_rgb(90, 170, 210),
            Some("Conecta..."),
        ),
        ActivityCard::new(
            "FruntiN",
            "Roblox · 1 h",
            "",
            Color32::from_rgb(200, 170, 60),
            None,
        ),
        ActivityCard::new(
            "Ignacio",
            "Dead by Daylight · 43 min",
            "",
            Color32::from_rgb(90, 60, 60),
            None,
        ),
        ActivityCard::new(
            "Grupo",
            "Dead by Daylight · 1 persona",
            "",
            Color32::from_rgb(60, 60, 70),
            None,
        ),
    ]
}

// ---------------------------------------------------------------------
// Servidores (rail izquierdo -> vista de canales, estilo imagen 3)
// ---------------------------------------------------------------------

/// Alguien conectado a un canal de voz, para mostrar debajo del canal en
/// el rail (avatar + nombre, estilo Discord).
#[derive(Clone)]
pub struct VoiceOccupant {
    pub user_id: String,
    pub name: String,
    pub avatar_color: Color32,
    pub avatar_url: Option<String>,
    /// `false` si el `VoiceState` de origen no traía `member.user`
    /// embebido (queda con el nombre placeholder "Usuario desconocido"
    /// hasta que responda el pedido REST de respaldo, ver
    /// `spawn_fetch_user`/`AppEvent::UserFetched`).
    pub known: bool,
    /// Mic propio apagado por la persona (`VoiceState::self_mute`). A
    /// diferencia de `known`/`name`/`avatar_url`, esto SIEMPRE se toma del
    /// último `VoiceState` recibido, nunca de un occupant anterior — ver el
    /// comentario en `Server::apply_voice_snapshot`.
    pub self_mute: bool,
    /// Ensordecido (`VoiceState::self_deaf`); en el cliente real ensordecer
    /// implica silenciarse también, así que esto suele venir junto con
    /// `self_mute == true`.
    pub self_deaf: bool,
    /// Transmite su pantalla (Go Live) desde este canal
    /// (`VoiceState::self_stream`). Igual que `self_mute`, siempre sale del
    /// último `VoiceState` recibido.
    pub streaming: bool,
}

impl VoiceOccupant {
    pub fn from_voice_state(state: &crate::discord::models::VoiceState) -> Self {
        Self {
            user_id: state.user_id.clone(),
            name: state.display_name(),
            avatar_color: color_from_id(&state.user_id),
            avatar_url: state.avatar_url(),
            known: state.known(),
            self_mute: state.self_mute,
            self_deaf: state.self_deaf,
            streaming: state.self_stream,
        }
    }

    pub fn initial(&self) -> String {
        self.name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string())
    }
}

#[derive(Clone)]
pub struct Channel {
    pub name: String,
    pub messages: Vec<ChatMessage>,
    /// Id real del canal en Discord (`None` en canales demo). Se usa para
    /// pedir el historial por REST la primera vez que se abre.
    pub channel_id: Option<String>,
    /// Si ya se pidió el historial de este canal (para no repetir el
    /// pedido cada vez que se vuelve a abrir en la misma sesión).
    pub loaded: bool,
    /// Pedido de historial inicial en curso — ver `Friend::loading`, es
    /// el mismo concepto para canales de server.
    pub loading: bool,
    /// Pedido de "cargar más" (página más vieja) en curso — ver
    /// `Friend::loading_more`.
    pub loading_more: bool,
    /// Si probablemente queda historial más viejo por cargar — ver
    /// `Friend::has_more`.
    pub has_more: bool,
    /// `true` para canales de voz/stage (tipo 2/13): no tienen mensajes,
    /// en cambio se les muestra la lista de quién está conectado.
    pub is_voice: bool,
    /// Quiénes están conectados ahora mismo a este canal de voz. Vacío
    /// para canales de texto, o mientras no llegó `GUILD_CREATE`/no se
    /// conectó nadie todavía.
    pub voice_members: Vec<VoiceOccupant>,
    /// Overwrites de permisos del canal, tal como llegaron de Discord (ver
    /// `lib::permissions`). Vacío en canales demo.
    pub overwrites: Vec<crate::discord::models::PermissionOverwrite>,
    /// Qué tan accesible es para esta cuenta: si se ve, si va con candado.
    /// Lo recalcula `Server::recompute_access` cada vez que llega algo que
    /// lo cambia (canales, roles, roles propios).
    pub access: crate::lib::permissions::ChannelAccess,
    /// Foro (tipo 15) o canal de medios (16): en vez de mensajes muestra la
    /// lista de posts (`forum`).
    pub is_forum: bool,
    /// Un post de foro abierto como canal propio (sus mensajes son las
    /// respuestas). Estos canales viven en una categoría oculta al final
    /// de `Server::categories` y NO se dibujan en la barra lateral.
    pub is_thread: bool,
    /// Foro al que pertenece, para los `is_thread` (sirve para volver).
    pub parent_id: Option<String>,
    /// Posts, etiquetas y formulario de un foro. Vacío en el resto.
    pub forum: ForumState,
}

/// Todo lo de un canal de foro que no son mensajes.
#[derive(Clone, Default)]
pub struct ForumState {
    /// Etiquetas disponibles (`available_tags`).
    pub tags: Vec<crate::discord::models::ForumTag>,
    pub posts: Vec<ForumPost>,
    /// Ya llegó la primera página.
    pub loaded: bool,
    pub loading: bool,
    pub has_more: bool,
    /// Offset de la próxima página (posts ya pedidos).
    pub next_offset: u32,
    /// Orden: `false` = actividad reciente, `true` = fecha de creación.
    pub by_creation: bool,
    /// Filtro local por texto (título / vista previa).
    pub search: String,
    /// Formulario de "Nueva publicación".
    pub composing: bool,
    pub creating: bool,
    pub draft_title: String,
    pub draft_body: String,
    pub draft_tags: Vec<String>,
}

/// Una tarjeta de la lista de un foro.
#[derive(Clone)]
pub struct ForumPost {
    /// Id del hilo (también el del mensaje inicial).
    pub id: String,
    pub title: String,
    pub author: String,
    pub author_color: Option<Color32>,
    /// Primeras líneas del mensaje inicial, en una sola línea.
    pub preview: String,
    /// Primera imagen adjunta del mensaje inicial.
    pub thumbnail: Option<String>,
    pub tag_ids: Vec<String>,
    /// Reacción unicode más usada del mensaje inicial: (emoji, cantidad).
    pub reaction: Option<(String, u32)>,
    pub replies: u32,
    /// Milisegundos desde epoch de la última actividad.
    pub last_activity_ms: i64,
    pub pinned: bool,
}

/// Milisegundos desde epoch, ahora.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Momento de creación (ms desde epoch) codificado en un snowflake.
pub fn snowflake_ms(id: &str) -> Option<i64> {
    let value: u64 = id.parse().ok()?;
    if value == 0 {
        return None;
    }
    Some(((value >> 22) + 1_420_070_400_000) as i64)
}

impl ForumPost {
    /// Arma la tarjeta de un hilo, con lo que se pueda sacar de su mensaje
    /// inicial (`first`, si vino).
    pub fn from_thread(
        thread: &crate::discord::models::ThreadChannel,
        first: Option<&crate::discord::models::GatewayMessage>,
        resolver: &NameResolver,
    ) -> Self {
        let (author, author_color) = match first {
            Some(m) => resolver.identity(&m.author.id, m.author.display_name()),
            None => (String::new(), None),
        };
        let preview = first
            .map(|m| {
                m.content
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(240)
                    .collect::<String>()
            })
            .unwrap_or_default();
        let thumbnail = first.and_then(|m| {
            m.attachments
                .iter()
                .find(|a| a.content_type.as_deref().is_some_and(|c| c.starts_with("image/")))
                .map(|a| match &a.proxy_url {
                    Some(proxy) if !proxy.contains('?') => format!("{proxy}?width=192&height=192"),
                    _ => a.url.clone(),
                })
        });
        let reaction = first.and_then(|m| {
            m.reactions
                .iter()
                .filter(|r| r.emoji.id.is_none())
                .max_by_key(|r| r.count)
                .and_then(|r| r.emoji.name.clone().map(|name| (name, r.count)))
        });
        let last_activity_ms = thread
            .last_message_id
            .as_deref()
            .and_then(snowflake_ms)
            .or_else(|| snowflake_ms(&thread.id))
            .unwrap_or(0);
        Self {
            id: thread.id.clone(),
            title: thread.name.clone().unwrap_or_else(|| "Sin título".to_string()),
            author,
            author_color,
            preview,
            thumbnail,
            tag_ids: thread.applied_tags.clone(),
            reaction,
            replies: thread.message_count.unwrap_or(0),
            last_activity_ms,
            pinned: thread.is_pinned(),
        }
    }
}

impl Channel {
    /// Pasa a este canal (recién armado a partir de los datos de Discord)
    /// el estado que ya tenía el anterior con el mismo id: mensajes,
    /// conectados a voz y posts de foro. Sin esto, un `CHANNEL_UPDATE`
    /// borraría todo lo cargado del canal.
    fn carry_over(&mut self, prev: Channel) {
        if self.is_voice != prev.is_voice || self.is_forum != prev.is_forum {
            return; // cambió de tipo: no hay nada que conservar
        }
        self.messages = prev.messages;
        self.loaded = prev.loaded;
        self.loading = prev.loading;
        self.loading_more = prev.loading_more;
        self.has_more = prev.has_more;
        self.voice_members = prev.voice_members;
        // Las etiquetas son las nuevas; lo demás del foro, lo de antes.
        let tags = std::mem::take(&mut self.forum.tags);
        self.forum = prev.forum;
        self.forum.tags = tags;
    }

    fn new(name: &str, messages: Vec<ChatMessage>) -> Self {
        Self {
            name: name.to_string(),
            messages,
            channel_id: None,
            loaded: true,
            loading: false,
            loading_more: false,
            has_more: false,
            is_voice: false,
            voice_members: Vec::new(),
            overwrites: Vec::new(),
            access: Default::default(),
            is_forum: false,
            is_thread: false,
            parent_id: None,
            forum: ForumState::default(),
        }
    }

    fn from_discord(id: String, name: &str, kind: u8) -> Self {
        let is_voice = matches!(kind, 2 | 13);
        // Foro (15) y canal de medios (16): no tienen mensajes propios, sus
        // "mensajes" son los posts (hilos) que se piden aparte.
        let is_forum = matches!(kind, 15 | 16);
        Self {
            name: name.to_string(),
            messages: Vec::new(),
            channel_id: Some(id),
            // Un canal de voz tiene su propio chat de texto (el panel de
            // la vista de la llamada), así que pide historial como un canal
            // de texto. Un foro no: sus "mensajes" son los posts.
            loaded: is_forum,
            loading: false,
            loading_more: false,
            has_more: !is_forum,
            is_voice,
            voice_members: Vec::new(),
            overwrites: Vec::new(),
            access: Default::default(),
            is_forum,
            is_thread: false,
            parent_id: None,
            forum: ForumState::default(),
        }
    }
}

#[derive(Clone)]
pub struct ChannelCategory {
    pub name: String,
    pub channels: Vec<Channel>,
}

#[derive(Clone)]
pub struct Member {
    pub name: String,
    pub avatar_color: Color32,
    pub avatar_url: Option<String>,
    /// Id real de Discord. Vacío en los miembros de demo.
    pub user_id: String,
    /// Estado en línea (punto sobre el avatar).
    pub status: Status,
    /// Color del rol más alto que tenga color. `None` = sin rol con color:
    /// el nombre se dibuja con el color de texto normal.
    pub name_color: Option<Color32>,
    /// Estado personalizado o actividad ("Jugando a X").
    pub subtitle: Option<String>,
}

#[derive(Clone)]
pub struct MemberGroup {
    pub name: String,
    pub members: Vec<Member>,
}

pub struct Server {
    pub name: String,
    pub icon_initial: String,
    pub icon_color: Color32,
    /// URL real del ícono del server (CDN de Discord). `None` en los
    /// servers de demo, o en servers reales sin ícono configurado.
    pub icon_url: Option<String>,
    /// Id real de Discord del server. Vacío en los servers de demo.
    pub guild_id: String,
    pub topic: String,
    pub categories: Vec<ChannelCategory>,
    pub member_groups: Vec<MemberGroup>,
    pub online_count: usize,
    pub total_count: usize,
    /// Roles del server (para nombrar y colorear los grupos de la lista de
    /// miembros). Vacío en los servers de demo.
    pub roles: Vec<crate::discord::models::Role>,
    /// Las listas de miembros que Discord mantiene para este server, por
    /// id de lista (`everyone` o un hash de permisos: canales con distintos
    /// permisos tienen listas distintas). Cada una es una lista PLANA donde
    /// se alternan encabezados de grupo y miembros, sobre la que el Gateway
    /// manda operaciones por índice (ver `MemberListState::apply_ops`).
    /// Se guardan todas, sin descartar ninguna: las novedades de una lista
    /// que ya no se ve no pueden pisar la que sí se ve.
    pub member_lists: std::collections::HashMap<String, MemberListState>,
    /// Id de la lista que se está mostrando: la última que llegó con un
    /// `SYNC` (la respuesta a una suscripción). De acá sale `member_groups`.
    pub active_member_list: String,
    /// Qué lista le tocó a cada canal (id de canal → id de lista), aprendido
    /// de las respuestas a las suscripciones. Sirve para no volver a pedir
    /// la lista al pasar a un canal que comparte la que ya se está viendo.
    pub channel_member_list: std::collections::HashMap<String, String>,
    /// Canal al que se le acaba de pedir la lista y todavía no contestó
    /// Discord: el próximo `SYNC` es la respuesta y le asigna su lista.
    pub pending_list_channel: Option<String>,
    /// Emojis personalizados de este server, para el picker de
    /// reacciones. Vacío en los servers de demo y en cualquier server
    /// real sin emojis subidos.
    pub custom_emojis: Vec<CustomEmoji>,
    /// Stickers personalizados de este server (pestaña "Stickers" del
    /// selector). Vacío en los servers de demo y en los reales sin stickers.
    pub custom_stickers: Vec<StickerItem>,
    /// Apodo y roles de los miembros que ya conocemos, por id de usuario.
    /// Se llena con lo que Discord manda embebido en los mensajes en vivo,
    /// con la lista de miembros y con los `GUILD_MEMBERS_CHUNK` que se piden
    /// para los autores del historial. Es lo que permite mostrar en el chat
    /// el nombre del server y el color del rol.
    pub member_info: std::collections::HashMap<String, MemberInfo>,
    /// Ids que ya se le pidieron al Gateway (opcode 8), para no volver a
    /// pedir a quien no está (bots, webhooks, gente que se fue del server).
    pub requested_members: std::collections::HashSet<String>,
    /// Estados de voz que llegaron para un canal que todavía no conocemos
    /// (el guild vino "unavailable" o sus canales aún no se pidieron). Se
    /// acomodan solos apenas llegan los canales (ver `apply_channels`).
    pub pending_voice_states: Vec<crate::discord::models::VoiceState>,
    /// Nombre global y avatar de los usuarios ya vistos, por id (ver
    /// [`KnownUser`]).
    pub known_users: std::collections::HashMap<String, KnownUser>,
    /// Datos para calcular qué canales puede ver la cuenta (dueño, id
    /// propio, roles propios). Ver `lib::permissions`.
    pub access_ctx: crate::lib::permissions::AccessContext,
    /// Los canales de Discord tal cual llegaron (incluye las categorías,
    /// tipo 4). `CHANNEL_CREATE/UPDATE/DELETE` modifican esta lista y de
    /// ahí se rearman las categorías (ver `apply_channels`). Vacía = todavía
    /// no se pidieron los canales de este server.
    pub raw_channels: Vec<crate::discord::models::Channel>,
}

impl Server {
    /// Índice del primer canal VISIBLE (categoría, canal), para abrir por
    /// defecto. Si no hay ninguno (server vacío / demo) cae a `(0, 0)`.
    pub fn first_channel(&self) -> (usize, usize) {
        for (cat_idx, category) in self.categories.iter().enumerate() {
            if let Some(chan_idx) = category.channels.iter().position(|c| c.access.can_view) {
                return (cat_idx, chan_idx);
            }
        }
        (0, 0)
    }

    /// ¿Se muestra este canal? (`false` = la cuenta no lo puede ver con su
    /// rol actual.) Un índice que no existe cuenta como visible.
    pub fn channel_visible(&self, category: usize, channel: usize) -> bool {
        // Un post de foro abierto nunca está en la barra lateral (por eso su
        // `can_view` es `false`), pero sí se puede estar viendo.
        self.channel(category, channel)
            .is_none_or(|c| c.is_thread || c.access.can_view)
    }

    /// Recalcula `Channel::access` de todos los canales con lo que se sabe
    /// ahora. Se llama cada vez que cambia algo de lo que depende: los
    /// canales, los roles del server o los roles propios.
    pub fn recompute_access(&mut self) {
        let guild_id = self.guild_id.clone();
        if guild_id.is_empty() {
            return; // server de demo: sin permisos que calcular
        }
        // Diagnóstico (nivel debug): cuando un canal se ve o se esconde mal,
        // casi siempre es porque falta algún dato y el cálculo cae a "todo
        // visible" (fail-open) — esto lo deja en el log.
        let complete = self.access_ctx.my_roles.is_some() && !self.access_ctx.my_id.is_empty();
        for category in &mut self.categories {
            for channel in &mut category.channels {
                if channel.is_thread {
                    continue; // hereda del foro; se muestra aparte
                }
                channel.access = crate::lib::permissions::channel_access(
                    &guild_id,
                    &self.access_ctx,
                    &self.roles,
                    &channel.overwrites,
                    channel.is_voice,
                );
            }
        }
        if crate::logging::debug_logging_enabled() {
            let hidden = self
                .categories
                .iter()
                .flat_map(|c| c.channels.iter())
                .filter(|c| !c.is_thread && !c.access.can_view)
                .count();
            crate::logging::debug(
                "access",
                format!(
                    "server {}: datos completos={complete} (roles propios={:?}, dueño conocido={}, roles del server={}) -> {hidden} canales ocultos",
                    guild_id,
                    self.access_ctx.my_roles,
                    self.access_ctx.owner_id.is_some(),
                    self.roles.len(),
                ),
            );
        }
    }

    /// Guarda los roles propios (`READY.merged_members`, `GUILD_MEMBER_UPDATE`
    /// de la propia cuenta) y recalcula el acceso.
    pub fn set_my_roles(&mut self, roles: Vec<String>) {
        self.access_ctx.my_roles = Some(roles);
        self.recompute_access();
    }

    pub fn channel(&self, category: usize, channel: usize) -> Option<&Channel> {
        self.categories.get(category)?.channels.get(channel)
    }

    pub fn channel_mut(&mut self, category: usize, channel: usize) -> Option<&mut Channel> {
        self.categories.get_mut(category)?.channels.get_mut(channel)
    }

    /// Busca un canal por su id real de Discord en cualquier categoría
    /// (usado para aplicarle estados de voz que llegan por su `channel_id`
    /// sin saber de antemano en qué categoría vive).
    pub fn channel_by_id_mut(&mut self, channel_id: &str) -> Option<&mut Channel> {
        self.categories
            .iter_mut()
            .flat_map(|cat| cat.channels.iter_mut())
            .find(|c| c.channel_id.as_deref() == Some(channel_id))
    }

    /// Saca a `user_id` de la lista de conectados de TODOS los canales de
    /// voz del server (se usa antes de aplicar un nuevo estado, porque un
    /// `VOICE_STATE_UPDATE` no avisa de qué canal se fue, solo a cuál
    /// está ahora — o a ninguno si se desconectó).
    pub fn remove_voice_member(&mut self, user_id: &str) {
        self.pending_voice_states.retain(|s| s.user_id != user_id);
        for cat in &mut self.categories {
            for ch in &mut cat.channels {
                ch.voice_members.retain(|m| m.user_id != user_id);
            }
        }
    }

    /// Arma las categorías/canales a partir de la lista plana que devuelve
    /// `GET /guilds/{id}/channels`. Nos quedamos con los canales de texto
    /// (`kind == 0`, texto normal; `5`, anuncios; `15`, foro) y de voz
    /// (`2`, voz; `13`, stage) — el resto (categorías, tipo `4`, ya se
    /// procesaron aparte) se descarta.
    ///
    /// Nota: esto NO trae la lista de miembros del server (`member_groups`
    /// queda vacía). Discord no la manda por REST para cuentas de usuario
    /// del mismo modo que para bots; hay que pedirla por el Gateway con el
    /// opcode 8 (`REQUEST_GUILD_MEMBERS`) o suscribirse al mecanismo de
    /// "lazy guilds" (`GUILD_MEMBER_LIST_UPDATE`), que no está implementado
    /// todavía en `discord::gateway`.
    pub fn apply_channels(&mut self, channels: Vec<crate::discord::models::Channel>) {
        // Copia cruda para poder rearmar todo cuando llegue un
        // `CHANNEL_CREATE/UPDATE/DELETE` (ver `upsert_channel`).
        self.raw_channels = channels.clone();
        self.rebuild_channels(channels);
    }

    /// Rearma `categories` desde la lista plana de canales SIN volver a
    /// guardarla en `raw_channels` (`apply_channels` ya lo hizo, o quien llama
    /// la sacó de ahí). Así `upsert_channel`/`remove_channel` clonan la lista
    /// una vez por evento en vez de dos.
    fn rebuild_channels(&mut self, channels: Vec<crate::discord::models::Channel>) {
        use std::collections::HashMap;

        // Lo que ya había: se rearma desde cero, pero mensajes, conectados
        // a voz, posts de foro y posts abiertos se conservan (ver más abajo).
        let previous = std::mem::take(&mut self.categories);

        let mut categories: HashMap<Option<String>, Vec<crate::discord::models::Channel>> =
            HashMap::new();
        // Se arma un `Channel` de la UI conservando los overwrites, para
        // poder recalcular el acceso cuando llegan los roles más tarde.
        fn build(c: crate::discord::models::Channel) -> Channel {
            let mut ch = Channel::from_discord(c.id.clone(), c.name.as_deref().unwrap_or("canal"), c.kind);
            ch.overwrites = c.permission_overwrites;
            ch.parent_id = c.parent_id;
            ch.forum.tags = c.available_tags;
            ch
        }
        // Antes solo se guardaba el nombre de cada categoría acá, así que
        // el orden final se armaba ordenando alfabéticamente por nombre —
        // básicamente al azar respecto al orden real que el usuario armó
        // arrastrando categorías en Discord. Ahora guardamos también su
        // `position` para poder ordenar por eso, como corresponde.
        let mut category_info: HashMap<String, (String, i64)> = HashMap::new();

        // Primero todas las categorías, para saber cuáles existen: un canal
        // cuya categoría se borró (o no vino) pasa a "sin categoría" en vez
        // de quedar en una categoría fantasma sin nombre.
        for ch in &channels {
            if ch.kind == 4 {
                category_info.insert(
                    ch.id.clone(),
                    (ch.name.clone().unwrap_or_default(), ch.position.unwrap_or(0)),
                );
            }
        }
        for ch in channels {
            if ch.kind == 4 {
                continue;
            }
            if !matches!(ch.kind, 0 | 5 | 15 | 16 | 2 | 13) {
                continue; // group-dm, etc.
            }
            let parent = ch.parent_id.clone().filter(|id| category_info.contains_key(id));
            categories.entry(parent).or_default().push(ch);
        }

        let mut result: Vec<ChannelCategory> = Vec::new();

        // Canales sin categoría van primero, como en el cliente real, y
        // ordenados entre sí por su propia `position`.
        if let Some(mut uncategorized) = categories.remove(&None) {
            uncategorized.sort_by_key(|c| c.position.unwrap_or(0));
            result.push(ChannelCategory {
                name: String::new(),
                channels: uncategorized.into_iter().map(build).collect(),
            });
        }

        let mut entries: Vec<_> = categories.into_iter().collect();
        // Orden real de las categorías: por su `position` de Discord, no
        // por nombre. Una categoría sin info (no debería pasar salvo datos
        // raros) cae al final con posición 0.
        entries.sort_by_key(|(parent_id, _)| {
            parent_id
                .as_ref()
                .and_then(|id| category_info.get(id))
                .map(|(_, position)| *position)
                .unwrap_or(0)
        });

        for (parent_id, mut chans) in entries {
            // Dentro de cada categoría, los canales también van por su
            // propia `position` (esto ya estaba bien).
            chans.sort_by_key(|c| c.position.unwrap_or(0));
            let name = parent_id
                .and_then(|id| category_info.get(&id).map(|(name, _)| name.clone()))
                .unwrap_or_default();
            result.push(ChannelCategory {
                name,
                channels: chans.into_iter().map(build).collect(),
            });
        }

        // Estado que hay que conservar de la versión anterior: el de cada
        // canal que sigue existiendo, y los posts de foro abiertos.
        let mut old_channels: HashMap<String, Channel> = HashMap::new();
        let mut open_threads: Vec<Channel> = Vec::new();
        for category in previous {
            for channel in category.channels {
                if channel.is_thread {
                    open_threads.push(channel);
                } else if let Some(id) = channel.channel_id.clone() {
                    old_channels.insert(id, channel);
                }
            }
        }
        for category in &mut result {
            for channel in &mut category.channels {
                let id = channel.channel_id.clone();
                if let Some(prev) = id.as_deref().and_then(|id| old_channels.remove(id)) {
                    channel.carry_over(prev);
                }
            }
        }
        if !open_threads.is_empty() {
            result.push(ChannelCategory { name: String::new(), channels: open_threads });
        }

        self.categories = result;
        self.recompute_access();
        self.place_pending_voice_states();
    }

    /// `CHANNEL_CREATE` / `CHANNEL_UPDATE`: agrega o reemplaza un canal y
    /// rearma las categorías. Si todavía no se pidieron los canales de este
    /// server no se hace nada: se van a pedir enteros al abrirlo, y armar
    /// una lista con un solo canal haría que nunca se pidan.
    pub fn upsert_channel(&mut self, channel: crate::discord::models::Channel) {
        if self.raw_channels.is_empty() {
            return;
        }
        match self.raw_channels.iter().position(|c| c.id == channel.id) {
            Some(index) => self.raw_channels[index] = channel,
            None => self.raw_channels.push(channel),
        }
        let raw = self.raw_channels.clone();
        self.rebuild_channels(raw);
    }

    /// `CHANNEL_DELETE`.
    pub fn remove_channel(&mut self, channel_id: &str) {
        if self.raw_channels.is_empty() {
            return;
        }
        self.raw_channels.retain(|c| c.id != channel_id);
        let raw = self.raw_channels.clone();
        self.rebuild_channels(raw);
    }

    /// `(categoría, canal)` del canal con ese id real de Discord, si existe
    /// (incluye los posts de foro abiertos).
    pub fn channel_position_by_id(&self, channel_id: &str) -> Option<(usize, usize)> {
        self.categories.iter().enumerate().find_map(|(cat_idx, category)| {
            category
                .channels
                .iter()
                .position(|c| c.channel_id.as_deref() == Some(channel_id))
                .map(|chan_idx| (cat_idx, chan_idx))
        })
    }

    /// Abre un post de foro como canal propio y devuelve dónde quedó. Si ya
    /// estaba abierto (se conservan sus mensajes) devuelve el mismo lugar.
    /// Los posts abiertos viven en una categoría al final, que la barra
    /// lateral no dibuja porque ninguno de sus canales es visible ahí.
    pub fn open_thread(
        &mut self,
        thread_id: &str,
        title: &str,
        forum_id: Option<String>,
    ) -> (usize, usize) {
        if let Some(position) = self.channel_position_by_id(thread_id) {
            return position;
        }
        let mut channel = Channel::from_discord(thread_id.to_string(), title, 11);
        channel.is_thread = true;
        channel.parent_id = forum_id;
        channel.access.can_view = false;
        let category = match self
            .categories
            .iter()
            .position(|c| c.channels.first().is_some_and(|ch| ch.is_thread))
        {
            Some(index) => index,
            None => {
                self.categories.push(ChannelCategory { name: String::new(), channels: Vec::new() });
                self.categories.len() - 1
            }
        };
        self.categories[category].channels.push(channel);
        (category, self.categories[category].channels.len() - 1)
    }

    /// Aplica una página de posts de un foro (`AppEvent::ForumPosts`).
    pub fn apply_forum_page(
        &mut self,
        forum_id: &str,
        offset: u32,
        by_creation: bool,
        page: crate::discord::models::ForumPage,
    ) {
        let posts: Vec<ForumPost> = {
            let resolver = NameResolver::new(&self.member_info, &self.roles);
            page.threads
                .iter()
                .map(|thread| {
                    let first = page
                        .first_messages
                        .iter()
                        .find(|m| m.id == thread.id || m.channel_id == thread.id);
                    ForumPost::from_thread(thread, first, &resolver)
                })
                .collect()
        };
        let received = page.threads.len() as u32;
        let Some(channel) = self.channel_by_id_mut(forum_id) else { return };
        let forum = &mut channel.forum;
        // Respuesta de un pedido con otro orden: ya no vale.
        if forum.by_creation != by_creation {
            return;
        }
        forum.loading = false;
        forum.loaded = true;
        forum.has_more = page.has_more;
        forum.next_offset = offset + received;
        if offset == 0 {
            forum.posts = posts;
        } else {
            for post in posts {
                if !forum.posts.iter().any(|p| p.id == post.id) {
                    forum.posts.push(post);
                }
            }
        }
        // Los fijados van arriba (el orden estable conserva el resto).
        forum.posts.sort_by_key(|p| !p.pinned);
    }

    /// Pone al día la tarjeta "Hilo de respuestas" debajo del mensaje que
    /// originó `thread` (nombre y cantidad de mensajes). Si el mensaje no
    /// tenía tarjeta todavía (el hilo se acaba de crear) se la agrega. No
    /// hace nada si el mensaje no está cargado.
    pub fn refresh_thread_card(&mut self, thread: &crate::discord::models::ThreadChannel) {
        let Some(parent_id) = thread.parent_id.clone() else { return };
        let Some(parent) = self.channel_by_id_mut(&parent_id) else { return };
        let Some(message) = parent.messages.iter_mut().find(|m| m.id == thread.id) else { return };
        match message.thread.as_mut() {
            Some(card) => {
                if let Some(name) = thread.name.as_ref() {
                    card.name = name.clone();
                }
                if let Some(count) = thread.message_count {
                    card.message_count = count;
                }
                if thread.last_message_id.is_some() {
                    card.last_message_id = thread.last_message_id.clone();
                }
            }
            None => message.thread = Some(ThreadCard::from_thread(thread)),
        }
    }

    /// Llegó (o se mandó) un mensaje dentro del hilo `thread_id`: sube el
    /// contador de la tarjeta que está debajo del mensaje original.
    pub fn note_thread_message(&mut self, thread_id: &str, message_id: &str) {
        let Some(parent_id) = self.channel_by_id_mut(thread_id).and_then(|c| c.parent_id.clone()) else {
            return;
        };
        let Some(parent) = self.channel_by_id_mut(&parent_id) else { return };
        if let Some(card) = parent
            .messages
            .iter_mut()
            .find(|m| m.id == thread_id)
            .and_then(|m| m.thread.as_mut())
        {
            card.message_count += 1;
            card.last_message_id = Some(message_id.to_string());
        }
    }

    /// `THREAD_CREATE` / `THREAD_UPDATE`: pone al día la tarjeta del post en
    /// su foro (o la agrega arriba si es nuevo) y el título del post si está
    /// abierto. Los hilos de canales que no son foros se ignoran.
    pub fn upsert_thread(&mut self, thread: &crate::discord::models::ThreadChannel) {
        self.refresh_thread_card(thread);
        let Some(forum_id) = thread.parent_id.clone() else { return };
        let fresh = {
            let resolver = NameResolver::new(&self.member_info, &self.roles);
            ForumPost::from_thread(thread, None, &resolver)
        };
        if let Some(channel) = self.channel_by_id_mut(&forum_id) {
            if channel.is_forum {
                let forum = &mut channel.forum;
                if let Some(existing) = forum.posts.iter_mut().find(|p| p.id == thread.id) {
                    // El mensaje inicial (autor, vista previa, imagen) no
                    // viene en estos eventos: se conserva lo que había.
                    existing.title = fresh.title.clone();
                    existing.tag_ids = fresh.tag_ids.clone();
                    existing.pinned = fresh.pinned;
                    if thread.message_count.is_some() {
                        existing.replies = fresh.replies;
                    }
                    existing.last_activity_ms = existing.last_activity_ms.max(fresh.last_activity_ms);
                } else if forum.loaded {
                    forum.posts.insert(0, fresh);
                }
                forum.posts.sort_by_key(|p| !p.pinned);
            }
        }
        if let (Some(name), Some(open)) = (thread.name.as_ref(), self.channel_by_id_mut(&thread.id)) {
            if open.is_thread {
                open.name = name.clone();
            }
        }
    }

    /// `THREAD_DELETE`: saca la tarjeta del post de su foro.
    pub fn remove_thread(&mut self, forum_id: &str, thread_id: &str) {
        if let Some(channel) = self.channel_by_id_mut(forum_id) {
            channel.forum.posts.retain(|p| p.id != thread_id);
        }
    }

    /// `GUILD_ROLE_CREATE` / `GUILD_ROLE_UPDATE`.
    pub fn upsert_role(&mut self, role: crate::discord::models::Role) {
        let mut roles = self.roles.clone();
        match roles.iter().position(|r| r.id == role.id) {
            Some(index) => roles[index] = role,
            None => roles.push(role),
        }
        self.apply_roles(roles);
    }

    /// `GUILD_ROLE_DELETE`.
    pub fn remove_role(&mut self, role_id: &str) {
        let mut roles = self.roles.clone();
        roles.retain(|r| r.id != role_id);
        self.apply_roles(roles);
        // El rol borrado ya no cuenta como propio.
        if let Some(mine) = self.access_ctx.my_roles.as_mut() {
            mine.retain(|id| id != role_id);
        }
        self.recompute_access();
    }

    /// Reemplaza TODOS los conectados a voz de este server por los de
    /// `states` (una foto completa: `READY`, `READY_SUPPLEMENTAL` o
    /// `GUILD_CREATE`), sin duplicar aunque la misma foto llegue por dos
    /// caminos. Como estas fotos casi nunca traen `member`, a quien ya
    /// teníamos con nombre se le conserva en vez de volver a "Usuario
    /// desconocido". Los estados de canales que todavía no conocemos quedan
    /// pendientes hasta que lleguen los canales.
    pub fn apply_voice_snapshot(&mut self, states: &[crate::discord::models::VoiceState]) {
        let mut known: std::collections::HashMap<String, VoiceOccupant> =
            std::collections::HashMap::new();
        for category in &mut self.categories {
            for channel in &mut category.channels {
                for occupant in channel.voice_members.drain(..) {
                    if occupant.known {
                        known.insert(occupant.user_id.clone(), occupant);
                    }
                }
            }
        }
        self.pending_voice_states.clear();

        for state in states {
            let Some(channel_id) = state.channel_id.as_deref() else { continue };
            let mut occupant = self.resolved_voice_occupant(state);
            if !occupant.known {
                if let Some(previous) = known.get(&occupant.user_id) {
                    // Solo tomamos nombre/avatar/`known` de lo que ya
                    // teníamos: `self_mute`/`self_deaf` tienen que quedar
                    // los de ESTE `state`, que es el dato fresco — si acá
                    // pisáramos con el occupant anterior entero, alguien
                    // que se silencia justo antes de una foto completa
                    // (`READY_SUPPLEMENTAL`/`GUILD_CREATE`) se vería sin
                    // silenciar hasta el próximo cambio de canal.
                    let (self_mute, self_deaf, streaming) =
                        (occupant.self_mute, occupant.self_deaf, occupant.streaming);
                    occupant = previous.clone();
                    occupant.self_mute = self_mute;
                    occupant.self_deaf = self_deaf;
                    occupant.streaming = streaming;
                }
            }
            if let Some(channel) = self.channel_by_id_mut(channel_id) {
                channel.voice_members.push(occupant);
            } else {
                self.pending_voice_states.push(state.clone());
            }
        }
    }

    /// Acomoda los estados de voz que estaban esperando a que se conocieran
    /// los canales.
    fn place_pending_voice_states(&mut self) {
        for state in std::mem::take(&mut self.pending_voice_states) {
            let Some(channel_id) = state.channel_id.clone() else { continue };
            let occupant = self.resolved_voice_occupant(&state);
            let placed = if let Some(channel) = self.channel_by_id_mut(&channel_id) {
                channel.voice_members.push(occupant);
                true
            } else {
                false
            };
            if !placed {
                self.pending_voice_states.push(state);
            }
        }
    }

    /// Arma el conectado a voz de un `VoiceState` usando todo lo que ya
    /// sabemos del server: los estados de voz del Gateway suelen traer solo
    /// `user_id`, pero si esa persona ya pasó por la lista de miembros (o
    /// escribió en el chat) tenemos su apodo, su nombre y su avatar, y no
    /// hace falta ir a buscarla por REST.
    pub fn resolved_voice_occupant(
        &self,
        state: &crate::discord::models::VoiceState,
    ) -> VoiceOccupant {
        let mut occupant = VoiceOccupant::from_voice_state(state);
        let nick = self
            .member_info
            .get(&state.user_id)
            .and_then(|info| info.nick.as_deref());
        if let Some(user) = self.known_users.get(&state.user_id) {
            if !occupant.known {
                occupant.avatar_url = user.avatar_url.clone();
                occupant.name = user.name.clone();
                occupant.known = true;
            }
        }
        // El apodo del server manda sobre el nombre global. Sin datos del
        // usuario, un apodo solo no alcanza para el avatar: `known` queda en
        // `false` y el pedido REST de respaldo completa el resto.
        if let Some(nick) = nick {
            occupant.name = nick.to_string();
        }
        occupant
    }

    /// Aprende de un miembro TODO lo que sirve para mostrarlo bien: guarda su
    /// apodo y roles (nombres y colores del chat), su nombre y avatar
    /// (conectados a voz que llegan solo como `user_id`) y actualiza ya mismo
    /// a quien de él esté en un canal de voz. Devuelve `true` si cambiaron
    /// apodo o roles, o sea si conviene repintar los mensajes.
    pub fn learn_member(
        &mut self,
        user: &crate::discord::models::User,
        nick: Option<&str>,
        roles: &[String],
    ) -> bool {
        if user_is_named(user) {
            self.known_users.insert(
                user.id.clone(),
                KnownUser { name: user.display_name().to_string(), avatar_url: user.avatar_url() },
            );
            self.apply_member_to_voice(user, nick);
        }
        self.cache_member(&user.id, nick, roles)
    }

    /// Ids de los conectados a voz cuyo nombre todavía es el placeholder
    /// (`VoiceOccupant::known == false`), o sea a los que hay que ir a
    /// buscar por REST.
    pub fn unknown_voice_user_ids(&self) -> std::collections::HashSet<String> {
        self.categories
            .iter()
            .flat_map(|category| category.channels.iter())
            .flat_map(|channel| channel.voice_members.iter())
            .filter(|occupant| !occupant.known)
            .map(|occupant| occupant.user_id.clone())
            .collect()
    }

    /// Actualiza nombre y avatar de un conectado a voz con los datos reales
    /// del miembro (llegan por `GUILD_MEMBERS_CHUNK`/`GUILD_MEMBER_UPDATE`):
    /// el apodo del server si tiene, si no su nombre global.
    pub fn apply_member_to_voice(
        &mut self,
        user: &crate::discord::models::User,
        nick: Option<&str>,
    ) {
        if !user_is_named(user) {
            return;
        }
        let name = nick
            .map(str::trim)
            .filter(|nick| !nick.is_empty())
            .unwrap_or_else(|| user.display_name())
            .to_string();
        for category in &mut self.categories {
            for channel in &mut category.channels {
                for occupant in &mut channel.voice_members {
                    if occupant.user_id == user.id {
                        occupant.name = name.clone();
                        occupant.avatar_url = user.avatar_url();
                        occupant.known = true;
                    }
                }
            }
        }
    }
}

fn member(name: &str, color: Color32) -> Member {
    Member {
        name: name.to_string(),
        avatar_color: color,
        avatar_url: None,
        user_id: String::new(),
        status: Status::Online,
        name_color: None,
        subtitle: None,
    }
}

impl Server {
    /// Server real construido a partir de un `Guild` de `READY`. Arranca
    /// sin canales/miembros: esos se piden por REST recién cuando el
    /// usuario lo abre (`App::open_server`), para no pedir los ~cientos de
    /// canales/miembros de *todos* los servers en el login.
    pub fn from_guild(guild: &crate::discord::models::Guild) -> Self {
        let name = guild.display_name();
        let mut server = Self {
            name: name.to_string(),
            icon_initial: name
                .chars()
                .find(|c| c.is_alphanumeric())
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_string()),
            icon_color: color_from_id(&guild.id),
            icon_url: guild.icon_url(),
            guild_id: guild.id.clone(),
            topic: String::new(),
            categories: Vec::new(),
            member_groups: Vec::new(),
            online_count: 0,
            total_count: 0,
            roles: guild.roles.clone(),
            member_lists: std::collections::HashMap::new(),
            active_member_list: String::new(),
            channel_member_list: std::collections::HashMap::new(),
            pending_list_channel: None,
            member_info: std::collections::HashMap::new(),
            requested_members: std::collections::HashSet::new(),
            pending_voice_states: Vec::new(),
            known_users: std::collections::HashMap::new(),
            raw_channels: Vec::new(),
            access_ctx: crate::lib::permissions::AccessContext {
                owner_id: guild.owner(),
                ..Default::default()
            },
            // Discord no manda emojis "vacíos" en la práctica, pero el
            // `name` viene opcional en el shape general de `Emoji`
            // (reutilizado acá para el mismo campo) así que igual nos
            // cubrimos filtrando cualquiera sin nombre en vez de
            // arriesgar un ":: :id" roto en el picker.
            custom_emojis: guild
                .emojis
                .iter()
                .filter_map(|e| {
                    let name = e.name.clone().filter(|n| !n.is_empty())?;
                    Some(CustomEmoji { id: e.id.clone(), name, animated: e.animated })
                })
                .collect(),
            custom_stickers: guild
                .stickers
                .iter()
                .filter(|s| !s.id.is_empty() && s.format_type != 3)
                .cloned()
                .collect(),
        };

        // El `READY` de una cuenta de usuario ya trae el guild completo
        // (canales + estados de voz) — nos ahorramos el pedido por REST
        // que `App::open_server` haría la primera vez que se abre este
        // server (esa función ya se fija si `categories` está vacío antes
        // de pedir nada, así que con esto alcanza para no repetir el
        // viaje a la API). Un guild "unavailable" llega sin nada de esto
        // (`channels`/`voice_states` vacíos): para esos sigue haciendo
        // falta el REST/`GUILD_CREATE` de siempre.
        if !guild.channels.is_empty() {
            server.apply_channels(guild.channels.clone());
        }
        server.apply_voice_snapshot(&guild.voice_states);

        server
    }
}

impl PrivateChannel {
    pub fn from_dm(dm: &crate::discord::models::PrivateChannel) -> Self {
        // `recipients` puede venir vacío en casos raros (canal de "notas
        // personales", datos parciales); indexar `[0]` directo ahí
        // reventaba toda la carga de `READY`. Con `.first()` nos
        // quedamos con placeholders en vez de tirar abajo la conexión.
        let recipient = dm.recipients.first();
        Self {
            channel_type: dm.channel_type,
            safety_warnings: dm.safety_warnings.clone(),
            recipients: dm.recipients.clone(),
            recipient_flags: dm.recipient_flags,
            last_message_id: dm.last_message_id.clone(),
            is_spam: dm.is_spam,
            is_message_request_timestamp: dm.is_message_request_timestamp.clone(),
            is_message_request: dm.is_message_request,
            id: dm.id.clone(),
            flags: dm.flags,
            username: recipient.map(|r| r.username.clone()).unwrap_or_default(),
            avatar: recipient.and_then(|r| r.avatar.clone()),
            avatar_url: recipient.and_then(crate::discord::models::User::avatar_url),
        }
    }
}

/// Servidores de demostración que aparecen en el rail izquierdo.
/// Nombres y contenido inventados; el primero está más detallado para
/// mostrar la estructura completa (categorías, canales, roles).
pub fn demo_servers() -> Vec<Server> {
    vec![
        Server {
            name: "Play Hosting".to_string(),
            icon_initial: "A".to_string(),
            icon_color: Color32::from_rgb(90, 170, 210),
            icon_url: None,
            guild_id: String::new(),
            topic: "USE community-support PARA SOPORTE".to_string(),
            categories: vec![
                ChannelCategory {
                    name: "INFO".to_string(),
                    channels: vec![Channel::new("vps-and-dedicated", vec![])],
                },
                ChannelCategory {
                    name: "AYUDA".to_string(),
                    channels: vec![
                        Channel::new("help", vec![]),
                        Channel::new("faq", vec![]),
                        Channel::new("booster-support", vec![]),
                        Channel::new(
                            "open-a-ticket",
                            vec![
                                ChatMessage::them(
                                    "Cobbledd",
                                    "18:48",
                                    "how do i get help",
                                    Color32::from_rgb(90, 130, 210),
                                ),
                                ChatMessage::them(
                                    "Qparz",
                                    "18:58",
                                    "serv not starting yo, like not downloading",
                                    Color32::from_rgb(150, 90, 210),
                                ),
                                ChatMessage::them(
                                    "Raptor",
                                    "19:49",
                                    "same",
                                    Color32::from_rgb(120, 120, 120),
                                ),
                                ChatMessage::them(
                                    "MsPengu",
                                    "19:58",
                                    "Heyyy. Solo quería decir que play.hosting es increíble y no puedo creer que sea gratis. Gran trabajo equipo 🙂",
                                    Color32::from_rgb(210, 150, 190),
                                )
                                .with_reactions(vec![("❤️", 5, false), ("🎉", 2, true)]),
                                ChatMessage::them(
                                    "Cobalt",
                                    "19:59",
                                    "⚠ Este no es el lugar para pedir soporte. Por favor lee #help",
                                    Color32::from_rgb(70, 90, 200),
                                ),
                            ],
                        ),
                        Channel::new("community-support", vec![]),
                    ],
                },
                ChannelCategory {
                    name: "COMUNIDAD".to_string(),
                    channels: vec![
                        Channel::new(
                            "general_not_support",
                            vec![
                                ChatMessage::them(
                                    "zribe",
                                    "19:20",
                                    "El ad revenue no puede alcanzar para cubrir 2 junipers",
                                    Color32::from_rgb(120, 170, 210),
                                ),
                                ChatMessage::them(
                                    "zribe",
                                    "19:21",
                                    "Me pregunto cuánto cubre en verdad el ad revenue",
                                    Color32::from_rgb(120, 170, 210),
                                )
                                .with_reactions(vec![("👍", 1, false)]),
                                ChatMessage::own(
                                    AIDEN,
                                    "19:25",
                                    "buena pregunta la verdad",
                                    AIDEN_COLOR,
                                ),
                            ],
                        ),
                        Channel::new("feedback", vec![]),
                        Channel::new("servers", vec![]),
                        Channel::new("bot-commands", vec![]),
                        Channel::new("counting", vec![]),
                        Channel::new("system-messages", vec![]),
                    ],
                },
            ],
            member_groups: vec![
                MemberGroup {
                    name: "¿Necesitás ayuda? — 2".to_string(),
                    members: vec![
                        member("Ticketbot", Color32::from_rgb(200, 90, 90)),
                        member("Play Hosting", Color32::from_rgb(90, 170, 210)),
                    ],
                },
                MemberGroup {
                    name: "Equipo — 4".to_string(),
                    members: vec![
                        member("Nathan", Color32::from_rgb(150, 90, 210)),
                        member("Robert", Color32::from_rgb(90, 150, 210)),
                        member("Santio", Color32::from_rgb(90, 200, 140)),
                        member("Tubbo", Color32::from_rgb(210, 160, 60)),
                    ],
                },
                MemberGroup {
                    name: "Soporte de tickets — 1".to_string(),
                    members: vec![member("Wallu", Color32::from_rgb(120, 120, 200))],
                },
                MemberGroup {
                    name: "Creator Program — 5".to_string(),
                    members: vec![
                        member("cayman", Color32::from_rgb(210, 170, 60)),
                        member("dandelionry", Color32::from_rgb(200, 120, 60)),
                        member("elskie", Color32::from_rgb(150, 90, 160)),
                        member("Knapp", Color32::from_rgb(90, 150, 90)),
                        member("Roscumber", Color32::from_rgb(90, 90, 200)),
                    ],
                },
            ],
            online_count: 46,
            total_count: 312,
            roles: Vec::new(),
            member_lists: std::collections::HashMap::new(),
            active_member_list: String::new(),
            channel_member_list: std::collections::HashMap::new(),
            pending_list_channel: None,
            custom_emojis: Vec::new(),
            custom_stickers: Vec::new(),
            member_info: std::collections::HashMap::new(),
            requested_members: std::collections::HashSet::new(),
            pending_voice_states: Vec::new(),
            known_users: std::collections::HashMap::new(),
            access_ctx: Default::default(),
            raw_channels: Vec::new(),
        },
        demo_small_server("B", "MaPaChEtE Club", Color32::from_rgb(200, 90, 150)),
        demo_small_server("C", "Estudio Indie", Color32::from_rgb(120, 180, 90)),
        demo_small_server("D", "Fan Server", Color32::from_rgb(210, 160, 60)),
        demo_small_server("E", "Amigos IRL", Color32::from_rgb(150, 110, 210)),
    ]
}

fn demo_small_server(initial: &str, name: &str, color: Color32) -> Server {
    Server {
        name: name.to_string(),
        icon_initial: initial.to_string(),
        icon_color: color,
        icon_url: None,
        guild_id: String::new(),
        topic: "Sin descripción".to_string(),
        categories: vec![ChannelCategory {
            name: "GENERAL".to_string(),
            channels: vec![
                Channel::new(
                    "general",
                    vec![ChatMessage::them(
                        name,
                        "hoy",
                        "¡Bienvenido al servidor!",
                        color,
                    )],
                ),
                Channel::new("random", vec![]),
            ],
        }],
        member_groups: vec![MemberGroup {
            name: format!("Miembros — 3"),
            members: vec![
                member("Miembro Uno", color),
                member("Miembro Dos", Color32::from_rgb(90, 150, 210)),
                member("Miembro Tres", Color32::from_rgb(200, 90, 90)),
            ],
        }],
        online_count: 3,
        total_count: 12,
        roles: Vec::new(),
        member_lists: std::collections::HashMap::new(),
        active_member_list: String::new(),
        channel_member_list: std::collections::HashMap::new(),
        pending_list_channel: None,
        custom_emojis: Vec::new(),
        custom_stickers: Vec::new(),
        member_info: std::collections::HashMap::new(),
        requested_members: std::collections::HashSet::new(),
        pending_voice_states: Vec::new(),
        known_users: std::collections::HashMap::new(),
        access_ctx: Default::default(),
            raw_channels: Vec::new(),
    }
}

// ---------------------------------------------------------------------
// Presencia y lista de miembros
// ---------------------------------------------------------------------

impl Status {
    /// El `status` de Discord (`online`, `idle`, `dnd`, `offline`,
    /// `invisible`). Cualquier otra cosa se toma como desconectado.
    pub fn from_gateway(status: &str) -> Self {
        match status {
            "online" => Status::Online,
            "idle" => Status::Idle,
            "dnd" => Status::Dnd,
            _ => Status::Offline,
        }
    }
}

impl Friend {
    /// Aplica un `PRESENCE_UPDATE` (o la presencia inicial de
    /// `READY_SUPPLEMENTAL`): el estado y el texto bajo el nombre.
    pub fn apply_presence(&mut self, status: &str, subtitle: Option<String>) {
        self.status = Status::from_gateway(status);
        self.subtitle = subtitle;
    }
}

/// `0xRRGGBB` → color de egui. `0` significa "sin color".
fn role_color(color: u32) -> Option<Color32> {
    if color == 0 {
        return None;
    }
    Some(Color32::from_rgb(
        ((color >> 16) & 0xFF) as u8,
        ((color >> 8) & 0xFF) as u8,
        (color & 0xFF) as u8,
    ))
}

/// Color del rol MÁS ALTO (mayor `position`) entre los roles con color de un
/// miembro — la regla de Discord para pintar un nombre. `None` si ninguno de
/// sus roles tiene color (o si no conocemos los roles del server).
fn top_role_color(
    roles: &[crate::discord::models::Role],
    member_roles: &[String],
) -> Option<Color32> {
    member_roles
        .iter()
        .filter_map(|id| roles.iter().find(|role| &role.id == id))
        .filter(|role| role.color != 0)
        .max_by_key(|role| role.position)
        .and_then(|role| role_color(role.color))
}

/// Lo que sabemos de un miembro DENTRO de un server concreto: su apodo (si
/// puso uno) y los ids de sus roles.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemberInfo {
    pub nick: Option<String>,
    pub roles: Vec<String>,
}

/// Nombre global y avatar de un usuario ya visto en este server (por la lista
/// de miembros, un `GUILD_MEMBERS_CHUNK` o un mensaje). Sirve para poner
/// nombre a alguien que solo llega como `user_id` (por ejemplo en un estado de
/// voz) sin tener que ir a buscarlo por REST.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownUser {
    pub name: String,
    pub avatar_url: Option<String>,
}

/// ¿El objeto de usuario trae un nombre real? (Los usuarios "parciales" de
/// Discord pueden venir sin `username`.)
fn user_is_named(user: &crate::discord::models::User) -> bool {
    !user.id.is_empty()
        && (!user.username.is_empty()
            || user.global_name.as_deref().is_some_and(|name| !name.is_empty()))
}

/// Resuelve el nombre a mostrar y el color de un autor a partir de lo que un
/// [`Server`] tiene cacheado. Toma prestados solo los dos campos que usa (no
/// el `Server` entero) para poder tenerlo a mano mientras se recorren los
/// canales del mismo server con `&mut`.
#[derive(Clone, Copy)]
pub struct NameResolver<'a> {
    members: &'a std::collections::HashMap<String, MemberInfo>,
    roles: &'a [crate::discord::models::Role],
}

impl<'a> NameResolver<'a> {
    pub fn new(
        members: &'a std::collections::HashMap<String, MemberInfo>,
        roles: &'a [crate::discord::models::Role],
    ) -> Self {
        Self { members, roles }
    }

    /// `(nombre a mostrar, color)` de `user_id`. Si todavía no conocemos al
    /// miembro (o no tiene apodo) el nombre es `base_name`, y sin roles
    /// conocidos no hay color.
    pub fn identity(&self, user_id: &str, base_name: &str) -> (String, Option<Color32>) {
        let Some(info) = self.members.get(user_id) else {
            return (base_name.to_string(), None);
        };
        let name = info
            .nick
            .as_deref()
            .map(str::trim)
            .filter(|nick| !nick.is_empty())
            .unwrap_or(base_name)
            .to_string();
        (name, top_role_color(self.roles, &info.roles))
    }
}

/// Una lista de miembros de Discord: la lista plana (encabezados de grupo y
/// miembros mezclados, ya ordenada por el servidor) y sus contadores.
#[derive(Clone, Default)]
pub struct MemberListState {
    /// `None` = casilla que Discord todavía no mandó (o que invalidó).
    pub items: Vec<Option<crate::discord::models::MemberListItem>>,
    pub member_count: usize,
    pub online_count: usize,
}

impl MemberListState {
    /// Aplica las operaciones de un `GUILD_MEMBER_LIST_UPDATE` sobre la
    /// lista plana. Discord no manda la lista entera: manda operaciones
    /// sobre índices.
    fn apply_ops(&mut self, ops: &[crate::discord::models::MemberListOp]) {
        for op in ops {
            match op.op.as_str() {
                // Reemplaza EXACTAMENTE el rango: si la lista nueva es más
                // corta que la vieja no quedan restos.
                "SYNC" => {
                    let Some((start, end)) = op.range else { continue };
                    if self.items.len() < start {
                        self.items.resize(start, None);
                    }
                    let stop = end.saturating_add(1).min(self.items.len()).max(start);
                    drop(self.items.splice(start..stop, op.items.iter().cloned().map(Some)));
                }
                "INSERT" => {
                    if let (Some(index), Some(item)) = (op.index, op.item.as_ref()) {
                        if self.items.len() < index {
                            self.items.resize(index, None);
                        }
                        self.items.insert(index, Some(item.clone()));
                    }
                }
                "UPDATE" => {
                    if let (Some(index), Some(item)) = (op.index, op.item.as_ref()) {
                        if self.items.len() <= index {
                            self.items.resize(index + 1, None);
                        }
                        self.items[index] = Some(item.clone());
                    }
                }
                "DELETE" => {
                    if let Some(index) = op.index {
                        if index < self.items.len() {
                            self.items.remove(index);
                        }
                    }
                }
                "INVALIDATE" => {
                    if let Some((start, end)) = op.range {
                        let count = end.saturating_add(1).saturating_sub(start);
                        for slot in self.items.iter_mut().skip(start).take(count) {
                            *slot = None;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

impl Server {
    /// Guarda los roles y rearma los grupos (los nombres de los grupos por
    /// rol salen de acá).
    pub fn apply_roles(&mut self, roles: Vec<crate::discord::models::Role>) {
        self.roles = roles;
        // Los permisos de los roles cambian qué canales se ven.
        self.recompute_access();
        self.rebuild_member_groups();
        // Los colores de los nombres del chat salen de los roles.
        self.refresh_author_names();
    }

    /// ¿Este server tiene un canal con ese id?
    pub fn has_channel(&self, channel_id: &str) -> bool {
        self.categories
            .iter()
            .any(|cat| cat.channels.iter().any(|ch| ch.channel_id.as_deref() == Some(channel_id)))
    }

    /// Guarda apodo y roles de un miembro. Devuelve `true` si algo cambió
    /// respecto de lo que ya había (para saber si vale la pena repintar los
    /// nombres del chat).
    pub fn cache_member(&mut self, user_id: &str, nick: Option<&str>, roles: &[String]) -> bool {
        if user_id.is_empty() {
            return false;
        }
        let info = MemberInfo {
            nick: nick
                .map(str::trim)
                .filter(|nick| !nick.is_empty())
                .map(str::to_owned),
            roles: roles.to_vec(),
        };
        if self.member_info.get(user_id) == Some(&info) {
            return false;
        }
        self.member_info.insert(user_id.to_owned(), info);
        true
    }

    /// De `user_ids`, los que todavía no conocemos y no se pidieron antes.
    /// Los devuelve y los deja marcados como pedidos.
    pub fn take_unresolved_members<'a>(
        &mut self,
        user_ids: impl IntoIterator<Item = &'a str>,
    ) -> Vec<String> {
        let mut missing = Vec::new();
        for id in user_ids {
            if id.is_empty() || self.member_info.contains_key(id) {
                continue;
            }
            if self.requested_members.insert(id.to_owned()) {
                missing.push(id.to_owned());
            }
        }
        missing
    }

    /// Vuelve a resolver el nombre y el color de todos los mensajes ya
    /// cargados de este server con lo que hay ahora en `member_info`/`roles`.
    pub fn refresh_author_names(&mut self) {
        let names = NameResolver::new(&self.member_info, &self.roles);
        for category in &mut self.categories {
            for channel in &mut category.channels {
                for message in &mut channel.messages {
                    message.reresolve_names(names);
                }
            }
        }
    }

    /// ¿Ya sabemos que este canal comparte la lista que se está mostrando?
    /// Si sí, no hace falta volver a pedirla: la suscripción vigente ya la
    /// mantiene al día.
    pub fn channel_shares_active_list(&self, channel_id: &str) -> bool {
        !self.active_member_list.is_empty()
            && self
                .member_lists
                .get(&self.active_member_list)
                .is_some_and(|list| !list.items.is_empty())
            && self
                .channel_member_list
                .get(channel_id)
                .is_some_and(|list_id| *list_id == self.active_member_list)
    }

    /// Aplica un `GUILD_MEMBER_LIST_UPDATE`.
    ///
    /// El orden por roles no se calcula acá: es el que manda Discord. Lo
    /// importante es NO descartar nunca una lista por haber llegado una
    /// novedad de otra: tras cambiar de canal siguen llegando novedades de
    /// la lista anterior, y si pisaran a la que se ve, la lista quedaría
    /// vacía sin que nada la vuelva a llenar.
    pub fn apply_member_list_update(&mut self, update: &crate::discord::models::MemberListUpdate) {
        let list_id = if update.id.is_empty() {
            "everyone".to_string()
        } else {
            update.id.clone()
        };

        // Un `SYNC` es la respuesta a una suscripción: esa lista pasa a ser
        // la que se muestra, y es la del canal que se acaba de pedir.
        if update.ops.iter().any(|op| op.op == "SYNC") {
            self.active_member_list = list_id.clone();
            if let Some(channel_id) = self.pending_list_channel.take() {
                self.channel_member_list.insert(channel_id, list_id.clone());
            }
        }

        // La lista de miembros ya trae apodo y roles de cada uno: sirve para
        // los nombres del chat sin pedir nada más.
        let mut names_changed = false;
        for item in update.ops.iter().flat_map(|op| op.items.iter().chain(op.item.iter())) {
            if let Some(member) = &item.member {
                names_changed |=
                    self.learn_member(&member.user, member.nick.as_deref(), &member.roles);
            }
        }
        if names_changed {
            self.refresh_author_names();
        }

        let list = self.member_lists.entry(list_id.clone()).or_default();
        list.apply_ops(&update.ops);
        if update.member_count > 0 {
            list.member_count = update.member_count as usize;
        }
        list.online_count = update.online_count as usize;

        if self.active_member_list.is_empty() {
            self.active_member_list = list_id.clone();
        }
        // Las novedades de una lista que no se ve se guardan, pero no
        // tocan la pantalla.
        if self.active_member_list == list_id {
            self.rebuild_member_groups();
        }
    }

    /// Actualiza el estado de un miembro (`PRESENCE_UPDATE` con
    /// `guild_id`) en todas las listas donde aparezca. Ojo: NO lo mueve de
    /// grupo — de eso se encarga Discord con sus propias operaciones.
    pub fn apply_presence(
        &mut self,
        user_id: &str,
        status: &str,
        activities: &[crate::discord::models::PresenceActivity],
    ) {
        let mut changed = false;
        for list in self.member_lists.values_mut() {
            for item in list.items.iter_mut().flatten() {
                if let Some(member) = item.member.as_mut() {
                    if member.user.id == user_id {
                        member.presence = Some(crate::discord::models::Presence {
                            status: status.to_owned(),
                            activities: activities.to_vec(),
                        });
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.rebuild_member_groups();
        }
    }

    /// Vuelve a armar `member_groups` (lo que dibuja la UI) desde la lista
    /// activa: cada encabezado abre un grupo y los miembros que le siguen
    /// caen adentro, en el orden en que vinieron.
    pub fn rebuild_member_groups(&mut self) {
        let Some(list) = self.member_lists.get(&self.active_member_list) else {
            return;
        };
        // Una lista todavía sin ninguna casilla no reemplaza a la que ya
        // está en pantalla: mejor lo anterior que un panel vacío.
        if list.items.is_empty() && !self.member_groups.is_empty() {
            return;
        }

        let mut groups: Vec<MemberGroup> = Vec::new();
        for item in list.items.iter().flatten() {
            if let Some(group) = &item.group {
                groups.push(MemberGroup {
                    name: self.member_group_title(group),
                    members: Vec::new(),
                });
            } else if let Some(member) = &item.member {
                // Un rango que arranca en medio de un grupo (o una lista a
                // medio cargar) puede traer miembros sin su encabezado.
                if groups.is_empty() {
                    groups.push(MemberGroup {
                        name: "Miembros".to_string(),
                        members: Vec::new(),
                    });
                }
                if let Some(group) = groups.last_mut() {
                    group.members.push(self.member_from_list(member));
                }
            }
        }
        let (member_count, online_count) = (list.member_count, list.online_count);
        if member_count > 0 {
            self.total_count = member_count;
        }
        self.online_count = online_count;
        self.member_groups = groups;
    }

    /// "Admins — 3", "En línea — 42", "Sin conexión — 270".
    fn member_group_title(&self, group: &crate::discord::models::MemberListGroup) -> String {
        let label = match group.id.as_str() {
            "online" => "En línea".to_string(),
            "offline" => "Sin conexión".to_string(),
            role_id => self
                .roles
                .iter()
                .find(|role| role.id == role_id)
                .map(|role| role.name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "Rol".to_string()),
        };
        format!("{label} — {}", group.count)
    }

    fn member_from_list(&self, member: &crate::discord::models::MemberListMember) -> Member {
        let name = member
            .nick
            .as_deref()
            .map(str::trim)
            .filter(|nick| !nick.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| member.user.display_name().to_string());
        // Como en Discord: el nombre se pinta con el color del rol MÁS ALTO
        // (mayor `position`) entre los que tienen color.
        let name_color = top_role_color(&self.roles, &member.roles);
        Member {
            name,
            avatar_color: color_from_id(&member.user.id),
            avatar_url: member.user.avatar_url(),
            user_id: member.user.id.clone(),
            status: member
                .presence
                .as_ref()
                .map(|presence| Status::from_gateway(&presence.status))
                .unwrap_or(Status::Offline),
            name_color,
            subtitle: member
                .presence
                .as_ref()
                .and_then(crate::discord::models::Presence::subtitle),
        }
    }
}

#[cfg(test)]
mod member_list_tests {
    use super::*;
    use crate::discord::models::{MemberListUpdate, Role};
    use serde_json::json;

    fn update(value: serde_json::Value) -> MemberListUpdate {
        serde_json::from_value(value).expect("update válido")
    }

    fn member(id: &str) -> serde_json::Value {
        json!({ "member": { "user": { "id": id, "username": id }, "roles": [] } })
    }

    fn server_with_admin_role() -> Server {
        let mut server = demo_small_server("A", "x", Color32::WHITE);
        server.roles = vec![Role {
            id: "r1".into(),
            name: "Admins".into(),
            color: 0xFF0000,
            position: 5,
            hoist: true,
            permissions: None,
            mentionable: false,
        }];
        server
    }

    #[test]
    fn groups_come_ordered_by_role_with_status_and_role_color() {
        let mut server = server_with_admin_role();
        server.apply_member_list_update(&update(json!({
            "guild_id": "1", "id": "everyone", "member_count": 4, "online_count": 3,
            "ops": [{ "op": "SYNC", "range": [0, 99], "items": [
                { "group": { "id": "r1", "count": 2 } },
                { "member": { "user": { "id": "10", "username": "ana" }, "roles": ["r1"],
                              "presence": { "status": "online", "activities": [] } } },
                { "member": { "user": { "id": "11", "username": "beto" }, "roles": ["r1"],
                              "presence": { "status": "dnd", "activities": [] } } },
                { "group": { "id": "offline", "count": 1 } },
                { "member": { "user": { "id": "12", "username": "caro" }, "roles": [] } }
            ]}]
        })));

        assert_eq!(server.member_groups.len(), 2);
        assert_eq!(server.member_groups[0].name, "Admins — 2");
        assert_eq!(server.member_groups[1].name, "Sin conexión — 1");
        assert_eq!(server.total_count, 4);
        assert_eq!(server.online_count, 3);

        let admins = &server.member_groups[0].members;
        assert!(admins[0].status == Status::Online);
        assert!(admins[1].status == Status::Dnd);
        assert!(admins[0].name_color == Some(Color32::from_rgb(255, 0, 0)));
        // Sin rol con color: sin color propio, y sin presencia = desconectado.
        let offline = &server.member_groups[1].members[0];
        assert!(offline.name_color.is_none());
        assert!(offline.status == Status::Offline);
    }

    #[test]
    fn presence_update_changes_the_member_status() {
        let mut server = server_with_admin_role();
        server.apply_member_list_update(&update(json!({
            "guild_id": "1", "id": "everyone",
            "ops": [{ "op": "SYNC", "range": [0, 99], "items": [
                { "group": { "id": "online", "count": 1 } },
                { "member": { "user": { "id": "10", "username": "ana" }, "roles": [],
                              "presence": { "status": "online", "activities": [] } } }
            ]}]
        })));
        server.apply_presence("10", "idle", &[]);
        assert!(server.member_groups[0].members[0].status == Status::Idle);
    }

    #[test]
    fn a_shorter_resync_leaves_no_stale_entries() {
        let mut server = server_with_admin_role();
        server.apply_member_list_update(&update(json!({
            "guild_id": "1", "id": "everyone",
            "ops": [{ "op": "SYNC", "range": [0, 99], "items": [
                { "group": { "id": "online", "count": 3 } }, member("a"), member("b"), member("c")
            ]}]
        })));
        server.apply_member_list_update(&update(json!({
            "guild_id": "1", "id": "everyone",
            "ops": [{ "op": "SYNC", "range": [0, 99], "items": [
                { "group": { "id": "online", "count": 1 } }, member("z")
            ]}]
        })));
        assert_eq!(server.member_groups.len(), 1);
        assert_eq!(server.member_groups[0].members.len(), 1);
        assert_eq!(server.member_groups[0].members[0].name, "z");
    }

    /// El bug de "la lista queda vacía al cambiar de canal": tras el SYNC de
    /// la lista nueva llega una novedad tardía de la anterior.
    #[test]
    fn a_late_update_from_another_list_does_not_wipe_the_visible_one() {
        let mut server = server_with_admin_role();
        server.apply_member_list_update(&update(json!({
            "guild_id": "1", "id": "everyone",
            "ops": [{ "op": "SYNC", "range": [0, 99], "items": [
                { "group": { "id": "online", "count": 1 } }, member("a")
            ]}]
        })));
        // Cambio de canal: otra lista (otros permisos).
        server.apply_member_list_update(&update(json!({
            "guild_id": "1", "id": "abc123",
            "ops": [{ "op": "SYNC", "range": [0, 99], "items": [
                { "group": { "id": "online", "count": 1 } }, member("b")
            ]}]
        })));
        // Novedad tardía de la lista vieja (índices que en la nueva no existen).
        server.apply_member_list_update(&update(json!({
            "guild_id": "1", "id": "everyone",
            "ops": [{ "op": "UPDATE", "index": 1, "item": member("a2") },
                    { "op": "DELETE", "index": 0 }]
        })));

        assert_eq!(server.member_groups.len(), 1);
        assert_eq!(server.member_groups[0].members.len(), 1);
        assert_eq!(server.member_groups[0].members[0].name, "b");
    }

    #[test]
    fn learns_which_channels_share_the_active_list() {
        let mut server = server_with_admin_role();
        server.pending_list_channel = Some("chan-1".into());
        server.apply_member_list_update(&update(json!({
            "guild_id": "1", "id": "everyone",
            "ops": [{ "op": "SYNC", "range": [0, 99], "items": [
                { "group": { "id": "online", "count": 1 } }, member("a")
            ]}]
        })));
        assert!(server.pending_list_channel.is_none());
        assert!(server.channel_shares_active_list("chan-1"));
        assert!(!server.channel_shares_active_list("chan-2"), "un canal desconocido hay que pedirlo");
    }
}

#[cfg(test)]
mod chat_names_tests {
    use super::*;
    use crate::discord::models::{GatewayMessage, Role};
    use serde_json::json;

    fn role(id: &str, color: u32, position: i64) -> Role {
        Role { id: id.into(), name: id.into(), color, position, hoist: false, permissions: None, mentionable: false }
    }

    fn server() -> Server {
        let mut server = demo_small_server("A", "x", Color32::WHITE);
        server.roles = vec![
            role("low", 0x00FF00, 1),
            role("high", 0xFF0000, 9),
            // Está más arriba que todos, pero sin color: no cuenta.
            role("plain", 0, 20),
        ];
        server
    }

    fn message(author_id: &str) -> GatewayMessage {
        serde_json::from_value(json!({
            "id": "1", "channel_id": "2",
            "timestamp": "2026-01-01T00:00:00.000000+00:00",
            "content": "hola",
            "author": { "id": author_id, "username": "ana_user", "global_name": "Ana" }
        }))
        .expect("mensaje válido")
    }

    fn message_with(extra: serde_json::Value) -> GatewayMessage {
        let mut value = json!({
            "id": "1", "channel_id": "2",
            "timestamp": "2026-01-01T10:30:00.000000+00:00",
            "content": "",
            "author": { "id": "10", "username": "ana_user", "global_name": "Ana" }
        });
        value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(value).expect("mensaje válido")
    }

    #[test]
    fn forwarded_message_shows_its_snapshot_and_is_not_a_reply() {
        let msg = message_with(json!({
            "message_reference": { "type": 1, "message_id": "77", "channel_id": "88", "guild_id": "66" },
            "message_snapshots": [{ "message": {
                "content": "texto original",
                "timestamp": "2025-12-31T23:59:00.000000+00:00",
                "edited_timestamp": null,
                "attachments": [{ "id": "5", "filename": "a.png", "url": "https://cdn/a.png" }],
                "embeds": [],
                "mentions": [{ "id": "20", "username": "bob_user", "global_name": "Bob" }],
                "type": 0, "flags": 0
            }}]
        }));
        let chat = ChatMessage::from_discord_in(&msg, "99", None);
        // Sin la guarda de `replied_to_from_discord` esto saldría como
        // "Mensaje original eliminado".
        assert!(chat.replied_to.is_none());
        let forward = chat.forwarded.expect("es un reenvío");
        assert_eq!(forward.source_channel_id.as_deref(), Some("88"));
        assert_eq!(forward.source_guild_id.as_deref(), Some("66"));
        assert_eq!(forward.snapshots.len(), 1);
        let snapshot = &forward.snapshots[0];
        assert_eq!(snapshot.content, "texto original");
        assert_eq!(snapshot.time, "23:59");
        assert!(!snapshot.edited);
        assert_eq!(snapshot.attachments.len(), 1);
        assert_eq!(snapshot.mentions, vec![("20".to_string(), "Bob".to_string())]);
    }

    #[test]
    fn unreadable_snapshot_still_marks_the_message_as_a_forward() {
        let msg = message_with(json!({
            "message_reference": { "type": 1, "message_id": "77", "channel_id": "88" },
            "message_snapshots": ["basura"]
        }));
        let chat = ChatMessage::from_discord_in(&msg, "99", None);
        assert!(chat.replied_to.is_none());
        assert!(chat.forwarded.expect("es un reenvío").snapshots.is_empty());
    }

    #[test]
    fn forward_is_detected_from_the_snapshot_flag_alone() {
        // Sin `type` en la referencia y sin snapshots: solo el flag HAS_SNAPSHOT.
        let msg = message_with(json!({
            "flags": 16384,
            "message_reference": { "message_id": "77", "channel_id": "88" }
        }));
        let chat = ChatMessage::from_discord_in(&msg, "99", None);
        assert!(chat.replied_to.is_none());
        assert!(chat.forwarded.is_some());
    }

    #[test]
    fn a_bad_embed_inside_a_snapshot_does_not_lose_the_text() {
        let msg = message_with(json!({
            "message_reference": { "type": 1, "message_id": "77", "channel_id": "88" },
            "message_snapshots": [{ "message": {
                "content": "sigue acá",
                "embeds": ["no soy un embed", 7],
                "mentions": [null]
            }}]
        }));
        let chat = ChatMessage::from_discord_in(&msg, "99", None);
        let forward = chat.forwarded.expect("es un reenvío");
        assert_eq!(forward.snapshots.len(), 1);
        assert_eq!(forward.snapshots[0].content, "sigue acá");
        assert!(forward.snapshots[0].embeds.is_empty());
    }

    #[test]
    fn plain_reply_keeps_working_with_and_without_the_embedded_original() {
        // Original borrado/viejo: referencia sin `referenced_message`.
        let gone = message_with(json!({ "type": 19, "message_reference": { "message_id": "5" } }));
        let chat = ChatMessage::from_discord_in(&gone, "99", None);
        assert!(chat.forwarded.is_none());
        assert!(chat.replied_to.expect("banner").deleted);

        // Original solo con un adjunto: el banner lo dice en vez de quedar vacío.
        let with_attachment = message_with(json!({
            "type": 19,
            "message_reference": { "message_id": "5" },
            "referenced_message": {
                "id": "5", "channel_id": "2", "content": "",
                "author": { "id": "20", "username": "bob_user", "global_name": "Bob" },
                "attachments": [{ "id": "6", "filename": "b.png", "url": "https://cdn/b.png" }]
            }
        }));
        let chat = ChatMessage::from_discord_in(&with_attachment, "99", None);
        let replied = chat.replied_to.expect("banner");
        assert!(!replied.deleted);
        assert_eq!(replied.author, "Bob");
        assert_eq!(replied.preview, "Adjunto");
    }

    #[test]
    fn unknown_member_keeps_the_global_name_and_default_color() {
        let server = server();
        let names = NameResolver::new(&server.member_info, &server.roles);
        let msg = ChatMessage::from_discord_in(&message("10"), "99", Some(names));
        assert_eq!(msg.author, "Ana");
        assert!(msg.author_color.is_none());
    }

    #[test]
    fn known_member_shows_server_nick_and_highest_colored_role() {
        let mut server = server();
        let roles = ["low".to_string(), "high".to_string(), "plain".to_string()];
        assert!(server.cache_member("10", Some("Anita"), &roles));
        let names = NameResolver::new(&server.member_info, &server.roles);
        let msg = ChatMessage::from_discord_in(&message("10"), "99", Some(names));
        assert_eq!(msg.author, "Anita");
        assert!(msg.author_color == Some(Color32::from_rgb(255, 0, 0)));
    }

    #[test]
    fn member_without_nick_keeps_global_name_but_gets_role_color() {
        let mut server = server();
        server.cache_member("10", None, &["low".to_string()]);
        let names = NameResolver::new(&server.member_info, &server.roles);
        let msg = ChatMessage::from_discord_in(&message("10"), "99", Some(names));
        assert_eq!(msg.author, "Ana");
        assert!(msg.author_color == Some(Color32::from_rgb(0, 255, 0)));
    }

    #[test]
    fn nick_arriving_later_updates_an_already_loaded_message() {
        let mut server = server();
        let names = NameResolver::new(&server.member_info, &server.roles);
        let mut msg = ChatMessage::from_discord_in(&message("10"), "99", Some(names));
        assert_eq!(msg.author, "Ana");

        server.cache_member("10", Some("Anita"), &["high".to_string()]);
        msg.reresolve_names(NameResolver::new(&server.member_info, &server.roles));
        assert_eq!(msg.author, "Anita");
        assert!(msg.author_color == Some(Color32::from_rgb(255, 0, 0)));
    }

    #[test]
    fn cache_member_reports_whether_anything_changed() {
        let mut server = server();
        assert!(server.cache_member("10", Some("Anita"), &[]));
        assert!(!server.cache_member("10", Some("Anita"), &[]));
        // Un apodo vacío es lo mismo que no tener apodo.
        assert!(server.cache_member("10", Some("  "), &[]));
        assert_eq!(server.member_info["10"].nick, None);
    }

    #[test]
    fn members_are_requested_only_once_and_only_when_unknown() {
        let mut server = server();
        server.cache_member("1", None, &[]);
        let first = server.take_unresolved_members(["1", "2", "3", ""]);
        assert_eq!(first, vec!["2".to_string(), "3".to_string()]);
        assert!(server.take_unresolved_members(["2", "3"]).is_empty());
    }

    #[test]
    fn message_create_member_carries_nick_and_roles() {
        let msg: GatewayMessage = serde_json::from_value(json!({
            "id": "1", "channel_id": "2",
            "author": { "id": "10", "username": "ana_user" },
            "member": { "nick": "Anita", "roles": ["high"] }
        }))
        .expect("mensaje válido");
        let member = msg.member.expect("member");
        assert_eq!(member.nick.as_deref(), Some("Anita"));
        assert_eq!(member.roles, ["high"]);
    }
}

#[cfg(test)]
mod voice_snapshot_tests {
    use super::*;
    use crate::discord::models::{Channel as ApiChannel, VoiceState};
    use serde_json::json;

    fn api_channel(id: &str, kind: u8) -> ApiChannel {
        serde_json::from_value(json!({ "id": id, "type": kind, "name": id })).expect("canal válido")
    }

    /// Estado de voz como llega en `READY_SUPPLEMENTAL`: solo ids, sin `member`.
    fn bare_state(user_id: &str, channel_id: &str) -> VoiceState {
        serde_json::from_value(json!({ "user_id": user_id, "channel_id": channel_id }))
            .expect("estado válido")
    }

    fn state_with_member(user_id: &str, channel_id: &str, nick: &str) -> VoiceState {
        serde_json::from_value(json!({
            "user_id": user_id, "channel_id": channel_id,
            "member": { "nick": nick, "user": { "id": user_id, "username": "u" } }
        }))
        .expect("estado válido")
    }

    fn server_with_voice_channels() -> Server {
        let mut server = demo_small_server("A", "x", Color32::WHITE);
        server.apply_channels(vec![api_channel("v1", 2), api_channel("v2", 2)]);
        server
    }

    fn occupants(server: &mut Server, channel_id: &str) -> Vec<String> {
        server
            .channel_by_id_mut(channel_id)
            .expect("canal")
            .voice_members
            .iter()
            .map(|o| o.user_id.clone())
            .collect()
    }

    #[test]
    fn the_same_snapshot_arriving_twice_does_not_duplicate_anyone() {
        let mut server = server_with_voice_channels();
        let states = vec![bare_state("1", "v1"), bare_state("2", "v1"), bare_state("3", "v2")];
        server.apply_voice_snapshot(&states);
        server.apply_voice_snapshot(&states);
        assert_eq!(occupants(&mut server, "v1"), ["1", "2"]);
        assert_eq!(occupants(&mut server, "v2"), ["3"]);
    }

    #[test]
    fn a_new_snapshot_replaces_the_old_one() {
        let mut server = server_with_voice_channels();
        server.apply_voice_snapshot(&[bare_state("1", "v1")]);
        server.apply_voice_snapshot(&[bare_state("2", "v2")]);
        assert!(occupants(&mut server, "v1").is_empty());
        assert_eq!(occupants(&mut server, "v2"), ["2"]);
        // Un snapshot vacío = nadie en voz.
        server.apply_voice_snapshot(&[]);
        assert!(occupants(&mut server, "v2").is_empty());
    }

    #[test]
    fn a_bare_snapshot_keeps_names_already_known() {
        let mut server = server_with_voice_channels();
        server.apply_voice_snapshot(&[state_with_member("1", "v1", "Anita")]);
        // El de `READY_SUPPLEMENTAL` no trae `member`: no pisa el nombre.
        server.apply_voice_snapshot(&[bare_state("1", "v1")]);
        let occupant = &server.channel_by_id_mut("v1").unwrap().voice_members[0];
        assert_eq!(occupant.name, "Anita");
        assert!(occupant.known);
        assert!(server.unknown_voice_user_ids().is_empty());
    }

    #[test]
    fn unknown_voice_users_are_listed_for_the_rest_fallback() {
        let mut server = server_with_voice_channels();
        server.apply_voice_snapshot(&[bare_state("1", "v1"), state_with_member("2", "v1", "Beto")]);
        let unknown = server.unknown_voice_user_ids();
        assert!(unknown.contains("1"));
        assert!(!unknown.contains("2"));
    }

    #[test]
    fn states_for_unloaded_channels_wait_for_the_channels() {
        let mut server = demo_small_server("A", "x", Color32::WHITE);
        server.categories.clear(); // guild sin canales cargados todavía
        server.apply_voice_snapshot(&[bare_state("1", "v1")]);
        assert_eq!(server.pending_voice_states.len(), 1);

        server.apply_channels(vec![api_channel("v1", 2)]);
        assert!(server.pending_voice_states.is_empty());
        assert_eq!(occupants(&mut server, "v1"), ["1"]);
    }

    #[test]
    fn a_member_update_names_a_voice_occupant() {
        let mut server = server_with_voice_channels();
        server.apply_voice_snapshot(&[bare_state("1", "v1")]);
        let user: crate::discord::models::User =
            serde_json::from_value(json!({ "id": "1", "username": "ana", "global_name": "Ana" }))
                .expect("usuario válido");
        server.apply_member_to_voice(&user, Some("Anita"));
        let occupant = &server.channel_by_id_mut("v1").unwrap().voice_members[0];
        assert_eq!(occupant.name, "Anita");
        assert!(occupant.known);
    }

    fn member_list_update(nick: Option<&str>) -> crate::discord::models::MemberListUpdate {
        serde_json::from_value(json!({
            "guild_id": "1", "id": "everyone",
            "ops": [{ "op": "SYNC", "range": [0, 99], "items": [
                { "member": { "user": { "id": "1", "username": "ana", "global_name": "Ana" },
                              "nick": nick, "roles": [] } }
            ]}]
        }))
        .expect("update válido")
    }

    #[test]
    fn the_member_list_names_someone_already_in_voice() {
        let mut server = server_with_voice_channels();
        // Llega solo con `user_id`, como en `READY_SUPPLEMENTAL`.
        server.apply_voice_snapshot(&[bare_state("1", "v1")]);
        assert!(server.unknown_voice_user_ids().contains("1"));

        server.apply_member_list_update(&member_list_update(Some("Anita")));
        let occupant = &server.channel_by_id_mut("v1").unwrap().voice_members[0];
        assert_eq!(occupant.name, "Anita");
        assert!(occupant.known);
        assert!(server.unknown_voice_user_ids().is_empty());
    }

    #[test]
    fn someone_joining_voice_after_the_member_list_needs_no_rest_lookup() {
        let mut server = server_with_voice_channels();
        server.apply_member_list_update(&member_list_update(Some("Anita")));

        // Un estado de voz posterior, otra vez solo con `user_id`.
        let occupant = server.resolved_voice_occupant(&bare_state("1", "v1"));
        assert_eq!(occupant.name, "Anita");
        assert!(occupant.known);

        // Y sin apodo se usa el nombre global, no el placeholder.
        let mut server = server_with_voice_channels();
        server.apply_member_list_update(&member_list_update(None));
        let occupant = server.resolved_voice_occupant(&bare_state("1", "v1"));
        assert_eq!(occupant.name, "Ana");
        assert!(occupant.known);
    }

    #[test]
    fn a_nick_alone_is_shown_but_still_asks_for_the_avatar() {
        let mut server = server_with_voice_channels();
        server.cache_member("1", Some("Anita"), &[]);
        let occupant = server.resolved_voice_occupant(&bare_state("1", "v1"));
        assert_eq!(occupant.name, "Anita");
        assert!(!occupant.known);
    }
}
#[cfg(test)]
mod channel_events_tests {
    use super::*;
    use crate::discord::models::{Channel as ApiChannel, ForumPage, Role, ThreadChannel};
    use serde_json::json;

    fn api(id: &str, kind: u8, name: &str, parent: Option<&str>, position: i64) -> ApiChannel {
        serde_json::from_value(json!({
            "id": id, "type": kind, "name": name, "parent_id": parent, "position": position
        }))
        .expect("canal válido")
    }

    fn server() -> Server {
        let mut server = demo_small_server("A", "x", Color32::WHITE);
        server.apply_channels(vec![
            api("cat", 4, "Texto", None, 0),
            api("t1", 0, "general", Some("cat"), 0),
            api("t2", 0, "random", Some("cat"), 1),
            api("f1", 15, "sugerencias", Some("cat"), 2),
        ]);
        server
    }

    fn thread(id: &str, flags: u64) -> ThreadChannel {
        serde_json::from_value(json!({
            "id": id, "name": id, "parent_id": "f1", "flags": flags,
            "message_count": 3, "last_message_id": id
        }))
        .expect("hilo válido")
    }

    fn role(id: &str, name: &str) -> Role {
        serde_json::from_value(json!({ "id": id, "name": name })).expect("rol válido")
    }

    #[test]
    fn snowflake_timestamp_matches_the_discord_docs_example() {
        assert_eq!(snowflake_ms("175928847299117063"), Some(1_462_015_105_796));
        assert_eq!(snowflake_ms("0"), None);
        assert_eq!(snowflake_ms("no-es-un-id"), None);
    }

    #[test]
    fn forums_are_flagged_and_do_not_ask_for_messages() {
        let mut server = server();
        let forum = server.channel_by_id_mut("f1").expect("foro");
        assert!(forum.is_forum && forum.loaded && !forum.is_voice && !forum.has_more);
        assert!(!server.channel_by_id_mut("t1").expect("texto").is_forum);
    }

    #[test]
    fn renaming_a_channel_keeps_what_was_already_loaded() {
        let mut server = server();
        let channel = server.channel_by_id_mut("t1").expect("canal");
        channel.loaded = true;
        channel.has_more = false;

        server.upsert_channel(api("t1", 0, "principal", Some("cat"), 0));

        let channel = server.channel_by_id_mut("t1").expect("canal");
        assert_eq!(channel.name, "principal");
        assert!(channel.loaded && !channel.has_more);
    }

    #[test]
    fn created_channels_show_up_and_deleted_ones_disappear() {
        let mut server = server();
        server.upsert_channel(api("t3", 0, "nuevo", Some("cat"), 3));
        assert!(server.channel_position_by_id("t3").is_some());
        server.remove_channel("t3");
        assert!(server.channel_position_by_id("t3").is_none());
        assert!(server.channel_position_by_id("t1").is_some());
    }

    #[test]
    fn channels_of_a_deleted_category_become_uncategorized() {
        let mut server = server();
        server.remove_channel("cat");
        assert_eq!(server.categories.len(), 1);
        assert!(server.categories[0].name.is_empty());
        assert!(server.channel_position_by_id("t1").is_some());
    }

    #[test]
    fn channel_events_before_the_first_load_are_ignored() {
        // Si no se pidieron los canales, armar una lista con uno solo haría
        // que `open_server` nunca los pida enteros.
        let mut server = demo_small_server("A", "x", Color32::WHITE);
        server.categories.clear();
        server.upsert_channel(api("t1", 0, "general", None, 0));
        assert!(server.categories.is_empty());
    }

    #[test]
    fn open_posts_survive_a_rebuild_and_stay_out_of_the_sidebar() {
        let mut server = server();
        server.open_thread("p1", "Mi post", Some("f1".to_string()));

        server.upsert_channel(api("t1", 0, "principal", Some("cat"), 0));

        let (category, channel) = server.channel_position_by_id("p1").expect("post abierto");
        let post = server.channel(category, channel).expect("post");
        assert!(post.is_thread && !post.access.can_view);
        assert_eq!(post.parent_id.as_deref(), Some("f1"));
        // Se puede estar viéndolo aunque la barra lateral no lo dibuje...
        assert!(server.channel_visible(category, channel));
        // ...y nunca es el canal que se abre por defecto.
        let (first_category, first_channel) = server.first_channel();
        assert!(!server.channel(first_category, first_channel).expect("canal").is_thread);
    }

    #[test]
    fn opening_the_same_post_twice_reuses_it() {
        let mut server = server();
        let first = server.open_thread("p1", "Mi post", Some("f1".to_string()));
        let second = server.open_thread("p1", "Mi post", Some("f1".to_string()));
        assert_eq!(first, second);
    }

    #[test]
    fn forum_page_puts_pinned_posts_first_and_drops_stale_sorts() {
        let mut server = server();
        let page = ForumPage {
            threads: vec![thread("100", 0), thread("200", 2)],
            first_messages: Vec::new(),
            has_more: true,
        };
        server.apply_forum_page("f1", 0, false, page);
        {
            let forum = &server.channel_by_id_mut("f1").expect("foro").forum;
            assert_eq!(forum.posts[0].id, "200");
            assert!(forum.posts[0].pinned);
            assert!(forum.loaded && forum.has_more);
            assert_eq!(forum.next_offset, 2);
        }
        // Respuesta a un pedido con otro orden: se descarta.
        server.apply_forum_page("f1", 0, true, ForumPage::default());
        assert_eq!(server.channel_by_id_mut("f1").expect("foro").forum.posts.len(), 2);
    }

    #[test]
    fn thread_events_update_the_forum_cards() {
        let mut server = server();
        let page = ForumPage {
            threads: vec![thread("100", 0), thread("200", 2)],
            first_messages: Vec::new(),
            has_more: false,
        };
        server.apply_forum_page("f1", 0, false, page);

        server.upsert_thread(&thread("300", 0));
        let ids = |server: &mut Server| -> Vec<String> {
            let forum = &server.channel_by_id_mut("f1").expect("foro").forum;
            forum.posts.iter().map(|p| p.id.clone()).collect()
        };
        // Nuevo arriba, pero el fijado sigue primero.
        assert_eq!(ids(&mut server), ["200", "300", "100"]);

        server.remove_thread("f1", "300");
        assert_eq!(ids(&mut server), ["200", "100"]);
    }

    #[test]
    fn role_events_add_edit_and_remove_roles() {
        let mut server = server();
        server.upsert_role(role("r1", "Mod"));
        assert!(server.roles.iter().any(|r| r.id == "r1" && r.name == "Mod"));

        server.upsert_role(role("r1", "Moderador"));
        assert_eq!(server.roles.iter().filter(|r| r.id == "r1").count(), 1);
        assert!(server.roles.iter().any(|r| r.name == "Moderador"));

        server.access_ctx.my_roles = Some(vec!["r1".to_string()]);
        server.remove_role("r1");
        assert!(!server.roles.iter().any(|r| r.id == "r1"));
        assert_eq!(server.access_ctx.my_roles, Some(Vec::new()));
    }
}
