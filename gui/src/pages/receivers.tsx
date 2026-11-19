import { signal, Text, View, Scroll } from "nsh";
import { Input } from "nsh/base";
import {
  activeReceiver,
  addReceiver,
  receivers,
  receiversError,
  receiversSearching,
  refreshReceivers,
  setPage,
  setReceiver,
} from "../state/session";
import { Button } from "../ui/controls";
import { Icon } from "../ui/icon";
import { Card, SectionLabel } from "../ui/surfaces";
import { PageHeader } from "../components/page-header";
import { ReceiverList } from "../components/receiver-list";

const adding = signal(false);
const manualName = signal("");
const manualHost = signal("");

function AddForm() {
  return (
    <Card class="flex-col gap-3 p-4">
      <SectionLabel label="Add a receiver" />
      <View class="flex-col gap-1.5">
        <Text class="text-[11px] font-medium text-foreground">Name</Text>
        <Input
          value={manualName()}
          onChange={({ value }) => manualName.set(value)}
          placeholder="Living room"
          class="h-8 rounded-(--radius) border border-input bg-background px-2.5 text-sm text-foreground"
        />
      </View>
      <View class="flex-col gap-1.5">
        <Text class="text-[11px] font-medium text-foreground">Address</Text>
        <Input
          value={manualHost()}
          onChange={({ value }) => manualHost.set(value)}
          placeholder="192.168.1.111 or host:5000"
          class="h-8 rounded-(--radius) border border-input bg-background px-2.5 font-mono text-sm text-foreground"
        />
        <Text class="text-[10px] text-muted-foreground">
          Accepts host, host:port, or [ipv6]:port. Saved on this PC; mDNS is
          not required.
        </Text>
      </View>
      <View class="flex-row items-center gap-2">
        <Button
          variant="primary"
          disabled={manualHost().trim().length === 0}
          onPress={() => {
            addReceiver(manualName(), manualHost().trim());
            adding.set(false);
            setPage("stream");
          }}
        >
          <Text>Use receiver</Text>
        </Button>
        <Button variant="ghost" onPress={() => adding.set(false)}>
          <Text>Cancel</Text>
        </Button>
      </View>
    </Card>
  );
}

export function ReceiversPage() {
  const found = () => receivers().filter((entry) => !entry.saved);
  const saved = () => receivers().filter((entry) => entry.saved);

  return (
    <View class="flex-1 flex-col">
      <PageHeader
        title="Receivers"
        subtitle={
          activeReceiver()
            ? "Choose a receiver for the next stream."
            : "Devices on this network that can play the stream."
        }
      >
        <View class="flex-row items-center gap-2">
          <Button
            variant="ghost"
            disabled={receiversSearching()}
            onPress={refreshReceivers}
          >
            <Icon name="refresh" size={14} />
            <Text>{receiversSearching() ? "Searching" : "Search"}</Text>
          </Button>
          <Button
            variant="secondary"
            onPress={() => adding.update((value) => !value)}
          >
            <Icon name="plus" size={14} />
            <Text>Add manually</Text>
          </Button>
        </View>
      </PageHeader>

      <Scroll scrollbar={true} class="flex-1" contentClass="flex-col gap-5 px-7 pb-24">
        <If when={adding()}>
          <AddForm />
        </If>

        <Card class="flex-col gap-2 p-4">
          <View class="flex-row items-center justify-between">
            <SectionLabel label="On this network" />
            <View class="flex-row items-center gap-1.5">
              <View
                class="h-1.5 w-1.5 rounded-full"
                classList={{
                  "bg-live animate-pulse motion-reduce:animate-none":
                    receiversSearching(),
                  "bg-idle": !receiversSearching(),
                }}
              />
              <Text class="text-[10px] text-muted-foreground">
                {receiversSearching() ? "Searching" : "mDNS"}
              </Text>
            </View>
          </View>
          <If when={receiversError() !== ""}>
            <Text class="py-2 text-[11px] text-destructive">
              {receiversError()}
            </Text>
          </If>
          <If
            when={found().length > 0}
            else={
              <View class="flex-col gap-2 py-3">
                <Text class="text-[13px] font-medium text-foreground">
                  {receiversSearching()
                    ? "Looking for mDNS receivers…"
                    : "No mDNS receivers found"}
                </Text>
                <Text class="text-[11px] text-muted-foreground">
                  Start teehee recv on the receiver with --mdns. If it is
                  running, allow mDNS through Windows Firewall and macOS Local
                  Network. A saved address above works even when mDNS is
                  blocked.
                </Text>
              </View>
            }
          >
            <Each of={found()} key={(entry) => entry.id}>
              {(entry) => (
                <View class="flex-row items-center gap-3 rounded-(--radius) px-2.5 py-2">
                  <View class="h-1.5 w-1.5 rounded-full bg-live" />
                  <View class="flex-1 flex-col gap-0.5">
                    <Text class="text-[13px] font-medium text-foreground">
                      {entry().name}
                    </Text>
                    <Text class="text-[10px] text-muted-foreground">
                      {entry().seen}
                    </Text>
                  </View>
                  <Text class="font-mono text-[11px] text-muted-foreground">
                    {entry().host}:{entry().port}
                  </Text>
                  <Button
                    variant="ghost"
                    class="h-6 px-2"
                    onPress={() => {
                      setReceiver(entry().id);
                      setPage("stream");
                    }}
                  >
                    <Text class="text-[11px]">Use</Text>
                  </Button>
                </View>
              )}
            </Each>
          </If>
        </Card>

        <Card class="flex-col gap-2 p-4">
          <SectionLabel label="Saved" />
          <If
            when={saved().length > 0}
            else={
              <Text class="py-2 text-[11px] text-muted-foreground">
                Receivers you add by address are saved on this PC.
              </Text>
            }
          >
            <ReceiverList savedOnly onChoose={() => setPage("stream")} />
          </If>
        </Card>

        <Card class="flex-col gap-2 p-4">
          <SectionLabel label="Run this on the receiver" />
          <View class="rounded-(--radius) border border-border bg-secondary/60 px-3 py-2">
            <Text class="font-mono text-[11px] text-foreground">
              ./teehee recv --port 5000 --mdns --prebuffer-ms 200
            </Text>
          </View>
          <Text class="text-[10px] text-muted-foreground">
            --mdns advertises the receiver so this PC can find it. Without it,
            add the receiver's IP above.
          </Text>
        </Card>

        <View class="flex-row items-center gap-2 px-1">
          <Icon name="shield" size={14} />
          <Text class="text-[11px] text-muted-foreground">
            Receivers are not authenticated. Only join networks you trust.
          </Text>
        </View>
      </Scroll>
    </View>
  );
}
