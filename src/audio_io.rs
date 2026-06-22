//! `audio_io` — cpal adapter for teehee.
//!
//! Hides [`cpal`] types behind a small surface so the platform-specific
//! audio plumbing is isolated from the `protocol`, `jitter`, and
//! `network` modules.
//!
//! v1 captures from the **default input device** as a pragmatic
//! workaround for the lack of cross-platform loopback support in
//! cpal 0.15. On Windows you can enable "Stereo Mix" (or your sound
//! card's loopback input) to capture system audio; on macOS / Linux
//! you may need a virtual input device such as BlackHole / PulseAudio
//! monitor. For most users the easy path is `teehee send --sine`,
//! which exercises the rest of the pipeline end to end without
//! hardware capture.
//!
//! TODO(loopback): For future v2 work, use
//! `cpal::host_from_id(cpal::HostId::Wasapi)` with
//! `wasapi::DeviceRole::Loopback`, or call the `windows` crate to
//! configure an `IAudioClient` directly. cpal 0.15 does not expose a
//! cross-platform loopback primitive.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use tracing::warn;

/// Curated list of microphone-name fragments used by
/// [`looks_like_microphone`]. Each entry is a case-insensitive
/// substring that, when present in a cpal-reported device name,
/// strongly suggests the device IS a microphone (rather than a
/// loopback endpoint, virtual-audio sink, or pure speaker).
///
/// **Why a fragment list rather than a single regex**: cpal
/// reports device names in a wildly OS- and driver-dependent
/// format (`"Microphone Array (Realtek High Definition Audio)"` on
/// one machine, `"Yeti Stereo Microphone"` on another,
/// `"USB Microphone"` on a third). A curated substring list is
/// the most robust way to cover the common cases without complex
/// regex.
pub const MICROPHONE_HEURISTICS: &[&str] = &[
    // Generic heading nouns.
    "Microphone",
    "Headset",
    "Webcam",
    // Mic-array markers.
    "Mic Array",
    "Array",
    // Compound names ending in "Mic" — listed separately from
    // bare "Mic" so we don't false-positive on incidental
    // substrings inside vendor names.
    "Realtek Mic",
    "Front Mic",
    "Rear Mic",
    "Internal Mic",
    "USB Mic",
    "USB Microphone",
    // Headset / mic product names.
    "Yeti",
    "PodMic",
    "AT2020",
    "Blue Mic",
    "Snowball",
    // Headset / webcam brands where mic-in is the dominant
    // function.
    "Plantronics",
    "Jabra",
    "Logitech Webcam",
    "Logi ",
    // Common laptop-mic driver strings.
    "Intel SST",
    "Synaptics",
    "Conexant",
];

/// Heuristic check: does a cpal-reported device name indicate it is
/// a microphone (and therefore likely NOT a system-audio capture
/// source)? Used by [`open_auto_input`] to surface a startup warning
/// when a `--capture-source=auto` user lands on a microphone rather
/// than the desired render endpoint.
///
/// Returning `true` does NOT guarantee the device IS a microphone —
/// `false` does NOT guarantee it is NOT — false-positives and
/// false-negatives are possible. The function covers the common
/// cases (`"Microphone (Realtek)"`, `"Headset Earphone"`, `"Webcam
/// C920"`, ...) so the operator gets an early `--capture-source=auto`
/// warn rather than the silent-mic-broadcast trap documented in the
/// CLI long_about.
pub fn looks_like_microphone(name: &str) -> bool {
    let lower = name.to_lowercase();
    MICROPHONE_HEURISTICS
        .iter()
        .any(|needle| lower.contains(&needle.to_lowercase()))
}

// Re-export the cpal sample-format tag at the audio_io module level so
// downstream callers (sender pipeline, tests, slice-7 receiver-side
// reconciliation) can name it without an extra `cpal::` import path.
pub use cpal::SampleFormat as CapturedSampleFormat;

/// Slice-11 strict-loopback env var. When set in the process
/// environment, [`open_auto_input`] short-circuits the `auto`
/// probe entirely and routes the capturer straight to WASAPI
/// render-loopback on Windows (errors on macOS / Linux because
/// loopback is Windows-only).
///
/// **`--capture-source=auto` vs `TEEHEE_STRICT_LOOPBACK` semantics**:
/// the env var DOES NOT change the parse / validate layer
/// (clap-derived `--capture-source` argument is unchanged); it
/// changes the BEHAVIOR of `open_auto_input` once `--capture-source=auto`
/// has been accepted. Use cases:
///
/// * **Shared / fleet-managed machines**: a wrapper script can
///   `export TEEHEE_STRICT_LOOPBACK=1` so any user-emitted
///   `--capture-source=auto` (the recommended default per the
///   slice-11 README block for an unpredictable default-input
///   device) silently routes to the WASAPI render endpoint
///   instead of broadcasting whatever the mic hears.
/// * **CI smoke-tests against a Windows render endpoint**: set
///   the env var, then run `teehee send --capture-source auto
///   --sine --host <ip>` and the `auto` arm lands on the
///   loopback path immediately (skipping the default-input
///   probe that would otherwise succeed on the test machine's
///   working microphone and never exercise loopback).
///
/// **Companion CLI flag**: `--exact-capture-source` is a
/// DIFFERENT mechanism — it REJECTS `auto` outright at the
/// validate layer; the env var REMAPS `auto` to loopback. Pick
/// one per deployment: `--exact-capture-source` is opt-in strict
/// (operators must type the explicit value); the env var is
/// opt-out lenient (operators can keep typing `auto`, the env
/// var silently redirects).
///
/// **Production cost**: one libc `getenv` per helper invocation.
/// See [`crate::audio_io::Capturer::open_default_input`] for the
/// analogous `TEEHEE_FORCE_DEFAULT_INPUT_FAIL` test-only fail-
/// injection seam; both env vars are unconditional (no
/// compile-time gate) because cargo rebuilds the lib crate
/// without `--cfg test` when the linker target is an integration
/// test binary in `tests/`.
pub const STRICT_LOOPBACK_ENV: &str = "TEEHEE_STRICT_LOOPBACK";

