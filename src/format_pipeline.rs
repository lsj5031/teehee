//! `format_pipeline` — pure-Rust sample-rate conversion and channel
//! reconciliation for the teehee receive path (slice 7).
//!
//! Sits between the receive-side [`JitterBuffer`] and the cpal
//! [`Player`] callback. Conversion is only needed when sender
//! format ≠ receiver format (a real possibility on cross-host LANs:
//! sender @ 48 kHz stereo into a Mac receiver running at 44.1 kHz
//! mono, etc.). When formats match, the pipeline is unused and the
//! cpal callback drains the jitter buffer in-place.
//!
//! ## Surface
//!
//! * [`LinearResampler`] — fixed-point linear-interpolation
//!   sample-rate conversion. Stateful across cpal callbacks; the
//!   internal cursor survives partial reads so no audio artefacts
//!   are introduced at chunk boundaries.
//! * [`ChannelMixer`] — channel-count reconciliation:
//!   mono↔stereo average / broadcast, N→M stride down-mix or
//!   zero-pad up-mix.
//! * [`FormatPipeline`] — composes resampler then mixer (order is
//!   fixed; reversing would mangle the mono→stereo + SR-convert case).
//!   Exposes `feed` / `drain` / `pending_output_frames` for streaming
//!   use; `process(input, out)` is a convenience wrapper that calls
//!   `feed` then `drain`.
//!
//! ## Quality bar
//!
//! Linear interpolation is good enough for voice-like audio at typical
//! LAN ratios (48↔44.1 is 0.91875× downsample or 1.0889× upsample).
//! For studio-grade music a sinc-windowed resampler (e.g. the
//! `rubato` crate) would be the v2 swap; deferred to keep teehee
//! v1 dep-free.
//!
//! ## Thread safety
//!
//! `LinearResampler` is `!Send` / `!Sync` (no `unsafe impl`). It
//! carries stateful interpolation cursors and is meant to live on
//! the cpal audio thread behind a single `Mutex` slot, exactly like
//! `JitterBuffer` itself. ChannelMixer is pure-math.
//!
//! [`JitterBuffer`]: crate::jitter::JitterBuffer
//! [`Player`]: crate::audio_io::Player

/// Sample-rate conversion via fixed-point linear interpolation
/// between adjacent input samples.
///
/// State machine (phase-continuous across calls):
/// * `last_frame` — left anchor of interp per channel. Carried
///   across `process` calls so chunk boundaries are seamless.
/// * `pos_q32` — fixed-point position within `[0, 1<<32)` in the
///   gap between `last_frame`'s input frame and the current right
///   anchor. `t = pos_q32 as f64 / 4294967296.0` is the
///   linear-interp weight. Carried across calls — no per-call
///   reseed.
/// * `step_q32 = input_rate * (1<<32) / output_rate` — how much
///   the cursor advances per produced output frame. Q32 kills the
///   ~165 ppm rate error that Q16 introduced.
/// * `seeded` — false until the first `process` call, which
///   consumes `input[0]` as the left anchor. Subsequent calls
///   start from `in_idx = 0` (the first frame of the new chunk
///   is the right anchor).
///
/// New-loop semantics: discharge carry before emitting, consume all
/// input, stop when the right anchor is exhausted (no `last_frame`
/// echo). This means each `process` call produces fewer outputs
/// than the old reseed-per-call loop, but the total over many calls
/// is exact: `total_out ≈ total_in × output_rate / input_rate`.
///
/// ### Known limitation
///
/// When `pos_q32` residual is non-zero at the end of a chunk and
/// the next chunk's first frame is the right anchor, interpolation
/// bridges seamlessly. No carry is discarded.
#[derive(Debug, Clone)]
pub struct LinearResampler {
    input_rate_hz: u32,
    output_rate_hz: u32,
    input_channels: u8,
    /// Advances the in-gap cursor per produced output frame.
    /// `1 << 32 == 1 input frame`.
    nominal_step_q32: u64,
    /// The actual step used, potentially adjusted by drift
    /// compensation. Equals `nominal_step_q32` when no drift
    /// correction is applied.
    step_q32: u64,
    /// Cursor in Q32 fixed-point. Always in `[0, 1<<32)` after
    /// the crossing-commit loop.
    pos_q32: u64,
    /// Left anchor of interpolation, per channel. Carried across
    /// `process` calls for phase continuity.
    last_frame: Vec<f32>,
    /// False until the first `process` call seeds `last_frame`.
    seeded: bool,
    /// Total input frames consumed (incl. the seed consume).
    frames_in: u64,
    /// Total output frames emitted.
    frames_out: u64,
    /// Whether drift compensation has been applied (forces
    /// non-passthrough even when rates match).
    drift_active: bool,
}

