# teehee GUI — design specification

A native desktop app for teehee, built with NSH. The visual reference is
[Spotish](https://github.com/huacnlee/spotish): one shell, a quiet persistent
rail, pages that do the work, and one floating bar at the bottom that always
answers "what is playing right now".

For teehee that bar answers **"is my sound getting there?"**

---

## 1. The product in one paragraph

teehee is a one-way audio bridge: a Windows PC captures what it is playing and
sends it over UDP to a Mac that plays it. Today the whole product is a CLI, a
Task Scheduler XML recipe, and PowerShell watchers that read JSONL. The GUI's
job is not to expose every flag. It is to make three things obvious at a
glance:

1. **Where the sound is going** — one receiver, named and reachable.
2. **Whether it is arriving** — live signal, not just a green "running" light.
3. **What to do when it is not** — one plain-language cause and one action.

Everything else (MTU tuning, drift compensation, capture rings) is real but
secondary. It belongs in Diagnostics, not on the home screen.

---

## 2. Design thesis

**A stream is the object, not the settings.**

The app is a signal path: **Source → Session → Receiver**. Every screen is a
view of that path. The primary action is always Start or Stop. Telemetry hangs
off the path, not in a dashboard grid.

Five rules that follow from it:

1. **The dangerous modes are explicit.** The `auto` capture source can silently
   broadcast a microphone. In the GUI there is no "auto": the source is a
   three-card choice with plain-language labels and a live input preview.
   System audio is the default on Windows.
2. **Never invent a number.** The sender knows packets, capture ring, drift and
   send errors. It does **not** know receiver playout latency. Show the
   receiver's prebuffer as a *setting*, not as a measurement, and label
   sender-side vs receiver-side everywhere.
3. **Silence is a state, not a failure.** WASAPI loopback produces no data for
   5–7 s after start and whenever nothing is playing. The UI says "Waiting for
   Windows to play something" with the real reason, not a spinner.
4. **Health is a sentence with an action.** Not a traffic light and not a wall
   of counters. `Packets are larger than the link MTU → Lower chunk size` is
   the unit of diagnosis.
5. **The app can be an appliance.** It replaces a scheduled task, so it must be
   able to start at login, go straight to the tray, stream to the saved
   receiver, and reconnect when the Mac comes back.

---

## 3. What to take from Spotish — and what to leave

**Take**

| Spotish | teehee equivalent |
|---|---|
| Sidebar + pages + floating player bar | Sidebar + pages + floating **Stream bar** |
| Player bar as the persistent transport | Stream bar: source → destination, level, state, pause, stop |
| Full-screen Now Playing | **Stream** page: same data, larger, no bar underneath |
| Adaptive rail: sidebar collapses to icons on narrow windows | Same, at 880 pt |
| `theme.json` tokens, dark/light, JetBrains Mono for numerals | Same system, different palette and semantics |
| Native menus, tooltips, keyboard regions | Same; tray menu instead of system media keys |

**Leave**

- Spotify green and album-art gradients. teehee is an instrument, not a media
  library.
- The seek bar. There is no timeline to scrub — the meaningful line is the
  network path, and it shows *flow*, not position.
- Library/collection navigation. teehee has at most a handful of receivers.
- Demo/sample-data modes. This app is always talking to real hardware.

---

## 4. Visual language

### 4.1 Concept: bench instrument, not dashboard

Muted graphite surfaces, one cool **signal** accent, monospaced numerals, and a
single animated element: the signal ribbon. No hero art, no card carousel, no
gradient meshes. The strongest visual moment is a thin line that comes alive
when audio flows through it.

### 4.2 Palette

The accent is teal, not red. Red is reserved for destructive actions and
errors; "live" is the presence of the signal itself. A red dot would read as
"recording" and fight every error state in the app.

`theme.json` sketch (NSH `cssVars` format):

```json
{
  "name": "teehee",
  "cssVars": {
    "theme": {
      "radius": "8px",
      "radius-inset": "6px",
      "radius-card": "12px",
      "radius-panel": "16px"
    },
    "light": {
      "background": "#f7f8f8",
      "foreground": "#121516",
      "card": "#ffffff",
      "card-foreground": "var(--foreground)",
      "popover": "#ffffff",
      "popover-foreground": "var(--foreground)",
      "secondary": "#eef1f1",
      "secondary-foreground": "var(--foreground)",
      "muted": "#e9eded",
      "muted-foreground": "#5f6b6d",
      "accent": "#e2f4f1",
      "accent-foreground": "#0b6b64",
      "primary": "#0f8f86",
      "primary-foreground": "#ffffff",
      "destructive": "#c93b45",
      "border": "#e1e6e5",
      "input": "#d4dbda",
      "ring": "#0f8f86",
      "sidebar": "#f0f3f2",
      "sidebar-foreground": "var(--foreground)",
      "sidebar-accent": "#e6ebea",
      "sidebar-accent-foreground": "var(--foreground)",
      "sidebar-border": "#e1e6e5",
      "live": "#0f8f86",
      "live-foreground": "#ffffff",
      "warning": "#a86500",
      "warning-foreground": "#ffffff",
      "idle": "#8a9698",
      "chart-1": "#0f8f86",
      "chart-2": "#3b82f6",
      "chart-3": "#8b5cf6",
      "chart-4": "#a86500",
      "chart-5": "#c93b45",
      "font-mono": "JetBrains Mono, Cascadia Mono, ui-monospace, monospace"
    },
    "dark": {
      "background": "#0b0e0f",
      "foreground": "#eef2f1",
      "card": "#121617",
      "card-foreground": "var(--foreground)",
      "popover": "#141819",
      "popover-foreground": "var(--foreground)",
      "secondary": "#1a1f20",
      "secondary-foreground": "var(--foreground)",
      "muted": "#242a2b",
      "muted-foreground": "#98a5a5",
      "accent": "#0e2b28",
      "accent-foreground": "#5eead4",
      "primary": "#2dd4bf",
      "primary-foreground": "#04211e",
      "destructive": "#f2686f",
      "border": "#232a2a",
      "input": "#2b3333",
      "ring": "#2dd4bf",
      "sidebar": "#080b0b",
      "sidebar-foreground": "var(--foreground)",
      "sidebar-accent": "#151b1b",
      "sidebar-accent-foreground": "var(--foreground)",
      "sidebar-border": "#232a2a",
      "live": "#2dd4bf",
      "live-foreground": "#04211e",
      "warning": "#e0a33e",
      "warning-foreground": "#1f1300",
      "idle": "#6b7778",
      "chart-1": "#2dd4bf",
      "chart-2": "#60a5fa",
      "chart-3": "#a78bfa",
      "chart-4": "#e0a33e",
      "chart-5": "#f2686f",
      "font-mono": "JetBrains Mono, Cascadia Mono, ui-monospace, monospace"
    }
  }
}
```

Semantic roles, so no screen uses literal colors:

| Token | Meaning | Used by |
|---|---|---|
| `primary` / `live` | Signal is flowing | Start button, ribbon, level meter, Live chip |
| `warning` | Healthy stream, wrong configuration | MTU fragmentation, ring saturation |
| `destructive` | Errors and destructive actions | Stop confirmation, unreachable receiver |
| `idle` | Configured but not sending | "Paused", "Waiting" dot |
| `chart-1..5` | Packets, ring depth, drift, drops | Sparklines and legend |

### 4.3 Type

Desktop density, system sans, mono for every number that updates.

| Role | Size / line | Weight | Use |
|---|---|---|---|
| Display | 28 / 34 | 600 | Elapsed session time (mono), never headings |
| Title | 20 / 26 | 600 | Page title |
| Heading | 15 / 20 | 600 | Section title, card title |
| Body | 13 / 19 | 400 | Default |
| Label | 13 / 18 | 500 | Control labels, row titles |
| Small | 12 / 16 | 400 | Descriptions, helper text |
| Caption | 11 / 14, +0.04em | 500, uppercase | Stat labels, group headers |
| Mono | 12 / 18 | 400 | Counters, addresses, ppm, ms |

Numbers are always `font-mono` and right-aligned in a stat lane, so digits do
not jitter as they change. Use a fixed-width numeric lane even for "0 drops".

### 4.4 Space, shape, elevation

- 4 pt scale only: 2, 4, 8, 12, 16, 24, 32.
- Page padding 24; card padding 16; row height 44 (compact 32); control height
  32; small control 28.
- Radii: 6 inset, 8 controls, 12 cards, 16 panels and sheets. The floating bar
  is 12, matching a card, because it is a control surface rather than a sheet.
- One elevation level: the floating stream bar (border + soft shadow) and
  popovers/sheets. Cards use a hairline border only. No nested cards.

### 4.5 Iconography and illustration

- 16 pt line icons, 1.5 pt stroke, rounded caps; 20 pt in empty states.
- Device icons: PC (tower), Mac (laptop), generic speaker. No brand logos.
- Empty states get a two-device line diagram with a dashed path between them,
  not an illustration of a person with headphones.

### 4.6 Motion

- Hover/press: 120 ms, ease-out, color/opacity only.
- Layout changes (sidebar rail, panel expand): 180 ms.
- Sheets/dialogs: 220 ms with the platform's spring; content fades 8 pt up.
- The ribbon's traveling highlight moves only while `peak > silence floor`;
  when silent it dims and stops. This is the app's one piece of continuous
  animation, and it is data, not decoration.
- Level meter: 30 Hz, 80 ms attack / 240 ms release, with a 1.5 s peak-hold
  tick. Reduce Motion replaces the ribbon motion with a static tint and the
  meter with a 5 Hz stepped bar.

---

## 5. Information architecture

Window: **1040 × 700** default, minimum **780 × 560**.

```
┌──────────────────┬──────────────────────────────────────────────┐
│ teehee           │                                              │
│                  │                                              │
│  [ Send | Recv ] │                 page                         │
│                  │                                              │
│  Stream          │                                              │
│  Receivers       │                                              │
│  Audio           │                                              │
│  Diagnostics     │                                              │
│                  │                                              │
│                  │                                              │
│  Settings        │                                              │
└──────────────────┴──────────────────────────────────────────────┘
```

- Sidebar 208 pt; collapses to a 64 pt icon rail below 880 pt. Page padding
  adjusts so the header spine stays aligned.
- Role switch at the top, under the app name. Default role is **Send** on
  Windows and **Receive** on macOS/Linux. It is a segmented control, because
  the two roles are exclusive (one process is either sending or receiving).
- Navigation items change with the role:

  **Send:** Stream · Receivers · Audio · Diagnostics
  **Receive:** Stream · Output · Diagnostics

- Settings is pinned to the bottom, above the version.
- The floating **Stream bar** (64 pt, inset 12 pt, radius 12) is visible on
  every page except Stream itself. On Stream it would duplicate the page, the
  same way Spotish hides its player bar under Now Playing.

### 5.1 The Stream bar

```
┌──────────────────────────────────────────────────────────────────────────┐
│ ●  PC  ━━━━━▮▮▮━━━━━▶  Living room MacBook    18 ms   0 drops   ‖   ■    │
└──────────────────────────────────────────────────────────────────────────┘
```

- Left: state dot + role icon.
- Center: the signal ribbon, destination name, and the two numbers that matter
  right now (sender-side delay, drops). Clicking the ribbon opens Stream.
- Right: pause/resume (ghost icon), stop (ghost icon). Stop is never the filled
  primary in the bar; the page owns the primary action.
- Idle: `Not streaming · Living room MacBook` with a **Start** text button.
- Waiting: `Waiting for Windows audio` with a dim ribbon.
- Error: `Receiver is not listening` with a **Fix** button that opens the
  health card.

---

## 6. Screens

### 6.1 Stream — idle (this is also first run)

The Stream page is the setup form when idle and the monitor when live. No
separate wizard, no modal on first launch; defaults are already valid, and a
new user can press Start after choosing a receiver.

```
Start a stream
Send this PC's sound to a Mac on your network.

Source
┌────────────────────────────────────────────────────────────────────┐
│ (•)  System audio       Everything you hear on this PC             │
│      WASAPI loopback · Windows only                                │
│                                                                    │
│ ( )  Microphone         The default recording device               │
│      Stereo Mix, a mic, or a virtual cable                         │
│                                                                    │
│ ( )  Test tone          A 440 Hz tone, to check the Mac            │
│                                                                    │
│      ┌────────────────────────────────────────────────────┐        │
│      │ ▂▃▅▇▆▅▃▂▁▂▃▅▆▇▆▅▃▂   input preview · -24 dBFS       │        │
│      └────────────────────────────────────────────────────┘        │
└────────────────────────────────────────────────────────────────────┘

Receiver
┌────────────────────────────────────────────────────────────────────┐
│ ●  Living room MacBook      192.168.1.111:5000      found 2m ago   │
│ ○  Studio iMac              192.168.1.140:5000      found 1h ago   │
│ ○  Add an IP address…                                              │
└────────────────────────────────────────────────────────────────────┘
  No receiver listed?  Show the command to run on the Mac  →

Quality   [ Low latency ]  [ Balanced ✓ ]  [ Stable ]        Advanced ▸
Chunk 3 ms · ~333 packets/s · fits a 1500-byte MTU · receiver buffer 200 ms

                                  [ Start streaming ]
  Audio is unencrypted. Use only on a network you trust.
```

Behavior and copy:

- Source preview starts when a source is selected, before streaming. This is
  how the silent-mic pitfall becomes impossible: the user sees the wrong input
  before pressing Start.
- If the selected source produces no samples for 3 s, the preview says
  `No audio yet — play something on this PC` rather than showing a flat meter.
- Receiver rows show how they were found (`found 2m ago`, `saved`, `manual`).
- The quality presets are the primary control. Advanced reveals chunk-ms, MTU,
  capture buffer, follow-system-volume, and control port. Every advanced field
  shows its effect in one line (`3 ms chunks ≈ 333 packets/s`).
- The trust line is a caption, not an alert. It appears again in the first
  live run as a one-time acknowledgement sheet, and never as a nag.
- If the saved receiver is missing, the primary button becomes **Find
  receiver** and shows the mDNS timeout explanation inline.

### 6.2 Stream — live

```
← Living room MacBook                                      ● Live   ⋯
Sending this PC's system audio

┌────────────────────────────────────────────────────────────────────┐
│                                                                    │
│      PC  ━━━━━━━━━▮▮▮▮▮━━━━━━━━━━━━━━━━▶  MacBook                 │
│      System audio                         192.168.1.111:5000      │
│                                                                    │
│      ████████████████████░░░░░░░░░░░░░░   -16 dBFS   peak -9       │
│                                                                    │
│      00:12:47        48 kHz · stereo · 3 ms         sender delay 18 ms │
└────────────────────────────────────────────────────────────────────┘

            [ Pause ]                    [ Stop streaming ]

All clear · last checked just now
┌ Packets ─────────┐ ┌ Drift ───────────┐ ┌ Capture ring ────────┐
│ 332.6 /s         │ │ +12 ppm          │ │ 14 ms  of 120 ms     │
│ 37,410 sent      │ │ within ±150      │ │ 0 overruns           │
└──────────────────┘ └──────────────────┘ └──────────────────────┘

Signal · last 60 seconds                                    [ 1m | 5m ]
┌────────────────────────────────────────────────────────────────────┐
│ 500 ┤   ▁▂▃▅▆▇▆▅▃▂▁▂▃▅▆▇▆▅▃▂▁▂▃▅▆▇▆▅▃▂▁                          │
│   0 └────────────────────────────────────────────────────────────┘
│      packets per second          ring ms          drops            │
└────────────────────────────────────────────────────────────────────┘

Session
  Started        today at 14:32
  Source         WASAPI loopback · default render endpoint
  Receiver       Living room MacBook · 192.168.1.111:5000
  Log            logs/send.jsonl                      Open folder
  Control port   9090 · localhost only
```

Rules:

- The hero card uses `bg-card` with a hairline; the ribbon and meter are the
  only saturated elements.
- Destinations always show address and port. A name alone is not enough to
  debug a LAN.
- The elapsed time is a display-size mono number; everything else is compact.
- `sender delay` is explicitly sender-side: chunk + capture ring. Receiver
  prebuffer is shown in Session as a receiver setting, not added to it.
- The Health card appears above the stats only when there is something to say.
  When all is clear, one line: `All clear · last checked just now`.
- Stop is filled (`primary`), Pause is outline. Stopping while live is
  immediate — no confirmation dialog; the stream is trivially restartable and
  the confirmation would be ceremony.
- `⋯` menu: Copy diagnostics, Open log folder, Edit settings, Disconnect.

### 6.3 Receivers

```
Receivers                                             [ Add manually ]

On this network                                        Searching…  ↻
┌────────────────────────────────────────────────────────────────────┐
│ ●  Living room MacBook     192.168.1.111:5000   found just now  Use│
│ ○  Studio iMac             192.168.1.140:5000   found 1h ago    Use│
└────────────────────────────────────────────────────────────────────┘

Saved
┌────────────────────────────────────────────────────────────────────┐
│    Basement Mac mini       10.0.0.42:5000       saved            ⋯ │
└────────────────────────────────────────────────────────────────────┘

  Receivers advertise as _teehee._udp.local. when started with --mdns.
  Receivers are not authenticated. Only join networks you trust.
```

Empty state:

```
        ┌────┐        ┄┄┄┄┄        ┌────┐
        │ PC │                     │ Mac│
        └────┘                     └────┘

        No receivers found
        Make sure the Mac is running teehee recv with --mdns,
        then search again.

        [ Search again ]   [ Add an IP address ]

        Run this on the Mac:
        ./teehee recv --port 5000 --mdns --prebuffer-ms 200    [ Copy ]
```

- The exact command is the most useful content on this page. It removes the
  need to read the README while sitting at the other machine.
- `Use` selects the receiver and returns to Stream. It does not start
  streaming.
- Rename is local-only (`⋯ → Rename`). The mDNS advert currently carries only
  `v=1`; until the engine adds a friendly name, display the service instance
  and address.
- Manual add opens a small dialog: address (accepts `host`, `host:port`,
  `[v6]:port`), optional name, **Use receiver**. Validation mirrors
  `ResolvedTarget` / `parse_host_port`; errors are inline and name the accepted
  forms. Do not duplicate the CLI's "ambiguous port" trap in the UI: if the
  address embeds a port, hide the separate port field.

### 6.4 Audio & network (Send)

```
Audio & network

Capture
┌ System audio ──────────────────────────────────────────────────────┐
│ WASAPI loopback of the default render endpoint.                    │
│ Captures everything: browser, Spotify, games, system sounds.       │
│                                              [ input preview ]     │
└────────────────────────────────────────────────────────────────────┘
┌ Microphone ────────────────────────────────────────────────────────┐
│ The Windows default recording device. This is a microphone unless  │
│ you have Stereo Mix or a virtual cable selected.                   │
└────────────────────────────────────────────────────────────────────┘

Quality                         chunk · packet rate · 1500-byte MTU
  ( ) Low latency    2 ms  · 500 packets/s · fits      (0.8 kB)
  (•) Balanced       3 ms  · 333 packets/s · fits      (1.2 kB)
  ( ) Stable        20 ms  ·  50 packets/s · fragments  (7.7 kB)

Advanced
  Chunk size        [ 3 ms         ]   packet 1.2 kB · envelope 1.4 kB · fits
  Link MTU          [ 1500 bytes ▾ ]   576 / 1280 / 1500 / 9000
  Capture buffer    [ 200 ms       ]   drop-oldest above this
  Follow Windows volume                                [ switch ]
  Control port      [ 9090         ]   localhost only, 0 disables
  Log file          [ logs/send.jsonl ]                [ Choose… ]
```

- Low latency/Balanced/Stable are presets over `chunk-ms` (2/3/20). Selecting
  a preset updates Advanced; editing Advanced switches the preset to Custom.
  Balanced (3 ms) matches the task-scheduler deployment and is the default on
  a 1500-byte link, because it is the largest chunk that still fits.
- MTU is not hidden in Advanced by accident: at 48 kHz stereo f32, 20 ms is
  7,680 bytes of payload plus the 25-byte header, while a 1500-byte link MTU
  leaves only 1,447 bytes after IP + UDP + teehee framing. The CLI treats this
  as a `fragmentations` counter; the GUI shows it as a first-class state. Each
  preset row says `fits` or `fragments` for the current MTU, and choosing a
  fragmenting preset raises an inline `warning` before Start:
  `Each packet is 7.7 kB, but a 1500-byte MTU allows 1.4 kB. Windows will
  fragment them. Use 3 ms chunks or raise the MTU to 9000 on a jumbo-frame
  network.`
- Follow-system-volume is Windows-only; elsewhere the switch is disabled with
  `Windows only` as its description.
- Every field validates on change, inline, before it reaches the engine.

### 6.5 Diagnostics

```
Diagnostics                                    [ Copy report ]  [ Clear ]

Checks
  ✓  Receiver is not rejecting packets        0 send errors
  ✓  Packets fit the link MTU                 0 fragmentations
  ⚠  Capture ring is close to its cap         96 of 120 ms for 12 s
     The encode loop is being preempted. Close CPU-heavy apps or raise
     the capture buffer.                         [ Raise to 200 ms ]

Live counters                                           last 60 s
  PACKETS    332.6 /s     149,640 sent     0 errors     0 fragmented
  CAPTURE     14 ms ring   0 overruns      0 latency drops
  CLOCK       +12 ppm      slope +0.4 f/s  0 encode stalls
  SESSION     live 12:47   paused no       gain 1.00 (manual)

Capture ring · ms                                              [ 1m ]
┌────────────────────────────────────────────────────────────────────┐
│ 120 ┤                                                              │
│  60 ┤      ▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃▂▃              │
│   0 └────────────────────────────────────────────────────────────┘
└────────────────────────────────────────────────────────────────────┘

Events                                              [ All ▾ ]  [ Follow ]
  14:44:58  info   send_stats  332.6 pkt/s · ring 14 ms · +12 ppm
  14:43:12  warn   fragmentation  3 packets exceeded the MTU envelope
  14:32:05  info   send_start  target 192.168.1.111:5000 · loopback
```

- The Checks list is the diagnosis surface. Counters are evidence underneath
  it, collapsed by default on short windows.
- Event rows are one line, mono timestamp, level as a small neutral chip
  (`info`, `warn`, `error`) — color is a secondary cue, never the only one.
- `Copy report` puts version, OS, resolved config, the checks, and the last
  60 s of stats on the clipboard as Markdown. Receiver IPs are included only
  if "Include addresses" is checked in Settings; default off for pasting into
  issues.
- The log tail is read-only. There is no "open a shell" button.

### 6.6 Receive mode

```
Listening on :5000                                          ● Live
Waiting for a sender on this network.

┌────────────────────────────────────────────────────────────────────┐
│   level  ██████████████░░░░░░░░░░░░░░░░░░   -18 dBFS               │
│                                                                    │
│   buffer  ▓▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░░░░░░░░░░░   74 of 2000 ms          │
│           └ prebuffer gate 200 ms                                  │
│                                                                    │
│   48 kHz · stereo  →  44.1 kHz · stereo  · resampling              │
└────────────────────────────────────────────────────────────────────┘

            [ Pause playback ]          [ Stop listening ]

┌ Queue ───────────┐ ┌ Drift ───────────┐ ┌ Network ─────────────┐
│ 74 ms            │ │ -8 ppm           │ │ 0 late · 0 dup       │
│ target 200 ms    │ │ within ±150      │ │ 0 silence · 0 resync │
└──────────────────┘ └──────────────────┘ └──────────────────────┘

Output
  Device      MacBook Pro Speakers (default)              [ Change ]
  Prebuffer   [ 200 ms ]   Lower starts faster; higher survives weak Wi-Fi
  Ring        [ 2000 ms ]  Must be at least the prebuffer
```

- The buffer visualization is the receiver's signature element: a ring with a
  playhead, a gate marker, and a fill that visibly sits between the two. It
  makes `--prebuffer-ms` and `--rx-buffer-ms` understandable without the
  README's prose.
- The listening state is honest about waiting: `Waiting for a sender` with the
  address it is bound to, plus which sender last arrived and when.
- Receiver-mode health checks differ: queue empty → raise prebuffer; queue near
  ring → raise ring or lower sender chunk; resampling active is `info`, not a
  warning.
- "This Mac" is the only role where Receive is default. On Windows the role
  switch remains available for loopback testing on one machine.

### 6.7 Settings

```
Appearance        System / Light / Dark
Behavior
  Open at login                                          [ switch ]
  Start streaming when teehee opens                      [ switch ]
  Keep running in the tray when the window closes        [ switch ]
  Reconnect automatically when the receiver is missing    [ switch ]
  Reopen the last role (Send / Receive)                  [ switch ]
Privacy
  Include addresses when copying a report                [ switch ]
Logs
  Path              logs/                                               Change
  Keep archives     8 files · 90 days                [ Rotate now ]
About
  teehee 0.2.1 · NSH 0.5
  This app sends audio unencrypted. It is not safe on public networks.
```

### 6.8 Tray

```
teehee — Live to Living room MacBook
────────────────────────────────────
  Pause streaming
  Stop streaming
  Send to            ▸  Living room MacBook
                        Studio iMac
────────────────────────────────────
  Open teehee
  Diagnostics
────────────────────────────────────
  Quit
```

- Tray icon: filled signal mark while live, outlined while idle, warning badge
  on error. No animation in the tray.
- Tooltip: `teehee · 332.6 pkt/s · 18 ms`.
- Closing the window hides to the tray only while streaming; when idle it
  closes. The first time it hides, a toast says
  `teehee is still streaming from the tray.` with a **Quit** action.
- Left-click opens the window on the Stream page; right-click opens the menu.

---

## 7. Component inventory (app-owned `ui` layer)

Per NSH's design guide, screens compose `ui`, never `nsh/base` directly and
never colors.

| Component | Variants / parts | Built from |
|---|---|---|
| `AppShell` | sidebar, page, stream bar | `View`, `Scroll` |
| `Sidebar` | expanded 208 / rail 64 | `View`, `Toolbar` |
| `RoleSwitch` | Send / Receive | App `SegmentedControl` |
| `PageHeader` | title, subtitle, actions, state chip | `View`, `Text`, `Button` |
| `Card` | default, inset, interactive | `View` |
| `StatTile` | label, value, unit, delta, tone | `Text` (mono) |
| `StateChip` | live, idle, waiting, warning, error | `Badge` + dot |
| `SignalPath` | compact (bar), hero (page) | `Canvas` ribbon + `Text` |
| `LevelMeter` | input, output, peak-hold | `Canvas` |
| `BufferTank` | fill, gate, playhead | `Canvas` |
| `Sparkline` | line, area, threshold line | `Canvas` |
| `DeviceRow` | found, saved, selected, disabled | `ListItem`, `Badge`, `Menu` |
| `SourceCard` | radio card with preview slot | `Button`, `View` |
| `PresetGroup` | three presets + Custom | `SegmentedControl` |
| `Callout` | info, warning, error, action | `View`, `Text`, `Button` |
| `HealthCard` | clear, warning, error, multiple | `Card` + `Callout` |
| `EmptyState` | diagram, title, body, actions, code block | `View`, `Button` |
| `EventLog` | row, filter, follow | `List` |
| `SettingsRow` | label, description, control | `View` |
| `CodeBlock` | command, copy | `Text` (mono), `Button` |
| `StreamBar` | live, idle, waiting, error | `Card` + `SignalPath` |

Charts are `Canvas` redraws from signals; at 1 Hz telemetry a 60-point
sparkline is cheap and needs no animation engine. The level meter is the only
30 Hz surface.

Window chrome:

- v1 keeps the **system title bar**. It is correct on Windows and avoids
  hand-drawing minimize/maximize/close.
- v1.1 can hide it (`"titleBar": { "style": "hidden", "height": 44 }`, as
  Spotish does) once the app header includes the desktop module's window
  controls and a drag region. The app header then carries the destination and
  state chip, which is a genuine improvement.

---

## 8. State model

```ts
type Role = "send" | "recv";
type Phase = "idle" | "starting" | "waiting" | "live" | "degraded" | "paused" | "error";

interface SendConfig {
  receiver: Receiver | null;
  source: "loopback" | "microphone" | "sine";
  chunkMs: number;
  mtu: number;
  captureBufferMs: number;
  followSystemVolume: boolean;
  controlPort: number;
  logFile: string;
}

interface SendTelemetry {
  tsMs: number;
  packetsSent: number;
  packetsPerSec: number;
  captureRingMs: number;
  captureOverruns: number;
  captureLatencyDrops: number;
  captureNonzeroSamples: number;
  sendErrors: number;
  fragmentations: number;
  driftPpm: number;
  peakDbfs: number;      // engine addition, see §9
  paused: boolean;
  gain: number;
}
```

Signals (one store module per domain, not one global):

- `session.ts` — role, phase, startedAt, config, errors.
- `telemetry.ts` — last 300 samples, derived rates, peak/peak-hold.
- `devices.ts` — discovered, saved, selection, discovery status.
- `health.ts` — derived `Issue[]` from telemetry + config.
- `overlays.ts` — dialogs, sheets, first-run acknowledgement.

Phase is derived, never set by the UI alone:

```
idle      → starting  (Start pressed)
starting  → waiting   (engine ran, packetsSent == 0 for > 1 s)
waiting   → live      (packetsPerSec > 0 and peak above floor)
live      → paused    (Pause pressed)
live      → degraded  (any warning-severity issue)
live      → waiting   (packetsPerSec == 0 for > 3 s)
any       → error     (engine could not start / exited non-zero)
```

### Health rules (the heart of the app)

Each rule produces `{ severity, title, detail, action }`.

| Condition | Severity | Title | Action |
|---|---|---|---|
| `send_errors` increasing | error | The receiver is not answering | Check `teehee recv`, UDP 5000 and the firewall |
| `fragmentations` increasing | warning | Packets are larger than the link MTU | Lower chunk size or raise MTU |
| configured chunk × format > envelope | warning | This configuration will fragment | Pick a preset that fits |
| `packetsPerSec == 0`, < 7 s since start | info | Waiting for Windows audio | none |
| `packetsPerSec == 0`, `capture_nonzero_samples` unchanged ≥ 3 s | info | Nothing is playing on this PC | none |
| `capture_ring_ms > 0.8 × captureBufferMs` for 5 s | warning | The encode loop is being preempted | Raise capture buffer / close CPU-heavy apps |
| `capture_latency_drops` increasing | warning | Audio is skipping to stay live | Check CPU load; raise chunk size |
| `capture_overruns` increasing | warning | Capture buffer is too small | Raise capture buffer |
| `|send_drift_ppm| >= 150` for 10 s **and** nonzero samples in last 2 min | warning | Clock correction is saturated | none (it recovers); link to explanation |
| `|send_drift_ppm| >= 150` while silent | info (suppressed) | — | hidden |
| Receiver `underrun_resyncs` increasing | warning | The buffer ran dry | Raise prebuffer |
| Receiver `late_drops` increasing | warning | Packets arrive too late | Raise sender chunk size |
| Receiver `ring_overruns` increasing | warning | Sender is outrunning the ring | Raise ring or lower sender chunk |
| Receiver `format_conversion_active` | info | Resampling 48 kHz → 44.1 kHz | none |
| `Local\TeeHeeSend` mutex held | error | teehee is already running outside this window | Attach or quit that instance |

Two rules are deliberately *not* shown: packet loss (UDP gives no sender-side
signal) and end-to-end latency (requires a receiver clock). Showing either
would be a lie.

---

## 9. Engine contract

The UI cannot be a pure TSX app. It needs process ownership, a local TCP
control socket, log files, and (later) the audio engine in-process. TSX in NSH
has no process or raw-socket access, so the app must be a **Rust NSH app**,
optionally with TSX screens on top.

### 9.1 Shape

```
teehee-gui (Rust NSH app)
├── engine/          Rust: owns the session
│   ├── sidecar.rs   spawn teehee.exe, parse JSONL, control TCP   (phase 1)
│   ├── embedded.rs  call teehee::session::{Sender,Receiver}      (phase 2)
│   ├── discovery.rs mDNS browse, saved receivers
│   └── module.rs    #[native_module] status/start/stop + signals
├── ui/              Rust or TSX styled components (one layer)
├── pages/           Stream, Receivers, Audio, Diagnostics, Settings
└── tray.rs          tray menu, close-to-tray, login item
```

The UI reads a `Status` signal published by Rust at 10 Hz (counters) and reads
a separate `Peak` signal at 30 Hz. It never parses logs; logs are history for
Diagnostics.

### 9.2 Phase 1: sidecar (ship fast, no engine refactor)

- Spawn `teehee.exe send --host … --control-port 9090 --log-file …`.
- Tail `send.jsonl` for the 1 Hz `send_stats` records.
- Use the existing control socket for pause/resume/gain/status.
- Stop = graceful shutdown; on Windows, closing stdin is not a stop signal, so
  phase 1 must terminate the child process. Accept this temporary roughness.

Limits to state in the UI: level meter is synthesized from
`capture_nonzero_samples` and `packets_per_sec` (a flow indicator, not a true
peak), and receiver telemetry is unavailable while sending.

### 9.3 Phase 2: embedded (the real design)

The current `run_send` / `run_recv` live in `main.rs` (2,130 lines) and block
until exit. To embed, move them into `teehee::session` behind a handle:

```rust
pub struct Sender { /* owns threads, sockets, cpal stream */ }

impl Sender {
    pub fn start(config: SendConfig) -> Result<Self>;
    pub fn status(&self) -> SendStatus;   // atomics, cheap, 30 Hz safe
    pub fn set_paused(&self, paused: bool);
    pub fn set_gain(&self, gain: f32);
    pub fn stop(self);                    // clears the stop flag and joins
}
```

Additions to the existing telemetry (all additive, no wire changes):

| Need | Today | Add |
|---|---|---|
| Level meter | `capture_nonzero_samples` only | `peak` (f32, per callback) and `rms` in `CaptureRing::stats` |
| Pause/volume | control TCP, send only | direct methods on `Sender` / `Receiver` |
| Receiver control | none | `Receiver::{set_paused, set_prebuffer, status}` |
| Discovery | sender browses once at startup | `discovery::browse(Duration) -> Vec<Peer>` with name/addr/last-seen |
| Readable receiver names | TXT `v=1` only | add `name` and `platform` TXT fields |
| Single instance | `Local\TeeHeeSend` mutex | try-lock as an API so the GUI can show "already running" |
| History | JSONL | keep; Diagnostics still reads it |

The embedded path also fixes the `--control-port` split: one local API for the
GUI, while the CLI keeps its TCP server for scripts.

### 9.4 What the GUI must not do

- Do not reimplement the audio pipeline or resampling.
- Do not run a second sender when the mutex says one exists.
- Do not start a receiver silently on app launch; receiving plays audio, so it
  needs an explicit action (or an explicit "start receiving at login" setting).

---

## 10. Interaction details

### Keyboard

| Key | Action |
|---|---|
| `Ctrl+1…4` | Switch page |
| `Ctrl+Enter` | Start / stop the stream |
| `Ctrl+Shift+P` | Pause / resume |
| `Ctrl+,` | Settings |
| `Ctrl+D` | Diagnostics |
| `Escape` | Close sheet/dialog; clear search |
| `Tab` / arrows | Move between pages, then within the page |

`Space` is not bound: it scrolls and toggles controls. All shortcuts appear in
tooltips and in the `?` shortcuts sheet.

### Focus and regions

- Sidebar is one tab stop group; the page another; the stream bar last.
- The level meter and charts are not focusable. Values are exposed as
  accessibility values (`"332.6 packets per second"`).
- The primary button takes initial focus when Stream mounts idle, so Enter
  starts the stream.

### States that must exist for every control

Hover, pressed, focus-visible, selected, disabled, busy, validation error.
The app's `Button`, `Input`, `Switch`, `SegmentedControl` and `Row` own all of
them; pages never restyle.

### Copy

Plain, specific, no exclamation marks, no "Oops".

| Situation | Copy |
|---|---|
| No receiver | `No receivers found` / `Make sure the Mac is running teehee recv with --mdns, then search again.` |
| Wrong input | `This is a microphone` / `System audio needs the WASAPI loopback source.` |
| Warm-up | `Waiting for Windows audio` / `Loopback can take up to 7 seconds to produce its first samples.` |
| Idle audio | `Nothing is playing on this PC` / `Windows stops producing loopback audio when the system is silent.` |
| MTU | `Packets are larger than the link MTU` / `Windows will fragment them. Use 3 ms chunks or raise the MTU to 9000.` |
| Drift | `Clock correction is saturated` / `Usually recovers on its own after a burst or a pause.` |
| Already running | `teehee is already running outside this window` / `Quit the other copy, or connect to its control port.` |
| Trust | `Audio is unencrypted` / `Use only on a network you trust.` |

---

## 11. Build plan

**M0 — Foundations (1–2 days).** `nsh init teehee-gui --template rust`, theme
tokens, `ui/` core set, shell with sidebar/rail, blank pages, light/dark and
high-contrast review. No engine.

**M1 — Stream, idle and live, send only (2–3 days).** Manual receiver entry;
sidecar start/stop; JSONL tail; config → CLI args mapping; phase derivation;
primary/secondary actions; session details.

**M2 — Receivers and first run (2 days).** mDNS browse via a Rust helper;
saved receivers; manual add dialog; empty state with the copyable receiver
command; first live-run trust acknowledgement.

**M3 — Health and Diagnostics (2–3 days).** Rules table, `Callout`/`HealthCard`,
checks list, 60 s sparklines, event log, copy report. This milestone is what
makes the app better than the CLI.

**M4 — Receive mode (2 days).** Bind, output picker, prebuffer/ring controls
with the buffer tank, receiver rules, receiver telemetry.

**M5 — Appliance behavior (1–2 days).** Tray menu, close-to-tray, open at
login, reconnect with a saved receiver, single-instance handling, window
position/size and last role persisted.

**M6 — Embedded engine (3–4 days).** Move `run_send`/`run_recv` into
`teehee::session`, add peak/RMS, receiver control, discovery API, try-lock
mutex; delete the sidecar path; add headless UI tests with
`nsh_testing`/`nsh test`.

M1–M3 are the honest MVP. M4 makes it useful on the Mac. M6 removes the
fragile parts; do it before calling the app stable.

### File layout

```
gui/
  nsh.json                 window, theme, id, rust crate
  theme.json               §4.2
  App.tsx                  shell + role switch + overlays (or app.rs if Rust-only)
  src/
    ui/                    button, input, switch, segmented, badge, card, meter,
                           sparkline, callout, dialog, sheet, toast, list-row
    components/            app-shell, sidebar, role-switch, stream-bar,
                           signal-path, level-meter, buffer-tank, health-card,
                           device-row, source-card, stat-tile, event-log
    pages/                 send-stream, send-audio, receivers, diagnostics,
                           receive-stream, receive-output, settings
    state/                 session, telemetry, devices, health, overlays
    engine/                Rust: sidecar/embedded, discovery, tray, native module
  assets/                  icons (SVG), app icon
```

Keep the engine boundary in one directory and the `ui` layer free of engine
imports, so the sidecar→embedded switch touches one module.

---

## 12. Decisions taken (and what is still open)

**Taken**

- NSH, desktop-first, Windows sender + macOS receiver; one app, two roles.
- The Stream page is setup when idle and the monitor when live.
- A floating Stream bar is the persistent transport, hidden on Stream.
- Teal signal accent; red reserved for errors; mono numerals.
- No fake latency or loss numbers; sender-side and receiver-side data are
  labeled.
- The app owns the engine session; phase 1 may use a sidecar, phase 2 embeds.
- Health is prose plus an action, not a dashboard of gauges.

**Open — worth deciding before M1**

1. **Rust-only UI or Rust + TSX?** Rust is required for the engine. TSX gives
   faster iteration and matches Spotish, at the cost of a QuickJS layer and
   interop plumbing. Recommendation: **Rust app + TSX screens** for the `ui`
   and pages, with a thin Rust module for engine, tray and discovery.
2. **Is refactoring `main.rs` into `teehee::session` acceptable?** If not, M6
   slips indefinitely and the app ships with sidecar quirks. Recommendation:
   accept the refactor; it also makes the engine testable.
3. **Should the GUI replace the Task Scheduler recipe entirely?** If yes,
   "Open at login" and "Reconnect" must be excellent, and the app should
   surface the existing `Local\TeeHeeSend` mutex state. Recommendation: yes for
   interactive desktops, keep the task recipe for headless/fleet machines.
4. **A second window or a sheet for Now Streaming?** Spotish uses a full-screen
   sheet. teehee is small enough that the Stream page is the full view; no
   second window. Recommendation: sheet only for the first-run trust and
   manual-add dialogs.
5. **Do we add receiver-side pairing/auth in this design?** Not in v1. The UI
   states the trust model plainly; authentication is a protocol change, not a
   screen.
