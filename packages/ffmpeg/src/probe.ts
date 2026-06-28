import type { AudioStreamInfo, MediaInfo } from "@qlipq/core";

/** Build the ffprobe argument list that produces parseable JSON for a file. */
export function buildProbeArgs(inputPath: string): string[] {
  return ["-v", "error", "-print_format", "json", "-show_format", "-show_streams", inputPath];
}

/** A single stream entry as produced by `ffprobe -show_streams -print_format json`. */
export interface FfprobeStream {
  index: number;
  codec_type: string;
  codec_name?: string;
  width?: number;
  height?: number;
  channels?: number;
  r_frame_rate?: string;
  avg_frame_rate?: string;
  tags?: Record<string, string>;
}

/** The relevant subset of ffprobe's JSON output. */
export interface FfprobeOutput {
  streams?: FfprobeStream[];
  format?: { duration?: string; size?: string };
}

/** Convert an ffmpeg rational frame rate like `30000/1001` into fps. */
export function parseFrameRate(rate: string | undefined): number {
  if (!rate) return 0;
  const [num, den] = rate.split("/").map(Number);
  if (!den || Number.isNaN(num)) return Number.isFinite(num) ? num : 0;
  return Math.round((num / den) * 1000) / 1000;
}

/** Parse ffprobe JSON (string or object) into a {@link MediaInfo}. */
export function parseFfprobe(input: string | FfprobeOutput): MediaInfo {
  const data: FfprobeOutput = typeof input === "string" ? JSON.parse(input) : input;
  const streams = data.streams ?? [];
  const video = streams.find((stream) => stream.codec_type === "video");

  const audioStreams: AudioStreamInfo[] = streams
    .filter((stream) => stream.codec_type === "audio")
    .map((stream, i) => ({
      streamIndex: stream.index,
      index: i,
      codec: stream.codec_name ?? "unknown",
      channels: stream.channels ?? 0,
      language: stream.tags?.language,
      title: stream.tags?.title,
    }));

  return {
    durationSec: Number.parseFloat(data.format?.duration ?? "0") || 0,
    width: video?.width ?? 0,
    height: video?.height ?? 0,
    videoCodec: video?.codec_name ?? "unknown",
    fps: parseFrameRate(video?.r_frame_rate ?? video?.avg_frame_rate),
    audioStreams,
    sizeBytes: data.format?.size ? Number.parseInt(data.format.size, 10) : undefined,
  };
}
