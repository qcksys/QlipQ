//! In-process export: decode → apply edits → hardware-encode → mux, fully
//! in-process (the sole export path). Uses the Step-5 HW quality model
//! ([`qlipq_ffmpeg::hw::plan_hw_video`]) to pick the encoder + rate control.
//!
//! - **Video:** `buffer → trim,setpts,crop,scale,fps,format → buffersink → HW encoder`.
//! - **Audio:** per enabled track `abuffer → atrim,asetpts,[volume] → amix(normalize=0) →
//!   aformat(fltp/48k/stereo) → abuffersink(frame_size) → AAC`. `amix` sums the enabled tracks at
//!   their set levels (matching the preview monitor mix).
//! - **Mux:** video + audio interleaved by DTS into an mp4 written to a temp file, then renamed.
//!
//! Progress (0..1) and a cancel flag are shared with the UI. Windows-first prototype: it assumes a
//! working HW encoder and uniform stereo audio tracks; there is no CLI fallback.

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rsmpeg::avcodec::{AVCodec, AVCodecContext, AVCodecParameters};
use rsmpeg::avfilter::{AVFilter, AVFilterContextMut, AVFilterGraph};
use rsmpeg::avformat::{AVFormatContextInput, AVFormatContextOutput};
use rsmpeg::avutil::{AVDictionary, AVFrame};
use rsmpeg::error::RsmpegError;
use rsmpeg::ffi;

use qlipq_core::config::OutputSettings;
use qlipq_core::edit_spec::EditSpec;
use qlipq_core::media::MediaInfo;
use qlipq_ffmpeg::args::output_settings_to_encode;
use qlipq_ffmpeg::hw::plan_hw_video;

/// Decode + apply `spec`/`settings` edits + hardware-encode + mux `input_path` to `output_path`.
/// `progress` is updated to the encoded fraction (0..1); set `cancel` to abort. Returns `Err` on any
/// failure (the partial temp file is removed).
#[allow(clippy::too_many_arguments)]
pub fn run_export(
    input_path: &str,
    output_path: &str,
    spec: &EditSpec,
    settings: &OutputSettings,
    media: &MediaInfo,
    is_hdr: bool,
    metadata: &[(String, String)],
    progress: Arc<Mutex<f32>>,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    let _log = crate::log_ctx::enter(input_path);
    let temp_path = format!("{output_path}.part.mp4");
    // Stream-copy (remux) the video when nothing forces a re-encode (Original quality, no crop /
    // downscale / fps change) — a lossless, fast trim. Audio is still mixed/encoded. Otherwise
    // decode → filter → hardware-encode.
    let resolved = output_settings_to_encode(settings, media);
    let video_reencode =
        spec.crop.is_some() || resolved.video.scale_height.is_some() || resolved.video.fps.is_some() || resolved.reencode;
    let result = if video_reencode {
        export_transcode(input_path, &temp_path, spec, settings, media, is_hdr, metadata, &progress, &cancel)
    } else {
        export_remux(input_path, &temp_path, spec, settings, media, metadata, &progress, &cancel)
    };
    match result {
        Ok(()) => {
            let _ = std::fs::remove_file(output_path); // overwrite target if present
            std::fs::rename(&temp_path, output_path).map_err(|e| format!("rename temp → output: {e}"))?;
            if let Ok(mut p) = progress.lock() {
                *p = 1.0;
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            Err(e)
        }
    }
}

/// One enabled audio track being mixed: its decoder and the `abuffer` source feeding the amix graph.
struct TrackDec<'g> {
    abs_idx: i32,
    dec: AVCodecContext,
    src: AVFilterContextMut<'g>,
}

