//! GIFs animados (avatares, emojis personalizados, reacciones, adjuntos)
//! decodificados y reproducidos "a mano", sin depender de que el loader
//! animado de `egui_extras` reconozca la URL.
//!
//! ## Por qué no alcanza con `egui::Image::new(url)`
//! El loader animado de `egui` decide si algo es un GIF mirando el FINAL de
//! la URI (o los bytes, si vinieron en memoria). Las URLs de Discord casi
//! nunca terminan en `.gif` (`…/a_hash.gif?size=128`, `…gif?ex=…&hm=…`), así
//! que el GIF cae al loader de imágenes normal, que solo decodifica el primer
//! cuadro: se ve, pero congelado. En vez de pelearnos con eso, acá:
//!
//! 1. se piden los bytes con el mismo loader HTTP de `egui` (`try_load_bytes`,
//!    que cachea por URL, así que no se descarga dos veces);
//! 2. si empiezan con la firma `GIF8` se decodifican TODOS los cuadros en un
//!    hilo aparte (con la crate `image`) y cada cuadro se sube como textura;
//! 3. en cada frame de la UI se elige el cuadro que corresponde al reloj.
//!
//! Se usa desde los puntos donde ya se dibujaba una imagen remota:
//! `egui::Image::new(anim::source(ctx, url))`. Mientras el GIF se baja o se
//! decodifica —o si no es un GIF animado— devuelve la URL tal cual, o sea el
//! comportamiento de antes (primer cuadro).
//!
//! ## Memoria
//! * Cada GIF se reduce a `MAX_SIDE` px de lado mayor, y si aun así pasa de
//!   `FRAME_BUDGET_BYTES` se lo sigue achicando: nunca se recorta la duración.
//! * Los GIF que dejan de dibujarse (se scrolleó lejos) se descartan a los
//!   `UNUSED_TTL` segundos, y hay un tope global (`TOTAL_BUDGET`).
//! * A lo sumo `MAX_DECODERS` decodificaciones en paralelo.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::load::{BytesPoll, SizedTexture};
use egui::{ColorImage, Context, ImageSource, TextureHandle, TextureOptions};
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::imageops::{self, FilterType};
use image::{AnimationDecoder, RgbaImage};

/// Lado mayor máximo de un cuadro (px). Un adjunto se muestra a ≤ 400 px
/// lógicos, así que esto alcanza aun con pantallas HiDPI.
const MAX_SIDE: u32 = 480;
/// Tope de memoria (CPU + GPU) de UN gif; si se pasa, se achica más.
const FRAME_BUDGET_BYTES: usize = 24 * 1024 * 1024;
/// Tope de cuadros por gif (un gif de 10 minutos no tiene sentido acá).
const MAX_FRAMES: usize = 300;
/// GIFs más pesados que esto no se animan (se ve el primer cuadro).
const MAX_DOWNLOAD: usize = 32 * 1024 * 1024;
/// Decodificaciones simultáneas.
const MAX_DECODERS: usize = 3;
/// Segundos sin dibujarse para soltar un gif de la caché.
const UNUSED_TTL: f64 = 12.0;
/// Lo mismo para banners, decoraciones y efectos de perfil (solo se ven con la
/// tarjeta abierta).
const PROFILE_TTL: f64 = 4.0;
/// Tope global de memoria de todos los gifs decodificados.
const TOTAL_BUDGET: usize = 96 * 1024 * 1024;

static DECODING: AtomicUsize = AtomicUsize::new(0);

struct Frame {
    tex: TextureHandle,
    /// Duración del cuadro en segundos.
    delay: f64,
}

struct Animation {
    frames: Vec<Frame>,
    /// Suma de las duraciones (segundos).
    total: f64,
    bytes: usize,
}

/// `None` = el hilo todavía no terminó; `Some(None)` = no se pudo / no es
/// animado; `Some(Some(_))` = listo.
type Slot = Arc<Mutex<Option<Option<Animation>>>>;

