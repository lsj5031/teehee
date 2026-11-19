import { Image, platform, Text, View } from "nsh";
import appIcon from "../../assets/icon.png";

function WindowControl(props: { kind: "minimize" | "maximize" | "close" }) {
  const closing = props.kind === "close";
  return (
    <View
      class="group h-11 w-[46px] items-center justify-center transition-colors ease-ui"
      classList={{
        "hover:bg-secondary active:bg-secondary/80": !closing,
        "hover:bg-destructive active:bg-destructive/90": closing,
      }}
      desktop={{ windowControl: props.kind, windowDrag: false }}
      focusable={false}
    >
      <If when={props.kind === "minimize"}>
        <View class="h-px w-2.5 bg-foreground/70 group-active:bg-destructive-foreground" />
      </If>
      <If when={props.kind === "maximize"}>
        <View class="h-2.5 w-2.5 rounded-sm border border-foreground/70 group-active:border-destructive-foreground" />
      </If>
      <If when={props.kind === "close"}>
        <View class="relative h-2.5 w-2.5">
          <View class="absolute left-0 top-1/2 h-px w-2.5 rotate-45 bg-foreground/70 group-hover:bg-destructive-foreground" />
          <View class="absolute left-0 top-1/2 h-px w-2.5 -rotate-45 bg-foreground/70 group-hover:bg-destructive-foreground" />
        </View>
      </If>
    </View>
  );
}

/** The app's own 44 pt title bar; the system bar is hidden in nsh.json. */
export function TitleBar() {
  return (
    <View
      class="h-11 shrink-0 flex-row items-center border-b border-border bg-background pl-3"
      desktop={{ windowDrag: true }}
    >
      <View
        class="flex-row items-center gap-2 flex-1 pl-1"
        classList={{ "pl-[72px]": platform.os === "macos" }}
      >
        <Image src={appIcon} class="h-5 w-5 rounded-[5px]" />
        <Text class="text-[13px] font-semibold tracking-tight text-foreground">
          teehee
        </Text>
      </View>

      <If when={platform.os === "windows" || platform.os === "linux"}>
        <View class="flex-row items-center" desktop={{ windowDrag: false }}>
          <WindowControl kind="minimize" />
          <WindowControl kind="maximize" />
          <WindowControl kind="close" />
        </View>
      </If>
    </View>
  );
}
