#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! qlipq — recording queue + libav clip editor desktop app.
//!
//! `qlipq-core` owns the domain model; `qlipq-ffmpeg` owns the pure encode/rate-control planning.
//! This binary is the host + UI, and it decodes, previews, probes, and exports **in process** via
//! libav (rsmpeg) — no external `ffmpeg`/`ffprobe` binary is spawned. Preview runs through libplacebo
//! (HDR→SDR tonemap) with synced cpal audio ([`libav`]); export decodes → edits → hardware-encodes →
//! muxes ([`export`]).

mod host;
mod iso;
mod theme;
mod video;
mod libav;
mod export;
mod log_ctx;

// The in-process libav preview player (libplacebo HDR tonemap + synced cpal audio), exposing
// `poll`/`dimensions`/`fps`/`position`/`try_seek` for the editor below.
use libav::{start_player, Player as PreviewPlayer, ScrubDecoder};

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::widget::{
    button, center, checkbox, column, container, mouse_area, opaque, pick_list, progress_bar,
    responsive, row, rule, scrollable, shader, slider, stack, text, text_input, tooltip, Space,
};
use iced::{Element, Font, Length, Size, Subscription, Task, Theme};

use qlipq_core::config::*;
use qlipq_core::edit_spec::{AudioTrackSpec, CropSpec, EditSpec, TrimSpec};
use qlipq_core::media::{audio_stream_label, format_bytes, MediaInfo};
use qlipq_core::{datetimes, queue::*, rename};
use qlipq_ffmpeg::args::output_settings_to_encode;
use qlipq_ffmpeg::estimate::estimate_export_size;

const DISMISSED_TAG: &str = "dismissed";
const TICK: Duration = Duration::from_millis(250);
const SIDEBAR_WIDTH: f32 = 360.0;

/// Caps concurrent background duration probes so the editor's on-demand probe (and the system)
/// are never starved by a folder full of recordings. Mirrors the web app's PROBE_CONCURRENCY=3.
static PROBE_SEM: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);

