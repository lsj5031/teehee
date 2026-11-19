import { Text, View } from "nsh";
import { Button as BaseButton } from "nsh/base";
import { computed } from "nsh";
import { cx } from "../lib/format";
import {
  receivers,
  selectedReceiverId,
  setReceiver,
} from "../state/session";
import { Icon } from "../ui/icon";

/** One receiver row, found or saved. Selection never starts the stream. */
export function ReceiverList(props: {
  onChoose?: () => void;
  onAdd?: () => void;
  savedOnly?: boolean;
}) {
  const rows = computed(() =>
    props.savedOnly ? receivers().filter((entry) => entry.saved) : receivers(),
  );

  return (
    <View class="flex-col gap-0.5">
      <Each of={rows()} key={(entry) => entry.id}>
        {(entry) => {
          const selected = () => entry().id === selectedReceiverId();
          return (
            <BaseButton
              onPress={() => {
                setReceiver(entry().id);
                props.onChoose?.();
              }}
              selected={selected()}
              class={cx(
                "flex-row items-center gap-3 rounded-(--radius) px-2.5 py-2 text-left transition-colors ease-ui focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background",
                selected() ? "bg-accent/60" : "hover:bg-secondary/60",
              )}
            >
              <View
                class={cx(
                  "h-1.5 w-1.5 rounded-full",
                  selected() ? "bg-live" : "bg-idle",
                )}
              />
              <View class="flex-1 flex-col gap-0.5 min-w-0">
                <Text class="text-[13px] font-medium text-foreground truncate">
                  {entry().name}
                </Text>
                <Text class="text-[10px] text-muted-foreground">
                  {entry().seen}
                </Text>
              </View>
              <Text class="font-mono text-[11px] text-muted-foreground">
                {entry().host}:{entry().port}
              </Text>
              <If when={selected()}>
                <Icon name="check" tone="primary" />
              </If>
            </BaseButton>
          );
        }}
      </Each>

      <If when={props.onAdd}>
        <BaseButton
          onPress={props.onAdd}
          class="flex-row items-center gap-2.5 rounded-(--radius) px-2.5 py-2 transition-colors hover:bg-secondary/60 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"
        >
          <View class="h-4 w-4 items-center justify-center rounded-full border border-input">
            <Icon name="plus" size={10} />
          </View>
          <Text class="text-[12px] text-muted-foreground">
            Add an IP address…
          </Text>
        </BaseButton>
      </If>
    </View>
  );
}