impl LinearResampler {
    /// Build a resampler for `input_rate_hz → output_rate_hz` over
    /// `input_channels` interleaved channels. Panics on
    /// `input_rate_hz == 0` or `output_rate_hz == 0`.
    pub fn new(input_rate_hz: u32, output_rate_hz: u32, input_channels: u8) -> Self {
        assert!(input_rate_hz > 0, "input_rate_hz must be > 0");
        assert!(output_rate_hz > 0, "output_rate_hz must be > 0");
        let step_q32 = (input_rate_hz as u64).wrapping_mul(1u64 << 32) / output_rate_hz as u64;
        Self {
            input_rate_hz,
            output_rate_hz,
            input_channels,
            nominal_step_q32: step_q32,
            step_q32,
            pos_q32: 0,
            last_frame: vec![0.0; input_channels as usize],
            seeded: false,
            frames_in: 0,
            frames_out: 0,
            drift_active: false,
        }
    }

    /// True if this resampler is a no-op (input_rate == output_rate
    /// AND no drift correction is active).
    pub fn is_passthrough(&self) -> bool {
        self.input_rate_hz == self.output_rate_hz && !self.drift_active
    }

    /// Apply a drift correction in parts-per-million. Positive ppm
    /// speeds up playback (drains buffer faster); negative slows it.
    /// The adjustment is applied to the resampler's step size:
    /// `step = nominal_step * (1 + ppm / 1e6)`.
    ///
    /// Setting ppm to 0.0 (or calling this with 0.0) reverts to
    /// nominal stepping but keeps `drift_active = true` so the
    /// pipeline remains non-passthrough.
    pub fn set_drift_correction(&mut self, ppm: f32) {
        self.drift_active = true;
        let factor = 1.0 + (ppm as f64) / 1_000_000.0;
        self.step_q32 = (self.nominal_step_q32 as f64 * factor) as u64;
        // Ensure step is at least 1 to prevent stalling.
        if self.step_q32 == 0 {
            self.step_q32 = 1;
        }
    }

    /// Whether drift compensation has been activated.
    pub fn drift_active(&self) -> bool {
        self.drift_active
    }

    /// Input sample rate in Hz.
    pub fn input_rate_hz(&self) -> u32 {
        self.input_rate_hz
    }

    /// Output sample rate in Hz.
    pub fn output_rate_hz(&self) -> u32 {
        self.output_rate_hz
    }

    /// Number of input channels the resampler was built for.
    pub fn input_channels(&self) -> u8 {
        self.input_channels
    }

    /// Cumulative diagnostics.
    pub fn stats(&self) -> ResamplerStats {
        ResamplerStats {
            frames_in: self.frames_in,
            frames_out: self.frames_out,
        }
    }

    /// Convert `input_frames` (interleaved f32, length =
    /// `input_frames × input_channels`) into OUTPUT frames written
    /// into `out` (interleaved f32, length = `out_capacity ×
    /// input_channels`). Returns the number of OUTPUT frames
    /// produced, capped by `out_capacity`. State is preserved
    /// exactly at the end of the input slice to bridge smoothly
    /// to the next `process` call.
    pub fn process(&mut self, input_frames: &[f32], out: &mut [f32]) -> usize {
        let ch = self.input_channels as usize;
        debug_assert_eq!(
            input_frames.len() % ch,
            0,
            "input_frames length must be a multiple of input_channels"
        );
        debug_assert_eq!(
            out.len() % ch,
            0,
            "out length must be a multiple of input_channels"
        );

        let in_frames = input_frames.len() / ch;
        let out_cap = out.len() / ch;
        if out_cap == 0 || in_frames == 0 {
            return 0;
        }

        let mut in_idx: usize = 0;

        // First-call seeding: consume input[0] as the left anchor
        // AND emit it verbatim (t=0). Afterward, in_idx = 1 so
        // input[1] is the first right anchor.
        if !self.seeded {
            self.last_frame.copy_from_slice(&input_frames[..ch]);
            self.seeded = true;
            in_idx = 1;
            self.frames_in += 1;
        }

        let mut written: usize = 0;

        // Phase 1: Discharge carry from the previous call.
        // pos_q32 may be >= 1<<32 if the last call ended with a
        // crossing mid-step. Advance through input to commit the
        // left anchor.
        while self.pos_q32 >= (1u64 << 32) && in_idx < in_frames {
            self.pos_q32 -= 1u64 << 32;
            let off = in_idx * ch;
            self.last_frame
                .copy_from_slice(&input_frames[off..off + ch]);
            self.frames_in += 1;
            in_idx += 1;
        }

        // Phase 2: Emit output frames while a right anchor exists.
        while written < out_cap && in_idx < in_frames {
            let t = (self.pos_q32 as f64 / 4294967296.0) as f32;
            let right_off = in_idx * ch;
            for c in 0..ch {
                let a = self.last_frame[c];
                let b = input_frames[right_off + c];
                out[written * ch + c] = a + (b - a) * t;
            }
            written += 1;
            self.frames_out += 1;

            // Advance cursor and commit each crossing as a new left
            // anchor.
            self.pos_q32 += self.step_q32;
            while self.pos_q32 >= (1u64 << 32) && in_idx < in_frames {
                self.pos_q32 -= 1u64 << 32;
                let off = in_idx * ch;
                self.last_frame
                    .copy_from_slice(&input_frames[off..off + ch]);
                self.frames_in += 1;
                in_idx += 1;
            }
        }

        written
    }
}

