import { expect, test } from "vite-plus/test";
import { parseProgress, parseTimecode, progressFraction } from "../src/progress.ts";

test("parseTimecode converts HH:MM:SS.micro to seconds", () => {
  expect(parseTimecode("00:00:01.500000")).toBe(1.5);
  expect(parseTimecode("01:02:03")).toBe(3723);
  expect(parseTimecode("nope")).toBeNull();
});

test("parseProgress reads out_time_us and continue state", () => {
  const chunk = [
    "frame=120",
    "out_time_us=2500000",
    "out_time=00:00:02.500000",
    "progress=continue",
  ].join("\n");
  const result = parseProgress(chunk);
  expect(result.outTimeSec).toBe(2.5);
  expect(result.done).toBe(false);
});

test("parseProgress detects the end marker", () => {
  const result = parseProgress("out_time_us=10000000\nprogress=end\n");
  expect(result.outTimeSec).toBe(10);
  expect(result.done).toBe(true);
});

test("parseProgress falls back to out_time timecode", () => {
  const result = parseProgress("out_time=00:00:04.000000\nprogress=continue");
  expect(result.outTimeSec).toBe(4);
});

test("progressFraction clamps to 0..1", () => {
  expect(progressFraction(5, 10)).toBe(0.5);
  expect(progressFraction(20, 10)).toBe(1);
  expect(progressFraction(null, 10)).toBe(0);
  expect(progressFraction(5, 0)).toBe(0);
});
