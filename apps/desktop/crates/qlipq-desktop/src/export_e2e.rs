use super::*;
use qlipq_core::config::{QualityMode, QualityPreset, VideoCodecChoice};
use qlipq_core::edit_spec::TrimSpec;
use qlipq_ffmpeg::estimate::estimate_export_size;
use rsmpeg::avfilter::AVFilterInOut;
use std::path::Path;

/// Requires the linked FFmpeg SDK and a working hardware encoder. No external media or CLI tools.
/// Run: cargo test -p qlipq-desktop export_settings_e2e -- --ignored --nocapture
#[test]
#[ignore = "requires a hardware video encoder"]
fn export_settings_e2e() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/export-e2e")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("source.mkv");
    create_source(&input);
    let (media, is_hdr) = crate::libav::probe(input.to_str().unwrap()).unwrap();
    assert!(!is_hdr);
    assert_eq!((media.width, media.height, media.fps), (960, 540, 60.0));

    let spec = EditSpec {
        trim: Some(TrimSpec {
            start_sec: 1.0,
            end_sec: 5.0,
        }),
        crop: None,
        audio_tracks: vec![],
    };
    let base = OutputSettings {
        quality_mode: QualityMode::Bitrate,
        video_bitrate_kbps: 1500,
        container: ContainerFormat::Mkv,
        ..OutputSettings::default()
    };
    let source_rate = export_case(
        &dir,
        &input,
        "bitrate-source-fps",
        &media,
        &spec,
        &base,
        "h264",
        960,
        540,
        60.0,
    );
    let half_rate = export_case(
        &dir,
        &input,
        "bitrate-30fps",
        &media,
        &spec,
        &OutputSettings {
            fps: 30,
            ..base.clone()
        },
        "h264",
        960,
        540,
        30.0,
    );
    export_case(
        &dir,
        &input,
        "bitrate-mp4",
        &media,
        &spec,
        &OutputSettings {
            container: ContainerFormat::Mp4,
            ..base.clone()
        },
        "h264",
        960,
        540,
        60.0,
    );
    let larger = export_case(
        &dir,
        &input,
        "bitrate-3000",
        &media,
        &spec,
        &OutputSettings {
            video_bitrate_kbps: 3000,
            ..base.clone()
        },
        "h264",
        960,
        540,
        60.0,
    );
    assert!(
        larger > source_rate * 1.4,
        "increasing target bitrate must increase output size"
    );
    assert!(
        source_rate / half_rate < 1.35 && half_rate / source_rate < 1.35,
        "target bitrate should not depend on whether the frame rate was explicitly changed"
    );

    let high = OutputSettings {
        quality_mode: QualityMode::Preset,
        quality_preset: QualityPreset::High,
        max_height: 360,
        ..base.clone()
    };
    let high_bytes = export_case(
        &dir,
        &input,
        "high-360p",
        &media,
        &spec,
        &high,
        "h264",
        640,
        360,
        60.0,
    );
    let original_bytes = export_case(
        &dir,
        &input,
        "original-360p",
        &media,
        &spec,
        &OutputSettings {
            quality_preset: QualityPreset::Original,
            ..high.clone()
        },
        "h264",
        640,
        360,
        60.0,
    );
    assert!((original_bytes / high_bytes - 1.0).abs() < 0.02,
        "Original forced to re-encode must use the same quality as High: original={original_bytes}, high={high_bytes}");

    let custom_bytes = export_case(
        &dir,
        &input,
        "custom-quality-18",
        &media,
        &spec,
        &OutputSettings {
            quality_mode: QualityMode::Crf,
            crf: 18,
            ..high.clone()
        },
        "h264",
        640,
        360,
        60.0,
    );
    assert!(
        (custom_bytes / high_bytes - 1.0).abs() < 0.02,
        "custom quality 18 must match High"
    );
    let small_bytes = export_case(
        &dir,
        &input,
        "small-360p",
        &media,
        &spec,
        &OutputSettings {
            quality_preset: QualityPreset::Small,
            ..high.clone()
        },
        "h264",
        640,
        360,
        60.0,
    );
    assert!(
        small_bytes < high_bytes * 0.85,
        "Small must produce fewer bytes than High"
    );

    export_case(
        &dir,
        &input,
        "hevc-360p-30fps",
        &media,
        &spec,
        &OutputSettings {
            video_codec: VideoCodecChoice::Libx265,
            fps: 30,
            ..high
        },
        "hevc",
        640,
        360,
        30.0,
    );
    let uncapped = export_case(
        &dir,
        &input,
        "quality-10",
        &media,
        &spec,
        &OutputSettings {
            quality_mode: QualityMode::Crf,
            crf: 10,
            ..base.clone()
        },
        "h264",
        960,
        540,
        60.0,
    );
    let capped = export_case(
        &dir,
        &input,
        "vbr-cap",
        &media,
        &spec,
        &OutputSettings {
            quality_mode: QualityMode::Vbr,
            crf: 10,
            ..base
        },
        "h264",
        960,
        540,
        60.0,
    );
    assert!(
        capped < uncapped * 0.8,
        "VBR must enforce the bitrate cap on complex content"
    );
    assert!(
        capped < 1500.0 * 1000.0 * 4.0 / 8.0 * 1.25,
        "VBR exceeds the cap plus mux/rate-control tolerance"
    );
    export_case(
        &dir,
        &input,
        "original-copy",
        &media,
        &spec,
        &OutputSettings {
            container: ContainerFormat::Mkv,
            ..OutputSettings::default()
        },
        "ffv1",
        960,
        540,
        60.0,
    );
    eprintln!("E2E outputs: {}", dir.display());
}

