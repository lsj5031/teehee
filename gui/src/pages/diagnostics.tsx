import { computed, Text, View, Scroll } from "nsh";
import {
  captureBufferMs,
  driftAvailable,
  driftPpm,
  events,
  fitsMtu,
  fragmentations,
  history,
  packetsPerSec,
  packetsSent,
  phase,
  prebufferMs,
  recvDriftAvailable,
  recvDriftPpm,
  recvError,
  recvLateDrops,
  recvPhase,
  recvQueuedMs,
  recvUnderruns,
  ringMs,
  role,
  rxBufferMs,
  sendErrors,
} from "../state/session";
import { Button } from "../ui/controls";
import { Icon } from "../ui/icon";
import { Callout, Card, SectionLabel } from "../ui/surfaces";
import { PageHeader } from "../components/page-header";
import { Sparkline } from "../components/sparkline";

const checks = computed(() => {
  const streaming = phase() === "live" || phase() === "paused";
  return [
    ...(streaming
      ? [
          {
            ok: sendErrors() === 0,
            title: "Receiver is accepting packets",
            detail: `${sendErrors()} send errors`,
          },
        ]
      : []),
    {
      ok: fitsMtu() && fragmentations() === 0,
      title: "Packets fit the link MTU",
      detail: `${fragmentations()} fragmentations`,
    },
    {
      ok: ringMs() < captureBufferMs() * 0.8,
      title: "Capture ring has headroom",
      detail: `${ringMs()} of ${captureBufferMs()} ms`,
    },
  ];
});

function Counter(props: { label: string; value: string; detail: string }) {
  return (
    <View class="flex-col gap-0.5 py-1.5">
      <Text class="text-[10px] font-semibold uppercase tracking-[0.09em] text-muted-foreground">
        {props.label}
      </Text>
      <Text class="font-mono text-[13px] text-foreground">{props.value}</Text>
      <Text class="text-[10px] text-muted-foreground">{props.detail}</Text>
    </View>
  );
}

function EventsCard() {
  return (
    <Card class="flex-col gap-1 p-4">
      <SectionLabel label="Events" />
      <If
        when={events().length > 0}
        else={
          <Text class="py-2 text-[11px] text-muted-foreground">
            No events yet.
          </Text>
        }
      >
        <Each of={events()} key={(event, index) => `${event.time}-${index}`}>
          {(event) => (
            <View class="flex-row items-start gap-3 border-t border-border py-1.5">
              <Text class="font-mono text-[10px] text-muted-foreground">
                {event().time}
              </Text>
              <Text
                class="font-mono text-[10px] w-8"
                classList={{
                  "text-warning": event().level === "warn",
                  "text-destructive": event().level === "error",
                  "text-muted-foreground": event().level === "info",
                }}
              >
                {event().level}
              </Text>
              <Text class="flex-1 font-mono text-[10px] text-foreground">
                {event().text}
              </Text>
            </View>
          )}
        </Each>
      </If>
    </Card>
  );
}

