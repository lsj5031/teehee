//! Bounded capture-side PCM ring for the sender encode loop.
//!
//! ## Why this exists
//!
//! The previous sender used an unbounded `Vec<f32>` filled by the cpal /
//! WASAPI callback and drained at most one `--chunk-ms` packet per
//! wall-clock tick. Any stall or post-idle burst left a permanent
//! backlog → permanent lag at the receiver.
//!
//! This ring:
//! * caps depth at `--capture-buffer-ms` (drop-oldest on overrun)
//! * exposes depth / high-water / overrun counters for `--stats` / JSONL
//! * tells the encode loop when to **catch up** (burst-send, no sleep)

/// Default capture buffer depth in ms of audio. Large enough for a
/// WASAPI 100 ms period + scheduling jitter; small enough that lag
/// cannot silently grow into multi-second delay.
pub const CAPTURE_BUFFER_DEFAULT_MS: usize = 200;

/// Minimum capture buffer (must hold at least a few chunks).
pub const CAPTURE_BUFFER_MIN_MS: usize = 40;

/// Maximum capture buffer (memory / lag ceiling).
pub const CAPTURE_BUFFER_MAX_MS: usize = 5_000;

/// Snapshot of capture-ring diagnostics for stats / JSONL.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CaptureRingStats {
    /// Current fill in interleaved f32 samples.
    pub samples: usize,
    /// Current fill converted to ms of audio at the ring's rate/channels.
    pub depth_ms: u64,
    /// Peak fill observed (ms) since construction / last reset.
    pub high_water_ms: u64,
    /// Cumulative samples dropped because the ring was full.
    pub overruns: u64,
    /// Capacity in samples.
    pub capacity_samples: usize,
}

/// Bounded interleaved-f32 capture buffer.
pub struct CaptureRing {
    samples: Vec<f32>,
    capacity_samples: usize,
    sample_rate: u32,
    channels: u8,
    /// Fill level (samples) that triggers catch-up mode on the encode loop.
    high_water_samples: usize,
    overruns: u64,
    high_water_samples_seen: usize,
}

impl CaptureRing {
    /// Build a ring sized for `buffer_ms` of audio at the given format.
    ///
    /// Capacity is at least `4 * chunk_samples` so a short chunk-ms never
    /// starves the floor, and at most the ms-derived sample count.
    pub fn new(buffer_ms: usize, sample_rate: u32, channels: u8, chunk_samples: usize) -> Self {
        assert!(sample_rate > 0);
        assert!(channels > 0);
        assert!(chunk_samples > 0);
        let ms = buffer_ms.clamp(CAPTURE_BUFFER_MIN_MS, CAPTURE_BUFFER_MAX_MS);
        let from_ms = (ms as u64)
            .saturating_mul(sample_rate as u64)
            .saturating_mul(channels as u64)
            / 1000;
        let capacity_samples = (from_ms as usize).max(chunk_samples.saturating_mul(4));
        // Catch-up when more than ~2 chunks or 50 ms is queued — whichever
        // is larger. Keeps steady-state paced; recovers after bursts.
        let fifty_ms = (50u64)
            .saturating_mul(sample_rate as u64)
            .saturating_mul(channels as u64)
            / 1000;
        let high_water_samples = chunk_samples
            .saturating_mul(2)
            .max(fifty_ms as usize)
            .min(capacity_samples.saturating_sub(1).max(chunk_samples));
        Self {
            samples: Vec::with_capacity(capacity_samples.min(capacity_samples)),
            capacity_samples,
            sample_rate,
            channels,
            high_water_samples,
            overruns: 0,
            high_water_samples_seen: 0,
        }
    }

    /// Append captured PCM. Drops oldest samples if over capacity.
    pub fn push(&mut self, data: &[f32]) {
        if data.is_empty() {
            return;
        }
        // Fast path: fits entirely.
        if self.samples.len() + data.len() <= self.capacity_samples {
            self.samples.extend_from_slice(data);
        } else {
            // Need room: drop oldest, then append (may still truncate data
            // if a single callback exceeds capacity).
            let need = data.len();
            if need >= self.capacity_samples {
                // Whole callback larger than ring — keep only the tail.
                let start = need - self.capacity_samples;
                let dropped = self.samples.len() + start;
                self.overruns = self.overruns.saturating_add(dropped as u64);
                self.samples.clear();
                self.samples.extend_from_slice(&data[start..]);
            } else {
                let overflow = self.samples.len() + need - self.capacity_samples;
                self.overruns = self.overruns.saturating_add(overflow as u64);
                if overflow >= self.samples.len() {
                    self.samples.clear();
                    let keep_from = need - self.capacity_samples.min(need);
                    self.samples.extend_from_slice(&data[keep_from..]);
                } else {
                    self.samples.drain(..overflow);
                    self.samples.extend_from_slice(data);
                }
            }
        }
        if self.samples.len() > self.high_water_samples_seen {
            self.high_water_samples_seen = self.samples.len();
        }
    }

