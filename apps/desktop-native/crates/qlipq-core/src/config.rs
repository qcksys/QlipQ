use serde::{Deserialize, Serialize};

/// How output video quality/bitrate is controlled. `Vbr` = CRF capped by a max bitrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityMode {
    Preset,
    Crf,
    Bitrate,
    Vbr,
}

/// Named quality presets; `Original` stream-copies when possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityPreset {
    Original,
    High,
    Balanced,
    Small,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodecChoice {
    Libx264,
    Libx265,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerFormat {
    Mp4,
    Mkv,
}

/// What to do with the source recording after a successful export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AfterExportAction {
    Nothing,
    Delete,
    Move,
    Rename,
    Prompt,
}

impl VideoCodecChoice {
    /// The ffmpeg encoder token (e.g. `libx264`).
    pub fn to_ffmpeg(self) -> &'static str {
        match self {
            VideoCodecChoice::Libx264 => "libx264",
            VideoCodecChoice::Libx265 => "libx265",
        }
    }
}

impl ContainerFormat {
    /// The container file extension (no dot).
    pub fn extension(self) -> &'static str {
        match self {
            ContainerFormat::Mp4 => "mp4",
            ContainerFormat::Mkv => "mkv",
        }
    }
}

/// Default encoding settings applied to every export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputSettings {
    pub quality_mode: QualityMode,
    pub quality_preset: QualityPreset,
    /// Constant Rate Factor (0–51, lower = better); used when `quality_mode` is `Crf`.
    pub crf: i64,
    /// Target video bitrate in kbps; used when `quality_mode` is `Bitrate`.
    pub video_bitrate_kbps: i64,
    pub encoder_preset: String,
    pub video_codec: VideoCodecChoice,
    pub container: ContainerFormat,
    /// Target frame rate; 0 keeps the source rate. Never up-rates.
    pub fps: i64,
    /// Downscale so height ≤ this many pixels; 0 keeps the source size. Never up-scales.
    pub max_height: i64,
    pub audio_bitrate_kbps: i64,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            quality_mode: QualityMode::Preset,
            quality_preset: QualityPreset::Original,
            crf: 20,
            video_bitrate_kbps: 8000,
            encoder_preset: "veryfast".into(),
            video_codec: VideoCodecChoice::Libx264,
            container: ContainerFormat::Mp4,
            fps: 0,
            max_height: 0,
            audio_bitrate_kbps: 192,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AfterExportSettings {
    pub action: AfterExportAction,
    /// Destination folder for the `Move` action.
    pub move_folder: String,
    /// Prefix/suffix added to the source file name for the `Rename` action.
    pub rename_prefix: String,
    pub rename_suffix: String,
}

impl Default for AfterExportSettings {
    fn default() -> Self {
        Self {
            action: AfterExportAction::Nothing,
            move_folder: String::new(),
            rename_prefix: String::new(),
            rename_suffix: String::new(),
        }
    }
}

/// Container extensions qlipq treats as editable video by default.
pub const DEFAULT_VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mkv", "mov", "flv", "webm", "ts"];

/// Persisted application configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub watched_folders: Vec<String>,
    pub output_folder: String,
    /// Lower-case extensions (no dot) considered video files.
    pub video_extensions: Vec<String>,
    pub naming_template: String,
    pub ffmpeg_path: String,
    pub ffprobe_path: String,
    pub after_export: AfterExportSettings,
    pub output: OutputSettings,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            watched_folders: Vec::new(),
            output_folder: String::new(),
            video_extensions: DEFAULT_VIDEO_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
            naming_template: "{date}_{source}_{name}".into(),
            ffmpeg_path: "ffmpeg".into(),
            ffprobe_path: "ffprobe".into(),
            after_export: AfterExportSettings::default(),
            output: OutputSettings::default(),
        }
    }
}

/// True if the file extension (case-insensitive) is one of the configured video types.
pub fn is_video_file(file_name: &str, video_extensions: &[String]) -> bool {
    match file_name.rfind('.') {
        None => false,
        Some(dot) => {
            let ext = file_name[dot + 1..].to_lowercase();
            video_extensions.iter().any(|x| *x == ext)
        }
    }
}
