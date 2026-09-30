use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::discord::ids::{
    Id,
    marker::{ChannelMarker, GuildMarker, UserMarker},
};
use crate::discord::{MicrophoneBufferMs, MicrophoneSensitivityDb, VoiceVolumePercent};
use crate::discord::{MemberInfo, VoiceScope, VoiceSoundKind, VoiceStateInfo};
use super::devices::VoiceAudioSources;

use crate::lib::state::App;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoiceParticipantState {
    pub user_id: Id<UserMarker>,
    pub display_name: String,
    pub deaf: bool,
    pub mute: bool,
    pub self_deaf: bool,
    pub self_mute: bool,
    pub speaking: bool,
}

/// `Serialize`/`Deserialize` con `#[serde(default)]` (así un blob viejo sin
/// algún campo, o directamente ausente, cae en los valores por default en
/// vez de fallar el parseo) son lo que le permite a `ui::settings`
/// persistir esto en `web_local_storage_api`, igual que ya hace con
/// `custom_themes` — ver `App::persist_voice_audio_settings`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct VoiceAudioSettings {
    pub allow_microphone_transmit: bool,
    pub noise_suppression: bool,
    pub microphone_buffer_ms: Option<MicrophoneBufferMs>,
    pub microphone_sensitivity: MicrophoneSensitivityDb,
    pub microphone_volume: VoiceVolumePercent,
    pub voice_output_volume: VoiceVolumePercent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CurrentVoiceConnectionState {
    pub scope: VoiceScope,
    pub channel_id: Id<ChannelMarker>,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub allow_microphone_transmit: bool,
    pub noise_suppression: bool,
    pub microphone_buffer_ms: Option<MicrophoneBufferMs>,
    pub microphone_sensitivity: MicrophoneSensitivityDb,
    pub microphone_volume: VoiceVolumePercent,
    pub voice_output_volume: VoiceVolumePercent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::discord) struct VoiceState {
    channel_id: Id<ChannelMarker>,
    user_id: Id<UserMarker>,
    display_name: Option<String>,
    deaf: bool,
    mute: bool,
    self_deaf: bool,
    self_mute: bool,
    speaking: bool,
}

/// Todo el estado de voz vivo (participantes por canal/DM). Vive colgado de
/// [`App`] como `App::voice`; a diferencia de concord, acá no hay cache de
/// streams: ecord no integra "compartir pantalla".
#[derive(Clone, Debug, Default)]
pub struct VoiceCache {
    pub(crate) states: HashMap<(VoiceScope, Id<UserMarker>), VoiceState>,
    /// Sensibilidad/volumen/supresión de ruido persistidos. Se cargan/
    /// guardan junto con los temas, ver `ui::settings`.
    pub audio: VoiceAudioSettings,
    /// Id de dispositivo (cpal) elegido para captura y reproducción,
    /// persistido por separado de `audio` porque viaja al runtime por un
    /// canal distinto (`VoiceRuntimeEvent::AudioSourcesChanged`, no atado
    /// a unirse/salir de un canal) — ver `App::set_voice_input_source`/
    /// `set_voice_output_source` en `ui::settings`.
    pub audio_sources: VoiceAudioSources,
}

impl App {
    /// Punto de entrada desde `lib::state::handle_discord_event`: aplica a
    /// `self.voice` (participantes por canal, quién habla) los eventos "de
    /// cable" que le importan. A diferencia de `voice::forward_app_event`
    /// (que alimenta el hilo del runtime, el que de verdad abre el socket
    /// de voz y manda/recibe audio), esto solo mantiene al día el cache que
    /// lee la UI — por eso puede vivir del lado sync sin tocar tokio.
    ///
    /// Los métodos de más abajo (`update_voice_state`, etc.) son
    /// `pub(in crate::discord)` a propósito: no se supone que cualquier
    /// `AppEvent` los dispare directo, solo los que ya pasaron por acá.
    pub(crate) fn apply_voice_wire_event(&mut self, event: &crate::discord::AppEvent) {
        use crate::discord::AppEvent;
        match event {
            AppEvent::VoiceStateUpdate(wire) => {
                if let Some(info) = wire_voice_state_info(wire) {
                    self.update_voice_state(&info);
                }
            }
            AppEvent::GuildVoiceStates { guild_id, states } => {
                // Foto completa: la reemplazamos entera en vez de
                // parchearla, igual que hace `Server::apply_voice_snapshot`
                // con el modelo viejo de `lib/data.rs`.
                if let Some(guild_id) = parse_guild_id(guild_id) {
                    self.remove_voice_states_for_guild(guild_id);
                    for wire in states {
                        if let Some(info) = wire_voice_state_info(wire) {
                            self.update_voice_state(&info);
                        }
                    }
                }
            }
            AppEvent::VoiceSpeakingUpdate { channel_id, user_id, speaking } => {
                // El evento no trae guild/scope (solo lo sabe el hilo de
                // voz, que es quien lo mandó a partir del RTP que recibió).
                // Como solo escuchamos audio en vivo del canal en el que
                // estamos conectados, el scope de nuestra propia conexión
                // es siempre el correcto acá.
                if let (Some(channel_id), Some(user_id), Some(current)) = (
                    parse_channel_id(channel_id),
                    parse_user_id(user_id),
                    self.current_user_voice_connection(),
                ) {
                    self.update_voice_speaking(current.scope, channel_id, user_id, *speaking);
                }
            }
            _ => {}
        }
    }

