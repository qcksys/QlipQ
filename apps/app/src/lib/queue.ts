import { createId, parseObsFilename, type QueueItem } from "@qlipq/core";

/** Last path segment for both Windows and POSIX separators. */
export function basename(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  return normalized.slice(normalized.lastIndexOf("/") + 1);
}

/** Directory portion of a path (Windows or POSIX). */
export function dirname(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  const idx = normalized.lastIndexOf("/");
  return idx <= 0 ? "" : path.slice(0, idx);
}

/** Join a directory and a name with a forward slash (accepted by ffmpeg on all platforms). */
export function joinPath(dir: string, name: string): string {
  if (!dir) return name;
  return `${dir.replace(/[/\\]+$/, "")}/${name}`;
}

/** Build a fresh queue item from a file path, parsing OBS metadata from the name. */
export function queueItemFromPath(path: string, addedAt: string): QueueItem {
  const fileName = basename(path);
  const parsed = parseObsFilename(fileName);
  return {
    id: createId(),
    path,
    fileName,
    addedAt,
    status: "pending",
    recordedAt: parsed.recordedAt?.toISOString(),
    source: parsed.source,
  };
}
