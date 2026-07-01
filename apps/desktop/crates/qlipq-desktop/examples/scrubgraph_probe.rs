//! Step-1 prototype gate (media-stack plan): does **reusing a warm libplacebo filter graph** across
//! scrubs recover the ~380 ms/scrub the graph rebuild costs for HDR? `scrub_probe` showed the warm
//! *decoder* gives ~0 win, and that ~50% of an HDR scrub is rebuilding the libplacebo/Vulkan graph.
//! This builds that graph **once** and feeds each seek's target frame through the same graph, vs the
//! current path that rebuilds the graph every scrub — and times both end-to-end.
//!
//! libplacebo's avfilter is 1-in-1-out, but peak detection can hold a frame, so the target frame is
//! flushed by feeding the following decoded frames until the first output appears (that first output
//! corresponds to the target). The graph is reused, so frames get monotonically-rewritten PTS to keep
//! buffersrc happy across backward seeks; the realized image is unaffected.
//!
//!   cargo run --release -p qlipq-desktop --example scrubgraph_probe -- "<hdr clip>"

use std::ffi::CString;
use std::time::Instant;

use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avfilter::{AVFilter, AVFilterContextMut, AVFilterGraph, AVFilterInOut};
use rsmpeg::avformat::AVFormatContextInput;
use rsmpeg::avutil::AVFrame;
use rsmpeg::error::RsmpegError;
use rsmpeg::ffi;

const TARGETS: &[f64] = &[2.0, 6.0, 1.0, 4.0, 0.5, 8.0, 3.0, 5.0];

struct Decoder {
    input: AVFormatContextInput,
    vdec: AVCodecContext,
    vid_idx: i32,
    tb: ffi::AVRational,
    sar: ffi::AVRational,
}

fn open(path: &str) -> Decoder {
    let cpath = CString::new(path).unwrap();
    let input = AVFormatContextInput::open(&cpath).unwrap();
    let vid_idx = input.find_best_stream(ffi::AVMEDIA_TYPE_VIDEO).unwrap().map(|(i, _)| i).expect("no video");
    let (vdec, tb, sar) = {
        let st = &input.streams()[vid_idx];
        let tb = st.time_base;
        let par = st.codecpar();
        let sar0 = par.sample_aspect_ratio;
        let sar = if sar0.num == 0 { ffi::AVRational { num: 1, den: 1 } } else { sar0 };
        let codec = AVCodec::find_decoder(par.codec_id).unwrap();
        let mut d = AVCodecContext::new(&codec);
        d.apply_codecpar(&par).unwrap();
        d.set_pkt_timebase(tb);
        unsafe { (*d.as_mut_ptr()).thread_count = 0 };
        d.open(None).unwrap();
        (d, tb, sar)
    };
    Decoder { input, vdec, vid_idx: vid_idx as i32, tb, sar }
}

fn build_graph<'g>(
    graph: &'g AVFilterGraph,
    d: &Decoder,
    w: u32,
    h: u32,
    is_hdr: bool,
) -> (AVFilterContextMut<'g>, AVFilterContextMut<'g>) {
    let args = CString::new(format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
        d.vdec.width, d.vdec.height, d.vdec.pix_fmt as i32, d.tb.num, d.tb.den, d.sar.num, d.sar.den
    ))
    .unwrap();
    let mut src = graph.create_filter_context(&AVFilter::get_by_name(c"buffer").unwrap(), c"in", Some(&args)).unwrap();
    let mut sink = graph.create_filter_context(&AVFilter::get_by_name(c"buffersink").unwrap(), c"out", None).unwrap();
    let outputs = AVFilterInOut::new(c"in", &mut src, 0);
    let inputs = AVFilterInOut::new(c"out", &mut sink, 0);
    let descr = if is_hdr {
        CString::new(format!(
            "libplacebo=w={w}:h={h}:tonemapping=auto:colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=pc,format=rgba"
        ))
    } else {
        CString::new(format!("scale={w}:{h}:flags=bilinear,format=rgba"))
    }
    .unwrap();
    graph.parse_ptr(&descr, Some(inputs), Some(outputs)).unwrap();
    graph.config().expect("graph config");
    (src, sink)
}

/// Seek + decode the first frame whose PTS reaches `target` (discarding earlier frames).
fn decode_target(d: &mut Decoder, target: f64) -> Option<AVFrame> {
    let tb_secs = d.tb.num as f64 / d.tb.den as f64;
    if d.tb.num != 0 {
        let ts = (target / tb_secs) as i64;
        let _ = d.input.seek(d.vid_idx, ts, ffi::AVSEEK_FLAG_BACKWARD as i32);
        d.vdec.flush_buffers();
    }
    let mut flushed = false;
    loop {
        match d.vdec.receive_frame() {
            Ok(f) => {
                if f.pts as f64 * tb_secs + 1e-3 >= target {
                    return Some(f);
                }
                continue; // before target — keep decoding
            }
            Err(RsmpegError::DecoderDrainError) => {}
            Err(_) => return None,
        }
        match d.input.read_packet() {
            Ok(Some(pkt)) => {
                if pkt.stream_index == d.vid_idx {
                    let _ = d.vdec.send_packet(Some(&pkt));
                }
            }
            Ok(None) if !flushed => {
                let _ = d.vdec.send_packet(None);
                flushed = true;
            }
            _ => return None,
        }
    }
}