function SenderDiagnostics() {
  return (
    <View class="flex-1 flex-col">
      <PageHeader
        title="Diagnostics"
        subtitle="Evidence for the sender's health, not a replacement for it."
      >
        <Button variant="ghost" onPress={() => events.set([])}>
          <Text>Clear</Text>
        </Button>
      </PageHeader>

      <Scroll scrollbar={true} class="flex-1" contentClass="flex-col gap-5 px-7 pb-24">
        <Card class="flex-col gap-1 p-4">
          <SectionLabel label="Checks" />
          <Each of={checks()} key={(check) => check.title}>
            {(check) => (
              <View class="flex-row items-center gap-3 py-2">
                <Icon
                  name={check().ok ? "check" : "alert"}
                  tone={check().ok ? "live" : "warning"}
                />
                <View class="flex-1 flex-col gap-0.5">
                  <Text class="text-[12px] text-foreground">
                    {check().title}
                  </Text>
                  <Text class="text-[10px] text-muted-foreground">
                    {check().detail}
                  </Text>
                </View>
              </View>
            )}
          </Each>
          <If when={phase() !== "live" && phase() !== "paused"}>
            <Text class="px-1 pb-1 text-[11px] text-muted-foreground">
              Start a stream to check receiver health.
            </Text>
          </If>
        </Card>

        <Card class="flex-col gap-2 p-4">
          <SectionLabel label="Live counters" />
          <View class="flex-row gap-6">
            <View class="flex-1">
              <Counter
                label="Packets"
                value={`${packetsPerSec().toFixed(1)} /s`}
                detail={`${packetsSent()} sent`}
              />
              <Counter
                label="Capture ring"
                value={`${ringMs()} ms`}
                detail={`of ${captureBufferMs()} ms`}
              />
            </View>
            <View class="flex-1">
              <Counter
                label="Send errors"
                value={`${sendErrors()}`}
                detail="ICMP unreachable"
              />
              <Counter
                label="Drift"
                value={driftAvailable() ? `${driftPpm() >= 0 ? "+" : ""}${driftPpm()} ppm` : "—"}
                detail={driftAvailable() ? "clamp ±150" : "not measured"}
              />
            </View>
            <View class="flex-1">
              <Counter
                label="Fragmentations"
                value={`${fragmentations()}`}
                detail="over the MTU"
              />
              <Counter
                label="Ring headroom"
                value={`${Math.max(0, captureBufferMs() - ringMs())} ms`}
                detail="before capture drops"
              />
            </View>
          </View>
        </Card>

        <Card class="flex-col gap-3 p-4">
          <View class="flex-row items-center justify-between">
            <SectionLabel label="Capture ring · ms" />
            <Text class="font-mono text-[10px] text-muted-foreground">
              cap {captureBufferMs()}
            </Text>
          </View>
          <Sparkline values={history()} max={Math.max(120, captureBufferMs())} height={52} />
        </Card>

        <EventsCard />
      </Scroll>
    </View>
  );
}

function ReceiverDiagnostics() {
  return (
    <View class="flex-1 flex-col">
      <PageHeader
        title="Diagnostics"
        subtitle="Evidence for the receiver's health, not a replacement for it."
      >
        <Button variant="ghost" onPress={() => events.set([])}>
          <Text>Clear</Text>
        </Button>
      </PageHeader>

      <Scroll scrollbar={true} class="flex-1" contentClass="flex-col gap-5 px-7 pb-24">
        <If when={recvError() !== ""}>
          <Callout tone="error" title="Receiver stopped" detail={recvError()} />
        </If>

        <Card class="flex-col gap-2 p-4">
          <SectionLabel label="Receiver" />
          <Text class="text-[13px] font-medium text-foreground">
            {recvPhase() === "live"
              ? "Listening on UDP 5000"
              : recvPhase() === "paused"
                ? "Listening paused"
                : recvPhase() === "starting"
                  ? "Opening the output"
                  : recvPhase() === "error"
                    ? "Receiver stopped"
                    : "Not listening"}
          </Text>
          <Text class="text-[11px] text-muted-foreground">
            {recvPhase() === "live"
              ? `${recvQueuedMs()} ms queued · target ${prebufferMs()} ms`
              : "Start listening on Stream to collect counters."}
          </Text>
        </Card>

        <Card class="flex-col gap-2 p-4">
          <SectionLabel label="Live counters" />
          <View class="flex-row gap-6">
            <View class="flex-1">
              <Counter
                label="Queue"
                value={`${recvQueuedMs()} ms`}
                detail={`target ${prebufferMs()} ms`}
              />
              <Counter
                label="Drift"
                value={recvDriftAvailable() ? `${recvDriftPpm() >= 0 ? "+" : ""}${recvDriftPpm()} ppm` : "—"}
                detail={recvDriftAvailable() ? "clamp ±150" : "not measured"}
              />
            </View>
            <View class="flex-1">
              <Counter
                label="Late drops"
                value={`${recvLateDrops()}`}
                detail="packets"
              />
              <Counter
                label="Underruns"
                value={`${recvUnderruns()}`}
                detail="resyncs"
              />
            </View>
            <View class="flex-1">
              <Counter
                label="Ring"
                value={`${rxBufferMs()} ms`}
                detail={`prebuffer ${prebufferMs()} ms`}
              />
              <Counter
                label="Port"
                value="UDP 5000"
                detail="local receiver"
              />
            </View>
          </View>
        </Card>

        <EventsCard />
      </Scroll>
    </View>
  );
}

export function DiagnosticsPage() {
  if (role() === "recv") return <ReceiverDiagnostics />;
  return <SenderDiagnostics />;
}
