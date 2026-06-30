use qlipq_core::config::*;
use qlipq_core::config_json;
use qlipq_core::datetimes::format_duration;
use qlipq_core::media::format_bytes;

#[test]
fn parse_fills_in_missing_fields() {
    let merged = config_json::parse(r#"{"outputFolder":"D:/out"}"#);
    assert_eq!(merged.output_folder, "D:/out");
    assert_eq!(merged.ffmpeg_path, "ffmpeg");
    assert_eq!(merged.naming_template, AppConfig::default().naming_template);
}

#[test]
fn parse_tolerates_empty_and_invalid_json() {
    assert_eq!(config_json::parse("").ffmpeg_path, "ffmpeg");
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
fn parse_repairs_invalid_enums_and_clamps_crf() {
    let merged = config_json::parse(r#"{"output":{"qualityPreset":"ultra","crf":99,"videoCodec":"av1"}}"#);
    assert_eq!(merged.output.quality_preset, QualityPreset::Original);
    assert_eq!(merged.output.crf, 51);
    assert_eq!(merged.output.video_codec, VideoCodecChoice::Libx264);
}

#[test]
fn serialize_stamps_schema_and_round_trips() {
    let mut cfg = AppConfig::default();
    cfg.output_folder = "D:/out".into();
    let json = config_json::serialize(&cfg);
    assert!(json.contains("\"$schema\": \"https://qlipq.com/schema/config.json\""));
    assert!(json.contains("\"qualityPreset\": \"original\""));
    assert!(json.contains("\"videoCodec\": \"libx264\""));

    let round = config_json::parse(&json);
    assert_eq!(round.output_folder, "D:/out");
    assert_eq!(round.output.quality_preset, QualityPreset::Original);
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
