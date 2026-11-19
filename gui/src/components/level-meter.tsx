import { computed, View } from "nsh";
import { cx } from "../lib/format";

/**
 * A peak meter: bars whose height follows one 0..1 value. The shape is
 * deterministic so the meter reads as an instrument, not noise.
 */
export function LevelMeter(props: {
  value: number;
  bars?: number;
  height?: number;
  tone?: "primary" | "live";
  class?: string;
}) {
  const heights = computed(() => {
    const count = props.bars ?? 36;
    const level = Math.max(0, Math.min(1, props.value));
    return Array.from({ length: count }, (_, index) => {
      const center = 1 - Math.abs(index / (count - 1) - 0.5) * 1.15;
      const wobble = 0.55 + 0.45 * Math.sin(index * 1.7);
      return Math.max(
        0.06,
        Math.min(1, level * (0.55 + center) * wobble + 0.04),
      );
    });
  });

  const height = props.height ?? 64;

  return (
    <View
      class={cx("flex-row items-end gap-0.5", props.class)}
      style={{ height }}
      accessibilityValue={{ min: 0, max: 100, now: Math.round(props.value * 100) }}
    >
      <Each of={heights()} key="index">
        {(bar) => (
          <View
            class={cx(
              "flex-1 origin-bottom rounded-full transition-transform ease-ui duration-100",
              props.tone === "live" ? "bg-live" : "bg-primary",
              `scale-y-[${Math.max(0.04, bar()).toFixed(2)}]`,
            )}
            style={{ height }}
          />
        )}
      </Each>
    </View>
  );
}
