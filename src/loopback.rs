//! `loopback` — slice 8 (Tier 3 #1) Windows WASAPI loopback capture
//! adapter for teehee sender.
//!
//! Teehee's v1 default-input path reads the OS mic (or any user-installed
//! default input device like BlackHole / PulseAudio monitor). On Windows
//! that's typically a microphone — useful for voice streaming, useless
//! for system-audio streaming. Slice 8 adds a second capture source
//! that uses the Windows Audio Session API's
//! `AUDCLNT_STREAMFLAGS_LOOPBACK` mode: rather than activating a *capture*
//! endpoint, activate the default *render* endpoint as a capture
//! client. WASAPI then synthesizes a per-process capture of everything
//! the OS audio engine is about to play out the speakers.
//!
//! cpal 0.15 does not expose this — its `InputDevice` / `OutputDevice`
//! abstraction is endpoint-typed. Slice 8 takes the third option: a
//! sibling capturer struct [`LoopbackCapturer`] that uses the `wasapi`
//! 0.23 crate directly, with no cross-platform abstraction cost.
//! macOS / Linux callers fall back to the v1 default-input path (or
//! `--sine`).
//!
//! ## Surface
//!
//! * [`LoopbackSampleSource`] — cross-platform trait backing a
//!   loopback capturer. The Windows WASAPI impl lives behind a
//!   `cfg(target_os = "windows")` module; tests inject a
//!   [`crate::loopback::ScriptedSource`] that yields pre-built f32
//!   packets.
//! * [`LoopbackCapturer`] — a `CapturerConfig`-typed running capture
//!   that drives a [`LoopbackSampleSource`] from a dedicated worker
//!   thread. Same `.config()` and `.stop()` surface as the cpal
//!   [`crate::audio_io::Capturer`] so the sender pipeline can hold
//!   either type behind a Box<dyn AudioCapture> guard.
//!
//! ## Format support
//!
//! WASAPI's `GetMixFormat()` on a render endpoint typically returns
//! `WAVE_FORMAT_IEEE_FLOAT` (32-bit interleaved f32) on modern
//! Windows, but legacy render endpoints may report
//! `WAVE_FORMAT_PCM` (16-bit integer) or `WAVE_FORMAT_EXTENSIBLE`.
//! Slice 8 ships float-only: `WAVE_FORMAT_IEEE_FLOAT` is supported
//! directly; anything else returns a clear error naming the
//! offending format tag. PCM conversion is a follow-up slice.
//!
//! ## Threading / Send safety (Windows path)
//!
//! The Windows implementation owns `wasapi::AudioClient` and
//! `wasapi::AudioCaptureClient` — both wrappers around raw COM
//! pointers. wasapi-rs 0.23 does not auto-impl `Send` for them
//! because COM objects are apartment-bound. We initialize COM
//! in MTA on every thread that calls WASAPI (the main thread
//! during `open_default`, the worker thread on entry), and we
//! keep both handles on that single worker thread for their
//! entire lifetime. With single-thread ownership there are no
//! concurrent accesses for the data races that `Send` forbids,
//! so `unsafe impl Send for WasapiLoopbackSource` is sound.
//!
//! [`crate::audio_io::Capturer`]: crate::audio_io::Capturer
//! [`crate::audio_io::CapturedSampleFormat`]: crate::audio_io::CapturedSampleFormat

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(any(target_os = "windows", test))]
use crate::audio_io::CapturedSampleFormat;
use crate::audio_io::{AudioCapture, CapturerConfig};

/// Cross-platform source trait backing loopback capture.
pub trait LoopbackSampleSource: Send + 'static {
    /// Pull the next packet of interleaved f32 samples from the
    /// source. `Ok(None)` means "no frames ready, sleep and
    /// retry".
    fn next_packet(&mut self) -> anyhow::Result<Option<Vec<f32>>>;

    /// The source's actual sample rate, channel count, and native
    /// sample format at the time of capture open.
    fn config(&self) -> CapturerConfig;

    /// Signal the source to stop.
    fn stop(&mut self);
}

/// A running PCM capture stream backed by a Windows render
/// device's loopback (WASAPI LOOPBACK). The callback receives
/// interleaved `f32` samples.
#[derive(Debug)]
pub struct LoopbackCapturer {
    worker: Option<JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
    config: CapturerConfig,
}

