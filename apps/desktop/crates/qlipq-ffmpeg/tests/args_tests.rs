use qlipq_core::config::{OutputSettings, QualityMode, QualityPreset};
use qlipq_core::media::MediaInfo;
use qlipq_ffmpeg::args::*;

fn media() -> MediaInfo {
    MediaInfo { duration_sec: 60.0, width: 2560, height: 1440, video_codec: "h264".into(), fps: 60.0, audio_streams: vec![], size_bytes: None }
}

fn settings(configure: impl FnOnce(&mut OutputSettings)) -> OutputSettings {
    let mut s = OutputSettings::default();
    configure(&mut s);
    s
}

#[test]
fn encode_original_is_stream_copy() {
    let r = output_settings_to_encode(&settings(|s| s.quality_preset = QualityPreset::Original), &media());
    assert!(!r.reencode);
}

#[test]
fn encode_named_presets_map_to_crf() {
    assert_eq!(output_settings_to_encode(&settings(|s| s.quality_preset = QualityPreset::High), &media()).video.crf, Some(18));
    assert_eq!(output_settings_to_encode(&settings(|s| s.quality_preset = QualityPreset::Balanced), &media()).video.crf, Some(23));
    let small = output_settings_to_encode(&settings(|s| s.quality_preset = QualityPreset::Small), &media());
    assert_eq!(small.video.crf, Some(28));
    assert!(small.reencode);
}

#[test]
fn encode_bitrate_mode() {
    let r = output_settings_to_encode(&settings(|s| { s.quality_mode = QualityMode::Bitrate; s.video_bitrate_kbps = 5000; }), &media());
    assert_eq!(r.video.bitrate_kbps, Some(5000));
    assert!(r.reencode);
}

#[test]
fn encode_vbr_maps_to_crf_plus_maxrate() {
    let r = output_settings_to_encode(&settings(|s| { s.quality_mode = QualityMode::Vbr; s.crf = 22; s.video_bitrate_kbps = 9000; }), &media());
    assert_eq!(r.video.crf, Some(22));
    assert_eq!(r.video.maxrate_kbps, Some(9000));
    assert!(r.reencode);
}

#[test]
fn encode_fps_maxheight_clamp_against_source() {
    let up = output_settings_to_encode(&settings(|s| { s.fps = 120; s.max_height = 2160; }), &media());
    assert_eq!(up.video.fps, None);
    assert_eq!(up.video.scale_height, None);
    let down = output_settings_to_encode(&settings(|s| { s.fps = 30; s.max_height = 1080; }), &media());
    assert_eq!(down.video.fps, Some(30));
    assert_eq!(down.video.scale_height, Some(1080));
}
