//! Integration tests for the `jitter` module — exercise the public
//! `JitterBuffer` interface (push / pop_frames / stats).

use teehee::jitter::{JitterBuffer, PushOutcome};

const SAMPLES_PER_PACKET: usize = 4;
const CAPACITY_PACKETS: usize = 8;

fn buffer() -> JitterBuffer {
    // Slice 6: pass `None` to disable the prebuffer gate; these
    // tests focus on the legacy anchor-on-first-packet path.
    JitterBuffer::new(SAMPLES_PER_PACKET, CAPACITY_PACKETS, None)
}

fn quiet() -> [f32; SAMPLES_PER_PACKET] {
    [0.0; SAMPLES_PER_PACKET]
}

#[test]
fn push_then_pop_yields_pushed_samples() {
    let mut jb = buffer();
    assert_eq!(jb.push(0, &[1.0, 2.0, 3.0, 4.0]), PushOutcome::Stored);

    let mut out = quiet();
    let n = jb.pop_frames(&mut out);
    assert_eq!(n, SAMPLES_PER_PACKET);
    assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn pop_before_any_push_yields_zero_silence() {
    let mut jb = buffer();
    let mut out = [99.0_f32; SAMPLES_PER_PACKET];
    let n = jb.pop_frames(&mut out);
    assert_eq!(n, SAMPLES_PER_PACKET);
    assert_eq!(out, quiet());
}

#[test]
fn missing_packets_play_silence_and_advance() {
    let mut jb = buffer();
    jb.push(0, &[1.0, 2.0, 3.0, 4.0]);
    jb.push(2, &[9.0, 10.0, 11.0, 12.0]); // skip seq=1

    let mut out = [0.0_f32; SAMPLES_PER_PACKET * 3];
    let n = jb.pop_frames(&mut out);
    assert_eq!(n, SAMPLES_PER_PACKET * 3);
    assert_eq!(&out[0..4], &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(&out[4..8], &[0.0, 0.0, 0.0, 0.0]);
    assert_eq!(&out[8..12], &[9.0, 10.0, 11.0, 12.0]);

    assert_eq!(jb.stats().silence_insertions, 1);
}

#[test]
fn out_of_order_packets_are_played_in_seq_order() {
    let mut jb = buffer();
    jb.push(2, &[9.0, 10.0, 11.0, 12.0]);
    jb.push(0, &[1.0, 2.0, 3.0, 4.0]);
    jb.push(1, &[5.0, 6.0, 7.0, 8.0]);

    let mut out = [0.0_f32; SAMPLES_PER_PACKET * 3];
    let n = jb.pop_frames(&mut out);
    assert_eq!(n, SAMPLES_PER_PACKET * 3);
    assert_eq!(&out[0..4], &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(&out[4..8], &[5.0, 6.0, 7.0, 8.0]);
    assert_eq!(&out[8..12], &[9.0, 10.0, 11.0, 12.0]);
}

#[test]
fn late_packet_after_head_advances_is_dropped() {
    let mut jb = buffer();
    jb.push(0, &[1.0, 2.0, 3.0, 4.0]);

    // Advance head by 1 packet.
    let mut out = quiet();
    let n = jb.pop_frames(&mut out);
    assert_eq!(n, SAMPLES_PER_PACKET);

    // Re-pushing the now-played seq must be classified Late.
    let outcome = jb.push(0, &[99.0; SAMPLES_PER_PACKET]);
    assert_eq!(outcome, PushOutcome::Late);
    assert_eq!(jb.stats().late_drops, 1);
}

#[test]
fn duplicate_packet_before_playout_is_ignored() {
    let mut jb = buffer();
    assert_eq!(jb.push(0, &[1.0; SAMPLES_PER_PACKET]), PushOutcome::Stored);

    let outcome = jb.push(0, &[99.0; SAMPLES_PER_PACKET]);
    assert_eq!(outcome, PushOutcome::Duplicate);
    assert_eq!(jb.stats().duplicates, 1);

    // First push's data should still play back unchanged.
    let mut out = quiet();
    let n = jb.pop_frames(&mut out);
    assert_eq!(n, SAMPLES_PER_PACKET);
    assert_eq!(out, [1.0; SAMPLES_PER_PACKET]);
}

#[test]
fn sequence_wraparound_plays_in_forward_ring_order() {
    let mut jb = buffer();
    jb.push(u32::MAX - 1, &[1.0, 2.0, 3.0, 4.0]);
    jb.push(u32::MAX, &[5.0, 6.0, 7.0, 8.0]);
    jb.push(0, &[9.0, 10.0, 11.0, 12.0]);

    let mut out = [0.0_f32; SAMPLES_PER_PACKET * 3];
    let n = jb.pop_frames(&mut out);
    assert_eq!(n, SAMPLES_PER_PACKET * 3);
    assert_eq!(&out[0..4], &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(&out[4..8], &[5.0, 6.0, 7.0, 8.0]);
    assert_eq!(&out[8..12], &[9.0, 10.0, 11.0, 12.0]);
}

#[test]
fn stats_aggregate_across_multiple_events() {
    let mut jb = buffer();
    jb.push(0, &[1.0; SAMPLES_PER_PACKET]);
    jb.push(3, &[99.0; SAMPLES_PER_PACKET]); // skip 1, 2

    // Pre-buffer duplicate (slot occupied, head still being anchored).
    assert_eq!(
        jb.push(3, &[0.0; SAMPLES_PER_PACKET]),
        PushOutcome::Duplicate
    );

    // Drain 0, 1(silence), 2(silence), 3.
    let mut out = [0.0_f32; SAMPLES_PER_PACKET * 4];
    jb.pop_frames(&mut out);

    // Post-pop late (head has passed seq=0).
    assert_eq!(jb.push(0, &[0.0; SAMPLES_PER_PACKET]), PushOutcome::Late);

    let s = jb.stats();
    assert_eq!(s.silence_insertions, 2);
    assert_eq!(s.duplicates, 1);
    assert_eq!(s.late_drops, 1);
}

#[test]
fn pop_filling_partial_packet_pads_the_rest_with_silence() {
    let mut jb = buffer();
    jb.push(0, &[1.0, 2.0, 3.0, 4.0]);

    // Request 6 samples — exactly 1.5 packets worth.
    let mut out = [99.0_f32; 6];
    let n = jb.pop_frames(&mut out);
    assert_eq!(n, 6);
    assert_eq!(&out[0..4], &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(&out[4..6], &[0.0, 0.0]);
}

#[test]
fn empty_request_yields_zero_bytes_written() {
    let mut jb = buffer();
    jb.push(0, &[1.0, 2.0, 3.0, 4.0]);

    let mut out = [0.0_f32; 0];
    let n = jb.pop_frames(&mut out);
    assert_eq!(n, 0);
}
