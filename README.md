# teehee

Stream Windows system audio to a Mac (or any LAN peer) over a tiny UDP
protocol, without AirPlay, without Python audio packages, and without
paid proprietary tools.

## What it is

`teehee` is a single Rust CLI binary with three commands:

* `teehee recv` — listen on a UDP port and play incoming audio through the
  default output device.
* `teehee send --host <peer-ip>` — capture the Windows default output
  (loopback) and stream it over UDP.
* `teehee devices` — list available audio devices on this machine.

It uses `cpal` for native audio access (WASAPI on Windows, CoreAudio on
macOS, ALSA on Linux) and a custom raw UDP packet protocol with a 25-byte
header and f32 PCM payload. No codec, no compression, no daemon, no
runtime dependencies.

## Wire format

Every packet is a 25-byte fixed little-endian header followed by an
interleaved f32 PCM payload:

| offset | size | field             |
|-------:|-----:|-------------------|
|      0 |    4 | magic = `"TEHE"`  |
|      4 |    1 | version = `0x01`  |
|      5 |    4 | sequence (u32)    |
|      9 |    8 | frame_timestamp (u64) |
|     17 |    4 | sample_rate (Hz, u32) |
|     21 |    1 | channels (u8)     |
|     22 |    1 | sample_format tag |
|     23 |    2 | payload_len (u16) |
|     25 |  …  | f32 samples, interleaved |

Each packet is independently interpretable — a receiver can recover from
startup packet loss without state from prior packets outside the sequence
number (used by the receiver for jitter reordering).

## Build

```bash
cargo build --release
```

The release binary lives at `target/release/teehee.exe` (Windows) or
`target/release/teehee` (macOS / Linux). It is fully self-contained — no
runtime, no Python, no PortAudio, no installer.

## Run

### On the Mac (or any Linux/macOS receiver)

```bash
./teehee recv --port 5000 --prebuffer-ms 200 --rx-buffer-ms 2000 --stats
```

`--prebuffer-ms` (slice 6) is the *gate*: the receiver waits in
silence until at least this many ms of audio have accumulated in
the jitter buffer, then playback starts. Lower for lower first-byte
latency (cost: more dropouts on shaky networks); raise for flaky
Wi-Fi.

`--rx-buffer-ms` (slice 10) is the *ring depth*: the total buffering
capacity, expressed in ms of audio. The jitter buffer is sized to
hold at least this much (bounded by `max(32, packets)` for
sanity). Default `2000` (= 10× prebuffer), which holds 100 packets
at 48 kHz stereo / chunk-ms=20 and absorbs typical home-Wi-Fi
reorders without dropping. Range [100, 30000]: 100 ms is the
smallest ring that still meets the OS-memory floor; 30 s is the
largest sane ring without blowing OS memory at high channel
counts.

**Cross-flag invariant**: `--rx-buffer-ms >= --prebuffer-ms` —
the gate target can never exceed the ring size, or playback
would block indefinitely. Passing `--rx-buffer-ms=200
--prebuffer-ms=500` is rejected at start-up with a clear error
rather than silently misbehaving.

**Sender bursts drive `ring_overruns`**: when the sender
out-paces the receiver long enough that the wrapper ring
reaches an unplayed future slot, the underlying `JitterBuffer`
overwrites and bumps `ring_overruns` on the receiver's
`--stats` line. Remediation is either `raise --rx-buffer-ms`
to grow the ring's `capacity_packets`, or `lower --chunk-ms` on
the sender to reduce the per-burst pressure. `ring_overruns` is
distinct from the older `mid_read_collisions` counter (which
fires only when the cpal callback is currently mid-draining the
colliding slot — a much rarer signature).

### Windows system-audio loopback sender

```bash
.\teehee.exe send --host <mac-ip> --port 5000 --chunk-ms 20 --capture-source=loopback --stats
```

`--chunk-ms` is the encoder packet interval. `20` (= 50 packets/sec) is a
reasonable starting point; smaller values give lower latency but higher
packet rate. Default format is 48 kHz stereo f32.

`--sample-rate` and `--channels` control the format for `--sine` dry-run
mode only. For real capture (no `--sine`), the cpal / WASAPI device's
actual sample rate and channel count are used and the CLI values are
silently overridden (the startup log shows the divergence).

`--mtu` (slice 9) sets the link MTU (default 1500, range [576, 9000]).
The sender is MTU-aware: it logs the configured MTU, its derived
`max_payload_bytes` envelope, and the current chunk-ms × audio-params
packet size at startup so you can see the relationship at a glance. If
an encoded packet exceeds the envelope, the OS IP-layer fragments it
transparently — the sender does NOT drop or clamp the packet — and the
`fragmentations` counter on the `--stats` line increments. A non-zero
rate means the chunk-ms × audio-params combination is larger than the
envelope; bump `--mtu` (e.g. to 9000 for a jumbo-frame LAN) or lower
`--chunk-ms`. Examples:

