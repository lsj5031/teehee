//! `mtu_budget` — slice 9 (Tier 3 #8) MTU strategy helpers for teehee.
//!
//! Pure math module: converts the user-supplied `--mtu` link-MTU
//! value into a per-sender budget for the on-wire packet-size
//! envelope, then derives the maximum interleaved-f32 sample count
//! that fits inside that envelope for a given channel count.
//!
//! ## Layer model
//!
//! The user supplies a **link MTU** (the OS-level maximum payload the
//! network interface can hand to the wire in a single frame). On a
//! typical Ethernet LAN that's 1500 B; on jumbo-frame networks
//! 9000 B; the IPv6 RFC minimum is 1280 B (with a deployment-wide
//! hard floor of 576 B for older / non-v6-aware paths). Every
//! teehee packet has the following layered envelope:
//!
//! ```text
//! link MTU (e.g. 1500)
//!   = IP header (20 B, no IPv4 options)
//!   + UDP header (8 B)
//!   + teehee protocol header ([`protocol::HEADER_LEN`] = 25 B)
//!   + teehee payload (interleaved f32 samples; 4 B/sample, channels
//!     samples/frame)
//! ```
//!
//! so for a link MTU of `N` bytes, the maximum teehee payload is
//! `N - framing_overhead`. The framing overhead is fixed at
//! `20 + 8 + 25 = 53` bytes across all targets — IPv6 adds
//! additional fixed fields but the IP-header budget does not change
//! in the v1 path (we send IPv4 over LAN only).
//!
//! ## Fragment-on-overrun semantics
//!
//! Slice 9 ships `--mtu` as an adaptive knob: the sender
//! (a) prints the configured MTU + budget at startup so the operator
//! sees the relationship between `chunk_ms` × audio params and the
//! wire envelope, and (b) tracks `fragmentations` — the count of
//! times an encoded packet exceeded the configured MTU. The
//! sender does NOT clamp `chunk_ms` (chunk_ms stays caller-driven);
//! when a packet overshoots, the OS IP layer handles
//! fragmentation transparently, and the count increments so
//! operators can spot the misconfiguration via `--stats`. This is
//! the "fragment-on-overrun behavior" the slice requested.
//!
//! [`protocol::HEADER_LEN`]: crate::protocol::HEADER_LEN

use thiserror::Error;

/// Lower-layer framing overhead: IP (20 B) + UDP (8 B) + teehee
/// protocol header ([`crate::protocol::HEADER_LEN`] = 25 B).
///
/// Constant across all targets we ship to. v1 IPv4 only; IPv6 would
/// push IP-header from 20 to 40 B (16 B more), but teehee v1 does
/// not use IPv6 on the wire.
pub const FRAMING_OVERHEAD_BYTES: usize = 20 + 8 + crate::protocol::HEADER_LEN;

/// Minimum sensible `--mtu` value. 576 is the IPv6 RFC-minimum
/// link-MTU (RFC 2460 / RFC 8200 path-MTU floor); below this, IPv6
/// nodes cannot send any unfragmented packet.
pub const MTU_MIN_BYTES: usize = 576;

/// Default `--mtu` value. 1500 matches typical Ethernet LAN MTUs
/// (per RFC 894 / RFC 791); the same number is also the
/// conservative ceiling for `teehee mtu_smoke` and `teehee
/// mtu_boundary_sweep` regression tests.
pub const MTU_DEFAULT_BYTES: usize = 1500;

/// Maximum sensible `--mtu` value. 9000 matches jumbo-frame Ethernet
/// (per IEEE 802.3 + various vendor extensions). Above this, very
/// few network devices forward the packet, so clamping the knob
/// avoids surprising failures.
pub const MTU_MAX_BYTES: usize = 9000;

/// Bytes per interleaved `f32` sample in the v1 wire format.
/// Matches [`crate::protocol::SampleFormat::F32`]'s
/// `sample_size_bytes()` return value.
pub const F32_SAMPLE_BYTES: usize = 4;

