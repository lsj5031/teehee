import { computed, Text, View } from "nsh";
import { Button as BaseButton } from "nsh/base";
import { cx } from "../lib/format";
import {
  page,
  phase,
  recvPhase,
  role,
  setPage,
  setRole,
  type Page,
} from "../state/session";
import { Segmented } from "../ui/controls";
import { Icon, type IconName } from "../ui/icon";

interface NavItem {
  id: Page;
  label: string;
  icon: IconName;
}

const sendNav: readonly NavItem[] = [
  { id: "stream", label: "Stream", icon: "stream" },
  { id: "receivers", label: "Receivers", icon: "devices" },
  { id: "audio", label: "Audio", icon: "sliders" },
  { id: "diagnostics", label: "Diagnostics", icon: "diagnostics" },
];

const recvNav: readonly NavItem[] = [
  { id: "stream", label: "Stream", icon: "stream" },
  { id: "diagnostics", label: "Diagnostics", icon: "diagnostics" },
];

const roles = [
  { value: "send", label: "Send" },
  { value: "recv", label: "Receive" },
] as const;

function NavRow(props: { item: NavItem }) {
  const selected = () => page() === props.item.id;
  return (
    <BaseButton
      onPress={() => setPage(props.item.id)}
      class={cx(
        "h-8 flex-row items-center gap-2.5 rounded-(--radius) px-2.5 transition-colors ease-ui focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background",
        selected()
          ? "bg-sidebar-accent text-foreground"
          : "text-muted-foreground hover:bg-sidebar-accent/70 hover:text-foreground",
      )}
    >
      <If when={selected()}>
        <View class="h-3.5 w-0.5 rounded-full bg-primary" />
      </If>
      <Icon
        name={props.item.icon}
        tone={selected() ? "primary" : "muted-foreground"}
      />
      <Text class="text-[13px] font-medium flex-1 text-left">
        {props.item.label}
      </Text>
      <If          when={
            props.item.id === "stream" &&
            (role() === "send"
              ? phase() === "live" || phase() === "paused"
              : recvPhase() === "live" || recvPhase() === "paused")
          }
      >
        <View class="h-1.5 w-1.5 rounded-full bg-live animate-pulse motion-reduce:animate-none" />
      </If>
    </BaseButton>
  );
}

export function Sidebar() {
  const items = computed(() => (role() === "send" ? sendNav : recvNav));

  return (
    <View class="w-[216px] shrink-0 flex-col gap-5 border-r border-sidebar-border bg-sidebar px-3 pb-3 pt-4">
      <View class="flex-col gap-2">
        <Text class="px-1 text-[10px] font-semibold uppercase tracking-[0.09em] text-muted-foreground">
          This device
        </Text>
        <Segmented
          options={roles}
          value={role()}
          onChange={setRole}
          class="bg-sidebar-accent"
        />
      </View>

      <View class="flex-col gap-0.5">
        <Each of={items()} key={(item) => item.id}>
          {(item) => <NavRow item={item()} />}
        </Each>
      </View>

      <View class="flex-1" />

      <View class="flex-col gap-0.5">
        <NavRow item={{ id: "settings", label: "Settings", icon: "settings" }} />
        <Text class="px-2.5 pt-2 text-[10px] text-muted-foreground">
          0.1.0 · native desktop
        </Text>
      </View>
    </View>
  );
}
