/**
 * The UI's session state, backed by the native teehee engine.
 *
 * The engine publishes a JSON snapshot; this module polls it and maps it
 * onto the signals the screens read. No telemetry is synthesized here.
 */
import { computed, Notifications, signal, Storage } from "nsh";
import { Application } from "nsh/desktop";
import { Engine } from "teehee/native";

export type Role = "send" | "recv";
export type Page =
  | "stream"
  | "receivers"
  | "audio"
  | "diagnostics"
  | "settings";
export type Phase = "idle" | "starting" | "live" | "paused" | "error";
export type SourceId = "system" | "microphone" | "sine";
export type Preset = "low" | "balanced" | "stable" | "custom";
export type Severity = "info" | "warning" | "error";

export interface Receiver {
  id: string;
  name: string;
  host: string;
  port: number;
  seen: string;
  saved: boolean;
}

export interface Issue {
  severity: Severity;
  title: string;
  detail: string;
}

export interface LogEvent {
  time: string;
  level: "info" | "warn" | "error";
  text: string;
}

interface NativeSend {
  phase: string;
  error: string | null;
  packets_sent: number;
  packets_per_sec: number;
  peak: number;
  ring_ms: number;
  capture_callbacks: number;
  capture_pushed_samples: number;
  capture_nonzero_samples: number;
  capture_overruns: number;
  capture_underruns: number;
  capture_latency_drops: number;
  capture_high_water_ms: number;
  capture_ring_samples: number;
  drift_ppm: number | null;
  fragmentations: number;
  send_errors: number;
  elapsed_ms: number;
  sample_rate: number;
  channels: number;
  chunk_ms: number;
  source: string;
}

interface NativeRecv {
  phase: string;
  error: string | null;
  packets_received: number;
  packets_per_sec: number;
  peak: number;
  queued_ms: number;
  late_drops: number;
  underruns: number;
  decode_errors: number;
  elapsed_ms: number;
  sample_rate: number;
  channels: number;
  drift_ppm: number | null;
}

interface NativeReceiver {
  id: string;
  name: string;
  host: string;
  port: number;
  seen: string;
}

interface NativeStatus {
  log_path: string | null;
  send: NativeSend;
  recv: NativeRecv;
  receivers: {
    searching: boolean;
    error: string | null;
    items: NativeReceiver[];
  };
}

const RECEIVERS_KEY = "teehee.receivers.v1";
const SELECTED_RECEIVER_KEY = "teehee.selected-receiver.v1";
const PREFERENCES_KEY = "teehee.preferences.v1";

interface StoredPreferences {
  source?: SourceId;
  preset?: Preset;
  chunkMs?: number;
  mtu?: number;
  captureBufferMs?: number;
  prebufferMs?: number;
  rxBufferMs?: number;
  startOnOpen?: boolean;
  reconnect?: boolean;
  alerts?: boolean;
}

const engine = new Engine();

// ---- Navigation ----------------------------------------------------------

export const role = signal<Role>("send");
export const page = signal<Page>("stream");

export function setRole(next: Role) {
  role.set(next);
  page.set("stream");
}

export function setPage(next: Page) {
  page.set(next);
}

// ---- Send session --------------------------------------------------------

export const phase = signal<Phase>("idle");
export const elapsed = signal(0);
export const peak = signal(0);
export const previewPeak = signal(0);
export const packetsPerSec = signal(0);
export const packetsSent = signal(0);
export const ringMs = signal(0);
export const driftPpm = signal(0);
export const driftAvailable = signal(false);
export const fragmentations = signal(0);
export const sendErrors = signal(0);
export const sendError = signal("");
export const history = signal<number[]>(new Array(60).fill(0));

export const source = signal<SourceId>("system");
export const sendRate = signal(48000);
export const sendChannels = signal(2);
export const preset = signal<Preset>("balanced");
export const chunkMs = signal(3);
export const mtu = signal(1500);
export const captureBufferMs = signal(120);

export const events = signal<LogEvent[]>([]);

