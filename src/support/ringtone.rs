//! Tono de llamada entrante de los DMs/grupos.
//!
//! No hay un archivo de audio: el tono se sintetiza (dos notas que se repiten y
//! una pausa) y se reproduce en bucle con `cpal`, por el dispositivo de salida
//! predeterminado, hasta que se suelta el [`RingHandle`] (al atender, rechazar,
//! cortarse la llamada o activar "No molestar").
//!
//! Corre en un hilo aparte: abrir el dispositivo no debe congelar la UI. Si algo
//! falla (sin dispositivo de audio, build sin `voice-playback`) solo se deja un
//! log; la tarjeta de llamada entrante se ve igual.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Mientras exista, el tono suena. Al soltarlo (`drop`) se corta.
pub struct RingHandle {
    stop: Arc<AtomicBool>,
}

impl Drop for RingHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Empieza a sonar el tono en bucle sin bloquear al que llama.
pub fn start() -> RingHandle {
    let stop = Arc::new(AtomicBool::new(false));
    #[cfg(feature = "voice-playback")]
    {
        let thread_stop = Arc::clone(&stop);
        let spawned = std::thread::Builder::new().name("ecord-ringtone".into()).spawn(move || {
            if let Err(error) = imp::run(&thread_stop) {
                log::warn!("No se pudo reproducir el tono de llamada: {error}");
            }
        });
        if let Err(error) = spawned {
            log::warn!("No se pudo crear el hilo del tono de llamada: {error}");
        }
    }
    #[cfg(not(feature = "voice-playback"))]
    log::debug!("Tono de llamada omitido: build sin el feature `voice-playback`");
    RingHandle { stop }
}

/// Un período del tono: (inicio, frecuencia) de cada nota, en segundos / Hz.
#[cfg_attr(not(feature = "voice-playback"), allow(dead_code))]
const NOTES: [(f32, f32); 4] = [(0.00, 659.25), (0.22, 880.0), (0.62, 659.25), (0.84, 880.0)];
/// Duración de cada nota.
#[cfg_attr(not(feature = "voice-playback"), allow(dead_code))]
const NOTE_LEN: f32 = 0.30;
/// Largo del patrón completo (las notas + la pausa hasta el próximo ring).
#[cfg_attr(not(feature = "voice-playback"), allow(dead_code))]
const PERIOD: f32 = 3.0;
/// Volumen del tono (0.0 a 1.0).
#[cfg_attr(not(feature = "voice-playback"), allow(dead_code))]
const GAIN: f32 = 0.30;

/// Valor del tono `t` segundos después del inicio del patrón (0 en la pausa).
#[cfg_attr(not(feature = "voice-playback"), allow(dead_code))]
fn pattern_sample(t: f32) -> f32 {
    let mut out = 0.0;
    for (start, freq) in NOTES {
        let local = t - start;
        if !(0.0..NOTE_LEN).contains(&local) {
            continue;
        }
        // Ataque corto (sin clics) y caída exponencial, como una campanita.
        let attack = (local / 0.008).min(1.0);
        let decay = (-local * 7.0).exp();
        let phase = local * freq * std::f32::consts::TAU;
        out += (phase.sin() + 0.25 * (phase * 2.0).sin()) * attack * decay;
    }
    out * GAIN
}

#[cfg(feature = "voice-playback")]
mod imp {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    use super::{PERIOD, pattern_sample};
    use crate::support::audio_output::{self, F32OutputSource};

    pub(super) fn run(stop: &Arc<AtomicBool>) -> Result<(), String> {
        // ALSA escribe sus errores directo a stderr; el mismo handler local que
        // usa la voz los captura mientras suena.
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

        let source = RingSource {
            sample_rate: config.sample_rate as f32,
            frame: 0,
            stop: Arc::clone(stop),
        };
        let stream = audio_output::build_f32_output_stream(
            &device,
            &config,
            sample_format,
            source,
            |error| {
                if !audio_output::is_recoverable_output_stream_error(&error) {
                    log::warn!("stream del tono de llamada: {error}");
                }
            },
            "ringtone audio output",
            "ringtone audio",
        )?;
        stream
            .play()
            .map_err(|error| format!("no se pudo iniciar el stream: {error}"))?;

        while !stop.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(40));
        }
        drop(stream);
        Ok(())
    }

    /// Genera el tono muestra a muestra para el callback de cpal.
    struct RingSource {
        sample_rate: f32,
        frame: u64,
        stop: Arc<AtomicBool>,
    }

    impl F32OutputSource for RingSource {
        fn fill<T>(&mut self, output: &mut [T], channels: usize, convert: fn(f32) -> T)
        where
            T: Default + Copy,
        {
            let channels = channels.max(1);
            let silent = self.stop.load(Ordering::Relaxed);
            for frame in output.chunks_mut(channels) {
                let value = if silent {
                    0.0
                } else {
                    let t = (self.frame as f32 / self.sample_rate) % PERIOD;
                    self.frame += 1;
                    pattern_sample(t)
                };
                for slot in frame.iter_mut() {
                    *slot = convert(value);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pause_is_silent_and_the_notes_are_not() {
        assert_eq!(pattern_sample(2.0), 0.0);
        let loud = (0..2000)
            .map(|i| pattern_sample(i as f32 / 48_000.0).abs())
            .fold(0.0_f32, f32::max);
        assert!(loud > 0.05, "el primer tramo tiene que sonar, máximo {loud}");
        assert!(loud <= 1.0);
    }
}
