import type { JSX } from "nsh";
import { Text, View } from "nsh";

/** One page header: title, one supporting line, and its actions. */
export function PageHeader(props: {
  title: string;
  subtitle: string;
  children?: JSX.Element;
}) {
  return (
    <View class="flex-row items-end justify-between gap-6 px-7 pb-5 pt-6">
      <View class="flex-col gap-1 min-w-0">
        <Text class="text-xl font-semibold tracking-tight text-foreground">
          {props.title}
        </Text>
        <Text class="text-[13px] text-muted-foreground">{props.subtitle}</Text>
      </View>
      <View class="flex-row items-center gap-2 shrink-0">
        {props.children}
      </View>
    </View>
  );
}
