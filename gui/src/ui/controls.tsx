import type { JSX } from "nsh";
import { Text, View } from "nsh";
import { Button as BaseButton, Switch } from "nsh/base";
import { cx } from "../lib/format";
import { Icon, type IconName } from "./icon";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

const variants: Record<ButtonVariant, string> = {
  primary:
    "bg-primary text-primary-foreground hover:bg-primary/90 active:bg-primary/80",
  secondary:
    "border border-border bg-card text-foreground hover:bg-secondary active:bg-secondary/80",
  ghost:
    "text-muted-foreground hover:bg-secondary hover:text-foreground active:bg-secondary/70",
  danger:
    "bg-destructive text-destructive-foreground hover:bg-destructive/90 active:bg-destructive/80",
};

/** The app's button: one press target, one set of states. */
export function Button(props: {
  children?: JSX.Element;
  onPress?: () => void;
  variant?: ButtonVariant;
  disabled?: boolean;
  class?: string;
  testID?: string;
}) {
  return (
    <BaseButton
      onPress={props.onPress}
      disabled={props.disabled}
      testID={props.testID}
      class={cx(
        "h-8 flex-row items-center justify-center gap-2 rounded-(--radius) px-3 text-sm font-medium transition ease-ui duration-150 active:scale-[0.98] focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background disabled:opacity-40 disabled:active:scale-100",
        variants[props.variant ?? "secondary"],
        props.class,
      )}
    >
      {props.children}
    </BaseButton>
  );
}

/** An icon-only button, always with an accessible label. */
export function IconButton(props: {
  icon: IconName;
  label: string;
  onPress?: () => void;
  tone?: "muted-foreground" | "foreground" | "primary-foreground";
  class?: string;
}) {
  return (
    <BaseButton
      accessibilityLabel={props.label}
      onPress={props.onPress}
      class={cx(
        "h-8 w-8 items-center justify-center rounded-(--radius) transition ease-ui duration-150 active:scale-[0.96] hover:bg-secondary focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background",
        props.class,
      )}
    >
      <Icon name={props.icon} tone={props.tone ?? "muted-foreground"} />
    </BaseButton>
  );
}

export interface SegmentOption<T extends string> {
  value: T;
  label: string;
}

/** A segmented control: one silhouette, one selected segment. */
export function Segmented<T extends string>(props: {
  options: readonly SegmentOption<T>[];
  value: T;
  onChange: (value: T) => void;
  class?: string;
}) {
  return (
    <View
      class={cx(
        "flex-row items-center gap-0.5 rounded-(--radius) border border-border bg-secondary p-0.5",
        props.class,
      )}
    >
      <Each of={props.options} key={(option) => option.value}>
        {(option) => (
          <BaseButton
            onPress={() => props.onChange(option().value)}
            selected={option().value === props.value}
            class={cx(
              "h-7 flex-1 items-center justify-center rounded-(--radius-inset) px-2.5 transition-colors ease-ui focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background",
              option().value === props.value
                ? "bg-card text-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            <Text class="text-xs font-medium">{option().label}</Text>
          </BaseButton>
        )}
      </Each>
    </View>
  );
}

/** A settings row with a switch. */
export function SwitchRow(props: {
  label: string;
  description?: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <View class="w-full flex-row items-center justify-between gap-4 py-2.5">
      <View class="flex-1 min-w-0 flex-col gap-0.5">
        <Text class="text-[13px] text-foreground">{props.label}</Text>
        <If when={props.description}>
          <Text class="text-[11px] text-muted-foreground">
            {props.description}
          </Text>
        </If>
      </View>
      <Switch
        checked={props.checked}
        onChange={(event) => props.onChange(event.checked)}
        accessibilityLabel={props.label}
        class="ml-auto w-9 shrink-0 grow-0"
        trackClass="h-5 w-9 p-0.5 rounded-full bg-muted transition-colors ease-ui checked:bg-primary"
        thumbClass="h-4 w-4 rounded-full bg-card shadow-sm transition-transform ease-ui checked:translate-x-4"
      />
    </View>
  );
}

/** A labeled control, with a hint underneath. */
export function Field(props: {
  label: string;
  hint?: string;
  children?: JSX.Element;
}) {
  return (
    <View class="flex-col gap-1.5">
      <Text class="text-xs font-medium text-foreground">{props.label}</Text>
      {props.children}
      <If when={props.hint}>
        <Text class="text-[11px] text-muted-foreground">{props.hint}</Text>
      </If>
    </View>
  );
}
