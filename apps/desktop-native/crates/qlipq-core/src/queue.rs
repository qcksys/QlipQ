use serde::{Deserialize, Serialize};

use crate::config::{ContainerFormat, QualityMode, QualityPreset, VideoCodecChoice};
use crate::edit_spec::EditSpec;
use crate::media::MediaInfo;

/// Lifecycle of a clip in the editing queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueueStatus {
    Pending,
    Ready,
    Editing,
    Exporting,
    Done,
    Error,
}

/// Per-clip output overrides, merged over the global [`OutputSettings`](crate::config::OutputSettings)
/// on export. Every field is optional so only set values persist/apply.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_mode: Option<QualityMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_preset: Option<QualityPreset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crf: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_bitrate_kbps: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder_preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<VideoCodecChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<ContainerFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_bitrate_kbps: Option<i64>,
}

/// A recording tracked in the queue, with any parsed metadata and edit state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub id: String,
    pub path: String,
    pub file_name: String,
    /// ISO timestamp of when it entered the queue.
    pub added_at: String,
    pub status: QueueStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size_bytes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_modified_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_sec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<EditSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_override: Option<OutputOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