/// Diagnostics for one [`LinearResampler`] instance.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ResamplerStats {
    pub frames_in: u64,
    pub frames_out: u64,
}

/// Channel-count reconciliation. Pure function — no state retained
/// across calls.
///
/// Rules:
/// * `same → same`: byte-wise pass-through on the overlap.
/// * `mono → stereo`: broadcast (each output frame's L = mono, R =
///   mono).
/// * `stereo → mono`: arithmetic average of L+R divided by 2 (lossy).
/// * `N → N` where N matches: pass-through.
/// * `N → M < N` (down-mix): each output channel averages a stride
///   of `M_step = n_in / n_out` input channels — 4→2 averages pairs,
///   4→1 averages all four. The remainder `n_in % n_out` is folded
///   into the first output channel so the average is mass-conserving.
/// * `N → M > N` (up-mix): place input channels in the first N
///   output slots, zero-pad the rest. v1 ships mono↔stereo only;
///   this branch is defensive and unreachable in `recv`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelMixer {
    input_channels: u8,
    output_channels: u8,
}

impl ChannelMixer {
    /// Build a mixer for a fixed `(input_channels, output_channels)`
    /// pair. Panics on either being 0.
    pub fn new(input_channels: u8, output_channels: u8) -> Self {
        assert!(input_channels > 0, "input_channels must be > 0");
        assert!(output_channels > 0, "output_channels must be > 0");
        Self {
            input_channels,
            output_channels,
        }
    }

    /// True if this mixer is a no-op (input == output channels).
    pub fn is_passthrough(&self) -> bool {
        self.input_channels == self.output_channels
    }

    /// Number of input channels.
    pub fn input_channels(&self) -> u8 {
        self.input_channels
    }

    /// Number of output channels.
    pub fn output_channels(&self) -> u8 {
        self.output_channels
    }

    /// Convert `input_frames` (interleaved f32, length =
    /// `n_frames × input_channels`) to `out` (interleaved f32,
    /// length = `out_capacity × output_channels`). Returns the count
    /// of OUTPUT frames produced.
    pub fn process(&self, input_frames: &[f32], out: &mut [f32]) -> usize {
        let ic = self.input_channels as usize;
        let oc = self.output_channels as usize;
        debug_assert_eq!(
            input_frames.len() % ic,
            0,
            "input_frames length must be a multiple of input_channels"
        );
        debug_assert_eq!(
            out.len() % oc,
            0,
            "out length must be a multiple of output_channels"
        );
        let in_frames = input_frames.len() / ic;
        let out_capacity = out.len() / oc;
        let n = in_frames.min(out_capacity);
        if n == 0 {
            return 0;
        }

        if self.is_passthrough() {
            let copy_len = n * ic;
            out[..copy_len].copy_from_slice(&input_frames[..copy_len]);
            return n;
        }

        for i in 0..n {
            let in_off = i * ic;
            let out_off = i * oc;
            match (ic, oc) {
                (1, 2) => {
                    let s = input_frames[in_off];
                    out[out_off] = s;
                    out[out_off + 1] = s;
                }
                (2, 1) => {
                    let l = input_frames[in_off];
                    let r = input_frames[in_off + 1];
                    out[out_off] = (l + r) * 0.5;
                }
                (n_in, n_out) if n_in < n_out => {
                    // Up-mix: input channels into the first `n_in`
                    // slots, zero-pad the rest. v1 unreachable path
                    // (recv ships mono↔stereo only).
                    out[out_off..out_off + n_in]
                        .copy_from_slice(&input_frames[in_off..in_off + n_in]);
                    for c in n_in..n_out {
                        out[out_off + c] = 0.0;
                    }
                }
                (n_in, n_out) => {
                    // Down-mix: average stride of `n_in / n_out`
                    // input channels into each output channel. The
                    // remainder `n_in % n_out` is folded into the
                    // first output channel so averages are
                    // mass-conserving.
                    let step = n_in / n_out;
                    let rem = n_in % n_out;
                    for c in 0..n_out {
                        let start = c * step;
                        let end = start + step + if c == 0 { rem } else { 0 };
                        let mut acc = 0.0_f32;
                        for j in start..end {
                            acc += input_frames[in_off + j];
                        }
                        let span = end - start;
                        out[out_off + c] = if span > 0 { acc / span as f32 } else { 0.0 };
                    }
                }
            }
        }
        n
    }
}

