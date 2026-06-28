import type { MediaInfo } from "./media.ts";

/** A trim window. `endSec` is exclusive (the cut ends at this timestamp). */
export interface TrimSpec {
  startSec: number;
  endSec: number;
}

/** A pixel-space crop rectangle relative to the source frame. */
export interface CropSpec {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Selection and level for one source audio track. */
export interface AudioTrackSpec {
  /** Audio-relative index matching {@link AudioStreamInfo.index}. */
  index: number;
  enabled: boolean;
  /** Linear gain multiplier: 1 = unchanged, 0 = muted, 2 = +6dB. */
  volume: number;
}

/** A complete description of the edits to apply to one clip. */
export interface EditSpec {
  trim?: TrimSpec;
  crop?: CropSpec;
  audioTracks: AudioTrackSpec[];
}

/** An edit spec that applies no changes, selecting every source audio track at unity gain. */
export function defaultEditSpec(media?: MediaInfo): EditSpec {
  return {
    audioTracks: (media?.audioStreams ?? []).map((stream) => ({
      index: stream.index,
      enabled: true,
      volume: 1,
    })),
  };
}

/** The output duration in seconds after trimming, or the full duration when untrimmed. */
export function effectiveDuration(spec: EditSpec, media: MediaInfo): number {
  if (!spec.trim) return media.durationSec;
  return Math.max(0, spec.trim.endSec - spec.trim.startSec);
}

/** Returns an error message if the spec is invalid for the given media, otherwise null. */
export function validateEditSpec(spec: EditSpec, media: MediaInfo): string | null {
  if (spec.trim) {
    const { startSec, endSec } = spec.trim;
    if (startSec < 0) return "Trim start cannot be negative.";
    if (endSec <= startSec) return "Trim end must be after the start.";
    if (endSec > media.durationSec + 0.5) return "Trim end is beyond the clip duration.";
  }
  if (spec.crop) {
    const { x, y, width, height } = spec.crop;
    if (width <= 0 || height <= 0) return "Crop width and height must be positive.";
    if (x < 0 || y < 0) return "Crop position cannot be negative.";
    if (x + width > media.width || y + height > media.height) {
      return "Crop rectangle extends outside the frame.";
    }
  }
  for (const track of spec.audioTracks) {
    if (track.volume < 0) return "Audio volume cannot be negative.";
  }
  if (spec.audioTracks.length > 0 && spec.audioTracks.every((t) => !t.enabled)) {
    // Allowed (produces a silent video), but callers may want to warn.
  }
  return null;
}
