//! `tests/mtu_boundary_sweep.rs` — MTU boundary sweep.
//!
//! For `chunk_ms` from 1 to 50 inclusive, encodes **one real packet**
//! through the production path (`SineSource::fill_chunk` →
//! `Packet::new` → `Packet::encode` → measure `Vec::len()`) at the
//! project's default 48 kHz stereo f32 config, classifies the on-wire
//! size against three MTU thresholds, prints a tabular report, and
//! regression-pins the exact `chunk_ms` at which each threshold is
//! first crossed.
//!
//! Three thresholds are tracked:
//!
//!   * **raw** `< 1500` — won't fragment at the IP layer (loose
//!     interpretation that ignores lower-layer framing).
//!   * **framed** `size + 53 ≤ 1500` — won't fragment, exact
//!     (counts IP + UDP + teehee protocol header as foreground;
//!     matches the invariant asserted by `tests/mtu_smoke.rs`).
//!   * **safe-payload** `size + 53 ≤ 1400` — leaves explicit
//!     headroom for IP + UDP + teehee + a small slack band — the
//!     conservative ceiling used by many real-time protocol
//!     families (RTP, WebRTC, QUIC).
//!
//! All three currently cross at the same `chunk_ms = 4` boundary
//! because the chunk-arithmetic jump between `chunk_ms = 3` (1177
//! bytes) and `chunk_ms = 4` (1561 bytes) is 384 bytes — much
//! larger than any framing-overhead-induced threshold shift. A
//! future change to a different sample-rate or channel count could
//! shift some of the three boundaries to different `chunk_ms`
//! values, which is exactly what the regression-pinned asserts in
//! the second half of the test will surface.
//!
//! The v1 default (`chunk_ms = 20`) is reported along with the
//! table for clarity, but its ~7705-byte wire size is not asserted:
//! the PRD's "where practical" wording acknowledges fragmentation
//! at the default — surfacing the *number* of fragments (or
//! total excess bytes) is the test's value here, not enforcement.

mod common;

use teehee::protocol::HEADER_LEN;

const SAMPLE_RATE_HZ: u32 = 48_000;
const CHANNELS: u8 = 2; // default stereo — the pre-existing config that fragments at the default
const FRAMING_OVERHEAD_BYTES: usize = 20 /* IP */ + 8 /* UDP */ + HEADER_LEN;
const MAX_CHUNK_MS: usize = 50;

/// Pure-function geometry helper. For a given `chunk_ms` returns
/// `(chunk_samples, wire_size)` — the interleaved f32 sample
/// count plus the on-wire packet byte count including the
/// fix-length 25-byte header. Used both inside the sweep loop
/// (to allocate the sample buffer) and below the table (to
/// report the v1 default config). Defining both values from a
/// single source keeps arithmetic drift impossible.
const fn chunk_samples_and_wire_size(chunk_ms: usize) -> (usize, usize) {
    let chunk_frames = SAMPLE_RATE_HZ as usize * chunk_ms / 1000;
    let chunk_samples = chunk_frames * CHANNELS as usize;
    let wire_size = HEADER_LEN + chunk_samples * 4;
    (chunk_samples, wire_size)
}

