//! `clock_drift` — adaptive clock-drift compensation for the teehee
//! receive path.
//!
//! Sender and receiver audio crystals differ by ±50–200 ppm. When
//! nominal rates match (both 48 kHz), the resampler is bypassed and
//! drift accumulates silently: sender-faster → buffer grows → periodic
//! packet-skip clicks; receiver-faster → buffer drains → underrun
//! freeze + full prebuffer re-fill (~200 ms silence).
//!
//! This module tracks the jitter-buffer fill level over a sliding
//! window, computes the fill slope via least-squares linear regression,
//! and derives a rate-correction factor (ppm) that nudges the
//! [`LinearResampler`]'s step size to keep the buffer at its target
//! fill. The correction is smooth (no audible wow/flutter at the
//! chosen gains) and converges within ~1–2 minutes (time constant
//! ≈ 42 s at kp=0.5, 95 % settled in ≈ 2 min).
//!
//! ## Algorithm
//!
//! Every cpal callback (~10 ms), the caller records
//! `(elapsed_seconds, queued_samples)` into a ring buffer.
//! Internally, `queued_samples` is normalised to audio frames
//! (÷ channels) so the slope and proportional term are
//! channel-count–agnostic (D1 fix). Over a
//! configurable window (default 8 seconds), least-squares linear
//! regression gives the slope `m` in frames/sec:
//!
//! ```text
//!   drift_ppm = (m / nominal_rate_hz) * 1_000_000
//! ```
//!
//! A proportional term gently pulls the buffer back toward the target:
//!
//! ```text
//!   p_ppm = kp * (queued_frames - target_frames)
//!   total_ppm = clamp(drift_ppm + p_ppm, ±MAX_DRIFT_PPM)
//! ```
//!
//! `total_ppm` is fed to
//! [`FormatPipeline::set_drift_correction`] which adjusts the
//! resampler's `step_q32` by that fraction.
//!
//! ## Thread safety
//!
//! `ClockDriftTracker` is `!Send` / `!Sync` (no `unsafe impl`). It
//! lives in `RxState` behind the same `Mutex` that guards the
//! `JitterBuffer` and `FormatPipeline` — the cpal callback reads and
//! updates it under that lock, so no additional synchronization is
//! needed.

use std::collections::VecDeque;
use std::time::Instant;

/// Maximum absolute drift correction in ppm. At ±500 ppm the
/// resampler's step changes by 0.05% — inaudible per callback
/// (~0.005 samples per 512-frame block at 48 kHz). The bound
/// prevents runaway corrections from destabilizing the pipeline.
const MAX_DRIFT_PPM: f32 = 500.0;

/// Default sliding window duration in seconds. Longer windows give
/// more stable slope estimates but slower convergence. 8 seconds
/// balances both.
const DEFAULT_WINDOW_SECS: f64 = 8.0;

/// Minimum number of samples before regression is meaningful.
/// At 10 ms/callback, 50 samples ≈ 500 ms of data.
const MIN_SAMPLES: usize = 50;

/// Default proportional gain: ppm correction per *frame* of offset
/// from the target (after the D1 fix normalises interleaved samples
/// to audio frames). At kp=0.5 and 48 kHz, a 100-frame overshoot
/// (~2 ms at stereo) produces 50 ppm correction — well within the
/// ±500 ppm clamp and inaudible. The resulting time constant is
/// τ ≈ 1 000 000 / (kp × nominal_rate) ≈ 42 s at 48 kHz, giving
/// 95 % convergence in ≈ 2 min.
const DEFAULT_KP: f32 = 0.5;

/// A single measurement point: elapsed time since tracker creation
/// and the jitter-buffer fill at that instant, in *audio frames*
/// (not interleaved samples — see [`ClockDriftTracker::update`]).
#[derive(Debug, Clone, Copy)]
struct Sample {
    t: f64,
    queued: f64,
}

