import { Text, View, Scroll } from "nsh";
import { clock, dbfs } from "../lib/format";
import {
  activeReceiver,
  allClear,
  captureBufferMs,
  chunkMs,
  driftAvailable,
  driftPpm,
  elapsed,
  fitsMtu,
  fragmentations,
  history,
  issues,
  packetBytes,
  packetsPerSec,
  packetsSent,
  peak,
  phase,
  prebufferMs,
  preset,
  recvDriftAvailable,
  recvDriftPpm,
  recvElapsed,
  recvError,
  recvLateDrops,
  recvChannels,
  recvPeak,
  recvPhase,
  recvQueuedMs,
  recvRate,
  recvUnderruns,
  receivers,
  receiversSearching,
  ringMs,
  role,
  rxBufferMs,
  selectedReceiver,
  sendChannels,
  sendErrors,
  sendRate,
  senderDelayMs,
  setPage,
  setPrebuffer,
  setPreset,
  setRxBuffer,
  setSource,
  source,
  startRecv,
  startSend,
  stopRecv,
  stopSend,
  toggleRecvPause,
  toggleSendPause,
} from "../state/session";
import { Button, Segmented } from "../ui/controls";
import { Icon } from "../ui/icon";
import {
  Callout,
  Card,
  SectionLabel,
  Stat,
  StateChip,
  type ChipTone,
} from "../ui/surfaces";
import { LevelMeter } from "../components/level-meter";
import { PageHeader } from "../components/page-header";
import { QualityPicker } from "../components/quality-picker";
import { ReceiverList } from "../components/receiver-list";
import { SignalPath } from "../components/signal-path";
import { SourcePicker } from "../components/source-picker";
import { Sparkline } from "../components/sparkline";

function sendTone(): ChipTone {
  switch (phase()) {
    case "live":
      return "live";
    case "paused":
      return "idle";
    case "starting":
      return "waiting";
    case "error":
      return "error";
    default:
      return "idle";
  }
}

function sendLabel(): string {
  switch (phase()) {
    case "live":
      return "Live";
    case "paused":
      return "Paused";
    case "starting":
      return "Starting";
    case "error":
      return "Error";
    default:
      return "Idle";
  }
}

function sourceName(): string {
  if (source() === "sine") return "440 Hz test tone";
  if (source() === "microphone") return "Default microphone";
  return "WASAPI loopback · default render endpoint";
}

function sourceSummary(): string {
  if (source() === "sine") return "440 Hz test tone";
  if (source() === "microphone") return "Microphone";
  return "System audio";
}

function sendSubtitle(): string {
  if (source() === "sine") return "Sending a 440 Hz test tone";
  if (source() === "microphone") return "Sending this PC's microphone";
  return "Sending this PC's system audio";
}

function MetaRow(props: { label: string; value: string; mono?: boolean }) {
  return (
    <View class="flex-row items-center justify-between gap-4 py-1.5">
      <Text class="text-[11px] text-muted-foreground">{props.label}</Text>
      <Text
        class="text-[11px] text-foreground text-right"
        classList={{ "font-mono": props.mono }}
      >
        {props.value}
      </Text>
    </View>
  );
}

function SendIdle() {
  return (
    <Scroll scrollbar={true} class="flex-1" contentClass="flex-col gap-5 px-7 pb-24">
      <Card class="flex-col gap-3 p-4">
        <View class="flex-row items-center justify-between">
          <SectionLabel label="Source" />
          <Text class="text-[10px] text-muted-foreground">Windows</Text>
        </View>
        <SourcePicker source={source()} onSelect={setSource} />
      </Card>

      <Card class="flex-col gap-2 p-4">
        <SectionLabel label="Receiver" />
        <ReceiverList onAdd={() => setPage("receivers")} />
        <If when={receiversSearching()}>
          <Text class="px-2.5 pt-1 text-[11px] text-muted-foreground">
            Searching for receivers on this network…
          </Text>
        </If>
        <If when={receivers().length === 0 && !receiversSearching()}>
          <Text class="px-2.5 pt-1 text-[11px] text-muted-foreground">
            No receiver listed? Run teehee recv on the receiver with --mdns.
          </Text>
        </If>
      </Card>

      <Card class="flex-col gap-3 p-4">
        <SectionLabel label="Quality" />
        <QualityPicker
          preset={preset()}
          chunkMs={chunkMs()}
          fits={fitsMtu()}
          onChange={setPreset}
        />
        <Button
          variant="ghost"
          class="h-6 self-start px-0"
          onPress={() => setPage("audio")}
        >
          <Text class="text-[11px]">Advanced</Text>
          <Icon name="arrowRight" size={12} />
        </Button>
      </Card>

      <View class="flex-row items-center gap-2 px-1">
        <Icon name="shield" size={14} />
        <Text class="text-[11px] text-muted-foreground">
          Audio is unencrypted. Use only on a network you trust.
        </Text>
      </View>
    </Scroll>
  );
}

