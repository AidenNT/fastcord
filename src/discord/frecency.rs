//! Favoritos y "frecency" de la cuenta (`FrecencyUserSettings`, el proto
//! tipo 2 de `settings-proto`): GIFs, stickers y emojis favoritos, y qué
//! tan seguido se usan emojis, reacciones, stickers, comandos, apps,
//! sonidos del soundboard y canales/servers.
//!
//! Discord lo sirve en `GET /users/@me/settings-proto/2` y lo actualiza con
//! `PATCH` al mismo endpoint (ver `UwuRest::{get,patch}_frecency_settings`).
//! Las reglas que importan para no pisar datos de la cuenta:
//!
//! * El `PATCH` REEMPLAZA cada campo de primer nivel que se manda: si mando
//!   `favorite_gifs`, tiene que ir ENTERO (todos los GIFs), o se pierden los
//!   que falten. Por eso acá se guarda el proto completo y, al sincronizar,
//!   se manda una copia completa de cada campo modificado (nunca un campo a
//!   medias) y se omiten los que no se tocaron.
//! * Nada se escribe hasta que llegó el `GET` inicial (`loaded`): antes de
//!   eso el proto local está vacío y mandarlo borraría los favoritos reales.
//!   Mientras tanto las funciones que modifican no hacen nada.
//! * Discord pide agrupar los cambios: favoritos a los 10 s, frecency a los
//!   30 s (ver `FAVORITES_DELAY` / `FRECENCY_DELAY`). La ventana sin foco
//!   fuerza el envío (ver `tick`), porque cerrar la app no avisa.
//!
//! El estado es global (como `OWN_USER_ID` o `PROFILE_CACHE` en
//! `discord::mod`) para que el selector de emojis, que es una función
//! suelta de egui, pueda leer y escribir sin pasar `App` por todos lados.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::discord::user_settings::proto::frecency_user_settings as pb;
use crate::discord::user_settings::proto::FrecencyUserSettings;

// ---------------------------------------------------------------------
// Campos de primer nivel (para saber cuáles hay que mandar)
// ---------------------------------------------------------------------

pub mod field {
    pub const FAVORITE_GIFS: u32 = 1 << 0;
    pub const FAVORITE_STICKERS: u32 = 1 << 1;
    pub const STICKER_FRECENCY: u32 = 1 << 2;
    pub const FAVORITE_EMOJIS: u32 = 1 << 3;
    pub const EMOJI_FRECENCY: u32 = 1 << 4;
    pub const APPLICATION_COMMAND_FRECENCY: u32 = 1 << 5;
    pub const FAVORITE_SOUNDBOARD_SOUNDS: u32 = 1 << 6;
    pub const APPLICATION_FRECENCY: u32 = 1 << 7;
    pub const HEARD_SOUND_FRECENCY: u32 = 1 << 8;
    pub const PLAYED_SOUND_FRECENCY: u32 = 1 << 9;
    pub const GUILD_AND_CHANNEL_FRECENCY: u32 = 1 << 10;
    pub const EMOJI_REACTION_FRECENCY: u32 = 1 << 11;

    /// Lo que el usuario toca a propósito (se manda enseguida).
    pub const FAVORITES: u32 = FAVORITE_GIFS | FAVORITE_STICKERS | FAVORITE_EMOJIS | FAVORITE_SOUNDBOARD_SOUNDS;
}

/// Espera antes de mandar favoritos (acciones poco frecuentes).
const FAVORITES_DELAY: Duration = Duration::from_secs(10);
/// Espera antes de mandar frecency (acciones automáticas, se agrupan).
const FRECENCY_DELAY: Duration = Duration::from_secs(30);
/// Si un `PATCH` falla (rate limit, red) no se reintenta antes de esto.
const RETRY_DELAY: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------
// Cálculo del puntaje
// ---------------------------------------------------------------------

/// Cuántos usos recientes se guardan por ítem.
const MAX_RECENT_USES: usize = 10;
const DAY_MS: u64 = 24 * 3600 * 1000;

