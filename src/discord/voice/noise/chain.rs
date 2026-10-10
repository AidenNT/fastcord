//! Cadena de limpieza de micrófono portada de "Clean Mic" (simple-clean-mic, MIT):
//! pasa-altos -> RNNoise -> compuerta de voz -> volumen automático -> EQ ->
//! compresor -> limitador. Todo a 48 kHz mono (10 ms = 480 muestras).
//! Cada etapa se puede apagar y ajustar con `CleanMicSettings` (popup de Clean Mic).
#![allow(dead_code)]

use nnnoiseless::DenoiseState;

use crate::discord::clean_mic::{CleanMicSettings, EQ_BANDS, EqBand};
use std::f32::consts::PI;

pub const SAMPLE_RATE: f32 = 48_000.0;
pub const FRAME: usize = DenoiseState::FRAME_SIZE; // 480 samples = 10 ms

fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// One-pole smoothing coefficient for a time constant in milliseconds (per sample).
fn coef(ms: f32) -> f32 {
    (-1.0 / (ms * 0.001 * SAMPLE_RATE)).exp()
}

// ---------------------------------------------------------------- biquad (RBJ cookbook)

#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn new(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: 0.0, z2: 0.0 }
    }

    fn highpass(freq: f32, q: f32) -> Self {
        let w = 2.0 * PI * freq / SAMPLE_RATE;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        Self::new((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    fn peaking(freq: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w = 2.0 * PI * freq / SAMPLE_RATE;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        Self::new(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a)
    }

    fn low_shelf(freq: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w = 2.0 * PI * freq / SAMPLE_RATE;
        let (s, c) = w.sin_cos();
        let alpha = s / 2.0 * std::f32::consts::SQRT_2; // shelf slope 1
        let sa = 2.0 * a.sqrt() * alpha;
        Self::new(
            a * ((a + 1.0) - (a - 1.0) * c + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sa),
            (a + 1.0) + (a - 1.0) * c + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sa,
        )
    }

    /// Take new coefficients but keep the filter memory, so live tweaks don't click.
    fn retune(&mut self, other: Biquad) {
        let (z1, z2) = (self.z1, self.z2);
        *self = other;
        self.z1 = z1;
        self.z2 = z2;
    }

    fn high_shelf(freq: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w = 2.0 * PI * freq / SAMPLE_RATE;
        let (s, c) = w.sin_cos();
        let alpha = s / 2.0 * std::f32::consts::SQRT_2; // shelf slope 1
        let sa = 2.0 * a.sqrt() * alpha;
        Self::new(
            a * ((a + 1.0) + (a - 1.0) * c + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sa),
            (a + 1.0) - (a - 1.0) * c + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sa,
        )
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        // transposed direct form II
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

// ---------------------------------------------------------------- user EQ

fn band_filter(i: usize, b: &EqBand) -> Biquad {
    let freq = b.freq.clamp(20.0, 20_000.0);
    let gain = b.gain.clamp(-18.0, 18.0);
    match i {
        0 => Biquad::low_shelf(freq, gain),
        i if i == EQ_BANDS - 1 => Biquad::high_shelf(freq, gain),
        _ => Biquad::peaking(freq, b.q.clamp(0.2, 8.0), gain),
    }
}

// ---------------------------------------------------------------- chain

pub struct Chain {
    hpf: [Biquad; 2],
    hpf_hz: f32,
    denoise: Box<DenoiseState<'static>>,
    denoise_out: Vec<f32>,
    eq: [Biquad; EQ_BANDS],

    // voice gate
    gate_gain: f32,
    gate_hold: usize,
    vad_smooth: f32,
    noise_floor: f32, // tracked background level before denoising (linear RMS)

    // auto gain
    speech_level: f32, // running estimate of speech RMS (linear)
    agc_gain: f32,

    // compressor / limiter envelopes
    comp_env: f32,
    lim_gain: f32,

    /// Nivel de entrada del último frame (dBFS) y si la compuerta estaba abierta,
    /// para el medidor del popup.
    pub last_level_db: f32,
    pub last_gate_open: bool,
}

impl Chain {
    pub fn new() -> Self {
        let defaults = CleanMicSettings::default();
        let mut chain = Self {
            hpf: [Biquad::highpass(80.0, 0.54), Biquad::highpass(80.0, 1.31)],
            hpf_hz: 80.0,
            denoise: DenoiseState::new(),
            denoise_out: vec![0.0; FRAME],
            eq: std::array::from_fn(|i| band_filter(i, &defaults.eq_bands[i])),
            gate_gain: 0.0,
            gate_hold: 0,
            vad_smooth: 0.0,
            noise_floor: 0.0,
            speech_level: db_to_lin(-30.0),
            agc_gain: 1.0,
            comp_env: 0.0,
            lim_gain: 1.0,
            last_level_db: -100.0,
            last_gate_open: false,
        };
        chain.configure(&defaults);
        chain
    }

    /// Aplica filtros y EQ nuevos conservando la memoria de cada filtro, así
    /// mover un slider en medio de una llamada no hace "clic".
    pub fn configure(&mut self, s: &CleanMicSettings) {
        let hz = s.high_pass_hz.clamp(20.0, 400.0);
        if (hz - self.hpf_hz).abs() > 0.01 {
            self.hpf[0].retune(Biquad::highpass(hz, 0.54));
            self.hpf[1].retune(Biquad::highpass(hz, 1.31));
            self.hpf_hz = hz;
        }
        for (i, (filter, band)) in self.eq.iter_mut().zip(s.eq_bands.iter()).enumerate() {
            filter.retune(band_filter(i, band));
        }
    }

    /// Process exactly one FRAME of 48 kHz mono audio in place (samples in -1..1).
    /// Returns the voice-activity probability for the frame.
    pub fn process_frame(&mut self, buf: &mut [f32], s: &CleanMicSettings) -> f32 {
        debug_assert_eq!(buf.len(), FRAME);

        // 1. high-pass, then scale to the i16 range RNNoise expects
        let mut energy = 0.0;
        for x in buf.iter_mut() {
            let mut y = *x;
            if s.high_pass {
                for f in self.hpf.iter_mut() {
                    y = f.process(y);
                }
            }
            energy += y * y;
            *x = y * 32768.0;
        }
        let level = (energy / FRAME as f32).sqrt().max(1e-7);
        self.last_level_db = 20.0 * level.log10();

        // track the background level: falls fast, rises ~10 dB/s (so speech bursts barely move it)
        if self.noise_floor == 0.0 || level < self.noise_floor {
            self.noise_floor = if self.noise_floor == 0.0 { level } else { self.noise_floor * 0.8 + level * 0.2 };
        } else {
            self.noise_floor *= 1.0116;
        }

        // 2. AI noise suppression
        let vad = self.denoise.process_frame(&mut self.denoise_out, buf);
        for (x, y) in buf.iter_mut().zip(self.denoise_out.iter()) {
            *x = *y / 32768.0;
        }

        // 3. voice gate: RNNoise's voice detector AND clearly louder than the background
        self.vad_smooth = self.vad_smooth * 0.6 + vad * 0.4;
        let talking = self.vad_smooth > s.gate_vad.clamp(0.05, 0.95)
            && level > self.noise_floor * db_to_lin(s.gate_margin_db.clamp(0.0, 30.0));
        if talking {
            self.gate_hold = (s.gate_hold_ms.clamp(0.0, 2000.0) / 10.0).round() as usize;
        } else if self.gate_hold > 0 {
            self.gate_hold -= 1;
        }
        let open = !s.gate || talking || self.gate_hold > 0;
        self.last_gate_open = open;
        let floor = db_to_lin(-6.0 - 54.0 * s.strength.clamp(0.0, 1.0));
        let gate_target = if open { 1.0 } else { floor };
        let gate_c = if open { coef(4.0) } else { coef(120.0) };

        // 4. auto gain: learn speech loudness only while talking
        // target speech level: -30 dBFS (quiet) .. -24 (normal) .. -18 (loud)
        let target_rms = db_to_lin(-30.0 + 12.0 * s.loudness.clamp(0.0, 1.0));
        if talking {
            let rms = (buf.iter().map(|x| x * x).sum::<f32>() / FRAME as f32).sqrt();
            if rms > db_to_lin(-60.0) {
                // ~1.5 s memory, measured in frames of 10 ms
                self.speech_level = self.speech_level * 0.993 + rms * 0.007;
            }
        }
        let wanted = if s.agc {
            (target_rms / self.speech_level).clamp(db_to_lin(-12.0), db_to_lin(15.0))
        } else {
            1.0
        };
        let agc_c = coef(400.0);

        // compressor: threshold and ratio from the settings (tames peaks)
        let comp_thresh = db_to_lin(s.compressor_threshold_db.clamp(-60.0, 0.0));
        let comp_slope = -(1.0 - 1.0 / s.compressor_ratio.clamp(1.0, 20.0));
        let comp_att = coef(5.0);
        let comp_rel = coef(120.0);

        // limiter: never exceed the ceiling
        let ceiling = db_to_lin(s.limiter_ceiling_db.clamp(-24.0, 0.0));
        let lim_rel = coef(60.0);

        for x in buf.iter_mut() {
            self.gate_gain = gate_target + gate_c * (self.gate_gain - gate_target);
            self.agc_gain = wanted + agc_c * (self.agc_gain - wanted);
            let mut y = *x * self.gate_gain * self.agc_gain;

            // 5. EQ
            if s.eq {
                for f in self.eq.iter_mut() {
                    y = f.process(y);
                }
            }

            // 6. compressor (peak envelope)
            if s.compressor {
                let level = y.abs();
                let c = if level > self.comp_env { comp_att } else { comp_rel };
                self.comp_env = level + c * (self.comp_env - level);
                if self.comp_env > comp_thresh {
                    y *= (self.comp_env / comp_thresh).powf(comp_slope);
                }
            }

            // 7. limiter: instant attack, smooth release
            if s.limiter {
                let peak = y.abs();
                let need = if peak > ceiling { ceiling / peak } else { 1.0 };
                let released = 1.0 + lim_rel * (self.lim_gain - 1.0);
                self.lim_gain = need.min(released);
                y *= self.lim_gain;
            }

            *x = y.clamp(-1.0, 1.0);
        }

        vad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic white noise in -amp..amp.
    fn noise(seed: &mut u32, amp: f32) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        (*seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
    }

    fn run(mut sample_fn: impl FnMut(usize) -> f32, seconds: usize) -> Vec<f32> {
        let mut chain = Chain::new();
        let s = CleanMicSettings::default();
        chain.configure(&s);
        let mut out = Vec::new();
        let mut buf = vec![0.0; FRAME];
        for f in 0..seconds * 100 {
            for (i, x) in buf.iter_mut().enumerate() {
                *x = sample_fn(f * FRAME + i);
            }
            chain.process_frame(&mut buf, &s);
            out.extend_from_slice(&buf);
        }
        out
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn background_noise_is_pushed_down() {
        let mut seed = 1;
        let out = run(|_| noise(&mut seed, 0.03), 3);
        let tail = &out[out.len() / 2..];
        assert!(tail.iter().all(|v| v.is_finite()));
        assert!(rms(tail) < 0.03 * 0.1, "noise rms {}", rms(tail));
    }

    #[test]
    fn loud_input_never_clips() {
        let mut seed = 7;
        let out = run(|i| (i as f32 * 0.05).sin() * 0.99 + noise(&mut seed, 0.3), 2);
        let peak = out.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(out.iter().all(|v| v.is_finite()));
        assert!(peak <= db_to_lin(-1.0) + 1e-4, "peak {peak}");
    }

    #[test]
    fn silence_stays_silent() {
        let out = run(|_| 0.0, 1);
        assert!(out.iter().all(|v| v.abs() < 1e-6));
    }
}