    fn current_voice_user_id(&self) -> Option<Id<UserMarker>> {
        self.me.as_ref().and_then(|me| parse_user_id(&me.id))
    }

    pub fn current_user_voice_connection(&self) -> Option<CurrentVoiceConnectionState> {
        let current_user_id = self.current_voice_user_id()?;
        let settings = &self.voice.audio;
        self.voice
            .states
            .iter()
            .find_map(|((scope, user_id), state)| {
                (*user_id == current_user_id).then_some(CurrentVoiceConnectionState {
                    scope: *scope,
                    channel_id: state.channel_id,
                    self_mute: state.self_mute,
                    self_deaf: state.self_deaf,
                    allow_microphone_transmit: settings.allow_microphone_transmit,
                    noise_suppression: settings.noise_suppression,
                    microphone_buffer_ms: settings.microphone_buffer_ms,
                    microphone_sensitivity: settings.microphone_sensitivity,
                    microphone_volume: settings.microphone_volume,
                    voice_output_volume: settings.voice_output_volume,
                })
            })
    }

    /// Versión "de UI" para saber si ya estamos conectados a
    /// `channel_id` (dentro del guild `guild_id`) — la usa
    /// `ui::server::voice_channel_view` para mostrar "Conectado" en vez
    /// de un botón "Unirse". `false` si alguno de los dos ids no parsea
    /// (canal de demo, sin id real de Discord).
    pub fn is_connected_to_voice_channel_str(&self, guild_id: &str, channel_id: &str) -> bool {
        let (Some(guild_id), Some(channel_id)) =
            (parse_guild_id(guild_id), parse_channel_id(channel_id))
        else {
            return false;
        };
        self.current_user_voice_connection().is_some_and(|conn| {
            conn.scope == VoiceScope::Guild(guild_id) && conn.channel_id == channel_id
        })
    }

    /// Igual que [`Self::is_connected_to_voice_channel_str`] pero para
    /// una llamada de DM/grupo (`VoiceScope::Private`, sin `guild_id`) —
    /// la usa `ui::dm::dm_top_bar` para mostrar "En llamada" en vez del
    /// botón de "Llamar".
    pub fn is_connected_to_voice_dm_str(&self, channel_id: &str) -> bool {
        let Some(channel_id) = parse_channel_id(channel_id) else { return false };
        self.current_user_voice_connection().is_some_and(|conn| {
            conn.scope == VoiceScope::Private(channel_id) && conn.channel_id == channel_id
        })
    }

    pub fn voice_participants_for_channel(
        &self,
        guild_id: Id<GuildMarker>,
        channel_id: Id<ChannelMarker>,
    ) -> Vec<VoiceParticipantState> {
        self.voice_participants_for_scope(VoiceScope::Guild(guild_id), channel_id)
    }

    /// Participantes de una llamada de DM o grupo.
    pub fn voice_participants_for_private_channel(
        &self,
        channel_id: Id<ChannelMarker>,
    ) -> Vec<VoiceParticipantState> {
        self.voice_participants_for_scope(VoiceScope::Private(channel_id), channel_id)
    }

    fn voice_participants_for_scope(
        &self,
        scope: VoiceScope,
        channel_id: Id<ChannelMarker>,
    ) -> Vec<VoiceParticipantState> {
        let mut participants = Vec::new();
        for ((state_scope, _), state) in &self.voice.states {
            if *state_scope == scope && state.channel_id == channel_id {
                participants.push(voice_participant_state(state));
            }
        }
        sort_voice_participants(&mut participants);
        participants
    }

    pub fn current_user_voice_speaking(&self) -> bool {
        let Some(current_user_id) = self.current_voice_user_id() else {
            return false;
        };
        self.user_voice_speaking(current_user_id)
    }

