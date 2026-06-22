//! `buffer_budget` — slice 10 (Tier 3 #9) receiver-side buffer
//! capacity helpers for teehee.
//!
//! Pure math module: converts the operator-supplied
//! `--rx-buffer-ms` value (the *total* receive-buffer depth in
//! milliseconds, including the prebuffer gate) into the
//! `capacity_packets` argument the `jitter::JitterBuffer` needs at
//! construction.
//!
//! ## Layer model
//!
//! Two receive-side knobs now coexist:
//!
//! * **`--prebuffer-ms`** (slice 6) is the *gate target* — the
//!   receiver waits in silence until at least this many ms have
//!   accumulated before starting playback. Stays treated as
//!   `queued_frames()` (interleaved f32 samples in
//!   input-rate × input-channels units) inside
//!   `JitterBuffer`.
//! * **`--rx-buffer-ms`** (slice 10) is the *ring depth* — total
//!   buffering capacity expressed in ms of audio. The ring must
//!   be sized to hold at least `--rx-buffer-ms` worth of packets
//!   so the receiver tolerates sender bursts / long reorders.
//!
//! The cross-flag invariant is `rx_buffer_ms >= prebuffer_ms` —
//! the gate target can never be larger than the ring itself,
//! otherwise the gate would block playback indefinitely because
//! no push could ever push enough samples into a too-small ring.
//!
//! ## Sizing
//!
//! Given `rx_buffer_ms`, `sample_rate_hz`, `channels`, and
//! `samples_per_packet`:
//!
//! ```text
//! rx_buffer_frames        = rx_buffer_ms * sample_rate * channels / 1000
//! rx_buffer_packets_ceil  = ceil(rx_buffer_frames / samples_per_packet)
//! capacity_packets        = max(32, rx_buffer_packets_ceil)
//! ```
//!
//! The `max(32, …)` floor is OS-memory-budget hygiene — a ring
//! with fewer than 32 packet slots is too small to absorb even a
//! mildly reordered 2-second stream and the prebuffer gate
//! targets on a fraction-of-second time base. The `>= prebuffer_ms`
//! invariant guarantees the gate target is reachable.
//!
//! ## Ring-overrun semantics (slice 10 second deliverable)
//!
//! Slice 10 also adds a `ring_overruns: u64` counter to
//! `jitter::Stats`. The counter increments when `push` overwrites
//! an unplayed future slot (sender outpaced the receiver long
//! enough that the ring wrapped around to a not-yet-drained
//! slot). The counter is distinct from the existing
//! `mid_read_collisions` (which only fires when the cpal callback
//! is currently mid-drain of the colliding slot — a much rarer
//! signature). See [`jitter::Stats::ring_overruns`] for details.
//!
//! [`jitter::Stats::ring_overruns`]: crate::jitter::Stats::ring_overruns

use thiserror::Error;

/// Minimum sensible `--rx-buffer-ms` value. 100 ms is a one-packet
/// floor at 48 kHz stereo / chunk_ms=20 (samples_per_packet = 1920),
/// so any ring capacity derivation falls above the `max(32, …)`
/// memory-hygiene floor naturally.
pub const RX_BUFFER_MIN_MS: usize = 100;

/// Default `--rx-buffer-ms` value. 2000 ms = 10× the default
/// `--prebuffer-ms=200`. Holds 100 packets at default 48 kHz stereo /
/// chunk_ms=20 — generous enough to absorb a typical home-Wi-Fi
/// burst of dropped-order packets without overrunning; raises by
/// `raise --rx-buffer-ms` for flaky links.
pub const RX_BUFFER_DEFAULT_MS: usize = 2_000;

/// Maximum sensible `--rx-buffer-ms` value. 30 s is large enough
/// for a generous reorder/recovery window without blowing OS
/// memory on a 48 kHz stereo ring (~ 28 MiB at f32 / 30 s × 8 ch).
pub const RX_BUFFER_MAX_MS: usize = 30_000;

/// Strict-mode validation: the operator's `--rx-buffer-ms` value
/// could not be accepted — either sub-floor or super-jumbo.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BufferError {
    /// `--rx-buffer-ms` is below [`RX_BUFFER_MIN_MS`] (100 ms).
    /// Below this, even default audio formats can't fill enough
    /// packets to meet the `max(32, …)` floor.
    #[error(
        "rx-buffer-ms {got} is below RX_BUFFER_MIN ({min}); the ring can \
         never reach the max(32, packets) floor at this size"
    )]
    TooSmall { got: usize, min: usize },

    /// `--rx-buffer-ms` is above [`RX_BUFFER_MAX_MS`] (30 s).
    /// Above this, OS memory budget risk dominates the latency
    /// benefit (at 48 kHz / 8 ch a 30 s ring is ~ 28 MiB).
    #[error("rx-buffer-ms {got} exceeds RX_BUFFER_MAX ({max}); too memory-heavy")]
    TooLarge { got: usize, max: usize },

    /// `--rx-buffer-ms` is below `--prebuffer-ms`. The gate target
    /// can never be bigger than the ring itself, or the receiver
    /// would block indefinitely waiting for samples the ring
    /// cannot hold.
    #[error(
        "rx-buffer-ms {rx_buf} must be >= --prebuffer-ms {prebuf}; the \\\n         prebuffer gate target must fit inside the ring it is gating"
    )]
    BufferSmallerThanPrebuffer { rx_buf: usize, prebuf: usize },
}

