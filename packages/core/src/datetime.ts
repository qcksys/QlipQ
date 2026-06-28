/** Zero-dependency date/time formatting helpers used by filename parsing and rename templating. */

function pad(value: number, length = 2): string {
  return String(value).padStart(length, "0");
}

/** `YYYY-MM-DD` in local time. */
export function formatDate(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** `HH-MM-SS` in local time (dashes are filesystem-safe, unlike colons). */
export function formatTime(date: Date): string {
  return `${pad(date.getHours())}-${pad(date.getMinutes())}-${pad(date.getSeconds())}`;
}

/** `YYYY-MM-DD_HH-MM-SS` in local time. */
export function formatDateTime(date: Date): string {
  return `${formatDate(date)}_${formatTime(date)}`;
}

/** Human duration like `1:02:03` or `2:05` from a number of seconds. */
export function formatDuration(totalSeconds: number): string {
  const safe = Math.max(0, Math.floor(totalSeconds));
  const hours = Math.floor(safe / 3600);
  const minutes = Math.floor((safe % 3600) / 60);
  const seconds = safe % 60;
  if (hours > 0) {
    return `${hours}:${pad(minutes)}:${pad(seconds)}`;
  }
  return `${minutes}:${pad(seconds)}`;
}
