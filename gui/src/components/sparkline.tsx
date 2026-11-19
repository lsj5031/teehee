import { computed, View } from "nsh";
import { cx } from "../lib/format";

/** A 60-sample bar history; compact enough to read at a glance. */
export function Sparkline(props: {
  values: number[];
  max?: number;
  height?: number;
  class?: string;
}) {
  const bars = computed(() => {
    // Keep the chart readable and the release host light: one bar per two
    // samples, so 60 seconds of history draws as 30 bars.
    const values = props.values.filter((_, index) => index % 2 === 0);
    const highest = Math.max(
      1,
      values.reduce(
        (current, value) => Math.max(current, value),
        props.max ?? 0,
      ),
    );
    return values.map((value) => Math.max(0.03, Math.min(1, value / highest)));
  });

  const height = props.height ?? 56;

  return (
    <View
      class={cx("flex-row items-end gap-0.5", props.class)}
      style={{ height }}
    >
      <Each of={bars()} key="index">
        {(bar) => (
          <View
            class="flex-1 rounded-sm bg-primary/35"
            style={{ height: Math.max(2, Math.round(bar() * height)) }}
          />
        )}
      </Each>
    </View>
  );
}