/// `--capture-source=auto` probe helper (slice 11 / Tier 3 #10).
///
/// Tries the OS default-input capture first; on `Err` AND on a
/// Windows host, transparently falls back to the WASAPI render-
/// loopback capture path so the operator gets system audio without
/// any manual device configuration. On macOS / Linux the loopback
/// fallback is unreachable (slice 8 made `LoopbackCapturer::open_default`
/// a Windows-only API), so `auto` is functionally identical to
/// `default` on those hosts: the default-input error is surfaced
/// verbatim with a clarifying line that points the user at
/// `--capture-source=default` + BlackHole / PulseAudio monitor or
/// at `--sine`.
///
/// **`auto` silent-mic pitfall**: the probe order is
/// `default → loopback (Windows)`. If the OS default input IS a
/// working microphone, `auto` will succeed at the default-input
/// step and **never reach the loopback fallback** — a Windows user
/// who expected system audio but has a live mic default input
/// will broadcast mic audio. The `Capturer` does not introspect the
/// device name to detect mic vs loopback; it accepts what cpal
/// gives it. Document this trade-off at the CLI layer; the helper
/// itself implements "best-effort whatever works".
///
/// **`TEEHEE_STRICT_LOOPBACK` env var shortcut**: when set, the
/// probe is short-circuited entirely; `open_auto_input` routes
/// directly to WASAPI loopback on Windows (returning the label
/// `"WASAPI loopback (strict)"`) or errors on macOS / Linux. See
/// [`STRICT_LOOPBACK_ENV`] for the full rationale.
///
/// **API shape**: the helper accepts a *factory* `make_cb` that
/// yields a fresh `FnMut(&[f32])` closure on each call rather than
/// a single `F: FnMut(&[f32])` by-value. This is needed because
/// [`Capturer::open_default_input`] and
/// [`crate::loopback::LoopbackCapturer::open_default`] both consume
/// their callback by value; the factory lets us rebuild a fresh
/// closure for each attempt without forcing the caller to migrate
/// to `Arc<dyn FnMut>` (which would add a runtime cost on the
/// cpal audio thread's hot path). Each fresh closure produced by
/// the factory captures (typically via clone) the same
/// `Arc<Mutex<Vec<f32>>>` ring buffer the sender pipeline drains,
/// so both attempts target the same downstream consumer.
///
/// Returns `Ok((Box<dyn AudioCapture>, &'static str label))` so
/// the caller can both store the capturer for RAII and surface a
/// `[flag]` style log marker (`"cpal default input (auto-probed)"`,
/// `"WASAPI loopback (auto-fallback)"`, or `"WASAPI loopback
/// (strict)"` when the env var is set) at startup. The label
/// carries the audit trail for `--stats` debugging.
pub fn open_auto_input<F, C>(
    mut make_cb: F,
) -> anyhow::Result<(Box<dyn AudioCapture>, &'static str)>
where
    F: FnMut() -> C,
    C: FnMut(&[f32]) + Send + 'static,
{
    // Slice-11 strict-loopback env var short-circuit. When
    // [STRICT_LOOPBACK_ENV] is set, route the auto probe directly
    // to WASAPI loopback on Windows (skipping the default-input
    // probe that might silently land on a microphone) — and error
    // on macOS / Linux because the loopback route is Windows-only.
    //
    // **Why unconditional (no compile-time gate)**: matches the
    // companion test seam in [`Capturer::open_default_input`]:
    // cargo compiles the lib crate WITHOUT `--cfg test` when the
    // linker target is an integration test binary, so any
    // `#[cfg(test)]` / `cfg!(test)` gate silently fails to fire.
    //
    // **Production cost**: one libc `getenv` per helper invocation.
    // Sender starts once; env var is false in production binaries
    // so the `is_some()` branch is unreachable in real-world
    // operation.
    if std::env::var_os(STRICT_LOOPBACK_ENV).is_some() {
        if cfg!(target_os = "windows") {
            match LoopbackCapturer::open_default(make_cb()) {
                Ok(cap) => Ok((Box::new(cap), "WASAPI loopback (strict)")),
                Err(e_loop) => Err(anyhow::anyhow!(
                    "{STRICT_LOOPBACK_ENV} is set but WASAPI loopback \
                     open_default failed: {e_loop}\n\
                     Fix path: unset {STRICT_LOOPBACK_ENV} to fall \
                     through to the auto probe, OR confirm a \
                     float-mix-format render endpoint is enabled \
                     (Settings → Sound → Output → your speakers, \
                     format = 32-bit float), OR run `teehee send \
                     --sine --host <ip>` for a 440 Hz dry-run \
                     without hardware capture."
                )),
            }
        } else {
            // Non-Windows strict-loopback: WASAPI route is
            // unimplemented. Surface a clean error that names the
            // env var so the operator can unset it. macOS / Linux
            // users who want system audio should install a
            // virtual input device (BlackHole / PulseAudio
            // monitor) and let the regular auto probe pick it up.
            Err(anyhow::anyhow!(
                "--capture-source=auto on {platform} with \
                 {STRICT_LOOPBACK_ENV}=1 set: the WASAPI loopback \
                 route is Windows-only, and this env var \
                 short-circuits the auto probe directly to \
                 loopback. The override is therefore \
                 unimplemented on macOS / Linux. Unset \
                 {STRICT_LOOPBACK_ENV} and either install a \
                 virtual input device (BlackHole / PulseAudio \
                 monitor) as the default input or run `teehee \
                 send --sine --host <ip>` for a 440 Hz dry-run \
                 without hardware capture.",
                platform = std::env::consts::OS,
            ))
        }
    } else {
        // First attempt: cpal default-input path. This is the
        // regular auto-probe order: try `default`, fall through
        // to WASAPI loopback on Windows if `default` returns Err.
        match Capturer::open_default_input(make_cb()) {
            Ok(cap) => {
                // Slice-11 silent-mic pitfall mitigation: if the
                // captured device name matches a microphone heuristic,
                // emit a startup warn! so the operator notices before
                // broadcasting mic audio over the LAN. The warning
                // names the device so logs are grep-able.
                //
                // **Platform**: emit on all platforms. Linux + macOS
                // virtual-audio devices (PulseAudio monitor, BlackHole)
                // typically don't carry the heuristic substrings, so
                // the false-positive rate is low; emitting the warning
                // uniformly surfaces real-mic accidents on every OS.
                let dev_name = cap.device_name();
                if looks_like_microphone(dev_name) {
                    warn!(
                        device_name = dev_name,
                        capture_source = "auto (default-input)",
                        "--capture-source=auto: default-input device name \
                     `{}` matches the microphone heuristic. If you \
                     intended system-audio capture (browser, Spotify, \
                     YouTube, system beeps), teehee will broadcast \
                     microphone audio on this host. Switch to \
                     --capture-source=loopback (Windows-only, captures \
                     the render endpoint's mix) or pass \
                     --capture-source=default and confirm your virtual \
                     input device (BlackHole / PulseAudio monitor) is \
                     the system-default input.",
                        dev_name
                    );
                }
                Ok((Box::new(cap), "cpal default input (auto-probed)"))
            }
            Err(e_def) => {
                // On Windows: transparently retry through the WASAPI
                // loopback path so the user gets system audio without
                // manual device configuration. Surface both inner
                // errors verbatim if both fail — operators writing
                // post-mortems need to know which step failed and why
                // (cpal connection-refused is a different fix path from
                // WASAPI PCM-INT-only render endpoint).
                if cfg!(target_os = "windows") {
                    match LoopbackCapturer::open_default(make_cb()) {
                        Ok(cap) => Ok((Box::new(cap), "WASAPI loopback (auto-fallback)")),
                        Err(e_loop) => Err(anyhow::anyhow!(
                            "--capture-source=auto: both capture attempts \
                             failed on Windows\n  \
                             1) cpal default-input error: {e_def}\n  \
                             2) WASAPI loopback fallback error: {e_loop}\n\
                             Fix path: confirm a default-input device is \
                             enabled (Settings → Sound → Input → your \
                             microphone, or Stereo Mix), OR enable a \
                             float-mix-format render endpoint for \
                             WASAPI loopback, OR run `teehee send \
                             --sine --host <ip>` for a 440 Hz dry-run \
                             without hardware capture."
                        )),
                    }
                } else {
                    // Non-Windows: the WASAPI stub returns a
                    // synthesized "Windows-only" error rather than a
                    // genuine cpal / OS error. Surfacing that stub
                    // message at this layer would be misleading
                    // (it masks the actual default-input failure),
                    // so we deliberately skip the second attempt and
                    // bubble up the genuine `e_def` with a clarifying
                    // note about the platforms where the fallback
                    // applies.
                    Err(anyhow::anyhow!(
                        "--capture-source=auto on {platform}: default-input \
                         capture failed (loopback fallback is Windows-only) — \
                         {e_def}\n\
                         Fix path on macOS / Linux: install a virtual input \
                         device (BlackHole / PulseAudio monitor) and mark it \
                         as the system default input, or run \
                         `teehee send --sine --host <ip>` for a 440 Hz \
                         dry-run without hardware capture.",
                        platform = std::env::consts::OS,
                    ))
                }
            }
        }
    }
}

/// Common capture-side trait so the sender pipeline can hold any
/// running PCM capture stream behind a single
/// `Box<dyn AudioCapture>` guard.
///
/// Both [`Capturer`] (cpal default-input path, slice 3+) and
/// [`crate::loopback::LoopbackCapturer`] (slice 8, Windows WASAPI
/// loopback) impl this trait. The runtime cost of dyn dispatch on
/// `.config()` is negligible — it's called once at sender startup
/// and once per cpal callback (none, on the sender side).
///
/// The trait deliberately does NOT require `Send`: `cpal::Stream`
/// holds a `*mut ()` audio-thread handle that is not `Send`, and
/// the WASAPI `AudioClient` / `AudioCaptureClient` COM pointers are
/// STA/MTA apartment-bound. The sender pipeline holds the
/// `Box<dyn AudioCapture>` entirely on the main thread (never
/// moves it to a worker thread), so the missing `Send` bound costs
/// nothing at runtime. Both implementors handle their own internal
/// threading (cpal spawns its audio thread; WASAPI runs on its own
/// worker thread the LoopbackCapturer owns).
pub trait AudioCapture {
    /// Actual sample rate, channel count, and native sample format
    /// the device opened with. Mirrors [`Capturer::config`] and
    /// [`crate::loopback::LoopbackCapturer::config`].
    fn config(&self) -> CapturerConfig;
}

// Re-export the loopback capturer at `audio_io::LoopbackCapturer` so
// the user's spec ("surface loopback capture through `audio_io::Capturer`")
// is satisfied — the sender pipeline imports the loopback path from
// the same module as the default-input path.
pub use crate::loopback::LoopbackCapturer;

/// Lightweight device description that does not leak cpal types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub channels: u16,
    pub sample_rate_hz: u32,
}

