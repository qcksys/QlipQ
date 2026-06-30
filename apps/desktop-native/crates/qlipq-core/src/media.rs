use serde::{Deserialize, Serialize};

/// Probed information about a single audio stream within a media file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStreamInfo {
    /// Absolute stream index in the container (as ffmpeg's `0:N`).
    pub stream_index: i64,
    /// Audio-relative index used by ffmpeg's `0:a:N` selector.
    pub index: i64,
    pub codec: String,
    pub channels: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Probed information about a media file, derived from ffprobe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub duration_sec: f64,
    pub width: i64,
    pub height: i64,
    pub video_codec: String,
    pub fps: f64,
    pub audio_streams: Vec<AudioStreamInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
}

/// A best-effort, human-friendly label for an audio stream.
pub fn audio_stream_label(stream: &AudioStreamInfo) -> String {
    if let Some(title) = &stream.title {
        if !title.is_empty() {
            return title.clone();
        }
    }
    if let Some(lang) = &stream.language {
        if !lang.is_empty() {
            return format!("Track {} ({})", stream.index + 1, lang);
        }
    }
    format!("Track {}", stream.index + 1)
}

/// Human-friendly file size, e.g. `1.4 GB`, using binary units (1024).
pub fn format_bytes(bytes: f64) -> String {
    if !bytes.is_finite() || bytes <= 0.0 {
        return "0 B".to_string();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let exp = ((bytes.ln() / 1024_f64.ln()).floor() as i64).clamp(0, UNITS.len() as i64 - 1) as usize;
    let value = bytes / 1024_f64.powi(exp as i32);
    if exp == 0 {
        format!("{:.0} {}", value, UNITS[exp])
    } else {
        format!("{:.1} {}", value, UNITS[exp])
    }
}
