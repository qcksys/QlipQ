import { expect, test } from "vite-plus/test";
import type { EditSpec, MediaInfo } from "@qcksys/qlipq-core";
import type { ResolvedEncode } from "../src/args.ts";
import { estimateExportSize } from "../src/estimate.ts";

const MEDIA: MediaInfo = {
  durationSec: 100,
  width: 1920,
  height: 1080,
  videoCodec: "h264",
  fps: 60,
  audioStreams: [],
  sizeBytes: 1_000_000_000, // 1 GB over 100s
};

const oneAudio: EditSpec = { audioTracks: [{ index: 0, enabled: true, volume: 1 }] };

const copy: ResolvedEncode = { video: {}, audio: { bitrate: "192k" }, reencode: false };
const crf = (over = {}): ResolvedEncode => ({
  video: { codec: "libx264", crf: 23, ...over },
  audio: { bitrate: "192k" },
  reencode: true,
});

test("stream copy scales source size by the kept duration (accurate)", () => {
  const full = estimateExportSize(MEDIA, oneAudio, copy);
  expect(full.bytes).toBeCloseTo(1_000_000_000, -3);
  expect(full.approximate).toBe(false);

  const half = estimateExportSize(MEDIA, { ...oneAudio, trim: { startSec: 0, endSec: 50 } }, copy);
  expect(half.bytes).toBeCloseTo(500_000_000, -3);
});

test("bitrate mode is an exact bitrate × duration figure", () => {
  const enc: ResolvedEncode = {
    video: { bitrateKbps: 8000 },
    audio: { bitrate: "0k" },
    reencode: true,
  };
  const r = estimateExportSize(MEDIA, { audioTracks: [] }, enc);
  // 8000 kbps × 100s = 800 Mbit = 100 MB.
  expect(r.bytes).toBeCloseTo((8000 * 1000 * 100) / 8, -3);
  expect(r.approximate).toBe(false);
});

test("CRF estimate is approximate and monotonic in quality and resolution", () => {
  const better = estimateExportSize(MEDIA, oneAudio, crf({ crf: 18 }));
  const worse = estimateExportSize(MEDIA, oneAudio, crf({ crf: 28 }));
  expect(better.approximate).toBe(true);
  expect(better.bytes).toBeGreaterThan(worse.bytes); // lower CRF ⇒ bigger file

  const downscaled = estimateExportSize(MEDIA, oneAudio, crf({ crf: 23, scaleHeight: 540 }));
  const fullRes = estimateExportSize(MEDIA, oneAudio, crf({ crf: 23 }));
  expect(downscaled.bytes).toBeLessThan(fullRes.bytes); // fewer pixels ⇒ smaller
});

test("h265 estimates smaller than h264 at the same CRF", () => {
  const h264 = estimateExportSize(MEDIA, oneAudio, crf({ codec: "libx264" }));
  const h265 = estimateExportSize(MEDIA, oneAudio, crf({ codec: "libx265" }));
  expect(h265.bytes).toBeLessThan(h264.bytes);
});

test("zero-length output estimates to zero", () => {
  const r = estimateExportSize(MEDIA, { ...oneAudio, trim: { startSec: 10, endSec: 10 } }, copy);
  expect(r.bytes).toBe(0);
});
