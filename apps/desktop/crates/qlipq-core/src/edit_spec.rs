use serde::{Deserialize, Serialize};

use crate::media::MediaInfo;

/// A trim window. `end_sec` is exclusive (the cut ends at this timestamp).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrimSpec {
    pub start_sec: f64,
    pub end_sec: f64,
}

/// A pixel-space crop rectangle relative to the source frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CropSpec {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

/// Selection and level for one source audio track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrackSpec {
    /// Audio-relative index matching [`AudioStreamInfo::index`](crate::media::AudioStreamInfo).
    pub index: i64,
    pub enabled: bool,
    /// Linear gain multiplier: 1 = unchanged, 0 = muted, 2 = +6dB.
    pub volume: f64,
}

/// A complete description of the edits to apply to one clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<TrimSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<CropSpec>,
    pub audio_tracks: Vec<AudioTrackSpec>,
}

/// An edit spec that applies no changes, selecting every source audio track at unity gain.
pub fn default_edit_spec(media: Option<&MediaInfo>) -> EditSpec {
    let audio_tracks = media
        .map(|m| {
            m.audio_streams
                .iter()
                .map(|s| AudioTrackSpec { index: s.index, enabled: true, volume: 1.0 })
                .collect()
        })
        .unwrap_or_default();
    EditSpec { trim: None, crop: None, audio_tracks }
}

/// The output duration in seconds after trimming, or the full duration when untrimmed.
pub fn effective_duration(spec: &EditSpec, media: &MediaInfo) -> f64 {
    match &spec.trim {
        None => media.duration_sec,
        Some(t) => (t.end_sec - t.start_sec).max(0.0),
    }
}

/// Returns an error message if the spec is invalid for the given media, otherwise `None`.
pub fn validate_edit_spec(spec: &EditSpec, media: &MediaInfo) -> Option<String> {
    if let Some(t) = &spec.trim {
        if t.start_sec < 0.0 {
            return Some("Trim start cannot be negative.".into());
        }
        if t.end_sec <= t.start_sec {
            return Some("Trim end must be after the start.".into());
        }
        if t.end_sec > media.duration_sec + 0.5 {
            return Some("Trim end is beyond the clip duration.".into());
        }
    }
    if let Some(c) = &spec.crop {
        if c.width <= 0 || c.height <= 0 {
            return Some("Crop width and height must be positive.".into());
        }
        if c.x < 0 || c.y < 0 {
            return Some("Crop position cannot be negative.".into());
        }
        if c.x + c.width > media.width || c.y + c.height > media.height {
            return Some("Crop rectangle extends outside the frame.".into());
        }
    }
    for track in &spec.audio_tracks {
        if track.volume < 0.0 {
            return Some("Audio volume cannot be negative.".into());
        }
    }
    None
}
