use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use garde::Validate;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DefaultOnError};
use tauri::{AppHandle, Emitter, Manager, State};

// Allowed values for the enum-like config strings; shared by garde validation and
// the repair pass so the "schema" lives in one place.
const QUALITY_MODES: &[&str] = &["preset", "crf", "bitrate", "vbr"];
const QUALITY_PRESETS: &[&str] = &["original", "high", "balanced", "small"];
const VIDEO_CODECS: &[&str] = &["libx264", "libx265"];
const CONTAINERS: &[&str] = &["mp4", "mkv"];

/// garde custom validator: value must be one of `allowed`.
fn one_of(allowed: &'static [&'static str]) -> impl Fn(&String, &()) -> garde::Result {
    move |value, _| {
        if allowed.contains(&value.as_str()) {
            Ok(())
        } else {
            Err(garde::Error::new(format!("must be one of {allowed:?}")))
        }
    }
}

/// Persisted configuration. Field names mirror `@qcksys/qlipq-core`'s `AppConfig`
/// (camelCase over the IPC boundary). Parsing is resilient: `DefaultOnError` reverts a
/// single bad field to its default instead of failing the whole parse, and `garde`
/// validates ranges/enums (see `get_config`).
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[serde(rename_all = "camelCase", default)]
struct AppConfig {
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(skip)]
    watched_folders: Vec<String>,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(skip)]
    output_folder: String,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(skip)]
    video_extensions: Vec<String>,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(skip)]
    naming_template: String,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(skip)]
    ffmpeg_path: String,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(skip)]
    ffprobe_path: String,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(skip)]
    delete_source_after_export: bool,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(dive)]
    output: OutputSettings,
}

/// Mirrors `@qcksys/qlipq-core`'s `OutputSettings`. The frontend builds ffmpeg args
/// from these; Rust round-trips and validates them.
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[serde(rename_all = "camelCase", default)]
struct OutputSettings {
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(custom(one_of(QUALITY_MODES)))]
    quality_mode: String,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(custom(one_of(QUALITY_PRESETS)))]
    quality_preset: String,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(range(min = 0, max = 51))]
    crf: u32,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(skip)]
    video_bitrate_kbps: u32,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(skip)]
    encoder_preset: String,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(custom(one_of(VIDEO_CODECS)))]
    video_codec: String,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(custom(one_of(CONTAINERS)))]
    container: String,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(skip)]
    fps: u32,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(skip)]
    max_height: u32,
    #[serde_as(deserialize_as = "DefaultOnError")]
    #[garde(skip)]
    audio_bitrate_kbps: u32,
}

/// Repair invalid values to defaults after a failed garde validation, keeping every
/// other (valid) field. Mirrors the garde rules above.
fn normalize_config(cfg: &mut AppConfig) {
    let d = OutputSettings::default();
    let o = &mut cfg.output;
    o.crf = o.crf.min(51);
    if !QUALITY_MODES.contains(&o.quality_mode.as_str()) {
        o.quality_mode = d.quality_mode.clone();
    }
    if !QUALITY_PRESETS.contains(&o.quality_preset.as_str()) {
        o.quality_preset = d.quality_preset.clone();
    }
    if !VIDEO_CODECS.contains(&o.video_codec.as_str()) {
        o.video_codec = d.video_codec.clone();
    }
    if !CONTAINERS.contains(&o.container.as_str()) {
        o.container = d.container.clone();
    }
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            quality_mode: "preset".to_string(),
            quality_preset: "original".to_string(),
            crf: 20,
            video_bitrate_kbps: 8000,
            encoder_preset: "veryfast".to_string(),
            video_codec: "libx264".to_string(),
            container: "mp4".to_string(),
            fps: 0,
            max_height: 0,
            audio_bitrate_kbps: 192,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            watched_folders: vec![],
            output_folder: String::new(),
            video_extensions: ["mp4", "mkv", "mov", "flv", "webm", "ts"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            naming_template: "{date}_{source}_{name}".to_string(),
            ffmpeg_path: "ffmpeg".to_string(),
            ffprobe_path: "ffprobe".to_string(),
            delete_source_after_export: false,
            output: OutputSettings::default(),
        }
    }
}

