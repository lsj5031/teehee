//! The Rust half of the teehee GUI.
//!
//! TSX owns the screens; this crate owns the real engine: a sender that
//! captures Windows audio and ships teehee packets, a receiver that plays
//! them, and mDNS discovery. The class is deliberately JSON-shaped at the
//! boundary so the screens can be developed and tested without exposing
//! NSH engine types to TypeScript.

use std::io::ErrorKind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nsh_core::prelude::*;
use serde_json::json;
use teehee::audio_io::{AudioCapture, Capturer, Player, PlayerConfig};
use teehee::buffer_budget::compute_capacity_packets;
use teehee::capture_ring::CaptureRing;
use teehee::discovery::{self, DiscoveredReceiver};
use teehee::format_pipeline::FormatPipeline;
use teehee::generated::SineSource;
use teehee::jitter::JitterBuffer;
use teehee::mtu_budget::{compute_budget, exceeds_budget, validate_mtu};
use teehee::network::{Receiver, Sender};
use teehee::protocol::{Packet, HEADER_LEN, MAX_PAYLOAD_LEN};

#[native_module("teehee/native")]
fn native(registry: &mut Registry) {
    registry.class::<Engine>();
}

#[derive(Clone)]
struct SendTelemetry {
    phase: String,
    error: Option<String>,
    packets_sent: u64,
    packets_per_sec: f64,
    peak: f32,
    ring_ms: u64,
    drift_ppm: Option<f32>,
    fragmentations: u64,
    send_errors: u64,
    elapsed_ms: u64,
    sample_rate: u32,
    channels: u32,
    chunk_ms: u32,
    source: String,
}

impl Default for SendTelemetry {
    fn default() -> Self {
        Self {
            phase: "idle".into(),
            error: None,
            packets_sent: 0,
            packets_per_sec: 0.0,
            peak: 0.0,
            ring_ms: 0,
            drift_ppm: None,
            fragmentations: 0,
            send_errors: 0,
            elapsed_ms: 0,
            sample_rate: 48_000,
            channels: 2,
            chunk_ms: 3,
            source: "System audio".into(),
        }
    }
}

#[derive(Clone)]
struct RecvTelemetry {
    phase: String,
    error: Option<String>,
    packets_received: u64,
    packets_per_sec: f64,
    peak: f32,
    queued_ms: u64,
    late_drops: u64,
    underruns: u64,
    decode_errors: u64,
    elapsed_ms: u64,
    sample_rate: u32,
    channels: u32,
    drift_ppm: Option<f32>,
}

impl Default for RecvTelemetry {
    fn default() -> Self {
        Self {
            phase: "idle".into(),
            error: None,
            packets_received: 0,
            packets_per_sec: 0.0,
            peak: 0.0,
            queued_ms: 0,
            late_drops: 0,
            underruns: 0,
            decode_errors: 0,
            elapsed_ms: 0,
            sample_rate: 48_000,
            channels: 2,
            drift_ppm: None,
        }
    }
}

#[derive(Clone)]
struct ReceiverItem {
    id: String,
    name: String,
    host: String,
    port: u16,
    seen: String,
}

#[derive(Clone, Default)]
struct ReceiverState {
    searching: bool,
    error: Option<String>,
    items: Vec<ReceiverItem>,
}

struct EngineInner {
    send: Mutex<SendTelemetry>,
    recv: Mutex<RecvTelemetry>,
    receivers: Mutex<ReceiverState>,
    send_stop: AtomicBool,
    send_pause: AtomicBool,
    recv_stop: AtomicBool,
    recv_pause: AtomicBool,
}

impl EngineInner {
    fn new() -> Self {
        Self {
            send: Mutex::new(SendTelemetry::default()),
            recv: Mutex::new(RecvTelemetry::default()),
            receivers: Mutex::new(ReceiverState::default()),
            send_stop: AtomicBool::new(false),
            send_pause: AtomicBool::new(false),
            recv_stop: AtomicBool::new(false),
            recv_pause: AtomicBool::new(false),
        }
    }

