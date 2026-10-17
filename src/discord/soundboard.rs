//! Soundboard de los canales de voz.
//!
//! - Lista de sonidos: los predeterminados de Discord
//!   (`GET /soundboard-default-sounds`) y los del server
//!   (`GET /guilds/{id}/soundboard-sounds`, que contesta `{ "items": [...] }`).
//! - Para tocar uno: `POST /channels/{id}/send-soundboard-sound`. Discord no
//!   mezcla el audio en la llamada: avisa a todos los que están en el canal
//!   con `VOICE_CHANNEL_EFFECT_SEND` y **cada cliente baja y reproduce el
//!   sonido por su cuenta** (también quien lo mandó). Por eso el audio se baja
//!   del CDN (`soundboard-sounds/{id}`) y se reproduce con
//!   `support::sound_alerts::play_clip`.
//!
//! El parseo es tolerante: un sonido raro se salta, no rompe la lista.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

/// Un sonido del soundboard.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundboardSound {
    pub sound_id: String,
    pub name: String,
    /// Volumen propio del sonido (0.0 a 1.0).
    pub volume: f32,
    pub emoji_name: Option<String>,
    pub emoji_id: Option<String>,
    /// Server dueño del sonido; `None` en los predeterminados.
    pub guild_id: Option<String>,
    /// `false` si el server perdió el boost que lo mantenía disponible.
    pub available: bool,
}

fn id_text(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

impl SoundboardSound {
    pub fn from_value(value: &Value) -> Option<Self> {
        let sound_id = id_text(value.get("sound_id"))?;
        Some(Self {
            name: value
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.trim().is_empty())
                .unwrap_or("Sonido")
                .to_owned(),
            volume: value
                .get("volume")
                .and_then(Value::as_f64)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0) as f32,
            emoji_name: value.get("emoji_name").and_then(Value::as_str).map(str::to_owned),
            emoji_id: id_text(value.get("emoji_id")),
            guild_id: id_text(value.get("guild_id")),
            available: value.get("available").and_then(Value::as_bool).unwrap_or(true),
            sound_id,
        })
    }
}

/// Lista de sonidos: un array suelto o `{ "items": [...] }`.
pub fn parse_list(raw: &Value) -> Vec<SoundboardSound> {
    let list = raw.as_array().or_else(|| raw.get("items").and_then(Value::as_array));
    list.map(|items| items.iter().filter_map(SoundboardSound::from_value).collect())
        .unwrap_or_default()
}

/// Dónde está el audio de un sonido.
pub fn audio_url(sound_id: &str) -> String {
    format!("https://cdn.discordapp.com/soundboard-sounds/{sound_id}")
}

/// Audios ya bajados (para no volver a pedir el mismo sonido cada vez).
static AUDIO_CACHE: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);
const AUDIO_CACHE_MAX: usize = 32;
/// Tope de tamaño de un audio (Discord los limita a 512 KB; margen de sobra).
const AUDIO_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Baja (o toma de la caché) el audio de un sonido. Bloquea: llamar desde un
/// hilo propio.
pub fn fetch_audio(sound_id: &str) -> Result<Vec<u8>, String> {
    if let Some(bytes) = AUDIO_CACHE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|cache| cache.get(sound_id).cloned())
    {
        return Ok(bytes);
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let sound_url = audio_url(sound_id);
    let _who = discord_client_rest::netlog::origin("soundboard (bajar audio)");
    let trace = crate::support::nettrace::begin("GET", &sound_url, "soundboard");
    let sent = client.get(&sound_url).send();
    crate::support::nettrace::observe_blocking(&trace, &sent);
    let response = sent.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    let bytes = response.bytes().map_err(|e| e.to_string())?;
    if bytes.len() > AUDIO_MAX_BYTES {
        return Err("el audio es demasiado grande".to_owned());
    }
    let bytes = bytes.to_vec();
    let mut guard = AUDIO_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    if cache.len() >= AUDIO_CACHE_MAX {
        cache.clear();
    }
    cache.insert(sound_id.to_owned(), bytes.clone());
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_items_and_arrays() {
        let items = json!({ "items": [
            { "sound_id": "1", "name": "Quack", "volume": 0.5, "emoji_name": "🦆", "guild_id": "9", "available": true },
            { "name": "sin id" },
            { "sound_id": 2, "name": "", "volume": 3.0, "available": false }
        ]});
        let list = parse_list(&items);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Quack");
        assert_eq!(list[0].guild_id.as_deref(), Some("9"));
        assert_eq!(list[1].sound_id, "2");
        assert_eq!(list[1].name, "Sonido");
        assert_eq!(list[1].volume, 1.0);
        assert!(!list[1].available);
        assert_eq!(parse_list(&json!([{ "sound_id": "5" }])).len(), 1);
        assert!(parse_list(&json!({})).is_empty());
    }
}
