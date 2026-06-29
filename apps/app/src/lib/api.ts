import { type AppConfig, detectObsRecordingFolder, type ObsConfigFiles } from "@qcksys/qlipq-core";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openPath, openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { toPosixPath } from "./queue.ts";

/** Public repository URL, used by the in-app GitHub link. */
export const REPO_URL = "https://github.com/qcksys/qlipq";

/** FFmpeg project homepage, linked from the "Powered by FFmpeg" callout. */
export const FFMPEG_URL = "https://ffmpeg.org";

/** Load persisted configuration (merged with defaults on the Rust side). */
export function getConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("get_config");
}

export function setConfig(config: AppConfig): Promise<void> {
  return invoke("set_config", { config });
}

/** Absolute path to the persisted config.json. */
export function getConfigPath(): Promise<string> {
  return invoke<string>("config_file_path");
}

/** Filesystem size + modified time for a batch of files. */
export interface FileInfo {
  path: string;
  size: number;
  modifiedMs: number;
}

export function fileInfo(paths: string[]): Promise<FileInfo[]> {
  return invoke<FileInfo[]>("file_info", { paths });
}

/** Run `<path> -version`; resolves to the version banner, rejects if not runnable. */
export function checkBinary(path: string): Promise<string> {
  return invoke<string>("check_binary", { path });
}

/** Read a named file from the app data dir (null if absent). */
export function readAppFile(name: string): Promise<string | null> {
  return invoke<string | null>("read_app_file", { name });
}

export function writeAppFile(name: string, contents: string): Promise<void> {
  return invoke("write_app_file", { name, contents });
}

/** List existing video files in the given folders and all their subfolders. */
export function scanFolders(folders: string[], extensions: string[]): Promise<string[]> {
  return invoke<string[]>("scan_folders", { folders, extensions });
}

/** Detected capture-app recording folders, used to offer one-click watch presets. */
export interface CapturePresets {
  /** OBS recording folder, from its profile's `basic.ini`. */
  obs?: string;
  /** NVIDIA Share (ShadowPlay) recording folder, from the registry. */
  nvidiaShare?: string;
}

/** Raw OBS config files (`user.ini` + each profile's `basic.ini`). */
function readObsConfig(): Promise<ObsConfigFiles> {
  return invoke<ObsConfigFiles>("read_obs_config");
}

/** NVIDIA Share recording folder from the registry, or null if unavailable. */
function detectNvidiaRecordingDir(): Promise<string | null> {
  return invoke<string | null>("detect_nvidia_recording_dir");
}

/**
 * Detect OBS and NVIDIA Share recording folders to offer as watch-folder presets.
 * Each source is probed independently; a failure in one leaves the other intact.
 */
export async function detectCapturePresets(): Promise<CapturePresets> {
  const presets: CapturePresets = {};
  try {
    const obs = detectObsRecordingFolder(await readObsConfig());
    if (obs) presets.obs = toPosixPath(obs);
  } catch (err) {
    console.error("OBS preset detection failed", err);
  }
  try {
    const nvidia = await detectNvidiaRecordingDir();
    if (nvidia) presets.nvidiaShare = toPosixPath(nvidia);
  } catch (err) {
    console.error("NVIDIA preset detection failed", err);
  }
  return presets;
}

/** Run ffprobe and return its raw JSON output for parsing with `@qcksys/qlipq-ffmpeg`. */
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
 * Run ffmpeg with a pre-built argument list (see `@qcksys/qlipq-ffmpeg`). Resolves when
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
  return typeof result === "string" ? toPosixPath(result) : null;
}

export function revealInExplorer(path: string): Promise<void> {
  return revealItemInDir(path);
}

export function openInDefaultApp(path: string): Promise<void> {
  return openPath(path);
}

/** Open a URL in the user's default browser. */
export function openExternal(url: string): Promise<void> {
  return openUrl(url);
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
