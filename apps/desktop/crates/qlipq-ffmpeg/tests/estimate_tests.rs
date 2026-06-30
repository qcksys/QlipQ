use qlipq_core::edit_spec::{AudioTrackSpec, EditSpec, TrimSpec};
use qlipq_core::media::MediaInfo;
use qlipq_ffmpeg::args::{AudioEncodeOptions, ResolvedEncode, VideoEncodeOptions};
use qlipq_ffmpeg::estimate::*;

fn media() -> MediaInfo {
    MediaInfo {
        duration_sec: 100.0,
        width: 1920,
        height: 1080,
        video_codec: "h264".into(),
        fps: 60.0,
        audio_streams: vec![],
        size_bytes: Some(1_000_000_000),
    }
}

fn one_audio() -> EditSpec {
    EditSpec { trim: None, crop: None, audio_tracks: vec![AudioTrackSpec { index: 0, enabled: true, volume: 1.0 }] }
}

fn copy() -> ResolvedEncode {
    ResolvedEncode {
        video: VideoEncodeOptions::default(),
        audio: AudioEncodeOptions { codec: None, bitrate: Some("192k".into()) },
        reencode: false,
    }
}

fn crf(configure: impl FnOnce(&mut VideoEncodeOptions)) -> ResolvedEncode {
    let mut video = VideoEncodeOptions { codec: Some("libx264".into()), crf: Some(23), ..Default::default() };
    configure(&mut video);
    ResolvedEncode { video, audio: AudioEncodeOptions { codec: None, bitrate: Some("192k".into()) }, reencode: true }
}

#[test]
fn stream_copy_scales_by_kept_duration() {
    let full = estimate_export_size(&media(), &one_audio(), &copy());
    assert!((full.bytes - 1_000_000_000.0).abs() < 500.0);
    assert!(!full.approximate);

    let half = estimate_export_size(
        &media(),
        &EditSpec { trim: Some(TrimSpec { start_sec: 0.0, end_sec: 50.0 }), crop: None, audio_tracks: one_audio().audio_tracks },
        &copy(),
    );
    assert!((half.bytes - 500_000_000.0).abs() < 500.0);
}

#[test]
fn bitrate_mode_is_exact() {
    let enc = ResolvedEncode {
        video: VideoEncodeOptions { bitrate_kbps: Some(8000), ..Default::default() },
        audio: AudioEncodeOptions { codec: None, bitrate: Some("0k".into()) },
        reencode: true,
    };
    let r = estimate_export_size(&media(), &EditSpec { trim: None, crop: None, audio_tracks: vec![] }, &enc);
    assert!((r.bytes - 8000.0 * 1000.0 * 100.0 / 8.0).abs() < 500.0);
    assert!(!r.approximate);
}

#[test]
fn crf_estimate_is_approximate_and_monotonic() {
    let better = estimate_export_size(&media(), &one_audio(), &crf(|v| v.crf = Some(18)));
    let worse = estimate_export_size(&media(), &one_audio(), &crf(|v| v.crf = Some(28)));
    assert!(better.approximate);
    assert!(better.bytes > worse.bytes);

    let downscaled = estimate_export_size(&media(), &one_audio(), &crf(|v| { v.crf = Some(23); v.scale_height = Some(540); }));
    let full_res = estimate_export_size(&media(), &one_audio(), &crf(|v| v.crf = Some(23)));
    assert!(downscaled.bytes < full_res.bytes);
}

#[test]
fn h265_smaller_than_h264() {
    let h264 = estimate_export_size(&media(), &one_audio(), &crf(|v| v.codec = Some("libx264".into())));
    let h265 = estimate_export_size(&media(), &one_audio(), &crf(|v| v.codec = Some("libx265".into())));
    assert!(h265.bytes < h264.bytes);
}

#[test]
fn zero_length_estimates_zero() {
    let r = estimate_export_size(
        &media(),
        &EditSpec { trim: Some(TrimSpec { start_sec: 10.0, end_sec: 10.0 }), crop: None, audio_tracks: one_audio().audio_tracks },
        &copy(),
    );
    assert_eq!(r.bytes, 0.0);
}
