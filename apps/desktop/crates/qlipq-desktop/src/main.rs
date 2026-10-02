#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! qlipq — recording queue + libav clip editor desktop app.
//!
//! `qlipq-core` owns the domain model; `qlipq-ffmpeg` owns the pure encode/rate-control planning.
//! This binary is the host + UI, and it decodes, previews, probes, and exports **in process** via
//! libav (rsmpeg) — no external `ffmpeg`/`ffprobe` binary is spawned. Preview runs through libplacebo
//! (HDR→SDR tonemap) with synced cpal audio ([`libav`]); export decodes → edits → hardware-encodes →
//! muxes ([`export`]).

mod discovery;
mod export;
mod host;
mod iso;
mod jobs;
mod libav;
mod log_ctx;
mod persistence;
mod preview;
mod seeker;
mod theme;
mod video;

// The in-process libav preview player (libplacebo HDR tonemap + synced cpal audio), exposing
// `poll`/`dimensions`/`fps`/`position`/`try_seek` for the editor below.
use jobs::{AfterExport, ExportRequest, Exports, FileMutation, FileOperation};
use libav::PlayerHandle as PreviewPlayer;

use std::collections::HashSet;
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
/// Sentinel "no filter" entries for the game/tag `pick_list`s (their reset option).
const ALL_GAMES: &str = "All games";
const ALL_TAGS_LABEL: &str = "All tags";

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
    "ultrafast",
    "superfast",
    "veryfast",
    "faster",
    "fast",
    "medium",
    "slow",
    "slower",
    "veryslow",
];

// ---- queue filter (pinned to the top of the queue sidebar) ----

/// Status facet of the queue filter, as a `pick_list` option (with an "all" reset entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusFilter {
    All,
    Only(QueueStatus),
}
impl StatusFilter {
    const ALL: &'static [StatusFilter] = &[
        StatusFilter::All,
        StatusFilter::Only(QueueStatus::Pending),
        StatusFilter::Only(QueueStatus::Ready),
        StatusFilter::Only(QueueStatus::Editing),
        StatusFilter::Only(QueueStatus::Exporting),
        StatusFilter::Only(QueueStatus::Done),
        StatusFilter::Only(QueueStatus::Error),
    ];
    fn from_opt(o: Option<QueueStatus>) -> Self {
        o.map_or(StatusFilter::All, StatusFilter::Only)
    }
    fn to_opt(self) -> Option<QueueStatus> {
        match self {
            StatusFilter::All => None,
            StatusFilter::Only(s) => Some(s),
        }
    }
}
impl std::fmt::Display for StatusFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatusFilter::All => write!(f, "All statuses"),
            StatusFilter::Only(s) => write!(f, "{}", status_label(*s)),
        }
    }
}

/// Highlight facet: show everything, only auto-captured highlights, or hide them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum HighlightFilter {
    #[default]
    All,
    Only,
    Hide,
}
impl HighlightFilter {
    const ALL: &'static [HighlightFilter] = &[
        HighlightFilter::All,
        HighlightFilter::Only,
        HighlightFilter::Hide,
    ];
    fn from_hidden(hidden: bool) -> Self {
        if hidden {
            HighlightFilter::Hide
        } else {
            HighlightFilter::All
        }
    }
}
impl std::fmt::Display for HighlightFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            HighlightFilter::All => "All clips",
            HighlightFilter::Only => "Only highlights",
            HighlightFilter::Hide => "Hide highlights",
        };
        write!(f, "{s}")
    }
}

/// The live queue filter, pinned above the queue list. Transient (not persisted); `highlights` is
/// seeded from `config.hide_highlights` and kept in sync with that persisted default.
#[derive(Debug, Clone, Default)]
struct QueueFilter {
    search: String,
    status: Option<QueueStatus>,
    game: Option<String>,
    tag: Option<String>,
    highlights: HighlightFilter,
}
impl QueueFilter {
    /// Whether `item` passes every active facet. `is_highlight` is whether the clip is an
    /// auto-captured highlight (the caller computes it from the media cache).
    fn matches(&self, item: &QueueItem, is_highlight: bool) -> bool {
        let tag_hit = || {
            self.tag
                .as_ref()
                .is_some_and(|t| item.tags.as_ref().is_some_and(|ts| ts.contains(t)))
        };
        // Dismissed clips stay hidden unless the tag filter targets them (so you can find/restore a
        // dismissed clip by picking its tag).
        if item_dismissed(item) && !tag_hit() {
            return false;
        }
        let highlight_ok = match self.highlights {
            HighlightFilter::All => true,
            HighlightFilter::Only => is_highlight,
            HighlightFilter::Hide => !is_highlight,
        };
        let search = self.search.trim().to_lowercase();
        highlight_ok
            && self.status.is_none_or(|s| item.status == s)
            && self
                .game
                .as_deref()
                .is_none_or(|g| item.source.as_deref() == Some(g))
            && self.tag.as_ref().is_none_or(|_| tag_hit())
            && (search.is_empty() || item.file_name.to_lowercase().contains(&search))
    }

    /// Whether any facet deviates from the default (empty search + no status/game/tag + the
    /// config-derived highlight default). Drives the "Clear filters" affordance and empty-state copy.
    fn is_active(&self, default_highlights: HighlightFilter) -> bool {
        !self.search.trim().is_empty()
            || self.status.is_some()
            || self.game.is_some()
            || self.tag.is_some()
            || self.highlights != default_highlights
    }
}

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
    /// Editable In/Out timestamp text, refreshed from `trim_start`/`trim_end` on each committed change.
    in_input: String,
    out_input: String,
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
    /// Set while a loop-back warm-seek to the in-point is in flight, so the tick doesn't spam seeks
    /// until the clock re-anchors at the in-point.
    awaiting_loop: bool,
    /// Owns the worker that opens, drives and closes this editor's media handles.
    preview: preview::Session,
}

struct RenameState {
    id: String,
    value: String,
}

struct App {
    config: AppConfig,
    persistence: persistence::Persistence,
    reveal_config_after_save: Option<u64>,
    items: Vec<QueueItem>,
    known_paths: HashSet<String>,
    edit_store: host::EditStore,
    /// Persisted probe results (duration + parsed metadata) keyed by path, so the backlog isn't
    /// re-probed on every launch. Written back debounced via `media_cache_dirty` on the tick.
    media_cache: host::MediaCache,
    media_cache_dirty: bool,
    selected_id: Option<String>,
    view: View,
    filter: QueueFilter,
    presets: host::CapturePresets,
    watcher: Option<host::Watcher>,
    discovery: discovery::Discovery,
    editor: Option<Editor>,
    audio_defaults: Vec<AudioTrackSpec>,
    rename: Option<RenameState>,
    delete_confirm: Option<String>,
    /// File-operation and persistence failures shown in a modal.
    delete_error: Option<String>,
    new_tag: String,
    exports: Exports,
    pending_export: Option<ExportRequest>,
    after_prompt: Option<AfterExport>,
    file_operations: HashSet<String>,
    retired_previews: Vec<(String, tokio::sync::oneshot::Receiver<()>)>,
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
    Scanned(u64, host::ScanResult),
    /// Freshly scanned files classified against the media cache: stats for the list, plus any cached
    /// probe so unchanged files skip re-probing.
    MediaResolved(u64, Vec<host::MediaResolution>),
    /// A background duration probe finished for a cache miss (`size`/`modified_ms` stamp the cache).
    MediaProbedBg {
        revision: u64,
        path: String,
        size: i64,
        modified_ms: i64,
        result: Result<(MediaInfo, bool), String>,
    },
    PresetsDetected(Option<String>, Option<String>),
    SelectItem(String),
    MediaProbed(u64, String, Result<(MediaInfo, bool), String>),
    PreviewReady(preview::Event),
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
    /// Nudge the in/out point by ± seconds (the 0.5/1/5 s bump buttons).
    BumpIn(f64),
    BumpOut(f64),
    /// Editable in/out timestamp fields: `*Edited` while typing, `*Submit` on Enter.
    InEdited(String),
    InSubmit,
    OutEdited(String),
    OutSubmit,
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
    ExportFinished(u64, Result<(), String>),
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
    FileOperationFinished(FileOperation, Result<(), String>),
    MoveFolderPicked(AfterExport, Option<String>),
    Dismiss(String),
    SetTagFilter(Option<String>),
    FilterSearch(String),
    SetStatusFilter(Option<QueueStatus>),
    SetGameFilter(Option<String>),
    SetHighlightFilter(HighlightFilter),
    ClearFilters,
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
    /// Restore the HDR preview brightness to its default and re-apply it to the preview.
    ResetHdrPreviewGamma,
    SetPreviewRes(PreviewResChoice),
    ToggleAutoplay(bool),
    ToggleDebug(bool),
    ToggleHideHighlights(bool),
    /// Copy the given text (the debug panel's diagnostics) to the system clipboard.
    CopyText(String),
    SetAfter(AfterChoice),
    MoveFolderChanged(String),
    RenamePrefixChanged(String),
    RenameSuffixChanged(String),
    OpenConfigFile,
}

/// Offload blocking host work onto tokio's blocking pool.
async fn blocking<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .expect("blocking task panicked")
}

