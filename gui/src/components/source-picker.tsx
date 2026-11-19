import { Text, View } from "nsh";
import { Button as BaseButton } from "nsh/base";
import { cx, dbfs } from "../lib/format";
import {
  monitorPeak,
  source,
  type SourceId,
} from "../state/session";
import { Icon, type IconName } from "../ui/icon";
import { SectionLabel } from "../ui/surfaces";
import { LevelMeter } from "./level-meter";

interface SourceOption {
  id: SourceId;
  title: string;
  detail: string;
  icon: IconName;
}

const options: readonly SourceOption[] = [
  {
    id: "system",
    title: "System audio",
    detail: "Everything you hear on this PC. WASAPI loopback.",
    icon: "pc",
  },
  {
    id: "microphone",
    title: "Microphone",
    detail: "The default recording device. Mic, Stereo Mix, or a cable.",
    icon: "stream",
  },
  {
    id: "sine",
    title: "Test tone",
    detail: "A 440 Hz tone, to check the receiver without any audio playing.",
    icon: "activity",
  },
];

export function SourcePicker(props: {
  source: SourceId;
  onSelect: (source: SourceId) => void;
}) {
  return (
    <View class="flex-col gap-2">
      <Each of={options} key={(option) => option.id}>
        {(option) => {
          const selected = () => option().id === props.source;
          return (
            <BaseButton
              onPress={() => props.onSelect(option().id)}
              selected={selected()}
              class={cx(
                "flex-row items-start gap-3 rounded-(--radius) border p-3 text-left transition-colors ease-ui focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background",
                selected()
                  ? "border-primary/40 bg-accent/60"
                  : "border-border bg-card hover:bg-secondary/60",
              )}
            >
              <View
                class={cx(
                  "mt-0.5 h-4 w-4 shrink-0 items-center justify-center rounded-full border",
                  selected() ? "border-primary" : "border-input",
                )}
              >
                <If when={selected()}>
                  <View class="h-2 w-2 rounded-full bg-primary" />
                </If>
              </View>
              <View class="h-7 w-7 shrink-0 items-center justify-center rounded-(--radius-inset) bg-secondary">
                <Icon
                  name={option().icon}
                  tone={selected() ? "primary" : "muted-foreground"}
                />
              </View>
              <View class="flex-1 flex-col gap-0.5 min-w-0">
                <Text class="text-[13px] font-medium text-foreground">
                  {option().title}
                </Text>
                <Text class="text-[11px] text-muted-foreground">
                  {option().detail}
                </Text>
              </View>
            </BaseButton>
          );
        }}
      </Each>

      <View class="mt-1 flex-row items-center gap-3 rounded-(--radius) border border-border bg-secondary/50 px-3 py-2">
        <View class="flex-1 flex-col gap-1">
          <SectionLabel label="Input preview" />
          <LevelMeter value={monitorPeak()} bars={30} height={28} />
          <Text class="text-[9px] text-muted-foreground">
            live once streaming
          </Text>
        </View>
        <Text class="font-mono text-[11px] text-muted-foreground">
          {dbfs(monitorPeak())}
        </Text>
      </View>

      <Text class="text-[10px] text-muted-foreground">
        {source() === "system"
          ? "Loopback captures the render endpoint, not a microphone."
          : source() === "microphone"
            ? "The default recording device; its preview starts with the stream."
            : "The test tone plays locally; it is not captured from Windows."}
      </Text>
    </View>
  );
}