/// Composed sample-rate conversion + channel reconciliation. The
/// resampler runs first (it operates in input channels), then the
/// mixer (it converts input-channels to output-channels). The
/// composition order is fixed; reversing mangles the
/// mono→stereo + SR-convert case.
///
/// **Streaming interface**: `feed()` pushes input through the
/// resampler + mixer and appends to an internal output FIFO.
/// `drain()` copies from the FIFO to the caller's buffer.
/// `process(input, out)` is a convenience wrapper that calls `feed`
/// then `drain`. `pending_output_frames()` reports how many output
/// frames are buffered.
///
/// **Capacity**: `feed` resizes the internal scratch buffer on
/// overflow so it never silently truncates.
pub struct FormatPipeline {
    resampler: LinearResampler,
    mixer: ChannelMixer,
    /// Scratch buffer for the resampler's INTERMEDIATE output
    /// (still in input_channels). Re-sized on overflow.
    scratch: Vec<f32>,
    /// Output FIFO: accumulated final-output samples ready for
    /// the cpal callback to drain.
    fifo: Vec<f32>,
    /// Read cursor within `fifo`. When `fifo_read == fifo.len()`,
    /// both are cleared.
    fifo_read: usize,
    /// Cumulative diagnostics.
    stats: FormatStats,
}

impl FormatPipeline {
    /// Build a pipeline for `input_rate_hz × input_channels →
    /// output_rate_hz × output_channels`.
    pub fn new(
        input_rate_hz: u32,
        output_rate_hz: u32,
        input_channels: u8,
        output_channels: u8,
    ) -> Self {
        let resampler = LinearResampler::new(input_rate_hz, output_rate_hz, input_channels);
        let mixer = ChannelMixer::new(input_channels, output_channels);
        Self {
            resampler,
            mixer,
            scratch: Vec::new(),
            fifo: Vec::new(),
            fifo_read: 0,
            stats: FormatStats::default(),
        }
    }

    /// True if this pipeline is a complete no-op (input_rate ==
    /// output_rate AND input_channels == output_channels). Hot path
    /// can skip the pipeline entirely.
    pub fn is_passthrough(&self) -> bool {
        self.resampler.is_passthrough() && self.mixer.is_passthrough()
    }

    /// Sample-rate conversion component.
    pub fn resampler(&self) -> &LinearResampler {
        &self.resampler
    }

    /// Channel-mix component.
    pub fn mixer(&self) -> &ChannelMixer {
        &self.mixer
    }

    /// Apply a drift correction in ppm to the underlying resampler.
    /// See [`LinearResampler::set_drift_correction`] for details.
    /// This also forces the pipeline into non-passthrough mode so
    /// the resampler runs even when nominal rates match.
    pub fn set_drift_correction(&mut self, ppm: f32) {
        self.resampler.set_drift_correction(ppm);
    }

    /// Whether drift correction has been applied.
    pub fn drift_active(&self) -> bool {
        self.resampler.drift_active()
    }

    /// Cumulative diagnostics.
    pub fn stats(&self) -> FormatStats {
        let rs = self.resampler.stats();
        FormatStats {
            samples_in: self.stats.samples_in,
            samples_out: self.stats.samples_out,
            resampler_frames_in: rs.frames_in,
            resampler_frames_out: rs.frames_out,
            mixer_frames_in: self.stats.mixer_frames_in,
        }
    }

    /// Number of output frames buffered in the FIFO, ready for
    /// `drain`.
    pub fn pending_output_frames(&self) -> usize {
        let oc = self.mixer.output_channels as usize;
        (self.fifo.len() - self.fifo_read)
            .checked_div(oc)
            .unwrap_or(0)
    }

