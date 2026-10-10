//! Miniaturas de las ventanas y pantallas del selector de "Compartir pantalla"
//! (`ui::share_picker`), como las del selector de Discord.
//!
//! Un hilo suelto (`share-thumbs`) saca una captura chica de cada fuente, una
//! tras otra, y las manda por un canal en cuanto están listas; la UI las sube
//! como texturas. Se repite cada pocos segundos mientras el selector está
//! abierto, así que las vistas se van actualizando.
//!
//! * Ventana: `PrintWindow` con `PW_RENDERFULLCONTENT`, que pide el contenido
//!   ya compuesto (navegadores y apps con GPU incluidas) sin tocar la
//!   ventana. Se usa esto y no Windows Graphics Capture porque WGC dibuja el
//!   borde amarillo sobre la ventana mientras captura, y se vería parpadear
//!   cada ventana del selector.
//! * Pantalla: un `StretchBlt` desde el escritorio al tamaño de la miniatura.
//!
//! Solo Windows; en el resto de sistemas no se genera ninguna miniatura y el
//! selector muestra un ícono.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

/// Tamaño máximo de una miniatura (entra en 16:9 sin deformar la fuente).
const THUMB_MAX: (u32, u32) = (480, 270);

/// Qué fuente se miniaturiza. Sirve también de clave de la textura.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ThumbKey {
    Window(isize),
    Monitor { x: i32, y: i32, width: u32, height: u32 },
}

/// Imagen RGBA lista para subir como textura.
pub(crate) struct Thumbnail {
    pub(crate) key: ThumbKey,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rgba: Vec<u8>,
}

/// Tamaño que entra en `max` manteniendo la proporción (mínimo 1x1).
fn fit_thumb(source: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let (sw, sh) = (u64::from(source.0.max(1)), u64::from(source.1.max(1)));
    let (mw, mh) = (u64::from(max.0), u64::from(max.1));
    let (w, h) = if sw * mh >= sh * mw {
        (mw, (mw * sh / sw).max(1))
    } else {
        ((mh * sw / sh).max(1), mh)
    };
    (w as u32, h as u32)
}

/// Saca las miniaturas de `sources` en un hilo aparte y las manda por `tx`.
/// `busy` queda en `true` hasta que termina (para no lanzar otra tanda encima),
/// y `repaint` avisa a la UI que llegó una imagen nueva.
pub(crate) fn spawn_source_thumbnails(
    sources: Vec<ThumbKey>,
    tx: Sender<Thumbnail>,
    busy: Arc<AtomicBool>,
    repaint: impl Fn() + Send + 'static,
) {
    busy.store(true, Ordering::Relaxed);
    let worker_busy = Arc::clone(&busy);
    let spawned = std::thread::Builder::new()
        .name("share-thumbs".to_owned())
        .spawn(move || {
            for key in sources {
                if let Some(thumbnail) = imp::capture(key) {
                    if tx.send(thumbnail).is_err() {
                        break;
                    }
                    repaint();
                }
            }
            worker_busy.store(false, Ordering::Relaxed);
        });
    if spawned.is_err() {
        busy.store(false, Ordering::Relaxed);
    }
}

