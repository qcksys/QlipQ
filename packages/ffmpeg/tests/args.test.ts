import { expect, test } from "vite-plus/test";
import type { EditSpec, MediaInfo, OutputSettings } from "@qcksys/qlipq-core";
import { DEFAULT_OUTPUT_SETTINGS } from "@qcksys/qlipq-core";
import { buildExportArgs, outputSettingsToEncode } from "../src/args.ts";

const io = { inputPath: "in.mkv", outputPath: "out.mp4" };

function args(spec: EditSpec, extra: Partial<Parameters<typeof buildExportArgs>[0]> = {}) {
  return buildExportArgs({ ...io, spec, ...extra });
}

test("trim-only defaults to a fast stream copy", () => {
  const out = args({
    trim: { startSec: 5, endSec: 12.5 },
    audioTracks: [{ index: 0, enabled: true, volume: 1 }],
  });
  expect(out).toEqual([
    "-y",
    "-ss",
    "5.000",
    "-i",
    "in.mkv",
    "-t",
    "7.500",
    "-map",
    "0:v:0",
    "-map",
    "0:a:0",
    "-c:v",
    "copy",
    "-c:a",
    "copy",
    "out.mp4",
  ]);
});

test("forced reencode on a trim-only spec re-encodes video, copies audio", () => {
  const out = args(
    { trim: { startSec: 0, endSec: 10 }, audioTracks: [{ index: 0, enabled: true, volume: 1 }] },
    { reencode: true },
  );
  expect(out).toContain("-c:v");
  expect(out).toContain("libx264");
  expect(out.join(" ")).toContain("-c:a copy");
  expect(out).not.toContain("-filter_complex");
});

test("crop builds a filter graph and re-encodes video", () => {
  const out = args({
    crop: { x: 100, y: 50, width: 1280, height: 720 },
    audioTracks: [{ index: 0, enabled: true, volume: 1 }],
  });
  const joined = out.join(" ");
  expect(joined).toContain("-filter_complex [0:v:0]crop=1280:720:100:50[vout]");
  expect(joined).toContain("-map [vout]");
  expect(joined).toContain("-map 0:a:0");
  expect(joined).toContain("-c:v libx264");
  expect(joined).toContain("-c:a copy");
});

test("audio volume change re-encodes audio via filter, copies video", () => {
  const out = args({
    audioTracks: [
      { index: 0, enabled: true, volume: 0.5 },
      { index: 1, enabled: true, volume: 1 },
    ],
  });
  const joined = out.join(" ");
  expect(joined).toContain("-filter_complex [0:a:0]volume=0.5[aout0]");
  expect(joined).toContain("-map 0:v:0");
  expect(joined).toContain("-map [aout0]");
  expect(joined).toContain("-map 0:a:1");
  expect(joined).toContain("-c:v copy");
  expect(joined).toContain("-c:a aac -b:a 192k");
});

test("crop plus volume combines video and audio filters", () => {
  const out = args({
    crop: { x: 0, y: 0, width: 640, height: 480 },
    audioTracks: [{ index: 0, enabled: true, volume: 2 }],
  });
  const joined = out.join(" ");
  expect(joined).toContain("[0:v:0]crop=640:480:0:0[vout];[0:a:0]volume=2[aout0]");
  expect(joined).toContain("-c:v libx264");
  expect(joined).toContain("-c:a aac");
});

test("disabling all audio yields -an and no audio codec", () => {
  const out = args({
    audioTracks: [
      { index: 0, enabled: false, volume: 1 },
      { index: 1, enabled: false, volume: 1 },
    ],
  });
  expect(out).toContain("-an");
  expect(out).not.toContain("-c:a");
});

test("progress flag appends machine-readable progress to stdout", () => {
  const out = args({ audioTracks: [{ index: 0, enabled: true, volume: 1 }] }, { progress: true });
  expect(out.join(" ")).toContain("-progress pipe:1 -nostats");
});

test("custom encoder options are honoured", () => {
  const out = args(
    {
      crop: { x: 0, y: 0, width: 100, height: 100 },
      audioTracks: [{ index: 0, enabled: true, volume: 0 }],
    },
    {
      video: { codec: "libx265", crf: 28, preset: "fast" },
      audio: { codec: "libopus", bitrate: "96k" },
    },
  );
  const joined = out.join(" ");
  expect(joined).toContain("-c:v libx265 -preset fast -crf 28");
  expect(joined).toContain("-c:a libopus -b:a 96k");
});

