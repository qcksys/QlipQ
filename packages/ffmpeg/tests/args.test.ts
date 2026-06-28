import { expect, test } from "vite-plus/test";
import type { EditSpec } from "@qlipq/core";
import { buildExportArgs } from "../src/args.ts";

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
