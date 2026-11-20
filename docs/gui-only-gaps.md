# GUI-only operation — closed gaps

**Status (2026-11-20): closed.** teehee is an interactive desktop app. The Task
Scheduler stack (`TeeHeeSender`, `TeeHeeWatch`, `TeeHeeWatchdog`,
`TeeHeeRotate`) is retired: the GUI now owns startup, self-heal, drift,
history, capture health and alerting. It still embeds its own send loop
(`gui/src/lib.rs::send_worker`) rather than driving the CLI binary; the loop
reuses the library's capture, protocol, drift and logging modules instead of
reimplementing them.

The native GUI added in `85d82cf` embeds its own sender. This file records what
GUI-only operation gives up, and how each gap was closed, so retiring the task
recipe stays a deliberate trade rather than an accident.

## Gaps and how they are closed

| Capability | Task stack (retired) | GUI today |
|---|---|---|
| Start at logon | `TeeHeeSender` at-logon trigger | **Open at login** in Settings calls `Application.setLaunchAtLogin`; the app opens in the interactive session where WASAPI loopback works, and **Start streaming when teehee opens** starts the saved receiver |
| Self-heal | 2-minute repeating trigger + `Local\TeeHeeSend` mutex | Sender errors enter `phase="error"`; the UI retries with exponential backoff (2 s → 30 s) while **Reconnect automatically** is on. A second sender is refused by the shared `Local\TeeHeeSend` mutex |
| Single instance | CLI acquires the named mutex before opening capture | `Engine::start_send` acquires the same mutex via `teehee::single_instance` for the life of the session; a running CLI sender is reported as "already running" and never double-streamed |
| Monitoring / alerts | `send_watch.ps1` every 5 min + watchdog every 10 min | Desktop notifications on sender errors, a silent loopback (`capture_callbacks == 0`), ring saturation, drift saturation and four hours of quiet capture; the in-window health card also shows overruns and latency drops |
| Drift compensation | Compensator with `send_drift_ppm`, saturation alerts | `send_worker` runs the library `ClockDriftTracker` + `Ema` + `period_for_ppm` against post-pop ring depth; `drift_ppm` is published and shown as measured |
| Durable history | `logs/send.jsonl` at 1 Hz, daily rotation | The GUI writes `send.jsonl` itself at 1 Hz (`send_start`, `send_stats`, `send_pause`/`send_resume`, `send_stop`, `send_error`) to the app-data directory and rotates at 16 MiB (one previous generation) |
| Capture-health visibility | `capture_callbacks`, `capture_nonzero_samples`, underruns in every stats line | All capture counters (callbacks, pushed/non-zero samples, ring depth/high-water, overruns, underruns, latency drops) are in the status snapshot, Diagnostics, health rules and the JSONL log |

The two spec deviations recorded earlier are resolved: the GUI no longer runs a
second sender while the mutex says one exists, and it uses the library's audio
pipeline rather than a private resampler. It does still keep the send
orchestration loop in the app (there is no `teehee::session` handle yet).

## Evidence from the 2026-11-20 outage

Kept for context; the failure mode below is now visible and alerted on.

- `logs/send.jsonl` stops at 10:04:14; `send_watch.heartbeat` stops at 10:05:02.
- `TeeHeeSender`, `TeeHeeWatch` and `TeeHeeWatchdog` were all Disabled;
  `TeeHeeSender` last ran 10:04:01 with result `267014` (`0x41306`, task
  terminated). The disable, not a code fault, is why nothing self-healed or
  alerted.
- The last two `send_start` events (10:02:02 and 10:04:02) logged
  `capture_ring_ready` with WASAPI loopback but then reported
  `capture_callbacks: 0`, `capture_nonzero_samples: 0`, and
  `capture_underruns == packets_sent` — sending silence that only the JSONL
  counters revealed.
- The GUI's own drift field previously read "not measured"; it now publishes
  the measured ppm.

## Remaining limitations (accepted)

- **No headless path.** There is no supported sender without an interactive
  session and the GUI process. Fleet/headless deployment is out of scope.
- **The app must stay running.** Alerts and reconnect live in the GUI process;
  closing the window quits it. There is no tray menu yet, so quit-to-tray and
  "keeps streaming with the window closed" are not available.
- **No `send_alerts.log` or watchdog heartbeat.** The JSONL history is the
  post-mortem surface; alerts are system notifications rather than a second
  process checking the first.
- **Log retention is one generation.** The GUI rotates `send.jsonl` at 16 MiB
  and keeps `send.jsonl.1`; it does not run the archive-pruning script.
- **Control port and follow-system-volume** are still CLI-only settings.

## See also

- `docs/gui-design.md` §9 (engine contract), §9.4 (what the GUI must not do),
  §12 open question 3 (retiring the Task Scheduler recipe)
- `docs/reference.md` §Windows deployment (GUI only)
- `gui/src/state/session.ts` (startup, reconnect, alerts, health rules)
- `gui/src/lib.rs` (embedded sender, drift, capture counters, JSONL)