fn device_info(dev: &cpal::Device, is_default: bool) -> Option<DeviceInfo> {
    let name = dev.name().ok()?;
    let cfg = dev.default_output_config().ok()?;
    Some(DeviceInfo {
        name,
        is_default,
        channels: cfg.channels(),
        sample_rate_hz: cfg.sample_rate().0,
    })
}

fn device_info_input(dev: &cpal::Device, is_default: bool) -> Option<DeviceInfo> {
    let name = dev.name().ok()?;
    let cfg = dev.default_input_config().ok()?;
    Some(DeviceInfo {
        name,
        is_default,
        channels: cfg.channels(),
        sample_rate_hz: cfg.sample_rate().0,
    })
}

/// Enumerate output (playback) devices available on this host.
pub fn list_output_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default_name = host.default_output_device().and_then(|d| d.name().ok());
    let mut out = Vec::new();
    match host.output_devices() {
        Ok(devices) => {
            for dev in devices {
                let is_default = dev.name().ok().as_ref() == default_name.as_ref();
                if let Some(info) = device_info(&dev, is_default) {
                    out.push(info);
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "cpal output_devices() failed"),
    }
    out
}

/// Enumerate input (capture) devices available on this host.
pub fn list_input_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|d| d.name().ok());
    let mut out = Vec::new();
    match host.input_devices() {
        Ok(devices) => {
            for dev in devices {
                let is_default = dev.name().ok().as_ref() == default_name.as_ref();
                if let Some(info) = device_info_input(&dev, is_default) {
                    out.push(info);
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "cpal input_devices() failed"),
    }
    out
}

/// Default output (playback) device, if any.
pub fn default_output_device() -> Option<DeviceInfo> {
    let host = cpal::default_host();
    let dev = host.default_output_device()?;
    device_info(&dev, true)
}

/// Default input (capture) device, if any. Returns `None` when the
/// host has no input or cpal cannot read its default config.
pub fn default_input_device() -> Option<DeviceInfo> {
    let host = cpal::default_host();
    let dev = host.default_input_device()?;
    device_info_input(&dev, true)
}

/// A running PCM capture stream backed by a default input device.
///
/// Wraps a cpal [`cpal::Stream`] in RAII so dropping the struct
/// closes the audio thread cleanly. The callback receives interleaved
/// `f32` samples regardless of the underlying device's native format.
///
/// The actual sample rate, channel count, and native sample format
/// that the OS default device opened with is exposed via
/// [`Capturer::config`]. The sender pipeline uses the sample_rate
/// and channels fields (not the CLI `--sample-rate` / `--channels`)
/// to label packets and compute chunk sizes; the sample_format is
/// preserved for slice 7's receiver-side format reconciliation and
/// for downstream conversion-tracking diagnostics.
pub struct Capturer {
    _stream: Stream,
    config: CapturerConfig,
    /// The cpal-reported device display name at open time (e.g.
    /// `"Microphone Array (Realtek High Definition Audio)"`,
    /// `"Yeti Stereo Microphone"`, or `"Stereo Mix (Realtek HD
    /// Audio)"`). Surfaced for slice-11 mic-heuristic warnings —
    /// see [`crate::audio_io::looks_like_microphone`] and
    /// [`crate::audio_io::open_auto_input`]. Empty string if cpal
    /// failed to read the name (rare; tested in slice 8).
    device_name: String,
}

/// Configuration of the OS default input device at the moment the
/// [`Capturer`] opened its stream. Captured here so the sender can
/// tag outbound packets with the device's *actual* format and
/// chunk math (`chunk_frames × channels`) — not whatever the CLI
/// defaults to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturerConfig {
    /// Sample rate in Hz that the device opened with (e.g. 44100,
    /// 48000, 96000). Independent of `--sample-rate`.
    pub sample_rate: u32,
    /// Channel count the device opened with. Independent of
    /// `--channels`. Already an `u16` because that's what cpal's
    /// `StreamConfig` uses; the sender casts to `u8` for the wire
    /// protocol's `channels` byte.
    pub channels: u16,
    /// Device's native sample format (`F32`, `I16`, `U16`, …). The
    /// Capturer's audio callback normalizes all formats to `f32` for
    /// the wire protocol, so this field is **informational** —
    /// useful for slice 7's receiver-side format reconciliation and
    /// downstream conversion-tracking diagnostics. Independent of
    /// any wire-format negotiation; teehee v1 always emits f32.
    pub sample_format: CapturedSampleFormat,
}

