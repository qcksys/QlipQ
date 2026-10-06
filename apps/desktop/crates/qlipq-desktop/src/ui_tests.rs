//! Headless E2E tests: real widgets, event subscriptions, tasks, libav and temporary files.
use super::*;
use iced::advanced::renderer::Headless;
use iced::advanced::widget::{operation, Operation};
use iced::futures::{channel::mpsc, executor::block_on, stream, StreamExt};
use iced::{Event, Point, Rectangle};
use iced_test::core::{clipboard, mouse, renderer, window};
use iced_test::futures::{subscription, Runtime};
use iced_test::runtime::{self, user_interface, UserInterface};
use iced_test::selector::{Bounded, Selector};

enum Output {
    Action(runtime::Action<Message>),
    Done,
}

struct Ui {
    app: App,
    runtime: Runtime<iced::executor::Default, mpsc::UnboundedSender<Output>, Output>,
    receive: mpsc::UnboundedReceiver<Output>,
    renderer: iced::Renderer,
    cache: Option<user_interface::Cache>,
    size: Size,
    cursor: mouse::Cursor,
    pending: usize,
    max_update: Duration,
    clipboard: Option<String>,
}

impl Ui {
    fn new(size: Size) -> Self {
        iced_test::renderer::graphics::text::font_system()
            .write()
            .unwrap()
            .load_font(
                include_bytes!("../assets/Inter-Variable.ttf")
                    .as_slice()
                    .into(),
            );
        let renderer = block_on(iced::Renderer::new(theme::FONT, 14.into(), None)).unwrap();
        let (send, receive) = mpsc::unbounded();
        let runtime = Runtime::new(iced::executor::Default::new().unwrap(), send);
        let (app, task) = runtime.enter(|| {
            let _ui = background::UiThread::enter();
            App::new()
        });
        let mut ui = Self {
            app,
            runtime,
            receive,
            renderer,
            cache: None,
            size,
            cursor: mouse::Cursor::Unavailable,
            pending: 0,
            max_update: Duration::ZERO,
            clipboard: None,
        };
        ui.track();
        ui.run(task);
        ui.settle();
        ui
    }

    fn track(&mut self) {
        self.runtime.track(subscription::into_recipes(
            self.app
                .subscription()
                .map(|m| Output::Action(runtime::Action::Output(m))),
        ));
    }

    fn run(&mut self, task: Task<Message>) {
        if let Some(events) = runtime::task::into_stream(task) {
            self.pending += 1;
            self.runtime.run(
                events
                    .map(Output::Action)
                    .chain(stream::once(async { Output::Done }))
                    .boxed(),
            );
        }
    }

    fn update(&mut self, message: Message) {
        let _ui = background::UiThread::enter();
        let start = Instant::now();
        let task = self.runtime.enter(|| self.app.update(message));
        self.max_update = self.max_update.max(start.elapsed());
        self.track();
        self.run(task);
    }

    fn pump(&mut self) {
        while let Ok(output) = self.receive.try_recv() {
            match output {
                Output::Done => self.pending -= 1,
                Output::Action(runtime::Action::Output(message)) => self.update(message),
                Output::Action(runtime::Action::Widget(mut op)) => {
                    let mut ui = UserInterface::build(
                        self.app.view(),
                        self.size,
                        self.cache.take().unwrap_or_default(),
                        &mut self.renderer,
                    );
                    ui.operate(&self.renderer, op.as_mut());
                    self.cache = Some(ui.into_cache());
                }
                Output::Action(runtime::Action::Clipboard(runtime::clipboard::Action::Write {
                    contents,
                    ..
                })) => self.clipboard = Some(contents),
                Output::Action(other) => panic!("Unhandled runtime action: {other:?}"),
            }
        }
    }

