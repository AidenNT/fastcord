use nnnoiseless::DenoiseState;

use super::DISCORD_OPUS_20MS_STEREO_SAMPLES;

const RNNOISE_FRAMES_PER_OPUS_FRAME: usize = 2;

pub(super) struct VoiceNoiseSuppressor {
    state: Box<DenoiseState<'static>>,
    input: [f32; DenoiseState::FRAME_SIZE],
    output: [f32; DenoiseState::FRAME_SIZE],
}

impl VoiceNoiseSuppressor {
    pub(super) fn new() -> Self {
        let mut suppressor = Self {
            state: DenoiseState::new(),
            input: [0.0; DenoiseState::FRAME_SIZE],
            output: [0.0; DenoiseState::FRAME_SIZE],
        };
        suppressor.prime();
        suppressor
    }

    pub(super) fn reset(&mut self) {
        self.state = DenoiseState::new();
        self.input.fill(0.0);
        self.output.fill(0.0);
        self.prime();
    }

    pub(super) fn process_20ms_stereo(&mut self, samples: &mut [i16]) -> bool {
        if samples.len() != DISCORD_OPUS_20MS_STEREO_SAMPLES {
            return false;
        }

        for frame_index in 0..RNNOISE_FRAMES_PER_OPUS_FRAME {
            let mono_start = frame_index * DenoiseState::FRAME_SIZE;
            for sample_index in 0..DenoiseState::FRAME_SIZE {
                let stereo_index = (mono_start + sample_index) * 2;
                self.input[sample_index] =
                    (f32::from(samples[stereo_index]) + f32::from(samples[stereo_index + 1])) * 0.5;
            }

            self.state.process_frame(&mut self.output, &self.input);

            // Voice capture is treated as mono even when the device supplies two
            // channels. Writing the same cleaned sample to both channels avoids
            // phase differences that can weaken speech during downmixing.
            for sample_index in 0..DenoiseState::FRAME_SIZE {
                let stereo_index = (mono_start + sample_index) * 2;
                let sample = self.output[sample_index]
                    .round()
                    .clamp(f32::from(i16::MIN), f32::from(i16::MAX))
                    as i16;
                samples[stereo_index] = sample;
                samples[stereo_index + 1] = sample;
            }
        }

        true
    }

    fn prime(&mut self) {
        // RNNoise documents a fade-in artifact on its first output frame.
        // Processing silence once keeps that artifact out of the first live frame.
        self.state.process_frame(&mut self.output, &self.input);
    }
}