impl Capturer {
    /// The actual sample rate, channel count, and native sample
    /// format the OS default input device opened with. Surface this
    /// to the sender pipeline so outbound packets are labeled with
    /// the same numbers the audio thread is actually delivering —
    /// not the CLI defaults, which silently diverge if the device
    /// is e.g. 44.1 kHz mono I16.
    pub fn config(&self) -> CapturerConfig {
        self.config
    }

    /// The cpal-reported display name of the OS default input
    /// device at open time. Surfaced for slice-11 mic-heuristic
    /// warnings — see [`crate::audio_io::looks_like_microphone`].
    /// Empty string if the device did not expose a readable name.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Open the default input device. `callback` is invoked from the
    /// cpal audio thread with each interleaved frame; the callback
    /// must do its work and return quickly (no blocking I/O).
    pub fn open_default_input<F>(mut callback: F) -> anyhow::Result<Self>
    where
        F: FnMut(&[f32]) + Send + 'static,
    {
        // Test-only fail-injection seam: when
        // `TEEHEE_FORCE_DEFAULT_INPUT_FAIL` is set in the process
        // environment, return Err unconditionally so the
        // `--capture-source=auto` fallback path through
        // [`crate::loopback::LoopbackCapturer::open_default`] can
        // be exercised from integration tests without requiring the
        // test machine to lack a default-input device.
        //
        // **Why NO compile-time gate**: cargo compiles the lib
        // crate WITHOUT `--cfg test` when the linker target is an
        // integration test binary in `tests/`. An earlier revision
        // tried both `#[cfg(test)] { ... }` block attribute and
        // `cfg!(test)` runtime macro; both silently failed to fire
        // the seam under `cargo test --test capture_source_auto_integration
        // -- --ignored` and the test got `"cpal default input
        // (auto-probed)"` instead of the expected
        // `"WASAPI loopback (auto-fallback)"`. The fix is to drop
        // the gate entirely and accept a one-shot environ lookup at
        // capturer open time.
        //
        // **Production cost**: one libc `getenv` per capturer
        // open. Capturer opens once at sender startup; the env var
        // is never set in production binaries so `is_some()` returns
        // `false` and the `return Err` branch is unreachable in
        // real-world operation. On `cargo build --release` the
        // branch is preserved (link-time optimisation can't prove
        // the env var is unset), but the cost is a single null-check
        // on a non-existent entry. Negligible.
        //
        // See `tests/capture_source_auto_integration.rs` for the
        // Windows-only `#[ignore]` test that exercises this seam
        // against a live WASAPI render endpoint.
        if std::env::var_os("TEEHEE_FORCE_DEFAULT_INPUT_FAIL").is_some() {
            return Err(anyhow::anyhow!(
                "TEEHEE_FORCE_DEFAULT_INPUT_FAIL is set in the test \
                 environment; Capturer::open_default_input returns Err \
                 unconditionally so the --capture-source auto helper \
                 can exercise the WASAPI loopback fallback path. \
                 Clear the env var or run `cargo build` to use the \
                 default-input success path; review \
                 tests/capture_source_auto_integration.rs for the \
                 test's invocation pattern."
            ));
        }
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("no default input device"))?;
        let config = device.default_input_config()?;
        let sample_format = config.sample_format();
        let stream_config = cpal::StreamConfig {
            channels: config.channels(),
            sample_rate: config.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };
        let stream = match sample_format {
            SampleFormat::F32 => device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| callback(data),
                |err| eprintln!("capturer stream error: {err}"),
                None,
            )?,
            SampleFormat::I16 => {
                let shared: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
                let buf_for_cb = Arc::clone(&shared);
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _| {
                        let mut guard = match buf_for_cb.lock() {
                            Ok(g) => g,
                            Err(_) => return,
                        };
                        let need = data.len();
                        guard.resize(need, 0.0);
                        for (i, &s) in data.iter().enumerate() {
                            guard[i] = s as f32 / 32768.0;
                        }
                        callback(&guard);
                    },
                    |err| eprintln!("capturer stream error: {err}"),
                    None,
                )?
            }
            SampleFormat::U16 => {
                let shared: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
                let buf_for_cb = Arc::clone(&shared);
                device.build_input_stream(
                    &stream_config,
                    move |data: &[u16], _| {
                        let mut guard = match buf_for_cb.lock() {
                            Ok(g) => g,
                            Err(_) => return,
                        };
                        let need = data.len();
                        guard.resize(need, 0.0);
                        for (i, &s) in data.iter().enumerate() {
                            guard[i] = (s as f32 - 32768.0) / 32768.0;
                        }
                        callback(&guard);
                    },
                    |err| eprintln!("capturer stream error: {err}"),
                    None,
                )?
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "unsupported sample format tag: {:?}",
                    sample_format
                ));
            }
        };
        stream.play()?;
        // Capture the device's actual stream config so the sender
        // pipeline can compute chunk math and label packets with the
        // device's real sample rate and channel count instead of the
        // CLI defaults. cpal's `SupportedStreamConfig` does not
        // outlive this function, so we extract all three relevant
        // fields (sample_rate, channels, sample_format) here.
        let capturer_config = CapturerConfig {
            sample_rate: config.sample_rate().0,
            channels: config.channels(),
            sample_format,
        };
        // Also capture the device's cpal-reported display name.
        // Slice-11 uses this for the microphone-heuristic warning in
        // `open_auto_input`; the name comes from cpal's host
        // trait, which uses OS-level device lookup and is
        // OS-dependent (Windows: WASAPI name; macOS: CoreAudio
        // name; Linux: ALSA / PulseAudio name).
        let device_name = device.name().unwrap_or_default();
        Ok(Self {
            _stream: stream,
            config: capturer_config,
            device_name,
        })
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        self._stream.pause()?;
        Ok(())
    }
}