test("metadata stamps -metadata entries before the output", () => {
  const out = args(
    { audioTracks: [{ index: 0, enabled: true, volume: 1 }] },
    { metadata: { game: "Deadlock" } },
  );
  const joined = out.join(" ");
  expect(joined).toContain("-metadata game=Deadlock");
  expect(out.indexOf("-metadata")).toBeLessThan(out.indexOf("out.mp4"));
});

test("frame rate change re-encodes and emits -r without a filter", () => {
  const out = args(
    { audioTracks: [{ index: 0, enabled: true, volume: 1 }] },
    { video: { fps: 30 } },
  );
  const joined = out.join(" ");
  expect(joined).toContain("-c:v libx264");
  expect(joined).toContain("-r 30");
  expect(out).not.toContain("-filter_complex");
  expect(joined).toContain("-c:a copy");
});

test("downscale builds a scale filter and re-encodes", () => {
  const out = args(
    { audioTracks: [{ index: 0, enabled: true, volume: 1 }] },
    { video: { scaleHeight: 720 } },
  );
  const joined = out.join(" ");
  expect(joined).toContain("-filter_complex [0:v:0]scale=-2:720[vout]");
  expect(joined).toContain("-map [vout]");
  expect(joined).toContain("-c:v libx264");
});

test("crop and downscale compose into one filter chain", () => {
  const out = args(
    { crop: { x: 0, y: 0, width: 1920, height: 1080 }, audioTracks: [] },
    { video: { scaleHeight: 720 } },
  );
  expect(out.join(" ")).toContain("[0:v:0]crop=1920:1080:0:0,scale=-2:720[vout]");
});

test("bitrate rate-control uses -b:v instead of -crf", () => {
  const out = args(
    { audioTracks: [{ index: 0, enabled: true, volume: 1 }] },
    { reencode: true, video: { bitrateKbps: 6000 } },
  );
  const joined = out.join(" ");
  expect(joined).toContain("-b:v 6000k");
  expect(joined).not.toContain("-crf");
});

const MEDIA: MediaInfo = {
  durationSec: 60,
  width: 2560,
  height: 1440,
  videoCodec: "h264",
  fps: 60,
  audioStreams: [],
};

function settings(over: Partial<OutputSettings>): OutputSettings {
  return { ...DEFAULT_OUTPUT_SETTINGS, ...over };
}

test("outputSettingsToEncode: original preset is a stream copy", () => {
  const r = outputSettingsToEncode(settings({ qualityPreset: "original" }), MEDIA);
  expect(r.reencode).toBe(false);
});

test("outputSettingsToEncode: named presets map to CRF and force re-encode", () => {
  expect(outputSettingsToEncode(settings({ qualityPreset: "high" }), MEDIA).video.crf).toBe(18);
  expect(outputSettingsToEncode(settings({ qualityPreset: "balanced" }), MEDIA).video.crf).toBe(23);
  const small = outputSettingsToEncode(settings({ qualityPreset: "small" }), MEDIA);
  expect(small.video.crf).toBe(28);
  expect(small.reencode).toBe(true);
});

test("outputSettingsToEncode: bitrate mode sets bitrateKbps", () => {
  const r = outputSettingsToEncode(
    settings({ qualityMode: "bitrate", videoBitrateKbps: 5000 }),
    MEDIA,
  );
  expect(r.video.bitrateKbps).toBe(5000);
  expect(r.reencode).toBe(true);
});

test("outputSettingsToEncode: vbr maps to crf + maxrate cap", () => {
  const r = outputSettingsToEncode(
    settings({ qualityMode: "vbr", crf: 22, videoBitrateKbps: 9000 }),
    MEDIA,
  );
  expect(r.video.crf).toBe(22);
  expect(r.video.maxrateKbps).toBe(9000);
  expect(r.reencode).toBe(true);
  const out = args(
    { audioTracks: [{ index: 0, enabled: true, volume: 1 }] },
    { reencode: true, video: r.video },
  );
  const joined = out.join(" ");
  expect(joined).toContain("-crf 22");
  expect(joined).toContain("-maxrate 9000k");
  expect(joined).toContain("-bufsize 18000k");
});

test("outputSettingsToEncode: fps/maxHeight clamp against the source (no up-rate/up-scale)", () => {
  const up = outputSettingsToEncode(settings({ fps: 120, maxHeight: 2160 }), MEDIA);
  expect(up.video.fps).toBeUndefined();
  expect(up.video.scaleHeight).toBeUndefined();
  const down = outputSettingsToEncode(settings({ fps: 30, maxHeight: 1080 }), MEDIA);
  expect(down.video.fps).toBe(30);
  expect(down.video.scaleHeight).toBe(1080);
});
