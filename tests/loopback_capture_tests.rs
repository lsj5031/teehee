//! Cross-platform integration tests for the slice 8 (Tier 3 #1)
//! WASAPI loopback capturer public API surface.
//!
//! The `src/loopback.rs::unit` block pins the internal packet-pump
//! algorithm and the cross-platform non-Windows stub error. This
//! file's value is verifying the *public* surface that the sender
//! pipeline + downstream callers actually use: the
//! `teehee::loopback::LoopbackCapturer` and
//! `teehee::audio_io::LoopbackCapturer` (re-export) paths are
//! the same type, and `Box<dyn AudioCapture>` exposes the right
//! dyn-dispatch behavior.
//!
//! Tests use the test-only `LoopbackCapturer::open_with_source`
//! constructor with an in-memory `ScriptedSource` so no audio
//! hardware is required.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use teehee::audio_io::{self, AudioCapture, CapturedSampleFormat, CapturerConfig};
use teehee::loopback::LoopbackCapturer;

/// In-memory scripted sample source yielding pre-built f32 packets.
/// Mirrors the `ScriptedSource` declared inside `src/loopback.rs::unit`
/// — re-declared here so the integration test exercises the public
/// `LoopbackSampleSource` trait shape (not the `pub(crate)` slice).
struct ScriptedSource {
    config: CapturerConfig,
    packets: Vec<Vec<f32>>,
    yielded: usize,
}

impl ScriptedSource {
    fn new(config: CapturerConfig, packets: Vec<Vec<f32>>) -> Self {
        Self {
            config,
            packets,
            yielded: 0,
        }
    }
}

impl teehee::loopback::LoopbackSampleSource for ScriptedSource {
    fn next_packet(&mut self) -> anyhow::Result<Option<Vec<f32>>> {
        thread::sleep(Duration::from_millis(5));
        if self.yielded < self.packets.len() {
            let p = self.packets[self.yielded].clone();
            self.yielded += 1;
            Ok(Some(p))
        } else {
            Ok(None)
        }
    }
    fn config(&self) -> CapturerConfig {
        self.config
    }
    fn stop(&mut self) {}
}

#[test]
fn audio_io_re_export_of_loopback_capturer_is_type_identical() {
    // The user's spec requires the loopback path to be reachable as
    // `teehee::audio_io::LoopbackCapturer`. Verify it's literally the
    // same type as the source-of-truth `teehee::loopback::LoopbackCapturer`
    // — a regression that introduces a wrapper type would silently
    // break the trait dispatch below.
    fn assertion<T>(_: T)
    where
        T: audio_io::AudioCapture + Send + 'static,
    {
    }
    let cap = LoopbackCapturer::open_with_source(
        ScriptedSource::new(
            CapturerConfig {
                sample_rate: 48_000,
                channels: 2,
                sample_format: CapturedSampleFormat::F32,
            },
            vec![vec![0.0_f32; 1920]],
        ),
        |_packet: &[f32]| {},
    )
    .expect("test open");
    // Capture the cap from `teehee::audio_io::LoopbackCapturer` (the
    // re-export) and verify it satisfies the trait bound that the
    // dispatch path requires.
    let audio_io_cap: audio_io::LoopbackCapturer = cap;
    assertion(audio_io_cap);
}

#[test]
fn box_dyn_audio_capture_dispatches_config_correctly() {
    // The sender pipeline stores the capturer as
    // `Box<dyn AudioCapture>`. Verify that dyn-dispatch through the
    // trait preserves the source's `CapturerConfig` correctly,
    // including the `sample_format` field.
    let cfg = CapturerConfig {
        sample_rate: 48_000,
        channels: 2,
        sample_format: CapturedSampleFormat::F32,
    };
    let cap = LoopbackCapturer::open_with_source(
        ScriptedSource::new(cfg, vec![vec![0.5_f32; 1920]]),
        |_packet: &[f32]| {},
    )
    .expect("test open");
    // The Box<dyn AudioCapture> path is the actual sender pipeline
    // shape (see main.rs run_send).
    let boxed: Box<dyn AudioCapture> = Box::new(cap);
    let reported = boxed.config();
    assert_eq!(reported.sample_rate, cfg.sample_rate);
    assert_eq!(reported.channels, cfg.channels);
    assert_eq!(reported.sample_format, cfg.sample_format);
}

