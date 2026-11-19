import type { JSX } from "nsh";
import { Text, View } from "nsh";
import { cx } from "../lib/format";
import { Icon } from "./icon";

/** A hairline surface; the only container in the app. */
export function Card(props: { children?: JSX.Element; class?: string }) {
  return (
    <View
      class={cx(
        "rounded-(--radius-card) border border-border bg-card",
        props.class,
      )}
    >
      {props.children}
    </View>
  );
}

/** A small uppercase group label. */
export function SectionLabel(props: { label: string }) {
  return (
    <Text class="text-[10px] font-semibold uppercase tracking-[0.09em] text-muted-foreground">
      {props.label}
    </Text>
  );
}

export type ChipTone = "live" | "idle" | "waiting" | "warning" | "error";

const chipTones: Record<ChipTone, string> = {
  live: "border-live/25 bg-live/10 text-live",
  idle: "border-border bg-secondary text-muted-foreground",
  waiting: "border-border bg-secondary text-muted-foreground",
  warning: "border-warning/30 bg-warning/10 text-warning",
  error: "border-destructive/30 bg-destructive/10 text-destructive",
};

const dotTones: Record<ChipTone, string> = {
  live: "bg-live",
  idle: "bg-idle",
  waiting: "bg-idle",
  warning: "bg-warning",
  error: "bg-destructive",
};

/** A short state label with a dot; never color alone. */
export function StateChip(props: { tone: ChipTone; label: string }) {
  return (
    <View
      class={cx(
        "flex-row items-center gap-1.5 rounded-full border px-2 py-0.5",
        chipTones[props.tone],
      )}
    >
      <View
        class={cx(
          "h-1.5 w-1.5 rounded-full",
          dotTones[props.tone],
          props.tone === "live" ? "animate-pulse motion-reduce:animate-none" : "",
        )}
      />
      <Text class="text-[11px] font-medium">{props.label}</Text>
    </View>
  );
}

/** A number with its label and supporting line. */
export function Stat(props: {
  label: string;
  value: string;
  unit?: string;
  detail?: string;
}) {
  return (
    <View class="flex-col gap-1">
      <Text class="text-[10px] font-semibold uppercase tracking-[0.09em] text-muted-foreground">
        {props.label}
      </Text>
      <View class="flex-row items-baseline gap-1.5">
        <Text class="font-mono text-[15px] font-medium text-foreground">
          {props.value}
        </Text>
        <If when={props.unit}>
          <Text class="text-[11px] text-muted-foreground">{props.unit}</Text>
        </If>
      </View>
      <If when={props.detail}>
        <Text class="text-[11px] text-muted-foreground">{props.detail}</Text>
      </If>
    </View>
  );
}

/** An inline explanation, optionally with one action. */
export function Callout(props: {
  tone: "info" | "warning" | "error" | "clear";
  title: string;
  detail: string;
  action?: JSX.Element;
}) {
  const surface =
    props.tone === "warning"
      ? "border-warning/30 bg-warning/10"
      : props.tone === "error"
        ? "border-destructive/30 bg-destructive/10"
        : props.tone === "clear"
          ? "border-border bg-secondary/50"
          : "border-border bg-secondary/50";
  const icon =
    props.tone === "warning" || props.tone === "error" ? "alert" : "check";
  const tone =
    props.tone === "warning"
      ? "warning"
      : props.tone === "error"
        ? "destructive"
        : "primary";

  return (
    <View
      class={cx(
        "flex-row items-start gap-3 rounded-(--radius-card) border p-3",
        surface,
      )}
    >
      <View class="pt-0.5">
        <Icon name={icon} tone={tone} />
      </View>
      <View class="flex-1 flex-col gap-0.5 min-w-0">
        <Text class="text-[13px] font-medium text-foreground">{props.title}</Text>
        <Text class="text-[11px] text-muted-foreground">{props.detail}</Text>
      </View>
      {props.action}
    </View>
  );
}
