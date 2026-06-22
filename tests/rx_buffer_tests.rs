//! Slice 10 (Tier 3 #9) receiver-side buffer-budget integration tests.
//!
//! Cross-platform (no cpal, no UDP): exercises
//! [`teehee::buffer_budget::compute_capacity_packets`] at every
//! documented boundary, the new
//! [`teehee::jitter::Stats::ring_overruns`] counter signal via
//! direct JitterBuffer::push / pop sequencing, and a regression
//! check that the slice-6 Stats fields still surface unchanged.

use teehee::buffer_budget::{
    compute_capacity_packets, validate_buffer_ms, BufferError, RX_BUFFER_DEFAULT_MS,
    RX_BUFFER_MAX_MS, RX_BUFFER_MIN_MS,
};
use teehee::jitter::{JitterBuffer, PushOutcome, Stats};

const DEFAULT_TX_HZ: u32 = 48_000;
const DEFAULT_TX_CH: u8 = 2;
const DEFAULT_SAMPLES_PER_PACKET: usize = 1_920; // 48k stereo × 20 ms chunk

// ----- Constants are stable -----

#[test]
fn default_min_and_max_constants_round_trip() {
    assert_eq!(RX_BUFFER_MIN_MS, 100);
    assert_eq!(RX_BUFFER_DEFAULT_MS, 2_000);
    assert_eq!(RX_BUFFER_MAX_MS, 30_000);
}

// ----- validate_buffer_ms (range-only) -----

#[test]
fn validate_accepts_default_2_seconds() {
    assert_eq!(validate_buffer_ms(2_000).unwrap(), 2_000);
}

#[test]
fn validate_rejects_sub_floor() {
    let err = validate_buffer_ms(99).unwrap_err();
    assert_eq!(err, BufferError::TooSmall { got: 99, min: 100 });
}

#[test]
fn validate_rejects_super_cap() {
    let err = validate_buffer_ms(30_001).unwrap_err();
    assert_eq!(
        err,
        BufferError::TooLarge {
            got: 30_001,
            max: 30_000
        }
    );
}

// ----- compute_capacity_packets math at the four boundary values
// the slice spec calls out -----

#[test]
fn capacity_at_boundary_rx_buffer_ms_100() {
    // 100 ms × 48000 × 2 = 9_600_000 / 1000 = 9_600 frames.
    // 9600 / 1920 = 5 packets. Floor (32) wins.
    let c = compute_capacity_packets(
        100,
        100,
        DEFAULT_TX_HZ,
        DEFAULT_TX_CH,
        DEFAULT_SAMPLES_PER_PACKET,
    )
    .unwrap();
    assert_eq!(
        c, 32,
        "default-audio 100 ms ring reaches the OS-memory floor"
    );
}

#[test]
fn capacity_at_boundary_rx_buffer_ms_1000() {
    // 1000 × 48000 × 2 = 96_000_000 / 1000 = 96_000 frames.
    // ceil(96_000 / 1920) = 50 packets. Floor (32) does not apply.
    let c = compute_capacity_packets(
        1_000,
        200,
        DEFAULT_TX_HZ,
        DEFAULT_TX_CH,
        DEFAULT_SAMPLES_PER_PACKET,
    )
    .unwrap();
    assert_eq!(c, 50, "default-audio 1000 ms ring = 50 packets");
}

#[test]
fn capacity_at_boundary_rx_buffer_ms_2000() {
    // Default. 2000 × 48000 × 2 / 1000 = 192_000 frames.
    // ceil(192_000 / 1920) = 100 packets.
    let c = compute_capacity_packets(
        2_000,
        200,
        DEFAULT_TX_HZ,
        DEFAULT_TX_CH,
        DEFAULT_SAMPLES_PER_PACKET,
    )
    .unwrap();
    assert_eq!(c, 100);
}

#[test]
fn capacity_at_boundary_rx_buffer_ms_5000() {
    // 5000 ms × 48000 Hz × 2 ch / 1000 = 480_000 frames.
    // ceil(480_000 / 1920) = 250 packets (2.5× the default 2000 ms
    // baseline of 100 packets). Floor (32) does not apply.
    let c = compute_capacity_packets(
        5_000,
        200,
        DEFAULT_TX_HZ,
        DEFAULT_TX_CH,
        DEFAULT_SAMPLES_PER_PACKET,
    )
    .unwrap();
    assert_eq!(c, 250, "5-second default-audio ring = 250 packets");
}

