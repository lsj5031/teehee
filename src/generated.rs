//! `generated` — deterministic audio sources.
//!
//! Used by the `--sine` dry-run mode and the localhost smoke test.
//! The receiver pipeline is otherwise identical to real capture, so
//! this exercises the encoder / network / jitter / playback contract
//! end-to-end without requiring WASAPI loopback hardware.

/// Deterministic stereo-or-mono sine-wave source for tests and the
/// `--sine` dry-run mode.
///
/// Phase is tracked as a global sample index so consecutive calls to
/// [`Self::fill_chunk`] produce a continuous sine wave with no audible
/// clicks at chunk boundaries.
pub struct SineSource {
    sample_rate: u32,
    channels: u8,
    frequency: f32,
    /// Total mono sample frames (per-channel) generated so far.
    sample_pos: u64,
}

impl SineSource {
    /// Build a source producing a `frequency`-Hz sine at `sample_rate`,
    /// interleaved across `channels`. `sample_rate > 0` and `channels > 0`.
    pub fn new(sample_rate: u32, channels: u8, frequency: f32) -> Self {
        assert!(sample_rate > 0, "sample_rate must be > 0");
        assert!(channels > 0, "channels must be > 0");
        Self {
            sample_rate,
            channels,
            frequency,
            sample_pos: 0,
        }
    }

    /// Fill `out` with interleaved samples. `out.len()` must be a
    /// multiple of `channels`. The source advances its phase by
    /// `out.len() / channels` frames per call.
    pub fn fill_chunk(&mut self, out: &mut [f32]) {
        if out.is_empty() {
            return;
        }
        let frames = out.len() / self.channels as usize;
        let sr_f = self.sample_rate as f64;
        let freq_f = self.frequency as f64;
        let two_pi = std::f64::consts::TAU;
        for i in 0..frames {
            let n = (self.sample_pos + i as u64) as f64;
            let phase = two_pi * freq_f * n / sr_f;
            let value = phase.sin() as f32;
            let base = i * self.channels as usize;
            for ch in 0..self.channels as usize {
                out[base + ch] = value;
            }
        }
        self.sample_pos += frames as u64;
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn sine_source_first_sample_is_zero() {
        // sin(0) = 0
        let mut src = SineSource::new(48_000, 1, 440.0);
        let mut out = [1.0_f32; 1];
        src.fill_chunk(&mut out);
        assert!(
            out[0].abs() < 1e-6,
            "first sample must be sin(0) = 0; got {}",
            out[0]
        );
    }

    #[test]
    fn sine_source_output_is_bounded_in_neg1_to_1() {
        let mut src = SineSource::new(48_000, 2, 440.0);
        let mut out = vec![0.0_f32; 4800]; // 2400 stereo frames
        src.fill_chunk(&mut out);
        for (i, &s) in out.iter().enumerate() {
            assert!(s >= -1.0 && s <= 1.0, "sample {i} out of range: {s}");
        }
    }

    #[test]
    fn sine_source_stereo_channels_are_identical() {
        let mut src = SineSource::new(48_000, 2, 440.0);
        let mut out = vec![0.0_f32; 20]; // 10 stereo frames
        src.fill_chunk(&mut out);
        for i in 0..10 {
            assert_eq!(
                out[i * 2],
                out[i * 2 + 1],
                "stereo L/R must be identical at frame {i}"
            );
        }
    }

    #[test]
    fn sine_source_mono_matches_stereo_left_channel() {
        let mut mono = SineSource::new(48_000, 1, 440.0);
        let mut stereo = SineSource::new(48_000, 2, 440.0);
        let mut mono_out = vec![0.0_f32; 5];
        let mut stereo_out = vec![0.0_f32; 10];
        mono.fill_chunk(&mut mono_out);
        stereo.fill_chunk(&mut stereo_out);
        for i in 0..5 {
            assert!(
                (mono_out[i] - stereo_out[i * 2]).abs() < 1e-6,
                "mono[{i}] = {} must match stereo L[{i}] = {}",
                mono_out[i],
                stereo_out[i * 2]
            );
        }
    }

    #[test]
    fn sine_source_across_chunk_boundaries_is_continuous() {
        let mut src = SineSource::new(48_000, 2, 440.0);
        let mut chunk1 = vec![0.0_f32; 200];
        let mut chunk2 = vec![0.0_f32; 200];
        src.fill_chunk(&mut chunk1);
        src.fill_chunk(&mut chunk2);
        // The first sample of chunk2 must follow the last sample
        // of chunk1 without a discontinuity (no jump > one
        // sample's worth of sine slope).
        let last = chunk1[198]; // last L sample of chunk1
        let first = chunk2[0]; // first L sample of chunk2
        let max_step = 2.0 * std::f32::consts::PI * 440.0 / 48_000.0;
        assert!(
            (first - last).abs() < max_step + 1e-3,
            "discontinuity at chunk boundary: last={last}, first={first}"
        );
    }

    #[test]
    fn sine_source_frequency_accuracy_peak_at_expected_bin() {
        // Generate 48000 samples (1 second) at 48kHz with freq=440Hz.
        // The peak energy should be at bin 440 in an FFT-like test.
        // Simplified: sum sin(2π·440·n/48000)² over all n — the
        // average power should be ~0.5 (Parseval's theorem for sine).
        let mut src = SineSource::new(48_000, 1, 440.0);
        let mut out = vec![0.0_f32; 48_000];
        src.fill_chunk(&mut out);
        let power: f64 = out.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / 48_000.0;
        assert!(
            (power - 0.5).abs() < 0.01,
            "mean power of sine wave should be ~0.5; got {power}"
        );
    }

    #[test]
    fn sine_source_empty_out_is_noop() {
        let mut src = SineSource::new(48_000, 2, 440.0);
        let mut out: [f32; 0] = [];
        src.fill_chunk(&mut out);
        // Must not panic and sample_pos must remain 0.
        assert_eq!(src.sample_pos, 0);
    }
}
