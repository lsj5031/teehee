//! Integration tests for the slice-7 receiver format-reconciliation
//! pipeline (Tier 2 #6). All access goes through the public
//! `teehee::` lib-root API so a regression in any module's re-export
//! or public signature surfaces here, not only inside the modules'
//! own unit tests.
//!
//! The cross-module exercise is the JitterBuffer → FormatPipeline
//! flow that `main.rs::run_recv` performs on every cpal data
//! callback once both formats are known. The unit tests in
//! `src/format_pipeline.rs::unit` and `src/jitter.rs::unit` already
//! pin each piece in isolation; the value of this file is to verify
//! the *composition* — sizing math, channel-reconciliation chaining,
//! and pass-through identity when formats match.

use teehee::format_pipeline::{FormatPipeline, FormatStats};
use teehee::jitter::JitterBuffer;

/// 4 packets × 20 ms at 48 kHz stereo = 1920 interleaved f32 samples
/// per packet (960 frames × 2 channels = 1920 samples). Matches the
/// sender's slice-3 default. Used as a small-but-realistic fixture.
const SAMPLES_PER_PACKET: usize = 1920;

/// 60 ms of DC = 0.5 stereo @ 48 kHz (3 packets × 20 ms each). Push 3
/// packets, then drain the jitter buffer in ONE big pop and pass
/// through a FormatPipeline targeting 44.1 kHz mono. Resampler DC
/// must survive the sample-rate conversion; mixer DC must survive
/// the L+R average.
///
/// This is the exact flow that `main.rs::run_recv` exercises per
/// cpal callback once the first packet sets both formats.
#[test]
fn jitter_drain_through_pipeline_preserves_dc_across_sr_and_channels() {
    // Sender format: 48 kHz / stereo. Receiver format: 44.1 kHz / mono.
    // The format pipeline MUST convert BOTH axes simultaneously,
    // and DC must round-trip cleanly end-to-end.
    let mut jb = JitterBuffer::new(SAMPLES_PER_PACKET, 32, None);
    for seq in 0..3u32 {
        let packet = vec![0.5_f32; SAMPLES_PER_PACKET];
        let outcome = jb.push(seq, &packet);
        assert_eq!(outcome, teehee::jitter::PushOutcome::Stored);
    }

    let mut pipeline = FormatPipeline::new(48_000, 44_100, 2, 1);
    assert!(
        !pipeline.is_passthrough(),
        "stereo→mono, 48→44.1 must be active"
    );

    // Drain the jitter buffer in a single big pop. pop_frames always
    // returns out.len() (it pads with silence), so the pipeline gets a
    // fully populated input regardless of how the upstream scheduling
    // chunked the drain.
    let mut input_scratch = vec![0.0_f32; 3 * SAMPLES_PER_PACKET];
    let popped = jb.pop_frames(&mut input_scratch);
    assert_eq!(
        popped,
        input_scratch.len(),
        "pop_frames must return out.len() per its contract"
    );
    for (f, &val) in input_scratch[..popped].iter().enumerate() {
        assert!(
            (val - 0.5).abs() < 1e-6,
            "queued audio must be DC = 0.5 across all {popped} input samples, got {val} at {f}",
        );
    }

    // Pipeline target capacity = ceil(2880 frames × 44100/48000) + slack.
    // 2880 / 48 kHz × 44.1 kHz ≈ 2646; round up to 2700 to leave headroom.
    let output_cap = 2700;
    let mut output = vec![0.0_f32; output_cap];
    let written = pipeline.process(&input_scratch, &mut output);
    assert!(
        written > 0,
        "pipeline must produce at least one output frame"
    );
    // 2880 input frames @ 48→44.1 → natural_end = ceil(2880 × 44100 / 48000)
    // = 2646 exact. The unit-test `resampler_keeps_running_process_via_carry`
    // pins the contract that carry resets across batches, so 2646 is the
    // canonical output frame count; we allow ±1 for fixed-point carry
    // rounding at the trailing edge.
    let expected = 2646;
    assert!(
        written == expected || written == expected - 1 || written == expected + 1,
        "expected {expected} ±1 output frames (resampler carry boundary), got {written}"
    );
    for (f, &v) in output[..written].iter().enumerate() {
        // (L + R) avg for DC stereo = (0.5 + 0.5) / 2 = 0.5; resampler
        // passes DC straight through (tested separately in unit).
        assert!((v - 0.5).abs() < 1e-5, "DC drift at output frame {f}: {v}");
    }

    // Stats line shape — confirm the fields appear as expected so
    // the spawn_periodic_receiver_stats macro doesn't break.
    let s: FormatStats = pipeline.stats();
    assert_eq!(s.samples_in as usize, input_scratch.len());
    assert_eq!(
        s.samples_out as usize, written,
        "samples_out accounts only for what hit the output buffer"
    );
    assert!(s.resampler_frames_in > 0);
    assert!(s.resampler_frames_out > 0);
    assert!(
        s.mixer_frames_in > 0,
        "mixer must have processed resampled frames"
    );
}

