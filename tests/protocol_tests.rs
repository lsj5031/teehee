//! Integration tests for the `protocol` module — exercise the public
//! encode / decode interface through real packets, no mocks.

use teehee::protocol::{Packet, ProtocolError, SampleFormat, HEADER_LEN, MAGIC, VERSION};

fn f32_pkt(seq: u32, frame_ts: u64, samples: &[f32]) -> Packet {
    Packet::new(seq, frame_ts, 48_000, 2, samples)
}

#[test]
fn header_layout_matches_pinned_constants() {
    // Pinned so future tweaks to the wire format are intentional.
    assert_eq!(MAGIC, *b"TEHE");
    assert_eq!(VERSION, 1);
    // 4 magic + 1 version + 4 seq + 8 frame_ts + 4 sample_rate + 1 ch + 1 fmt + 2 payload_len
    assert_eq!(HEADER_LEN, 25);
}

#[test]
fn encode_decode_roundtrip_preserves_every_field() {
    let samples = vec![0.0_f32, 1.0, -1.0, 0.5, -0.5, 0.25];
    let pcm_frames = samples.len() / 2; // 3 stereo frames
    let pkt = f32_pkt(42, 1_234_567, &samples);
    let bytes = pkt.encode();

    // Header + 6 * 4 bytes of f32 payload = 25 + 24
    assert_eq!(bytes.len(), HEADER_LEN + samples.len() * 4);

    let decoded = Packet::decode(&bytes).expect("valid packet must decode");
    assert_eq!(decoded.sequence, 42);
    assert_eq!(decoded.frame_timestamp, 1_234_567);
    assert_eq!(decoded.sample_rate, 48_000);
    assert_eq!(decoded.channels, 2);
    assert_eq!(decoded.sample_format, SampleFormat::F32);
    assert_eq!(decoded.pcm_frames(), pcm_frames);
    let back: Vec<f32> = decoded.pcm_f32().collect();
    assert_eq!(back, samples);
}

#[test]
fn encoded_packet_starts_with_magic_and_version() {
    let pkt = f32_pkt(0, 0, &[0.0, 0.0]);
    let bytes = pkt.encode();
    assert_eq!(&bytes[0..4], MAGIC);
    assert_eq!(bytes[4], VERSION);
}

#[test]
fn decode_rejects_invalid_magic() {
    let mut bytes = f32_pkt(1, 0, &[0.0, 0.0]).encode();
    bytes[0] = b'X';
    let err = Packet::decode(&bytes).unwrap_err();
    assert!(matches!(err, ProtocolError::BadMagic));
}

#[test]
fn decode_rejects_unsupported_version() {
    let mut bytes = f32_pkt(1, 0, &[0.0, 0.0]).encode();
    bytes[4] = 99;
    let err = Packet::decode(&bytes).unwrap_err();
    assert!(matches!(
        err,
        ProtocolError::UnsupportedVersion { found: 99 }
    ));
}

#[test]
fn decode_rejects_truncated_header() {
    let bytes = vec![0u8; 10]; // < HEADER_LEN
    let err = Packet::decode(&bytes).unwrap_err();
    assert!(matches!(err, ProtocolError::TruncatedHeader { .. }));
}

#[test]
fn decode_rejects_truncated_payload() {
    let mut bytes = f32_pkt(1, 0, &[0.0, 0.0]).encode();
    bytes.truncate(HEADER_LEN + 2); // only 2 bytes of declared 8-byte f32 payload
    let err = Packet::decode(&bytes).unwrap_err();
    assert!(matches!(err, ProtocolError::TruncatedPayload { .. }));
}

