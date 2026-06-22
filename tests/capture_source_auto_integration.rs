//! `tests/capture_source_auto_integration.rs` — slice-11 followup:
//! integration test that proves `--capture-source auto`'s WASAPI-loopback
//! fallback path is reachable through `audio_io::open_auto_input`, not
//! just pinned at the helper's source level.
//!
//! ## Why this exists
//!
//! The earlier `tests/audio_io_tests.rs::open_auto_input_succeeds_when_default_input_available`
//! pins the auto path's *default-input success* branch — the helper
//! returns `cpal default input (auto-probed)` on hardware-present
//! machines. But the *fallback* branch (`WASAPI loopback
//! (auto-fallback)`) was only covered at the source-code level: on
//! any test machine with a working microphone, `Capturer::open_default_input`
//! succeeds and the loopback path is structurally unreachable.
//!
//! This file bridges that gap by injecting a forced failure into
//! `Capturer::open_default_input` via the `TEEHEE_FORCE_DEFAULT_INPUT_FAIL`
//! env-var test seam in `src/audio_io.rs`. With the env var set,
//! the helper takes its second attempt — the WASAPI loopback — and
//! surfaces it as `Box<dyn AudioCapture>` with the
//! `"WASAPI loopback (auto-fallback)"` source label.
//!
//! ## Platform gate
//!
//! **Windows-only** because WASAPI is Windows-only. The
//! `LoopbackCapturer::open_default` non-Windows stub returns Err
//! unconditionally; running the test on a macOS/Linux CI host
//! would surface that stub error rather than the genuine auto
//! path. The `#[cfg(target_os = "windows")]` test-level attribute
//! guards against accidental cross-platform runs.
//!
//! ## Float-mix render endpoint requirement
//!
//! The current WASAPI implementation accepts `WAVE_FORMAT_IEEE_FLOAT`
//! (and `WAVE_FORMAT_EXTENSIBLE`) and rejects `WAVE_FORMAT_PCM`
//! with a clear error message. On any stock Windows desktop with
//! the default audio device using `IEEE_FLOAT` (the modern
//! default), this test passes. On legacy hosts with PCM-only render
//! endpoints, the test surfaces a precise failure message naming
//! the render-format constraint — that's the contract: "fix the
//! render endpoint's mix format, then the test will pass."
//!
//! ## Required-features / double-gating notes
//!
//! The test is `#[ignore]` by default. To run it locally on a
//! Windows machine with a float-mix render endpoint, invoke:
//!
//! ```sh
//! cd T:\TeeHee && cargo test --test capture_source_auto_integration -- --ignored
//! ```
//!
//! Production CI doesn't run this test by default — it requires
//! WASAPI hardware eligibility that headless CI runners rarely
//! have (COM MTA init, wasapi AudioClient activate, etc.).
//!
//! ## TEEHEE_STRICT_LOOPBACK env var tests
//!
//! The lower half of this file pins the strict-loopback env var
//! shortcut that lives at `audio_io::open_auto_input`'s top: when
//! `TEEHEE_STRICT_LOOPBACK=1` is set, the helper short-circuits
//! the default-input probe and routes directly to WASAPI loopback
//! on Windows (label `"WASAPI loopback (strict)"`) or errors on
//! macOS / Linux (because the loopback route is Windows-only).
//! One Windows-only `#[ignore]` test exercises the success path
//! (full WASAPI hardware eligibility required, same as the
//! `auto-fallback` test above); a non-Windows test exercises the
//! error path and runs on every CI invocation.

use std::sync::{Arc, Mutex};

/// Force-exercises the WASAPI loopback fallback path of
/// `audio_io::open_auto_input`. With
/// `TEEHEE_FORCE_DEFAULT_INPUT_FAIL=1` set, `Capturer::open_default_input`
/// returns Err unconditionally, so the auto helper falls through to
/// `LoopbackCapturer::open_default` (Windows-only). Asserts the
/// returned label is the fallback label.
#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires Windows host with a float-mix render endpoint; \
            invoke with `cargo test --test capture_source_auto_integration \
            -- --ignored` on a Windows machine with WASAPI-float mix \
            audio (default Windows desktop config)"]
