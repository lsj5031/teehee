//! `tests/localhost_smoke.rs` — end-to-end integration test for the
//! teehee pipeline using real UDP on the loopback interface. The
//! sender thread generates deterministic sine audio, encodes / sends
//! via `protocol` and `network`. The receiver thread decodes via
//! `protocol` and pushes into a shared `JitterBuffer`. The test
//! compares the popped samples against a fresh `SineSource` to prove
//! that no packets were dropped, reordered incorrectly, or misaligned
//! in the receive path. No mocking.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use teehee::generated::SineSource;
use teehee::jitter::{JitterBuffer, PushOutcome};
use teehee::network::{Receiver, Sender};
use teehee::protocol::{DecodeStats, Packet};

const SAMPLE_RATE_HZ: u32 = 48_000;
const CHANNELS: u8 = 2;
const CHUNK_MS: usize = 20;
// PACKET_COUNT must be <= capacity_packets below. With capacity = 20
// slots and head advancing in seq order, pushing more than 20 packets
// before the first pop would silently overwrite the older slots and
// the test would (correctly) observe dropped audio. 18 leaves a
// small headroom inside the default 200ms / 20ms / *2 capacity.
const PACKET_COUNT: u32 = 18;
// Slice 6: PREBUFFER_MS replaces the old BUFFER_MS slot-count flag
// and now controls how long the receiver waits in silence before
// playback starts (in milliseconds of buffered audio).
const PREBUFFER_MS: usize = 200;

fn expected_sample_buffer(total: usize) -> Vec<f32> {
    let mut source = SineSource::new(SAMPLE_RATE_HZ, CHANNELS, 440.0);
    let mut out = vec![0.0_f32; total];
    source.fill_chunk(&mut out);
    out
}

