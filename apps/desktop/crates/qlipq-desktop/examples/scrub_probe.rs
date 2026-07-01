//! Step-1 verification (media-stack plan): measure scrub latency of the **warm** single-frame decoder
//! (reuse the open demuxer + decoder across scrubs — what the new `libav::ScrubDecoder` does) against
//! the **cold** path (reopen the file + rebuild the decoder every frame — the old `extract_frame`).
//! Prints per-seek and average latency for both; warm should be clearly faster because it skips the
//! container open + decoder init on each scrub.
//!
//! Examples can't import the binary crate, so this re-implements both loops (same pattern as
//! `play_probe`/`hdr_probe`). It decodes through the same libplacebo (HDR) / scale (SDR) pipeline and
//! packs RGBA, so the measured work matches the real scrub.
//!
//!   cargo run --release -p qlipq-desktop --example scrub_probe -- "<clip>"

use std::ffi::CString;
use std::time::Instant;

use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avfilter::{AVFilter, AVFilterGraph, AVFilterInOut};
use rsmpeg::avformat::AVFormatContextInput;
use rsmpeg::avutil::AVFrame;
use rsmpeg::error::RsmpegError;
use rsmpeg::ffi;

/// Seek targets (seconds): a mix of forward jumps and backward jumps, like real scrubbing.
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
    let input = AVFormatContextInput::open(&cpath).expect("open");
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

/// Seek + decode + filter to `target`. Returns `(rgba_bytes, graph_ms, decode_ms)` so the caller can
/// see where the time goes: graph build (libplacebo/Vulkan init for HDR) vs keyframe→target decode.
fn decode_at(d: &mut Decoder, dims: (u32, u32), is_hdr: bool, target: f64) -> Option<(usize, f64, f64)> {
    let (w, h) = dims;
    if d.tb.num != 0 {
        let ts = (target * d.tb.den as f64 / d.tb.num as f64) as i64;
        let _ = d.input.seek(d.vid_idx, ts, ffi::AVSEEK_FLAG_BACKWARD as i32);
        d.vdec.flush_buffers();
    }
    let graph_start = Instant::now();
    let graph = AVFilterGraph::new();
    let args = CString::new(format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
        d.vdec.width, d.vdec.height, d.vdec.pix_fmt as i32, d.tb.num, d.tb.den, d.sar.num, d.sar.den
    ))
    .unwrap();
    let mut src = graph.create_filter_context(&AVFilter::get_by_name(c"buffer").unwrap(), c"in", Some(&args)).ok()?;
    let mut sink = graph.create_filter_context(&AVFilter::get_by_name(c"buffersink").unwrap(), c"out", None).ok()?;
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
    graph.parse_ptr(&descr, Some(inputs), Some(outputs)).ok()?;
    graph.config().ok()?;
    let graph_ms = graph_start.elapsed().as_secs_f64() * 1e3;

    let decode_start = Instant::now();
    let tb_secs = d.tb.num as f64 / d.tb.den as f64;
    let mut flushed = false;
    loop {
        match sink.buffersink_get_frame(None) {
            Ok(out) => {
                let pts = out.pts as f64 * tb_secs;
                if pts + 1e-3 < target && !flushed {
                    continue;
                }
                return Some((pack_rgba(&out), graph_ms, decode_start.elapsed().as_secs_f64() * 1e3));
            }
            Err(RsmpegError::BufferSinkDrainError) => {}
            Err(_) => return None,
        }
        match d.vdec.receive_frame() {
            Ok(frame) => {
                let _ = src.buffersrc_add_frame(Some(frame), None);
                continue;
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
                let _ = src.buffersrc_add_frame(None, None);
                flushed = true;
            }
            _ => return None,
        }
    }
}

fn pack_rgba(frame: &AVFrame) -> usize {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let stride = frame.linesize[0] as usize;
    let tight = w * 4;
    let mut out = vec![0u8; tight * h];
    let sptr = frame.data[0];
    unsafe {
        for y in 0..h {
            std::ptr::copy_nonoverlapping(sptr.add(y * stride), out.as_mut_ptr().add(y * tight), tight);
        }
    }
    out.len()
}

fn placebo_dims(src_w: i64, src_h: i64) -> (u32, u32) {
    let sw = src_w.max(2) as f64;
    let sh = src_h.max(2) as f64;
    let h = ((sh.min(720.0).round() as i64) & !1).max(2);
    let w = (((sw * h as f64 / sh).round() as i64) & !1).max(2);
    (w as u32, h as u32)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: scrub_probe <media path>");

    // Detect HDR + source dimensions from the first decoder.
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

    // --- WARM: open once, seek+decode each target reusing the decoder ---
    let mut warm = open(&path);
    let (mut warm_total, mut warm_graph, mut warm_decode) = (0.0, 0.0, 0.0);
    println!("warm (reuse demuxer + decoder)        total =  graph + decode:");
    for &t in TARGETS {
        let started = Instant::now();
        let r = decode_at(&mut warm, dims, is_hdr, t);
        let ms = started.elapsed().as_secs_f64() * 1e3;
        warm_total += ms;
        let (g, d) = r.map(|(_, g, d)| (g, d)).unwrap_or((0.0, 0.0));
        warm_graph += g;
        warm_decode += d;
        println!("  seek {t:>4.1}s  {ms:7.1} ms = {g:6.1} + {d:6.1}");
    }
    drop(warm);

    // --- COLD: reopen + rebuild the decoder for each target (the old per-frame path) ---
    let mut cold_total = 0.0;
    println!("\ncold (reopen file + rebuild decoder each scrub):");
    for &t in TARGETS {
        let started = Instant::now();
        let mut d = open(&path);
        let ok = decode_at(&mut d, dims, is_hdr, t).is_some();
        let ms = started.elapsed().as_secs_f64() * 1e3;
        cold_total += ms;
        println!("  seek {t:>4.1}s  {ms:7.1} ms  {}", if ok { "ok" } else { "MISS" });
    }

    let n = TARGETS.len() as f64;
    println!(
        "\navg warm {:.1} ms (graph {:.1} + decode {:.1})   cold {:.1} ms   speedup {:.2}x",
        warm_total / n,
        warm_graph / n,
        warm_decode / n,
        cold_total / n,
        cold_total / warm_total.max(1e-9),
    );
    println!("note: 'graph' = filter build incl. libplacebo/Vulkan init (HDR); 'decode' = keyframe->target.");
}