#[test]
fn capacity_at_max_rx_buffer_ms_30000() {
    // 30000 × 48000 × 2 / 1000 = 2_880_000 frames.
    // 2_880_000 / 1920 = 1500 packets.
    let c = compute_capacity_packets(
        30_000,
        200,
        DEFAULT_TX_HZ,
        DEFAULT_TX_CH,
        DEFAULT_SAMPLES_PER_PACKET,
    )
    .unwrap();
    assert_eq!(c, 1_500);
}

// ----- Cross-flag invariant -----

#[test]
fn capacity_rejects_rx_buffer_smaller_than_prebuffer() {
    let err = compute_capacity_packets(
        200,
        500,
        DEFAULT_TX_HZ,
        DEFAULT_TX_CH,
        DEFAULT_SAMPLES_PER_PACKET,
    )
    .unwrap_err();
    assert_eq!(
        err,
        BufferError::BufferSmallerThanPrebuffer {
            rx_buf: 200,
            prebuf: 500
        }
    );
}

#[test]
fn capacity_accepts_rx_buffer_at_prebuffer_equality() {
    // 200 == 200. Capacity comes from rx_buffer_frames / packets.
    let c = compute_capacity_packets(
        200,
        200,
        DEFAULT_TX_HZ,
        DEFAULT_TX_CH,
        DEFAULT_SAMPLES_PER_PACKET,
    )
    .unwrap();
    // 200 × 48000 × 2 / 1000 = 19200 frames / 1920 = 10 packets.
    // max(32, 10) = 32 (floor still applies).
    assert_eq!(c, 32);
}

#[test]
fn capacity_propagates_too_small_from_validate() {
    // rx_buffer_ms = 50 < RX_BUFFER_MIN_MS=100. validate_buffer_ms
    // rejects; compute_capacity_packets forwards.
    let err = compute_capacity_packets(
        50,
        200,
        DEFAULT_TX_HZ,
        DEFAULT_TX_CH,
        DEFAULT_SAMPLES_PER_PACKET,
    )
    .unwrap_err();
    assert!(matches!(err, BufferError::TooSmall { .. }));
}

// ----- Sustained-overload ring_overruns scenario via direct JitterBuffer ----
//
// Build a tiny 4-slot ring with 4-sample packets, anchor head, then
// push enough packets to force the ring to wrap an unplayed future
// slot. Verify ring_overruns increments to the expected count and
// the disjoint `mid_read_collisions` stays at zero.

/// Helper: 4 samples per packet, 4 ring slots, NO prebuffer gate
/// (legacy anchor-on-first-packet path).
fn small_ring() -> JitterBuffer {
    JitterBuffer::new(4, 4, None)
}

/// Push a tag-marker packet (samples = seq + 0.001) for clarity.
fn push_tag(buf: &mut JitterBuffer, seq: u32) {
    let mut samples = vec![seq as f32; 4];
    for s in samples.iter_mut() {
        *s += 0.001;
    }
    assert_eq!(
        buf.push(seq, &samples),
        PushOutcome::Stored,
        "expected Stored for fresh seq {seq}"
    );
}

#[test]
fn ring_overruns_default_is_zero() {
    // Pin the new Stats field's default so a future contributor
    // adding a field doesn't accidentally surface non-zero
    // overruns on a fresh buffer.
    let buf = small_ring();
    let s: Stats = buf.stats();
    assert_eq!(s.ring_overruns, 0);
}

#[test]
fn ring_overruns_increments_on_overwrite_of_unplayed_future_slot() {
    let mut buf = small_ring();
    // Fill 4 packets (slots 0..3 with seq 0..3).
    for s in 0..4u32 {
        push_tag(&mut buf, s);
    }
    // Drain exactly 1 packet (head = 1, head_offset = 0,
    // slot[0] cleared, slots[1..4] hold seq=1..3 — all future).
    let mut out = [0.0_f32; 4];
    buf.pop_frames(&mut out);
    assert_eq!(buf.stats().ring_overruns, 0);

    // Push seq=4 into slot[0] (cleared) — clean store.
    push_tag(&mut buf, 4);
    assert_eq!(buf.stats().ring_overruns, 0);

    // Push seq=5 into slot[1] (currently holds seq=1, unplayed
    // future) — true ring overrun.
    push_tag(&mut buf, 5);
    assert_eq!(
        buf.stats().ring_overruns,
        1,
        "overwriting unplayed future slot must increment ring_overruns"
    );
}