// Capture counters and derived health timers. "Quiet" means the loopback
// is delivering callbacks but every sample is silence; "no callbacks"
// means WASAPI never called us at all — the distinction the outage
// evidence could not make without the JSONL log.
export const captureCallbacks = signal(0);
export const captureNonzero = signal(0);
export const captureOverruns = signal(0);
export const captureUnderruns = signal(0);
export const captureLatencyDrops = signal(0);
export const captureHighWaterMs = signal(0);
export const captureQuiet = signal(false);
export const ringSaturated = signal(false);
export const driftSaturated = signal(false);
export const overrunRecent = signal(false);
export const latencyDropRecent = signal(false);
export const sessionLogPath = signal("");

// Runtime behavior. `reconnect` is on by default: the GUI replaces the
// Task Scheduler's 2-minute self-heal, so an error must not wait for a
// user click.
export const startOnOpen = signal(false);
export const reconnect = signal(true);
export const alertsEnabled = signal(true);
export const launchAtLogin = signal(false);
export const launchAtLoginError = signal("");

// Discovery starts empty: the engine replaces it with real mDNS results.
export const receivers = signal<Receiver[]>([]);
export const selectedReceiverId = signal("");
export const receiversSearching = signal(false);
export const receiversError = signal("");

export const selectedReceiver = computed<Receiver | undefined>(() =>
  receivers().find((entry) => entry.id === selectedReceiverId()),
);
export const activeReceiver = signal<{ name: string; address: string } | undefined>(
  undefined,
);

// ---- Receive session -----------------------------------------------------

export const recvPhase = signal<Phase>("idle");
export const recvPeak = signal(0);
export const recvQueuedMs = signal(0);
export const recvUnderruns = signal(0);
export const recvLateDrops = signal(0);
export const recvDriftPpm = signal(0);
export const recvDriftAvailable = signal(false);
export const recvElapsed = signal(0);
export const recvRate = signal(48000);
export const recvChannels = signal(2);
export const recvError = signal("");
export const prebufferMs = signal(200);
export const rxBufferMs = signal(2000);

// ---- Derived values ------------------------------------------------------

/** Bytes on the wire for one chunk at the sender's actual rate, header included. */
export const packetBytes = computed(
  () => Math.round(sendRate() * (chunkMs() / 1000) * sendChannels() * 4) + 25,
);

/** Payload an MTU leaves after IP (20) + UDP (8) + teehee (25) framing. */
export const envelopeBytes = computed(() => Math.max(0, mtu() - 53));

export const fitsMtu = computed(() => packetBytes() <= envelopeBytes());

/** What the sender owns: chunk plus the capture ring. */
export const senderDelayMs = computed(() => Math.round(chunkMs() + ringMs()));

/** Input preview while idle; real level while a session runs. */
export const monitorPeak = computed(() => {
  const p = phase();
  return p === "live" || p === "paused" ? peak() : previewPeak();
});

