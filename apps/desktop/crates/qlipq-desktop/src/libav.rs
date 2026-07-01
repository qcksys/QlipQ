//! In-process libav preview player.
//!
//! The preview decoder runs *inside* the process, so it can use **libplacebo** for VLC-quality
//! HDR→SDR tonemapping and play **synced audio** — neither of which an external per-frame ffmpeg
//! process could do. It exposes `poll`/`dimensions`/`fps` plus `position()` (an authoritative master
//! clock) and `try_seek()` (a seek command channel — no decoder re-init).
//!
//! Threading: **audio and video decode on separate threads**, each with its own demuxer (the file is
//! opened twice). This is deliberate — **audio is the master clock**, so it must keep its output
//! buffer full regardless of how slow HDR video filtering is; coupling them on one thread let a slow
//! libplacebo frame starve the audio and stutter it. The audio thread decodes → resamples → feeds a
//! lock-free ring buffer drained by cpal (which counts samples played into the clock); the video
//! thread decodes → libplacebo/scale → RGBA into a small queue. The UI presents the video frame whose
//! PTS is due against the master clock (a wall clock when the clip has no audio). Export runs through a
//! separate in-process libav path ([`crate::export`]); only the preview lives here.

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
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

use qlipq_core::media::{AudioStreamInfo, MediaInfo};

pub use crate::host::FramePoll;

/// How many decoded video frames the video thread may run ahead before it blocks. Presentation
/// drains the queue at the master-clock rate, so this caps lookahead (≈0.3 s) and paces decoding.
pub const VIDEO_LOOKAHEAD: usize = 12;

/// Live playback health counters, shared between the decode/audio threads (writers) and the UI
/// (reader), so the editor's debug panel can surface *why* a preview stutters. A starving video
/// queue means decode can't keep realtime; audio underruns are the audible dropouts that follow.
#[derive(Default)]
pub struct PlayerStats {
    /// Decoded frames dropped in [`Player::poll`] because presentation fell behind the master clock.
    pub dropped_frames: AtomicU64,
    /// Samples-per-channel the cpal callback had to zero-fill because the ring ran dry (→ dropouts).
    pub audio_underruns: AtomicU64,
    /// Audio ring occupancy as a fraction ×1000 (0..=1000), sampled by the cpal callback.
    pub audio_fill_permille: AtomicU32,
    /// Current decoded-video queue depth (0..=[`VIDEO_LOOKAHEAD`]); low/zero = decode-starved.
    pub video_queue: AtomicUsize,
}

/// Probe a media file **in process** (libav), returning its [`MediaInfo`] and whether the video
/// stream is HDR (PQ/HLG). This replaces the old `ffprobe` shell-out so the app needs no external
/// binary: it opens the container, reads codec parameters off the streams, and reports the same
/// fields the editor/queue consume. Audio-relative `index` matches ffmpeg's `0:a:N` selector.
pub fn probe(path: &str) -> Result<(MediaInfo, bool), String> {
    let _log = crate::log_ctx::enter(path);
    let cpath = CString::new(path).map_err(|e| e.to_string())?;
    let input = AVFormatContextInput::open(&cpath).map_err(|e| format!("Failed to open {path}: {e}"))?;

    // Container duration is in AV_TIME_BASE units (like ffprobe's `format.duration`).
    let duration_sec = {
        let d = unsafe { (*input.as_ptr()).duration };
        if d > 0 { d as f64 / ffi::AV_TIME_BASE as f64 } else { 0.0 }
    };

    let (mut width, mut height, mut video_codec, mut fps, mut is_hdr) = (0i64, 0i64, String::from("unknown"), 0.0f64, false);
    let mut have_video = false;
    let mut audio_streams: Vec<AudioStreamInfo> = Vec::new();

    for (i, stream) in input.streams().iter().enumerate() {
        let par = stream.codecpar();
        if par.codec_type == ffi::AVMEDIA_TYPE_VIDEO && !have_video {
            have_video = true;
            width = par.width as i64;
            height = par.height as i64;
            video_codec = codec_name(par.codec_id);
            // `r_frame_rate`/`metadata` are raw `AVStream` fields, reached through the stream ref's Deref.
            fps = stream_fps(stream.r_frame_rate, stream.avg_frame_rate);
            // HDR transfer = PQ (smpte2084) or HLG (arib-std-b67), mirroring the old ffprobe check.
            is_hdr = par.color_trc == ffi::AVCOL_TRC_SMPTE2084 || par.color_trc == ffi::AVCOL_TRC_ARIB_STD_B67;
        } else if par.codec_type == ffi::AVMEDIA_TYPE_AUDIO {
            audio_streams.push(AudioStreamInfo {
                stream_index: i as i64,
                index: audio_streams.len() as i64,
                codec: codec_name(par.codec_id),
                channels: par.ch_layout.nb_channels as i64,
                language: dict_tag(stream.metadata, c"language"),
                title: dict_tag(stream.metadata, c"title"),
            });
        }
    }

    let size_bytes = std::fs::metadata(path).ok().map(|m| m.len() as i64);
    // Container-level `encoder` ("encoded by") tag — e.g. "NVIDIA APP" / "NVIDIA APP (Highlights)".
    let encoder = dict_tag(unsafe { (*input.as_ptr()).metadata }, c"encoder");
    let media = MediaInfo { duration_sec, width, height, video_codec, fps, audio_streams, size_bytes, encoder };
    Ok((media, is_hdr))
}

