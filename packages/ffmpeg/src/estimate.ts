import { type EditSpec, effectiveDuration, type MediaInfo } from "@qcksys/qlipq-core";
import type { ResolvedEncode } from "./args.ts";

export interface SizeEstimate {
  bytes: number;
  /** True when the figure is a quality-model ballpark (CRF/preset), not a hard target. */
  approximate: boolean;
}

/**
 * Baseline bits-per-pixel (per frame) for each codec at CRF 23. Each 6 CRF steps
 * roughly halves/doubles bitrate, so `bpp = base * 2^((23 - crf) / 6)`. These are
 * deliberately rough — CRF output is content-dependent.
 */
const BPP_AT_CRF23: Record<string, number> = {
  libx264: 0.095,
  libx265: 0.06,
};

/** Estimate the exported file size for a clip under the resolved encode settings. */
export function estimateExportSize(
  media: MediaInfo,
  spec: EditSpec,
  encode: ResolvedEncode,
): SizeEstimate {
  const duration = effectiveDuration(spec, media);
  if (duration <= 0) return { bytes: 0, approximate: false };

  const { video } = encode;
  const forcedReencode = !!spec.crop || !!video.scaleHeight || !!video.fps;
  const reencoding = encode.reencode || forcedReencode;

  // Pure stream-copy: output ≈ the source scaled by the fraction of duration kept.
  if (!reencoding) {
    const sourceDuration = media.durationSec || duration;
    const sourceSize = media.sizeBytes ?? 0;
    return { bytes: sourceSize * (duration / sourceDuration), approximate: false };
  }

  // Output frame dimensions after crop + downscale.
  const cropW = spec.crop?.width ?? media.width;
  const cropH = spec.crop?.height ?? media.height;
  let outW = cropW;
  let outH = cropH;
  if (video.scaleHeight && cropH > 0) {
    outH = video.scaleHeight;
    outW = Math.round((cropW * (video.scaleHeight / cropH)) / 2) * 2;
  }
  const outFps = video.fps && video.fps > 0 ? video.fps : media.fps || 30;

  const audioTracks = spec.audioTracks.filter((track) => track.enabled).length;
  const audioKbps = audioTracks * parseInt(encode.audio.bitrate ?? "0", 10);
  const audioBytes = (audioKbps * 1000 * duration) / 8;

  if (video.bitrateKbps) {
    const videoBytes = (video.bitrateKbps * 1000 * duration) / 8;
    return { bytes: videoBytes + audioBytes, approximate: false };
  }

  const base = BPP_AT_CRF23[video.codec ?? "libx264"] ?? BPP_AT_CRF23.libx264;
  const bpp = base * 2 ** ((23 - (video.crf ?? 20)) / 6);
  let videoBps = outW * outH * outFps * bpp;
  // Constrained VBR caps the bitrate, so the estimate can't exceed the cap.
  if (video.maxrateKbps) videoBps = Math.min(videoBps, video.maxrateKbps * 1000);
  const videoBytes = (videoBps * duration) / 8;
  return { bytes: videoBytes + audioBytes, approximate: true };
}
