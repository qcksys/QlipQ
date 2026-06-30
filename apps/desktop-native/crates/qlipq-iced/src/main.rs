#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! qlipq — cross-platform recording queue + FFmpeg clip editor (iced front-end).
//!
//! The pure crates (`qlipq-core`, `qlipq-ffmpeg`) build the ffmpeg command lines and parse
//! output; this binary is the host + UI. Video preview uses ffmpeg frame extraction (a frame is
//! decoded at the playhead) rather than a media framework, keeping the build dependency-light and
//! portable — the export cut comes from ffmpeg `-ss`/`-t`, the preview is an advisory guide.

mod host;
mod iso;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::widget::{
    button, checkbox, column, container, image, pick_list, progress_bar, row, scrollable, slider,
    text, text_input, Space,
};
use iced::{Element, Length, Subscription, Task, Theme};

use qlipq_core::config::*;
use qlipq_core::edit_spec::{AudioTrackSpec, CropSpec, EditSpec, TrimSpec};
use qlipq_core::media::{audio_stream_label, format_bytes, MediaInfo};
use qlipq_core::{datetimes, queue::*, rename};
use qlipq_ffmpeg::args::*;
use qlipq_ffmpeg::estimate::estimate_export_size;

const DISMISSED_TAG: &str = "dismissed";
const TICK: Duration = Duration::from_millis(250);

/// Caps concurrent background duration probes so the editor's on-demand probe (and the system)
/// are never starved by a folder full of recordings. Mirrors the web app's PROBE_CONCURRENCY=3.
static PROBE_SEM: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("QlipQ")
        .subscription(App::subscription)
        .theme(App::theme)
        .run()
}

// ---- pick_list choice enums (label + core conversions) ----

macro_rules! choice {
    ($name:ident, $core:ty, { $($variant:ident => ($label:expr, $val:expr)),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum $name { $($variant),+ }
        impl $name {
            const ALL: &'static [$name] = &[$($name::$variant),+];
            fn label(self) -> &'static str { match self { $($name::$variant => $label),+ } }
            fn to_core(self) -> $core { match self { $($name::$variant => $val),+ } }
            fn from_core(v: $core) -> Self { $(if v == $val { return $name::$variant; })+ Self::ALL[0] }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.label()) }
        }
    };
}

choice!(QmChoice, QualityMode, {
    Preset => ("Preset", QualityMode::Preset),
    Crf => ("Custom quality (CRF)", QualityMode::Crf),
    Vbr => ("VBR (quality + max bitrate)", QualityMode::Vbr),
    Bitrate => ("Target bitrate", QualityMode::Bitrate),
});
choice!(QpChoice, QualityPreset, {
    Original => ("Original — no re-encode", QualityPreset::Original),
    High => ("High", QualityPreset::High),
    Balanced => ("Balanced", QualityPreset::Balanced),
    Small => ("Small", QualityPreset::Small),
});
choice!(CodecChoice, VideoCodecChoice, {
    H264 => ("H.264", VideoCodecChoice::Libx264),
    H265 => ("H.265 (smaller, slower)", VideoCodecChoice::Libx265),
});
choice!(ContainerChoice, ContainerFormat, {
    Mp4 => ("mp4", ContainerFormat::Mp4),
    Mkv => ("mkv", ContainerFormat::Mkv),
});
choice!(FpsChoice, i64, {
    Source => ("Source", 0),
    Sixty => ("60", 60),
    Thirty => ("30", 30),
});
choice!(ResChoice, i64, {
    Source => ("Source", 0),
    K4 => ("4K (2160p)", 2160),
    P1440 => ("1440p", 1440),
    P1080 => ("1080p", 1080),
    P720 => ("720p", 720),
});
choice!(AudioKbpsChoice, i64, {
    K128 => ("128 kbps", 128),
    K192 => ("192 kbps", 192),
    K256 => ("256 kbps", 256),
});
choice!(AfterChoice, AfterExportAction, {
    Nothing => ("Do nothing", AfterExportAction::Nothing),
    Delete => ("Delete", AfterExportAction::Delete),
    Move => ("Move to folder", AfterExportAction::Move),
    Rename => ("Rename (prefix/suffix)", AfterExportAction::Rename),
    Prompt => ("Prompt each time", AfterExportAction::Prompt),
});

const ENCODER_PRESETS: [&str; 9] = [
    "ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow", "slower", "veryslow",
];

#[derive(Debug, Clone, Copy)]
enum View {
    Queue,
    Settings,
}

#[derive(Debug, Clone, Copy)]
enum PickPurpose {
    WatchedFolder,
    OutputFolder,
    MoveFolder,
}

struct AudioRow {
    index: i64,
    label: String,
    detail: String,
    enabled: bool,
    volume: f64,
}

struct Editor {
    item_id: String,
    media: Option<MediaInfo>,
    load_error: Option<String>,
    trim_start: f64,
    trim_end: f64,
    crop_enabled: bool,
    crop: CropSpec,
    audio: Vec<AudioRow>,
    current_time: f64,
    frame: Option<image::Handle>,
    frame_dirty: bool,
    extracting: bool,
    playing: bool,
    exporting: bool,
    progress: Arc<Mutex<f32>>,
    progress_display: f32,
    overwrite_target: Option<String>,
    after_prompt: bool,
}

struct RenameState {
    id: String,
    value: String,
}