/// Peso de un uso según su antigüedad (los recientes valen más).
fn bucket_weight(age_ms: u64) -> f64 {
    match age_ms {
        a if a <= 3 * DAY_MS => 100.0,
        a if a <= 15 * DAY_MS => 70.0,
        a if a <= 30 * DAY_MS => 50.0,
        a if a <= 45 * DAY_MS => 30.0,
        a if a <= 80 * DAY_MS => 10.0,
        _ => 0.0,
    }
}

/// Puntaje de un ítem: usos totales por el peso promedio de sus usos
/// recientes. Es el mismo esquema que usa el cliente oficial (los
/// umbrales exactos son los que conozco de él; si Discord los cambia solo
/// cambia el orden de "más usados", nunca se rompe nada, porque acá se
/// recalcula siempre a partir de `recent_uses` y `total_uses`).
pub fn compute_score(total_uses: u32, recent_uses: &[u64], now_ms: u64) -> i32 {
    if recent_uses.is_empty() {
        return 0;
    }
    let bonus: f64 = recent_uses.iter().map(|t| bucket_weight(now_ms.saturating_sub(*t))).sum();
    let average = bonus / recent_uses.len() as f64;
    (f64::from(total_uses) * average).round().min(f64::from(i32::MAX)) as i32
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Suma un uso a `key`: total, usos recientes (tope `MAX_RECENT_USES`) y
/// puntaje.
fn bump<K: Eq + std::hash::Hash>(map: &mut HashMap<K, pb::FrecencyItem>, key: K, now: u64) {
    let item = map.entry(key).or_default();
    item.total_uses = item.total_uses.saturating_add(1);
    item.recent_uses.push(now);
    let extra = item.recent_uses.len().saturating_sub(MAX_RECENT_USES);
    if extra > 0 {
        item.recent_uses.drain(..extra);
    }
    let score = compute_score(item.total_uses, &item.recent_uses, now);
    item.score = score;
    item.frecency = score;
}

/// Las claves de `map` de más a menos usadas (puntaje recalculado; a igual
/// puntaje, la de uso más reciente; después, por clave para que sea estable).
fn ranked<K: Clone + Ord>(map: &HashMap<K, pb::FrecencyItem>, now: u64) -> Vec<K> {
    let mut rows: Vec<(K, i32, u64)> = map
        .iter()
        .filter(|(_, item)| item.total_uses > 0)
        .map(|(key, item)| {
            let score = compute_score(item.total_uses, &item.recent_uses, now);
            let last = item.recent_uses.iter().copied().max().unwrap_or(0);
            (key.clone(), score, last)
        })
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    rows.into_iter().map(|(key, _, _)| key).collect()
}

// ---------------------------------------------------------------------
// Estado global
// ---------------------------------------------------------------------

#[derive(Default)]
struct Store {
    settings: FrecencyUserSettings,
    /// Ya llegó el `GET` inicial: recién ahí se puede escribir.
    loaded: bool,
    /// Campos (`field::*`) modificados que todavía no se mandaron.
    dirty: u32,
    favorites_dirty_at: Option<Instant>,
    frecency_dirty_at: Option<Instant>,
    in_flight: bool,
    retry_at: Option<Instant>,
    /// Sube con cada `reset` (cambio o cierre de cuenta). Un envío que
    /// termina después de un reset es de la cuenta anterior y se ignora.
    generation: u64,
}

impl Store {
    fn touch(&mut self, bit: u32) {
        self.dirty |= bit;
        let now = Instant::now();
        if bit & field::FAVORITES != 0 {
            self.favorites_dirty_at.get_or_insert(now);
        } else {
            self.frecency_dirty_at.get_or_insert(now);
        }
    }

    /// Copia COMPLETA de cada campo de `mask` (y nada más): lo que va en el
    /// `PATCH`.
    fn partial_for(&self, mask: u32) -> FrecencyUserSettings {
        let s = &self.settings;
        let mut out = FrecencyUserSettings::default();
        if mask & field::FAVORITE_GIFS != 0 {
            out.favorite_gifs = Some(s.favorite_gifs.clone().unwrap_or_default());
        }
        if mask & field::FAVORITE_STICKERS != 0 {
            out.favorite_stickers = Some(s.favorite_stickers.clone().unwrap_or_default());
        }
        if mask & field::STICKER_FRECENCY != 0 {
            out.sticker_frecency = Some(s.sticker_frecency.clone().unwrap_or_default());
        }
        if mask & field::FAVORITE_EMOJIS != 0 {
            out.favorite_emojis = Some(s.favorite_emojis.clone().unwrap_or_default());
        }
        if mask & field::EMOJI_FRECENCY != 0 {
            out.emoji_frecency = Some(s.emoji_frecency.clone().unwrap_or_default());
        }
        if mask & field::APPLICATION_COMMAND_FRECENCY != 0 {
            out.application_command_frecency = Some(s.application_command_frecency.clone().unwrap_or_default());
        }
        if mask & field::FAVORITE_SOUNDBOARD_SOUNDS != 0 {
            out.favorite_soundboard_sounds = Some(s.favorite_soundboard_sounds.clone().unwrap_or_default());
        }
        if mask & field::APPLICATION_FRECENCY != 0 {
            out.application_frecency = Some(s.application_frecency.clone().unwrap_or_default());
        }
        if mask & field::HEARD_SOUND_FRECENCY != 0 {
            out.heard_sound_frecency = Some(s.heard_sound_frecency.clone().unwrap_or_default());
        }
        if mask & field::PLAYED_SOUND_FRECENCY != 0 {
            out.played_sound_frecency = Some(s.played_sound_frecency.clone().unwrap_or_default());
        }
        if mask & field::GUILD_AND_CHANNEL_FRECENCY != 0 {
            out.guild_and_channel_frecency = Some(s.guild_and_channel_frecency.clone().unwrap_or_default());
        }
        if mask & field::EMOJI_REACTION_FRECENCY != 0 {
            out.emoji_reaction_frecency = Some(s.emoji_reaction_frecency.clone().unwrap_or_default());
        }
        out
    }

    /// Mezcla lo que llegó del servidor. `partial` = solo trae los campos que
    /// cambiaron. Un campo que acá está modificado y sin mandar NO se pisa
    /// (si no, el eco de nuestro propio `PATCH` deshacería un cambio hecho
    /// justo después).
    fn merge_remote(&mut self, mut remote: FrecencyUserSettings, partial: bool) {
        macro_rules! take {
            ($name:ident, $bit:expr) => {
                if self.dirty & $bit == 0 && (!partial || remote.$name.is_some()) {
                    self.settings.$name = remote.$name.take();
                }
            };
        }
        take!(favorite_gifs, field::FAVORITE_GIFS);
        take!(favorite_stickers, field::FAVORITE_STICKERS);
        take!(sticker_frecency, field::STICKER_FRECENCY);
        take!(favorite_emojis, field::FAVORITE_EMOJIS);
        take!(emoji_frecency, field::EMOJI_FRECENCY);
        take!(application_command_frecency, field::APPLICATION_COMMAND_FRECENCY);
        take!(favorite_soundboard_sounds, field::FAVORITE_SOUNDBOARD_SOUNDS);
        take!(application_frecency, field::APPLICATION_FRECENCY);
        take!(heard_sound_frecency, field::HEARD_SOUND_FRECENCY);
        take!(played_sound_frecency, field::PLAYED_SOUND_FRECENCY);
        take!(guild_and_channel_frecency, field::GUILD_AND_CHANNEL_FRECENCY);
        take!(emoji_reaction_frecency, field::EMOJI_REACTION_FRECENCY);
        if remote.versions.is_some() {
            self.settings.versions = remote.versions.take();
        }
    }
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();

fn with<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mutex = STORE.get_or_init(|| Mutex::new(Store::default()));
    let mut guard = mutex.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

/// ¿Ya se bajaron los favoritos de la cuenta? (Hasta entonces no se puede
/// marcar ni desmarcar nada.)
pub fn is_loaded() -> bool {
    with(|s| s.loaded)
}

/// Al cerrar sesión: se olvida todo lo de la cuenta anterior.
pub fn reset() {
    with(|s| {
        let generation = s.generation.wrapping_add(1);
        *s = Store { generation, ..Store::default() };
    });
}

// ---------------------------------------------------------------------
// Entrada desde el servidor
// ---------------------------------------------------------------------

/// El `GET` inicial: reemplaza todo y habilita las escrituras.
pub fn ingest_initial(remote: FrecencyUserSettings) {
    with(|s| {
        s.merge_remote(remote, false);
        s.loaded = true;
    });
}

/// `USER_SETTINGS_PROTO_UPDATE` (tipo 2) del Gateway: la cuenta cambió desde
/// otro dispositivo (o es el eco de nuestro propio `PATCH`).
pub fn ingest_update(remote: FrecencyUserSettings, partial: bool) {
    with(|s| {
        if s.loaded {
            s.merge_remote(remote, partial);
        }
    });
}

pub fn decode_base64(settings_b64: &str) -> anyhow::Result<FrecencyUserSettings> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(settings_b64)?;
    Ok(<FrecencyUserSettings as prost::Message>::decode(bytes.as_slice())?)
}

pub fn encode_base64(settings: &FrecencyUserSettings) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(<FrecencyUserSettings as prost::Message>::encode_to_vec(settings))
}

