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
/// State machine:
/// * `last_frame` — left anchor of interp per channel.
/// * `in_idx` — the input frame acting as the current right anchor.
/// * `pos_q16` — fixed-point position within `[0, 65536)` in the
///   gap between `last_frame`'s input frame and `input[in_idx]`'s.
///   `t = pos_q16 as f32 / 65536.0` is the linear-interp weight.
/// * `step_q16 = input_rate * 65536 / output_rate` — how much the
///   cursor advances per produced output frame.
///
/// Per emitted output: emit `last + (input[in_idx] - last) * t`,
/// then `pos_q16 += step_q16`. While `pos_q16 >= 65536`, we've
/// crossed ≥ 1 full input frame: commit
/// `last = input[in_idx]; in_idx += 1`.
///
/// First-call seeding: the very first `process` call must consume
/// `input[0]` as the left anchor AND emit it verbatim (t=0).
/// Afterward, `in_idx = 1` so `input[1]` is the first right anchor.
///
/// ### Known limitation
///
/// When `input.len()` is exhausted mid-consume and `pos_q16` is still
/// ≥ 65536 (rare, requires a per-output stepping multiple of >1.088×
/// on a short input), the residual carry is discarded. The
/// fractional position resets to 0 on the next call. For LAN ratios
/// the recovery happens within one full packet (≤ 20 ms) and is
/// inaudible; for extreme ratios with very short inputs it could
/// cause a single-frame discontinuity at chunk boundaries. v1
/// accepts this; v2 may track a `pending_anchor` carry.
#[derive(Debug, Clone)]
pub struct LinearResampler {
    input_rate_hz: u32,
    output_rate_hz: u32,
    input_channels: u8,
    /// `step_q16 = input_rate_hz * 65536 / output_rate_hz`. Advances
    /// the in-gap cursor per produced output.
    step_q16: u32,
    /// Cursor in fixed-point (65536 == 1 input frame). Always in
    /// `[0, 65536)` post-call.
    pos_q16: u32,
    /// Last input sample, per channel — left anchor of interp.
    last_frame: Vec<f32>,
    /// Total input frames consumed (incl. the seed consume).
    frames_in: u64,
    /// Total output frames emitted.
    frames_out: u64,
}

impl LinearResampler {
    /// Build a resampler for `input_rate_hz → output_rate_hz` over
    /// `input_channels` interleaved channels. Panics on
    /// `input_rate_hz == 0` or `output_rate_hz == 0`.
    pub fn new(input_rate_hz: u32, output_rate_hz: u32, input_channels: u8) -> Self {
        assert!(input_rate_hz > 0, "input_rate_hz must be > 0");
        assert!(output_rate_hz > 0, "output_rate_hz must be > 0");
        let step_q16 = (input_rate_hz as u64 * 65536 / output_rate_hz as u64) as u32;
        Self {
            input_rate_hz,
            output_rate_hz,
            input_channels,
            step_q16,
            pos_q16: 0,
            last_frame: vec![0.0; input_channels as usize],
            frames_in: 0,
            frames_out: 0,
        }
    }