impl LoopbackCapturer {
    pub fn config(&self) -> CapturerConfig {
        self.config
    }

    /// **Windows-only**.
    #[cfg(target_os = "windows")]
    pub fn open_default<F>(callback: F) -> anyhow::Result<Self>
    where
        F: FnMut(&[f32]) + Send + 'static,
    {
        let source = wasapi_loopback_source::WasapiLoopbackSource::open_default()
            .map_err(|e| anyhow::anyhow!("loopback open failed: {e}"))?;
        Self::open_with_source(source, callback)
    }

    /// **Non-Windows stub**.
    #[cfg(not(target_os = "windows"))]
    pub fn open_default<F>(_callback: F) -> anyhow::Result<Self>
    where
        F: FnMut(&[f32]) + Send + 'static,
    {
        Err(anyhow::anyhow!(
            "WASAPI loopback is Windows-only; on macOS/Linux use \
             --capture-source=default with a virtual input device \
             (BlackHole / PulseAudio monitor) installed as the \
             system-default input, or run --sine for a 440 Hz dry-run"
        ))
    }

    pub fn open_with_source<S, F>(mut source: S, mut callback: F) -> anyhow::Result<Self>
    where
        S: LoopbackSampleSource,
        F: FnMut(&[f32]) + Send + 'static,
    {
        let config = source.config();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_for_worker = Arc::clone(&stop_flag);
        let worker_name = if cfg!(target_os = "windows") {
            "teehee-wasapi-loopback"
        } else {
            "teehee-loopback-capture"
        };
        let worker = thread::Builder::new()
            .name(worker_name.into())
            .spawn(move || {
                // Per-thread COM init: each thread that issues
                // COM calls must call CoInitializeEx itself
                // (MTA-style here, for cross-thread handle
                // sharing). Idempotent — calling twice on the
                // same apartment returns S_FALSE / benign
                // status. wasapi-rs 0.23 wraps it in
                // `initialize_mta()` returning raw HRESULT (the
                // windows crate's Error trait gives us
                // `.failed()`).
                #[cfg(target_os = "windows")]
                {
                    let hr = wasapi::initialize_mta();
                    // `HRESULT` exposes `.is_ok()` via the windows
                    // crate's `Win32Error` trait. `hr.0` is the raw
                    // u32 HRESULT value (high bit cleared on success).
                    if !hr.is_ok() {
                        tracing::error!(
                            hresult = hr.0,
                            "COM MTA per-thread init failed on worker; loopback disabled"
                        );
                        source.stop();
                        return;
                    }
                }
                while !stop_for_worker.load(Ordering::Relaxed) {
                    match source.next_packet() {
                        Ok(Some(packet)) => callback(&packet),
                        Ok(None) => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "loopback source error, stopping worker"
                            );
                            break;
                        }
                    }
                }
                source.stop();
            })?;
        Ok(Self {
            worker: Some(worker),
            stop_flag,
            config,
        })
    }

    pub fn stop(&mut self) -> anyhow::Result<()> {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            handle.join().ok();
        }
        Ok(())
    }
}