struct App {
    config: AppConfig,
    items: Vec<QueueItem>,
    known_paths: HashSet<String>,
    edit_store: host::EditStore,
    selected_id: Option<String>,
    view: View,
    tag_filter: Option<String>,
    presets: host::CapturePresets,
    watcher: Option<host::Watcher>,
    editor: Option<Editor>,
    audio_defaults: Vec<AudioTrackSpec>,
    ffmpeg_test: Option<(bool, String)>,
    ffprobe_test: Option<(bool, String)>,
    rename: Option<RenameState>,
    delete_confirm: Option<String>,
    new_tag: String,
    export_target: Option<String>,
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
    ShowQueue,
    ShowSettings,
    OpenRepo,
    OpenFfmpeg,
    RescanAll,
    Scanned(Vec<String>),
    FileInfoLoaded(Vec<(String, i64, i64)>),
    PresetsDetected(Option<String>, Option<String>),
    SelectItem(String),
    MediaProbed(String, Result<MediaInfo, String>),
    FrameExtracted(String, Option<(u32, u32, Vec<u8>)>),
    Seek(f64),
    Skip(f64),
    TogglePlay,
    SetIn,
    SetOut,
    ToggleCrop(bool),
    CropEdited(u8, String),
    AudioToggle(i64, bool),
    AudioVolume(i64, f64),
    ToggleOverride(bool),
    OverrideQm(QmChoice),
    OverrideQp(QpChoice),
    OverrideCrf(String),
    OverrideBitrate(String),
    NewTagChanged(String),
    AddTag,
    RemoveTag(String),
    Export,
    ExportFinished(String, Result<(), String>),
    AfterChoice(AfterExportAction),
    Overwrite(u8),
    ShowExported,
    RenameOpen(String),
    RenameValue(String),
    RenameTemplate,
    RenameConfirm,
    RenameCancel,
    RequestDelete(String),
    DeleteConfirm,
    DeleteCancel,
    Deleted(String, Result<(), String>),
    Dismiss(String),
    SetTagFilter(Option<String>),
    PickFolder(PickPurpose),
    FolderPicked(PickPurpose, Option<String>),
    RemoveFolder(String),
    Reprocess(String),
    AddPreset(String),
    OutputFolderChanged(String),
    NamingChanged(String),
    FfmpegPathChanged(String),
    FfprobePathChanged(String),
    TestFfmpeg,
    TestFfprobe,
    BinaryTested(bool, Result<String, String>),
    SetQm(QmChoice),
    SetQp(QpChoice),
    SetCrf(String),
    SetBitrate(String),
    SetEncoder(String),
    SetCodec(CodecChoice),
    SetContainer(ContainerChoice),
    SetFps(FpsChoice),
    SetRes(ResChoice),
    SetAudioKbps(AudioKbpsChoice),
    SetAfter(AfterChoice),
    MoveFolderChanged(String),
    RenamePrefixChanged(String),
    RenameSuffixChanged(String),
    OpenConfigFile,
    Ignore,
}

/// Offload blocking host work onto tokio's blocking pool.
async fn blocking<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).await.expect("blocking task panicked")
}

