//! `tests/common/mod.rs` — shared helpers for teehee integration
//! tests.
//!
//! Each top-level integration test under `tests/` (`localhost_smoke.rs`,
//! `mtu_smoke.rs`, `mtu_boundary_sweep.rs`) includes this module via
//! `mod common;` at the top of the file. Cargo treats files inside
//! `tests/common/` (a subdirectory of `tests/`) as ordinary
//! compiled modules, NOT as additional test targets, so adding
//! helpers here never produces a phantom test binary. There are no
//! `#[test]` functions in this module — every item is a plain
//! helper consumed by each consumer test.
//!
//! Helpers are kept intentionally small and stable. If a test
//! needs a shared helper that is currently inline, prefer lifting
//! it here rather than duplicating across test files.

#![allow(dead_code)] // each consumer test binary compiles this once; helpers used by only one are silenced here

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use teehee::generated::SineSource;
use teehee::protocol::Packet;

/// Return a localhost `SocketAddr` with port `0` so the OS picks a
/// free ephemeral port. The receiver binds here; the actual port
/// is read via `Receiver::local_addr()` and passed to
/// `Sender::connect(...)`.
pub fn loopback_any_port() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
}

/// Number of PCM frames per packet at this sample rate + chunk_ms.
/// Integer arithmetic — floor of `sample_rate * chunk_ms / 1000`.
/// Caller multiplies by `channels` to get interleaved sample count.
pub fn chunk_frames(sample_rate: u32, chunk_ms: usize) -> usize {
    sample_rate as usize * chunk_ms / 1000
}

/// Number of interleaved PCM samples per packet:
/// `chunk_frames(sample_rate, chunk_ms) * channels`. Equal to the
/// `samples.len()` argument expected by `Packet::new` for the
/// payload slice of a packet.
pub fn chunk_samples(sample_rate: u32, chunk_ms: usize, channels: u8) -> usize {
    chunk_frames(sample_rate, chunk_ms) * channels as usize
}

/// True when running on a CI runner (GitHub Actions sets `CI=true`).
/// Hardware-dependent tests should skip on CI where no real audio
/// devices are available.
pub fn is_ci() -> bool {
    std::env::var("CI").is_ok()
}

/// Encode one wire packet carrying a `frequency_hz` sine at
/// `sample_rate` across `channels`, padded to `chunk_samples`
/// interleaved f32 samples. Returns the full wire-encoded `Vec<u8>`
/// — caller can record `encoded.len()` for size tracking, send via
/// `tx.send(&encoded)`, or compare against an MTU-derived
/// expected size.
///
/// Sample values are irrelevant for size tracking; the sine fill
/// just exercises the production `SineSource::fill_chunk` path so
/// any sine-source drift surfaces immediately in the consumer test.
pub fn encode_sine_packet(
    sequence: u32,
    frame_timestamp: u64,
    sample_rate: u32,
    channels: u8,
    chunk_samples: usize,
    frequency_hz: f32,
) -> Vec<u8> {
    let mut sine = SineSource::new(sample_rate, channels, frequency_hz);
    let mut buf = vec![0.0_f32; chunk_samples];
    sine.fill_chunk(&mut buf);
    Packet::new(sequence, frame_timestamp, sample_rate, channels, &buf).encode()
}
