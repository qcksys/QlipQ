use qlipq_core::edit_spec::*;
use qlipq_core::media::{AudioStreamInfo, MediaInfo};

fn media() -> MediaInfo {
    MediaInfo {
        duration_sec: 120.0,
        width: 1920,
        height: 1080,
        video_codec: "h264".into(),
        fps: 60.0,
        audio_streams: vec![
            AudioStreamInfo { stream_index: 1, index: 0, codec: "aac".into(), channels: 2, language: None, title: Some("Desktop".into()) },
            AudioStreamInfo { stream_index: 2, index: 1, codec: "aac".into(), channels: 1, language: None, title: Some("Mic".into()) },
        ],
        size_bytes: None,
    }
}

#[test]
fn default_edit_spec_enables_every_track_at_unity() {
    let spec = default_edit_spec(Some(&media()));
    assert_eq!(
        spec.audio_tracks,
        vec![
            AudioTrackSpec { index: 0, enabled: true, volume: 1.0 },
            AudioTrackSpec { index: 1, enabled: true, volume: 1.0 },
        ]
    );
    assert!(spec.trim.is_none());
}

#[test]
fn effective_duration_reflects_trim() {
    let m = media();
    assert_eq!(effective_duration(&EditSpec { trim: None, crop: None, audio_tracks: vec![] }, &m), 120.0);
    assert_eq!(
        effective_duration(
            &EditSpec { trim: Some(TrimSpec { start_sec: 10.0, end_sec: 40.0 }), crop: None, audio_tracks: vec![] },
            &m
        ),
        30.0
    );
}

#[test]
fn validate_accepts_sane_spec() {
    let mut spec = default_edit_spec(Some(&media()));
    spec.trim = Some(TrimSpec { start_sec: 5.0, end_sec: 50.0 });
    spec.crop = Some(CropSpec { x: 0, y: 0, width: 1280, height: 720 });
    assert_eq!(validate_edit_spec(&spec, &media()), None);
}

#[test]
fn validate_rejects_inverted_trim() {
    let spec = EditSpec { trim: Some(TrimSpec { start_sec: 30.0, end_sec: 10.0 }), crop: None, audio_tracks: vec![] };
    assert!(validate_edit_spec(&spec, &media()).unwrap().contains("after the start"));
}

#[test]
fn validate_rejects_crop_outside_frame() {
    let spec = EditSpec { trim: None, crop: Some(CropSpec { x: 1000, y: 0, width: 1280, height: 720 }), audio_tracks: vec![] };
    assert!(validate_edit_spec(&spec, &media()).unwrap().contains("outside the frame"));
}

#[test]
fn validate_rejects_negative_volume() {
    let spec = EditSpec { trim: None, crop: None, audio_tracks: vec![AudioTrackSpec { index: 0, enabled: true, volume: -1.0 }] };
    assert!(validate_edit_spec(&spec, &media()).unwrap().contains("volume cannot be negative"));
}
