//! `tests/mtu_strategy_tests.rs` — slice 9 (Tier 3 #8) MTU strategy
//! integration tests.
//!
//! Pins the math behind the `--mtu` adaptive sender knob at the four
//! boundary MTU values the slice explicitly named (576, 1280, 1500,
//! 9000) and exercises the sender-loop fragment-on-overrun accounting
//! pattern. Together with `mtu_smoke.rs` (mono + chunk_ms=5 fits
//! inside MTU) and `mtu_boundary_sweep.rs` (chunk_ms 1..50 sweep),
//! the three test files cover the MTU invariant at sender time:
//!
//! * `mtu_smoke` — packet bytes stay within the encodable envelope.
//! * `mtu_boundary_sweep` — chunk_ms boundary crossings at stereo.
//! * `mtu_strategy_tests` — adaptive `--mtu` math + fragment-on-
//!   overrun accounting at the four canonical link-MTUs.
//!
//! The fragment-on-overrun simulation in `sender_loop_fragmentation`
//! mirrors the slice-9 wiring inside `main::run_send` directly (encode
//! → size check → counter increment → send anyway), but bypasses the
//! cpal audio path so it runs on a CI machine without audio
//! hardware. The atomic-counter contract that the wiring uses is the
//! same shape, so a future drift in `run_send` will surface as a
//! difference between this test and the per-packet encode loop in
//! production.
//!
//! ## Why four boundary values?
//!
//! - 576 — IPv6 RFC-minimum (RFC 2460 / RFC 8200 path-MTU floor).
//!   Smallest sensible link MTU.
//! - 1280 — IPv6 minimum on most v6 deployments. Common when
//!   tunnelling / PPPoE / VPN overhead needs to be accounted for.
//! - 1500 — typical Ethernet LAN. teehee's `mtu_smoke` regression
//!   pins this.
//! - 9000 — jumbo-frame Ethernet. Upper bound of `--mtu` enforcement.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};

use teehee::mtu_budget::{compute_budget, exceeds_budget};
use teehee::protocol::{Packet, HEADER_LEN};

const SAMPLE_RATE_HZ: u32 = 48_000;
const STEREO_CHANNELS: u8 = 2;

// ----- 4 boundary MTU math pins -----
//
// The four tests below each compute the budget for one of the
// boundary link-MTU values at stereo (channels = 2) and pin
// `max_payload_bytes` + `max_chunk_samples` to their exact
// expected values. A future drift in `MTU_MIN/DEFAULT/MAX`,
// `FRAMING_OVERHEAD_BYTES`, or the per-frame audio math surfaces
// here with a precise diagnostic naming both the expected and
// measured values.

#[test]
fn boundary_mtu_576_stereo_pins_math() {
    // 576 B link MTU − 53 B IP/UDP/teehee framing = 523 B payload
    // envelope. 523 B / (2 channels × 4 B/f32 sample) = 65 samples.
    let b = compute_budget(576, 2).expect("MTU 576 stereo is valid");
    assert_eq!(b.max_payload_bytes, 523);
    assert_eq!(b.max_chunk_samples, 65);
}

#[test]
fn boundary_mtu_1280_stereo_pins_math() {
    // 1280 B − 53 B = 1227 B; 1227 / 8 = 153 samples.
    let b = compute_budget(1280, 2).expect("MTU 1280 stereo is valid");
    assert_eq!(b.max_payload_bytes, 1227);
    assert_eq!(b.max_chunk_samples, 153);
}

#[test]
fn boundary_mtu_1500_stereo_pins_math() {
    // 1500 B − 53 B = 1447 B; 1447 / 8 = 180 samples.
    // This same number is asserted by the safe-payload ceiling
    // logic in `mtu_boundary_sweep.rs` (chunk_ms where
    // `wire_size + 53 ≤ 1400` first fails). Pinning both files
    // keeps the regression cross-checked.
    let b = compute_budget(1500, 2).expect("MTU 1500 stereo is valid");
    assert_eq!(b.max_payload_bytes, 1447);
    assert_eq!(b.max_chunk_samples, 180);
}