    fn settle(&mut self) {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            self.pump();
            if self.pending == 0 {
                break;
            }
            assert!(Instant::now() < until, "UI tasks timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
        self.event(vec![Event::Window(window::Event::RedrawRequested(
            iced_test::core::time::Instant::now(),
        ))]);
    }

    fn wait_for(&mut self, predicate: impl Fn(&App) -> bool) {
        let until = Instant::now() + Duration::from_secs(20);
        while !predicate(&self.app) {
            self.pump();
            assert!(Instant::now() < until, "UI condition timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn event(&mut self, events: Vec<Event>) {
        let _ui = background::UiThread::enter();
        let mut ui = UserInterface::build(
            self.app.view(),
            self.size,
            self.cache.take().unwrap_or_default(),
            &mut self.renderer,
        );
        let mut messages = Vec::new();
        let (_, statuses) = ui.update(
            &events,
            self.cursor,
            &mut self.renderer,
            &mut clipboard::Null,
            &mut messages,
        );
        self.cache = Some(ui.into_cache());
        for (event, status) in events.into_iter().zip(statuses) {
            self.runtime.broadcast(subscription::Event::Interaction {
                window: window::Id::unique(),
                event,
                status,
            });
        }
        for message in messages {
            self.update(message);
        }
    }

    fn find<S>(&mut self, selector: S) -> Option<S::Output>
    where
        S: Selector + Send,
        S::Output: Clone + Send + 'static,
    {
        let _ui = background::UiThread::enter();
        let mut ui = UserInterface::build(
            self.app.view(),
            self.size,
            self.cache.take().unwrap_or_default(),
            &mut self.renderer,
        );
        let mut op = selector.find();
        ui.operate(&self.renderer, &mut operation::black_box(&mut op));
        self.cache = Some(ui.into_cache());
        match op.finish() {
            operation::Outcome::Some(result) => result,
            _ => None,
        }
    }

    fn bounds<S>(&mut self, selector: S) -> Rectangle
    where
        S: Selector + Send,
        S::Output: Bounded + Clone + Send + 'static,
    {
        self.find(selector)
            .expect("Widget must exist")
            .visible_bounds()
            .expect("Widget must be visible")
    }

    fn click_at(&mut self, point: Point) {
        self.cursor = mouse::Cursor::Available(point);
        self.event(vec![
            Event::Mouse(mouse::Event::CursorMoved { position: point }),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ]);
    }

    fn click(&mut self, label: &str) {
        let point = self.bounds(label).center();
        self.click_at(point);
        self.settle();
    }

    fn confirm(&mut self, label: &str) {
        let mut ui = UserInterface::build(
            self.app.view(),
            self.size,
            self.cache.take().unwrap_or_default(),
            &mut self.renderer,
        );
        let mut op = label.find_all();
        ui.operate(&self.renderer, &mut operation::black_box(&mut op));
        self.cache = Some(ui.into_cache());
        let operation::Outcome::Some(matches) = op.finish() else {
            panic!("No {label} control");
        };
        let point = matches.last().unwrap().visible_bounds().unwrap().center();
        self.click_at(point);
        self.settle();
    }

    fn choose(&mut self, id: &str, index: usize, count: usize) {
        let bounds = self.bounds(iced_test::selector::id(id.to_owned()));
        self.click_at(bounds.center());
        let below = self.size.height - bounds.y - bounds.height;
        let menu_top = if below > bounds.y {
            bounds.y + bounds.height
        } else {
            bounds.y - (bounds.height * count as f32).min(bounds.y)
        };
        self.click_at(Point::new(
            bounds.center_x(),
            menu_top + bounds.height * (index as f32 + 0.5),
        ));
        self.settle();
    }

    fn fill(&mut self, id: &str, value: &str) {
        let point = self.bounds(iced_test::selector::id(id.to_owned())).center();
        self.click_at(point);
        self.key(
            iced::keyboard::Key::Character("a".into()),
            Some(iced::keyboard::Modifiers::CTRL),
        );
        if value.is_empty() {
            self.key(iced::keyboard::key::Named::Backspace.into(), None);
        }
        self.event(iced_test::simulator::typewrite(value).collect());
        self.settle();
    }

    fn slider(&mut self, id: &str, fraction: f32) {
        let bounds = self.bounds(iced_test::selector::id(id.to_owned()));
        self.click_at(Point::new(
            bounds.x + bounds.width * fraction,
            bounds.center_y(),
        ));
        self.settle();
    }

    fn key(&mut self, key: iced::keyboard::Key, modifiers: Option<iced::keyboard::Modifiers>) {
        let modifiers = modifiers.unwrap_or_default();
        let mut events: Vec<_> = iced_test::simulator::tap_key(key, None).collect();
        for event in &mut events {
            match event {
                Event::Keyboard(
                    iced::keyboard::Event::KeyPressed { modifiers: m, .. }
                    | iced::keyboard::Event::KeyReleased { modifiers: m, .. },
                ) => *m = modifiers,
                _ => {}
            }
        }
        events.insert(
            0,
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)),
        );
        events.push(Event::Keyboard(iced::keyboard::Event::ModifiersChanged(
            iced::keyboard::Modifiers::default(),
        )));
        self.event(events);
        self.settle();
    }

    fn scroll(&mut self, point: Point, lines: f32) {
        self.cursor = mouse::Cursor::Available(point);
        self.event(vec![Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: lines },
        })]);
        self.settle();
    }

    fn screenshot(&mut self, name: &str) {
        let _ui = background::UiThread::enter();
        let mut ui = UserInterface::build(
            self.app.view(),
            self.size,
            self.cache.take().unwrap_or_default(),
            &mut self.renderer,
        );
        let _ = ui.update(
            &[Event::Window(window::Event::RedrawRequested(
                iced_test::core::time::Instant::now(),
            ))],
            self.cursor,
            &mut self.renderer,
            &mut clipboard::Null,
            &mut Vec::new(),
        );
        ui.draw(
            &mut self.renderer,
            &self.app.theme,
            &renderer::Style {
                text_color: self.app.theme.palette().text,
            },
            mouse::Cursor::Unavailable,
        );
        self.cache = Some(ui.into_cache());
        let size = Size::new(self.size.width as u32, self.size.height as u32);
        let pixels = self
            .renderer
            .screenshot(size, 1.0, self.app.theme.palette().background);
        if name == "editor" {
            let decoded = video::frame_sample(&self.app.editor.as_ref().unwrap().shared_frame).1;
            let rendered = pixels[(250 * size.width as usize + 770) * 4];
            assert!(
                decoded.abs_diff(rendered) <= 2,
                "Preview changed luminance: decoded {decoded}, displayed {rendered}"
            );
        }
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-test-artifacts");
        std::fs::create_dir_all(&dir).unwrap();
        image::save_buffer(
            dir.join(format!("{name}.png")),
            &pixels,
            size.width,
            size.height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}

#[path = "test_media.rs"]
mod test_media;
use test_media::recording;

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let captures = dir.path().join("captures");
    std::fs::create_dir(&captures).unwrap();
    recording(&captures.join("Arena_2026-10-06_12-00-00.mkv"));
    recording(&captures.join("Raid_2026-10-06_13-00-00.mkv"));
    let mut config = AppConfig::default();
    config.watched_folders = vec![host::to_posix(&captures.to_string_lossy())];
    config.video_extensions = vec!["mkv".into()];
    config.autoplay = false;
    config.output_folder = host::to_posix(&dir.path().join("exports").to_string_lossy());
    config.output.quality_preset = QualityPreset::Original;
    config.output.container = ContainerFormat::Mkv;
    host::save_config(&config).unwrap();
    dir
}

