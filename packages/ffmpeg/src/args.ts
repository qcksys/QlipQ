import type { EditSpec, MediaInfo, OutputSettings, QualityPreset } from "@qcksys/qlipq-core";

export interface VideoEncodeOptions {
  codec?: string;
  crf?: number;
  /** Target video bitrate in kbps. When set, takes precedence over {@link crf}. */
  bitrateKbps?: number;
  preset?: string;
  /** Output frame rate; when set, forces a re-encode. */
  fps?: number;
  /** Downscale to this height (keeps aspect, even width); forces a re-encode. */
  scaleHeight?: number;
}

export interface AudioEncodeOptions {
  codec?: string;
  bitrate?: string;
}

/** Resolved encoding choices, ready to feed {@link buildExportArgs}. */
export interface ResolvedEncode {
  video: VideoEncodeOptions;
  audio: AudioEncodeOptions;
  /** Whether the chosen quality wants a re-encode (edits may force one regardless). */
  reencode: boolean;
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
  /** Container metadata to stamp into the output, e.g. `{ game: "Deadlock" }`. */
  metadata?: Record<string, string>;
}

/** CRF values backing each named quality preset (`original` stream-copies instead). */
const PRESET_CRF: Record<Exclude<QualityPreset, "original">, number> = {
  high: 18,
  balanced: 23,
  small: 28,
};

/** Format a number of seconds for ffmpeg's `-ss`/`-t` (millisecond precision). */
export function formatSeconds(sec: number): string {
  return Math.max(0, sec).toFixed(3);
}

function formatVolume(volume: number): string {
  return String(Number(volume.toFixed(4)));
}

/**
 * Resolve persisted {@link OutputSettings} into concrete encode options for a clip.
 * fps and downscale are clamped against the source so we never up-rate or up-scale.
 */
export function outputSettingsToEncode(output: OutputSettings, media: MediaInfo): ResolvedEncode {
  const fps = output.fps > 0 && output.fps < media.fps ? output.fps : undefined;
  const scaleHeight =
    output.maxHeight > 0 && output.maxHeight < media.height ? output.maxHeight : undefined;

  const video: VideoEncodeOptions = {
    codec: output.videoCodec,
    preset: output.encoderPreset,
    fps,
    scaleHeight,
  };

  let reencode = false;
  if (output.qualityMode === "bitrate") {
    video.bitrateKbps = output.videoBitrateKbps;
    reencode = true;
  } else if (output.qualityMode === "crf") {
    video.crf = output.crf;
    reencode = true;
  } else if (output.qualityPreset === "original") {
    // Stream-copy by default; this crf only applies if an edit forces a re-encode.
    video.crf = 18;
    reencode = false;
  } else {
    video.crf = PRESET_CRF[output.qualityPreset];
    reencode = true;
  }

  return { video, audio: { codec: "aac", bitrate: `${output.audioBitrateKbps}k` }, reencode };
}

/**
 * Build the ffmpeg argument list to apply an {@link EditSpec} to a clip.
 *
 * Behaviour:
 * - Trim uses a fast seek (`-ss` before `-i`, `-t` after). With a stream copy
 *   this snaps to the nearest keyframe; pass `reencode: true` for frame accuracy.
 * - Crop and downscale (`video.scaleHeight`) compose into one video filter and
 *   force a re-encode; a changed frame rate (`video.fps`) also forces one.
 * - `video.bitrateKbps` selects bitrate rate-control (`-b:v`), else CRF (`-crf`).
 * - Audio tracks are mapped by their audio-relative index; a non-unity volume
 *   re-encodes audio (aac by default). Disabling all audio yields `-an`.
 */
export function buildExportArgs(opts: BuildExportOptions): string[] {
  const { inputPath, outputPath, spec } = opts;
  const video = {
    codec: opts.video?.codec ?? "libx264",
    crf: opts.video?.crf ?? 20,
    preset: opts.video?.preset ?? "veryfast",
    bitrateKbps: opts.video?.bitrateKbps,
    fps: opts.video?.fps,
    scaleHeight: opts.video?.scaleHeight,
  };
  const audio = {
    codec: opts.audio?.codec ?? "aac",
    bitrate: opts.audio?.bitrate ?? "192k",
  };

  const enabledAudio = spec.audioTracks.filter((track) => track.enabled);
  const needsVideoFilter = !!spec.crop || !!video.scaleHeight;
  const needsAudioFilter = enabledAudio.some((track) => track.volume !== 1);
  const videoReencode = needsVideoFilter || !!video.fps || !!opts.reencode;
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

    const videoSteps: string[] = [];
    if (spec.crop) {
      const { width, height, x, y } = spec.crop;
      videoSteps.push(`crop=${width}:${height}:${x}:${y}`);
    }
    if (video.scaleHeight) videoSteps.push(`scale=-2:${video.scaleHeight}`);
    if (videoSteps.length > 0) {
      filters.push(`[0:v:0]${videoSteps.join(",")}[vout]`);
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

  if (videoReencode) {
    args.push("-c:v", video.codec, "-preset", video.preset);
    if (video.bitrateKbps) args.push("-b:v", `${video.bitrateKbps}k`);
    else args.push("-crf", String(video.crf));
    if (video.fps) args.push("-r", String(video.fps));
  } else {
    args.push("-c:v", "copy");
  }

  if (enabledAudio.length > 0) {
    args.push(...(audioReencode ? ["-c:a", audio.codec, "-b:a", audio.bitrate] : ["-c:a", "copy"]));
  }

  if (opts.metadata) {
    for (const [key, value] of Object.entries(opts.metadata)) {
      if (value) args.push("-metadata", `${key}=${value}`);
    }
  }

  if (opts.progress) args.push("-progress", "pipe:1", "-nostats");

  args.push(outputPath);
  return args;
}
