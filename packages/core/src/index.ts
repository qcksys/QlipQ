export * from "./datetime.ts";
export * from "./media.ts";
export * from "./edit-spec.ts";
export * from "./obs.ts";
export * from "./rename.ts";
export * from "./config.ts";
export * from "./queue.ts";

/** Generate a short, URL/file-safe random id for queue items. */
export function createId(): string {
  return Math.random().toString(36).slice(2, 10) + Date.now().toString(36).slice(-4);
}
