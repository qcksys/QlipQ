/** Container extensions qlipq treats as editable video by default. */
export const DEFAULT_VIDEO_EXTENSIONS = ["mp4", "mkv", "mov", "flv", "webm", "ts"] as const;

/** How output video quality/bitrate is controlled. `vbr` = CRF capped by a max bitrate. */
export type QualityMode = "preset" | "crf" | "bitrate" | "vbr";
/** Named quality presets; `original` stream-copies when possible. */
export type QualityPreset = "original" | "high" | "balanced" | "small";
export type VideoCodecChoice = "libx264" | "libx265";
export type ContainerFormat = "mp4" | "mkv";

/** Default encoding settings applied to every export. */
export interface OutputSettings {
  qualityMode: QualityMode;
  /** Used when {@link qualityMode} is `preset`. */
  qualityPreset: QualityPreset;
  /** Constant Rate Factor (0–51, lower = better); used when `qualityMode` is `crf`. */
  crf: number;
  /** Target video bitrate in kbps; used when `qualityMode` is `bitrate`. */
  videoBitrateKbps: number;
  /** x26x encoder speed preset, e.g. `veryfast`. */
  encoderPreset: string;
  videoCodec: VideoCodecChoice;
  container: ContainerFormat;
  /** Target frame rate; 0 keeps the source rate. Never up-rates. */
  fps: number;
  /** Downscale so height ≤ this many pixels; 0 keeps the source size. Never up-scales. */
  maxHeight: number;
  audioBitrateKbps: number;
}

/** What to do with the source recording after a successful export. */
export type AfterExportAction = "nothing" | "delete" | "move" | "rename" | "prompt";

export interface AfterExportSettings {
  action: AfterExportAction;
  /** Destination folder for the `move` action. */
  moveFolder: string;
  /** Prefix/suffix added to the source file name for the `rename` action. */
  renamePrefix: string;
  renameSuffix: string;
}

export const DEFAULT_AFTER_EXPORT: AfterExportSettings = {
  action: "nothing",
  moveFolder: "",
  renamePrefix: "",
  renameSuffix: "",
};

export const DEFAULT_OUTPUT_SETTINGS: OutputSettings = {
  // `original` preserves the current fast-trim default: stream-copy unless an edit
  // (crop/scale/fps/volume) forces a re-encode.
  qualityMode: "preset",
  qualityPreset: "original",
  crf: 20,
  videoBitrateKbps: 8000,
  encoderPreset: "veryfast",
  videoCodec: "libx264",
  container: "mp4",
  fps: 0,
  maxHeight: 0,
  audioBitrateKbps: 192,
};

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
  /** What to do with the source recording after a successful export. */
  afterExport: AfterExportSettings;
  /** Default encoding settings applied to every export. */
  output: OutputSettings;
}

export const DEFAULT_CONFIG: AppConfig = {
  watchedFolders: [],
  outputFolder: "",
  videoExtensions: [...DEFAULT_VIDEO_EXTENSIONS],
  namingTemplate: "{date}_{source}_{name}",
  ffmpegPath: "ffmpeg",
  ffprobePath: "ffprobe",
  afterExport: { ...DEFAULT_AFTER_EXPORT },
  output: { ...DEFAULT_OUTPUT_SETTINGS },
};

/** Merge a partial (e.g. loaded from disk) over defaults, so new fields gain defaults. */
export function withConfigDefaults(partial: Partial<AppConfig> | null | undefined): AppConfig {
  return {
    ...DEFAULT_CONFIG,
    ...partial,
    // Deep-merge nested groups so sub-fields added in future versions gain defaults.
    afterExport: { ...DEFAULT_AFTER_EXPORT, ...partial?.afterExport },
    output: { ...DEFAULT_OUTPUT_SETTINGS, ...partial?.output },
  };
}

/** True if the file extension (case-insensitive) is one of the configured video types. */
export function isVideoFile(fileName: string, config: Pick<AppConfig, "videoExtensions">): boolean {
  const dot = fileName.lastIndexOf(".");
  if (dot < 0) return false;
  const ext = fileName.slice(dot + 1).toLowerCase();
  return config.videoExtensions.includes(ext);
}
