import type { AppConfig } from "@qlipq/core";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";

/** Load persisted configuration (merged with defaults on the Rust side). */
export function getConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("get_config");
}

export function setConfig(config: AppConfig): Promise<void> {
  return invoke("set_config", { config });
}

/** List existing video files in the given folders (non-recursive per OBS layout). */
export function scanFolders(folders: string[], extensions: string[]): Promise<string[]> {
  return invoke<string[]>("scan_folders", { folders, extensions });
}

/** Run ffprobe and return its raw JSON output for parsing with `@qlipq/ffmpeg`. */
export function probeRaw(path: string, ffprobePath: string): Promise<string> {
  return invoke<string>("probe_raw", { path, ffprobePath });
}

/** Rename a file on disk; returns the new absolute path. */
export function renameFile(from: string, to: string): Promise<string> {
  return invoke<string>("rename_file", { from, to });
}

export function deleteFile(path: string): Promise<void> {
  return invoke("delete_file", { path });
}

/** (Re)start filesystem watchers for the given folders. */
export function startWatching(folders: string[], extensions: string[]): Promise<void> {
  return invoke("start_watching", { folders, extensions });
}

/**
 * Run ffmpeg with a pre-built argument list (see `@qlipq/ffmpeg`). Resolves when
 * the process exits successfully and rejects with stderr otherwise. Progress is
 * delivered via {@link onExportProgress} events keyed by `id`.
 */
export function runExport(id: string, ffmpegPath: string, args: string[]): Promise<void> {
  return invoke("run_export", { id, ffmpegPath, args });
}

/** Turn an absolute path into a URL the webview can load (video preview). */
export function fileUrl(path: string): string {
  return convertFileSrc(path);
}

/** Open a native folder picker; returns the chosen path or null. */
export async function pickFolder(): Promise<string | null> {
  const result = await openDialog({ directory: true, multiple: false });
  return typeof result === "string" ? result : null;
}

export function revealInExplorer(path: string): Promise<void> {
  return revealItemInDir(path);
}

export function openInDefaultApp(path: string): Promise<void> {
  return openPath(path);
}

export interface ExportProgressEvent {
  id: string;
  line: string;
}

export function onFileAdded(cb: (path: string) => void): Promise<UnlistenFn> {
  return listen<string>("file-added", (event) => cb(event.payload));
}

export function onExportProgress(cb: (event: ExportProgressEvent) => void): Promise<UnlistenFn> {
  return listen<ExportProgressEvent>("export-progress", (event) => cb(event.payload));
}