enum Stage {
    /// Esperando los bytes del loader HTTP.
    Fetching,
    /// Un hilo lo está decodificando.
    Decoding(Slot),
    Ready(Animation),
    /// No se anima (no es GIF, tiene un solo cuadro, falló…): se usa la
    /// ruta normal de `egui`.
    Plain,
}

struct Entry {
    stage: Stage,
    last_used: f64,
    /// Segundos sin dibujarse antes de soltarlo.
    ttl: f64,
}

impl Entry {
    fn bytes(&self) -> usize {
        match &self.stage {
            Stage::Ready(a) => a.bytes,
            _ => 0,
        }
    }
}

#[derive(Default)]
struct Cache {
    map: HashMap<String, Entry>,
    last_gc: f64,
}

thread_local! {
    // La UI de egui corre siempre en el hilo principal; los hilos de
    // decodificación solo escriben en un `Slot` compartido.
    static ANIMS: RefCell<Cache> = RefCell::new(Cache::default());
}


// ---------------------------------------------------------------------
// Modo de imágenes (diagnóstico de memoria)
// ---------------------------------------------------------------------

/// Cuánto se renderiza de las imágenes remotas animadas / de perfil. Sirve
/// para comprobar cuánta RAM se va en GIFs y perfiles: se cambia desde
/// Ajustes → Apariencia y se persiste (`App::set_image_mode`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ImageMode {
    /// Todo como siempre: GIF/APNG animados.
    #[default]
    Normal,
    /// Nada se anima ni se decodifica cuadro por cuadro: los avatares,
    /// emojis y GIFs se ven como imagen fija.
    StaticOnly,
    /// No se dibujan avatares (queda el círculo con la inicial) ni banners,
    /// decoraciones ni efectos de perfil.
    NoProfileImages,
}

impl ImageMode {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::StaticOnly => 1,
            Self::NoProfileImages => 2,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::StaticOnly,
            2 => Self::NoProfileImages,
            _ => Self::Normal,
        }
    }
}

static IMAGE_MODE: AtomicU8 = AtomicU8::new(0);

pub fn image_mode() -> ImageMode {
    ImageMode::from_u8(IMAGE_MODE.load(Ordering::Relaxed))
}

/// Cambia el modo y suelta lo ya decodificado (así la RAM baja al instante).
pub fn set_image_mode(mode: ImageMode) {
    IMAGE_MODE.store(mode.to_u8(), Ordering::Relaxed);
    if mode != ImageMode::Normal {
        ANIMS.with_borrow_mut(|cache| cache.map.clear());
    }
}

/// ¿Hay que saltarse avatares, banners, decoraciones y efectos de perfil?
pub fn hide_profile_images() -> bool {
    image_mode() == ImageMode::NoProfileImages
}

/// URL de la versión fija: los avatares/emojis/íconos animados del CDN de
/// Discord existen también en `.png` (primer cuadro). Para un GIF de otro
/// host se le agrega un fragmento, así el loader animado de `egui_extras`
/// (que mira el final de la URI) no lo reconoce y dibuja solo el primer cuadro.
pub fn static_url(url: &str) -> String {
    let cut = url.find(['?', '#']).unwrap_or(url.len());
    let (path, rest) = url.split_at(cut);
    if !path.to_ascii_lowercase().ends_with(".gif") {
        return url.to_string();
    }
    if path.contains("cdn.discordapp.com") {
        format!("{}.png{rest}", &path[..path.len() - 4])
    } else if rest.is_empty() {
        format!("{url}#static")
    } else {
        url.to_string()
    }
}

// ---------------------------------------------------------------------
// Memoria de imágenes: medición y recorte
// ---------------------------------------------------------------------

/// Tope de imágenes YA decodificadas que guarda `egui` (su loader las cachea
/// sin límite: un avatar por cada persona que se vio, banners, etc.).
const DECODED_BUDGET: usize = 48 * 1024 * 1024;
/// Cada cuánto se revisa (segundos de la UI).
const MAINTAIN_EVERY: f64 = 1.5;

