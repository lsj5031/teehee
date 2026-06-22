//! `protocol` — wire format for teehee audio packets.
//!
//! The protocol is intentionally simple: a fixed 25-byte little-endian
//! header followed by a PCM payload whose interpretation depends on the
//! `SampleFormat` tag. Every packet is independently interpretable so a
//! receiver can recover after startup packet loss.
//!
//! Wire layout (little-endian for all multi-byte fields):
//!
//! ```text
//! offset  size  field
//! ------  ----  -------------------------------------------------
//!   0       4   magic               = b"TEHE"
//!   4       1   version             = 0x01
//!   5       4   sequence            u32
//!   9       8   frame_timestamp     u64  (first frame in payload)
//!  17       4   sample_rate         u32  (Hz)
//!  21       1   channels            u8
//!  22       1   sample_format       u8   (see SampleFormat)
//!  23       2   payload_len         u16  (PCM payload byte length)
//!  25       ... PCM payload (interleaved, native byte order)
//! ```
//!
//! Each packet is independently interpretable: a fresh receiver can
//! restart decoding at any packet boundary without state from prior
//! packets outside the sequence number (used for jitter ordering).

use thiserror::Error;

/// Magic bytes at the start of every teehee packet.
pub const MAGIC: [u8; 4] = *b"TEHE";

/// Protocol version supported by this build.
pub const VERSION: u8 = 1;

/// Fixed header size in bytes.
pub const HEADER_LEN: usize = 25;

/// Maximum payload length supported by the [`u16`] field
/// (`u16::MAX` = 65 535 bytes — many seconds of audio at 48 kHz stereo f32).
pub const MAX_PAYLOAD_LEN: usize = u16::MAX as usize;

/// Sample-format tag stored in the packet header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SampleFormat {
    /// 32-bit floating-point PCM, little-endian.
    F32 = 0x01,
    /// 16-bit signed integer PCM, little-endian.
    ///
    /// **v1 inbound rejection**: teehee v1 receivers decode and emit
    /// only `f32` PCM. Inbound packets tagged `I16` are surfaced as a
    /// [`ProtocolError::UnsupportedInboundFormatI16`] rather than
    /// silently mis-decoded (which would split the 2-byte samples into
    /// 4-byte f32 frames and produce double-rate garbage on the
    /// speakers). The wire-format tag is preserved so future builds can
    /// negotiate `I16` ↔ `f32` (or `U16`) without changing the format
    /// byte.
    ///
    /// **v1 outbound**: [`Packet::new`] always tags its payload as
    /// [`SampleFormat::F32`] — the I16 enum variant is retained here
    /// only for forward-compatible wire format.
    I16 = 0x02,
}

impl SampleFormat {
    fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0x01 => Some(Self::F32),
            0x02 => Some(Self::I16),
            _ => None,
        }
    }

    fn tag(self) -> u8 {
        self as u8
    }

    /// Size in bytes of one PCM sample for this format.
    pub fn sample_size_bytes(self) -> usize {
        match self {
            Self::F32 => 4,
            Self::I16 => 2,
        }
    }
}

/// Errors returned when decoding a packet.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("packet too short to contain header ({have} < {HEADER_LEN})")]
    TruncatedHeader { have: usize },
    #[error("payload truncated: declared {needed} bytes but only {available} available")]
    TruncatedPayload { needed: usize, available: usize },
    #[error("packet magic did not match expected teehee marker")]
    BadMagic,
    #[error("unsupported protocol version: {found}")]
    UnsupportedVersion { found: u8 },
    #[error("unsupported sample format tag: 0x{0:02x}")]
    UnsupportedSampleFormat(u8),
    #[error("invalid channel count in packet header: {channels} (must be >= 1)")]
    InvalidChannels { channels: u8 },
    #[error(
        "invalid f32 payload length {payload_len} for {channels} channel(s); \
         payload must be non-empty and a whole number of interleaved f32 frames"
    )]
    InvalidPayloadLength { payload_len: usize, channels: u8 },
    /// Inbound `SampleFormat::I16` packets are surfaced as this error
    /// in teehee v1 receivers. v1 only emits and accepts interleaved
    /// `f32` PCM; an I16-tagged packet on the wire is most likely a
    /// misconfigured future-format sender. Update the remote sender
    /// to emit `f32` (the v1 default) and the receiver will accept
    /// the stream.
    #[error(
        "inbound I16 sample format is unsupported in teehee v1; \
             update sender to emit f32 (the default) for v1 receivers"
    )]
    UnsupportedInboundFormatI16,
}

