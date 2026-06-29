/** Probed information about a single audio stream within a media file. */
export interface AudioStreamInfo {
  /** Absolute stream index in the container (as ffmpeg's `0:N`). */
  streamIndex: number;
  /** Audio-relative index used by ffmpeg's `0:a:N` selector. */
  index: number;
  codec: string;
  channels: number;
  language?: string;
  title?: string;
}

/** Probed information about a media file, derived from ffprobe. */
export interface MediaInfo {
  durationSec: number;
  width: number;
  height: number;
  videoCodec: string;
  fps: number;
  audioStreams: AudioStreamInfo[];
  sizeBytes?: number;
}

/** A best-effort, human-friendly label for an audio stream. */
export function audioStreamLabel(stream: AudioStreamInfo): string {
  if (stream.title) return stream.title;
  if (stream.language) return `Track ${stream.index + 1} (${stream.language})`;
  return `Track ${stream.index + 1}`;
}

/** Human-friendly file size, e.g. `1.4 GB`, using binary units (1024). */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const exp = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exp;
  return `${value.toFixed(exp === 0 ? 0 : 1)} ${units[exp]}`;
}