/// Strict mode validation: the `--mtu` value the user supplied
/// could not fit any payload — either sub-framing or super-jumbo.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MtuError {
    /// The supplied MTU is below [`MTU_MIN_BYTES`] (576). Surface
    /// this so users running over narrow links (cellular, some
    /// VPNs, IPv6-only paths) get a clear actionable error rather
    /// than silent payload rejection.
    #[error(
        "mtu {got} is below MTU_MIN ({min}); teehee needs at least \
         (mtu - framing_overhead) > 0 bytes for any payload"
    )]
    TooSmall { got: usize, min: usize },
    /// The supplied MTU is above [`MTU_MAX_BYTES`] (9000); no real
    /// hardware forwards jumbo+ packets.
    #[error("mtu {got} exceeds MTU_MAX ({max}); jumbo+ unsupported")]
    TooLarge { got: usize, max: usize },
    /// The supplied MTU minus the framing overhead leaves no room
    /// for even a single frame at the chosen channel count (e.g.
    /// MTU 53 with channels=2 needs 8 B/frame, but max_payload =
    /// 0). Surfaces the impossible-to-decode case at startup.
    #[error(
        "mtu {mtu} leaves no room for an f32 frame at channels={channels}; \
         (mtu - framing_overhead = {max_payload}) / (channels \\u00b7 4) = \
         {max_chunk_samples} samples"
    )]
    NoFrameFits {
        mtu: usize,
        channels: u8,
        max_payload: usize,
        max_chunk_samples: usize,
    },
}

/// Per-MTU operating envelope for the sender.
///
/// Computed once at sender startup from `(mtu_bytes, channels)`.
/// Both fields are simple derived quantities — no caching or
/// mutability — so the structure is `Copy` and can be passed by
/// value without allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MtuBudget {
    /// Maximum number of bytes the teehee payload can occupy
    /// without overshooting the link MTU after IP + UDP + teehee
    /// headers are added. Equals `mtu - 53`.
    pub max_payload_bytes: usize,
    /// Maximum number of interleaved `f32` samples per packet at
    /// the given channel count. Equals
    /// `max_payload_bytes / (channels \\u00d7 4)`, integer floor.
    pub max_chunk_samples: usize,
}

/// Parse + validate an `--mtu` value from the CLI. Returns the
/// validated value, or an error.
///
/// Range: [`MTU_MIN_BYTES`, `MTU_MAX_BYTES`] inclusive = [576, 9000].
/// `parse_mtu` does NOT validate `channels` because that's the
/// `CapturerConfig.channels` (device-actual, not CLI) — the
/// `channels` runtime check happens in [`compute_budget`] when
/// called from `run_send`.
pub const fn validate_mtu(n: usize) -> Result<usize, MtuError> {
    if n < MTU_MIN_BYTES {
        return Err(MtuError::TooSmall {
            got: n,
            min: MTU_MIN_BYTES,
        });
    }
    if n > MTU_MAX_BYTES {
        return Err(MtuError::TooLarge {
            got: n,
            max: MTU_MAX_BYTES,
        });
    }
    Ok(n)
}

/// Compute the per-sender MTU budget for a given link MTU + channel
/// count.
///
/// Performs the validation chain:
/// 1. `mtu` must be in `[MTU_MIN_BYTES, MTU_MAX_BYTES]` —
///    [`MtuError::TooSmall`] / [`MtuError::TooLarge`] otherwise.
/// 2. `channels` must be `>= 1` — silently returning a zero-byte
///    budget would be a clearer error to surface.
/// 3. The derived `max_chunk_samples` must be `>= 1` — at MTU near
///    floor and high channel counts, a single f32 frame can be
///    larger than the link MTU. This triggers
///    [`MtuError::NoFrameFits`].
pub fn compute_budget(mtu_bytes: usize, channels: u8) -> Result<MtuBudget, MtuError> {
    let mtu = validate_mtu(mtu_bytes)?;
    if channels == 0 {
        // Treat as NoFrameFits with channels=0 so the diagnostic
        // is consistent with the impossible state (you can't have
        // a 0-channel user).
        return Err(MtuError::NoFrameFits {
            mtu,
            channels: 0,
            max_payload: mtu.saturating_sub(FRAMING_OVERHEAD_BYTES),
            max_chunk_samples: 0,
        });
    }
    let max_payload = mtu
        .checked_sub(FRAMING_OVERHEAD_BYTES)
        .ok_or(MtuError::TooSmall {
            got: mtu,
            min: FRAMING_OVERHEAD_BYTES,
        })?;
    let max_chunk_samples = max_payload / (channels as usize * F32_SAMPLE_BYTES);
    if max_chunk_samples == 0 {
        return Err(MtuError::NoFrameFits {
            mtu,
            channels,
            max_payload,
            max_chunk_samples,
        });
    }
    Ok(MtuBudget {
        max_payload_bytes: max_payload,
        max_chunk_samples,
    })
}