function SendLive() {
  return (
    <Scroll scrollbar={true} class="flex-1" contentClass="flex-col gap-5 px-7 pb-24">
      <Card class="flex-col gap-5 p-5">
        <View class="flex-row items-center justify-between">
          <SectionLabel label="Signal path" />
          <StateChip tone={sendTone()} label={sendLabel()} />
        </View>

        <SignalPath
          source={sourceSummary()}
          receiverName={activeReceiver()?.name ?? "No active receiver"}
          address={activeReceiver()?.address ?? ""}
          level={phase() === "live" ? peak() : 0}
          active={phase() === "live"}
        />

        <View class="flex-col gap-2">
          <LevelMeter
            value={phase() === "live" ? peak() : 0}
            bars={30}
            height={62}
            tone="live"
          />
          <View class="flex-row items-end justify-between">
            <View class="flex-row items-baseline gap-2">
              <Text class="font-mono text-2xl font-medium text-foreground">
                {clock(elapsed())}
              </Text>
              <Text class="font-mono text-[11px] text-muted-foreground">
                {dbfs(phase() === "live" ? peak() : 0)}
              </Text>
            </View>
            <Text class="font-mono text-[11px] text-muted-foreground">
              {`${sendRate()} Hz · ${sendChannels() === 1 ? "mono" : "stereo"} · ${chunkMs()} ms`}
            </Text>
          </View>
        </View>
      </Card>

      <View class="flex-row gap-3">
        <Card class="flex-1 p-4">
          <Stat
            label="Packets"
            value={packetsPerSec().toFixed(1)}
            unit="/s"
            detail={`${packetsSent()} sent`}
          />
        </Card>
        <Card class="flex-1 p-4">
          <Stat
            label="Drift"
            value={driftAvailable() ? `${driftPpm() >= 0 ? "+" : ""}${driftPpm()}` : "—"}
            unit={driftAvailable() ? "ppm" : undefined}
            detail={driftAvailable() ? "within ±150" : "not measured"}
          />
        </Card>
        <Card class="flex-1 p-4">
          <Stat
            label="Capture ring"
            value={`${ringMs()}`}
            unit="ms"
            detail={`of ${captureBufferMs()} ms`}
          />
        </Card>
      </View>

      <If when={allClear()}>
        <View class="flex-row items-center gap-2 px-1">
          <Icon name="check" tone="live" size={14} />
          <Text class="text-[11px] text-muted-foreground">
            All clear · sender-side checks
          </Text>
        </View>
      </If>

      <Each of={issues()} key={(issue) => issue.title}>
        {(issue) => (
          <Callout
            tone={
              issue().severity === "error"
                ? "error"
                : issue().severity === "warning"
                  ? "warning"
                  : "info"
            }
            title={issue().title}
            detail={issue().detail}
          />
        )}
      </Each>

      <Card class="flex-col gap-3 p-4">
        <View class="flex-row items-center justify-between">
          <SectionLabel label="Signal · last 60 seconds" />
          <Text class="font-mono text-[10px] text-muted-foreground">
            {Math.round(1000 / chunkMs())} pkt/s ceiling
          </Text>
        </View>
        <Sparkline values={history()} height={54} />
        <View class="flex-row items-center justify-between">
          <Text class="text-[10px] text-muted-foreground">
            packets per second
          </Text>
          <Text class="text-[10px] text-muted-foreground">
            {fragmentations()} fragmented · {sendErrors()} errors
          </Text>
        </View>
      </Card>

      <Card class="flex-col px-4 py-2">
        <SectionLabel label="Session" />
        <View class="mt-1">
          <MetaRow label="Source" value={sourceName()} />
          <MetaRow
            label="Active receiver"
            value={
              activeReceiver()
                ? `${activeReceiver()!.name} · ${activeReceiver()!.address}`
                : "No active receiver"
            }
          />
          <MetaRow label="Sender delay" value={`${senderDelayMs()} ms`} mono />
          <MetaRow label="Packet" value={`${(packetBytes() / 1000).toFixed(1)} kB · ${fitsMtu() ? "fits" : "fragments"} 1500 MTU`} mono />
        </View>
      </Card>
    </Scroll>
  );
}

