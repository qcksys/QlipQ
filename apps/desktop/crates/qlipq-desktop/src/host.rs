//! Cross-platform host layer: the I/O the pure crates can't do — spawning ffmpeg/ffprobe,
//! filesystem scan/watch, config/edits persistence, and OBS/NVIDIA capture-folder detection.
//! Ported from the Tauri Rust backend, made portable (per-OS config paths, Windows-gated registry).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
// The streaming CLI decoder (Player) only exists when the in-process libav preview is off.
#[cfg(not(feature = "libav-preview"))]
use std::process::Child;
#[cfg(not(feature = "libav-preview"))]
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use qlipq_core::config::AppConfig;
use qlipq_core::config_json;
use qlipq_core::detect::{detect_obs_recording_folder, ObsConfigFiles};
use qlipq_core::media::MediaInfo;
use qlipq_ffmpeg::probe::{build_probe_args, parse_ffprobe};
use qlipq_ffmpeg::progress::{parse_progress, progress_fraction};

/// Normalize a path to forward slashes (matches the web app's `toPosixPath`).
pub fn to_posix(path: &str) -> String {
    path.replace('\\', "/")
}

/// `~/.com.qcksys.qlipq` — keep this exact location for config/edits continuity.
pub fn data_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".com.qcksys.qlipq")
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

/// One-time copy of config.json + edits.json from the old Roaming AppData location.
pub fn migrate_legacy_data() {
    let Some(old_dir) = dirs::config_dir().map(|d| d.join("com.qcksys.qlipq")) else {
        return;
    };
    let new_dir = data_dir();
    if old_dir == new_dir {
        return;
    }
    for name in ["config.json", "edits.json"] {
        let new_path = new_dir.join(name);
        let old_path = old_dir.join(name);
        if !new_path.exists() && old_path.exists() {
            let _ = std::fs::create_dir_all(&new_dir);
            let _ = std::fs::copy(&old_path, &new_path);
        }
    }
}

pub fn load_config() -> AppConfig {
    match std::fs::read_to_string(config_path()) {
        Ok(text) => config_json::parse(&text),
        Err(_) => AppConfig::default(),
    }
}

pub fn save_config(cfg: &AppConfig) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, config_json::serialize(cfg))
}

/// Write the JSON Schema for `config.json` next to it, so editors validate the config against the
/// relative `$schema` ref the app stamps. Called on startup; cheap to refresh.
pub fn write_config_schema() -> std::io::Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(config_json::SCHEMA_FILE), config_json::schema_json())
}

fn is_valid_name(name: &str) -> bool {
    !name.contains('/') && !name.contains('\\') && !name.contains("..")
}

pub fn read_app_file(name: &str) -> Option<String> {
    if !is_valid_name(name) {
        return None;
    }
    std::fs::read_to_string(data_dir().join(name)).ok()
}

pub fn write_app_file(name: &str, contents: &str) -> std::io::Result<()> {
    if !is_valid_name(name) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid file name"));
    }
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(name), contents)
}

fn has_video_ext(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| extensions.iter().any(|x| x.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

/// Recursively collect video files, skipping symlinks/junctions (not followed), like `scan_folders`.
pub fn scan_folders(folders: &[String], extensions: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack: Vec<PathBuf> = folders.iter().map(PathBuf::from).collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() && has_video_ext(&entry.path(), extensions) {
                found.push(to_posix(&entry.path().to_string_lossy()));
            }
        }
    }
    found
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: String,
    pub size: i64,
    pub modified_ms: i64,
}

pub fn file_info(paths: &[String]) -> Vec<FileInfo> {
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        out.push(FileInfo { path: path.clone(), size: meta.len() as i64, modified_ms });
    }
    out
}

pub fn file_exists(path: &str) -> bool {
    Path::new(path).is_file()
}

/// Rename a file on disk; returns the new path. Cross-device moves fall back to copy+delete.
pub fn rename_file(from: &str, to: &str) -> Result<String, String> {
    if from == to {
        return Ok(to.to_string());
    }
    if Path::new(to).exists() {
        return Err(format!("A file already exists at {to}"));
    }
    if let Some(parent) = Path::new(to).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(to.to_string()),
        Err(_) => {
            std::fs::copy(from, to).map_err(|e| e.to_string())?;
            std::fs::remove_file(from).map_err(|e| e.to_string())?;
            Ok(to.to_string())
        }
    }
}