/// Range-only validator for the `--rx-buffer-ms` flag. Doesn't
/// know about `--prebuffer-ms`; use [`compute_capacity_packets`]
/// when both knobs need cross-validation.
pub const fn validate_buffer_ms(n: usize) -> Result<usize, BufferError> {
    if n < RX_BUFFER_MIN_MS {
        return Err(BufferError::TooSmall {
            got: n,
            min: RX_BUFFER_MIN_MS,
        });
    }
    if n > RX_BUFFER_MAX_MS {
        return Err(BufferError::TooLarge {
            got: n,
            max: RX_BUFFER_MAX_MS,
        });
    }
    Ok(n)
}

/// Compute the receiver-side `JitterBuffer` capacity (in packets)
/// for the given operator knobs. Performs the validation chain:
///
/// 1. `rx_buffer_ms` must be in [`RX_BUFFER_MIN_MS`,
///    `RX_BUFFER_MAX_MS`] — [`BufferError::TooSmall`] /
///    [`BufferError::TooLarge`] otherwise.
/// 2. `rx_buffer_ms` must be `>= prebuffer_ms` —
///    [`BufferError::BufferSmallerThanPrebuffer`] otherwise.
/// 3. The derived `rx_buffer_packets_ceil` must be `> 0` — at
///    default audio formats with stock chunk-ms the answer is
///    `>= 5` packets, so the zero path is unreachable through
///    normal validated inputs.
pub fn compute_capacity_packets(
    rx_buffer_ms: usize,
    prebuffer_ms: usize,
    sample_rate_hz: u32,
    channels: u8,
    samples_per_packet: usize,
) -> Result<usize, BufferError> {
    let rx_buf = validate_buffer_ms(rx_buffer_ms)?;
    if rx_buf < prebuffer_ms {
        return Err(BufferError::BufferSmallerThanPrebuffer {
            rx_buf,
            prebuf: prebuffer_ms,
        });
    }
    // Convert ms → samples (interleaved f32). `sample_rate ×
    // channels / 1000` is the audio-frame rate at input
    // resolution. `samples_per_packet` is typically
    // `chunk_ms * sample_rate * channels / 1000` at 48 kHz
    // stereo / chunk-ms=20 → 1920.
    let rx_buffer_frames = rx_buf
        .checked_mul(sample_rate_hz as usize)
        .and_then(|x| x.checked_mul(channels as usize))
        .map(|x| x / 1000)
        .unwrap_or(0);
    let rx_buffer_packets = rx_buffer_frames.div_ceil(samples_per_packet);
    // OS-memory-hygiene floor: never below 32 ring slots so even
    // a very short `--rx-buffer-ms` keeps the reorder window
    // usable on the cpal side.
    let capacity = std::cmp::max(32usize, rx_buffer_packets);
    Ok(capacity)
}

#[cfg(test)]
mod unit {
    use super::*;

    // ----- Constants -----

    #[test]
    fn constants_pin_default_and_bounds() {
        assert_eq!(RX_BUFFER_MIN_MS, 100);
        assert_eq!(RX_BUFFER_DEFAULT_MS, 2_000);
        assert_eq!(RX_BUFFER_MAX_MS, 30_000);
    }

    // ----- validate_buffer_ms (range only) -----

    #[test]
    fn validate_buffer_ms_accepts_range_inclusive() {
        assert_eq!(validate_buffer_ms(100).unwrap(), 100);
        assert_eq!(validate_buffer_ms(2_000).unwrap(), 2_000);
        assert_eq!(validate_buffer_ms(30_000).unwrap(), 30_000);
    }

    #[test]
    fn validate_buffer_ms_rejects_below_min() {
        let err = validate_buffer_ms(99).unwrap_err();
        assert_eq!(err, BufferError::TooSmall { got: 99, min: 100 });
    }

    #[test]
    fn validate_buffer_ms_rejects_above_max() {
        let err = validate_buffer_ms(30_001).unwrap_err();
        assert_eq!(
            err,
            BufferError::TooLarge {
                got: 30_001,
                max: 30_000
            }
        );
    }