#[test]
fn ui_e2e_recording_workflow() {
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    let _fixture = fixture();
    let mut ui = Ui::new(Size::new(1200.0, 900.0));
    assert_eq!(ui.app.items.len(), 2);
    ui.screenshot("queue");
    ui.fill("queue-search", "Arena");
    assert_eq!(ui.app.sorted_queue().len(), 1);
    ui.click("Clear filters");
    assert_eq!(ui.app.sorted_queue().len(), 2);
    ui.click("Arena_2026-10-06_12-00-00.mkv");
    assert!(ui.app.editor.as_ref().unwrap().has_frame);
    ui.screenshot("editor");
    ui.fill("playhead", "2");
    ui.key(iced::keyboard::key::Named::Enter.into(), None);
    assert!((ui.app.editor.as_ref().unwrap().current_time - 2.0).abs() < 0.11);
    ui.click("+1s");
    assert!((ui.app.editor.as_ref().unwrap().current_time - 3.0).abs() < 0.11);
    ui.click("Play");
    ui.wait_for(|app| app.editor.as_ref().unwrap().current_time > 3.1);
    ui.click("Pause");
    ui.click("Fullscreen");
    assert!(ui.app.fullscreen);
    ui.click("Exit fullscreen");
    assert!(!ui.app.fullscreen);
    ui.click("Audio & crop");
    ui.click("Enable crop (160×90 source)");
    assert!(ui.app.editor.as_ref().unwrap().crop_enabled);
    ui.click("Enable crop (160×90 source)");
    ui.click("Output & tags");
    ui.click("Override quality for this clip");
    assert!(ui
        .app
        .items
        .iter()
        .find(|i| Some(&i.id) == ui.app.selected_id.as_ref())
        .unwrap()
        .output_override
        .is_some());
    ui.choose("override-mode", 1, 4);
    ui.fill("number-CRF", "28");
    let item = ui
        .app
        .items
        .iter()
        .find(|i| Some(&i.id) == ui.app.selected_id.as_ref())
        .unwrap();
    assert_eq!(ui.app.effective_output(item).crf, 28);
    ui.click("Override quality for this clip");
    ui.click("Rename");
    ui.screenshot("rename");
    ui.click("Cancel");
    assert!(ui.app.rename.is_none());
    ui.click("Delete");
    ui.click("Cancel");
    assert_eq!(ui.app.items.len(), 2);
    ui.click("Settings");
    ui.click("Export");
    ui.screenshot("settings-export");
    ui.fill("naming-template", "{name}_edited");
    assert_eq!(host::load_config().naming_template, "{name}_edited");
    ui.click("Preview");
    ui.click("Play clips automatically when selected");
    assert!(host::load_config().autoplay);
    ui.click("Shortcuts");
    ui.screenshot("settings-shortcuts");
    ui.click("Queue (2)");
    ui.click("Export clip");
    let item = ui
        .app
        .items
        .iter()
        .find(|i| Some(&i.id) == ui.app.selected_id.as_ref())
        .unwrap();
    assert_eq!(item.status, QueueStatus::Done, "{:?}", item.error);
    let output = item.export_path.clone().unwrap();
    assert!(std::path::Path::new(&output).is_file());
    assert!((libav::probe(&output).unwrap().0.duration_sec - 10.0).abs() < 0.2);
    for amplitude in test_media::audio_amplitudes(&output) {
        assert!(
            (amplitude - 2000.0 / 32768.0).abs() < 0.005,
            "Unexpected mix amplitude: {amplitude}"
        );
    }
    ui.click("Export clip");
    ui.screenshot("overwrite");
    assert!(ui.app.editor.as_ref().unwrap().overwrite_target.is_some());
    ui.click("Cancel");
    assert!(ui.app.editor.as_ref().unwrap().overwrite_target.is_none());
    ui.click("Export clip");
    ui.confirm("Overwrite");
    assert!(std::path::Path::new(&output).is_file());
    assert!(ui.app.editor.as_ref().unwrap().overwrite_target.is_none());
    assert!(
        ui.max_update < Duration::from_millis(100),
        "UI update blocked for {:?}",
        ui.max_update
    );
}

static UI_SUITE: Mutex<()> = Mutex::new(());