// ---------------------------------------------------------------------
// Salida hacia el servidor
// ---------------------------------------------------------------------

struct FlushJob {
    mask: u32,
    generation: u64,
    partial: FrecencyUserSettings,
}

fn take_due_job(s: &mut Store, now: Instant, force: bool) -> Option<FlushJob> {
    if !s.loaded || s.dirty == 0 || s.in_flight {
        return None;
    }
    if let Some(at) = s.retry_at
        && now < at
    {
        return None;
    }
    let favorites_due = s.favorites_dirty_at.is_some_and(|t| now.duration_since(t) >= FAVORITES_DELAY);
    let frecency_due = s.frecency_dirty_at.is_some_and(|t| now.duration_since(t) >= FRECENCY_DELAY);
    if !(force || favorites_due || frecency_due) {
        return None;
    }
    let mask = s.dirty;
    let partial = s.partial_for(mask);
    s.dirty = 0;
    s.favorites_dirty_at = None;
    s.frecency_dirty_at = None;
    s.retry_at = None;
    s.in_flight = true;
    Some(FlushJob { mask, generation: s.generation, partial })
}

/// Termina un envío: si falló, los campos vuelven a quedar pendientes.
fn finish(mask: u32, generation: u64, ok: bool) {
    with(|s| {
        if s.generation != generation {
            return;
        }
        s.in_flight = false;
        if !ok {
            let now = Instant::now();
            s.dirty |= mask;
            if mask & field::FAVORITES != 0 {
                s.favorites_dirty_at.get_or_insert(now);
            }
            if mask & !field::FAVORITES != 0 {
                s.frecency_dirty_at.get_or_insert(now);
            }
            s.retry_at = Some(now + RETRY_DELAY);
        }
    });
}