impl App {
    fn new() -> (Self, Task<Message>) {
        host::migrate_legacy_data();
        let config = host::load_config();
        let edit_store = host::load_edit_store();
        let watcher = host::start_watch(&config.watched_folders, &config.video_extensions);

        let app = App {
            config,
            items: Vec::new(),
            known_paths: HashSet::new(),
            edit_store,
            selected_id: None,
            view: View::Queue,
            tag_filter: None,
            presets: host::CapturePresets::default(),
            watcher,
            editor: None,
            audio_defaults: Vec::new(),
            ffmpeg_test: None,
            ffprobe_test: None,
            rename: None,
            delete_confirm: None,
            new_tag: String::new(),
            export_target: None,
        };

        let folders = app.config.watched_folders.clone();
        let exts = app.config.video_extensions.clone();
        let scan = Task::perform(blocking(move || host::scan_folders(&folders, &exts)), Message::Scanned);
        let presets = Task::perform(
            blocking(host::detect_capture_presets),
            |p| Message::PresetsDetected(p.obs, p.nvidia_share),
        );
        (app, Task::batch([scan, presets]))
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::time::every(TICK).map(|_| Message::Tick)
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn save_config_task(&self) -> Task<Message> {
        let cfg = self.config.clone();
        Task::perform(blocking(move || { let _ = host::save_config(&cfg); }), |_| Message::Ignore)
    }

    fn restart_watch_and_scan(&mut self) -> Task<Message> {
        self.watcher = host::start_watch(&self.config.watched_folders, &self.config.video_extensions);
        let folders = self.config.watched_folders.clone();
        let exts = self.config.video_extensions.clone();
        Task::perform(blocking(move || host::scan_folders(&folders, &exts)), Message::Scanned)
    }

    fn persist_edit(&mut self, id: &str) {
        if let Some(item) = self.items.iter().find(|i| i.id == id) {
            self.edit_store.insert(
                item.path.clone(),
                host::StoredEdit {
                    edit: item.edit.clone(),
                    output_override: item.output_override.clone(),
                    tags: item.tags.clone().filter(|t| !t.is_empty()),
                },
            );
            let store = self.edit_store.clone();
            // Fire-and-forget save (small file).
            std::thread::spawn(move || host::save_edit_store(&store));
        }
    }

    fn add_paths(&mut self, paths: Vec<String>) -> Task<Message> {
        let mut fresh = Vec::new();
        for raw in paths {
            let path = host::to_posix(&raw);
            if self.known_paths.insert(path.clone()) {
                fresh.push(path);
            }
        }
        if fresh.is_empty() {
            return Task::none();
        }
        let roots = self.config.watched_folders.clone();
        let mut new_items = Vec::new();
        for path in &fresh {
            let mut item = build_item(path, &roots);
            if let Some(stored) = self.edit_store.get(path) {
                item.edit = stored.edit.clone();
                item.output_override = stored.output_override.clone();
                item.tags = stored.tags.clone();
            }
            new_items.push(item);
        }
        // Newest first.
        for item in new_items.into_iter().rev() {
            self.items.insert(0, item);
        }
        let ffprobe = self.config.ffprobe_path.clone();
        let to_probe = fresh.clone();
        let info = Task::perform(
            blocking(move || {
                host::file_info(&fresh).into_iter().map(|f| (f.path, f.size, f.modified_ms)).collect()
            }),
            Message::FileInfoLoaded,
        );
        // Background duration probing, capped at PROBE_SEM permits so a large folder doesn't
        // saturate the blocking pool and starve the editor's on-demand probe.
        let durations = Task::batch(to_probe.into_iter().map(move |path| {
            let ffprobe = ffprobe.clone();
            let id_path = path.clone();
            Task::perform(
                async move {
                    let _permit = PROBE_SEM.acquire().await;
                    blocking(move || host::probe(&path, &ffprobe)).await
                },
                move |res| {
                    Message::FileInfoLoaded(match res {
                        Ok(m) => vec![(format!("dur:{id_path}"), m.duration_sec as i64, m.duration_sec.to_bits() as i64)],
                        Err(_) => vec![],
                    })
                },
            )
        }));
        Task::batch([info, durations])
    }

    fn effective_output(&self, item: &QueueItem) -> OutputSettings {
        let mut out = self.config.output.clone();
        if let Some(o) = &item.output_override {
            if let Some(v) = o.quality_mode { out.quality_mode = v; }
            if let Some(v) = o.quality_preset { out.quality_preset = v; }
            if let Some(v) = o.crf { out.crf = v; }
            if let Some(v) = o.video_bitrate_kbps { out.video_bitrate_kbps = v; }
            if let Some(v) = &o.encoder_preset { out.encoder_preset = v.clone(); }
            if let Some(v) = o.video_codec { out.video_codec = v; }
            if let Some(v) = o.container { out.container = v; }
            if let Some(v) = o.fps { out.fps = v; }
            if let Some(v) = o.max_height { out.max_height = v; }
            if let Some(v) = o.audio_bitrate_kbps { out.audio_bitrate_kbps = v; }
        }
        out
    }

    fn commit_spec(&mut self) {
        let Some(ed) = &self.editor else { return };
        let id = ed.item_id.clone();
        let spec = editor_spec(ed);
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.edit = Some(spec);
        }
        self.persist_edit(&id);
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => return self.on_tick(),
            Message::ShowQueue => self.view = View::Queue,
            Message::ShowSettings => self.view = View::Settings,
            Message::OpenRepo => host::open_external("https://github.com/qcksys/qlipq"),
            Message::OpenFfmpeg => host::open_external("https://ffmpeg.org"),
            Message::RescanAll => {
                let folders = self.config.watched_folders.clone();
                let exts = self.config.video_extensions.clone();
                return Task::perform(blocking(move || host::scan_folders(&folders, &exts)), Message::Scanned);
            }
            Message::Scanned(paths) => return self.add_paths(paths),
            Message::FileInfoLoaded(infos) => {
                for (path, size, modified) in infos {
                    if let Some(rest) = path.strip_prefix("dur:") {
                        let dur = f64::from_bits(modified as u64);
                        if let Some(item) = self.items.iter_mut().find(|i| i.path == rest) {
                            if item.duration_sec.is_none() {
                                item.duration_sec = Some(dur);
                            }
                        }
                    } else if let Some(item) = self.items.iter_mut().find(|i| i.path == path) {
                        item.file_size_bytes = Some(size);
                        item.file_modified_at = Some(iso::from_unix_ms(modified));
                    }
                }
            }
            Message::PresetsDetected(obs, nvidia) => {
                self.presets = host::CapturePresets { obs, nvidia_share: nvidia };
            }
            Message::SelectItem(id) => return self.select_item(id),
            Message::MediaProbed(id, result) => self.on_media_probed(id, result),
            Message::FrameExtracted(id, frame) => {
                if let Some(ed) = &mut self.editor {
                    if ed.item_id == id {
                        ed.extracting = false;
                        if let Some((w, h, rgba)) = frame {
                            ed.frame = Some(image::Handle::from_rgba(w, h, rgba));
                        }
                    }
                }
            }
            Message::Seek(sec) => {
                if let Some(ed) = &mut self.editor {
                    ed.current_time = sec;
                    ed.frame_dirty = true;
                }
            }
            Message::Skip(delta) => {
                if let Some(ed) = &mut self.editor {
                    let max = ed.media.as_ref().map(|m| m.duration_sec).unwrap_or(0.0);
                    ed.current_time = (ed.current_time + delta).clamp(0.0, max);
                    ed.frame_dirty = true;
                }
            }
            Message::TogglePlay => {
                if let Some(ed) = &mut self.editor {
                    ed.playing = !ed.playing;
                }
            }
            Message::SetIn => {
                if let Some(ed) = &mut self.editor {
                    ed.trim_start = ed.current_time.min(ed.trim_end - 0.1).clamp(0.0, ed.trim_end);
                }
                self.commit_spec();
            }
            Message::SetOut => {
                if let Some(ed) = &mut self.editor {
                    let max = ed.media.as_ref().map(|m| m.duration_sec).unwrap_or(ed.trim_end);
                    ed.trim_end = ed.current_time.max(ed.trim_start + 0.1).clamp(0.0, max);
                }
                self.commit_spec();
            }
            Message::ToggleCrop(on) => {
                if let Some(ed) = &mut self.editor {
                    ed.crop_enabled = on;
                    if on {
                        if let Some(m) = &ed.media {
                            if ed.crop.width <= 0 {
                                ed.crop = CropSpec { x: 0, y: 0, width: m.width, height: m.height };
                            }
                        }
                    }
                }
                self.commit_spec();
            }
            Message::CropEdited(field, value) => {
                if let (Some(ed), Ok(v)) = (&mut self.editor, value.parse::<i64>()) {
                    match field {
                        0 => ed.crop.x = v,
                        1 => ed.crop.y = v,
                        2 => ed.crop.width = v,
                        _ => ed.crop.height = v,
                    }
                }
                self.commit_spec();
            }
            Message::AudioToggle(index, on) => {
                if let Some(ed) = &mut self.editor {
                    if let Some(r) = ed.audio.iter_mut().find(|r| r.index == index) {
                        r.enabled = on;
                    }
                    self.audio_defaults = editor_audio_specs(ed);
                }
                self.commit_spec();
            }
            Message::AudioVolume(index, vol) => {
                if let Some(ed) = &mut self.editor {
                    if let Some(r) = ed.audio.iter_mut().find(|r| r.index == index) {
                        r.volume = vol;
                    }
                    self.audio_defaults = editor_audio_specs(ed);
                }
                self.commit_spec();
            }
            Message::ToggleOverride(on) => return self.toggle_override(on),
            Message::OverrideQm(c) => return self.patch_override(|o| o.quality_mode = Some(c.to_core())),
            Message::OverrideQp(c) => return self.patch_override(|o| o.quality_preset = Some(c.to_core())),
            Message::OverrideCrf(s) => {
                if let Ok(v) = s.parse::<i64>() {
                    return self.patch_override(move |o| o.crf = Some(v.clamp(0, 51)));
                }
            }
            Message::OverrideBitrate(s) => {
                if let Ok(v) = s.parse::<i64>() {
                    return self.patch_override(move |o| o.video_bitrate_kbps = Some(v.max(100)));
                }
            }
            Message::NewTagChanged(s) => self.new_tag = s,
            Message::AddTag => {
                let t = self.new_tag.trim().to_string();
                self.new_tag.clear();
                if !t.is_empty() {
                    if let Some(id) = self.selected_id.clone() {
                        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
                            let tags = item.tags.get_or_insert_with(Vec::new);
                            if !tags.contains(&t) {
                                tags.push(t);
                            }
                        }
                        self.persist_edit(&id);
                    }
                }
            }
            Message::RemoveTag(t) => {
                if let Some(id) = self.selected_id.clone() {
                    if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
                        if let Some(tags) = &mut item.tags {
                            tags.retain(|x| x != &t);
                        }
                    }
                    self.persist_edit(&id);
                }
            }
            Message::Export => return self.start_export(false),
            Message::Overwrite(choice) => {
                if let Some(ed) = &mut self.editor {
                    let target = ed.overwrite_target.take();
                    if let Some(target) = target {
                        match choice {
                            0 => return self.run_export_to(target),
                            1 => return self.run_export_to(append_timestamp(&target)),
                            _ => {}
                        }
                    }
                }
            }
            Message::ExportFinished(id, result) => return self.on_export_finished(id, result),
            Message::AfterChoice(action) => {
                if let Some(ed) = &mut self.editor {
                    ed.after_prompt = false;
                }
                return self.run_after_action(action);
            }
            Message::ShowExported => {
                if let Some(id) = &self.selected_id {
                    if let Some(item) = self.items.iter().find(|i| &i.id == id) {
                        if let Some(p) = &item.export_path {
                            host::reveal(p);
                        }
                    }
                }
            }
            Message::RenameOpen(id) => {
                if let Some(item) = self.items.iter().find(|i| i.id == id) {
                    let (name, _) = rename::split_file_name(&item.file_name);
                    self.rename = Some(RenameState { id, value: name });
                }
            }
            Message::RenameValue(v) => {
                if let Some(r) = &mut self.rename {
                    r.value = v;
                }
            }
            Message::RenameTemplate => {
                if let Some(r) = &mut self.rename {
                    if let Some(item) = self.items.iter().find(|i| i.id == r.id) {
                        let (name, ext) = rename::split_file_name(&item.file_name);
                        let vars = rename::RenameVars {
                            name,
                            ext,
                            recorded_at: item.recorded_at.as_deref().and_then(iso::to_local),
                            source: item.source.clone(),
                            index: None,
                        };
                        let suggested = rename::build_renamed_file_name(&self.config.naming_template, &vars);
                        r.value = rename::split_file_name(&suggested).0;
                    }
                }
            }
            Message::RenameConfirm => return self.confirm_rename(),
            Message::RenameCancel => self.rename = None,
            Message::RequestDelete(id) => self.delete_confirm = Some(id),
            Message::DeleteConfirm => {
                if let Some(id) = self.delete_confirm.take() {
                    if let Some(item) = self.items.iter().find(|i| i.id == id) {
                        let path = item.path.clone();
                        return Task::perform(blocking(move || host::delete_file(&path)), move |r| Message::Deleted(id.clone(), r));
                    }
                }
            }
            Message::DeleteCancel => self.delete_confirm = None,
            Message::Deleted(id, result) => {
                if result.is_ok() {
                    self.remove_item(&id);
                }
            }
            Message::Dismiss(id) => {
                if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
                    let tags = item.tags.get_or_insert_with(Vec::new);
                    if let Some(pos) = tags.iter().position(|t| t == DISMISSED_TAG) {
                        tags.remove(pos);
                    } else {
                        tags.push(DISMISSED_TAG.to_string());
                        if self.selected_id.as_deref() == Some(&id) {
                            self.selected_id = None;
                            self.editor = None;
                        }
                    }
                }
                self.persist_edit(&id);
            }
            Message::SetTagFilter(t) => self.tag_filter = t,
            Message::PickFolder(purpose) => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .pick_folder()
                            .await
                            .map(|h| host::to_posix(&h.path().to_string_lossy()))
                    },
                    move |opt| Message::FolderPicked(purpose, opt),
                );
            }
            Message::FolderPicked(purpose, Some(path)) => return self.on_folder_picked(purpose, path),
            Message::FolderPicked(_, None) => {}
            Message::RemoveFolder(folder) => {
                self.config.watched_folders.retain(|f| f != &folder);
                return Task::batch([self.save_config_task(), self.restart_watch_and_scan()]);
            }
            Message::Reprocess(folder) => {
                let exts = self.config.video_extensions.clone();
                self.view = View::Queue;
                return Task::perform(blocking(move || host::scan_folders(&[folder], &exts)), Message::Scanned);
            }
            Message::AddPreset(folder) => return self.add_watched_folder(folder),
            Message::OutputFolderChanged(s) => { self.config.output_folder = s; return self.save_config_task(); }
            Message::NamingChanged(s) => { self.config.naming_template = s; return self.save_config_task(); }
            Message::FfmpegPathChanged(s) => { self.config.ffmpeg_path = s; return self.save_config_task(); }
            Message::FfprobePathChanged(s) => { self.config.ffprobe_path = s; return self.save_config_task(); }
            Message::TestFfmpeg => {
                self.ffmpeg_test = Some((true, "Testing…".into()));
                let path = self.config.ffmpeg_path.clone();
                return Task::perform(blocking(move || host::check_binary(&path)), |r| Message::BinaryTested(true, r));
            }
            Message::TestFfprobe => {
                self.ffprobe_test = Some((true, "Testing…".into()));
                let path = self.config.ffprobe_path.clone();
                return Task::perform(blocking(move || host::check_binary(&path)), |r| Message::BinaryTested(false, r));
            }
            Message::BinaryTested(is_ffmpeg, result) => {
                let entry = match &result {
                    Ok(v) => (true, if v.is_empty() { "OK".to_string() } else { v.clone() }),
                    Err(e) => (false, e.clone()),
                };
                if is_ffmpeg { self.ffmpeg_test = Some(entry); } else { self.ffprobe_test = Some(entry); }
            }
            Message::SetQm(c) => { self.config.output.quality_mode = c.to_core(); return self.save_config_task(); }
            Message::SetQp(c) => { self.config.output.quality_preset = c.to_core(); return self.save_config_task(); }
            Message::SetCrf(s) => { if let Ok(v) = s.parse::<i64>() { self.config.output.crf = v.clamp(0, 51); return self.save_config_task(); } }
            Message::SetBitrate(s) => { if let Ok(v) = s.parse::<i64>() { self.config.output.video_bitrate_kbps = v.max(100); return self.save_config_task(); } }
            Message::SetEncoder(s) => { self.config.output.encoder_preset = s; return self.save_config_task(); }
            Message::SetCodec(c) => { self.config.output.video_codec = c.to_core(); return self.save_config_task(); }
            Message::SetContainer(c) => { self.config.output.container = c.to_core(); return self.save_config_task(); }
            Message::SetFps(c) => { self.config.output.fps = c.to_core(); return self.save_config_task(); }
            Message::SetRes(c) => { self.config.output.max_height = c.to_core(); return self.save_config_task(); }
            Message::SetAudioKbps(c) => { self.config.output.audio_bitrate_kbps = c.to_core(); return self.save_config_task(); }
            Message::SetAfter(c) => { self.config.after_export.action = c.to_core(); return self.save_config_task(); }
            Message::MoveFolderChanged(s) => { self.config.after_export.move_folder = s; return self.save_config_task(); }
            Message::RenamePrefixChanged(s) => { self.config.after_export.rename_prefix = s; return self.save_config_task(); }
            Message::RenameSuffixChanged(s) => { self.config.after_export.rename_suffix = s; return self.save_config_task(); }
            Message::OpenConfigFile => {
                let cfg = self.config.clone();
                return Task::perform(blocking(move || { let _ = host::save_config(&cfg); host::reveal(&host::config_path().to_string_lossy()); }), |_| Message::Ignore);
            }
            Message::Ignore => {}
        }
        Task::none()
    }

    fn on_tick(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        // Drain the folder watcher.
        if let Some(w) = &self.watcher {
            let new = w.drain();
            if !new.is_empty() {
                tasks.push(self.add_paths(new));
            }
        }
        // Advance playback and refresh the preview frame.
        let mut extract: Option<(String, String, String, f64)> = None; // id, path, ffmpeg, time
        if let Some(ed) = &mut self.editor {
            if ed.exporting {
                if let Ok(p) = ed.progress.lock() {
                    ed.progress_display = *p;
                }
            }
            if ed.playing {
                if let Some(m) = &ed.media {
                    ed.current_time = (ed.current_time + TICK.as_secs_f64()).min(m.duration_sec);
                    ed.frame_dirty = true;
                    if ed.current_time >= m.duration_sec {
                        ed.playing = false;
                    }
                }
            }
            if ed.frame_dirty && !ed.extracting && ed.media.is_some() {
                ed.frame_dirty = false;
                ed.extracting = true;
                if let Some(item) = self.items.iter().find(|i| i.id == ed.item_id) {
                    extract = Some((ed.item_id.clone(), item.path.clone(), self.config.ffmpeg_path.clone(), ed.current_time));
                }
            }
        }
        if let Some((id, path, ffmpeg, time)) = extract {
            tasks.push(Task::perform(
                blocking(move || host::extract_frame(&path, &ffmpeg, time).ok()),
                move |frame| Message::FrameExtracted(id.clone(), frame),
            ));
        }
        Task::batch(tasks)
    }

    fn select_item(&mut self, id: String) -> Task<Message> {
        self.selected_id = Some(id.clone());
        let Some(item) = self.items.iter().find(|i| i.id == id) else {
            return Task::none();
        };
        self.editor = Some(Editor {
            item_id: id.clone(),
            media: None,
            load_error: None,
            trim_start: 0.0,
            trim_end: 0.0,
            crop_enabled: false,
            crop: CropSpec { x: 0, y: 0, width: 0, height: 0 },
            audio: Vec::new(),
            current_time: 0.0,
            frame: None,
            frame_dirty: false,
            extracting: false,
            playing: false,
            exporting: false,
            progress: Arc::new(Mutex::new(0.0)),
            progress_display: 0.0,
            overwrite_target: None,
            after_prompt: false,
        });
        let path = item.path.clone();
        let ffprobe = self.config.ffprobe_path.clone();
        Task::perform(blocking(move || host::probe(&path, &ffprobe)), move |r| Message::MediaProbed(id.clone(), r))
    }

    fn on_media_probed(&mut self, id: String, result: Result<MediaInfo, String>) {
        let Some(ed) = &mut self.editor else { return };
        if ed.item_id != id {
            return;
        }
        match result {
            Err(e) => ed.load_error = Some(e),
            Ok(media) => {
                let stored_edit = self.items.iter().find(|i| i.id == id).and_then(|i| i.edit.clone());
                if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
                    item.duration_sec = Some(media.duration_sec);
                }
                let spec = stored_edit.unwrap_or_else(|| EditSpec {
                    trim: Some(TrimSpec { start_sec: 0.0, end_sec: media.duration_sec }),
                    crop: None,
                    audio_tracks: qlipq_core::edit_spec::default_edit_spec(Some(&media)).audio_tracks,
                });
                ed.trim_start = spec.trim.as_ref().map(|t| t.start_sec).unwrap_or(0.0);
                ed.trim_end = spec.trim.as_ref().map(|t| t.end_sec).unwrap_or(media.duration_sec);
                if let Some(c) = &spec.crop {
                    ed.crop_enabled = true;
                    ed.crop = c.clone();
                } else {
                    ed.crop = CropSpec { x: 0, y: 0, width: media.width, height: media.height };
                }
                ed.audio = media
                    .audio_streams
                    .iter()
                    .map(|s| {
                        let ts = spec.audio_tracks.iter().find(|t| t.index == s.index);
                        let carried = self.audio_defaults.iter().find(|d| d.index == s.index);
                        AudioRow {
                            index: s.index,
                            label: audio_stream_label(s),
                            detail: format!("{} · {}ch", s.codec, s.channels),
                            enabled: ts.map(|t| t.enabled).or(carried.map(|c| c.enabled)).unwrap_or(true),
                            volume: ts.map(|t| t.volume).or(carried.map(|c| c.volume)).unwrap_or(1.0),
                        }
                    })
                    .collect();
                ed.media = Some(media);
                ed.frame_dirty = true;
            }
        }
    }

    fn toggle_override(&mut self, on: bool) -> Task<Message> {
        let Some(id) = self.selected_id.clone() else { return Task::none() };
        let base = self.config.output.clone();
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.output_override = on.then(|| OutputOverride {
                quality_mode: Some(base.quality_mode),
                quality_preset: Some(base.quality_preset),
                crf: Some(base.crf),
                video_bitrate_kbps: Some(base.video_bitrate_kbps),
                ..Default::default()
            });
        }
        self.persist_edit(&id);
        Task::none()
    }

    fn patch_override(&mut self, patch: impl FnOnce(&mut OutputOverride)) -> Task<Message> {
        let Some(id) = self.selected_id.clone() else { return Task::none() };
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            let o = item.output_override.get_or_insert_with(OutputOverride::default);
            patch(o);
        }
        self.persist_edit(&id);
        Task::none()
    }

    fn start_export(&mut self, _force: bool) -> Task<Message> {
        let Some(id) = self.selected_id.clone() else { return Task::none() };
        let Some(item) = self.items.iter().find(|i| i.id == id) else { return Task::none() };
        let Some(ed) = &self.editor else { return Task::none() };
        let Some(media) = &ed.media else { return Task::none() };
        if qlipq_core::edit_spec::validate_edit_spec(&editor_spec(ed), media).is_some() || self.config.output_folder.is_empty() {
            return Task::none();
        }
        let output = self.effective_output(item);
        let (name, _) = rename::split_file_name(&item.file_name);
        let out_name = build_export_name(&self.config, item, &name, output.container.extension());
        let output_path = host::join_path(&self.config.output_folder, &out_name);

        let exists = host::file_exists(&output_path);
        if exists {
            if let Some(ed) = &mut self.editor {
                ed.overwrite_target = Some(output_path);
            }
            return Task::none();
        }
        self.run_export_to(output_path)
    }

    fn run_export_to(&mut self, output_path: String) -> Task<Message> {
        let Some(id) = self.selected_id.clone() else { return Task::none() };
        let Some(ed) = &self.editor else { return Task::none() };
        let Some(media) = ed.media.clone() else { return Task::none() };
        let Some(item) = self.items.iter().find(|i| i.id == id).cloned() else { return Task::none() };

        let output = self.effective_output(&item);
        let encode = output_settings_to_encode(&output, &media);
        let spec = editor_spec(ed);
        let total = qlipq_core::edit_spec::effective_duration(&spec, &media);
        let metadata = item.source.clone().map(|s| vec![("game".to_string(), s)]).unwrap_or_default();
        let args = build_export_args(&BuildExportOptions {
            input_path: item.path.clone(),
            output_path: output_path.clone(),
            spec,
            reencode: encode.reencode,
            progress: true,
            video: Some(encode.video),
            audio: Some(encode.audio),
            metadata,
        });

        self.export_target = Some(output_path);

        let progress = Arc::new(Mutex::new(0.0_f32));
        if let Some(ed) = &mut self.editor {
            ed.exporting = true;
            ed.progress_display = 0.0;
            ed.progress = Arc::clone(&progress);
        }
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = QueueStatus::Exporting;
            item.error = None;
        }

        let ffmpeg = self.config.ffmpeg_path.clone();
        let prog = Arc::clone(&progress);
        Task::perform(
            blocking(move || host::run_export(&ffmpeg, &args, total, prog)),
            move |result| Message::ExportFinished(id.clone(), result),
        )
    }

    fn on_export_finished(&mut self, id: String, result: Result<(), String>) -> Task<Message> {
        if let Some(ed) = &mut self.editor {
            if ed.item_id == id {
                ed.exporting = false;
                ed.progress_display = if result.is_ok() { 1.0 } else { ed.progress_display };
            }
        }
        let export_path = self.export_target.take();
        match result {
            Ok(()) => {
                if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
                    item.status = QueueStatus::Done;
                    item.export_path = export_path;
                }
                // After-export.
                match self.config.after_export.action {
                    AfterExportAction::Prompt => {
                        if let Some(ed) = &mut self.editor {
                            ed.after_prompt = true;
                        }
                        Task::none()
                    }
                    action => self.run_after_action(action),
                }
            }
            Err(e) => {
                if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
                    item.status = QueueStatus::Error;
                    item.error = Some(e);
                }
                Task::none()
            }
        }
    }

    fn run_after_action(&mut self, action: AfterExportAction) -> Task<Message> {
        let Some(id) = self.selected_id.clone() else { return Task::none() };
        let Some(item) = self.items.iter().find(|i| i.id == id).cloned() else { return Task::none() };
        match action {
            AfterExportAction::Delete => {
                let path = item.path.clone();
                Task::perform(blocking(move || { let _ = host::delete_file(&path); }), |_| Message::Ignore)
            }
            AfterExportAction::Move => {
                let folder = self.config.after_export.move_folder.clone();
                let path = item.path.clone();
                if folder.is_empty() {
                    Task::perform(
                        async move {
                            rfd::AsyncFileDialog::new().pick_folder().await.map(|h| host::to_posix(&h.path().to_string_lossy()))
                        },
                        move |opt| match opt {
                            Some(folder) => Message::FolderPicked(PickPurpose::MoveFolder, Some(format!("move::{path}::{folder}"))),
                            None => Message::Ignore,
                        },
                    )
                } else {
                    let dest = host::join_path(&folder, &host::base_name(&path));
                    Task::perform(blocking(move || { let _ = host::rename_file(&path, &dest); }), |_| Message::Ignore)
                }
            }
            AfterExportAction::Rename => {
                let (name, ext) = rename::split_file_name(&item.file_name);
                let renamed = format!(
                    "{}{}{}{}",
                    self.config.after_export.rename_prefix,
                    name,
                    self.config.after_export.rename_suffix,
                    if ext.is_empty() { String::new() } else { format!(".{ext}") }
                );
                let from = item.path.clone();
                let to = host::join_path(&host::dir_name(&item.path), &renamed);
                Task::perform(blocking(move || { let _ = host::rename_file(&from, &to); }), |_| Message::Ignore)
            }
            AfterExportAction::Nothing | AfterExportAction::Prompt => Task::none(),
        }
    }

    fn confirm_rename(&mut self) -> Task<Message> {
        let Some(r) = self.rename.take() else { return Task::none() };
        let Some(item) = self.items.iter().find(|i| i.id == r.id) else { return Task::none() };
        let (_, ext) = rename::split_file_name(&item.file_name);
        let trimmed = r.value.trim().to_string();
        if trimmed.is_empty() {
            return Task::none();
        }
        let new_name = if ext.is_empty() { trimmed } else { format!("{trimmed}.{ext}") };
        let new_path = host::join_path(&host::dir_name(&item.path), &new_name);
        let from = item.path.clone();
        let id = r.id.clone();
        Task::perform(blocking(move || host::rename_file(&from, &new_path)), move |res| {
            Message::FolderPicked(PickPurpose::WatchedFolder, res.ok().map(|p| format!("renamed::{id}::{p}")))
        })
    }

    fn on_folder_picked(&mut self, purpose: PickPurpose, path: String) -> Task<Message> {
        // Reuse FolderPicked for a few side-channel results encoded in the string.
        if let Some(rest) = path.strip_prefix("renamed::") {
            if let Some((id, new_path)) = rest.split_once("::") {
                self.apply_rename(id, new_path);
            }
            return Task::none();
        }
        if let Some(rest) = path.strip_prefix("move::") {
            let parts: Vec<&str> = rest.splitn(2, "::").collect();
            if parts.len() == 2 {
                let (from, folder) = (parts[0].to_string(), parts[1].to_string());
                let dest = host::join_path(&folder, &host::base_name(&from));
                return Task::perform(blocking(move || { let _ = host::rename_file(&from, &dest); }), |_| Message::Ignore);
            }
            return Task::none();
        }
        match purpose {
            PickPurpose::WatchedFolder => self.add_watched_folder(path),
            PickPurpose::OutputFolder => {
                self.config.output_folder = path;
                self.save_config_task()
            }
            PickPurpose::MoveFolder => {
                self.config.after_export.move_folder = path;
                self.save_config_task()
            }
        }
    }

    fn apply_rename(&mut self, id: &str, new_path: &str) {
        let new_name = host::base_name(new_path);
        let parsed = qlipq_core::obs::parse_obs_filename(&new_name);
        let old_path = self.items.iter().find(|i| i.id == id).map(|i| i.path.clone());
        if let Some(old) = &old_path {
            self.known_paths.remove(old);
            if let Some(stored) = self.edit_store.remove(old) {
                self.edit_store.insert(new_path.to_string(), stored);
            }
        }
        self.known_paths.insert(new_path.to_string());
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.path = new_path.to_string();
            item.file_name = new_name;
            if let Some(r) = parsed.recorded_at {
                item.recorded_at = Some(iso::from_local(r));
            }
            if parsed.source.is_some() {
                item.source = parsed.source;
            }
        }
        self.persist_edit(id);
    }

    fn add_watched_folder(&mut self, folder: String) -> Task<Message> {
        if self.config.watched_folders.contains(&folder) {
            return Task::none();
        }
        self.config.watched_folders.push(folder);
        Task::batch([self.save_config_task(), self.restart_watch_and_scan()])
    }

    fn remove_item(&mut self, id: &str) {
        if let Some(pos) = self.items.iter().position(|i| i.id == id) {
            let path = self.items[pos].path.clone();
            self.known_paths.remove(&path);
            self.items.remove(pos);
        }
        if self.selected_id.as_deref() == Some(id) {
            self.selected_id = None;
            self.editor = None;
        }
    }

    fn view(&self) -> Element<'_, Message> {
        if let Some(r) = &self.rename {
            return self.rename_modal(r);
        }
        if let Some(id) = &self.delete_confirm {
            return self.delete_modal(id);
        }
        if let Some(ed) = &self.editor {
            if let Some(target) = &ed.overwrite_target {
                return self.overwrite_modal(target);
            }
            if ed.after_prompt {
                return self.after_modal();
            }
        }

        let content: Element<Message> = match self.view {
            View::Settings => self.settings_view(),
            View::Queue => row![
                container(self.queue_sidebar()).width(Length::Fixed(360.0)),
                container(self.editor_view()).width(Length::Fill),
            ]
            .into(),
        };

        column![self.top_bar(), content].into()
    }

    // ---- view helpers are defined in the `views` impl block below ----
}

