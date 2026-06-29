import { expect, test } from "vite-plus/test";
import { DEFAULT_CONFIG, isVideoFile, withConfigDefaults } from "../src/config.ts";
import { formatDuration } from "../src/datetime.ts";
import { formatBytes } from "../src/media.ts";

test("withConfigDefaults fills in missing fields", () => {
  const merged = withConfigDefaults({ outputFolder: "D:/out" });
  expect(merged.outputFolder).toBe("D:/out");
  expect(merged.ffmpegPath).toBe("ffmpeg");
  expect(merged.namingTemplate).toBe(DEFAULT_CONFIG.namingTemplate);
});

test("withConfigDefaults tolerates null", () => {
  expect(withConfigDefaults(null)).toEqual(DEFAULT_CONFIG);
});

test("withConfigDefaults deep-merges output, keeping defaults for absent sub-fields", () => {
  const merged = withConfigDefaults({ output: { qualityMode: "bitrate" } } as never);
  expect(merged.output.qualityMode).toBe("bitrate");
  // Untouched sub-fields fall back to defaults rather than becoming undefined.
  expect(merged.output.audioBitrateKbps).toBe(DEFAULT_CONFIG.output.audioBitrateKbps);
  expect(merged.output.container).toBe("mp4");
});

test("formatBytes renders binary units", () => {
  expect(formatBytes(0)).toBe("0 B");
  expect(formatBytes(512)).toBe("512 B");
  expect(formatBytes(1024)).toBe("1.0 KB");
  expect(formatBytes(1024 * 1024 * 1.5)).toBe("1.5 MB");
  expect(formatBytes(3.2 * 1024 ** 3)).toBe("3.2 GB");
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