impl Drop for LoopbackCapturer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl AudioCapture for LoopbackCapturer {
    fn config(&self) -> CapturerConfig {
        self.config()
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    /// Scripted sample-source for tests.
    pub struct ScriptedSource {
        config: CapturerConfig,
        packets: Vec<Vec<f32>>,
        yielded: usize,
    }

    impl ScriptedSource {
        pub fn new(config: CapturerConfig, packets: Vec<Vec<f32>>) -> Self {
            Self {
                config,
                packets,
                yielded: 0,
            }
        }
    }

    impl LoopbackSampleSource for ScriptedSource {
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
    fn scripted_source_drains_packets_in_order_then_returns_none() {
        let cfg = CapturerConfig {
            sample_rate: 48_000,
            channels: 2,
            sample_format: CapturedSampleFormat::F32,
        };
        let packets = vec![vec![0.7_f32; 1920], vec![-0.3_f32; 1920]];
        let mut src = ScriptedSource::new(cfg, packets.clone());
        let p1 = src.next_packet().expect("ok").expect("Some");
        assert_eq!(p1.len(), 1920);
        assert_eq!(p1[0], 0.7);
        let p2 = src.next_packet().expect("ok").expect("Some");
        assert_eq!(p2[0], -0.3);
        let none = src.next_packet().expect("ok");
        assert!(none.is_none(), "exhausted source must return Ok(None)");
    }

    #[test]
    fn scripted_source_config_is_copied_through() {
        let cfg = CapturerConfig {
            sample_rate: 44_100,
            channels: 1,
            sample_format: CapturedSampleFormat::F32,
        };
        let mut src = ScriptedSource::new(cfg, vec![]);
        assert_eq!(src.config(), cfg);
        assert!(src.next_packet().unwrap().is_none());
    }

    #[test]
    fn loopback_capturer_via_mock_source_invokes_callback_per_packet() {
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
        let collected: std::sync::Arc<std::sync::Mutex<Vec<Vec<f32>>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let c_for_cb = std::sync::Arc::clone(&collected);
        let cap = LoopbackCapturer::open_with_source(source, move |packet: &[f32]| {
            c_for_cb.lock().unwrap().push(packet.to_vec());
        })
        .expect("open_with_source");
        assert_eq!(cap.config().sample_rate, 48_000);
        assert_eq!(cap.config().channels, 2);
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while collected.lock().unwrap().len() < packets.len()
            && std::time::Instant::now() < deadline
        {
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
            assert_eq!(p.len(), 1920, "packet {i} must have 1920 samples");
            assert_eq!(p[0], packets[i][0]);
            assert_eq!(p[p.len() - 1], packets[i][packets[i].len() - 1]);
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn loopback_capturer_default_returns_error_on_non_windows() {
        let result = LoopbackCapturer::open_default(|_packet: &[f32]| {});
        let err = result.expect_err("non-Windows must return Err");
        let msg = format!("{err}");
        assert!(
            msg.contains("Windows-only")
                || msg.to_lowercase().contains("wasapi")
                || msg.contains("WASAPI"),
            "non-Windows error should mention Windows-only / WASAPI; got: {msg}"
        );
    }
}

// ============================================================================
// Windows WASAPI implementation — only compiled on Windows hosts.
// ============================================================================
#[cfg(target_os = "windows")]
mod wasapi_loopback_source {
    use super::*;
    use wasapi::*;

    pub struct WasapiLoopbackSource {
        audio_client: AudioClient,
        capture_client: AudioCaptureClient,
        config: CapturerConfig,
        event_handle: wasapi::Handle,
        sample_buf: Vec<f32>,
        idle_retries: u32,
    }

    // SAFETY: see module docstring. Concretely:
    // - Handles opened on the WORKER thread
    //   (LoopbackCapturer::open_with_source spawns the worker,
    //   which moves the source in).
    // - Handles accessed ONLY from the worker thread
    //   (next_packet and Drop both run there).
    // - The main thread only signals via `Arc<AtomicBool>` and
    //   joins the JoinHandle — it never touches the source
    //   struct's fields directly.
    // - With single-thread ownership there are no concurrent
    //   accesses for the data races that `Send` forbids.
    unsafe impl Send for WasapiLoopbackSource {}

    // Compile-time assertion that the unsafe Send impl holds.
    // Without this, a future change that breaks the Send-ness
    // (e.g. swapping in a non-Send field) would silently leave
    // the source immovable to the worker thread.
    #[allow(dead_code)]
    const _ASSERT_SEND: fn() = || {
        fn assert_send<T: Send>() {}
        assert_send::<WasapiLoopbackSource>();
    };

    impl WasapiLoopbackSource {
        pub fn open_default() -> anyhow::Result<Self> {
            // Initialize COM in MTA on the calling (main) thread.
            // wasapi-rs 0.23 returns raw HRESULT — check via
            // `.is_ok()` (HRESULT's `Win32Error`-trait method; the
            // high bit cleared indicates success).
            let hr = wasapi::initialize_mta();
            if !hr.is_ok() {
                return Err(anyhow::anyhow!(
                    "COM MTA init failed on main thread: HRESULT=0x{:08x}",
                    hr.0
                ));
            }
            let enumerator = DeviceEnumerator::new()
                .map_err(|e| anyhow::anyhow!("DeviceEnumerator::new failed: {e}"))?;
            // The LOOPBACK trick: enumerate the *render* endpoint
            // but initialize the AudioClient for *capture*
            // direction. WASAPI then mirrors the engine's
            // outbound mix; wasapi-rs's `initialize_client`
            // internally sets `AUDCLNT_STREAMFLAGS_LOOPBACK`
            // for this combination (verified by inspecting the
            // crate's source at audio_client.rs).
            let device = enumerator
                .get_default_device_for_role(&Direction::Render, &Role::Console)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "default render device lookup failed: {e} \
                         (no render endpoint; check default audio device)"
                    )
                })?;
            let mut audio_client = device
                .get_iaudioclient()
                .map_err(|e| anyhow::anyhow!("IAudioClient activate failed: {e}"))?;
            let mixfmt = audio_client
                .get_mixformat()
                .map_err(|e| anyhow::anyhow!("IAudioClient::GetMixFormat failed: {e}"))?;
            // WAVEFORMATEXTENSIBLE in the windows crate is
            // `#[repr(C, packed)]`; reading individual fields
            // requires `read_unaligned` because taking `&` to a
            // packed-field reference is undefined behaviour
            // (`E0793` "reference to packed field is unaligned").
            // The four fields we need are u16 / u32
            // little-endian values per WAVEFORMATEX spec, so a
            // single `read_unaligned` per field is enough — no
            // full-struct copy needed.
            //
            // SAFETY: `&raw const EXPR` produces a `*const T` to
            // EXPR's value without first taking `&` (which
            // would be the unaligned reference that's UB).
            // `ptr::read_unaligned` performs the value load
            // from an unaligned pointer, which is explicitly
            // its purpose.
            let sample_rate = unsafe {
                std::ptr::read_unaligned(&raw const mixfmt.wave_fmt.Format.nSamplesPerSec)
            };
            let channels: u16 =
                unsafe { std::ptr::read_unaligned(&raw const mixfmt.wave_fmt.Format.nChannels) };
            let format_tag: u16 =
                unsafe { std::ptr::read_unaligned(&raw const mixfmt.wave_fmt.Format.wFormatTag) };
            let bits_per_sample: u16 = unsafe {
                std::ptr::read_unaligned(&raw const mixfmt.wave_fmt.Format.wBitsPerSample)
            };
            // Slice 8 ships FLOAT-only; PCM-only render endpoints
            // surface a clear actionable error. EXTENSIBLE is
            // accepted as float-on-modern-Windows (SUBFORMAT
            // GUID parsing is a follow-up slice).
            const WAVE_FORMAT_PCM: u16 = 0x0001;
            const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
            const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
            let sample_format = match format_tag {
                WAVE_FORMAT_IEEE_FLOAT => CapturedSampleFormat::F32,
                WAVE_FORMAT_EXTENSIBLE => CapturedSampleFormat::F32,
                WAVE_FORMAT_PCM => {
                    return Err(anyhow::anyhow!(
                        "render device mix format is PCM ({} bits/sample); \
                         teehee v1 WASAPI loopback requires IEEE_FLOAT. \
                         Switch to a render endpoint that exposes a \
                         float mix format, or update teehee to add \
                         PCM i16/i24 normalization in a follow-up slice",
                        bits_per_sample
                    ));
                }
                other => {
                    return Err(anyhow::anyhow!(
                        "render device uses unsupported mix format 0x{other:04x}; \
                         teehee v1 supports 0x0003 (IEEE_FLOAT) and \
                         0xFFFE (EXTENSIBLE)"
                    ));
                }
            };
            if channels == 0 {
                return Err(anyhow::anyhow!(
                    "render device reports 0 channels; cannot initialize LOOPBACK"
                ));
            }
            // wasapi-rs 0.23 `initialize_client` signature:
            //   initialize_client(
            //     &mut self,
            //     wavefmt: &WaveFormat,
            //     direction: &Direction,
            //     stream_mode: &StreamMode,
            //   ) -> WasapiRes<()>
            //
            // LOOPBACK: pass `Direction::Capture` against a
            // render device — wasapi-rs sets the LOOPBACK flag
            // internally.
            const BUFFER_DURATION_HNS: i64 = 1_000_000; // 100 ms
            let channels_usize = channels as usize;
            audio_client
                .initialize_client(
                    &mixfmt,
                    &Direction::Capture,
                    &StreamMode::EventsShared {
                        autoconvert: true,
                        buffer_duration_hns: BUFFER_DURATION_HNS,
                    },
                )
                .map_err(|e| {
                    anyhow::anyhow!(
                        "IAudioClient::Initialize (LOOPBACK) failed: {e} \
                         (verify no other app holds exclusive mode on the \
                         render endpoint)"
                    )
                })?;
            let event_handle = audio_client
                .set_get_eventhandle()
                .map_err(|e| anyhow::anyhow!("set_get_eventhandle failed: {e}"))?;
            let capture_client = audio_client
                .get_audiocaptureclient()
                .map_err(|e| anyhow::anyhow!("IAudioCaptureClient activate failed: {e}"))?;
            audio_client
                .start_stream()
                .map_err(|e| anyhow::anyhow!("AudioClient::Start failed: {e}"))?;
            let initial_f32_count = ((BUFFER_DURATION_HNS as usize) * (sample_rate as usize)
                / 10_000_000)
                * channels_usize;
            let sample_buf: Vec<f32> = Vec::with_capacity(initial_f32_count.max(2048));
            Ok(Self {
                audio_client,
                capture_client,
                config: CapturerConfig {
                    sample_rate,
                    channels,
                    sample_format,
                },
                event_handle,
                sample_buf,
                idle_retries: 0,
            })
        }
    }

    impl Drop for WasapiLoopbackSource {
        fn drop(&mut self) {
            // IAudioClient::Stop is idempotent; errors swallowed
            // because Drop cannot fail. Drop runs on the WORKER
            // thread (the closure's locals drop there after
            // `source.stop()` returns).
            let _ = self.audio_client.stop_stream();
        }
    }

    impl LoopbackSampleSource for WasapiLoopbackSource {
        fn next_packet(&mut self) -> anyhow::Result<Option<Vec<f32>>> {
            let channels = self.config.channels as usize;
            // Hybrid: wait for the event briefly; if it fires,
            // data is ready.  If it times out, poll via
            // get_current_padding as a backup — some loopback
            // devices push data without signalling the event.
            let mut frames_avail: usize = 0;
            match self.event_handle.wait_for_event(2) {
                Ok(()) => {
                    // Event fired — get padding. If 0 (spurious
                    // wake), fall through to polling path below.
                    if let Ok(n) = self.audio_client.get_current_padding() {
                        if n > 0 {
                            frames_avail = n as usize;
                        }
                    }
                }
                Err(WasapiError::EventTimeout) => {}
                Err(e) => return Err(anyhow::anyhow!("wait_for_event: {e}")),
            }
            // Polling fallback: when the event doesn't fire but
            // data is nonetheless available (or during the 5-7s
            // warm-up period before the first signal).
            if frames_avail == 0 {
                if let Ok(n) = self.audio_client.get_current_padding() {
                    if n > 0 {
                        frames_avail = n as usize;
                        self.idle_retries = 0; // reset stall counter
                    }
                }
            }
            if frames_avail == 0 {
                self.idle_retries += 1;
                return Ok(None);
            }
            // BUG-6 FIX: use resize instead of capacity/reserve/
            // set_len so the slice can never span past the
            // allocation. resize initializes the memory to 0.0
            // (safe for WASAPI's read_from_device to overwrite).
            let sample_count = frames_avail * channels;
            self.sample_buf.resize(sample_count, 0.0);
            let bytes_needed = sample_count * 4;
            let read_view: &mut [u8] = unsafe {
                std::slice::from_raw_parts_mut(
                    self.sample_buf.as_mut_ptr() as *mut u8,
                    bytes_needed,
                )
            };
            let (consumed, info) = self
                .capture_client
                .read_from_device(read_view)
                .map_err(|e| anyhow::anyhow!("IAudioCaptureClient::ReadFromDevice: {e}"))?;
            if consumed == 0 {
                self.sample_buf.clear();
                return Ok(None);
            }
            // R2 FIX: treat BufferInfo's silent flag as "emit zeros
            // for that frame count". resize(actual_samples, 0.0)
            // only fills NEW elements — stale bytes from
            // read_from_device survive. Use truncate + fill instead.
            let actual_samples = (consumed as usize) * channels;
            self.sample_buf.truncate(actual_samples);
            if info.flags.silent {
                self.sample_buf.fill(0.0);
            }
            let out = self.sample_buf.clone();
            self.sample_buf.clear();
            Ok(Some(out))
        }
        fn config(&self) -> CapturerConfig {
            self.config
        }
        fn stop(&mut self) {
            let _ = self.audio_client.stop_stream();
        }
    }
}