#[test]
fn ui_e2e_queue_filters_tags_rename_restore_delete() {
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = fixture();
    let mut ui = Ui::new(Size::new(1200.0, 900.0));
    for (index, sort) in QueueSort::ALL.iter().enumerate() {
        ui.choose("queue-sort", index, QueueSort::ALL.len());
        assert_eq!(ui.app.queue_sort, *sort);
    }
    ui.choose("filter-game", 1, 3);
    assert_eq!(ui.app.sorted_queue().len(), 1);
    ui.click("Clear filters");
    ui.choose("filter-status", 6, 7);
    assert!(ui.app.sorted_queue().is_empty());
    ui.click("Clear filters");
    ui.choose("filter-highlights", 1, 3);
    assert!(ui.app.sorted_queue().is_empty());
    ui.click("Clear filters");
    ui.click("Arena_2026-10-06_12-00-00.mkv");
    let id = ui.app.selected_id.clone().unwrap();
    ui.click("Output & tags");
    ui.fill("new-tag", "favorite");
    ui.key(iced::keyboard::key::Named::Enter.into(), None);
    assert_eq!(
        ui.app
            .items
            .iter()
            .find(|i| i.id == id)
            .unwrap()
            .tags
            .as_ref()
            .unwrap(),
        &["favorite"]
    );
    ui.click("✕");
    assert!(ui
        .app
        .items
        .iter()
        .find(|i| i.id == id)
        .unwrap()
        .tags
        .as_ref()
        .unwrap()
        .is_empty());
    ui.fill("new-tag", "favorite");
    ui.key(iced::keyboard::key::Named::Enter.into(), None);
    ui.click("Dismiss");
    assert!(ui.app.editor.is_none());
    ui.choose("filter-tag", 1, 3);
    assert_eq!(ui.app.sorted_queue().len(), 1);
    ui.click("Arena_2026-10-06_12-00-00.mkv");
    ui.click("Restore");
    ui.click("Clear filters");
    ui.click("Rename");
    ui.fill("rename", "Renamed clip");
    ui.confirm("Rename");
    let renamed = fixture.path().join("captures/Renamed clip.mkv");
    assert!(renamed.is_file());
    assert!(!fixture
        .path()
        .join("captures/Arena_2026-10-06_12-00-00.mkv")
        .exists());
    ui.click("Delete");
    ui.confirm("Delete");
    assert!(!renamed.exists());
    assert!(!ui.app.items.iter().any(|i| i.id == id));
    assert!(ui.app.editor.is_none());
}

#[test]
fn ui_e2e_settings_and_shortcuts_persist() {
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    let _fixture = fixture();
    let mut ui = Ui::new(Size::new(1200.0, 1000.0));
    ui.click("Settings");
    ui.click("Export");
    for (index, choice) in QmChoice::ALL.iter().enumerate() {
        ui.choose("quality-mode", index, QmChoice::ALL.len());
        assert_eq!(ui.app.config.output.quality_mode, choice.to_core());
    }
    ui.fill("number-Video kbps", "4500");
    assert_eq!(ui.app.config.output.video_bitrate_kbps, 4500);
    ui.choose("quality-mode", 1, 4);
    ui.fill("number-CRF", "23");
    assert_eq!(ui.app.config.output.crf, 23);
    ui.choose("quality-mode", 0, 4);
    for (index, choice) in QpChoice::ALL.iter().enumerate() {
        ui.choose("quality-preset", index, 4);
        assert_eq!(ui.app.config.output.quality_preset, choice.to_core());
    }
    ui.choose("codec", 1, 2);
    ui.choose("container", 0, 2);
    ui.choose("fps", 2, 3);
    ui.choose("resolution", 4, 5);
    ui.choose("audio-bitrate", 2, 3);
    assert_eq!(ui.app.config.output.video_codec, VideoCodecChoice::Libx265);
    assert_eq!(ui.app.config.output.container, ContainerFormat::Mp4);
    assert_eq!(ui.app.config.output.fps, 30);
    assert_eq!(ui.app.config.output.max_height, 720);
    assert_eq!(ui.app.config.output.audio_bitrate_kbps, 256);
    ui.choose("encoder", 2, ENCODER_PRESETS.len());
    assert_eq!(ui.app.config.output.encoder_preset, ENCODER_PRESETS[2]);
    ui.choose("quality-mode", 2, 4);
    ui.fill("number-Max kbps", "7000");
    assert_eq!(ui.app.config.output.video_bitrate_kbps, 7000);
    ui.choose("after-export", 2, 5);
    ui.fill("move-folder", "archive");
    ui.choose("after-export", 3, 5);
    ui.fill("rename-prefix", "finished_");
    ui.fill("rename-suffix", "_original");
    ui.choose("after-export", 4, 5);
    assert_eq!(ui.app.config.after_export.action, AfterExportAction::Prompt);
    ui.click("Preview");
    ui.choose("preview-resolution", 3, 4);
    assert_eq!(ui.app.config.preview_max_height, 720);
    ui.click("Show debug panel in the editor");
    assert!(ui.app.config.debug);
    ui.fill("highlight-model", "another-local-model");
    assert_eq!(host::load_config().highlight_model, "another-local-model");
    ui.click("Shortcuts");
    ui.fill("shortcut-Set in", "B");
    ui.fill("shortcut-Set out", "N");
    let persisted = host::load_config();
    assert_eq!(persisted.keybinds.set_in, "B");
    assert_eq!(persisted.keybinds.set_out, "N");
    assert_eq!(persisted.output, ui.app.config.output);
    ui.click("Queue (2)");
    ui.click("Arena_2026-10-06_12-00-00.mkv");
    ui.click("+1s");
    ui.key(iced::keyboard::Key::Character("b".into()), None);
    ui.wait_for(|app| app.editor.as_ref().unwrap().trim_start > 0.9);
    ui.click("+5s");
    ui.key(iced::keyboard::Key::Character("n".into()), None);
    ui.wait_for(|app| app.editor.as_ref().unwrap().trim_end < 6.2);
    ui.click("Settings");
    let trim = ui.app.editor.as_ref().unwrap().trim_end;
    ui.key(iced::keyboard::Key::Character("n".into()), None);
    assert_eq!(ui.app.editor.as_ref().unwrap().trim_end, trim);
}