    /// Pop exactly `chunk_samples` if available.
    pub fn pop_chunk(&mut self, chunk_samples: usize) -> Option<Vec<f32>> {
        if self.samples.len() < chunk_samples {
            return None;
        }
        Some(self.samples.drain(..chunk_samples).collect())
    }

    /// Drain all buffered samples. Used on pause/resume so stale
    /// audio captured during the pause is not replayed.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Current sample count.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// True when the encode loop should burst-send (skip inter-packet sleep).
    pub fn should_catch_up(&self) -> bool {
        self.samples.len() > self.high_water_samples
    }

    pub fn capacity_samples(&self) -> usize {
        self.capacity_samples
    }

    pub fn high_water_samples(&self) -> usize {
        self.high_water_samples
    }

    fn samples_to_ms(&self, n: usize) -> u64 {
        if self.sample_rate == 0 || self.channels == 0 {
            return 0;
        }
        (n as u64 * 1000) / (self.sample_rate as u64 * self.channels as u64)
    }

    pub fn stats(&self) -> CaptureRingStats {
        CaptureRingStats {
            samples: self.samples.len(),
            depth_ms: self.samples_to_ms(self.samples.len()),
            high_water_ms: self.samples_to_ms(self.high_water_samples_seen),
            overruns: self.overruns,
            capacity_samples: self.capacity_samples,
        }
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    fn ring_48k_stereo_20ms_chunk(buffer_ms: usize) -> CaptureRing {
        // 20 ms @ 48k stereo = 1920 samples
        CaptureRing::new(buffer_ms, 48_000, 2, 1_920)
    }

    #[test]
    fn push_pop_round_trip() {
        let mut r = ring_48k_stereo_20ms_chunk(200);
        let chunk: Vec<f32> = (0..1920).map(|i| i as f32).collect();
        r.push(&chunk);
        assert_eq!(r.len(), 1920);
        let out = r.pop_chunk(1920).unwrap();
        assert_eq!(out, chunk);
        assert!(r.is_empty());
    }

    #[test]
    fn overrun_drops_oldest_and_counts() {
        // Tiny ring: capacity floor = 4 * chunk = 400 samples.
        let mut r = CaptureRing::new(40, 1_000, 1, 100);
        let cap = r.capacity_samples();
        assert!(cap >= 400);
        r.push(&vec![1.0; cap]);
        assert_eq!(r.len(), cap);
        r.push(&[2.0; 50]);
        assert_eq!(r.len(), cap);
        assert!(r.stats().overruns >= 50);
        // Newest samples sit at the tail — pop until empty and require
        // at least one 2.0 was retained.
        let mut saw_two = false;
        while r.len() >= 50 {
            let c = r.pop_chunk(50).unwrap();
            if c.contains(&2.0) {
                saw_two = true;
            }
        }
        if !r.is_empty() {
            let rest = r.pop_chunk(r.len()).unwrap();
            if rest.contains(&2.0) {
                saw_two = true;
            }
        }
        assert!(saw_two, "newest samples must survive drop-oldest");
    }

    #[test]
    fn catch_up_when_above_high_water() {
        let mut r = ring_48k_stereo_20ms_chunk(200);
        assert!(!r.should_catch_up());
        // Push more than high-water (2 chunks or 50ms).
        r.push(&vec![0.0; r.high_water_samples() + 1]);
        assert!(r.should_catch_up());
    }

    #[test]
    fn depth_ms_matches_format() {
        let mut r = ring_48k_stereo_20ms_chunk(200);
        // 20 ms of audio = 1920 samples
        r.push(&vec![0.0; 1920]);
        assert_eq!(r.stats().depth_ms, 20);
    }
}
