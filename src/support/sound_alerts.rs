//! Alertas sonoras de la app (aviso de seguridad, etc.), sin dependencias
//! extra: el MP3 se decodifica con `ffmpeg` (la misma crate que ya usan el
//! reproductor de video y el visor de streams) y se reproduce con `cpal`,
//! reutilizando los helpers de [`crate::support::audio_output`].
//!
//! # Agregar una alerta nueva
//!
//! 1. Dejar el archivo en `assets/sounds/` (cualquier formato que ffmpeg lea).
//! 2. Sumar una variante a [`SoundAlert`] y completar [`SoundAlert::bytes`],
//!    [`SoundAlert::file_name`] y, si hace falta, [`SoundAlert::gain`].
//! 3. Llamar a `sound_alerts::play(SoundAlert::LaNueva)` donde corresponda.
//!
//! # Cómo funciona
//!
//! * Los archivos van embebidos en el binario (`include_bytes!`), igual que
//!   las fuentes y los iconos de `theme.rs`, así que no dependen de la
//!   carpeta de trabajo.
//! * ffmpeg solo abre archivos por ruta, así que el audio se vuelca una vez a
//!   la carpeta de caché de la app (`paths::sound_cache_dir`).
//! * Todo corre en un hilo aparte: abrir el dispositivo y decodificar no debe
//!   congelar la UI. Si algo falla (sin dispositivo, archivo inválido, build
//!   sin `voice-playback`) solo se deja un log; el aviso visual no depende
//!   del sonido.
//! * Suena por el dispositivo de salida predeterminado del sistema.

/// Alertas sonoras disponibles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoundAlert {
    /// Aviso "Conexión no segura" (`ui::security_warning`).
    SecurityWarning,
}

impl SoundAlert {
    /// Contenido del archivo, embebido en el binario. La ruta es relativa a
    /// este archivo: `src/support/` → `../../assets/sounds/`.
    #[cfg_attr(not(feature = "voice-playback"), allow(dead_code))]
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::SecurityWarning => include_bytes!("../../assets/sounds/alert.mp3"),
        }
    }

    /// Nombre con el que se guarda en la caché (tiene que ser único por alerta
    /// y conservar la extensión, que ffmpeg usa para detectar el formato).
    #[cfg_attr(not(feature = "voice-playback"), allow(dead_code))]
    fn file_name(self) -> &'static str {
        match self {
            Self::SecurityWarning => "alert.mp3",
        }
    }

    /// Ganancia (1.0 = volumen original del archivo).
    #[cfg_attr(not(feature = "voice-playback"), allow(dead_code))]
    fn gain(self) -> f32 {
        match self {
            Self::SecurityWarning => 1.0,
        }
    }
}

/// Reproduce `alert` una vez sin bloquear al que llama.
pub fn play(alert: SoundAlert) {
    #[cfg(feature = "voice-playback")]
    {
        let spawned = std::thread::Builder::new()
            .name("sound-alert".into())
            .spawn(move || {
                if let Err(error) = imp::play_blocking(alert) {
                    log::warn!("No se pudo reproducir la alerta {alert:?}: {error}");
                }
            });
        if let Err(error) = spawned {
            log::warn!("No se pudo crear el hilo de la alerta {alert:?}: {error}");
        }
    }
    #[cfg(not(feature = "voice-playback"))]
    log::debug!("Alerta {alert:?} omitida: build sin el feature `voice-playback`");
}