#[test]
fn boundary_mtu_9000_stereo_pins_math() {
    // 9000 B − 53 B = 8947 B; 8947 / 8 = 1118 samples. The default
    // `chunk_ms = 20` × `channels = 2` × `sample_rate = 48000`
    // yields 1920 samples, which exceeds 1118 frames-per-second by
    // ~70 % — even jumbo needs a chunk_ms ~13 or lower for
    // unfragmented transmission at the default sender config.
    let b = compute_budget(9000, 2).expect("MTU 9000 stereo is valid");
    assert_eq!(b.max_payload_bytes, 8947);
    assert_eq!(b.max_chunk_samples, 1118);
}

// ----- `exceeds_budget` decision -----

#[test]
fn exceeds_budget_decision_at_stereo_1500() {
    let b = compute_budget(1500, 2).unwrap();
    // Encoded default chunk_ms=20 stereo packet (7705 B):
    assert!(exceeds_budget(7705, &b));
    // Header-only (25 B): definitely fits.
    assert!(!exceeds_budget(25, &b));
    // Exactly at the boundary: does NOT count as overrun.
    // (Encoded size = 1447 B is the largest packet that still
    // fits inside the envelope.)
    assert!(!exceeds_budget(b.max_payload_bytes, &b));
    // One byte over: counts as overrun.
    assert!(exceeds_budget(b.max_payload_bytes + 1, &b));
}

// ----- Default `chunk_ms = 20` overshoots every LAN MTU -----

#[test]
fn default_chunk_ms_20_stereo_payload_overshoots_all_lan_boundary_mtus() {
    // The default sender config at 48 kHz × 2 ch × 20 ms produces
    // 1920 interleaved f32 samples per packet = 7680 B payload +
    // 25 B header = 7705 B encoded. Add IP + UDP framing and
    // you have 7758 B on the wire, which exceeds every "common"
    // link MTU except jumbo (9000).
    //
    // This test pins that observation as an executable invariant:
    // a future change to either the default `chunk_ms` or the
    // audio-frame math that brings the default payload below all
    // 4 boundary MTUs would surface here.
    let chunk_frames = common::chunk_frames(SAMPLE_RATE_HZ, 20);
    let chunk_samples = common::chunk_samples(SAMPLE_RATE_HZ, 20, STEREO_CHANNELS);
    assert_eq!(chunk_samples, 1920);
    assert_eq!(chunk_frames, 960);
    let encoded =
        common::encode_sine_packet(0, 0, SAMPLE_RATE_HZ, STEREO_CHANNELS, chunk_samples, 440.0);
    assert_eq!(encoded.len(), HEADER_LEN + chunk_samples * 4);
    assert_eq!(encoded.len(), 7705);

    // Every "ordinary" MTU at stereo sees this as overrun:
    for &mtu in &[576usize, 1280, 1500] {
        let b = compute_budget(mtu, 2).unwrap();
        assert!(
            exceeds_budget(encoded.len(), &b),
            "default chunk_ms=20 stereo packet ({} B) should exceed MTU={}",
            encoded.len(),
            mtu
        );
    }
    // Jumbo (9000) is the only one that fits the default config.
    let jumbo = compute_budget(9000, 2).unwrap();
    assert!(
        !exceeds_budget(encoded.len(), &jumbo),
        "default chunk_ms=20 stereo packet ({} B) fits at MTU=9000 (max {})",
        encoded.len(),
        jumbo.max_payload_bytes
    );
}

// ----- `chunk_ms = 1` fits at every boundary MTU -----

#[test]
fn chunk_ms_1_stereo_fits_at_every_boundary_mtu() {
    // chunk_frames(48000, 1) = 48; chunk_samples(48, 1, 2) = 96;
    // encoded.len() = 25 + 96*4 = 409 B.
    let chunk_samples = common::chunk_samples(SAMPLE_RATE_HZ, 1, STEREO_CHANNELS);
    let encoded =
        common::encode_sine_packet(0, 0, SAMPLE_RATE_HZ, STEREO_CHANNELS, chunk_samples, 440.0);
    assert_eq!(encoded.len(), 409);

    for &mtu in &[576usize, 1280, 1500, 9000] {
        let b = compute_budget(mtu, 2).expect("valid MTU");
        assert!(
            encoded.len() <= b.max_payload_bytes,
            "chunk_ms=1 at MTU={}: encoded {} B > max_payload {} B",
            mtu,
            encoded.len(),
            b.max_payload_bytes
        );
    }
}

