//! Phase-3 verification: run the in-process preview pipeline end-to-end for a few seconds, using the
//! same **two-thread** design as `src/libav.rs` — one thread decodes video → libplacebo (HDR→SDR) →
//! RGBA, another decodes audio → swresample → **cpal output** — and confirm both keep up with
//! wall-clock time (A/V synced and realtime). Audio is actually played, so you can also verify by ear.
//!
//! The bin has no lib target to import (like the other examples), so this re-implements the loops.
//! It prints, twice a second: wall elapsed, the audio master clock, the latest video PTS, and the
//! video frame count. All three of {wall, audio clock, video PTS} should advance together at ~1×.
//!
//!   cargo run --release -p qlipq-desktop --example play_probe -- "<clip>"

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;

use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avfilter::{AVFilter, AVFilterGraph, AVFilterInOut};
use rsmpeg::avformat::AVFormatContextInput;
use rsmpeg::avutil::AVChannelLayout;
use rsmpeg::ffi;
use rsmpeg::swresample::SwrContext;

const RUN_SECS: f64 = 6.0;

struct Probe {
    stop: AtomicBool,
    frames: AtomicU64,
    last_vpts: AtomicU64, // f64 bits
    played: AtomicU64,    // audio samples per channel
    out_rate: AtomicU64,  // f64 bits
}