#[test]
fn localhost_smoke_pipeline_returns_expected_audio() {
    let chunk_samples_ = common::chunk_samples(SAMPLE_RATE_HZ, CHUNK_MS, CHANNELS);
    let chunk_frames_ = common::chunk_frames(SAMPLE_RATE_HZ, CHUNK_MS);

    // Bind receiver, connect sender. Both move into worker threads
    // immediately, so no local mutation is needed.
    let rx = Receiver::bind(common::loopback_any_port()).expect("bind receiver");
    let rx_addr = rx.local_addr().expect("receiver local_addr");
    let tx = Sender::connect(rx_addr).expect("connect sender");

    // Pre-compute expected audio (same SineSource seed: starts at frame 0).
    let total_samples = chunk_samples_ * PACKET_COUNT as usize;
    let expected = expected_sample_buffer(total_samples);

    // Slice 6: derive the JitterBuffer's prebuffer target and ring
    // capacity from PREBUFFER_MS × the sender's actual format. The
    // gate target is `preprebuffer_ms * sample_rate * channels / 1000`
    // (19_200 frames at 48 kHz stereo / 200 ms). Capacity =
    // `max(32, required_packets * 3)` — required_packets is
    // `div_ceil(target_frames / samples_per_packet)` so the ring
    // absorbs both the prebuffer load and subsequent network
    // reorders without overwriting the active read tail.
    let prebuffer_target_frames =
        (PREBUFFER_MS * SAMPLE_RATE_HZ as usize * CHANNELS as usize) / 1000;
    let samples_per_packet = chunk_samples_;
    let required_packets = prebuffer_target_frames.div_ceil(samples_per_packet);
    let capacity_packets = std::cmp::max(32usize, required_packets * 3);
    let jb = Arc::new(Mutex::new(JitterBuffer::new(
        chunk_samples_,
        capacity_packets,
        Some(prebuffer_target_frames),
    )));

    // Receiver thread: loop with recv_timeout until PACKET_COUNT are decoded.
    // Has a hard deadline so a stalled sender fails the test instead of hanging.
    let jb_for_rx = Arc::clone(&jb);
    let ds_for_rx = Arc::new(Mutex::new(DecodeStats::default()));
    let ds_for_rx_thread = Arc::clone(&ds_for_rx);
    let rx_handle = std::thread::spawn(move || {
        let mut pkt_buf = vec![0u8; 16 * 1024];
        let mut sample_buf: Vec<f32> = Vec::with_capacity(chunk_samples_);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut received: u32 = 0;
        while received < PACKET_COUNT {
            if std::time::Instant::now() > deadline {
                panic!(
                    "receiver did not receive {PACKET_COUNT} packets within 10s deadline (got {received})"
                );
            }
            match rx.recv_timeout(&mut pkt_buf, Duration::from_millis(100)) {
                Ok(Some(n)) => {
                    let pkt = match Packet::decode(&pkt_buf[..n]) {
                        Ok(p) => p,
                        Err(e) => {
                            // Mirror the production receiver path:
                            // record() bumps the running counter BEFORE
                            // the panic, which means the post-join
                            // assertion below can catch a malformed
                            // sender even if a future maintainer weakens
                            // the panic to a continue (since the count
                            // would then be > 0).
                            ds_for_rx_thread.lock().unwrap().record(&e);
                            panic!("decode must succeed; got error: {e}");
                        }
                    };
                    pkt.pcm_f32_into(&mut sample_buf);
                    // Fail-fast on any non-Stored outcome — a duplicate or
                    // late seq means the wiring has drifted.
                    match jb_for_rx.lock().unwrap().push(pkt.sequence, &sample_buf) {
                        PushOutcome::Stored => {
                            received += 1;
                        }
                        other => panic!(
                            "unexpected push outcome for seq {}: {:?}",
                            pkt.sequence, other
                        ),
                    }
                }
                Ok(None) => {
                    // Loop until deadline or success.
                }
                Err(e) => panic!("receiver recv error: {e}"),
            }
        }
    });

    // Sender thread: encode and ship PACKET_COUNT chunks with a small
    // spacing so the receiver has time to drain the OS buffer between
    // packets. 10 ms is generous on Windows CI runners.
    let snd_handle = std::thread::spawn(move || {
        let mut sine = SineSource::new(SAMPLE_RATE_HZ, CHANNELS, 440.0);
        let mut chunk_buf = vec![0.0_f32; chunk_samples_];
        for seq in 0..PACKET_COUNT {
            sine.fill_chunk(&mut chunk_buf);
            let frame_ts = (seq as u64) * chunk_frames_ as u64;
            let pkt = Packet::new(seq, frame_ts, SAMPLE_RATE_HZ, CHANNELS, &chunk_buf);
            tx.send(&pkt.encode()).expect("send must succeed");
            std::thread::sleep(Duration::from_millis(10));
        }
    });

    snd_handle.join().expect("sender thread");
    rx_handle.join().expect("receiver thread");

    // The receiver thread's local `received` counter succeeded in
    // reaching PACKET_COUNT (otherwise it would have panicked with
    // the deadline or unexpected-outcome message), so we already
    // know all expected packets were decoded — no need for a shared
    // counter Arc.
    //
    // Slice 5 wiring: assert the DecodeStats total stayed at zero
    // across the happy-path run. The receiver thread records every
    // decode error into `ds_for_rx` before panicking, so a positive
    // count here would surface a malformed sender (broken header,
    // wrong magic, etc.) even if a future maintainer weakened the
    // per-error panic to a continue inside the loop.
    {
        let ds = ds_for_rx.lock().unwrap();
        assert_eq!(
            ds.total(),
            0,
            "happy-path smoke test must produce zero decode errors; got {ds:?}"
        );
    }

    let mut jb = Arc::try_unwrap(jb)
        .ok()
        .expect("jitter Arc has single owner")
        .into_inner()
        .unwrap();
    let stats = jb.stats();
    assert_eq!(stats.late_drops, 0, "late_drops tally: {stats:?}");
    assert_eq!(stats.duplicates, 0, "duplicates tally: {stats:?}");

    // Pop all packets and compare against the fresh SineSource buffer.
    let mut popped = vec![0.0_f32; total_samples];
    let n = jb.pop_frames(&mut popped);
    assert_eq!(n, total_samples, "pop should drain the full buffer");

    let eps = 1e-5_f32;
    let mismatches: Vec<usize> = (0..n)
        .filter(|&i| (popped[i] - expected[i]).abs() > eps)
        .take(8)
        .collect();
    assert!(
        mismatches.is_empty(),
        "got {n} samples; first {mismatches:?} deviate from fresh SineSource beyond {eps}"
    );
}
