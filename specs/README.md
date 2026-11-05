# Verification of the reported teehee issues

Date: 2026-11-05
Scope: source at the current working tree. The models and verdicts below
capture the pre-fix behavior; all ten findings have since been fixed in the
production sources. The models are retained as pre-fix regression evidence.

Tooling: `tla2tools.jar` v1.7.4, TLC 2.19, OpenJDK 17. The models below are
abstractions of the implementation (small sequence spaces, packet-level
playout). TLC exhaustively checks each finite configuration. They establish
whether the algorithm/state machine admits the reported behavior; they are not
refinement proofs of the Rust code.

## TLA+ results

TLC exit code 0 means the property held for every reachable state; exit code 12
means an invariant was violated and a counterexample was produced.

| Config | Property | Result |
|---|---|---|
| `JitterRing_bug.cfg` (`SeqMax=16`, `Cap=3`) | `AnchorConsistent` | **Violated.** Head is 15 with slot 0 holding 15; `Push(0)` stores 0 into the same slot, so head 15 and live seq 0 collide. |
| `JitterRing_safe.cfg` (`SeqMax=16`, `Cap=4`) | `AnchorConsistent` | **Holds.** 256 distinct states checked. A power-of-two capacity avoids the wrap collision. |
| `PrebufferAnchor_anchor.cfg` (`Cap=3`, `Target=3`) | `AnchorSound` | **Violated.** `minPushed` remains 0 after seq 3 overwrites slot 0. |
| `PrebufferAnchor_discard.cfg` (`Cap=3`, `Target=3`) | `NoRetainedDiscard` | **Violated.** Anchoring at stale 0 reads slot 0 (holding 3), emits silence, and drops retained seq 3. |
| `SenderPacing_underflow.cfg` (44.1 kHz, 3 ms, 200 ms, +150 ppm) | `BufferNeverNegative` | **Violated.** Even with the maximum +150 ppm correction, the residual deficit is 281 frames per 3 s; a 200 ms buffer underflows after 32 macro-steps (~96 s). |
| `SenderPacing_ppm.cfg` | `PpmExceedsClamp` | **Holds.** `RequiredPpm` evaluates to 2272, which is greater than the 150 ppm clamp. |

The `Nil` value in `PrebufferAnchor.tla` is `SeqMax` (a positive out-of-range
sentinel) because TLA+ parses a bare negative literal poorly in that position.
TLC prints it as the number `8` in counterexamples.

## Finding-by-finding verdicts (pre-fix)

1. **Zero sample rate — confirmed, P1.** `Packet::decode` validates magic,
   version, channels, format, and payload length, but never checks
   `sample_rate` (`src/protocol.rs:240`). The receive path constructs
   `FormatPipeline::new` while holding the `rx_state` mutex
   (`src/main.rs:1605,1680`). That calls `LinearResampler::new`, which asserts
   `input_rate_hz > 0` (`src/format_pipeline.rs:108`). One framed packet with
   rate 0 panics the receive thread and poisons the playback-state mutex.

2. **mDNS empty address — confirmed, P1.** `Advertiser::advertise` passes `""`
   as `host_ipv4` and never calls `enable_addr_auto()` (`src/discovery.rs:163-172`).
   In mdns-sd 0.7.5, `ServiceInfo::new` maps an empty string to an empty address
   set; `is_ready()` requires a non-empty address set; and `register()` only
   fills addresses when `is_addr_auto()` is true. The receiver therefore
   registers a service with no address records, so a fresh sender waits out the
   browse timeout.

3. **WASAPI extensible subformat — confirmed, P1.** `WAVE_FORMAT_EXTENSIBLE`
   (0xFFFE) is mapped directly to `CapturedSampleFormat::F32`
   (`src/loopback.rs:443-447`) without inspecting the SubFormat GUID. For an
   extensible PCM (integer) mix format, `read_from_device` fills a byte view of
   a `Vec<f32>` (`src/loopback.rs:579-585`), so integer samples are reinterpreted
   as floats and emitted as corrupted audio instead of being rejected.

4. **Sender pacing quantization — confirmed, P1.** At 44.1 kHz and 3 ms,
   `chunk_frames = 44100 * 3 / 1000 = 132`, which is 2.993197 ms of audio, while
   `SendPacer` sends every 3 ms (`src/main.rs:731`, `src/main.rs:1046`). The
   deficit is 6.803 µs per chunk, or 2272 ppm. The drift controller clamps at
   ±150 ppm (`src/clock_drift.rs:101`). The TLA+ model shows that even the
   maximum +150 ppm leaves a 281-frame/3 s deficit; a 200 ms receiver buffer
   underflows in about 96 s.

5. **Uneven downmix — confirmed, P2.** For 3→2, `step = 1`, `rem = 1`: output
   channel 0 averages inputs 0 and 1, and output channel 1 reads input 1 again
   (`src/format_pipeline.rs:374-396`). Input 2 is dropped. The reported
   `[0, 0, 1]` example produces `[0, 0]`. The general 5→2 case likewise
   duplicates channel 2 and omits channel 4. Reachability is narrow in the
   current CLI (normal v1 traffic is mono/stereo), but the mixer itself is
   wrong for non-divisible channel counts.

6. **Stale prebuffer anchor — confirmed by model, with a reachability
   caveat, P2.** `min_pushed` is updated on push but is not revised when its
   ring slot is overwritten during prebuffer (`src/jitter.rs:425-434`).
   `PrebufferAnchor.tla` reproduces the overwrite and the resulting silence /
   discard of a still-retained future packet. It requires more than
   `capacity_packets` distinct sequences to be pushed before the gate opens.
   With `rx_buffer_ms == prebuffer_ms` plus startup loss (or a burst), that is
   easy; with the defaults (200 ms prebuffer, 2 s ring) it needs severe loss or
   a long scheduling stall, so the P2 severity is configuration-dependent.