/// A decoded teehee audio packet.
#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub sequence: u32,
    pub frame_timestamp: u64,
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_format: SampleFormat,
    /// PCM payload as interleaved samples. Always f32 in v1; the
    /// `sample_format` field nevertheless roundtrips so future format
    /// negotiation stays compatible on the wire.
    pub payload: Vec<u8>,
}

impl Packet {
    /// Construct a new packet carrying interleaved `f32` PCM samples.
    ///
    /// `samples.len()` must be a multiple of `channels`. v1 only
    /// supports f32 (see the [`SampleFormat`] enum — I16 is defined for
    /// forward-compatible wire format but the encode path in v1 always
    /// produces f32 bytes).
    ///
    /// **Inbound is not the same as outbound.** Inbound packets
    /// tagged `SampleFormat::I16` are explicitly hard-rejected by
    /// [`Packet::decode`] — see [`SampleFormat::I16`] for the v2
    /// forward-compat note. A future contributor who "fixes"
    /// this constructor to also accept `SampleFormat::I16` would
    /// break v1 receivers; keep the outbound path f32-locked.
    pub fn new(
        sequence: u32,
        frame_timestamp: u64,
        sample_rate: u32,
        channels: u8,
        samples: &[f32],
    ) -> Self {
        debug_assert!(channels > 0, "channels must be >= 1");
        debug_assert!(
            samples.len().is_multiple_of(channels as usize),
            "samples.len() must be a multiple of channels"
        );
        let mut payload = Vec::with_capacity(samples.len() * 4);
        for s in samples {
            payload.extend_from_slice(&s.to_le_bytes());
        }
        Self {
            sequence,
            frame_timestamp,
            sample_rate,
            channels,
            sample_format: SampleFormat::F32,
            payload,
        }
    }

    /// Number of PCM frames in the payload (samples / channels).
    pub fn pcm_frames(&self) -> usize {
        self.payload.len() / self.sample_format.sample_size_bytes() / self.channels as usize
    }

    /// Iterate the decoded payload as `f32` samples.
    pub fn pcm_f32(&self) -> impl Iterator<Item = f32> + '_ {
        self.payload
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
    }

    /// Append the decoded payload as `f32` samples to a caller-owned
    /// buffer. This is the **hot-path** API used by the jitter buffer
    /// and sender pipeline — it copies bytes directly into a `Vec<f32>`
    /// without the per-chunk allocation that [`Self::pcm_f32`] would
    /// trigger if collected into a fresh `Vec`.
    pub fn pcm_f32_into(&self, out: &mut Vec<f32>) {
        out.clear();
        out.reserve(self.payload.len() / 4);
        for c in self.payload.chunks_exact(4) {
            out.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
        }
    }

    /// Serialize the packet into its wire bytes.
    pub fn encode(&self) -> Vec<u8> {
        assert!(
            self.payload.len() <= MAX_PAYLOAD_LEN,
            "payload ({} bytes) exceeds MAX_PAYLOAD_LEN ({})",
            self.payload.len(),
            MAX_PAYLOAD_LEN
        );
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.sequence.to_le_bytes());
        out.extend_from_slice(&self.frame_timestamp.to_le_bytes());
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.push(self.channels);
        out.push(self.sample_format.tag());
        out.extend_from_slice(&(self.payload.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// Parse a packet from wire bytes. Returns [`ProtocolError`] for any
    /// framing or wire-format violation so callers see a clear "this
    /// packet is corrupt" signal rather than silently playing garbage.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < HEADER_LEN {
            return Err(ProtocolError::TruncatedHeader { have: bytes.len() });
        }
        if bytes[0..4] != MAGIC {
            return Err(ProtocolError::BadMagic);
        }
        if bytes[4] != VERSION {
            return Err(ProtocolError::UnsupportedVersion { found: bytes[4] });
        }
        let sequence = u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]);
        let frame_timestamp = u64::from_le_bytes([
            bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], bytes[16],
        ]);
        let sample_rate = u32::from_le_bytes([bytes[17], bytes[18], bytes[19], bytes[20]]);
        let channels = bytes[21];
        let sample_format = SampleFormat::from_tag(bytes[22])
            .ok_or(ProtocolError::UnsupportedSampleFormat(bytes[22]))?;
        // v1: I16 inbound is rejected explicitly to avoid the silent
        // pcm_f32_into() bug where a 2-byte I16 sample would be split
        // into half a 4-byte f32 frame, producing offset-and-rate-
        // shifted garbage at the receiver. The wire-format tag stays
        // valid for forward-compatible negotiation; the receiver
        // simply refuses to decode it until a v2 receiver-side I16
        // path lands.
        if sample_format == SampleFormat::I16 {
            return Err(ProtocolError::UnsupportedInboundFormatI16);
        }
        let payload_len = u16::from_le_bytes([bytes[23], bytes[24]]) as usize;

        if bytes.len() < HEADER_LEN + payload_len {
            return Err(ProtocolError::TruncatedPayload {
                needed: HEADER_LEN + payload_len,
                available: bytes.len(),
            });
        }
        let payload = bytes[HEADER_LEN..HEADER_LEN + payload_len].to_vec();

        if channels == 0 {
            return Err(ProtocolError::InvalidChannels { channels });
        }
        let frame_bytes = sample_format.sample_size_bytes() * channels as usize;
        if payload_len == 0 || !payload_len.is_multiple_of(frame_bytes) {
            return Err(ProtocolError::InvalidPayloadLength {
                payload_len,
                channels,
            });
        }

        Ok(Packet {
            sequence,
            frame_timestamp,
            sample_rate,
            channels,
            sample_format,
            payload,
        })
    }
}

