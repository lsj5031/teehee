import { Text, View } from "nsh";
import { kilobytes } from "../lib/format";
import {
  packetBytes,
  preset,
  setPreset,
  type Preset,
} from "../state/session";
import { Segmented } from "../ui/controls";

const presets = [
  { value: "low" as const, label: "Low latency" },
  { value: "balanced" as const, label: "Balanced" },
  { value: "stable" as const, label: "Stable" },
];

/** Quality presets, with the packet/MTU trade-off stated plainly. */
export function QualityPicker(props: {
  preset: Preset;
  chunkMs: number;
  fits: boolean;
  onChange: (preset: Preset) => void;
}) {
  const current = () => (props.preset === "custom" ? "balanced" : props.preset);

  return (
    <View class="flex-col gap-2">
      <Segmented
        options={presets}
        value={current()}
        onChange={props.onChange}
      />
      <View class="flex-row items-center justify-between gap-3">
        <Text class="text-[11px] text-muted-foreground">
          {props.chunkMs} ms chunks · ~{Math.round(1000 / props.chunkMs)}{" "}
          packets/s · {kilobytes(packetBytes())} per packet
        </Text>
        <View class="flex-row items-center gap-1">
          <View
            class="h-1.5 w-1.5 rounded-full"
            classList={{
              "bg-warning": !props.fits,
              "bg-live": props.fits,
            }}
          />
          <Text class="text-[10px] text-muted-foreground">
            {props.fits ? "fits 1500 MTU" : "fragments"}
          </Text>
        </View>
      </View>
    </View>
  );
}

/** Used by both pages; keeps the preset wiring in one place. */
export function useQuality() {
  return {
    preset,
    setPreset,
  };
}
