//! `teehee` CLI binary — wires the seven library modules into a single
//! user-facing tool with three subcommands.
//!
//! Pipeline topology:
//!
//! * `send` — capture (or generate) audio → chunk into packets →
//!   encode via `protocol::Packet::encode` → `network::Sender::send`
//!   on a dedicated thread.
//! * `recv` — `network::Receiver::bind` + a decoder thread that
//!   decodes via `protocol::Packet::decode` and pushes into a shared
//!   `jitter::JitterBuffer`. The main thread opens a cpal
//!   `audio_io::Player` whose data callback drains the jitter buffer.
//! * `devices` — print `audio_io::list_input_devices` /
//!   `list_output_devices` for cpal device introspection.
//!
//! The cpal callback's `data: &mut [f32]` size is determined by the
//! audio endpoint (OS-picked, often a power of two like 256/512/1024).
//! `JitterBuffer::pop_frames` already zero-pads the tail when the
//! request doesn't align with a full packet — so this all wires up
//! without a custom resampler.
//!
//! Slice 7 wire-up: when the receiver's cpal default output device
//! opens at a different sample rate or channel count than the
//! sender's input device, a `format_pipeline::FormatPipeline`
//! reconciles the two formats between the JitterBuffer pop and the
//! cpal data callback. PlayerConfig is extracted from the cpal
//! stream at open time and paired with the sender-side format on
//! first packet arrival to lazily initialize both pieces.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use tracing::{info, warn};

use teehee::audio_io;
use teehee::audio_io::{AudioCapture, PlayerConfig};
use teehee::buffer_budget::compute_capacity_packets;
use teehee::cli::{CaptureSource, Cli, Command, RecvArgs, SendArgs};
use teehee::format_pipeline::FormatPipeline;
use teehee::generated::SineSource;
use teehee::jitter::JitterBuffer;
use teehee::mtu_budget::compute_budget;
use teehee::network::{Receiver, Sender};
use teehee::protocol::{DecodeStats, Packet, HEADER_LEN};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Send(args) => run_send(&args),
        Command::Recv(args) => run_recv(&args),
        Command::Devices => run_devices(),
    }
}

