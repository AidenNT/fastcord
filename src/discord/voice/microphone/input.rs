use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
    },
    time::{Duration, Instant},
};

use super::VOICE_MIC_QUEUE_BUDGET;

const MAX_TIMESTAMP_JITTER: Duration = Duration::from_millis(20);

pub(super) struct InputChunk<T: Send> {
    pub(super) samples: Vec<T>,
    pub(super) captured_at: Instant,
    pub(super) discontinuity: bool,
    epoch: u64,
    recycle_tx: SyncSender<Vec<T>>,
}

impl<T: Send> Drop for InputChunk<T> {
    fn drop(&mut self) {
        let mut samples = std::mem::take(&mut self.samples);
        if samples.capacity() == 0 {
            return;
        }
        samples.clear();
        let _ = self.recycle_tx.try_send(samples);
    }
}

pub(super) struct InputSender<T: Copy + Send> {
    tx: SyncSender<InputChunk<T>>,
    recycle_rx: Receiver<Vec<T>>,
    recycle_tx: SyncSender<Vec<T>>,
    overflow_epoch: Arc<AtomicU64>,
    epoch: u64,
    block_samples: usize,
    capacity: usize,
    sample_rate: u32,
    channels: usize,
    pending: Option<Vec<T>>,
    pending_at: Option<Instant>,
    pending_discontinuity: bool,
}

pub(super) struct InputReceiver<T: Send> {
    rx: Receiver<InputChunk<T>>,
    overflow_epoch: Arc<AtomicU64>,
    observed_epoch: u64,
    capacity: usize,
}

pub(super) fn channel<T: Copy + Send>(
    sample_rate: u32,
    channels: usize,
) -> (InputSender<T>, InputReceiver<T>) {
    let sample_rate = sample_rate.max(1);
    let channels = channels.max(1);
    let block_frames = usize::try_from(sample_rate.div_ceil(100)).unwrap_or(usize::MAX);
    let block_samples = block_frames.saturating_mul(channels).max(1);
    let block_duration = duration_for_audio_frames(block_frames, sample_rate);
    let capacity = queue_capacity(VOICE_MIC_QUEUE_BUDGET, block_duration);
    let (tx, rx) = mpsc::sync_channel(capacity);
    let (recycle_tx, recycle_rx) = mpsc::sync_channel(capacity.saturating_add(1));
    for _ in 0..capacity.saturating_add(1) {
        recycle_tx
            .try_send(Vec::with_capacity(block_samples))
            .expect("voice microphone input buffer pool has capacity");
    }
    let pending = recycle_rx
        .try_recv()
        .expect("input buffer pool is populated");
    let overflow_epoch = Arc::new(AtomicU64::new(0));

    (
        InputSender {
            tx,
            recycle_rx,
            recycle_tx,
            overflow_epoch: Arc::clone(&overflow_epoch),
            epoch: 0,
            block_samples,
            capacity,
            sample_rate,
            channels,
            pending: Some(pending),
            pending_at: None,
            pending_discontinuity: false,
        },
        InputReceiver {
            rx,
            overflow_epoch,
            observed_epoch: 0,
            capacity,
        },
    )
}

