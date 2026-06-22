//! Integration tests for the `generated` module — the deterministic
//! sine source used by `--sine` dry-run mode and the localhost smoke
//! integration test.

use teehee::generated::SineSource;

/// Build the default 48 kHz stereo 440 Hz source.
fn source() -> SineSource {
    SineSource::new(48_000, 2, 440.0)
}

#[test]
fn first_stereo_frame_at_phase_zero_is_zero() {
    // One stereo frame = 2 samples. The phase at sample 0 is exactly 0.
    let mut s = source();
    let mut out = [99.0_f32; 2];
    s.fill_chunk(&mut out);
    assert!(out[0].abs() < 1e-6, "L = {} (want ~0)", out[0]);
    assert!(out[1].abs() < 1e-6, "R = {} (want ~0)", out[1]);
}

#[test]
fn stereo_channels_broadcast_the_same_value_per_frame() {
    // L == R per frame in the dry-run source (mono broadcast).
    let mut s = source();
    let mut out = [0.0_f32; 512];
    s.fill_chunk(&mut out);
    for frame in 0..(out.len() / 2) {
        assert_eq!(
            out[frame * 2],
            out[frame * 2 + 1],
            "L != R at frame {frame}"
        );
    }
}

#[test]
fn next_chunk_continues_phase_where_previous_left_off() {
    // Two 4-sample chunks should equal one 8-sample chunk from a fresh source.
    let mut split = source();
    let mut split_buf = [0.0_f32; 8];
    split.fill_chunk(&mut split_buf[..4]);
    split.fill_chunk(&mut split_buf[4..]);

    let mut whole = source();
    let mut whole_buf = [0.0_f32; 8];
    whole.fill_chunk(&mut whole_buf);

    let eps = 1e-6;
    for i in 0..8 {
        assert!(
            (split_buf[i] - whole_buf[i]).abs() < eps,
            "sample {i}: split={} whole={}",
            split_buf[i],
            whole_buf[i]
        );
    }
}

#[test]
fn samples_are_bounded_between_minus_one_and_one() {
    let mut s = source();
    let mut out = [0.0_f32; 4096];
    s.fill_chunk(&mut out);
    let min = out.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = out.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    assert!(min >= -1.0, "min sample out of range: {min}");
    assert!(max <= 1.0, "max sample out of range: {max}");
}

#[test]
fn sine_produces_full_period_at_unitary_sample_rate() {
    // sample_rate = 4, freq = 1, channels = 1. One period is exactly
    // 4 samples: 0, +1, 0, -1. This is the simplest deterministic
    // end-to-end check on the phase math. Use `~=` (within epsilon)
    // because f64 π has rounding error.
    let mut s = SineSource::new(4, 1, 1.0);
    let mut buf = [0.0_f32; 4];
    s.fill_chunk(&mut buf);
    let eps = 1e-5;
    assert!(buf[0].abs() < eps, "start = {} (want ~0)", buf[0]);
    assert!(
        (buf[1] - 1.0).abs() < eps,
        "quarter = {} (want ~+1)",
        buf[1]
    );
    assert!(buf[2].abs() < eps, "half = {} (want ~0)", buf[2]);
    assert!(
        (buf[3] - -1.0).abs() < eps,
        "three-quarter = {} (want ~-1)",
        buf[3]
    );
}

#[test]
fn stereo_output_broadcasts_one_phase_per_frame() {
    let mut s = SineSource::new(4, 2, 1.0);
    let mut buf = [0.0_f32; 8]; // 4 stereo frames
    s.fill_chunk(&mut buf);
    // Pinned to integer sample_rate so phase land on exact multiples of
    // π/2; still use epsilon because f64 → f32 casts of values like
    // sin(π) introduce a residual ~1e-16.
    let eps = 1e-5;
    assert!(buf[0].abs() < eps, "L0 = {}", buf[0]);
    assert!(buf[1].abs() < eps, "R0 = {}", buf[1]);
    assert!((buf[2] - 1.0).abs() < eps, "L1 = {}", buf[2]);
    assert!((buf[3] - 1.0).abs() < eps, "R1 = {}", buf[3]);
    assert!(buf[4].abs() < eps, "L2 = {}", buf[4]);
    assert!(buf[5].abs() < eps, "R2 = {}", buf[5]);
    assert!((buf[6] - -1.0).abs() < eps, "L3 = {}", buf[6]);
    assert!((buf[7] - -1.0).abs() < eps, "R3 = {}", buf[7]);
}