#[cfg(feature = "voice-playback")]
mod imp {
    use std::{
        fs,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use ffmpeg_the_third as ffmpeg;
    use ffmpeg::{ChannelLayout, format::sample::Type as SampleType, media::Type as MediaType};

    use super::SoundAlert;
    use crate::support::{
        audio_output::{self, F32OutputSource},
        private_file,
    };

    /// Tope de seguridad: un archivo mal armado no puede llenar la memoria.
    const MAX_ALERT_SECONDS: usize = 30;
    /// Margen para que el dispositivo termine de vaciar su búfer antes de
    /// cerrar el stream (si no, se corta el final del sonido).
    const DRAIN_TAIL: Duration = Duration::from_millis(250);
    /// Pausa entre chequeos mientras se espera que termine de sonar.
    const POLL: Duration = Duration::from_millis(20);

    pub(super) fn play_blocking(alert: SoundAlert) -> Result<(), String> {
        // ALSA escribe sus errores directo a stderr; el mismo handler local
        // que usa la voz los captura mientras dura la reproducción.
        #[cfg(target_os = "linux")]
        let _alsa_errors = alsa::Output::local_error_handler().ok();

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no hay dispositivo de salida de audio".to_owned())?;
        let supported = device
            .default_output_config()
            .map_err(|error| format!("config de salida: {error}"))?;
        let sample_format = supported.sample_format();
        let config = supported.config();

        let path = materialize(alert)?;
        let max_samples = MAX_ALERT_SECONDS * config.sample_rate as usize * 2;
        let samples = decode_stereo_f32(&path, config.sample_rate, max_samples)?;
        if samples.is_empty() {
            return Err("el archivo no tiene audio".into());
        }

        let finished = Arc::new(AtomicBool::new(false));
        let total_frames = samples.len() / 2;
        let stream = audio_output::build_f32_output_stream(
            &device,
            &config,
            sample_format,
            AlertSource {
                samples,
                position: 0,
                gain: alert.gain(),
                finished: Arc::clone(&finished),
            },
            |error| {
                if !audio_output::is_recoverable_output_stream_error(&error) {
                    log::warn!("stream de la alerta: {error}");
                }
            },
            "alert audio output",
            "alert audio",
        )?;
        stream
            .play()
            .map_err(|error| format!("no se pudo iniciar el stream: {error}"))?;

        // Duración esperada + holgura: si el dispositivo se cuelga y nunca
        // pide muestras, el hilo igual termina.
        let expected = Duration::from_secs_f64(total_frames as f64 / f64::from(config.sample_rate));
        let deadline = Instant::now() + expected + Duration::from_secs(2);
        while !finished.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        std::thread::sleep(DRAIN_TAIL);
        Ok(())
    }

    /// ffmpeg abre por ruta, así que el audio embebido se escribe una vez en
    /// la caché. Si ya está idéntico no se vuelve a escribir.
    fn materialize(alert: SoundAlert) -> Result<PathBuf, String> {
        let dir = crate::paths::sound_cache_dir()
            .ok_or_else(|| "no se pudo resolver la carpeta de caché".to_owned())?;
        fs::create_dir_all(&dir).map_err(|error| format!("crear {}: {error}", dir.display()))?;
        let _ = private_file::set_private_dir_permissions(&dir);

        let path = dir.join(alert.file_name());
        let bytes = alert.bytes();
        if fs::read(&path).is_ok_and(|current| current == bytes) {
            return Ok(path);
        }
        private_file::write_private_file(&path, bytes)
            .map_err(|error| format!("escribir {}: {error}", path.display()))?;
        Ok(path)
    }

    /// Decodifica el primer stream de audio de `path` y lo deja como f32
    /// intercalado estéreo a `out_rate` Hz (`[L, R, L, R, ...]`), cortado a
    /// `max_samples` muestras.
    fn decode_stereo_f32(
        path: &std::path::Path,
        out_rate: u32,
        max_samples: usize,
    ) -> Result<Vec<f32>, String> {
        let err = |context: &str, error: ffmpeg::Error| format!("{context}: {error}");

        ffmpeg::init().map_err(|e| err("ffmpeg init", e))?;
        let mut input = ffmpeg::format::input(path).map_err(|e| err("abrir audio", e))?;

        // El bloque termina el préstamo de `input` antes de pedir los paquetes.
        let (stream_index, mut decoder) = {
            let stream = input
                .streams()
                .find(|stream| stream.parameters().medium() == MediaType::Audio)
                .ok_or_else(|| "el archivo no tiene stream de audio".to_owned())?;
            let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
                .map_err(|e| err("contexto del decoder", e))?
                .decoder()
                .audio()
                .map_err(|e| err("decoder de audio", e))?;
            (stream.index(), decoder)
        };

        let mut resampler = ffmpeg::software::resampling::context::Context::get2(
            decoder.format(),
            decoder.ch_layout(),
            decoder.rate(),
            ffmpeg::format::Sample::F32(SampleType::Packed),
            ChannelLayout::STEREO,
            out_rate,
        )
        .map_err(|e| err("resampler", e))?;

        let mut samples: Vec<f32> = Vec::new();

        for packet in input.packets() {
            let (stream, packet) = packet.map_err(|e| err("leer paquete", e))?;
            if stream.index() != stream_index {
                continue;
            }
            decoder
                .send_packet(&packet)
                .map_err(|e| err("enviar paquete", e))?;
            drain_decoder(&mut decoder, &mut resampler, &mut samples, max_samples)?;
            if samples.len() >= max_samples {
                break;
            }
        }
        // Vaciar lo que quedó en el decoder y en el resampler.
        let _ = decoder.send_eof();
        drain_decoder(&mut decoder, &mut resampler, &mut samples, max_samples)?;
        let mut tail = ffmpeg::frame::Audio::empty();
        if resampler.flush(&mut tail).is_ok() {
            append_packed_f32(&tail, &mut samples, max_samples);
        }

        // Estéreo intercalado: siempre un número par de muestras.
        samples.truncate(samples.len() & !1);
        Ok(samples)
    }

    fn drain_decoder(
        decoder: &mut ffmpeg::decoder::Audio,
        resampler: &mut ffmpeg::software::resampling::Context,
        samples: &mut Vec<f32>,
        max_samples: usize,
    ) -> Result<(), String> {
        let mut decoded = ffmpeg::frame::Audio::empty();
        // `receive_frame` devuelve error cuando no hay más frames listos
        // (EAGAIN) o se llegó al final (EOF): en ambos casos se corta.
        while decoder.receive_frame(&mut decoded).is_ok() {
            let mut resampled = ffmpeg::frame::Audio::empty();
            resampler
                .run(&decoded, &mut resampled)
                .map_err(|error| format!("remuestreo: {error}"))?;
            append_packed_f32(&resampled, samples, max_samples);
            if samples.len() >= max_samples {
                break;
            }
        }
        Ok(())
    }

    /// Agrega las muestras de un frame f32 *packed* estéreo (misma lectura que
    /// `egui-video`: `data[0]` con `samples * canales` valores).
    fn append_packed_f32(frame: &ffmpeg::frame::Audio, out: &mut Vec<f32>, max_samples: usize) {
        let count = frame.samples() * frame.ch_layout().channels() as usize;
        if count == 0 || !frame.is_packed() {
            return;
        }
        // SAFETY: el resampler se creó con salida F32 packed estéreo, así que
        // `data[0]` apunta a `samples * 2` f32 válidos mientras `frame` viva.
        let data = unsafe { std::slice::from_raw_parts((*frame.as_ptr()).data[0] as *const f32, count) };
        let room = max_samples.saturating_sub(out.len());
        out.extend_from_slice(&data[..count.min(room)]);
    }

    /// Fuente de muestras para el callback de cpal.
    struct AlertSource {
        /// f32 estéreo intercalado.
        samples: Vec<f32>,
        position: usize,
        gain: f32,
        finished: Arc<AtomicBool>,
    }

    impl F32OutputSource for AlertSource {
        fn fill<T>(&mut self, output: &mut [T], channels: usize, convert: fn(f32) -> T)
        where
            T: Default + Copy,
        {
            let channels = channels.max(1);
            for frame in output.chunks_mut(channels) {
                let (left, right) = match self.samples.get(self.position..self.position + 2) {
                    Some(&[l, r]) => {
                        self.position += 2;
                        (l * self.gain, r * self.gain)
                    }
                    _ => {
                        self.finished.store(true, Ordering::Release);
                        (0.0, 0.0)
                    }
                };
                for (index, slot) in frame.iter_mut().enumerate() {
                    let value = match (channels, index) {
                        (1, _) => (left + right) * 0.5,
                        (_, 0) => left,
                        (_, 1) => right,
                        _ => 0.0,
                    };
                    // `convert(0.0)` y no `T::default()`: en formatos sin
                    // signo el silencio está a mitad de escala.
                    *slot = convert(value);
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::support::audio_output::f32_sample_to_i16;

        fn source(samples: Vec<f32>) -> AlertSource {
            AlertSource {
                samples,
                position: 0,
                gain: 1.0,
                finished: Arc::new(AtomicBool::new(false)),
            }
        }

        #[test]
        fn plays_stereo_then_goes_silent_and_flags_finished() {
            let mut src = source(vec![0.5, -0.5]);
            let mut out = [0i16; 4];
            src.fill(&mut out, 2, f32_sample_to_i16);
            assert_eq!(out[0], f32_sample_to_i16(0.5));
            assert_eq!(out[1], f32_sample_to_i16(-0.5));
            assert_eq!(&out[2..], &[0, 0]);
            assert!(src.finished.load(Ordering::Acquire));
        }

        #[test]
        fn downmixes_to_mono_and_pads_extra_channels() {
            let mut mono = source(vec![1.0, 0.0]);
            let mut out = [0i16; 1];
            mono.fill(&mut out, 1, f32_sample_to_i16);
            assert_eq!(out[0], f32_sample_to_i16(0.5));

            let mut surround = source(vec![0.25, 0.75]);
            let mut out = [1i16; 4];
            surround.fill(&mut out, 4, f32_sample_to_i16);
            assert_eq!(out[2], 0);
            assert_eq!(out[3], 0);
        }
    }
}