    fn send(&self) -> MutexGuard<'_, SendTelemetry> {
        self.send.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn recv(&self) -> MutexGuard<'_, RecvTelemetry> {
        self.recv.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn receivers(&self) -> MutexGuard<'_, ReceiverState> {
        self.receivers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn set_send_error(&self, error: String) {
        let mut state = self.send();
        if state.phase != "idle" {
            state.phase = "error".into();
            state.error = Some(error);
            state.packets_per_sec = 0.0;
            state.peak = 0.0;
        }
    }

    fn set_recv_error(&self, error: String) {
        let mut state = self.recv();
        if state.phase != "idle" {
            state.phase = "error".into();
            state.error = Some(error);
            state.packets_per_sec = 0.0;
            state.peak = 0.0;
        }
    }
}

/// The real teehee engine, one instance per app.
#[class]
pub struct Engine {
    inner: Arc<EngineInner>,
    send_thread: Option<JoinHandle<()>>,
    recv_thread: Option<JoinHandle<()>>,
    discovery_thread: Option<JoinHandle<()>>,
}

#[class]
impl Engine {
    /// A stopped engine with no sessions.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(EngineInner::new()),
            send_thread: None,
            recv_thread: None,
            discovery_thread: None,
        }
    }

    /// Start sending to `target` (`host:port`).
    pub fn start_send(
        &mut self,
        target: String,
        source: String,
        chunk_ms: u32,
        mtu: u32,
        capture_buffer_ms: u32,
    ) -> Result<(), String> {
        if target.trim().is_empty() {
            return Err("Choose a receiver first.".into());
        }
        if !matches!(source.as_str(), "system" | "microphone" | "sine") {
            return Err(format!("Unknown source: {source}"));
        }
        if !(1..=100).contains(&chunk_ms) {
            return Err("Chunk size must be between 1 and 100 ms.".into());
        }
        validate_mtu(mtu as usize).map_err(|error| error.to_string())?;
        if !(40..=5_000).contains(&capture_buffer_ms) {
            return Err("Capture buffer must be between 40 and 5000 ms.".into());
        }

        self.stop_send()?;
        self.inner.send_stop.store(false, Ordering::SeqCst);
        self.inner.send_pause.store(false, Ordering::SeqCst);
        {
            let mut state = self.inner.send();
            *state = SendTelemetry {
                phase: "starting".into(),
                source: match source.as_str() {
                    "microphone" => "Microphone".into(),
                    "sine" => "440 Hz test tone".into(),
                    _ => "System audio".into(),
                },
                chunk_ms,
                ..SendTelemetry::default()
            };
        }

        let inner = Arc::clone(&self.inner);
        self.send_thread = Some(thread::spawn(move || {
            if let Err(error) = send_worker(
                Arc::clone(&inner),
                target,
                source,
                chunk_ms as usize,
                mtu as usize,
                capture_buffer_ms as usize,
            ) {
                inner.set_send_error(error);
            }
        }));
        Ok(())
    }

    /// Stop the sender and wait for its capture and network resources.
    pub fn stop_send(&mut self) -> Result<(), String> {
        self.inner.send_stop.store(true, Ordering::SeqCst);
        self.inner.send_pause.store(false, Ordering::SeqCst);
        if let Some(handle) = self.send_thread.take() {
            let _ = handle.join();
        }
        let mut state = self.inner.send();
        state.phase = "idle".into();
        state.error = None;
        state.packets_sent = 0;
        state.packets_per_sec = 0.0;
        state.peak = 0.0;
        state.ring_ms = 0;
        state.elapsed_ms = 0;
        state.fragmentations = 0;
        state.send_errors = 0;
        Ok(())
    }

    /// Pause or resume the sender. The capture stream keeps running; its
    /// ring is cleared while paused so resuming never replays stale audio.
    pub fn set_send_paused(&mut self, paused: bool) {
        self.inner.send_pause.store(paused, Ordering::SeqCst);
        let mut state = self.inner.send();
        if paused {
            if state.phase == "live" || state.phase == "starting" {
                state.phase = "paused".into();
                state.packets_per_sec = 0.0;
                state.peak = 0.0;
            }
        } else if state.phase == "paused" {
            state.phase = if state.packets_sent > 0 {
                "live".into()
            } else {
                "starting".into()
            };
        }
    }

    /// Listen for teehee packets on `port` and play them on the default output.
    pub fn start_recv(
        &mut self,
        port: u16,
        prebuffer_ms: u32,
        rx_buffer_ms: u32,
    ) -> Result<(), String> {
        if port < 1024 {
            return Err("Use a port between 1024 and 65535.".into());
        }
        if prebuffer_ms < 20 {
            return Err("Prebuffer must be at least 20 ms.".into());
        }
        if rx_buffer_ms < prebuffer_ms {
            return Err("Ring must be at least the prebuffer.".into());
        }
        teehee::buffer_budget::validate_buffer_ms(rx_buffer_ms as usize)
            .map_err(|error| error.to_string())?;

        self.stop_recv()?;
        self.inner.recv_stop.store(false, Ordering::SeqCst);
        self.inner.recv_pause.store(false, Ordering::SeqCst);
        {
            let mut state = self.inner.recv();
            *state = RecvTelemetry {
                phase: "starting".into(),
                ..RecvTelemetry::default()
            };
        }

        let inner = Arc::clone(&self.inner);
        self.recv_thread = Some(thread::spawn(move || {
            if let Err(error) = recv_worker(
                Arc::clone(&inner),
                port,
                prebuffer_ms as usize,
                rx_buffer_ms as usize,
            ) {
                inner.set_recv_error(error);
            }
        }));
        Ok(())
    }

    /// Stop the receiver and release its port and output stream.
    pub fn stop_recv(&mut self) -> Result<(), String> {
        self.inner.recv_stop.store(true, Ordering::SeqCst);
        self.inner.recv_pause.store(false, Ordering::SeqCst);
        if let Some(handle) = self.recv_thread.take() {
            let _ = handle.join();
        }
        let mut state = self.inner.recv();
        state.phase = "idle".into();
        state.error = None;
        state.packets_received = 0;
        state.packets_per_sec = 0.0;
        state.peak = 0.0;
        state.queued_ms = 0;
        state.elapsed_ms = 0;
        Ok(())
    }

    /// Pause or resume playback without closing the receive socket.
    pub fn set_recv_paused(&mut self, paused: bool) {
        self.inner.recv_pause.store(paused, Ordering::SeqCst);
        let mut state = self.inner.recv();
        if paused && state.phase == "live" {
            state.phase = "paused".into();
            state.peak = 0.0;
        } else if !paused && state.phase == "paused" {
            state.phase = "live".into();
        }
    }

    /// Start a short mDNS browse. Poll `status()` for the result.
    pub fn refresh_receivers(&mut self) {
        if self
            .discovery_thread
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
        {
            return;
        }
        if let Some(handle) = self.discovery_thread.take() {
            let _ = handle.join();
        }
        {
            let mut state = self.inner.receivers();
            state.searching = true;
            state.error = None;
        }
        let inner = Arc::clone(&self.inner);
        self.discovery_thread = Some(thread::spawn(move || {
            // Windows mDNS daemons can take a few seconds to bind, send, and
            // receive the first response. Give the UI one useful window.
            let result = discovery::discover_all(Duration::from_millis(4_000));
            let mut state = inner.receivers();
            state.searching = false;
            match result {
                Ok(found) => {
                    state.items = found.into_iter().map(receiver_item).collect();
                    state.error = None;
                }
                Err(error) => state.error = Some(error.to_string()),
            }
        }));
    }

    /// A JSON snapshot of both sessions and the discovered receivers.
    pub fn status(&self) -> String {
        let send = self.inner.send().clone();
        let recv = self.inner.recv().clone();
        let receivers = self.inner.receivers().clone();
        json!({
            "send": {
                "phase": send.phase,
                "error": send.error,
                "packets_sent": send.packets_sent,
                "packets_per_sec": send.packets_per_sec,
                "peak": send.peak,
                "ring_ms": send.ring_ms,
                "drift_ppm": send.drift_ppm,
                "fragmentations": send.fragmentations,
                "send_errors": send.send_errors,
                "elapsed_ms": send.elapsed_ms,
                "sample_rate": send.sample_rate,
                "channels": send.channels,
                "chunk_ms": send.chunk_ms,
                "source": send.source,
            },
            "recv": {
                "phase": recv.phase,
                "error": recv.error,
                "packets_received": recv.packets_received,
                "packets_per_sec": recv.packets_per_sec,
                "peak": recv.peak,
                "queued_ms": recv.queued_ms,
                "late_drops": recv.late_drops,
                "underruns": recv.underruns,
                "decode_errors": recv.decode_errors,
                "elapsed_ms": recv.elapsed_ms,
                "sample_rate": recv.sample_rate,
                "channels": recv.channels,
                "drift_ppm": recv.drift_ppm,
            },
            "receivers": {
                "searching": receivers.searching,
                "error": receivers.error,
                "items": receivers.items.iter().map(|item| json!({
                    "id": item.id,
                    "name": item.name,
                    "host": item.host,
                    "port": item.port,
                    "seen": item.seen,
                })).collect::<Vec<_>>(),
            },
        })
        .to_string()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.inner.send_stop.store(true, Ordering::SeqCst);
        self.inner.recv_stop.store(true, Ordering::SeqCst);
    }
}

