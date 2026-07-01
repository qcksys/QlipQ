use serde::{Deserialize, Serialize};

/// How output video quality/bitrate is controlled. `Vbr` = CRF capped by a max bitrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum QualityMode {
    Preset,
    Crf,
    Bitrate,
    Vbr,
}

/// Named quality presets; `Original` stream-copies when possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum QualityPreset {
    Original,
    High,
    Balanced,
    Small,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodecChoice {
    Libx264,
    Libx265,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ContainerFormat {
    Mp4,
    Mkv,
}

/// What to do with the source recording after a successful export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct OutputSettings {
    pub quality_mode: QualityMode,
    pub quality_preset: QualityPreset,
    /// Constant Rate Factor (0–51, lower = better); used when `quality_mode` is `Crf`.
    #[schemars(range(min = 0, max = 51))]
    pub crf: i64,
    /// Target video bitrate in kbps; used when `quality_mode` is `Bitrate`.
    #[schemars(range(min = 0))]
    pub video_bitrate_kbps: i64,
    pub encoder_preset: String,
    pub video_codec: VideoCodecChoice,
    pub container: ContainerFormat,
    /// Target frame rate; 0 keeps the source rate. Never up-rates.
    #[schemars(range(min = 0))]
    pub fps: i64,
    /// Downscale so height ≤ this many pixels; 0 keeps the source size. Never up-scales.
    #[schemars(range(min = 0))]
    pub max_height: i64,
    #[schemars(range(min = 0))]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", default)]
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

/// Editor keyboard shortcuts. Each value is a key combo string like `"Space"`, `"I"`, `"Shift+Left"`,
/// or `"Ctrl+M"` (modifiers `Ctrl`/`Shift`/`Alt`/`Cmd` joined with `+`, then the key). Defaults align
/// to Adobe Premiere Pro where it has an equivalent. Editable in Settings and in `config.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Keybinds {
    pub play_pause: String,
    pub set_in: String,
    pub set_out: String,
    pub frame_back: String,
    pub frame_forward: String,
    pub jump_back: String,
    pub jump_forward: String,
    pub go_to_start: String,
    pub go_to_end: String,
    pub export: String,
}

impl Default for Keybinds {
    fn default() -> Self {
        Self {
            play_pause: "Space".into(),
            set_in: "I".into(),
            set_out: "O".into(),
            frame_back: "Left".into(),
            frame_forward: "Right".into(),
            jump_back: "Shift+Left".into(),
            jump_forward: "Shift+Right".into(),
            go_to_start: "Home".into(),
            go_to_end: "End".into(),
            export: "Ctrl+M".into(),
        }
    }
}

/// Container extensions qlipq treats as editable video by default.
pub const DEFAULT_VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mkv", "mov", "flv", "webm", "ts"];

/// Schema for qlipq's config.json.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", default)]
#[schemars(title = "qlipq configuration")]
pub struct AppConfig {
    pub watched_folders: Vec<String>,
    pub output_folder: String,
    /// Lower-case extensions (no dot) considered video files.
    pub video_extensions: Vec<String>,
    pub naming_template: String,
    /// HDR→SDR **preview** brightness, as an `eq` gamma applied after the tonemap (higher = brighter;
    /// `1.0` = off). Compensates for HDR (esp. Windows desktop) captures that preview too dark.
    /// Preview only — exports are unaffected; SDR clips ignore it.
    #[schemars(range(min = 0.1, max = 10.0))]
    pub hdr_preview_gamma: f64,
    /// Max height (px) the **preview** decodes/tonemaps at; `0` = source resolution. Never upscales.
    /// Higher is sharper but costs more decode/GPU work — lower it if the preview stutters. Preview
    /// only; exports use the source and the separate output resolution setting.
    #[schemars(range(min = 0))]
    pub preview_max_height: i64,
    /// Start playback automatically as soon as a clip is selected. Off opens clips paused.
    pub autoplay: bool,
    /// Show the editor's debug panel: clip details, the active decoder (hardware vs software), and
    /// live preview buffer stats to help diagnose playback stutter.
    pub debug: bool,
    pub after_export: AfterExportSettings,
    pub output: OutputSettings,
    pub keybinds: Keybinds,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            watched_folders: Vec::new(),
            output_folder: String::new(),
            video_extensions: DEFAULT_VIDEO_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
            naming_template: "{date}_{source}_{name}".into(),
            hdr_preview_gamma: 1.8,
            preview_max_height: 1080,
            autoplay: true,
            debug: false,
            after_export: AfterExportSettings::default(),
            output: OutputSettings::default(),
            keybinds: Keybinds::default(),
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
