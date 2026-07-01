//! Phase-2 verification: decode video IN-PROCESS and tonemap HDR→SDR through the **libplacebo**
//! avfilter — exactly the pipeline the real preview will use — then print the first frame's size +
//! brightness distribution. Compare against `ffmpeg -vf libplacebo=...` (the VLC-quality reference)
//! to confirm the in-process path is equivalent.
//!
//!   cargo run -p qlipq-desktop --example hdr_probe -- "<clip>"

use std::ffi::CString;

use rsmpeg::avcodec::AVCodecContext;
use rsmpeg::avfilter::{AVFilter, AVFilterContextMut, AVFilterGraph, AVFilterInOut};
use rsmpeg::avformat::AVFormatContextInput;
use rsmpeg::avutil::AVFrame;
use rsmpeg::error::RsmpegError;
use rsmpeg::ffi;

fn main() {
    let arg = std::env::args().nth(1).expect("usage: hdr_probe <media path>");
    let path = CString::new(arg).unwrap();
    let mut input = AVFormatContextInput::open(&path).expect("open failed");

    // --- pick the video stream + build its decoder ---
    let (vid_idx, codec) = input
        .find_best_stream(ffi::AVMEDIA_TYPE_VIDEO)
        .expect("find_best_stream failed")
        .expect("no video stream");

    let (mut dec, args) = {
        let stream = &input.streams()[vid_idx];
        let tb = stream.time_base;
        let par = stream.codecpar();
        let sar = par.sample_aspect_ratio;
        let sar = if sar.num == 0 { ffi::AVRational { num: 1, den: 1 } } else { sar };

        let mut dec = AVCodecContext::new(&codec);
        dec.apply_codecpar(&par).expect("apply_codecpar failed");
        dec.open(None).expect("open decoder failed");

        // buffersrc init args; the decoded frames carry the BT.2020/PQ color tags that libplacebo
        // reads for tonemapping, so we only need geometry/format/time_base here.
        let args = CString::new(format!(
            "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
            dec.width, dec.height, dec.pix_fmt as i32, tb.num, tb.den, sar.num, sar.den
        ))
        .unwrap();
        (dec, args)
    };

    // --- filter graph: buffer -> libplacebo (tonemap HDR→BT.709 SDR, ≤720p, RGBA) -> buffersink ---
    let graph = AVFilterGraph::new();
    let mut src = graph
        .create_filter_context(&AVFilter::get_by_name(c"buffer").unwrap(), c"in", Some(&args))
        .expect("create buffer");
    let mut sink = graph
        .create_filter_context(&AVFilter::get_by_name(c"buffersink").unwrap(), c"out", None)
        .expect("create buffersink");

    // parse_ptr's inverted convention: `outputs` feeds the chain's input (buffersrc, label "in");
    // `inputs` is the chain's output (buffersink, label "out").
    let outputs = AVFilterInOut::new(c"in", &mut src, 0);
    let inputs = AVFilterInOut::new(c"out", &mut sink, 0);
    let descr = c"libplacebo=w=-2:h=min(ih\\,720):tonemapping=auto:colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=pc,format=rgba";
    graph.parse_ptr(descr, Some(inputs), Some(outputs)).expect("parse_ptr failed");
    graph.config().expect("graph config failed (libplacebo/Vulkan init?)");

    // --- pump demux → decode → filter until the first RGBA frame ---
    let (w, h, rgba) = pump_one(&mut input, &mut dec, &mut src, &mut sink, vid_idx as i32)
        .expect("produced no frame");

    // --- report size + brightness distribution (RGB only) ---
    let px: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mean = px.iter().map(|&v| v as u64).sum::<u64>() as f64 / px.len() as f64;
    let shadows = 100.0 * px.iter().filter(|&&v| v < 48).count() as f64 / px.len() as f64;
    let blown = 100.0 * px.iter().filter(|&&v| v > 235).count() as f64 / px.len() as f64;
    println!("in-process libplacebo: {w}x{h}  mean={mean:.1}  shadows<48={shadows:.1}%  blown>235={blown:.1}%");
    println!("(compare to: ffmpeg -ss 0 -i CLIP -frames:v 1 -vf 'libplacebo=...,format=rgba' -pix_fmt rgba)");
}

/// Standard demux→decode→filter pump: returns the first RGBA frame as tight `w*h*4` bytes.
fn pump_one(
    input: &mut AVFormatContextInput,
    dec: &mut AVCodecContext,
    src: &mut AVFilterContextMut,
    sink: &mut AVFilterContextMut,
    vid_idx: i32,
) -> Option<(u32, u32, Vec<u8>)> {
    let mut flushed = false;
    loop {
        // 1) a finished frame waiting at the sink?
        match sink.buffersink_get_frame(None) {
            Ok(frame) => return Some(extract_rgba(&frame)),
            Err(RsmpegError::BufferSinkDrainError) => {}
            Err(e) => {
                eprintln!("buffersink_get_frame error: {e:?}");
                return None;
            }
        }
        // 2) push a decoded frame into the graph
        match dec.receive_frame() {
            Ok(decoded) => {
                if let Err(e) = src.buffersrc_add_frame(Some(decoded), None) {
                    eprintln!("buffersrc_add_frame error: {e:?}");
                    return None;
                }
                continue;
            }
            Err(RsmpegError::DecoderDrainError) => {}
            Err(e) => {
                eprintln!("receive_frame error: {e:?}");
                return None;
            }
        }
        // 3) decoder is hungry → feed it the next video packet (or flush at EOF)
        match input.read_packet() {
            Ok(Some(pkt)) => {
                if pkt.stream_index == vid_idx {
                    if let Err(e) = dec.send_packet(Some(&pkt)) {
                        eprintln!("send_packet error: {e:?}");
                        return None;
                    }
                }
            }
            Ok(None) if !flushed => {
                dec.send_packet(None).ok();
                flushed = true;
            }
            other => {
                eprintln!("read_packet end/error: {other:?}");
                return None;
            }
        }
    }
}

fn extract_rgba(frame: &AVFrame) -> (u32, u32, Vec<u8>) {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let stride = frame.linesize[0] as usize;
    let tight = w * 4;
    let mut out = vec![0u8; tight * h];
    let sptr = frame.data[0];
    // RGBA rows are `stride`-padded in the AVFrame; copy each to a tight `w*4` row.
    unsafe {
        for y in 0..h {
            std::ptr::copy_nonoverlapping(sptr.add(y * stride), out.as_mut_ptr().add(y * tight), tight);
        }
    }
    (w as u32, h as u32, out)
}
