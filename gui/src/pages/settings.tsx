import { Text, View, Scroll } from "nsh";
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
        <Card class="flex-col gap-2 p-4">
          <SectionLabel label="This build" />
          <Text class="text-[13px] text-foreground">
            Startup, tray, and reconnection controls are not available yet.
          </Text>
          <Text class="text-[11px] text-muted-foreground">
            teehee starts and stops streams manually. This build does not write
            a session log, expose a control port, or follow the Windows volume.
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