#[allow(clippy::too_many_arguments)]
fn export_case(
    dir: &Path,
    input: &Path,
    name: &str,
    media: &MediaInfo,
    spec: &EditSpec,
    settings: &OutputSettings,
    codec: &str,
    width: i64,
    height: i64,
    fps: f64,
) -> f64 {
    let path = dir.join(format!("{name}.{}", settings.container.extension()));
    let progress = Arc::new(Mutex::new(0.0));
    run_export(
        input.to_str().unwrap(),
        path.to_str().unwrap(),
        spec,
        settings,
        media,
        false,
        &[],
        progress.clone(),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(*progress.lock().unwrap(), 1.0, "{name}: progress");
    let (actual, _) = crate::libav::probe(path.to_str().unwrap()).unwrap();
    assert_eq!(actual.video_codec, codec, "{name}: codec setting");
    assert_eq!(
        (actual.width, actual.height),
        (width, height),
        "{name}: resolution setting"
    );
    assert!(
        (actual.fps - fps).abs() < 0.01,
        "{name}: frame rate {} != {fps}",
        actual.fps
    );
    assert!(
        (actual.duration_sec - 4.0).abs() < 0.1,
        "{name}: trim duration {}",
        actual.duration_sec
    );
    assert!(actual.audio_streams.is_empty(), "{name}: unexpected audio");
    assert_eq!(
        decoded_frames(&path),
        (4.0 * fps) as usize,
        "{name}: decoded frame count"
    );
    let estimate = estimate_export_size(media, spec, &output_settings_to_encode(settings, media));
    let bytes = actual.size_bytes.unwrap() as f64;
    eprintln!(
        "{name}: estimated={:.0}, actual={bytes:.0}, ratio={:.3}",
        estimate.bytes,
        bytes / estimate.bytes
    );
    if settings.quality_mode == QualityMode::Bitrate {
        // A moving fixture needs the bitrate budget; allow normal hardware RC/mux variation.
        assert!(
            (bytes / estimate.bytes - 1.0).abs() < 0.25,
            "{name}: target-bitrate estimate {:.0} differs from actual {bytes:.0} by more than 25%",
            estimate.bytes
        );
    }
    bytes
}

fn create_source(path: &Path) {
    let graph = AVFilterGraph::new();
    let mut sink = graph
        .create_filter_context(&AVFilter::get_by_name(c"buffersink").unwrap(), c"out", None)
        .unwrap();
    graph
        .parse_ptr(
            c"testsrc2=size=960x540:rate=60:duration=6,format=yuv420p[out]",
            Some(AVFilterInOut::new(c"out", &mut sink, 0)),
            None,
        )
        .unwrap();
    graph.config().unwrap();
    let tb = sink.get_time_base();
    let mut enc = AVCodecContext::new(&AVCodec::find_encoder(ffi::AV_CODEC_ID_FFV1).unwrap());
    enc.set_width(960);
    enc.set_height(540);
    enc.set_pix_fmt(ffi::AV_PIX_FMT_YUV420P);
    enc.set_time_base(tb);
    enc.set_framerate(ffi::AVRational { num: 60, den: 1 });
    enc.set_gop_size(1);
    enc.set_flags(enc.flags | ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32);
    enc.open(None).unwrap();
    let mut mux =
        AVFormatContextOutput::create(&CString::new(path.to_str().unwrap()).unwrap()).unwrap();
    {
        let mut stream = mux.new_stream();
        stream.set_codecpar(enc.extract_codecpar());
        stream.set_time_base(tb);
    }
    mux.write_header(&mut None).unwrap();
    let out_tb = mux.streams()[0].time_base;
    for _ in 0..360 {
        let mut frame = sink.buffersink_get_frame(None).unwrap();
        encode_video_frame(&mut enc, Some(&mut frame), &mut mux, tb, out_tb, 0).unwrap();
    }
    encode_video_frame(&mut enc, None, &mut mux, tb, out_tb, 0).unwrap();
    mux.write_trailer().unwrap();
}

fn decoded_frames(path: &Path) -> usize {
    let mut input =
        AVFormatContextInput::open(&CString::new(path.to_str().unwrap()).unwrap()).unwrap();
    let (index, codec) = input
        .find_best_stream(ffi::AVMEDIA_TYPE_VIDEO)
        .unwrap()
        .unwrap();
    let mut dec = AVCodecContext::new(&codec);
    dec.apply_codecpar(&input.streams()[index].codecpar())
        .unwrap();
    dec.open(None).unwrap();
    let mut count = 0;
    while let Some(packet) = input.read_packet().unwrap() {
        if packet.stream_index == index as i32 {
            dec.send_packet(Some(&packet)).unwrap();
            count += drain_frames(&mut dec);
        }
    }
    dec.send_packet(None).unwrap();
    count + drain_frames(&mut dec)
}

fn drain_frames(dec: &mut AVCodecContext) -> usize {
    let mut count = 0;
    loop {
        match dec.receive_frame() {
            Ok(_) => count += 1,
            Err(RsmpegError::DecoderDrainError | RsmpegError::DecoderFlushedError) => return count,
            Err(e) => panic!("output failed to decode: {e:?}"),
        }
    }
}