function BufferTank() {
  const fill = () => {
    const percent = (recvQueuedMs() / rxBufferMs()) * 100;
    return Math.min(100, percent > 0 ? Math.max(1.5, percent) : 0);
  };
  const gate = () => Math.min(100, (prebufferMs() / rxBufferMs()) * 100);

  return (
    <View class="flex-col gap-1.5">
      <View class="relative h-3 w-full overflow-hidden rounded-full bg-secondary">
        <View
          class="absolute inset-y-0 left-0 rounded-full bg-live/70"
          style={{ width: `${fill()}%` }}
        />
        <View
          class="absolute inset-y-0 w-px bg-foreground/60"
          style={{ left: `${gate()}%` }}
        />
      </View>
      <View class="flex-row items-center justify-between">
        <Text class="font-mono text-[10px] text-muted-foreground">
          {recvQueuedMs()} of {rxBufferMs()} ms
        </Text>
        <Text class="text-[10px] text-muted-foreground">
          gate {prebufferMs()} ms
        </Text>
      </View>
    </View>
  );
}

function RecvLive() {
  return (
    <Scroll scrollbar={true} class="flex-1" contentClass="flex-col gap-5 px-7 pb-24">
      <Card class="flex-col gap-5 p-5">
        <View class="flex-row items-center justify-between">
          <SectionLabel label="Input" />
          <StateChip
            tone={
              recvPhase() === "live"
                ? "live"
                : recvPhase() === "error"
                  ? "error"
                  : "waiting"
            }
            label={
              recvPhase() === "live"
                ? "Live"
                : recvPhase() === "paused"
                  ? "Paused"
                  : recvPhase() === "error"
                    ? "Error"
                    : "Listening"
            }
          />
        </View>

        <View class="flex-col gap-2">
          <LevelMeter
            value={recvPhase() === "live" ? recvPeak() : 0}
            bars={30}
            height={54}
            tone="live"
          />
          <View class="flex-row items-baseline justify-between">
            <Text class="font-mono text-2xl font-medium text-foreground">
              {clock(recvElapsed())}
            </Text>
            <Text class="font-mono text-[11px] text-muted-foreground">
              {dbfs(recvPhase() === "live" ? recvPeak() : 0)}
            </Text>
          </View>
        </View>

        <BufferTank />

        <If when={recvError() !== ""}>
          <Callout tone="error" title="Receiver stopped" detail={recvError()} />
        </If>

        <Text class="font-mono text-[11px] text-muted-foreground">
          {`${recvRate()} Hz · ${recvChannels() === 1 ? "mono" : "stereo"} → default output`}
        </Text>
      </Card>

      <View class="flex-row gap-3">
        <Card class="flex-1 p-4">
          <Stat
            label="Queue"
            value={`${recvQueuedMs()}`}
            unit="ms"
            detail={`target ${prebufferMs()} ms`}
          />
        </Card>
        <Card class="flex-1 p-4">
          <Stat
            label="Drift"
            value={recvDriftAvailable() ? `${recvDriftPpm() >= 0 ? "+" : ""}${recvDriftPpm()}` : "—"}
            unit={recvDriftAvailable() ? "ppm" : undefined}
            detail={recvDriftAvailable() ? "within ±150" : "not measured"}
          />
        </Card>
        <Card class="flex-1 p-4">
          <Stat
            label="Network"
            value={`${recvLateDrops()}`}
            unit="late"
            detail={`${recvUnderruns()} resyncs`}
          />
        </Card>
      </View>

      <Card class="flex-col gap-4 p-4">
        <SectionLabel label="Output" />
        <View class="flex-row items-center justify-between">
          <View class="flex-col gap-0.5">
            <Text class="text-[13px] font-medium text-foreground">
              System default output
            </Text>
            <Text class="text-[11px] text-muted-foreground">
              Opened when listening starts
            </Text>
          </View>
        </View>

        <View class="flex-col gap-1.5">
          <Text class="text-xs font-medium text-foreground">Prebuffer</Text>
          <Segmented
            options={[
              { value: "100", label: "100 ms" },
              { value: "200", label: "200 ms" },
              { value: "500", label: "500 ms" },
            ]}
            value={String(prebufferMs())}
            onChange={(value) => setPrebuffer(Number(value))}
          />
          <Text class="text-[10px] text-muted-foreground">
            Lower starts faster; higher survives weak Wi-Fi.
          </Text>
        </View>

        <View class="flex-col gap-1.5">
          <Text class="text-xs font-medium text-foreground">Ring</Text>
          <Segmented
            options={[
              { value: "1000", label: "1 s" },
              { value: "2000", label: "2 s" },
              { value: "4000", label: "4 s" },
            ]}
            value={String(rxBufferMs())}
            onChange={(value) => setRxBuffer(Number(value))}
          />
          <Text class="text-[10px] text-muted-foreground">
            Must be at least the prebuffer.
          </Text>
        </View>
      </Card>
    </Scroll>
  );
}