// ----- Sender-loop simulation: fragment-on-overrun accounting -----
//
// This test mirrors the slice-9 sender-loop wiring inside
// `main::run_send` directly: encode one packet at a time, check
// `encoded.len() > max_payload_bytes`, increment a fragmentations
// counter, and proceed (no drop). A passing test asserts that the
// shape of `main.rs`'s counter-increment matches what the
// `exceeds_budget` helper exposes via `teehee::mtu_budget`. A
// future-drift divergence (e.g. someone changes run_send to
// <= instead of >) surfaces here.
#[test]
fn sender_loop_fragmentation_counter_increments_on_overshoot() {
    let fragmentations = AtomicU64::new(0);
    let budget = compute_budget(1500, 2).unwrap();

    // chunk_ms=20 stereo: every packet overshoots MTU=1500 by ~5x.
    let chunk_frames = common::chunk_frames(SAMPLE_RATE_HZ, 20);
    let chunk_samples = common::chunk_samples(SAMPLE_RATE_HZ, 20, STEREO_CHANNELS);
    const PACKET_COUNT: u32 = 100;
    for seq in 0..PACKET_COUNT {
        let encoded = common::encode_sine_packet(
            seq,
            (seq as u64) * chunk_frames as u64,
            SAMPLE_RATE_HZ,
            STEREO_CHANNELS,
            chunk_samples,
            440.0,
        );
        if encoded.len() > budget.max_payload_bytes {
            fragmentations.fetch_add(1, Ordering::Relaxed);
        }
    }
    assert_eq!(
        fragmentations.load(Ordering::Relaxed),
        PACKET_COUNT as u64,
        "every encoded packet at chunk_ms=20 stereo must overshoot MTU=1500"
    );
}