    /// Push `input` (interleaved f32 at input-rate, input-channels)
    /// through the resampler and mixer. Output is appended to the
    /// internal FIFO; use `drain` to pull it into a cpal buffer.
    pub fn feed(&mut self, input: &[f32]) {
        let ic = self.resampler.input_channels as usize;
        let oc = self.mixer.output_channels as usize;
        let in_frames = input.len() / ic;
        if in_frames == 0 {
            return;
        }

        // Sizing: the resampler may produce up to
        // ceil(in_frames * output_rate / input_rate) + a small
        // slack for carry alignment. Generous bound avoids
        // per-call resize.
        let needed_scratch_frames = (in_frames as u64 * self.resampler.output_rate_hz as u64)
            .div_ceil(self.resampler.input_rate_hz as u64)
            as usize
            + 16;

        let scratch_needed = needed_scratch_frames * ic;
        if self.scratch.len() < scratch_needed {
            self.scratch.resize(scratch_needed, 0.0);
        }

        let resampled = self
            .resampler
            .process(input, &mut self.scratch[..scratch_needed]);
        if resampled == 0 {
            return;
        }

        let scratch_used = resampled * ic;

        // Run mixer directly into the tail of the FIFO.
        let fifo_start = self.fifo.len();
        self.fifo.resize(fifo_start + resampled * oc, 0.0);
        let mixed = self
            .mixer
            .process(&self.scratch[..scratch_used], &mut self.fifo[fifo_start..]);
        // Trim any excess (mixed may be < resampled if mixer
        // capacity was tighter — defensive).
        self.fifo.truncate(fifo_start + mixed * oc);

        // Stats.
        let rs = self.resampler.stats();
        self.stats.samples_in += input.len() as u64;
        self.stats.samples_out += mixed as u64 * oc as u64;
        self.stats.resampler_frames_in = rs.frames_in;
        self.stats.resampler_frames_out = rs.frames_out;
        self.stats.mixer_frames_in += mixed as u64;
    }

    /// Copy accumulated output from the FIFO into `out`. Returns
    /// the number of OUTPUT frames written. Advances the internal
    /// read cursor; compacts when the consumed region exceeds 4096
    /// samples and is past the halfway mark.
    pub fn drain(&mut self, out: &mut [f32]) -> usize {
        let oc = self.mixer.output_channels as usize;
        let available = self.fifo.len() - self.fifo_read;
        let copy_len = available.min(out.len());
        if copy_len == 0 {
            return 0;
        }

        out[..copy_len].copy_from_slice(&self.fifo[self.fifo_read..self.fifo_read + copy_len]);
        self.fifo_read += copy_len;

        // Compact: when all consumed, clear outright. When the
        // consumed prefix is large, shift the remainder forward.
        if self.fifo_read == self.fifo.len() {
            self.fifo.clear();
            self.fifo_read = 0;
        } else if self.fifo_read > 4096 && self.fifo_read > self.fifo.len() / 2 {
            let unread = self.fifo.len() - self.fifo_read;
            self.fifo.copy_within(self.fifo_read.., 0);
            self.fifo.truncate(unread);
            self.fifo_read = 0;
        }

        copy_len / oc
    }

    /// Convert `input` (interleaved f32 at input-rate, input-channels)
    /// into the cpal output buffer `out` (interleaved f32 at
    /// output-rate, output-channels). Returns the number of OUTPUT
    /// frames written. Convenience wrapper: `feed` then `drain`.
    pub fn process(&mut self, input: &[f32], out: &mut [f32]) -> usize {
        self.feed(input);
        self.drain(out)
    }
}

/// Cumulative diagnostics for one [`FormatPipeline`] instance.
/// Reported via `--stats` so operators can see the conversion
/// activity (rate × channels) over the receiver's lifetime.
///
/// **Field semantics**:
/// * `samples_in / samples_out` — interleaved f32 sample counts at
///   the input / output sides.
/// * `resampler_frames_in / out` — frames the resampler actually
///   consumed / produced.
/// * `mixer_frames_in` — frames leaving the resampler (= frames
///   entering the mixer); frames leaving the mixer equal the output
///   frame count.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FormatStats {
    pub samples_in: u64,
    pub samples_out: u64,
    pub resampler_frames_in: u64,
    pub resampler_frames_out: u64,
    pub mixer_frames_in: u64,
}

#[cfg(test)]
mod unit {
    use super::*;

    fn resampler_between(ir: u32, or: u32) -> LinearResampler {
        LinearResampler::new(ir, or, 1)
    }

    #[test]
    fn resampler_is_passthrough_only_at_exact_1_to_1() {
        assert!(resampler_between(48_000, 48_000).is_passthrough());
        assert!(!resampler_between(48_000, 44_100).is_passthrough());
        assert!(!resampler_between(44_100, 48_000).is_passthrough());
    }