#[test]
fn loopback_capturer_invokes_callback_per_packet_via_public_path() {
    let cfg = CapturerConfig {
        sample_rate: 48_000,
        channels: 2,
        sample_format: CapturedSampleFormat::F32,
    };
    let packets = vec![
        vec![0.7_f32; 1920],
        vec![0.3_f32; 1920],
        vec![-0.5_f32; 1920],
    ];
    let source = ScriptedSource::new(cfg, packets.clone());
    let collected: Arc<Mutex<Vec<Vec<f32>>>> = Arc::new(Mutex::new(Vec::new()));
    let c_for_cb = Arc::clone(&collected);
    let cap = LoopbackCapturer::open_with_source(source, move |packet: &[f32]| {
        c_for_cb.lock().unwrap().push(packet.to_vec());
    })
    .expect("open_with_source");
    assert_eq!(cap.config().sample_rate, 48_000);
    assert_eq!(cap.config().channels, 2);
    let deadline = Instant::now() + Duration::from_secs(1);
    while collected.lock().unwrap().len() < packets.len() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    drop(cap);
    let got = collected.lock().unwrap();
    assert_eq!(
        got.len(),
        packets.len(),
        "worker must drain all {} scripted packets",
        packets.len()
    );
    for (i, p) in got.iter().enumerate() {
        assert_eq!(p.len(), 1920);
        assert_eq!(p[0], packets[i][0]);
        assert_eq!(p[p.len() - 1], packets[i][packets[i].len() - 1]);
    }
}

#[test]
fn loopback_capturer_default_errors_on_non_windows_under_public_path() {
    // Cross-platform error contract: on macOS / Linux,
    // `LoopbackCapturer::open_default` returns Err with a message
    // that gives the operator an actionable next step (use
    // `--capture-source=default` + BlackHole / PulseAudio monitor,
    // or `--sine`). Verified *via* both the `teehee::loopback::` and
    // `teehee::audio_io::` paths to pin the re-export.
    #[cfg(not(target_os = "windows"))]
    {
        let r1 = teehee::loopback::LoopbackCapturer::open_default(|_packet: &[f32]| {});
        let r2 = audio_io::LoopbackCapturer::open_default(|_packet: &[f32]| {});
        for (path_name, result) in [("loopback::", &r1), ("audio_io::", &r2)] {
            let err = result.expect_err(&format!(
                "{path_name}LoopbackCapturer::open_default must Err on non-Windows"
            ));
            let msg = format!("{err}");
            assert!(
                msg.contains("Windows-only") || msg.to_lowercase().contains("wasapi"),
                "{path_name}non-Windows error should mention Windows-only / WASAPI; got: {msg}"
            );
        }
    }
}

#[test]
fn loopback_capturer_stops_cleanly_without_drop_panic() {
    // The `Drop` impl runs `stop()` which signals the worker to exit
    // and joins it. Verify that dropping the capturer doesn't panic
    // even when the worker is in the middle of pulling a packet.
    let cfg = CapturerConfig {
        sample_rate: 48_000,
        channels: 2,
        sample_format: CapturedSampleFormat::F32,
    };
    // 100 small packets keep the worker busy for ~5 s.
    let packets = vec![vec![0.5_f32; 192]; 100];
    let source = ScriptedSource::new(cfg, packets);
    let cap =
        LoopbackCapturer::open_with_source(source, |_packet: &[f32]| {}).expect("open_with_source");
    // Drop immediately rather than waiting for the source to drain.
    drop(cap);
}