include!("views.rs");

fn build_item(path: &str, roots: &[String]) -> QueueItem {
    let file_name = host::base_name(path);
    let parsed = qlipq_core::obs::parse_obs_filename(&file_name);
    let game = roots.iter().find_map(|r| qlipq_core::obs::infer_game_from_path(r, path));
    QueueItem {
        id: qlipq_core::ids::create_id(),
        path: path.to_string(),
        file_name,
        added_at: iso::now(),
        status: QueueStatus::Pending,
        recorded_at: parsed.recorded_at.map(iso::from_local),
        source: parsed.source.or(game),
        media: None,
        file_size_bytes: None,
        file_modified_at: None,
        duration_sec: None,
        edit: None,
        output_override: None,
        tags: None,
        export_path: None,
        error: None,
    }
}

fn editor_audio_specs(ed: &Editor) -> Vec<AudioTrackSpec> {
    ed.audio.iter().map(|r| AudioTrackSpec { index: r.index, enabled: r.enabled, volume: r.volume }).collect()
}

fn editor_spec(ed: &Editor) -> EditSpec {
    EditSpec {
        trim: Some(TrimSpec { start_sec: ed.trim_start, end_sec: ed.trim_end }),
        crop: if ed.crop_enabled { Some(ed.crop.clone()) } else { None },
        audio_tracks: editor_audio_specs(ed),
    }
}

fn append_timestamp(path: &str) -> String {
    let (name, ext) = rename::split_file_name(&host::base_name(path));
    let now = chrono::Local::now().naive_local();
    let stamped = format!(
        "{}_{}{}",
        name,
        datetimes::format_datetime(&now),
        if ext.is_empty() { String::new() } else { format!(".{ext}") }
    );
    host::join_path(&host::dir_name(path), &stamped)
}

fn build_export_name(config: &AppConfig, item: &QueueItem, name: &str, ext: &str) -> String {
    let vars = rename::RenameVars {
        name: name.to_string(),
        ext: ext.to_string(),
        recorded_at: item.recorded_at.as_deref().and_then(iso::to_local),
        source: item.source.clone(),
        index: None,
    };
    rename::build_renamed_file_name(&config.naming_template, &vars)
}