/// Full decode → filter (trim/crop/scale/fps) → hardware-encode → mux path.
#[allow(clippy::too_many_arguments)]
fn export_transcode(
    input_path: &str,
    temp_path: &str,
    spec: &EditSpec,
    settings: &OutputSettings,
    media: &MediaInfo,
    is_hdr: bool,
    metadata: &[(String, String)],
    progress: &Arc<Mutex<f32>>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let start = spec.trim.as_ref().map(|t| t.start_sec).unwrap_or(0.0).max(0.0);
    let end = spec.trim.as_ref().map(|t| t.end_sec).unwrap_or(media.duration_sec).max(start);
    let dur = (end - start).max(0.001);

    let resolved = output_settings_to_encode(settings, media);
    let scale_height = resolved.video.scale_height;
    let fps = resolved.video.fps;

    let cin = CString::new(input_path).map_err(|e| e.to_string())?;
    let mut ictx = AVFormatContextInput::open(&cin).map_err(|e| format!("open input: {e:?}"))?;
    let vid_idx = ictx
        .find_best_stream(ffi::AVMEDIA_TYPE_VIDEO)
        .map_err(|e| format!("{e:?}"))?
        .map(|(i, _)| i)
        .ok_or("no video stream")? as i32;

    // ---- video decoder ----
    let (mut vdec, tb_v, sar, src_color) = {
        let st = &ictx.streams()[vid_idx as usize];
        let tb = st.time_base;
        let par = st.codecpar();
        let sar0 = par.sample_aspect_ratio;
        let sar = if sar0.num == 0 { ffi::AVRational { num: 1, den: 1 } } else { sar0 };
        let color = (par.color_primaries, par.color_trc, par.color_space, par.color_range);
        let codec = AVCodec::find_decoder(par.codec_id).ok_or("no video decoder")?;
        let mut dec = AVCodecContext::new(&codec);
        dec.apply_codecpar(&par).map_err(|e| format!("apply_codecpar: {e:?}"))?;
        dec.set_pkt_timebase(tb);
        unsafe { (*dec.as_mut_ptr()).thread_count = 0 };
        dec.open(None).map_err(|e| format!("open video decoder: {e:?}"))?;
        (dec, tb, sar, color)
    };

    let ten_bit = is_hdr || pix_depth(vdec.pix_fmt) > 8;
    let plan = plan_hw_video(settings, ten_bit, encoder_usable).ok_or("no usable hardware encoder")?;

    // ---- enabled audio tracks ----
    let audio_abs: Vec<i32> = ictx
        .streams()
        .iter()
        .enumerate()
        .filter(|(_, s)| s.codecpar().codec_type == ffi::AVMEDIA_TYPE_AUDIO)
        .map(|(i, _)| i as i32)
        .collect();
    let enabled: Vec<(i32, f64)> = spec
        .audio_tracks
        .iter()
        .filter(|t| t.enabled)
        .filter_map(|t| audio_abs.get(t.index.max(0) as usize).map(|&abs| (abs, t.volume)))
        .collect();
    let has_audio = !enabled.is_empty();

    // ---- seek to the trim start (the trim filter does the exact cut + pts rebase) ----
    if tb_v.num != 0 && start > 0.0 {
        let ts = (start * tb_v.den as f64 / tb_v.num as f64) as i64;
        let _ = ictx.seek(vid_idx, ts, ffi::AVSEEK_FLAG_BACKWARD as i32);
        vdec.flush_buffers();
    }

    // ---- video filter graph ----
    let vgraph = AVFilterGraph::new();
    let descr = video_filter_descr(spec, scale_height, fps, ten_bit, start, end);
    let (mut vsrc, mut vsink) = build_video_filter(&vgraph, &vdec, tb_v, sar, &descr)?;

    // ---- prime: pull the first filtered frame to learn the output geometry + time base ----
    let first = prime_video(&mut ictx, &mut vdec, vid_idx, &mut vsrc, &mut vsink)?;
    let out_w = first.width;
    let out_h = first.height;
    // The encoder time base is the buffersink's (the frame's own `time_base` field isn't populated).
    let venc_tb = {
        let tb = vsink.get_time_base();
        if tb.num > 0 && tb.den > 0 { tb } else { ffi::AVRational { num: 1, den: 90_000 } }
    };

    // ---- video encoder ----
    let mut venc = build_video_encoder(&plan, out_w, out_h, sar, venc_tb, fps, ten_bit, is_hdr, src_color)?;

    // ---- audio: decoders + amix graph + AAC encoder ----
    let agraph = AVFilterGraph::new();
    let mut tracks: Vec<TrackDec> = Vec::new();
    let mut aenc: Option<AVCodecContext> = None;
    let mut asink_opt: Option<AVFilterContextMut> = None;
    if has_audio {
        let mut aencoder = build_audio_encoder(settings)?;
        let frame_size = if aencoder.frame_size > 0 { aencoder.frame_size as u32 } else { 1024 };
        let (decs, asink) = build_audio_graph(&agraph, &ictx, &enabled, start, end, frame_size)?;
        tracks = decs;
        asink_opt = Some(asink);
        // The encoder's time base is the sample clock.
        aencoder.set_time_base(ffi::AVRational { num: 1, den: aencoder.sample_rate });
        let _ = &mut aencoder; // (already opened in build_audio_encoder)
        aenc = Some(aencoder);
    }

    // ---- muxer: add streams, then write the header ----
    let cout = CString::new(temp_path).map_err(|e| e.to_string())?;
    let mut octx = AVFormatContextOutput::create(&cout).map_err(|e| format!("create output: {e:?}"))?;
    let v_stream = {
        let mut s = octx.new_stream();
        s.set_codecpar(venc.extract_codecpar());
        s.set_time_base(venc_tb);
        0usize
    };
    let a_stream = if let Some(ref ae) = aenc {
        let mut s = octx.new_stream();
        s.set_codecpar(ae.extract_codecpar());
        s.set_time_base(ae.time_base);
        Some(1usize)
    } else {
        None
    };
    unsafe {
        let p = octx.as_mut_ptr();
        for (k, val) in metadata {
            if val.is_empty() {
                continue;
            }
            if let (Ok(ck), Ok(cv)) = (CString::new(k.as_str()), CString::new(val.as_str())) {
                ffi::av_dict_set(&mut (*p).metadata, ck.as_ptr(), cv.as_ptr(), 0);
            }
        }
    }
    let mut header_opts = Some(AVDictionary::new(c"movflags", c"+faststart", 0));
    octx.write_header(&mut header_opts).map_err(|e| format!("write_header: {e:?}"))?;
    let v_out_tb = octx.streams()[v_stream].time_base;
    let a_out_tb = a_stream.map(|i| octx.streams()[i].time_base);

    // ---- encode the primed frame, then run the main loop ----
    let venc_tb_secs = venc_tb.num as f64 / venc_tb.den.max(1) as f64;
    encode_video_frame(&mut venc, Some(&first), &mut octx, venc_tb, v_out_tb, 0)?;
    let mut last_v_secs = first.pts as f64 * venc_tb_secs;

    let mut flushed_audio_srcs = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match ictx.read_packet() {
            Ok(Some(pkt)) => {
                let si = pkt.stream_index;
                if si == vid_idx {
                    let _ = vdec.send_packet(Some(&pkt));
                    drain_video(&mut vdec, &mut vsrc, &mut vsink, &mut venc, &mut octx, venc_tb, v_out_tb, &mut last_v_secs)?;
                } else if let Some(t) = tracks.iter_mut().find(|t| t.abs_idx == si) {
                    let _ = t.dec.send_packet(Some(&pkt));
                    feed_audio_decoder(t);
                    if let (Some(ae), Some(asink), Some(ao)) = (aenc.as_mut(), asink_opt.as_mut(), a_out_tb) {
                        drain_audio(asink, ae, &mut octx, a_stream.unwrap(), ao)?;
                    }
                }
                // Progress + early stop once we've encoded past the trim window (the trim filter has
                // dropped everything beyond `end`, so reading further only wastes decode).
                if let Ok(mut p) = progress.lock() {
                    *p = (last_v_secs / dur).clamp(0.0, 0.999) as f32;
                }
                if last_v_secs >= dur + 0.5 {
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    // ---- flush video: filter EOF → drain → encoder flush ----
    let _ = vdec.send_packet(None);
    drain_video(&mut vdec, &mut vsrc, &mut vsink, &mut venc, &mut octx, venc_tb, v_out_tb, &mut last_v_secs)?;
    let _ = vsrc.buffersrc_add_frame(None, None);
    drain_filtered_video(&mut vsink, &mut venc, &mut octx, venc_tb, v_out_tb, &mut last_v_secs)?;
    encode_video_frame(&mut venc, None, &mut octx, venc_tb, v_out_tb, 0)?;

    // ---- flush audio: decoders → abuffer EOF → drain → encoder flush ----
    if let (Some(ae), Some(asink), Some(ao)) = (aenc.as_mut(), asink_opt.as_mut(), a_out_tb) {
        if !flushed_audio_srcs {
            for t in tracks.iter_mut() {
                let _ = t.dec.send_packet(None);
                feed_audio_decoder(t);
                let _ = t.src.buffersrc_add_frame(None, None);
            }
            flushed_audio_srcs = true;
        }
        drain_audio(asink, ae, &mut octx, a_stream.unwrap(), ao)?;
        encode_audio_frame(ae, None, &mut octx, a_stream.unwrap(), ao)?;
    }
    let _ = flushed_audio_srcs;

    octx.write_trailer().map_err(|e| format!("write_trailer: {e:?}"))?;
    Ok(())
}

/// Lossless trim: **stream-copy** the video (seek to the keyframe ≤ start, copy packets with their
/// timestamps rebased to the trim) while audio is still decoded → mixed → AAC-encoded. The video
/// keyframe pre-roll (the gap from the keyframe to the exact in-point) is handled by the muxer's
/// `avoid_negative_ts`, which shifts all streams together so A/V stays in sync.
fn export_remux(
    input_path: &str,
    temp_path: &str,
    spec: &EditSpec,
    settings: &OutputSettings,
    media: &MediaInfo,
    metadata: &[(String, String)],
    progress: &Arc<Mutex<f32>>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let start = spec.trim.as_ref().map(|t| t.start_sec).unwrap_or(0.0).max(0.0);
    let end = spec.trim.as_ref().map(|t| t.end_sec).unwrap_or(media.duration_sec).max(start);
    let dur = (end - start).max(0.001);

    let cin = CString::new(input_path).map_err(|e| e.to_string())?;
    let mut ictx = AVFormatContextInput::open(&cin).map_err(|e| format!("open input: {e:?}"))?;
    let vid_idx = ictx
        .find_best_stream(ffi::AVMEDIA_TYPE_VIDEO)
        .map_err(|e| format!("{e:?}"))?
        .map(|(i, _)| i)
        .ok_or("no video stream")? as i32;
    let in_tb = ictx.streams()[vid_idx as usize].time_base;
    let in_tb_secs = in_tb.num as f64 / in_tb.den.max(1) as f64;

    // Enabled audio tracks (audio-relative index → absolute), same as the transcode path.
    let audio_abs: Vec<i32> = ictx
        .streams()
        .iter()
        .enumerate()
        .filter(|(_, s)| s.codecpar().codec_type == ffi::AVMEDIA_TYPE_AUDIO)
        .map(|(i, _)| i as i32)
        .collect();
    let enabled: Vec<(i32, f64)> = spec
        .audio_tracks
        .iter()
        .filter(|t| t.enabled)
        .filter_map(|t| audio_abs.get(t.index.max(0) as usize).map(|&abs| (abs, t.volume)))
        .collect();
    let has_audio = !enabled.is_empty();

    if start > 0.0 && in_tb.num != 0 {
        let ts = (start / in_tb_secs) as i64;
        let _ = ictx.seek(vid_idx, ts, ffi::AVSEEK_FLAG_BACKWARD as i32);
    }

    // Audio: decoders + amix graph + AAC encoder (identical to the transcode path).
    let agraph = AVFilterGraph::new();
    let mut tracks: Vec<TrackDec> = Vec::new();
    let mut aenc: Option<AVCodecContext> = None;
    let mut asink_opt: Option<AVFilterContextMut> = None;
    if has_audio {
        let aencoder = build_audio_encoder(settings)?;
        let frame_size = if aencoder.frame_size > 0 { aencoder.frame_size as u32 } else { 1024 };
        let (decs, asink) = build_audio_graph(&agraph, &ictx, &enabled, start, end, frame_size)?;
        tracks = decs;
        asink_opt = Some(asink);
        aenc = Some(aencoder);
    }

    // Copy the source video stream's parameters onto a new output stream (no encoder).
    let mut vpar = AVCodecParameters::new();
    unsafe {
        ffi::avcodec_parameters_copy(vpar.as_mut_ptr(), ictx.streams()[vid_idx as usize].codecpar().as_ptr());
    }

    let cout = CString::new(temp_path).map_err(|e| e.to_string())?;
    let mut octx = AVFormatContextOutput::create(&cout).map_err(|e| format!("create output: {e:?}"))?;
    {
        let mut s = octx.new_stream();
        s.set_codecpar(vpar);
        s.set_time_base(in_tb);
    }
    let a_stream = if let Some(ref ae) = aenc {
        let mut s = octx.new_stream();
        s.set_codecpar(ae.extract_codecpar());
        s.set_time_base(ae.time_base);
        Some(1usize)
    } else {
        None
    };
    unsafe {
        let p = octx.as_mut_ptr();
        // Keep the rebased (in-point = 0, keyframe pre-roll negative) timestamps so the mp4 muxer
        // writes an edit list that starts presentation at the in-point — playback + duration match the
        // trim, the keyframe pre-roll is carried but skipped on play.
        (*p).avoid_negative_ts = ffi::AVFMT_AVOID_NEG_TS_DISABLED as i32;
        for (k, val) in metadata {
            if val.is_empty() {
                continue;
            }
            if let (Ok(ck), Ok(cv)) = (CString::new(k.as_str()), CString::new(val.as_str())) {
                ffi::av_dict_set(&mut (*p).metadata, ck.as_ptr(), cv.as_ptr(), 0);
            }
        }
    }
    let mut header_opts = Some(AVDictionary::new(c"movflags", c"+faststart", 0));
    octx.write_header(&mut header_opts).map_err(|e| format!("write_header: {e:?}"))?;
    let v_out_tb = octx.streams()[0].time_base;
    let a_out_tb = a_stream.map(|i| octx.streams()[i].time_base);

    let start_ts = (start / in_tb_secs) as i64; // trim start in input video time base

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match ictx.read_packet() {
            Ok(Some(mut pkt)) => {
                if pkt.stream_index == vid_idx {
                    let pos_secs = if pkt.pts != ffi::AV_NOPTS_VALUE { pkt.pts as f64 * in_tb_secs } else { start };
                    // Copy only video within the out-point; keep reading a little past it so the audio
                    // (capped by `atrim`) is complete before we stop.
                    if pos_secs <= end {
                        if pkt.pts != ffi::AV_NOPTS_VALUE {
                            pkt.set_pts(pkt.pts - start_ts);
                        }
                        if pkt.dts != ffi::AV_NOPTS_VALUE {
                            pkt.set_dts(pkt.dts - start_ts);
                        }
                        pkt.set_stream_index(0);
                        pkt.rescale_ts(in_tb, v_out_tb);
                        octx.interleaved_write_frame(&mut pkt).map_err(|e| format!("write video: {e:?}"))?;
                    }
                    if let Ok(mut p) = progress.lock() {
                        *p = ((pos_secs - start) / dur).clamp(0.0, 0.999) as f32;
                    }
                    if pos_secs >= end + 0.5 {
                        break;
                    }
                } else if let Some(t) = tracks.iter_mut().find(|t| t.abs_idx == pkt.stream_index) {
                    let _ = t.dec.send_packet(Some(&pkt));
                    feed_audio_decoder(t);
                    if let (Some(ae), Some(asink), Some(ao)) = (aenc.as_mut(), asink_opt.as_mut(), a_out_tb) {
                        drain_audio(asink, ae, &mut octx, a_stream.unwrap(), ao)?;
                    }
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    // Flush audio (decoders → abuffer EOF → drain → encoder flush).
    if let (Some(ae), Some(asink), Some(ao)) = (aenc.as_mut(), asink_opt.as_mut(), a_out_tb) {
        for t in tracks.iter_mut() {
            let _ = t.dec.send_packet(None);
            feed_audio_decoder(t);
            let _ = t.src.buffersrc_add_frame(None, None);
        }
        drain_audio(asink, ae, &mut octx, a_stream.unwrap(), ao)?;
        encode_audio_frame(ae, None, &mut octx, a_stream.unwrap(), ao)?;
    }

    octx.write_trailer().map_err(|e| format!("write_trailer: {e:?}"))?;
    Ok(())
}

// ---- video ----

fn video_filter_descr(spec: &EditSpec, scale_height: Option<i64>, fps: Option<i64>, ten_bit: bool, start: f64, end: f64) -> String {
    let mut steps: Vec<String> = vec![format!("trim=start={start:.4}:end={end:.4}"), "setpts=PTS-STARTPTS".to_string()];
    if let Some(c) = &spec.crop {
        steps.push(format!("crop={}:{}:{}:{}", c.width, c.height, c.x, c.y));
    }
    if let Some(h) = scale_height {
        steps.push(format!("scale=-2:{h}"));
    }
    if let Some(r) = fps {
        steps.push(format!("fps={r}"));
    }
    steps.push(format!("format={}", if ten_bit { "p010le" } else { "nv12" }));
    steps.join(",")
}

fn build_video_filter<'g>(
    graph: &'g AVFilterGraph,
    vdec: &AVCodecContext,
    tb_v: ffi::AVRational,
    sar: ffi::AVRational,
    descr: &str,
) -> Result<(AVFilterContextMut<'g>, AVFilterContextMut<'g>), String> {
    use rsmpeg::avfilter::AVFilterInOut;
    let args = CString::new(format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
        vdec.width, vdec.height, vdec.pix_fmt as i32, tb_v.num, tb_v.den, sar.num, sar.den
    ))
    .map_err(|e| e.to_string())?;
    let mut src = graph
        .create_filter_context(&AVFilter::get_by_name(c"buffer").unwrap(), c"in", Some(&args))
        .map_err(|e| format!("buffer: {e:?}"))?;
    let mut sink = graph
        .create_filter_context(&AVFilter::get_by_name(c"buffersink").unwrap(), c"out", None)
        .map_err(|e| format!("buffersink: {e:?}"))?;
    let outputs = AVFilterInOut::new(c"in", &mut src, 0);
    let inputs = AVFilterInOut::new(c"out", &mut sink, 0);
    let cdescr = CString::new(descr).map_err(|e| e.to_string())?;
    graph.parse_ptr(&cdescr, Some(inputs), Some(outputs)).map_err(|e| format!("parse video: {e:?}"))?;
    graph.config().map_err(|e| format!("video graph config: {e:?}"))?;
    Ok((src, sink))
}

/// Decode + filter until the first filtered video frame appears; returns it (its geometry/time-base
/// configure the encoder, and it is the first frame encoded).
fn prime_video(
    input: &mut AVFormatContextInput,
    vdec: &mut AVCodecContext,
    vid_idx: i32,
    src: &mut AVFilterContextMut,
    sink: &mut AVFilterContextMut,
) -> Result<AVFrame, String> {
    let mut flushed = false;
    loop {
        match sink.buffersink_get_frame(None) {
            Ok(f) => return Ok(f),
            Err(RsmpegError::BufferSinkDrainError) => {}
            Err(e) => return Err(format!("prime buffersink: {e:?}")),
        }
        match vdec.receive_frame() {
            Ok(f) => {
                let _ = src.buffersrc_add_frame(Some(f), None);
                continue;
            }
            Err(RsmpegError::DecoderDrainError) => {}
            Err(_) => return Err("prime: no video frame".into()),
        }
        match input.read_packet() {
            Ok(Some(pkt)) => {
                if pkt.stream_index == vid_idx {
                    let _ = vdec.send_packet(Some(&pkt));
                }
            }
            Ok(None) if !flushed => {
                let _ = vdec.send_packet(None);
                let _ = src.buffersrc_add_frame(None, None);
                flushed = true;
            }
            _ => return Err("prime: input ended before a frame".into()),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_video_encoder(
    plan: &qlipq_ffmpeg::hw::HwVideo,
    w: i32,
    h: i32,
    sar: ffi::AVRational,
    time_base: ffi::AVRational,
    fps: Option<i64>,
    ten_bit: bool,
    is_hdr: bool,
    src_color: (ffi::AVColorPrimaries, ffi::AVColorTransferCharacteristic, ffi::AVColorSpace, ffi::AVColorRange),
) -> Result<AVCodecContext, String> {
    let cenc = CString::new(plan.encoder.as_str()).map_err(|e| e.to_string())?;
    let codec = AVCodec::find_encoder_by_name(&cenc).ok_or_else(|| format!("encoder '{}' not found", plan.encoder))?;
    let mut enc = AVCodecContext::new(&codec);
    enc.set_width(w);
    enc.set_height(h);
    enc.set_pix_fmt(if ten_bit { ffi::AV_PIX_FMT_P010LE } else { ffi::AV_PIX_FMT_NV12 });
    enc.set_sample_aspect_ratio(sar);
    enc.set_time_base(time_base);
    if let Some(r) = fps {
        enc.set_framerate(ffi::AVRational { num: r as i32, den: 1 });
    }
    enc.set_gop_size(120);
    unsafe {
        let p = enc.as_mut_ptr();
        (*p).color_primaries = if is_hdr { ffi::AVCOL_PRI_BT2020 } else { src_color.0 };
        (*p).color_trc = if is_hdr { ffi::AVCOL_TRC_SMPTE2084 } else { src_color.1 };
        (*p).colorspace = if is_hdr { ffi::AVCOL_SPC_BT2020_NCL } else { src_color.2 };
        (*p).color_range = src_color.3;
    }
    enc.set_flags(enc.flags | ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32);
    if let Some(b) = plan.bitrate_kbps {
        enc.set_bit_rate(b * 1000);
    }
    if let Some(m) = plan.maxrate_kbps {
        unsafe { (*enc.as_mut_ptr()).rc_max_rate = m * 1000 };
    }
    enc.open(dict_from(&plan.opts)).map_err(|e| format!("open video encoder '{}': {e:?}", plan.encoder))?;
    Ok(enc)
}

#[allow(clippy::too_many_arguments)]
fn drain_video(
    vdec: &mut AVCodecContext,
    src: &mut AVFilterContextMut,
    sink: &mut AVFilterContextMut,
    venc: &mut AVCodecContext,
    octx: &mut AVFormatContextOutput,
    enc_tb: ffi::AVRational,
    out_tb: ffi::AVRational,
    last_secs: &mut f64,
) -> Result<(), String> {
    while let Ok(frame) = vdec.receive_frame() {
        let _ = src.buffersrc_add_frame(Some(frame), None);
    }
    drain_filtered_video(sink, venc, octx, enc_tb, out_tb, last_secs)
}

fn drain_filtered_video(
    sink: &mut AVFilterContextMut,
    venc: &mut AVCodecContext,
    octx: &mut AVFormatContextOutput,
    enc_tb: ffi::AVRational,
    out_tb: ffi::AVRational,
    last_secs: &mut f64,
) -> Result<(), String> {
    let tb_secs = enc_tb.num as f64 / enc_tb.den.max(1) as f64;
    loop {
        match sink.buffersink_get_frame(None) {
            Ok(frame) => {
                *last_secs = frame.pts as f64 * tb_secs;
                encode_video_frame(venc, Some(&frame), octx, enc_tb, out_tb, 0)?;
            }
            Err(_) => break,
        }
    }
    Ok(())
}

fn encode_video_frame(
    venc: &mut AVCodecContext,
    frame: Option<&AVFrame>,
    octx: &mut AVFormatContextOutput,
    enc_tb: ffi::AVRational,
    out_tb: ffi::AVRational,
    stream_index: i32,
) -> Result<(), String> {
    venc.send_frame(frame).map_err(|e| format!("video send_frame: {e:?}"))?;
    loop {
        match venc.receive_packet() {
            Ok(mut pkt) => {
                pkt.set_stream_index(stream_index);
                pkt.rescale_ts(enc_tb, out_tb);
                octx.interleaved_write_frame(&mut pkt).map_err(|e| format!("write video: {e:?}"))?;
            }
            Err(RsmpegError::EncoderDrainError) | Err(RsmpegError::EncoderFlushedError) => break,
            Err(e) => return Err(format!("video receive_packet: {e:?}")),
        }
    }
    Ok(())
}

// ---- audio ----

fn build_audio_encoder(settings: &OutputSettings) -> Result<AVCodecContext, String> {
    let codec = AVCodec::find_encoder(ffi::AV_CODEC_ID_AAC).ok_or("no AAC encoder")?;
    let mut enc = AVCodecContext::new(&codec);
    enc.set_sample_fmt(ffi::AV_SAMPLE_FMT_FLTP);
    enc.set_sample_rate(48_000);
    enc.set_ch_layout(stereo_layout());
    enc.set_bit_rate(settings.audio_bitrate_kbps.max(64) * 1000);
    enc.set_time_base(ffi::AVRational { num: 1, den: 48_000 });
    enc.set_flags(enc.flags | ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32);
    enc.open(None).map_err(|e| format!("open AAC encoder: {e:?}"))?;
    Ok(enc)
}

/// Build the amix graph by hand (rsmpeg can't chain `AVFilterInOut` for multi-input): each track gets
/// `abuffer → atrim → asetpts → [volume] → amix:i`, then `amix → aformat(fltp/48k/stereo) → abuffersink`.
fn build_audio_graph<'g>(
    graph: &'g AVFilterGraph,
    input: &AVFormatContextInput,
    enabled: &[(i32, f64)],
    start: f64,
    end: f64,
    frame_size: u32,
) -> Result<(Vec<TrackDec<'g>>, AVFilterContextMut<'g>), String> {
    let n = enabled.len();
    let mut amix = graph
        .create_filter_context(
            &AVFilter::get_by_name(c"amix").unwrap(),
            c"amix",
            Some(&CString::new(format!("inputs={n}:normalize=0")).unwrap()),
        )
        .map_err(|e| format!("amix: {e:?}"))?;

    let mut tracks: Vec<TrackDec> = Vec::new();
    for (i, &(abs_idx, vol)) in enabled.iter().enumerate() {
        // decoder for this track
        let st = &input.streams()[abs_idx as usize];
        let tb = st.time_base;
        let par = st.codecpar();
        let codec = AVCodec::find_decoder(par.codec_id).ok_or("no audio decoder")?;
        let mut dec = AVCodecContext::new(&codec);
        dec.apply_codecpar(&par).map_err(|e| format!("audio apply_codecpar: {e:?}"))?;
        dec.set_pkt_timebase(tb);
        dec.open(None).map_err(|e| format!("open audio decoder: {e:?}"))?;

        let args = CString::new(format!(
            "sample_rate={}:sample_fmt={}:channel_layout={}:time_base={}/{}",
            dec.sample_rate,
            dec.sample_fmt as i32,
            layout_name(dec.ch_layout().nb_channels),
            tb.num,
            tb.den
        ))
        .unwrap();
        let mut src = graph
            .create_filter_context(&AVFilter::get_by_name(c"abuffer").unwrap(), &CString::new(format!("ain{i}")).unwrap(), Some(&args))
            .map_err(|e| format!("abuffer {i}: {e:?}"))?;

        let mut atrim = graph
            .create_filter_context(
                &AVFilter::get_by_name(c"atrim").unwrap(),
                &CString::new(format!("atrim{i}")).unwrap(),
                Some(&CString::new(format!("start={start:.4}:end={end:.4}")).unwrap()),
            )
            .map_err(|e| format!("atrim {i}: {e:?}"))?;
        let mut aset = graph
            .create_filter_context(
                &AVFilter::get_by_name(c"asetpts").unwrap(),
                &CString::new(format!("aset{i}")).unwrap(),
                Some(c"PTS-STARTPTS"),
            )
            .map_err(|e| format!("asetpts {i}: {e:?}"))?;
        src.link(0, &mut atrim, 0).map_err(|e| format!("link abuffer→atrim: {e:?}"))?;
        atrim.link(0, &mut aset, 0).map_err(|e| format!("link atrim→asetpts: {e:?}"))?;

        if (vol - 1.0).abs() > f64::EPSILON {
            let mut vol_ctx = graph
                .create_filter_context(
                    &AVFilter::get_by_name(c"volume").unwrap(),
                    &CString::new(format!("vol{i}")).unwrap(),
                    Some(&CString::new(format!("{vol}")).unwrap()),
                )
                .map_err(|e| format!("volume {i}: {e:?}"))?;
            aset.link(0, &mut vol_ctx, 0).map_err(|e| format!("link asetpts→volume: {e:?}"))?;
            vol_ctx.link(0, &mut amix, i as u32).map_err(|e| format!("link volume→amix: {e:?}"))?;
        } else {
            aset.link(0, &mut amix, i as u32).map_err(|e| format!("link asetpts→amix: {e:?}"))?;
        }

        tracks.push(TrackDec { abs_idx, dec, src });
    }

    let mut aformat = graph
        .create_filter_context(
            &AVFilter::get_by_name(c"aformat").unwrap(),
            c"afmt",
            Some(c"sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo"),
        )
        .map_err(|e| format!("aformat: {e:?}"))?;
    let mut asink = graph
        .create_filter_context(&AVFilter::get_by_name(c"abuffersink").unwrap(), c"aout", None)
        .map_err(|e| format!("abuffersink: {e:?}"))?;
    amix.link(0, &mut aformat, 0).map_err(|e| format!("link amix→aformat: {e:?}"))?;
    aformat.link(0, &mut asink, 0).map_err(|e| format!("link aformat→abuffersink: {e:?}"))?;

    graph.config().map_err(|e| format!("audio graph config: {e:?}"))?;
    asink.buffersink_set_frame_size(frame_size);
    Ok((tracks, asink))
}

fn feed_audio_decoder(t: &mut TrackDec) {
    while let Ok(frame) = t.dec.receive_frame() {
        let _ = t.src.buffersrc_add_frame(Some(frame), None);
    }
}

fn drain_audio(
    asink: &mut AVFilterContextMut,
    aenc: &mut AVCodecContext,
    octx: &mut AVFormatContextOutput,
    stream_index: usize,
    out_tb: ffi::AVRational,
) -> Result<(), String> {
    loop {
        match asink.buffersink_get_frame(None) {
            Ok(frame) => encode_audio_frame(aenc, Some(&frame), octx, stream_index, out_tb)?,
            Err(_) => break,
        }
    }
    Ok(())
}

fn encode_audio_frame(
    aenc: &mut AVCodecContext,
    frame: Option<&AVFrame>,
    octx: &mut AVFormatContextOutput,
    stream_index: usize,
    out_tb: ffi::AVRational,
) -> Result<(), String> {
    let enc_tb = aenc.time_base;
    aenc.send_frame(frame).map_err(|e| format!("audio send_frame: {e:?}"))?;
    loop {
        match aenc.receive_packet() {
            Ok(mut pkt) => {
                pkt.set_stream_index(stream_index as i32);
                pkt.rescale_ts(enc_tb, out_tb);
                octx.interleaved_write_frame(&mut pkt).map_err(|e| format!("write audio: {e:?}"))?;
            }
            Err(RsmpegError::EncoderDrainError) | Err(RsmpegError::EncoderFlushedError) => break,
            Err(e) => return Err(format!("audio receive_packet: {e:?}")),
        }
    }
    Ok(())
}

// ---- helpers ----

fn stereo_layout() -> ffi::AVChannelLayout {
    use rsmpeg::avutil::AVChannelLayout;
    AVChannelLayout::from_nb_channels(2).into_inner()
}

fn layout_name(nb_channels: i32) -> &'static str {
    match nb_channels {
        1 => "mono",
        2 => "stereo",
        6 => "5.1",
        8 => "7.1",
        _ => "stereo",
    }
}

fn pix_depth(pix_fmt: ffi::AVPixelFormat) -> i32 {
    unsafe {
        let d = ffi::av_pix_fmt_desc_get(pix_fmt);
        if d.is_null() {
            8
        } else {
            (*d.cast::<ffi::AVPixFmtDescriptor>()).comp[0].depth
        }
    }
}

/// Runtime usability probe for [`plan_hw_video`]: does this encoder actually open here?
fn encoder_usable(name: &str) -> bool {
    let Ok(cname) = CString::new(name) else { return false };
    let Some(codec) = AVCodec::find_encoder_by_name(&cname) else { return false };
    let mut enc = AVCodecContext::new(&codec);
    enc.set_width(320);
    enc.set_height(240);
    enc.set_pix_fmt(ffi::AV_PIX_FMT_NV12);
    enc.set_time_base(ffi::AVRational { num: 1, den: 30 });
    enc.open(None).is_ok()
}

fn dict_from(opts: &[(&str, String)]) -> Option<AVDictionary> {
    let mut dict: Option<AVDictionary> = None;
    for (k, v) in opts {
        let ck = CString::new(*k).ok()?;
        let cv = CString::new(v.as_str()).ok()?;
        dict = Some(match dict {
            Some(d) => d.set(&ck, &cv, 0),
            None => AVDictionary::new(&ck, &cv, 0),
        });
    }
    dict
}

#[cfg(test)]
mod tests {
    use super::*;
    use qlipq_core::edit_spec::{AudioTrackSpec, TrimSpec};

    /// End-to-end export against a real file. Ignored by default; run with a real clip:
    ///   QLIPQ_TEST_INPUT="E:/clip.mkv" cargo test -p qlipq-desktop \
    ///     -- --ignored --nocapture export_real_clip
    #[test]
    #[ignore]
    fn export_real_clip() {
        let Ok(input) = std::env::var("QLIPQ_TEST_INPUT") else {
            eprintln!("set QLIPQ_TEST_INPUT to run");
            return;
        };
        let out = std::env::var("QLIPQ_TEST_OUTPUT").unwrap_or_else(|_| format!("{input}.export-test.mp4"));

        // Probe the source for geometry/duration via libav.
        let cin = CString::new(input.as_str()).unwrap();
        let ictx = AVFormatContextInput::open(&cin).unwrap();
        let vidx = ictx.find_best_stream(ffi::AVMEDIA_TYPE_VIDEO).unwrap().map(|(i, _)| i).unwrap();
        let (w, h, is_hdr) = {
            let par = ictx.streams()[vidx].codecpar();
            let hdr = par.color_trc == ffi::AVCOL_TRC_SMPTE2084 || par.color_trc == ffi::AVCOL_TRC_ARIB_STD_B67;
            (par.width as i64, par.height as i64, hdr)
        };
        let audio_n = ictx
            .streams()
            .iter()
            .filter(|s| s.codecpar().codec_type == ffi::AVMEDIA_TYPE_AUDIO)
            .count();
        drop(ictx);

        let media = MediaInfo { duration_sec: 60.0, width: w, height: h, video_codec: "av1".into(), fps: 60.0, audio_streams: vec![], size_bytes: None };
        let spec = EditSpec {
            trim: Some(TrimSpec { start_sec: 1.0, end_sec: 4.0 }),
            crop: None,
            audio_tracks: (0..audio_n).map(|i| AudioTrackSpec { index: i as i64, enabled: true, volume: 1.0 }).collect(),
        };
        let mut settings = OutputSettings::default();
        if let Ok(s) = std::env::var("QLIPQ_TEST_SCALE") {
            settings.max_height = s.parse().unwrap_or(0);
        }
        let progress = Arc::new(Mutex::new(0.0f32));
        let cancel = Arc::new(AtomicBool::new(false));

        run_export(&input, &out, &spec, &settings, &media, is_hdr, &[("game".into(), "Test".into())], progress.clone(), cancel)
            .expect("export failed");

        // Validate the output: it opens, has a video + (if input had audio) an audio stream, ~3 s.
        let cout = CString::new(out.as_str()).unwrap();
        let octx = AVFormatContextInput::open(&cout).unwrap();
        let v = octx.streams().iter().filter(|s| s.codecpar().codec_type == ffi::AVMEDIA_TYPE_VIDEO).count();
        let a = octx.streams().iter().filter(|s| s.codecpar().codec_type == ffi::AVMEDIA_TYPE_AUDIO).count();
        assert_eq!(v, 1, "expected one video stream");
        assert_eq!(a, usize::from(audio_n > 0), "expected one mixed audio stream when source had audio");
        assert!(*progress.lock().unwrap() >= 0.99, "progress should reach ~1.0");
        eprintln!("export OK: {out}  video={v} audio={a}");
    }
}