/// Sender pipeline. Choose capture path based on `args.sine`.
fn run_send(args: &SendArgs) -> anyhow::Result<()> {
    // Validate parses positional vs --host and `host:port` ↔ `--port`
    // collisions BEFORE we open any cpal stream or socket; this keeps
    // the user's typo or ambiguity at startup noise level, not a
    // hard-to-debug mid-stream error.
    let target = args.validate().map_err(anyhow::Error::msg)?;
    let sample_rate = args.sample_rate;
    let channels = args.channels;
    let chunk_ms = args.chunk_ms;
    let chunk_frames = (sample_rate as usize) * chunk_ms / 1000;
    // `chunk_samples` (= chunk_frames * channels) is computed inline at
    // the two use-sites below; no need for a top-level binding here.

    let target_str = target.to_socket_string();
    info!(target = %target_str, sample_rate, channels, chunk_ms, mtu = args.mtu, "teehee send connecting");
    let tx = Sender::connect(&target_str)?;

    let packets_sent = Arc::new(AtomicU64::new(0));
    // Slice 9 MTU strategy: per-packet fragmentations counter. The
    // sender does NOT clamp chunk_ms — it always sends the encoded
    // payload, even if the resulting datagram overshoots the link
    // MTU. The OS handles IP-layer fragmentation transparently;
    // this counter lets the operator see (via `--stats`) how often
    // it happens. A non-zero rate means the chunk_ms × audio-params
    // combination is larger than the envelope, and a `--mtu` bump
    // or `--chunk-ms` lower is warranted.
    let fragmentations = Arc::new(AtomicU64::new(0));

    if args.sine {
        // --sine mode: encode loop runs on a dedicated thread; budget
        // computation lives on the worker because chunk_frames/cfg
        // here are the only authoritative ones (no device to
        // negotiate with, contrary to the real-capture branch below).
        let budget = compute_budget(args.mtu, channels)?;
        let requested_chunk_samples = chunk_frames * channels as usize;
        let requested_payload_bytes = HEADER_LEN + requested_chunk_samples * 4;
        info!(
            mtu_bytes = args.mtu,
            max_payload_bytes = budget.max_payload_bytes,
            max_chunk_samples = budget.max_chunk_samples,
            requested_chunk_ms = chunk_ms,
            requested_chunk_samples,
            requested_payload_bytes,
            fits_in_mtu = requested_payload_bytes <= budget.max_payload_bytes,
            "teehee send (--sine): MTU budget anchored; packets larger than \
             max_payload_bytes will be counted as fragmentations, not \
             rejected"
        );
        if requested_payload_bytes > budget.max_payload_bytes {
            // CAREFUL #1 (slice 9 review): elevate startup log to
            // warn! when the configured chunk-ms × audio-params combo
            // is known to fragment on this MTU. The user can fix
            // this by raising --mtu (jumbo on a 9000-B LAN) or
            // lowering --chunk-ms (5-ms chunks fit at 1500-B MTU).
            let max_chunk_ms =
                (budget.max_chunk_samples * 1000) / (sample_rate as usize * channels as usize);
            warn!(
                packet_bytes = requested_payload_bytes,
                max_payload_bytes = budget.max_payload_bytes,
                max_fitting_chunk_ms = max_chunk_ms,
                "configured --chunk-ms sends packets larger than the MTU \
                 envelope; OS IP-fragmentation will occur for every packet \
                 (--stats: fragmentation counter). To avoid fragmentation \
                 either lower --chunk-ms to {max_chunk_ms} or raise --mtu to \
                 a larger value (e.g. 9000 for jumbo)."
            );
        }
        let ps = Arc::clone(&packets_sent);
        let frag = Arc::clone(&fragmentations);
        let max_payload = budget.max_payload_bytes;
        // Dry-run: pure SineSource, no audio_io. Sleep chunk_ms between sends.
        let handle = thread::spawn(move || -> anyhow::Result<()> {
            let mut sine = SineSource::new(sample_rate, channels, 440.0);
            let mut chunk_buf = vec![0.0_f32; chunk_frames * channels as usize];
            let period = Duration::from_millis(chunk_ms as u64);
            let mut seq: u32 = 0;
            let mut next_tick = Instant::now();
            loop {
                sine.fill_chunk(&mut chunk_buf);
                let frame_ts = (seq as u64) * chunk_frames as u64;
                let pkt = Packet::new(seq, frame_ts, sample_rate, channels, &chunk_buf);
                let encoded = pkt.encode();
                // Slice 9: per-packet MTU fragment-on-overrun accounting.
                // The OS will IP-fragment if encoded.len() exceeds the
                // envelope; we count the event instead of dropping.
                if encoded.len() > max_payload {
                    frag.fetch_add(1, Ordering::Relaxed);
                }
                tx.send(&encoded)?;
                ps.fetch_add(1, Ordering::Relaxed);
                seq = seq.wrapping_add(1);

                next_tick += period;
                let now = Instant::now();
                if next_tick > now {
                    thread::sleep(next_tick - now);
                } else {
                    next_tick = now;
                }
            }
        });

        if args.stats {
            spawn_periodic_sender_stats(
                Arc::clone(&packets_sent),
                args.mtu,
                budget.max_payload_bytes,
                Arc::clone(&fragmentations),
            );
        }
        // Block until the worker errors out (in practice: never on a real LAN).
        handle.join().ok();
        Ok(())
    } else {
        // Real capture: cpal (default input) or WASAPI loopback fills
        // a shared ring buffer; we pull chunks off it on the main
        // thread and encode. Slice 8 routes the capturer selection
        // through `--capture-source` (`default` → cpal input device,
        // `loopback` → Windows WASAPI render endpoint with
        // AUDCLNT_STREAMFLAGS_LOOPBACK); both paths produce a capturer
        // implementing the shared [`AudioCapture`] trait so the
        // downstream encode loop is agnostic.
        //
        // Note: the `chunk_samples` (and `chunk_frames`,
        // `sample_rate`, `channels`) bindings are shadowed below from
        // `capturer.config()` — they must NOT be computed from the
        // CLI args here, because the device's actual format may
        // differ. The CLI values are only authoritative in `--sine`
        // mode (above), which has no device.
        //
        // Ring is allocated empty; the cpal/WASAPI callback grows the
        // buffer naturally via extend_from_slice. Pre-warming capacity
        // with `chunk_samples` here would require knowing the
        // device's actual sample_rate + channels before the capturer
        // opens — not possible. The first few callbacks reallocate
        // 1-2 times before stabilizing; perf is fine.
        let ring: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let ring_for_capture = Arc::clone(&ring);

        // Build the capturer per `--capture-source`. Both arms
        // produce a capturer implementing [`AudioCapture`]; storing
        // it in `Box<dyn AudioCapture>` lets `_capturer_keepalive`
        // hold either type without a third-crate Sum-implementing
        // type. The closure `ring_cb` is consumed by whichever
        // branch runs — exclusive moves, no cloning required.
        let ring_cb = move |data: &[f32]| {
            let mut buf = ring_for_capture.lock().unwrap();
            buf.extend_from_slice(data);
        };
        let capturer: Box<dyn AudioCapture>;
        let capturer_source_label: &'static str;
        match args.capture_source {
            CaptureSource::Default => {
                let cb = ring_cb;
                let cap = audio_io::Capturer::open_default_input(cb)?;
                capturer_source_label = "cpal default input";
                capturer = Box::new(cap);
            }
            CaptureSource::Loopback => {
                let cb = ring_cb;
                let cap = audio_io::LoopbackCapturer::open_default(cb)?;
                capturer_source_label = "WASAPI loopback";
                capturer = Box::new(cap);
            }
            CaptureSource::Auto => {
                // `ring_cb` is consumed by `open_default_input`
                // (cpal moves the closure into the audio-thread
                // context); the auto helper needs to construct
                // fresh closures for *each* of its two attempts
                // (cpal-default and WASAPI-loopback-fallback) so
                // we bridge into the factory-shape by rebuilding
                // `ring_cb` with a fresh `Arc` clone each time.
                //
                // The state the closures mutate is the same
                // `Arc<Mutex<Vec<f32>>>` (captured above); every
                // fresh closure clones the same shared ring, so
                // both attempts write into the same encode-loop
                // drain.
                //
                // The `ring` binding is already in scope at this
                // point (the run_send outer `ring: Arc<Mutex<Vec<f32>>>`
                // captured the original state for the cpal
                // callback); we just re-bind a fresh clone-witness
                // thread-closure factory that the auto helper can
                // invoke twice. Note: this closure-factory shape
                // avoids the `Arc<dyn FnMut + Send>` dispatch
                // cost on the audio thread's hot path — every
                // callback is a static, monomorphized
                // `move |data| { ... }` body, identical to the
                // v1 / slice-8 path.
                let ring_for_factory = Arc::clone(&ring);
                let make_cb = move || {
                    let ring_inner = Arc::clone(&ring_for_factory);
                    move |data: &[f32]| {
                        let mut buf = ring_inner.lock().unwrap();
                        buf.extend_from_slice(data);
                    }
                };
                let (cap, label) = audio_io::open_auto_input(make_cb)?;
                capturer_source_label = label;
                capturer = cap;
            }
        }
        // Slice 3 fix: the CLI bindings `sample_rate`, `channels`,
        // `chunk_frames`, `chunk_samples` are SHADOWED here from the
        // device's actual format. Packet metadata (`Packet::new`'s
        // third and fourth args) and chunk math below now use this
        // device-actual pair, not the CLI defaults. The `--sine`
        // (dry-run) branch above keeps the CLI bindings because
        // there's no physical device to negotiate with.
        let dev_cfg = capturer.config();
        let sample_rate = dev_cfg.sample_rate;
        let channels = dev_cfg.channels as u8;
        let chunk_frames = (sample_rate as usize) * chunk_ms / 1000;
        let chunk_samples = chunk_frames * channels as usize;
        // Slice 9 MTU strategy: compute the per-sender budget from
        // device-actual channels (CLI --channels may diverge from
        // what the device opened at). If the budget can't fit even
        // one f32 frame, surface a clear error so the user knows
        // the device + MTU combination is impossible.
        let budget = compute_budget(args.mtu, channels)?;
        let requested_payload_bytes = HEADER_LEN + chunk_samples * 4;
        // Surface CLI/device divergence explicitly so a user passing
        // --sample-rate=48000 --channels=2 against a 44100/1 mic can
        // see their CLI args were silently dropped (and not assume
        // the samplerate knob is broken). Each divergence is logged
        // independently with both `requested` and `actual` values.
        if args.sample_rate != dev_cfg.sample_rate {
            info!(
                requested_sample_rate = args.sample_rate,
                actual_sample_rate = dev_cfg.sample_rate,
                "--sample-rate ignored: cpal device format wins for real capture"
            );
        }
        if args.channels != dev_cfg.channels as u8 {
            info!(
                requested_channels = args.channels,
                actual_channels = dev_cfg.channels,
                "--channels ignored: cpal device format wins for real capture"
            );
        }
        info!(
            device_sample_rate = sample_rate,
            device_channels = channels,
            source = capturer_source_label,
            chunk_ms,
            mtu_bytes = args.mtu,
            max_payload_bytes = budget.max_payload_bytes,
            max_chunk_samples = budget.max_chunk_samples,
            requested_chunk_samples = chunk_samples,
            requested_payload_bytes,
            fits_in_mtu = requested_payload_bytes <= budget.max_payload_bytes,
            "teehee send: capture source opened; using device's actual format \
             (CLI --sample-rate/--channels apply only to --sine dry-run)"
        );
        if requested_payload_bytes > budget.max_payload_bytes {
            // CAREFUL #1 (slice 9 review): elevate startup log to
            // warn! when the configured chunk-ms × device-actual
            // channels combo is known to fragment on this MTU. The
            // user can fix by raising --mtu or lowering --chunk-ms.
            let max_chunk_ms =
                (budget.max_chunk_samples * 1000) / (sample_rate as usize * channels as usize);
            warn!(
                packet_bytes = requested_payload_bytes,
                max_payload_bytes = budget.max_payload_bytes,
                max_fitting_chunk_ms = max_chunk_ms,
                "configured --chunk-ms sends packets larger than the MTU \
                 envelope at {channels} ch {sample_rate} Hz; OS IP-fragmentation \
                 will occur for every packet (--stats: fragmentation counter). \
                 To avoid fragmentation either lower --chunk-ms to {max_chunk_ms} \
                 or raise --mtu to a larger value (e.g. 9000 for jumbo)."
            );
        }
        let max_payload = budget.max_payload_bytes;

        if args.stats {
            spawn_periodic_sender_stats(
                Arc::clone(&packets_sent),
                args.mtu,
                budget.max_payload_bytes,
                Arc::clone(&fragmentations),
            );
        }

        // Encode loop runs on the main thread because cpal's data
        // callback must stay fast and we already drain on the main.
        let ps = Arc::clone(&packets_sent);
        let mut seq: u32 = 0;
        let period = Duration::from_millis(chunk_ms as u64);
        let mut next_tick = Instant::now();
        loop {
            let chunk = {
                let mut buf = ring.lock().unwrap();
                if buf.len() < chunk_samples {
                    // Not enough captured audio yet; wait briefly and try.
                    drop(buf);
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                buf.drain(..chunk_samples).collect::<Vec<f32>>()
            };
            let frame_ts = (seq as u64) * chunk_frames as u64;
            let pkt = Packet::new(seq, frame_ts, sample_rate, channels, &chunk);
            let encoded = pkt.encode();
            // Slice 9: per-packet MTU fragment-on-overrun accounting.
            // The OS will IP-fragment if encoded.len() exceeds the
            // envelope; we count the event instead of dropping so the
            // operator can spot the misconfiguration via --stats.
            if encoded.len() > max_payload {
                fragmentations.fetch_add(1, Ordering::Relaxed);
            }
            tx.send(&encoded)?;
            ps.fetch_add(1, Ordering::Relaxed);
            seq = seq.wrapping_add(1);

            next_tick += period;
            let now = Instant::now();
            if next_tick > now {
                thread::sleep(next_tick - now);
            } else {
                next_tick = now;
            }
        }
    }
}

/// Periodic stats reporter for the sender. Emits a `teehee send stats`
/// line every second — `packets_sent`, `packets_per_sec`, plus, in
/// slice 9, the configured `--mtu`, the derived `max_payload_bytes`,
/// and a cumulative `fragmentations` count of packets that
/// overshot the envelope (the OS IP-fragmented them transparently,
/// but a non-zero count surfaces the misconfiguration that caused
/// it).
fn spawn_periodic_sender_stats(
    packets_sent: Arc<AtomicU64>,
    mtu_bytes: usize,
    max_payload_bytes: usize,
    fragmentations: Arc<AtomicU64>,
) {
    thread::spawn(move || {
        let mut last: u64 = 0;
        let mut last_at = Instant::now();
        loop {
            thread::sleep(Duration::from_secs(1));
            let now = Instant::now();
            let n = packets_sent.load(Ordering::Relaxed);
            let frag = fragmentations.load(Ordering::Relaxed);
            let elapsed = now.duration_since(last_at).as_secs_f64();
            let rate = if elapsed > 0.0 {
                (n - last) as f64 / elapsed
            } else {
                0.0
            };
            info!(
                packets_sent = n,
                packets_per_sec = format!("{rate:.1}"),
                mtu_bytes,
                max_payload_bytes,
                fragmentations = frag,
                "teehee send stats"
            );
            last = n;
            last_at = now;
        }
    });
}

/// Slice 7 receive-state. Initialized lazily after the cpal Player
/// opens (we know output format) AND the first packet arrives (we
/// know input format). The cpal callback and the decoder thread
/// share `rx_state` exclusively via a Mutex — never both touching
/// state at the same instant.
struct RxState {
    /// Receive-side reorder buffer with slice-6 prebuffer gate.
    jb: JitterBuffer,
    /// Slice 7 sample-rate + channel reconciliation. Pure-Rust
    /// LinearResampler + ChannelMixer composition; sits between
    /// [`JitterBuffer::pop_frames`] and the cpal data callback.
    pipeline: FormatPipeline,
    /// Sender-side sample rate captured on first packet; used by
    /// the cpal callback to size the scratch buffer.
    input_rate_hz: u32,
    /// Sender-side channel count captured on first packet; same
    /// use as above. `u8` to match the wire protocol's channels
    /// byte — `FormatPipeline` takes `u8` for channels too.
    input_channels: u8,
    /// Reusable scratch buffer for `jb.pop_frames` output (input
    /// rate × input channels interleaved f32). Grown lazily — the
    /// cpal callback fires every ~10 ms and the worst-case
    /// amplification factor is the rate-and-channel ratio of
    /// (input × output_channels output-rate) / (output × input_channels
    /// input-rate). For LAN ratios this is < 4×, so the first
    /// callback reaches steady-state in 1–2 reallocations.
    scratch: Vec<f32>,
}

/// Receiver pipeline. Bounded to a single, default output device for
/// v1.
fn run_recv(args: &RecvArgs) -> anyhow::Result<()> {
    // Cross-flag validation BEFORE the cpal Player / UDP socket
    // open: --rx-buffer-ms must be >= --prebuffer-ms (slice 10
    // invariant). clap can range-check each flag independently but
    // can't compare two flags — surfaced here at start-up so a
    // misconfigured pair (e.g. a typo in one of the flags) halts
    // the binary, not the audio stream later.
    args.validate().map_err(anyhow::Error::msg)?;
    let addr = ("0.0.0.0", args.port);
    let rx = Receiver::bind(addr)?;
    info!(
        local_addr = ?rx.local_addr()?,
        port = args.port,
        prebuffer_ms = args.prebuffer_ms,
        rx_buffer_ms = args.rx_buffer_ms,
        "teehee recv listening"
    );

    // Slice 6: prebuffer gate target is `--prebuffer-ms` translated
    // into INPUT-rate × INPUT-channels unit samples. Slice 10: ring
    // depth is `--rx-buffer-ms` translated into
    // [`compute_capacity_packets`]. Both inputs feed into the
    // [`JitterBuffer::new`] call below at first-packet arrival, where
    // the device-actual sample_rate × channels are known.
    //
    // Copy the fields out of the `&RecvArgs` borrow before the
    // `thread::spawn(move || ...)` so the closure satisfies the
    // `'static` bound. `args` itself is `&RecvArgs` borrowed for
    // `run_recv`'s body lifetime — trying to move `&args` into a
    // 'static closure trips E0521. Both `prebuffer_ms: usize` and
    // `rx_buffer_ms: usize` are `Copy` so local bindings are the
    // cleanest fix.
    let prebuffer_ms = args.prebuffer_ms;
    let rx_buffer_ms = args.rx_buffer_ms;

    // Slice 7: lazily-initialized receive state. Holds the
    // JitterBuffer + FormatPipeline + scratch buffer together so a
    // single mutex acquisition gives the cpal callback a consistent
    // view (`jb` and `pipeline` are paired at construction time,
    // never paired after). Output-side state — `player_cfg_slot` —
    // is cached separately so the receiver thread can read it
    // independently when initializing the lazy `rx_state` on first
    // packet.
    let rx_state: Arc<Mutex<Option<RxState>>> = Arc::new(Mutex::new(None::<RxState>));
    let player_cfg_slot: Arc<Mutex<Option<PlayerConfig>>> =
        Arc::new(Mutex::new(None::<PlayerConfig>));
    // Decode-side stats aggregator (Tier 1 #9 — "Stats gap"). The
    // receiver thread bumps the matching counter on every
    // Packet::decode Err via teehee::protocol::DecodeStats::record.
    // The periodic stats thread (gated by --stats) snapshots and
    // logs the per-category breakdown alongside jitter / format
    // stats. The per-packet warn! on decode Err fires regardless
    // of --stats.
    let decode_stats: Arc<Mutex<DecodeStats>> = Arc::new(Mutex::new(DecodeStats::default()));

    // Open the cpal Player FIRST so its actual output format is
    // cached into `player_cfg_slot` BEFORE the decoder thread can
    // possibly see its first packet. This ordering eliminates the
    // race that would otherwise force the first-packet handler to
    // either drop its payload or spin-wait on the slot.
    //
    // The cpal callback may fire (and silence-fill) during the
    // small interval between Player::open_default_output returning
    // and this `player_cfg_slot` write — that interval is exactly
    // prebuffer semantics. No special-casing required.
    let player_cfg_slot_for_player = Arc::clone(&player_cfg_slot);
    let rx_state_for_player = Arc::clone(&rx_state);
    let player = audio_io::Player::open_default_output(move |data: &mut [f32]| {
        let cfg_guard = player_cfg_slot_for_player.lock().unwrap();
        let cfg = match *cfg_guard {
            Some(c) => c,
            // Output format not yet known (sub-microsecond startup
            // window before main sets the slot). Silence-fill.
            None => {
                for s in data.iter_mut() {
                    *s = 0.0;
                }
                return;
            }
        };
        let mut state_guard = rx_state_for_player.lock().unwrap();
        let state = match state_guard.as_mut() {
            Some(s) => s,
            // Prebuffer gate still held: silence-fill. Identical to
            // slice-6 behaviour — just gated on rx_state instead of
            // a bare Option<JitterBuffer>.
            None => {
                for s in data.iter_mut() {
                    *s = 0.0;
                }
                return;
            }
        };
        // Scratch sizing: input samples = ceil(out_frames ×
        // input_rate / output_rate) × input_channels + 1 for the
        // resampler's seed-frame carry. CRITICAL: do NOT multiply
        // by channels in the rate ratio — the ratio is FRAME-rate,
        // not SAMPLE-rate. Mixing channels into the rate computation
        // (a previous-draft bug) gives incorrect sizes when input
        // and output channel counts differ (e.g. 48k stereo ->
        // 48k mono). Frames-only math keeps the time domain
        // aligned; channels multiply at the end.
        let output_channels = cfg.channels as usize;
        let output_rate = cfg.sample_rate as u64;
        let input_rate = state.input_rate_hz as u64;
        let input_channels = state.input_channels as usize;
        let out_frames = data.len() / output_channels;
        let in_frames = ((out_frames as u64 * input_rate).div_ceil(output_rate)) as usize + 1;
        let input_samples = in_frames * input_channels;
        if state.scratch.len() < input_samples {
            // Amortized geometric growth: size doubles until it
            // covers the new request, plus a small constant for
            // alignment slack. First resize (cold start) lands at
            // `max(input_samples, 64)`; subsequent resizes grow by
            // ~2× each time the cpal callback size creeps up.
            let next = state.scratch.len().max(64) * 2 + 64;
            state.scratch.resize(next.max(input_samples), 0.0);
        }
        // Drain the jitter buffer into scratch (input-rate ×
        // input-channels interleaved f32). `pop_frames` ALWAYS
        // returns `out.len()` and zero-pads missing queued audio
        // up to its request size — the pipeline sees a fully
        // populated `scratch` even when the network has starved.
        state.jb.pop_frames(&mut state.scratch[..input_samples]);
        // Convert input-rate × input-channels scratch to
        // output-rate × output-channels `data`. The pipeline
        // returns the OUTPUT FRAMES written; linear-resampler
        // carry state lives inside the pipeline struct (not the
        // scratch buffer).
        let written = state
            .pipeline
            .process(&state.scratch[..input_samples], data);
        let written_samples = written * output_channels;
        // The fixed-point resampler can leave a single output
        // frame unwritten at the tail of a chunk when carry
        // state is non-zero and the input ends mid-step. The
        // next callback fills it in; filling that 1-frame tail
        // with zeros here is inaudible (likely already silent on
        // natural-end reach).
        if written_samples < data.len() {
            for s in &mut data[written_samples..] {
                *s = 0.0;
            }
        }
    })?;
    // Cache the cpal device's actual output format BEFORE the
    // decoder thread spawns. After this line runs, the first
    // packet handler is GUARANTEED to read `Some(_)` from the
    // slot — no race, no spin-wait, no dropped first packet.
    *player_cfg_slot.lock().unwrap() = Some(player.config());

    let stop_flag = Arc::new(AtomicBool::new(false));
    let rx_state_for_recv = Arc::clone(&rx_state);
    let player_cfg_slot_for_recv = Arc::clone(&player_cfg_slot);
    let ds_for_recv = Arc::clone(&decode_stats);
    let stop_for_recv = Arc::clone(&stop_flag);
    let _rx_handle = thread::spawn(move || {
        let mut pkt_buf = vec![0u8; 16 * 1024];
        let mut sample_buf: Vec<f32> = Vec::new();
        while !stop_for_recv.load(Ordering::Relaxed) {
            match rx.recv_timeout(&mut pkt_buf, Duration::from_millis(100)) {
                Ok(Some(n)) => {
                    let pkt = match Packet::decode(&pkt_buf[..n]) {
                        Ok(p) => p,
                        Err(e) => {
                            // Record into the running counter BEFORE the
                            // warn — both fire on every error, the warn
                            // is the per-packet line, the stats counter
                            // is what `--stats` aggregates over time.
                            // warn! borrows e by reference (Display
                            // formatting), record(e) consumes it —
                            // fires the warn first so the still-borrowed
                            // `e` is alive for the format call.
                            warn!(error = %e, "decode error, dropping packet");
                            ds_for_recv.lock().unwrap().record(&e);
                            continue;
                        }
                    };
                    let mut state_guard = rx_state_for_recv.lock().unwrap();
                    if state_guard.is_none() {
                        // reader.cfg is guaranteed Some by the
                        // run_recv ordering invariant (Player is
                        // opened and cached before this thread
                        // spawns). If we observably reach this
                        // unreachable, fix the run_recv ordering.
                        let cfg = player_cfg_slot_for_recv
                            .lock()
                            .unwrap()
                            .expect("player_cfg_slot must be Some by run_recv contract");
                        let samples_per_packet = pkt.payload.len() / 4; // f32 = 4 bytes
                                                                        // Slice 6 prebuffer gate target — unchanged
                                                                        // after slice 7/10. The gate compares
                                                                        // `queued_frames()` (interleaved f32
                                                                        // SAMPLES) against this value, so both
                                                                        // sides of the comparison use the
                                                                        // INPUT-rate × INPUT-channels unit. The
                                                                        // slice-7 format pipeline runs DOWNSTREAM
                                                                        // of the gate release, so it does NOT
                                                                        // affect prebuffer timing.
                                                                        //
                                                                        // The validate() cross-flag check at the
                                                                        // top of run_recv guarantees
                                                                        // `rx_buffer_ms >= prebuffer_ms`, so
                                                                        // `capacity_packets` is always >= the gate
                                                                        // target.
                        let prebuffer_target_frames =
                            (prebuffer_ms * pkt.sample_rate as usize * pkt.channels as usize)
                                / 1000;
                        // Slice 10 (Tier 3 #9): explicitly derive
                        // `capacity_packets` from `--rx-buffer-ms`
                        // rather than the slice-6 hardcoded
                        // `max(32, required_packets × 3)`. Operator
                        // now expresses total ring depth directly
                        // in ms of audio. The cross-flag validate()
                        // at the top of run_recv and clap's
                        // range-check value_parser both gate the
                        // realized error paths in compute_capacity_packets,
                        // so an `Err` here is unreachable — log and
                        // keep the receiver thread alive rather than
                        // `?` (which would require the closure to
                        // return a `Result` we don't use elsewhere).
                        let capacity_packets = match compute_capacity_packets(
                            rx_buffer_ms,
                            prebuffer_ms,
                            pkt.sample_rate,
                            pkt.channels,
                            samples_per_packet,
                        ) {
                            Ok(c) => c,
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    rx_buffer_ms,
                                    prebuffer_ms,
                                    sample_rate = pkt.sample_rate,
                                    channels = pkt.channels,
                                    "rx ring capacity derivation failed; \
                                     dropping first packet (should be \
                                     unreachable after run_recv startup \
                                     validation)"
                                );
                                continue;
                            }
                        };
                        let jb = JitterBuffer::new(
                            samples_per_packet,
                            capacity_packets,
                            Some(prebuffer_target_frames),
                        );
                        let pipeline = FormatPipeline::new(
                            pkt.sample_rate,    // input_rate_hz
                            cfg.sample_rate,    // output_rate_hz
                            pkt.channels,       // input_channels
                            cfg.channels as u8, // output_channels
                        );
                        let fp_active = !pipeline.is_passthrough();
                        let sent_rate = pkt.sample_rate;
                        let sent_ch = pkt.channels;
                        let recv_rate = cfg.sample_rate;
                        let recv_ch = cfg.channels;
                        *state_guard = Some(RxState {
                            jb,
                            pipeline,
                            input_rate_hz: sent_rate,
                            input_channels: sent_ch,
                            scratch: Vec::new(),
                        });
                        info!(
                            sender_sample_rate = sent_rate,
                            sender_channels = sent_ch,
                            receiver_sample_rate = recv_rate,
                            receiver_channels = recv_ch,
                            samples_per_packet,
                            capacity_packets,
                            prebuffer_target_frames,
                            prebuffer_ms,
                            rx_buffer_ms,
                            ring_floor_ms_at_default_audio = capacity_packets as u64
                                * pkt.sample_rate as u64
                                * pkt.channels as u64
                                / 1000,
                            format_conversion_active = fp_active,
                            "first packet received — jitter buffer + format pipeline anchored"
                        );
                    }
                    pkt.pcm_f32_into(&mut sample_buf);
                    let state = state_guard
                        .as_mut()
                        .expect("rx_state must be Some after first packet");
                    match state.jb.push(pkt.sequence, &sample_buf) {
                        teehee::jitter::PushOutcome::Stored => {}
                        other => warn!(?other, seq = pkt.sequence, "non-Stored push"),
                    }
                }
                Ok(None) => continue,
                Err(e) => {
                    warn!(error = %e, "recv error");
                    continue;
                }
            }
        }
    });

    if args.stats {
        spawn_periodic_receiver_stats(
            Arc::clone(&rx_state),
            Arc::clone(&decode_stats),
            Arc::clone(&stop_flag),
        );
    }

    info!("teehee recv running. Press Ctrl+C to stop.");

    // Shutdown model: the main thread polls `stop_flag` every 100ms.
    // When the flag is set (e.g. by a Ctrl+C handler in a wrapper,
    // or by embedding code calling `stop_flag.store(true)`), the
    // decoder thread exits its recv loop, the stats thread exits
    // its sleep loop, and the cpal Player + audio stream are
    // dropped as `run_recv` returns. The OS reaps any stragglers.
    //
    // For the CLI binary, Ctrl+C kills the process directly (the
    // default signal handler), which is equivalent. The stop flag
    // exists so embedding code can shut down the receiver cleanly
    // without process exit.
    while !stop_flag.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(100));
    }
    info!("teehee recv shutting down");
    Ok(())
}