#[test]
fn decode_rejects_payload_length_mismatch() {
    // Build a packet whose declared payload_len doesn't match actual payload.
    let samples = vec![1.0_f32, -1.0];
    let pkt = f32_pkt(7, 0, &samples);
    let mut bytes = pkt.encode();
    let declared = u16::from_le_bytes([bytes[23], bytes[24]]) as usize;
    let new_len = (declared + 4) as u16;
    bytes[23..25].copy_from_slice(&new_len.to_le_bytes());
    let err = Packet::decode(&bytes).unwrap_err();
    assert!(matches!(err, ProtocolError::TruncatedPayload { .. }));
}

#[test]
fn decode_rejects_zero_channels() {
    let mut bytes = f32_pkt(1, 0, &[0.0, 0.0]).encode();
    bytes[21] = 0;

    let err = Packet::decode(&bytes).unwrap_err();
    assert!(
        matches!(err, ProtocolError::InvalidChannels { channels: 0 }),
        "zero-channel packet must be rejected before receiver ring setup can panic; got {err:?}"
    );
}

#[test]
fn decode_rejects_empty_f32_payload() {
    let bytes = Packet::new(1, 0, 48_000, 2, &[]).encode();

    let err = Packet::decode(&bytes).unwrap_err();
    assert!(
        matches!(
            err,
            ProtocolError::InvalidPayloadLength {
                payload_len: 0,
                channels: 2
            }
        ),
        "empty audio packet must be rejected before receiver capacity math can divide by zero; got {err:?}"
    );
}

#[test]
fn decode_rejects_non_frame_aligned_f32_payload() {
    let mut bytes = f32_pkt(1, 0, &[0.0, 0.0]).encode();
    // Declared length is inside the datagram but cannot hold a whole
    // stereo f32 frame (2 channels × 4 bytes).
    bytes[23..25].copy_from_slice(&4u16.to_le_bytes());

    let err = Packet::decode(&bytes).unwrap_err();
    assert!(
        matches!(
            err,
            ProtocolError::InvalidPayloadLength {
                payload_len: 4,
                channels: 2
            }
        ),
        "partial interleaved f32 frame must be rejected, got {err:?}"
    );
}

#[test]
fn decode_rejects_unsupported_sample_format_tag() {
    let mut bytes = f32_pkt(1, 0, &[0.0, 0.0]).encode();
    bytes[22] = 0xEE; // unknown sample-format tag
    let err = Packet::decode(&bytes).unwrap_err();
    assert!(matches!(err, ProtocolError::UnsupportedSampleFormat(0xEE)));
}

#[test]
fn sequence_wraparound_roundtrips() {
    let near_max = u32::MAX - 1;
    for seq in [near_max, u32::MAX, 0, 1, 2] {
        let pkt = f32_pkt(seq, seq as u64, &[0.0, 0.0]);
        let bytes = pkt.encode();
        let decoded = Packet::decode(&bytes).unwrap();
        assert_eq!(decoded.sequence, seq);
    }
}

#[test]
fn frame_timestamp_wide_roundtrips() {
    let ts = u64::MAX / 2;
    let pkt = f32_pkt(1, ts, &[0.0, 0.0]);
    let bytes = pkt.encode();
    let decoded = Packet::decode(&bytes).unwrap();
    assert_eq!(decoded.frame_timestamp, ts);
}

#[test]
fn multi_byte_fields_are_little_endian_on_the_wire() {
    // Three deliberate endianness probes pinned to the documented offsets.
    // This is part of the wire-format contract, not just an internal
    // detail — a big-endian Mac build must produce exactly these bytes.
    let pkt = Packet::new(0x0403_0201, 0, 48_000, 2, &[0.0, 0.0]);
    let bytes = pkt.encode();
    assert_eq!(
        &bytes[5..9],
        &[0x01, 0x02, 0x03, 0x04],
        "sequence is little-endian"
    );

    let pkt = Packet::new(1, 0, 0x0403_0201, 2, &[0.0, 0.0]);
    let bytes = pkt.encode();
    assert_eq!(
        &bytes[17..21],
        &[0x01, 0x02, 0x03, 0x04],
        "sample_rate is little-endian"
    );

    // payload_len = 8 (f32 stereo sample pair = 8 bytes), little-endian
    let pkt = Packet::new(1, 0, 48_000, 2, &[0.0, 0.0]);
    let bytes = pkt.encode();
    assert_eq!(
        &bytes[23..25],
        &[0x08, 0x00],
        "payload_len is little-endian"
    );
}