impl App {
    fn new() -> (Self, Task<Message>) {
        host::migrate_legacy_data();
        let _ = host::write_config_schema();
        let config = host::load_config();
        let edit_store = host::load_edit_store();
        let media_cache = host::load_media_cache();
        let mut app = Self::with_data(
            config,
            edit_store,
            media_cache,
            persistence::Persistence::new(),
        );
        app.watcher = host::start_watch(&app.config.watched_folders, &app.config.video_extensions);
        let folders = app.config.watched_folders.clone();
        let scan = app.request_scan(folders);
        let presets = Task::perform(blocking(host::detect_capture_presets), |p| {
            Message::PresetsDetected(p.obs, p.nvidia_share)
        });
        (app, Task::batch([scan, presets]))
    }

    fn with_data(
        config: AppConfig,
        edit_store: host::EditStore,
        media_cache: host::MediaCache,
        persistence: persistence::Persistence,
    ) -> Self {
        let filter = QueueFilter {
            highlights: HighlightFilter::from_hidden(config.hide_highlights),
            ..Default::default()
        };
        App {
            config,
            persistence,
            reveal_config_after_save: None,
            items: Vec::new(),
            known_paths: HashSet::new(),
            edit_store,
            media_cache,
            media_cache_dirty: false,
            selected_id: None,
            view: View::Queue,
            filter,
            presets: host::CapturePresets::default(),
            watcher: None,
            discovery: discovery::Discovery::default(),
            editor: None,
            audio_defaults: Vec::new(),
            rename: None,
            delete_confirm: None,
            delete_error: None,
            new_tag: String::new(),
            exports: Exports::default(),
            pending_export: None,
            after_prompt: None,
            file_operations: HashSet::new(),
            retired_previews: Vec::new(),
            hovered_card: None,
            fullscreen: false,
            preview_scale: 1.0,
            theme: theme::dark(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![iced::time::every(TICK).map(|_| Message::Tick)];
        // Editor keyboard shortcuts — active only with a clip open and no modal in front. A focused
        // text field captures the key (Status::Captured), so `editor_key_event` ignores it; here we
        // just avoid binding over the queue/settings or a dialog.
        let modal = self.rename.is_some()
            || self.delete_confirm.is_some()
            || self.delete_error.is_some()
            || self.pending_export.is_some()
            || self.after_prompt.is_some();
        if self.editor.is_some() && !modal {
            subs.push(iced::event::listen_with(editor_key_event));
        }
        // Escape dismisses a modal; otherwise it exits fullscreen.
        if modal {
            subs.push(iced::event::listen_with(modal_escape_event));
            // Enter confirms the delete prompt (matches its primary Delete button).
            if self.delete_confirm.is_some() {
                subs.push(iced::event::listen_with(delete_confirm_enter_event));
            }
        } else if self.fullscreen {
            subs.push(iced::event::listen_with(fullscreen_escape_event));
        }
        // While playing, add a fast tick at the preview frame rate to pull streamed frames.
        if let Some(player) = self
            .editor
            .as_ref()
            .filter(|e| e.playing)
            .and_then(|e| e.player.as_ref())
        {
            let dt = Duration::from_secs_f64(1.0 / player.fps().clamp(1.0, 60.0));
            subs.push(iced::time::every(dt).map(|_| Message::PlaybackTick));
        }
        Subscription::batch(subs)
    }

    fn theme(&self) -> Theme {
        self.theme.clone()
    }

    fn save_config_task(&mut self) -> Task<Message> {
        if let Err(error) = self.persistence.save_config(self.config.clone()) {
            self.delete_error = Some(error);
        }
        Task::none()
    }

    fn restart_watch_and_scan(&mut self) -> Task<Message> {
        self.watcher =
            host::start_watch(&self.config.watched_folders, &self.config.video_extensions);
        self.discovery.reset(self.config.watched_folders.clone());
        self.scan_pending(true)
    }

    fn request_scan(&mut self, roots: Vec<String>) -> Task<Message> {
        self.discovery.request(roots);
        self.scan_pending(true)
    }

    fn start_pending_scan(&mut self) -> Task<Message> {
        self.scan_pending(false)
    }

    fn scan_pending(&mut self, immediate: bool) -> Task<Message> {
        if !self.file_operations.is_empty() {
            return Task::none();
        }
        let Some((generation, folders)) = self.discovery.begin(immediate) else {
            return Task::none();
        };
        let exts = self.config.video_extensions.clone();
        Task::perform(
            blocking(move || host::scan_folders(&folders, &exts)),
            move |scan| Message::Scanned(generation, scan),
        )
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
            if let Err(error) = self.persistence.save_edits(self.edit_store.clone()) {
                self.delete_error = Some(error);
            }
        }
    }

    fn add_paths(&mut self, generation: u64, paths: Vec<String>) -> Task<Message> {
        let roots = self.config.watched_folders.clone();
        let mut refresh = Vec::new();
        let mut new_items = Vec::new();
        for raw in paths {
            let path = host::to_posix(&raw);
            if self
                .items
                .iter()
                .any(|item| item.path == path && self.file_operations.contains(&item.id))
            {
                continue;
            }
            if self.known_paths.insert(path.clone()) {
                let mut item = build_item(&path, &roots);
                if let Some(stored) = self.edit_store.get(&path) {
                    item.edit = stored.edit.clone();
                    item.output_override = stored.output_override.clone();
                    item.tags = stored.tags.clone();
                }
                new_items.push(item);
            }
            refresh.push(path);
        }
        if refresh.is_empty() {
            return Task::none();
        }
        // Newest first.
        for item in new_items.into_iter().rev() {
            self.items.insert(0, item);
        }
        // Existing recordings are re-statted too: a create event can arrive before recording ends.
        let cache = self.media_cache.clone();
        Task::perform(
            blocking(move || host::resolve_media(&refresh, &cache)),
            move |list| Message::MediaResolved(generation, list),
        )
    }

    fn effective_output(&self, item: &QueueItem) -> OutputSettings {
        let mut out = self.config.output.clone();
        if let Some(o) = &item.output_override {
            if let Some(v) = o.quality_mode {
                out.quality_mode = v;
            }
            if let Some(v) = o.quality_preset {
                out.quality_preset = v;
            }
            if let Some(v) = o.crf {
                out.crf = v;
            }
            if let Some(v) = o.video_bitrate_kbps {
                out.video_bitrate_kbps = v;
            }
            if let Some(v) = &o.encoder_preset {
                out.encoder_preset = v.clone();
            }
            if let Some(v) = o.video_codec {
                out.video_codec = v;
            }
            if let Some(v) = o.container {
                out.container = v;
            }
            if let Some(v) = o.fps {
                out.fps = v;
            }
            if let Some(v) = o.max_height {
                out.max_height = v;
            }
            if let Some(v) = o.audio_bitrate_kbps {
                out.audio_bitrate_kbps = v;
            }
        }
        out
    }

    /// Set the in-point to `secs`, clamped to `[0, out − 0.1s]` (out never moves), then persist +
    /// refresh the editable field. Shared by Set-in, the ± bumps, and the text entry.
    fn apply_trim_in(&mut self, secs: f64) {
        if !secs.is_finite() {
            return;
        }
        if let Some(ed) = &mut self.editor {
            ed.trim_start = secs.clamp(0.0, (ed.trim_end - 0.1).max(0.0));
            sync_inout_inputs(ed);
        }
        self.commit_spec();
    }

    /// Set the out-point to `secs`, clamped to `[in + 0.1s, duration]`, then persist + refresh the field.
    fn apply_trim_out(&mut self, secs: f64) {
        if !secs.is_finite() {
            return;
        }
        if let Some(ed) = &mut self.editor {
            let max = ed
                .media
                .as_ref()
                .map(|m| m.duration_sec)
                .unwrap_or(ed.trim_end);
            ed.trim_end = secs.clamp(ed.trim_start + 0.1, max.max(ed.trim_start + 0.1));
            sync_inout_inputs(ed);
        }
        self.commit_spec();
    }

    /// Whether the clip at `path` is an auto-captured highlight, per its cached `encoder` tag (NVIDIA
    /// App tags highlights `"NVIDIA APP (Highlights)"`). Used to hide auto-saves from the queue.
    fn is_highlight(&self, path: &str) -> bool {
        self.media_cache
            .get(path)
            .is_some_and(|c| qlipq_core::media::is_auto_highlight(c.media.encoder.as_deref()))
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
            Message::PreviewZoom(delta) => {
                self.preview_scale = (self.preview_scale + delta).clamp(0.5, 2.5)
            }
            Message::OpenRepo => host::open_external("https://github.com/qcksys/qlipq"),
            Message::RescanAll => {
                return self.request_scan(self.config.watched_folders.clone());
            }
            Message::Scanned(generation, scan) => {
                if !self.file_operations.is_empty() {
                    self.discovery.request(self.config.watched_folders.clone());
                }
                if !self.discovery.finish(generation) {
                    return self.start_pending_scan();
                }
                let missing = discovery::missing_paths(
                    self.known_paths.iter(),
                    &scan.paths,
                    &scan.complete_roots,
                );
                for path in missing {
                    let id = self
                        .items
                        .iter()
                        .find(|item| item.path == path)
                        .map(|item| item.id.clone());
                    if let Some(id) = id {
                        if !self.file_operations.contains(&id) && !self.exports.contains(&id) {
                            self.remove_item(&id);
                        }
                    }
                }
                return self.add_paths(generation, scan.paths);
            }
            Message::MediaResolved(generation, list) => {
                if !self.discovery.is_current(generation) {
                    return Task::none();
                }
                let mut to_probe: Vec<(String, i64, i64, u64)> = Vec::new();
                for host::MediaResolution {
                    path,
                    size,
                    modified_ms,
                    cached,
                } in list
                {
                    if let Some(item) = self.items.iter_mut().find(|i| i.path == path) {
                        if self.file_operations.contains(&item.id) {
                            continue;
                        }
                        item.file_size_bytes = Some(size);
                        item.file_modified_at = Some(iso::from_unix_ms(modified_ms));
                        let revision =
                            self.discovery
                                .probe(&path, size, modified_ms, cached.is_some());
                        if let Some(c) = cached {
                            item.duration_sec = Some(c.media.duration_sec);
                            continue;
                        }
                        item.duration_sec = None;
                        item.media = None;
                        if self.media_cache.remove(&path).is_some() {
                            self.media_cache_dirty = true;
                        }
                        if let Some(revision) = revision {
                            to_probe.push((path, size, modified_ms, revision));
                        }
                    }
                }
                if to_probe.is_empty() {
                    return Task::none();
                }
                // Probe only the misses, capped at PROBE_SEM permits so a large folder doesn't
                // saturate the blocking pool and starve the editor's on-demand probe.
                return Task::batch(to_probe.into_iter().map(
                    |(path, size, modified_ms, revision)| {
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
                                revision,
                                result,
                            },
                        )
                    },
                ));
            }
            Message::MediaProbedBg {
                path,
                size,
                modified_ms,
                revision,
                result,
            } => {
                if !self.discovery.finish_probe(&path, revision) {
                    return Task::none();
                }
                if let Ok((media, is_hdr)) = result {
                    if self.known_paths.contains(&path) {
                        if let Some(item) = self.items.iter_mut().find(|i| i.path == path) {
                            item.duration_sec = Some(media.duration_sec);
                        }
                        self.media_cache.insert(
                            path,
                            host::CachedMedia {
                                size_bytes: size,
                                modified_ms,
                                media,
                                is_hdr,
                            },
                        );
                        // Debounced write on the tick — a first-run backlog probes many files at once.
                        self.media_cache_dirty = true;
                    }
                }
            }
            Message::PresetsDetected(obs, nvidia) => {
                self.presets = host::CapturePresets {
                    obs,
                    nvidia_share: nvidia,
                };
            }
            Message::SelectItem(id) => return self.select_item(id),
            Message::MediaProbed(session, id, result) => {
                if self
                    .editor
                    .as_ref()
                    .is_none_or(|ed| ed.preview.id() != session)
                {
                    return Task::none();
                }
                self.on_media_probed(id, result);
                // Autoplay the freshly opened clip when enabled; otherwise show a paused first frame.
                let ready = self
                    .editor
                    .as_ref()
                    .map_or(false, |e| e.load_error.is_none() && e.media.is_some());
                if self.config.autoplay && ready {
                    return self.play_from_current();
                }
                return self.request_frame();
            }
            Message::PreviewReady(event) => {
                let mut redo = false;
                if let Some(ed) = &mut self.editor {
                    if !ed.preview.accepts(&event) {
                        return Task::none();
                    }
                    ed.decode_hw = event.decode_hw;
                    ed.preview_dims = event.dimensions;
                    match event.outcome {
                        preview::Outcome::Started(player) => {
                            if let Some(player) = &player {
                                for track in &ed.audio {
                                    player.set_gain(track.index, track.volume);
                                }
                            }
                            ed.playing = player.is_some();
                            ed.player = player;
                            redo = !ed.playing;
                        }
                        preview::Outcome::Frame(frame) => {
                            ed.extracting = false;
                            redo = ed.frame_dirty;
                            if let Some((w, h, rgba, realized)) = frame {
                                video::push_frame(&ed.shared_frame, w, h, rgba);
                                ed.has_frame = true;
                                if !redo {
                                    ed.current_time = realized;
                                    sync_time_input(ed);
                                }
                            }
                        }
                        preview::Outcome::Superseded => {}
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
                        seeked = ed
                            .player
                            .as_ref()
                            .map(|p| p.try_seek(ed.current_time))
                            .unwrap_or(false);
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
                let parsed = self
                    .editor
                    .as_ref()
                    .and_then(|ed| parse_timestamp(&ed.time_input, ed_fps(ed)));
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
                // Delete the highlighted clip: plain Delete asks first (Enter confirms), Shift+Delete
                // removes it immediately. A focused text field captures the key first, so this only
                // fires when the editor itself has focus.
                if matches!(
                    key,
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Delete)
                ) {
                    let Some(id) = self.selected_id.clone() else {
                        return Task::none();
                    };
                    if mods.shift() {
                        return self.delete_now(id);
                    }
                    self.delete_confirm = Some(id);
                    return Task::none();
                }
                let snap = self
                    .editor
                    .as_ref()
                    .and_then(|e| e.media.as_ref())
                    .map(|m| (m.fps.max(1.0), m.duration_sec));
                let Some((fps, dur)) = snap else {
                    return Task::none();
                };
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
                        seeked = ed
                            .player
                            .as_ref()
                            .map(|p| p.try_seek(ed.current_time))
                            .unwrap_or(false);
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
                        ed.preview.stop();
                        ed.extracting = false;
                    }
                    return self.request_frame(); // crisp, exact frame at the pause point
                } else {
                    return self.play_from_current();
                }
            }
            Message::SetIn => {
                if let Some(t) = self.editor.as_ref().map(|e| e.current_time) {
                    self.apply_trim_in(t);
                }
            }
            Message::SetOut => {
                if let Some(t) = self.editor.as_ref().map(|e| e.current_time) {
                    self.apply_trim_out(t);
                }
            }
            Message::BumpIn(delta) => {
                if let Some(v) = self.editor.as_ref().map(|e| e.trim_start + delta) {
                    self.apply_trim_in(v);
                }
            }
            Message::BumpOut(delta) => {
                if let Some(v) = self.editor.as_ref().map(|e| e.trim_end + delta) {
                    self.apply_trim_out(v);
                }
            }
            Message::InEdited(s) => {
                if let Some(ed) = &mut self.editor {
                    ed.in_input = s;
                }
            }
            Message::InSubmit => {
                let parsed = self
                    .editor
                    .as_ref()
                    .and_then(|e| parse_timestamp(&e.in_input, ed_fps(e)));
                match parsed {
                    Some(v) => self.apply_trim_in(v),
                    None => {
                        if let Some(ed) = &mut self.editor {
                            sync_inout_inputs(ed); // invalid input → snap back to the current in-point
                        }
                    }
                }
            }
            Message::OutEdited(s) => {
                if let Some(ed) = &mut self.editor {
                    ed.out_input = s;
                }
            }
            Message::OutSubmit => {
                let parsed = self
                    .editor
                    .as_ref()
                    .and_then(|e| parse_timestamp(&e.out_input, ed_fps(e)));
                match parsed {
                    Some(v) => self.apply_trim_out(v),
                    None => {
                        if let Some(ed) = &mut self.editor {
                            sync_inout_inputs(ed);
                        }
                    }
                }
            }
            Message::ToggleCrop(on) => {
                if let Some(ed) = &mut self.editor {
                    ed.crop_enabled = on;
                    if on {
                        if let Some(m) = &ed.media {
                            if ed.crop.width <= 0 {
                                ed.crop = CropSpec {
                                    x: 0,
                                    y: 0,
                                    width: m.width,
                                    height: m.height,
                                };
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
            Message::OverrideQm(c) => {
                return self.patch_override(|o| o.quality_mode = Some(c.to_core()))
            }
            Message::OverrideQp(c) => {
                return self.patch_override(|o| o.quality_preset = Some(c.to_core()))
            }
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
                self.exports.cancel();
            }
            Message::Overwrite(choice) => {
                if let Some(mut request) = self.pending_export.take() {
                    match choice {
                        0 => return self.run_export_to(request),
                        1 => {
                            request.output_path = append_timestamp(&request.output_path);
                            return self.run_export_to(request);
                        }
                        _ => {}
                    }
                }
            }
            Message::ExportFinished(id, result) => return self.on_export_finished(id, result),
            Message::AfterChoice(action) => {
                if let Some(source) = self.after_prompt.take() {
                    return self.run_after_action(source, action);
                }
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
                        let suggested =
                            rename::build_renamed_file_name(&self.config.naming_template, &vars);
                        r.value = rename::split_file_name(&suggested).0;
                    }
                }
            }
            Message::RenameConfirm => return self.confirm_rename(),
            Message::RenameCancel => self.rename = None,
            Message::RequestDelete(id) => self.delete_confirm = Some(id),
            Message::DeleteConfirm => {
                if let Some(id) = self.delete_confirm.take() {
                    return self.delete_now(id);
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
                } else if self.pending_export.is_some() {
                    self.pending_export = None;
                } else {
                    self.after_prompt = None;
                }
            }
            Message::FileOperationFinished(operation, result) => {
                return self.on_file_operation_finished(operation, result);
            }
            Message::MoveFolderPicked(source, folder) => {
                if let Some(folder) = folder {
                    let dest = host::join_path(&folder, &host::base_name(&source.input));
                    return self.start_file_operation(FileOperation {
                        item_id: source.item_id,
                        path: source.input,
                        mutation: FileMutation::Rename(dest),
                    });
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
                            self.retire_editor();
                        }
                    }
                }
                self.persist_edit(&id);
            }
            Message::SetTagFilter(t) => self.filter.tag = t,
            Message::FilterSearch(s) => self.filter.search = s,
            Message::SetStatusFilter(s) => self.filter.status = s,
            Message::SetGameFilter(g) => self.filter.game = g,
            Message::SetHighlightFilter(h) => self.filter.highlights = h,
            Message::ClearFilters => {
                self.filter = QueueFilter {
                    highlights: HighlightFilter::from_hidden(self.config.hide_highlights),
                    ..Default::default()
                };
            }
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
            Message::FolderPicked(purpose, Some(path)) => {
                return self.on_folder_picked(purpose, path)
            }
            Message::FolderPicked(_, None) => {}
            Message::RemoveFolder(folder) => {
                self.config.watched_folders.retain(|f| f != &folder);
                return Task::batch([self.save_config_task(), self.restart_watch_and_scan()]);
            }
            Message::Reprocess(folder) => {
                self.view = View::Queue;
                return self.request_scan(vec![folder]);
            }
            Message::AddPreset(folder) => return self.add_watched_folder(folder),
            Message::OutputFolderChanged(s) => {
                self.config.output_folder = s;
                return self.save_config_task();
            }
            Message::NamingChanged(s) => {
                self.config.naming_template = s;
                return self.save_config_task();
            }
            Message::SetQm(c) => {
                self.config.output.quality_mode = c.to_core();
                return self.save_config_task();
            }
            Message::SetQp(c) => {
                self.config.output.quality_preset = c.to_core();
                return self.save_config_task();
            }
            Message::SetCrf(s) => {
                if let Ok(v) = s.parse::<i64>() {
                    self.config.output.crf = v.clamp(0, 51);
                    return self.save_config_task();
                }
            }
            Message::SetBitrate(s) => {
                if let Ok(v) = s.parse::<i64>() {
                    self.config.output.video_bitrate_kbps = v.max(100);
                    return self.save_config_task();
                }
            }
            Message::SetEncoder(s) => {
                self.config.output.encoder_preset = s;
                return self.save_config_task();
            }
            Message::SetCodec(c) => {
                self.config.output.video_codec = c.to_core();
                return self.save_config_task();
            }
            Message::SetContainer(c) => {
                self.config.output.container = c.to_core();
                return self.save_config_task();
            }
            Message::SetFps(c) => {
                self.config.output.fps = c.to_core();
                return self.save_config_task();
            }
            Message::SetRes(c) => {
                self.config.output.max_height = c.to_core();
                return self.save_config_task();
            }
            Message::SetAudioKbps(c) => {
                self.config.output.audio_bitrate_kbps = c.to_core();
                return self.save_config_task();
            }
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
            Message::ResetHdrPreviewGamma => {
                self.config.hdr_preview_gamma = AppConfig::default().hdr_preview_gamma;
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
            Message::ToggleAutoplay(on) => {
                self.config.autoplay = on;
                return self.save_config_task();
            }
            Message::ToggleDebug(on) => {
                self.config.debug = on;
                return self.save_config_task();
            }
            Message::ToggleHideHighlights(on) => {
                self.config.hide_highlights = on;
                self.filter.highlights = HighlightFilter::from_hidden(on);
                return self.save_config_task();
            }
            Message::CopyText(s) => return iced::clipboard::write(s),
            Message::SetAfter(c) => {
                self.config.after_export.action = c.to_core();
                return self.save_config_task();
            }
            Message::MoveFolderChanged(s) => {
                self.config.after_export.move_folder = s;
                return self.save_config_task();
            }
            Message::RenamePrefixChanged(s) => {
                self.config.after_export.rename_prefix = s;
                return self.save_config_task();
            }
            Message::RenameSuffixChanged(s) => {
                self.config.after_export.rename_suffix = s;
                return self.save_config_task();
            }
            Message::OpenConfigFile => match self.persistence.save_config(self.config.clone()) {
                Ok(id) => self.reveal_config_after_save = Some(id),
                Err(error) => self.delete_error = Some(error),
            },
        }
        Task::none()
    }

    fn on_tick(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        if let Some(w) = &self.watcher {
            if w.drain() {
                self.discovery.request(self.config.watched_folders.clone());
            }
        }
        tasks.push(self.start_pending_scan());
        self.retired_previews.retain_mut(|(_, release)| {
            matches!(
                release.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            )
        });
        for completion in self.persistence.take_results() {
            if completion.id.is_some() && completion.id == self.reveal_config_after_save {
                self.reveal_config_after_save = None;
                if completion.result.is_ok() {
                    host::reveal(&host::config_path().to_string_lossy());
                }
            }
            if let Err(error) = completion.result {
                self.delete_error = Some(error);
            }
        }
        // Flush new probe results to the media cache, coalesced so a backlog's probe storm is a
        // handful of writes rather than one per file.
        if self.media_cache_dirty {
            self.media_cache_dirty = false;
            if let Err(error) = self.persistence.save_media_cache(self.media_cache.clone()) {
                self.delete_error = Some(error);
            }
        }
        Task::batch(tasks)
    }

    /// Pull one streamed frame from the warm decoder per playback tick (real-time pacing comes
    /// from the channel backpressure: the decoder produces at roughly the rate we consume).
    fn on_playback_tick(&mut self) {
        let Some(ed) = &mut self.editor else { return };
        let polled = ed
            .player
            .as_ref()
            .map(|p| (p.poll(), p.dimensions(), p.fps(), p.position()));
        let Some((frame, (w, h), fps, position)) = polled else {
            return;
        };
        let dur = ed
            .media
            .as_ref()
            .map(|m| m.duration_sec)
            .filter(|d| *d > 0.0);

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
                ed.preview.stop();
                sync_time_input(ed);
                return;
            }
        };
        if !advance {
            return;
        }
        // Advance the playhead from the master clock (audio-synced, or a wall clock without audio);
        // fall back to one frame interval only if the clock reports no position.
        match position {
            Some(p) => ed.current_time = p,
            None => ed.current_time += 1.0 / fps,
        }
        // Keep playback inside the in/out window by looping: at the out-point, warm-seek back to the
        // in-point and keep playing (a continuous preview of the trim). Looping — rather than stopping —
        // means Play/pause always toggles cleanly instead of leaving a "stopped at out" state that the
        // next Play would restart from the in-point. `awaiting_loop` suppresses repeat seeks until the
        // clock re-anchors at the in-point. The out-point is ≤ duration (clamped on load / by SetOut).
        let stop_at = match dur {
            Some(d) => ed.trim_end.min(d),
            None => ed.trim_end,
        };
        // Only loop for a real trim that ends before EOF. When the out-point IS the clip end (untrimmed,
        // or out set to the very end), stop cleanly — looping there would race the decoder's EOF and can
        // seek a thread that's already exiting (hang) instead of stopping. Unknown duration → stop too.
        let at_eof = dur.map_or(true, |d| stop_at >= d - 1e-3);
        if stop_at > 0.0 && ed.current_time >= stop_at {
            if at_eof {
                ed.current_time = stop_at;
                ed.playing = false;
                ed.player = None;
                ed.preview.stop();
            } else if !ed.awaiting_loop {
                let looped = ed
                    .player
                    .as_ref()
                    .map(|p| p.try_seek(ed.trim_start))
                    .unwrap_or(false);
                if looped {
                    ed.awaiting_loop = true;
                } else {
                    // No warm decoder to loop with (e.g. it just ended) — stop cleanly at the out-point.
                    ed.current_time = stop_at;
                    ed.playing = false;
                    ed.player = None;
                    ed.preview.stop();
                }
            }
        } else {
            ed.awaiting_loop = false;
        }
        sync_time_input(ed);
    }

    /// Rebuild the warm scrub decoder for the open clip, picking up the current preview settings
    /// (`hdr_preview_gamma`, `preview_max_height`). Used when a preview-affecting setting changes so
    /// the next extracted frame reflects it.
    fn reopen_scrubber(&mut self) {
        let Some(ed) = self.editor.as_mut() else {
            return;
        };
        let Some(media) = ed.media.as_ref() else {
            return;
        };
        let Some(path) = self
            .items
            .iter()
            .find(|i| i.id == ed.item_id)
            .map(|i| i.path.clone())
        else {
            return;
        };
        ed.preview.configure(preview::Settings {
            path,
            width: media.width,
            height: media.height,
            fps: media.fps,
            is_hdr: ed.is_hdr,
            gamma: self.config.hdr_preview_gamma,
            max_height: self.config.preview_max_height,
        });
        ed.player = None;
        ed.extracting = false;
        ed.decode_hw = None;
        ed.preview_dims = None;
    }

    /// Extract a single preview frame at the playhead (scrubbing / paused). Coalesces: if an
    /// extraction is already in flight, just mark the frame dirty and re-request on completion.
    fn request_frame(&mut self) -> Task<Message> {
        let Some(ed) = self.editor.as_mut() else {
            return Task::none();
        };
        if ed.playing || ed.media.is_none() {
            return Task::none();
        }
        if ed.extracting {
            ed.frame_dirty = true;
            return Task::none();
        }
        ed.extracting = true;
        ed.frame_dirty = false;
        Task::perform(ed.preview.frame_at(ed.current_time), Message::PreviewReady)
    }

    /// (Re)start the warm streaming decoder from the current playhead and enter the playing state.
    /// Returns a fallback single-frame task if the decoder can't be started (e.g. an unreadable file).
    fn play_from_current(&mut self) -> Task<Message> {
        let Some(ed) = self.editor.as_mut().filter(|ed| ed.media.is_some()) else {
            return Task::none();
        };
        // Play only within the in/out window: (re)start at the in-point whenever the playhead sits
        // outside it (before the in-point, or at/after the out-point). `trim_end` is 0 until the media
        // loads, so guard on a real out-point before snapping.
        let start = if ed.trim_end > 0.0
            && (ed.current_time < ed.trim_start || ed.current_time >= ed.trim_end)
        {
            ed.trim_start
        } else {
            ed.current_time
        };
        let audio_tracks = ed
            .audio
            .iter()
            .filter(|r| r.enabled)
            .map(|r| (r.index, r.volume))
            .collect();
        ed.current_time = start;
        ed.playing = true;
        ed.player = None;
        ed.extracting = false;
        ed.frame_dirty = false;
        ed.awaiting_loop = false;
        Task::perform(ed.preview.start(start, audio_tracks), Message::PreviewReady)
    }

    fn select_item(&mut self, id: String) -> Task<Message> {
        if self.file_operations.contains(&id) {
            return Task::none();
        }
        self.retire_editor();
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
            crop: CropSpec {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            audio: Vec::new(),
            current_time: 0.0,
            time_input: format_timestamp(0.0, 30.0),
            editing_time: false,
            in_input: format_timestamp(0.0, 30.0),
            out_input: format_timestamp(0.0, 30.0),
            shared_frame: video::new_shared_frame(),
            has_frame: false,
            is_hdr: false,
            decode_hw: None,
            preview_dims: None,
            frame_dirty: false,
            extracting: false,
            playing: false,
            player: None,
            awaiting_loop: false,
            preview: preview::Session::new(),
        });
        let path = item.path.clone();
        let session = self.editor.as_ref().unwrap().preview.id();
        let probe = self.editor.as_ref().unwrap().preview.probe(path);
        Task::perform(probe, move |r| Message::MediaProbed(session, id.clone(), r))
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
                let stored_edit = self
                    .items
                    .iter()
                    .find(|i| i.id == id)
                    .and_then(|i| i.edit.clone());
                if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
                    item.duration_sec = Some(media.duration_sec);
                }
                let spec = stored_edit.unwrap_or_else(|| EditSpec {
                    trim: Some(TrimSpec {
                        start_sec: 0.0,
                        end_sec: media.duration_sec,
                    }),
                    crop: None,
                    audio_tracks: qlipq_core::edit_spec::default_edit_spec(Some(&media))
                        .audio_tracks,
                });
                // Clamp a persisted trim to the real duration (a stale edits.json or a shrunk file can
                // hold end_sec > duration), so the out-point stays ≤ duration like the SetOut handler
                // enforces — the playback stop/restart logic relies on that invariant.
                let dur = media.duration_sec.max(0.0);
                ed.trim_start = spec
                    .trim
                    .as_ref()
                    .map(|t| t.start_sec)
                    .unwrap_or(0.0)
                    .clamp(0.0, dur);
                ed.trim_end = spec
                    .trim
                    .as_ref()
                    .map(|t| t.end_sec)
                    .unwrap_or(dur)
                    .clamp(ed.trim_start, dur);
                if let Some(c) = &spec.crop {
                    ed.crop_enabled = true;
                    ed.crop = c.clone();
                } else {
                    ed.crop = CropSpec {
                        x: 0,
                        y: 0,
                        width: media.width,
                        height: media.height,
                    };
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
                            enabled: ts
                                .map(|t| t.enabled)
                                .or(carried.map(|c| c.enabled))
                                .unwrap_or(true),
                            volume: ts
                                .map(|t| t.volume)
                                .or(carried.map(|c| c.volume))
                                .unwrap_or(1.0),
                        }
                    })
                    .collect();
                ed.media = Some(media);
                // Now that media (and its fps) is set, format the In/Out fields at the real frame rate.
                sync_inout_inputs(ed);
                ed.frame_dirty = true;
            }
        }
        self.reopen_scrubber();
    }

    fn toggle_override(&mut self, on: bool) -> Task<Message> {
        let Some(id) = self.selected_id.clone() else {
            return Task::none();
        };
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
        let Some(id) = self.selected_id.clone() else {
            return Task::none();
        };
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            let o = item
                .output_override
                .get_or_insert_with(OutputOverride::default);
            patch(o);
        }
        self.persist_edit(&id);
        Task::none()
    }

    fn start_export(&mut self, _force: bool) -> Task<Message> {
        if self.exports.active().is_some()
            || self.pending_export.is_some()
            || self.after_prompt.is_some()
        {
            return Task::none();
        }
        let Some(id) = self.selected_id.clone() else {
            return Task::none();
        };
        let Some(item) = self.items.iter().find(|i| i.id == id) else {
            return Task::none();
        };
        let Some(ed) = &self.editor else {
            return Task::none();
        };
        if ed.item_id != id || self.file_operations.contains(&id) {
            return Task::none();
        }
        let Some(media) = &ed.media else {
            return Task::none();
        };
        if qlipq_core::edit_spec::validate_edit_spec(&editor_spec(ed), media).is_some()
            || self.config.output_folder.is_empty()
        {
            return Task::none();
        }
        let output = self.effective_output(item);
        let (name, _) = rename::split_file_name(&item.file_name);
        let out_name = build_export_name(&self.config, item, &name, output.container.extension());
        let output_path = host::join_path(&self.config.output_folder, &out_name);
        let request = ExportRequest {
            source: AfterExport {
                item_id: id,
                input: item.path.clone(),
                settings: self.config.after_export.clone(),
            },
            output_path: output_path.clone(),
            spec: editor_spec(ed),
            output,
            media: media.clone(),
            is_hdr: ed.is_hdr,
            metadata: item
                .source
                .clone()
                .map(|s| vec![("game".into(), s)])
                .unwrap_or_default(),
        };
        let exists = host::file_exists(&output_path);
        if exists {
            self.pending_export = Some(request);
            return Task::none();
        }
        self.run_export_to(request)
    }

    fn run_export_to(&mut self, request: ExportRequest) -> Task<Message> {
        let id = request.source.item_id.clone();
        if host::same_file(&request.source.input, &request.output_path) {
            self.delete_error = Some("Choose a different output folder or filename; an export cannot replace its source recording.".into());
            return Task::none();
        }
        if self.file_operations.contains(&id)
            || !self
                .items
                .iter()
                .any(|i| i.id == id && i.path == request.source.input)
        {
            return Task::none();
        }
        let Some(job) = self.exports.start(request) else {
            return Task::none();
        };
        if let Some(ed) = self.editor.as_mut().filter(|ed| ed.item_id == id) {
            ed.preview.stop();
            ed.extracting = false;
            ed.playing = false;
            ed.player = None;
        }
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = QueueStatus::Exporting;
            item.error = None;
        }

        let job_id = job.id;
        Task::perform(
            blocking(move || {
                let request = job.request;
                export::run_export(
                    &request.source.input,
                    &request.output_path,
                    &request.spec,
                    &request.output,
                    &request.media,
                    request.is_hdr,
                    &request.metadata,
                    job.progress,
                    job.cancel,
                )
            }),
            move |result| Message::ExportFinished(job_id, result),
        )
    }

    fn on_export_finished(&mut self, job_id: u64, result: Result<(), String>) -> Task<Message> {
        let Some(job) = self.exports.finish(job_id) else {
            return Task::none();
        };
        let source = job.request.source;
        let id = source.item_id.clone();
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
                    item.export_path = Some(job.request.output_path);
                }
                // After-export.
                match source.settings.action {
                    AfterExportAction::Prompt => {
                        self.after_prompt = Some(source);
                        Task::none()
                    }
                    action => self.run_after_action(source, action),
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

    fn run_after_action(
        &mut self,
        source: AfterExport,
        action: AfterExportAction,
    ) -> Task<Message> {
        match action {
            AfterExportAction::Delete => self.start_file_operation(FileOperation {
                item_id: source.item_id,
                path: source.input,
                mutation: FileMutation::Delete,
            }),
            AfterExportAction::Move => {
                let folder = source.settings.move_folder.clone();
                if folder.is_empty() {
                    Task::perform(
                        async move {
                            rfd::AsyncFileDialog::new()
                                .pick_folder()
                                .await
                                .map(|h| host::to_posix(&h.path().to_string_lossy()))
                        },
                        move |opt| Message::MoveFolderPicked(source.clone(), opt),
                    )
                } else {
                    let dest = host::join_path(&folder, &host::base_name(&source.input));
                    self.start_file_operation(FileOperation {
                        item_id: source.item_id,
                        path: source.input,
                        mutation: FileMutation::Rename(dest),
                    })
                }
            }
            AfterExportAction::Rename => {
                let (name, ext) = rename::split_file_name(&host::base_name(&source.input));
                let renamed = format!(
                    "{}{}{}{}",
                    source.settings.rename_prefix,
                    name,
                    source.settings.rename_suffix,
                    if ext.is_empty() {
                        String::new()
                    } else {
                        format!(".{ext}")
                    }
                );
                let to = host::join_path(&host::dir_name(&source.input), &renamed);
                self.start_file_operation(FileOperation {
                    item_id: source.item_id,
                    path: source.input,
                    mutation: FileMutation::Rename(to),
                })
            }
            AfterExportAction::Nothing | AfterExportAction::Prompt => Task::none(),
        }
    }

    fn confirm_rename(&mut self) -> Task<Message> {
        let Some(r) = self.rename.take() else {
            return Task::none();
        };
        let Some(item) = self.items.iter().find(|i| i.id == r.id) else {
            return Task::none();
        };
        let (_, ext) = rename::split_file_name(&item.file_name);
        let trimmed = r.value.trim().to_string();
        if trimmed.is_empty() {
            return Task::none();
        }
        let new_name = if ext.is_empty() {
            trimmed
        } else {
            format!("{trimmed}.{ext}")
        };
        let new_path = host::join_path(&host::dir_name(&item.path), &new_name);
        self.start_file_operation(FileOperation {
            item_id: r.id,
            path: item.path.clone(),
            mutation: FileMutation::Rename(new_path),
        })
    }

    fn on_folder_picked(&mut self, purpose: PickPurpose, path: String) -> Task<Message> {
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
        self.discovery.remove(new_path);
        self.items
            .retain(|item| item.id == id || item.path != new_path);
        let new_name = host::base_name(new_path);
        let parsed = qlipq_core::obs::parse_obs_filename(&new_name);
        let old_path = self
            .items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.path.clone());
        if let Some(old) = &old_path {
            self.discovery.remove(old);
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

    fn delete_now(&mut self, id: String) -> Task<Message> {
        let Some(path) = self
            .items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.path.clone())
        else {
            return Task::none();
        };
        self.start_file_operation(FileOperation {
            item_id: id,
            path,
            mutation: FileMutation::Delete,
        })
    }

    fn retire_editor(&mut self) {
        if let Some(mut editor) = self.editor.take() {
            let released = editor.preview.release();
            self.retired_previews.push((editor.item_id, released));
        }
    }

    fn start_file_operation(&mut self, operation: FileOperation) -> Task<Message> {
        let id = &operation.item_id;
        if self.exports.contains(id) || self.file_operations.contains(id) {
            self.delete_error = Some(
                "Wait for this clip's current operation to finish before changing its file.".into(),
            );
            return Task::none();
        }
        if !self
            .items
            .iter()
            .any(|item| item.id == *id && item.path == operation.path)
        {
            self.delete_error =
                Some("The recording has moved or is no longer in the queue.".into());
            return Task::none();
        }
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.item_id == *id)
        {
            self.retire_editor();
        }
        let mut releases = Vec::new();
        let mut other = Vec::new();
        for (item_id, release) in self.retired_previews.drain(..) {
            if item_id == *id {
                releases.push(release)
            } else {
                other.push((item_id, release))
            }
        }
        self.retired_previews = other;
        self.file_operations.insert(id.clone());
        self.discovery.request(self.config.watched_folders.clone());
        let work = operation.clone();
        Task::perform(
            async move {
                for release in releases {
                    release.await.map_err(|_| {
                        "Preview shutdown failed; the recording was not changed.".to_string()
                    })?;
                }
                blocking(move || match work.mutation {
                    FileMutation::Delete => host::delete_file(&work.path),
                    FileMutation::Rename(to) => host::rename_file(&work.path, &to).map(|_| ()),
                })
                .await
            },
            move |result| Message::FileOperationFinished(operation.clone(), result),
        )
    }

    fn on_file_operation_finished(
        &mut self,
        operation: FileOperation,
        result: Result<(), String>,
    ) -> Task<Message> {
        self.file_operations.remove(&operation.item_id);
        match result {
            Ok(()) => match operation.mutation {
                FileMutation::Delete => self.remove_item(&operation.item_id),
                FileMutation::Rename(path) => self.apply_rename(&operation.item_id, &path),
            },
            Err(error) => {
                self.delete_error = Some(format!(
                    "Couldn't change {}.\n\n{error}",
                    host::base_name(&operation.path)
                ))
            }
        }
        if self.selected_id.as_deref() == Some(&operation.item_id) {
            return self.select_item(operation.item_id);
        }
        Task::none()
    }

    fn remove_item(&mut self, id: &str) {
        if let Some(pos) = self.items.iter().position(|i| i.id == id) {
            let path = self.items[pos].path.clone();
            self.discovery.remove(&path);
            self.known_paths.remove(&path);
            self.items.remove(pos);
            // A deleted recording keeps no persisted app data: drop its edit + cached probe.
            if self.edit_store.remove(&path).is_some() {
                if let Err(error) = self.persistence.save_edits(self.edit_store.clone()) {
                    self.delete_error = Some(error);
                }
            }
            if self.media_cache.remove(&path).is_some() {
                if let Err(error) = self.persistence.save_media_cache(self.media_cache.clone()) {
                    self.delete_error = Some(error);
                }
            }
        }
        if self.selected_id.as_deref() == Some(id) {
            self.selected_id = None;
            self.retire_editor();
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
                    container(self.editor_view())
                        .width(Length::Fill)
                        .height(Length::Fill),
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
        } else if let Some(request) = &self.pending_export {
            Some(self.overwrite_modal(&request.output_path))
        } else if self.after_prompt.is_some() {
            Some(self.after_modal())
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
    let game = roots
        .iter()
        .find_map(|r| qlipq_core::obs::infer_game_from_path(r, path));
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
    ed.audio
        .iter()
        .map(|r| AudioTrackSpec {
            index: r.index,
            enabled: r.enabled,
            volume: r.volume,
        })
        .collect()
}

fn editor_spec(ed: &Editor) -> EditSpec {
    EditSpec {
        trim: Some(TrimSpec {
            start_sec: ed.trim_start,
            end_sec: ed.trim_end,
        }),
        crop: if ed.crop_enabled {
            Some(ed.crop.clone())
        } else {
            None
        },
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
        if ext.is_empty() {
            String::new()
        } else {
            format!(".{ext}")
        }
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

/// A usable frame rate for timecode math — the clip's fps, or 30 as a fallback before media loads.
fn ed_fps(ed: &Editor) -> f64 {
    ed.media
        .as_ref()
        .map(|m| m.fps)
        .filter(|f| *f >= 1.0)
        .unwrap_or(30.0)
}

/// Format seconds as a frame-accurate timecode `M:SS.FF` (or `H:MM:SS.FF` past an hour), where `FF` is
/// the frame within the second (0..fps-1). Frames are the editor's natural unit — the ←/→ keys and the
/// preview step by whole frames — so the readout matches what a single step changes.
fn format_timestamp(secs: f64, fps: f64) -> String {
    let fpr = (fps.round() as u64).max(1);
    let total_frames = (secs.max(0.0) * fpr as f64).round() as u64;
    let f = total_frames % fpr;
    let total_s = total_frames / fpr;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{f:02}")
    } else {
        format!("{m}:{s:02}.{f:02}")
    }
}

/// Parse `S`, `M:SS`, `H:MM:SS`, or any of those with a trailing `.FF` frame count, into seconds. The
/// part after the final `.` is a frame count (converted via `fps`); everything before is base-60
/// H/M/S. Returns `None` on any non-numeric, negative, or non-finite component (so a stray `nan`/`inf`
/// can never poison a trim point).
fn parse_timestamp(text: &str, fps: f64) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // Round the rate to a whole number of frames per second, matching `format_timestamp`, so a value
    // it produced parses back to the same time even at fractional rates (59.94/29.97).
    let fpr = fps.round().max(1.0);
    // Optional trailing `.FF` frames component.
    let (time_part, frames) = match text.rsplit_once('.') {
        Some((t, f)) => {
            let fr: f64 = f.trim().parse().ok()?;
            if fr < 0.0 || !fr.is_finite() {
                return None;
            }
            (t, fr / fpr)
        }
        None => (text, 0.0),
    };
    let mut total = 0.0;
    for part in time_part.split(':') {
        let v: f64 = part.trim().parse().ok()?;
        if v < 0.0 || !v.is_finite() {
            return None;
        }
        total = total * 60.0 + v;
    }
    let total = total + frames;
    total.is_finite().then_some(total)
}

/// Refresh the playhead field from `current_time`, unless the user is mid-edit (the fast playback tick
/// calls this, so the guard keeps live playback from clobbering what's being typed).
fn sync_time_input(ed: &mut Editor) {
    if !ed.editing_time {
        ed.time_input = format_timestamp(ed.current_time, ed_fps(ed));
    }
}

/// Refresh the editable In/Out fields from `trim_start`/`trim_end`. Only ever called on a committed
/// change (Set/bump/submit/load), never per-tick, so it can safely overwrite — no edit latch needed.
fn sync_inout_inputs(ed: &mut Editor) {
    let fps = ed_fps(ed);
    ed.in_input = format_timestamp(ed.trim_start, fps);
    ed.out_input = format_timestamp(ed.trim_end, fps);
}

/// Raw key event → [`Message::EditorKey`], but only when no widget captured it (a focused text field
/// reports `Status::Captured`, so its keystrokes are left alone). Must be a plain `fn` —
/// `iced::event::listen_with` takes a function pointer, not a closure.
fn editor_key_event(
    event: iced::Event,
    status: iced::event::Status,
    _id: iced::window::Id,
) -> Option<Message> {
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
fn modal_escape_event(
    event: iced::Event,
    _status: iced::event::Status,
    _id: iced::window::Id,
) -> Option<Message> {
    use iced::keyboard::{key::Named, Event::KeyPressed, Key};
    if let iced::Event::Keyboard(KeyPressed {
        key: Key::Named(Named::Escape),
        ..
    }) = event
    {
        Some(Message::DismissModal)
    } else {
        None
    }
}

/// Enter key → confirm the open delete prompt. A plain `fn` for `listen_with`.
fn delete_confirm_enter_event(
    event: iced::Event,
    _status: iced::event::Status,
    _id: iced::window::Id,
) -> Option<Message> {
    use iced::keyboard::{key::Named, Event::KeyPressed, Key};
    if let iced::Event::Keyboard(KeyPressed {
        key: Key::Named(Named::Enter),
        ..
    }) = event
    {
        Some(Message::DeleteConfirm)
    } else {
        None
    }
}

/// Escape key → exit fullscreen preview. A plain `fn` for `listen_with`.
fn fullscreen_escape_event(
    event: iced::Event,
    _status: iced::event::Status,
    _id: iced::window::Id,
) -> Option<Message> {
    use iced::keyboard::{key::Named, Event::KeyPressed, Key};
    if let iced::Event::Keyboard(KeyPressed {
        key: Key::Named(Named::Escape),
        ..
    }) = event
    {
        Some(Message::ToggleFullscreen)
    } else {
        None
    }
}

/// True if `binding` (e.g. `"Shift+Left"`, `"Ctrl+M"`, `"I"`) matches the pressed key + modifiers.
fn binding_matches(
    binding: &str,
    key: &iced::keyboard::Key,
    mods: iced::keyboard::Modifiers,
) -> bool {
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
    if mods.control() != need_ctrl
        || mods.shift() != need_shift
        || mods.alt() != need_alt
        || mods.logo() != need_logo
    {
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

    struct TestDirectory(std::path::PathBuf);

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Fixture {
        app: App,
        _directory: TestDirectory,
    }

    fn fixture() -> Fixture {
        let directory = TestDirectory(
            std::env::temp_dir().join(format!("qlipq-workflow-{}", qlipq_core::ids::create_id())),
        );
        let mut app = App::with_data(
            AppConfig::default(),
            host::EditStore::default(),
            host::MediaCache::default(),
            persistence::Persistence::at_path(directory.0.clone()),
        );
        app.items = vec![
            qi("a.mp4", QueueStatus::Exporting, None, &["keep"]),
            qi("b.mp4", QueueStatus::Pending, None, &[]),
        ];
        app.known_paths = app.items.iter().map(|item| item.path.clone()).collect();
        app.selected_id = Some("b.mp4".into());
        Fixture {
            app,
            _directory: directory,
        }
    }

    fn test_export(action: AfterExportAction) -> ExportRequest {
        ExportRequest {
            source: AfterExport {
                item_id: "a.mp4".into(),
                input: "C:/clips/a.mp4".into(),
                settings: AfterExportSettings {
                    action,
                    ..Default::default()
                },
            },
            output_path: "C:/exports/a.mp4".into(),
            spec: qlipq_core::edit_spec::default_edit_spec(None),
            output: OutputSettings::default(),
            media: MediaInfo {
                duration_sec: 1.0,
                width: 16,
                height: 16,
                fps: 30.0,
                video_codec: "h264".into(),
                audio_streams: vec![],
                size_bytes: None,
                encoder: None,
            },
            is_hdr: false,
            metadata: vec![],
        }
    }

    #[test]
    fn changed_existing_recording_reprobes_and_rejects_its_old_result() {
        let mut fixture = fixture();
        let app = &mut fixture.app;
        let path = "C:/clips/a.mp4";
        let media = test_export(AfterExportAction::Nothing).media;
        app.items[0].duration_sec = Some(media.duration_sec);
        app.items[0].media = Some(media.clone());
        app.media_cache.insert(
            path.into(),
            host::CachedMedia {
                size_bytes: 10,
                modified_ms: 20,
                media: media.clone(),
                is_hdr: false,
            },
        );
        let old_revision = app.discovery.probe(path, 10, 20, false).unwrap();
        app.discovery.request(vec!["C:/clips".into()]);
        let (generation, _) = app.discovery.begin(true).unwrap();
        assert!(app.discovery.finish(generation));

        let _ = app.update(Message::MediaResolved(
            generation,
            vec![host::MediaResolution {
                path: path.into(),
                size: 30,
                modified_ms: 40,
                cached: None,
            }],
        ));

        assert_eq!(app.items[0].file_size_bytes, Some(30));
        assert_eq!(app.items[0].file_modified_at, Some(iso::from_unix_ms(40)));
        assert!(app.items[0].duration_sec.is_none());
        assert!(app.items[0].media.is_none());
        assert!(!app.media_cache.contains_key(path));
        assert!(app.media_cache_dirty);
        assert!(
            app.discovery.probe(path, 30, 40, false).is_none(),
            "replacement probe is already pending"
        );

        let _ = app.update(Message::MediaProbedBg {
            path: path.into(),
            size: 10,
            modified_ms: 20,
            revision: old_revision,
            result: Ok((media, false)),
        });
        assert!(app.items[0].duration_sec.is_none());
        assert!(!app.media_cache.contains_key(path));
    }

    #[test]
    fn export_completion_uses_job_source_and_captured_after_settings() {
        let mut fixture = fixture();
        let app = &mut fixture.app;
        let job = app
            .exports
            .start(test_export(AfterExportAction::Prompt))
            .unwrap();
        app.config.after_export.action = AfterExportAction::Delete;
        let _ = app.update(Message::ExportFinished(job.id, Ok(())));
        assert_eq!(
            app.items[0].export_path.as_deref(),
            Some("C:/exports/a.mp4")
        );
        assert_eq!(app.items[1].status, QueueStatus::Pending);
        assert_eq!(app.after_prompt.as_ref().unwrap().item_id, "a.mp4");
        assert!(app.file_operations.is_empty());
        let _ = app.update(Message::AfterChoice(AfterExportAction::Delete));
        assert!(app.file_operations.contains("a.mp4"));
        assert!(!app.file_operations.contains("b.mp4"));
    }

    #[test]
    fn automatic_after_action_does_not_follow_selection() {
        let mut fixture = fixture();
        let app = &mut fixture.app;
        let job = app
            .exports
            .start(test_export(AfterExportAction::Delete))
            .unwrap();
        let _ = app.update(Message::ExportFinished(job.id, Ok(())));
        assert!(app.file_operations.contains("a.mp4"));
        assert!(!app.file_operations.contains("b.mp4"));
    }

    #[test]
    fn export_failure_and_cancellation_never_start_after_actions() {
        for error in ["read packet failed", "cancelled"] {
            let mut fixture = fixture();
            let app = &mut fixture.app;
            let job = app
                .exports
                .start(test_export(AfterExportAction::Delete))
                .unwrap();
            let _ = app.update(Message::ExportFinished(job.id, Err(error.into())));
            assert!(app.exports.active().is_none());
            assert!(app.file_operations.is_empty());
            assert!(app.after_prompt.is_none());
            assert!(app.items[0].export_path.is_none());
            assert_eq!(
                app.items[0].status,
                if error == "cancelled" {
                    QueueStatus::Pending
                } else {
                    QueueStatus::Error
                }
            );
        }
    }

    #[test]
    fn duplicate_export_messages_and_file_mutations_cannot_replace_a_job() {
        let mut fixture = fixture();
        let app = &mut fixture.app;
        let job = app
            .exports
            .start(test_export(AfterExportAction::Nothing))
            .unwrap();
        let _ = app.update(Message::Export);
        let _ = app.delete_now("a.mp4".into());
        assert_eq!(app.exports.active().unwrap().id, job.id);
        assert!(app.file_operations.is_empty());
        let _ = app.update(Message::CancelExport);
        assert!(job.cancel.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn file_completions_update_queue_edits_and_cache_together() {
        let mut fixture = fixture();
        let app = &mut fixture.app;
        let old = "C:/clips/a.mp4";
        let new = "C:/clips/renamed.mp4";
        app.edit_store.insert(
            old.into(),
            host::StoredEdit {
                tags: Some(vec!["keep".into()]),
                ..Default::default()
            },
        );
        app.media_cache.insert(
            old.into(),
            host::CachedMedia {
                size_bytes: 12,
                modified_ms: 34,
                media: test_export(AfterExportAction::Nothing).media,
                is_hdr: false,
            },
        );
        app.file_operations.insert("a.mp4".into());
        let _ = app.update(Message::FileOperationFinished(
            FileOperation {
                item_id: "a.mp4".into(),
                path: old.into(),
                mutation: FileMutation::Rename(new.into()),
            },
            Ok(()),
        ));
        assert_eq!(app.items[0].path, new);
        assert!(app.known_paths.contains(new));
        assert!(!app.known_paths.contains(old));
        assert!(app.edit_store.contains_key(new));
        assert!(!app.edit_store.contains_key(old));
        assert!(app.media_cache.contains_key(new));
        assert!(!app.media_cache.contains_key(old));
        let _ = app.update(Message::FileOperationFinished(
            FileOperation {
                item_id: "a.mp4".into(),
                path: new.into(),
                mutation: FileMutation::Delete,
            },
            Ok(()),
        ));
        assert!(!app.items.iter().any(|item| item.id == "a.mp4"));
        assert!(!app.edit_store.contains_key(new));
        assert!(!app.media_cache.contains_key(new));
    }

    #[test]
    fn file_failure_preserves_queue_and_surfaces_error() {
        let mut fixture = fixture();
        let app = &mut fixture.app;
        let _ = app.update(Message::FileOperationFinished(
            FileOperation {
                item_id: "a.mp4".into(),
                path: "C:/clips/a.mp4".into(),
                mutation: FileMutation::Delete,
            },
            Err("file in use".into()),
        ));
        assert!(app.items.iter().any(|item| item.id == "a.mp4"));
        assert!(app.delete_error.as_ref().unwrap().contains("file in use"));
    }

    #[test]
    fn export_cannot_overwrite_its_source_before_after_action() {
        let mut fixture = fixture();
        let app = &mut fixture.app;
        let mut request = test_export(AfterExportAction::Delete);
        request.output_path = request.source.input.clone();
        let _ = app.run_export_to(request);
        assert!(app.exports.active().is_none());
        assert!(app
            .delete_error
            .as_ref()
            .unwrap()
            .contains("source recording"));
    }
    use iced::keyboard::{key::Named, Key, Modifiers};

    #[test]
    fn timecode_formats_as_frames() {
        assert_eq!(format_timestamp(0.0, 60.0), "0:00.00");
        // 5.5s @ 60fps = frame 30 within the second.
        assert_eq!(format_timestamp(5.5, 60.0), "0:05.30");
        // 30fps: 2.5s = frame 15.
        assert_eq!(format_timestamp(2.5, 30.0), "0:02.15");
        // Past an hour shows the hours field.
        assert_eq!(format_timestamp(3661.0, 60.0), "1:01:01.00");
    }

    #[test]
    fn timecode_round_trips() {
        // Includes fractional (NTSC) rates: format + parse must use the same rounded fps so a produced
        // timecode parses back to the same time (within a frame's quantization).
        for &(secs, fps) in &[
            (5.5, 60.0),
            (2.5, 30.0),
            (61.25, 60.0),
            (0.0, 60.0),
            (5.5, 59.94),
            (10.0, 29.97),
        ] {
            let parsed = parse_timestamp(&format_timestamp(secs, fps), fps).unwrap();
            assert!(
                (parsed - secs).abs() < 1.0 / fps.round(),
                "{secs} @ {fps} -> {parsed}"
            );
        }
    }

    #[test]
    fn parse_frames_and_plain_seconds() {
        assert_eq!(parse_timestamp("5", 60.0), Some(5.0)); // plain seconds
        assert_eq!(parse_timestamp("1:00", 60.0), Some(60.0)); // m:ss
        assert_eq!(parse_timestamp("0:05.30", 60.0), Some(5.5)); // frame 30 @ 60 = 0.5s
    }

    #[test]
    fn parse_rejects_non_finite_and_negative() {
        // A stray nan/inf must never reach a trim clamp (it would panic).
        for bad in [
            "nan", "NaN", "-nan", "inf", "infinity", "1e400", "1:nan", "5.nan", "-1",
        ] {
            assert_eq!(parse_timestamp(bad, 60.0), None, "{bad} should be rejected");
        }
    }

    #[test]
    fn parse_handles_hms_and_whitespace() {
        assert_eq!(parse_timestamp("90", 60.0), Some(90.0));
        assert_eq!(parse_timestamp("1:01:01", 60.0), Some(3661.0));
        assert_eq!(parse_timestamp("  2:00 ", 60.0), Some(120.0));
        assert_eq!(parse_timestamp("nope", 60.0), None);
        assert_eq!(parse_timestamp("", 60.0), None);
    }

    #[test]
    fn keybind_matching() {
        let none = Modifiers::empty();
        // Premiere defaults dispatch to the right key/modifier combos.
        assert!(binding_matches("Space", &Key::Named(Named::Space), none));
        assert!(binding_matches("i", &Key::Character("i".into()), none));
        assert!(binding_matches("I", &Key::Character("i".into()), none)); // case-insensitive
        assert!(!binding_matches(
            "I",
            &Key::Character("i".into()),
            Modifiers::SHIFT
        )); // bare I, not Shift+I
        assert!(binding_matches(
            "Shift+Left",
            &Key::Named(Named::ArrowLeft),
            Modifiers::SHIFT
        ));
        assert!(!binding_matches(
            "Shift+Left",
            &Key::Named(Named::ArrowLeft),
            none
        )); // shift is required
        assert!(binding_matches(
            "Ctrl+M",
            &Key::Character("m".into()),
            Modifiers::CTRL
        ));
        assert!(!binding_matches(
            "Left",
            &Key::Named(Named::ArrowRight),
            none
        )); // wrong key
        assert!(!binding_matches("", &Key::Named(Named::Space), none)); // unbound never matches
    }

    fn qi(file_name: &str, status: QueueStatus, source: Option<&str>, tags: &[&str]) -> QueueItem {
        QueueItem {
            id: file_name.to_string(),
            path: format!("C:/clips/{file_name}"),
            file_name: file_name.to_string(),
            added_at: String::new(),
            status,
            recorded_at: None,
            source: source.map(str::to_string),
            media: None,
            file_size_bytes: None,
            file_modified_at: None,
            duration_sec: None,
            edit: None,
            output_override: None,
            tags: (!tags.is_empty()).then(|| tags.iter().map(|t| t.to_string()).collect()),
            export_path: None,
            error: None,
        }
    }

    #[test]
    fn empty_filter_hides_dismissed_only() {
        let f = QueueFilter::default();
        assert!(f.matches(&qi("a.mp4", QueueStatus::Ready, None, &[]), false));
        assert!(!f.matches(
            &qi("b.mp4", QueueStatus::Ready, None, &[DISMISSED_TAG]),
            false
        ));
    }

    #[test]
    fn search_is_case_insensitive_substring_of_name() {
        let f = QueueFilter {
            search: "  APEX ".to_string(),
            ..Default::default()
        };
        assert!(f.matches(&qi("apex-clutch.mp4", QueueStatus::Ready, None, &[]), false));
        assert!(!f.matches(&qi("cs2-ace.mp4", QueueStatus::Ready, None, &[]), false));
    }

    #[test]
    fn status_and_game_facets() {
        let status = QueueFilter {
            status: Some(QueueStatus::Done),
            ..Default::default()
        };
        assert!(status.matches(&qi("a.mp4", QueueStatus::Done, None, &[]), false));
        assert!(!status.matches(&qi("a.mp4", QueueStatus::Ready, None, &[]), false));

        let game = QueueFilter {
            game: Some("Apex Legends".to_string()),
            ..Default::default()
        };
        assert!(game.matches(
            &qi("a.mp4", QueueStatus::Ready, Some("Apex Legends"), &[]),
            false
        ));
        assert!(!game.matches(&qi("a.mp4", QueueStatus::Ready, Some("CS2"), &[]), false));
        assert!(!game.matches(&qi("a.mp4", QueueStatus::Ready, None, &[]), false));
    }

    #[test]
    fn tag_filter_reveals_dismissed_clip_with_that_tag() {
        let f = QueueFilter {
            tag: Some("keep".to_string()),
            ..Default::default()
        };
        // A dismissed clip is normally hidden, but selecting its tag surfaces it (for restore).
        assert!(f.matches(
            &qi("a.mp4", QueueStatus::Ready, None, &["keep", DISMISSED_TAG]),
            false
        ));
        // A dismissed clip without the tag stays hidden.
        assert!(!f.matches(
            &qi("b.mp4", QueueStatus::Ready, None, &[DISMISSED_TAG]),
            false
        ));
        // A live clip without the tag is filtered out.
        assert!(!f.matches(&qi("c.mp4", QueueStatus::Ready, None, &["other"]), false));
    }

    #[test]
    fn highlight_facet_only_and_hide() {
        let only = QueueFilter {
            highlights: HighlightFilter::Only,
            ..Default::default()
        };
        assert!(only.matches(&qi("a.mp4", QueueStatus::Ready, None, &[]), true));
        assert!(!only.matches(&qi("a.mp4", QueueStatus::Ready, None, &[]), false));

        let hide = QueueFilter {
            highlights: HighlightFilter::Hide,
            ..Default::default()
        };
        assert!(!hide.matches(&qi("a.mp4", QueueStatus::Ready, None, &[]), true));
        assert!(hide.matches(&qi("a.mp4", QueueStatus::Ready, None, &[]), false));
    }

    #[test]
    fn is_active_measured_against_highlight_default() {
        // With highlights hidden by default (config), an untouched filter reads as inactive.
        let seeded = QueueFilter {
            highlights: HighlightFilter::Hide,
            ..Default::default()
        };
        assert!(!seeded.is_active(HighlightFilter::Hide));
        assert!(seeded.is_active(HighlightFilter::All));
        // Any other facet marks it active regardless of the highlight default.
        let searching = QueueFilter {
            search: "x".to_string(),
            ..Default::default()
        };
        assert!(searching.is_active(HighlightFilter::All));
    }
}