#[test]
fn ui_e2e_minimum_window_and_background_responsiveness() {
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    let _fixture = fixture();
    let mut ui = Ui::new(Size::new(960.0, 660.0));
    let path = ui
        .app
        .items
        .iter()
        .find(|i| i.file_name.starts_with("Arena"))
        .unwrap()
        .path
        .clone();
    ui.settle();
    let lease = background::write_media(&path);
    let point = ui.bounds("Arena_2026-10-06_12-00-00.mkv").center();
    ui.click_at(point);
    assert!(ui.pending > 0);
    let point = ui.bounds("Settings").center();
    let before = Instant::now();
    ui.click_at(point);
    assert!(matches!(ui.app.view, View::Settings));
    assert!(before.elapsed() < Duration::from_millis(100));
    drop(lease);
    ui.settle();
    ui.click("Queue (2)");
    ui.screenshot("editor-minimum");
    let export = ui.bounds("Export clip");
    assert!(export.y + export.height <= 660.0);
    ui.scroll(Point::new(800.0, 400.0), -15.0);
    assert_eq!(ui.bounds("Export clip"), export);
    ui.click("Settings");
    ui.screenshot("settings-minimum");
    ui.click("Shortcuts");
    ui.scroll(Point::new(700.0, 400.0), -15.0);
    assert!(ui.find("Open config file").is_some());
    assert!(ui.max_update < Duration::from_millis(100));
}

#[test]
fn ui_e2e_trim_audio_validation_and_edit_persistence() {
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    let _fixture = fixture();
    let mut ui = Ui::new(Size::new(1200.0, 1000.0));
    ui.click("Arena_2026-10-06_12-00-00.mkv");
    ui.fill("trim-In", "2");
    ui.key(iced::keyboard::key::Named::Enter.into(), None);
    ui.fill("trim-Out", "7");
    ui.key(iced::keyboard::key::Named::Enter.into(), None);
    assert_eq!(ui.app.editor.as_ref().unwrap().trim_start, 2.0);
    assert_eq!(ui.app.editor.as_ref().unwrap().trim_end, 7.0);
    ui.fill("trim-In", "invalid");
    ui.key(iced::keyboard::key::Named::Enter.into(), None);
    assert_eq!(ui.app.editor.as_ref().unwrap().trim_start, 2.0);
    ui.slider("timeline", 0.4);
    assert!((ui.app.editor.as_ref().unwrap().current_time - 4.0).abs() < 0.2);
    ui.click("Audio & crop");
    let label = ui.app.editor.as_ref().unwrap().audio[1].label.clone();
    ui.click(&label);
    assert!(!ui.app.editor.as_ref().unwrap().audio[1].enabled);
    ui.slider("volume-0", 0.25);
    assert!((ui.app.editor.as_ref().unwrap().audio[0].volume - 0.5).abs() <= 0.05);
    ui.click("Enable crop (160×90 source)");
    ui.fill("number-W", "999");
    ui.click("Export clip");
    assert!(!ui.app.editor.as_ref().unwrap().exporting);
    assert!(qlipq_core::edit_spec::validate_edit_spec(
        &editor_spec(ui.app.editor.as_ref().unwrap()),
        ui.app.editor.as_ref().unwrap().media.as_ref().unwrap()
    )
    .is_some());
    ui.fill("number-W", "144");
    ui.fill("number-H", "80");
    ui.screenshot("audio-crop");
    ui.click("Enable crop (160×90 source)");
    let path = ui
        .app
        .items
        .iter()
        .find(|i| Some(&i.id) == ui.app.selected_id.as_ref())
        .unwrap()
        .path
        .clone();
    ui.click("Raid_2026-10-06_13-00-00.mkv");
    ui.click("Arena_2026-10-06_12-00-00.mkv");
    let ed = ui.app.editor.as_ref().unwrap();
    assert_eq!((ed.trim_start, ed.trim_end), (2.0, 7.0));
    assert!(!ed.audio[1].enabled);
    assert_eq!(host::load_edit_store()[&path].edit, Some(editor_spec(ed)));
    ui.click("Export clip");
    let output = ui
        .app
        .items
        .iter()
        .find(|i| i.path == path)
        .unwrap()
        .export_path
        .as_ref()
        .unwrap();
    let media = libav::probe(output).unwrap().0;
    assert_eq!(media.audio_streams.len(), 1);
    assert!((media.duration_sec - 5.0).abs() < 0.2);
    let amplitudes = test_media::audio_amplitudes(output);
    assert!(
        (amplitudes[0] - 1000.0 / 32768.0).abs() < 0.005,
        "Gain was not applied: {amplitudes:?}"
    );
    assert!(
        amplitudes[1] < 0.001,
        "Muted track leaked into the export: {amplitudes:?}"
    );
    ui.click("Export clip");
    ui.confirm("Append timestamp");
    assert!(ui.app.items.iter().find(|i| i.path == path).unwrap().status == QueueStatus::Done);
}