/// Decide whether an encoded packet of `encoded_bytes` size overshoots
/// the budget's `max_payload_bytes`. Used by the send loop to count
/// fragmentation events without re-running the full compute path.
///
/// Returns `true` whenever the encoded packet is one byte too large —
/// at which point the OS will IP-fragment at the wire.
#[inline]
pub const fn exceeds_budget(encoded_bytes: usize, budget: &MtuBudget) -> bool {
    encoded_bytes > budget.max_payload_bytes
}

#[cfg(test)]
mod unit {
    use super::*;

    // ----- Constants -----

    #[test]
    fn framing_overhead_matches_ip_udp_teehee_sum() {
        // Pin the FRAMING_OVERHEAD constant at 20 (IP) + 8 (UDP) + 25
        // (teehee) = 53. If a future contributor changes HEADER_LEN
        // or wraps IPv6, this assertion surfaces the drift immediately.
        assert_eq!(FRAMING_OVERHEAD_BYTES, 53);
        assert_eq!(MTU_MIN_BYTES, 576);
        assert_eq!(MTU_DEFAULT_BYTES, 1500);
        assert_eq!(MTU_MAX_BYTES, 9000);
        assert_eq!(F32_SAMPLE_BYTES, 4);
    }

    // ----- validate_mtu (range only) -----

    #[test]
    fn validate_mtu_accepts_range_inclusive() {
        assert_eq!(validate_mtu(576).unwrap(), 576);
        assert_eq!(validate_mtu(1500).unwrap(), 1500);
        assert_eq!(validate_mtu(9000).unwrap(), 9000);
    }

    #[test]
    fn validate_mtu_rejects_below_min() {
        let err = validate_mtu(575).unwrap_err();
        assert_eq!(err, MtuError::TooSmall { got: 575, min: 576 });
    }

    #[test]
    fn validate_mtu_rejects_above_max() {
        let err = validate_mtu(9001).unwrap_err();
        assert_eq!(
            err,
            MtuError::TooLarge {
                got: 9001,
                max: 9000
            }
        );
    }

    // ----- compute_budget: per-MTU boundary arithmetic -----
    //
    // The four boundary MTU values the user explicitly named
    // (576, 1280, 1500, 9000) tested at stereo (channels=2) and
    // mono (channels=1) audio pins the math so a future change to
    // the constant table, header layout, or sample-size table
    // surfaces immediately with a precise diagnostic.

    // ----- Stereo (channels = 2) -----

    #[test]
    fn stereo_mtu_576() {
        // max_payload = 576 - 53 = 523
        // max_chunk_samples (f32 stereo, 8 B/frame) = floor(523/8) = 65
        let b = compute_budget(576, 2).unwrap();
        assert_eq!(b.max_payload_bytes, 523);
        assert_eq!(b.max_chunk_samples, 65);
    }

    #[test]
    fn stereo_mtu_1280() {
        // max_payload = 1280 - 53 = 1227
        // max_chunk_samples (f32 stereo, 8 B/frame) = floor(1227/8) = 153
        let b = compute_budget(1280, 2).unwrap();
        assert_eq!(b.max_payload_bytes, 1227);
        assert_eq!(b.max_chunk_samples, 153);
    }

    #[test]
    fn stereo_mtu_1500() {
        // max_payload = 1500 - 53 = 1447
        // max_chunk_samples (f32 stereo, 8 B/frame) = floor(1447/8) = 180
        let b = compute_budget(1500, 2).unwrap();
        assert_eq!(b.max_payload_bytes, 1447);
        assert_eq!(b.max_chunk_samples, 180);
    }

    #[test]
    fn stereo_mtu_9000() {
        // max_payload = 9000 - 53 = 8947
        // max_chunk_samples (f32 stereo, 8 B/frame) = floor(8947/8) = 1118
        let b = compute_budget(9000, 2).unwrap();
        assert_eq!(b.max_payload_bytes, 8947);
        assert_eq!(b.max_chunk_samples, 1118);
    }

    // ----- Mono (channels = 1) -----

    #[test]
    fn mono_mtu_576() {
        // max_payload = 523; max_chunk_samples (mono, 4 B/frame) = 130
        let b = compute_budget(576, 1).unwrap();
        assert_eq!(b.max_payload_bytes, 523);
        assert_eq!(b.max_chunk_samples, 130);
    }

    #[test]
    fn mono_mtu_1280() {
        // max_payload = 1227; max_chunk_samples = 306
        let b = compute_budget(1280, 1).unwrap();
        assert_eq!(b.max_payload_bytes, 1227);
        assert_eq!(b.max_chunk_samples, 306);
    }