export const issues = computed<Issue[]>(() => {
  const out: Issue[] = [];

  if (sendError()) {
    out.push({
      severity: "error",
      title: "Sender stopped",
      detail: sendError(),
    });
  }

  if (recvError()) {
    out.push({
      severity: "error",
      title: "Receiver stopped",
      detail: recvError(),
    });
  }

  if (role() === "send" && phase() === "starting") {
    out.push({
      severity: "info",
      title: "Opening the audio path",
      detail: "Waiting for the capture endpoint and the first packet.",
    });
  }

  if (role() === "send" && sendErrors() > 0 && !sendError()) {
    out.push({
      severity: "error",
      title: "The receiver is not answering",
      detail: "Check that teehee recv is running and UDP 5000 is open.",
    });
  }

  if (role() === "send" && phase() === "live" && !fitsMtu()) {
    out.push({
      severity: "warning",
      title: "Packets are larger than the link MTU",
      detail: `Each packet is ${(packetBytes() / 1000).toFixed(1)} kB; a ${mtu()}-byte MTU allows ${(envelopeBytes() / 1000).toFixed(1)} kB. Windows will fragment them.`,
    });
  }

  if (
    role() === "send" &&
    phase() === "live" &&
    source() !== "sine" &&
    packetsPerSec() === 0
  ) {
    out.push({
      severity: "info",
      title: "Nothing is playing on this PC",
      detail:
        "Windows stops producing loopback audio when the system is silent.",
    });
  }

  if (
    role() === "send" &&
    phase() === "live" &&
    source() === "system" &&
    elapsed() >= 10 &&
    captureCallbacks() === 0
  ) {
    out.push({
      severity: "warning",
      title: "System audio is not delivering samples",
      detail:
        "WASAPI loopback is open but has not produced a capture callback. Check the default render device or restart the stream.",
    });
  }

  if (role() === "send" && phase() === "live" && captureQuiet()) {
    out.push({
      severity: "info",
      title: "Nothing is playing on this PC",
      detail: "The loopback is delivering callbacks, but every sample is silence.",
    });
  }

  if (role() === "send" && phase() === "live" && ringSaturated()) {
    out.push({
      severity: "warning",
      title: "The encode loop is being preempted",
      detail: `The capture ring has been near its cap for 5 s (${ringMs()} of ${captureBufferMs()} ms). Close CPU-heavy apps or raise the capture buffer.`,
    });
  }

  if (role() === "send" && phase() === "live" && overrunRecent()) {
    out.push({
      severity: "warning",
      title: "Capture buffer is too small",
      detail: `${captureOverruns()} overruns — the ring filled faster than the sender drained it.`,
    });
  }

  if (role() === "send" && phase() === "live" && latencyDropRecent()) {
    out.push({
      severity: "warning",
      title: "Audio is skipping to stay live",
      detail: `${captureLatencyDrops()} latency drops — the encode loop missed its schedule.`,
    });
  }

  if (role() === "send" && phase() === "live" && driftSaturated()) {
    out.push({
      severity: "warning",
      title: "Clock correction is saturated",
      detail: `Drift has pinned ±150 ppm for 10 s (+${driftPpm()} ppm). It recovers on its own; check the log if this persists.`,
    });
  }

  if (
    role() === "recv" &&
    recvPhase() === "live" &&
    recvQueuedMs() < prebufferMs() * 0.3
  ) {
    out.push({
      severity: "warning",
      title: "The buffer is running dry",
      detail: "Raise the prebuffer to absorb network jitter.",
    });
  }

  return out;
});

export const allClear = computed(() => issues().length === 0);

// ---- Native status polling ----------------------------------------------

let pollTimer: ReturnType<typeof setInterval> | undefined;
let lastHistoryAt = Date.now();
let lastErrorText = "";
let announcedLive = false;
let wantSend = false;
let retryTimer: ReturnType<typeof setTimeout> | undefined;
let retryDelay = 2000;
let lastNonzero = 0;
let lastNonzeroAt = 0;
let ringHighSince = 0;
let driftHighSince = 0;
let lastOverruns = 0;
let lastLatencyDrops = 0;
let overrunAt = 0;
let latencyAt = 0;
let quietSince = 0;
const notified = new Set<string>();

function clockNow(): string {
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
}

function pushEvent(level: LogEvent["level"], text: string) {
  events.update((rows) =>
    [{ time: clockNow(), level, text }, ...rows].slice(0, 40),
  );
}

function asPhase(value: string): Phase {
  if (
    value === "starting" ||
    value === "live" ||
    value === "paused" ||
    value === "error"
  ) {
    return value;
  }
  return "idle";
}

function noteSendError(message: string) {
  sendError.set(message);
  if (message && message !== lastErrorText) {
    lastErrorText = message;
    pushEvent("error", message);
  }
  if (!message) lastErrorText = "";
}

function resetCaptureHealth() {
  captureQuiet.set(false);
  ringSaturated.set(false);
  driftSaturated.set(false);
  overrunRecent.set(false);
  latencyDropRecent.set(false);
  lastNonzero = 0;
  lastNonzeroAt = 0;
  ringHighSince = 0;
  driftHighSince = 0;
  lastOverruns = 0;
  lastLatencyDrops = 0;
  overrunAt = 0;
  latencyAt = 0;
  quietSince = 0;
}

function isAlreadyRunning(message: string): boolean {
  return message.toLowerCase().includes("already running");
}

function requestAlertPermission() {
  // Defer the call so a host without notifications cannot throw
  // synchronously out of a click handler.
  void Promise.resolve()
    .then(() => Notifications.requestPermission())
    .catch(() => {});
}

