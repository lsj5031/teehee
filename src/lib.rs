//! teehee — stream Windows system audio over UDP to a Mac.
//!
//! The crate is organised into deep, narrowly-interfaced modules so each
//! one can be exercised by integration tests through its public API:
//!
//! * [`protocol`] — packet encode/decode (deterministic, pure).
//! * [`jitter`]   — receive-side jitter buffer (pure, frame-based).
//! * [`generated`] — deterministic sine source for tests and dry-run mode.
//! * [`network`] — UDP socket adapters (real sockets, no mocks).
//! * [`audio_io`] — cpal capture/playback adapters.
//! * [`format_pipeline`] — slice-7 receiver-side sample-rate
//!   conversion + channel reconciliation (pure Rust, no cpal).
//! * [`loopback`] — slice-8 Windows WASAPI loopback capture
//!   (Windows-only build; non-Windows targets compile a stub).
//! * [`mtu_budget`] — slice-9 MTU strategy helpers. Pure math
//!   that converts the user-supplied `--mtu` link-MTU value into
//!   the per-sender payload envelope; used by `main` for
//!   fragment-on-overrun accounting.
//! * [`buffer_budget`] — slice-10 receiver-side buffer depth
//!   helpers. Pure math that converts the operator-supplied
//!   `--rx-buffer-ms` value into the `jitter::JitterBuffer`
//!   `capacity_packets` argument, with cross-flag validation
//!   against `--prebuffer-ms`.
//! * [`cli`]      — clap-derived CLI types.
//! * [`discovery`] — slice-12 mDNS auto-discovery (opt-in `--mdns`
//!   on both sides).

pub mod audio_io;
pub mod buffer_budget;
pub mod cli;
pub mod discovery;
pub mod format_pipeline;
pub mod generated;
pub mod jitter;
pub mod loopback;
pub mod mtu_budget;
pub mod network;
pub mod protocol;

pub use loopback::LoopbackCapturer;
pub use protocol::{Packet, SampleFormat, HEADER_LEN, MAGIC, VERSION};

// --- spike modules live below this line ---------------------------------
// Spike: fixed-capacity jitter buffer pattern borrowed from neoeinstein/aliri
// (rolling-sum jitter, O(1) read, no allocator on hot path). Compiled but
// not yet wired into runtime. Promote to `jitter::buffer` after a `cargo bench`.
#[allow(dead_code)]
pub mod spike_packet_ring;