fn main() -> iced::Result {
    // Quiet libav's demuxer/filter chatter (e.g. the harmless "UDTA parsing failed retrying raw"
    // most recorders trigger, logged once per file open) while keeping real errors visible — and
    // prefix each surviving libav line with the file the thread is decoding, so a broken/truncated
    // recording's demuxer/decoder errors name the clip that caused them (see `log_ctx`).
    log_ctx::install();
    iced::application(App::new, App::update, App::view)
        .title("QlipQ")
        .subscription(App::subscription)
        .theme(App::theme)
        .font(include_bytes!("../assets/Inter-Variable.ttf").as_slice())
        .default_font(theme::FONT)
        .antialiasing(true)
        .window(iced::window::Settings {
            size: Size::new(1200.0, 800.0),
            min_size: Some(Size::new(960.0, 660.0)),
            ..Default::default()
        })
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
choice!(PreviewResChoice, i64, {
    Source => ("Source (sharpest)", 0),
    P1440 => ("1440p", 1440),
    P1080 => ("1080p", 1080),
    P720 => ("720p (fastest)", 720),
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

/// Which editor shortcut a Settings text field rebinds.
#[derive(Debug, Clone, Copy)]
enum KbField {
    PlayPause,
    SetIn,
    SetOut,
    FrameBack,
    FrameForward,
    JumpBack,
    JumpForward,
    GoToStart,
    GoToEnd,
    Export,
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
    /// Editable playhead timestamp text (kept in sync with `current_time` unless `editing_time`).
    time_input: String,
    /// True while the user is typing in the timestamp field, so live playback doesn't clobber it.
    editing_time: bool,
    /// Latest decoded preview frame, shared with the `video` shader widget (persistent GPU texture).
    shared_frame: video::SharedFrame,
    has_frame: bool,
    /// Source is HDR (PQ/HLG) — the preview must tonemap to SDR.
    is_hdr: bool,
    /// Decode path for this clip (GPU D3D11VA vs software), captured when the scrubber opens; shown
    /// in the debug panel even while paused. `None` until the clip is probed / if the decoder fails.
    decode_hw: Option<bool>,
    /// Preview output size (≤720 tall) the decoders render at, for the debug panel.
    preview_dims: Option<(u32, u32)>,
    frame_dirty: bool,
    extracting: bool,
    playing: bool,
    /// Warm streaming decoder, present only while playing (dropped → decoder stopped).
    player: Option<PreviewPlayer>,
    /// Warm single-frame decoder for scrubbing/paused preview, opened once per clip (a warm libav
    /// demuxer+decoder). Behind `Arc<Mutex>` so the blocking scrub task
    /// can drive it. `None` until the clip is probed, or if the decoder fails to open.
    scrubber: Option<Arc<Mutex<ScrubDecoder>>>,
    exporting: bool,
    progress: Arc<Mutex<f32>>,
    progress_display: f32,
    /// Set true to abort the in-process export (the worker polls it). Held so the Cancel button works.
    export_cancel: Option<Arc<AtomicBool>>,
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
    /// Persisted probe results (duration + parsed metadata) keyed by path, so the backlog isn't
    /// re-probed on every launch. Written back debounced via `media_cache_dirty` on the tick.
    media_cache: host::MediaCache,
    media_cache_dirty: bool,
    selected_id: Option<String>,
    view: View,
    tag_filter: Option<String>,
    presets: host::CapturePresets,
    watcher: Option<host::Watcher>,
    editor: Option<Editor>,
    audio_defaults: Vec<AudioTrackSpec>,
    rename: Option<RenameState>,
    delete_confirm: Option<String>,
    /// Set when a delete fails (e.g. the file is still open in another app) to show an error dialog.
    delete_error: Option<String>,
    new_tag: String,
    export_target: Option<String>,
    /// Queue card currently under the cursor (whole-card hover styling).
    hovered_card: Option<String>,
    /// The video preview is expanded to fill the window.
    fullscreen: bool,
    /// Multiplier on the preview pane height (zoom control).
    preview_scale: f32,
    theme: Theme,
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
    PlaybackTick,
    ShowQueue,
    ShowSettings,
    OpenRepo,
    RescanAll,
    Scanned(Vec<String>),
    /// Freshly scanned files classified against the media cache: stats for the list, plus any cached
    /// probe so unchanged files skip re-probing.
    MediaResolved(Vec<host::MediaResolution>),
    /// A background duration probe finished for a cache miss (`size`/`modified_ms` stamp the cache).
    MediaProbedBg {
        path: String,
        size: i64,
        modified_ms: i64,
        result: Result<(MediaInfo, bool), String>,
    },
    PresetsDetected(Option<String>, Option<String>),
    SelectItem(String),
    MediaProbed(String, Result<(MediaInfo, bool), String>),
    FrameExtracted(String, Option<(u32, u32, Vec<u8>, f64)>),
    Seek(f64),
    Skip(f64),
    ToggleFullscreen,
    PreviewZoom(f32),
    HoverCard(String),
    HoverLeave(String),
    RevealItem(String),
    TimestampEdited(String),
    TimestampSubmit,
    EditorKey(iced::keyboard::Key, iced::keyboard::Modifiers),
    SetKeybind(KbField, String),
    TogglePlay,
    SetIn,
    SetOut,
    ToggleCrop(bool),
    CropEdited(u8, String),
    AudioToggle(i64, bool),
    AudioVolume(i64, f64),
    /// Slider released — persist the volume once (the drag itself only updates live, no per-frame save).
    AudioVolumeCommit,
    ToggleOverride(bool),
    OverrideQm(QmChoice),
    OverrideQp(QpChoice),
    OverrideCrf(String),
    OverrideBitrate(String),
    NewTagChanged(String),
    AddTag,
    RemoveTag(String),
    Export,
    CancelExport,
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
    /// Escape / backdrop click — dismiss whichever modal is open.
    DismissModal,
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
    /// HDR preview brightness slider: dragging updates the value live; release re-applies it to the
    /// preview and persists.
    SetHdrPreviewGamma(f64),
    ApplyHdrPreviewGamma,
    SetPreviewRes(PreviewResChoice),
    ToggleAutoplay(bool),
    ToggleDebug(bool),
    /// Copy the given text (the debug panel's diagnostics) to the system clipboard.
    CopyText(String),
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
        let _ = host::write_config_schema();
        let config = host::load_config();
        let edit_store = host::load_edit_store();
        let media_cache = host::load_media_cache();
        let watcher = host::start_watch(&config.watched_folders, &config.video_extensions);

        let app = App {
            config,
            items: Vec::new(),
            known_paths: HashSet::new(),
            edit_store,
            media_cache,
            media_cache_dirty: false,
            selected_id: None,
            view: View::Queue,
            tag_filter: None,
            presets: host::CapturePresets::default(),
            watcher,
            editor: None,
            audio_defaults: Vec::new(),
            rename: None,
            delete_confirm: None,
            delete_error: None,
            new_tag: String::new(),
            export_target: None,
            hovered_card: None,
            fullscreen: false,
            preview_scale: 1.0,
            theme: theme::dark(),
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
        let mut subs = vec![iced::time::every(TICK).map(|_| Message::Tick)];
        // Editor keyboard shortcuts — active only with a clip open and no modal in front. A focused
        // text field captures the key (Status::Captured), so `editor_key_event` ignores it; here we
        // just avoid binding over the queue/settings or a dialog.
        let modal = self.rename.is_some()
            || self.delete_confirm.is_some()
            || self.delete_error.is_some()
            || self.editor.as_ref().map(|e| e.overwrite_target.is_some() || e.after_prompt).unwrap_or(false);
        if self.editor.is_some() && !modal {
            subs.push(iced::event::listen_with(editor_key_event));
        }
        // Escape dismisses a modal; otherwise it exits fullscreen.
        if modal {
            subs.push(iced::event::listen_with(modal_escape_event));
        } else if self.fullscreen {
            subs.push(iced::event::listen_with(fullscreen_escape_event));
        }
        // While playing, add a fast tick at the preview frame rate to pull streamed frames.
        if let Some(player) = self.editor.as_ref().filter(|e| e.playing).and_then(|e| e.player.as_ref()) {
            let dt = Duration::from_secs_f64(1.0 / player.fps().clamp(1.0, 60.0));
            subs.push(iced::time::every(dt).map(|_| Message::PlaybackTick));
        }
        Subscription::batch(subs)
    }

    fn theme(&self) -> Theme {
        self.theme.clone()
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
        // Stat the fresh files and pair each with its cached probe when unchanged; only misses are
        // probed (see `Message::MediaResolved`), so a full backlog isn't re-probed on every launch.
        let cache = self.media_cache.clone();
        Task::perform(blocking(move || host::resolve_media(&fresh, &cache)), Message::MediaResolved)
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
            Message::PlaybackTick => self.on_playback_tick(),
            Message::ShowQueue => self.view = View::Queue,
            Message::ShowSettings => self.view = View::Settings,
            Message::ToggleFullscreen => self.fullscreen = !self.fullscreen,
            Message::PreviewZoom(delta) => self.preview_scale = (self.preview_scale + delta).clamp(0.5, 2.5),
            Message::OpenRepo => host::open_external("https://github.com/qcksys/qlipq"),
            Message::RescanAll => {
                let folders = self.config.watched_folders.clone();
                let exts = self.config.video_extensions.clone();
                return Task::perform(blocking(move || host::scan_folders(&folders, &exts)), Message::Scanned);
            }
            Message::Scanned(paths) => return self.add_paths(paths),
            Message::MediaResolved(list) => {
                let mut to_probe: Vec<(String, i64, i64)> = Vec::new();
                for host::MediaResolution { path, size, modified_ms, cached } in list {
                    if let Some(item) = self.items.iter_mut().find(|i| i.path == path) {
                        item.file_size_bytes = Some(size);
                        item.file_modified_at = Some(iso::from_unix_ms(modified_ms));
                        if let Some(c) = cached {
                            item.duration_sec = Some(c.media.duration_sec);
                            continue; // cache hit — no re-probe
                        }
                    } else if cached.is_some() {
                        continue; // item already gone; nothing to probe
                    }
                    to_probe.push((path, size, modified_ms));
                }
                if to_probe.is_empty() {
                    return Task::none();
                }
                // Probe only the misses, capped at PROBE_SEM permits so a large folder doesn't
                // saturate the blocking pool and starve the editor's on-demand probe.
                return Task::batch(to_probe.into_iter().map(|(path, size, modified_ms)| {
                    let id_path = path.clone();
                    Task::perform(
                        async move {
                            let _permit = PROBE_SEM.acquire().await;
                            blocking(move || libav::probe(&path)).await
                        },
                        move |result| Message::MediaProbedBg {
                            path: id_path.clone(),
                            size,
                            modified_ms,
                            result,
                        },
                    )
                }));
            }
            Message::MediaProbedBg { path, size, modified_ms, result } => {
                // Ignore a probe whose recording was deleted while it was in flight (probes queue
                // behind PROBE_SEM) — caching it would re-persist app data for a file that's gone.
                if let Ok((media, is_hdr)) = result {
                    if self.known_paths.contains(&path) {
                        if let Some(item) = self.items.iter_mut().find(|i| i.path == path) {
                            item.duration_sec = Some(media.duration_sec);
                        }
                        self.media_cache.insert(
                            path,
                            host::CachedMedia { size_bytes: size, modified_ms, media, is_hdr },
                        );
                        // Debounced write on the tick — a first-run backlog probes many files at once.
                        self.media_cache_dirty = true;
                    }
                }
            }
            Message::PresetsDetected(obs, nvidia) => {
                self.presets = host::CapturePresets { obs, nvidia_share: nvidia };
            }
            Message::SelectItem(id) => return self.select_item(id),
            Message::MediaProbed(id, result) => {
                self.on_media_probed(id.clone(), result);
                // Only react to a probe that still matches the open editor. Probes run on the
                // multi-threaded blocking pool and can finish out of order, so a stale probe from a
                // superseded selection must not restart (or re-extract) whatever clip is open now —
                // that would tear down and rebuild a clip that is already playing.
                if self.editor.as_ref().map_or(true, |e| e.item_id != id) {
                    return Task::none();
                }
                // Autoplay the freshly opened clip when enabled; otherwise show a paused first frame.
                let ready = self.editor.as_ref().map_or(false, |e| e.load_error.is_none() && e.media.is_some());
                if self.config.autoplay && ready {
                    return self.play_from_current();
                }
                return self.request_frame();
            }
            Message::FrameExtracted(id, frame) => {
                let mut redo = false;
                if let Some(ed) = &mut self.editor {
                    if ed.item_id == id {
                        ed.extracting = false;
                        redo = ed.frame_dirty; // position moved again while extracting
                        if let Some((w, h, rgba, realized)) = frame {
                            video::push_frame(&ed.shared_frame, w, h, rgba);
                            ed.has_frame = true;
                            // Snap the playhead to the frame we actually decoded (frame-accurate
                            // scrubber) — but not if a newer scrub is pending, where `current_time`
                            // already holds the new target.
                            if !redo {
                                ed.current_time = realized;
                                sync_time_input(ed);
                            }
                        }
                    }
                }
                if redo {
                    return self.request_frame();
                }
            }
            Message::Seek(sec) => {
                // Scrubbing keeps the current transport state: if it was playing, keep playing from the
                // new position (warm-seek the decoder, or restart it); if paused, just show the frame.
                let playing = self.editor.as_ref().map(|e| e.playing).unwrap_or(false);
                let mut seeked = false;
                if let Some(ed) = &mut self.editor {
                    let max = ed.media.as_ref().map(|m| m.duration_sec).unwrap_or(sec);
                    ed.current_time = sec.clamp(0.0, max);
                    ed.editing_time = false;
                    sync_time_input(ed);
                    if playing {
                        seeked = ed.player.as_ref().map(|p| p.try_seek(ed.current_time)).unwrap_or(false);
                    }
                }
                if playing && !seeked {
                    return self.play_from_current();
                }
                if !playing {
                    return self.request_frame();
                }
            }
            Message::TimestampEdited(s) => {
                if let Some(ed) = &mut self.editor {
                    ed.editing_time = true;
                    ed.time_input = s;
                }
            }
            Message::TimestampSubmit => {
                let parsed = self.editor.as_ref().and_then(|ed| parse_timestamp(&ed.time_input));
                if let Some(ed) = &mut self.editor {
                    ed.editing_time = false;
                }
                match parsed {
                    Some(sec) => return self.update(Message::Seek(sec)),
                    None => {
                        if let Some(ed) = &mut self.editor {
                            sync_time_input(ed); // invalid input → snap the field back to the playhead
                        }
                    }
                }
            }
            Message::EditorKey(key, mods) => {
                let snap = self.editor.as_ref().and_then(|e| e.media.as_ref()).map(|m| (m.fps.max(1.0), m.duration_sec));
                let Some((fps, dur)) = snap else { return Task::none() };
                let kb = &self.config.keybinds;
                let action = if binding_matches(&kb.play_pause, &key, mods) {
                    Some(Message::TogglePlay)
                } else if binding_matches(&kb.set_in, &key, mods) {
                    Some(Message::SetIn)
                } else if binding_matches(&kb.set_out, &key, mods) {
                    Some(Message::SetOut)
                } else if binding_matches(&kb.frame_back, &key, mods) {
                    Some(Message::Skip(-1.0 / fps))
                } else if binding_matches(&kb.frame_forward, &key, mods) {
                    Some(Message::Skip(1.0 / fps))
                } else if binding_matches(&kb.jump_back, &key, mods) {
                    Some(Message::Skip(-5.0))
                } else if binding_matches(&kb.jump_forward, &key, mods) {
                    Some(Message::Skip(5.0))
                } else if binding_matches(&kb.go_to_start, &key, mods) {
                    Some(Message::Seek(0.0))
                } else if binding_matches(&kb.go_to_end, &key, mods) {
                    Some(Message::Seek(dur))
                } else if binding_matches(&kb.export, &key, mods) {
                    Some(Message::Export)
                } else {
                    None
                };
                if let Some(msg) = action {
                    return self.update(msg);
                }
            }
            Message::SetKeybind(field, value) => {
                let kb = &mut self.config.keybinds;
                match field {
                    KbField::PlayPause => kb.play_pause = value,
                    KbField::SetIn => kb.set_in = value,
                    KbField::SetOut => kb.set_out = value,
                    KbField::FrameBack => kb.frame_back = value,
                    KbField::FrameForward => kb.frame_forward = value,
                    KbField::JumpBack => kb.jump_back = value,
                    KbField::JumpForward => kb.jump_forward = value,
                    KbField::GoToStart => kb.go_to_start = value,
                    KbField::GoToEnd => kb.go_to_end = value,
                    KbField::Export => kb.export = value,
                }
                return self.save_config_task();
            }
            Message::Skip(delta) => {
                let playing = self.editor.as_ref().map(|e| e.playing).unwrap_or(false);
                let mut seeked = false;
                if let Some(ed) = &mut self.editor {
                    let max = ed.media.as_ref().map(|m| m.duration_sec).unwrap_or(0.0);
                    ed.current_time = (ed.current_time + delta).clamp(0.0, max);
                    ed.editing_time = false;
                    sync_time_input(ed);
                    if playing {
                        // Seek the warm decoder in-process; if it can't (no live decode thread) it
                        // reports false and we restart below instead.
                        seeked = ed.player.as_ref().map(|p| p.try_seek(ed.current_time)).unwrap_or(false);
                    }
                }
                if playing && !seeked {
                    return self.play_from_current(); // re-seek by restarting the warm decoder
                }
                if !playing {
                    return self.request_frame();
                }
            }
            Message::TogglePlay => {
                let playing = self.editor.as_ref().map(|e| e.playing).unwrap_or(false);
                if playing {
                    if let Some(ed) = &mut self.editor {
                        ed.playing = false;
                        ed.player = None;
                    }
                    return self.request_frame(); // crisp, exact frame at the pause point
                } else {
                    return self.play_from_current();
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
                let playing = self.editor.as_ref().map(|e| e.playing).unwrap_or(false);
                if let Some(ed) = &mut self.editor {
                    if let Some(r) = ed.audio.iter_mut().find(|r| r.index == index) {
                        r.enabled = on;
                    }
                    self.audio_defaults = editor_audio_specs(ed);
                }
                self.commit_spec();
                // The running monitor mix was started with a fixed track set, so a live volume change
                // (set_gain) can't add/remove a track. Restart from the current position so the toggle
                // is heard immediately instead of only after the next play/seek.
                if playing {
                    return self.play_from_current();
                }
            }
            Message::AudioVolume(index, vol) => {
                // Live update only — a drag fires this per frame, so persisting here would spawn a
                // save per frame. The slider's on_release (AudioVolumeCommit) persists once at the end.
                if let Some(ed) = &mut self.editor {
                    if let Some(r) = ed.audio.iter_mut().find(|r| r.index == index) {
                        r.volume = vol;
                    }
                    // Apply to the running monitor mix so the change is audible immediately.
                    if let Some(p) = &ed.player {
                        p.set_gain(index, vol);
                    }
                    self.audio_defaults = editor_audio_specs(ed);
                }
            }
            Message::AudioVolumeCommit => self.commit_spec(),
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
            Message::CancelExport => {
                if let Some(ed) = &self.editor {
                    if let Some(c) = &ed.export_cancel {
                        c.store(true, Ordering::Relaxed);
                    }
                }
            }
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
            Message::HoverCard(id) => self.hovered_card = Some(id),
            Message::HoverLeave(id) => {
                if self.hovered_card.as_deref() == Some(id.as_str()) {
                    self.hovered_card = None;
                }
            }
            Message::RevealItem(path) => host::reveal(&path),
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
                        // The preview player + scrub decoder keep the file open, and Windows refuses
                        // to delete an open file. Tear the editor down first so both are dropped —
                        // Player::drop joins its decode threads synchronously, closing the handle —
                        // before we attempt the delete below.
                        if self.editor.as_ref().is_some_and(|e| e.item_id == id) {
                            self.selected_id = None;
                            self.editor = None;
                        }
                        return Task::perform(blocking(move || host::delete_file(&path)), move |r| Message::Deleted(id.clone(), r));
                    }
                }
            }
            Message::DeleteCancel => self.delete_confirm = None,
            Message::DismissModal => {
                if self.rename.is_some() {
                    self.rename = None;
                } else if self.delete_confirm.is_some() {
                    self.delete_confirm = None;
                } else if self.delete_error.is_some() {
                    self.delete_error = None;
                } else if let Some(ed) = &mut self.editor {
                    if ed.overwrite_target.is_some() {
                        ed.overwrite_target = None;
                    } else {
                        ed.after_prompt = false;
                    }
                }
            }
            Message::Deleted(id, result) => match result {
                Ok(()) => self.remove_item(&id),
                Err(e) => {
                    // Deletion failed even after we released our handles — usually another app (OBS
                    // still recording, a media player) holds the file. Tell the user instead of
                    // silently leaving the clip in the queue.
                    let name = self.items.iter().find(|i| i.id == id).map(|i| i.file_name.clone()).unwrap_or_default();
                    self.delete_error = Some(format!("Couldn't delete {name}.\n\n{e}"));
                }
            },
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
            Message::SetHdrPreviewGamma(v) => self.config.hdr_preview_gamma = v.clamp(1.0, 3.0),
            Message::ApplyHdrPreviewGamma => {
                // Rebuild the scrub graph with the new gamma, persist, and refresh what's on screen
                // (restart playback if playing, else re-extract the current frame).
                self.reopen_scrubber();
                let save = self.save_config_task();
                let refresh = if self.editor.as_ref().map(|e| e.playing).unwrap_or(false) {
                    self.play_from_current()
                } else {
                    self.request_frame()
                };
                return Task::batch([save, refresh]);
            }
            Message::SetPreviewRes(c) => {
                // Rebuild the decoders at the new preview size, persist, and refresh what's on screen
                // (restart playback if playing, else re-extract the current frame).
                self.config.preview_max_height = c.to_core();
                self.reopen_scrubber();
                let save = self.save_config_task();
                let refresh = if self.editor.as_ref().map(|e| e.playing).unwrap_or(false) {
                    self.play_from_current()
                } else {
                    self.request_frame()
                };
                return Task::batch([save, refresh]);
            }
            Message::ToggleAutoplay(on) => { self.config.autoplay = on; return self.save_config_task(); }
            Message::ToggleDebug(on) => { self.config.debug = on; return self.save_config_task(); }
            Message::CopyText(s) => return iced::clipboard::write(s),
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
        // Mirror export progress for the bar.
        if let Some(ed) = &mut self.editor {
            if ed.exporting {
                if let Ok(p) = ed.progress.lock() {
                    ed.progress_display = *p;
                }
            }
        }
        // Flush new probe results to the media cache, coalesced so a backlog's probe storm is a
        // handful of writes rather than one per file.
        if self.media_cache_dirty {
            self.media_cache_dirty = false;
            let cache = self.media_cache.clone();
            std::thread::spawn(move || host::save_media_cache(&cache));
        }
        Task::batch(tasks)
    }

    /// Pull one streamed frame from the warm decoder per playback tick (real-time pacing comes
    /// from the channel backpressure: the decoder produces at roughly the rate we consume).
    fn on_playback_tick(&mut self) {
        let Some(ed) = &mut self.editor else { return };
        let polled = ed.player.as_ref().map(|p| (p.poll(), p.dimensions(), p.fps(), p.position()));
        let Some((frame, (w, h), fps, position)) = polled else { return };
        let dur = ed.media.as_ref().map(|m| m.duration_sec).filter(|d| *d > 0.0);

        let advance = match frame {
            host::FramePoll::Frame(bytes) => {
                video::push_frame(&ed.shared_frame, w, h, bytes);
                ed.has_frame = true;
                true
            }
            // Keep the playhead tracking the master clock between video frames (smooth scrubber with
            // synced audio).
            host::FramePoll::Empty => position.is_some(),
            host::FramePoll::Ended => {
                if let Some(dur) = dur {
                    ed.current_time = dur;
                }
                ed.playing = false;
                ed.player = None;
                sync_time_input(ed);
                return;
            }
        };
        if !advance {
            return;
        }
        // Advance the playhead from the master clock (audio-synced, or a wall clock without audio);
        // fall back to one frame interval only if the clock reports no position. Then hard-stop at the
        // known end so playback halts cleanly and never overruns or loops — `Ended` (decoder EOF) is
        // the fallback when dur is 0.
        match position {
            Some(p) => ed.current_time = p,
            None => ed.current_time += 1.0 / fps,
        }
        if let Some(dur) = dur {
            if ed.current_time >= dur {
                ed.current_time = dur;
                ed.playing = false;
                ed.player = None;
            }
        }
        sync_time_input(ed);
    }

    /// Rebuild the warm scrub decoder for the open clip, picking up the current preview settings
    /// (`hdr_preview_gamma`, `preview_max_height`). Used when a preview-affecting setting changes so
    /// the next extracted frame reflects it.
    fn reopen_scrubber(&mut self) {
        let Some(ed) = self.editor.as_ref() else { return };
        let Some(media) = ed.media.as_ref() else { return };
        let (mw, mh, is_hdr, id) = (media.width, media.height, ed.is_hdr, ed.item_id.clone());
        let path = self.items.iter().find(|i| i.id == id).map(|i| i.path.clone());
        let gamma = self.config.hdr_preview_gamma;
        let max_h = self.config.preview_max_height;
        let scrubber = path
            .and_then(|p| ScrubDecoder::open(&p, mw, mh, is_hdr, gamma, max_h))
            .map(|s| Arc::new(Mutex::new(s)));
        if let Some(ed) = self.editor.as_mut() {
            // Preview size may have changed, so refresh the debug-panel capture too.
            if let Some(s) = scrubber.as_ref().and_then(|s| s.lock().ok()) {
                ed.preview_dims = Some(s.preview_dims());
            }
            ed.scrubber = scrubber;
        }
    }

    /// Extract a single preview frame at the playhead (scrubbing / paused). Coalesces: if an
    /// extraction is already in flight, just mark the frame dirty and re-request on completion.
    fn request_frame(&mut self) -> Task<Message> {
        let snap = match self.editor.as_ref() {
            Some(ed) if !ed.playing && ed.media.is_some() => {
                Some((ed.extracting, ed.current_time, ed.item_id.clone(), ed.scrubber.clone()))
            }
            _ => None,
        };
        let Some((extracting, sec, id, scrubber)) = snap else { return Task::none() };
        if extracting {
            if let Some(ed) = &mut self.editor {
                ed.frame_dirty = true;
            }
            return Task::none();
        }
        let Some(scrubber) = scrubber else { return Task::none() };
        if let Some(ed) = &mut self.editor {
            ed.extracting = true;
            ed.frame_dirty = false;
        }
        // Drive the warm scrub decoder on the blocking pool; coalescing (extracting/frame_dirty)
        // keeps at most one in flight, so the Mutex is never contended.
        Task::perform(
            blocking(move || scrubber.lock().ok().and_then(|mut s| s.frame_at(sec))),
            move |frame| Message::FrameExtracted(id.clone(), frame),
        )
    }

    /// (Re)start the warm streaming decoder from the current playhead and enter the playing state.
    /// Returns a fallback single-frame task if the decoder can't be started (e.g. an unreadable file).
    fn play_from_current(&mut self) -> Task<Message> {
        let snap = match self.editor.as_ref() {
            Some(ed) if ed.media.is_some() => {
                let m = ed.media.as_ref().unwrap();
                // Enabled tracks (audio-relative index + gain) drive the preview's monitor mixdown.
                let audio_tracks: Vec<(i64, f64)> =
                    ed.audio.iter().filter(|r| r.enabled).map(|r| (r.index, r.volume)).collect();
                Some((ed.item_id.clone(), ed.current_time, m.width, m.height, m.fps, m.duration_sec, ed.is_hdr, audio_tracks))
            }
            _ => None,
        };
        let Some((id, cur, mw, mh, mfps, dur, is_hdr, audio_tracks)) = snap else { return Task::none() };
        // Restart from the top only when we know we're at the real end (avoid rewinding on a 0/unknown duration).
        let start = if dur > 0.0 && cur >= dur { 0.0 } else { cur };
        let Some(path) = self.items.iter().find(|i| i.id == id).map(|i| i.path.clone()) else {
            return Task::none();
        };
        let gamma = self.config.hdr_preview_gamma;
        let max_h = self.config.preview_max_height;
        let player = start_player(&path, start, mw, mh, mfps, is_hdr, audio_tracks, gamma, max_h);
        let started = player.is_some();
        if let Some(ed) = &mut self.editor {
            ed.current_time = start;
            ed.playing = started;
            ed.player = player;
            ed.frame_dirty = false;
        }
        if started { Task::none() } else { self.request_frame() }
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
            time_input: format_timestamp(0.0),
            editing_time: false,
            shared_frame: video::new_shared_frame(),
            has_frame: false,
            is_hdr: false,
            decode_hw: None,
            preview_dims: None,
            frame_dirty: false,
            extracting: false,
            playing: false,
            player: None,
            scrubber: None,
            exporting: false,
            progress: Arc::new(Mutex::new(0.0)),
            progress_display: 0.0,
            export_cancel: None,
            overwrite_target: None,
            after_prompt: false,
        });
        let path = item.path.clone();
        Task::perform(blocking(move || libav::probe(&path)), move |r| Message::MediaProbed(id.clone(), r))
    }

    fn on_media_probed(&mut self, id: String, result: Result<(MediaInfo, bool), String>) {
        let Some(ed) = &mut self.editor else { return };
        if ed.item_id != id {
            return;
        }
        match result {
            Err(e) => ed.load_error = Some(e),
            Ok((media, is_hdr)) => {
                ed.is_hdr = is_hdr;
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
                let (mw, mh) = (media.width, media.height);
                ed.media = Some(media);
                ed.frame_dirty = true;
                // Open the warm scrub decoder for this clip (synchronous, like `start_player`); it
                // lives until the next selection drops this Editor.
                let path = self.items.iter().find(|i| i.id == id).map(|i| i.path.clone());
                let gamma = self.config.hdr_preview_gamma;
                let max_h = self.config.preview_max_height;
                ed.scrubber = path
                    .and_then(|p| ScrubDecoder::open(&p, mw, mh, is_hdr, gamma, max_h))
                    .map(|s| Arc::new(Mutex::new(s)));
                // Capture the decode path + preview size once (the scrubber isn't driven yet, so the
                // lock is free) for the debug panel — representative of the streaming player too.
                if let Some(s) = ed.scrubber.as_ref().and_then(|s| s.lock().ok()) {
                    ed.decode_hw = Some(s.hw_decode());
                    ed.preview_dims = Some(s.preview_dims());
                }
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
        let is_hdr = ed.is_hdr;
        let Some(item) = self.items.iter().find(|i| i.id == id).cloned() else { return Task::none() };

        let output = self.effective_output(&item);
        let spec = editor_spec(ed);
        let total = qlipq_core::edit_spec::effective_duration(&spec, &media);
        let metadata = item.source.clone().map(|s| vec![("game".to_string(), s)]).unwrap_or_default();

        self.export_target = Some(output_path.clone());

        let progress = Arc::new(Mutex::new(0.0_f32));
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(ed) = &mut self.editor {
            ed.exporting = true;
            ed.progress_display = 0.0;
            ed.progress = Arc::clone(&progress);
            ed.export_cancel = Some(Arc::clone(&cancel));
            // Stop the preview decoder so it doesn't contend with the export for CPU.
            ed.playing = false;
            ed.player = None;
        }
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = QueueStatus::Exporting;
            item.error = None;
        }

        self.spawn_export(id, item.path.clone(), output_path, spec, output, media, total, metadata, is_hdr, progress, cancel)
    }

    /// In-process export: decode → edits → hardware encode → mux ([`export::run_export`]). No CLI.
    #[allow(clippy::too_many_arguments)]
    fn spawn_export(
        &self,
        id: String,
        input: String,
        output_path: String,
        spec: EditSpec,
        output: OutputSettings,
        media: MediaInfo,
        _total: f64,
        metadata: Vec<(String, String)>,
        is_hdr: bool,
        progress: Arc<Mutex<f32>>,
        cancel: Arc<AtomicBool>,
    ) -> Task<Message> {
        Task::perform(
            blocking(move || {
                export::run_export(&input, &output_path, &spec, &output, &media, is_hdr, &metadata, progress, cancel)
            }),
            move |result| Message::ExportFinished(id.clone(), result),
        )
    }

    fn on_export_finished(&mut self, id: String, result: Result<(), String>) -> Task<Message> {
        if let Some(ed) = &mut self.editor {
            if ed.item_id == id {
                ed.exporting = false;
                ed.export_cancel = None;
                ed.progress_display = if result.is_ok() { 1.0 } else { ed.progress_display };
            }
        }
        let export_path = self.export_target.take();
        // A user-cancelled export isn't an error: reset the item, don't show an error banner.
        if matches!(&result, Err(e) if e == "cancelled") {
            if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
                item.status = QueueStatus::Pending;
                item.error = None;
            }
            return Task::none();
        }
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
            // The file's bytes are unchanged, so its cached probe follows the new path.
            if let Some(cached) = self.media_cache.remove(old) {
                self.media_cache.insert(new_path.to_string(), cached);
                self.media_cache_dirty = true;
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
            // A deleted recording keeps no persisted app data: drop its edit + cached probe.
            if self.edit_store.remove(&path).is_some() {
                let store = self.edit_store.clone();
                std::thread::spawn(move || host::save_edit_store(&store));
            }
            if self.media_cache.remove(&path).is_some() {
                let cache = self.media_cache.clone();
                std::thread::spawn(move || host::save_media_cache(&cache));
            }
        }
        if self.selected_id.as_deref() == Some(id) {
            self.selected_id = None;
            self.editor = None;
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let fullscreen = self.fullscreen
            && matches!(self.view, View::Queue)
            && self.editor.as_ref().map_or(false, |e| e.media.is_some());

        let base: Element<Message> = if fullscreen {
            self.fullscreen_view()
        } else {
            let content: Element<Message> = match self.view {
                View::Settings => self.settings_view(),
                View::Queue => row![
                    container(self.queue_sidebar())
                        .width(Length::Fixed(SIDEBAR_WIDTH))
                        .height(Length::Fill)
                        .style(theme::sidebar),
                    rule::vertical(1),
                    container(self.editor_view()).width(Length::Fill).height(Length::Fill),
                ]
                .into(),
            };
            container(column![self.top_bar(), content])
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::canvas)
                .into()
        };

        // A modal layers over the dimmed app rather than replacing it.
        let overlay: Option<Element<Message>> = if let Some(r) = &self.rename {
            Some(self.rename_modal(r))
        } else if let Some(id) = &self.delete_confirm {
            Some(self.delete_modal(id))
        } else if let Some(msg) = &self.delete_error {
            Some(self.delete_error_modal(msg))
        } else if let Some(ed) = &self.editor {
            if let Some(target) = &ed.overwrite_target {
                Some(self.overwrite_modal(target))
            } else if ed.after_prompt {
                Some(self.after_modal())
            } else {
                None
            }
        } else {
            None
        };

        // Always root the tree in a `stack`, even with no modal, so the root widget's type never
        // changes. iced rebuilds a widget's whole state subtree when the root tag flips (container ↔
        // stack), which would reset the queue scrollable to the top when a modal opens — e.g. the
        // delete-confirm dialog. Keeping `base` as stack child 0 preserves its scroll offset across
        // modal open/close and the item removal that follows a delete.
        let mut layers = stack![base];
        if let Some(m) = overlay {
            layers = layers.push(m);
        }
        layers.into()
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

/// Format seconds as `M:SS.mmm` (or `H:MM:SS.mmm` past an hour) for the editable playhead field.
fn format_timestamp(secs: f64) -> String {
    let total_ms = (secs.max(0.0) * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let s = (total_ms / 1000) % 60;
    let m = (total_ms / 60_000) % 60;
    let h = total_ms / 3_600_000;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{ms:03}")
    } else {
        format!("{m}:{s:02}.{ms:03}")
    }
}

/// Parse `S`, `M:SS`, `M:SS.mmm`, or `H:MM:SS(.mmm)` into seconds. `None` if a component isn't a number.
fn parse_timestamp(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut total = 0.0;
    for part in text.split(':') {
        let v: f64 = part.trim().parse().ok()?;
        if v < 0.0 {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

/// Refresh the timestamp field from the playhead, unless the user is mid-edit.
fn sync_time_input(ed: &mut Editor) {
    if !ed.editing_time {
        ed.time_input = format_timestamp(ed.current_time);
    }
}

/// Raw key event → [`Message::EditorKey`], but only when no widget captured it (a focused text field
/// reports `Status::Captured`, so its keystrokes are left alone). Must be a plain `fn` —
/// `iced::event::listen_with` takes a function pointer, not a closure.
fn editor_key_event(event: iced::Event, status: iced::event::Status, _id: iced::window::Id) -> Option<Message> {
    if status != iced::event::Status::Ignored {
        return None;
    }
    if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) = event {
        Some(Message::EditorKey(key, modifiers))
    } else {
        None
    }
}

/// Escape key (from anywhere) → dismiss the open modal. A plain `fn` for `listen_with`.
fn modal_escape_event(event: iced::Event, _status: iced::event::Status, _id: iced::window::Id) -> Option<Message> {
    use iced::keyboard::{key::Named, Event::KeyPressed, Key};
    if let iced::Event::Keyboard(KeyPressed { key: Key::Named(Named::Escape), .. }) = event {
        Some(Message::DismissModal)
    } else {
        None
    }
}

/// Escape key → exit fullscreen preview. A plain `fn` for `listen_with`.
fn fullscreen_escape_event(event: iced::Event, _status: iced::event::Status, _id: iced::window::Id) -> Option<Message> {
    use iced::keyboard::{key::Named, Event::KeyPressed, Key};
    if let iced::Event::Keyboard(KeyPressed { key: Key::Named(Named::Escape), .. }) = event {
        Some(Message::ToggleFullscreen)
    } else {
        None
    }
}

/// True if `binding` (e.g. `"Shift+Left"`, `"Ctrl+M"`, `"I"`) matches the pressed key + modifiers.
fn binding_matches(binding: &str, key: &iced::keyboard::Key, mods: iced::keyboard::Modifiers) -> bool {
    let binding = binding.trim();
    if binding.is_empty() {
        return false;
    }
    let parts: Vec<&str> = binding.split('+').map(|p| p.trim()).collect();
    let Some((token, mod_parts)) = parts.split_last() else {
        return false;
    };
    let (mut need_ctrl, mut need_shift, mut need_alt, mut need_logo) = (false, false, false, false);
    for m in mod_parts {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => need_ctrl = true,
            "shift" => need_shift = true,
            "alt" | "option" => need_alt = true,
            "cmd" | "command" | "super" | "win" | "logo" => need_logo = true,
            _ => return false,
        }
    }
    if mods.control() != need_ctrl || mods.shift() != need_shift || mods.alt() != need_alt || mods.logo() != need_logo {
        return false;
    }
    key_token_matches(token, key)
}

fn key_token_matches(token: &str, key: &iced::keyboard::Key) -> bool {
    use iced::keyboard::key::Named;
    use iced::keyboard::Key;
    match key {
        Key::Character(c) => token.eq_ignore_ascii_case(c.as_str()),
        Key::Named(named) => {
            let name = match named {
                Named::Space => "Space",
                Named::ArrowLeft => "Left",
                Named::ArrowRight => "Right",
                Named::ArrowUp => "Up",
                Named::ArrowDown => "Down",
                Named::Home => "Home",
                Named::End => "End",
                Named::Enter => "Enter",
                Named::Escape => "Escape",
                _ => return false,
            };
            token.eq_ignore_ascii_case(name)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{key::Named, Key, Modifiers};

    #[test]
    fn timestamp_round_trip() {
        assert_eq!(format_timestamp(65.5), "1:05.500");
        assert_eq!(format_timestamp(3661.0), "1:01:01.000");
        assert_eq!(parse_timestamp("90"), Some(90.0));
        assert_eq!(parse_timestamp("1:05.5"), Some(65.5));
        assert_eq!(parse_timestamp("1:01:01"), Some(3661.0));
        assert_eq!(parse_timestamp("  2:00 "), Some(120.0));
        assert_eq!(parse_timestamp("nope"), None);
        assert_eq!(parse_timestamp(""), None);
    }

    #[test]
    fn keybind_matching() {
        let none = Modifiers::empty();
        // Premiere defaults dispatch to the right key/modifier combos.
        assert!(binding_matches("Space", &Key::Named(Named::Space), none));
        assert!(binding_matches("i", &Key::Character("i".into()), none));
        assert!(binding_matches("I", &Key::Character("i".into()), none)); // case-insensitive
        assert!(!binding_matches("I", &Key::Character("i".into()), Modifiers::SHIFT)); // bare I, not Shift+I
        assert!(binding_matches("Shift+Left", &Key::Named(Named::ArrowLeft), Modifiers::SHIFT));
        assert!(!binding_matches("Shift+Left", &Key::Named(Named::ArrowLeft), none)); // shift is required
        assert!(binding_matches("Ctrl+M", &Key::Character("m".into()), Modifiers::CTRL));
        assert!(!binding_matches("Left", &Key::Named(Named::ArrowRight), none)); // wrong key
        assert!(!binding_matches("", &Key::Named(Named::Space), none)); // unbound never matches
    }
}