async fn send(token: String, partial: &FrecencyUserSettings) -> anyhow::Result<()> {
    // TODO: implementar `PATCH` de frecency.
    let rest = crate::discord::uwu_rest::UwuRest::for_token(token).await?;
    rest.patch_frecency_settings(partial).await
    //Ok(())
}

/// Se llama una vez por frame. Manda lo pendiente si ya pasó la espera (o si
/// `force`, p. ej. con la ventana sin foco). Devuelve `true` mientras quede
/// algo sin mandar, para que quien llama pida un repaint más adelante.
pub fn tick(token: &str, force: bool) -> bool {
    if let Some(job) = with(|s| take_due_job(s, Instant::now(), force)) {
        let token = token.to_string();
        std::thread::spawn(move || {
            let ok = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => match rt.block_on(send(token, &job.partial)) {
                    Ok(()) => true,
                    Err(e) => {
                        log::warn!("No se pudieron guardar los favoritos/frecency: {e}");
                        false
                    }
                },
                Err(_) => false,
            };
            finish(job.mask, job.generation, ok);
        });
    }
    with(|s| s.loaded && (s.dirty != 0 || s.in_flight))
}

/// Manda lo pendiente ahora y espera (hasta 4 s). Para el cierre de la app.
pub fn flush_blocking(token: &str) {
    let Some(job) = with(|s| take_due_job(s, Instant::now(), true)) else {
        return;
    };
    let ok = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(4), send(token.to_string(), &job.partial))
                .await
                .map(|r| r.is_ok())
                .unwrap_or(false)
        }),
        Err(_) => false,
    };
    finish(job.mask, job.generation, ok);
}

