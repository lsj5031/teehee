//! `cli` — clap-derived `teehee` command-line surface.
//!
//! Three primary subcommands:
//!
//! * `teehee send --host <mac-ip> [--port 5000] [--chunk-ms 20] [--stats] [--sine]`
//!   —capture (or generate) audio and ship it via `protocol::Packet` over UDP.
//! * `teehee recv [--port 5000] [--prebuffer-ms 200] [--stats]`
//!   —listen on a UDP port and play back through the default audio device.
//! * `teehee devices` — print available audio devices on this machine.

use clap::{Args, Parser, Subcommand};

/// Top-level `teehee` invocation. Parses via [`clap::Parser::try_parse_from`].
#[derive(Debug, Parser)]
#[command(
    name = "teehee",
    version,
    about = "Stream Windows system audio to a Mac over UDP",
    long_about = "teehee ships audio over a small UDP packet protocol without \
                  AirPlay, Python, or paid proprietary tools. Use only on trusted LANs."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// One of the three primary subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Capture (or generate) audio and ship it to the receiver.
    #[command(
        name = "send",
        about = "Capture (or generate) audio and ship it to the receiver.",
        long_about = "Capture (or generate) audio and ship it to the receiver.\n\n\
                     v1 LOOPBACK LIMITATION: cpal 0.15 does not expose a cross-platform \
                     loopback primitive, so capture without --sine reads from the OS \
                     *default input device* (typically a microphone), NOT system audio. \
                     To stream system audio on Windows / macOS / Linux you must first \
                     enable a loopback input:\n  \
                     \n  - Windows: enable \"Stereo Mix\" (or your sound card's \
                     loopback input) in the sound control panel and mark it default.\n  \
                     \n  - macOS: install BlackHole (or Loopback by Rogue Amoeba) and \
                     route system audio to it as default input.\n  \
                     \n  - Linux: enable the PulseAudio monitor of your default sink.\n\n\
                     When in doubt, --sine generates a 440 Hz tone at the default format \
                     so you can verify the receiver on a single machine without any \
                     hardware-capture setup.\n\n\
                     --capture-source controls the audio source for real capture (slice 8 / \
                     slice 11): `default` opens the OS default-input device as before, \
                     `loopback` opens the OS default *render* endpoint with WASAPI's \
                     AUDCLNT_STREAMFLAGS_LOOPBACK so the sender captures system audio on \
                     Windows without requiring Stereo Mix, and `auto` probes the OS \
                     default-input first and falls back to WASAPI loopback on Windows \
                     when that fails (slice 11 — the `auto` value is the recommended \
                     default on Windows for users who don't know whether their rig has \
                     Stereo Mix enabled or BlackHole installed; it Just Works on a \
                     stock Windows desktop with a default audio device). `loopback` is \
                     Windows-only; on macOS/Linux it returns a clean error so the user \
                     can fall back to `--capture-source=default` (with BlackHole / \
                     PulseAudio monitor configured as the system default input) or to \
                     `--sine`.                     `auto` is also Windows-aware: on macOS / Linux the \
                     loopback fallback is unreachable, so `auto` behaves identically to \
                     `default` and the per-packet flow is unchanged.\n\
                     WARNING (auto + silent microphone): --capture-source=auto tries \
                     the default-input path FIRST. On a Windows desktop with a working \
                     microphone as the system-default input, --capture-source=auto \
                     succeeds at the default-input step and never reaches the WASAPI \
                     fallback -- meaning it broadcasts whatever your microphone hears. \
                     For system audio (browser, Spotify, YouTube, system beeps) instead \
                     of microphone audio, use --capture-source=loopback EXPLICITLY so \
                     the probe order is bypassed and teehee captures the render \
                     endpoint's mix.\n\n\
                     --sample-rate and --channels apply ONLY to --sine dry-run mode. \
                     For real capture, the cpal / WASAPI device's actual sample rate \
                     and channel count are used; if they differ from the CLI values, \
                     the mismatch is surfaced at startup so the divergence is visible."
    )]
    Send(SendArgs),
    /// Listen on a UDP port and play audio on the default output device.
    /// Format reconciliation is automatic (slice 7): if the sender's
    /// sample rate or channel count differs from the receiver's cpal
    /// default output device, the receiver transparently resamples
    /// (linear interpolation — voice-grade for LAN ratios) and
    /// reconciles channels (mono↔stereo average/broadcast; defensive
    /// 1→N up-mix and N→M down-mix for unusual layouts). No CLI
    /// flag is required; the receiver auto-discovers formats from
    /// the wire and the cpal device at first packet. Run with
    /// `--stats` to see sender vs receiver sample-rate, channel
    /// count, and per-second conversion activity when formats
    /// differ.
    Recv(RecvArgs),
    /// List audio devices available on this machine.
    Devices,
}

/// Configuration for the `send` subcommand.
///
/// Either the positional `HOST` argument OR the `--host` flag must be
/// supplied; supplying both is rejected. The `HOST` field may also
/// embed a port in `host:port` form — that port is used unless
/// `--port` is also explicitly set, in which case the call is
/// rejected with an "ambiguous port" error rather than silently
/// doubling into `host:port:port`.
#[derive(Debug, Args)]
pub struct SendArgs {
    /// Destination host — IPv4/IPv6 literal or hostname. May also
    /// embed a port in `host:port` form (e.g. `192.168.0.10:6000`).
    /// Either this positional OR `--host` is required.
    #[arg(value_name = "HOST")]
    pub host: Option<String>,

    /// Same as the positional `HOST`, but explicit. Praised by the
    /// PRD's canonical command form (`teehee send --host <ip>`).
    #[arg(long = "host", value_name = "HOST", conflicts_with = "host")]
    pub host_flag: Option<String>,

    /// UDP destination port. Defaults to 5000; if the `HOST` field
    /// already encodes a port (`host:port`), `--port` MUST be left
    /// at its default or the call is rejected as ambiguous.
    #[arg(long, default_value_t = 5000, value_parser = parse_port)]
    pub port: u16,