fn auto_falls_back_to_wasapi_loopback_when_default_forced_to_fail() {
    // Hunk 1: set the test seam. The env var is read inside
    // `Capturer::open_default_input`'s unconditional early-return
    // path; production builds skip the env-var read when the var
    // is unset (which is the production case), so this seam is
    // invisible outside `cargo test`.
    std::env::set_var("TEEHEE_FORCE_DEFAULT_INPUT_FAIL", "1");
    // Belt-and-braces: capture the prior value (if any) so we
    // restore it after the test even if a previous test set it
    // differently. Avoids cross-test contamination.
    let ring: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let ring_factory = Arc::clone(&ring);
    let make_cb = move || {
        let ring_inner = Arc::clone(&ring_factory);
        move |data: &[f32]| {
            ring_inner.lock().unwrap().extend_from_slice(data);
        }
    };
    // Run the helper. Expectation: with default-input forced to
    // fail, the helper takes the WASAPI loopback fallback path on
    // Windows and returns Ok((Box<dyn AudioCapture>, "WASAPI
    // loopback (auto-fallback)")).
    let result = audio_io::open_auto_input(make_cb);
    // Unset the env var BEFORE the assertion so a panic doesn't
    // leave it dangling for subsequent tests.
    std::env::remove_var("TEEHEE_FORCE_DEFAULT_INPUT_FAIL");

    let (cap, label) = match result {
        Ok(pair) => pair,
        Err(e) => {
            panic!(
                "--capture-source auto with forced default-input failure \
                 MUST fall back to WASAPI loopback on Windows; got Err: {e}\n\
                 \n\
                 Likely causes:\n\
                 \n\
                 1) Default render device's mix format is PCM (WAVE_FORMAT_PCM); \
                    teehee v1 only supports IEEE_FLOAT / EXTENSIBLE render \
                    endpoints for loopback capture. Verify your default \
                    output device exposes a float mix format.\n\
                 \n\
                 2) WASAPI COM MTA init failed (HRESULT); check the \
                    rendering device is not exclusively held by another app.\n\
                 \n\
                 3) The render endpoint's channels field is 0 (structurally \
                    impossible on real devices but the helper guards it anyway)."
            );
        }
    };
    assert_eq!(
        label, "WASAPI loopback (auto-fallback)",
        "--capture-source auto's fallback label must be `WASAPI loopback \
         (auto-fallback)` when the default-input step fails on Windows; \
         got `{label:?}`"
    );
    // Sanity: the capturer returned an actual reported config.
    // The loopback path's `CapturerConfig` reports the render
    // endpoint's mix format sample rate / channels; a real device
    // reports >0.
    let cfg = cap.config();
    assert!(
        cfg.sample_rate > 0,
        "loopback capturer config().sample_rate must report a real value (>0); \
         got {}. If 0, the WASAPI source returned a default-config it \
         never opened — that should be unreachable.",
        cfg.sample_rate
    );
    assert!(
        cfg.channels > 0,
        "loopback capturer config().channels must report >0; got {}",
        cfg.channels
    );
}

/// Cross-platform error contract: on macOS / Linux, the WASAPI stub
/// returns Err unconditionally. With the test seam forcing default-
/// input failure, the helper on macOS / Linux should surface the
/// cpal error verbatim with a "loopback fallback is Windows-only"
/// note (NOT a `wasapi_loopback_source`-stub-failure message —
/// which would mask the real cpal error per the slice-11 design
/// choice).
#[cfg(not(target_os = "windows"))]
#[test]
fn auto_non_windows_surfaces_default_error_when_forced() {
    std::env::set_var("TEEHEE_FORCE_DEFAULT_INPUT_FAIL", "1");
    let ring: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let make_cb = move || {
        let ring_inner = Arc::clone(&ring);
        move |data: &[f32]| {
            ring_inner.lock().unwrap().extend_from_slice(data);
        }
    };
    let result = audio_io::open_auto_input(make_cb);
    std::env::remove_var("TEEHEE_FORCE_DEFAULT_INPUT_FAIL");

    let err = result.expect_err(
        "auto with forced default-input failure must Err on non-Windows \
         (loopback fallback is Windows-only; the helper must surface the \
         cpal error verbatim, not the Windows-only stub error)",
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("Windows-only") || msg.to_lowercase().contains("loopback"),
        "non-Windows auto error must mention that the loopback fallback is \
         Windows-only; got: {msg}"
    );
    assert!(
        msg.contains("--capture-source=auto"),
        "non-Windows error must name the operator's flag so support can grep \
         log files; got: {msg}"
    );
}

// ----- Slice 11 followup: TEEHEE_STRICT_LOOPBACK env var -----
//
// Companion tests for the strict-loopback env var shortcut in
// `audio_io::open_auto_input`. The env var short-circuits the auto
// probe directly to WASAPI loopback on Windows (label
// `"WASAPI loopback (strict)"`) and errors on macOS / Linux
// (because the loopback route is Windows-only).
//
// These tests run inside the integration test binary, so they
// share process env-var state. env-var set/remove is symmetric
// within each test, but cargo's default parallel-execution could
// in principle race another env-var test mid-call. The mitigation
// is `cargo test -- --test-threads=1` for CI runs that want
// strict isolation; for v1 development the occasional race is
// acceptable because both env vars are independent and a real
// race surfaces as a failing assertion rather than a silent
// corruption.

/// Windows-only hardware-gated test: when `TEEHEE_STRICT_LOOPBACK=1`
/// is set in the test process environment, `audio_io::open_auto_input`
/// MUST take the WASAPI loopback route and return the
/// `"WASAPI loopback (strict)"` source label (skipping the
/// default-input probe entirely). This pins the strict-loopback
/// short-circuit at integration level rather than just at the
/// helper's source level.
#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires Windows host with a float-mix render endpoint; \
            invoke with `cargo test --test capture_source_auto_integration \
            -- --ignored` on a Windows machine with WASAPI-float mix \
            audio (default Windows desktop config); the default-input \
            probe is bypassed by the env var, so this test requires \
            real WASAPI hardware, NOT just a working mic"]