#[test]
fn ui_e2e_watched_folders_and_os_actions() {
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = fixture();
    let mut ui = Ui::new(Size::new(1200.0, 900.0));
    ui.click("GitHub");
    assert!(host::test_os::OPENED
        .lock()
        .unwrap()
        .contains(&"https://github.com/qcksys/qlipq".into()));
    ui.click("Settings");
    host::test_os::FOLDERS.lock().unwrap().push_back(None);
    ui.click("Add folder…");
    assert_eq!(ui.app.config.watched_folders.len(), 1);
    let extra = fixture.path().join("extra");
    std::fs::create_dir(&extra).unwrap();
    recording(&extra.join("New recording.mkv"));
    host::test_os::FOLDERS
        .lock()
        .unwrap()
        .push_back(Some(host::to_posix(&extra.to_string_lossy())));
    ui.click("Add folder…");
    assert_eq!(ui.app.config.watched_folders.len(), 2);
    assert_eq!(ui.app.items.len(), 3);
    ui.click("Queue (3)");
    recording(&extra.join("Watched recording.mkv"));
    ui.wait_for(|app| app.items.len() == 4);
    ui.settle();
    ui.click("Rescan all folders");
    assert_eq!(ui.app.items.len(), 4);
    ui.click("Settings");
    ui.click("Reprocess");
    assert!(matches!(ui.app.view, View::Queue));
    ui.click("Settings");
    ui.click("Remove");
    assert_eq!(ui.app.config.watched_folders.len(), 1);
    ui.click("Export");
    let out = host::to_posix(&fixture.path().join("chosen-output").to_string_lossy());
    host::test_os::FOLDERS
        .lock()
        .unwrap()
        .push_back(Some(out.clone()));
    ui.click("Browse…");
    assert_eq!(host::load_config().output_folder, out);
    ui.click("Open config file");
    assert!(host::test_os::OPENED
        .lock()
        .unwrap()
        .iter()
        .any(|path| path.ends_with("config.json")));
}

#[test]
fn ui_e2e_highlight_suggestion_apply_dismiss_and_failure() {
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    let _fixture = fixture();
    let mut ui = Ui::new(Size::new(1200.0, 1000.0));
    ui.click("Arena_2026-10-06_12-00-00.mkv");
    for (content, apply) in [
        (
            r#"{"event":{"startSec":4,"endSec":5,"score":95,"reason":"Test event"}}"#,
            false,
        ),
        (
            r#"{"event":{"startSec":4,"endSec":5,"score":95,"reason":"Test event"}}"#,
            true,
        ),
        ("invalid JSON", false),
    ] {
        let (url, server) = highlight::tests::mock_ollama(2, move |index, _, body| {
            if index == 0 {
                return (200, serde_json::json!({"capabilities":["vision"]}));
            }
            assert!(!body["messages"][0]["images"].as_array().unwrap().is_empty());
            (
                200,
                serde_json::json!({"done":true,"message":{"content":content}}),
            )
        });
        *highlight::TEST_ENDPOINT.lock().unwrap() = Some(url);
        let trim = editor_spec(ui.app.editor.as_ref().unwrap()).trim.unwrap();
        ui.click("Suggest highlight");
        server.join().unwrap();
        *highlight::TEST_ENDPOINT.lock().unwrap() = None;
        assert_eq!(
            editor_spec(ui.app.editor.as_ref().unwrap()).trim.unwrap(),
            trim
        );
        if content == "invalid JSON" {
            assert!(ui.app.editor.as_ref().unwrap().highlight_status.is_some());
        } else if apply {
            ui.click("Apply trim");
            assert_eq!(ui.app.editor.as_ref().unwrap().trim_start, 1.0);
            assert_eq!(ui.app.editor.as_ref().unwrap().trim_end, 7.0);
        } else {
            ui.screenshot("highlight-suggestion");
            ui.confirm("Dismiss");
            assert!(ui
                .app
                .editor
                .as_ref()
                .unwrap()
                .highlight_suggestion
                .is_none());
        }
    }
}

