//! Referencia para la cancelación de eco: copia (mono, 48 kHz) de lo que el
//! cliente manda a los parlantes. `playback.rs` la escribe desde el callback de
//! salida y `noise/echo.rs` la consume junto con cada frame del micrófono.
//!
//! Es una cola global con `try_lock` del lado de audio: el callback de salida
//! nunca se bloquea; si no consigue el lock, ese pedazo de referencia se pierde
//! (el AEC lo tolera).

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

const REFERENCE_RATE: u32 = 48_000;
/// Tope de la cola: 500 ms.
const MAX_QUEUED_SAMPLES: usize = REFERENCE_RATE as usize / 2;
/// Si el consumidor se atrasó más de 200 ms se salta casi todo (AEC3 re-encuentra el delay).
const MAX_BEHIND_SAMPLES: usize = REFERENCE_RATE as usize / 5;

fn queue() -> &'static Mutex<VecDeque<f32>> {
    static QUEUE: OnceLock<Mutex<VecDeque<f32>>> = OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_QUEUED_SAMPLES)))
}

pub(super) fn push(samples: &[f32]) {
    let Ok(mut queue) = queue().try_lock() else {
        return;
    };
    queue.extend(samples.iter().copied());
    let len = queue.len();
    if len > MAX_QUEUED_SAMPLES {
        queue.drain(..len - MAX_QUEUED_SAMPLES);
    }
}

pub(super) fn clear() {
    if let Ok(mut queue) = queue().lock() {
        queue.clear();
    }
}

/// Llena `out` con la próxima porción de referencia; si no hay suficiente
/// (parlantes en silencio), la completa con ceros.
pub(super) fn pop_frame(out: &mut [f32]) {
    out.fill(0.0);
    let Ok(mut queue) = queue().lock() else {
        return;
    };
    let len = queue.len();
    if len > MAX_BEHIND_SAMPLES {
        queue.drain(..len - out.len() * 2);
    }
    let frame_len = out.len();
    if queue.len() >= frame_len {
        for (slot, sample) in out.iter_mut().zip(queue.drain(..frame_len)) {
            *slot = sample;
        }
    }
}

/// Convierte lo que se reproduce (mono, a la frecuencia del dispositivo) a
/// 48 kHz con interpolación lineal y lo manda a la cola.
pub(super) struct EchoReferenceTap {
    step: f32,
    position: f32,
    previous: f32,
    passthrough: bool,
    scratch: Vec<f32>,
}

impl EchoReferenceTap {
    pub(super) fn new(device_sample_rate: u32) -> Self {
        Self {
            step: device_sample_rate.max(1) as f32 / REFERENCE_RATE as f32,
            position: 0.0,
            previous: 0.0,
            passthrough: device_sample_rate == REFERENCE_RATE,
            scratch: Vec::with_capacity(4096),
        }
    }

    pub(super) fn feed(&mut self, mono: f32) {
        if self.passthrough {
            self.scratch.push(mono);
            return;
        }
        while self.position < 1.0 {
            self.scratch
                .push(self.previous + (mono - self.previous) * self.position);
            self.position += self.step;
        }
        self.position -= 1.0;
        self.previous = mono;
    }

    /// Manda lo acumulado en este callback a la cola compartida.
    pub(super) fn flush(&mut self) {
        if !self.scratch.is_empty() {
            push(&self.scratch);
            self.scratch.clear();
        }
    }
}