    /// Encoder chunk duration in milliseconds. Smaller chunks = lower latency,
    /// higher packet rate. Keep payload well below the LAN MTU.
    #[arg(long, default_value_t = 20, value_parser = parse_chunk_ms)]
    pub chunk_ms: usize,

    /// Link MTU in bytes (the OS-level maximum payload the network
    /// interface accepts in a single frame). Defaults to 1500 (typical
    /// Ethernet LAN). Range [576, 9000]: 576 is the IPv6 RFC-minimum,
    /// 9000 is jumbo-frame Ethernet. The sender is MTU-aware — it
    /// logs the configured MTU, its derived payload envelope, and the
    /// current chunk-ms × audio-params packet size at startup so the
    /// operator sees the relationship. Packets that overshoot the
    /// envelope are still emitted and let the OS handle IP-layer
    /// fragmentation, but each event increments a `fragmentations`
    /// counter on the `--stats` line so a misconfiguration is
    /// visible at runtime.
    #[arg(long, default_value_t = crate::mtu_budget::MTU_DEFAULT_BYTES, value_parser = parse_mtu)]
    pub mtu: usize,

    /// Optional sample-rate override when paired with `--sine`.
    #[arg(long, default_value_t = 48_000)]
    pub sample_rate: u32,

    /// Optional channel-count override (1 or 2). Default 2 (stereo).
    #[arg(long, default_value_t = 2)]
    pub channels: u8,

    /// Generate a 440 Hz sine wave instead of capturing real audio. Useful
    /// for verifying the receiver on a single machine or running the
    /// localhost smoke test.
    #[arg(long)]
    pub sine: bool,

    /// Print periodic sender stats (packets sent, overrun counters).
    #[arg(long)]
    pub stats: bool,

    /// Capture source. Three values, picked by the operator's
    /// confidence in the host's audio setup:
    ///
    /// * `default` — cpal opens the OS default input device (the
    ///   existing v1 path; needs Stereo Mix on Windows, BlackHole or
    ///   PulseAudio monitor on macOS/Linux).
    /// * `loopback` — opens the OS default *render* endpoint with
    ///   WASAPI's `AUDCLNT_STREAMFLAGS_LOOPBACK` and captures system
    ///   audio on Windows without requiring Stereo Mix.
    ///   **Windows-only**; on macOS/Linux it returns a clean error
    ///   so the user can fall back to `--capture-source=default` with
    ///   a virtual input device or to `--sine`. Slice 8.
    /// * `auto` — probe `default` first; on Windows, if `default`
    ///   returns an error, transparently fall back to `loopback`.
    ///   On macOS/Linux the loopback fallback is unreachable
    ///   (Windows-only), so `auto` behaves identically to `default`.
    ///   Slice 11.
    ///
    ///   WARNING: silent-mic pitfall. `auto` tries `default` first,
    ///   so on a Windows desktop with a working microphone as the
    ///   system-default input, `auto` will succeed at the
    ///   default-input step and *never reach the WASAPI fallback*,
    ///   broadcasting whatever the microphone hears. If you want
    ///   system audio (not microphone audio) on Windows, use
    ///   `--capture-source=loopback` explicitly instead. The helper
    ///   logs which capture path it landed on (`cpal default input
    ///   (auto-probed)` or `WASAPI loopback (auto-fallback)`) at
    ///   startup; grep `--stats` for the substring `auto-` to
    ///   confirm the probe decision.
    #[arg(long, value_enum, default_value_t = CaptureSource::Default)]
    pub capture_source: CaptureSource,

    /// Strict-mode flag: when true, REJECTS `--capture-source=auto`
    /// (or its omit-default variant) at the parse / validate layer so
    /// the operator must explicitly type `default` or `loopback`.
    /// Slice 11 — fleet-managed / shared machines where the default-
    /// input device is unpredictable and the silent-mic pitfall is
    /// unacceptable. Without this flag, `auto` parses cleanly
    /// (preserving v1 behaviour).
    #[arg(long, default_value_t = false)]
    pub exact_capture_source: bool,
}

/// Capture-source selector for `teehee send`. Slice 8 added the
/// `Loopback` variant alongside the existing default-input device path;
/// slice 11 added `Auto` which probes `Default` first and falls back to
/// `Loopback` on Windows when `Default` fails.
///
/// clap derives `--capture-source={default|loopback|auto}` from this
/// enum. Default = `default` (preserves v1 behaviour for existing
/// scripts / shell aliases — `auto` is opt-in because it has a
/// different Windows-host semantics: a missing-loopback-fallback path
/// could silently ship mic audio where the operator expected system
/// audio; explicit `auto` is required to opt into the lenient
/// detection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum CaptureSource {
    /// Capture from the OS default input device via cpal (v1 path).
    Default,
    /// Capture from the OS default render endpoint via WASAPI
    /// loopback (Windows-only, slice 8).
    Loopback,
    /// Probe the OS default input device first, then fall back to
    /// WASAPI render-loopback on Windows when the default-input
    /// attempt returns an error. On macOS/Linux the loopback
    /// fallback is unreachable (Windows-only), so `auto` behaves
    /// identically to `default` on those hosts. Slice 11.
    Auto,
}

/// Configuration for the `recv` subcommand.
#[derive(Debug, Args)]
pub struct RecvArgs {
    /// UDP listen port.
    #[arg(long, default_value_t = 5000)]
    pub port: u16,

