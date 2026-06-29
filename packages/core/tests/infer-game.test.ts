import { expect, test } from "vite-plus/test";
import { inferGameFromPath } from "../src/obs.ts";

test("returns the per-game subfolder under the root", () => {
  expect(inferGameFromPath("E:/Shadowplay", "E:/Shadowplay/Counter-strike 2/clip.mp4")).toBe(
    "Counter-strike 2",
  );
});

test("ignores files directly in the root (flat OBS output)", () => {
  expect(inferGameFromPath("E:/OBS Recordings", "E:/OBS Recordings/clip.mkv")).toBeUndefined();
});

test("tolerates backslashes, mixed separators, and a trailing slash", () => {
  expect(inferGameFromPath("E:\\Shadowplay\\", "E:\\Shadowplay\\Deadlock\\a.mp4")).toBe("Deadlock");
  expect(inferGameFromPath("E:/Shadowplay", "E:/Shadowplay/Deadlock\\a.mp4")).toBe("Deadlock");
});

test("matches the root case-insensitively", () => {
  expect(inferGameFromPath("e:/shadowplay", "E:/Shadowplay/Apex/a.mp4")).toBe("Apex");
});

test("returns undefined when the file is not under the root", () => {
  expect(inferGameFromPath("E:/Shadowplay", "D:/Other/x.mp4")).toBeUndefined();
});