/// Clock-drift tracker using sliding-window linear regression and a
/// proportional controller.
pub struct ClockDriftTracker {
    /// Ring buffer of recent `(time, queued_frames)` samples.
    /// After the D1 fix, `queued` is stored in *audio frames*
    /// (interleaved samples ÷ channels) so the slope and
    /// proportional term are channel-count–agnostic.
    samples: VecDeque<Sample>,
    /// Wall-clock origin (first `update` call).
    origin: Instant,
    /// Has `origin` been set?
    origin_set: bool,
    /// Sliding window duration in seconds.
    window_secs: f64,
    /// Nominal sample rate in Hz (used to convert slope → ppm).
    nominal_rate: u32,
    /// Target buffer fill in *audio frames* (prebuffer target
    /// converted from interleaved samples at construction).
    target_frames: f64,
    /// Channel count — used to convert interleaved-sample fill
    /// values to audio frames for regression and P-term.
    channels: u8,
    /// Proportional gain.
    kp: f32,
    /// Last computed drift correction in ppm (for stats).
    current_ppm: f32,
    /// Last computed fill slope in frames/sec (for stats).
    current_slope: f32,
    /// Whether enough data has accumulated for meaningful correction.
    warmed_up: bool,
}

impl ClockDriftTracker {
    /// Create a new tracker.
    ///
    /// * `nominal_rate` — sender's sample rate in Hz (e.g. 48000).
    /// * `target_frames` — prebuffer target in interleaved samples
    ///   (the value used by the jitter buffer's prebuffer gate).
    ///   Internally converted to audio frames (÷ channels).
    /// * `channels` — sender channel count (e.g. 2 for stereo).
    ///   Used to normalise interleaved samples → audio frames.
    pub fn new(nominal_rate: u32, target_frames: usize, channels: u8) -> Self {
        let ch = channels.max(1) as f64;
        Self {
            samples: VecDeque::with_capacity(1024),
            origin: Instant::now(),
            origin_set: false,
            window_secs: DEFAULT_WINDOW_SECS,
            nominal_rate,
            target_frames: target_frames as f64 / ch,
            channels: channels.max(1),
            kp: DEFAULT_KP,
            current_ppm: 0.0,
            current_slope: 0.0,
            warmed_up: false,
        }
    }

    /// Record a new measurement. Call this once per cpal callback
    /// with the current jitter-buffer `queued_frames()` value
    /// (which is in *interleaved samples*, not audio frames — the
    /// D1 fix normalises by channel count here).
    pub fn update(&mut self, queued_samples: usize) {
        let now = Instant::now();
        if !self.origin_set {
            self.origin = now;
            self.origin_set = true;
        }
        let t = now.duration_since(self.origin).as_secs_f64();
        let ch = self.channels as f64;
        self.samples.push_back(Sample {
            t,
            queued: queued_samples as f64 / ch,
        });

        // Evict samples outside the sliding window.
        let cutoff = t - self.window_secs;
        while self
            .samples
            .front()
            .is_some_and(|s| s.t < cutoff)
        {
            self.samples.pop_front();
        }

        // Need enough data for a meaningful regression.
        if self.samples.len() < MIN_SAMPLES {
            self.warmed_up = false;
            return;
        }
        self.warmed_up = true;

        // Least-squares linear regression: slope m in frames/sec.
        let (m, _b) = self.least_squares_slope();

        // Convert slope to ppm drift: positive slope = buffer growing
        // = sender is faster than receiver = receiver needs to speed
        // up (consume faster). A positive m means we need a POSITIVE
        // ppm correction (increase step → produce more output per
        // input → drain faster).
        let drift_ppm = if self.nominal_rate > 0 {
            (m / self.nominal_rate as f64) * 1_000_000.0
        } else {
            0.0
        };

        // Proportional term: gently pull buffer toward target.
        // Use the most recent queued value. Both `latest` and
        // `target_frames` are in audio frames after the D1 fix.
        let latest = self.samples.back().unwrap().queued;
        let offset = latest - self.target_frames;
        let p_ppm = self.kp as f64 * offset;

        // Total correction, clamped.
        let total = (drift_ppm + p_ppm) as f32;
        self.current_ppm = total.clamp(-MAX_DRIFT_PPM, MAX_DRIFT_PPM);
        self.current_slope = m as f32;
    }

    /// Current drift correction in ppm. Returns 0.0 until enough
    /// data has accumulated (`MIN_SAMPLES` callbacks).
    pub fn current_ppm(&self) -> f32 {
        if self.warmed_up {
            self.current_ppm
        } else {
            0.0
        }
    }

