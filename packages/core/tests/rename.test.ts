import { expect, test } from "vite-plus/test";
import {
  applyNamingTemplate,
  buildRenamedFileName,
  sanitizeFileName,
  splitFileName,
} from "../src/rename.ts";

const recordedAt = new Date(2024, 0, 31, 18, 9, 5);

test("expands date/source/name tokens", () => {
  const out = applyNamingTemplate("{date}_{source}_{name}", {
    name: "raw",
    ext: "mp4",
    recordedAt,
    source: "Apex",
  });
  expect(out).toBe("2024-01-31_Apex_raw");
});

test("collapses separators when a token is empty", () => {
  const out = applyNamingTemplate("{date}_{source}_{name}", {
    name: "raw",
    ext: "mp4",
    recordedAt,
  });
  expect(out).toBe("2024-01-31_raw");
});

test("preserves the original extension when building a filename", () => {
  const out = buildRenamedFileName("{datetime}", {
    name: "raw",
    ext: "MKV",
    recordedAt,
  });
  expect(out).toBe("2024-01-31_18-09-05.MKV");
});

test("falls back to 'clip' when everything resolves away", () => {
  const out = applyNamingTemplate("{source}", { name: "x", ext: "mp4" });
  expect(out).toBe("clip");
});

test("sanitizes illegal characters but keeps dashes", () => {
  expect(sanitizeFileName("a:b/c?d-2024-01-01")).toBe("a_b_c_d-2024-01-01");
});

test("splitFileName separates base and extension", () => {
  expect(splitFileName("clip.final.mp4")).toEqual({ name: "clip.final", ext: "mp4" });
  expect(splitFileName("noext")).toEqual({ name: "noext", ext: "" });
});

test("index token renders a 1-based position", () => {
  const out = applyNamingTemplate("{name}-{index}", { name: "clip", ext: "mp4", index: 3 });
  expect(out).toBe("clip-3");
});