* `--mtu 576` — IPv6 RFC-minimum link MTU. Forces small packets
  (~1 ms of audio at stereo).
* `--mtu 1280` — IPv6 deployment minimum; common on tunnel/VPN paths.
* `--mtu 1500` — typical Ethernet LAN. The default; matches what
  `mtu_smoke` and `mtu_boundary_sweep` regression tests pin.
* `--mtu 9000` — jumbo-frame Ethernet. The default `chunk-ms=20`
  stereo config still fits (within the envelope) at this size.

`--capture-source=loopback` is the **Windows WASAPI loopback** path (slice 8).
It captures the default render endpoint with `AUDCLNT_STREAMFLAGS_LOOPBACK`,
so `teehee` ships whatever is about to play out the speakers — no
"Stereo Mix" or sound-card loopback-input setup needed. The render
endpoint's mix format must be IEEE_FLOAT (which is the default on modern
Windows); legacy PCM-only render endpoints surface a clear error so you
switch render devices or update teehee for PCM support in a follow-up
slice.

**Note on loopback timing.** The WASAPI loopback stream can take 5–7
seconds to produce the first audio data after `start_stream()` — this is
a Windows audio-engine warm-up characteristic, not a golive bug. During
this period the sender logs `packets_sent=0`. Additionally, loopback
capture only generates data while audio is actively being rendered on
Windows (music, YouTube, games, system sounds). When no audio plays,
the audio engine goes idle and data stops; it resumes automatically when
audio starts again. There is no timeout or disconnect — the sender keeps
polling. If you hear the audio on your headphones instead of the Mac,
ensure `teehee recv` is running on the Mac side.

For the legacy default-input path on Windows (e.g. capturing a real
microphone), drop `--capture-source` or pass `--capture-source=default`
instead. That path requires "Stereo Mix" (or your sound card's loopback
input) enabled in the sound control panel and marked default.

`--capture-source=auto` (slice 11) probes the OS default input first
AND, on Windows only, transparently falls back to the WASAPI loopback
path when the default-input open returns an error. On macOS / Linux
the loopback fallback is unreachable (slice 8 made `LoopbackCapturer`
a Windows-only API), so `auto` is functionally identical to
`default` on those hosts. On a stock Windows desktop with a working
microphone as the system default input, `auto` will succeed at the
default-input step and **never reach the loopback fallback** — if you
need system audio (not microphone audio) on Windows, use
`--capture-source=loopback` explicitly so the probe order is bypassed
and you broadcast the render endpoint's mix instead. The `auto`
helper logs which capture path it landed on (`cpal default input
(auto-probed)` or `WASAPI loopback (auto-fallback)`) at the startup
info! line; grep `--stats` for the substring `auto-` to confirm the
choice.

**Slice-11 strict-mode: `--exact-capture-source` and `TEEHEE_STRICT_LOOPBACK`.**
Two complementary mechanisms shield operators from the silent-mic
pitfall on shared / fleet-managed machines where the default-input
device is unpredictable. They are NOT the same; pick one per
deployment based on which trade-off fits your workflow:

* **CLI flag: `--exact-capture-source`**. REJECTS `--capture-source=auto`
  at parse / validate time; the operator MUST type either `default`
  or `loopback` explicitly. Use this on machines where someone is
  packaging teehee into a deploy script / config-management recipe
  and wants to fail loudly at startup if a downstream operator
  accidentally types `--capture-source=auto`. The flag does not
  change the value of `--capture-source` itself; it just disallows
  `auto`. Omitted = off (`--capture-source=auto` parses cleanly, v1
  back-compat preserved).

  ```bash
  # This errors at startup with a clear message naming both flags
  # so the fix is grep-able in logs:
  .\teehee.exe send --host <peer-ip> --capture-source=auto \
    --exact-capture-source
  # error: --exact-capture-source: --capture-source=auto is rejected.
  # Pass --capture-source=default (or =loopback on Windows) ...

  # This works:
  .\teehee.exe send --host <peer-ip> --capture-source=loopback \
    --exact-capture-source
  ```