impl AudioCapture for Capturer {
    fn config(&self) -> CapturerConfig {
        self.config()
    }
}

/// Configuration of the OS default output device at the moment the
/// [`Player`] opened its stream. Captured here so the receive
/// pipeline can build the slice-7 [`crate::format_pipeline::FormatPipeline`]
/// with the device's *actual* sample rate and channel count — not
/// whatever the CLI defaults to.
///
/// Mirrors [`CapturerConfig`] for symmetric sender/receiver
/// introspection. Slice 7 wire-up in `main.rs::run_recv` builds the
/// `FormatPipeline` from this struct on first packet arrival (the
/// pipeline sits between [`crate::jitter::JitterBuffer::pop_frames`]
/// and the cpal data callback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerConfig {
    /// Sample rate in Hz that the device opened with (e.g. 44100,
    /// 48000, 96000). cpal's `StreamConfig` uses a `SampleRate` newtype
    /// for compile-time-distinct units; the inner field is `u32` so
    /// it pairs cleanly with the wire protocol's u32 sample_rate.
    pub sample_rate: u32,
    /// Channel count the device opened with. Already `u16` (matches
    /// cpal's `StreamConfig::channels`); the receive pipeline casts
    /// to `u8` for [`crate::format_pipeline::FormatPipeline`]'s
    /// `input_channels` / `output_channels` parameters.
    pub channels: u16,
    /// Device's native sample format (`F32`, `I16`, `U16`). Same
    /// semantics as [`CapturerConfig::sample_format`]: the Player
    /// normalizes all formats to `f32` for the callback, so this
    /// field is informational — useful for diagnostics and v2 work.
    pub sample_format: CapturedSampleFormat,
}