    // ----- compute_capacity_packets: per-knob boundary arithmetic -----
    //
    // Default 48 kHz stereo / chunk_ms=20 — samples_per_packet =
    // 48_000 × 20 × 2 / 1000 = 1920. We also test 44.1 kHz mono and
    // 96 kHz stereo at boundaries so the math (`* / / div_ceil`) is
    // pinned at realistic configurations.

    #[test]
    fn capacity_default_48k_stereo_chunk_20ms_rx_buf_2000() {
        // rx_buffer_frames = 2000 × 48000 × 2 / 1000 = 192_000
        // rx_buffer_packets = ceil(192_000 / 1920) = 100
        // capacity = max(32, 100) = 100
        let c = compute_capacity_packets(2_000, 200, 48_000, 2, 1_920).unwrap();
        assert_eq!(c, 100);
    }

    #[test]
    fn capacity_floor_applies_when_rx_buffer_ms_at_min() {
        // rx_buffer_frames = 100 × 48000 × 2 / 1000 = 9_600
        // rx_buffer_packets = ceil(9_600 / 1920) = 5
        // capacity = max(32, 5) = 32  ← memory floor wins
        let c = compute_capacity_packets(100, 100, 48_000, 2, 1_920).unwrap();
        assert_eq!(c, 32, "OS-memory floor must win at the lower edge");
    }

    #[test]
    fn capacity_at_max_rx_buffer_ms_30s() {
        // rx_buffer_frames = 30_000 × 48000 × 2 / 1000 = 2_880_000
        // rx_buffer_packets = ceil(2_880_000 / 1920) = 1500
        // capacity = max(32, 1500) = 1500
        let c = compute_capacity_packets(30_000, 200, 48_000, 2, 1_920).unwrap();
        assert_eq!(c, 1500);
    }

    #[test]
    fn capacity_44_1k_mono_at_default_chunk() {
        // chunk_ms=20 mono ⇒ samples_per_packet = 44_100 × 20 × 1 / 1000 = 882.
        // rx_buffer_frames = 2_000 × 44_100 × 1 / 1000 = 88_200
        // rx_buffer_packets = ceil(88_200 / 882) = 100
        let c = compute_capacity_packets(2_000, 200, 44_100, 1, 882).unwrap();
        assert_eq!(c, 100);
    }

    #[test]
    fn capacity_96k_stereo_at_default_chunk() {
        // samples_per_packet = 96_000 × 20 × 2 / 1000 = 3840.
        // rx_buffer_frames = 2_000 × 96_000 × 2 / 1000 = 384_000
        // rx_buffer_packets = ceil(384_000 / 3840) = 100
        let c = compute_capacity_packets(2_000, 200, 96_000, 2, 3_840).unwrap();
        assert_eq!(c, 100);
    }

    // ----- Cross-flag validation -----

    #[test]
    fn capacity_rejects_when_rx_buffer_smaller_than_prebuffer() {
        // prebuffer_ms=500 > rx_buffer_ms=200 → gate target is unreachable.
        let err = compute_capacity_packets(200, 500, 48_000, 2, 1_920).unwrap_err();
        assert_eq!(
            err,
            BufferError::BufferSmallerThanPrebuffer {
                rx_buf: 200,
                prebuf: 500
            }
        );
    }

    #[test]
    fn capacity_accepts_at_equality() {
        // rx_buffer_ms == prebuffer_ms — invariant is satisfied at the
        // boundary; this is the smallest ring that still meets the gate.
        let c = compute_capacity_packets(200, 200, 48_000, 2, 1_920).unwrap();
        // rx_buffer_frames = 200 × 48000 × 2 / 1000 = 19_200 = 10 packets
        // capacity = max(32, 10) = 32
        assert_eq!(c, 32);
    }

    #[test]
    fn capacity_propagates_too_small_from_validate() {
        // rx_buffer_ms = 50 < RX_BUFFER_MIN_MS=100. Should propagate as
        // TooSmall (NOT BufferSmallerThanPrebuffer — range check wins
        // because it's first in the chain).
        let err = compute_capacity_packets(50, 200, 48_000, 2, 1_920).unwrap_err();
        assert_eq!(err, BufferError::TooSmall { got: 50, min: 100 });
    }

    // ----- BufferError Display -----

    #[test]
    fn buffer_error_messages_mention_key_numbers() {
        let e1 = BufferError::TooSmall { got: 50, min: 100 };
        assert!(format!("{e1}").contains("50"));
        assert!(format!("{e1}").contains("100"));

        let e2 = BufferError::TooLarge {
            got: 60_000,
            max: 30_000,
        };
        let s2 = format!("{e2}");
        assert!(s2.contains("60000") || s2.contains("60_000"));

        let e3 = BufferError::BufferSmallerThanPrebuffer {
            rx_buf: 100,
            prebuf: 500,
        };
        let s3 = format!("{e3}");
        assert!(s3.contains("100"));
        assert!(s3.contains("500"));
    }
}
