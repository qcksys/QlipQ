import { formatDate, formatDateTime, formatTime } from "./datetime.ts";

/** Values available to a naming template when renaming a clip. */
export interface RenameVars {
  /** Original base name without extension. */
  name: string;
  /** Original extension without the leading dot. */
  ext: string;
  recordedAt?: Date;
  source?: string;
  /** 1-based position used by the `{index}` token. */
  index?: number;
}

const FALLBACK_BASE = "clip";

// Characters illegal in Windows filenames (the strictest common target).
// Dashes and spaces are intentionally allowed; date/time tokens rely on dashes.
const ILLEGAL = /[<>:"/\\|?*]/g;

/** Replace illegal filename characters (incl. control chars) and trim trailing dots/spaces. */
export function sanitizeFileName(name: string): string {
  const withoutControls = Array.from(name, (ch) => (ch.charCodeAt(0) < 0x20 ? "_" : ch)).join("");
  return withoutControls
    .replace(ILLEGAL, "_")
    .replace(/[ .]+$/, "")
    .trim();
}

/** Collapse runs of separators left behind by empty tokens, and trim edge separators. */
function tidySeparators(value: string): string {
  return value
    .replace(/[_.\s-]{2,}/g, (run) => (run.includes(" ") ? " " : run[0]))
    .replace(/^[_.\s-]+|[_.\s-]+$/g, "");
}

/**
 * Expand a naming template into a base filename (no extension).
 *
 * Supported tokens: `{name} {source} {date} {time} {datetime} {index} {ext}`.
 * Unknown tokens expand to an empty string; the result is sanitized and falls
 * back to `clip` if everything resolved away.
 */
export function applyNamingTemplate(template: string, vars: RenameVars): string {
  const expanded = template.replace(/\{(\w+)\}/g, (_match, token: string) => {
    switch (token) {
      case "name":
        return vars.name;
      case "source":
        return vars.source ?? "";
      case "date":
        return vars.recordedAt ? formatDate(vars.recordedAt) : "";
      case "time":
        return vars.recordedAt ? formatTime(vars.recordedAt) : "";
      case "datetime":
        return vars.recordedAt ? formatDateTime(vars.recordedAt) : "";
      case "index":
        return vars.index == null ? "" : String(vars.index);
      case "ext":
        return vars.ext;
      default:
        return "";
    }
  });
  const base = tidySeparators(sanitizeFileName(expanded));
  return base || FALLBACK_BASE;
}

/** Build a full target filename (base + preserved extension) from a template. */
export function buildRenamedFileName(template: string, vars: RenameVars): string {
  const base = applyNamingTemplate(template, vars);
  return vars.ext ? `${base}.${vars.ext}` : base;
}

/** Split a filename into its base name and extension (without dot). */
export function splitFileName(fileName: string): { name: string; ext: string } {
  const dot = fileName.lastIndexOf(".");
  if (dot <= 0) return { name: fileName, ext: "" };
  return { name: fileName.slice(0, dot), ext: fileName.slice(dot + 1) };
}