fn main() {
    let arg = std::env::args().nth(1).expect("usage: play_probe <media path>");
    let probe = Arc::new(Probe {
        stop: AtomicBool::new(false),
        frames: AtomicU64::new(0),
        last_vpts: AtomicU64::new(0),
        played: AtomicU64::new(0),
        out_rate: AtomicU64::new(48_000.0f64.to_bits()),
    });

    let v = { let p = Arc::clone(&probe); let path = arg.clone(); std::thread::spawn(move || video_thread(path, p)) };
    let a = { let p = Arc::clone(&probe); let path = arg.clone(); std::thread::spawn(move || audio_thread(path, p)) };

    let started = Instant::now();
    let mut next = 0.5;
    while started.elapsed().as_secs_f64() < RUN_SECS {
        let elapsed = started.elapsed().as_secs_f64();
        if elapsed >= next {
            let aclock = probe.played.load(Ordering::Relaxed) as f64 / f64::from_bits(probe.out_rate.load(Ordering::Relaxed));
            let vpts = f64::from_bits(probe.last_vpts.load(Ordering::Relaxed));
            let frames = probe.frames.load(Ordering::Relaxed);
            println!("  t={elapsed:5.2}s  audio_clock={aclock:5.2}s  video_pts={vpts:5.2}s  frames={frames}");
            next += 0.5;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    probe.stop.store(true, Ordering::Relaxed);
    let _ = v.join();
    let _ = a.join();

    let aclock = probe.played.load(Ordering::Relaxed) as f64 / f64::from_bits(probe.out_rate.load(Ordering::Relaxed));
    let vpts = f64::from_bits(probe.last_vpts.load(Ordering::Relaxed));
    println!(
        "done: wall={:.2}s  audio_clock={:.2}s  video_pts={:.2}s  frames={}  ({:.1} fps decoded)",
        started.elapsed().as_secs_f64(),
        aclock,
        vpts,
        probe.frames.load(Ordering::Relaxed),
        probe.frames.load(Ordering::Relaxed) as f64 / started.elapsed().as_secs_f64(),
    );
}

fn video_thread(path: String, probe: Arc<Probe>) {
    let cpath = CString::new(path).unwrap();
    let mut input = AVFormatContextInput::open(&cpath).expect("open");
    let vid_idx = input.find_best_stream(ffi::AVMEDIA_TYPE_VIDEO).unwrap().map(|(i, _)| i).expect("no video");
    let (mut vdec, tb_v, sar) = {
        let st = &input.streams()[vid_idx];
        let tb = st.time_base;
        let par = st.codecpar();
        let sar = par.sample_aspect_ratio;
        let sar = if sar.num == 0 { ffi::AVRational { num: 1, den: 1 } } else { sar };
        let codec = AVCodec::find_decoder(par.codec_id).unwrap();
        let mut d = AVCodecContext::new(&codec);
        d.apply_codecpar(&par).unwrap();
        d.set_pkt_timebase(tb);
        unsafe { (*d.as_mut_ptr()).thread_count = 0 }; // auto multithreading (key for realtime)
        d.open(None).unwrap();
        (d, tb, sar)
    };
    let tb_v_secs = tb_v.num as f64 / tb_v.den as f64;
    let (w, h) = {
        let hh = ((vdec.height as f64).min(720.0) as i64 & !1).max(2);
        let ww = ((vdec.width as f64 * hh as f64 / vdec.height as f64).round() as i64 & !1).max(2);
        (ww, hh)
    };
    println!("video out: {w}x{h}  (decoding {RUN_SECS}s, threads=auto)");

    let graph = AVFilterGraph::new();
    let args = CString::new(format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
        vdec.width, vdec.height, vdec.pix_fmt as i32, tb_v.num, tb_v.den, sar.num, sar.den
    ))
    .unwrap();
    let mut src = graph.create_filter_context(&AVFilter::get_by_name(c"buffer").unwrap(), c"in", Some(&args)).unwrap();
    let mut sink = graph.create_filter_context(&AVFilter::get_by_name(c"buffersink").unwrap(), c"out", None).unwrap();
    let outputs = AVFilterInOut::new(c"in", &mut src, 0);
    let inputs = AVFilterInOut::new(c"out", &mut sink, 0);
    let descr = CString::new(format!(
        "libplacebo=w={w}:h={h}:tonemapping=auto:colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=pc,format=rgba"
    ))
    .unwrap();
    graph.parse_ptr(&descr, Some(inputs), Some(outputs)).unwrap();
    graph.config().expect("graph config (libplacebo/Vulkan?)");

    // QLIPQ_NO_FILTER=1 measures pure decode throughput (skip libplacebo) to locate the bottleneck.
    let no_filter = std::env::var("QLIPQ_NO_FILTER").is_ok();
    let mut eof = false;
    while !probe.stop.load(Ordering::Relaxed) {
        if !no_filter {
            while let Ok(out) = sink.buffersink_get_frame(None) {
                probe.frames.fetch_add(1, Ordering::Relaxed);
                probe.last_vpts.store((out.pts as f64 * tb_v_secs).to_bits(), Ordering::Relaxed);
            }
        }
        if eof {
            break;
        }
        match input.read_packet() {
            Ok(Some(pkt)) => {
                if pkt.stream_index == vid_idx as i32 {
                    let _ = vdec.send_packet(Some(&pkt));
                    while let Ok(f) = vdec.receive_frame() {
                        if no_filter {
                            probe.frames.fetch_add(1, Ordering::Relaxed);
                            probe.last_vpts.store((f.pts as f64 * tb_v_secs).to_bits(), Ordering::Relaxed);
                        } else {
                            let _ = src.buffersrc_add_frame(Some(f), None);
                        }
                    }
                }
            }
            Ok(None) => {
                let _ = vdec.send_packet(None);
                while let Ok(f) = vdec.receive_frame() {
                    if no_filter {
                        probe.frames.fetch_add(1, Ordering::Relaxed);
                    } else {
                        let _ = src.buffersrc_add_frame(Some(f), None);
                    }
                }
                let _ = src.buffersrc_add_frame(None, None);
                eof = true;
            }
            Err(_) => break,
        }
    }
}

fn audio_thread(path: String, probe: Arc<Probe>) {
    let cpath = CString::new(path).unwrap();
    let mut input = AVFormatContextInput::open(&cpath).expect("open");
    let Some(aud_idx) = input.find_best_stream(ffi::AVMEDIA_TYPE_AUDIO).ok().flatten().map(|(i, _)| i) else {
        println!("(no audio stream)");
        return;
    };
    let mut adec = {
        let st = &input.streams()[aud_idx];
        let tb = st.time_base;
        let par = st.codecpar();
        let codec = AVCodec::find_decoder(par.codec_id).unwrap();
        let mut d = AVCodecContext::new(&codec);
        d.apply_codecpar(&par).unwrap();
        d.set_pkt_timebase(tb);
        d.open(None).unwrap();
        d
    };

    let host = cpal::default_host();
    let device = host.default_output_device().expect("no output device");
    let supported = device.default_output_config().expect("default config");
    let out_rate = supported.sample_rate();
    let out_ch = supported.channels() as usize;
    probe.out_rate.store((out_rate as f64).to_bits(), Ordering::Relaxed);
    let cap = (out_rate as usize * out_ch / 2).max(out_ch * 2048);
    let (mut producer, mut consumer) = HeapRb::<f32>::new(cap).split();
    let played_cb = Arc::clone(&probe);
    let stream = device
        .build_output_stream::<f32, _, _>(
            supported.config(),
            move |data: &mut [f32], _| {
                let got = consumer.pop_slice(data);
                for s in data[got..].iter_mut() {
                    *s = 0.0;
                }
                played_cb.played.fetch_add((got / out_ch) as u64, Ordering::Relaxed);
            },
            move |e| eprintln!("cpal error: {e}"),
            None,
        )
        .expect("build output stream");
    stream.play().expect("play");
    println!("audio out: {out_rate} Hz, {out_ch} ch");

    let mut swr: Option<SwrContext> = None;
    while !probe.stop.load(Ordering::Relaxed) {
        match input.read_packet() {
            Ok(Some(pkt)) => {
                if pkt.stream_index == aud_idx as i32 {
                    let _ = adec.send_packet(Some(&pkt));
                    while let Ok(frame) = adec.receive_frame() {
                        push_audio(&mut swr, &frame, out_ch, out_rate as i32, &mut producer, &probe);
                    }
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
}

fn push_audio(
    swr: &mut Option<SwrContext>,
    frame: &rsmpeg::avutil::AVFrame,
    out_ch: usize,
    out_rate: i32,
    producer: &mut ringbuf::HeapProd<f32>,
    probe: &Arc<Probe>,
) {
    if swr.is_none() {
        let in_layout = AVChannelLayout::from_nb_channels(frame.ch_layout().nb_channels.max(1));
        let out_layout = AVChannelLayout::from_nb_channels(out_ch as i32);
        let mut s = SwrContext::new(&out_layout, ffi::AV_SAMPLE_FMT_FLT, out_rate, &in_layout, frame.format, frame.sample_rate)
            .expect("swr");
        s.init().expect("swr init");
        *swr = Some(s);
    }
    let s = swr.as_mut().unwrap();
    let out_count = s.get_out_samples(frame.nb_samples);
    if out_count <= 0 {
        return;
    }
    let mut buf = vec![0f32; out_count as usize * out_ch];
    let mut out_ptr = buf.as_mut_ptr() as *mut u8;
    let in_ptr = frame.data.as_ptr() as *const *const u8;
    let got = match unsafe { s.convert(&mut out_ptr, out_count, in_ptr, frame.nb_samples) } {
        Ok(n) => n as usize,
        Err(_) => return,
    };
    let n = got * out_ch;
    let mut off = 0;
    while off < n {
        off += producer.push_slice(&buf[off..n]);
        if off < n {
            if probe.stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(3));
        }
    }
}