thread_local! {
    static LAST_MAINTAIN: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

/// Cuánta memoria ocupa cada capa de imágenes (bytes).
#[derive(Clone, Copy, Default)]
pub struct MemStats {
    /// Archivos descargados que se guardan en RAM (`support::http_cache`).
    pub downloaded: usize,
    /// Imágenes ya decodificadas por `egui`.
    pub decoded: usize,
    /// Cuadros de GIF/APNG animados (`ui::anim`).
    pub animations: usize,
}

pub fn mem_stats(ctx: &Context) -> MemStats {
    let loaders = ctx.loaders();
    let downloaded = loaders.bytes.lock().iter().map(|l| l.byte_size()).sum();
    let decoded = loaders.image.lock().iter().map(|l| l.byte_size()).sum();
    let animations = ANIMS.with_borrow(|c| c.map.values().map(Entry::bytes).sum());
    MemStats { downloaded, decoded, animations }
}

/// Se llama en cada frame; cada pocos segundos, si lo decodificado pasa de
/// `DECODED_BUDGET`, lo suelta (lo que se sigue viendo se vuelve a decodificar
/// solo, desde los bytes o el disco).
pub fn maintain(ctx: &Context) {
    let now = ctx.input(|i| i.time);
    if LAST_MAINTAIN.with(|t| now - t.get()) < MAINTAIN_EVERY {
        return;
    }
    LAST_MAINTAIN.with(|t| t.set(now));
    if mem_stats(ctx).decoded <= DECODED_BUDGET {
        return;
    }
    let loaders = ctx.loaders();
    for l in loaders.image.lock().iter() {
        l.forget_all();
    }
    for l in loaders.texture.lock().iter() {
        l.forget_all();
    }
    ctx.request_repaint();
}

/// ¿El *path* de la URL (sin `?query` ni `#fragmento`) termina en `.gif`?
pub fn is_gif_url(url: &str) -> bool {
    url.split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase()
        .ends_with(".gif")
}

/// La URL tal cual, para `egui::Image::new(...)`: la ruta normal de `egui`
/// (primer cuadro de un GIF). Es lo que hay que usar para lo que NO está a la
/// vista, así no se baja/decodifica/anima nada fuera de pantalla.
pub fn plain(url: &str) -> ImageSource<'static> {
    ImageSource::Uri(Cow::Owned(url.to_string()))
}

/// Fuente de imagen para `egui::Image::new(...)`: el cuadro actual si `url`
/// es un GIF animado ya decodificado; si no, la URL (ruta normal de `egui`).
///
/// Hay que llamarla en cada frame en que se dibuje la imagen: es lo que
/// avanza la animación y pide el próximo repaint.
pub fn source(ctx: &Context, url: &str) -> ImageSource<'static> {
    source_sized(ctx, url, MAX_SIDE)
}

/// Como [`source`], pero decodificando los cuadros a `max_side` px de lado
/// mayor como mucho (vale el de la PRIMERA llamada para esa URL). Para
/// imágenes que se muestran chicas (avatares) no tiene sentido guardar
/// cientos de cuadros a 480 px.
pub fn source_sized(ctx: &Context, url: &str, max_side: u32) -> ImageSource<'static> {
    if image_mode() != ImageMode::Normal {
        return plain(&static_url(url));
    }
    if !is_gif_url(url) {
        return plain(url);
    }
    source_impl(ctx, url, Mode { apng: false, max_side: max_side.clamp(32, MAX_SIDE), max_frames: MAX_FRAMES, budget: FRAME_BUDGET_BYTES, ttl: UNUSED_TTL }, None)
}