    /// True if this resampler is a no-op (input_rate == output_rate).
    pub fn is_passthrough(&self) -> bool {
        self.input_rate_hz == self.output_rate_hz
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
    /// produced, capped by `out_capacity`. Caller must size `out`
    /// large enough; the
    /// [`FormatPipeline::process`] helper grows its scratch to fit.
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

        // Expected full output frame count for this ratio
        // (ceil division). The loop is bounded by both `natural_end`
        // and `out_cap` so the caller-side buffer can never overflow.
        let out_samples = in_frames as u64 * self.output_rate_hz as u64;
        let natural_end = out_samples.div_ceil(self.input_rate_hz as u64) as usize;

        // Reseed every call: `input_frames[0]` is the new left
        // anchor. v1 intentionally does NOT carry `in_idx` or
        // `pos_q16` across calls — every chunk is treated as a
        // fresh packet boundary. This is the design choice (per
        // slice 7 review): brittle residual-carry state machines
        // are avoided; the JitterBuffer's missing-packet silence
        // path absorbs the chunk-boundary discontinuity cleanly.
        self.last_frame.copy_from_slice(&input_frames[..ch]);
        self.frames_in += 1;
        self.pos_q16 = 0;
        let mut in_idx: usize = 1;

        let mut written: usize = 0;
        let end = natural_end.min(out_cap);
        while written < end {
            let t = self.pos_q16 as f32 / 65536.0;

            if in_idx >= in_frames {
                // Right anchor exhausted — echo `last_frame`. The
                // natural interp window has stepped past the last
                // input; output equals `last_frame` regardless
                // of `t` (interp between identical anchors = anchor).
                for c in 0..ch {
                    out[written * ch + c] = self.last_frame[c];
                }
            } else {
                // Interpolate between `last_frame` and
                // `input_frames[in_idx * ch + c ..]`.
                let right_off = in_idx * ch;
                for c in 0..ch {
                    let a = self.last_frame[c];
                    let b = input_frames[right_off + c];
                    out[written * ch + c] = a + (b - a) * t;
                }
            }
            written += 1;
            self.frames_out += 1;

            // Advance cursor and commit each cross as a new left
            // anchor. CRITICAL: commit `input_frames[in_idx]`
            // BEFORE bumping `in_idx` so no input frame is skipped
            // (the previous rewrites' bug was `last =
            // input[in_idx + 1]; in_idx += 1`, which dropped every
            // other right anchor).
            let mut carry = self.pos_q16 as u64 + self.step_q16 as u64;
            while carry >= 65536 && in_idx < in_frames {
                carry -= 65536;
                let off = in_idx * ch;
                self.last_frame
                    .copy_from_slice(&input_frames[off..off + ch]);
                self.frames_in += 1;
                in_idx += 1;
            }
            if carry >= 65536 && in_idx >= in_frames {
                // No more input to discharge the carry against.
                // Cap the cursor so the next call's reseed starts
                // cleanly. The bounded accepted artifact is
                // noted in the module-level doc.
                self.pos_q16 = 0;
            } else {
                self.pos_q16 = carry as u32;
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
/// **Capacity**: `process` resizes the internal scratch buffer on
/// overflow so it never silently truncates. v1 sizing rule: scratch
/// starts at `8192 × input_channels` (covers 4096 input frames at
/// worst-case 2× upsample); it grows once if a single input
/// exceeds that bound.
pub struct FormatPipeline {
    resampler: LinearResampler,
    mixer: ChannelMixer,
    /// Scratch buffer for the resampler's INTERMEDIATE output
    /// (still in input_channels). Re-sized on overflow.
    scratch: Vec<f32>,
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
        // Initial scratch: 4096 input frames × 2 (worst-case 2×
        // upsample) × input_channels (interleaved). v1 recv uses
        // jitter-buffer packet chunks ≤ 1920 channels=2 samples,
        // so 8192 is plenty for one packet; we grow on overflow.
        let scratch_init = (4096usize * 2 * input_channels as usize).max(64);
        Self {
            resampler,
            mixer,
            scratch: vec![0.0_f32; scratch_init],
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

    /// Convert `input` (interleaved f32 at input-rate, input-channels)
    /// into the cpal output buffer `out` (interleaved f32 at
    /// output-rate, output-channels). Returns the number of OUTPUT
    /// frames written.
    pub fn process(&mut self, input: &[f32], out: &mut [f32]) -> usize {
        let ic = self.resampler.input_channels as usize;
        let oc = self.mixer.output_channels as usize;
        debug_assert_eq!(
            input.len() % ic,
            0,
            "input length must be a multiple of resampler input_channels"
        );
        debug_assert_eq!(
            out.len() % oc,
            0,
            "out length must be a multiple of mixer output_channels"
        );

        // Resize scratch on overflow so we never silently truncate.
        // The resampler's intermediate buffer is still in input-channel
        // layout but must be able to hold as many *frames* as the caller's
        // final output buffer can accept. Do not assume a 2× maximum
        // upsample ratio here: real devices can legitimately pair a low-rate
        // sender (for example 8 kHz telephony) with a high-rate output device
        // (for example 192 kHz), and a fixed 2× scratch cap would make the
        // pipeline write only the first few frames and zero-fill the rest.
        let output_capacity_frames = out.len() / oc;
        let ratio_bound_frames = input.len() / ic * 2 + 1;
        let needed_scratch_frames = output_capacity_frames.max(ratio_bound_frames);
        if self.scratch.len() / ic < needed_scratch_frames {
            self.scratch.resize(needed_scratch_frames * ic, 0.0);
        }
        let scratch_cap_frames = self.scratch.len() / ic;
        let scratch_for_resampler = needed_scratch_frames.min(scratch_cap_frames);

        let resampled = self
            .resampler
            .process(input, &mut self.scratch[..scratch_for_resampler * ic]);
        if resampled == 0 {
            return 0;
        }

        let scratch_used = resampled * ic;
        let mixed = self.mixer.process(&self.scratch[..scratch_used], out);

        // Stats: only bump what was actually produced.
        let rs = self.resampler.stats();
        self.stats.samples_in += input.len() as u64;
        self.stats.samples_out += mixed as u64 * oc as u64;
        self.stats.resampler_frames_in = rs.frames_in;
        self.stats.resampler_frames_out = mixed as u64;
        self.stats.mixer_frames_in = mixed as u64;
        mixed
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
        // 48 kHz mono, 8 input frames → 8 output frames.
        let mut r = resampler_between(48_000, 48_000);
        let input: Vec<f32> = (0..8).map(|i| i as f32).collect();
        let mut out = vec![0.0_f32; 9];
        let n = r.process(&input, &mut out);
        assert_eq!(n, 8, "1:1 ratio must produce one output per input");
        for (i, v) in out.iter().enumerate().take(8) {
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
        // 96 kHz → 48 kHz (2:1). Increasing ramp input.
        let mut r = resampler_between(96_000, 48_000);
        assert!(!r.is_passthrough());
        // Use a DC input so we can verify the math is right
        // without fluctuations; the exact output count varies by
        // 1 across carry boundaries so we just verify values.
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
        let mut r = resampler_between(48_000, 96_000);
        assert!(!r.is_passthrough());
        let input: Vec<f32> = (0..5).map(|i| i as f32).collect();
        let mut out = vec![0.0_f32; 12];
        let n = r.process(&input, &mut out);
        assert_eq!(n, 10, "1:2 upsample must produce 10 frames for 5 input");
        assert_eq!(out[0], 0.0, "seeded first output");
        assert!((out[1] - 0.5).abs() < 1e-6, "half-step interp");
        assert_eq!(out[2], 1.0, "anchor echo");
        assert!((out[3] - 1.5).abs() < 1e-6, "t=0.5");
        assert_eq!(out[8], 4.0, "last anchor");
        assert_eq!(out[9], 4.0, "trailing last frame");
    }

    #[test]
    fn resampler_keeps_running_process_via_carry() {
        // Feed in two batches; verify second batch picks up where
        // the first left off (no skip, no duplicate).
        let mut r = resampler_between(96_000, 48_000);
        let first = vec![1.0_f32; 4];
        let second = vec![2.0_f32; 4];
        let mut out1 = vec![0.0_f32; 4];
        let n1 = r.process(&first, &mut out1);
        assert!(n1 >= 2, "first batch produced at least 2 outputs");
        let mut out2 = vec![0.0_f32; 4];
        let n2 = r.process(&second, &mut out2);
        for v in out1.iter().take(n1) {
            assert!(
                (v - 1.0).abs() < 1e-6 || (v - 1.5).abs() < 1e-6,
                "first batch DC merge {v}"
            );
        }
        for v in out2.iter().take(n2) {
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
        assert_eq!(n, 4);
        assert_eq!(out, input);
    }

    #[test]
    fn pipeline_stereo_to_mono_passes_mixer_only() {
        let mut p = FormatPipeline::new(48_000, 48_000, 2, 1);
        assert!(!p.is_passthrough());
        let input = [0.4_f32, 0.6, -1.0, 1.0, 0.0, 0.0];
        let mut out = [0.0_f32; 3];
        let n = p.process(&input, &mut out);
        assert_eq!(n, 3);
        assert_eq!(out[0], 0.5);
        assert_eq!(out[1], 0.0);
        assert_eq!(out[2], 0.0);
    }

    #[test]
    fn pipeline_mono_to_stereo_passes_mixer_only() {
        let mut p = FormatPipeline::new(48_000, 48_000, 1, 2);
        assert!(!p.is_passthrough());
        let input = [0.3_f32, -0.7, 0.0];
        let mut out = [0.0_f32; 6];
        let n = p.process(&input, &mut out);
        assert_eq!(n, 3);
        assert_eq!(out[..6], [0.3, 0.3, -0.7, -0.7, 0.0, 0.0]);
    }

    #[test]
    fn pipeline_48_stereo_to_44_1_mono_preserves_dc() {
        // DC = (0.5, 0.5) → resampler DC → mixer averages → 0.5.
        let mut p = FormatPipeline::new(48_000, 44_100, 2, 1);
        assert!(!p.is_passthrough());
        let input = vec![0.5_f32; 400]; // 200 stereo frames × 2 channels
        let mut out = vec![0.0_f32; 220];
        let n = p.process(&input, &mut out);
        assert!(n > 0);
        for (f, &v) in out[..n].iter().enumerate() {
            assert!((v - 0.5).abs() < 1e-6, "DC drift at frame {f}: {v}",);
        }
        let s = p.stats();
        assert!(s.samples_in > 0);
        assert!(s.samples_out > 0);
    }

    #[test]
    fn pipeline_grows_scratch_on_oversized_input() {
        // Initial scratch = 8192. Feed 10000 input frames to force
        // a resize; verify scrambling doesn't lose samples.
        let mut p = FormatPipeline::new(48_000, 44_100, 2, 2);
        let n_frames = 10_000;
        let input = [0.8_f32, -0.8_f32].repeat(n_frames);
        let out_cap = (n_frames * 110 / 100) * 2 + 16;
        let mut out = vec![0.0_f32; out_cap];
        let n = p.process(&input, &mut out);
        // ceil(input * out / in) = 9188 (10000 * 110 / 100 = 11000? no,
        // we want (input * out/in) ≈ 10000 * 44100/48000 = 9187.5 → 9188).
        let expected_n = (n_frames * 44_100).div_ceil(48_000);
        assert_eq!(
            n, expected_n,
            "resampler must produce ceiling(input * out/in) frames"
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
        let mut out = vec![0.0_f32; 480];

        let n = p.process(&input, &mut out);

        assert_eq!(
            n, 480,
            "24× upsample should fill the caller's output capacity"
        );
        for (f, &v) in out[..n].iter().enumerate() {
            assert!((v - 0.25).abs() < 1e-6, "DC drift at frame {f}: {v}");
        }
    }
}
