use std::collections::HashMap;

use qlipq_core::media::AudioStreamInfo;
use qlipq_ffmpeg::probe::*;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn build_probe_args_requests_json() {
    assert_eq!(
        build_probe_args("clip.mkv"),
        vec!["-v", "error", "-print_format", "json", "-show_format", "-show_streams", "clip.mkv"]
    );
}

#[test]
fn parse_frame_rate_handles_rationals_and_integers() {
    assert!(close(parse_frame_rate(Some("30000/1001")), 29.97));
    assert!(close(parse_frame_rate(Some("60/1")), 60.0));
    assert!(close(parse_frame_rate(Some("0/0")), 0.0));
    assert!(close(parse_frame_rate(None), 0.0));
}

#[test]
fn parse_ffprobe_extracts_video_and_audio_indices() {
    let mut t1 = HashMap::new();
    t1.insert("title".to_string(), "Desktop".to_string());
    let mut t2 = HashMap::new();
    t2.insert("language".to_string(), "eng".to_string());
    t2.insert("title".to_string(), "Mic".to_string());

    let data = FfprobeOutput {
        streams: Some(vec![
            FfprobeStream { index: 0, codec_type: Some("video".into()), codec_name: Some("h264".into()), width: Some(2560), height: Some(1440), r_frame_rate: Some("60/1".into()), ..Default::default() },
            FfprobeStream { index: 1, codec_type: Some("audio".into()), codec_name: Some("aac".into()), channels: Some(2), tags: Some(t1), ..Default::default() },
            FfprobeStream { index: 2, codec_type: Some("audio".into()), codec_name: Some("aac".into()), channels: Some(1), tags: Some(t2), ..Default::default() },
        ]),
        format: Some(FfprobeFormat { duration: Some("63.500000".into()), size: Some("104857600".into()) }),
    };
    let info = parse_ffprobe_output(&data);
    assert!(close(info.duration_sec, 63.5));
    assert_eq!(info.width, 2560);
    assert_eq!(info.height, 1440);
    assert_eq!(info.video_codec, "h264");
    assert!(close(info.fps, 60.0));
    assert_eq!(info.size_bytes, Some(104857600));
    assert_eq!(
        info.audio_streams,
        vec![
            AudioStreamInfo { stream_index: 1, index: 0, codec: "aac".into(), channels: 2, language: None, title: Some("Desktop".into()) },
            AudioStreamInfo { stream_index: 2, index: 1, codec: "aac".into(), channels: 1, language: Some("eng".into()), title: Some("Mic".into()) },
        ]
    );
}

#[test]
fn parse_ffprobe_accepts_json_string() {
    let info = parse_ffprobe(r#"{"streams":[],"format":{"duration":"1.0"}}"#);
    assert!(close(info.duration_sec, 1.0));
    assert!(info.audio_streams.is_empty());
}
