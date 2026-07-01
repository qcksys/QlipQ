//! Step-3 gate (media-stack plan): prove **in-process hardware encode + mux** end-to-end via rsmpeg —
//! the go/no-go for moving export off the CLI `ffmpeg` path onto the libav stack.
//!
//! It decodes the input video, normalizes to the encoder's pixel format through an avfilter chain
//! (`nv12` for SDR, `p010le` for 10-bit / HDR sources, carrying the BT.2020 PQ color tags), encodes
//! with a hardware encoder (NVENC/AMF/QSV), and muxes a faststart mp4. This is the missing half the
//! other examples don't cover (they decode + filter only); together they prove the full pipeline.
//!
//! Standalone on purpose — zero blast radius on the app. Mirrors the decode/filter pattern of
//! `play_probe`/`hdr_probe` and adds the encoder + muxer.
//!
//!   cargo run --release -p qlipq-desktop --example encode_probe -- \
//!       "<input>" "<out.mp4>" [encoder=h264_nvenc|auto] [seconds=5]
//!
//! Verify by re-probing the output (`ffprobe -show_streams out.mp4`: codec, pix_fmt, color tags,
//! duration) and by playing it. Try encoder = h264_nvenc | hevc_nvenc | av1_nvenc | h264_amf |
//! hevc_amf | h264_qsv | hevc_qsv to exercise each HW path. 10-bit/HDR inputs auto-upgrade an h264
//! request to HEVC (NVENC h264 has no 10-bit profile). `auto` runs the Step-5 HW quality model
//! (`qlipq_ffmpeg::hw::plan_hw_video`) with a runtime usability probe to pick the encoder + rate control.

use std::ffi::CString;
use std::time::Instant;

use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avfilter::{AVFilter, AVFilterGraph, AVFilterInOut};
use rsmpeg::avformat::{AVFormatContextInput, AVFormatContextOutput};
use rsmpeg::avutil::{AVDictionary, AVFrame};
use rsmpeg::error::RsmpegError;
use rsmpeg::ffi;

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("usage: encode_probe <input> <out.mp4> [encoder] [seconds]");
    let output = args.next().expect("usage: encode_probe <input> <out.mp4> [encoder] [seconds]");
    let encoder = args.next().unwrap_or_else(|| "h264_nvenc".to_string());
    let max_secs: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(5.0);

    match run(&input, &output, &encoder, max_secs) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("FAILED ({encoder}): {e}");
            std::process::exit(1);
        }
    }
}

/// Source color tags (copied so we can stamp them onto the encoder once the input borrow is gone).
struct SrcColor {
    primaries: ffi::AVColorPrimaries,
    trc: ffi::AVColorTransferCharacteristic,
    space: ffi::AVColorSpace,
    range: ffi::AVColorRange,
}