#[cfg(windows)]
mod imp {
    use std::mem::{size_of, zeroed};
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HALFTONE, HBITMAP, HDC, HGDIOBJ, ReleaseDC,
        SRCCOPY, SelectObject, SetStretchBltMode, StretchBlt,
    };
    use windows_sys::Win32::Storage::Xps::PrintWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic};

    use super::{THUMB_MAX, ThumbKey, Thumbnail, fit_thumb};

    /// Ventanas más grandes que esto (en píxeles) no se miniaturizan: el
    /// buffer intermedio pesaría cientos de MB.
    const MAX_SOURCE_PIXELS: i64 = 8192 * 4608;
    /// Flag de `PrintWindow`: pide el contenido ya compuesto por DWM
    /// (windows-sys no exporta esta constante).
    const PW_RENDERFULLCONTENT: u32 = 2;

    pub(super) fn capture(key: ThumbKey) -> Option<Thumbnail> {
        // SAFETY: GDI/user32 con handles propios que se liberan acá mismo.
        unsafe {
            let screen = GetDC(null_mut());
            if screen.is_null() {
                return None;
            }
            let thumbnail = match key {
                ThumbKey::Window(hwnd) => window(screen, hwnd, key),
                ThumbKey::Monitor { x, y, width, height } => monitor(screen, x, y, width, height, key),
            };
            ReleaseDC(null_mut(), screen);
            thumbnail
        }
    }

    /// Bitmap de 32 bits en memoria con su DC, para dibujar y leer los píxeles.
    struct Dib {
        dc: HDC,
        bitmap: HBITMAP,
        previous: HGDIOBJ,
        bits: *const u8,
        width: u32,
        height: u32,
    }

    impl Dib {
        /// # Safety
        /// `screen` tiene que ser un DC válido.
        unsafe fn new(screen: HDC, width: u32, height: u32) -> Option<Self> {
            unsafe {
                let dc = CreateCompatibleDC(screen);
                if dc.is_null() {
                    return None;
                }
                let mut info: BITMAPINFO = zeroed();
                info.bmiHeader = BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    // Negativo: filas de arriba hacia abajo.
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                };
                let mut bits = null_mut();
                let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
                if bitmap.is_null() || bits.is_null() {
                    DeleteDC(dc);
                    return None;
                }
                let previous = SelectObject(dc, bitmap);
                Some(Self {
                    dc,
                    bitmap,
                    previous,
                    bits: bits as *const u8,
                    width,
                    height,
                })
            }
        }

        /// Los píxeles BGRA como RGBA opaco.
        fn to_rgba(&self) -> Vec<u8> {
            let length = self.width as usize * self.height as usize * 4;
            // SAFETY: el DIB tiene exactamente `width * height * 4` bytes y
            // vive mientras `self`.
            let bgra = unsafe { std::slice::from_raw_parts(self.bits, length) };
            let mut rgba = Vec::with_capacity(length);
            for pixel in bgra.chunks_exact(4) {
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
            rgba
        }
    }

    impl Drop for Dib {
        fn drop(&mut self) {
            // SAFETY: son los objetos que creó `new`.
            unsafe {
                SelectObject(self.dc, self.previous);
                DeleteObject(self.bitmap);
                DeleteDC(self.dc);
            }
        }
    }

    /// # Safety
    /// `screen` tiene que ser un DC válido.
    unsafe fn window(screen: HDC, hwnd: isize, key: ThumbKey) -> Option<Thumbnail> {
        unsafe {
            let hwnd = hwnd as *mut std::ffi::c_void;
            // Una ventana minimizada no tiene contenido que pintar.
            if IsIconic(hwnd) != 0 {
                return None;
            }
            let mut rect: RECT = zeroed();
            if GetWindowRect(hwnd, &mut rect) == 0 {
                return None;
            }
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            if width <= 0 || height <= 0 || i64::from(width) * i64::from(height) > MAX_SOURCE_PIXELS {
                return None;
            }
            let full = Dib::new(screen, width as u32, height as u32)?;
            if PrintWindow(hwnd, full.dc, PW_RENDERFULLCONTENT) == 0 {
                return None;
            }
            let (thumb_w, thumb_h) = fit_thumb((width as u32, height as u32), THUMB_MAX);
            let small = Dib::new(screen, thumb_w, thumb_h)?;
            SetStretchBltMode(small.dc, HALFTONE);
            if StretchBlt(
                small.dc, 0, 0, thumb_w as i32, thumb_h as i32, full.dc, 0, 0, width, height, SRCCOPY,
            ) == 0
            {
                return None;
            }
            Some(Thumbnail {
                key,
                width: thumb_w as usize,
                height: thumb_h as usize,
                rgba: small.to_rgba(),
            })
        }
    }

    /// # Safety
    /// `screen` tiene que ser un DC válido.
    unsafe fn monitor(screen: HDC, x: i32, y: i32, width: u32, height: u32, key: ThumbKey) -> Option<Thumbnail> {
        unsafe {
            if width == 0 || height == 0 {
                return None;
            }
            let (thumb_w, thumb_h) = fit_thumb((width, height), THUMB_MAX);
            let small = Dib::new(screen, thumb_w, thumb_h)?;
            SetStretchBltMode(small.dc, HALFTONE);
            if StretchBlt(
                small.dc,
                0,
                0,
                thumb_w as i32,
                thumb_h as i32,
                screen,
                x,
                y,
                width as i32,
                height as i32,
                SRCCOPY | CAPTUREBLT,
            ) == 0
            {
                return None;
            }
            Some(Thumbnail {
                key,
                width: thumb_w as usize,
                height: thumb_h as usize,
                rgba: small.to_rgba(),
            })
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{ThumbKey, Thumbnail};

    pub(super) fn capture(_key: ThumbKey) -> Option<Thumbnail> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_keep_aspect_ratio_inside_the_box() {
        assert_eq!(fit_thumb((1920, 1080), (480, 270)), (480, 270));
        assert_eq!(fit_thumb((800, 600), (480, 270)), (360, 270));
        assert_eq!(fit_thumb((3440, 1440), (480, 270)), (480, 200));
        assert_eq!(fit_thumb((300, 3000), (480, 270)), (27, 270));
    }
}