/// When sender format == receiver format, FormatPipeline is a no-op
/// (is_passthrough returns true). This is the "common case" the
/// cpal callback's hot path optimizes for — pop_frames runs, but
/// Process() doesn't actually resample or remix.
#[test]
fn pipeline_passthrough_via_jitter_is_byte_identical() {
    let mut jb = JitterBuffer::new(SAMPLES_PER_PACKET, 32, None);
    // Push 1 packet of `[0.1, -0.1, 0.2, -0.2, ...]` interleaved stereo.
    let mut stereo_input: Vec<f32> = Vec::with_capacity(SAMPLES_PER_PACKET);
    for i in 0..(SAMPLES_PER_PACKET / 2) {
        stereo_input.push(0.1 + 0.001 * i as f32);
        stereo_input.push(-0.1 - 0.001 * i as f32);
    }
    assert_eq!(stereo_input.len(), SAMPLES_PER_PACKET);
    let outcome = jb.push(0, &stereo_input);
    assert_eq!(outcome, teehee::jitter::PushOutcome::Stored);

    let mut pipeline = FormatPipeline::new(48_000, 48_000, 2, 2);
    assert!(
        pipeline.is_passthrough(),
        "format-identity must be detected"
    );

    // Drain 1920 samples (= 1 packet exactly) so pop_frames returns
    // 1920 samples — verifiable against the pipeline output without
    // rolling over to the next packet.
    let mut scratch = vec![0.0_f32; SAMPLES_PER_PACKET];
    let popped = jb.pop_frames(&mut scratch);
    assert_eq!(popped, SAMPLES_PER_PACKET);

    let mut out = vec![0.0_f32; SAMPLES_PER_PACKET];
    let written = pipeline.process(&scratch, &mut out);
    assert_eq!(
        written,
        SAMPLES_PER_PACKET / 2,
        "passthrough = one frame per input frame"
    );
    for f in 0..written {
        for c in 0..2 {
            assert_eq!(
                out[f * 2 + c],
                scratch[f * 2 + c],
                "passthrough byte-identity at frame {f} channel {c}"
            );
        }
    }
}

/// Sender 96 kHz / stereo → Receiver 48 kHz / mono (2× downsample +
/// down-mix). Verifies the cpal-callback scratch-sizing math from
/// `main.rs::run_recv` matches the public-API math: asking for ~960
/// OUTPUT frames (at 48 kHz mono) requires 1920 INPUT frames (at 96
/// kHz stereo).
#[test]
fn scratch_sizing_2x_downsample_to_mono() {
    let mut jb = JitterBuffer::new(SAMPLES_PER_PACKET, 32, None);
    // 4 packets @ 96 kHz stereo → 8 packets' worth of audio in
    // input-rate time. Use a single pre-allocated vec for all pushes
    // to avoid per-iteration allocation churn.
    let mut packet = vec![0.7_f32; SAMPLES_PER_PACKET];
    for seq in 0..4u32 {
        let outcome = jb.push(seq, &packet);
        assert_eq!(outcome, teehee::jitter::PushOutcome::Stored);
    }
    packet.clear();
    let mut pipeline = FormatPipeline::new(96_000, 48_000, 2, 1);

    // Pop all 4 packets at once.
    let mut input_scratch = vec![0.0_f32; 4 * SAMPLES_PER_PACKET];
    let popped = jb.pop_frames(&mut input_scratch);
    assert_eq!(popped, input_scratch.len());

    // Math sanity: 4 packets × 960 input frames each = 3840 input
    // frames at 96 kHz. Pipeline 96 kHz → 48 kHz is 2× downsample
    // (one output per ~2 input frames in time), so output frames =
    // 3840 / 2 = 1920. Confirming natural_end-computed-by-resampler
    // matches: ceil(3840 × 48000 / 96000) = 1920 exact, see the
    // existing `resampler_2_to_1_downsample_interpolates_between_pairs`
    // unit test which pins the ratio against 10 inputs → 5 outputs.
    let mut output = vec![0.0_f32; 1920];
    let written = pipeline.process(&input_scratch, &mut output);
    // Allow ±1 for fixed-point carry rounding at the trailing edge.
    let expected = 1920;
    assert!(
        written == expected || written == expected - 1 || written == expected + 1,
        "expected {expected} ±1 output frames for 2x downsample, got {written}"
    );
    for (f, &v) in output[..written].iter().enumerate() {
        assert!(
            (v - 0.7).abs() < 1e-5,
            "DC drift at 2x-downsample output frame {f}: {v}"
        );
    }
}