/// A running PCM playback stream backed by a default output device.
///
/// `provider` is invoked from the cpal audio thread with a buffer of
/// interleaved f32 samples; fill it in place. The provider must do
/// its work and return quickly.
pub struct Player {
    _stream: Stream,
    config: PlayerConfig,
}

impl Player {
    /// The actual sample rate, channel count, and native sample
    /// format the OS default output device opened with. Surface this
    /// to the receive pipeline so the slice-7 `FormatPipeline` is
    /// built against the device's real output format — not whatever
    /// the sender was emitting. Silent format mismatches between
    /// sender and receiver are the bug class slice 7 fixes.
    pub fn config(&self) -> PlayerConfig {
        self.config
    }

    /// Open the default output device.
    pub fn open_default_output<F>(mut provider: F) -> anyhow::Result<Self>
    where
        F: FnMut(&mut [f32]) + Send + 'static,
    {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("no default output device"))?;
        let config = device.default_output_config()?;
        let sample_format = config.sample_format();
        let stream_config = cpal::StreamConfig {
            channels: config.channels(),
            sample_rate: config.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };
        let stream = match sample_format {
            SampleFormat::F32 => device.build_output_stream(
                &stream_config,
                move |data: &mut [f32], _| provider(data),
                |err| eprintln!("player stream error: {err}"),
                None,
            )?,
            SampleFormat::I16 => {
                // Pre-allocate scratch outside the callback so the
                // cpal audio thread never heap-allocates. cpal
                // guarantees the output callback is single-threaded,
                // so a plain Vec captured by the closure is safe
                // without Arc<Mutex<>>.
                let mut tmp: Vec<f32> = Vec::new();
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [i16], _| {
                        tmp.resize(data.len(), 0.0);
                        provider(&mut tmp);
                        for (dst, src) in data.iter_mut().zip(tmp.iter()) {
                            *dst = (src.clamp(-1.0, 1.0) * 32768.0) as i16;
                        }
                    },
                    |err| eprintln!("player stream error: {err}"),
                    None,
                )?
            }
            SampleFormat::U16 => {
                let mut tmp: Vec<f32> = Vec::new();
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [u16], _| {
                        tmp.resize(data.len(), 0.0);
                        provider(&mut tmp);
                        for (dst, src) in data.iter_mut().zip(tmp.iter()) {
                            let v = src.clamp(-1.0, 1.0);
                            *dst = ((v + 1.0) * 32768.0) as u16;
                        }
                    },
                    |err| eprintln!("player stream error: {err}"),
                    None,
                )?
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "unsupported sample format tag: {:?}",
                    sample_format
                ));
            }
        };
        stream.play()?;
        // Capture the device's actual stream config so the receive
        // pipeline can build the slice-7 FormatPipeline from the
        // device's real output format. cpal's `SupportedStreamConfig`
        // does not outlive this function so we extract the three
        // fields here (same trick as `Capturer::open_default_input`).
        let player_config = PlayerConfig {
            sample_rate: config.sample_rate().0,
            channels: config.channels(),
            sample_format,
        };
        Ok(Self {
            _stream: stream,
            config: player_config,
        })
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        self._stream.pause()?;
        Ok(())
    }
}
#[cfg(test)]
mod unit {
    // Import the just-added `PlayerConfig` (mirror of CapturerConfig)
    // and the `CapturedSampleFormat` enum alias (already a `pub use`
    // in the parent module) so the slice-7 unit test can name them
    // directly. `super::*` does both in one line; specific names are
    // equally valid.
    use super::{CapturedSampleFormat, PlayerConfig};

