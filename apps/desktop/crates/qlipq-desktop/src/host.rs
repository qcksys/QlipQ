//! Host layer: the I/O the pure crates can't do — filesystem scan/watch, config/edits persistence,
//! and OBS/NVIDIA capture-folder detection. Media decode/probe/export all run in process via libav
//! (see [`crate::libav`] / [`crate::export`]); the host spawns no `ffmpeg`/`ffprobe` binary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
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

/// Write the JSON Schema for `config.json` next to it, so editors validate the config against the
/// relative `$schema` ref the app stamps. Called on startup; cheap to refresh.
pub fn write_config_schema() -> std::io::Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(config_json::SCHEMA_FILE),
        config_json::schema_json(),
    )
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

fn has_video_ext(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| extensions.iter().any(|x| x.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct ScanResult {
    pub paths: Vec<String>,
    /// Only complete scans can prove that an earlier recording has disappeared.
    pub complete_roots: Vec<String>,
}

/// Recursively collect video files, skipping symlinks/junctions.
pub fn scan_folders(folders: &[String], extensions: &[String]) -> ScanResult {
    let mut found = Vec::new();
    let mut complete_roots = Vec::new();
    for root in folders {
        let mut complete = true;
        let mut stack = vec![PathBuf::from(root)];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            for entry in entries {
                let Ok(entry) = entry else {
                    complete = false;
                    continue;
                };
                let Ok(file_type) = entry.file_type() else {
                    complete = false;
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
        if complete {
            complete_roots.push(to_posix(root));
        }
    }
    found.sort();
    found.dedup();
    ScanResult {
        paths: found,
        complete_roots,
    }
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
        out.push(MediaResolution {
            path: path.clone(),
            size,
            modified_ms,
            cached,
        });
    }
    out
}

pub fn file_exists(path: &str) -> bool {
    Path::new(path).is_file()
}

pub fn same_file(left: &str, right: &str) -> bool {
    let equal = |a: &str, b: &str| {
        if cfg!(windows) {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    if equal(&to_posix(left), &to_posix(right)) {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(a), Ok(b)) => equal(&a.to_string_lossy(), &b.to_string_lossy()),
        _ => false,
    }
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

/// Marks the library dirty after filesystem mutations, including directory moves and removals.
pub struct Watcher {
    _watcher: notify::RecommendedWatcher,
    dirty: Arc<std::sync::atomic::AtomicBool>,
}

impl Watcher {
    pub fn drain(&self) -> bool {
        self.dirty.swap(false, std::sync::atomic::Ordering::Relaxed)
    }
}

/// Start watching `folders` recursively for changes. Hold the returned [`Watcher`]
/// for the app's lifetime; dropping it stops watching.
pub fn start_watch(folders: &[String], _extensions: &[String]) -> Option<Watcher> {
    use notify::{EventKind, RecursiveMode, Watcher as _};

    let dirty = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sink = Arc::clone(&dirty);

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res
            .as_ref()
            .is_ok_and(|event| !event.need_rescan() && matches!(event.kind, EventKind::Access(_)))
        {
            return;
        }
        sink.store(true, std::sync::atomic::Ordering::Relaxed);
    })
    .ok()?;

    for folder in folders {
        let _ = watcher.watch(Path::new(folder), RecursiveMode::Recursive);
    }

    Some(Watcher {
        _watcher: watcher,
        dirty,
    })
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
        let _ = Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{win}\""))
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

/// Bump when a probed field is added, so caches written by an older build (which lack it) are
/// discarded and the library is re-probed once. Raised to 2 for the `encoder` tag (highlight
/// filtering) — old entries have no `encoder`, so without this they'd never be recognized.
const MEDIA_CACHE_VERSION: u32 = 2;

#[derive(serde::Deserialize)]
struct MediaCacheFile {
    version: u32,
    entries: MediaCache,
}

/// Borrowing counterpart for serialization (avoids cloning the whole cache on each debounced save).
#[derive(serde::Serialize)]
struct MediaCacheFileRef<'a> {
    version: u32,
    entries: &'a MediaCache,
}

pub fn load_media_cache() -> MediaCache {
    read_app_file("media-cache.json")
        .and_then(|t| serde_json::from_str::<MediaCacheFile>(&t).ok())
        .filter(|f| f.version == MEDIA_CACHE_VERSION)
        .map(|f| f.entries)
        .unwrap_or_default()
}

pub(crate) fn serialize_media_cache(cache: &MediaCache) -> serde_json::Result<String> {
    let file = MediaCacheFileRef {
        version: MEDIA_CACHE_VERSION,
        entries: cache,
    };
    serde_json::to_string(&file)
}

#[cfg(test)]
mod discovery_tests {
    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "qlipq-discovery-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn existing_recording_changes_invalidate_cached_metadata() {
        let dir = TestDir::new();
        let path = dir.0.join("recording.mp4");
        std::fs::write(&path, "early").unwrap();
        let path = to_posix(&path.to_string_lossy());
        let initial = resolve_media(std::slice::from_ref(&path), &MediaCache::new()).remove(0);
        let cache = MediaCache::from([(
            path.clone(),
            CachedMedia {
                size_bytes: initial.size,
                modified_ms: initial.modified_ms,
                media: MediaInfo {
                    duration_sec: 1.0,
                    width: 16,
                    height: 16,
                    video_codec: "h264".into(),
                    fps: 30.0,
                    audio_streams: vec![],
                    size_bytes: None,
                    encoder: None,
                },
                is_hdr: false,
            },
        )]);
        assert!(resolve_media(std::slice::from_ref(&path), &cache)[0]
            .cached
            .is_some());
        std::fs::write(&path, "complete recording").unwrap();
        let refreshed = resolve_media(std::slice::from_ref(&path), &cache).remove(0);
        assert_eq!(refreshed.size, 18);
        assert!(refreshed.cached.is_none());
    }

    #[test]
    fn completed_scans_reconcile_removals_but_failed_roots_do_not() {
        let dir = TestDir::new();
        let root = to_posix(&dir.0.to_string_lossy());
        let path = dir.0.join("recording.mp4");
        std::fs::write(&path, "recording").unwrap();
        let before = scan_folders(std::slice::from_ref(&root), &["mp4".into()]);
        assert_eq!(before.paths.len(), 1);
        std::fs::remove_file(&path).unwrap();
        let after = scan_folders(std::slice::from_ref(&root), &["mp4".into()]);
        assert_eq!(
            crate::discovery::missing_paths(
                before.paths.iter(),
                &after.paths,
                &after.complete_roots
            ),
            before.paths,
        );
        std::fs::write(&path, "not a directory").unwrap();
        let failed = scan_folders(&[to_posix(&path.to_string_lossy())], &["mp4".into()]);
        assert!(failed.complete_roots.is_empty());
    }

    #[test]
    fn unavailable_roots_preserve_their_recordings() {
        let dir = TestDir::new();
        let path = dir.0.join("temporarily-unavailable");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("recording.mp4"), "recording").unwrap();
        let root = to_posix(&path.to_string_lossy());
        let before = scan_folders(std::slice::from_ref(&root), &["mp4".into()]);
        assert_eq!(before.paths.len(), 1);
        std::fs::rename(&path, dir.0.join("available-elsewhere")).unwrap();
        let missing = scan_folders(std::slice::from_ref(&root), &["mp4".into()]);
        assert!(missing.complete_roots.is_empty());
        assert!(crate::discovery::missing_paths(
            before.paths.iter(),
            &missing.paths,
            &missing.complete_roots,
        )
        .is_empty());
    }
}