    pub fn user_voice_speaking_in_guild(
        &self,
        guild_id: Id<GuildMarker>,
        user_id: Id<UserMarker>,
    ) -> bool {
        self.voice
            .states
            .get(&(VoiceScope::Guild(guild_id), user_id))
            .map(|state| state.speaking)
            .unwrap_or(false)
    }

    /// Versión "de UI" de [`Self::user_voice_speaking_in_guild`]: recibe
    /// los ids tal cual llegan de `lib::data` (`String`, sin tipar) y
    /// devuelve `false` si no parsean, en vez de que quien la llama tenga
    /// que ver `parse_guild_id`/`parse_user_id` — son `pub(in
    /// crate::discord)` a propósito, y `ui::server` (el indicador de
    /// quién habla en la lista de canales) está afuera de ese módulo.
    pub fn user_voice_speaking_in_guild_str(&self, guild_id: &str, user_id: &str) -> bool {
        let (Some(guild_id), Some(user_id)) = (parse_guild_id(guild_id), parse_user_id(user_id))
        else {
            return false;
        };
        self.user_voice_speaking_in_guild(guild_id, user_id)
    }

    fn user_voice_speaking(&self, user_id: Id<UserMarker>) -> bool {
        self.voice
            .states
            .iter()
            .find_map(|((_, state_user_id), state)| {
                (*state_user_id == user_id).then_some(state.speaking)
            })
            .unwrap_or(false)
    }