fn run(input: &str, output: &str, encoder: &str, max_secs: f64) -> Result<(), String> {
    let started = Instant::now();
    let cin = CString::new(input).map_err(|e| e.to_string())?;
    let mut ictx = AVFormatContextInput::open(&cin).map_err(|e| format!("open input: {e:?}"))?;
    let vid_idx = ictx
        .find_best_stream(ffi::AVMEDIA_TYPE_VIDEO)
        .map_err(|e| format!("{e:?}"))?
        .map(|(i, _)| i)
        .ok_or("no video stream")?;

    // --- decoder (multithreaded, like the preview path) ---
    let (mut dec, tb_in, sar, fps, src_color) = {
        let st = &ictx.streams()[vid_idx];
        let tb = st.time_base;
        let afr = st.avg_frame_rate;
        let fps = if afr.num > 0 && afr.den > 0 { afr } else { st.r_frame_rate };
        let par = st.codecpar();
        let sar0 = par.sample_aspect_ratio;
        let sar = if sar0.num == 0 { ffi::AVRational { num: 1, den: 1 } } else { sar0 };
        let src_color = SrcColor {
            primaries: par.color_primaries,
            trc: par.color_trc,
            space: par.color_space,
            range: par.color_range,
        };
        let codec = AVCodec::find_decoder(par.codec_id).ok_or("no decoder for input")?;
        let mut dec = AVCodecContext::new(&codec);
        dec.apply_codecpar(&par).map_err(|e| format!("apply_codecpar: {e:?}"))?;
        dec.set_pkt_timebase(tb);
        unsafe { (*dec.as_mut_ptr()).thread_count = 0 };
        dec.open(None).map_err(|e| format!("open decoder: {e:?}"))?;
        (dec, tb, sar, fps, src_color)
    };

    // --- pick encoder pixel format from source bit depth / HDR transfer ---
    let depth = unsafe {
        let d = ffi::av_pix_fmt_desc_get(dec.pix_fmt);
        if d.is_null() { 8 } else { (*d.cast::<ffi::AVPixFmtDescriptor>()).comp[0].depth }
    };
    let is_hdr = src_color.trc == ffi::AVCOL_TRC_SMPTE2084 || src_color.trc == ffi::AVCOL_TRC_ARIB_STD_B67;
    let ten_bit = depth > 8 || is_hdr;

    // Encoder + rate-control. `auto` runs the Step-5 HW quality model (`plan_hw_video`) with a runtime
    // usability probe, so it picks the encoder + opts and falls back vendor by vendor; otherwise the
    // named encoder is used (with a fixed NVENC rate-control for the gate). 10-bit/HDR forces HEVC.
    let (encoder, planned_opts, bitrate_kbps, maxrate_kbps): (String, Vec<(&str, String)>, Option<i64>, Option<i64>) =
        if encoder == "auto" {
            let settings = qlipq_core::config::OutputSettings::default();
            let plan = qlipq_ffmpeg::hw::plan_hw_video(&settings, ten_bit, encoder_usable).ok_or("no usable HW encoder")?;
            eprintln!("auto: {} opts={:?} bitrate={:?} maxrate={:?}", plan.encoder, plan.opts, plan.bitrate_kbps, plan.maxrate_kbps);
            (plan.encoder, plan.opts, plan.bitrate_kbps, plan.maxrate_kbps)
        } else if ten_bit && encoder == "h264_nvenc" {
            eprintln!("note: 10-bit/HDR source — upgrading h264_nvenc → hevc_nvenc (h264 has no 10-bit profile)");
            ("hevc_nvenc".to_string(), vec![("preset", "p5".into()), ("rc", "vbr".into()), ("cq", "23".into())], None, None)
        } else {
            let o = if encoder.contains("nvenc") {
                vec![("preset", "p5".into()), ("rc", "vbr".into()), ("cq", "23".into())]
            } else {
                vec![]
            };
            (encoder.to_string(), o, None, None)
        };

    let (enc_pix_name, enc_pix) = if ten_bit { ("p010le", ffi::AV_PIX_FMT_P010LE) } else { ("nv12", ffi::AV_PIX_FMT_NV12) };

    // --- filter: buffer -> format=<nv12|p010le> -> buffersink (keep source resolution) ---
    let graph = AVFilterGraph::new();
    let bufargs = CString::new(format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}:colorspace={}:range={}",
        dec.width, dec.height, dec.pix_fmt as i32, tb_in.num, tb_in.den, sar.num, sar.den,
        src_color.space as i32, src_color.range as i32
    ))
    .map_err(|e| e.to_string())?;
    let mut src = graph
        .create_filter_context(&AVFilter::get_by_name(c"buffer").unwrap(), c"in", Some(&bufargs))
        .map_err(|e| format!("buffer: {e:?}"))?;
    let mut sink = graph
        .create_filter_context(&AVFilter::get_by_name(c"buffersink").unwrap(), c"out", None)
        .map_err(|e| format!("buffersink: {e:?}"))?;
    let outputs = AVFilterInOut::new(c"in", &mut src, 0);
    let inputs = AVFilterInOut::new(c"out", &mut sink, 0);
    let descr = CString::new(format!("format={enc_pix_name}")).map_err(|e| e.to_string())?;
    graph.parse_ptr(&descr, Some(inputs), Some(outputs)).map_err(|e| format!("parse: {e:?}"))?;
    graph.config().map_err(|e| format!("graph config: {e:?}"))?;

    // --- encoder ---
    let cenc = CString::new(encoder.as_str()).map_err(|e| e.to_string())?;
    let codec = AVCodec::find_encoder_by_name(&cenc).ok_or_else(|| format!("encoder '{encoder}' not found (HW present?)"))?;
    let mut enc = AVCodecContext::new(&codec);
    enc.set_width(dec.width);
    enc.set_height(dec.height);
    enc.set_pix_fmt(enc_pix);
    enc.set_sample_aspect_ratio(sar);
    enc.set_time_base(tb_in); // frames pass through carrying their stream-timebase pts
    enc.set_framerate(fps);
    enc.set_gop_size(120);
    // Carry color metadata so HDR survives (x265's automatic carry-through is gone on HW encoders).
    unsafe {
        let p = enc.as_mut_ptr();
        (*p).color_primaries = if is_hdr { ffi::AVCOL_PRI_BT2020 } else { src_color.primaries };
        (*p).color_trc = if is_hdr { ffi::AVCOL_TRC_SMPTE2084 } else { src_color.trc };
        (*p).colorspace = if is_hdr { ffi::AVCOL_SPC_BT2020_NCL } else { src_color.space };
        (*p).color_range = src_color.range;
    }
    let global_header = ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
    enc.set_flags(enc.flags | global_header); // mp4 needs out-of-band extradata
    if let Some(b) = bitrate_kbps {
        enc.set_bit_rate(b * 1000);
    }
    if let Some(m) = maxrate_kbps {
        unsafe { (*enc.as_mut_ptr()).rc_max_rate = m * 1000 };
    }

    let opts = dict_from(&planned_opts);
    enc.open(opts).map_err(|e| format!("open encoder '{encoder}': {e:?}"))?;
    let enc_tb = enc.time_base;

    // --- muxer ---
    let cout = CString::new(output).map_err(|e| e.to_string())?;
    let mut octx = AVFormatContextOutput::create(&cout).map_err(|e| format!("create output: {e:?}"))?;
    {
        let mut stream = octx.new_stream();
        stream.set_codecpar(enc.extract_codecpar());
        stream.set_time_base(enc_tb);
    }
    let mut header_opts = Some(AVDictionary::new(c"movflags", c"+faststart", 0));
    octx.write_header(&mut header_opts).map_err(|e| format!("write_header: {e:?}"))?;
    let out_tb = octx.streams()[0].time_base;

    // --- decode -> filter -> encode -> mux, stopping after `max_secs` of content ---
    let tb_secs = tb_in.num as f64 / tb_in.den as f64;
    let mut first_pts: Option<i64> = None;
    let mut frames_in = 0u64;
    let mut frames_out = 0u64;
    let mut flushed_filter = false;

    'pump: loop {
        // 1) drain ready filtered frames into the encoder
        loop {
            match sink.buffersink_get_frame(None) {
                Ok(frame) => {
                    let f0 = *first_pts.get_or_insert(frame.pts);
                    if max_secs > 0.0 && (frame.pts - f0) as f64 * tb_secs >= max_secs {
                        break 'pump; // reached the cut; flush the encoder below
                    }
                    encode(&mut enc, Some(&frame), &mut octx, enc_tb, out_tb, &mut frames_out)?;
                    frames_in += 1;
                }
                Err(RsmpegError::BufferSinkDrainError) => break,
                Err(RsmpegError::BufferSinkEofError) => break 'pump,
                Err(e) => return Err(format!("buffersink: {e:?}")),
            }
        }
        // 2) push one decoded frame into the filter
        match dec.receive_frame() {
            Ok(frame) => {
                let _ = src.buffersrc_add_frame(Some(frame), None);
                continue;
            }
            Err(RsmpegError::DecoderDrainError) => {}
            Err(RsmpegError::DecoderFlushedError) => {}
            Err(e) => return Err(format!("receive_frame: {e:?}")),
        }
        // 3) feed the decoder the next video packet, or flush both at input EOF
        match ictx.read_packet() {
            Ok(Some(pkt)) => {
                if pkt.stream_index == vid_idx as i32 {
                    let _ = dec.send_packet(Some(&pkt));
                }
            }
            Ok(None) => {
                if !flushed_filter {
                    let _ = dec.send_packet(None);
                    while let Ok(frame) = dec.receive_frame() {
                        let _ = src.buffersrc_add_frame(Some(frame), None);
                    }
                    let _ = src.buffersrc_add_frame(None, None);
                    flushed_filter = true;
                }
            }
            Err(e) => return Err(format!("read_packet: {e:?}")),
        }
    }

    // flush the encoder, then finalize the container
    encode(&mut enc, None, &mut octx, enc_tb, out_tb, &mut frames_out)?;
    octx.write_trailer().map_err(|e| format!("write_trailer: {e:?}"))?;

    let bytes = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    println!(
        "OK  {encoder}  {}x{} {enc_pix_name}{}  in={frames_in}f out={frames_out}pkt  {:.2}MB  ({:.1}s wall)",
        dec.width,
        dec.height,
        if is_hdr { " HDR/PQ" } else { "" },
        bytes as f64 / 1.0e6,
        started.elapsed().as_secs_f64(),
    );
    Ok(())
}