    /// Prebuffer duration in milliseconds. The receiver waits in
    /// silence until at least this many ms of audio have accumulated
    /// in the jitter buffer, then it starts playing through. The
    /// underlying ring capacity is derived from [`Self::rx_buffer_ms`]
    /// (slice 10) — this knob is the *gate target only*; the ring
    /// itself is sized on `rx_buffer_ms`. Slice 6 hard-break:
    /// replaces the ambiguous `--buffer-ms` (which used to mean
    /// ring slot count, a leaky abstraction).
    #[arg(
        long,
        default_value_t = 200,
        value_parser = parse_prebuffer_ms
    )]
    pub prebuffer_ms: usize,

    /// Receive-buffer depth in milliseconds. Slice 10 (Tier 3 #9).
    /// This is the *total* ring capacity expressed in ms of audio,
    /// including the prebuffer gate. Defaults to 2000 ms (10× the
    /// default prebuffer-ms=200) which holds 100 packets at 48 kHz
    /// stereo / chunk-ms=20 — generous enough for typical home-Wi-Fi
    /// bursts. Range [100, 30000]: 100 ms is the smallest ring that
    /// still satisfies the `max(32, ...)` OS-memory floor, 30 s is
    /// the largest sane ring without blowing OS memory at high
    /// channel counts.
    ///
    /// **Cross-flag invariant**: `rx_buffer_ms >= prebuffer_ms` —
    /// the gate target can never exceed the ring size, or playback
    /// would block indefinitely. Violations are surfaced as a
    /// validation error at startup, not mid-stream.
    ///
    /// Senders that burst-pace over `--rx-buffer-ms / chunk-ms`
    /// packets stored ahead of the play head trigger the
    /// `ring_overruns` counter on the receivers `--stats` line —
    /// raise `--rx-buffer-ms` for more headroom or shrink the
    /// sender's `--chunk-ms` to reduce the burst rate.
    #[arg(
        long,
        default_value_t = crate::buffer_budget::RX_BUFFER_DEFAULT_MS,
        value_parser = parse_rx_buffer_ms
    )]
    pub rx_buffer_ms: usize,

    /// Print periodic receiver stats (jitter fill, late drops,
    /// underruns, and — when the sender and receiver formats differ
    /// — sample-rate / channel-count reconciliation activity).
    /// Slice 7: when format conversion runs, the line shape
    /// broadens to include `sender_sample_rate`,
    /// `sender_channels`, `receiver_sample_rate`,
    /// `receiver_channels`, and `fp_in` / `fp_out` interleaved
    /// sample counts (so operators can spot mismatched-cross-host
    /// formats and confirm the conversion is active). When sender
    /// and receiver formats match, the line stays identical to
    /// slice-6 to keep operators' grep muscle memory intact.
    #[arg(long)]
    pub stats: bool,
}

// Validation hooks: clap's `value_parser` covers prebuffer_ms above;
// we add chunk_ms and channels validation through custom parsers.

/// Parse `--prebuffer-ms` with an inclusive 20..=10_000 range. The
/// lower bound matches the smallest useful chunk duration (2 chunks
/// at 20 ms); the upper bound keeps the ring capacity derivation
/// within a sane OS-memory budget. The actual ring depth is
/// `--rx_buffer-ms`, not this value (slice 10).
fn parse_prebuffer_ms(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    if !(20..=10_000).contains(&n) {
        return Err(format!("prebuffer-ms must be in 20..=10000 (got {n})"));
    }
    Ok(n)
}

/// Parse `--chunk-ms` with an inclusive 1..=200 range. Note: clap's
/// default range check happens at parse time and is reported as a
/// value validation error.
fn parse_chunk_ms(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    if !(1..=200).contains(&n) {
        return Err(format!("chunk-ms must be in 1..=200 (got {n})"));
    }
    Ok(n)
}

/// Parse `--rx-buffer-ms` with an inclusive 100..=30_000 range.
/// Slice 10 (Tier 3 #9). The range check mirrors
/// [`crate::buffer_budget::RX_BUFFER_MIN_MS`] /
/// [`crate::buffer_budget::RX_BUFFER_MAX_MS`]; clap's value_parser
/// enforces it at the CLI parse boundary so out-of-range values
/// are surfaced as a clean start-up error rather than a generic
/// clap arg-not-found. The cross-flag
/// `rx_buffer_ms >= prebuffer_ms` invariant is checked separately
/// inside [`RecvArgs::validate`] because clap can't compare flags
/// against each other at parse time.
fn parse_rx_buffer_ms(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    let min = crate::buffer_budget::RX_BUFFER_MIN_MS;
    let max = crate::buffer_budget::RX_BUFFER_MAX_MS;
    if !(min..=max).contains(&n) {
        return Err(format!("rx-buffer-ms must be in {min}..={max} (got {n})"));
    }
    Ok(n)
}
/// Parse `--mtu` with an inclusive [576, 9000] range. 576 is the
/// IPv6 RFC-minimum (RFC 2460 / RFC 8200 path-MTU floor); 9000 is
/// jumbo-frame Ethernet. The `--mtu` value is the LINK MTU
/// (Ethernet / Wi-Fi frame payload after L2 stripping); the OS adds
/// IP + UDP + teehee headers separately, and the sender subtracts
/// that fixed 53-byte framing overhead before computing the
/// envelope.
fn parse_mtu(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    if !(576..=9000).contains(&n) {
        return Err(format!("mtu must be in 576..=9000 (got {n})"));
    }
    Ok(n)
}

/// Parse `--port` so out-of-range values are surfaced as a clean value
/// validation error rather than a generic clap argument-not-found.
fn parse_port(s: &str) -> Result<u16, String> {
    let n: u32 = s
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    if n > u16::MAX as u32 {
        return Err(format!("port must be in 1..={} (got {n})", u16::MAX));
    }
    Ok(n as u16)
}

/// A destination resolved from `SendArgs`. Returned by
/// [`SendArgs::validate`] so `run_send` formats a single
/// `host:port` string with no further logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub host: String,
    pub port: u16,
}