* **Env var: `TEEHEE_STRICT_LOOPBACK=1`**. Silently REMAPS
  `--capture-source=auto` to the WASAPI loopback path on Windows
  (skipping the default-input probe entirely), or errors on
  macOS / Linux because the loopback route is Windows-only. The
  env var does NOT change the parse / validate layer; clap-derived
  `--capture-source` argument is unchanged. Use this when wrapping
  teehee in a CI / shell pipeline where you'd rather keep
  `--capture-source=auto` in the user-facing recipe (so it Just
  Works on macOS / Linux where `loopback` errors out anyway) and
  want Linux / Windows servers to silently route the auto path to
  loopback. The startup info! line shows the strict label
  (`WASAPI loopback (strict)`) when this env var is the reason
  auto landed on the loopback path, distinct from the
  `WASAPI loopback (auto-fallback)` label that fires through the
  regular auto-probe-and-fail flow on Windows.

  ```bash
  # Git Bash / MSYS2 on Windows: export the env var, then run
  export TEEHEE_STRICT_LOOPBACK=1
  ./teehee.exe send --host <peer-ip> --capture-source=auto --stats
  # startup info label: WASAPI loopback (strict)
  ```

  ```powershell
  # PowerShell: set the env var for the current shell
  $env:TEEHEE_STRICT_LOOPBACK = "1"
  .\teehee.exe send --host <peer-ip> --capture-source=auto --stats
  ```

**Choosing between them**: `--exact-capture-source` is opt-in strict
(operators MUST type the explicit value; failures are loud);
`TEEHEE_STRICT_LOOPBACK` is opt-out lenient (the operator can keep
typing `auto` and the env var silently redirects; failures are
silent on the happy path). Pick `--exact-capture-source` for packagers
and CI policies that want to catch configuration drift at parse
time. Pick `TEEHEE_STRICT_LOOPBACK` for wrappers / dotfiles where the
script is fine with `auto`, but the host config (default-input
device unpredictable, fleet-managed / shared rig) needs the
strict-loopback side-effect. They are independently usable; setting
both is well-defined (validate layer rejects `auto` first, the env
var is never consulted).

### Dry-run / loopback test without hardware

`teehee send --host 127.0.0.1 --sine` generates a 440 Hz tone at the
default format instead of capturing real audio. Use this to verify the
receiver on a single machine, or to run the localhost smoke test:

```bash
# Terminal A
.\teehee.exe send --host 127.0.0.1 --sine --stats
# Terminal B
.\teehee.exe recv --port 5000 --prebuffer-ms 200 --stats
```

You should see the sender log ~50 packets/sec and the receiver log
`late_drops=0 duplicates=0 silence_insertions=0` every second.

## Firewall configuration

v1 sends **UDP only**, port `5000` by default. Both sides need the port
open:

### Windows (sender side)

Windows Defender Firewall may block inbound responses or outbound UDP on
first launch. Two options:

1. Allow on prompt — when you first run `.\teehee.exe`, Windows shows the
   firewall prompt. Tick both "Private" and "Domain" networks.
