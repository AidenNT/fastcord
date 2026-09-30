//! Decodificador H.264 del visor de streams.
//!
//! Recibe access units en Annex B (ya descifrados con DAVE) y devuelve frames
//! RGBA listos para subir a una textura de egui. Usa la misma crate de ffmpeg
//! (`ffmpeg-the-third`) y las mismas llamadas que el reproductor embebido
//! (`vendor/egui-video`), así que no agrega nada nuevo a las libs que hay que
//! tener instaladas.
//!
//! Corre en su propio hilo (ver `stream_watch::run_decode_thread`): decodificar
//! y escalar es CPU pesado y no puede bloquear ni el runtime de red ni la UI.

use ffmpeg_the_third as ffmpeg;

use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{context::Context as ScalerContext, flag::Flags};
use ffmpeg::util::frame::video::Video;

/// Ancho máximo del frame que se entrega a la UI. Un stream 1080p/1440p se
/// reduce acá: la textura pesa menos y el visor nunca lo muestra más grande.
const MAX_OUTPUT_WIDTH: u32 = 1280;

/// Un frame decodificado, RGBA (4 bytes por píxel, sin padding entre filas).
pub(super) struct DecodedVideoFrame {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
}

struct CachedScaler {
    source_format: Pixel,
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
    context: ScalerContext,
}

pub(super) struct H264Decoder {
    decoder: ffmpeg::decoder::Video,
    scaler: Option<CachedScaler>,
}

impl H264Decoder {
    pub(super) fn new() -> Result<Self, String> {
        ffmpeg::init().map_err(|error| format!("ffmpeg init failed: {error}"))?;
        let codec = ffmpeg::decoder::find(ffmpeg::codec::Id::H264)
            .ok_or_else(|| "ffmpeg has no H.264 decoder available".to_owned())?;
        let context = ffmpeg::codec::context::Context::new_with_codec(codec);
        let decoder = context
            .decoder()
            .video()
            .map_err(|error| format!("could not open the H.264 decoder: {error}"))?;
        Ok(Self {
            decoder,
            scaler: None,
        })
    }

    /// Decodifica un access unit y devuelve los frames que salieron (casi
    /// siempre 0 o 1: el decoder puede necesitar más datos antes de emitir).
    pub(super) fn decode(&mut self, access_unit: &[u8]) -> Result<Vec<DecodedVideoFrame>, String> {
        let packet = ffmpeg::Packet::copy(access_unit);
        self.decoder
            .send_packet(&packet)
            .map_err(|error| format!("H.264 send_packet failed: {error}"))?;

        let mut frames = Vec::new();
        loop {
            let mut decoded = Video::empty();
            // EAGAIN ("necesito más datos") y EOF terminan el ciclo igual
            // que cualquier otro error: no hay nada más que sacar ahora.
            if self.decoder.receive_frame(&mut decoded).is_err() {
                break;
            }
            frames.push(self.to_rgba(&decoded)?);
        }
        Ok(frames)
    }

    fn to_rgba(&mut self, frame: &Video) -> Result<DecodedVideoFrame, String> {
        let source_width = frame.width();
        let source_height = frame.height();
        if source_width == 0 || source_height == 0 {
            return Err("decoded H.264 frame has no size".to_owned());
        }
        let (output_width, output_height) = output_size(source_width, source_height);
        let source_format = frame.format();

        let needs_new_scaler = match &self.scaler {
            Some(cached) => {
                cached.source_format != source_format
                    || cached.source_width != source_width
                    || cached.source_height != source_height
            }
            None => true,
        };
        if needs_new_scaler {
            let context = ScalerContext::get(
                source_format,
                source_width,
                source_height,
                Pixel::RGBA,
                output_width,
                output_height,
                Flags::BILINEAR,
            )
            .map_err(|error| format!("could not create the video scaler: {error}"))?;
            self.scaler = Some(CachedScaler {
                source_format,
                source_width,
                source_height,
                output_width,
                output_height,
                context,
            });
        }
        let Some(scaler) = self.scaler.as_mut() else {
            return Err("video scaler missing".to_owned());
        };

        let mut rgba_frame = Video::empty();
        scaler
            .context
            .run(frame, &mut rgba_frame)
            .map_err(|error| format!("video scaling failed: {error}"))?;

        let width = scaler.output_width as usize;
        let height = scaler.output_height as usize;
        let stride = rgba_frame.stride(0);
        let data = rgba_frame.data(0);
        let row_bytes = width * 4;
        if stride < row_bytes || data.len() < stride * (height - 1) + row_bytes {
            return Err("scaled video frame has an unexpected layout".to_owned());
        }
        let mut rgba = Vec::with_capacity(row_bytes * height);
        for line in 0..height {
            let begin = line * stride;
            rgba.extend_from_slice(&data[begin..begin + row_bytes]);
        }
        // Por las dudas: la conversión YUV -> RGBA tiene que dejar el alfa en
        // 255, pero un frame con alfa 0 se vería totalmente transparente.
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        Ok(DecodedVideoFrame {
            width: scaler.output_width,
            height: scaler.output_height,
            rgba,
        })
    }
}

/// Tamaño de salida: el del stream si entra en `MAX_OUTPUT_WIDTH`, o
/// reducido manteniendo la proporción. Siempre par (lo pide el escalador de
/// algunos formatos YUV) y nunca menor a 2.
fn output_size(width: u32, height: u32) -> (u32, u32) {
    let (out_width, out_height) = if width <= MAX_OUTPUT_WIDTH {
        (width, height)
    } else {
        let scaled = u64::from(height) * u64::from(MAX_OUTPUT_WIDTH) / u64::from(width);
        (MAX_OUTPUT_WIDTH, scaled as u32)
    };
    ((out_width & !1).max(2), (out_height & !1).max(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_streams_keep_their_size_rounded_to_even() {
        assert_eq!(output_size(640, 360), (640, 360));
        assert_eq!(output_size(641, 361), (640, 360));
    }

    #[test]
    fn large_streams_are_scaled_down_keeping_the_aspect_ratio() {
        assert_eq!(output_size(1920, 1080), (1280, 720));
        assert_eq!(output_size(2560, 1440), (1280, 720));
        assert_eq!(output_size(3440, 1440), (1280, 534));
    }

    #[test]
    fn degenerate_sizes_never_reach_zero() {
        assert_eq!(output_size(1, 1), (2, 2));
    }
}