#[test]
fn ring_overruns_independent_of_mid_read_collisions() {
    // A sequence of overload pushes (no MidReadCollision path active)
    // increments ring_overruns exactly N times for N overwrites,
    // and never inadvertently bumps mid_read_collisions.
    //
    // Slice 10 (Tier 3 #9) introduced `ring_overruns` with an
    // at-or-forward-within-capacity window (not strict-future-only),
    // so the slot.seq == head case also counts. This test pins
    // both shapes under the new semantic.
    let mut buf = small_ring();
    for s in 0..4u32 {
        push_tag(&mut buf, s);
    }
    // Drain 2 packets → head=2, slot[0] and slot[1] cleared, slots
    // [2..=3] hold seq=2 and seq=3 (slot[2].seq == head, slot[3].seq
    // strictly forward within capacity).
    let mut out = [0.0_f32; 4];
    buf.pop_frames(&mut out);
    buf.pop_frames(&mut out);
    // Push seq=4 → idx=0, slot[0] empty → clean. ring_overruns=0.
    push_tag(&mut buf, 4);
    assert_eq!(buf.stats().ring_overruns, 0);
    // Push seq=5 → idx=1, slot[1] empty (cleared by 2nd pop) → clean.
    // ring_overruns=0 still.
    push_tag(&mut buf, 5);
    assert_eq!(
        buf.stats().ring_overruns,
        0,
        "store into cleared slot is not an overrun"
    );
    // Push seq=6 → idx=2, slot[2].seq=2 == head=2 → the
    // "slot-at-head" race window path (`fwd == 0`). ring_overruns++.
    push_tag(&mut buf, 6);
    assert_eq!(
        buf.stats().ring_overruns,
        1,
        "slot.seq == head race window counts as overrun"
    );
    // Push seq=7 → idx=3, slot[3].seq=3, head=2, fwd=1 < cap=4 →
    // strict future within capacity. ring_overruns++.
    push_tag(&mut buf, 7);
    assert_eq!(
        buf.stats().ring_overruns,
        2,
        "strict future within capacity also counts as overrun"
    );
    // mid_read_collisions remained zero throughout (no cpal mid-read
    // scenario was constructed in this test — only the Stored path).
    assert_eq!(buf.stats().mid_read_collisions, 0);
}

#[test]
fn ring_overruns_does_not_fire_on_late_packets_at_anchor() {
    // A Late push never reaches the overrun check (the Late path
    // returns before the slot is touched). The counter must stay
    // at zero in a buffer that sees only Late duplicates of an
    // already-played sequence.
    let mut buf = small_ring();
    push_tag(&mut buf, 0);
    let mut out = [0.0_f32; 4];
    buf.pop_frames(&mut out); // head=1
    for _ in 0..5 {
        let outcome = buf.push(0, &[99.0_f32; 4]);
        assert_eq!(outcome, PushOutcome::Late);
    }
    assert_eq!(buf.stats().late_drops, 5);
    assert_eq!(buf.stats().ring_overruns, 0);
}

// ----- Existing Stats fields still surface unchanged -----

#[test]
fn legacy_stats_fields_remain_after_slice10() {
    // Regression: slice 6's `prebuffer_holds`, `silence_insertions`,
    // `mid_read_collisions` etc. all still live on Stats. This test
    // pins the field names so a slice-10 renaming pass doesn't
    // silently break `--stats` consumers / grep patterns.
    let s = Stats::default();
    let _: u64 = s.late_drops;
    let _: u64 = s.duplicates;
    let _: u64 = s.silence_insertions;
    let _: u64 = s.mid_read_collisions;
    let _: u64 = s.prebuffer_holds;
    let _: u64 = s.ring_overruns;
}

// ----- Sender-vs-receiver math sanity -----
//
// Sanity check that compute_capacity_packets math matches what
// RunRecv uses after first-packet arrival: capacity_packets
// >= required_packets derived from `prebuffer_ms`.

#[test]
fn capacity_meets_or_exceeds_prebuffer_requirement_at_every_boundary() {
    // For each boundary rx_buffer_ms, derive the required packets
    // for the prebuffer_ms and confirm capacity >= required.
    for (rx_buf, pre_buf) in [
        (100usize, 100usize),
        (500, 200),
        (1_000, 200),
        (2_000, 200),
        (5_000, 200),
        (10_000, 1_000),
        (30_000, 200),
    ] {
        let capacity = compute_capacity_packets(
            rx_buf,
            pre_buf,
            DEFAULT_TX_HZ,
            DEFAULT_TX_CH,
            DEFAULT_SAMPLES_PER_PACKET,
        )
        .unwrap();
        let prebuffer_frames = (pre_buf * DEFAULT_TX_HZ as usize * DEFAULT_TX_CH as usize) / 1000;
        let required = prebuffer_frames.div_ceil(DEFAULT_SAMPLES_PER_PACKET);
        assert!(
            capacity >= required,
            "capacity={capacity} must be >= required={required} for \
             rx_buf={rx_buf} pre_buf={pre_buf} (else the gate cannot release)"
        );
    }
}
