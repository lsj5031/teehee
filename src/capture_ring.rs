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
        let frames_from_ms = (ms as u64)
            .saturating_mul(sample_rate as u64)
            / 1000;
        // FIX-5: force capacity to a whole number of audio frames.
        // A non-frame-aligned capacity (e.g. 4851 samples for 55 ms
        // at 44.1 kHz stereo) causes drop-oldest to drain a
        // non-multiple-of-channels count, permanently swapping L/R
        // for the rest of the session.
        let ch = channels as usize;
        let capacity_samples =
            ((frames_from_ms as usize).saturating_mul(ch)).max(chunk_samples.saturating_mul(4));
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
                // FIX-5: round overflow drop to whole frames so
                // channel alignment is preserved. When overflow <
                // channels, aligned_overflow would be 0 — drop at
                // least one frame to stay within capacity.
                let ch = self.channels as usize;
                let aligned_overflow = if ch > 1 {
                    let aligned = (overflow / ch) * ch;
                    if aligned == 0 { ch.min(overflow) } else { aligned }
                } else {
                    overflow
                };
                if aligned_overflow >= self.samples.len() {
                    self.samples.clear();
                    let keep_from = need - self.capacity_samples.min(need);
                    self.samples.extend_from_slice(&data[keep_from..]);
                } else {
                    self.samples.drain(..aligned_overflow);
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

    // ── FIX-5: frame-aligned drops ────────────────────────────────

    #[test]
    fn capacity_is_frame_aligned_stereo() {
        // Every capacity for stereo (channels=2) must be even so
        // drop-oldest never swaps L/R.
        for buffer_ms in [40, 100, 200, 500, 1000, 2000] {
            let r = CaptureRing::new(buffer_ms, 48_000, 2, 1_920);
            assert_eq!(
                r.capacity_samples() % 2,
                0,
                "stereo capacity must be even at {buffer_ms} ms"
            );
        }
    }

    #[test]
    fn capacity_is_frame_aligned_for_non_aligned_ms() {
        // 55 ms at 44.1 kHz = 2425.5 frames → floor to 2425 frames
        // × 2 ch = 4850 samples (even). Before FIX-5, capacity was
        // frames_from_ms directly (= 2425, odd) → L/R swap on
        // first overrun.
        let r = CaptureRing::new(55, 44_100, 2, 960);
        assert_eq!(r.capacity_samples(), 4850, "pin exact value");
        assert_eq!(
            r.capacity_samples() % 2,
            0,
            "55 ms @ 44.1 kHz stereo must produce even capacity"
        );
        // Also verify it's at least the floor.
        assert!(
            r.capacity_samples() >= 960 * 4,
            "capacity must be >= 4 * chunk_samples"
        );
    }

    #[test]
    fn capacity_is_frame_aligned_for_4_channels() {
        // 4 channels: capacity must be a multiple of 4.
        let r = CaptureRing::new(100, 48_000, 4, 3_840);
        assert_eq!(
            r.capacity_samples() % 4,
            0,
            "4-channel capacity must be a multiple of 4"
        );
    }

    #[test]
    fn overflow_drops_preserve_channel_alignment_stereo() {
        // Fill a stereo ring to capacity, then push one more frame.
        // The popped data must start on an L sample (even index),
        // confirming the drop was frame-granular.
        let mut r = CaptureRing::new(100, 48_000, 2, 1_920);
        let cap = r.capacity_samples();
        // Fill with tagged samples: value = index so we can trace
        // which samples survived.
        let fill: Vec<f32> = (0..cap).map(|i| i as f32).collect();
        r.push(&fill);
        assert_eq!(r.len(), cap);

        // Push 2 new samples (one stereo frame) to trigger overflow.
        r.push(&[9998.0, 9999.0]);
        // The ring must still be at capacity.
        assert_eq!(r.len(), cap);
        // Pop everything and verify L/R pairing: every even index
        // should be an L sample (>= 2 since oldest frame was dropped).
        let all = r.pop_chunk(cap).unwrap();
        // The very last two samples must be the newly pushed frame.
        assert_eq!(all[cap - 2], 9998.0, "last L must be new");
        assert_eq!(all[cap - 1], 9999.0, "last R must be new");
        // Check that no 0.0 or 1.0 (from the original fill's first
        // frame) survived — they were dropped.
        assert!(
            !all.contains(&0.0),
            "oldest sample (index 0) must have been dropped"
        );
    }

    #[test]
    fn overflow_drops_multiple_frames_preserve_alignment() {
        // Push enough to overflow by several frames. Verify every
        // popped pair is a valid stereo frame (L, R).
        let mut r = CaptureRing::new(100, 48_000, 2, 1_920);
        let cap = r.capacity_samples();
        r.push(&vec![1.0; cap]);

        // Push 100 new samples = 50 stereo frames.
        let new_data: Vec<f32> = (0..100).map(|i| (i + 10_000) as f32).collect();
        r.push(&new_data);
        assert_eq!(r.len(), cap);

        // Pop all and verify the last 100 samples are the new data.
        let all = r.pop_chunk(cap).unwrap();
        for (i, expected) in new_data.iter().enumerate() {
            assert_eq!(
                all[cap - 100 + i],
                *expected,
                "new data must survive at tail"
            );
        }
    }

    // ── FIX-4 related: pop behavior for silence-sender ───────────

    #[test]
    fn pop_chunk_returns_none_on_empty_ring() {
        // The silence-sender relies on pop_chunk returning None to
        // trigger silence synthesis.
        let mut r = ring_48k_stereo_20ms_chunk(200);
        assert!(r.is_empty());
        assert!(r.pop_chunk(1920).is_none(), "empty ring must yield None");
    }

    #[test]
    fn pop_chunk_returns_none_when_insufficient_samples() {
        // Ring has data but not enough for a full chunk.
        let mut r = ring_48k_stereo_20ms_chunk(200);
        r.push(&vec![0.5; 100]); // 100 samples, need 1920
        assert!(r.pop_chunk(1920).is_none());
    }

    #[test]
    fn pop_chunk_succeeds_when_exact_chunk_available() {
        let mut r = ring_48k_stereo_20ms_chunk(200);
        let chunk: Vec<f32> = (0..1920).map(|i| i as f32).collect();
        r.push(&chunk);
        let out = r.pop_chunk(1920).unwrap();
        assert_eq!(out, chunk);
        assert!(r.is_empty());
    }

    // ── FIX-1 related: stats snapshot correctness ────────────────

    #[test]
    fn stats_snapshot_is_independent_of_subsequent_pushes() {
        // Capture a stats snapshot, then push more data. The old
        // snapshot must not change (it's a Copy struct).
        let mut r = ring_48k_stereo_20ms_chunk(200);
        r.push(&vec![0.0; 1920]);
        let snap1 = r.stats();
        assert_eq!(snap1.samples, 1920);
        assert_eq!(snap1.depth_ms, 20);
        assert_eq!(snap1.overruns, 0);

        // Push more data — snap1 must be unchanged.
        r.push(&vec![0.0; 1920]);
        assert_eq!(snap1.samples, 1920, "snapshot must not change");
        assert_eq!(snap1.depth_ms, 20, "snapshot must not change");

        let snap2 = r.stats();
        assert_eq!(snap2.samples, 3840);
        assert_eq!(snap2.depth_ms, 40);
    }

    #[test]
    fn overrun_count_tracks_actual_drops() {
        // Push data that triggers overflow multiple times. Verify
        // the overrun counter accumulates correctly.
        let mut r = CaptureRing::new(40, 1_000, 1, 100);
        let cap = r.capacity_samples();
        r.push(&vec![1.0; cap]);
        assert_eq!(r.stats().overruns, 0, "no overflow yet");

        // Push 50 samples — triggers overflow of 50 samples.
        r.push(&vec![2.0; 50]);
        assert_eq!(r.stats().overruns, 50, "first overflow: 50 dropped");

        // Push another 30 samples — triggers overflow of 30.
        r.push(&vec![3.0; 30]);
        assert_eq!(r.stats().overruns, 80, "cumulative: 50 + 30 = 80");
    }

    #[test]
    fn high_water_tracks_peak_fill() {
        let mut r = ring_48k_stereo_20ms_chunk(200);
        assert_eq!(r.stats().high_water_ms, 0, "no data yet");

        r.push(&vec![0.0; 1920]); // 20 ms
        assert_eq!(r.stats().high_water_ms, 20);

        r.push(&vec![0.0; 1920]); // 40 ms total
        assert_eq!(r.stats().high_water_ms, 40);

        // Drain some — high water should stay at peak.
        r.pop_chunk(1920);
        let snap = r.stats();
        assert_eq!(snap.samples, 1920, "current fill is 20 ms");
        assert_eq!(snap.high_water_ms, 40, "high water stays at peak");
    }

    #[test]
    fn mono_capacity_is_any_alignment() {
        // Mono (channels=1): any capacity is valid (everything is
        // frame-aligned). Just verify it doesn't crash and meets
        // the floor.
        for buffer_ms in [40, 55, 100, 200] {
            let r = CaptureRing::new(buffer_ms, 44_100, 1, 441);
            assert!(
                r.capacity_samples() >= 441 * 4,
                "capacity floor at {buffer_ms} ms"
            );
        }
    }

    #[test]
    fn single_push_larger_than_capacity_keeps_tail() {
        // When a single cpal callback delivers more samples than
        // the ring can hold, only the tail (latest) samples survive.
        // This exercises the `need >= self.capacity_samples` branch.
        let mut r = CaptureRing::new(40, 1_000, 1, 100);
        let cap = r.capacity_samples();
        assert!(cap >= 400);
        // Push a single buffer that's 2× the capacity.
        let big: Vec<f32> = (0..cap * 2).map(|i| i as f32).collect();
        r.push(&big);
        assert_eq!(r.len(), cap, "ring must be at capacity");
        // The surviving samples must be the TAIL of the big push.
        let out = r.pop_chunk(cap).unwrap();
        for (i, &v) in out.iter().enumerate() {
            let expected = (cap + i) as f32;
            assert_eq!(v, expected, "tail sample {i} must be {expected}");
        }
    }
}
