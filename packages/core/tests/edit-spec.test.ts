import { expect, test } from "vite-plus/test";
import { defaultEditSpec, effectiveDuration, validateEditSpec } from "../src/edit-spec.ts";
import type { MediaInfo } from "../src/media.ts";

const media: MediaInfo = {
  durationSec: 120,
  width: 1920,
  height: 1080,
  videoCodec: "h264",
  fps: 60,
  audioStreams: [
    { streamIndex: 1, index: 0, codec: "aac", channels: 2, title: "Desktop" },
    { streamIndex: 2, index: 1, codec: "aac", channels: 1, title: "Mic" },
  ],
};

test("defaultEditSpec enables every source audio track at unity gain", () => {
  const spec = defaultEditSpec(media);
  expect(spec.audioTracks).toEqual([
    { index: 0, enabled: true, volume: 1 },
    { index: 1, enabled: true, volume: 1 },
  ]);
  expect(spec.trim).toBeUndefined();
});

test("effectiveDuration reflects the trim window", () => {
  expect(effectiveDuration({ audioTracks: [] }, media)).toBe(120);
  expect(effectiveDuration({ audioTracks: [], trim: { startSec: 10, endSec: 40 } }, media)).toBe(
    30,
  );
});

test("validateEditSpec accepts a sane spec", () => {
  const spec = defaultEditSpec(media);
  spec.trim = { startSec: 5, endSec: 50 };
  spec.crop = { x: 0, y: 0, width: 1280, height: 720 };
  expect(validateEditSpec(spec, media)).toBeNull();
});

test("validateEditSpec rejects an inverted trim", () => {
  expect(validateEditSpec({ audioTracks: [], trim: { startSec: 30, endSec: 10 } }, media)).toMatch(
    /after the start/,
  );
});

test("validateEditSpec rejects a crop outside the frame", () => {
  expect(
    validateEditSpec({ audioTracks: [], crop: { x: 1000, y: 0, width: 1280, height: 720 } }, media),
  ).toMatch(/outside the frame/);
});

test("validateEditSpec rejects negative volume", () => {
  expect(
    validateEditSpec({ audioTracks: [{ index: 0, enabled: true, volume: -1 }] }, media),
  ).toMatch(/volume cannot be negative/);
});
