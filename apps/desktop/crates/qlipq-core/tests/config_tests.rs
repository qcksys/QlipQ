use qlipq_core::config::*;
use qlipq_core::config_json;
use qlipq_core::datetimes::format_duration;
use qlipq_core::media::format_bytes;

#[test]
fn parse_fills_in_missing_fields() {
    let merged = config_json::parse(r#"{"outputFolder":"D:/out"}"#);
    assert_eq!(merged.output_folder, "D:/out");
    assert_eq!(merged.naming_template, AppConfig::default().naming_template);
}

#[test]
fn autoplay_defaults_on_and_debug_off() {
    let cfg = AppConfig::default();
    assert!(cfg.autoplay);
    assert!(!cfg.debug);
    assert_eq!(cfg.preview_max_height, 1080);
    // Missing keys keep the defaults; an explicit value is honored.
    assert!(config_json::parse("{}").autoplay);
    assert!(!config_json::parse(r#"{"autoplay":false}"#).autoplay);
    assert!(config_json::parse(r#"{"debug":true}"#).debug);
}

#[test]
fn parse_tolerates_empty_and_invalid_json() {
    assert_eq!(config_json::parse("").naming_template, "{date}_{source}_{name}");
    assert_eq!(config_json::parse("not json").naming_template, "{date}_{source}_{name}");
}

#[test]
fn parse_deep_merges_output_keeping_defaults() {
    let merged = config_json::parse(r#"{"output":{"qualityMode":"bitrate"}}"#);
    assert_eq!(merged.output.quality_mode, QualityMode::Bitrate);
    assert_eq!(merged.output.audio_bitrate_kbps, OutputSettings::default().audio_bitrate_kbps);
    assert_eq!(merged.output.container, ContainerFormat::Mp4);
}

#[test]
fn parse_accepts_numbers_as_is() {
    // serde doesn't range-check (the derived schema documents 0–51; the editor + UI enforce it).
    let merged = config_json::parse(r#"{"output":{"crf":99}}"#);
    assert_eq!(merged.output.crf, 99);
}

#[test]
fn parse_reverts_to_default_on_invalid_value() {
    // Unlike the old hand-written parser, serde is all-or-nothing per document: an invalid enum value
    // makes the whole config revert to defaults rather than repairing that one field.
    let merged = config_json::parse(r#"{"outputFolder":"D:/out","output":{"qualityPreset":"ultra"}}"#);
    assert_eq!(merged, AppConfig::default());
}

#[test]
fn serialize_stamps_schema_and_round_trips() {
    let mut cfg = AppConfig::default();
    cfg.output_folder = "D:/out".into();
    let json = config_json::serialize(&cfg);
    assert!(json.contains("\"$schema\": \"./config.schema.json\""));
    assert!(json.contains("\"qualityPreset\": \"original\""));
    assert!(json.contains("\"videoCodec\": \"libx264\""));

    let round = config_json::parse(&json);
    assert_eq!(round.output_folder, "D:/out");
    assert_eq!(round.output.quality_preset, QualityPreset::Original);
}

#[test]
fn schema_json_describes_config() {
    let s = config_json::schema_json();
    assert!(s.contains("\"$schema\""));
    assert!(s.contains("\"watchedFolders\""));
    assert!(s.contains("\"qualityPreset\""));
    assert!(s.contains("\"afterExport\""));
}

#[test]
fn format_bytes_renders_binary_units() {
    assert_eq!(format_bytes(0.0), "0 B");
    assert_eq!(format_bytes(512.0), "512 B");
    assert_eq!(format_bytes(1024.0), "1.0 KB");
    assert_eq!(format_bytes(1024.0 * 1024.0 * 1.5), "1.5 MB");
    assert_eq!(format_bytes(3.2 * 1024_f64.powi(3)), "3.2 GB");
}

#[test]
fn is_video_file_matches_case_insensitively() {
    let ext = AppConfig::default().video_extensions;
    assert!(is_video_file("clip.MKV", &ext));
    assert!(is_video_file("clip.mp4", &ext));
    assert!(!is_video_file("notes.txt", &ext));
    assert!(!is_video_file("noext", &ext));
}

#[test]
fn format_duration_renders_minutes_and_hours() {
    assert_eq!(format_duration(65.0), "1:05");
    assert_eq!(format_duration(3725.0), "1:02:05");
    assert_eq!(format_duration(-5.0), "0:00");
}