fn receiver_item(found: DiscoveredReceiver) -> ReceiverItem {
    let host = found.host;
    let port = found.port;
    ReceiverItem {
        id: format!("{host}:{port}"),
        name: if found.name.is_empty() {
            host.clone()
        } else {
            found.name
        },
        host,
        port,
        seen: "found just now".into(),
    }
}

fn push_capture(ring: &Arc<Mutex<Option<CaptureRing>>>, data: &[f32]) {
    if let Ok(mut guard) = ring.lock() {
        if let Some(capture) = guard.as_mut() {
            capture.push(data);
        }
    }
}

fn pop_capture(ring: &Arc<Mutex<Option<CaptureRing>>>, samples: usize) -> Option<Vec<f32>> {
    ring.lock()
        .ok()
        .and_then(|mut guard| guard.as_mut().and_then(|capture| capture.pop_chunk(samples)))
}

fn ring_depth_ms(ring: &Arc<Mutex<Option<CaptureRing>>>, sample_rate: u32, channels: u8) -> u64 {
    let divisor = sample_rate as u64 * channels as u64;
    if divisor == 0 {
        return 0;
    }
    ring.lock()
        .ok()
        .and_then(|guard| {
            guard.as_ref().map(|capture| {
                capture.len() as u64 * 1_000 / divisor
            })
        })
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
struct TimerResolution;

#[cfg(target_os = "windows")]
impl TimerResolution {
    fn acquire() -> Self {
        unsafe {
            winmm::timeBeginPeriod(1);
        }
        Self
    }
}

#[cfg(target_os = "windows")]
impl Drop for TimerResolution {
    fn drop(&mut self) {
        unsafe {
            winmm::timeEndPeriod(1);
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(non_snake_case)]
mod winmm {
    #[link(name = "winmm")]
    unsafe extern "system" {
        pub fn timeBeginPeriod(period: u32) -> u32;
        pub fn timeEndPeriod(period: u32) -> u32;
    }
}

fn validate_send_chunk(
    sample_rate: u32,
    channels: u16,
    chunk_ms: usize,
) -> Result<(u8, usize, usize), String> {
    if sample_rate == 0 || channels == 0 {
        return Err("The capture device reported an empty format.".into());
    }
    let channels = u8::try_from(channels).map_err(|_| {
        format!(
            "The capture device reported {channels} channels, but the wire format supports up to {}. Choose a capture format with fewer channels.",
            u8::MAX
        )
    })?;

    let chunk_frames = (u128::from(sample_rate) * chunk_ms as u128 / 1000).max(1);
    let chunk_samples = chunk_frames * u128::from(channels);
    let payload_bytes = chunk_samples * std::mem::size_of::<f32>() as u128;
    let chunk_description = || format!("{chunk_ms} ms at {sample_rate} Hz with {channels} channels");
    let remedy = "Reduce the chunk duration or choose a lower sample rate or channel count.";
    if payload_bytes > MAX_PAYLOAD_LEN as u128 {
        return Err(format!(
            "A {} chunk needs {payload_bytes} PCM bytes, exceeding the {MAX_PAYLOAD_LEN}-byte wire payload limit. {remedy}",
            chunk_description()
        ));
    }

    const MAX_IPV4_UDP_PAYLOAD: usize =
        u16::MAX as usize - (teehee::mtu_budget::FRAMING_OVERHEAD_BYTES - HEADER_LEN);
    let datagram_bytes = HEADER_LEN + payload_bytes as usize;
    if datagram_bytes > MAX_IPV4_UDP_PAYLOAD {
        return Err(format!(
            "A {} chunk makes a {datagram_bytes}-byte packet, exceeding the {MAX_IPV4_UDP_PAYLOAD}-byte IPv4 UDP payload limit. {remedy}",
            chunk_description()
        ));
    }

    Ok((channels, chunk_frames as usize, chunk_samples as usize))
}

fn send_worker(
    inner: Arc<EngineInner>,
    target: String,
    source: String,
    chunk_ms: usize,
    mtu: usize,
    capture_buffer_ms: usize,
) -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    if source == "system" {
        return Err("System audio capture is only available on Windows. Choose Microphone or Test tone to use a different source.".into());
    }

    #[cfg(target_os = "windows")]
    let _timer = TimerResolution::acquire();

    let sender = Sender::connect(&target).map_err(|error| format!("connect {target}: {error}"))?;
    let ring: Arc<Mutex<Option<CaptureRing>>> = Arc::new(Mutex::new(None));
    let mut sine: Option<SineSource> = None;

    let callback = {
        let ring = Arc::clone(&ring);
        move |data: &[f32]| push_capture(&ring, data)
    };
    let (capturer, source_label): (Option<Box<dyn AudioCapture>>, String) = match source.as_str() {
        "sine" => {
            sine = Some(SineSource::new(48_000, 2, 440.0));
            (None, "440 Hz test tone".to_string())
        }
        "microphone" => {
            let capture = Capturer::open_default_input(callback)
                .map_err(|error| format!("open microphone: {error}"))?;
            let label = format!("Microphone · {}", capture.device_name());
            (Some(Box::new(capture)), label)
        }
        #[cfg(target_os = "windows")]
        "system" => {
            let capture = teehee::LoopbackCapturer::open_default(callback)
                .map_err(|error| format!("open system audio: {error}"))?;
            (
                Some(Box::new(capture)),
                "System audio · WASAPI loopback".to_string(),
            )
        }
        _ => return Err(format!("Unknown source: {source}")),
    };
    let (sample_rate, channels) = capturer.as_ref().map_or((48_000, 2), |capture| {
        let config = capture.config();
        (config.sample_rate, config.channels)
    });
    let (channels, chunk_frames, chunk_samples) = validate_send_chunk(sample_rate, channels, chunk_ms)?;
    let budget = compute_budget(mtu, channels).map_err(|error| error.to_string())?;

    if capturer.is_some() {
        *ring.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(CaptureRing::new(
            capture_buffer_ms,
            sample_rate,
            channels,
            chunk_samples,
        ));
    }

    let mut chunk = vec![0.0_f32; chunk_samples];
    let period = Duration::from_nanos(
        ((chunk_frames as u64 * 1_000_000_000) / sample_rate as u64).max(1),
    );

    {
        let mut state = inner.send();
        state.phase = "starting".into();
        state.error = None;
        state.sample_rate = sample_rate;
        state.channels = channels as u32;
        state.chunk_ms = chunk_ms as u32;
        state.source = source_label.clone();
    }

    let started = Instant::now();
    let mut next_tick = Instant::now();
    let mut last_report = Instant::now();
    let mut last_packets = 0_u64;
    let mut packets = 0_u64;
    let mut errors = 0_u64;
    let mut fragmentations = 0_u64;
    let mut sequence = 0_u32;
    let mut peak;

    while !inner.send_stop.load(Ordering::SeqCst) {
        if inner.send_pause.load(Ordering::SeqCst) {
            if let Ok(mut guard) = ring.lock() {
                if let Some(capture) = guard.as_mut() {
                    capture.clear();
                }
            }
            {
                let mut state = inner.send();
                if state.phase != "paused" {
                    state.phase = "paused".into();
                    state.packets_per_sec = 0.0;
                    state.peak = 0.0;
                }
            }
            thread::sleep(Duration::from_millis(20));
            next_tick = Instant::now();
            continue;
        }

        if let Some(source) = sine.as_mut() {
            source.fill_chunk(&mut chunk);
        } else {
            match pop_capture(&ring, chunk_samples) {
                Some(captured) => chunk = captured,
                None => chunk.fill(0.0),
            }
        }

        peak = chunk.iter().fold(0.0_f32, |current, sample| {
            current.max(sample.abs())
        });
        let packet = Packet::new(
            sequence,
            sequence as u64 * chunk_frames as u64,
            sample_rate,
            channels,
            &chunk,
        );
        let encoded = packet.encode();
        if exceeds_budget(encoded.len(), &budget) {
            fragmentations = fragmentations.saturating_add(1);
        }

        match sender.send(&encoded) {
            Ok(_) => {
                packets = packets.saturating_add(1);
                if packets == 1 {
                    let mut state = inner.send();
                    state.phase = "live".into();
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused | ErrorKind::WouldBlock
                ) =>
            {
                errors = errors.saturating_add(1);
            }
            Err(error) => return Err(format!("send: {error}")),
        }
        sequence = sequence.wrapping_add(1);

        if last_report.elapsed() >= Duration::from_millis(200) {
            let elapsed = last_report.elapsed().as_secs_f64().max(0.001);
            let rate = (packets.saturating_sub(last_packets)) as f64 / elapsed;
            let depth = ring_depth_ms(&ring, sample_rate, channels);
            {
                let mut state = inner.send();
                state.packets_sent = packets;
                state.packets_per_sec = rate;
                state.peak = peak;
                state.ring_ms = depth;
                state.fragmentations = fragmentations;
                state.send_errors = errors;
                state.elapsed_ms = started.elapsed().as_millis() as u64;
                state.drift_ppm = None;
            }
            last_report = Instant::now();
            last_packets = packets;
        }

        next_tick += period;
        let now = Instant::now();
        if next_tick > now {
            thread::sleep(next_tick - now);
        } else if now.duration_since(next_tick) > Duration::from_millis(200) {
            next_tick = now;
        }
    }

    Ok(())
}

#[cfg(test)]
mod sender_tests {
    use super::*;

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn system_audio_requires_windows() {
        let error = send_worker(
            Arc::new(EngineInner::new()),
            "invalid target".into(),
            "system".into(),
            3,
            1500,
            100,
        )
        .unwrap_err();
        assert!(error.contains("System audio capture is only available on Windows"));
    }

    #[test]
    fn rejects_empty_or_unrepresentable_capture_formats() {
        for (sample_rate, channels) in [(0, 2), (48_000, 0)] {
            assert!(
                validate_send_chunk(sample_rate, channels, 3)
                    .unwrap_err()
                    .contains("empty format")
            );
        }

        let error = validate_send_chunk(48_000, 257, 3).unwrap_err();
        assert!(error.contains("257 channels"));
        assert!(error.contains("255"));
    }

    #[test]
    fn sizes_chunks_in_whole_frames() {
        assert_eq!(validate_send_chunk(44_100, 2, 3), Ok((2, 132, 264)));
        assert_eq!(validate_send_chunk(999, 1, 1), Ok((1, 1, 1)));
    }

    #[test]
    fn rejects_chunks_above_wire_payload_limit() {
        let error = validate_send_chunk(16_384, 1, 1000).unwrap_err();
        assert!(error.contains("65536 PCM bytes"));
        assert!(error.contains("65535-byte wire payload limit"));
        assert!(error.contains("Reduce the chunk duration"));
    }

    #[test]
    fn rejects_chunks_that_fit_the_wire_but_exceed_ipv4_udp() {
        assert_eq!(
            validate_send_chunk(16_370, 1, 1000),
            Ok((1, 16_370, 16_370))
        );
        let error = validate_send_chunk(16_371, 1, 1000).unwrap_err();
        assert!(error.contains("65509-byte packet"));
        assert!(error.contains("65507-byte IPv4 UDP payload limit"));
        assert!(error.contains("Reduce the chunk duration"));
    }
}

struct RxState {
    jb: JitterBuffer,
    pipeline: FormatPipeline,
    samples_per_packet: usize,
    input_rate: u32,
    input_channels: u8,
    scratch: Vec<f32>,
}

fn recv_worker(
    inner: Arc<EngineInner>,
    port: u16,
    prebuffer_ms: usize,
    rx_buffer_ms: usize,
) -> Result<(), String> {
    let socket = Receiver::bind(("0.0.0.0", port))
        .map_err(|error| format!("bind UDP {port}: {error}"))?;
    let _advertiser = discovery::Advertiser::advertise(port, "teehee-gui", "teehee.local.").ok();
    let output_config: Arc<OnceLock<PlayerConfig>> = Arc::new(OnceLock::new());
    let state: Arc<Mutex<Option<RxState>>> = Arc::new(Mutex::new(None));

    let player = {
        let state = Arc::clone(&state);
        let output_config = Arc::clone(&output_config);
        let inner = Arc::clone(&inner);
        Player::open_default_output(move |data: &mut [f32]| {
            let Some(config) = output_config.get().copied() else {
                data.fill(0.0);
                return;
            };
            let Ok(mut guard) = state.lock() else {
                data.fill(0.0);
                return;
            };
            let Some(rx) = guard.as_mut() else {
                data.fill(0.0);
                return;
            };
            if inner.recv_pause.load(Ordering::Relaxed) {
                rx.jb.pop_frames(data);
                data.fill(0.0);
                return;
            }

            let out_channels = config.channels as usize;
            let out_frames = data.len() / out_channels.max(1);
            if rx.pipeline.is_passthrough() {
                rx.jb.pop_frames(data);
                return;
            }

            let mut written = rx.pipeline.drain(data);
            let mut passes = 0_u32;
            while written < out_frames && passes < 8 {
                let needed_frames = out_frames - written;
                let input_frames = ((needed_frames as u64 * rx.input_rate as u64)
                    .div_ceil(config.sample_rate as u64))
                    .max(1) as usize;
                let input_samples = input_frames * rx.input_channels as usize;
                if rx.scratch.len() < input_samples {
                    rx.scratch.resize(input_samples, 0.0);
                }
                rx.jb.pop_frames(&mut rx.scratch[..input_samples]);
                rx.pipeline.feed(&rx.scratch[..input_samples]);
                written += rx.pipeline.drain(&mut data[written * out_channels..]);
                passes += 1;
            }
            for sample in &mut data[written * out_channels..] {
                *sample = 0.0;
            }
        })
        .map_err(|error| format!("open output: {error}"))?
    };
    let config = player.config();
    let _ = output_config.set(config);
    {
        let mut recv = inner.recv();
        recv.sample_rate = config.sample_rate;
        recv.channels = config.channels as u32;
    }

    receive_packets(&inner, &socket, &state, config, prebuffer_ms, rx_buffer_ms)
}

fn receive_packets(
    inner: &EngineInner,
    socket: &Receiver,
    state: &Mutex<Option<RxState>>,
    output: PlayerConfig,
    prebuffer_ms: usize,
    rx_buffer_ms: usize,
) -> Result<(), String> {
    let mut packet_buffer = vec![0_u8; HEADER_LEN + MAX_PAYLOAD_LEN];
    let mut samples = Vec::new();
    let started = Instant::now();
    let mut last_report = Instant::now();
    let mut last_packets = 0_u64;
    let mut packets = 0_u64;
    let mut decode_errors = 0_u64;
    let mut peak = 0.0_f32;

    while !inner.recv_stop.load(Ordering::SeqCst) {
        match socket.recv_timeout(&mut packet_buffer, Duration::from_millis(100)) {
            Ok(Some(length)) => 'packet: {
                let packet = match Packet::decode(&packet_buffer[..length]) {
                    Ok(packet) => packet,
                    Err(_) => {
                        decode_errors = decode_errors.saturating_add(1);
                        break 'packet;
                    }
                };
                packet.pcm_f32_into(&mut samples);
                let mut guard = state.lock().unwrap_or_else(|poison| poison.into_inner());
                let needs_init = match guard.as_ref() {
                    None => true,
                    Some(rx) => {
                        rx.samples_per_packet != samples.len()
                            || rx.input_rate != packet.sample_rate
                            || rx.input_channels != packet.channels
                    }
                };
                if needs_init {
                    let Ok(capacity_packets) = compute_capacity_packets(
                        rx_buffer_ms,
                        prebuffer_ms,
                        packet.sample_rate,
                        packet.channels,
                        samples.len(),
                    ) else {
                        decode_errors = decode_errors.saturating_add(1);
                        break 'packet;
                    };
                    let samples_per_ms = packet.sample_rate as usize * packet.channels as usize;
                    let prebuffer_frames = prebuffer_ms * samples_per_ms / 1000;
                    let capacity_frames = rx_buffer_ms * samples_per_ms / 1000;
                    let high_water = (prebuffer_ms + 100)
                        .saturating_mul(samples_per_ms)
                        / 1000;
                    *guard = Some(RxState {
                        jb: JitterBuffer::with_high_water(
                            samples.len(),
                            capacity_packets,
                            Some(prebuffer_frames.max(1)),
                            Some(high_water.min(capacity_frames).max(prebuffer_frames).max(1)),
                        ),
                        pipeline: FormatPipeline::new(
                            packet.sample_rate,
                            output.sample_rate,
                            packet.channels,
                            output.channels as u8,
                        ),
                        samples_per_packet: samples.len(),
                        input_rate: packet.sample_rate,
                        input_channels: packet.channels,
                        scratch: Vec::new(),
                    });
                    let mut recv = inner.recv();
                    recv.sample_rate = packet.sample_rate;
                    recv.channels = packet.channels as u32;
                }

                peak = samples.iter().fold(peak, |current, sample| current.max(sample.abs()));
                if let Some(rx) = guard.as_mut() {
                    if samples.len() == rx.samples_per_packet {
                        let _ = rx.jb.push(packet.sequence, &samples);
                    }
                }
                packets = packets.saturating_add(1);
                if packets == 1 {
                    let mut recv = inner.recv();
                    recv.phase = "live".into();
                }
            }
            Ok(None) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused | ErrorKind::WouldBlock
                ) => {}
            Err(error) => return Err(format!("receive: {error}")),
        }

        let now = Instant::now();
        let interval = now.duration_since(last_report);
        if interval >= Duration::from_millis(200) {
            let guard = state.lock().unwrap_or_else(|poison| poison.into_inner());
            let queued_ms = guard.as_ref().map_or(0, |rx| {
                rx.jb.queued_frames() as u64 * 1_000
                    / (u64::from(rx.input_rate) * u64::from(rx.input_channels))
            });
            let stats = guard.as_ref().map(|rx| rx.jb.stats()).unwrap_or_default();
            drop(guard);
            let mut recv = inner.recv();
            recv.packets_received = packets;
            recv.packets_per_sec = (packets - last_packets) as f64 / interval.as_secs_f64();
            recv.peak = if inner.recv_pause.load(Ordering::Relaxed) { 0.0 } else { peak };
            recv.queued_ms = queued_ms;
            recv.late_drops = stats.late_drops;
            recv.underruns = stats.underrun_resyncs;
            recv.decode_errors = decode_errors;
            recv.elapsed_ms = now.duration_since(started).as_millis() as u64;
            last_report = now;
            last_packets = packets;
            peak = 0.0;
        }
    }

    Ok(())
}

#[cfg(test)]
mod receive_tests {
    use super::*;
    use teehee::audio_io::CapturedSampleFormat;

    #[test]
    fn quiet_socket_reports_drained_queue_and_elapsed_time() {
        let socket = Receiver::bind(("127.0.0.1", 0)).unwrap();
        let sender = Sender::connect(socket.local_addr().unwrap()).unwrap();
        let inner = Arc::new(EngineInner::new());
        let state = Arc::new(Mutex::new(None));
        let output = PlayerConfig {
            sample_rate: 48_000,
            channels: 1,
            sample_format: CapturedSampleFormat::F32,
        };
        let packet = Packet::new(0, 0, 48_000, 1, &[0.5; 960]);
        sender.send(&packet.encode()).unwrap();
        let worker = {
            let inner = Arc::clone(&inner);
            let state = Arc::clone(&state);
            thread::spawn(move || receive_packets(&inner, &socket, &state, output, 20, 100))
        };

        let deadline = Instant::now() + Duration::from_secs(3);
        while inner.recv().queued_ms == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let before = inner.recv().clone();
        if let Some(rx) = state.lock().unwrap().as_mut() {
            rx.jb.pop_frames(&mut [0.0; 960]);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while inner.recv().queued_ms > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        inner.recv_stop.store(true, Ordering::SeqCst);
        worker.join().unwrap().unwrap();
        let after = inner.recv();
        assert_eq!(before.queued_ms, 20);
        assert_eq!(after.queued_ms, 0);
        assert_eq!(after.peak, 0.0);
        assert_eq!(after.packets_received, 1);
        assert_eq!(after.packets_per_sec, 0.0);
        assert!(after.elapsed_ms > before.elapsed_ms);
    }
}
