//! Host layer: the I/O the pure crates can't do — filesystem scan/watch, config/edits persistence,
//! and OBS/NVIDIA capture-folder detection. Media decode/probe/export all run in process via libav
//! (see [`crate::libav`] / [`crate::export`]); the host spawns no `ffmpeg`/`ffprobe` binary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use qlipq_core::config::AppConfig;
use qlipq_core::config_json;
use qlipq_core::detect::{detect_obs_recording_folder, ObsConfigFiles};
use qlipq_core::media::MediaInfo;

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
    // Atomic write: stage into a unique temp file, then rename over the target. Several fire-and-
    // forget savers can hit the same JSON at once (e.g. edits.json from both persist_edit and
    // remove_item) and the process can exit mid-write; a plain truncating `write` would leave a torn
    // file, and since load parses with `unwrap_or_default` that silently wipes the whole store.
    // `rename` replaces atomically (on Windows too), so every reader sees the old or the new complete
    // file — never a partial one. Last writer wins, which is fine: each map is a full recent snapshot.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!("{name}.{}.{seq}.tmp", std::process::id()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, dir.join(name)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
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

/// One recording's on-disk stats paired with a still-valid cached probe, if any. `cached` is `Some`
/// when a [`MediaCache`] entry matches the file's current size + mtime, so it needs no re-probe.
#[derive(Debug, Clone)]
pub struct MediaResolution {
    pub path: String,
    pub size: i64,
    pub modified_ms: i64,
    pub cached: Option<CachedMedia>,
}

/// Stat each path and pair it with its cached probe when the file is unchanged (size + mtime match).
/// Runs on the blocking pool; the caller probes only the misses (`cached == None`), so a whole
/// backlog isn't re-probed on every launch.
pub fn resolve_media(paths: &[String], cache: &MediaCache) -> Vec<MediaResolution> {
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        let size = meta.len() as i64;
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let cached = cache
            .get(path)
            .filter(|c| c.size_bytes == size && c.modified_ms == modified_ms)
            .cloned();
        out.push(MediaResolution { path: path.clone(), size, modified_ms, cached });
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

/// Result of polling the preview player for the next decoded frame.
pub enum FramePoll {
    /// A frame is ready (raw RGBA, `width * height * 4` bytes).
    Frame(Vec<u8>),
    /// No frame ready yet (decoder still working).
    Empty,
    /// The decoder finished or died — playback should stop.
    Ended,
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
        // explorer's `/select,` parsing breaks when Rust quotes the whole argument (which it does as
        // soon as the path contains a space) — explorer then ignores it and opens the default folder.
        // Use `raw_arg` and quote only the path so explorer gets `/select,"C:\dir\file name.mp4"`.
        use std::os::windows::process::CommandExt;
        let win = path.replace('/', "\\");
        let _ = Command::new("explorer.exe").raw_arg(format!("/select,\"{win}\"")).spawn();
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

/// A cached probe result for one recording, persisted to `media-cache.json` so the queue's durations
/// (and other parsed metadata) survive restarts without re-probing every file. `size_bytes` +
/// `modified_ms` invalidate the entry when the file changes on disk, so a replaced/re-encoded
/// recording is re-probed rather than trusted.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedMedia {
    pub size_bytes: i64,
    pub modified_ms: i64,
    pub media: MediaInfo,
    pub is_hdr: bool,
}

pub type MediaCache = HashMap<String, CachedMedia>;

pub fn load_media_cache() -> MediaCache {
    read_app_file("media-cache.json")
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_media_cache(cache: &MediaCache) {
    if let Ok(json) = serde_json::to_string(cache) {
        let _ = write_app_file("media-cache.json", &json);
    }
}