2. Pre-authorize in PowerShell (admin):
   ```powershell
   New-NetFirewallRule -DisplayName "teehee send (UDP out)" `
     -Direction Outbound -Protocol UDP -RemotePort 5000 -Action Allow
   ```

If your network is "Public," inbound rules are stricter — switch the
network profile to *Private* in *Settings → Network & Internet → Properties*,
or use `--host` with an IP that doesn't trigger Windows Network Discovery
prompts.

### macOS (receiver side)

macOS Application Firewall blocks unsolicited inbound UDP by default.
Allow `teehee` listening:

1. *System Settings → Network → Firewall → Options…*
2. Click `+`, navigate to the `teehee` binary, set *Allow incoming
   connections*.
3. Or in Terminal:
   ```bash
   sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add /path/to/teehee
   sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblock /path/to/teehee
   ```

The receiver binds on `0.0.0.0:5000` so it accepts UDP from any
interface once the firewall allows it.

### Test connectivity

From the Windows sender, before running the full pipeline, verify the
macOS receiver is reachable:

```powershell
Test-NetConnection -ComputerName <mac-ip> -Port 5000
```

You should see `TcpTestSucceeded: True` only if something is listening
for TCP on that port; this is a coarse IP/DNS/firewall sanity check, not
a proof that UDP audio packets can pass. The real UDP proof is running
`teehee recv --stats` and watching packets arrive from the sender.

## Finding peer IPs

### Mac's IP (from the Mac)

* *System Settings → Wi-Fi → Details… → TCP/IP → IPv4 Address* — most
  reliable.
* Or in Terminal: `ipconfig getifaddr en0` (Wi-Fi) or
  `ipconfig getifaddr en1` (Ethernet / Thunderbolt bridge).

### Windows PC's IP (from the PC)

* `ipconfig` in cmd.exe — look for the active interface's `IPv4 Address`
  under the heading for Wi-Fi or Ethernet.
* Or `Get-NetIPAddress -AddressFamily IPv4 -PrefixOrigin Dhcp |
  Select-Object IPAddress` in PowerShell.

### Both — `teehee devices`

`teehee devices` lists cpal-reported audio endpoints on the current
machine. Useful for confirming the OS sees your sound card before you
debug a streaming issue.

## Trusted-LAN limitations

v1 traffic is **not encrypted** and **not authenticated**. Anyone on the
same LAN who can reach the receiver's UDP port can inject audio (which
the receiver will dutifully play out the speakers) or sniff the wire.
Use only on a network you control and trust. **Do not** expose the
receiver port to the public internet.

## Non-goals (v1)

* AirPlay / RAOP compatibility.
* Mobile clients (iOS / Android).
* GUI / tray / menu-bar app.
* Encryption or authentication.
* Multicast / mDNS / Bonjour discovery.
* Opus / FLAC / AAC / other compression.
* Ultra-low-latency gaming or live-performance use.
* Fan-out (one sender to many receivers).
* Audio/video sync with remote video playback.
* Recording or saving audio to disk.
* Windows WASAPI loopback on macOS / Linux (slice 8 ships Windows-only;
  macOS / Linux callers fall back to cpal default-input + a virtual input
  device, or `--sine`).
* Render-endpoint PCM-only support (slice 8 ships IEEE_FLOAT only; legacy
  render endpoints surface a clear error to switch devices).

## 10-minute LAN acceptance test

Before considering the build "good enough," run this on your real LAN:

1. Plug a Windows PC + Mac into the same switch / Wi-Fi.
2. On the Mac: `./teehee recv --port 5000 --prebuffer-ms 200 --stats`.
3. On the Windows PC:
   `.\teehee.exe send --host <mac-ip> --port 5000 --chunk-ms 20 --stats`.
4. Play any continuous audio source on Windows that is **not** a
   microphone — browser, Spotify, YouTube, system beeps, anything that
   exercises the loopback path — for ten minutes.
5. Verify:
   * Sender's `packets_sent` counter increases by ~50/sec steadily.
   * Receiver's `late_drops`, `duplicates`, `silence_insertions` stay at
     0 or low single digits throughout the run.
   * Listening on the Mac's default output, the audio matches the
     sender's source audibly with at most tolerable glitches on a
     typical home Wi-Fi network.
   * No crashes, no panics, the receiver still responds to Ctrl+C
     cleanly.

v1 is considered usable when there is no steady buffer drift, no crash,
and only tolerable glitches. If the receiver logs show drift
(`silence_insertions > 0` accumulating minute-over-minute), raise
`--prebuffer-ms` on the receiver. If `late_drops > 0` rises, raise
`--chunk-ms` on the sender (bigger packets, fewer per second, less
network pressure).

## Troubleshooting

### Receiver hears nothing

1. Confirm `teehee recv` is running and printed
   `listening on 0.0.0.0:5000`.
2. Confirm the sender can reach the receiver port
   (`Test-NetConnection -Port 5000` from PowerShell, or `nc -u -z
   <mac-ip> 5000` from the Mac).
3. Check the receiver startup log for `format_conversion_active`. The
   receiver anchors its jitter buffer from the **first packet's**
   sample_rate / channels, then converts to the local output device's
   format when needed. If you hear silence, verify the sender is still
   emitting non-empty f32 packets and that the receiver opened the
   expected output device.
4. Wait 5–10 seconds after starting the sender — the WASAPI loopback
   engine takes 5–7 seconds to start producing data. Check `--stats` for
   `packets_sent` increasing.
5. For `--capture-source=loopback`: audio must be actively playing on
   Windows (music, YouTube, system sounds). The loopback engine goes
   idle when no audio is rendered and resumes automatically when audio
   starts.
6. Check the firewall instructions above.

### Sender reports `connect failed` immediately

* Confirm the destination host resolves. `--host 127.0.0.1` is always
  valid; for remote peers, use the IP not the hostname unless DNS is
  set up. Either the positional `HOST` argument or the `--host` flag
  is accepted (not both); the `host:port` form (e.g.
  `--host 192.168.0.10:6000`) embeds the port and rejects an
  accompanying `--port 6000` as "ambiguous port" rather than silently
  doubling into `192.168.0.10:6000:6000`.
* Confirm the receiver is bound on the target port
  (`ss -ulnp 'sport = :5000'` on Linux/macOS, `Get-NetUDPEndpoint
  -LocalPort 5000` on Windows).

### Receiver cpal errors

cpal formats vary by OS. On a headless Windows server with no audio
device, `teehee recv` will exit at startup with a cpal "no output
device" message — expected, harmless, run it on a machine with speakers.

---

See `tests/` for the test suite. Run `cargo test` to validate the build
end-to-end.
