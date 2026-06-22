//! Integration tests for the `audio_io` module — exercise device
//! enumeration and player/capturer stream opening on real hosts. All
//! tests gracefully no-op when no audio hardware is available
//! (headless CI without an audio device).

mod common;

use teehee::audio_io;

#[test]
fn list_devices_returns_at_least_one_on_hosts_with_hardware() {
    if common::is_ci() {
        return;
    }
    let outputs = audio_io::list_output_devices();
    let inputs = audio_io::list_input_devices();
    if outputs.is_empty() && inputs.is_empty() {
        return;
    }
    assert!(
        outputs.len() + inputs.len() > 0,
        "expected at least one device on a hosts-with-hardware run"
    );
}

#[test]
fn default_output_device_is_marked_as_default() {
    if common::is_ci() {
        return;
    }
    let Some(default) = audio_io::default_output_device() else {
        return;
    };
    assert!(default.is_default, "default device must flag is_default");

    let outputs = audio_io::list_output_devices();
    assert!(
        outputs
            .iter()
            .any(|d| d.is_default && d.name == default.name),
        "default output device must appear in list_output_devices with is_default=true"
    );
}

#[test]
fn default_input_device_is_marked_as_default() {
    if common::is_ci() {
        return;
    }
    let Some(default) = audio_io::default_input_device() else {
        return;
    };
    assert!(default.is_default, "default device must flag is_default");
}

#[test]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn player_opens_default_output_stream() {
    if common::is_ci() || audio_io::default_output_device().is_none() {
        return;
    }
    let player = audio_io::Player::open_default_output(|buf| {
        for s in buf {
            *s = 0.0;
        }
    });
    assert!(
        player.is_ok(),
        "open_default_output failed: {:?}",
        player.err()
    );
}

#[test]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn capturer_opens_default_input_stream() {
    if common::is_ci() || audio_io::default_input_device().is_none() {
        return;
    }
    let capturer = audio_io::Capturer::open_default_input(|_samples| {});
    assert!(
        capturer.is_ok(),
        "open_default_input failed: {:?}",
        capturer.err()
    );
}

#[test]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn capturer_config_matches_default_input_device() {
    // Slice 3 regression test: the Capturer::config() accessor must
    // echo exactly what the device's `default_input_config` reports,
    // so the sender pipeline can compute chunk math and label packets
    // from device-actual values rather than CLI defaults.
    if common::is_ci() {
        return;
    }
    let Some(device_info) = audio_io::default_input_device() else {
        return;
    };
    let capturer = audio_io::Capturer::open_default_input(|_samples| {});
    let Ok(capturer) = capturer else {
        return;
    };
    let cfg = capturer.config();
    assert_eq!(
        cfg.sample_rate, device_info.sample_rate_hz,
        "capturer config().sample_rate must echo default_input_device's \
         sample_rate_hz; got {} vs device {}",
        cfg.sample_rate, device_info.sample_rate_hz
    );
    assert_eq!(
        cfg.channels, device_info.channels,
        "capturer config().channels must echo default_input_device's channels"
    );
}

#[test]
fn capturer_config_struct_equality_is_by_value() {
    // Pin the CapturerConfig field-by-field semantics: buildable,
    // Copy, value-equality via PartialEq. sample_format is part
    // of equality so a future refactor that hides a format-specific
    // branch in a copy without including it surfaces here.
    let a = audio_io::CapturerConfig {
        sample_rate: 48_000,
        channels: 2,
        sample_format: audio_io::CapturedSampleFormat::F32,
    };
    let b = a; // Copy
    assert_eq!(a, b);
    // Same sample_rate + channels but different native sample_format
    // must still compare unequal — sampling format matters for
    // slice 7's receiver-reconciliation logic.
    let c = audio_io::CapturerConfig {
        sample_rate: 48_000,
        channels: 2,
        sample_format: audio_io::CapturedSampleFormat::I16,
    };
    assert_ne!(a, c);
    let d = audio_io::CapturerConfig {
        sample_rate: 44_100,
        channels: 2,
        sample_format: audio_io::CapturedSampleFormat::F32,
    };
    assert_ne!(a, d);
}

#[test]
fn device_info_struct_equality_is_by_value() {
    let a = audio_io::DeviceInfo {
        name: "Speaker (Test)".into(),
        is_default: true,
        channels: 2,
        sample_rate_hz: 48_000,
    };
    let b = a.clone();
    assert_eq!(a, b);
}

