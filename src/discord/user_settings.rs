//! `PreloadedUserSettings`, el proto real de Discord con la configuración
//! que se sincroniza entre clientes (tema, locale, status, notificaciones,
//! privacidad, favoritos, etc.). Discord lo sirve en
//! `GET /users/@me/settings-proto/1` como `{ "settings": "<base64>" }` —
//! el base64 son los bytes del mensaje protobuf tal cual, sin envoltorio
//! extra. `PATCH` al mismo endpoint con el mismo shape lo actualiza
//! (Discord hace merge de los campos que mandes, no reemplaza todo el
//! blob).
//!
//! OJO: ese merge es por sección de primer nivel (`privacy`, `notifications`,
//! `text_and_images`, `status`...): cada sección que viaja REEMPLAZA a la
//! guardada entera, y sus subcampos que no vengan vuelven a su valor por
//! defecto (docs.discord.food, "Modify User Settings Proto"). Para cambiar un
//! solo campo hay que mandar su sección completa (ver
//! `lib::state::account_edit`).
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
pub fn theme_mode_from_settings(settings: &PreloadedUserSettings) -> Option<crate::theme::ThemeMode> {
    use crate::theme::ThemeMode;
    use proto::preloaded_user_settings::Theme;

    let theme = settings.appearance.as_ref()?.theme();
    match theme {
        Theme::Dark | Theme::Darker | Theme::Midnight => Some(ThemeMode::Dark),
        Theme::Light => Some(ThemeMode::Light),
        Theme::Unset => None,
    }
}

// --- Presencia (estado + estado personalizado) ------------------------

/// Arma el opcode 3 a partir de `PreloadedUserSettings.status`: el estado
/// que elegiste (en línea / ausente / no molestar / invisible) y tu estado
/// personalizado. `idle` (la ventana lleva un rato sin foco) pasa "en línea"
/// a "ausente" con `afk: true`, como el cliente oficial; un estado elegido a
/// mano (no molestar, invisible, ausente) nunca se pisa.
///
/// Un estado personalizado vencido (`expires_at_ms` en el pasado) se ignora.
pub fn presence_from_settings(
    settings: &PreloadedUserSettings,
    idle: bool,
) -> crate::discord::gateway::PresenceUpdate {
    use crate::discord::gateway::{CustomStatus, PresenceStatus, PresenceUpdate};

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let status_settings = settings.status.as_ref();
    let mut status = status_settings
        .and_then(|s| s.status.as_deref())
        .and_then(PresenceStatus::parse)
        .unwrap_or(PresenceStatus::Online);
    // Un estado con vencimiento (p. ej. "no molestar por 1 hora") que ya pasó
    // vuelve a "en línea".
    if let Some(s) = status_settings {
        if s.status_expires_at_ms != 0 && s.status_expires_at_ms <= now_ms {
            status = PresenceStatus::Online;
        }
    }
    if idle && status == PresenceStatus::Online {
        status = PresenceStatus::Idle;
    }

    let mut update = PresenceUpdate::new(status);
    // Mismo criterio que el oficial: ausente por inactividad => `since` con la
    // hora en que quedó ausente.
    if idle && status == PresenceStatus::Idle {
        update.since = Some(now_ms);
        update.afk = true;
    }
    if let Some(custom) = status_settings.and_then(|s| s.custom_status.as_ref()) {
        let expired = custom.expires_at_ms != 0 && custom.expires_at_ms <= now_ms;
        if !expired {
            update.custom_status = Some(CustomStatus {
                text: custom.text.clone(),
                emoji_name: (!custom.emoji_name.is_empty()).then(|| custom.emoji_name.clone()),
                emoji_id: (custom.emoji_id != 0).then(|| custom.emoji_id.to_string()),
                expires_at: (custom.expires_at_ms != 0).then(|| iso8601_from_ms(custom.expires_at_ms)),
            });
        }
    }
    update
}

/// `1761433200000` -> `"2025-10-25T23:00:00.000Z"` (sin depender de `chrono`).
fn iso8601_from_ms(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let millis = ms % 1000;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod presence_tests {
    use super::*;
    use crate::discord::gateway::PresenceStatus;
    use proto::preloaded_user_settings::{CustomStatus, StatusSettings};

    fn with_status(status: Option<&str>, custom: Option<CustomStatus>) -> PreloadedUserSettings {
        let mut s = PreloadedUserSettings::default();
        s.status = Some(StatusSettings { status: status.map(str::to_owned), custom_status: custom, ..Default::default() });
        s
    }

    #[test]
    fn iso8601_known_value() {
        assert_eq!(iso8601_from_ms(1_761_433_200_000), "2025-10-25T23:00:00.000Z");
        assert_eq!(iso8601_from_ms(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn idle_online_becomes_idle_afk_but_dnd_stays() {
        let online = with_status(Some("online"), None);
        assert_eq!(presence_from_settings(&online, false).status, PresenceStatus::Online);
        let idle = presence_from_settings(&online, true);
        assert_eq!(idle.status, PresenceStatus::Idle);
        assert!(idle.afk && idle.since.is_some());
        assert!(!presence_from_settings(&online, false).afk);
        let dnd = with_status(Some("dnd"), None);
        assert_eq!(presence_from_settings(&dnd, true).status, PresenceStatus::Dnd);
    }

    #[test]
    fn custom_status_is_carried_and_expired_one_is_dropped() {
        let live = CustomStatus { text: "hola".into(), emoji_name: "🎧".into(), ..Default::default() };
        let p = presence_from_settings(&with_status(None, Some(live)), false);
        assert_eq!(p.custom_status.unwrap().text, "hola");
        let expired = CustomStatus { text: "viejo".into(), expires_at_ms: 1, ..Default::default() };
        assert!(presence_from_settings(&with_status(None, Some(expired)), false).custom_status.is_none());
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