function SendPage() {
  const idle = () => phase() === "idle";
  return (
    <View class="flex-1 flex-col">
      <PageHeader
        title={
          idle()
            ? "Start a stream"
            : phase() === "error"
              ? "Sender stopped"
              : (activeReceiver()?.name ?? "Streaming")
        }
        subtitle={
          idle()
            ? "Send this PC's sound to another device on your network."
            : phase() === "error"
              ? "The last attempt failed; the reason is below."
              : sendSubtitle()
        }
      >
        <View class="flex-row items-center gap-2">
          <If when={!idle()}>
            <If
              when={phase() === "paused"}
              else={
                <Button onPress={toggleSendPause}>
                  <Icon name="pause" size={14} />
                  <Text>Pause</Text>
                </Button>
              }
            >
              <Button onPress={toggleSendPause}>
                <Icon name="play" size={14} />
                <Text>Resume</Text>
              </Button>
            </If>
          </If>
          <If
            when={idle()}
            else={
              <Button variant="primary" onPress={stopSend}>
                <Icon name="stop" tone="primary-foreground" size={14} />
                <Text>Stop streaming</Text>
              </Button>
            }
          >
            <Button
              variant="primary"
              disabled={!selectedReceiver()}
              onPress={startSend}
            >
              <Icon name="play" tone="primary-foreground" size={14} />
              <Text>Start streaming</Text>
            </Button>
          </If>
        </View>
      </PageHeader>
      <If when={idle()} else={<SendLive />}>
        <SendIdle />
      </If>
    </View>
  );
}

function RecvPage() {
  const idle = () => recvPhase() === "idle";
  return (
    <View class="flex-1 flex-col">
      <PageHeader
        title={
          idle()
            ? "Start listening"
            : recvPhase() === "error"
              ? "Receiver stopped"
              : "Listening on UDP 5000"
        }
        subtitle={
          idle()
            ? "Play audio sent to this device on the default output."
            : recvPhase() === "live"
              ? "Playing from a sender on this network."
              : recvPhase() === "error"
                ? "UDP 5000 or the output device could not be opened."
                : "Waiting for a sender on this network."
        }
      >
        <View class="flex-row items-center gap-2">
          <If when={!idle()}>
            <If
              when={recvPhase() === "paused"}
              else={
                <Button onPress={toggleRecvPause}>
                  <Icon name="pause" size={14} />
                  <Text>Pause</Text>
                </Button>
              }
            >
              <Button onPress={toggleRecvPause}>
                <Icon name="play" size={14} />
                <Text>Resume</Text>
              </Button>
            </If>
          </If>
          <If
            when={idle()}
            else={
              <Button variant="primary" onPress={stopRecv}>
                <Icon name="stop" tone="primary-foreground" size={14} />
                <Text>Stop listening</Text>
              </Button>
            }
          >
            <Button variant="primary" onPress={startRecv}>
              <Icon name="play" tone="primary-foreground" size={14} />
              <Text>Start listening</Text>
            </Button>
          </If>
        </View>
      </PageHeader>
      <If when={idle()}>
        <Scroll scrollbar={true} class="flex-1" contentClass="flex-col gap-5 px-7 pb-24">
          <Card class="flex-col gap-3 p-5">
            <SectionLabel label="Receiver" />
            <Text class="text-[13px] font-medium text-foreground">
              UDP 5000
            </Text>
            <Text class="text-[12px] text-muted-foreground">
              The receiver will bind this port when listening starts. It
              advertises as _teehee._udp.local. when mDNS is on, so a sender can
              find it without an IP address.
            </Text>
            <BufferTank />
          </Card>
          <View class="flex-row items-center gap-2 px-1">
            <Icon name="shield" size={14} />
            <Text class="text-[11px] text-muted-foreground">
              Anyone who can reach this port can send audio. Use a network you
              trust.
            </Text>
          </View>
        </Scroll>
      </If>
      <If when={!idle()}>
        <RecvLive />
      </If>
    </View>
  );
}

export function StreamPage() {
  return (
    <If when={role() === "send"} else={<RecvPage />}>
      <SendPage />
    </If>
  );
}
