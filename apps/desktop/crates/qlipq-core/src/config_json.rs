//! Lenient load / pretty save of `config.json`, mirroring the Rust Tauri host and the C#
//! `ConfigJson`: missing fields keep defaults, present-but-bad fields revert to their default,
//! enum/range values are validated and repaired, and a `$schema` reference is stamped on write.

use serde_json::{Map, Value};

use crate::config::*;

pub const SCHEMA_URL: &str = "https://qlipq.com/schema/config.json";

/// Parse config JSON onto defaults, tolerating missing/invalid fields.
pub fn parse(text: &str) -> AppConfig {
    let mut cfg = AppConfig::default();
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(text) else {
        return cfg;
    };

    if let Some(v) = read_string_list(&obj, "watchedFolders") {
        cfg.watched_folders = v;
    }
    if let Some(v) = read_string(&obj, "outputFolder") {
        cfg.output_folder = v;
    }
    if let Some(v) = read_string_list(&obj, "videoExtensions") {
        cfg.video_extensions = v;
    }
    if let Some(v) = read_string(&obj, "namingTemplate") {
        cfg.naming_template = v;
    }
    if let Some(v) = read_string(&obj, "ffmpegPath") {
        cfg.ffmpeg_path = v;
    }
    if let Some(v) = read_string(&obj, "ffprobePath") {
        cfg.ffprobe_path = v;
    }

    if let Some(Value::Object(ae)) = obj.get("afterExport") {
        let a = &mut cfg.after_export;
        a.action = parse_after_action(read_str(ae, "action"), a.action);
        if let Some(v) = read_string(ae, "moveFolder") {
            a.move_folder = v;
        }
        if let Some(v) = read_string(ae, "renamePrefix") {
            a.rename_prefix = v;
        }
        if let Some(v) = read_string(ae, "renameSuffix") {
            a.rename_suffix = v;
        }
    }

    if let Some(Value::Object(o)) = obj.get("output") {
        let s = &mut cfg.output;
        s.quality_mode = parse_quality_mode(read_str(o, "qualityMode"), s.quality_mode);
        s.quality_preset = parse_quality_preset(read_str(o, "qualityPreset"), s.quality_preset);
        if let Some(c) = read_int(o, "crf") {
            // garde range(0,51): a negative reverts to default; >51 clamps.
            s.crf = if c < 0 { 20 } else { c.min(51) };
        }
        if let Some(v) = read_int(o, "videoBitrateKbps") {
            s.video_bitrate_kbps = v;
        }
        if let Some(v) = read_string(o, "encoderPreset") {
            s.encoder_preset = v;
        }
        s.video_codec = parse_video_codec(read_str(o, "videoCodec"), s.video_codec);
        s.container = parse_container(read_str(o, "container"), s.container);
        if let Some(v) = read_int(o, "fps") {
            s.fps = v;
        }
        if let Some(v) = read_int(o, "maxHeight") {
            s.max_height = v;
        }
        if let Some(v) = read_int(o, "audioBitrateKbps") {
            s.audio_bitrate_kbps = v;
        }
    }

    cfg
}

/// Serialize to pretty JSON with a leading `$schema` reference (keys are sorted, so `$` sorts first).
pub fn serialize(cfg: &AppConfig) -> String {
    let mut value = serde_json::to_value(cfg).expect("AppConfig serializes");
    if let Value::Object(ref mut map) = value {
        map.insert("$schema".to_string(), Value::String(SCHEMA_URL.to_string()));
    }
    serde_json::to_string_pretty(&value).expect("Value serializes")
}

fn read_str<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(|v| v.as_str())
}

fn read_string(map: &Map<String, Value>, key: &str) -> Option<String> {
    read_str(map, key).map(|s| s.to_string())
}

fn read_int(map: &Map<String, Value>, key: &str) -> Option<i64> {
    map.get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
}

fn read_string_list(map: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    match map.get(key) {
        Some(Value::Array(arr)) => {
            Some(arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        }
        _ => None,
    }
}

fn parse_after_action(s: Option<&str>, default: AfterExportAction) -> AfterExportAction {
    match s {
        Some("nothing") => AfterExportAction::Nothing,
        Some("delete") => AfterExportAction::Delete,
        Some("move") => AfterExportAction::Move,
        Some("rename") => AfterExportAction::Rename,
        Some("prompt") => AfterExportAction::Prompt,
        _ => default,
    }
}

fn parse_quality_mode(s: Option<&str>, default: QualityMode) -> QualityMode {
    match s {
        Some("preset") => QualityMode::Preset,
        Some("crf") => QualityMode::Crf,
        Some("bitrate") => QualityMode::Bitrate,
        Some("vbr") => QualityMode::Vbr,
        _ => default,
    }
}

fn parse_quality_preset(s: Option<&str>, default: QualityPreset) -> QualityPreset {
    match s {
        Some("original") => QualityPreset::Original,
        Some("high") => QualityPreset::High,
        Some("balanced") => QualityPreset::Balanced,
        Some("small") => QualityPreset::Small,
        _ => default,
    }
}

fn parse_video_codec(s: Option<&str>, default: VideoCodecChoice) -> VideoCodecChoice {
    match s {
        Some("libx264") => VideoCodecChoice::Libx264,
        Some("libx265") => VideoCodecChoice::Libx265,
        _ => default,
    }
}

fn parse_container(s: Option<&str>, default: ContainerFormat) -> ContainerFormat {
    match s {
        Some("mp4") => ContainerFormat::Mp4,
        Some("mkv") => ContainerFormat::Mkv,
        _ => default,
    }
}
