import { Text, View, Scroll } from "nsh";
import {
  alertsEnabled,
  launchAtLogin,
  launchAtLoginError,
  reconnect,
  sessionLogPath,
  setAlerts,
  setOpenAtLogin,
  setReconnect,
  setStartOnOpen,
  startOnOpen,
} from "../state/session";
import { SwitchRow } from "../ui/controls";
import { Card, SectionLabel } from "../ui/surfaces";
import { PageHeader } from "../components/page-header";

export function SettingsPage() {
  return (
    <View class="flex-1 flex-col">
      <PageHeader
        title="Settings"
        subtitle="What this build does, and what it leaves for later."
      />

      <Scroll scrollbar={true} class="flex-1" contentClass="flex-col gap-5 px-7 pb-24">
        <Card class="flex-col gap-1 p-4">
          <SectionLabel label="Behavior" />
          <SwitchRow
            label="Open at login"
            description="Start teehee when you sign in, in this session where loopback capture works."
            checked={launchAtLogin()}
            onChange={(checked) => {
              void setOpenAtLogin(checked);
            }}
          />
          <SwitchRow
            label="Start streaming when teehee opens"
            description="Begin sending to the saved receiver without pressing Start."
            checked={startOnOpen()}
            onChange={setStartOnOpen}
          />
          <SwitchRow
            label="Reconnect automatically"
            description="Restart the sender after an error, with backoff."
            checked={reconnect()}
            onChange={setReconnect}
          />
          <SwitchRow
            label="Desktop alerts"
            description="Notify when the sender stops or capture needs attention."
            checked={alertsEnabled()}
            onChange={setAlerts}
          />
          <If when={launchAtLoginError() !== ""}>
            <Text class="text-[11px] text-destructive">
              {launchAtLoginError()}
            </Text>
          </If>
        </Card>

        <Card class="flex-col gap-2 p-4">
          <SectionLabel label="Session log" />
          <Text class="font-mono text-[11px] text-foreground">
            {sessionLogPath() || "No session log — the log folder is not writable."}
          </Text>
          <Text class="text-[11px] text-muted-foreground">
            One JSON object per second: packets, ring depth, drift and capture
            counters. The GUI keeps this history itself; no scheduled task is
            involved.
          </Text>
        </Card>

        <Card class="flex-col gap-2 p-4">
          <SectionLabel label="This build" />
          <Text class="text-[11px] text-muted-foreground">
            The local control port, follow-system-volume and the tray menu are
            not exposed here yet. Audio is unencrypted, so use teehee only on a
            network you trust.
          </Text>
        </Card>

        <Card class="flex-col gap-2 p-4">
          <SectionLabel label="Privacy" />
          <Text class="text-[11px] text-muted-foreground">
            Audio travels over your local network to the receiver. It is not
            encrypted or authenticated.
          </Text>
        </Card>

        <Card class="flex-col gap-1 p-4">
          <SectionLabel label="About" />
          <View class="flex-row items-center justify-between py-2">
            <View class="flex-col gap-0.5">
              <Text class="text-[13px] font-medium text-foreground">
                teehee 0.1.0
              </Text>
              <Text class="text-[11px] text-muted-foreground">
                NSH 0.6 · native desktop
              </Text>
            </View>
            <Text class="font-mono text-[10px] text-muted-foreground">
              dev.teehee.desktop
            </Text>
          </View>
          <Text class="border-t border-border pt-2 text-[10px] text-muted-foreground">
            Audio is unencrypted and unauthenticated. Do not expose the
            receiver port to the public internet.
          </Text>
        </Card>
      </Scroll>
    </View>
  );
}