impl TryFrom<u8> for SampleFormat {
    type Error = ();
    fn try_from(tag: u8) -> Result<Self, Self::Error> {
        SampleFormat::from_tag(tag).ok_or(())
    }
}

/// Cumulative diagnostics for the receive-side decode path.
///
/// `[recv]` shell pollutes this on every `Packet::decode` Err by
/// calling [`Self::record`]. v1 stats (debug + `--stats` log line)
/// read the snapshot back; the receiver loop keeps producing warn
/// lines per packet regardless. The 4-field rollup keeps log noise
/// low while preserving enough triage value to spot the dominant
/// failure mode (`truncated` ~= MTU/UDP-fragment issue;
/// `bad_format` ~= cross-version sender; `bad_magic` / `bad_version`
/// ~= talking to a totally different protocol).
///
/// `total()` is computed on demand — there is no separate `total`
/// field, so a future field add can't desync from the sum.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DecodeStats {
    /// `TruncatedHeader` | `TruncatedPayload`: header too short or
    /// payload_len extends past the datagram end. LAN packets hitting
    /// these imply MTU fragmentation (teehee emits well below MTU)
    /// or a sender that's truncating its packet emission.
    pub truncated: u64,
    /// `BadMagic`: prefix bytes ≠ `b"TEHE"`. Implies non-teehee UDP
    /// traffic on the same port — almost always a config typo
    /// (wrong receiver port, two senders colliding, etc.).
    pub bad_magic: u64,
    /// `UnsupportedVersion`: version byte > `VERSION` (v1 = 1). A
    /// future teehee sender without version negotiation will hit
    /// this; v1 receivers only accept `VERSION`.
    pub bad_version: u64,
    /// `UnsupportedSampleFormat` (unknown tag),
    /// `UnsupportedInboundFormatI16` (tag 0x02 explicitly rejected),
    /// `InvalidChannels`, and `InvalidPayloadLength`. Known-but-
    /// unsupported and malformed audio format metadata are grouped
    /// together because both share a fix path: fix/upgrade the sender
    /// so it emits non-empty interleaved F32 frames.
    pub bad_format: u64,
}

impl DecodeStats {
    /// Sum of all four fields. Computed on demand so a future
    /// field addition is automatically included and cannot desync
    /// from any caller-maintained `total` field.
    pub fn total(&self) -> u64 {
        self.truncated + self.bad_magic + self.bad_version + self.bad_format
    }

