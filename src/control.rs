//! `control` — local TCP control server for runtime volume / pause.
//!
//! Listens on `127.0.0.1:{port}` (disabled by default) with no auth
//! needed — reachable only from localhost. Enable with
//! `--control-port <N>` (e.g. `--control-port 9090`).
//!
//! ## Commands
//!
//! | Command            | Effect                                  |
//! |--------------------|-----------------------------------------|
//! | `pause` / `p`      | Stop encoding and sending packets       |
//! | `resume` / `r`     | Resume sending (capture ring cleared)   |
//! | `volume <N>`       | Set volume as percentage (0–100)        |
//! | `v <N>`            | Same as volume                          |
//! | `gain <N>`         | Set gain multiplier (0.0–10.0)          |
//! | `g <N>`            | Same as gain                            |
//! | `status` / `s`     | Print paused state, gain, source        |
//! | `help` / `h` / `?` | List available commands                 |
//!
//! Sending `volume` / `gain` without a value prints the current gain.
//!
//! ## Pause semantics
//!
//! When paused the encode loop stops draining the capture ring.
//! On resume the capture ring is cleared so only fresh audio is sent —
//! no stale samples are replayed. The ring continues to fill from the
//! capture device while paused; those samples are discarded on resume.
//!
//! ## Examples (Windows)
//!
//! ```powershell
//! # Check status:
//! $c = New-Object System.Net.Sockets.TcpClient("127.0.0.1", 9090)
//! $s = $c.GetStream(); $w = New-Object System.IO.StreamWriter($s)
//! $w.WriteLine("status"); $w.Flush()
//! $r = New-Object System.IO.StreamReader($s); $r.ReadLine()
//! $c.Close()
//! ```

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Whether the gain was last set by a manual TCP command or by the
/// system-volume follower (`--follow-system-volume`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GainSource {
    /// Manually set via a TCP control command.
    Manual = 0,
    /// Set by `--follow-system-volume`.
    System = 1,
}

impl GainSource {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Manual,
            1 => Self::System,
            // Unknown discriminant — default to Manual (safe fallback).
            _ => Self::Manual,
        }
    }
}

/// Runtime control state shared between the control-server thread and
/// the sender encode loop. Uses lock-free atomics so the encode loop
/// (hot path) never blocks on a mutex.
#[derive(Clone)]
pub struct ControlState {
    pub paused: Arc<AtomicBool>,
    /// f32 gain stored as u32 bits — `f32::to_bits()` / `f32::from_bits()`.
    pub gain: Arc<AtomicU32>,
    /// 0 = Manual, 1 = System.
    gain_source: Arc<AtomicU8>,
}

impl Default for ControlState {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlState {
    pub fn new() -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
            gain: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            gain_source: Arc::new(AtomicU8::new(GainSource::Manual as u8)),
        }
    }

    /// True when the encode loop should skip encoding/sending.
    #[inline]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Current gain multiplier. 1.0 = unity, 0.0 = mute.
    #[inline]
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    /// Whether the gain was last set manually or by the system follower.
    pub fn gain_source(&self) -> GainSource {
        GainSource::from_u8(self.gain_source.load(Ordering::Relaxed))
    }

    /// Set gain and record its source. Rejects non-finite values.
    pub fn set_gain_with_source(&self, g: f32, source: GainSource) {
        debug_assert!(
            g.is_finite(),
            "set_gain_with_source called with non-finite gain"
        );
        self.gain.store(g.to_bits(), Ordering::Relaxed);
        self.gain_source.store(source as u8, Ordering::Relaxed);
    }

    /// Apply the current gain multiplier to a sample buffer in place.
    /// Skips work entirely when gain is 1.0 (unity) so the hot encode
    /// loop pays one atomic load plus a single f32 compare in the
    /// steady-state case. `#[inline]` keeps the no-gain path on
    /// par with hand-rolled `if gain != 1.0` in the call sites.
    #[inline]
    pub fn apply_gain(&self, buf: &mut [f32]) {
        let g = self.gain();
        if g == 1.0 {
            return;
        }
        for s in buf {
            *s *= g;
        }
    }
}