// ---------------------------------------------------------------------
// GIFs favoritos
// ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FavoriteGif {
    /// Link del GIF (Tenor/Giphy): es la clave en el proto y lo que se manda
    /// como mensaje.
    pub url: String,
    /// Medio a mostrar: un `.gif` si `video` es `false`, un `.mp4` si no.
    pub src: String,
    pub width: u32,
    pub height: u32,
    pub video: bool,
    pub order: u32,
}

/// Los favoritos, el último agregado primero.
pub fn favorite_gifs() -> Vec<FavoriteGif> {
    with(|s| {
        let Some(favs) = s.settings.favorite_gifs.as_ref() else {
            return Vec::new();
        };
        let mut list: Vec<FavoriteGif> = favs
            .gifs
            .iter()
            .map(|(url, gif)| FavoriteGif {
                url: url.clone(),
                src: gif.src.clone(),
                width: gif.width,
                height: gif.height,
                video: gif.format() == pb::GifType::Video,
                order: gif.order,
            })
            .collect();
        list.sort_by(|a, b| b.order.cmp(&a.order).then_with(|| a.url.cmp(&b.url)));
        list
    })
}

pub fn is_favorite_gif(url: &str) -> bool {
    with(|s| s.settings.favorite_gifs.as_ref().is_some_and(|f| f.gifs.contains_key(url)))
}

/// Marca o desmarca un GIF. Devuelve si quedó como favorito (`false` también
/// si todavía no se pueden escribir los favoritos).
pub fn toggle_favorite_gif(gif: &FavoriteGif) -> bool {
    with(|s| {
        if !s.loaded || gif.url.is_empty() || gif.src.is_empty() {
            return false;
        }
        let favs = s.settings.favorite_gifs.get_or_insert_with(Default::default);
        let now_favorite = if favs.gifs.remove(&gif.url).is_some() {
            false
        } else {
            let order = favs.gifs.values().map(|g| g.order).max().unwrap_or(0).saturating_add(1);
            let format = if gif.video { pb::GifType::Video } else { pb::GifType::Image };
            favs.gifs.insert(
                gif.url.clone(),
                pb::FavoriteGif { format: format as i32, src: gif.src.clone(), width: gif.width, height: gif.height, order },
            );
            true
        };
        s.touch(field::FAVORITE_GIFS);
        now_favorite
    })
}

// ---------------------------------------------------------------------
// Emojis
// ---------------------------------------------------------------------
//
// Las claves del proto son el id (emojis personalizados) o el nombre
// "amigable" de Discord (Unicode: `joy`, `skull_crossbones`...). Para los
// Unicode se usan los shortcodes de la crate `emojis`, que coinciden con los
// de Discord en casi todos; un emoji sin shortcode simplemente no se
// registra, y una clave que no se reconoce se ignora al mostrarla.