    /// Increment the right bucket for `err`. Called by the
    /// receive thread on every `Packet::decode => Err(_)`. Exhaustive
    /// match — if a new `ProtocolError` variant is added, this
    /// function must be updated (Rust's match enforcement catches it).
    /// Pass-by-reference: keeps the receive thread's `e` alive so
    /// `warn!(error = %e, ...)` (which borrows for Display) can fire
    /// in the same match arm without an unreachable_code dance.
    pub fn record(&mut self, err: &ProtocolError) {
        match err {
            ProtocolError::TruncatedHeader { .. } | ProtocolError::TruncatedPayload { .. } => {
                self.truncated += 1
            }
            ProtocolError::BadMagic => self.bad_magic += 1,
            ProtocolError::UnsupportedVersion { .. } => self.bad_version += 1,
            ProtocolError::UnsupportedSampleFormat(_)
            | ProtocolError::InvalidChannels { .. }
            | ProtocolError::InvalidPayloadLength { .. }
            | ProtocolError::UnsupportedInboundFormatI16 => self.bad_format += 1,
        }
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn sample_size_bytes_per_format() {
        // I16 tag exists for forward-compatible wire format; v1 only emits f32.
        assert_eq!(SampleFormat::F32.sample_size_bytes(), 4);
        assert_eq!(SampleFormat::I16.sample_size_bytes(), 2);
    }

    // ----- DecodeStats -----

    #[test]
    fn default_decode_stats_is_all_zero_with_zero_total() {
        let s = DecodeStats::default();
        assert_eq!(s.truncated, 0);
        assert_eq!(s.bad_magic, 0);
        assert_eq!(s.bad_version, 0);
        assert_eq!(s.bad_format, 0);
        assert_eq!(s.total(), 0);
    }

    #[test]
    fn decode_stats_record_maps_each_variant_to_correct_bucket() {
        let mut s = DecodeStats::default();
        // Routing: each variant lands in exactly one bucket, no
        // double-counting, no misses. Exhaustive across ProtocolError.
        s.record(&ProtocolError::TruncatedHeader { have: 10 });
        assert_eq!(s.truncated, 1);
        assert_eq!(s.total(), 1);

        s.record(&ProtocolError::TruncatedPayload {
            needed: 100,
            available: 50,
        });
        assert_eq!(s.truncated, 2);
        assert_eq!(s.total(), 2);

        s.record(&ProtocolError::BadMagic);
        assert_eq!(s.bad_magic, 1);
        assert_eq!(s.total(), 3);

        s.record(&ProtocolError::UnsupportedVersion { found: 2 });
        assert_eq!(s.bad_version, 1);
        assert_eq!(s.total(), 4);

        s.record(&ProtocolError::UnsupportedSampleFormat(0xEE));
        assert_eq!(s.bad_format, 1);
        assert_eq!(s.total(), 5);

        s.record(&ProtocolError::UnsupportedInboundFormatI16);
        assert_eq!(s.bad_format, 2);
        assert_eq!(s.total(), 6);

        s.record(&ProtocolError::InvalidChannels { channels: 0 });
        assert_eq!(s.bad_format, 3);
        assert_eq!(s.total(), 7);

        s.record(&ProtocolError::InvalidPayloadLength {
            payload_len: 2,
            channels: 2,
        });
        assert_eq!(s.bad_format, 4);
        assert_eq!(s.total(), 8);
    }

    #[test]
    fn decode_stats_record_is_exhaustive_at_compile_time() {
        // This test exists to make the exhaustive match in `record`
        // a compile-time invariant: if a new ProtocolError variant
        // is added without updating `record`, the compilation will
        // (correctly) fail. The test itself just confirms all
        // current variants route correctly.
        let mut s = DecodeStats::default();
        let variants = [
            ProtocolError::TruncatedHeader { have: 0 },
            ProtocolError::TruncatedPayload {
                needed: 0,
                available: 0,
            },
            ProtocolError::BadMagic,
            ProtocolError::UnsupportedVersion { found: 0 },
            ProtocolError::UnsupportedSampleFormat(0),
            ProtocolError::InvalidChannels { channels: 0 },
            ProtocolError::InvalidPayloadLength {
                payload_len: 2,
                channels: 2,
            },
            ProtocolError::UnsupportedInboundFormatI16,
        ];
        // Cache length before the consuming loop: `record()` takes
        // ProtocolError by value, so the loop moves `variants`.
        let n_variants = variants.len() as u64;
        for v in variants {
            s.record(&v);
        }
        assert_eq!(s.total(), n_variants);
    }
}