/// Start the TCP control server on `127.0.0.1:{port}`.
///
/// Spawns a thread named `"teehee-control"` that accepts connections
/// and handles each in its own short-lived thread. Returns immediately;
/// the server runs until process exit.
///
/// # Errors
///
/// Returns `Err` if the port cannot be bound (already in use, or
/// permission denied on a privileged port).
pub fn start_server(port: u16, state: ControlState) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    thread::Builder::new()
        .name("teehee-control".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let st = state.clone();
                        thread::spawn(move || handle_connection(stream, &st));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "control server accept error");
                    }
                }
            }
        })?;
    Ok(())
}

fn handle_connection(mut stream: TcpStream, state: &ControlState) {
    // Guard against hung clients: drop after 5 s of inactivity.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

    // Clone the stream so we can keep `stream` for writing while
    // `BufReader` owns the read-half clone.
    let read_half = match stream.try_clone() {
        Ok(h) => h,
        Err(_) => return,
    };
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => {} // EOF (client closed without sending)
        Ok(_) => {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let response = execute(trimmed, state);
                let _ = writeln!(stream, "{response}");
            }
        }
        Err(_) => {
            let _ = writeln!(stream, "error: read timeout or I/O error");
        }
    }
}

fn execute(cmd: &str, state: &ControlState) -> String {
    let mut parts = cmd.split_whitespace();
    let verb = parts.next().unwrap_or("");

    match verb {
        "pause" | "p" => {
            state.paused.store(true, Ordering::Relaxed);
            "paused".into()
        }
        "resume" | "r" => {
            state.paused.store(false, Ordering::Relaxed);
            "resumed".into()
        }
        "volume" | "v" => match parts.next() {
            None => {
                let g = state.gain();
                let pct = (g * 100.0).round() as u32;
                format!("volume={pct}% gain={g:.3}")
            }
            Some(val) => {
                if parts.next().is_some() {
                    return format!(
                        "error: unexpected arguments after '{val}' \
                             (volume takes a single value 0..100)"
                    );
                }
                match val.parse::<u32>() {
                    Ok(pct) if pct <= 100 => {
                        let g = pct as f32 / 100.0;
                        state.set_gain_with_source(g, GainSource::Manual);
                        format!("volume={pct}% gain={g:.3}")
                    }
                    Ok(pct) => {
                        format!("error: volume must be 0..100 (got {pct})")
                    }
                    Err(_) => format!("error: invalid percentage '{val}'"),
                }
            }
        },
        "gain" | "g" => match parts.next() {
            None => format!("gain={:.3}", state.gain()),
            Some(val) => {
                if parts.next().is_some() {
                    return format!(
                        "error: unexpected arguments after '{val}' \
                             (gain takes a single value 0.0..10.0)"
                    );
                }
                match val.parse::<f32>() {
                    Ok(v) if !v.is_finite() => {
                        format!("error: gain must be a finite number (got {v})")
                    }
                    Ok(v) => {
                        let clamped = v.clamp(0.0, 10.0);
                        state.set_gain_with_source(clamped, GainSource::Manual);
                        format!("gain={clamped:.3}")
                    }
                    Err(_) => format!("error: invalid number '{val}'"),
                }
            }
        },
        "status" | "s" => {
            let src = match state.gain_source() {
                GainSource::Manual => "manual",
                GainSource::System => "system",
            };
            format!(
                "paused={} gain={:.3} source={src}",
                state.is_paused(),
                state.gain(),
            )
        }
        "help" | "h" | "?" => {
            "commands: pause, resume, volume <0-100>, gain <0.0-10.0>, status, help\n\
             volume accepts a percentage (0=silent, 100=unity)\n\
             gain accepts a raw multiplier (0.0=mute, 1.0=unity, 10.0=max)\n\
             \n\
             PowerShell example:\n\
             > $c = New-Object System.Net.Sockets.TcpClient('127.0.0.1', 9090)\n\
             > $s = $c.GetStream()\n\
             > $w = New-Object System.IO.StreamWriter($s)\n\
             > $w.WriteLine('volume 50'); $w.Flush()\n\
             > $r = New-Object System.IO.StreamReader($s); $r.ReadLine()\n\
             > $c.Close()"
                .into()
        }
        "" => "OK".into(),
        _ => format!("unknown: '{verb}' (try 'help')"),
    }
}