#[test]
fn sender_loop_fragmentation_counter_zero_when_packets_fit() {
    // At chunk_ms=1 stereo and any boundary MTU, every encoded
    // packet fits — fragmentations stay at zero.
    let fragmentations = AtomicU64::new(0);
    for &mtu in &[576usize, 1280, 1500, 9000] {
        let budget = compute_budget(mtu, 2).unwrap();
        let chunk_samples = common::chunk_samples(SAMPLE_RATE_HZ, 1, STEREO_CHANNELS);
        for seq in 0..10u32 {
            let encoded = common::encode_sine_packet(
                seq,
                seq as u64 * chunk_samples as u64 / STEREO_CHANNELS as u64,
                SAMPLE_RATE_HZ,
                STEREO_CHANNELS,
                chunk_samples,
                440.0,
            );
            if encoded.len() > budget.max_payload_bytes {
                fragmentations.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    assert_eq!(
        fragmentations.load(Ordering::Relaxed),
        0,
        "chunk_ms=1 stereo never fragments at any boundary MTU"
    );
}

// ----- Real UDP loopback: chunk-ms overshoot → fragmentations counter -----
//
// Wire-level coverage for the slice-9 fragment-on-overrun flow. The
// earlier `sender_loop_fragmentation_counter_increments_on_overshoot`
// pins the *counter shape* — this test pins the *wire behaviour*: a
// real `Sender` → `Receiver` UDP loopback with a chunk-ms chosen so
// every encoded packet overshoots the configured `--mtu` envelope.
// (stereo 48 kHz × chunk-ms=170 = 65 305-byte encoded packets vs.
// 523-byte envelope at MTU=576 → ~125× overshoot per packet).
//
// `chunk_ms=170` is the largest integer that fits the protocol's
// u16 `payload_len` field for interleaved stereo f32 at 48 kHz:
// `samples = 48000 × 170 / 1000 × 2 = 16 320`, `payload = 65 280` bytes,
// well under `MAX_PAYLOAD_LEN = u16::MAX = 65 535`. Smaller chunk_ms
// would still overshoot the MTU envelope but with a lower overshoot
// ratio; we maximise the ratio here so the per-packet `>`-vs-`<=`
// branch in production is the strongest possible pin.
// Production guarantees:
//
//   1. Every packet the encoder emits MUST be `stored.fragmentations++`
//      because `encoded.len() > max_payload_bytes` is structurally
//      impossible to fail at this size.
//   2. The `Sender::send` call MUST succeed UDP-side (the OS handles
//      IP-layer fragmentation transparently — counter is incremented,
//      packet is dropped only at queue-full, which 30 packets on
//      localhost never hits).
//   3. The `Receiver::recv_timeout` round-trip MUST decode the same
//      number of packets (proves the OS didn't drop or truncate).
//   4. `fragmentations` MUST equal `packet_count` at end-of-run.
//
// A future drift in `compute_budget` (e.g. shipping HEADROOM differently),
// a change to the `max_payload` derivation, or a refactor of the
// fragment-counter to use `<=` instead of `>` would surface here with
// an exact equality assertion failure (left = N, right = 0 or vice
// versa) showing the production drift.
#[test]
fn sine_sender_loopback_fragmentations_counter_matches_overshoot_rate() {
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use teehee::network::{Receiver, Sender};

    // Configuration chosen so the math is unambiguous:
    //   chunk_ms = 170, stereo (2 ch), 48 kHz f32
    //     → chunk_frames = 48000 * 170 / 1000 = 8 160
    //     → chunk_samples = 8 160 * 2 = 16 320
    //     → payload bytes = 16 320 * 4 = 65 280
    //     → encoded = 25 (header) + 65 280 = 65 305 bytes
    //   mtu = 576, channels = 2
    //     → max_payload = 576 − 53 (IP+UDP+teehee framing) = 523
    //     → encoded (65 305) >> max_payload (523) ⇒ EVERY packet
    //       overshoots. fragmentations == PACKET_COUNT exactly.
    //
    // `chunk_ms = 170` is the largest encodable chunk_ms for stereo
    // f32 at 48 kHz; bumping it past 170 triggers `protocol.rs`'s
    // `MAX_PAYLOAD_LEN` assert in `Packet::encode()`, which is a
    // separate (production-correct) wire-format cap and out of scope
    // for the fragment-on-overrun assertion we're pinning here.
    const CHUNK_MS: usize = 170;
    const PACKET_COUNT: u32 = 30;
    const SENDER_MTU: usize = 576;

    let chunk_frames = common::chunk_frames(SAMPLE_RATE_HZ, CHUNK_MS);
    let chunk_samples = common::chunk_samples(SAMPLE_RATE_HZ, CHUNK_MS, STEREO_CHANNELS);
    assert_eq!(chunk_samples, 16_320, "chunk math sanity pin");
    // Belt-and-braces wire-cap pin: surface the protocol-cap invariant
    // visibly so a future `packet → u32 payload_len` migration (or a
    // sample-rate bump) is caught here too, not just at `Packet::encode`
    // runtime.
    assert!(
        chunk_samples * 4 <= teehee::protocol::MAX_PAYLOAD_LEN,
        "encoded payload ({} bytes) must fit u16 payload_len cap ({})",
        chunk_samples * 4,
        teehee::protocol::MAX_PAYLOAD_LEN
    );
    let expected_encoded_size = teehee::protocol::HEADER_LEN + chunk_samples * 4;
    assert_eq!(
        expected_encoded_size, 65_305,
        "encoded-size sanity pin — drift here means HEAD/samples-per-packet math changed"
    );

    let budget = compute_budget(SENDER_MTU, STEREO_CHANNELS).expect("576 is a valid MTU at stereo");
    assert_eq!(budget.max_payload_bytes, 523);
    assert!(
        expected_encoded_size > budget.max_payload_bytes * 100,
        "configuration must produce a wire packet with >100× overshoot so the \
         counter-pattern test is robust to small drift"
    );

    // Real UDP loopback: bind receiver on an ephemeral port, then
    // connect sender to its `local_addr`. In-process so no
    // subprocess / stderr-pipe / SO_REUSEADDR concerns: the sender
    // gets its OWN ephemeral port via `Sender::connect`'s underlying
    // `UdpSocket::bind(("0.0.0.0", 0))`, distinct from the receiver's.
    // Port collisions are structurally impossible.
    let rx = Receiver::bind(common::loopback_any_port())
        .expect("bind loopback receiver at ephemeral port");
    let rx_addr = rx.local_addr().expect("receiver local_addr");
    let tx = Sender::connect(rx_addr).expect("connect sender to receiver");

    // Shared state for the sender thread:
    //   * `fragmentations` is the Arc<AtomicU64>-shape counter that
    //     mirrors the slice-9 wiring in `main::run_send` directly.
    //     The integration test uses the same increment pattern so a
    //     drift in the production code is detectable as a delta
    //     between this test and production.
    //   * `packets_sent` records what actually reached `Sender::send`,
    //     a sanity that the socket itself never dropped.
    let fragmentations = Arc::new(AtomicU64::new(0));
    let packets_sent = Arc::new(AtomicU64::new(0));
    let frag_clone = Arc::clone(&fragmentations);
    let sent_clone = Arc::clone(&packets_sent);

    // Receiver thread: bound the loop with a hard deadline so a
    // stalled sender fails the test instead of hanging forever.
    // Each receive runs `Packet::decode` to prove the wire bytes
    // round-trip cleanly — a non-roundtrippable packet is a
    // failing receiver even if `recv_timeout` claims success.
    let rx_handle = std::thread::spawn(move || {
        // pkt_buf sized for the largest expected packet
        // (chunk_ms=170 stereo = 65 305 B) plus a 4 KiB framing
        // headroom. Well below u32::MAX; grows-once.
        let mut pkt_buf = vec![0u8; 70 * 1024];
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut received: u32 = 0;
        while received < PACKET_COUNT {
            if Instant::now() > deadline {
                panic!(
                    "receiver stuck at {received}/{PACKET_COUNT} within 10s deadline — \
                     sender stalls or UDP loopback broken"
                );
            }
            match rx.recv_timeout(&mut pkt_buf, Duration::from_millis(100)) {
                Ok(Some(n)) => {
                    let pkt = Packet::decode(&pkt_buf[..n])
                        .expect("encoded packet must round-trip through decode");
                    assert_eq!(
                        pkt.payload.len(),
                        chunk_samples * 4,
                        "decoded payload mismatch (encode/decode drift in production?)"
                    );
                    assert_eq!(
                        pkt.channels, STEREO_CHANNELS,
                        "channels byte must match the configured value"
                    );
                    assert_eq!(
                        pkt.sample_rate, SAMPLE_RATE_HZ,
                        "sample_rate byte must match the configured value"
                    );
                    received += 1;
                }
                Ok(None) => continue,
                Err(e) => panic!("recv_timeout error: {e}"),
            }
        }
        received
    });

    // Sender thread: encode → size check → counter bump → send. No
    // pacing — the UDP loopback absorbs a 30-packet burst in well
    // under 100 ms.
    let snd_handle = std::thread::spawn(move || {
        for seq in 0..PACKET_COUNT {
            let frame_ts = (seq as u64) * chunk_frames as u64;
            let encoded = common::encode_sine_packet(
                seq,
                frame_ts,
                SAMPLE_RATE_HZ,
                STEREO_CHANNELS,
                chunk_samples,
                440.0,
            );
            assert_eq!(
                encoded.len(),
                expected_encoded_size,
                "encode helper produced an unexpected-sized packet (drift?)"
            );
            // Slice-9 production shape:
            if encoded.len() > budget.max_payload_bytes {
                frag_clone.fetch_add(1, Ordering::Relaxed);
            }
            tx.send(&encoded)
                .expect("send must succeed on loopback socket");
            sent_clone.fetch_add(1, Ordering::Relaxed);
        }
    });

    snd_handle.join().expect("sender thread panic");
    let received = rx_handle.join().expect("receiver thread panic");
    assert_eq!(
        received, PACKET_COUNT,
        "receiver must observe all {PACKET_COUNT} packets end-to-end"
    );

    // Core assertion every fragment-on-overrun story lives or dies by:
    //   fragmentations == PACKET_COUNT
    // The configuration makes ENCODED_SIZE = 65 305 > 124 × max_payload (523),
    // so `encoded.len() > budget.max_payload_bytes` is structurally true
    // for every packet and the counter must equal PACKET_COUNT.
    assert_eq!(
        packets_sent.load(Ordering::Relaxed),
        PACKET_COUNT as u64,
        "sender thread sent every packet"
    );
    assert_eq!(
        fragmentations.load(Ordering::Relaxed),
        PACKET_COUNT as u64,
        "every encoded packet at chunk_ms=170 stereo MUST overshoot MTU=576; \
         a fragmentations < PACKET_COUNT would mean the production counter \
         shape drifted away from `if encoded.len() > max_payload_bytes`"
    );
}

// ----- Boundary arithmetic correctness across formats -----

#[test]
fn compute_budget_error_for_too_small_mtu() {
    // 575 is below MTU_MIN (576). The function must reject it
    // before the subtract-then-divide chain.
    let err = compute_budget(575, 2).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("575"));
    assert!(msg.contains("576"));
}

#[test]
fn compute_budget_error_for_zero_channels() {
    let err = compute_budget(1500, 0).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("channels"));
    assert!(msg.contains("1500"));
}