/// Banner de perfil (GIF animado): cuadros al ancho en que se muestra, pocos
/// cuadros y poca memoria. Se suelta apenas se cierra la tarjeta.
pub fn source_banner(ctx: &Context, url: &str, width_px: f32) -> ImageSource<'static> {
    if image_mode() != ImageMode::Normal {
        return plain(&static_url(url));
    }
    if !is_gif_url(url) {
        return plain(url);
    }
    let max_side = (width_px.ceil() as u32).clamp(64, 480);
    source_impl(
        ctx,
        url,
        Mode { apng: false, max_side, max_frames: 200, budget: 10 * 1024 * 1024, ttl: PROFILE_TTL },
        None,
    )
}

/// Como [`source`], pero además anima PNG animados (APNG), que es lo que
/// sirve Discord para las decoraciones de avatar y los efectos de perfil
/// (sus URLs terminan en `.png`, así que no se pueden reconocer por la
/// extensión: se mira la firma del archivo una vez descargado).
///
/// * `max_side`: lado mayor máximo de cada cuadro (px). Vale el de la PRIMERA
///   llamada para esa URL.
/// * `clock`: segundo de la animación a mostrar. `None` = el reloj de la UI
///   (en bucle, como los GIF); `Some(t)` deja a quien llama controlar el
///   tiempo (los efectos de perfil arrancan de cero al abrir la tarjeta).
pub fn source_animated(
    ctx: &Context,
    url: &str,
    max_side: u32,
    clock: Option<f64>,
) -> ImageSource<'static> {
    if image_mode() != ImageMode::Normal {
        // Sin animar: quien llama espera una textura (efectos) o dibuja la
        // URL tal cual (decoraciones, primer cuadro).
        return plain(url);
    }
    // Decoraciones (≈160 px) y capas de efectos: mucho más ajustadas que un
    // GIF cualquiera, y se sueltan enseguida al cerrar la tarjeta.
    let max_side = max_side.clamp(32, 480);
    let (max_frames, budget) = if max_side <= 200 { (180, 6 * 1024 * 1024) } else { (180, 12 * 1024 * 1024) };
    source_impl(ctx, url, Mode { apng: true, max_side, max_frames, budget, ttl: PROFILE_TTL }, clock)
}

#[derive(Clone, Copy)]
struct Mode {
    /// ¿Aceptar también APNG (además de GIF)?
    apng: bool,
    max_side: u32,
    /// Tope de cuadros y de memoria de esta animación, y cuánto vive sin usarse.
    max_frames: usize,
    budget: usize,
    ttl: f64,
}

fn source_impl(ctx: &Context, url: &str, mode: Mode, clock: Option<f64>) -> ImageSource<'static> {
    let now = ctx.input(|i| i.time);
    ANIMS.with_borrow_mut(|cache| {
        gc(ctx, cache, now);
        let entry = cache
            .map
            .entry(url.to_string())
            .or_insert_with(|| Entry { stage: Stage::Fetching, last_used: now, ttl: mode.ttl });
        entry.last_used = now;

        let stage = std::mem::replace(&mut entry.stage, Stage::Plain);
        entry.stage = match stage {
            Stage::Fetching => fetch_step(ctx, url, mode),
            Stage::Decoding(slot) => decode_step(slot),
            other => other,
        };

        match &entry.stage {
            Stage::Ready(anim) => current_frame(ctx, anim, clock.unwrap_or(now)),
            _ => plain(url),
        }
    })
}

