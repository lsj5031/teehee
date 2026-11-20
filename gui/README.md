# teehee GUI

The native desktop app for teehee, built with NSH: one Rust crate and TSX
screens on the same engine. The visual and interaction design is in
[`docs/gui-design.md`](../docs/gui-design.md). The GUI is the supported sender;
the Task Scheduler recipe is retired.

## Run

```bash
nsh run App.tsx
```

`nsh check` type-checks the TSX and the Rust crate; `nsh test` runs the
headless UI test.

## What is here

```
App.tsx                 shell: title bar, sidebar, routing, stream bar
theme.json              design tokens (light/dark)
src/lib.rs              Rust engine: capture, drift, JSONL, single-instance
src/state/session.ts    native status polling, health rules, preferences
src/ui/                 app-owned component layer (button, chip, card, ...)
src/components/         shell pieces and data views
src/pages/              Stream, Receivers, Audio, Diagnostics, Settings
assets/icons/           iced SVG line icons (tinted from the theme)
```

## Engine

`src/lib.rs` owns the sender and receiver. It does not spawn `teehee.exe` and
does not read the CLI's logs. The sender runs the same library modules as the
CLI:

- `ClockDriftTracker` + `period_for_ppm` keep the capture ring at its target
  fill, and `send_drift_ppm` is published to the UI.
- `SingleInstanceGuard` takes `Local\TeeHeeSend` for the session, so a CLI
  sender and the GUI cannot stream at once.
- `JsonlLogger` writes `send.jsonl` at 1 Hz with `send_start`, `send_stats`,
  `send_pause`/`send_resume`, `send_stop` and `send_error`. The path is shown
  in Settings and Diagnostics; it lives under the app-data directory
  (`%LOCALAPPDATA%\teehee\logs` on Windows).
- Capture counters (`callbacks`, `pushed_samples`, `nonzero_samples`,
  `overruns`, `underruns`, `latency_drops`) drive the health rules, so a
  loopback that opens but delivers no audio is distinguishable from a PC that
  is simply silent.

## Startup and recovery

Settings has the appliance controls that used to belong to Task Scheduler:

- **Open at login** (`Application.setLaunchAtLogin`).
- **Start streaming when teehee opens** (after saved-receiver hydration).
- **Reconnect automatically** (exponential backoff, 2 s → 30 s, while the
  sender is in `error`).
- **Desktop alerts** (`Notifications.show`, once per condition per session).

Closing the window quits the app; there is no tray menu yet, so these controls
require the app to be running.

## Next

1. Move the send loop into `teehee::session` so the CLI and GUI share one
   session owner.
2. Add the tray menu and close-to-tray while streaming.
3. Expose the control port and follow-system-volume in Advanced.
