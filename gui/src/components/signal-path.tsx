import { Text, View } from "nsh";
import { Icon, type IconName } from "../ui/icon";

function Endpoint(props: {
  icon: IconName;
  title: string;
  subtitle: string;
  align?: "start" | "end";
}) {
  return (
    <View
      class="flex-col gap-1 w-[132px]"
      classList={{ "items-end": props.align === "end" }}
    >
      <View class="h-9 w-9 items-center justify-center rounded-(--radius) border border-border bg-secondary">
        <Icon name={props.icon} tone="foreground" size={18} />
      </View>
      <Text class="text-[13px] font-medium text-foreground w-[132px] truncate" classList={{ "text-right": props.align === "end" }}>
        {props.title}
      </Text>
      <Text class="font-mono text-[10px] text-muted-foreground w-[132px] truncate" classList={{ "text-right": props.align === "end" }}>
        {props.subtitle}
      </Text>
    </View>
  );
}

/** The visual object of the app: this PC, the link, the receiver. */
export function SignalPath(props: {
  receiverName: string;
  address: string;
  source: string;
  level: number;
  active: boolean;
}) {
  return (
    <View class="flex-row items-center gap-4">
      <Endpoint icon="pc" title="This PC" subtitle={props.source} />

      <View class="flex-1 flex-col items-center gap-2 min-w-0">
        <View class="h-1 w-full overflow-hidden rounded-full bg-border">
          <View
            class={`h-1 w-full origin-left rounded-full bg-primary transition-transform duration-150 ${
              props.active
                ? `scale-x-[${(0.28 + props.level * 0.72).toFixed(2)}]`
                : "scale-x-[0.18]"
            }`}
          />
        </View>
        <View class="flex-row items-center gap-1.5">
          <View
            class="h-1.5 w-1.5 rounded-full"
            classList={{
              "bg-live animate-pulse motion-reduce:animate-none": props.active,
              "bg-idle": !props.active,
            }}
          />
          <Text class="text-[10px] text-muted-foreground">
            {props.active ? "sending" : "idle"}
          </Text>
        </View>
      </View>

      <Endpoint
        icon="laptop"
        title={props.receiverName}
        subtitle={props.address}
        align="end"
      />
    </View>
  );
}