/// Pide los bytes al loader HTTP y, cuando llegan, lanza la decodificación.
fn fetch_step(ctx: &Context, url: &str, mode: Mode) -> Stage {
    match ctx.try_load_bytes(url) {
        Ok(BytesPoll::Ready { bytes, .. }) => {
            let data: &[u8] = bytes.as_ref();
            let animated_format = data.starts_with(b"GIF8") || (mode.apng && is_apng(data));
            if data.len() > MAX_DOWNLOAD || !animated_format {
                return Stage::Plain;
            }
            if DECODING.load(Ordering::Relaxed) >= MAX_DECODERS {
                // Hay cola: se reintenta en un rato.
                ctx.request_repaint_after(Duration::from_millis(100));
                return Stage::Fetching;
            }
            DECODING.fetch_add(1, Ordering::Relaxed);
            let slot: Slot = Arc::new(Mutex::new(None));
            let (ctx2, slot2, name) = (ctx.clone(), slot.clone(), url.to_string());
            std::thread::spawn(move || {
                let anim = catch_unwind(AssertUnwindSafe(|| {
                    decode(&ctx2, &name, bytes.as_ref(), mode)
                }))
                    .ok()
                    .flatten();
                if let Ok(mut guard) = slot2.lock() {
                    *guard = Some(anim);
                }
                DECODING.fetch_sub(1, Ordering::Relaxed);
                ctx2.request_repaint();
            });
            Stage::Decoding(slot)
        }
        // `egui` vuelve a dibujar solo cuando el loader termina.
        Ok(BytesPoll::Pending { .. }) => Stage::Fetching,
        Err(_) => Stage::Plain,
    }
}

fn decode_step(slot: Slot) -> Stage {
    let done = slot.lock().ok().and_then(|mut guard| guard.take());
    match done {
        Some(Some(anim)) => Stage::Ready(anim),
        Some(None) => Stage::Plain,
        None => Stage::Decoding(slot),
    }
}

/// Cuadro que corresponde al reloj de la UI, y repaint para el siguiente.
fn current_frame(ctx: &Context, anim: &Animation, now: f64) -> ImageSource<'static> {
    let pos = now % anim.total.max(0.001);
    let mut end = 0.0;
    let mut chosen = &anim.frames[0];
    let mut remaining = chosen.delay;
    for frame in &anim.frames {
        end += frame.delay;
        chosen = frame;
        remaining = end - pos;
        if pos < end {
            break;
        }
    }
    ctx.request_repaint_after(Duration::from_secs_f64(remaining.clamp(0.005, 1.0)));
    ImageSource::Texture(SizedTexture::new(chosen.tex.id(), chosen.tex.size_vec2()))
}

/// Suelta los gifs que hace rato no se dibujan y respeta el tope global.
fn gc(ctx: &Context, cache: &mut Cache, now: f64) {
    if now - cache.last_gc < 5.0 {
        return;
    }
    cache.last_gc = now;
    cache.map.retain(|url, e| {
        let keep = now - e.last_used <= e.ttl;
        if !keep {
            // Suelta también los bytes descargados y el primer cuadro
            // decodificado por `egui`, no solo las texturas de la animación.
            ctx.forget_image(url);
        }
        keep
    });

    let mut total: usize = cache.map.values().map(Entry::bytes).sum();
    if total > TOTAL_BUDGET {
        let mut by_age: Vec<(String, f64, usize)> = cache
            .map
            .iter()
            .filter(|(_, e)| e.bytes() > 0)
            .map(|(k, e)| (k.clone(), e.last_used, e.bytes()))
            .collect();
        by_age.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        for (key, _, bytes) in by_age {
            if total <= TOTAL_BUDGET {
                break;
            }
            cache.map.remove(&key);
            ctx.forget_image(&key);
            total = total.saturating_sub(bytes);
        }
    }
}

/// ¿Es un PNG animado? Un APNG es un PNG con un chunk `acTL` ANTES del
/// primer `IDAT`.
fn is_apng(data: &[u8]) -> bool {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if data.len() < 8 || data[..8] != SIGNATURE {
        return false;
    }
    let mut pos = 8usize;
    while pos + 8 <= data.len() {
        let len = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let kind = &data[pos + 4..pos + 8];
        if kind == b"acTL" {
            return true;
        }
        if kind == b"IDAT" {
            return false;
        }
        // longitud + tipo + datos + CRC
        pos = pos.saturating_add(12).saturating_add(len);
    }
    false
}