function notifySend(key: string, title: string, body: string) {
  if (!alertsEnabled() || notified.has(key)) return;
  notified.add(key);
  void Promise.resolve()
    .then(() => Notifications.show({ id: key, title, body }))
    .catch(() => {});
}

function scheduleSendRetry() {
  if (retryTimer !== undefined || !wantSend || !reconnect()) return;
  if (isAlreadyRunning(sendError())) return;
  retryTimer = setTimeout(() => {
    retryTimer = undefined;
    if (!wantSend || !reconnect() || phase() !== "error") return;
    retryDelay = Math.min(retryDelay * 2, 30_000);
    startSend();
  }, retryDelay);
}

function applySend(state: NativeSend) {
  const next = asPhase(state.phase);
  if (next === "idle" || next === "error") activeReceiver.set(undefined);
  if (next !== phase()) {
    if (next === "live" && !announcedLive) {
      announcedLive = true;
      pushEvent("info", `send_live  ${state.sample_rate} Hz · ${state.packets_per_sec.toFixed(0)} pkt/s`);
    }
    if (next === "idle") announcedLive = false;
    phase.set(next);
  }
  noteSendError(state.error ?? "");
  packetsSent.set(state.packets_sent);
  packetsPerSec.set(state.packets_per_sec);
  peak.set(state.peak);
  ringMs.set(state.ring_ms);
  captureCallbacks.set(state.capture_callbacks);
  captureNonzero.set(state.capture_nonzero_samples);
  captureOverruns.set(state.capture_overruns);
  captureUnderruns.set(state.capture_underruns);
  captureLatencyDrops.set(state.capture_latency_drops);
  captureHighWaterMs.set(state.capture_high_water_ms);
  driftAvailable.set(state.drift_ppm !== null);
  driftPpm.set(state.drift_ppm ?? 0);
  fragmentations.set(state.fragmentations);
  sendErrors.set(state.send_errors);
  elapsed.set(Math.floor(state.elapsed_ms / 1000));
  if (state.sample_rate > 0) sendRate.set(state.sample_rate);
  if (state.channels > 0) sendChannels.set(state.channels);

  const now = Date.now();
  if (next === "live") {
    if (lastNonzeroAt === 0) lastNonzeroAt = now;
    if (state.capture_nonzero_samples !== lastNonzero) {
      lastNonzero = state.capture_nonzero_samples;
      lastNonzeroAt = now;
      captureQuiet.set(false);
    } else if (state.capture_callbacks > 0 && now - lastNonzeroAt >= 3000) {
      captureQuiet.set(true);
    }

    // The retired watcher alerted after four hours of unchanged
    // non-silent samples; keep that long-horizon check, but not a
    // notification for every silent moment.
    if (captureQuiet()) {
      if (quietSince === 0) quietSince = now;
      if (now - quietSince >= 4 * 60 * 60 * 1000) {
        notifySend(
          "teehee-send-quiet",
          "teehee capture is quiet",
          "No non-silent audio has been captured for 4 hours. This is normal if the PC is idle.",
        );
      }
    } else {
      quietSince = 0;
    }

    if (state.ring_ms > captureBufferMs() * 0.8) {
      if (ringHighSince === 0) ringHighSince = now;
      ringSaturated.set(now - ringHighSince >= 5000);
    } else {
      ringHighSince = 0;
      ringSaturated.set(false);
    }

    if (state.drift_ppm !== null && Math.abs(state.drift_ppm) >= 150) {
      if (driftHighSince === 0) driftHighSince = now;
      driftSaturated.set(now - driftHighSince >= 10_000);
    } else {
      driftHighSince = 0;
      driftSaturated.set(false);
    }

    if (state.capture_overruns > lastOverruns) overrunAt = now;
    if (state.capture_latency_drops > lastLatencyDrops) latencyAt = now;
    lastOverruns = state.capture_overruns;
    lastLatencyDrops = state.capture_latency_drops;
    overrunRecent.set(overrunAt > 0 && now - overrunAt < 5000);
    latencyDropRecent.set(latencyAt > 0 && now - latencyAt < 5000);

    let warning: { key: string; detail: string } | undefined;
    if (ringSaturated()) {
      warning = {
        key: "ring",
        detail: `The capture ring is at ${state.ring_ms} ms of ${captureBufferMs()} ms.`,
      };
    } else if (driftSaturated() && !captureQuiet()) {
      warning = {
        key: "drift",
        detail: `Clock correction is pinned at ${state.drift_ppm?.toFixed(0)} ppm.`,
      };
    } else if (
      source() === "system" &&
      state.elapsed_ms > 10_000 &&
      state.capture_callbacks === 0
    ) {
      warning = {
        key: "callbacks",
        detail: "WASAPI loopback is open but has not delivered a capture callback.",
      };
    }
    if (warning) {
      notifySend(`teehee-send-${warning.key}`, "teehee needs attention", warning.detail);
    }
  } else if (next === "idle") {
    resetCaptureHealth();
    retryDelay = 2000;
    if (retryTimer !== undefined) {
      clearTimeout(retryTimer);
      retryTimer = undefined;
    }
    notified.clear();
  } else if (next === "error" && wantSend) {
    if (state.error) {
      notifySend("teehee-send-error", "teehee stopped sending", state.error);
    }
    scheduleSendRetry();
  }

  if (phase() === "live" && Date.now() - lastHistoryAt >= 1000) {
    lastHistoryAt = Date.now();
    history.update((rows) => [
      ...rows.slice(1),
      Math.round(state.packets_per_sec),
    ]);
  }
}