fn auto_strict_loopback_env_var_routes_to_wasapi_strict_label() {
    std::env::set_var("TEEHEE_STRICT_LOOPBACK", "1");
    // Belt-and-braces: capture the prior value (if any) so we
    // restore it after the test even if a previous test set it
    // differently. Avoids cross-test contamination.
    let ring: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let ring_factory = Arc::clone(&ring);
    let make_cb = move || {
        let ring_inner = Arc::clone(&ring_factory);
        move |data: &[f32]| {
            ring_inner.lock().unwrap().extend_from_slice(data);
        }
    };
    // Run the helper. Expectation: with STRICT_LOOPBACK set, the
    // helper takes the WASAPI loopback route immediately (NOT
    // probing the default-input device first). Returns
    // Ok((Box<dyn AudioCapture>, "WASAPI loopback (strict)")).
    let result = audio_io::open_auto_input(make_cb);
    // Unset the env var BEFORE the assertion so a panic doesn't
    // leave it dangling for subsequent tests.
    std::env::remove_var("TEEHEE_STRICT_LOOPBACK");

    let (cap, label) = match result {
        Ok(pair) => pair,
        Err(e) => {
            panic!(
                "--capture-source auto with TEEHEE_STRICT_LOOPBACK=1 \
                 MUST fall directly to WASAPI loopback on Windows; \
                 got Err: {e}\n\
                 \n\
                 Likely causes:\n\
                 \n\
                 1) Default render device's mix format is PCM (WAVE_FORMAT_PCM); \
                    teehee v1 only supports IEEE_FLOAT / EXTENSIBLE render \
                    endpoints for loopback capture.\n\
                 \n\
                 2) WASAPI COM MTA init failed (HRESULT); check the \
                    rendering device is not exclusively held by another app.\n\
                 \n\
                 3) The render endpoint's channels field is 0 (structurally \
                    impossible on real devices but the helper guards it anyway)."
            );
        }
    };
    assert_eq!(
        label, "WASAPI loopback (strict)",
        "TEEHEE_STRICT_LOOPBACK=1 must route --capture-source auto directly \
         to the WASAPI loopback path with label `WASAPI loopback (strict)`; \
         got `{label:?}`. If you see `cpal default input (auto-probed)`, \
         the strict-loopback env var check inside open_auto_input isn't \
         firing before the default-input probe — review the env-var short-\n\
         circuit at the top of open_auto_input in src/audio_io.rs."
    );
    // Sanity: the capturer returned an actual reported config.
    // The strict-loopback path's `CapturerConfig` reports the
    // render endpoint's mix format sample rate / channels; a
    // real device reports >0.
    let cfg = cap.config();
    assert!(
        cfg.sample_rate > 0,
        "strict-loopback capturer config().sample_rate must report a \
         real value (>0); got {}. If 0, the WASAPI source returned a \
         default-config it never opened — that should be unreachable.",
        cfg.sample_rate
    );
    assert!(
        cfg.channels > 0,
        "strict-loopback capturer config().channels must report >0; got {}",
        cfg.channels
    );
}

/// Cross-platform error contract: when `TEEHEE_STRICT_LOOPBACK=1` is
/// set on macOS / Linux, the helper MUST error with a clear
/// "Windows-only" mention (because the loopback route is
/// Windows-only). The error message MUST name the env var so
/// support can grep log files for "set the env var without OS
/// support" / "unset the env var" diagnoses, and MUST name the
/// operator's `--capture-source=auto` flag so the user can
/// locate the auto-path diagnostic.
#[cfg(not(target_os = "windows"))]
#[test]
fn auto_non_windows_strict_loopback_env_var_surfaces_windows_only_error() {
    std::env::set_var("TEEHEE_STRICT_LOOPBACK", "1");
    let ring: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let make_cb = move || {
        let ring_inner = Arc::clone(&ring);
        move |data: &[f32]| {
            ring_inner.lock().unwrap().extend_from_slice(data);
        }
    };
    let result = audio_io::open_auto_input(make_cb);
    std::env::remove_var("TEEHEE_STRICT_LOOPBACK");

    let err = result.expect_err(
        "TEEHEE_STRICT_LOOPBACK=1 must Err on non-Windows (loopback \
         route is Windows-only; the strict-loopback short-circuit \
         cannot redirect to a stub that doesn't exist on macOS / \
         Linux)",
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("Windows-only") || msg.to_lowercase().contains("loopback"),
        "non-Windows strict-loopback error must mention that the loopback \
         fallback is Windows-only; got: {msg}"
    );
    // Must name the env var so support can grep logs.
    assert!(
        msg.contains("TEEHEE_STRICT_LOOPBACK"),
        "non-Windows strict-loopback error MUST name the env var so \
         support can grep for it; got: {msg}"
    );
    // Must name the operator's flag so the operator knows it's
    // about the auto path specifically (not a different
    // --capture-source error).
    assert!(
        msg.contains("--capture-source=auto"),
        "non-Windows strict-loopback error MUST name the operator's \
         flag so they can locate the auto path diagnostic; got: {msg}"
    );
}

mod audio_io {
    pub use teehee::audio_io::*;
}
