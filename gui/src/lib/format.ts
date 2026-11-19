/** Join classes, dropping the ones that are off. */
export function cx(
  ...parts: Array<string | false | null | undefined>
): string {
  return parts.filter(Boolean).join(" ");
}

/** `12:47`, or `1:02:47` past an hour. */
export function clock(totalSeconds: number): string {
  const total = Math.max(0, Math.floor(totalSeconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const mm = String(minutes).padStart(2, "0");
  const ss = String(seconds).padStart(2, "0");
  return hours > 0 ? `${hours}:${mm}:${ss}` : `${mm}:${ss}`;
}

/** A 0..1 peak as dBFS, or an em dash for silence. */
export function dbfs(peak: number): string {
  if (peak <= 0.001) return "—";
  return `${Math.max(-60, Math.round(20 * Math.log10(peak)))} dBFS`;
}

/** Bytes as `1.2 kB` / `7.7 kB`. */
export function kilobytes(bytes: number): string {
  return `${(bytes / 1000).toFixed(1)} kB`;
}