function applyRecv(state: NativeRecv) {
  recvPhase.set(asPhase(state.phase));
  recvPeak.set(state.peak);
  recvQueuedMs.set(state.queued_ms);
  recvLateDrops.set(state.late_drops);
  recvUnderruns.set(state.underruns);
  recvDriftAvailable.set(state.drift_ppm !== null);
  recvDriftPpm.set(state.drift_ppm ?? 0);
  recvElapsed.set(Math.floor(state.elapsed_ms / 1000));
  if (state.sample_rate > 0) recvRate.set(state.sample_rate);
  if (state.channels > 0) recvChannels.set(state.channels);
  recvError.set(state.error ?? "");
}

function applyReceivers(state: NativeStatus["receivers"]) {
  receiversSearching.set(state.searching);
  receiversError.set(state.error ?? "");
  const discovered = state.items.map((item) => ({
    id: item.id,
    name: item.name,
    host: item.host,
    port: item.port,
    seen: item.seen,
    saved: false,
  }));
  const saved = receivers().filter((entry) => entry.saved);
  const merged = [
    ...saved,
    ...discovered.filter(
      (entry) =>
        !saved.some(
          (savedEntry) =>
            savedEntry.id === entry.id ||
            (savedEntry.host === entry.host && savedEntry.port === entry.port),
        ),
    ),
  ];
  const unchanged =
    merged.length === receivers().length &&
    merged.every(
      (entry, index) =>
        entry.id === receivers()[index]?.id &&
        entry.name === receivers()[index]?.name &&
        entry.host === receivers()[index]?.host &&
        entry.port === receivers()[index]?.port,
    );
  if (!unchanged) receivers.set(merged);
  if (!merged.some((entry) => entry.id === selectedReceiverId())) {
    selectedReceiverId.set(merged[0]?.id ?? "");
  }
}

function isNumberInRange(value: unknown, min: number, max: number): value is number {
  return (
    typeof value === "number" &&
    Number.isFinite(value) &&
    value >= min &&
    value <= max
  );
}

function persistPreferences() {
  const preferences: StoredPreferences = {
    source: source(),
    preset: preset(),
    chunkMs: chunkMs(),
    mtu: mtu(),
    captureBufferMs: captureBufferMs(),
    prebufferMs: prebufferMs(),
    rxBufferMs: rxBufferMs(),
    startOnOpen: startOnOpen(),
    reconnect: reconnect(),
    alerts: alertsEnabled(),
  };
  void Storage.setItem(PREFERENCES_KEY, JSON.stringify(preferences)).catch(
    () => {},
  );
}

function persistSavedReceivers() {
  const saved = receivers()
    .filter((entry) => entry.saved)
    .map(({ id, name, host, port }) => ({ id, name, host, port }));
  void Storage.setItem(RECEIVERS_KEY, JSON.stringify(saved)).catch(() => {});
}