/// Clave de proto de un emoji Unicode (`😂` → `joy`).
pub fn emoji_key_for_unicode(emoji: &str) -> Option<String> {
    let found = emojis::get(emoji)
        .or_else(|| emojis::get(emoji.trim_end_matches('\u{FE0F}')))
        .or_else(|| emojis::get(&format!("{emoji}\u{FE0F}")))?;
    found
        .shortcodes()
        .find(|code| !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .map(str::to_owned)
}

/// Al revés: el emoji Unicode de una clave (`joy` → `😂`). `None` si es un id
/// numérico (emoji personalizado) o un nombre desconocido.
pub fn unicode_for_emoji_key(key: &str) -> Option<&'static str> {
    if key.is_empty() || key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    emojis::get_by_shortcode(key).map(|e| e.as_str())
}

pub fn favorite_emojis() -> Vec<String> {
    with(|s| s.settings.favorite_emojis.as_ref().map(|f| f.emojis.clone()).unwrap_or_default())
}

pub fn is_favorite_emoji(key: &str) -> bool {
    with(|s| s.settings.favorite_emojis.as_ref().is_some_and(|f| f.emojis.iter().any(|e| e == key)))
}

/// Devuelve si quedó como favorito.
pub fn toggle_favorite_emoji(key: &str) -> bool {
    with(|s| {
        if !s.loaded || key.is_empty() {
            return false;
        }
        let favs = s.settings.favorite_emojis.get_or_insert_with(Default::default);
        let now_favorite = if let Some(index) = favs.emojis.iter().position(|e| e == key) {
            favs.emojis.remove(index);
            false
        } else {
            favs.emojis.push(key.to_owned());
            true
        };
        s.touch(field::FAVORITE_EMOJIS);
        now_favorite
    })
}

/// Un emoji usado en un mensaje.
pub fn record_emoji_use(key: &str) {
    with(|s| {
        if !s.loaded || key.is_empty() {
            return;
        }
        let map = &mut s.settings.emoji_frecency.get_or_insert_with(Default::default).emojis;
        bump(map, key.to_owned(), now_ms());
        s.touch(field::EMOJI_FRECENCY);
    });
}

/// Los `limit` emojis más usados en mensajes.
pub fn top_emojis(limit: usize) -> Vec<String> {
    with(|s| {
        let Some(f) = s.settings.emoji_frecency.as_ref() else {
            return Vec::new();
        };
        ranked(&f.emojis, now_ms()).into_iter().take(limit).collect()
    })
}

/// Una reacción puesta a un mensaje.
pub fn record_reaction_use(key: &str) {
    with(|s| {
        if !s.loaded || key.is_empty() {
            return;
        }
        let map = &mut s.settings.emoji_reaction_frecency.get_or_insert_with(Default::default).emojis;
        bump(map, key.to_owned(), now_ms());
        s.touch(field::EMOJI_REACTION_FRECENCY);
    });
}

/// Las `limit` reacciones más usadas.
pub fn top_reactions(limit: usize) -> Vec<String> {
    with(|s| {
        let Some(f) = s.settings.emoji_reaction_frecency.as_ref() else {
            return Vec::new();
        };
        ranked(&f.emojis, now_ms()).into_iter().take(limit).collect()
    })
}

// ---------------------------------------------------------------------
// Stickers
// ---------------------------------------------------------------------

pub fn favorite_stickers() -> Vec<u64> {
    with(|s| s.settings.favorite_stickers.as_ref().map(|f| f.sticker_ids.clone()).unwrap_or_default())
}

pub fn is_favorite_sticker(id: u64) -> bool {
    with(|s| s.settings.favorite_stickers.as_ref().is_some_and(|f| f.sticker_ids.contains(&id)))
}

/// Devuelve si quedó como favorito.
pub fn toggle_favorite_sticker(id: u64) -> bool {
    with(|s| {
        if !s.loaded || id == 0 {
            return false;
        }
        let favs = s.settings.favorite_stickers.get_or_insert_with(Default::default);
        let now_favorite = if let Some(index) = favs.sticker_ids.iter().position(|x| *x == id) {
            favs.sticker_ids.remove(index);
            false
        } else {
            favs.sticker_ids.push(id);
            true
        };
        s.touch(field::FAVORITE_STICKERS);
        now_favorite
    })
}