    /// Current fill slope in *audio frames*/sec. Positive = buffer
    /// growing. Note: after the D1 fix, this is in frames, not
    /// interleaved samples.
    pub fn current_slope(&self) -> f32 {
        self.current_slope
    }

    /// Whether the tracker has accumulated enough data to produce
    /// meaningful corrections.
    pub fn is_warmed_up(&self) -> bool {
        self.warmed_up
    }

    /// Reset the tracker's internal state, discarding all
    /// accumulated samples and zeroing the current correction.
    /// Call this when the jitter buffer underruns and reverts to
    /// prebuffer mode (D3 fix) — the fill cliff and subsequent
    /// ramp would corrupt the regression window and produce
    /// wrong-direction corrections.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.origin_set = false;
        self.current_ppm = 0.0;
        self.current_slope = 0.0;
        self.warmed_up = false;
    }

    /// Least-squares linear regression slope and intercept.
    /// Returns `(m, b)` where `queued ≈ m * t + b`.
    fn least_squares_slope(&self) -> (f64, f64) {
        let n = self.samples.len() as f64;
        let mut sum_t = 0.0;
        let mut sum_q = 0.0;
        let mut sum_tt = 0.0;
        let mut sum_tq = 0.0;
        for s in &self.samples {
            sum_t += s.t;
            sum_q += s.queued;
            sum_tt += s.t * s.t;
            sum_tq += s.t * s.queued;
        }
        let denom = n * sum_tt - sum_t * sum_t;
        if denom.abs() < 1e-12 {
            // Degenerate case (all timestamps identical).
            return (0.0, sum_q / n.max(1.0));
        }
        let m = (n * sum_tq - sum_t * sum_q) / denom;
        let b = (sum_q - m * sum_t) / n;
        (m, b)
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    fn make_tracker(target: usize) -> ClockDriftTracker {
        // mono (channels=1) so samples == frames and existing
        // numeric assertions stay valid.
        ClockDriftTracker::new(48_000, target, 1)
    }

    #[test]
    fn tracker_starts_cold() {
        let t = make_tracker(9600);
        assert!(!t.is_warmed_up());
        assert_eq!(t.current_ppm(), 0.0);
        assert_eq!(t.current_slope(), 0.0);
    }

    #[test]
    fn tracker_warms_up_after_min_samples() {
        let mut t = make_tracker(9600);
        for i in 0..MIN_SAMPLES {
            t.update(9600);
            if i < MIN_SAMPLES - 1 {
                assert!(!t.is_warmed_up());
            }
        }
        assert!(t.is_warmed_up());
    }

    #[test]
    fn stable_fill_produces_near_zero_drift() {
        let mut t = make_tracker(9600);
        for _ in 0..200 {
            t.update(9600); // exactly at target
        }
        let ppm = t.current_ppm();
        assert!(
            ppm.abs() < 5.0,
            "stable fill should produce near-zero drift, got {ppm} ppm"
        );
    }

    #[test]
    fn growing_fill_produces_positive_drift() {
        let mut t = make_tracker(9600);
        // Simulate buffer growing by 10 frames per callback (~1 frame/ms at 10ms/callback).
        for i in 0..200 {
            t.update(9600 + i * 10);
        }
        let ppm = t.current_ppm();
        assert!(
            ppm > 10.0,
            "growing fill should produce positive drift correction, got {ppm} ppm"
        );
        assert!(
            t.current_slope() > 0.0,
            "slope should be positive for growing fill"
        );
    }

    #[test]
    fn shrinking_fill_produces_negative_drift() {
        let mut t = make_tracker(9600);
        // Simulate buffer shrinking.
        for i in 0..200 {
            let fill = 9600_usize.saturating_sub(i * 10);
            t.update(fill);
        }
        let ppm = t.current_ppm();
        assert!(
            ppm < -10.0,
            "shrinking fill should produce negative drift correction, got {ppm} ppm"
        );
        assert!(
            t.current_slope() < 0.0,
            "slope should be negative for shrinking fill"
        );
    }

    #[test]
    fn drift_is_clamped_to_max() {
        let mut t = make_tracker(9600);
        // Extreme growth: +1000 frames per callback.
        for i in 0..200 {
            t.update(9600 + i * 1000);
        }
        let ppm = t.current_ppm();
        assert!(
            ppm.abs() <= MAX_DRIFT_PPM + 0.1,
            "drift must be clamped to ±{MAX_DRIFT_PPM}, got {ppm}"
        );
    }

    #[test]
    fn proportional_term_pulls_toward_target() {
        let mut t = make_tracker(9600);
        // Feed stable data but offset from target by 500 frames.
        for _ in 0..200 {
            t.update(10100); // 500 frames above target
        }
        let ppm = t.current_ppm();
        // The slope is ~0 (stable), but the proportional term should
        // produce a positive correction (drain faster to reach target).
        // With kp=0.5 and 500 frames offset: p_ppm = 0.5 * 500 = 250.
        assert!(
            ppm > 100.0,
            "fill above target should produce strong positive correction, got {ppm} ppm"
        );
    }

    // ── D1 fix: channel-count normalisation ─────────────────────

    #[test]
    fn stereo_normalises_samples_to_frames() {
        // Stereo tracker (channels=2) with target in interleaved
        // samples = 9600 (4800 frames). Feed 4800 frames worth
        // of samples (9600) — should be at target, near-zero ppm.
        let mut t = ClockDriftTracker::new(48_000, 9600, 2);
        for _ in 0..200 {
            t.update(9600); // 9600 samples / 2ch = 4800 frames
        }
        let ppm = t.current_ppm();
        assert!(
            ppm.abs() < 5.0,
            "stereo stable fill should produce near-zero drift, got {ppm} ppm"
        );
    }

    #[test]
    fn stereo_growing_fill_produces_correct_drift() {
        // With stereo, if the buffer grows by 20 samples/callback
        // (= 10 frames/callback), the drift should be the same as
        // mono growing by 10 frames/callback.
        let mut t_stereo = ClockDriftTracker::new(48_000, 9600, 2);
        let mut t_mono = ClockDriftTracker::new(48_000, 4800, 1);
        for i in 0..200 {
            t_stereo.update(9600 + i * 20); // 20 samples = 10 frames at stereo
            t_mono.update(4800 + i * 10);    // 10 frames
        }
        let ppm_s = t_stereo.current_ppm();
        let ppm_m = t_mono.current_ppm();
        // Both should produce similar drift corrections (within 5%)
        // because the channel normalisation makes them equivalent.
        let diff = (ppm_s - ppm_m).abs();
        let avg = (ppm_s.abs() + ppm_m.abs()) / 2.0;
        assert!(
            diff / avg.max(0.01) < 0.05,
            "stereo and mono drift should match after D1 fix: stereo={ppm_s}, mono={ppm_m}"
        );
    }

    // ── D3 fix: reset on underrun ──────────────────────────────

    #[test]
    fn reset_clears_all_state() {
        let mut t = make_tracker(9600);
        // Warm up with growing data.
        for i in 0..200 {
            t.update(9600 + i * 10);
        }
        assert!(t.is_warmed_up());
        assert!(t.current_ppm() != 0.0);
        assert!(t.current_slope() != 0.0);

        t.reset();
        assert!(!t.is_warmed_up(), "must be cold after reset");
        assert_eq!(t.current_ppm(), 0.0, "ppm must zero after reset");
        assert_eq!(t.current_slope(), 0.0, "slope must zero after reset");
    }

    #[test]
    fn reset_then_re_warm_produces_fresh_estimate() {
        let mut t = make_tracker(9600);
        // Warm up.
        for _ in 0..200 {
            t.update(10100); // above target
        }
        assert!(t.current_ppm() > 0.0);
        let old_ppm = t.current_ppm();

        // Simulate underrun: reset.
        t.reset();
        assert_eq!(t.current_ppm(), 0.0);

        // Feed stable data at target — after re-warm, ppm should be
        // near zero (not carry over the old positive correction).
        for _ in 0..200 {
            t.update(9600);
        }
        assert!(t.is_warmed_up());
        let new_ppm = t.current_ppm();
        assert!(
            new_ppm.abs() < 5.0,
            "after reset + stable fill, ppm should be near zero, got {new_ppm} (was {old_ppm})"
        );
    }
}