/// Holds the active filesystem watcher so it is not dropped (which stops it).
#[derive(Default)]
struct WatcherState(Mutex<Option<RecommendedWatcher>>);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportProgress {
    id: String,
    line: String,
}

fn config_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("config.json"))
}

#[tauri::command]
fn get_config(app: AppHandle) -> Result<AppConfig, String> {
    let path = config_path(&app)?;
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(AppConfig::default()); // no file yet
    };
    // Lenient parse: missing fields use serde defaults; a present-but-wrong-typed field
    // reverts to its default (serde_with DefaultOnError) instead of failing the whole
    // parse. unwrap_or_default() now only triggers on truly unparseable JSON.
    let mut cfg: AppConfig = serde_json::from_str(&text).unwrap_or_default();
    // Validate ranges/enums (garde); on failure keep the good fields by repairing offenders.
    if let Err(report) = cfg.validate() {
        eprintln!("config.json has invalid values, repairing:\n{report}");
        normalize_config(&mut cfg);
    }
    Ok(cfg)
}

#[tauri::command]
fn set_config(app: AppHandle, config: AppConfig) -> Result<(), String> {
    let path = config_path(&app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// Absolute path to the persisted config.json (for "open config file" in the UI).
#[tauri::command]
fn config_file_path(app: AppHandle) -> Result<String, String> {
    Ok(config_path(&app)?.to_string_lossy().to_string())
}

/// Read a named file from the app config dir (e.g. `edits.json`). Returns `None` if
/// it doesn't exist yet. `name` must be a bare filename (no path separators).
#[tauri::command]
fn read_app_file(app: AppHandle, name: String) -> Result<Option<String>, String> {
    if name.contains(['/', '\\']) || name.contains("..") {
        return Err("invalid file name".into());
    }
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    match std::fs::read_to_string(dir.join(&name)) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Write a named file into the app config dir. `name` must be a bare filename.
#[tauri::command]
fn write_app_file(app: AppHandle, name: String, contents: String) -> Result<(), String> {
    if name.contains(['/', '\\']) || name.contains("..") {
        return Err("invalid file name".into());
    }
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(&name), contents).map_err(|e| e.to_string())
}

/// Filesystem size + modified time for queue display. The `path` is echoed back
/// verbatim so the frontend can match results to its (forward-slash) queue paths.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileInfo {
    path: String,
    size: u64,
    modified_ms: i64,
}

#[tauri::command]
fn file_info(paths: Vec<String>) -> Vec<FileInfo> {
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        out.push(FileInfo {
            path,
            size: meta.len(),
            modified_ms,
        });
    }
    out
}

fn has_video_ext(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| extensions.iter().any(|x| x.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

/// Recursively collect video files in the given folders and all their subfolders
/// (NVIDIA Share, for one, nests recordings in per-game subfolders). Iterative to
/// avoid deep recursion; uses `file_type()` so symlinked dirs are not followed.
#[tauri::command]
fn scan_folders(folders: Vec<String>, extensions: Vec<String>) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack: Vec<PathBuf> = folders.into_iter().map(PathBuf::from).collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() && has_video_ext(&entry.path(), &extensions) {
                found.push(entry.path().to_string_lossy().to_string());
            }
        }
    }
    found
}

/// Raw OBS config files for the frontend to parse with `@qcksys/qlipq-core`'s
/// `detectObsRecordingFolder`. Empty when OBS is not installed.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ObsConfigFiles {
    user_ini: Option<String>,
    profiles: HashMap<String, String>,
}

/// Read OBS's `user.ini` and every profile's `basic.ini` from the standard config
/// directory (`%APPDATA%/obs-studio` on Windows). Returns raw text; parsing lives
/// in the core package. Missing files/dirs yield empty fields, never an error.
#[tauri::command]
fn read_obs_config(app: AppHandle) -> Result<ObsConfigFiles, String> {
    let base = app
        .path()
        .config_dir()
        .map_err(|e| e.to_string())?
        .join("obs-studio");

    let mut files = ObsConfigFiles::default();
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
                files.profiles.insert(name.to_string(), text);
            }
        }
    }

    Ok(files)
}