7. **Initial smoothed gain — confirmed, P2.** `ControlState::new` initializes
   `smoothed_gain` to 1.0 (`src/control.rs:95`). The startup system-volume read
   changes the target gain via `set_gain_with_source`, but does not synchronize
   the smoothed value.   `apply_gain` then ramps from 1.0 to the target across the first chunk
   (`src/control.rs:137-157`); at 1920 samples the first sample is
   still multiplied by about 0.9995. Starting muted therefore leaks nearly a
   full-scale first chunk.

8. **Log rotation truncates after a failed copy — confirmed, P2.** The script
   sets `$ErrorActionPreference = 'Continue'` (`scripts/rotate_logs.ps1:34`).
   `Copy-Item` is not in a try/catch and a failure is non-terminating
   (`scripts/rotate_logs.ps1:63`), after which the script proceeds to truncate
   the live log (`scripts/rotate_logs.ps1:66-72`). Reproduced in an isolated
   copy under `target/`: `Copy-Item` failed with `DirectoryNotFoundException`,
   the script printed `[ROTATE]`, and the live log ended at 0 bytes.

9. **Ring indexing at u32 wrap — confirmed by model, P3.** The index is
   `seq % capacity_packets` (`src/jitter.rs:281`). If the capacity does not
   divide 2^32, adjacent sequences can share a slot at the wrap. For capacity
   3, `(2^32 - 1) % 3 == 0` and `0 % 3 == 0`, so seq 2^32-1 and seq 0 collide.
   `JitterRing.tla` exhibits the overwrite; the power-of-two configuration
   passes. At 3 ms chunks this takes about 149 days of continuous streaming, so
   P3 is appropriate. Capacities such as the default-derived 100 or 1500 are
   not powers of two.

10. **MTU header double-count — confirmed, P3.** `compute_budget` subtracts
   53 bytes including the 25-byte teehee header, leaving
   `max_payload_bytes = 1447` as the maximum PCM payload
   (`src/mtu_budget.rs:167-190`). `exceeds_budget` compares a header-inclusive
   encoded length against that already header-subtracted value
   (`src/mtu_budget.rs:208`). A 1440-byte PCM payload encodes to 1465 bytes and
   fits the 1500-byte wire MTU, but is reported as fragmenting. The unit test
   `exceeds_budget(1447) == false` labels an encoded length of 1447 as "exactly
   fits" (`src/mtu_budget.rs:416-418`), encoding the same off-by-header error.

## Fixes applied

All ten findings are fixed in the production tree. The line numbers and
behavior above describe the pre-fix tree.

1. `Packet::decode` rejects `sample_rate == 0` with `InvalidSampleRate` and
   routes it to `DecodeStats::bad_format` (`src/protocol.rs`).
2. The advertised mDNS service calls `enable_addr_auto()` so its A records are
   populated from the host interfaces (`src/discovery.rs`).
3. WASAPI loopback validates the `WAVE_FORMAT_EXTENSIBLE` SubFormat GUID and
   requires 32-bit IEEE float; PCM subformats are rejected instead of being
   reinterpreted as f32 (`src/loopback.rs`).
4. Sender pacing uses `chunk_frames / sample_rate` instead of the nominal
   integer `chunk_ms`, eliminating the 2272 ppm quantization at 44.1 kHz/3 ms
   (`src/main.rs`).
5. Downmixing partitions input channels into contiguous groups and spreads the
   remainder, so every input is consumed exactly once (`src/format_pipeline.rs`).
6. Prebuffering retains at most one ring-sized sequence window, even when
   startup loss leaves holes. Advancing the window removes stale packets and
   recomputes its anchor; late arrivals cannot stretch it backward beyond
   capacity. This also handles anchor overwrites (`src/jitter.rs`).
7. The synchronous startup system-volume read snaps the smoothed gain to the
   real volume instead of ramping from unity (`src/control.rs`).
8. Log rotation copies with `-ErrorAction Stop` and requires the archive to
   contain at least the initial source size. Concurrent appends may make a
   successful archive larger; failed or short copies still return before
   truncation (`scripts/rotate_logs.ps1`).
9. Jitter-ring capacity is rounded up to a power of two so `seq % capacity`
   divides the u32 sequence space evenly (`src/jitter.rs`).
10. `exceeds_budget` compares header-inclusive encoded bytes against
    `max_encoded_bytes() = max_payload_bytes + HEADER_LEN` (`src/mtu_budget.rs`,
    `src/main.rs`).

Regression coverage: unit tests cover findings 1, 4, 5, 6, 7, 9, and 10;
`tests/mtu_strategy_tests.rs` covers the encoded-size boundary and sender-loop
counter; `tests/rotate_logs_tests.ps1` covers stable rotation, concurrent
appends, failed copies, and short archives using temporary files, and runs in
Windows CI. The prebuffer tests also cover startup gaps, out-of-order packets,
and sequence wrap. Findings 2 and 3 are
platform-specific and were compile-checked, but not exercised against an mDNS
network or a live WASAPI endpoint in this environment.

## Reproduce

Place a trusted, locally supplied TLC v1.7.4 `tla2tools.jar` at
`target/tla/tla2tools.jar` (not committed; `target/` is ignored).
The checks do not require network access. The models remain pre-fix evidence,
not proofs of the updated implementation.

Run a model:

```bash
cd specs
java -cp ../target/tla/tla2tools.jar tlc2.TLC \
  -config JitterRing_bug.cfg JitterRing.tla
```

Use a distinct `-metadir` per run if invoking several configs in the same
second; TLC names its metadata directory from the current time.
