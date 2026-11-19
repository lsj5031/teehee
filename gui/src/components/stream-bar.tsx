import { computed, Text, View } from "nsh";
import {
  activeReceiver,
  phase,
  senderDelayMs,
  stopSend,
  toggleSendPause,
} from "../state/session";
import { IconButton } from "../ui/controls";
import { Icon } from "../ui/icon";
import { StateChip } from "../ui/surfaces";

/** The persistent transport on every page except Stream itself. */
export function StreamBar() {
  const paused = computed(() => phase() === "paused");
  const state = computed(() => {
    switch (phase()) {
      case "live":
        return { tone: "live" as const, label: "Live" };
      case "paused":
        return { tone: "idle" as const, label: "Paused" };
      case "starting":
        return { tone: "waiting" as const, label: "Starting" };
      default:
        return { tone: "idle" as const, label: "Idle" };
    }
  });

  return (
    <View class="absolute inset-x-4 bottom-4 h-16 flex-row items-center gap-3 rounded-(--radius-card) border border-border bg-card px-4 shadow-lg">
      <StateChip tone={state().tone} label={state().label} />

      <View class="flex-1 flex-row items-center gap-2 min-w-0">
        <Icon name="pc" tone="foreground" />
        <Text class="text-xs font-medium text-foreground">This PC</Text>
        <Icon name="arrowRight" size={12} />
        <Text class="text-xs font-medium text-foreground truncate">
          {activeReceiver()?.name ?? "No active receiver"}
        </Text>
        <Text class="font-mono text-[10px] text-muted-foreground">
          {activeReceiver()?.address ?? ""}
        </Text>
      </View>

      <Text class="font-mono text-xs text-muted-foreground">
        {senderDelayMs()} ms
      </Text>

      <IconButton
        icon={paused() ? "play" : "pause"}
        label={paused() ? "Resume streaming" : "Pause streaming"}
        onPress={toggleSendPause}
      />
      <IconButton
        icon="stop"
        label="Stop streaming"
        onPress={stopSend}
        class="hover:bg-destructive/10"
      />
    </View>
  );
}
