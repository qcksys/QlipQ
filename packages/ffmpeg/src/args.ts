import type { EditSpec } from "@qlipq/core";

export interface VideoEncodeOptions {
  codec?: string;
  crf?: number;
  preset?: string;
}

export interface AudioEncodeOptions {
  codec?: string;
  bitrate?: string;
}

export interface BuildExportOptions {
  inputPath: string;
  outputPath: string;
  spec: EditSpec;
  /** Force a full re-encode even when a stream copy would suffice. */
  reencode?: boolean;
  /** Append `-progress pipe:1 -nostats` for machine-readable progress on stdout. */
  progress?: boolean;
  video?: VideoEncodeOptions;
  audio?: AudioEncodeOptions;
}

const DEFAULT_VIDEO: Required<VideoEncodeOptions> = {
  codec: "libx264",
  crf: 20,
  preset: "veryfast",
};
const DEFAULT_AUDIO: Required<AudioEncodeOptions> = {
  codec: "aac",
  bitrate: "192k",
};

/** Format a number of seconds for ffmpeg's `-ss`/`-t` (millisecond precision). */
export function formatSeconds(sec: number): string {
  return Math.max(0, sec).toFixed(3);
}

function formatVolume(volume: number): string {
  return String(Number(volume.toFixed(4)));
}

/**
 * Build the ffmpeg argument list to apply an {@link EditSpec} to a clip.
 *
 * Behaviour:
 * - Trim uses a fast seek (`-ss` before `-i`, `-t` after). With a stream copy
 *   this snaps to the nearest keyframe; pass `reencode: true` for frame accuracy.
 * - Crop forces a video re-encode (libx264 by default).
 * - Audio tracks are mapped by their audio-relative index; a non-unity volume
 *   re-encodes audio (aac by default). Disabling all audio yields `-an`.
 */
export function buildExportArgs(opts: BuildExportOptions): string[] {
  const { inputPath, outputPath, spec } = opts;
  const video = { ...DEFAULT_VIDEO, ...opts.video };
  const audio = { ...DEFAULT_AUDIO, ...opts.audio };

  const enabledAudio = spec.audioTracks.filter((track) => track.enabled);
  const needsVideoFilter = !!spec.crop;
  const needsAudioFilter = enabledAudio.some((track) => track.volume !== 1);
  const videoReencode = needsVideoFilter || !!opts.reencode;
  const audioReencode = needsAudioFilter;

  const args: string[] = ["-y"];

  let duration: number | undefined;
  if (spec.trim) {
    args.push("-ss", formatSeconds(spec.trim.startSec));
    duration = Math.max(0, spec.trim.endSec - spec.trim.startSec);
  }
  args.push("-i", inputPath);
  if (duration !== undefined) args.push("-t", formatSeconds(duration));

  if (needsVideoFilter || needsAudioFilter) {
    const filters: string[] = [];
    let videoMap = "0:v:0";
    if (spec.crop) {
      const { width, height, x, y } = spec.crop;
      filters.push(`[0:v:0]crop=${width}:${height}:${x}:${y}[vout]`);
      videoMap = "[vout]";
    }
    const audioMaps: string[] = [];
    enabledAudio.forEach((track, i) => {
      if (track.volume !== 1) {
        const label = `[aout${i}]`;
        filters.push(`[0:a:${track.index}]volume=${formatVolume(track.volume)}${label}`);
        audioMaps.push(label);
      } else {
        audioMaps.push(`0:a:${track.index}`);
      }
    });
    args.push("-filter_complex", filters.join(";"));
    args.push("-map", videoMap);
    for (const map of audioMaps) args.push("-map", map);
  } else {
    args.push("-map", "0:v:0");
    if (enabledAudio.length === 0) {
      args.push("-an");
    } else {
      for (const track of enabledAudio) args.push("-map", `0:a:${track.index}`);
    }
  }

  args.push(
    ...(videoReencode
      ? ["-c:v", video.codec, "-preset", video.preset, "-crf", String(video.crf)]
      : ["-c:v", "copy"]),
  );

  if (enabledAudio.length > 0) {
    args.push(...(audioReencode ? ["-c:a", audio.codec, "-b:a", audio.bitrate] : ["-c:a", "copy"]));
  }

  if (opts.progress) args.push("-progress", "pipe:1", "-nostats");

  args.push(outputPath);
  return args;
}