    #[test]
    fn mono_mtu_1500() {
        // max_payload = 1447; max_chunk_samples = 361
        let b = compute_budget(1500, 1).unwrap();
        assert_eq!(b.max_payload_bytes, 1447);
        assert_eq!(b.max_chunk_samples, 361);
    }

    #[test]
    fn mono_mtu_9000() {
        // max_payload = 8947; max_chunk_samples = 2236
        let b = compute_budget(9000, 1).unwrap();
        assert_eq!(b.max_payload_bytes, 8947);
        assert_eq!(b.max_chunk_samples, 2236);
    }

    // ----- Edge cases -----

    #[test]
    fn compute_budget_rejects_zero_channels() {
        // channels=0 is malformed CLI input (SendArgs::validate
        // already rejects it, but we don't want this layer to
        // silently produce a zero-byte budget that crashes the
        // sender). Surface as NoFrameFits with channels=0.
        let err = compute_budget(1500, 0).unwrap_err();
        assert_eq!(
            err,
            MtuError::NoFrameFits {
                mtu: 1500,
                channels: 0,
                max_payload: 1500 - 53,
                max_chunk_samples: 0,
            }
        );
    }

    #[test]
    fn compute_budget_rejects_when_no_frame_fits() {
        // 8 B/frame × 1 sample/frame × at least 1 frame = 8 B
        // payload. To fail, max_payload must be < 8 B, i.e.
        // mtu - 53 < 8 → mtu < 61. But MTU_MIN is 576, so this
        // error path is unreachable through validate_mtu alone.
        // Verify the high-channels path triggers it instead:
        // channels=8 at MTU_MIN=576 → max_payload=523 → max_chunk
        // = floor(523/(8*4)) = floor(523/32) = 16. Still positive.
        // We artificially construct the impossible case by passing
        // a channels value the high-byte math reaches: with
        // channels=255 and mtu=576, max_payload=523, max_chunk
        // = floor(523/(255*4)) = floor(523/1020) = 0. ✓
        let err = compute_budget(576, 255).unwrap_err();
        assert_eq!(
            err,
            MtuError::NoFrameFits {
                mtu: 576,
                channels: 255,
                max_payload: 523,
                max_chunk_samples: 0,
            }
        );
    }

    #[test]
    fn compute_budget_octaphonic_at_typical_ethernet() {
        // Realistic 8-channel config (e.g. for room-ambience capture or
        // 7.1 surround downmix paths) at default Ethernet MTU. Sanity
        // check that high channel counts are still within the math
        // envelope. `compute_budget` is parameterized by `mtu` and
        // `channels` only — sample_rate doesn't enter (chunk_ms
        // validation lives in cli.rs).
        // channels=8 at MTU=1500:
        // max_payload = 1447; max_chunk_samples = floor(1447/32) = 45.
        let b = compute_budget(1500, 8).unwrap();
        assert_eq!(b.max_payload_bytes, 1447);
        assert_eq!(b.max_chunk_samples, 45);
    }

    // ----- exceeds_budget decision -----

    #[test]
    fn exceeds_budget_true_when_encoded_overshoots() {
        let b = MtuBudget {
            max_payload_bytes: 1447,
            max_chunk_samples: 180,
        };
        assert!(exceeds_budget(1448, &b));
        assert!(exceeds_budget(1500, &b));
        assert!(exceeds_budget(7705, &b));
    }

    #[test]
    fn exceeds_budget_false_at_and_below_max() {
        let b = MtuBudget {
            max_payload_bytes: 1447,
            max_chunk_samples: 180,
        };
        assert!(!exceeds_budget(0, &b));
        assert!(!exceeds_budget(25, &b)); // header-only
        assert!(!exceeds_budget(1447, &b)); // exactly fits
    }

    // ----- MtuError Display -----

    #[test]
    fn mtu_error_messages_mention_key_numbers() {
        let e1 = MtuError::TooSmall { got: 100, min: 576 };
        let s1 = format!("{e1}");
        assert!(s1.contains("100"));
        assert!(s1.contains("576"));
        let e2 = MtuError::TooLarge {
            got: 99999,
            max: 9000,
        };
        let s2 = format!("{e2}");
        assert!(s2.contains("99999"));
        assert!(s2.contains("9000"));
        let e3 = MtuError::NoFrameFits {
            mtu: 576,
            channels: 255,
            max_payload: 523,
            max_chunk_samples: 0,
        };
        let s3 = format!("{e3}");
        assert!(s3.contains("576"));
        assert!(s3.contains("255"));
    }
}