function rememberSelectedReceiver() {
  if (!selectedReceiverId()) return;
  void Storage.setItem(SELECTED_RECEIVER_KEY, selectedReceiverId()).catch(
    () => {},
  );
}

function parseStoredReceiver(value: unknown): Receiver | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Record<string, unknown>;
  if (typeof raw.id !== "string" || typeof raw.host !== "string") return null;
  const host = raw.host.trim();
  if (!host) return null;
  const port = typeof raw.port === "number" ? raw.port : Number(raw.port);
  if (!Number.isInteger(port) || port < 1 || port > 65535) return null;
  const name =
    typeof raw.name === "string" && raw.name.trim() ? raw.name.trim() : host;
  return {
    id: raw.id,
    name,
    host,
    port,
    seen: "saved on this PC",
    saved: true,
  };
}

async function hydrateStoredState() {
  try {
    const [receiversRaw, selectedRaw, preferencesRaw] = await Promise.all([
      Storage.getItem(RECEIVERS_KEY),
      Storage.getItem(SELECTED_RECEIVER_KEY),
      Storage.getItem(PREFERENCES_KEY),
    ]);

    if (receiversRaw) {
      const parsed: unknown = JSON.parse(receiversRaw);
      if (Array.isArray(parsed)) {
        const stored = parsed
          .map(parseStoredReceiver)
          .filter((entry): entry is Receiver => entry !== null);
        if (stored.length > 0) {
          receivers.update((rows) => {
            const merged = [...stored];
            for (const row of rows) {
              if (
                !merged.some(
                  (entry) =>
                    entry.id === row.id ||
                    (entry.host === row.host && entry.port === row.port),
                )
              ) {
                merged.push(row);
              }
            }
            return merged;
          });
          if (selectedRaw && stored.some((entry) => entry.id === selectedRaw)) {
            selectedReceiverId.set(selectedRaw);
          } else if (!selectedReceiverId()) {
            selectedReceiverId.set(stored[0].id);
          }
        }
      }
    }

    if (preferencesRaw) {
      const parsed: unknown = JSON.parse(preferencesRaw);
      if (parsed && typeof parsed === "object") {
        const prefs = parsed as StoredPreferences;
        if (
          prefs.source === "system" ||
          prefs.source === "microphone" ||
          prefs.source === "sine"
        ) {
          source.set(prefs.source);
        }
        if (
          prefs.preset === "low" ||
          prefs.preset === "balanced" ||
          prefs.preset === "stable" ||
          prefs.preset === "custom"
        ) {
          preset.set(prefs.preset);
        }
        if (isNumberInRange(prefs.chunkMs, 1, 100)) {
          chunkMs.set(Math.round(prefs.chunkMs));
        }
        if (isNumberInRange(prefs.mtu, 500, 9000)) {
          mtu.set(Math.round(prefs.mtu));
        }
        if (isNumberInRange(prefs.captureBufferMs, 20, 5000)) {
          captureBufferMs.set(Math.round(prefs.captureBufferMs));
        }
        if (isNumberInRange(prefs.prebufferMs, 20, 5000)) {
          prebufferMs.set(Math.round(prefs.prebufferMs));
        }
        if (isNumberInRange(prefs.rxBufferMs, 20, 10000)) {
          rxBufferMs.set(Math.round(prefs.rxBufferMs));
        }
        if (rxBufferMs() < prebufferMs()) rxBufferMs.set(prebufferMs());
        if (typeof prefs.startOnOpen === "boolean") {
          startOnOpen.set(prefs.startOnOpen);
        }
        if (typeof prefs.reconnect === "boolean") {
          reconnect.set(prefs.reconnect);
        }
        if (typeof prefs.alerts === "boolean") {
          alertsEnabled.set(prefs.alerts);
        }
      }
    }
  } catch {
    // Storage is optional. A missing or corrupt entry falls back to defaults.
  }
}

function pollStatus() {
  let status: NativeStatus;
  try {
    status = JSON.parse(engine.status()) as NativeStatus;
  } catch {
    return;
  }
  applySend(status.send);
  applyRecv(status.recv);
  applyReceivers(status.receivers);
  const logPath = status.log_path ?? "";
  if (logPath !== sessionLogPath()) sessionLogPath.set(logPath);
  if (
    phase() === "idle" &&
    recvPhase() === "idle" &&
    !receiversSearching()
  ) {
    stopPolling();
  }
}