/// Runtime usability probe for the Step-5 HW quality model: does this encoder actually open here?
/// Catches the "listed but unusable" case (e.g. QSV with no working iGPU — `MFX session` errors).
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

/// Build an `AVDictionary` from the planned `(key, value)` encoder options.
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

/// Send a frame (or `None` to flush) to the encoder and interleave every emitted packet into the mux.
fn encode(
    enc: &mut AVCodecContext,
    frame: Option<&AVFrame>,
    octx: &mut AVFormatContextOutput,
    enc_tb: ffi::AVRational,
    out_tb: ffi::AVRational,
    frames_out: &mut u64,
) -> Result<(), String> {
    enc.send_frame(frame).map_err(|e| format!("send_frame: {e:?}"))?;
    loop {
        match enc.receive_packet() {
            Ok(mut pkt) => {
                pkt.set_stream_index(0);
                pkt.rescale_ts(enc_tb, out_tb);
                octx.interleaved_write_frame(&mut pkt).map_err(|e| format!("write_frame: {e:?}"))?;
                *frames_out += 1;
            }
            Err(RsmpegError::EncoderDrainError) | Err(RsmpegError::EncoderFlushedError) => break,
            Err(e) => return Err(format!("receive_packet: {e:?}")),
        }
    }
    Ok(())
}