impl ResolvedTarget {
    /// `host:port` form ready for `Sender::connect`.
    pub fn to_socket_string(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Split a `HOST` field into `(hostname, embedded_port?)`.
/// Accepts:
/// * `host` (no port)
/// * `host:port` (one colon — IPv4 / hostname)
/// * `[ipv6]` (no port)
/// * `[ipv6]:port`
///
/// Bare `::1` (multi-colon without brackets) is treated as a
/// hostname with no embedded port, which fails downstream `connect`
/// for IPv6 — callers should use the bracketed form.
pub(crate) fn parse_host_port(raw: &str) -> Result<(String, Option<u16>), String> {
    if let Some(rest) = raw.strip_prefix('[') {
        // IPv6 [host] or [host]:port
        let end = rest
            .find(']')
            .ok_or_else(|| format!("unterminated '[' in '{raw}'"))?;
        let host = &rest[..end];
        let after = &rest[end + 1..];
        if let Some(port_str) = after.strip_prefix(':') {
            if port_str.is_empty() {
                return Err(format!("empty port in '{raw}'"));
            }
            let port: u16 = port_str
                .parse()
                .map_err(|e| format!("invalid port in '{raw}': {e}"))?;
            return Ok((format!("[{host}]"), Some(port)));
        }
        if !after.is_empty() {
            return Err(format!(
                "unexpected trailing '{after}' after ']' in '{raw}'"
            ));
        }
        return Ok((format!("[{host}]"), None));
    }

    let colon_count = raw.matches(':').count();
    match colon_count {
        0 => Ok((raw.to_string(), None)),
        1 => {
            let (host, port_str) = raw.split_once(':').expect("by count");
            if port_str.is_empty() {
                return Err(format!("empty port in '{raw}'"));
            }
            let port: u16 = port_str
                .parse()
                .map_err(|e| format!("invalid port in '{raw}': {e}"))?;
            Ok((host.to_string(), Some(port)))
        }
        _ => {
            // Multiple unbracketed colons = bare IPv6, no port.
            Ok((raw.to_string(), None))
        }
    }
}

impl RecvArgs {
    /// Validate parsed `RecvArgs`. Slice 10 cross-flag check:
    /// `rx_buffer_ms >= prebuffer_ms` so the prebuffer gate
    /// target never exceeds the ring's `capacity_packets`. clap
    /// can enforce range checks on each flag individually but
    /// can't compare two flags against each other — that's
    /// surfaced here, at startup, before any cpal stream or
    /// socket is opened.
    pub fn validate(&self) -> Result<(), String> {
        if self.rx_buffer_ms < self.prebuffer_ms {
            return Err(format!(
                "--rx-buffer-ms ({rx}) must be >= --prebuffer-ms ({pre}); \\\n                 the prebuffer gate target must fit inside the ring it is \\\n                 gating — raise --rx-buffer-ms or lower --prebuffer-ms",
                rx = self.rx_buffer_ms,
                pre = self.prebuffer_ms,
            ));
        }
        Ok(())
    }
}

impl SendArgs {
    /// Validate parsed args and return a resolved `host:port` target.
    /// Range-checks `channels` / `chunk-ms` AND reconciles the
    /// positional `HOST` argument with the `--host` flag AND any
    /// `host:port`-embedded port against `--port`.
    pub fn validate(&self) -> Result<ResolvedTarget, String> {
        if self.channels == 0 || self.channels > 8 {
            return Err(format!("channels must be in 1..=8 (got {})", self.channels));
        }
        if !(1..=200).contains(&self.chunk_ms) {
            return Err(format!(
                "chunk-ms must be in 1..=200 (got {})",
                self.chunk_ms
            ));
        }

        // Slice-11 strict-mode: when --exact-capture-source is set,
        // reject the auto probe path. Operators on shared / fleet
        // machines have already configured their default-input
        // device (Stereo Mix, BlackHole, PulseAudio monitor) and
        // want the operator (themself) to type the exact value.
        // Auto's probe logic is disabled at the parse path so the
        // silent-mic pitfall is unreachable: the operator MUST
        // pass --capture-source=default or --capture-source=loopback
        // explicitly.
        if self.exact_capture_source && self.capture_source == CaptureSource::Auto {
            return Err(
                "--exact-capture-source: --capture-source=auto is rejected. \
                 Pass --capture-source=default (or =loopback on Windows) \
                 explicitly, or unset --exact-capture-source to keep the \
                 auto-probe behaviour."
                    .to_string(),
            );
        }

        // Source the host string from exactly one of the two fields.
        let raw_host = match (self.host.as_deref(), self.host_flag.as_deref()) {
            (Some(h), None) => h,
            (None, Some(h)) => h,
            (Some(_), Some(_)) => {
                return Err(
                    "specify destination as either HOST positional or --host, not both".into(),
                );
            }
            (None, None) => {
                return Err("destination host required (pass positional HOST or --host)".into());
            }
        };

        // Embedded-port reconciliation with --port.
        let (host, embedded_port) = parse_host_port(raw_host)?;
        let port = match (embedded_port, self.port) {
            (Some(p), 5000) => p,
            (Some(p), p_override) => {
                return Err(format!(
                    "ambiguous port: --port {p_override} conflicts with embedded \
                     port {p} in '{raw_host}'; pass only one"
                ));
            }
            (None, p) => p,
        };

        Ok(ResolvedTarget { host, port })
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    fn make_args(host: Option<&str>, host_flag: Option<&str>, port: u16) -> SendArgs {
        SendArgs {
            host: host.map(String::from),
            host_flag: host_flag.map(String::from),
            port,
            chunk_ms: 20,
            mtu: crate::mtu_budget::MTU_DEFAULT_BYTES,
            sample_rate: 48_000,
            channels: 2,
            sine: false,
            stats: false,
            // Slice 8: existing tests default to default-input path
            // (preserves v1 behavior under the new field).
            capture_source: CaptureSource::Default,
            // Slice 11 strict-mode: tests default to off (false) so
            // the v1 / slice-8 / slice-10 behaviour is unchanged
            // across pre-existing tests.
            exact_capture_source: false,
        }
    }

    // ----- Range / format validation (existing tests adapted) -----

    #[test]
    fn send_args_validate_accepts_defaults() {
        let t = make_args(Some("127.0.0.1"), None, 5000)
            .validate()
            .expect("ok");
        assert_eq!(t.host, "127.0.0.1");
        assert_eq!(t.port, 5000);
    }

    #[test]
    fn send_args_validate_rejects_zero_chunk() {
        let mut args = make_args(Some("127.0.0.1"), None, 5000);
        args.chunk_ms = 0;
        let err = args.validate().unwrap_err();
        assert!(err.contains("chunk-ms"));
    }

    #[test]
    fn send_args_validate_rejects_zero_channels() {
        let mut args = make_args(Some("127.0.0.1"), None, 5000);
        args.channels = 0;
        let err = args.validate().unwrap_err();
        assert!(err.contains("channels"));
    }

    // ----- Host sourcing / --host flag -----

    #[test]
    fn validate_accepts_long_form_host() {
        let t = make_args(None, Some("10.0.0.5"), 5000)
            .validate()
            .expect("ok");
        assert_eq!(t.host, "10.0.0.5");
        assert_eq!(t.port, 5000);
    }

    #[test]
    fn validate_host_port_form_uses_embedded_port() {
        let t = make_args(Some("10.0.0.5:6000"), None, 5000)
            .validate()
            .expect("ok");
        assert_eq!(t.host, "10.0.0.5");
        assert_eq!(t.port, 6000);
    }

    #[test]
    fn validate_flag_host_with_embedded_port_uses_embedded() {
        let t = make_args(None, Some("10.0.0.5:6000"), 5000)
            .validate()
            .expect("ok");
        assert_eq!(t.host, "10.0.0.5");
        assert_eq!(t.port, 6000);
    }

    #[test]
    fn validate_rejects_port_override_when_embedded_set() {
        let err = make_args(Some("10.0.0.5:6000"), None, 6001)
            .validate()
            .unwrap_err();
        assert!(err.contains("ambiguous port"));
        assert!(err.contains("6000"));
        assert!(err.contains("6001"));
    }

    #[test]
    fn validate_rejects_missing_host() {
        let err = make_args(None, None, 5000).validate().unwrap_err();
        assert!(err.contains("destination host required"));
    }

    #[test]
    fn validate_rejects_both_positional_and_flag_host() {
        let err = make_args(Some("127.0.0.1"), Some("10.0.0.5"), 5000)
            .validate()
            .unwrap_err();
        assert!(err.contains("not both"));
    }

    #[test]
    fn validate_rejects_invalid_embedded_port() {
        let err = make_args(Some("127.0.0.1:notaport"), None, 5000)
            .validate()
            .unwrap_err();
        assert!(err.contains("invalid port"));
    }

    #[test]
    fn validate_rejects_empty_embedded_port() {
        let err = make_args(Some("127.0.0.1:"), None, 5000)
            .validate()
            .unwrap_err();
        assert!(err.contains("empty port") || err.contains("invalid port"));
    }

    // ----- parse_host_port coverage -----

    #[test]
    fn parse_host_port_ipv4_no_port() {
        let (h, p) = parse_host_port("192.168.0.10").unwrap();
        assert_eq!(h, "192.168.0.10");
        assert_eq!(p, None);
    }

    #[test]
    fn parse_host_port_ipv4_with_port() {
        let (h, p) = parse_host_port("192.168.0.10:6000").unwrap();
        assert_eq!(h, "192.168.0.10");
        assert_eq!(p, Some(6000));
    }

    #[test]
    fn parse_host_port_ipv6_bracketed_no_port() {
        let (h, p) = parse_host_port("[fe80::1]").unwrap();
        assert_eq!(h, "[fe80::1]");
        assert_eq!(p, None);
    }

    #[test]
    fn parse_host_port_ipv6_bracketed_with_port() {
        let (h, p) = parse_host_port("[fe80::1]:6000").unwrap();
        assert_eq!(h, "[fe80::1]");
        assert_eq!(p, Some(6000));
    }

    #[test]
    fn parse_host_port_bare_ipv6_treated_as_hostname() {
        let (h, p) = parse_host_port("::1").unwrap();
        assert_eq!(h, "::1");
        assert_eq!(p, None);
    }

    #[test]
    fn parse_host_port_rejects_out_of_range_port() {
        assert!(parse_host_port("192.168.0.10:99999").is_err());
    }

    #[test]
    fn parse_host_port_rejects_unterminated_bracket() {
        assert!(parse_host_port("[fe80::1:6000").is_err());
    }

    #[test]
    fn parse_host_port_rejects_unterminated_bracket_no_port() {
        assert!(parse_host_port("[fe80::1").is_err());
    }

    // ----- clap parse-from coverage for the failure modes the review
    //      explicitly flagged (cargo run send --host 127.0.0.1 must
    //      not error with "unexpected argument"). -----

    #[test]
    fn clap_parses_dash_dash_host_flag() {
        let cli = Cli::try_parse_from(["teehee", "send", "--host", "10.0.0.5", "--sine"])
            .expect("clap must accept --host <ip>");
        let Command::Send(args) = cli.command else {
            panic!("expected Send subcommand");
        };
        assert!(args.host.is_none());
        assert_eq!(args.host_flag.as_deref(), Some("10.0.0.5"));
        assert!(args.sine);
    }

    #[test]
    fn clap_parses_positional_host() {
        let cli = Cli::try_parse_from(["teehee", "send", "127.0.0.1"])
            .expect("clap must accept positional HOST");
        let Command::Send(args) = cli.command else {
            panic!("expected Send subcommand");
        };
        assert_eq!(args.host.as_deref(), Some("127.0.0.1"));
        assert!(args.host_flag.is_none());
    }

    #[test]
    fn clap_parses_host_with_embedded_port() {
        let cli = Cli::try_parse_from(["teehee", "send", "--host", "10.0.0.5:6000", "--sine"])
            .expect("clap must accept --host with host:port embedded");
        let Command::Send(args) = cli.command else {
            panic!("expected Send subcommand");
        };
        let t = args.validate().expect("validate ok");
        assert_eq!(t.host, "10.0.0.5");
        assert_eq!(t.port, 6000);
    }

    #[test]
    fn clap_rejects_ambiguous_port_at_parse_time() {
        // conflicts_with on host_flag makes clap refuse up-front.
        let cli = Cli::try_parse_from(["teehee", "send", "127.0.0.1", "--host", "10.0.0.5"]);
        assert!(
            cli.is_err(),
            "clap must reject positional HOST + --host via conflicts_with"
        );
    }

    #[test]
    fn resolved_target_socket_string_formatting() {
        let t = ResolvedTarget {
            host: "10.0.0.5".into(),
            port: 6000,
        };
        assert_eq!(t.to_socket_string(), "10.0.0.5:6000");
    } // ----- Slice 8: CaptureSource enum -----
    #[test]
    fn capture_source_default_value_is_default() {
        let cli = Cli::try_parse_from(["teehee", "send", "--host", "10.0.0.5"])
            .expect("clap must accept default send args");
        let Command::Send(args) = cli.command else {
            panic!("expected Send subcommand");
        };
        assert_eq!(
            args.capture_source,
            CaptureSource::Default,
            "CaptureSource default must preserve v1 default-input behavior"
        );
    }

    #[test]
    fn capture_source_loopback_parses_cleanly() {
        let cli = Cli::try_parse_from([
            "teehee",
            "send",
            "--host",
            "10.0.0.5",
            "--capture-source",
            "loopback",
        ])
        .expect("clap must accept --capture-source loopback");
        let Command::Send(args) = cli.command else {
            panic!("expected Send subcommand");
        };
        assert_eq!(args.capture_source, CaptureSource::Loopback);
    }

    #[test]
    fn capture_source_loopback_default_keyword_rejected() {
        // clap ValueEnum derives values from variant names only — no
        // implicit aliases. Confirm that "default" is the only spelling
        // for CaptureSource::Default (so users can't accidentally type
        // "Default" with a capital D and silently fall back).
        let cli = Cli::try_parse_from([
            "teehee",
            "send",
            "--host",
            "10.0.0.5",
            "--capture-source",
            "Default",
        ]);
        assert!(
            cli.is_err(),
            "clap must reject 'Default' (capitalised variant name)"
        );
    }

    #[test]
    fn capture_source_unknown_value_rejected() {
        let cli = Cli::try_parse_from([
            "teehee",
            "send",
            "--host",
            "10.0.0.5",
            "--capture-source",
            "ffmpeg",
        ]);
        assert!(cli.is_err(), "clap must reject unknown capture sources");
    }

    #[test]
    fn capture_source_is_comparable_by_value() {
        // Pin Clone + Copy + PartialEq + Eq + Debug derives are
        // intact — needed because run_send compares
        // `args.capture_source == CaptureSource::Loopback` once at
        // startup, and the enum is stored by-value in SendArgs.
        let a = CaptureSource::Loopback;
        let b = CaptureSource::Loopback;
        assert_eq!(a, b, "Copy + PartialEq round-trip");
        let c = CaptureSource::Default;
        assert_ne!(a, c, "different variants must compare unequal");
    }

    // ----- Slice 11: CaptureSource::Auto -----

    #[test]
    fn capture_source_auto_parses_cleanly() {
        let cli = Cli::try_parse_from([
            "teehee",
            "send",
            "--host",
            "10.0.0.5",
            "--capture-source",
            "auto",
        ])
        .expect("clap must accept --capture-source auto");
        let Command::Send(args) = cli.command else {
            panic!("expected Send subcommand");
        };
        assert_eq!(
            args.capture_source,
            CaptureSource::Auto,
            "--capture-source auto parses to CaptureSource::Auto"
        );
    }

    #[test]
    fn capture_source_auto_capitalised_rejected() {
        // Clap ValueEnum derives values from variant names only — no
        // implicit aliases. Confirm "Auto" (capitalised) is rejected
        // so users on case-sensitive filesystems don't silently fall
        // back to the v1 default.
        let cli = Cli::try_parse_from([
            "teehee",
            "send",
            "--host",
            "10.0.0.5",
            "--capture-source",
            "Auto",
        ]);
        assert!(
            cli.is_err(),
            "clap must reject 'Auto' (capitalised variant name)"
        );
    }

    #[test]
    fn capture_source_auto_partial_alias_rejected() {
        // "automatic" or "auto-detect" are NOT valid aliases — the
        // operator must type exactly "auto" (clap ValueEnum does
        // not fuzzy-match). This pins the strict-value-enum semantics
        // so a future contributor doesn't add an alias and silently
        // change the binding surface.
        let cli_alias = Cli::try_parse_from([
            "teehee",
            "send",
            "--host",
            "10.0.0.5",
            "--capture-source",
            "automatic",
        ]);
        assert!(
            cli_alias.is_err(),
            "clap must reject 'automatic' alias for auto"
        );
        let cli_dash = Cli::try_parse_from([
            "teehee",
            "send",
            "--host",
            "10.0.0.5",
            "--capture-source",
            "auto-detect",
        ]);
        assert!(
            cli_dash.is_err(),
            "clap must reject 'auto-detect' partial alias"
        );
    }

    #[test]
    fn capture_source_default_remains_v1_back_compat() {
        // Slice 11 invariant: even though `--capture-source auto` is
        // available, the *omitted* `--capture-source` argument must
        // still resolve to `Default` (the v1 behaviour) so existing
        // user scripts / shell aliases don't suddenly start probing.
        // Pin both the explicit "default" value AND the omitted path.
        let cli_explicit = Cli::try_parse_from([
            "teehee",
            "send",
            "--host",
            "10.0.0.5",
            "--capture-source",
            "default",
        ])
        .expect("clap must accept --capture-source default");
        let Command::Send(args) = cli_explicit.command else {
            panic!("expected Send subcommand");
        };
        assert_eq!(args.capture_source, CaptureSource::Default);

        let cli_omitted = Cli::try_parse_from(["teehee", "send", "--host", "10.0.0.5"])
            .expect("clap must accept default send args");
        let Command::Send(args) = cli_omitted.command else {
            panic!("expected Send subcommand");
        };
        assert_eq!(
            args.capture_source,
            CaptureSource::Default,
            "omitted --capture-source must preserve the v1 default-input path"
        );
    }

    #[test]
    fn capture_source_auto_in_make_args_helper_roundtrip() {
        // Pin the unit-test make_args helper continues to take the
        // new Auto variant — the Slice-11 enum addition is visible
        // field-by-field in SendArgs construction, so a future
        // refactor that hides the field (e.g. a Box<EnumOfChoices>)
        // surfaces here.
        let mut args = make_args(Some("127.0.0.1"), None, 5000);
        args.capture_source = CaptureSource::Auto;
        let resolved = args.validate().expect("validate ok");
        assert_eq!(resolved.host, "127.0.0.1");
        assert_eq!(resolved.port, 5000);
        assert_eq!(
            args.capture_source,
            CaptureSource::Auto,
            "make_args must round-trip CaptureSource::Auto"
        );
    }

    // ----- Slice 11: --exact-capture-source strict-mode flag -----
    //
    // The strict-mode flag REJECTS --capture-source=auto at the
    // parse / validate layer. Operators on shared / fleet-managed
    // machines (where the default-input device is unpredictable)
    // opt in to disallow the auto probe so the silent-mic pitfall
    // is unreachable.
    //
    // The env-var counterpart TEEHEE_STRICT_LOOPBACK lives in
    // `audio_io::open_auto_input` and has a different semantics
    // (silent redirect to loopback); the unit tests for that live
    // in src/audio_io.rs::unit.

    #[test]
    fn exact_capture_source_with_auto_rejected_at_validate() {
        let mut args = make_args(Some("127.0.0.1"), None, 5000);
        args.exact_capture_source = true;
        args.capture_source = CaptureSource::Auto;
        let err = args
            .validate()
            .expect_err("validate must reject --exact-capture-source with --capture-source=auto");
        assert!(
            err.contains("--exact-capture-source"),
            "err must name the strict-mode flag; got: {err}"
        );
        assert!(
            err.contains("auto"),
            "err must name the rejected value; got: {err}"
        );
    }

    #[test]
    fn exact_capture_source_with_default_passes_validate() {
        let mut args = make_args(Some("127.0.0.1"), None, 5000);
        args.exact_capture_source = true;
        args.capture_source = CaptureSource::Default;
        let resolved = args
            .validate()
            .expect("--exact-capture-source + --capture-source=default must validate");
        assert_eq!(resolved.host, "127.0.0.1");
    }

    #[test]
    fn exact_capture_source_with_loopback_passes_validate() {
        let mut args = make_args(Some("127.0.0.1"), None, 5000);
        args.exact_capture_source = true;
        args.capture_source = CaptureSource::Loopback;
        let resolved = args
            .validate()
            .expect("--exact-capture-source + --capture-source=loopback must validate");
        assert_eq!(resolved.host, "127.0.0.1");
    }

    #[test]
    fn exact_capture_source_omitted_with_auto_passes_validate() {
        // V1 behaviour must be preserved: omitted --exact-capture-source
        // (default_value_t = false) and --capture-source=auto (or its
        // omit-default variant) must still validate cleanly so
        // existing user scripts / shell aliases don't suddenly
        // start failing.
        let mut args = make_args(Some("127.0.0.1"), None, 5000);
        args.exact_capture_source = false;
        args.capture_source = CaptureSource::Auto;
        let resolved = args.validate().expect(
            "omitted --exact-capture-source + --capture-source=auto must validate \
                 (v1 compat: strict-mode is opt-in)",
        );
        assert_eq!(resolved.host, "127.0.0.1");
    }

    #[test]
    fn exact_capture_source_clap_parses_cleanly() {
        let cli = Cli::try_parse_from([
            "teehee",
            "send",
            "--host",
            "10.0.0.5",
            "--exact-capture-source",
        ])
        .expect("clap must accept --exact-capture-source without other adjustments");
        let Command::Send(args) = cli.command else {
            panic!("expected Send subcommand");
        };
        assert!(
            args.exact_capture_source,
            "clap must set exact_capture_source from --exact-capture-source"
        );
        assert_eq!(
            args.capture_source,
            CaptureSource::Default,
            "capture_source must remain the v1 default (Default) under strict-mode only"
        );
    }

    #[test]
    fn exact_capture_source_clap_rejects_combination_at_parse_time() {
        // The combination --exact-capture-source + --capture-source=auto
        // is caught by SendArgs::validate (the parser can't catch it
        // because capture_source is a separate flag). When parsed,
        // validate must surface a clear error message.
        let cli = Cli::try_parse_from([
            "teehee",
            "send",
            "--host",
            "10.0.0.5",
            "--exact-capture-source",
            "--capture-source",
            "auto",
        ])
        .expect("clap parses height OK; validate-step rejects the combination");
        let Command::Send(args) = cli.command else {
            panic!("expected Send subcommand");
        };
        let err = args.validate().expect_err(
            "validate must reject --exact-capture-source + auto (caught late, not at parse)",
        );
        assert!(err.contains("--exact-capture-source"), "err: {err}");
    }

    // ----- Slice 9: --mtu flag -----

    #[test]
    fn mtu_default_is_default_lan_ethernet_1500() {
        // The default MTU matches typical Ethernet LAN (RFC 894
        // / RFC 791). The constant is exposed via mtu_budget
        // and used as default_value_t; pin both.
        assert_eq!(crate::mtu_budget::MTU_DEFAULT_BYTES, 1500);
        let cli = Cli::try_parse_from(["teehee", "send", "--host", "10.0.0.5"])
            .expect("clap must accept default send args");
        let Command::Send(args) = cli.command else {
            panic!("expected Send subcommand");
        };
        assert_eq!(
            args.mtu, 1500,
            "--mtu default must be MTU_DEFAULT_BYTES (1500)"
        );
    }

    #[test]
    fn mtu_explicit_ipv6_min_1280_passes_validation() {
        // 1280 is the IPv6 minimum link-MTU per RFC 8200; explicit
        // --mtu=1280 must round-trip.
        let cli = Cli::try_parse_from(["teehee", "send", "--host", "10.0.0.5", "--mtu", "1280"])
            .expect("clap must accept --mtu 1280");
        let Command::Send(args) = cli.command else {
            panic!("expected Send subcommand");
        };
        assert_eq!(args.mtu, 1280);
    }

    #[test]
    fn mtu_jumbo_9000_passes_validation() {
        let cli = Cli::try_parse_from(["teehee", "send", "--host", "10.0.0.5", "--mtu", "9000"])
            .expect("clap must accept --mtu 9000 (jumbo)");
        let Command::Send(args) = cli.command else {
            panic!("expected Send subcommand");
        };
        assert_eq!(args.mtu, 9000);
    }

    #[test]
    fn mtu_boundary_576_rfc_min_passes_validation() {
        // 576 is the IPv6 RFC-minimum (RFC 2460); must be accepted.
        let cli = Cli::try_parse_from(["teehee", "send", "--host", "10.0.0.5", "--mtu", "576"])
            .expect("clap must accept --mtu 576 (IPv6 RFC-min)");
        let Command::Send(args) = cli.command else {
            panic!("expected Send subcommand");
        };
        assert_eq!(args.mtu, 576);
    }

    #[test]
    fn mtu_below_min_575_rejected_at_parse_time() {
        let cli = Cli::try_parse_from(["teehee", "send", "--host", "10.0.0.5", "--mtu", "575"]);
        assert!(
            cli.is_err(),
            "clap must reject --mtu 575 (below IPv6 RFC-min 576)"
        );
    }

    #[test]
    fn mtu_above_max_9001_rejected_at_parse_time() {
        let cli = Cli::try_parse_from(["teehee", "send", "--host", "10.0.0.5", "--mtu", "9001"]);
        assert!(
            cli.is_err(),
            "clap must reject --mtu 9001 (above jumbo max 9000)"
        );
    }

    #[test]
    fn mtu_non_integer_rejected_at_parse_time() {
        let cli = Cli::try_parse_from([
            "teehee",
            "send",
            "--host",
            "10.0.0.5",
            "--mtu",
            "fivethousand",
        ]);
        assert!(cli.is_err(), "clap must reject non-integer --mtu");
    }

    // ----- Slice 10: --rx-buffer-ms flag + cross-flag validation -----

    #[test]
    fn rx_buffer_default_is_2_seconds() {
        // 2000 ms = 10× the default prebuffer-ms of 200. Pin this so
        // operators' grep muscle memory for the default value stays
        // reliable across refactors.
        assert_eq!(crate::buffer_budget::RX_BUFFER_DEFAULT_MS, 2_000);
    }

    #[test]
    fn clap_parses_rx_buffer_default_value() {
        let cli =
            Cli::try_parse_from(["teehee", "recv"]).expect("clap must accept default recv args");
        let Command::Recv(args) = cli.command else {
            panic!("expected Recv subcommand");
        };
        assert_eq!(
            args.rx_buffer_ms, 2_000,
            "--rx-buffer-ms default must be RX_BUFFER_DEFAULT_MS (2000)"
        );
        assert_eq!(args.prebuffer_ms, 200);
    }

    #[test]
    fn clap_parses_rx_buffer_explicit_5000() {
        let cli = Cli::try_parse_from(["teehee", "recv", "--rx-buffer-ms", "5000"])
            .expect("clap must accept --rx-buffer-ms 5000");
        let Command::Recv(args) = cli.command else {
            panic!("expected Recv subcommand");
        };
        assert_eq!(args.rx_buffer_ms, 5_000);
    }

    #[test]
    fn clap_rejects_rx_buffer_below_min_99() {
        let cli = Cli::try_parse_from(["teehee", "recv", "--rx-buffer-ms", "99"]);
        assert!(
            cli.is_err(),
            "clap must reject --rx-buffer-ms 99 (below min 100)"
        );
    }

    #[test]
    fn clap_rejects_rx_buffer_above_max_30001() {
        let cli = Cli::try_parse_from(["teehee", "recv", "--rx-buffer-ms", "30001"]);
        assert!(
            cli.is_err(),
            "clap must reject --rx-buffer-ms 30001 (above max 30000)"
        );
    }

    #[test]
    fn clap_accepts_rx_buffer_at_min_boundary_100() {
        let cli = Cli::try_parse_from(["teehee", "recv", "--rx-buffer-ms", "100"])
            .expect("clap must accept --rx-buffer-ms 100 (boundary)");
        let Command::Recv(args) = cli.command else {
            panic!("expected Recv subcommand");
        };
        assert_eq!(args.rx_buffer_ms, 100);
    }

    #[test]
    fn clap_accepts_rx_buffer_at_max_boundary_30000() {
        let cli = Cli::try_parse_from(["teehee", "recv", "--rx-buffer-ms", "30000"])
            .expect("clap must accept --rx-buffer-ms 30000 (boundary)");
        let Command::Recv(args) = cli.command else {
            panic!("expected Recv subcommand");
        };
        assert_eq!(args.rx_buffer_ms, 30_000);
    }

    #[test]
    fn validate_rejects_rx_buffer_smaller_than_prebuffer() {
        // 200 < 500 -- the gate target is unreachable.
        let res = make_recv_args(500, 200).validate();
        let err = res.unwrap_err();
        assert!(err.contains("rx-buffer-ms"), "err: {err}");
        assert!(err.contains("prebuffer-ms"), "err: {err}");
        assert!(err.contains("500"), "err: {err}");
        assert!(err.contains("200"), "err: {err}");
    }

    #[test]
    fn validate_accepts_rx_buffer_at_prebuffer_equality() {
        // rx_buffer_ms == prebuffer_ms -- invariant satisfied at the
        // boundary; this is the smallest ring that still meets the gate.
        let res = make_recv_args(500, 500).validate();
        assert!(res.is_ok(), "rx_buffer == prebuffer must be valid");
    }

    #[test]
    fn validate_accepts_rx_buffer_strictly_greater_than_prebuffer() {
        // Default config (rx=2000, pre=200) -- healthy.
        let res = make_recv_args(200, 2_000).validate();
        assert!(res.is_ok());
    }

    // ----- helpers for slice 10 tests -----

    /// Build a default RecvArgs so we can drive `RecvArgs::validate`
    /// without going through clap round-trip.
    fn make_recv_args(prebuffer_ms: usize, rx_buffer_ms: usize) -> RecvArgs {
        RecvArgs {
            port: 5000,
            prebuffer_ms,
            rx_buffer_ms,
            stats: false,
        }
    }
}