/// The canonical ffmpeg short name for a codec id (e.g. `h264`, `av1`, `aac`); `unknown` if unnamed.
fn codec_name(id: ffi::AVCodecID) -> String {
    let p = unsafe { ffi::avcodec_get_name(id) };
    if p.is_null() {
        return String::from("unknown");
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Frame rate as fps (prefer `r_frame_rate`, fall back to `avg_frame_rate`), rounded to 3 dp to
/// match the old ffprobe parse.
fn stream_fps(r: ffi::AVRational, avg: ffi::AVRational) -> f64 {
    let pick = if r.num != 0 && r.den != 0 { r } else { avg };
    if pick.den != 0 {
        ((pick.num as f64 / pick.den as f64) * 1000.0).round() / 1000.0
    } else {
        0.0
    }
}

/// Read a tag (e.g. `language`, `title`) from a libav metadata dictionary, if present.
fn dict_tag(metadata: *mut ffi::AVDictionary, key: &CStr) -> Option<String> {
    let entry = unsafe { ffi::av_dict_get(metadata, key.as_ptr(), std::ptr::null(), 0) };
    if entry.is_null() {
        return None;
    }
    let value = unsafe { (*entry).value };
    if value.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned())
}

// ---- master clock ----

/// The playback clock. With audio it advances by the number of samples the output device has
/// actually consumed (so it pauses naturally during underruns/pre-roll); without audio it is a plain
/// wall clock anchored to the first presented frame.
struct Clock {
    use_audio: AtomicBool,
    /// Position (seconds) the current segment started at; the clock is relative to this.
    base: Mutex<f64>,
    /// Samples-per-channel the cpal callback has played since the segment (re)started. This is the
    /// *same* atomic the cpal callback increments (handed to `build_audio_out`), so the clock
    /// actually advances with audio — otherwise it stays at `base` and video freezes while audio plays.
    played: Arc<AtomicU64>,
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
    /// Set by the video thread once the first frame of the current segment is queued, cleared when a
    /// segment (re)starts. The audio thread waits on this before starting the device, so the master
    /// clock doesn't run ahead of a cold video start (per-segment libplacebo graph rebuild takes
    /// ~0.5 s) — otherwise video begins behind the clock and stutters/freezes catching up, worst at
    /// the higher preview resolutions.
    video_ready: AtomicBool,
    clock: Clock,
    /// HDR→SDR preview brightness (`eq` gamma applied after the tonemap); read when (re)building the
    /// per-segment filter graph. Set once at `start_player`.
    gamma: f64,
    /// Pixel format the filter buffer source must declare — the post-transfer NV12/P010 when the
    /// decoder is hardware-accelerated, else the decoder's native format. Set once at `start_player`.
    video_pix_fmt: i32,
    /// Live playback health counters (written by the decode/audio threads, read by the UI). An
    /// `Arc` so the cpal callback can hold its own clone to record underruns without the whole `Shared`.
    stats: Arc<PlayerStats>,
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

/// A warm in-process decoder feeding [`crate::video::SharedFrame`]. Provides `poll`/`dimensions`/`fps`
/// plus `position()`/`try_seek()` for live playback control.
pub struct Player {
    shared: Arc<Shared>,
    video_cmd: Option<Sender<Command>>,
    audio_cmd: Option<Sender<Command>>,
    video_thread: Option<JoinHandle<()>>,
    audio_thread: Option<JoinHandle<()>>,
    width: u32,
    height: u32,
    fps: f64,
    /// Per-track monitor-mix gain (audio-relative index → gain as f32 bits), shared live with the
    /// audio thread so volume-slider changes take effect during playback without a restart.
    gains: Vec<(i64, Arc<AtomicU32>)>,
}

impl Player {
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    /// The master clock position in seconds (audio clock, or wall clock when there is no audio).
    /// Always `Some`: the clock is authoritative, so the caller advances the playhead from it.
    pub fn position(&self) -> Option<f64> {
        Some(self.shared.clock.now())
    }

    /// True while the master clock is audio-driven (has decodable enabled audio); false = wall clock.
    pub fn audio_clock(&self) -> bool {
        self.shared.clock.use_audio.load(Ordering::Relaxed)
    }

    /// Decoded-video queue depth now (0..=[`VIDEO_LOOKAHEAD`]). Persistently low = decode-starved.
    pub fn queue_depth(&self) -> usize {
        self.shared.stats.video_queue.load(Ordering::Relaxed)
    }

    /// Cumulative video frames dropped for lateness since playback (re)started.
    pub fn dropped_frames(&self) -> u64 {
        self.shared.stats.dropped_frames.load(Ordering::Relaxed)
    }

    /// Cumulative audio-ring underruns (zero-filled samples-per-channel) — the audible dropouts.
    pub fn audio_underruns(&self) -> u64 {
        self.shared.stats.audio_underruns.load(Ordering::Relaxed)
    }

    /// Audio ring occupancy as a 0.0..=1.0 fraction (last sampled by the cpal callback).
    pub fn audio_fill(&self) -> f32 {
        self.shared.stats.audio_fill_permille.load(Ordering::Relaxed) as f32 / 1000.0
    }

    /// Non-blocking: present the newest video frame that is due at the current clock, dropping any
    /// earlier frames we fell behind on. Returns [`FramePoll::Ended`] once the decoder is done and
    /// the queue has drained.
    pub fn poll(&self) -> FramePoll {
        let clock = self.shared.clock.now();
        let mut q = self.shared.video.lock().unwrap();
        let mut chosen = None;
        let mut popped = 0u64;
        while let Some((pts, _)) = q.front() {
            if *pts <= clock + 1e-3 {
                chosen = q.pop_front().map(|(_, rgba)| rgba);
                popped += 1;
            } else {
                break;
            }
        }
        let empty = q.is_empty();
        self.shared.stats.video_queue.store(q.len(), Ordering::Relaxed);
        drop(q);
        // Every frame past the one we present was decoded but arrived too late — count it as dropped.
        if popped > 1 {
            self.shared.stats.dropped_frames.fetch_add(popped - 1, Ordering::Relaxed);
        }
        match chosen {
            Some(rgba) => FramePoll::Frame(rgba),
            None if empty && self.shared.ended.load(Ordering::Relaxed) => FramePoll::Ended,
            None => FramePoll::Empty,
        }
    }

    /// Seek both decode threads to `sec` without re-opening files or rebuilding the Vulkan/libplacebo
    /// graph. Returns `true` once the command is queued; returns `false` when there is no live decode
    /// thread to seek (e.g. the player never started), so the caller falls back to a full restart.
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

    /// Live-update an enabled track's monitor-mix gain (audio-relative index `rel`) while playing, so
    /// the volume slider takes effect immediately instead of only on the next playback restart.
    pub fn set_gain(&self, rel: i64, vol: f64) {
        if let Some((_, g)) = self.gains.iter().find(|(i, _)| *i == rel) {
            g.store((vol as f32).to_bits(), Ordering::Relaxed);
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

/// Start the in-process player from `start_sec`. Returns `None` if the file/decoder can't open,
/// so the caller falls back to a single-frame preview.
#[allow(clippy::too_many_arguments)]
pub fn start_player(
    path: &str,
    start_sec: f64,
    src_w: i64,
    src_h: i64,
    src_fps: f64,
    is_hdr: bool,
    audio_tracks: Vec<(i64, f64)>,
    gamma: f64,
    max_h: i64,
) -> Option<Player> {
    let _log = crate::log_ctx::enter(path);
    let cpath = CString::new(path).ok()?;
    let input = AVFormatContextInput::open(&cpath).ok()?;

    let vid_idx = input.find_best_stream(ffi::AVMEDIA_TYPE_VIDEO).ok()?.map(|(i, _)| i)?;
    // The scrubber (always open for the clip) reports the decode path to the debug panel, so the
    // streaming player doesn't need its own copy — just the buffersrc pixel format.
    let (vdec, tb_v, sar, video_pix_fmt, _hw_decode) = build_video_decoder(&input, vid_idx)?;
    // Preview audio is a monitor mixdown of the *enabled* tracks (per-track gain), so "has audio"
    // means the file has audio AND at least one track is enabled — disabling them all plays silence
    // (video then runs on the wall clock).
    let file_has_audio = input.find_best_stream(ffi::AVMEDIA_TYPE_AUDIO).ok().flatten().is_some();
    let has_audio = file_has_audio && !audio_tracks.is_empty();

    let dims = placebo_dims(src_w, src_h, max_h);
    let fps = if src_fps.is_finite() && src_fps > 0.0 { src_fps.min(60.0) } else { 30.0 };

    let shared = Arc::new(Shared {
        video: Mutex::new(VecDeque::new()),
        ended: AtomicBool::new(false),
        video_ready: AtomicBool::new(false),
        gamma,
        video_pix_fmt,
        stats: Arc::new(PlayerStats::default()),
        clock: Clock {
            use_audio: AtomicBool::new(has_audio),
            base: Mutex::new(start_sec),
            played: Arc::new(AtomicU64::new(0)),
            rate: AtomicU64::new(0.0f64.to_bits()),
            wall: Mutex::new(None),
        },
    });

    let (video_cmd, video_rx) = channel::<Command>();
    let shared_v = Arc::clone(&shared);
    let vid_idx = vid_idx as i32;
    let v_path = path.to_string();
    let video_thread = std::thread::spawn(move || {
        video_loop(v_path, input, vdec, vid_idx, tb_v, sar, dims, is_hdr, !has_audio, start_sec, shared_v, video_rx);
    });

    // Per-track gains live in shared atomics so the UI can adjust them mid-playback (see `set_gain`).
    let gains: Vec<(i64, Arc<AtomicU32>)> = audio_tracks
        .iter()
        .map(|(rel, vol)| (*rel, Arc::new(AtomicU32::new((*vol as f32).to_bits()))))
        .collect();

    let (audio_cmd, audio_thread) = if has_audio {
        let (tx, rx) = channel::<Command>();
        let shared_a = Arc::clone(&shared);
        let path = path.to_string();
        let track_gains = gains.clone();
        let handle = std::thread::spawn(move || {
            audio_loop(path, start_sec, shared_a, rx, track_gains);
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
        gains,
    })
}

/// A warm single-frame decoder for scrubbing / paused preview. Holds the demuxer, video decoder, **and
/// the libplacebo/scale filter graph** open across scrubs, so each [`frame_at`](ScrubDecoder::frame_at)
/// only warm-seeks, decodes forward to the target frame, and pushes that **one** frame through the warm
/// graph — avoiding the per-scrub graph rebuild that dominates HDR scrub latency (measured ~11× faster
/// for HDR; see `examples/scrubgraph_probe.rs`). It uses the same pipeline as playback so the color
/// matches. Interface-compatible with `host::ScrubDecoder`.
///
/// The graph is built lazily on the first `frame_at` — its libplacebo/Vulkan init is a one-time cost,
/// paid off the UI thread (scrubs already run on the blocking pool). Because the graph is reused, pushed
/// frames get a monotonic PTS (`mono_pts`) so a backward seek doesn't look like time running backwards;
/// the realized image is unaffected.
///
/// _Caveat:_ libplacebo's temporal peak-detect state carries across reused frames, so a frame's tonemap
/// could in principle adapt slightly between scrubs. If that's ever visible, add `peak_detect=0` to the
/// libplacebo args in [`build_video_filter`].
pub struct ScrubDecoder {
    input: AVFormatContextInput,
    vdec: AVCodecContext,
    vid_idx: i32,
    tb_v: ffi::AVRational,
    sar: ffi::AVRational,
    dims: (u32, u32),
    is_hdr: bool,
    /// HDR→SDR preview brightness (`eq` gamma); baked into the warm graph on first `frame_at`.
    gamma: f64,
    /// Buffer-source pixel format (NV12/P010 if hardware-decoded, else native). See `build_video_decoder`.
    pix_fmt: i32,
    /// True if this clip decodes on the GPU (D3D11VA). Surfaced in the debug panel while paused.
    hw_decode: bool,
    /// Warm filter graph, built lazily on the first `frame_at` and reused for every scrub.
    graph: Option<AVFilterGraph>,
    /// Monotonic PTS handed to buffersrc so reused-graph pushes never look like backward time.
    mono_pts: i64,
    /// Source path, so libav decode errors during a scrub name the clip (see `crate::log_ctx`).
    path: String,
}

impl ScrubDecoder {
    /// Open the file and build the video decoder once (the filter graph is built lazily on first use).
    /// `None` if the file/decoder can't open.
    pub fn open(path: &str, src_w: i64, src_h: i64, is_hdr: bool, gamma: f64, max_h: i64) -> Option<Self> {
        let _log = crate::log_ctx::enter(path);
        let cpath = CString::new(path).ok()?;
        let input = AVFormatContextInput::open(&cpath).ok()?;
        let vid_idx = input.find_best_stream(ffi::AVMEDIA_TYPE_VIDEO).ok()?.map(|(i, _)| i)?;
        let (vdec, tb_v, sar, pix_fmt, hw_decode) = build_video_decoder(&input, vid_idx)?;
        Some(Self {
            input,
            vdec,
            vid_idx: vid_idx as i32,
            tb_v,
            sar,
            dims: placebo_dims(src_w, src_h, max_h),
            is_hdr,
            gamma,
            pix_fmt,
            hw_decode,
            graph: None,
            mono_pts: 0,
            path: path.to_owned(),
        })
    }

    /// GPU (D3D11VA) decode vs software decode for this clip. Static per clip; drives the debug panel.
    pub fn hw_decode(&self) -> bool {
        self.hw_decode
    }

    /// The preview output size (aspect-preserved, capped per the preview-quality setting) this
    /// decoder renders frames at.
    pub fn preview_dims(&self) -> (u32, u32) {
        self.dims
    }

    /// Decode the frame at `sec` as tight RGBA, returning `(w, h, rgba, realized_sec)` where
    /// `realized_sec` is the PTS of the frame actually returned (≈ `sec`, within one frame) so the
    /// caller can snap the playhead to the real frame (frame-accurate scrubber). `None` on decode end.
    pub fn frame_at(&mut self, sec: f64) -> Option<(u32, u32, Vec<u8>, f64)> {
        let _log = crate::log_ctx::enter(&self.path);
        let target = sec.max(0.0);
        let (w, h) = self.dims;
        let tb_v_secs = rational_secs(self.tb_v);

        // Warm-seek, then decode forward to the first frame at/after the target.
        if self.tb_v.num != 0 {
            let ts = (target * self.tb_v.den as f64 / self.tb_v.num as f64) as i64;
            let _ = self.input.seek(self.vid_idx, ts, ffi::AVSEEK_FLAG_BACKWARD as i32);
            self.vdec.flush_buffers();
        }
        let frame = decode_to_target(&mut self.input, &mut self.vdec, self.vid_idx, tb_v_secs, target)?;
        let realized = (frame.pts as f64 * tb_v_secs).max(0.0);

        // Build the warm graph once (its libplacebo/Vulkan init is the one-time cost).
        if self.graph.is_none() {
            let graph = AVFilterGraph::new();
            build_video_filter(&graph, &self.vdec, self.tb_v, self.sar, w, h, self.is_hdr, self.gamma, self.pix_fmt).ok()?;
            self.graph = Some(graph);
        }

        // Push the target frame through the warm graph (take/restore so the graph isn't a self-borrow).
        let graph = self.graph.take().unwrap();
        let rgba = filter_target_frame(&graph, &mut self.input, &mut self.vdec, self.vid_idx, frame, &mut self.mono_pts);
        self.graph = Some(graph);
        rgba.map(|px| (w, h, px, realized))
    }
}

/// Seek-forward decode: return the first frame whose PTS reaches `target` (discarding earlier frames).
fn decode_to_target(
    input: &mut AVFormatContextInput,
    vdec: &mut AVCodecContext,
    vid_idx: i32,
    tb_v_secs: f64,
    target: f64,
) -> Option<AVFrame> {
    let mut flushed = false;
    loop {
        match vdec.receive_frame() {
            Ok(f) => {
                let f = to_sw_frame(f)?; // download from GPU if hw-decoded (pts is carried across)
                if f.pts as f64 * tb_v_secs + 1e-3 >= target {
                    return Some(f);
                }
                continue; // before the target — keep decoding forward
            }
            Err(RsmpegError::DecoderDrainError) => {}
            Err(_) => return None,
        }
        match input.read_packet() {
            Ok(Some(pkt)) => {
                if pkt.stream_index == vid_idx {
                    let _ = vdec.send_packet(Some(&pkt));
                }
            }
            Ok(None) if !flushed => {
                let _ = vdec.send_packet(None);
                flushed = true;
            }
            _ => return None,
        }
    }
}

/// Decode the next frame in sequence — used to flush libplacebo's frame latency after the target push.
fn decode_next_frame(input: &mut AVFormatContextInput, vdec: &mut AVCodecContext, vid_idx: i32) -> Option<AVFrame> {
    loop {
        match vdec.receive_frame() {
            Ok(f) => return to_sw_frame(f),
            Err(RsmpegError::DecoderDrainError) => {}
            Err(_) => return None,
        }
        match input.read_packet() {
            Ok(Some(pkt)) => {
                if pkt.stream_index == vid_idx {
                    let _ = vdec.send_packet(Some(&pkt));
                }
            }
            _ => return None,
        }
    }
}

/// Push `first` (the target frame) through the warm graph, feeding the following frames until the graph
/// emits its first output — that output is the target (1-in-1-out, but libplacebo may hold one frame).
/// Stale outputs from a prior scrub are drained first; pushed frames get a monotonic PTS so backward
/// seeks don't look like time going backwards.
fn filter_target_frame(
    graph: &AVFilterGraph,
    input: &mut AVFormatContextInput,
    vdec: &mut AVCodecContext,
    vid_idx: i32,
    first: AVFrame,
    mono: &mut i64,
) -> Option<Vec<u8>> {
    let mut src = graph.get_filter(c"in")?;
    let mut sink = graph.get_filter(c"out")?;
    while sink.buffersink_get_frame(None).is_ok() {} // drop stale output left from the previous scrub

    let mut next = Some(first);
    let mut pushes = 0;
    loop {
        if let Some(mut f) = next.take() {
            unsafe { (*f.as_mut_ptr()).pts = *mono };
            *mono += 1;
            let _ = src.buffersrc_add_frame(Some(f), None);
            pushes += 1;
        }
        match sink.buffersink_get_frame(None) {
            Ok(out) => return Some(frame_to_rgba(&out)),
            Err(RsmpegError::BufferSinkDrainError) => {}
            Err(_) => return None,
        }
        if pushes > 8 {
            return None; // gave up flushing the target frame through
        }
        next = decode_next_frame(input, vdec, vid_idx);
        next.as_ref()?; // decode ended before the frame flushed through
    }
}

// ---- video thread ----

#[allow(clippy::too_many_arguments)]
fn video_loop(
    path: String,
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
    let _log = crate::log_ctx::enter(&path);
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
    let (mut src, mut sink) = match build_video_filter(&graph, vdec, tb_v, sar, w, h, is_hdr, shared.gamma, shared.video_pix_fmt) {
        Ok(pair) => pair,
        Err(_) => return SegEnd::Eof,
    };

    shared.video.lock().unwrap().clear();
    shared.video_ready.store(false, Ordering::Relaxed);
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
                if let Some(sw) = to_sw_frame(frame) {
                    let _ = src.buffersrc_add_frame(Some(sw), None);
                }
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
                    // Tell the audio thread the first frame is ready, so it can start the clock in sync
                    // (see `Shared::video_ready`).
                    shared.video_ready.store(true, Ordering::Relaxed);
                    if manages_clock {
                        *shared.clock.wall.lock().unwrap() = Some((Instant::now(), pts));
                    }
                }
            }
            Err(_) => break,
        }
    }
}

// ---- audio thread (monitor mixdown) ----

/// One enabled audio track in the monitor mix: its decoder, per-track gain, a resampler to the device
/// format, and a FIFO of resampled-but-not-yet-mixed interleaved f32 samples.
struct MixTrack {
    abs_idx: i32,
    dec: AVCodecContext,
    tb_secs: f64,
    /// Gain as f32 bits, shared live with the UI (`Player::set_gain`) so slider moves apply immediately.
    gain: Arc<AtomicU32>,
    swr: Option<SwrContext>,
    pending: Vec<f32>,
    seeking: bool,
}

/// Decode the **enabled** audio tracks, apply per-track gain, and **sum** them into one cpal output —
/// a monitor mixdown so you hear what you configured. (This is preview-only; export still writes each
/// enabled track as its own stream, so it never byte-matches this mix, by design.) The mix is built
/// on this thread and pushed through the ring buffer, so the cpal callback (the master clock) never
/// blocks. `audio_tracks` is `(audio-relative index, volume)` for each enabled track.
fn audio_loop(
    path: String,
    start_sec: f64,
    shared: Arc<Shared>,
    cmd_rx: Receiver<Command>,
    audio_tracks: Vec<(i64, Arc<AtomicU32>)>,
) {
    let _log = crate::log_ctx::enter(&path);
    let Ok(cpath) = CString::new(path) else { return };
    let Ok(mut input) = AVFormatContextInput::open(&cpath) else { return };

    // Map audio-relative indices (0:a:N, as the editor/export use) to absolute stream indices.
    let abs: Vec<i32> = input
        .streams()
        .iter()
        .enumerate()
        .filter(|(_, s)| s.codecpar().codec_type == ffi::AVMEDIA_TYPE_AUDIO)
        .map(|(i, _)| i as i32)
        .collect();

    let mut tracks: Vec<MixTrack> = Vec::new();
    for (rel, gain) in audio_tracks {
        let Some(&abs_idx) = abs.get(rel.max(0) as usize) else { continue };
        if let Some((dec, tb)) = build_audio_decoder(&input, abs_idx as usize) {
            tracks.push(MixTrack {
                abs_idx,
                dec,
                tb_secs: rational_secs(tb),
                gain,
                swr: None,
                pending: Vec::new(),
                seeking: true,
            });
        }
    }
    if tracks.is_empty() {
        // `start_player` optimistically set `has_audio` (so the video thread runs with
        // `manages_clock = false`), but none of the enabled tracks turned out to be decodable. Hand
        // the clock to wall time here so the (now silent) video still plays instead of freezing.
        shared.clock.use_audio.store(false, Ordering::Relaxed);
        *shared.clock.base.lock().unwrap() = start_sec;
        *shared.clock.wall.lock().unwrap() = Some((Instant::now(), start_sec));
        return;
    }

    // The audio thread only needs the enabled audio streams. Tell the demuxer to drop everything else
    // (video, subtitles, disabled tracks) so it doesn't read/parse the whole — often much larger —
    // video stream just to discard it, and doesn't log decode errors for corrupt video it never uses.
    unsafe {
        let ctx = input.as_ptr();
        for i in 0..(*ctx).nb_streams as i32 {
            let keep = tracks.iter().any(|t| t.abs_idx == i);
            let s = *(*ctx).streams.offset(i as isize);
            (*s).discard = if keep { ffi::AVDISCARD_DEFAULT } else { ffi::AVDISCARD_ALL };
        }
    }

    let host = cpal::default_host();
    let device = host.default_output_device();
    let mut start = start_sec;
    loop {
        match audio_segment(&mut input, &mut tracks, start, &shared, device.as_ref(), &cmd_rx) {
            SegEnd::Seek(t) => start = t,
            SegEnd::Eof | SegEnd::Stopped => break,
        }
    }
}

/// Decode all tracks for one segment: seek the container once, reset every track's decoder + FIFO,
/// build a fresh cpal output (so stale audio never survives a seek), then decode → resample → sum →
/// feed until EOF / seek / drop. Owns the master clock.
fn audio_segment(
    input: &mut AVFormatContextInput,
    tracks: &mut [MixTrack],
    start: f64,
    shared: &Arc<Shared>,
    device: Option<&cpal::Device>,
    cmd_rx: &Receiver<Command>,
) -> SegEnd {
    // Seek the container once (by the first track's timeline), then reset every track.
    if let Some(t0) = tracks.first() {
        if t0.tb_secs > 0.0 {
            let ts = (start / t0.tb_secs) as i64;
            let _ = input.seek(t0.abs_idx, ts, ffi::AVSEEK_FLAG_BACKWARD as i32);
        }
    }
    for t in tracks.iter_mut() {
        t.dec.flush_buffers();
        t.pending.clear();
        t.swr = None;
        t.seeking = true;
    }

    // Reset the played counter *before* building the stream — the cpal callback shares this exact
    // atomic and starts incrementing it as soon as the stream plays, so resetting after would wipe
    // its first samples and stall the clock.
    shared.clock.played.store(0, Ordering::Relaxed);
    let mut audio = device.and_then(|d| build_audio_out(d, Arc::clone(&shared.clock.played), Arc::clone(&shared.stats)));
    let use_audio = audio.is_some();
    let out_ch = audio.as_ref().map(|a| a.out_channels).unwrap_or(0);
    let out_rate = audio.as_ref().map(|a| a.out_rate as i32).unwrap_or(48_000);

    // Reset the master clock for this segment (the audio thread owns it when audio is playing).
    shared.clock.use_audio.store(use_audio, Ordering::Relaxed);
    *shared.clock.base.lock().unwrap() = start;
    shared.clock.rate.store((out_rate as f64).to_bits(), Ordering::Relaxed);
    let played = audio.as_ref().map(|a| Arc::clone(&a.played));

    // Pre-roll cushion: keep the device paused until the ring holds ~125 ms, so the first callbacks
    // never drain a near-empty ring while decoder/graph startup is hogging CPU (the first-seconds
    // skip). The clock advances on `played`, so video simply holds the start frame until we start.
    let prime = (out_rate.max(1) as usize * out_ch.max(1)) / 8;
    let mut started = false;

    loop {
        if let Some(end) = poll_cmd(cmd_rx) {
            return end;
        }
        match input.read_packet() {
            Ok(Some(pkt)) => {
                if let Some(t) = tracks.iter_mut().find(|t| t.abs_idx == pkt.stream_index) {
                    let _ = t.dec.send_packet(Some(&pkt));
                    decode_into_pending(t, out_ch, out_rate, start);
                }
                if let Some(end) = mix_and_push(tracks, &mut audio, out_ch, out_rate, false, started, cmd_rx) {
                    return end;
                }
                // Start the device once the cushion is buffered (prime ≪ ring capacity, so mix_and_push
                // never backs up against a not-yet-draining ring before we get here).
                if !started {
                    if let Some(a) = audio.as_ref() {
                        if a.producer.occupied_len() >= prime {
                            // Also wait for the video thread's first frame so audio and video start
                            // together (see `Shared::video_ready`). Bounded, so an audio-only or
                            // slow-to-decode clip still starts; the ring holds at `prime` meanwhile
                            // (we've stopped pushing), so no underrun on start.
                            let wait = Instant::now();
                            while !shared.video_ready.load(Ordering::Relaxed)
                                && wait.elapsed() < Duration::from_millis(1500)
                            {
                                if let Some(end) = poll_cmd(cmd_rx) {
                                    return end;
                                }
                                std::thread::sleep(Duration::from_millis(5));
                            }
                            started = a.start();
                            if !started {
                                // The device won't start (e.g. a WASAPI failure). Don't keep filling a
                                // ring that will never drain — that hangs this thread and freezes video
                                // on the first frame. Fall back to silent wall-clock playback, matching
                                // the pre-roll refactor's predecessor (`build_audio_out`'s old play()?).
                                *shared.clock.wall.lock().unwrap() = Some((Instant::now(), start));
                                shared.clock.use_audio.store(false, Ordering::Relaxed);
                                return SegEnd::Eof;
                            }
                        }
                    }
                }
            }
            Ok(None) => {
                for t in tracks.iter_mut() {
                    let _ = t.dec.send_packet(None);
                    decode_into_pending(t, out_ch, out_rate, start);
                }
                if let Some(end) = mix_and_push(tracks, &mut audio, out_ch, out_rate, true, started, cmd_rx) {
                    return end;
                }
                // A clip shorter than the pre-roll cushion never crossed `prime`; start the device now
                // so the drain loop below can actually empty the ring instead of spinning forever.
                let running = match audio.as_ref() {
                    Some(a) => started || a.start(),
                    None => false,
                };
                // Keep the device alive until its buffer drains (so the last ~0.5 s isn't cut off),
                // then hand the clock to wall time so any longer video tail keeps playing smoothly.
                // If the device never started, don't wait on a ring that can't drain.
                loop {
                    if let Some(end) = poll_cmd(cmd_rx) {
                        return end;
                    }
                    if !running || audio.as_ref().map(|a| a.producer.is_empty()).unwrap_or(true) {
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

/// Pull all decoded frames from one track, discard the pre-seek ones, and resample the rest into its
/// pending FIFO. Never touches the device (that's [`mix_and_push`]), so it can't block the clock.
fn decode_into_pending(t: &mut MixTrack, out_ch: usize, out_rate: i32, start: f64) {
    loop {
        let frame = match t.dec.receive_frame() {
            Ok(f) => f,
            Err(_) => break,
        };
        // Discard whole audio frames that end before the seek target.
        let apts = frame.pts as f64 * t.tb_secs;
        let dur = if frame.sample_rate > 0 { frame.nb_samples as f64 / frame.sample_rate as f64 } else { 0.0 };
        if t.seeking && apts + dur <= start {
            continue;
        }
        t.seeking = false;
        resample_into(&mut t.swr, &frame, out_ch, out_rate, &mut t.pending);
    }
}

/// Resample one frame to f32 interleaved at the device rate, appending to the track's pending FIFO.
fn resample_into(swr: &mut Option<SwrContext>, frame: &AVFrame, out_ch: usize, out_rate: i32, dst: &mut Vec<f32>) {
    if out_ch == 0 {
        return;
    }
    if swr.is_none() {
        let in_ch = frame.ch_layout().nb_channels.max(1);
        let in_layout = AVChannelLayout::from_nb_channels(in_ch);
        let out_layout = AVChannelLayout::from_nb_channels(out_ch as i32);
        let mut s =
            match SwrContext::new(&out_layout, ffi::AV_SAMPLE_FMT_FLT, out_rate, &in_layout, frame.format, frame.sample_rate) {
                Ok(s) => s,
                Err(_) => return,
            };
        if s.init().is_err() {
            return;
        }
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
    buf.truncate(got * out_ch);
    dst.extend_from_slice(&buf);
}

/// Sum each track's pending samples (× its gain) into one interleaved buffer and push it to the ring
/// (with backpressure). Once playing, mixes only the length all tracks share so they stay aligned;
/// on EOF (`flush`), before the device has `started`, or if a stalled track makes another's FIFO back
/// up past ~1 s, it mixes everything, padding short tracks with silence. Returns `Some(SegEnd)` if
/// seek/stop interrupts the blocking push.
///
/// `started` is critical for correctness: while the device is **paused** (pre-roll), it isn't draining
/// the ring, so a full ring would never free up — blocking on it would hang the audio thread forever
/// (silent, frozen clock). So before start we (a) mix `max_len` rather than the shared min, so a clip
/// whose second audio stream is muxed far behind can't hold the min at 0 and starve the ring fill, and
/// (b) never block: push what fits and return, letting the caller start the device once the ring
/// reaches `prime`.
fn mix_and_push(
    tracks: &mut [MixTrack],
    audio: &mut Option<AudioOut>,
    out_ch: usize,
    out_rate: i32,
    flush: bool,
    started: bool,
    cmd_rx: &Receiver<Command>,
) -> Option<SegEnd> {
    let Some(out) = audio.as_mut() else {
        // No output device — drop pending so it can't grow without bound.
        for t in tracks.iter_mut() {
            t.pending.clear();
        }
        return None;
    };
    if out_ch == 0 {
        return None;
    }
    let backlog_cap = out_rate as usize * out_ch; // ~1 s; force a drain if a track stalls
    loop {
        let max_len = tracks.iter().map(|t| t.pending.len()).max().unwrap_or(0);
        let n = if flush || !started || max_len > backlog_cap {
            max_len
        } else {
            tracks.iter().map(|t| t.pending.len()).min().unwrap_or(0)
        };
        if n == 0 {
            return None;
        }
        let mut mix = vec![0f32; n];
        for t in tracks.iter() {
            let g = f32::from_bits(t.gain.load(Ordering::Relaxed)); // live gain (UI can change it mid-play)
            for (m, &s) in mix.iter_mut().zip(t.pending.iter()) {
                *m += s * g;
            }
        }
        for m in mix.iter_mut() {
            *m = m.clamp(-1.0, 1.0); // summed tracks can exceed full scale; clamp like a hard limiter
        }
        let mut off = 0;
        while off < n {
            off += out.producer.push_slice(&mix[off..n]);
            if off < n {
                if !started {
                    // Device paused (pre-roll) → the ring won't drain, so don't block on it (that
                    // deadlocks the player). Drain what we pushed and return; the caller starts the
                    // device once the ring reaches `prime`, after which the blocking path below is safe.
                    for t in tracks.iter_mut() {
                        let take = t.pending.len().min(off);
                        t.pending.drain(0..take);
                    }
                    return None;
                }
                // Ring full → audio is keeping pace; wait, but stay responsive to seek/stop.
                if let Some(end) = poll_cmd(cmd_rx) {
                    return Some(end);
                }
                std::thread::sleep(Duration::from_millis(3));
            }
        }
        for t in tracks.iter_mut() {
            let take = t.pending.len().min(n);
            t.pending.drain(0..take);
        }
    }
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

impl AudioOut {
    /// Start the device draining the ring; `true` if it's now playing. The stream is built **paused**
    /// so the caller can pre-fill a cushion first — otherwise the first callbacks drain an empty ring
    /// and glitch (the audible skip in the first moments of playback), especially while decoder/graph
    /// startup is hogging CPU.
    fn start(&self) -> bool {
        self._stream.play().is_ok()
    }
}

/// Build a **paused** f32 output stream on the default device, draining a fresh ring buffer; the
/// caller pre-fills the ring, then calls [`AudioOut::start`]. The callback increments `played` — the
/// **clock's** counter, passed in by the caller — counting only the samples it actually plays
/// (silence-filled underruns don't count), which is what advances the master clock through pre-roll
/// and seeks. Sharing the clock's atomic (rather than a private one) is essential: otherwise the clock
/// never moves and video freezes while audio plays.
fn build_audio_out(device: &cpal::Device, played: Arc<AtomicU64>, stats: Arc<PlayerStats>) -> Option<AudioOut> {
    let supported = device.default_output_config().ok()?;
    let out_rate = supported.sample_rate();
    let out_ch = supported.channels() as usize;
    if out_ch == 0 {
        return None;
    }
    let config = supported.config();
    // ~0.5 s of audio. The ring is the monitor-mix latency: everything in it was already summed at
    // the levels in force when it was pushed, so per-track volume/enable changes aren't heard until it
    // drains. A shallow ring keeps that responsive while still giving the (cheap AAC) audio thread a
    // cushion to coast through brief CPU contention from the video decode/libplacebo work.
    let cap = (out_rate as usize * out_ch / 2).max(out_ch * 2048);
    let (producer, mut consumer) = HeapRb::<f32>::new(cap).split();
    let played_cb = Arc::clone(&played);
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |data: &mut [f32], _| {
                let got = consumer.pop_slice(data);
                // Any samples the ring couldn't supply are zero-filled — an audible dropout. Record
                // them (per-channel) and the current ring occupancy so the debug panel can show
                // whether the audio buffer is starving.
                if got < data.len() {
                    stats.audio_underruns.fetch_add(((data.len() - got) / out_ch) as u64, Ordering::Relaxed);
                    for s in data[got..].iter_mut() {
                        *s = 0.0;
                    }
                }
                played_cb.fetch_add((got / out_ch) as u64, Ordering::Relaxed);
                stats.audio_fill_permille.store((consumer.occupied_len() * 1000 / cap) as u32, Ordering::Relaxed);
            },
            move |_err| {},
            None,
        )
        .ok()?;
    // Built paused — `audio_segment` primes the ring, then calls `AudioOut::start`.
    Some(AudioOut { _stream: stream, producer, played, out_rate, out_channels: out_ch })
}

// ---- setup helpers ----

/// Preview output size: height capped at `max_h` (px; `0` = source, no cap — never upscales),
/// preserving aspect, both dimensions even (libplacebo/scale want even).
fn placebo_dims(src_w: i64, src_h: i64, max_h: i64) -> (u32, u32) {
    let sw = src_w.max(2) as f64;
    let sh = src_h.max(2) as f64;
    let cap = if max_h > 0 { (max_h as f64).min(sh) } else { sh };
    let h = ((cap.round() as i64) & !1).max(2);
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

/// The D3D11VA hardware pixel format (e.g. `AV_PIX_FMT_D3D11`), read from `avcodec_get_hw_config` and
/// shared with the `get_hw_format` callback (a C function pointer that can't capture). It's the same
/// value for every decoder in the process, so a single global is race-free.
static HW_PIX_FMT: AtomicI32 = AtomicI32::new(ffi::AV_PIX_FMT_NONE);

/// Decoder `get_format` callback: pick the D3D11VA surface format if the decoder offers it, else fall
/// back to its first (software) choice. Selecting the hw format is what actually engages hwaccel.
unsafe extern "C" fn get_hw_format(
    _ctx: *mut ffi::AVCodecContext,
    fmts: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    let want = HW_PIX_FMT.load(Ordering::Relaxed);
    let mut p = fmts;
    while unsafe { *p } != ffi::AV_PIX_FMT_NONE {
        if unsafe { *p } == want {
            return want;
        }
        p = unsafe { p.add(1) };
    }
    unsafe { *fmts } // hw not offered for this stream → first (software) format
}

/// The D3D11VA hardware pixel format this decoder supports via a hw device context, if any.
fn d3d11va_hw_pix_fmt(codec: &AVCodec) -> Option<i32> {
    let mut i = 0;
    loop {
        let cfg = unsafe { ffi::avcodec_get_hw_config(codec.as_ptr(), i) };
        if cfg.is_null() {
            return None;
        }
        let cfg = unsafe { &*cfg };
        if cfg.methods & ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
            && cfg.device_type == ffi::AV_HWDEVICE_TYPE_D3D11VA
        {
            return Some(cfg.pix_fmt as i32);
        }
        i += 1;
    }
}

/// True if `fmt` (an `AVPixelFormat`) carries more than 8 bits per component (→ P010 after hw transfer).
fn pix_fmt_is_10bit(fmt: i32) -> bool {
    let desc = unsafe { ffi::av_pix_fmt_desc_get(fmt) };
    !desc.is_null() && unsafe { (*desc).comp[0].depth } > 8
}

/// If `frame` is a hardware surface (D3D11), download it to a software frame (NV12/P010) for the CPU
/// filter graph, carrying timestamps/color props across. Software frames pass through untouched.
fn to_sw_frame(frame: AVFrame) -> Option<AVFrame> {
    if unsafe { (*frame.as_ptr()).hw_frames_ctx.is_null() } {
        return Some(frame);
    }
    let mut sw = AVFrame::new();
    unsafe {
        if ffi::av_hwframe_transfer_data(sw.as_mut_ptr(), frame.as_ptr(), 0) < 0 {
            return None;
        }
        ffi::av_frame_copy_props(sw.as_mut_ptr(), frame.as_ptr());
    }
    Some(sw)
}

/// Build the video decoder, enabling **D3D11VA hardware decode** when the codec + GPU support it (it
/// works on Windows for all vendors). Returns the pixel format the filter buffer source should expect:
/// the post-transfer software format (NV12 / P010) when hwaccel engages, else the decoder's native
/// format. Falls back to software decode automatically if the hw device can't be created — so this is
/// a no-op on machines without a usable GPU decoder. Heavy 1440p10 AV1/HEVC is where this earns its
/// keep (software decode there runs below realtime and starves preview audio).
fn build_video_decoder(
    input: &AVFormatContextInput,
    idx: usize,
) -> Option<(AVCodecContext, ffi::AVRational, ffi::AVRational, i32, bool)> {
    let stream = &input.streams()[idx];
    let tb = stream.time_base;
    let par = stream.codecpar();
    let sar = par.sample_aspect_ratio;
    let src_fmt = par.format; // bitstream sw pixel format
    // FFmpeg's default AV1 decoder is software-only `libdav1d` (no hwaccel); the native `av1` decoder
    // supports D3D11VA, so prefer it for AV1 — the GPU decodes when it can (RTX 40 / RX 7000 / Arc),
    // else it just software-decodes. hevc/h264 already default to their hw-capable native decoders.
    let codec = if par.codec_id == ffi::AV_CODEC_ID_AV1 {
        AVCodec::find_decoder_by_name(c"av1").or_else(|| AVCodec::find_decoder(par.codec_id))?
    } else {
        AVCodec::find_decoder(par.codec_id)?
    };
    let mut dec = AVCodecContext::new(&codec);
    dec.apply_codecpar(&par).ok()?;
    dec.set_pkt_timebase(tb);
    // Multithreaded decode — essential to hit realtime on 1440p10 AV1 (the default is one thread).
    // `thread_count` isn't in rsmpeg's setter list, so poke it through the raw context; 0 = auto.
    unsafe {
        (*dec.as_mut_ptr()).thread_count = 0;
    }

    // Try D3D11VA. If the device is created, frames come back as hw surfaces and `to_sw_frame`
    // downloads them to NV12 (8-bit) / P010 (10-bit) — which is what the buffer source must declare.
    let mut buffersrc_fmt = src_fmt;
    let mut hw_engaged = false;
    if let Some(hw_pix_fmt) = d3d11va_hw_pix_fmt(&codec) {
        let mut hw_device: *mut ffi::AVBufferRef = std::ptr::null_mut();
        let created = unsafe {
            ffi::av_hwdevice_ctx_create(
                &mut hw_device,
                ffi::AV_HWDEVICE_TYPE_D3D11VA,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            )
        };
        if created >= 0 {
            HW_PIX_FMT.store(hw_pix_fmt, Ordering::Relaxed);
            unsafe {
                (*dec.as_mut_ptr()).hw_device_ctx = hw_device; // ownership → freed with the context
                (*dec.as_mut_ptr()).get_format = Some(get_hw_format);
            }
            buffersrc_fmt = if pix_fmt_is_10bit(src_fmt) { ffi::AV_PIX_FMT_P010LE } else { ffi::AV_PIX_FMT_NV12 };
            hw_engaged = true;
        }
    }

    dec.open(None).ok()?;
    // One line per decoder open so it's clear whether the GPU is doing the work. Software decode of
    // heavy 1440p AV1/HEVC runs below realtime and is the usual cause of preview audio stutter — if a
    // clip logs "software" here (e.g. AV1 on a GPU without hw AV1 decode), that's why.
    let codec = codec_name(par.codec_id);
    let mode = if hw_engaged {
        "D3D11VA hardware decode"
    } else {
        "software decode (no D3D11VA for this codec/GPU)"
    };
    match crate::log_ctx::current() {
        Some(file) => eprintln!("qlipq[{file}]: {codec} video → {mode}"),
        None => eprintln!("qlipq: {codec} video → {mode}"),
    }
    let sar = if sar.num == 0 { ffi::AVRational { num: 1, den: 1 } } else { sar };
    Some((dec, tb, sar, buffersrc_fmt, hw_engaged))
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
    gamma: f64,
    pix_fmt: i32,
) -> Result<(AVFilterContextMut<'g>, AVFilterContextMut<'g>), String> {
    // Carry the decoded color matrix + range into the buffer source. Without them the source is
    // created as colorspace/range "unknown", so the first HDR frame (bt2020nc / tv) trips
    // "Changing video frame properties on the fly is not supported by all filters" — which stalls
    // libplacebo's continuous output (playback freezes after one frame) and feeds it the wrong
    // input colorimetry (a limited-range source read as unknown comes out dull/dark). color_primaries
    // and color_trc ride along on each frame, so the buffer only needs colorspace + range.
    // `pix_fmt` is the frame format the source will actually receive — NV12/P010 after a hardware
    // decode + transfer, the decoder's native format otherwise (not `vdec.pix_fmt`, which is the
    // opaque D3D11 surface format under hwaccel).
    let (colorspace, color_range) =
        unsafe { ((*vdec.as_ptr()).colorspace as i32, (*vdec.as_ptr()).color_range as i32) };
    let args = CString::new(format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}:colorspace={}:range={}",
        vdec.width, vdec.height, pix_fmt, tb_v.num, tb_v.den, sar.num, sar.den, colorspace, color_range
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
        // HDR→BT.709 SDR tonemap (full-range RGB out for the GPU upload), then a luma-gamma midtone
        // lift (the `hdr_preview_gamma` setting). Windows HDR *desktop* capture pins SDR-content white
        // at the Windows "SDR content brightness" level (often well above the 203-nit reference)
        // inside the PQ container, so libplacebo maps that down to its 203-nit SDR target and the UI
        // reads ~half brightness. The ffmpeg libplacebo wrapper exposes no source/target-peak knob, so
        // we compensate with gamma (>1 brightens, preserves true black). 1.0 = off (filter skipped).
        // `lutyuv` (not `eq`) does the gamma because `eq` is a GPL filter, absent from the LGPL build.
        let g = gamma.clamp(0.1, 10.0);
        let lift = if (g - 1.0).abs() > 1e-3 {
            format!(",lutyuv=y='pow(val/maxval,1/{g:.3})*maxval'")
        } else {
            String::new()
        };
        CString::new(format!(
            "libplacebo=w={w}:h={h}:tonemapping=auto:colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=pc{lift},format=rgba"
        ))
    } else {
        // lanczos: a sharper downscale than bilinear (the preview is usually shrinking the source),
        // and cheap on the CPU for SDR. HDR goes through libplacebo above, which scales itself.
        CString::new(format!("scale={w}:{h}:flags=lanczos,format=rgba"))
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
