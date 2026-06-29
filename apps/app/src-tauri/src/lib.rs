use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

/// Persisted configuration. Field names mirror `@qcksys/qlipq-core`'s `AppConfig`
/// (camelCase over the IPC boundary).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct AppConfig {
    watched_folders: Vec<String>,
    output_folder: String,
    video_extensions: Vec<String>,
    naming_template: String,
    ffmpeg_path: String,
    ffprobe_path: String,
    delete_source_after_export: bool,
    output: OutputSettings,
}

/// Mirrors `@qcksys/qlipq-core`'s `OutputSettings`. The frontend builds ffmpeg args
/// from these; Rust only round-trips them so they survive a config save.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct OutputSettings {
    quality_mode: String,
    quality_preset: String,
    crf: u32,
    video_bitrate_kbps: u32,
    encoder_preset: String,
    video_codec: String,
    container: String,
    fps: u32,
    max_height: u32,
    audio_bitrate_kbps: u32,
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
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(serde_json::from_str(&text).unwrap_or_default()),
        Err(_) => Ok(AppConfig::default()),
    }
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