// ── System volume follower (Windows) ──────────────────────────

/// Read the current master volume level and mute state of the default
/// render endpoint. Returns `Some(0.0)` when the system is muted,
/// `Some(scalar)` when unmuted (0.0–1.0), or `None` if the OS API
/// call fails.
///
/// Uses `eConsole` (not `eMultimedia`) so the follower tracks the
/// same endpoint role that WASAPI loopback captures from.
///
/// On non-Windows hosts this always returns `None`.
fn read_system_volume() -> Option<f32> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
        use windows::Win32::Media::Audio::{
            eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
        };

        // SAFETY: COM must be initialised on the calling thread.
        // CoInitializeEx is idempotent (returns S_FALSE if already
        // initialised with the same model on this thread).
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let endpoint_volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
            // Check mute state first — if the system is muted,
            // return 0.0 regardless of the scalar level.
            let muted = endpoint_volume.GetMute().ok()?.as_bool();
            if muted {
                return Some(0.0);
            }
            endpoint_volume.GetMasterVolumeLevelScalar().ok()
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// Read the system master volume once and apply it to `state`.
/// Returns `Ok(vol)` on success, or `Err` if the OS API call fails.
///
/// Uses `eConsole` (not `eMultimedia`) so the follower tracks the
/// same endpoint role that WASAPI loopback captures from.
///
/// On non-Windows hosts this always returns `Err`.
pub fn read_and_apply_system_volume(state: &ControlState) -> Result<f32, &'static str> {
    read_system_volume()
        .inspect(|&vol| {
            state.set_gain_with_source(vol, GainSource::System);
        })
        .ok_or("failed to read system volume (no audio device or unsupported platform)")
}

