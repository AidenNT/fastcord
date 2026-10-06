//! Afinidades de la cuenta con amigos y servers, tal como las calcula
//! Discord (`GET /users/@me/affinities/users` y `.../affinities/guilds`).
//!
//! Discord devuelve un puntaje por usuario/server que sube cuanto más
//! hablás, llamás o jugás con esa persona / en ese server. El valor crudo no
//! está acotado a un rango fijo, así que acá se lo pasa a un PORCENTAJE
//! relativo al más alto de la lista (el de mayor afinidad = 100 %). Con eso
//! se ordena el Inicio de la interfaz nueva (ver `ui::home`).

use std::collections::HashMap;

use serde_json::Value;

/// Porcentajes (0.0..=100.0) por id de usuario / de server.
#[derive(Default, Clone)]
pub struct AffinityMap(pub HashMap<String, f32>);

impl AffinityMap {
    /// Porcentaje de `id`, o `None` si Discord no lo mencionó.
    pub fn pct(&self, id: &str) -> Option<f32> {
        self.0.get(id).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Lee una lista `[{ "<id_key>": "...", "affinity": 0.42 }, ...]` y la deja
/// normalizada contra el valor máximo.
fn parse_list(list: Option<&Value>, id_key: &str) -> AffinityMap {
    let Some(items) = list.and_then(Value::as_array) else {
        return AffinityMap::default();
    };
    let raw: Vec<(String, f32)> = items
        .iter()
        .filter_map(|item| {
            let id = item.get(id_key)?.as_str()?.to_owned();
            let score = item.get("affinity")?.as_f64()? as f32;
            (score.is_finite() && score > 0.0).then_some((id, score))
        })
        .collect();
    let max = raw.iter().map(|(_, s)| *s).fold(0.0_f32, f32::max);
    if max <= 0.0 {
        return AffinityMap::default();
    }
    AffinityMap(raw.into_iter().map(|(id, s)| (id, s / max * 100.0)).collect())
}

/// Respuesta de `affinities/users`: `{ "user_affinities": [{ "user_id", "affinity" }], ... }`.
pub fn parse_users(value: &Value) -> AffinityMap {
    parse_list(value.get("user_affinities"), "user_id")
}

/// Respuesta de `affinities/guilds`: `{ "guild_affinities": [{ "guild_id", "affinity" }] }`.
pub fn parse_guilds(value: &Value) -> AffinityMap {
    parse_list(value.get("guild_affinities"), "guild_id")
}
