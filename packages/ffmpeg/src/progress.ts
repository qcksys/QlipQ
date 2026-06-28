export interface ProgressUpdate {
  /** Output timestamp reached so far, in seconds, or null if unknown. */
  outTimeSec: number | null;
  /** True once ffmpeg reports `progress=end`. */
  done: boolean;
}

/** Parse a `HH:MM:SS.micro` timecode into seconds, or null if unparseable. */
export function parseTimecode(value: string): number | null {
  const match = /^(\d+):(\d{2}):(\d{2}(?:\.\d+)?)$/.exec(value.trim());
  if (!match) return null;
  return Number(match[1]) * 3600 + Number(match[2]) * 60 + Number(match[3]);
}

/**
 * Parse one or more `-progress pipe:1` chunks. ffmpeg emits `key=value` lines in
 * blocks terminated by `progress=continue|end`; we return the latest timestamp
 * seen and whether the run finished.
 */
export function parseProgress(text: string): ProgressUpdate {
  let outTimeSec: number | null = null;
  let done = false;

  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    const eq = line.indexOf("=");
    if (eq < 0) continue;
    const key = line.slice(0, eq);
    const value = line.slice(eq + 1);

    if (key === "out_time_us" || key === "out_time_ms") {
      const micros = Number(value);
      if (Number.isFinite(micros) && micros >= 0) outTimeSec = micros / 1_000_000;
    } else if (key === "out_time") {
      const parsed = parseTimecode(value);
      if (parsed !== null) outTimeSec = parsed;
    } else if (key === "progress") {
      done = value === "end";
    }
  }

  return { outTimeSec, done };
}

/** Clamp an export progress fraction (0..1) from the current and total seconds. */
export function progressFraction(outTimeSec: number | null, durationSec: number): number {
  if (outTimeSec === null || durationSec <= 0) return 0;
  return Math.min(1, Math.max(0, outTimeSec / durationSec));
}