#[test]
fn payload_len_matches_actual_payload_bytes() {
    let samples = vec![0.0_f32; 256];
    let pkt = f32_pkt(1, 0, &samples);
    let bytes = pkt.encode();
    let declared = u16::from_le_bytes([bytes[23], bytes[24]]) as usize;
    let expected = samples.len() * std::mem::size_of::<f32>();
    assert_eq!(declared, expected);
    assert_eq!(bytes.len(), HEADER_LEN + expected);
}

// v1 receivers must hard-reject inbound I16-tagged packets rather than
// mis-decoding them as truncated f32 (which would split each 2-byte I16
// sample into half of a 4-byte f32 frame and play double-rate garbage
// at the speaker). The wire-format tag is preserved on the encode side
// so a future v2 receiver can negotiate I16 ↔ F32 without changing the
// schema.
#[test]
fn decode_rejects_i16_inbound_in_v1() {
    let mut bytes = f32_pkt(1, 0, &[0.0_f32, 0.0, 0.0, 0.0]).encode();
    assert_eq!(bytes[22], 0x01, "sanity: starting tag is F32");
    bytes[22] = 0x02;
    let err = Packet::decode(&bytes).unwrap_err();
    assert!(
        matches!(err, ProtocolError::UnsupportedInboundFormatI16),
        "I16 inbound must be rejected explicitly; got {:?}",
        err
    );

    // Precedence pin: I16 rejection takes priority over payload-length
    // errors. The decode method must surface "wrong format" before
    // "wrong length" so a misconfigured sender is diagnosed as a
    // format mismatch (not a streaming corruption). If a future
    // refactor reorders these checks, this assertion catches it.
    let mut i16_then_overrun = bytes.clone();
    let declared_len = u16::from_le_bytes([i16_then_overrun[23], i16_then_overrun[24]]);
    i16_then_overrun[23..25].copy_from_slice(&(declared_len + 4).to_le_bytes());
    let err2 = Packet::decode(&i16_then_overrun).unwrap_err();
    assert!(
        matches!(err2, ProtocolError::UnsupportedInboundFormatI16),
        "I16 reject must precede PayloadLength errors; got {:?}",
        err2
    );
}

// Property-pin: every v1 outbound packet must encode as F32 (0x01) at
// offset 22, and any I16-tagged version of the same packet must decode
// to `UnsupportedInboundFormatI16`. Tested across three payload sizes —
// empty, mid, and a realistic ~20 ms chunk at 48 kHz stereo — so a
// future refactor that conditionally tags the payload (e.g. "if
// channels > 2 use I16") would fail loudly on at least one size.
#[test]
fn v1_encode_always_emits_f32_tag_across_payload_sizes() {
    let sizes: &[usize] = &[0usize, 2, 960];
    for &sz in sizes {
        let samples = vec![0.0_f32; sz];
        let pkt = f32_pkt(7, 0, &samples);
        let bytes = pkt.encode();
        assert_eq!(
            bytes[22], 0x01,
            "v1 Packet::new must always tag samples as F32 (0x01) regardless of \
             payload size; got 0x{:02x} for {} samples",
            bytes[22], sz
        );
        let mut as_i16 = bytes.clone();
        as_i16[22] = 0x02;
        let dec = Packet::decode(&as_i16);
        assert!(
            matches!(dec, Err(ProtocolError::UnsupportedInboundFormatI16)),
            "I16-tagged variant of a v1 packet must decode to UnsupportedInboundFormatI16; \
             got {:?} for {} samples",
            dec,
            sz
        );
    }
}
