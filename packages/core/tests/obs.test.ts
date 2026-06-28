import { expect, test } from "vite-plus/test";
import { parseObsFilename } from "../src/obs.ts";

test("parses OBS default recording filename", () => {
  const result = parseObsFilename("2024-01-31 18-09-05.mkv");
  expect(result.recordedAt?.getFullYear()).toBe(2024);
  expect(result.recordedAt?.getMonth()).toBe(0);
  expect(result.recordedAt?.getDate()).toBe(31);
  expect(result.recordedAt?.getHours()).toBe(18);
  expect(result.recordedAt?.getMinutes()).toBe(9);
  expect(result.recordedAt?.getSeconds()).toBe(5);
  expect(result.source).toBeUndefined();
  expect(result.isReplay).toBe(false);
});

test("parses replay buffer filename and flags it", () => {
  const result = parseObsFilename("Replay 2024-12-01_07-30-00.mp4");
  expect(result.isReplay).toBe(true);
  expect(result.recordedAt?.getHours()).toBe(7);
  expect(result.source).toBeUndefined();
});

test("extracts a leading game/scene name as the source", () => {
  const result = parseObsFilename("Apex Legends 2024-03-15 21-45-10.mkv");
  expect(result.source).toBe("Apex Legends");
  expect(result.recordedAt?.getDate()).toBe(15);
});

test("strips a Replay prefix from the source label", () => {
  const result = parseObsFilename("Replay - Valorant - 2024-05-05 12-00-00.mp4");
  expect(result.isReplay).toBe(true);
  expect(result.source).toBe("Valorant");
});

test("supports dotted time separators", () => {
  const result = parseObsFilename("2024-06-28 14.02.59.mov");
  expect(result.recordedAt?.getMinutes()).toBe(2);
});

test("returns no timestamp for an unrecognised name", () => {
  const result = parseObsFilename("random-clip.mp4");
  expect(result.recordedAt).toBeUndefined();
  expect(result.isReplay).toBe(false);
});