/// The folder NVIDIA Share (ShadowPlay) records into. It is not stored in any
/// plaintext config — only in the registry as a `REG_BINARY` UTF-16LE string at
/// `HKCU\Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS\DefaultPathW`.
/// Returns `None` off Windows or when NVIDIA Share is not present.
#[tauri::command]
fn detect_nvidia_recording_dir() -> Option<String> {
    #[cfg(windows)]
    {
        read_nvidia_default_path()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn read_nvidia_default_path() -> Option<String> {
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

/// Run `<path> -version` to confirm ffmpeg/ffprobe is reachable. Returns the first
/// line of output (the version banner) on success, or an error message.
#[tauri::command]
fn check_binary(path: String) -> Result<String, String> {
    let output = hidden_command(&path)
        .arg("-version")
        .output()
        .map_err(|e| format!("Not found ({path}): {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().next().unwrap_or("").trim().to_string())
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

#[tauri::command]
fn probe_raw(path: String, ffprobe_path: String) -> Result<String, String> {
    let output = hidden_command(&ffprobe_path)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            &path,
        ])
        .output()
        .map_err(|e| format!("Failed to run ffprobe ({ffprobe_path}): {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[tauri::command]
fn rename_file(from: String, to: String) -> Result<String, String> {
    if from != to && Path::new(&to).exists() {
        return Err(format!("A file already exists at {to}"));
    }
    std::fs::rename(&from, &to).map_err(|e| e.to_string())?;
    Ok(to)
}

#[tauri::command]
fn delete_file(path: String) -> Result<(), String> {
    std::fs::remove_file(&path).map_err(|e| e.to_string())
}

#[tauri::command]
async fn run_export(
    app: AppHandle,
    id: String,
    ffmpeg_path: String,
    args: Vec<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || run_ffmpeg(app, id, ffmpeg_path, args))
        .await
        .map_err(|e| e.to_string())?
}

/// Spawn ffmpeg, stream `-progress` lines to the frontend, and collect stderr
/// on a separate thread to avoid pipe-buffer deadlocks.
fn run_ffmpeg(
    app: AppHandle,
    id: String,
    ffmpeg_path: String,
    args: Vec<String>,
) -> Result<(), String> {
    let mut child = hidden_command(&ffmpeg_path)
        .args(&args)
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
            let _ = app.emit(
                "export-progress",
                ExportProgress {
                    id: id.clone(),
                    line,
                },
            );
        }
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr_text = stderr_handle.join().unwrap_or_default();

    if status.success() {
        Ok(())
    } else {
        // Surface the last few stderr lines (ffmpeg puts the real error there).
        let lines: Vec<&str> = stderr_text.lines().collect();
        let tail = lines[lines.len().saturating_sub(8)..].join("\n");
        Err(if tail.is_empty() {
            format!("ffmpeg exited with status {status}")
        } else {
            tail
        })
    }
}

#[tauri::command]
fn start_watching(
    app: AppHandle,
    state: State<WatcherState>,
    folders: Vec<String>,
    extensions: Vec<String>,
) -> Result<(), String> {
    let exts: Vec<String> = extensions.iter().map(|e| e.to_lowercase()).collect();
    let emitter = app.clone();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if !matches!(event.kind, EventKind::Create(_)) {
            return;
        }
        for path in event.paths {
            if path.is_file() && has_video_ext(&path, &exts) {
                let _ = emitter.emit("file-added", path.to_string_lossy().to_string());
            }
        }
    })
    .map_err(|e| e.to_string())?;

    for folder in &folders {
        // Recursive so new files in subfolders (e.g. NVIDIA Share's per-game dirs)
        // are detected live. Ignore individual folder failures (e.g. a missing path).
        let _ = watcher.watch(Path::new(folder), RecursiveMode::Recursive);
    }

    *state.0.lock().map_err(|e| e.to_string())? = Some(watcher);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(WatcherState::default())
        .invoke_handler(tauri::generate_handler![
            get_config,
            set_config,
            config_file_path,
            read_app_file,
            write_app_file,
            file_info,
            check_binary,
            scan_folders,
            read_obs_config,
            detect_nvidia_recording_dir,
            probe_raw,
            rename_file,
            delete_file,
            run_export,
            start_watching,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