fn fit(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    let longest = w.max(h);
    if longest <= max_side {
        return (w.max(1), h.max(1));
    }
    let scale = max_side as f32 / longest as f32;
    (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1))
}

/// Decodifica todos los cuadros (ya compuestos: la crate `image` aplica el
/// desecho/blending de cada uno) y los sube como texturas. Corre en un hilo
/// aparte; `Context::load_texture` se puede llamar desde cualquier hilo.
/// `None` si tiene un solo cuadro o no se pudo leer.
fn decode(ctx: &Context, name: &str, data: &[u8], mode: Mode) -> Option<Animation> {
    let max_side = mode.max_side;
    // GIF o APNG: los dos entregan el mismo tipo de iterador de cuadros.
    let frames_iter = if data.starts_with(b"GIF8") {
        GifDecoder::new(Cursor::new(data)).ok()?.into_frames()
    } else {
        PngDecoder::new(Cursor::new(data)).ok()?.apng().ok()?.into_frames()
    };

    let mut stored: Vec<(RgbaImage, f64)> = Vec::new();
    let mut target: Option<(u32, u32)> = None;
    let mut bytes = 0usize;

    for frame in frames_iter {
        // Un cuadro roto a mitad de camino corta la animación ahí en vez de
        // tirar todo lo que ya se decodificó.
        let Ok(frame) = frame else { break };
        let (num, den) = frame.delay().numer_denom_ms();
        let mut delay = num as f64 / den.max(1) as f64 / 1000.0;
        // Igual que los navegadores: 0/10 ms se toman como 100 ms.
        if delay <= 0.011 {
            delay = 0.1;
        }
        let buf = frame.into_buffer();
        let (w, h) = buf.dimensions();
        let (mut tw, mut th) = match target {
            Some(t) => t,
            None => {
                let t = fit(w, h, max_side);
                target = Some(t);
                t
            }
        };
        let buf = if (w, h) == (tw, th) {
            buf
        } else {
            imageops::resize(&buf, tw, th, FilterType::Triangle)
        };
        bytes += buf.as_raw().len();
        stored.push((buf, delay));

        // Pasó del presupuesto: se achica TODO lo guardado y lo que sigue.
        while bytes > mode.budget && tw > 16 && th > 16 {
            tw = (tw * 3 / 4).max(1);
            th = (th * 3 / 4).max(1);
            bytes = 0;
            for (img, _) in stored.iter_mut() {
                *img = imageops::resize(&*img, tw, th, FilterType::Triangle);
                bytes += img.as_raw().len();
            }
            target = Some((tw, th));
        }
        if stored.len() >= mode.max_frames {
            break;
        }
    }

    if stored.len() < 2 {
        return None;
    }

    let mut frames = Vec::with_capacity(stored.len());
    let mut total = 0.0;
    let mut total_bytes = 0usize;
    for (i, (img, delay)) in stored.into_iter().enumerate() {
        let (w, h) = img.dimensions();
        total_bytes += img.as_raw().len();
        let color = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
        let tex = ctx.load_texture(format!("gif:{name}#{i}"), color, TextureOptions::LINEAR);
        total += delay;
        frames.push(Frame { tex, delay });
    }
    Some(Animation { frames, total, bytes: total_bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_gif_by_path_ignoring_query_and_fragment() {
        assert!(is_gif_url("https://cdn.discordapp.com/avatars/1/a_h.gif?size=128"));
        assert!(is_gif_url("https://x.com/a.GIF#frag"));
        assert!(is_gif_url("https://media.tenor.com/x/a.gif"));
        assert!(!is_gif_url("https://cdn.discordapp.com/avatars/1/h.png?size=128"));
        assert!(!is_gif_url("https://x.com/a.gifv"));
    }

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit(100, 50, 640), (100, 50));
        assert_eq!(fit(1280, 640, 640), (640, 320));
        assert_eq!(fit(320, 1280, 640), (160, 640));
    }
}