    pub fn voice_participants_by_channel_for_guild(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> BTreeMap<Id<ChannelMarker>, Vec<VoiceParticipantState>> {
        let scope = VoiceScope::Guild(guild_id);
        let mut participants_by_channel: BTreeMap<Id<ChannelMarker>, Vec<VoiceParticipantState>> =
            BTreeMap::new();
        for ((state_scope, _), state) in &self.voice.states {
            if *state_scope != scope {
                continue;
            }
            participants_by_channel
                .entry(state.channel_id)
                .or_default()
                .push(voice_participant_state(state));
        }
        for participants in participants_by_channel.values_mut() {
            sort_voice_participants(participants);
        }
        participants_by_channel
    }

    pub(crate) fn voice_participant_counts_by_channel_for_guild(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> BTreeMap<Id<ChannelMarker>, usize> {
        let scope = VoiceScope::Guild(guild_id);
        let mut counts = BTreeMap::new();
        for ((state_scope, _), state) in &self.voice.states {
            if *state_scope != scope {
                continue;
            }
            let count = counts.entry(state.channel_id).or_insert(0usize);
            *count = count.saturating_add(1);
        }
        counts
    }

    pub(crate) fn voice_sound_for_state_update(
        &self,
        state: &VoiceStateInfo,
    ) -> Option<VoiceSoundKind> {
        let before_channel = self
            .voice
            .states
            .iter()
            .find(|((_, user_id), _)| *user_id == state.user_id)
            .map(|(_, current)| current.channel_id);
        let after = state.channel_id;

        if before_channel == after {
            return None;
        }

        if self.current_voice_user_id() == Some(state.user_id) {
            return match (before_channel, after) {
                (None, Some(_)) | (Some(_), Some(_)) => Some(VoiceSoundKind::Join),
                (Some(_), None) => Some(VoiceSoundKind::Leave),
                (None, None) => None,
            };
        }

        // Para otros usuarios, solo sonar si es el canal donde está el usuario actual.
        let active_voice_channel = self.current_user_voice_connection()?.channel_id;
        match (
            before_channel == Some(active_voice_channel),
            after == Some(active_voice_channel),
        ) {
            (false, true) => Some(VoiceSoundKind::Join),
            (true, false) => Some(VoiceSoundKind::Leave),
            _ => None,
        }
    }

    pub(in crate::discord) fn update_voice_state(&mut self, state: &VoiceStateInfo) {
        let user_id = state.user_id;
        let is_current_user = self.current_voice_user_id() == Some(user_id);

        let scope = state.scope();

        if is_current_user
            && let Some((previous_scope, previous_channel_id)) = self
                .voice
                .states
                .iter()
                .find(|((_, state_user_id), _)| *state_user_id == user_id)
                .map(|((scope, _), current)| (*scope, current.channel_id))
            && state.channel_id != Some(previous_channel_id)
        {
            self.clear_voice_speaking_for_channel(previous_scope, previous_channel_id);
        }

        if let Some(channel_id) = state.channel_id {
            let scope = scope.expect("a voice state with a channel always has a scope");
            let key = (scope, user_id);
            let speaking = self
                .voice
                .states
                .get(&key)
                .is_some_and(|current| current.channel_id == channel_id && current.speaking);
            self.voice
                .states
                .retain(|(state_scope, state_user_id), _| {
                    *state_user_id != user_id || *state_scope == scope
                });
            self.voice.states.insert(
                key,
                VoiceState {
                    channel_id,
                    user_id,
                    display_name: state.member.as_ref().map(|m| m.display_name.clone()),
                    deaf: state.deaf,
                    mute: state.mute,
                    self_deaf: state.self_deaf,
                    self_mute: state.self_mute,
                    speaking,
                },
            );
        } else {
            match state.guild_id {
                Some(guild_id) => {
                    self.voice
                        .states
                        .remove(&(VoiceScope::Guild(guild_id), user_id));
                }
                None => {
                    self.voice.states.retain(|(scope, state_user_id), _| {
                        !(matches!(scope, VoiceScope::Private(_)) && *state_user_id == user_id)
                    });
                }
            }
        }
    }

    pub(in crate::discord) fn update_voice_speaking(
        &mut self,
        scope: VoiceScope,
        channel_id: Id<ChannelMarker>,
        user_id: Id<UserMarker>,
        speaking: bool,
    ) {
        let Some(state) = self.voice.states.get_mut(&(scope, user_id)) else {
            return;
        };
        if state.channel_id == channel_id {
            state.speaking = speaking;
        }
    }

    pub(in crate::discord) fn remove_voice_state(
        &mut self,
        guild_id: Id<GuildMarker>,
        user_id: Id<UserMarker>,
    ) {
        self.voice
            .states
            .remove(&(VoiceScope::Guild(guild_id), user_id));
    }

    pub(in crate::discord) fn remove_voice_states_for_guild(&mut self, guild_id: Id<GuildMarker>) {
        self.voice
            .states
            .retain(|(scope, _), _| *scope != VoiceScope::Guild(guild_id));
    }

    pub(in crate::discord) fn remove_voice_states_for_channel(
        &mut self,
        channel_id: Id<ChannelMarker>,
    ) {
        self.voice
            .states
            .retain(|_, state| state.channel_id != channel_id);
    }

    fn clear_voice_speaking_for_channel(&mut self, scope: VoiceScope, channel_id: Id<ChannelMarker>) {
        for ((state_scope, _), state) in &mut self.voice.states {
            if *state_scope == scope && state.channel_id == channel_id {
                state.speaking = false;
            }
        }
    }
}

fn voice_participant_state(state: &VoiceState) -> VoiceParticipantState {
    VoiceParticipantState {
        user_id: state.user_id,
        display_name: state
            .display_name
            .clone()
            .unwrap_or_else(|| format!("user-{}", state.user_id.get())),
        deaf: state.deaf,
        mute: state.mute,
        self_deaf: state.self_deaf,
        self_mute: state.self_mute,
        speaking: state.speaking,
    }
}

/// Los ids de Discord llegan al resto de ecord como `String`; acá se
/// convierten al `Id<T>` tipado que usa todo el subsistema de voz (portado
/// de concord).
pub(crate) fn parse_user_id(raw: &str) -> Option<Id<UserMarker>> {
    raw.parse::<u64>().ok().and_then(Id::new_checked)
}

pub(crate) fn parse_guild_id(raw: &str) -> Option<Id<GuildMarker>> {
    raw.parse::<u64>().ok().and_then(Id::new_checked)
}

pub(crate) fn parse_channel_id(raw: &str) -> Option<Id<ChannelMarker>> {
    raw.parse::<u64>().ok().and_then(Id::new_checked)
}

/// Convierte el `VoiceState` "de cable" (ids como `String`) al
/// `VoiceStateInfo` tipado del cache — es el mismo mapeo que hace
/// `voice::forward_app_event` para el runtime (ver
/// `voice::runtime::voice_state_info_from_wire`), pero éste vive en el lado
/// sync (`lib::state`) así que va por separado en vez de compartir la
/// función privada de `runtime`.
fn wire_voice_state_info(wire: &crate::discord::models::VoiceState) -> Option<VoiceStateInfo> {
    let user_id = parse_user_id(&wire.user_id)?;
    let display_name = wire.display_name();
    Some(VoiceStateInfo {
        guild_id: wire.guild_id.as_deref().and_then(parse_guild_id),
        channel_id: wire.channel_id.as_deref().and_then(parse_channel_id),
        user_id,
        session_id: wire.session_id.clone(),
        member: Some(MemberInfo { user_id, display_name }),
        deaf: wire.deaf,
        mute: wire.mute,
        self_deaf: wire.self_deaf,
        self_mute: wire.self_mute,
    })
}

fn sort_voice_participants(participants: &mut [VoiceParticipantState]) {
    participants.sort_by_cached_key(|participant| {
        (participant.display_name.to_lowercase(), participant.user_id)
    });
}