pub fn record_sticker_use(id: u64) {
    with(|s| {
        if !s.loaded || id == 0 {
            return;
        }
        let map = &mut s.settings.sticker_frecency.get_or_insert_with(Default::default).stickers;
        bump(map, id, now_ms());
        s.touch(field::STICKER_FRECENCY);
    });
}

pub fn top_stickers(limit: usize) -> Vec<u64> {
    with(|s| {
        let Some(f) = s.settings.sticker_frecency.as_ref() else {
            return Vec::new();
        };
        ranked(&f.stickers, now_ms()).into_iter().take(limit).collect()
    })
}

// ---------------------------------------------------------------------
// Comandos, aplicaciones, soundboard, canales/servers
// ---------------------------------------------------------------------
//
// Todavía no hay una pantalla en eCord que los use (no hay selector de
// comandos ni soundboard ni buscador rápido), pero se leen, se guardan y se
// mandan como cualquier otro, así que cuando exista esa pantalla alcanza con
// llamar a estas funciones.

/// Clave de un comando: el id con, opcionalmente, el nombre y el server
/// (`1221148400637050941\0set:550327071407079444`).
pub fn record_command_use(key: &str) {
    with(|s| {
        if !s.loaded || key.is_empty() {
            return;
        }
        let map = &mut s.settings.application_command_frecency.get_or_insert_with(Default::default).application_commands;
        bump(map, key.to_owned(), now_ms());
        s.touch(field::APPLICATION_COMMAND_FRECENCY);
    });
}

pub fn top_commands(limit: usize) -> Vec<String> {
    with(|s| {
        let Some(f) = s.settings.application_command_frecency.as_ref() else {
            return Vec::new();
        };
        ranked(&f.application_commands, now_ms()).into_iter().take(limit).collect()
    })
}

pub fn record_application_use(application_id: &str) {
    with(|s| {
        if !s.loaded || application_id.is_empty() {
            return;
        }
        let map = &mut s.settings.application_frecency.get_or_insert_with(Default::default).applications;
        bump(map, application_id.to_owned(), now_ms());
        s.touch(field::APPLICATION_FRECENCY);
    });
}

pub fn top_applications(limit: usize) -> Vec<String> {
    with(|s| {
        let Some(f) = s.settings.application_frecency.as_ref() else {
            return Vec::new();
        };
        ranked(&f.applications, now_ms()).into_iter().take(limit).collect()
    })
}

/// Un canal o server elegido (buscador rápido). Id numérico de Discord.
pub fn record_channel_visit(id: u64) {
    with(|s| {
        if !s.loaded || id == 0 {
            return;
        }
        let map = &mut s.settings.guild_and_channel_frecency.get_or_insert_with(Default::default).guild_and_channels;
        bump(map, id, now_ms());
        s.touch(field::GUILD_AND_CHANNEL_FRECENCY);
    });
}

pub fn top_channels(limit: usize) -> Vec<u64> {
    with(|s| {
        let Some(f) = s.settings.guild_and_channel_frecency.as_ref() else {
            return Vec::new();
        };
        ranked(&f.guild_and_channels, now_ms()).into_iter().take(limit).collect()
    })
}

pub fn favorite_sounds() -> Vec<u64> {
    with(|s| {
        let Some(f) = s.settings.favorite_soundboard_sounds.as_ref() else {
            return Vec::new();
        };
        // `ordered_sound_ids` es el orden elegido por el usuario; `sound_ids`
        // es la lista plana. Se prefiere la ordenada si la hay.
        if f.ordered_sound_ids.is_empty() { f.sound_ids.clone() } else { f.ordered_sound_ids.clone() }
    })
}