pub fn delete_file(path: &str) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

/// Build a Command, hiding the console window on Windows.
fn hidden_command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Run `<path> -version`; returns the first stdout line (version banner).
pub fn check_binary(path: &str) -> Result<String, String> {
    let output = hidden_command(path)
        .arg("-version")
        .output()
        .map_err(|e| format!("Not found ({path}): {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().next().unwrap_or("").trim().to_string())
}

pub fn probe(path: &str, ffprobe_path: &str) -> Result<MediaInfo, String> {
    probe_with_hdr(path, ffprobe_path).map(|(media, _)| media)
}

/// Probe like [`probe`], but also report whether the video stream is HDR (PQ/HLG).
/// HDR detection reads `color_transfer` straight from the ffprobe JSON in the host layer — the
/// shared `qlipq-ffmpeg` parse stays untouched so cross-port parity is unaffected.
pub fn probe_with_hdr(path: &str, ffprobe_path: &str) -> Result<(MediaInfo, bool), String> {
    let output = hidden_command(ffprobe_path)
        .args(build_probe_args(path))
        .output()
        .map_err(|e| format!("Failed to run ffprobe ({ffprobe_path}): {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    let json = String::from_utf8_lossy(&output.stdout);
    Ok((parse_ffprobe(&json), detect_hdr(&json)))
}

/// True if the file's video stream uses an HDR transfer function (PQ `smpte2084` or HLG).
fn detect_hdr(probe_json: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct ColorProbe {
        streams: Option<Vec<ColorStream>>,
    }
    #[derive(serde::Deserialize)]
    struct ColorStream {
        codec_type: Option<String>,
        color_transfer: Option<String>,
    }
    serde_json::from_str::<ColorProbe>(probe_json)
        .ok()
        .and_then(|p| p.streams)
        .unwrap_or_default()
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .and_then(|s| s.color_transfer.as_deref())
        .is_some_and(|t| matches!(t, "smpte2084" | "arib-std-b67"))
}

/// Build the preview filter chain. For HDR sources, tonemap BT.2020 PQ/HLG → BT.709 SDR (otherwise
/// HDR is shown with washed-out/clipped colors); for SDR, a plain scale. Both fix the output size so
/// the raw RGBA byte count is exact. Pass `fps` for the streaming decoder (resamples the rate).
#[cfg(not(feature = "libav-preview"))]
fn preview_vf(w: u32, h: u32, is_hdr: bool, fps: Option<f64>) -> String {
    let fps_part = fps.map(|f| format!(",fps={f:.5}")).unwrap_or_default();
    if is_hdr {
        // zscale resizes + linearizes from the source's HDR transfer, Hable tonemaps to SDR (clean
        // highlight rolloff), then back to full-range BT.709; a small `eq` gamma lift restores
        // midtones (Hable alone reads too dark; Mobius alone too bright). `-pix_fmt rgba` finishes.
        // NOTE: this is an interim fixed-operator approximation. VLC-quality HDR (libplacebo with
        // dynamic peak detection) is too slow to spawn per-frame and will come via the in-process
        // rsmpeg + libplacebo preview path.
        format!(
            "zscale=w={w}:h={h}:t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,\
             tonemap=tonemap=hable:desat=2,zscale=t=bt709:m=bt709:r=pc,eq=gamma=1.3{fps_part}"
        )
    } else {
        format!("scale={w}:{h}{fps_part}")
    }
}

/// Preview output dimensions: scaled to ≤720 tall (never upscaled), preserving aspect.
/// Both dimensions are fixed explicitly so raw RGBA frames have an exactly known byte count.
#[cfg(not(feature = "libav-preview"))]
pub fn preview_dims(src_w: i64, src_h: i64) -> (u32, u32) {
    let src_w = src_w.max(2) as f64;
    let src_h = src_h.max(2) as f64;
    let out_h = src_h.min(720.0);
    let out_w = (src_w * out_h / src_h).round().max(2.0);
    (out_w as u32, out_h as u32)
}

/// Extract a single frame at `sec` as raw RGBA bytes for the preview (scrubbing / paused).
/// Outputs `rawvideo` — no PNG encode/decode round-trip — at a fixed, pre-computed size.
#[cfg(not(feature = "libav-preview"))]
pub fn extract_frame(
    path: &str,
    ffmpeg_path: &str,
    sec: f64,
    src_w: i64,
    src_h: i64,
    is_hdr: bool,
) -> Result<(u32, u32, Vec<u8>), String> {
    let (w, h) = preview_dims(src_w, src_h);
    let sec_arg = format!("{:.3}", sec.max(0.0));
    let vf = preview_vf(w, h, is_hdr, None);
    let mut output = hidden_command(ffmpeg_path)
        .args([
            "-hide_banner", "-loglevel", "error", "-ss", &sec_arg, "-i", path, "-frames:v", "1",
            "-an", "-vf", &vf, "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1",
        ])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("Failed to run ffmpeg ({ffmpeg_path}): {e}"))?;
    let expected = (w as usize) * (h as usize) * 4;
    if !output.status.success() || output.stdout.len() < expected {
        return Err("frame extraction failed".to_string());
    }
    output.stdout.truncate(expected);
    Ok((w, h, output.stdout))
}

/// CLI-path scrub decoder. The streaming ffmpeg path can't hold a warm single-frame decoder, so this
/// just remembers the source parameters and spawns ffmpeg per frame (the original behaviour) behind
/// the same `open`/`frame_at` interface as `libav::ScrubDecoder`, keeping the editor feature-agnostic.
#[cfg(not(feature = "libav-preview"))]
pub struct ScrubDecoder {
    path: String,
    ffmpeg: String,
    src_w: i64,
    src_h: i64,
    is_hdr: bool,
}

#[cfg(not(feature = "libav-preview"))]
impl ScrubDecoder {
    pub fn open(path: &str, ffmpeg_path: &str, src_w: i64, src_h: i64, is_hdr: bool) -> Option<Self> {
        Some(Self { path: path.to_string(), ffmpeg: ffmpeg_path.to_string(), src_w, src_h, is_hdr })
    }

    /// Extract the frame at `sec`. The CLI path can't report the exact decoded PTS, so `realized_sec`
    /// echoes the request.
    pub fn frame_at(&mut self, sec: f64) -> Option<(u32, u32, Vec<u8>, f64)> {
        extract_frame(&self.path, &self.ffmpeg, sec, self.src_w, self.src_h, self.is_hdr)
            .ok()
            .map(|(w, h, rgba)| (w, h, rgba, sec))
    }
}

/// Result of polling a [`Player`] for the next decoded frame.
pub enum FramePoll {
    /// A frame is ready (raw RGBA, `width * height * 4` bytes).
    Frame(Vec<u8>),
    /// No frame ready yet (decoder still working).
    Empty,
    /// The decoder finished or died — playback should stop.
    Ended,
}

/// A persistent ffmpeg decoder that streams sequential RGBA frames for smooth playback.
///
/// One process is kept warm (input opened, decoder state retained) instead of re-spawning ffmpeg
/// per frame, so sequential playback avoids the per-frame spawn + container-open + seek cost. A
/// reader thread pushes frames into a small bounded channel; the full pipe applies backpressure so
/// ffmpeg decodes at roughly the consumption rate rather than buffering the whole clip in memory.
#[cfg(not(feature = "libav-preview"))]
pub struct Player {
    child: Child,
    rx: Receiver<Vec<u8>>,
    width: u32,
    height: u32,
    fps: f64,
}

#[cfg(not(feature = "libav-preview"))]
impl Player {
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    /// The CLI player has no master clock — the caller advances time by 1/fps per frame.
    /// (The in-process libav player returns `Some(position)` here instead.)
    pub fn position(&self) -> Option<f64> {
        None
    }

    /// The CLI player can't seek a warm process; `false` tells the caller to restart instead.
    /// (The in-process libav player seeks via a command channel and returns `true`.)
    pub fn try_seek(&self, _sec: f64) -> bool {
        false
    }

    /// Non-blocking: take the next decoded frame if one is ready.
    pub fn poll(&self) -> FramePoll {
        match self.rx.try_recv() {
            Ok(frame) => FramePoll::Frame(frame),
            Err(TryRecvError::Empty) => FramePoll::Empty,
            Err(TryRecvError::Disconnected) => FramePoll::Ended,
        }
    }
}

#[cfg(not(feature = "libav-preview"))]
impl Drop for Player {
    fn drop(&mut self) {
        // Kill the decoder; the reader thread then hits EOF (or the dropped receiver) and exits.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start a streaming decoder from `start_sec`, emitting frames at ≤30fps (downsampled from source).
/// The CLI preview is video-only, so `_audio_tracks` (the enabled monitor-mix tracks the libav player
/// honors) is ignored here; the parameter just keeps the two `start_player`s interchangeable.
#[cfg(not(feature = "libav-preview"))]
#[allow(clippy::too_many_arguments)]
pub fn start_player(
    path: &str,
    ffmpeg_path: &str,
    start_sec: f64,
    src_w: i64,
    src_h: i64,
    src_fps: f64,
    is_hdr: bool,
    _audio_tracks: Vec<(i64, f64)>,
) -> Option<Player> {
    let (w, h) = preview_dims(src_w, src_h);
    let fps = if src_fps.is_finite() && src_fps > 0.0 { src_fps.min(30.0) } else { 30.0 };
    let start = format!("{:.3}", start_sec.max(0.0));
    let vf = preview_vf(w, h, is_hdr, Some(fps));

    let mut child = hidden_command(ffmpeg_path)
        .args([
            "-hide_banner", "-loglevel", "error", "-ss", &start, "-i", path, "-an", "-vf", &vf,
            "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    let frame_bytes = (w as usize) * (h as usize) * 4;
    // Small buffer: enough to smooth jitter, small enough that backpressure paces the decoder.
    let (tx, rx) = sync_channel::<Vec<u8>>(3);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut buf = vec![0u8; frame_bytes];
            if reader.read_exact(&mut buf).is_err() {
                break; // EOF or the process was killed
            }
            if tx.send(buf).is_err() {
                break; // the Player (and its receiver) was dropped
            }
        }
    });

    Some(Player { child, rx, width: w, height: h, fps })
}

/// Run ffmpeg to export, streaming `-progress` into `progress` (0..1). On failure returns
/// the last few stderr lines.
pub fn run_export(
    ffmpeg_path: &str,
    args: &[String],
    total_sec: f64,
    progress: Arc<Mutex<f32>>,
) -> Result<(), String> {
    let mut child = hidden_command(ffmpeg_path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start ffmpeg ({ffmpeg_path}): {e}"))?;

    let stderr = child.stderr.take();
    let stderr_handle = std::thread::spawn(move || {
        let mut buffer = String::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_string(&mut buffer);
        }
        buffer
    });

    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let update = parse_progress(&line);
            if let Some(out_time) = update.out_time_sec {
                if let Ok(mut p) = progress.lock() {
                    *p = progress_fraction(Some(out_time), total_sec) as f32;
                }
            }
        }
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr_text = stderr_handle.join().unwrap_or_default();

    if status.success() {
        Ok(())
    } else {
        let lines: Vec<&str> = stderr_text.lines().collect();
        let tail = lines[lines.len().saturating_sub(8)..].join("\n");
        Err(if tail.is_empty() { format!("ffmpeg exited with status {status}") } else { tail })
    }
}

#[derive(Debug, Clone, Default)]
pub struct CapturePresets {
    pub obs: Option<String>,
    pub nvidia_share: Option<String>,
}

/// Read OBS `user.ini` + each profile's `basic.ini` from the per-OS config dir.
pub fn read_obs_config() -> ObsConfigFiles {
    let mut files = ObsConfigFiles::default();
    let Some(base) = dirs::config_dir().map(|d| d.join("obs-studio")) else {
        return files;
    };
    if let Ok(text) = std::fs::read_to_string(base.join("user.ini")) {
        files.user_ini = Some(text);
    }
    let profiles_dir = base.join("basic").join("profiles");
    if let Ok(entries) = std::fs::read_dir(&profiles_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if let Ok(text) = std::fs::read_to_string(path.join("basic.ini")) {
                files.profiles.push((name.to_string(), text));
            }
        }
    }
    files
}

#[cfg(windows)]
fn detect_nvidia_recording_dir() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS")
        .ok()?;
    let raw = key.get_raw_value("DefaultPathW").ok()?;
    let utf16: Vec<u16> = raw
        .bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let decoded = String::from_utf16_lossy(&utf16);
    let trimmed = decoded.trim_end_matches('\0').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(not(windows))]
fn detect_nvidia_recording_dir() -> Option<String> {
    None
}

pub fn detect_capture_presets() -> CapturePresets {
    let mut presets = CapturePresets::default();
    if let Some(obs) = detect_obs_recording_folder(&read_obs_config()) {
        presets.obs = Some(to_posix(&obs));
    }
    if let Some(nvidia) = detect_nvidia_recording_dir() {
        presets.nvidia_share = Some(to_posix(&nvidia));
    }
    presets
}

/// Holds the live notify watcher and a buffer of newly-created video paths, drained by the UI tick.
pub struct Watcher {
    _watcher: notify::RecommendedWatcher,
    buffer: Arc<Mutex<Vec<String>>>,
}

impl Watcher {
    /// Drain and return the paths discovered since the last call.
    pub fn drain(&self) -> Vec<String> {
        self.buffer.lock().map(|mut b| std::mem::take(&mut *b)).unwrap_or_default()
    }
}

/// Start watching `folders` (recursively) for new video files. Hold the returned [`Watcher`]
/// for the app's lifetime; dropping it stops watching.
pub fn start_watch(folders: &[String], extensions: &[String]) -> Option<Watcher> {
    use notify::{EventKind, RecursiveMode, Watcher as _};

    let buffer: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let exts: Vec<String> = extensions.iter().map(|e| e.to_lowercase()).collect();
    let sink = Arc::clone(&buffer);

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if !matches!(event.kind, EventKind::Create(_)) {
            return;
        }
        for path in event.paths {
            if path.is_file() && has_video_ext(&path, &exts) {
                if let Ok(mut b) = sink.lock() {
                    b.push(to_posix(&path.to_string_lossy()));
                }
            }
        }
    })
    .ok()?;

    for folder in folders {
        let _ = watcher.watch(Path::new(folder), RecursiveMode::Recursive);
    }

    Some(Watcher { _watcher: watcher, buffer })
}

