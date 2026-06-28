use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

/// Persisted configuration. Field names mirror `@qlipq/core`'s `AppConfig`
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

#[tauri::command]
fn scan_folders(folders: Vec<String>, extensions: Vec<String>) -> Vec<String> {
    let mut found = Vec::new();
    for folder in folders {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && has_video_ext(&path, &extensions) {
                found.push(path.to_string_lossy().to_string());
            }
        }
    }
    found
}

/// Build a Command, hiding the console window on Windows.
fn hidden_command(program: &str) -> Command {
    let cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = cmd;
        cmd.creation_flags(CREATE_NO_WINDOW);
        return cmd;
    }
    #[cfg(not(windows))]
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
fn run_ffmpeg(app: AppHandle, id: String, ffmpeg_path: String, args: Vec<String>) -> Result<(), String> {
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
        let tail: String = stderr_text.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
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
        // Ignore individual folder failures (e.g. a path that no longer exists).
        let _ = watcher.watch(Path::new(folder), RecursiveMode::NonRecursive);
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
            probe_raw,
            rename_file,
            delete_file,
            run_export,
            start_watching,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
