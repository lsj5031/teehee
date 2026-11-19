import { Text, View, Scroll } from "nsh";
import { kilobytes } from "../lib/format";
import {
  captureBufferMs,
  chunkMs,
  envelopeBytes,
  fitsMtu,
  mtu,
  packetBytes,
  preset,
  setCaptureBuffer,
  setChunk,
  setMtu,
  setPage,
  setPreset,
  setSource,
  source,
} from "../state/session";
import { Button, Field, Segmented } from "../ui/controls";
import { Card, Callout, SectionLabel } from "../ui/surfaces";
import { PageHeader } from "../components/page-header";
import { QualityPicker } from "../components/quality-picker";
import { SourcePicker } from "../components/source-picker";

export function AudioPage() {
  return (
    <View class="flex-1 flex-col">
      <PageHeader
        title="Audio & network"
        subtitle="How this PC is captured, and how packets fit the link."
      >
        <Button variant="ghost" onPress={() => setPage("diagnostics")}>
          <Text>Diagnostics</Text>
        </Button>
      </PageHeader>

      <Scroll scrollbar={true} class="flex-1" contentClass="flex-col gap-5 px-7 pb-24">
        <Card class="flex-col gap-3 p-4">
          <SectionLabel label="Capture" />
          <SourcePicker source={source()} onSelect={setSource} />
        </Card>

        <Card class="flex-col gap-3 p-4">
          <SectionLabel label="Quality" />
          <QualityPicker
            preset={preset()}
            chunkMs={chunkMs()}
            fits={fitsMtu()}
            onChange={setPreset}
          />
          <View class="flex-col gap-1.5 border-t border-border pt-3">
            <Text class="text-[11px] text-muted-foreground">
              Low latency · 2 ms · 500 packets/s · {kilobytes(793)} per packet
            </Text>
            <Text class="text-[11px] text-muted-foreground">
              Balanced · 3 ms · 333 packets/s · {kilobytes(1177)} per packet
            </Text>
            <Text class="text-[11px] text-muted-foreground">
              Stable · 20 ms · 50 packets/s · {kilobytes(7705)} per packet
            </Text>
          </View>
        </Card>

        <If when={!fitsMtu()}>
          <Callout
            tone="warning"
            title="This configuration will fragment"
            detail={`A packet is ${kilobytes(packetBytes())}, but a ${mtu()}-byte MTU leaves ${kilobytes(envelopeBytes())}. Windows will split it into IP fragments. Use 3 ms chunks, or 9000 on a jumbo-frame network.`}
          />
        </If>

        <Card class="flex-col gap-4 p-4">
          <SectionLabel label="Advanced" />
          <Field
            label="Chunk size"
            hint={`${Math.round(1000 / chunkMs())} packets/s at 48 kHz stereo`}
          >
            <Segmented
              options={[
                { value: "2", label: "2 ms" },
                { value: "3", label: "3 ms" },
                { value: "20", label: "20 ms" },
              ]}
              value={String(chunkMs())}
              onChange={(value) => setChunk(Number(value))}
            />
          </Field>

          <Field
            label="Link MTU"
            hint="The envelope is MTU minus 53 bytes of IP, UDP and teehee framing."
          >
            <Segmented
              options={[
                { value: "1500", label: "1500" },
                { value: "9000", label: "9000" },
              ]}
              value={String(mtu())}
              onChange={(value) => setMtu(Number(value))}
            />
          </Field>

          <Field
            label="Capture buffer"
            hint="Drop-oldest above this depth, so lag cannot climb."
          >
            <Segmented
              options={[
                { value: "120", label: "120 ms" },
                { value: "200", label: "200 ms" },
                { value: "500", label: "500 ms" },
              ]}
              value={String(captureBufferMs())}
              onChange={(value) => setCaptureBuffer(Number(value))}
            />
          </Field>
        </Card>
      </Scroll>
    </View>
  );
}
