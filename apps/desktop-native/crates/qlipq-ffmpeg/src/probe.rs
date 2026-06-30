use std::collections::HashMap;

use serde::Deserialize;

use qlipq_core::media::{AudioStreamInfo, MediaInfo};

/// A single stream entry as produced by `ffprobe -show_streams -print_format json`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FfprobeStream {
    #[serde(default)]
    pub index: i64,
    pub codec_type: Option<String>,
    pub codec_name: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub channels: Option<i64>,
    pub r_frame_rate: Option<String>,
    pub avg_frame_rate: Option<String>,
    pub tags: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FfprobeFormat {
    pub duration: Option<String>,
    pub size: Option<String>,
}

/// The relevant subset of ffprobe's JSON output.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FfprobeOutput {
    pub streams: Option<Vec<FfprobeStream>>,
    pub format: Option<FfprobeFormat>,
}

/// Build the ffprobe argument list that produces parseable JSON for a file.
pub fn build_probe_args(input_path: &str) -> Vec<String> {
    vec![
        "-v".into(),
        "error".into(),
        "-print_format".into(),
        "json".into(),
        "-show_format".into(),
        "-show_streams".into(),
        input_path.to_string(),
    ]
}

/// Convert an ffmpeg rational frame rate like `30000/1001` into fps.
pub fn parse_frame_rate(rate: Option<&str>) -> f64 {
    let rate = match rate {
        Some(r) if !r.is_empty() => r,
        _ => return 0.0,
    };
    let parts: Vec<&str> = rate.split('/').collect();
    let num = parts.first().map(|s| js_number(s)).unwrap_or(f64::NAN);
    let den = parts.get(1).map(|s| js_number(s)).unwrap_or(f64::NAN);
    if den == 0.0 || den.is_nan() || num.is_nan() {
        return if num.is_finite() { num } else { 0.0 };
    }
    (num / den * 1000.0).round() / 1000.0
}

/// Parse ffprobe JSON text into a [`MediaInfo`].
pub fn parse_ffprobe(json: &str) -> MediaInfo {
    let data: FfprobeOutput = serde_json::from_str(json).unwrap_or_default();
    parse_ffprobe_output(&data)
}

/// Parse already-deserialized ffprobe output into a [`MediaInfo`].
pub fn parse_ffprobe_output(data: &FfprobeOutput) -> MediaInfo {
    let empty: Vec<FfprobeStream> = Vec::new();
    let streams = data.streams.as_ref().unwrap_or(&empty);
    let video = streams.iter().find(|s| s.codec_type.as_deref() == Some("video"));

    let audio_streams: Vec<AudioStreamInfo> = streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("audio"))
        .enumerate()
        .map(|(i, s)| AudioStreamInfo {
            stream_index: s.index,
            index: i as i64,
            codec: s.codec_name.clone().unwrap_or_else(|| "unknown".to_string()),
            channels: s.channels.unwrap_or(0),
            language: s.tags.as_ref().and_then(|t| t.get("language").cloned()),
            title: s.tags.as_ref().and_then(|t| t.get("title").cloned()),
        })
        .collect();

    MediaInfo {
        duration_sec: js_parse_float_or_zero(data.format.as_ref().and_then(|f| f.duration.as_deref())),
        width: video.and_then(|v| v.width).unwrap_or(0),
        height: video.and_then(|v| v.height).unwrap_or(0),
        video_codec: video.and_then(|v| v.codec_name.clone()).unwrap_or_else(|| "unknown".to_string()),
        fps: parse_frame_rate(
            video
                .and_then(|v| v.r_frame_rate.as_deref())
                .or_else(|| video.and_then(|v| v.avg_frame_rate.as_deref())),
        ),
        audio_streams,
        size_bytes: data
            .format
            .as_ref()
            .and_then(|f| f.size.as_deref())
            .filter(|s| !s.is_empty())
            .and_then(js_parse_int),
    }
}

/// Mimics JS `Number(s)` for a single field: blank → 0, non-numeric → NaN.
fn js_number(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        0.0
    } else {
        t.parse::<f64>().unwrap_or(f64::NAN)
    }
}

/// Mimics JS `Number.parseFloat(x ?? "0") || 0`.
fn js_parse_float_or_zero(s: Option<&str>) -> f64 {
    match s {
        Some(s) if !s.is_empty() => s.trim().parse::<f64>().ok().filter(|v| *v != 0.0 && v.is_finite()).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Mimics JS `Number.parseInt(x, 10)` for a clean integer field.
fn js_parse_int(s: &str) -> Option<i64> {
    s.trim().parse::<i64>().ok()
}