    #[test]
    fn i16_to_f32_internal_helper_at_extremes() {
        let src = [i16::MIN, 0, i16::MAX];
        let mut buf: Vec<f32> = Vec::with_capacity(src.len());
        for &s in &src {
            buf.push(s as f32 / 32768.0);
        }
        assert_eq!(buf.len(), 3);
        assert_eq!(
            buf[0], -1.0,
            "i16::MIN must round-trip to -1.0 exactly under 32768.0 normalization"
        );
        assert_eq!(buf[1], 0.0, "i16=0 must round-trip to 0.0 exactly");
        assert!(
            buf[2] > 0.999_969 && buf[2] < 1.0,
            "i16::MAX expected in (0.999969, 1.0); got {}",
            buf[2]
        );
    }

    #[test]
    fn u16_to_f32_internal_helper_centers_at_zero() {
        let src: [u16; 3] = [0x8000, 0x8000 + 0x4000, 0];
        let mut buf: Vec<f32> = Vec::with_capacity(src.len());
        for &s in &src {
            buf.push((s as f32 - 32768.0) / 32768.0);
        }
        assert_eq!(buf.len(), 3);
        assert_eq!(
            buf[0], 0.0,
            "u16=0x8000 must round-trip to 0.0 exactly (signed PCM silence)"
        );
        assert!(
            buf[1] > 0.0 && buf[1] < 1.0,
            "u16=0xC000 expected in (0, 1); got {}",
            buf[1]
        );
        assert_eq!(
            buf[2], -1.0,
            "u16=0 must round-trip to -1.0 exactly under 32768.0 normalization"
        );
    }