impl<T: Copy + Send> InputSender<T> {
    pub(super) fn push(&mut self, input: &[T], captured_at: Instant) -> u64 {
        if input.is_empty() {
            return 0;
        }
        let max_callback_samples = self.capacity.saturating_mul(self.block_samples);
        let skipped_samples = input.len().saturating_sub(max_callback_samples);
        let (input, captured_at, mut dropped_blocks) = if skipped_samples > 0 {
            if let Some(pending) = &mut self.pending {
                pending.clear();
            }
            self.pending_at = None;
            self.pending_discontinuity = true;
            (
                &input[skipped_samples..],
                add_audio_duration(
                    captured_at,
                    skipped_samples / self.channels,
                    self.sample_rate,
                ),
                blocks_for_samples(skipped_samples, self.block_samples),
            )
        } else {
            (input, captured_at, 0)
        };
        if self.pending.is_none() {
            let Some(buffer) = self.take_buffer() else {
                self.begin_overflow();
                return dropped_blocks
                    .saturating_add(blocks_for_samples(input.len(), self.block_samples));
            };
            self.pending = Some(buffer);
        }
        self.align_partial(captured_at);

        let mut consumed = 0;
        while consumed < input.len() {
            let pending = self.pending.as_mut().expect("input buffer is available");
            if pending.is_empty() {
                self.pending_at = Some(add_audio_duration(
                    captured_at,
                    consumed / self.channels,
                    self.sample_rate,
                ));
            }

            let copied = (self.block_samples - pending.len()).min(input.len() - consumed);
            pending.extend_from_slice(&input[consumed..consumed + copied]);
            consumed += copied;
            if pending.len() < self.block_samples {
                break;
            }

            let samples = self
                .pending
                .take()
                .expect("completed input block is available");
            let chunk = InputChunk {
                samples,
                captured_at: self.pending_at.take().unwrap_or(captured_at),
                discontinuity: self.pending_discontinuity,
                epoch: self.epoch,
                recycle_tx: self.recycle_tx.clone(),
            };
            match self.tx.try_send(chunk) {
                Ok(()) => {
                    self.pending_discontinuity = false;
                    if consumed < input.len() {
                        let Some(buffer) = self.take_buffer() else {
                            self.begin_overflow();
                            return dropped_blocks.saturating_add(blocks_for_samples(
                                input.len() - consumed,
                                self.block_samples,
                            ));
                        };
                        self.pending = Some(buffer);
                    }
                }
                Err(TrySendError::Full(mut chunk)) => {
                    let dropped_samples = chunk.samples.len() + input.len() - consumed;
                    chunk.samples.clear();
                    self.pending = Some(std::mem::take(&mut chunk.samples));
                    self.pending_at = None;
                    self.begin_overflow();
                    dropped_blocks = dropped_blocks
                        .saturating_add(blocks_for_samples(dropped_samples, self.block_samples));
                    return dropped_blocks;
                }
                Err(TrySendError::Disconnected(mut chunk)) => {
                    let dropped_samples = chunk.samples.len() + input.len() - consumed;
                    chunk.samples.clear();
                    self.pending = Some(std::mem::take(&mut chunk.samples));
                    self.pending_at = None;
                    dropped_blocks = dropped_blocks
                        .saturating_add(blocks_for_samples(dropped_samples, self.block_samples));
                    return dropped_blocks;
                }
            }
        }
        dropped_blocks
    }

    fn align_partial(&mut self, captured_at: Instant) {
        let Some(pending_at) = self.pending_at else {
            return;
        };
        let pending = self.pending.as_mut().expect("partial input buffer exists");
        let expected_at =
            add_audio_duration(pending_at, pending.len() / self.channels, self.sample_rate);
        let timestamp_gap = if captured_at >= expected_at {
            captured_at.duration_since(expected_at)
        } else {
            expected_at.duration_since(captured_at)
        };
        if timestamp_gap > MAX_TIMESTAMP_JITTER {
            pending.clear();
            self.pending_at = None;
            self.pending_discontinuity = true;
        }
    }

    fn take_buffer(&mut self) -> Option<Vec<T>> {
        match self.recycle_rx.try_recv() {
            Ok(samples) if samples.capacity() >= self.block_samples => Some(samples),
            Ok(_) | Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    fn begin_overflow(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.clear();
        }
        self.pending_at = None;
        self.pending_discontinuity = true;
        self.epoch = self
            .overflow_epoch
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
    }
}

impl<T: Send> InputReceiver<T> {
    pub(super) fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<InputChunk<T>, RecvTimeoutError> {
        let started_at = Instant::now();
        let mut discarded = 0;
        loop {
            let remaining = timeout.saturating_sub(started_at.elapsed());
            let mut chunk = self.rx.recv_timeout(remaining)?;
            let overflow_epoch = self.overflow_epoch.load(Ordering::Acquire);
            if chunk.epoch < overflow_epoch {
                discarded += 1;
                if discarded >= self.capacity {
                    return Err(RecvTimeoutError::Timeout);
                }
                continue;
            }

            if overflow_epoch > self.observed_epoch {
                chunk.discontinuity = true;
                self.observed_epoch = overflow_epoch;
            }
            return Ok(chunk);
        }
    }
}

fn queue_capacity(budget: Duration, block_duration: Duration) -> usize {
    let budget_ns = budget.as_nanos();
    let block_ns = block_duration.as_nanos().max(1);
    let blocks = budget_ns.div_ceil(block_ns);
    usize::try_from(blocks)
        .unwrap_or(usize::MAX - 1)
        .saturating_add(1)
}

fn blocks_for_samples(samples: usize, block_samples: usize) -> u64 {
    u64::try_from(samples.div_ceil(block_samples)).unwrap_or(u64::MAX)
}

fn add_audio_duration(captured_at: Instant, frames: usize, sample_rate: u32) -> Instant {
    let duration = duration_for_audio_frames(frames, sample_rate);
    captured_at.checked_add(duration).unwrap_or(captured_at)
}

fn duration_for_audio_frames(frames: usize, sample_rate: u32) -> Duration {
    let numerator = (frames as u128).saturating_mul(Duration::from_secs(1).as_nanos());
    let nanos = numerator.div_ceil(u128::from(sample_rate));
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