function ensurePolling() {
  if (pollTimer === undefined) {
    pollTimer = setInterval(pollStatus, 100);
  }
}

function stopPolling() {
  if (pollTimer !== undefined) {
    clearInterval(pollTimer);
    pollTimer = undefined;
  }
}

function stopPollingIfIdle() {
  if (
    phase() === "idle" &&
    recvPhase() === "idle" &&
    !receiversSearching()
  ) {
    stopPolling();
  }
}

// ---- Commands ------------------------------------------------------------

export function startSend() {
  if (phase() !== "idle" && phase() !== "error") return;
  const target = selectedReceiver();
  if (!target) {
    phase.set("error");
    noteSendError("Choose a receiver first — search or add its IP address.");
    return;
  }
  const address = target.host.includes(":")
    ? `[${target.host}]:${target.port}`
    : `${target.host}:${target.port}`;
  wantSend = true;
  // The first Start is a user gesture: ask for notification permission
  // here rather than at module load.
  if (alertsEnabled()) requestAlertPermission();
  try {
    engine.startSend(
      address,
      source(),
      chunkMs(),
      mtu(),
      captureBufferMs(),
    );
    activeReceiver.set({ name: target.name, address });
    phase.set("starting");
    noteSendError("");
    announcedLive = false;
    notified.clear();
    retryDelay = 2000;
    if (retryTimer !== undefined) {
      clearTimeout(retryTimer);
      retryTimer = undefined;
    }
    elapsed.set(0);
    packetsSent.set(0);
    packetsPerSec.set(0);
    peak.set(0);
    ringMs.set(0);
    driftAvailable.set(false);
    fragmentations.set(0);
    sendErrors.set(0);
    history.set(new Array(60).fill(0));
    pushEvent("info", `send_start  ${address} · ${source()}`);
    ensurePolling();
  } catch (error) {
    activeReceiver.set(undefined);
    phase.set("error");
    noteSendError(error instanceof Error ? error.message : String(error));
    scheduleSendRetry();
  }
}

export function toggleSendPause() {
  if (phase() === "live" || phase() === "starting") {
    engine.setSendPaused(true);
    phase.set("paused");
    peak.set(0);
    packetsPerSec.set(0);
    pushEvent("info", "send_pause");
  } else if (phase() === "paused") {
    engine.setSendPaused(false);
    phase.set("live");
    pushEvent("info", "send_resume");
  }
}

export function stopSend() {
  wantSend = false;
  retryDelay = 2000;
  if (retryTimer !== undefined) {
    clearTimeout(retryTimer);
    retryTimer = undefined;
  }
  resetCaptureHealth();
  notified.clear();
  try {
    engine.stopSend();
  } catch {
    // The engine is already gone at shutdown.
  }
  activeReceiver.set(undefined);
  phase.set("idle");
  noteSendError("");
  peak.set(0);
  packetsPerSec.set(0);
  packetsSent.set(0);
  ringMs.set(0);
  driftAvailable.set(false);
  driftPpm.set(0);
  elapsed.set(0);
  history.set(new Array(60).fill(0));
  stopPollingIfIdle();
  pushEvent("info", "send_stop");
}

export function startRecv() {
  if (recvPhase() !== "idle" && recvPhase() !== "error") return;
  try {
    engine.startRecv(5000, prebufferMs(), rxBufferMs());
    recvPhase.set("starting");
    recvError.set("");
    recvElapsed.set(0);
    recvQueuedMs.set(0);
    recvUnderruns.set(0);
    recvLateDrops.set(0);
    pushEvent("info", "recv_listen  udp 0.0.0.0:5000");
    ensurePolling();
  } catch (error) {
    recvPhase.set("error");
    recvError.set(error instanceof Error ? error.message : String(error));
  }
}

export function toggleRecvPause() {
  if (recvPhase() === "live") {
    engine.setRecvPaused(true);
    recvPhase.set("paused");
    recvPeak.set(0);
  } else if (recvPhase() === "paused") {
    engine.setRecvPaused(false);
    recvPhase.set("live");
  }
}