/// Decode the next frame in sequence (used to flush libplacebo's 1-frame latency).
fn decode_next(d: &mut Decoder) -> Option<AVFrame> {
    loop {
        match d.vdec.receive_frame() {
            Ok(f) => return Some(f),
            Err(RsmpegError::DecoderDrainError) => {}
            Err(_) => return None,
        }
        match d.input.read_packet() {
            Ok(Some(pkt)) => {
                if pkt.stream_index == d.vid_idx {
                    let _ = d.vdec.send_packet(Some(&pkt));
                }
            }
            _ => return None,
        }
    }
}

/// Push `first` (the target frame) into the graph, then feed following frames until the graph emits
/// its first output — that output is the target frame. `mono` keeps buffersrc PTS monotonic across
/// reuse. Returns the realized RGBA byte count.
fn filter_target(d: &mut Decoder, src: &mut AVFilterContextMut, sink: &mut AVFilterContextMut, first: AVFrame, mono: &mut i64) -> Option<usize> {
    // Drain any stale output left from a previous scrub's flush-pushes.
    while sink.buffersink_get_frame(None).is_ok() {}

    let mut frame = Some(first);
    let mut pushes = 0;
    loop {
        if let Some(mut f) = frame.take() {
            unsafe { (*f.as_mut_ptr()).pts = *mono };
            *mono += 1;
            let _ = src.buffersrc_add_frame(Some(f), None);
            pushes += 1;
        }
        match sink.buffersink_get_frame(None) {
            Ok(out) => return Some((out.width as usize) * (out.height as usize) * 4),
            Err(RsmpegError::BufferSinkDrainError) => {}
            Err(_) => return None,
        }
        if pushes > 8 {
            return None; // gave up flushing
        }
        frame = decode_next(d); // feed one more to flush latency
        if frame.is_none() {
            return None;
        }
    }
}

fn placebo_dims(src_w: i64, src_h: i64) -> (u32, u32) {
    let sw = src_w.max(2) as f64;
    let sh = src_h.max(2) as f64;
    let h = ((sh.min(720.0).round() as i64) & !1).max(2);
    let w = (((sw * h as f64 / sh).round() as i64) & !1).max(2);
    (w as u32, h as u32)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: scrubgraph_probe <hdr clip>");
    let probe = open(&path);
    let (src_w, src_h) = (probe.vdec.width as i64, probe.vdec.height as i64);
    let is_hdr = {
        let st = &probe.input.streams()[probe.vid_idx as usize];
        let trc = st.codecpar().color_trc;
        trc == ffi::AVCOL_TRC_SMPTE2084 || trc == ffi::AVCOL_TRC_ARIB_STD_B67
    };
    let dims = placebo_dims(src_w, src_h);
    println!("clip: {src_w}x{src_h} -> {}x{} preview, {}\n", dims.0, dims.1, if is_hdr { "HDR (libplacebo)" } else { "SDR (scale)" });
    drop(probe);

    // --- FRESH: rebuild the graph every scrub (current behaviour) ---
    let mut fresh = open(&path);
    let mut fresh_total = 0.0;
    let mut mono = 0i64;
    println!("fresh graph (rebuilt per scrub):");
    for &t in TARGETS {
        let started = Instant::now();
        let frame = decode_target(&mut fresh, t);
        let ok = if let Some(f) = frame {
            let graph = AVFilterGraph::new();
            let (mut src, mut sink) = build_graph(&graph, &fresh, dims.0, dims.1, is_hdr);
            filter_target(&mut fresh, &mut src, &mut sink, f, &mut mono).is_some()
        } else {
            false
        };
        let ms = started.elapsed().as_secs_f64() * 1e3;
        fresh_total += ms;
        println!("  seek {t:>4.1}s  {ms:7.1} ms  {}", if ok { "ok" } else { "MISS" });
    }
    drop(fresh);

    // --- WARM: build the graph once, reuse it for every scrub ---
    let mut warm = open(&path);
    let graph = AVFilterGraph::new();
    let build0 = Instant::now();
    let (mut src, mut sink) = build_graph(&graph, &warm, dims.0, dims.1, is_hdr);
    let build_ms = build0.elapsed().as_secs_f64() * 1e3;
    let mut warm_total = 0.0;
    let mut mono2 = 0i64;
    println!("\nwarm graph (built once in {build_ms:.1} ms, then reused):");
    for &t in TARGETS {
        let started = Instant::now();
        let frame = decode_target(&mut warm, t);
        let ok = if let Some(f) = frame {
            filter_target(&mut warm, &mut src, &mut sink, f, &mut mono2).is_some()
        } else {
            false
        };
        let ms = started.elapsed().as_secs_f64() * 1e3;
        warm_total += ms;
        println!("  seek {t:>4.1}s  {ms:7.1} ms  {}", if ok { "ok" } else { "MISS" });
    }

    let n = TARGETS.len() as f64;
    println!(
        "\navg: fresh {:.1} ms   warm {:.1} ms   speedup {:.2}x   (one-time warm graph build {:.0} ms)",
        fresh_total / n,
        warm_total / n,
        fresh_total / warm_total.max(1e-9),
        build_ms,
    );
}