fn spawn_periodic_receiver_stats(
    rx_state: Arc<Mutex<Option<RxState>>>,
    decode_stats: Arc<Mutex<DecodeStats>>,
    stop_flag: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(1));
            let guard = rx_state.lock().unwrap();
            if let Some(state) = guard.as_ref() {
                let jb_stats = state.jb.stats();
                let fp = state.pipeline.stats();
                let fp_active = !state.pipeline.is_passthrough();
                let d = decode_stats.lock().unwrap();
                // Per slice-7 design: when sender format == receiver
                // format the pipeline is a no-op. To preserve
                // operators' grep muscle memory for the
                // matching-format case, we emit TWO distinct line
                // shapes — the original slice-6 line when
                // passthrough, and an extended line including
                // sender/receiver sample-rate, channel count, and
                // format-pipeline sample in/out counts when
                // conversion is active. Operators grep for
                // "format conversion active" to find the line
                // shape that names both formats.
                //
                // Slice 10 (Tier 3 #9) ADDS one field to BOTH line
                // shapes: `ring_overruns`. Non-zero means the sender
                // out-paced the receiver long enough that the ring
                // wrapped and we overwrote an unplayed future packet.
                // Remediation: `raise --rx-buffer-ms` to grow the
                // ring's `capacity_packets`, or `lower --chunk-ms`
                // on the sender to reduce the per-burst pressure.
                if fp_active {
                    info!(
                        decode_errors = d.total(),
                        decode_truncated = d.truncated,
                        decode_bad_magic = d.bad_magic,
                        decode_bad_version = d.bad_version,
                        decode_bad_format = d.bad_format,
                        late_drops = jb_stats.late_drops,
                        duplicates = jb_stats.duplicates,
                        silence_insertions = jb_stats.silence_insertions,
                        prebuffer_holds = jb_stats.prebuffer_holds,
                        ring_overruns = jb_stats.ring_overruns,
                        sender_sample_rate = state.input_rate_hz,
                        sender_channels = state.input_channels,
                        receiver_sample_rate = state.pipeline.resampler().output_rate_hz(),
                        receiver_channels = state.pipeline.mixer().output_channels(),
                        fp_in = fp.samples_in,
                        fp_out = fp.samples_out,
                        "teehee recv stats (format conversion active)"
                    );
                } else {
                    info!(
                        decode_errors = d.total(),
                        decode_truncated = d.truncated,
                        decode_bad_magic = d.bad_magic,
                        decode_bad_version = d.bad_version,
                        decode_bad_format = d.bad_format,
                        late_drops = jb_stats.late_drops,
                        duplicates = jb_stats.duplicates,
                        silence_insertions = jb_stats.silence_insertions,
                        prebuffer_holds = jb_stats.prebuffer_holds,
                        ring_overruns = jb_stats.ring_overruns,
                        sample_rate = state.input_rate_hz,
                        channels = state.input_channels,
                        "teehee recv stats"
                    );
                }
            }
        }
    });
}

/// Enumerate audio devices via cpal and print a short table.
fn run_devices() -> anyhow::Result<()> {
    use teehee::audio_io::{list_input_devices, list_output_devices};

    println!("Input (capture) devices:");
    let inputs = list_input_devices();
    if inputs.is_empty() {
        println!("  (none reported by cpal)");
    } else {
        for d in &inputs {
            let marker = if d.is_default { "* " } else { "  " };
            println!(
                "  {marker}{name}: {ch} ch @ {sr} Hz",
                name = d.name,
                ch = d.channels,
                sr = d.sample_rate_hz
            );
        }
    }
    println!();
    println!("Output (playback) devices:");
    let outputs = list_output_devices();
    if outputs.is_empty() {
        println!("  (none reported by cpal)");
    } else {
        for d in &outputs {
            let marker = if d.is_default { "* " } else { "  " };
            println!(
                "  {marker}{name}: {ch} ch @ {sr} Hz",
                name = d.name,
                ch = d.channels,
                sr = d.sample_rate_hz
            );
        }
    }
    Ok(())
}
