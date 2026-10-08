//! `PreloadedUserSettings`, el proto real de Discord con la configuración
//! que se sincroniza entre clientes (tema, locale, status, notificaciones,
//! privacidad, favoritos, etc.). Discord lo sirve en
//! `GET /users/@me/settings-proto/1` como `{ "settings": "<base64>" }` —
//! el base64 son los bytes del mensaje protobuf tal cual, sin envoltorio
//! extra. `PATCH` al mismo endpoint con el mismo shape lo actualiza
//! (Discord hace merge de los campos que mandes, no reemplaza todo el
//! blob).
//!
//! El código del mensaje (`proto::PreloadedUserSettings` y todos sus
//! anidados) sale generado por `prost` a partir de
//! `proto/discord_user_settings.proto` — ver `build.rs`. Este módulo solo
//! pone encima: decode/encode a bytes, y algunos helpers puntuales para
//! traducir un par de campos a los tipos que ya usa el resto de la app
//! (`ui::theme::ThemeMode`, etc.) sin que el resto del código tenga que
//! saber nada de protobuf.

pub mod proto {
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/discord_protos.discord_users.v1.rs"));
}

pub use proto::FrecencyUserSettings;
pub use proto::PreloadedUserSettings;

/// Decodifica los bytes crudos del proto (ya sin el base64 del JSON de
/// Discord) a `PreloadedUserSettings`. Todos los campos del mensaje son
/// `optional`/`repeated`/tienen default, así que esto no falla salvo que
/// los bytes estén directamente corrompidos o truncados.
pub fn decode(bytes: &[u8]) -> Result<PreloadedUserSettings, prost::DecodeError> {
    <PreloadedUserSettings as prost::Message>::decode(bytes)
}

/// Serializa de vuelta a bytes protobuf, listos para base64-encodear y
/// mandar en el body de un `PATCH /users/@me/settings-proto/1`.
///
/// Ojo: para un PATCH normalmente conviene mandar SOLO los campos que
/// cambiaron (un `PreloadedUserSettings` "parcial", con el resto en
/// default/`None`), no el blob entero re-serializado — Discord lo trata
/// como un merge de campos presentes, y un `Theme::THEME_UNSET` explícito
/// en `appearance.theme` es indistinguible de "no toqué el tema" porque
/// proto3 no manda los campos en default. Para overrides puntuales armá
/// un `PreloadedUserSettings::default()` y llenale solo lo que
/// corresponda (ver `set_theme` más abajo para un ejemplo).
pub fn encode(settings: &PreloadedUserSettings) -> Vec<u8> {
    <PreloadedUserSettings as prost::Message>::encode_to_vec(settings)
}

/// Decodifica directo desde el string base64 que devuelve Discord en el
/// campo `"settings"` de la respuesta de `settings-proto/1`.
pub fn decode_base64(settings_b64: &str) -> anyhow::Result<PreloadedUserSettings> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(settings_b64)?;
    Ok(decode(&bytes)?)
}

/// Igual que `encode`, pero ya en base64 — lo que espera el body del
/// PATCH (`{"settings": "<esto>"}`).
pub fn encode_base64(settings: &PreloadedUserSettings) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(encode(settings))
}

// --- Puente con los tipos propios de `ecord` --------------------------

/// El `ThemeMode` que entiende `ui::theme`/`App::theme_mode` no tiene un
/// equivalente para `THEME_UNSET` (el usuario nunca lo tocó desde este
/// cliente) — en ese caso devolvemos `None` y quien llame decide si deja
/// el tema que ya tenía puesto localmente.
///
/// `THEME_DARKER` y `THEME_MIDNIGHT` son variantes más oscuras del modo
/// oscuro que `ecord` todavía no distingue como paletas propias, así que
/// las dos caen en `ThemeMode::Dark` por ahora.
pub fn theme_mode_from_settings(
    settings: &PreloadedUserSettings,
) -> Option<crate::theme::ThemeMode> {
    use crate::theme::ThemeMode;
    use proto::preloaded_user_settings::Theme;

    let theme = settings.appearance.as_ref()?.theme();
    match theme {
        Theme::Dark | Theme::Darker | Theme::Midnight => Some(ThemeMode::Dark),
        Theme::Light => Some(ThemeMode::Light),
        Theme::Unset => None,
    }
}

/// Locale guardado en la cuenta (`"es-419"`, `"en-US"`, etc.), si el
/// usuario lo configuró alguna vez desde algún cliente oficial.
pub fn locale_from_settings(settings: &PreloadedUserSettings) -> Option<&str> {
    settings
        .localization
        .as_ref()?
        .locale
        .as_ref()
        .map(String::as_str)
}

