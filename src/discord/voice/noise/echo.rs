//! Cancelación de eco con WebRTC AEC3 (crate `sonora`, igual que Clean Mic).
//!
//! Recibe cada frame de 10 ms del micrófono y, en paralelo, lo que suena en los
//! parlantes (`echo_reference`), y le resta al micrófono lo que se escucha de
//! los parlantes. Corre siempre que la supresión de ruido esté activa, para que
//! el filtro se mantenga adaptado.

use sonora::config::EchoCanceller;
use sonora::{AudioProcessing, Config, StreamConfig};

use super::chain::FRAME;
use super::super::echo_reference;

const SAMPLE_RATE: u32 = 48_000;

pub(super) struct EchoCancel {
    apm: AudioProcessing,
    render_in: [f32; FRAME],
    render_out: [f32; FRAME],
    capture_in: [f32; FRAME],
    capture_out: [f32; FRAME],
}

impl EchoCancel {
    pub(super) fn new() -> Self {
        echo_reference::clear();
        let apm = AudioProcessing::builder()
            .config(Config {
                echo_canceller: Some(EchoCanceller::default()),
                ..Default::default()
            })
            .capture_config(StreamConfig::new(SAMPLE_RATE, 1))
            .render_config(StreamConfig::new(SAMPLE_RATE, 1))
            .build();
        Self {
            apm,
            render_in: [0.0; FRAME],
            render_out: [0.0; FRAME],
            capture_in: [0.0; FRAME],
            capture_out: [0.0; FRAME],
        }
    }

    /// Quita el eco de un frame de micrófono (mono, 48 kHz, -1..1) en el lugar.
    pub(super) fn process(&mut self, frame: &mut [f32; FRAME]) {
        echo_reference::pop_frame(&mut self.render_in);
        let _ = self
            .apm
            .process_render_f32(&[&self.render_in[..]], &mut [&mut self.render_out[..]]);
        self.capture_in.copy_from_slice(frame);
        if self
            .apm
            .process_capture_f32(&[&self.capture_in[..]], &mut [&mut self.capture_out[..]])
            .is_ok()
        {
            frame.copy_from_slice(&self.capture_out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_is_removed() {
        let mut echo = EchoCancel::new();
        let mut seed: u32 = 3;
        let mut speaker: Vec<f32> = Vec::new();
        let delay = 48 * 40; // 40 ms del parlante al micrófono
        let (mut before, mut after) = (0.0f32, 0.0f32);
        for f in 0..600 {
            let mut frame = [0.0f32; FRAME];
            let mut prev = 0.0;
            for x in frame.iter_mut() {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let w = seed as f32 / u32::MAX as f32 * 2.0 - 1.0;
                prev = prev * 0.7 + w * 0.3;
                *x = prev * 0.5;
            }
            speaker.extend_from_slice(&frame);
            echo_reference::push(&frame);

            let start = (f * FRAME) as isize - delay as isize;
            let mut mic = [0.0f32; FRAME];
            for (i, m) in mic.iter_mut().enumerate() {
                let idx = start + i as isize;
                if idx >= 0 {
                    *m = speaker[idx as usize] * 0.5;
                }
            }
            if f >= 500 {
                before += mic.iter().map(|x| x * x).sum::<f32>();
            }
            echo.process(&mut mic);
            if f >= 500 {
                after += mic.iter().map(|x| x * x).sum::<f32>();
            }
        }
        let db = 10.0 * (after / before).log10();
        assert!(db < -15.0, "el eco solo bajó {:.1} dB", -db);
    }
}
