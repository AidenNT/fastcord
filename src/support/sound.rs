//! Efectos de sonido de la app.
//!
//! Hoy hay uno: [`Sound::Alert`], el de los avisos urgentes (los de tipo
//! `ToastKind::Warning`, ver `App::push_toast`). El mp3 es
//! `assets/sounds/alert.mp3`: `build.rs` lo copia a `OUT_DIR` y se embebe en
//! el binario. Si el archivo no está, la app compila igual y no suena nada.
//!
//! Se decodifica una sola vez (symphonia, Rust puro) y se reproduce por el
//! mismo `cpal` que usa la voz, en el dispositivo de salida por defecto, desde
//! un hilo corto por cada sonido para no bloquear la UI. Sin el feature
//! `voice-playback` (sin `cpal`) todo esto es un no-op.

use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug)]
pub enum Sound {
    /// Aviso urgente.
    Alert,
}

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Prende o apaga todos los efectos de sonido (para un futuro ajuste de
/// "Sonidos de la app").
#[allow(dead_code)]
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Reproduce `sound` sin bloquear. Si suena otro hace menos de ~1,5 s se
/// ignora, para que una ráfaga de avisos (p. ej. reconexiones seguidas) no
/// se convierta en una ametralladora.
pub fn play(sound: Sound) {
    if ENABLED.load(Ordering::Relaxed) {
        imp::play(sound);
    }
}

#[cfg(not(feature = "voice-playback"))]
mod imp {
    pub fn play(_sound: super::Sound) {}
}

#[cfg(feature = "voice-playback")]
mod imp {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    use super::Sound;
    use crate::logging;
    use crate::support::audio_output::{self, F32OutputSource};

    /// Lo deja `build.rs` (vacío si falta `assets/sounds/alert.mp3`).
    static ALERT_MP3: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/alert.mp3"));

    /// Volumen de los efectos (0.0 a 1.0), por debajo de la voz.
    const VOLUME: f32 = 0.8;
    /// Mínimo entre dos sonidos.
    const MIN_GAP: Duration = Duration::from_millis(1500);

    struct Clip {
        rate: u32,
        channels: usize,
        /// Intercalado (L R L R… o solo mono).
        samples: Vec<f32>,
    }

    static ALERT: OnceLock<Option<Arc<Clip>>> = OnceLock::new();
    static LAST_PLAYED: Mutex<Option<Instant>> = Mutex::new(None);

    pub fn play(sound: Sound) {
        {
            let mut last = LAST_PLAYED.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            if last.is_some_and(|t| now.duration_since(t) < MIN_GAP) {
                return;
            }
            *last = Some(now);
        }
        let spawned = std::thread::Builder::new()
            .name("ecord-sound".into())
            .spawn(move || {
                let clip = match sound {
                    Sound::Alert => ALERT.get_or_init(|| decode(ALERT_MP3).map(Arc::new)).clone(),
                };
                let Some(clip) = clip else {
                    logging::debug("sound", "sin audio: falta assets/sounds/alert.mp3 o no se pudo decodificar");
                    return;
                };
                if let Err(error) = output(&clip) {
                    logging::debug("sound", format!("no se pudo reproducir el sonido: {error}"));
                }
            });
        if let Err(error) = spawned {
            logging::debug("sound", format!("no se pudo crear el hilo de sonido: {error}"));
        }
    }

    fn decode(bytes: &'static [u8]) -> Option<Clip> {
        if bytes.is_empty() {
            return None;
        }
        let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("mp3");
        let probed = symphonia::default::get_probe()
            .format(&hint, stream, &FormatOptions::default(), &MetadataOptions::default())
            .ok()?;
        let mut format = probed.format;
        let track = format.default_track()?;
        let track_id = track.id;
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .ok()?;

        let mut rate = 0u32;
        let mut channels = 0usize;
        let mut samples: Vec<f32> = Vec::new();
        loop {
            let Ok(packet) = format.next_packet() else { break };
            if packet.track_id() != track_id {
                continue;
            }
            match decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    rate = spec.rate;
                    channels = spec.channels.count();
                    let mut buffer =
                        symphonia::core::audio::SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                    buffer.copy_interleaved_ref(decoded);
                    samples.extend_from_slice(buffer.samples());
                }
                // Un frame corrupto no tira abajo todo el sonido.
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(_) => break,
            }
        }
        if samples.is_empty() || rate == 0 || channels == 0 {
            return None;
        }
        Some(Clip { rate, channels, samples })
    }

    fn output(clip: &Arc<Clip>) -> Result<(), String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no hay dispositivo de salida de audio".to_string())?;
        let supported = device
            .default_output_config()
            .map_err(|error| format!("config de salida: {error}"))?;
        let sample_format = supported.sample_format();
        let config = supported.config();

        let done = Arc::new(AtomicBool::new(false));
        let source = ClipSource {
            clip: Arc::clone(clip),
            out_rate: config.sample_rate,
            pos: 0.0,
            done: Arc::clone(&done),
        };
        let stream = audio_output::build_f32_output_stream(
            &device,
            &config,
            sample_format,
            source,
            |error| logging::debug("sound", format!("error en el stream de sonido: {error}")),
            "sound",
            "sound",
        )?;
        stream
            .play()
            .map_err(|error| format!("no arrancó el stream de sonido: {error}"))?;

        // Esperar a que termine (con tope por si el dispositivo se cuelga).
        let seconds = clip.samples.len() as f32 / clip.channels as f32 / clip.rate as f32;
        let deadline = Instant::now() + Duration::from_secs_f32(seconds + 0.5);
        while !done.load(Ordering::Relaxed) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Cola para que el buffer del dispositivo vacíe lo último.
        std::thread::sleep(Duration::from_millis(200));
        Ok(())
    }

    /// Reproduce un `Clip` con resampleo lineal a la tasa del dispositivo.
    struct ClipSource {
        clip: Arc<Clip>,
        out_rate: u32,
        /// Posición en frames del clip (fraccionaria).
        pos: f64,
        done: Arc<AtomicBool>,
    }

    impl F32OutputSource for ClipSource {
        fn fill<T>(&mut self, output: &mut [T], channels: usize, convert: fn(f32) -> T)
        where
            T: Default + Copy,
        {
            let clip = Arc::clone(&self.clip);
            let clip_channels = clip.channels.max(1);
            let frames = clip.samples.len() / clip_channels;
            let step = f64::from(clip.rate) / f64::from(self.out_rate.max(1));

            for frame in output.chunks_mut(channels.max(1)) {
                let index = self.pos as usize;
                if index >= frames {
                    for slot in frame.iter_mut() {
                        *slot = convert(0.0);
                    }
                    self.done.store(true, Ordering::Relaxed);
                    continue;
                }
                let next = (index + 1).min(frames - 1);
                let fraction = (self.pos - index as f64) as f32;
                for (channel, slot) in frame.iter_mut().enumerate() {
                    // Mono suena en L y R; estéreo, cada canal en el suyo;
                    // los canales extra de un parlante envolvente quedan mudos.
                    let value = if channel >= 2 {
                        0.0
                    } else {
                        let source = if clip_channels == 1 { 0 } else { channel.min(clip_channels - 1) };
                        let a = clip.samples[index * clip_channels + source];
                        let b = clip.samples[next * clip_channels + source];
                        ((a + (b - a) * fraction) * VOLUME).clamp(-1.0, 1.0)
                    };
                    *slot = convert(value);
                }
                self.pos += step;
            }
        }
    }
}
