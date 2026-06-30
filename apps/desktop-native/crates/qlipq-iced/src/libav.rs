//! In-process libav preview player (feature `libav-preview`).
//!
//! This replaces the CLI ffmpeg preview ([`crate::host::Player`]) with a decoder that runs *inside*
//! the process, so it can use **libplacebo** for VLC-quality HDR→SDR tonemapping and play **synced
//! audio** — neither of which the per-frame CLI path can do. It mirrors `host::Player`'s interface
//! (`poll`/`dimensions`/`fps`) so [`crate::main`] is feature-agnostic, and adds `position()` (an
//! authoritative master clock) plus `try_seek()` (a seek command channel — no decoder re-init).
//!
//! Threading: **audio and video decode on separate threads**, each with its own demuxer (the file is
//! opened twice). This is deliberate — **audio is the master clock**, so it must keep its output
//! buffer full regardless of how slow HDR video filtering is; coupling them on one thread let a slow
//! libplacebo frame starve the audio and stutter it. The audio thread decodes → resamples → feeds a
//! lock-free ring buffer drained by cpal (which counts samples played into the clock); the video
//! thread decodes → libplacebo/scale → RGBA into a small queue. The UI presents the video frame whose
//! PTS is due against the master clock (a wall clock when the clip has no audio). Export is unaffected
//! — it stays on the parity-tested CLI arg-vector; only the preview decodes in-process.

use std::collections::VecDeque;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapProd, HeapRb};

use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avfilter::{AVFilter, AVFilterContextMut, AVFilterGraph, AVFilterInOut};
use rsmpeg::avformat::AVFormatContextInput;
use rsmpeg::avutil::{AVChannelLayout, AVFrame};
use rsmpeg::error::RsmpegError;
use rsmpeg::ffi;
use rsmpeg::swresample::SwrContext;

pub use crate::host::FramePoll;

/// How many decoded video frames the video thread may run ahead before it blocks. Presentation
/// drains the queue at the master-clock rate, so this caps lookahead (≈0.3 s) and paces decoding.
const VIDEO_LOOKAHEAD: usize = 12;

// ---- master clock ----

/// The playback clock. With audio it advances by the number of samples the output device has
/// actually consumed (so it pauses naturally during underruns/pre-roll); without audio it is a plain
/// wall clock anchored to the first presented frame.
struct Clock {
    use_audio: AtomicBool,
    /// Position (seconds) the current segment started at; the clock is relative to this.
    base: Mutex<f64>,
    /// Samples-per-channel the cpal callback has played since the segment (re)started.
    played: AtomicU64,
    /// Output sample rate (Hz) as f64 bits, set when the audio output is built.
    rate: AtomicU64,
    /// `(anchor_instant, anchor_pts)` for the wall clock (no-audio clips, or after audio ends).
    wall: Mutex<Option<(Instant, f64)>>,
}

impl Clock {
    fn now(&self) -> f64 {
        if self.use_audio.load(Ordering::Relaxed) {
            let rate = f64::from_bits(self.rate.load(Ordering::Relaxed));
            let base = *self.base.lock().unwrap();
            if rate > 0.0 {
                base + self.played.load(Ordering::Relaxed) as f64 / rate
            } else {
                base
            }
        } else {
            match *self.wall.lock().unwrap() {
                Some((inst, pts)) => pts + inst.elapsed().as_secs_f64(),
                None => *self.base.lock().unwrap(),
            }
        }
    }
}

struct Shared {
    /// Decoded RGBA frames awaiting presentation, tagged with their PTS in seconds.
    video: Mutex<VecDeque<(f64, Vec<u8>)>>,
    /// Set when the video decoder reaches EOF (or dies); playback ends once the queue drains.
    ended: AtomicBool,
    clock: Clock,
}

enum Command {
    Seek(f64),
}

/// What ended a decode segment, so a thread knows whether to restart (seek) or exit.
enum SegEnd {
    Eof,
    Seek(f64),
    Stopped,
}

/// A warm in-process decoder feeding [`crate::video::SharedFrame`]. Interface-compatible with
/// `host::Player` (`poll`/`dimensions`/`fps`) plus `position()`/`try_seek()`.
pub struct Player {
    shared: Arc<Shared>,
    video_cmd: Option<Sender<Command>>,
    audio_cmd: Option<Sender<Command>>,
    video_thread: Option<JoinHandle<()>>,
    audio_thread: Option<JoinHandle<()>>,
    width: u32,
    height: u32,
    fps: f64,
}

