import { expect, test } from "vite-plus/test";
import { buildProbeArgs, type FfprobeOutput, parseFfprobe, parseFrameRate } from "../src/probe.ts";

test("buildProbeArgs requests JSON format and streams", () => {
  expect(buildProbeArgs("clip.mkv")).toEqual([
    "-v",
    "error",
    "-print_format",
    "json",
    "-show_format",
    "-show_streams",
    "clip.mkv",
  ]);
});

test("parseFrameRate handles rationals and integers", () => {
  expect(parseFrameRate("30000/1001")).toBe(29.97);
  expect(parseFrameRate("60/1")).toBe(60);
  expect(parseFrameRate("0/0")).toBe(0);
  expect(parseFrameRate(undefined)).toBe(0);
});

test("parseFfprobe extracts video and audio-relative indices", () => {
  const probe: FfprobeOutput = {
    streams: [
      {
        index: 0,
        codec_type: "video",
        codec_name: "h264",
        width: 2560,
        height: 1440,
        r_frame_rate: "60/1",
      },
      { index: 1, codec_type: "audio", codec_name: "aac", channels: 2, tags: { title: "Desktop" } },
      {
        index: 2,
        codec_type: "audio",
        codec_name: "aac",
        channels: 1,
        tags: { language: "eng", title: "Mic" },
      },
    ],
    format: { duration: "63.500000", size: "104857600" },
  };
  const info = parseFfprobe(probe);
  expect(info.durationSec).toBe(63.5);
  expect(info.width).toBe(2560);
  expect(info.height).toBe(1440);
  expect(info.videoCodec).toBe("h264");
  expect(info.fps).toBe(60);
  expect(info.sizeBytes).toBe(104857600);
  expect(info.audioStreams).toEqual([
    { streamIndex: 1, index: 0, codec: "aac", channels: 2, language: undefined, title: "Desktop" },
    { streamIndex: 2, index: 1, codec: "aac", channels: 1, language: "eng", title: "Mic" },
  ]);
});

test("parseFfprobe accepts a JSON string", () => {
  const info = parseFfprobe(JSON.stringify({ streams: [], format: { duration: "1.0" } }));
  expect(info.durationSec).toBe(1);
  expect(info.audioStreams).toEqual([]);
});