/// Spawn a background thread that polls the system master volume
/// (Windows default render endpoint) every 500 ms and writes the
/// result into `state.gain` with `GainSource::System`.
///
/// The caller should call [`read_and_apply_system_volume`] **before**
/// spawning this thread so the initial gain is set synchronously
/// before any audio packets are sent.
///
/// On non-Windows hosts this logs a warning and returns without
/// spawning a thread — the flag is accepted but has no effect.
pub fn spawn_volume_follower(state: ControlState) {
    #[cfg(not(target_os = "windows"))]
    {
        tracing::warn!(
            "--follow-system-volume has no effect on this platform \
             (Windows WASAPI only); the flag is accepted but ignored"
        );
        return;
    }

    #[cfg(target_os = "windows")]
    {
        let _ = thread::Builder::new()
            .name("teehee-vol-follow".into())
            .spawn(move || {
                // COM must be initialized on this thread before any
                // WASAPI calls. CoInitializeEx is idempotent.
                use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
                let mut error_count: u64 = 0;
                loop {
                    match read_system_volume() {
                        Some(vol) => {
                            error_count = 0;
                            state.set_gain_with_source(vol, GainSource::System);
                        }
                        None => {
                            error_count += 1;
                            // Log the first error and then every 60th
                            // (~30 s at 500 ms polling) to avoid spam.
                            if error_count == 1 || error_count.is_multiple_of(60) {
                                tracing::warn!(
                                    count = error_count,
                                    "failed to read system volume \
                                     (is an audio device connected?)"
                                );
                            }
                        }
                    }
                    thread::sleep(Duration::from_millis(500));
                }
            });
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn control_state_defaults() {
        let cs = ControlState::new();
        assert!(!cs.is_paused());
        assert!((cs.gain() - 1.0).abs() < f32::EPSILON);
        assert_eq!(cs.gain_source(), GainSource::Manual);
    }

    #[test]
    fn pause_resume() {
        let cs = ControlState::new();
        execute("pause", &cs);
        assert!(cs.is_paused());
        execute("r", &cs);
        assert!(!cs.is_paused());
        execute("resume", &cs);
        assert!(!cs.is_paused());
        execute("p", &cs);
        assert!(cs.is_paused());
    }

    #[test]
    fn volume_percentage() {
        let cs = ControlState::new();
        // volume 50 → gain 0.5
        let r = execute("volume 50", &cs);
        assert!(r.contains("volume=50%"), "got: {r}");
        assert!(r.contains("gain=0.500"), "got: {r}");
        assert!((cs.gain() - 0.5).abs() < 0.001);
        assert_eq!(cs.gain_source(), GainSource::Manual);

        // volume 0 → gain 0.0 (mute)
        let r = execute("v 0", &cs);
        assert!(r.contains("volume=0%"), "got: {r}");
        assert!((cs.gain() - 0.0).abs() < 0.001);

        // volume 100 → gain 1.0 (unity)
        let r = execute("volume 100", &cs);
        assert!(r.contains("volume=100%"), "got: {r}");
        assert!((cs.gain() - 1.0).abs() < 0.001);

        // volume 101 → rejected
        let r = execute("volume 101", &cs);
        assert!(r.contains("error"), "got: {r}");
        assert!((cs.gain() - 1.0).abs() < 0.001, "gain unchanged");

        // Query without value → shows current volume %
        execute("v 25", &cs);
        let r = execute("volume", &cs);
        assert!(r.contains("gain=0.250"), "got: {r}");
    }

    #[test]
    fn gain_multiplier() {
        let cs = ControlState::new();
        let r = execute("gain 2.0", &cs);
        assert!(r.contains("gain=2.000"), "got: {r}");
        assert!((cs.gain() - 2.0).abs() < 0.001);
        assert_eq!(cs.gain_source(), GainSource::Manual);

        // Clamp high
        execute("g 999.0", &cs);
        assert!((cs.gain() - 10.0).abs() < 0.001);

        // Clamp low
        execute("g -5.0", &cs);
        assert!((cs.gain() - 0.0).abs() < 0.001);

        // Query without value
        execute("g 0.25", &cs);
        let r = execute("gain", &cs);
        assert!(r.contains("gain=0.250"), "got: {r}");
    }

    #[test]
    fn nan_inf_rejected() {
        let cs = ControlState::new();
        let r = execute("gain NaN", &cs);
        assert!(r.contains("error"), "NaN must be rejected: {r}");
        assert!((cs.gain() - 1.0).abs() < 0.001, "gain unchanged");

        let r = execute("gain inf", &cs);
        assert!(r.contains("error"), "inf must be rejected: {r}");
        assert!((cs.gain() - 1.0).abs() < 0.001);

        let r = execute("gain -inf", &cs);
        assert!(r.contains("error"), "-inf must be rejected: {r}");
        assert!((cs.gain() - 1.0).abs() < 0.001);
    }

    #[test]
    fn trailing_arguments_rejected() {
        let cs = ControlState::new();
        let r = execute("volume 50 extra", &cs);
        assert!(r.contains("error"), "trailing args rejected: {r}");
        assert!((cs.gain() - 1.0).abs() < 0.001);

        let r = execute("gain 2.0 extra", &cs);
        assert!(r.contains("error"), "trailing args rejected: {r}");
        assert!((cs.gain() - 1.0).abs() < 0.001);
    }

    #[test]
    fn status_line() {
        let cs = ControlState::new();
        let r = execute("status", &cs);
        assert!(r.contains("paused=false"), "got: {r}");
        assert!(r.contains("gain=1.000"), "got: {r}");
        assert!(r.contains("source=manual"), "got: {r}");

        execute("pause", &cs);
        execute("g 0.5", &cs);
        let r = execute("s", &cs);
        assert!(r.contains("paused=true"), "got: {r}");
        assert!(r.contains("gain=0.500"), "got: {r}");
        assert!(r.contains("source=manual"), "got: {r}");
    }

    #[test]
    fn status_shows_system_source() {
        let cs = ControlState::new();
        cs.set_gain_with_source(0.7, GainSource::System);
        let r = execute("status", &cs);
        assert!(r.contains("source=system"), "got: {r}");
        assert!(r.contains("gain=0.700"), "got: {r}");
    }

    #[test]
    fn help_and_unknown() {
        let cs = ControlState::new();
        let r = execute("help", &cs);
        assert!(r.contains("pause"), "got: {r}");
        assert!(r.contains("resume"), "got: {r}");
        assert!(r.contains("volume"), "got: {r}");
        assert!(r.contains("status"), "got: {r}");
        // Verify PowerShell example is present (finding 6).
        assert!(r.contains("PowerShell"), "got: {r}");

        let r = execute("h", &cs);
        assert!(r.contains("pause"));

        let r = execute("?", &cs);
        assert!(r.contains("pause"));

        let r = execute("bogus", &cs);
        assert!(r.contains("unknown"));
        assert!(r.contains("help"));
    }

    #[test]
    fn aliases() {
        let cs = ControlState::new();
        execute("p", &cs);
        assert!(cs.is_paused());
        execute("r", &cs);
        assert!(!cs.is_paused());
        execute("g 0.2", &cs);
        assert!((cs.gain() - 0.2).abs() < 0.001);
    }

    #[test]
    fn invalid_number_reported() {
        let cs = ControlState::new();
        let r = execute("volume xyz", &cs);
        assert!(r.contains("invalid percentage"), "got: {r}");
        assert!(r.contains("xyz"), "got: {r}");
        // Gain unchanged
        assert!((cs.gain() - 1.0).abs() < 0.001);
    }

    #[test]
    fn gain_source_manual_default() {
        let cs = ControlState::new();
        assert_eq!(cs.gain_source(), GainSource::Manual);
        execute("volume 50", &cs);
        assert_eq!(cs.gain_source(), GainSource::Manual);
        execute("gain 2.0", &cs);
        assert_eq!(cs.gain_source(), GainSource::Manual);
    }

    #[test]
    fn gain_source_system_override() {
        let cs = ControlState::new();
        cs.set_gain_with_source(0.7, GainSource::System);
        assert_eq!(cs.gain_source(), GainSource::System);
        assert!((cs.gain() - 0.7).abs() < 0.001);
        // Manual command overrides system source.
        execute("volume 50", &cs);
        assert_eq!(cs.gain_source(), GainSource::Manual);
    }

    #[test]
    fn apply_gain_unity_is_no_op() {
        // Default state is gain=1.0; apply_gain must leave samples
        // untouched (no allocation, no copy, no multiply).
        let cs = ControlState::new();
        let original: Vec<f32> = (0..64).map(|i| (i as f32) * 0.01 - 0.5).collect();
        let mut buf = original.clone();
        cs.apply_gain(&mut buf);
        assert_eq!(buf, original, "unity gain must be a no-op");
    }

    #[test]
    fn apply_gain_multiplies_samples() {
        let cs = ControlState::new();
        cs.set_gain_with_source(0.5, GainSource::Manual);
        let mut buf: Vec<f32> = vec![1.0, 2.0, -1.0, 0.0];
        cs.apply_gain(&mut buf);
        assert!((buf[0] - 0.5).abs() < 1e-6);
        assert!((buf[1] - 1.0).abs() < 1e-6);
        assert!((buf[2] - -0.5).abs() < 1e-6);
        assert!(buf[3].abs() < 1e-6);
    }

    #[test]
    fn apply_gain_with_empty_buffer_is_safe() {
        // Empty slice should be a no-op even at non-unity gain
        // (early-return path tested).
        let cs = ControlState::new();
        cs.set_gain_with_source(2.0, GainSource::Manual);
        let mut buf: Vec<f32> = Vec::new();
        cs.apply_gain(&mut buf);
        assert!(buf.is_empty());
    }
}