    #[test]
    fn resampler_identity_passes_each_input_through_one_output() {
        // 48 kHz mono, 8 input frames → 7 output frames.
        // Phase-continuous: the last input frame has no right anchor
        // so it is not emitted in this call (carried as left anchor).
        let mut r = resampler_between(48_000, 48_000);
        let input: Vec<f32> = (0..8).map(|i| i as f32).collect();
        let mut out = vec![0.0_f32; 9];
        let n = r.process(&input, &mut out);
        assert_eq!(
            n, 7,
            "1:1 ratio: 8 inputs → 7 outputs (last is left anchor)"
        );
        for (i, v) in out.iter().enumerate().take(7) {
            assert_eq!(*v, i as f32, "out[{i}] must equal input frame {i}");
        }
    }

    #[test]
    fn resampler_dc_input_remains_constant_across_ratios() {
        // DC = 0.5 must round-trip as 0.5 across every ratio.
        for (ir, or) in [
            (48_000_u32, 48_000_u32),
            (48_000, 44_100),
            (96_000, 48_000),
            (44_100, 48_000),
        ] {
            let mut r = LinearResampler::new(ir, or, 1);
            let input = vec![0.5_f32; 200];
            let mut out = vec![0.0_f32; 240];
            let n = r.process(&input, &mut out);
            assert!(n > 0, "rate pair ({ir}→{or}) produced no output");
            for (f, &v) in out[..n].iter().enumerate() {
                assert!(
                    (v - 0.5).abs() < 1e-6,
                    "DC drift at ({ir}→{or}) frame {f}: {v}"
                );
            }
        }
    }

    #[test]
    fn resampler_2_to_1_downsample_interpolates_between_pairs() {
        // 96 kHz → 48 kHz (2:1). DC input.
        let mut r = resampler_between(96_000, 48_000);
        assert!(!r.is_passthrough());
        let input = vec![0.5_f32; 10];
        let mut out = vec![0.0_f32; 6];
        let n = r.process(&input, &mut out);
        assert!(
            (4..=6).contains(&n),
            "expected 4-6 outputs for 10 DC inputs, got {n}"
        );
        for v in &out[..n] {
            assert!((v - 0.5).abs() < 1e-6, "DC drift: {v}");
        }
    }

    #[test]
    fn resampler_1_to_2_upsample_walks_at_half_step() {
        // 48 kHz → 96 kHz (1:2 upsample). Increasing ramp.
        // Phase-continuous: produces fewer outputs than the old
        // reseed-per-call loop, but the values are correct.
        let mut r = resampler_between(48_000, 96_000);
        assert!(!r.is_passthrough());
        let input: Vec<f32> = (0..5).map(|i| i as f32).collect();
        let mut out = vec![0.0_f32; 12];
        let n = r.process(&input, &mut out);
        // With phase-continuous Q32 and no echo, 5 input frames
        // at 2× produce 8 outputs (each pair of inputs yields ~4
        // outputs, minus the last frame which has no right anchor).
        assert_eq!(n, 8, "1:2 upsample: 5 inputs → 8 outputs");
        assert_eq!(out[0], 0.0, "seeded first output (t=0)");
        assert!((out[1] - 0.5).abs() < 1e-6, "half-step interp");
        assert_eq!(out[2], 1.0, "anchor at input[1]");
        assert!((out[3] - 1.5).abs() < 1e-6, "t=0.5 between [1] and [2]");
        assert_eq!(out[6], 3.0, "anchor at input[3]");
        assert!((out[7] - 3.5).abs() < 1e-6, "t=0.5 between [3] and [4]");
    }

    #[test]
    fn resampler_keeps_running_process_via_carry() {
        // Feed in two batches; verify second batch picks up where
        // the first left off (no skip, no duplicate, no reseed).
        let mut r = resampler_between(96_000, 48_000);
        let first = vec![1.0_f32; 4];
        let second = vec![2.0_f32; 4];
        let mut out1 = vec![0.0_f32; 4];
        let n1 = r.process(&first, &mut out1);
        assert!(n1 >= 1, "first batch produced at least 1 output");
        let mut out2 = vec![0.0_f32; 4];
        let n2 = r.process(&second, &mut out2);
        for v in out1.iter().take(n1) {
            // First batch: all DC=1.0 (input is constant 1.0)
            assert!((v - 1.0).abs() < 1e-6, "first batch DC {v}");
        }
        for v in out2.iter().take(n2) {
            // Second batch: may be 1.5 (carry from first) or 2.0
            assert!(
                (v - 2.0).abs() < 1e-6 || (v - 1.5).abs() < 1e-6,
                "second batch DC merge {v}"
            );
        }
    }

