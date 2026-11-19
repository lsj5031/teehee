import { cleanup, mount, View } from "nsh";
import { Scrollbar } from "nsh/base";
import "./assets/fonts/InstrumentSans.ttf";
import { TitleBar } from "./src/components/title-bar";
import { Sidebar } from "./src/components/sidebar";
import { StreamBar } from "./src/components/stream-bar";
import { AudioPage } from "./src/pages/audio";
import { DiagnosticsPage } from "./src/pages/diagnostics";
import { ReceiversPage } from "./src/pages/receivers";
import { SettingsPage } from "./src/pages/settings";
import { StreamPage } from "./src/pages/stream";
import { page, phase, role, stopRecv, stopSend } from "./src/state/session";

Scrollbar.setTheme({
  mode: "always",
  vertical: {
    class: "absolute inset-y-0 right-0 z-10 w-3 p-1",
    thumbClass:
      "mx-auto w-1.5 rounded-full bg-foreground/25 hover:bg-foreground/40",
    accessibilityLabel: "Scroll",
  },
});

function App() {
  cleanup(() => {
    stopSend();
    stopRecv();
  });

  return (
    <View class="flex-1 flex-col bg-background text-foreground font-sans">
      <TitleBar />
      <View class="flex-1 flex-row min-h-0">
        <Sidebar />
        <View class="relative flex-1 min-w-0">
          <If when={page() === "stream"}>
            <StreamPage />
          </If>
          <If when={page() === "receivers" && role() === "send"}>
            <ReceiversPage />
          </If>
          <If when={page() === "audio" && role() === "send"}>
            <AudioPage />
          </If>
          <If when={page() === "diagnostics"}>
            <DiagnosticsPage />
          </If>
          <If when={page() === "settings"}>
            <SettingsPage />
          </If>

          <If
            when={
              role() === "send" &&
              page() !== "stream" &&
              (phase() === "live" ||
                phase() === "paused" ||
                phase() === "starting")
            }
          >
            <StreamBar />
          </If>
        </View>
      </View>
    </View>
  );
}

mount(App);
