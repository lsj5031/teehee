# teehee GUI

The native desktop app for teehee, built with NSH: one Rust crate and TSX
screens on the same engine. The visual and interaction design is in
[`docs/gui-design.md`](../docs/gui-design.md).

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
src/lib.rs              Rust side; the engine boundary lands here
src/state/session.ts    mock session with the shape of the Rust status
src/ui/                 app-owned component layer (button, chip, card, ...)
src/components/         shell pieces and data views
src/pages/              Stream, Receivers, Audio, Diagnostics, Settings
assets/icons/           iced SVG line icons (tinted from the theme)
```

## Mock, not engine

`src/state/session.ts` simulates a sender and a receiver so the pages,
transitions and health states can be built and tested without hardware. It is
intentionally one file: the real implementation publishes the same values from
Rust (`teehee::session`), and the pages change only at their imports.

The mock already models the parts that matter to the UI:

- `starting` → `live` after the loopback warm-up, and paused/resumed phases.
- packet rate from `chunk-ms`, capture ring, drift, fragmentation when the
  packet exceeds the MTU envelope, and receiver queue/underrun.
- derived health issues with one sentence and one cause each.

## Next

1. Move `run_send` / `run_recv` into `teehee::session` behind a
   start/stop-able handle.
2. Add `peak` / `rms` to the capture stats for the real level meter.
3. Replace `session.ts` with a Rust module exported through `teehee/native`.
4. Add mDNS browsing and saved receivers, then the tray.