impl Player {
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    /// The master clock position in seconds (audio clock, or wall clock when there is no audio).
    /// `Some` tells the caller this is authoritative — unlike the CLI player, which advances by 1/fps.
    pub fn position(&self) -> Option<f64> {
        Some(self.shared.clock.now())
    }

    /// Non-blocking: present the newest video frame that is due at the current clock, dropping any
    /// earlier frames we fell behind on. Returns [`FramePoll::Ended`] once the decoder is done and
    /// the queue has drained.
    pub fn poll(&self) -> FramePoll {
        let clock = self.shared.clock.now();
        let mut q = self.shared.video.lock().unwrap();
        let mut chosen = None;
        while let Some((pts, _)) = q.front() {
            if *pts <= clock + 1e-3 {
                chosen = q.pop_front().map(|(_, rgba)| rgba);
            } else {
                break;
            }
        }
        let empty = q.is_empty();
        drop(q);
        match chosen {
            Some(rgba) => FramePoll::Frame(rgba),
            None if empty && self.shared.ended.load(Ordering::Relaxed) => FramePoll::Ended,
            None => FramePoll::Empty,
        }
    }

    /// Seek both decode threads to `sec` without re-opening files or rebuilding the Vulkan/libplacebo
    /// graph. Returns `true` (the command was queued); the CLI player returns `false` here so the
    /// caller knows to fall back to a full restart.
    pub fn try_seek(&self, sec: f64) -> bool {
        let sec = sec.max(0.0);
        if let Some(tx) = &self.audio_cmd {
            let _ = tx.send(Command::Seek(sec));
        }
        match &self.video_cmd {
            Some(tx) => tx.send(Command::Seek(sec)).is_ok(),
            None => false,
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Dropping the senders disconnects the command channels; the decode threads see that and exit.
        self.video_cmd.take();
        self.audio_cmd.take();
        if let Some(handle) = self.video_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.audio_thread.take() {
            let _ = handle.join();
        }
    }
}

/// Start the in-process player from `start_sec`. Signature matches `host::start_player` (the
/// `ffmpeg_path` is unused — decoding is in-process). Returns `None` if the file/decoder can't open,
/// so the caller falls back to a single-frame preview.
#[allow(clippy::too_many_arguments)]
pub fn start_player(
    path: &str,
    _ffmpeg_path: &str,
    start_sec: f64,
    src_w: i64,
    src_h: i64,
    src_fps: f64,
    is_hdr: bool,
) -> Option<Player> {
    let cpath = CString::new(path).ok()?;
    let input = AVFormatContextInput::open(&cpath).ok()?;

    let vid_idx = input.find_best_stream(ffi::AVMEDIA_TYPE_VIDEO).ok()?.map(|(i, _)| i)?;
    let (vdec, tb_v, sar) = build_video_decoder(&input, vid_idx)?;
    let has_audio = input.find_best_stream(ffi::AVMEDIA_TYPE_AUDIO).ok().flatten().is_some();

    let dims = placebo_dims(src_w, src_h);
    let fps = if src_fps.is_finite() && src_fps > 0.0 { src_fps.min(60.0) } else { 30.0 };

    let shared = Arc::new(Shared {
        video: Mutex::new(VecDeque::new()),
        ended: AtomicBool::new(false),
        clock: Clock {
            use_audio: AtomicBool::new(has_audio),
            base: Mutex::new(start_sec),
            played: AtomicU64::new(0),
            rate: AtomicU64::new(0.0f64.to_bits()),
            wall: Mutex::new(None),
        },
    });

    let (video_cmd, video_rx) = channel::<Command>();
    let shared_v = Arc::clone(&shared);
    let vid_idx = vid_idx as i32;
    let video_thread = std::thread::spawn(move || {
        video_loop(input, vdec, vid_idx, tb_v, sar, dims, is_hdr, !has_audio, start_sec, shared_v, video_rx);
    });

    let (audio_cmd, audio_thread) = if has_audio {
        let (tx, rx) = channel::<Command>();
        let shared_a = Arc::clone(&shared);
        let path = path.to_string();
        let handle = std::thread::spawn(move || {
            audio_loop(path, start_sec, shared_a, rx);
        });
        (Some(tx), Some(handle))
    } else {
        (None, None)
    };

    Some(Player {
        shared,
        video_cmd: Some(video_cmd),
        audio_cmd,
        video_thread: Some(video_thread),
        audio_thread,
        width: dims.0,
        height: dims.1,
        fps,
    })
}

/// Extract a single preview frame at `sec` (scrubbing / paused), in-process through the same
/// libplacebo/scale pipeline as playback so the color matches. Signature matches `host::extract_frame`.
#[allow(clippy::too_many_arguments)]
pub fn extract_frame(
    path: &str,
    _ffmpeg_path: &str,
    sec: f64,
    src_w: i64,
    src_h: i64,
    is_hdr: bool,
) -> Result<(u32, u32, Vec<u8>), String> {
    let (w, h) = placebo_dims(src_w, src_h);
    let cpath = CString::new(path).map_err(|e| e.to_string())?;
    let mut input = AVFormatContextInput::open(&cpath).map_err(|e| format!("open failed: {e:?}"))?;

    let vid_idx = input
        .find_best_stream(ffi::AVMEDIA_TYPE_VIDEO)
        .map_err(|e| format!("{e:?}"))?
        .map(|(i, _)| i)
        .ok_or("no video stream")?;
    let (mut vdec, tb_v, sar) = build_video_decoder(&input, vid_idx).ok_or("decoder init failed")?;

    let target = sec.max(0.0);
    if tb_v.num != 0 {
        let ts = (target * tb_v.den as f64 / tb_v.num as f64) as i64;
        let _ = input.seek(vid_idx as i32, ts, ffi::AVSEEK_FLAG_BACKWARD as i32);
        vdec.flush_buffers();
    }

    let graph = AVFilterGraph::new();
    let (mut src, mut sink) = build_video_filter(&graph, &vdec, tb_v, sar, w, h, is_hdr)
        .map_err(|e| format!("filter graph init failed: {e}"))?;

    let tb_v_secs = rational_secs(tb_v);
    let mut flushed = false;
    let vid_idx = vid_idx as i32;
    loop {
        // Pull any filtered frame; return the first whose PTS reaches the target.
        match sink.buffersink_get_frame(None) {
            Ok(out) => {
                let pts = out.pts as f64 * tb_v_secs;
                if pts + 1e-3 < target && !flushed {
                    continue;
                }
                return Ok((w, h, frame_to_rgba(&out)));
            }
            Err(RsmpegError::BufferSinkDrainError) => {}
            Err(RsmpegError::BufferSinkEofError) => return Err("no frame decoded".into()),
            Err(e) => return Err(format!("buffersink: {e:?}")),
        }
        match vdec.receive_frame() {
            Ok(decoded) => {
                let _ = src.buffersrc_add_frame(Some(decoded), None);
                continue;
            }
            Err(RsmpegError::DecoderDrainError) => {}
            Err(RsmpegError::DecoderFlushedError) => return Err("no frame decoded".into()),
            Err(e) => return Err(format!("decode: {e:?}")),
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
            _ => return Err("no frame decoded".into()),
        }
    }
}

// ---- video thread ----

#[allow(clippy::too_many_arguments)]
fn video_loop(
    mut input: AVFormatContextInput,
    mut vdec: AVCodecContext,
    vid_idx: i32,
    tb_v: ffi::AVRational,
    sar: ffi::AVRational,
    dims: (u32, u32),
    is_hdr: bool,
    manages_clock: bool,
    start_sec: f64,
    shared: Arc<Shared>,
    cmd_rx: Receiver<Command>,
) {
    let mut start = start_sec;
    loop {
        match video_segment(&mut input, &mut vdec, vid_idx, tb_v, sar, dims, is_hdr, manages_clock, start, &shared, &cmd_rx) {
            SegEnd::Seek(t) => start = t,
            SegEnd::Eof | SegEnd::Stopped => break,
        }
    }
}

/// Decode video for one continuous segment from `start` until EOF / seek / drop. Builds a fresh
/// filter graph per segment, which cleanly resets libplacebo state on every seek.
#[allow(clippy::too_many_arguments)]
fn video_segment(
    input: &mut AVFormatContextInput,
    vdec: &mut AVCodecContext,
    vid_idx: i32,
    tb_v: ffi::AVRational,
    sar: ffi::AVRational,
    dims: (u32, u32),
    is_hdr: bool,
    manages_clock: bool,
    start: f64,
    shared: &Arc<Shared>,
    cmd_rx: &Receiver<Command>,
) -> SegEnd {
    let (w, h) = dims;
    let tb_v_secs = rational_secs(tb_v);

    shared.ended.store(false, Ordering::Relaxed);
    if tb_v.num != 0 {
        let ts = (start * tb_v.den as f64 / tb_v.num as f64) as i64;
        let _ = input.seek(vid_idx, ts, ffi::AVSEEK_FLAG_BACKWARD as i32);
    }
    vdec.flush_buffers();

    let graph = AVFilterGraph::new();
    let (mut src, mut sink) = match build_video_filter(&graph, vdec, tb_v, sar, w, h, is_hdr) {
        Ok(pair) => pair,
        Err(_) => return SegEnd::Eof,
    };

    shared.video.lock().unwrap().clear();
    if manages_clock {
        // No audio thread → the video thread owns the (wall) clock.
        shared.clock.use_audio.store(false, Ordering::Relaxed);
        *shared.clock.base.lock().unwrap() = start;
        *shared.clock.wall.lock().unwrap() = None;
    }

    let mut first_video = false;
    let mut seeking = true;

    loop {
        if let Some(end) = poll_cmd(cmd_rx) {
            return end;
        }
        // Block before reading more once we're a comfortable lookahead ahead; presentation drains the
        // queue at the master-clock rate, so this paces decoding without dropping frames.
        while shared.video.lock().unwrap().len() >= VIDEO_LOOKAHEAD {
            if let Some(end) = poll_cmd(cmd_rx) {
                return end;
            }
            std::thread::sleep(Duration::from_millis(3));
        }

        match input.read_packet() {
            Ok(Some(pkt)) => {
                if pkt.stream_index == vid_idx {
                    let _ = vdec.send_packet(Some(&pkt));
                    feed_video(vdec, &mut src);
                    pull_video(&mut sink, shared, tb_v_secs, start, &mut seeking, &mut first_video, manages_clock);
                }
            }
            Ok(None) => {
                let _ = vdec.send_packet(None);
                feed_video(vdec, &mut src);
                let _ = src.buffersrc_add_frame(None, None);
                pull_video(&mut sink, shared, tb_v_secs, start, &mut seeking, &mut first_video, manages_clock);
                shared.ended.store(true, Ordering::Relaxed);
                return SegEnd::Eof;
            }
            Err(_) => {
                shared.ended.store(true, Ordering::Relaxed);
                return SegEnd::Eof;
            }
        }
    }
}

/// Push every newly decoded video frame into the filter graph's source.
fn feed_video(vdec: &mut AVCodecContext, src: &mut AVFilterContextMut) {
    loop {
        match vdec.receive_frame() {
            Ok(frame) => {
                let _ = src.buffersrc_add_frame(Some(frame), None);
            }
            Err(_) => break, // drain / flushed / error — nothing more to pull right now
        }
    }
}

/// Pull all ready filtered frames and enqueue them (dropping pre-seek frames before `start`).
#[allow(clippy::too_many_arguments)]
fn pull_video(
    sink: &mut AVFilterContextMut,
    shared: &Arc<Shared>,
    tb_v_secs: f64,
    start: f64,
    seeking: &mut bool,
    first: &mut bool,
    manages_clock: bool,
) {
    loop {
        match sink.buffersink_get_frame(None) {
            Ok(out) => {
                let pts = (out.pts as f64 * tb_v_secs).max(start);
                if *seeking && pts + 1e-3 < start {
                    continue; // a frame from before the seek target — discard
                }
                *seeking = false;
                let rgba = frame_to_rgba(&out);
                shared.video.lock().unwrap().push_back((pts, rgba));
                if !*first {
                    *first = true;
                    if manages_clock {
                        *shared.clock.wall.lock().unwrap() = Some((Instant::now(), pts));
                    }
                }
            }
            Err(_) => break,
        }
    }
}

// ---- audio thread ----

fn audio_loop(path: String, start_sec: f64, shared: Arc<Shared>, cmd_rx: Receiver<Command>) {
    let Ok(cpath) = CString::new(path) else { return };
    let Ok(mut input) = AVFormatContextInput::open(&cpath) else { return };
    let Some(aud_idx) = input.find_best_stream(ffi::AVMEDIA_TYPE_AUDIO).ok().flatten().map(|(i, _)| i) else {
        return;
    };
    let Some((mut adec, tb_a)) = build_audio_decoder(&input, aud_idx) else { return };
    let aud_idx = aud_idx as i32;

    let host = cpal::default_host();
    let device = host.default_output_device();
    let mut start = start_sec;
    loop {
        match audio_segment(&mut input, &mut adec, aud_idx, tb_a, start, &shared, device.as_ref(), &cmd_rx) {
            SegEnd::Seek(t) => start = t,
            SegEnd::Eof | SegEnd::Stopped => break,
        }
    }
}

/// Decode audio for one segment: seek, build a fresh cpal output + ring buffer (so stale audio never
/// survives a seek), then resample and feed it until EOF / seek / drop. Owns the master clock.
#[allow(clippy::too_many_arguments)]
fn audio_segment(
    input: &mut AVFormatContextInput,
    adec: &mut AVCodecContext,
    aud_idx: i32,
    tb_a: ffi::AVRational,
    start: f64,
    shared: &Arc<Shared>,
    device: Option<&cpal::Device>,
    cmd_rx: &Receiver<Command>,
) -> SegEnd {
    let tb_a_secs = rational_secs(tb_a);

    if tb_a.num != 0 {
        let ts = (start * tb_a.den as f64 / tb_a.num as f64) as i64;
        let _ = input.seek(aud_idx, ts, ffi::AVSEEK_FLAG_BACKWARD as i32);
    }
    adec.flush_buffers();

    let mut audio = device.and_then(build_audio_out);
    let use_audio = audio.is_some();
    let out_ch = audio.as_ref().map(|a| a.out_channels).unwrap_or(0);
    let out_rate = audio.as_ref().map(|a| a.out_rate as i32).unwrap_or(48_000);

    // Reset the master clock for this segment (the audio thread owns it when audio is playing).
    shared.clock.use_audio.store(use_audio, Ordering::Relaxed);
    *shared.clock.base.lock().unwrap() = start;
    shared.clock.played.store(0, Ordering::Relaxed);
    shared.clock.rate.store((out_rate as f64).to_bits(), Ordering::Relaxed);
    let played = audio.as_ref().map(|a| Arc::clone(&a.played));

    let mut swr: Option<SwrContext> = None;
    let mut seeking = true;

    loop {
        if let Some(end) = poll_cmd(cmd_rx) {
            return end;
        }
        match input.read_packet() {
            Ok(Some(pkt)) => {
                if pkt.stream_index == aud_idx {
                    let _ = adec.send_packet(Some(&pkt));
                    if let Some(end) =
                        drain_audio(adec, &mut swr, &mut audio, out_ch, out_rate, tb_a_secs, start, &mut seeking, cmd_rx)
                    {
                        return end;
                    }
                }
            }
            Ok(None) => {
                let _ = adec.send_packet(None);
                if let Some(end) =
                    drain_audio(adec, &mut swr, &mut audio, out_ch, out_rate, tb_a_secs, start, &mut seeking, cmd_rx)
                {
                    return end;
                }
                // Keep the device alive until its buffer drains (so the last ~0.5 s isn't cut off),
                // then hand the clock to wall time so any longer video tail keeps playing smoothly.
                loop {
                    if let Some(end) = poll_cmd(cmd_rx) {
                        return end;
                    }
                    if audio.as_ref().map(|a| a.producer.is_empty()).unwrap_or(true) {
                        if use_audio {
                            let pos = match &played {
                                Some(p) => start + p.load(Ordering::Relaxed) as f64 / out_rate as f64,
                                None => start,
                            };
                            *shared.clock.wall.lock().unwrap() = Some((Instant::now(), pos));
                            shared.clock.use_audio.store(false, Ordering::Relaxed);
                        }
                        return SegEnd::Eof;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Err(_) => return SegEnd::Eof,
        }
    }
}

/// Pull decoded audio frames, resample to the device format, and push them into the ring buffer
/// (with backpressure). Returns `Some(SegEnd)` if a seek/stop interrupts the (blocking) push.
#[allow(clippy::too_many_arguments)]
fn drain_audio(
    adec: &mut AVCodecContext,
    swr: &mut Option<SwrContext>,
    audio: &mut Option<AudioOut>,
    out_ch: usize,
    out_rate: i32,
    tb_a_secs: f64,
    start: f64,
    seeking: &mut bool,
    cmd_rx: &Receiver<Command>,
) -> Option<SegEnd> {
    loop {
        let frame = match adec.receive_frame() {
            Ok(f) => f,
            Err(_) => break,
        };
        // Discard whole audio frames that end before the seek target.
        let apts = frame.pts as f64 * tb_a_secs;
        let dur = if frame.sample_rate > 0 { frame.nb_samples as f64 / frame.sample_rate as f64 } else { 0.0 };
        if *seeking && apts + dur <= start {
            continue;
        }
        *seeking = false;
        let Some(out) = audio.as_mut() else { continue };
        if let Some(end) = push_audio(swr, &frame, out_ch, out_rate, &mut out.producer, cmd_rx) {
            return Some(end);
        }
    }
    None
}

/// Resample one audio frame to f32 interleaved at the device rate and push it into the ring buffer.
fn push_audio(
    swr: &mut Option<SwrContext>,
    frame: &AVFrame,
    out_ch: usize,
    out_rate: i32,
    producer: &mut HeapProd<f32>,
    cmd_rx: &Receiver<Command>,
) -> Option<SegEnd> {
    if out_ch == 0 {
        return None;
    }
    if swr.is_none() {
        let in_ch = frame.ch_layout().nb_channels.max(1);
        let in_layout = AVChannelLayout::from_nb_channels(in_ch);
        let out_layout = AVChannelLayout::from_nb_channels(out_ch as i32);
        let mut s =
            match SwrContext::new(&out_layout, ffi::AV_SAMPLE_FMT_FLT, out_rate, &in_layout, frame.format, frame.sample_rate) {
                Ok(s) => s,
                Err(_) => return None,
            };
        if s.init().is_err() {
            return None;
        }
        *swr = Some(s);
    }
    let s = swr.as_mut().unwrap();
    let out_count = s.get_out_samples(frame.nb_samples);
    if out_count <= 0 {
        return None;
    }
    let mut buf = vec![0f32; out_count as usize * out_ch];
    let mut out_ptr = buf.as_mut_ptr() as *mut u8;
    let in_ptr = frame.data.as_ptr() as *const *const u8;
    let got = match unsafe { s.convert(&mut out_ptr, out_count, in_ptr, frame.nb_samples) } {
        Ok(n) => n as usize,
        Err(_) => return None,
    };
    let n = got * out_ch;
    let mut off = 0;
    while off < n {
        off += producer.push_slice(&buf[off..n]);
        if off < n {
            // Ring full → audio is keeping pace; wait, but stay responsive to seek/stop.
            if let Some(end) = poll_cmd(cmd_rx) {
                return Some(end);
            }
            std::thread::sleep(Duration::from_millis(3));
        }
    }
    None
}

fn poll_cmd(cmd_rx: &Receiver<Command>) -> Option<SegEnd> {
    match cmd_rx.try_recv() {
        Ok(Command::Seek(t)) => Some(SegEnd::Seek(t)),
        Err(TryRecvError::Disconnected) => Some(SegEnd::Stopped),
        Err(TryRecvError::Empty) => None,
    }
}

// ---- audio output ----

struct AudioOut {
    _stream: cpal::Stream,
    producer: HeapProd<f32>,
    /// Samples-per-channel played by the cpal callback (shared with [`Clock`]).
    played: Arc<AtomicU64>,
    out_rate: u32,
    out_channels: usize,
}

/// Build an f32 output stream on the default device, draining a fresh ring buffer. The callback
/// counts only the samples it actually plays (silence-filled underruns don't count), which keeps the
/// master clock honest through pre-roll and seeks.
fn build_audio_out(device: &cpal::Device) -> Option<AudioOut> {
    let supported = device.default_output_config().ok()?;
    let out_rate = supported.sample_rate();
    let out_ch = supported.channels() as usize;
    if out_ch == 0 {
        return None;
    }
    let config = supported.config();
    let cap = (out_rate as usize * out_ch / 2).max(out_ch * 2048); // ~0.5 s of audio
    let (producer, mut consumer) = HeapRb::<f32>::new(cap).split();
    let played = Arc::new(AtomicU64::new(0));
    let played_cb = Arc::clone(&played);
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |data: &mut [f32], _| {
                let got = consumer.pop_slice(data);
                for s in data[got..].iter_mut() {
                    *s = 0.0;
                }
                played_cb.fetch_add((got / out_ch) as u64, Ordering::Relaxed);
            },
            move |_err| {},
            None,
        )
        .ok()?;
    stream.play().ok()?;
    Some(AudioOut { _stream: stream, producer, played, out_rate, out_channels: out_ch })
}

// ---- setup helpers ----

/// Preview output size: ≤720 tall, preserving aspect, both dimensions even (libplacebo/scale want
/// even). Matches `host::preview_dims` so the two preview paths agree on geometry.
fn placebo_dims(src_w: i64, src_h: i64) -> (u32, u32) {
    let sw = src_w.max(2) as f64;
    let sh = src_h.max(2) as f64;
    let h = ((sh.min(720.0).round() as i64) & !1).max(2);
    let w = (((sw * h as f64 / sh).round() as i64) & !1).max(2);
    (w as u32, h as u32)
}

fn rational_secs(r: ffi::AVRational) -> f64 {
    if r.den != 0 {
        r.num as f64 / r.den as f64
    } else {
        0.0
    }
}

fn build_video_decoder(
    input: &AVFormatContextInput,
    idx: usize,
) -> Option<(AVCodecContext, ffi::AVRational, ffi::AVRational)> {
    let stream = &input.streams()[idx];
    let tb = stream.time_base;
    let par = stream.codecpar();
    let sar = par.sample_aspect_ratio;
    let codec = AVCodec::find_decoder(par.codec_id)?;
    let mut dec = AVCodecContext::new(&codec);
    dec.apply_codecpar(&par).ok()?;
    dec.set_pkt_timebase(tb);
    // Multithreaded decode — essential to hit realtime on 1440p10 AV1 (the default is one thread).
    // `thread_count` isn't in rsmpeg's setter list, so poke it through the raw context; 0 = auto.
    unsafe {
        (*dec.as_mut_ptr()).thread_count = 0;
    }
    dec.open(None).ok()?;
    let sar = if sar.num == 0 { ffi::AVRational { num: 1, den: 1 } } else { sar };
    Some((dec, tb, sar))
}

fn build_audio_decoder(input: &AVFormatContextInput, idx: usize) -> Option<(AVCodecContext, ffi::AVRational)> {
    let stream = &input.streams()[idx];
    let tb = stream.time_base;
    let par = stream.codecpar();
    let codec = AVCodec::find_decoder(par.codec_id)?;
    let mut dec = AVCodecContext::new(&codec);
    dec.apply_codecpar(&par).ok()?;
    dec.set_pkt_timebase(tb);
    dec.open(None).ok()?;
    Some((dec, tb))
}

/// Build `buffer → (libplacebo | scale) → format=rgba → buffersink`. HDR sources go through
/// libplacebo (dynamic-peak HDR→BT.709 SDR tonemap, VLC's engine); SDR uses a plain scale (no Vulkan).
fn build_video_filter<'g>(
    graph: &'g AVFilterGraph,
    vdec: &AVCodecContext,
    tb_v: ffi::AVRational,
    sar: ffi::AVRational,
    w: u32,
    h: u32,
    is_hdr: bool,
) -> Result<(AVFilterContextMut<'g>, AVFilterContextMut<'g>), String> {
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

    // parse_ptr's inverted convention: `outputs` feeds the chain input (buffersrc, "in"); `inputs`
    // is the chain output (buffersink, "out").
    let outputs = AVFilterInOut::new(c"in", &mut src, 0);
    let inputs = AVFilterInOut::new(c"out", &mut sink, 0);
    let descr = if is_hdr {
        CString::new(format!(
            "libplacebo=w={w}:h={h}:tonemapping=auto:colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=pc,format=rgba"
        ))
    } else {
        CString::new(format!("scale={w}:{h}:flags=bilinear,format=rgba"))
    }
    .map_err(|e| e.to_string())?;

    graph.parse_ptr(&descr, Some(inputs), Some(outputs)).map_err(|e| format!("parse: {e:?}"))?;
    graph.config().map_err(|e| format!("config (libplacebo/Vulkan?): {e:?}"))?;
    Ok((src, sink))
}

/// Repack a (possibly stride-padded) RGBA filter frame into tight `w*h*4` bytes for the GPU upload.
fn frame_to_rgba(frame: &AVFrame) -> Vec<u8> {
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
    out
}