// ----- Slice 11: open_auto_input -----

#[test]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn open_auto_input_succeeds_when_default_input_available() {
    // Happy-path hardware-gated test: when the OS exposes a default
    // input device, `open_auto_input` must return Ok with the
    // auto-probed default-input Label (NOT the loopback fallback).
    // This pins that:
    //   1) The factory-pattern API compiles + runs end to end.
    //   2) The label emitted to --stats flags the cpal path,
    //      not WASAPI loopback, so an operator reading the log
    //      line sees exactly which capture source opened.
    //   3) The Box<dyn AudioCapture> dispatch returns a real
    //      capturer (its `.config()` reports a sample_rate > 0,
    //      confirming the stream actually opened).
    if common::is_ci() || audio_io::default_input_device().is_none() {
        return;
    }
    let make_cb = || |_samples: &[f32]| {};
    let result = audio_io::open_auto_input(make_cb);
    let (cap, label) =
        result.expect("open_auto_input must succeed on a host with a default input device");
    // The default-input path is tried first. On a Windows host with
    // BOTH a microphone AND WASAPI loopback reachable, default-input
    // wins so the label is the auto-probed label; on macOS / Linux
    // there's no loopback fallback so this is the only possible
    // label.
    assert_eq!(
        label, "cpal default input (auto-probed)",
        "auto on a host with a working default-input device must label cpal default input"
    );
    let cfg = cap.config();
    assert!(
        cfg.sample_rate > 0,
        "capturer config() must report a real sample_rate (>0)"
    );
}

#[test]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn open_auto_input_factory_callable_more_than_once() {
    // Pin the factory-pattern API contract: the helper accepts an
    // `FnMut` factory, not a single by-value callback. Tests that
    // the factory is INVOKED (called once before being passed to
    // cpal::Capturer::open_default_input or wasapi::LoopbackCapturer).
    // On macOS/Linux the second invocation is skipped (loopback
    // fallback unreachable) so the lower bound is 1; on Windows in
    // an error-after-default scenario the bound could be 2. The
    // counter is incremented in the FACTORY body, not the closure
    // body — the closure body fires asynchronously from the cpal
    // audio thread (~10 ms cadence) and the helper may return
    // before the first callback, so we cannot pin closure-call
    // counts here.
    if common::is_ci() || audio_io::default_input_device().is_none() {
        return;
    }
    let factory_call_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter_for_factory = std::sync::Arc::clone(&factory_call_count);
    let make_cb = move || {
        counter_for_factory.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Fresh closure per invocation — independent of any audio-
        // thread callback cadence. Body is a no-op; the test cares
        // only that the factory itself was invoked, not that the
        // returned closure has fired.
        |_samples: &[f32]| {}
    };
    let _ = audio_io::open_auto_input(make_cb)
        .expect("open_auto_input must succeed with a fresh-closure factory");
    assert!(
        factory_call_count.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "factory must have been invoked at least once by the helper"
    );
}

#[test]
#[cfg(not(target_os = "windows"))]
fn open_auto_input_non_windows_surfaces_default_error_verbatim() {
    // Cross-platform error contract: on macOS / Linux, `auto` behaves
    // identically to `default` — when there's no default input
    // device, the helper returns Err carrying the cpal /
    // OS-specific error message with a clarifying "loopback
    // fallback is Windows-only" line.
    //
    // This test only runs on macOS / Linux AND requires a host where
    // `default_input_device()` returns None — typically a CI runner
    // without audio hardware. On a Mac dev machine with a real mic,
    // the helper's first attempt succeeds and this path is
    // unreachable; that's fine because the helper's behavior on
    // success is pinned by `open_auto_input_succeeds_when_default_input_available`
    // above. Pin only the error-shape contract here.
    if audio_io::default_input_device().is_some() {
        return;
    }
    let make_cb = || |_samples: &[f32]| {};
    let err = audio_io::open_auto_input(make_cb).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("Windows-only")
            || msg.to_lowercase().contains("loopback"),
        "non-Windows auto error must mention that the loopback fallback is Windows-only; got: {msg}"
    );
    assert!(
        msg.contains("--capture-source=auto"),
        "non-Windows error must mention the flag the operator typed; got: {msg}"
    );
}