    #[test]
    fn mixer_passthrough_copies_in_place() {
        let m = ChannelMixer::new(2, 2);
        assert!(m.is_passthrough());
        let input = [1.0_f32, -1.0, 0.5, 0.25];
        let mut out = [0.0_f32; 4];
        let n = m.process(&input, &mut out);
        assert_eq!(n, 2);
        assert_eq!(out, input);
    }

    #[test]
    fn mixer_mono_to_stereo_broadcasts() {
        let m = ChannelMixer::new(1, 2);
        let input = [0.3_f32, -0.7, 0.0];
        let mut out = [0.0_f32; 6];
        let n = m.process(&input, &mut out);
        assert_eq!(n, 3);
        assert_eq!(out, [0.3, 0.3, -0.7, -0.7, 0.0, 0.0]);
    }

    #[test]
    fn mixer_stereo_to_mono_averages_l_plus_r_over_two() {
        let m = ChannelMixer::new(2, 1);
        let input = [0.4_f32, 0.6, -1.0, 1.0];
        let mut out = [0.0_f32; 2];
        let n = m.process(&input, &mut out);
        assert_eq!(n, 2);
        assert_eq!(out[0], 0.5);
        assert_eq!(out[1], 0.0);
    }

    #[test]
    fn mixer_4_to_2_averages_pairs() {
        let m = ChannelMixer::new(4, 2);
        let input = [1.0_f32, 3.0, 5.0, 7.0];
        let mut out = [0.0_f32; 2];
        let n = m.process(&input, &mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0], 2.0);
        assert_eq!(out[1], 6.0);
    }

    #[test]
    fn mixer_4_to_1_averages_all_four() {
        let m = ChannelMixer::new(4, 1);
        let input = [1.0, 2.0, 3.0, 4.0];
        let mut out = [0.0_f32; 1];
        let n = m.process(&input, &mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0], 2.5);
    }

    #[test]
    fn mixer_1_to_3_defensive_zero_pads() {
        // v1 ships mono↔stereo only; this branch is defensive.
        let m = ChannelMixer::new(1, 3);
        let input = [0.42_f32, 0.0];
        let mut out = [0.0_f32; 6];
        let n = m.process(&input, &mut out);
        assert_eq!(n, 2);
        assert_eq!(out, [0.42, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn pipeline_passthrough_copies_correctly() {
        let mut p = FormatPipeline::new(48_000, 48_000, 2, 2);
        assert!(p.is_passthrough());
        let input = [1.0_f32, -1.0, 0.5, 0.25, 0.0, 0.75, -0.5, 1.0];
        let mut out = [0.0_f32; 8];
        let n = p.process(&input, &mut out);
        // 4 stereo frames in, 3 out (last frame has no right anchor
        // in this call — carried as left anchor for the next call).
        assert_eq!(n, 3, "passthrough: 4 frames → 3 (last carried)");
        // Verify the first 3 frames match.
        assert_eq!(&out[..6], &[1.0, -1.0, 0.5, 0.25, 0.0, 0.75]);
    }

    #[test]
    fn pipeline_stereo_to_mono_passes_mixer_only() {
        let mut p = FormatPipeline::new(48_000, 48_000, 2, 1);
        assert!(!p.is_passthrough());
        let input = [0.4_f32, 0.6, -1.0, 1.0, 0.0, 0.0];
        let mut out = [0.0_f32; 3];
        let n = p.process(&input, &mut out);
        // 3 stereo frames → 2 mono frames (last carried).
        assert_eq!(n, 2, "stereo→mono: 3 frames → 2 (last carried)");
        assert_eq!(out[0], 0.5);
        assert_eq!(out[1], 0.0);
    }

    #[test]
    fn pipeline_mono_to_stereo_passes_mixer_only() {
        let mut p = FormatPipeline::new(48_000, 48_000, 1, 2);
        assert!(!p.is_passthrough());
        let input = [0.3_f32, -0.7, 0.0];
        let mut out = [0.0_f32; 6];
        let n = p.process(&input, &mut out);
        // 3 mono frames → 2 stereo frames (last carried).
        assert_eq!(n, 2, "mono→stereo: 3 frames → 2 (last carried)");
        assert_eq!(out[..4], [0.3, 0.3, -0.7, -0.7]);
    }

    #[test]
    fn pipeline_48_stereo_to_44_1_mono_preserves_dc() {
        // DC = (0.5, 0.5) → resampler DC → mixer averages → 0.5.
        let mut p = FormatPipeline::new(48_000, 44_100, 2, 1);
        assert!(!p.is_passthrough());
        let input = vec![0.5_f32; 400]; // 200 stereo frames × 2 channels
        p.feed(&input);
        let n = p.pending_output_frames();
        assert!(n > 0, "feed must produce output frames");
        let mut out = vec![0.0_f32; n];
        p.drain(&mut out);
        for (f, &v) in out[..n].iter().enumerate() {
            assert!((v - 0.5).abs() < 1e-6, "DC drift at frame {f}: {v}",);
        }
        let s = p.stats();
        assert!(s.samples_in > 0);
        assert!(s.samples_out > 0);
    }

    #[test]
    fn pipeline_grows_scratch_on_oversized_input() {
        // Feed 10000 input frames to force a resize; verify
        // scrambling doesn't lose samples.
        let mut p = FormatPipeline::new(48_000, 44_100, 2, 2);
        let n_frames = 10_000;
        let input = [0.8_f32, -0.8_f32].repeat(n_frames);
        p.feed(&input);
        let n = p.pending_output_frames();
        let mut out = vec![0.0_f32; n * 2];
        p.drain(&mut out);
        let expected_n = (n_frames * 44_100).div_ceil(48_000);
        // Allow ±1 for rounding at the last anchor boundary.
        assert!(
            (n as i64 - expected_n as i64).unsigned_abs() <= 1,
            "expected ~{expected_n} output frames, got {n}"
        );
        for f in 0..n {
            let l = out[f * 2];
            let r = out[f * 2 + 1];
            assert!((l - 0.8).abs() < 1e-6, "DC drift on L at {f}: {l}");
            assert!((r + 0.8).abs() < 1e-6, "DC drift on R at {f}: {r}");
        }
    }

    #[test]
    fn pipeline_high_ratio_upsample_fills_output_capacity() {
        // Regression: the intermediate scratch buffer used to assume a
        // worst-case 2× upsample. That silently truncated legitimate
        // high-ratio conversions like 8 kHz input into a 192 kHz output
        // device, then the caller zero-filled most of the output callback.
        let mut p = FormatPipeline::new(8_000, 192_000, 1, 1);
        let input = vec![0.25_f32; 21];
        p.feed(&input);
        let n = p.pending_output_frames();
        let mut out = vec![0.0_f32; n];
        p.drain(&mut out);
        assert!(n >= 480, "24× upsample should produce ≥480 frames, got {n}");
        for (f, &v) in out[..n].iter().enumerate() {
            assert!((v - 0.25).abs() < 1e-6, "DC drift at frame {f}: {v}");
        }
    }

    #[test]
    fn resampler_drift_regression_48k_to_44_1k() {
        // 500 consecutive 512-frame output requests at 48k→44.1k
        // must consume total_out × 48/44.1 ± 2 input frames.
        let mut p = FormatPipeline::new(48_000, 44_100, 1, 1);
        let mut total_in: u64 = 0;
        let mut total_out: u64 = 0;
        let mut out_buf = vec![0.0_f32; 512];

        while total_out < 500 * 512 {
            // Feed one frame at a time to simulate streaming.
            p.feed(&[0.5_f32]);
            total_in += 1;
            while p.pending_output_frames() >= 512 {
                let n = p.drain(&mut out_buf);
                assert_eq!(n, 512, "drain must return exactly 512 frames");
                total_out += n as u64;
            }
        }

        let expected_in = total_out as f64 * 48_000.0 / 44_100.0;
        let diff = (total_in as f64 - expected_in).abs();
        assert!(
            diff <= 2.0,
            "drift exceeded ±2 input frames: total_in={total_in} \
             expected={expected_in:.1} diff={diff:.3}"
        );
    }

    #[test]
    fn pipeline_feed_drain_pending_round_trip() {
        // Verify feed/drain/pending_output_frames work together.
        let mut p = FormatPipeline::new(48_000, 44_100, 1, 1);
        assert_eq!(p.pending_output_frames(), 0, "no output before feed");

        p.feed(&[0.5_f32; 480]); // 10 ms at 48 kHz
        let pending = p.pending_output_frames();
        assert!(pending > 0, "feed must produce output");
        assert!(
            pending <= 441,
            "10 ms at 44.1 kHz ≈ 441 frames max, got {pending}"
        );

        let mut out = vec![0.0_f32; pending];
        let drained = p.drain(&mut out);
        assert_eq!(drained, pending, "drain must return all pending frames");
        assert_eq!(
            p.pending_output_frames(),
            0,
            "FIFO must be empty after full drain"
        );
        for (f, &v) in out.iter().enumerate() {
            assert!((v - 0.5).abs() < 1e-6, "DC drift at frame {f}: {v}");
        }
    }
}
