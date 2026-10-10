//! Supresión de ruido del micrófono ("Supresión de ruido" en Ajustes → Voz).
//!
//! Antes esto era solo RNNoise. Ahora pasa cada frame por la cadena completa de
//! Clean Mic (`chain.rs`): pasa-altos, RNNoise, compuerta de voz, volumen
//! automático, EQ, compresor y limitador (todo ajustable en vivo desde
//! `ui::clean_mic_popup`, vía `discord::clean_mic`), precedida por cancelación de eco
//! (AEC3, `echo.rs`). La API hacia `microphone.rs` no cambió.

mod chain;
mod echo;

use chain::{Chain, FRAME};
use echo::EchoCancel;

use crate::discord::clean_mic::{self, CleanMicSettings};

use super::DISCORD_OPUS_20MS_STEREO_SAMPLES;

const CHAIN_FRAMES_PER_OPUS_FRAME: usize = 2;
const _: () = assert!(FRAME * CHAIN_FRAMES_PER_OPUS_FRAME * 2 == DISCORD_OPUS_20MS_STEREO_SAMPLES);

pub(super) struct VoiceNoiseSuppressor {
    chain: Chain,
    echo: EchoCancel,
    settings: CleanMicSettings,
    settings_version: u64,
    mono: [f32; FRAME],
}

impl VoiceNoiseSuppressor {
    pub(super) fn new() -> Self {
        let mut suppressor = Self {
            chain: Chain::new(),
            echo: EchoCancel::new(),
            settings: CleanMicSettings::default(),
            // Fuerza la primera lectura de los ajustes guardados.
            settings_version: 0,
            mono: [0.0; FRAME],
        };
        suppressor.prime();
        suppressor
    }

    pub(super) fn reset(&mut self) {
        self.chain = Chain::new();
        self.chain.configure(&self.settings);
        self.mono.fill(0.0);
        self.prime();
    }

    pub(super) fn process_20ms_stereo(&mut self, samples: &mut [i16]) -> bool {
        if samples.len() != DISCORD_OPUS_20MS_STEREO_SAMPLES {
            return false;
        }

        self.refresh_settings();

        for frame_index in 0..CHAIN_FRAMES_PER_OPUS_FRAME {
            let mono_start = frame_index * FRAME;
            for sample_index in 0..FRAME {
                let stereo_index = (mono_start + sample_index) * 2;
                self.mono[sample_index] = (f32::from(samples[stereo_index])
                    + f32::from(samples[stereo_index + 1]))
                    * 0.5
                    / 32768.0;
            }

            // Primero el eco (necesita la señal cruda), después la cadena.
            if self.settings.echo_cancel {
                self.echo.process(&mut self.mono);
            }
            let voice = self.chain.process_frame(&mut self.mono, &self.settings);
            clean_mic::publish_meter(self.chain.last_level_db, voice, self.chain.last_gate_open);

            // La captura se trata como mono aunque el dispositivo entregue dos
            // canales: el mismo valor en ambos evita diferencias de fase.
            for sample_index in 0..FRAME {
                let stereo_index = (mono_start + sample_index) * 2;
                let sample = (self.mono[sample_index] * 32768.0)
                    .round()
                    .clamp(f32::from(i16::MIN), f32::from(i16::MAX))
                    as i16;
                samples[stereo_index] = sample;
                samples[stereo_index + 1] = sample;
            }
        }

        true
    }

    /// Si el popup cambió algo, relee los ajustes y reconfigura filtros y EQ.
    fn refresh_settings(&mut self) {
        let version = clean_mic::version();
        if version != self.settings_version {
            self.settings = clean_mic::current();
            self.chain.configure(&self.settings);
            self.settings_version = version;
        }
    }

    fn prime(&mut self) {
        // RNNoise tiene un artefacto de fade-in en su primer frame; procesar
        // silencio una vez lo deja fuera del primer frame real.
        self.refresh_settings();
        self.mono.fill(0.0);
        self.chain.process_frame(&mut self.mono, &self.settings);
    }
}
