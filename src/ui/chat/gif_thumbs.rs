//! Miniaturas livianas para la grilla del selector de GIFs.
//!
//! ## Por qué existe
//! Antes cada celda pedía su `preview` a `egui` (`Image::new(url)`). Si el
//! `preview` es un GIF animado, el loader de `egui` decodifica TODOS sus
//! cuadros a tamaño completo; con ~50 celdas se pasa enseguida de
//! `anim::DECODED_BUDGET` (32 MB) y `anim::maintain` suelta cada 1.5 s todo lo
//! decodificado (`forget_all`). Resultado: todas las celdas se borran y se
//! vuelven a decodificar a la vez (el parpadeo) y, si el presupuesto se vuelve
//! a pasar antes de terminar, nunca llegan a verse hasta que el mouse fuerza
//! la animación de una celda.
//!
//! ## Cómo funciona ahora
//! * Los bytes salen del mismo loader HTTP de siempre (RAM + disco, un pedido
//!   por URL).
//! * Se decodifica SOLO el primer cuadro, en un hilo, y se reduce a
//!   `max_side` px: ~100 KB por celda en vez de varios MB.
//! * El resultado es una `TextureHandle` propia de este módulo, así que
//!   `anim::maintain` no la toca (solo suelta los loaders de `egui`).
//! * Máximo `MAX_DECODING` decodificaciones a la vez para no saturar la CPU.
//! * `release()` las suelta todas al cerrar el selector.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use egui::{ColorImage, Context, TextureHandle, TextureId, TextureOptions};

const MAX_DECODING: usize = 3;
const RETRY_AFTER: f64 = 30.0;
/// Segundos sin dibujarse para soltar una miniatura (solo se revisa con el
/// selector abierto y más de `GC_MIN_ENTRIES` miniaturas).
const UNUSED_TTL: f64 = 20.0;
const GC_MIN_ENTRIES: usize = 60;

static DECODING: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static LAST_USE: std::cell::Cell<f64> = const { std::cell::Cell::new(f64::NEG_INFINITY) };
}

/// ¿Se dibujó alguna miniatura en el último segundo (selector de GIFs abierto)?
/// `anim::maintain` lo usa para no soltar las imágenes de la grilla.
pub fn active(now: f64) -> bool {
    LAST_USE.with(|t| now - t.get() < 1.0)
}

/// Estado de una miniatura.
#[derive(Clone, Copy, PartialEq)]
pub enum Thumb {
    Ready(TextureId),
    Pending,
    Failed,
}

enum Slot {
    Fetching,
    Decoding(Receiver<Option<ColorImage>>),
    Ready { tex: TextureHandle, last_used: f64 },
    Failed(f64),
}

thread_local! {
    static THUMBS: RefCell<HashMap<String, Slot>> = RefCell::new(HashMap::new());
}

/// Miniatura de `url` (primer cuadro, lado mayor `max_side` px). Hay que
/// llamarla cada frame mientras se dibuja la celda hasta que devuelva
/// `Ready`; el pedido de red y la decodificación ocurren una sola vez.
pub fn get(ctx: &Context, url: &str, max_side: u32) -> Thumb {
    if url.is_empty() {
        return Thumb::Failed;
    }
    let now = ctx.input(|i| i.time);
    LAST_USE.with(|t| t.set(now));
    THUMBS.with_borrow_mut(|map| {
        if map.len() > GC_MIN_ENTRIES {
            map.retain(|_, s| match s {
                Slot::Ready { last_used, .. } => now - *last_used < UNUSED_TTL,
                _ => true,
            });
        }
        let slot = map.entry(url.to_string()).or_insert(Slot::Fetching);

        // 1. ¿Terminó la decodificación? ¿Toca reintentar un fallo viejo?
        let next = match slot {
            Slot::Decoding(rx) => match rx.try_recv() {
                Ok(Some(image)) => Some(Slot::Ready {
                    tex: ctx.load_texture(format!("gifthumb:{url}"), image, TextureOptions::LINEAR),
                    last_used: now,
                }),
                Ok(None) | Err(TryRecvError::Disconnected) => Some(Slot::Failed(now)),
                Err(TryRecvError::Empty) => None,
            },
            Slot::Failed(at) if now - *at > RETRY_AFTER => Some(Slot::Fetching),
            _ => None,
        };
        if let Some(n) = next {
            *slot = n;
        }

        // 2. Según el estado.
        match slot {
            Slot::Ready { tex, last_used } => {
                *last_used = now;
                return Thumb::Ready(tex.id());
            }
            Slot::Failed(_) => return Thumb::Failed,
            Slot::Decoding(_) => {
                ctx.request_repaint_after(Duration::from_millis(60));
                return Thumb::Pending;
            }
            Slot::Fetching => {}
        }

        match ctx.try_load_bytes(url) {
            Ok(egui::load::BytesPoll::Ready { bytes, .. }) => {
                if DECODING.load(Ordering::Relaxed) >= MAX_DECODING {
                    ctx.request_repaint_after(Duration::from_millis(40));
                    return Thumb::Pending;
                }
                DECODING.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = mpsc::channel();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let image = std::panic::catch_unwind(AssertUnwindSafe(|| decode(bytes.as_ref(), max_side)))
                        .ok()
                        .flatten();
                    let was_last = DECODING.fetch_sub(1, Ordering::Relaxed) == 1;
                    let _ = tx.send(image);
                    // Terminó la tanda: los buffers temporales de la decodificación
                    // (cuadro completo + reducido + RGBA) quedan libres pero
                    // mimalloc los retiene; se devuelven al sistema ahora y no
                    // recién en el próximo ciclo de 20 s.
                    if was_last {
                        crate::support::mem_report::release_free_memory();
                    }
                    ctx2.request_repaint();
                });
                *slot = Slot::Decoding(rx);
                ctx.request_repaint_after(Duration::from_millis(60));
                Thumb::Pending
            }
            // El loader HTTP pide un repaint al terminar; esto es un respaldo.
            Ok(egui::load::BytesPoll::Pending { .. }) => {
                ctx.request_repaint_after(Duration::from_millis(80));
                Thumb::Pending
            }
            Err(_) => {
                *slot = Slot::Failed(now);
                Thumb::Failed
            }
        }
    })
}

fn decode(data: &[u8], max_side: u32) -> Option<ColorImage> {
    // Para un GIF/APNG/WebP animado `load_from_memory` devuelve solo el
    // primer cuadro.
    let img = image::load_from_memory(data).ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    let scale = (max_side as f32 / w.max(h) as f32).min(1.0);
    let img = if scale < 1.0 {
        img.resize(
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    // `into_rgba8` evita una copia completa si ya es RGBA8.
    let rgba = img.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// Bytes de texturas (GPU) que ocupan las miniaturas ahora.
pub fn bytes() -> usize {
    THUMBS.with_borrow(|m| {
        m.values()
            .map(|s| match s {
                Slot::Ready { tex, .. } => {
                    let [w, h] = tex.size();
                    w * h * 4
                }
                _ => 0,
            })
            .sum()
    })
}

/// Suelta todas las miniaturas (al cerrar el selector).
pub fn release() {
    THUMBS.with_borrow_mut(|m| m.clear());
}
