/** Container extensions qlipq treats as editable video by default. */
export const DEFAULT_VIDEO_EXTENSIONS = ["mp4", "mkv", "mov", "flv", "webm", "ts"] as const;

/** Persisted application configuration. */
export interface AppConfig {
  /** Folders watched for new recordings. */
  watchedFolders: string[];
  /** Where exported clips are written. */
  outputFolder: string;
  /** Lower-case extensions (no dot) considered video files. */
  videoExtensions: string[];
  /** Naming template applied on rename/export. See {@link applyNamingTemplate}. */
  namingTemplate: string;
  /** Path or command name for ffmpeg. */
  ffmpegPath: string;
  /** Path or command name for ffprobe. */
  ffprobePath: string;
  /** Remove the source file after a successful export. */
  deleteSourceAfterExport: boolean;
}

export const DEFAULT_CONFIG: AppConfig = {
  watchedFolders: [],
  outputFolder: "",
  videoExtensions: [...DEFAULT_VIDEO_EXTENSIONS],
  namingTemplate: "{date}_{source}_{name}",
  ffmpegPath: "ffmpeg",
  ffprobePath: "ffprobe",
  deleteSourceAfterExport: false,
};

/** Merge a partial (e.g. loaded from disk) over defaults, so new fields gain defaults. */
export function withConfigDefaults(partial: Partial<AppConfig> | null | undefined): AppConfig {
  return { ...DEFAULT_CONFIG, ...partial };
}

/** True if the file extension (case-insensitive) is one of the configured video types. */
export function isVideoFile(fileName: string, config: Pick<AppConfig, "videoExtensions">): boolean {
  const dot = fileName.lastIndexOf(".");
  if (dot < 0) return false;
  const ext = fileName.slice(dot + 1).toLowerCase();
  return config.videoExtensions.includes(ext);
}