/// Texto del status personalizado actual (el que se ve al lado del
/// nombre en la lista de miembros), si hay uno puesto y no expiró.
pub fn custom_status_text(settings: &PreloadedUserSettings) -> Option<&str> {
    let status = settings.status.as_ref()?.custom_status.as_ref()?;
    if status.text.is_empty() {
        None
    } else {
        Some(status.text.as_str())
    }
}

/// Arma un `PreloadedUserSettings` "parcial" con solo `appearance.theme`
/// seteado, pensado para mandar como body de un PATCH sin pisar el resto
/// de la configuración de la cuenta (ver el comentario de `encode`).
pub fn partial_with_theme(theme: proto::preloaded_user_settings::Theme) -> PreloadedUserSettings {
    use proto::preloaded_user_settings::AppearanceSettings;

    PreloadedUserSettings {
        appearance: Some(AppearanceSettings {
            theme: theme as i32,
            ..Default::default()
        }),
        ..Default::default()
    }
}

// --- Volumen / silencio por persona (`audio_context_settings`) ----------

/// Volumen y silencio local de cada persona de las llamadas, tal cual los
/// sincroniza la cuenta (`PreloadedUserSettings.audio_context_settings.user`).
/// El volumen del proto va en la misma escala que el slider del cliente
/// oficial: 0..=200 con 100 como valor normal.
pub fn participant_playback_from_settings(
    settings: &PreloadedUserSettings,
) -> Vec<(
    crate::discord::ids::Id<crate::discord::ids::marker::UserMarker>,
    crate::discord::VoiceParticipantPlaybackSettings,
)> {
    let Some(audio) = settings.audio_context_settings.as_ref() else {
        return Vec::new();
    };
    playback_entries(&audio.user)
}

/// Lo mismo para el audio de los streams (`audio_context_settings.stream`),
/// con la clave puesta en quien transmite.
pub fn stream_playback_from_settings(
    settings: &PreloadedUserSettings,
) -> Vec<(
    crate::discord::ids::Id<crate::discord::ids::marker::UserMarker>,
    crate::discord::VoiceParticipantPlaybackSettings,
)> {
    let Some(audio) = settings.audio_context_settings.as_ref() else {
        return Vec::new();
    };
    playback_entries(&audio.stream)
}

fn playback_entries(
    map: &std::collections::HashMap<u64, proto::preloaded_user_settings::AudioContextSetting>,
) -> Vec<(
    crate::discord::ids::Id<crate::discord::ids::marker::UserMarker>,
    crate::discord::VoiceParticipantPlaybackSettings,
)> {
    map.iter()
        .filter_map(|(user_id, entry)| {
            let id = crate::discord::ids::Id::new_checked(*user_id)?;
            Some((id, playback_from_entry(entry)))
        })
        .collect()
}

fn playback_from_entry(
    entry: &proto::preloaded_user_settings::AudioContextSetting,
) -> crate::discord::VoiceParticipantPlaybackSettings {
    // Una entrada que nunca se tocó (todo en cero, sin fecha) no es "volumen
    // 0": es la persona en sus valores normales.
    if entry.volume == 0.0 && !entry.muted && !entry.soundboard_muted && entry.modified_at == 0 {
        return crate::discord::VoiceParticipantPlaybackSettings::default();
    }
    let percent = entry.volume.round().clamp(0.0, 200.0) as u16;
    crate::discord::VoiceParticipantPlaybackSettings {
        volume: crate::discord::VoiceParticipantVolumePercent::new(percent),
        muted: entry.muted,
        soundboard_muted: entry.soundboard_muted,
    }
}

/// Arma un `PreloadedUserSettings` parcial con SOLO las personas indicadas en
/// `audio_context_settings.user`, para mandarlo como `PATCH`. Discord mergea
/// los mapas por clave, así que las demás personas quedan como estaban. Una
/// persona vuelta a los valores normales se manda igual (100 %, sin silenciar):
/// un merge no puede borrar una clave, y así los otros clientes también la
/// restablecen.
pub fn partial_with_participant_audio(
    users: &[(u64, crate::discord::VoiceParticipantPlaybackSettings)],
    streams: &[(u64, crate::discord::VoiceParticipantPlaybackSettings)],
    modified_at_ms: u64,
) -> PreloadedUserSettings {
    use proto::preloaded_user_settings::{AudioContextSetting, AudioSettings};

    let to_map = |entries: &[(u64, crate::discord::VoiceParticipantPlaybackSettings)]| {
        entries
            .iter()
            .map(|(user_id, playback)| {
                (
                    *user_id,
                    AudioContextSetting {
                        muted: playback.muted,
                        volume: f32::from(playback.volume.value()),
                        modified_at: modified_at_ms,
                        soundboard_muted: playback.soundboard_muted,
                    },
                )
            })
            .collect()
    };
    PreloadedUserSettings {
        audio_context_settings: Some(AudioSettings {
            user: to_map(users),
            stream: to_map(streams),
        }),
        ..Default::default()
    }
}