/// Devuelve si quedó como favorito. Mantiene las dos listas del proto iguales.
pub fn toggle_favorite_sound(id: u64) -> bool {
    with(|s| {
        if !s.loaded || id == 0 {
            return false;
        }
        let favs = s.settings.favorite_soundboard_sounds.get_or_insert_with(Default::default);
        let now_favorite = if favs.sound_ids.contains(&id) || favs.ordered_sound_ids.contains(&id) {
            favs.sound_ids.retain(|x| *x != id);
            favs.ordered_sound_ids.retain(|x| *x != id);
            false
        } else {
            favs.sound_ids.push(id);
            favs.ordered_sound_ids.push(id);
            true
        };
        s.touch(field::FAVORITE_SOUNDBOARD_SOUNDS);
        now_favorite
    })
}

/// Un sonido del soundboard que el usuario reprodujo.
pub fn record_sound_played(sound_id: &str) {
    with(|s| {
        if !s.loaded || sound_id.is_empty() {
            return;
        }
        let map = &mut s.settings.played_sound_frecency.get_or_insert_with(Default::default).played_sounds;
        bump(map, sound_id.to_owned(), now_ms());
        s.touch(field::PLAYED_SOUND_FRECENCY);
    });
}

/// Un sonido del soundboard que el usuario escuchó (de otra persona).
pub fn record_sound_heard(sound_id: &str) {
    with(|s| {
        if !s.loaded || sound_id.is_empty() {
            return;
        }
        let map = &mut s.settings.heard_sound_frecency.get_or_insert_with(Default::default).heard_sounds;
        bump(map, sound_id.to_owned(), now_ms());
        s.touch(field::HEARD_SOUND_FRECENCY);
    });
}

pub fn top_played_sounds(limit: usize) -> Vec<String> {
    with(|s| {
        let Some(f) = s.settings.played_sound_frecency.as_ref() else {
            return Vec::new();
        };
        ranked(&f.played_sounds, now_ms()).into_iter().take(limit).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_uses_outrank_old_ones() {
        let now = 100 * DAY_MS;
        let fresh = compute_score(3, &[now - 1000, now - 2000, now - 3000], now);
        let stale = compute_score(3, &[now - 90 * DAY_MS, now - 91 * DAY_MS, now - 92 * DAY_MS], now);
        assert!(fresh > stale);
        assert_eq!(stale, 0);
        assert_eq!(compute_score(5, &[], now), 0);
    }

    #[test]
    fn bump_keeps_only_the_last_samples() {
        let mut map: HashMap<String, pb::FrecencyItem> = HashMap::new();
        for i in 0..25 {
            bump(&mut map, "joy".to_string(), 1_000 + i);
        }
        let item = &map["joy"];
        assert_eq!(item.total_uses, 25);
        assert_eq!(item.recent_uses.len(), MAX_RECENT_USES);
        assert_eq!(*item.recent_uses.last().unwrap(), 1_024);
    }

    #[test]
    fn partial_only_carries_dirty_fields_in_full() {
        let mut store = Store { loaded: true, ..Default::default() };
        let favs = store.settings.favorite_emojis.get_or_insert_with(Default::default);
        favs.emojis.push("joy".into());
        favs.emojis.push("skull".into());
        store.touch(field::FAVORITE_EMOJIS);
        let partial = store.partial_for(store.dirty);
        assert_eq!(partial.favorite_emojis.unwrap().emojis.len(), 2);
        assert!(partial.favorite_gifs.is_none());
        assert!(partial.emoji_frecency.is_none());
    }

    #[test]
    fn remote_echo_does_not_undo_unsent_local_change() {
        let mut store = Store { loaded: true, ..Default::default() };
        store.settings.favorite_emojis = Some(pb::FavoriteEmojis { emojis: vec!["joy".into()] });
        store.touch(field::FAVORITE_EMOJIS);
        let remote = FrecencyUserSettings {
            favorite_emojis: Some(pb::FavoriteEmojis { emojis: vec![] }),
            ..Default::default()
        };
        store.merge_remote(remote, true);
        assert_eq!(store.settings.favorite_emojis.unwrap().emojis, vec!["joy".to_string()]);
    }
}