/// Open a file/URL in its default handler.
pub fn open_external(target: &str) {
    let _ = open::that(target);
}

/// Reveal a file in the platform file manager (selecting it where supported).
pub fn reveal(path: &str) {
    #[cfg(windows)]
    {
        let _ = Command::new("explorer")
            .arg(format!("/select,{}", path.replace('/', "\\")))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").args(["-R", path]).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
        let _ = open::that(dir);
    }
}

/// Forward-slash path helpers (matching the web app's queue.ts).
pub fn base_name(path: &str) -> String {
    let n = path.replace('\\', "/");
    n.rsplit('/').next().unwrap_or(&n).to_string()
}

pub fn dir_name(path: &str) -> String {
    let n = path.replace('\\', "/");
    match n.rfind('/') {
        Some(0) | None => String::new(),
        Some(idx) => path[..idx].to_string(),
    }
}

pub fn join_path(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", dir.trim_end_matches(['/', '\\']), name)
    }
}

/// Per-file edit state persisted to `edits.json`, matching the web/C# `StoredEdit` shape.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<qlipq_core::edit_spec::EditSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_override: Option<qlipq_core::queue::OutputOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

pub type EditStore = HashMap<String, StoredEdit>;

pub fn load_edit_store() -> EditStore {
    read_app_file("edits.json")
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_edit_store(store: &EditStore) {
    if let Ok(json) = serde_json::to_string(store) {
        let _ = write_app_file("edits.json", &json);
    }
}