export function stopRecv() {
  try {
    engine.stopRecv();
  } catch {
    // The engine is already gone at shutdown.
  }
  recvPhase.set("idle");
  recvError.set("");
  recvPeak.set(0);
  recvQueuedMs.set(0);
  recvElapsed.set(0);
  stopPollingIfIdle();
  pushEvent("info", "recv_stop");
}

export function refreshReceivers() {
  try {
    engine.refreshReceivers();
    receiversSearching.set(true);
    receiversError.set("");
    ensurePolling();
    pushEvent("info", "mdns_browse  _teehee._udp.local");
  } catch (error) {
    receiversError.set(error instanceof Error ? error.message : String(error));
  }
}

// ---- Configuration -------------------------------------------------------

export function setSource(next: SourceId) {
  source.set(next);
  persistPreferences();
  pushEvent("info", `source  ${next}`);
}

export function setReceiver(id: string) {
  selectedReceiverId.set(id);
  rememberSelectedReceiver();
}

export function setPreset(next: Preset) {
  preset.set(next);
  if (next === "low") chunkMs.set(2);
  if (next === "balanced") chunkMs.set(3);
  if (next === "stable") chunkMs.set(20);
  persistPreferences();
}

export function setChunk(next: number) {
  chunkMs.set(next);
  preset.set("custom");
  persistPreferences();
}

export function setMtu(next: number) {
  mtu.set(next);
  persistPreferences();
}

export function setCaptureBuffer(next: number) {
  captureBufferMs.set(next);
  persistPreferences();
}

export function setPrebuffer(next: number) {
  prebufferMs.set(next);
  if (rxBufferMs() < next) rxBufferMs.set(next);
  persistPreferences();
}

export function setRxBuffer(next: number) {
  rxBufferMs.set(Math.max(next, prebufferMs()));
  persistPreferences();
}

export function setStartOnOpen(next: boolean) {
  startOnOpen.set(next);
  persistPreferences();
}

export function setReconnect(next: boolean) {
  reconnect.set(next);
  if (!next && retryTimer !== undefined) {
    clearTimeout(retryTimer);
    retryTimer = undefined;
  }
  persistPreferences();
}

export function setAlerts(next: boolean) {
  alertsEnabled.set(next);
  persistPreferences();
  if (next) requestAlertPermission();
}

export async function setOpenAtLogin(next: boolean) {
  try {
    await Application.setLaunchAtLogin(next);
    launchAtLogin.set(next);
    launchAtLoginError.set("");
  } catch (error) {
    launchAtLoginError.set(error instanceof Error ? error.message : String(error));
  }
}

export async function loadLaunchAtLogin() {
  try {
    launchAtLogin.set(await Application.launchAtLogin());
  } catch {
    launchAtLogin.set(false);
  }
}

function splitAddress(input: string): { host: string; port: number } {
  const trimmed = input.trim();
  const bracketed = trimmed.match(/^\[([^\]]+)\](?::(\d+))?$/);
  if (bracketed) {
    return { host: bracketed[1], port: Number(bracketed[2] ?? 5000) };
  }
  const parts = trimmed.split(":");
  if (parts.length === 2 && /^\d+$/.test(parts[1])) {
    return { host: parts[0], port: Number(parts[1]) };
  }
  return { host: trimmed, port: 5000 };
}

export function addReceiver(name: string, address: string) {
  const { host, port } = splitAddress(address);
  const id = `manual-${host}:${port}`;
  const entry: Receiver = {
    id,
    name: name.trim() || host,
    host,
    port,
    seen: "saved on this PC",
    saved: true,
  };
  receivers.update((rows) => [
    ...rows.filter(
      (row) => row.id !== id && !(row.host === host && row.port === port),
    ),
    entry,
  ]);
  selectedReceiverId.set(id);
  persistSavedReceivers();
  rememberSelectedReceiver();
}

void hydrateStoredState().then(() => {
  // Auto-start needs the saved receiver from storage, so it runs after
  // hydration rather than at module load.
  if (startOnOpen() && selectedReceiver() && phase() === "idle") startSend();
});
void loadLaunchAtLogin();
// Discover once when the app opens so the setup form is useful immediately.
refreshReceivers();