#[test]
fn ui_e2e_export_cancellation_keeps_ui_responsive() {
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = fixture();
    let mut ui = Ui::new(Size::new(1200.0, 900.0));
    ui.click("Arena_2026-10-06_12-00-00.mkv");
    let selected = ui.app.selected_id.clone();
    let (entered, receive) = std::sync::mpsc::channel();
    let (resume, worker) = std::sync::mpsc::channel();
    *export::TEST_PAUSE.lock().unwrap() = Some((entered, worker));
    let point = ui.bounds("Export clip").center();
    ui.click_at(point);
    ui.wait_for(|app| app.editor.as_ref().unwrap().exporting);
    receive.recv_timeout(Duration::from_secs(5)).unwrap();
    let point = ui.bounds("Raid_2026-10-06_13-00-00.mkv").center();
    ui.click_at(point);
    assert_eq!(ui.app.selected_id, selected);
    for label in ["Dismiss", "Rename", "Delete"] {
        let point = ui.bounds(label).center();
        ui.click_at(point);
        assert_eq!(ui.app.selected_id, selected);
        assert!(ui.app.rename.is_none() && ui.app.delete_confirm.is_none());
    }
    let point = ui.bounds("Cancel export").center();
    let before = Instant::now();
    ui.click_at(point);
    assert!(before.elapsed() < Duration::from_millis(100));
    assert!(ui
        .app
        .editor
        .as_ref()
        .unwrap()
        .export_cancel
        .as_ref()
        .unwrap()
        .load(Ordering::Relaxed));
    resume.send(()).unwrap();
    ui.settle();
    let item = ui
        .app
        .items
        .iter()
        .find(|i| Some(&i.id) == selected.as_ref())
        .unwrap();
    assert_eq!(item.status, QueueStatus::Pending);
    assert!(item.error.is_none());
    assert!(fixture
        .path()
        .join("captures/Arena_2026-10-06_12-00-00.mkv")
        .is_file());
    assert_eq!(
        std::fs::read_dir(fixture.path().join("exports"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn ui_e2e_after_export_actions_update_files_and_queue() {
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    for action in [
        AfterExportAction::Delete,
        AfterExportAction::Move,
        AfterExportAction::Rename,
        AfterExportAction::Prompt,
    ] {
        let fixture = fixture();
        let mut config = host::load_config();
        config.after_export.action = action;
        config.after_export.rename_prefix = "done_".into();
        config.after_export.move_folder =
            host::to_posix(&fixture.path().join("archive").to_string_lossy());
        host::save_config(&config).unwrap();
        let mut ui = Ui::new(Size::new(1200.0, 900.0));
        ui.click("Arena_2026-10-06_12-00-00.mkv");
        ui.click("Export clip");
        let source = fixture
            .path()
            .join("captures/Arena_2026-10-06_12-00-00.mkv");
        match action {
            AfterExportAction::Delete => {
                assert!(!source.exists());
                assert_eq!(ui.app.items.len(), 1);
            }
            AfterExportAction::Move => assert!(fixture
                .path()
                .join("archive/Arena_2026-10-06_12-00-00.mkv")
                .is_file()),
            AfterExportAction::Rename => assert!(fixture
                .path()
                .join("captures/done_Arena_2026-10-06_12-00-00.mkv")
                .is_file()),
            AfterExportAction::Prompt => {
                assert!(ui.app.editor.as_ref().unwrap().after_prompt);
                ui.confirm("Keep");
                assert!(source.is_file());
                ui.click("Show file");
                assert!(host::test_os::OPENED
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|path| path.contains("exports")));
                ui.click("Settings");
                ui.click("Export");
                ui.choose("after-export", 2, 5);
                ui.fill("move-folder", "");
                ui.choose("after-export", 4, 5);
                ui.click("Queue (1)");
                ui.click("Export clip");
                ui.confirm("Overwrite");
                host::test_os::FOLDERS.lock().unwrap().push_back(None);
                ui.confirm("Move…");
                let before = video::frame_sample(&ui.app.editor.as_ref().unwrap().shared_frame).0;
                ui.click("+1s");
                assert!(
                    video::frame_sample(&ui.app.editor.as_ref().unwrap().shared_frame).0 > before
                );
                assert!(source.is_file());
                ui.click("Export clip");
                ui.confirm("Overwrite");
                host::test_os::FOLDERS
                    .lock()
                    .unwrap()
                    .push_back(Some(host::to_posix(
                        &fixture.path().join("picked-archive").to_string_lossy(),
                    )));
                ui.confirm("Move…");
                assert!(fixture
                    .path()
                    .join("picked-archive/Arena_2026-10-06_12-00-00.mkv")
                    .is_file());
            }
            _ => unreachable!(),
        }
        assert!(ui
            .app
            .items
            .iter()
            .all(|i| std::path::Path::new(&i.path).is_file()));
    }
}

#[test]
fn ui_e2e_errors_remain_actionable() {
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = fixture();
    let mut ui = Ui::new(Size::new(1200.0, 900.0));
    ui.click("Arena_2026-10-06_12-00-00.mkv");
    ui.click("Rename");
    ui.fill("rename", "Raid_2026-10-06_13-00-00");
    ui.confirm("Rename");
    assert!(ui
        .app
        .rename
        .as_ref()
        .unwrap()
        .error
        .as_ref()
        .unwrap()
        .contains("already exists"));
    ui.confirm("Cancel");
    let before = video::frame_sample(&ui.app.editor.as_ref().unwrap().shared_frame).0;
    ui.click("+1s");
    assert!(video::frame_sample(&ui.app.editor.as_ref().unwrap().shared_frame).0 > before);
    ui.click("Settings");
    ui.click("Export");
    let blocker = fixture.path().join("not-a-directory");
    std::fs::write(&blocker, b"fixture").unwrap();
    ui.fill("output-folder", &blocker.to_string_lossy());
    ui.click("Queue (2)");
    ui.click("Export clip");
    assert_eq!(
        ui.app
            .items
            .iter()
            .find(|i| Some(&i.id) == ui.app.selected_id.as_ref())
            .unwrap()
            .status,
        QueueStatus::Error
    );
    ui.screenshot("export-error");
    let source = ui
        .app
        .items
        .iter()
        .find(|i| Some(&i.id) == ui.app.selected_id.as_ref())
        .unwrap()
        .path
        .clone();
    ui.click("Raid_2026-10-06_13-00-00.mkv");
    host::delete_file(&source).unwrap();
    ui.click("Arena_2026-10-06_12-00-00.mkv");
    assert!(ui.app.editor.as_ref().unwrap().load_error.is_some());
    ui.click("Delete");
    ui.confirm("Delete");
    assert!(ui.app.delete_error.is_some());
    ui.confirm("OK");
    assert!(ui.app.delete_error.is_none());
}

#[test]
#[ignore = "requires an available NVENC, AMF or QSV hardware encoder"]
fn ui_e2e_hardware_crop_export() {
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    let fixture = fixture();
    test_media::recording_sized(
        &fixture
            .path()
            .join("captures/Arena_2026-10-06_12-00-00.mkv"),
        640,
        360,
    );
    let mut ui = Ui::new(Size::new(1200.0, 1000.0));
    ui.click("Arena_2026-10-06_12-00-00.mkv");
    ui.click("Audio & crop");
    ui.click("Enable crop (640×360 source)");
    ui.fill("number-W", "480");
    ui.fill("number-H", "270");
    ui.click("Export clip");
    let item = ui
        .app
        .items
        .iter()
        .find(|i| Some(&i.id) == ui.app.selected_id.as_ref())
        .unwrap();
    assert_eq!(item.status, QueueStatus::Done, "{:?}", item.error);
    let media = libav::probe(item.export_path.as_ref().unwrap()).unwrap().0;
    assert_eq!(
        (media.width, media.height, media.audio_streams.len()),
        (480, 270, 1)
    );
}

#[test]
fn ui_e2e_hover_preview_keyboard_modals_and_debug_copy() {
    use iced::keyboard::{key::Named, Key, Modifiers};
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    let _fixture = fixture();
    let mut ui = Ui::new(Size::new(1200.0, 1000.0));
    let point = ui.bounds("Arena_2026-10-06_12-00-00.mkv").center();
    ui.cursor = mouse::Cursor::Available(point);
    ui.event(vec![Event::Mouse(mouse::Event::CursorMoved {
        position: point,
    })]);
    ui.wait_for(|app| app.queue_preview.as_ref().is_some_and(|p| p.has_frame()));
    assert!(ui.app.selected_id.is_none());
    let point = ui.bounds("Settings").center();
    ui.cursor = mouse::Cursor::Available(point);
    ui.event(vec![Event::Mouse(mouse::Event::CursorMoved {
        position: point,
    })]);
    ui.settle();
    assert!(ui.app.queue_preview.is_none());
    ui.click("Arena_2026-10-06_12-00-00.mkv");
    ui.fill("highlight-query", "i");
    assert_eq!(ui.app.editor.as_ref().unwrap().trim_start, 0.0);
    ui.click("Fullscreen");
    ui.key(Named::Escape.into(), None);
    ui.wait_for(|app| !app.fullscreen);
    ui.click("Rename");
    ui.click("Use template");
    assert_ne!(
        ui.app.rename.as_ref().unwrap().value,
        "Arena_2026-10-06_12-00-00"
    );
    ui.key(Named::Escape.into(), None);
    ui.wait_for(|app| app.rename.is_none());
    ui.click("Settings");
    ui.click("Preview");
    ui.click("Show debug panel in the editor");
    ui.slider("hdr-brightness", 0.25);
    assert!((ui.app.config.hdr_preview_gamma - 1.5).abs() < 0.05);
    ui.choose("preview-resolution", 3, 4);
    ui.click("Reset");
    assert_eq!(
        ui.app.config.hdr_preview_gamma,
        AppConfig::default().hdr_preview_gamma
    );
    ui.click("Queue (2)");
    ui.scroll(Point::new(900.0, 600.0), -20.0);
    ui.click("Copy");
    assert!(ui.clipboard.as_ref().unwrap().contains("Decoder:"));
    ui.key(Key::Named(Named::Delete), None);
    ui.wait_for(|app| app.delete_confirm.is_some());
    ui.key(Named::Enter.into(), None);
    ui.wait_for(|app| app.items.len() == 1);
    ui.settle();
    ui.click("Raid_2026-10-06_13-00-00.mkv");
    ui.key(Key::Named(Named::Delete), Some(Modifiers::SHIFT));
    ui.wait_for(|app| app.items.is_empty());
    ui.settle();
}

#[test]
fn ui_e2e_cancel_highlight_and_switch_clips_while_model_is_busy() {
    let _serial = UI_SUITE.lock().unwrap_or_else(|e| e.into_inner());
    let _fixture = fixture();
    let mut ui = Ui::new(Size::new(1200.0, 1000.0));
    for switch in [false, true] {
        ui.click("Arena_2026-10-06_12-00-00.mkv");
        let (entered, waiting) = std::sync::mpsc::channel();
        let (resume, blocked) = std::sync::mpsc::channel();
        let (url, server) = highlight::tests::mock_ollama(1, move |_, _, _| {
            entered.send(()).unwrap();
            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
            (200, serde_json::json!({"capabilities":["vision"]}))
        });
        *highlight::TEST_ENDPOINT.lock().unwrap() = Some(url);
        let point = ui.bounds("Suggest highlight").center();
        ui.click_at(point);
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        let cancel = ui
            .app
            .editor
            .as_ref()
            .unwrap()
            .highlight_job
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        let point = ui
            .bounds(if switch {
                "Raid_2026-10-06_13-00-00.mkv"
            } else {
                "Cancel"
            })
            .center();
        ui.click_at(point);
        assert!(cancel.load(Ordering::Relaxed));
        ui.wait_for(|app| app.editor.as_ref().unwrap().highlight_job.is_none());
        resume.send(()).unwrap();
        server.join().unwrap();
        ui.settle();
        *highlight::TEST_ENDPOINT.lock().unwrap() = None;
        let ed = ui.app.editor.as_ref().unwrap();
        assert!(ed.highlight_suggestion.is_none());
        assert_eq!((ed.trim_start, ed.trim_end), (0.0, 10.0));
    }
}
