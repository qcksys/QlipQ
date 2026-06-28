import { expect, test } from "vite-plus/test";
import { DEFAULT_CONFIG, isVideoFile, withConfigDefaults } from "../src/config.ts";
import { formatDuration } from "../src/datetime.ts";

test("withConfigDefaults fills in missing fields", () => {
  const merged = withConfigDefaults({ outputFolder: "D:/out" });
  expect(merged.outputFolder).toBe("D:/out");
  expect(merged.ffmpegPath).toBe("ffmpeg");
  expect(merged.namingTemplate).toBe(DEFAULT_CONFIG.namingTemplate);
});

test("withConfigDefaults tolerates null", () => {
  expect(withConfigDefaults(null)).toEqual(DEFAULT_CONFIG);
});

test("isVideoFile matches configured extensions case-insensitively", () => {
  expect(isVideoFile("clip.MKV", DEFAULT_CONFIG)).toBe(true);
  expect(isVideoFile("clip.mp4", DEFAULT_CONFIG)).toBe(true);
  expect(isVideoFile("notes.txt", DEFAULT_CONFIG)).toBe(false);
  expect(isVideoFile("noext", DEFAULT_CONFIG)).toBe(false);
});

test("formatDuration renders mm:ss and h:mm:ss", () => {
  expect(formatDuration(65)).toBe("1:05");
  expect(formatDuration(3725)).toBe("1:02:05");
  expect(formatDuration(-5)).toBe("0:00");
});