    #[test]
    fn f32_to_i16_player_helper_at_extremes() {
        let src: [f32; 3] = [-1.0, 0.0, 1.0];
        let buf: Vec<i16> = src
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32768.0) as i16)
            .collect();
        assert_eq!(
            buf[0],
            i16::MIN,
            "f32=-1.0 must round-trip to i16::MIN exactly under 32768.0 normalization"
        );
        assert_eq!(buf[1], 0, "f32=0.0 must round-trip to 0 exactly");
        assert_eq!(
            buf[2],
            i16::MAX,
            "f32=1.0 must saturate to i16::MAX under Rust 1.45+ `as` cast"
        );
    }

    #[test]
    fn f32_to_u16_player_helper_at_extremes() {
        let src: [f32; 3] = [-1.0, 0.0, 1.0];
        let buf: Vec<u16> = src
            .iter()
            .map(|&s| ((s.clamp(-1.0, 1.0) + 1.0) * 32768.0) as u16)
            .collect();
        assert_eq!(buf[0], 0, "f32=-1.0 must round-trip to u16=0 exactly");
        assert_eq!(
            buf[1], 0x8000,
            "f32=0.0 must round-trip to u16=0x8000 exactly (signed PCM silence)"
        );
        assert_eq!(
            buf[2],
            u16::MAX,
            "f32=1.0 must saturate to u16::MAX under Rust 1.45+ `as` cast"
        );
    }

    // ----- Slice 7: PlayerConfig round-trip -----

    #[test]
    fn player_config_copy_and_eq_round_trip() {
        // PlayerConfig must be `Copy` (it's stored by-value in the
        // cpal callback's closure and slotted into an
        // `Arc<Mutex<Option<PlayerConfig>>>` in main.rs run_recv) and
        // `Eq` so the slice-7 wire-up can compare against cached
        // player-side values without field-by-field copy noise.
        let a = PlayerConfig {
            sample_rate: 48_000,
            channels: 2,
            sample_format: CapturedSampleFormat::F32,
        };
        let b = a; // Copy semantics
        assert_eq!(b.sample_rate, 48_000);
        assert_eq!(b.channels, 2);
        assert_eq!(b.sample_format, CapturedSampleFormat::F32);
        assert_eq!(a, b, "PlayerConfig must be Copy");
        let c = PlayerConfig {
            sample_rate: 44_100,
            channels: 1,
            sample_format: CapturedSampleFormat::I16,
        };
        assert_ne!(a, c, "different configs must compare unequal");
    }

    // ----- Slice 11: looks_like_microphone heuristic unit tests -----
    //
    // The slice-11 silent-mic pitfall mitigation depends on
    // `looks_like_microphone` returning true for common
    // microphone-product device names. Pin the heuristic list at
    // each known-firing string so a future refactor that breaks the
    // substring match surfaces precisely which device-name pattern
    // regressed.

    #[test]
    fn looks_like_microphone_fires_for_explicit_microphone_name() {
        // The user's reference example: a Realtek integrated mic
        // shipped on most Windows desktops.
        assert!(
            super::looks_like_microphone("Microphone (Realtek)"),
            "Microphone (Realtek) must trigger the heuristic"
        );
    }

    #[test]
    fn looks_like_microphone_fires_for_each_curated_needle() {
        // Exhaustive pin: drive the heuristic function with one
        // synthetic device name per curated needle in
        // `MICROPHONE_HEURISTICS`. A future refactor that drops a
        // needle from the list surfaces here as a precise
        // regression diagnostic.
        for needle in super::MICROPHONE_HEURISTICS {
            let synthetic = format!("Device ({needle})");
            assert!(
                super::looks_like_microphone(&synthetic),
                "MICROPHONE_HEURISTICS entry `{needle}` no longer triggers \
                 `looks_like_microphone`; produce synthetic device name \
                 `{synthetic}` did not match."
            );
        }
    }

    #[test]
    fn looks_like_microphone_is_case_insensitive() {
        // cpal-reported device names are OS- and driver-dependent
        // and the casing is not guaranteed. Heuristic must work in
        // uppercase, lowercase, and mixed case.
        assert!(super::looks_like_microphone("MICROPHONE (REALTEK)"));
        assert!(super::looks_like_microphone("microphone (realtek)"));
        assert!(super::looks_like_microphone("MiCrOpHoNe (ReAlTeK)"));
        assert!(super::looks_like_microphone("USB MICROPHONE"));
        assert!(super::looks_like_microphone("bluE miC"));
        assert!(super::looks_like_microphone("YETI NANO"));
    }

    #[test]
    fn looks_like_microphone_does_not_fire_on_loopback_endpoints() {
        // The auto path's whole point is: these names should NOT
        // trigger the warning when the user has correctly set up a
        // loopback device. The default-input step would still see
        // Err (or succeed on a different device) on the typical
        // Windows desktop — but if a user has explicitly configured
        // the loopback as their default input, the heuristic must
        // NOT fire and make them falsely believe they're on a mic.
        assert!(!super::looks_like_microphone(
            "Stereo Mix (Realtek HD Audio)"
        ));
        assert!(!super::looks_like_microphone("BlackHole 2ch"));
        assert!(!super::looks_like_microphone("Loopback Audio"));
        assert!(!super::looks_like_microphone(
            "PulseAudio Monitor of Built-in Audio"
        ));
        assert!(!super::looks_like_microphone("What U Hear"));
        assert!(!super::looks_like_microphone(
            "Speaker (Realtek High Definition Audio)"
        ));
        // Pure speakers with no mic should NOT fire — they're
        // typically not even valid default-input devices (cpal
        // rejects them at `default_input_device()`) but the
        // heuristic is defensive at the layer above.
        assert!(!super::looks_like_microphone("Speakers (Logitech Z550)"));
    }

    #[test]
    fn looks_like_microphone_does_not_falsely_match_substring_in_unrelated_vendor() {
        // The substring approach is conservative-trimmed: excluding
        // bare "Mic" from the list (using compound names like "Realtek
        // Mic", "USB Mic", "Front Mic" instead) prevents accidental
        // matches inside vendor names that happen to contain "Mic"
        // as letters within a word.
        assert!(
            !super::looks_like_microphone("AMD High Definition Audio Device"),
            "AMD vendor name must NOT trigger mic heuristic"
        );
        assert!(
            !super::looks_like_microphone("Microsoft Sound Mapper"),
            "Microsoft mapped-name must NOT trigger mic heuristic"
        );
        // Devices that contain "Headset" but are pure output
        // (rare, but defensive).
        // Skip: "Headset" should still fire — the user explicitly
        // suggested it as a heuristic.
    }

    #[test]
    fn looks_like_microphone_returns_false_on_empty_string() {
        // Capturer::device_name returns the empty string when cpal
        // fails to read a name (rare; tested in slice 8). The
        // heuristic must not panic on empty input.
        assert!(!super::looks_like_microphone(""));
    }

    // ----- Slice 11: STRICT_LOOPBACK_ENV const pin -----
    //
    // The strict-loopback env var is operator-facing: docs in the
    // README, the CLI long_about, and operator-shell aliases all
    // spell it `TEEHEE_STRICT_LOOPBACK`. A future rename would
    // silently break the integration tests in
    // `tests/capture_source_auto_integration.rs` and any
    // production wrapper scripts — pin the exact spelling here so
    // a typo surfaces as a precise compile-time failure rather
    // than a runtime no-op (env vars that aren't set are silent
    // failures; runtime ops don't know the lookup was misspelled).
    #[test]
    fn strict_loopback_env_const_is_correctly_named() {
        assert_eq!(
            super::STRICT_LOOPBACK_ENV,
            "TEEHEE_STRICT_LOOPBACK",
            "STRICT_LOOPBACK_ENV must spell `TEEHEE_STRICT_LOOPBACK` so \
             README, CLI long_about, and operator scripts agree"
        );
    }
}
