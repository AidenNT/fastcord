use std::fmt;

use crate::discord::ids::{
    Id,
    marker::{ChannelMarker, GuildMarker, UserMarker},
};

/// Identifica el servidor de voz al que pertenece una conexión. La voz de
/// servidor se indexa por `guild_id`; las llamadas de DM/grupo no tienen
/// guild, así que Discord indexa lo mismo por `channel_id` con `guild_id`
/// en null.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum VoiceScope {
    Guild(Id<GuildMarker>),
    Private(Id<ChannelMarker>),
}

impl VoiceScope {
    pub fn guild_id(self) -> Option<Id<GuildMarker>> {
        match self {
            Self::Guild(guild_id) => Some(guild_id),
            Self::Private(_) => None,
        }
    }

    pub fn private_channel_id(self) -> Option<Id<ChannelMarker>> {
        match self {
            Self::Private(channel_id) => Some(channel_id),
            Self::Guild(_) => None,
        }
    }

    /// `server_id` de IDENTIFY: el guild id para voz de servidor, el channel
    /// id del DM para una llamada privada.
    pub fn server_id_string(self) -> String {
        match self {
            Self::Guild(guild_id) => guild_id.to_string(),
            Self::Private(channel_id) => channel_id.to_string(),
        }
    }

    /// Deriva un scope de un par `(guild_id, channel_id)`. Un guild gana; una
    /// llamada privada se indexa por su canal; ninguno presente (salida de
    /// un DM) da `None`.
    fn from_ids(
        guild_id: Option<Id<GuildMarker>>,
        channel_id: Option<Id<ChannelMarker>>,
    ) -> Option<Self> {
        match (guild_id, channel_id) {
            (Some(guild_id), _) => Some(Self::Guild(guild_id)),
            (None, Some(channel_id)) => Some(Self::Private(channel_id)),
            (None, None) => None,
        }
    }
}

/// Recorte de la info de miembro que ya viaja embebida en un `VOICE_STATE_UPDATE`
/// (a diferencia de concord, acá no se mantiene un cache de miembros aparte:
/// alcanza con lo que manda el propio evento de voz).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberInfo {
    pub user_id: Id<UserMarker>,
    pub display_name: String,
}

#[derive(Clone, Eq, PartialEq)]
pub struct VoiceStateInfo {
    /// `None` para voice states de llamadas de DM/grupo, que llegan con
    /// `guild_id` null.
    pub guild_id: Option<Id<GuildMarker>>,
    pub channel_id: Option<Id<ChannelMarker>>,
    pub user_id: Id<UserMarker>,
    pub session_id: Option<String>,
    pub member: Option<MemberInfo>,
    pub deaf: bool,
    pub mute: bool,
    pub self_deaf: bool,
    pub self_mute: bool,
}

impl fmt::Debug for VoiceStateInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VoiceStateInfo")
            .field("guild_id", &self.guild_id)
            .field("channel_id", &self.channel_id)
            .field("user_id", &self.user_id)
            .field(
                "session_id",
                &self.session_id.as_ref().map(|_| "<redacted>"),
            )
            .field("member", &self.member)
            .field("deaf", &self.deaf)
            .field("mute", &self.mute)
            .field("self_deaf", &self.self_deaf)
            .field("self_mute", &self.self_mute)
            .finish()
    }
}

impl VoiceStateInfo {
    pub fn scope(&self) -> Option<VoiceScope> {
        VoiceScope::from_ids(self.guild_id, self.channel_id)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct VoiceServerInfo {
    /// `None` para servidores de llamadas de DM/grupo, que llegan con
    /// `channel_id` en su lugar.
    pub guild_id: Option<Id<GuildMarker>>,
    pub channel_id: Option<Id<ChannelMarker>>,
    pub endpoint: Option<String>,
    pub token: String,
}

impl VoiceServerInfo {
    pub fn scope(&self) -> Option<VoiceScope> {
        VoiceScope::from_ids(self.guild_id, self.channel_id)
    }
}

impl fmt::Debug for VoiceServerInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VoiceServerInfo")
            .field("guild_id", &self.guild_id)
            .field("endpoint", &self.endpoint)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceConnectionStatus {
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceSoundKind {
    Join,
    Leave,
}
