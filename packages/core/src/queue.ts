import type { EditSpec } from "./edit-spec.ts";
import type { MediaInfo } from "./media.ts";

/** Lifecycle of a clip in the editing queue. */
export type QueueStatus = "pending" | "ready" | "editing" | "exporting" | "done" | "error";

/** A recording tracked in the queue, with any parsed metadata and edit state. */
export interface QueueItem {
  id: string;
  /** Absolute path on disk. */
  path: string;
  fileName: string;
  /** ISO timestamp of when it entered the queue. */
  addedAt: string;
  status: QueueStatus;
  /** ISO timestamp parsed from the filename, if any. */
  recordedAt?: string;
  /** Scene/game label parsed from the filename, if any. */
  source?: string;
  /** Probed media info, populated lazily when the clip is opened. */
  media?: MediaInfo;
  /** Working edit spec, persisted so re-opening a clip restores progress. */
  edit?: EditSpec;
  /** Where the last successful export was written. */
  exportPath?: string;
  error?: string;
}
