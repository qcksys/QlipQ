use qlipq_core::config::{OutputSettings, QualityMode, QualityPreset};
use qlipq_core::edit_spec::{AudioTrackSpec, CropSpec, EditSpec, TrimSpec};
use qlipq_core::media::MediaInfo;
use qlipq_ffmpeg::args::*;

fn track(index: i64, enabled: bool, volume: f64) -> AudioTrackSpec {
    AudioTrackSpec { index, enabled, volume }
}

fn args(spec: EditSpec, configure: impl FnOnce(&mut BuildExportOptions)) -> Vec<String> {
    let mut opts = BuildExportOptions {
        input_path: "in.mkv".into(),
        output_path: "out.mp4".into(),
        spec,
        reencode: false,
        progress: false,
        video: None,
        audio: None,
        metadata: vec![],
    };
    configure(&mut opts);
    build_export_args(&opts)
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn trim_only_defaults_to_fast_stream_copy() {
    let out = args(
        EditSpec { trim: Some(TrimSpec { start_sec: 5.0, end_sec: 12.5 }), crop: None, audio_tracks: vec![track(0, true, 1.0)] },
        |_| {},
    );
    assert_eq!(
        out,
        strs(&["-y", "-ss", "5.000", "-i", "in.mkv", "-t", "7.500", "-map", "0:v:0", "-map", "0:a:0", "-c:v", "copy", "-c:a", "copy", "out.mp4"])
    );
}

#[test]
fn forced_reencode_reencodes_video_copies_audio() {
    let out = args(
        EditSpec { trim: Some(TrimSpec { start_sec: 0.0, end_sec: 10.0 }), crop: None, audio_tracks: vec![track(0, true, 1.0)] },
        |o| o.reencode = true,
    );
    assert!(out.iter().any(|s| s == "-c:v"));
    assert!(out.iter().any(|s| s == "libx264"));
    assert!(out.join(" ").contains("-c:a copy"));
    assert!(!out.iter().any(|s| s == "-filter_complex"));
}

#[test]
fn crop_builds_filter_and_reencodes() {
    let out = args(
        EditSpec { trim: None, crop: Some(CropSpec { x: 100, y: 50, width: 1280, height: 720 }), audio_tracks: vec![track(0, true, 1.0)] },
        |_| {},
    );
    let j = out.join(" ");
    assert!(j.contains("-filter_complex [0:v:0]crop=1280:720:100:50[vout]"));
    assert!(j.contains("-map [vout]"));
    assert!(j.contains("-map 0:a:0"));
    assert!(j.contains("-c:v libx264"));
    assert!(j.contains("-c:a copy"));
}

#[test]
fn audio_volume_reencodes_audio_copies_video() {
    let out = args(
        EditSpec { trim: None, crop: None, audio_tracks: vec![track(0, true, 0.5), track(1, true, 1.0)] },
        |_| {},
    );
    let j = out.join(" ");
    assert!(j.contains("-filter_complex [0:a:0]volume=0.5[aout0]"));
    assert!(j.contains("-map 0:v:0"));
    assert!(j.contains("-map [aout0]"));
    assert!(j.contains("-map 0:a:1"));
    assert!(j.contains("-c:v copy"));
    assert!(j.contains("-c:a aac -b:a 192k"));
}

#[test]
fn crop_plus_volume_combines_filters() {
    let out = args(
        EditSpec { trim: None, crop: Some(CropSpec { x: 0, y: 0, width: 640, height: 480 }), audio_tracks: vec![track(0, true, 2.0)] },
        |_| {},
    );
    let j = out.join(" ");
    assert!(j.contains("[0:v:0]crop=640:480:0:0[vout];[0:a:0]volume=2[aout0]"));
    assert!(j.contains("-c:v libx264"));
    assert!(j.contains("-c:a aac"));
}

#[test]
fn disabling_all_audio_yields_an() {
    let out = args(
        EditSpec { trim: None, crop: None, audio_tracks: vec![track(0, false, 1.0), track(1, false, 1.0)] },
        |_| {},
    );
    assert!(out.iter().any(|s| s == "-an"));
    assert!(!out.iter().any(|s| s == "-c:a"));
}

#[test]
fn progress_flag_appends_progress() {
    let out = args(EditSpec { trim: None, crop: None, audio_tracks: vec![track(0, true, 1.0)] }, |o| o.progress = true);
    assert!(out.join(" ").contains("-progress pipe:1 -nostats"));
}

#[test]
fn custom_encoder_options() {
    let out = args(
        EditSpec { trim: None, crop: Some(CropSpec { x: 0, y: 0, width: 100, height: 100 }), audio_tracks: vec![track(0, true, 0.0)] },
        |o| {
            o.video = Some(VideoEncodeOptions { codec: Some("libx265".into()), crf: Some(28), preset: Some("fast".into()), ..Default::default() });
            o.audio = Some(AudioEncodeOptions { codec: Some("libopus".into()), bitrate: Some("96k".into()) });
        },
    );
    let j = out.join(" ");
    assert!(j.contains("-c:v libx265 -preset fast -crf 28"));
    assert!(j.contains("-c:a libopus -b:a 96k"));
}

#[test]
fn metadata_stamps_before_output() {
    let out = args(EditSpec { trim: None, crop: None, audio_tracks: vec![track(0, true, 1.0)] }, |o| {
        o.metadata = vec![("game".into(), "Deadlock".into())];
    });
    assert!(out.join(" ").contains("-metadata game=Deadlock"));
    let md = out.iter().position(|s| s == "-metadata").unwrap();
    let outp = out.iter().position(|s| s == "out.mp4").unwrap();
    assert!(md < outp);
}

#[test]
fn frame_rate_change_emits_r_without_filter() {
    let out = args(EditSpec { trim: None, crop: None, audio_tracks: vec![track(0, true, 1.0)] }, |o| {
        o.video = Some(VideoEncodeOptions { fps: Some(30), ..Default::default() });
    });
    let j = out.join(" ");
    assert!(j.contains("-c:v libx264"));
    assert!(j.contains("-r 30"));
    assert!(!out.iter().any(|s| s == "-filter_complex"));
    assert!(j.contains("-c:a copy"));
}

#[test]
fn downscale_builds_scale_filter() {
    let out = args(EditSpec { trim: None, crop: None, audio_tracks: vec![track(0, true, 1.0)] }, |o| {
        o.video = Some(VideoEncodeOptions { scale_height: Some(720), ..Default::default() });
    });
    let j = out.join(" ");
    assert!(j.contains("-filter_complex [0:v:0]scale=-2:720[vout]"));
    assert!(j.contains("-map [vout]"));
    assert!(j.contains("-c:v libx264"));
}

#[test]
fn crop_and_downscale_compose() {
    let out = args(
        EditSpec { trim: None, crop: Some(CropSpec { x: 0, y: 0, width: 1920, height: 1080 }), audio_tracks: vec![] },
        |o| o.video = Some(VideoEncodeOptions { scale_height: Some(720), ..Default::default() }),
    );
    assert!(out.join(" ").contains("[0:v:0]crop=1920:1080:0:0,scale=-2:720[vout]"));
}

#[test]
fn bitrate_uses_bv_not_crf() {
    let out = args(EditSpec { trim: None, crop: None, audio_tracks: vec![track(0, true, 1.0)] }, |o| {
        o.reencode = true;
        o.video = Some(VideoEncodeOptions { bitrate_kbps: Some(6000), ..Default::default() });
    });
    let j = out.join(" ");
    assert!(j.contains("-b:v 6000k"));
    assert!(!j.contains("-crf"));
}

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

    let out = args(EditSpec { trim: None, crop: None, audio_tracks: vec![track(0, true, 1.0)] }, |o| {
        o.reencode = true;
        o.video = Some(r.video.clone());
    });
    let j = out.join(" ");
    assert!(j.contains("-crf 22"));
    assert!(j.contains("-maxrate 9000k"));
    assert!(j.contains("-bufsize 18000k"));
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