#[test]
fn mtu_boundary_sweep_at_default_stereo() {
    println!(
        "\nMTU boundary sweep at {} Hz × {} ch × f32 (default sender config):",
        SAMPLE_RATE_HZ, CHANNELS
    );
    println!(
        "  FRAMING_OVERHEAD = IP(20) + UDP(8) + teehee({}) = {} bytes",
        HEADER_LEN, FRAMING_OVERHEAD_BYTES
    );
    println!();
    println!("  chunk_ms | wire_size | raw<1500 | framed≤1500 | safe-payload ≤1400");
    println!("  ---------+-----------+----------+-------------+-------------------");

    let mut first_over_raw: Option<usize> = None;
    let mut first_over_framed_1500: Option<usize> = None;
    let mut first_over_safe_1400: Option<usize> = None;

    for chunk_ms in 1..=MAX_CHUNK_MS {
        let (chunk_samples, expected_size) = chunk_samples_and_wire_size(chunk_ms);

        // Real-encode one packet through the production path so the
        // measured wire size equals what `teehee send` actually emits
        // on the network, not just a hand-computed estimate. A simple
        // sine source of 440 Hz is enough for `Packet::encode` — its
        // PCM sample values don't affect the byte count.
        let wire_size =
            common::encode_sine_packet(0, 0, SAMPLE_RATE_HZ, CHANNELS, chunk_samples, 440.0).len();

        // Sanity: on-wire size must equal the formula. Any drift
        // here means the wire-format or chunk-arithmetic has
        // drifted — surface with a precise diagnostic.
        assert_eq!(
            wire_size, expected_size,
            "encoded {wire_size} != computed {expected_size} at chunk_ms={chunk_ms} \
             (wire-format or chunk-arithmetic drift?)"
        );

        let raw_ok = wire_size < 1500;
        let framed_total = wire_size + FRAMING_OVERHEAD_BYTES;
        let framed_ok = framed_total <= 1500;
        let safe_ok = framed_total <= 1400;

        if !raw_ok && first_over_raw.is_none() {
            first_over_raw = Some(chunk_ms);
        }
        if !framed_ok && first_over_framed_1500.is_none() {
            first_over_framed_1500 = Some(chunk_ms);
        }
        if !safe_ok && first_over_safe_1400.is_none() {
            first_over_safe_1400 = Some(chunk_ms);
        }

        println!(
            "  {:>8} | {:>9} | {:>8} | {:>11} | {:>17}",
            chunk_ms,
            wire_size,
            if raw_ok { "yes" } else { "**no**" },
            if framed_ok { "yes" } else { "**no**" },
            if safe_ok { "yes" } else { "**no**" }
        );
    }

    println!();
    println!(
        "Boundary crossings (first `chunk_ms` that fails each threshold) at \
         {} Hz × {} ch × f32:",
        SAMPLE_RATE_HZ, CHANNELS
    );
    println!(
        "  raw<1500:                       first failing chunk_ms = {:?}",
        first_over_raw
    );
    println!(
        "  framed: size+{}≤1500 (exact):   first failing chunk_ms = {:?}",
        FRAMING_OVERHEAD_BYTES, first_over_framed_1500
    );
    println!(
        "  framed: size+{}≤1400 (safe):    first failing chunk_ms = {:?}",
        FRAMING_OVERHEAD_BYTES, first_over_safe_1400
    );

    // v1 default is informational, not an invariant (the PRD
    // explicitly accepts this fragmentation).
    let default_chunk_ms_size = chunk_samples_and_wire_size(20).1;
    let default_framed = default_chunk_ms_size + FRAMING_OVERHEAD_BYTES;
    println!(
        "\nv1 default chunk_ms=20 → {} B raw / {} B framed (with IP+UDP+teehee)",
        default_chunk_ms_size, default_framed
    );

    // ----- Regression-pin: bundle the boundary assertions -----
    //
    // Boundary mathematics at 48 kHz × 2 ch × f32: the
    // chunk-arithmetic jump from `chunk_ms = 3` (1177 B wire) to
    // `chunk_ms = 4` (1561 B wire) is a 384-byte step. That dwarfs:
    //
    //   * the framing overhead of 53 B (IP + UDP + teehee), and
    //   * any safe-payload ceiling variation between 1400 and 1500.
    //
    // So all three thresholds flip at the *same* `chunk_ms = 4`. A
    // future change to `SAMPLE_RATE_HZ` or `CHANNELS` could split
    // them to adjacent `chunk_ms` values — e.g. at 22050 Hz the
    // chunk-arithmetic jump shrinks to 176 B/tick (vs the 100-byte
    // gap between 1400 and 1500 thresholds) and the boundaries
    // would diverge; the asserts document this expectation and
    // surface that drift if it ever happens.
    assert_eq!(
        first_over_raw,
        Some(4),
        "raw MTU boundary should be at chunk_ms=4 \
         (chunk_ms=3 → 1177 B fits; chunk_ms=4 → 1561 B fails)"
    );
    assert_eq!(
        first_over_framed_1500,
        Some(4),
        "framed-MTU ceiling should also cross at chunk_ms=4 \
         (chunk_ms=3 → 1177+53=1230 B fits ≤1500; \
         chunk_ms=4 → 1561+53=1614 B fails)"
    );
    assert_eq!(
        first_over_safe_1400,
        Some(4),
        "safe-payload ceiling should also cross at chunk_ms=4 \
         (chunk_ms=3 → 1177+53=1230 B ≤1400; \
         chunk_ms=4 → 1561+53=1614 B >1400)"
    );
}
