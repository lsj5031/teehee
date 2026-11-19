import { Image, token } from "nsh";
import activity from "../../assets/icons/activity.svg";
import alert from "../../assets/icons/alert.svg";
import arrowRight from "../../assets/icons/arrow-right.svg";
import check from "../../assets/icons/check.svg";
import copy from "../../assets/icons/copy.svg";
import devices from "../../assets/icons/devices.svg";
import diagnostics from "../../assets/icons/diagnostics.svg";
import laptop from "../../assets/icons/laptop.svg";
import pause from "../../assets/icons/pause.svg";
import pc from "../../assets/icons/pc.svg";
import play from "../../assets/icons/play.svg";
import plus from "../../assets/icons/plus.svg";
import refresh from "../../assets/icons/refresh.svg";
import search from "../../assets/icons/search.svg";
import settings from "../../assets/icons/settings.svg";
import shield from "../../assets/icons/shield.svg";
import sliders from "../../assets/icons/sliders.svg";
import stop from "../../assets/icons/stop.svg";
import stream from "../../assets/icons/stream.svg";

const icons = {
  activity,
  alert,
  arrowRight,
  check,
  copy,
  devices,
  diagnostics,
  laptop,
  pause,
  pc,
  play,
  plus,
  refresh,
  search,
  settings,
  shield,
  sliders,
  stop,
  stream,
};

export type IconName = keyof typeof icons;

export type IconTone =
  | "foreground"
  | "muted-foreground"
  | "primary"
  | "primary-foreground"
  | "live"
  | "warning"
  | "destructive"
  | "destructive-foreground";

/** A 16 pt line icon, tinted from the theme. */
export function Icon(props: {
  name: IconName;
  size?: number;
  tone?: IconTone;
}) {
  return (
    <Image
      src={icons[props.name]}
      style={{
        width: props.size ?? 16,
        height: props.size ?? 16,
        tintColor: token(props.tone ?? "muted-foreground"),
      }}
    />
  );
}
