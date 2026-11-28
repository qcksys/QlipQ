//! The QlipQ actions: keys and dials that drive the QlipQ desktop app's editor through its local
//! control pipe (`qlipq-desktop/src/control.rs`). This module holds the pipe client, the protocol,
//! the actions' settings, and their key images and touch-strip payloads.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

/// Also defined in `qlipq-desktop/src/control.rs`.
#[cfg_attr(not(windows), allow(dead_code))]
pub const PIPE_NAME: &str = r"\\.\pipe\qlipq-control";
#[cfg_attr(not(windows), allow(dead_code))]
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const UUID_PREFIX: &str = "com.qcksys.qlipq.meters.qlipq-";
const MAX_LABEL_CHARS: usize = 18;
/// QlipQ's brand accent, and a lighter tint of it that reads on the dark key.
const ACCENT: &str = "#6d5cff";
const ACCENT_LIGHT: &str = "#8676ff";

pub enum QlipqEvent {
    /// Connected; commands are sent as JSON lines through the sender.
    Connected(mpsc::UnboundedSender<String>),
    Disconnected,
    Line(Value),
}

/// Keeps a connection to QlipQ open for the life of the plugin, retrying while QlipQ is closed.
#[cfg(windows)]
pub async fn run(events: mpsc::UnboundedSender<QlipqEvent>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::ClientOptions;

    loop {
        if let Ok(pipe) = ClientOptions::new().open(PIPE_NAME) {
            let (reader, mut writer) = tokio::io::split(pipe);
            let (commands, mut outgoing) = mpsc::unbounded_channel::<String>();
            if events.send(QlipqEvent::Connected(commands)).is_err() {
                return;
            }
            let mut lines = BufReader::new(reader).lines();
            loop {
                tokio::select! {
                    line = lines.next_line() => match line {
                        Ok(Some(line)) => {
                            if let Ok(value) = serde_json::from_str(&line) {
                                if events.send(QlipqEvent::Line(value)).is_err() {
                                    return;
                                }
                            }
                        }
                        _ => break,
                    },
                    command = outgoing.recv() => match command {
                        Some(command) => {
                            if writer.write_all(format!("{command}\n").as_bytes()).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    },
                }
            }
            if events.send(QlipqEvent::Disconnected).is_err() {
                return;
            }
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

/// QlipQ's control pipe is Windows-only.
#[cfg(not(windows))]
pub async fn run(_events: mpsc::UnboundedSender<QlipqEvent>) {}

pub fn command(cmd: &str) -> String {
    json!({ "cmd": cmd }).to_string()
}

pub fn command_with(cmd: &str, data: Value) -> String {
    let mut message = data;
    message["cmd"] = json!(cmd);
    message.to_string()
}

/// The clip open in QlipQ's editor.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    pub name: String,
    pub duration: Option<f64>,
    pub position: f64,
    pub trim_in: f64,
    pub trim_out: f64,
    pub playing: bool,
}

/// QlipQ's `state` line.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct State {
    pub clip: Option<Clip>,
    /// Export progress from 0 to 1 while an export runs.
    pub exporting: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub duration: Option<f64>,
}

/// QlipQ's `queue` line: the visible queue in its current order.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Queue {
    pub items: Vec<Entry>,
    pub selected: Option<String>,
}

pub enum Line {
    State(State),
    Queue(Queue),
}

pub fn parse_line(value: &Value) -> Option<Line> {
    match value["type"].as_str()? {
        "state" => serde_json::from_value(value.clone()).ok().map(Line::State),
        "queue" => serde_json::from_value(value.clone()).ok().map(Line::Queue),
        _ => None,
    }
}

/// `m:ss.s`, or `h:mm:ss` from an hour.
pub fn timecode(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    if seconds >= 3600.0 {
        let whole = seconds as u64;
        return format!("{}:{:02}:{:02}", whole / 3600, whole / 60 % 60, whole % 60);
    }
    let minutes = (seconds / 60.0).floor();
    format!("{}:{:04.1}", minutes as u64, seconds - minutes * 60.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyKind {
    Play,
    SetIn,
    SetOut,
    Export,
    Rescan,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DialKind {
    Scrub,
    Picker,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Key(KeyKind),
    Dial(DialKind),
}

impl Action {
    pub fn from_uuid(uuid: &str) -> Option<Self> {
        Some(match uuid.strip_prefix(UUID_PREFIX)? {
            "play" => Self::Key(KeyKind::Play),
            "in" => Self::Key(KeyKind::SetIn),
            "out" => Self::Key(KeyKind::SetOut),
            "export" => Self::Key(KeyKind::Export),
            "rescan" => Self::Key(KeyKind::Rescan),
            "scrub" => Self::Dial(DialKind::Scrub),
            "picker" => Self::Dial(DialKind::Picker),
            _ => return None,
        })
    }
}

/// A QlipQ key's settings, as its property inspector (`pi/qlipq-key.html`) saves them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KeySettings {
    /// Text on the key; empty uses the action's own name.
    pub label: String,
    /// Export key only: what a press does while an export runs, "cancel" or "nothing".
    pub while_exporting: String,
}

impl KeySettings {
    pub fn from_payload(payload: &Value) -> Self {
        let mut settings: Self =
            serde_json::from_value(payload["settings"].clone()).unwrap_or_default();
        settings.label = settings
            .label
            .trim()
            .chars()
            .take(MAX_LABEL_CHARS)
            .collect();
        if settings.while_exporting != "nothing" {
            settings.while_exporting = "cancel".into();
        }
        settings
    }

    pub fn cancels_export(&self) -> bool {
        self.while_exporting == "cancel"
    }
}

/// A QlipQ dial's settings, as its property inspector (`pi/qlipq-scrub.html`,
/// `pi/qlipq-picker.html`) saves them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DialSettings {
    /// Scrub: "frames" or "seconds" per dial click.
    pub unit: String,
    /// Scrub: how many frames or seconds one click moves.
    pub amount: f64,
    /// Scrub: what a press or tap does, "play" (play or pause) or "none".
    pub press: String,
    /// Clip picker: turning past the last clip goes back to the first.
    pub wrap: bool,
}

impl Default for DialSettings {
    fn default() -> Self {
        Self {
            unit: "frames".into(),
            amount: 1.0,
            press: "play".into(),
            wrap: false,
        }
    }
}

impl DialSettings {
    pub fn from_payload(payload: &Value) -> Self {
        let mut settings: Self =
            serde_json::from_value(payload["settings"].clone()).unwrap_or_default();
        let defaults = Self::default();
        if settings.unit != "seconds" {
            settings.unit = defaults.unit;
        }
        let max = if settings.unit == "seconds" {
            60.0
        } else {
            120.0
        };
        if !(settings.amount > 0.0 && settings.amount <= max) {
            settings.amount = defaults.amount;
        }
        if settings.unit == "frames" {
            settings.amount = settings.amount.round().max(1.0);
        }
        if settings.press != "none" {
            settings.press = defaults.press;
        }
        settings
    }

    /// The command for turning the scrub dial `ticks` clicks.
    pub fn scrub_command(&self, ticks: i32) -> String {
        if self.unit == "seconds" {
            command_with("skip", json!({ "seconds": f64::from(ticks) * self.amount }))
        } else {
            command_with(
                "stepFrames",
                json!({ "frames": ticks * self.amount as i32 }),
            )
        }
    }

    pub fn plays_on_press(&self) -> bool {
        self.press == "play"
    }
}

/// Moves the clip picker's cursor `ticks` clips through `count` clips from `index`.
pub fn move_cursor(index: usize, ticks: i32, count: usize, wrap: bool) -> usize {
    if count == 0 {
        return 0;
    }
    let next = index as i64 + i64::from(ticks);
    if wrap {
        next.rem_euclid(count as i64) as usize
    } else {
        next.clamp(0, count as i64 - 1) as usize
    }
}

/// What a QlipQ key looks like.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyLook {
    pub label: String,
    pub status: String,
    /// Drawn in the accent gradient (playing, exporting).
    pub on: bool,
    /// Greyed out (QlipQ closed, or nothing to act on).
    pub idle: bool,
    pub glyph: Glyph,
    /// Export progress from 0 to 1, drawn along the bottom edge.
    pub progress: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Glyph {
    Play,
    Pause,
    In,
    Out,
    Export,
    Rescan,
}

pub fn key_look(
    kind: KeyKind,
    settings: &KeySettings,
    connected: bool,
    state: &State,
    queue: &Queue,
) -> KeyLook {
    let clip = state.clip.as_ref();
    let playing = clip.is_some_and(|clip| clip.playing);
    let (name, glyph) = match kind {
        KeyKind::Play if playing => ("Pause", Glyph::Pause),
        KeyKind::Play => ("Play", Glyph::Play),
        KeyKind::SetIn => ("Set In", Glyph::In),
        KeyKind::SetOut => ("Set Out", Glyph::Out),
        KeyKind::Export => ("Export", Glyph::Export),
        KeyKind::Rescan => ("Rescan", Glyph::Rescan),
    };
    let label = if settings.label.is_empty() {
        name.to_string()
    } else {
        settings.label.clone()
    };
    let look = |status: String, on: bool, idle: bool, progress: Option<f32>| KeyLook {
        label: label.clone(),
        status,
        on,
        idle,
        glyph,
        progress,
    };
    if !connected {
        return look("QlipQ off".into(), false, true, None);
    }
    match kind {
        KeyKind::Rescan => {
            let count = queue.items.len();
            let clips = if count == 1 { "clip" } else { "clips" };
            look(format!("{count} {clips}"), false, false, None)
        }
        KeyKind::Export => match state.exporting {
            Some(progress) => look(
                format!("{:.0}%", progress * 100.0),
                true,
                false,
                Some(progress.clamp(0.0, 1.0)),
            ),
            None if clip.is_none() => look("No clip".into(), false, true, None),
            None => look("Ready".into(), false, false, None),
        },
        _ => match clip {
            None => look("No clip".into(), false, true, None),
            Some(clip) => {
                let time = match kind {
                    KeyKind::SetIn => clip.trim_in,
                    KeyKind::SetOut => clip.trim_out,
                    _ => clip.position,
                };
                look(
                    timecode(time),
                    kind == KeyKind::Play && playing,
                    false,
                    None,
                )
            }
        },
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The key image, in the deck's key style with QlipQ's accent, as a data URL for `setImage`.
pub fn key_image(look: &KeyLook) -> String {
    let glyph = match look.glyph {
        Glyph::Play => r#"<path d="M30 16 L82 50 L30 84 Z"/>"#,
        Glyph::Pause => r#"<path d="M34 18 V82 M66 18 V82"/>"#,
        Glyph::In => {
            r#"<path d="M42 12 H22 V88 H42"/><path d="M52 50 H84"/><path d="M70 36 L84 50 L70 64"/>"#
        }
        Glyph::Out => {
            r#"<path d="M58 12 H78 V88 H58"/><path d="M16 50 H48"/><path d="M34 36 L48 50 L34 64"/>"#
        }
        Glyph::Export => {
            r#"<path d="M18 58 V86 H82 V58"/><path d="M50 12 V64"/><path d="M30 32 L50 12 L70 32"/>"#
        }
        Glyph::Rescan => r#"<path d="M82 44 A32 32 0 1 0 74 74"/><path d="M84 16 V44 H56"/>"#,
    };
    let gradient = if look.on {
        format!(
            r#"<stop offset="0" stop-color="{ACCENT}" stop-opacity="0.55"/><stop offset="1" stop-color="{ACCENT}" stop-opacity="0.22"/>"#
        )
    } else {
        r##"<stop offset="0" stop-color="#202835"/><stop offset="1" stop-color="#0f141b"/>"##.into()
    };
    let (bar, glyph_color, label_color) = match (look.idle, look.on) {
        (true, _) => ("#3d4858", "#5c6878", "#9fabbb"),
        (false, true) => (ACCENT, "#ffffff", "#f2f5f8"),
        (false, false) => (ACCENT, ACCENT_LIGHT, "#f2f5f8"),
    };
    let status_color = if look.on { "#ffffff" } else { "#9fabbb" };
    let size = match look.label.chars().count() {
        0..=10 => 19,
        11..=13 => 17,
        _ => 15,
    };
    let progress = look.progress.map_or(String::new(), |progress| {
        format!(
            r##"<rect y="138" width="144" height="6" fill="#1d2430"/><rect y="138" width="{:.1}" height="6" fill="#ffffff"/>"##,
            144.0 * progress
        )
    });
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144"><defs><linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">{gradient}</linearGradient></defs><rect width="144" height="144" fill="#0f141b"/><rect width="144" height="144" fill="url(#bg)"/><rect width="144" height="6" fill="{bar}"/><g transform="translate(39 14) scale(0.66)"><g fill="none" stroke="{glyph_color}" stroke-width="7" stroke-linecap="round" stroke-linejoin="round">{glyph}</g></g><text x="72" y="103" text-anchor="middle" font-family="Segoe UI" font-weight="600" font-size="{size}" fill="{label_color}">{label}</text><text x="72" y="128" text-anchor="middle" font-family="Segoe UI" font-size="15" fill="{status_color}">{status}</text>{progress}</svg>"##,
        label = escape(&look.label),
        status = escape(&look.status),
    );
    format!("data:image/svg+xml;base64,{}", BASE64.encode(svg))
}

/// The scrub dial's `setFeedback` payload (`layouts/qlipq-scrub.json`).
pub fn scrub_feedback(connected: bool, state: &State) -> Value {
    let idle = |title: &str| {
        json!({
            "title": title, "time": { "value": "", "color": "#9fabbb" },
            "position": 0, "trim": "",
        })
    };
    if !connected {
        return idle("QlipQ off");
    }
    let Some(clip) = &state.clip else {
        return idle("No clip open");
    };
    let duration = clip.duration.unwrap_or(0.0);
    let percent = if duration > 0.0 {
        (clip.position / duration * 100.0).clamp(0.0, 100.0).round() as i32
    } else {
        0
    };
    json!({
        "title": clip.name,
        "time": {
            "value": format!("{} / {}", timecode(clip.position), timecode(duration)),
            "color": if clip.playing { "#ffffff" } else { "#c5ccd6" },
        },
        "position": percent,
        "trim": format!("In {}  ·  Out {}", timecode(clip.trim_in), timecode(clip.trim_out)),
    })
}

/// The clip picker's `setFeedback` payload (`layouts/qlipq-picker.json`).
pub fn picker_feedback(connected: bool, queue: &Queue, cursor: Option<usize>) -> Value {
    let idle = |title: &str, name: &str| json!({ "title": title, "name": name, "detail": "" });
    if !connected {
        return idle("Clips", "QlipQ off");
    }
    let Some((index, entry)) = cursor.and_then(|index| Some((index, queue.items.get(index)?)))
    else {
        return idle("Clips", "No clips");
    };
    let open = queue.selected.as_deref() == Some(entry.id.as_str());
    let length = entry.duration.map(timecode).unwrap_or_default();
    let detail = match (open, length.is_empty()) {
        (true, true) => "Open".to_string(),
        (true, false) => format!("{length}  ·  Open"),
        (false, _) => length,
    };
    json!({
        "title": format!("Clip {} of {}", index + 1, queue.items.len()),
        "name": { "value": entry.name, "color": if open { ACCENT_LIGHT } else { "#ffffff" } },
        "detail": detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip() -> Clip {
        Clip {
            id: "a".into(),
            name: "2026-11-28 18-17-26.mp4".into(),
            duration: Some(65.0),
            position: 12.44,
            trim_in: 3.0,
            trim_out: 45.0,
            playing: false,
        }
    }

    fn state(clip: Option<Clip>, exporting: Option<f32>) -> State {
        State { clip, exporting }
    }

    fn queue(names: &[&str], selected: Option<&str>) -> Queue {
        Queue {
            items: names
                .iter()
                .map(|name| Entry {
                    id: name.to_string(),
                    name: name.to_string(),
                    duration: Some(30.0),
                })
                .collect(),
            selected: selected.map(str::to_string),
        }
    }

    #[test]
    fn actions_come_from_their_uuids() {
        assert_eq!(
            Action::from_uuid("com.qcksys.qlipq.meters.qlipq-play"),
            Some(Action::Key(KeyKind::Play))
        );
        assert_eq!(
            Action::from_uuid("com.qcksys.qlipq.meters.qlipq-picker"),
            Some(Action::Dial(DialKind::Picker))
        );
        assert_eq!(Action::from_uuid("com.qcksys.qlipq.meters.replay"), None);
        assert_eq!(
            Action::from_uuid("com.qcksys.qlipq.meters.qlipq-nope"),
            None
        );
    }

    #[test]
    fn parses_qlipq_status_lines() {
        let Some(Line::State(parsed)) = parse_line(&json!({
            "type": "state",
            "clip": { "id": "a", "name": "a.mp4", "duration": 10.0, "position": 2.0,
                      "trimIn": 1.0, "trimOut": 9.0, "playing": true },
            "exporting": 0.5,
        })) else {
            panic!("a state line");
        };
        assert_eq!(parsed.clip.as_ref().unwrap().trim_in, 1.0);
        assert_eq!(parsed.exporting, Some(0.5));
        let Some(Line::Queue(parsed)) = parse_line(&json!({
            "type": "queue", "items": [{ "id": "a", "name": "a.mp4", "duration": null }], "selected": "a",
        })) else {
            panic!("a queue line");
        };
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.selected.as_deref(), Some("a"));
        assert!(parse_line(&json!({ "type": "other" })).is_none());
    }

    #[test]
    fn commands_are_json_lines() {
        let command: Value = serde_json::from_str(&command("togglePlay")).unwrap();
        assert_eq!(command, json!({ "cmd": "togglePlay" }));
        let select: Value =
            serde_json::from_str(&command_with("select", json!({ "id": "a" }))).unwrap();
        assert_eq!(select, json!({ "cmd": "select", "id": "a" }));
    }

    #[test]
    fn timecodes() {
        assert_eq!(timecode(0.0), "0:00.0");
        assert_eq!(timecode(12.44), "0:12.4");
        assert_eq!(timecode(65.0), "1:05.0");
        assert_eq!(timecode(3725.0), "1:02:05");
        assert_eq!(timecode(-3.0), "0:00.0");
    }

    #[test]
    fn dial_settings_default_and_sanitise() {
        assert_eq!(
            DialSettings::from_payload(&json!({})),
            DialSettings::default()
        );
        let parsed = DialSettings::from_payload(&json!({ "settings": {
            "unit": "parsecs", "amount": -2, "press": "explode", "wrap": true,
        }}));
        assert_eq!(
            parsed,
            DialSettings {
                wrap: true,
                ..DialSettings::default()
            }
        );
        let frames = DialSettings::from_payload(&json!({ "settings": { "amount": 2.6 } }));
        assert_eq!(frames.amount, 3.0);
        let seconds = DialSettings::from_payload(
            &json!({ "settings": { "unit": "seconds", "amount": 0.5 } }),
        );
        assert_eq!(seconds.amount, 0.5);
    }

    #[test]
    fn scrub_commands_step_frames_or_seconds() {
        let frames = DialSettings {
            amount: 2.0,
            ..DialSettings::default()
        };
        let sent: Value = serde_json::from_str(&frames.scrub_command(-3)).unwrap();
        assert_eq!(sent, json!({ "cmd": "stepFrames", "frames": -6 }));
        let seconds = DialSettings {
            unit: "seconds".into(),
            amount: 0.5,
            ..DialSettings::default()
        };
        let sent: Value = serde_json::from_str(&seconds.scrub_command(3)).unwrap();
        assert_eq!(sent, json!({ "cmd": "skip", "seconds": 1.5 }));
    }

    #[test]
    fn cursor_clamps_or_wraps() {
        assert_eq!(move_cursor(0, -1, 5, false), 0);
        assert_eq!(move_cursor(3, 4, 5, false), 4);
        assert_eq!(move_cursor(0, -1, 5, true), 4);
        assert_eq!(move_cursor(4, 3, 5, true), 2);
        assert_eq!(move_cursor(2, 1, 0, true), 0);
    }

    #[test]
    fn key_looks_follow_qlipq() {
        let settings = KeySettings::from_payload(&json!({}));
        let open = state(Some(clip()), None);
        let empty = queue(&[], None);

        let off = key_look(KeyKind::Play, &settings, false, &open, &empty);
        assert!(off.idle && off.status == "QlipQ off");

        let play = key_look(KeyKind::Play, &settings, true, &open, &empty);
        assert_eq!(
            (play.label.as_str(), play.status.as_str(), play.on),
            ("Play", "0:12.4", false)
        );
        let mut playing = clip();
        playing.playing = true;
        let pause = key_look(
            KeyKind::Play,
            &settings,
            true,
            &state(Some(playing), None),
            &empty,
        );
        assert_eq!(
            (pause.label.as_str(), pause.glyph, pause.on),
            ("Pause", Glyph::Pause, true)
        );

        let set_in = key_look(KeyKind::SetIn, &settings, true, &open, &empty);
        assert_eq!(set_in.status, "0:03.0");
        let set_out = key_look(KeyKind::SetOut, &settings, true, &state(None, None), &empty);
        assert!(set_out.idle && set_out.status == "No clip");

        let exporting = key_look(
            KeyKind::Export,
            &settings,
            true,
            &state(Some(clip()), Some(0.42)),
            &empty,
        );
        assert_eq!(
            (exporting.status.as_str(), exporting.on, exporting.progress),
            ("42%", true, Some(0.42))
        );

        let rescan = key_look(
            KeyKind::Rescan,
            &settings,
            true,
            &open,
            &queue(&["a", "b"], None),
        );
        assert_eq!(rescan.status, "2 clips");

        let named = KeySettings::from_payload(&json!({ "settings": { "label": " Mark in " } }));
        assert_eq!(
            key_look(KeyKind::SetIn, &named, true, &open, &empty).label,
            "Mark in"
        );
    }

    #[test]
    fn key_settings_default_to_cancelling_exports() {
        assert!(KeySettings::from_payload(&json!({})).cancels_export());
        assert!(!KeySettings::from_payload(
            &json!({ "settings": { "whileExporting": "nothing" } })
        )
        .cancels_export());
    }

    #[test]
    fn key_image_draws_progress_and_escapes() {
        let look = KeyLook {
            label: "A & B".into(),
            status: "50%".into(),
            on: true,
            idle: false,
            glyph: Glyph::Export,
            progress: Some(0.5),
        };
        let image = key_image(&look);
        let svg = String::from_utf8(
            BASE64
                .decode(image.strip_prefix("data:image/svg+xml;base64,").unwrap())
                .unwrap(),
        )
        .unwrap();
        assert!(svg.contains(">A &amp; B<"));
        assert!(svg.contains(r#"width="72.0""#));
    }

    #[test]
    fn scrub_strip_shows_time_position_and_trim() {
        let payload = scrub_feedback(true, &state(Some(clip()), None));
        assert_eq!(payload["title"], "2026-11-28 18-17-26.mp4");
        assert_eq!(payload["time"]["value"], "0:12.4 / 1:05.0");
        assert_eq!(payload["position"], 19);
        assert_eq!(payload["trim"], "In 0:03.0  ·  Out 0:45.0");
        assert_eq!(
            scrub_feedback(false, &State::default())["title"],
            "QlipQ off"
        );
        assert_eq!(
            scrub_feedback(true, &State::default())["title"],
            "No clip open"
        );
    }

    #[test]
    fn picker_strip_shows_the_cursor_clip() {
        let clips = queue(&["a.mp4", "b.mp4", "c.mp4"], Some("b.mp4"));
        let payload = picker_feedback(true, &clips, Some(1));
        assert_eq!(payload["title"], "Clip 2 of 3");
        assert_eq!(payload["name"]["value"], "b.mp4");
        assert_eq!(payload["detail"], "0:30.0  ·  Open");
        let other = picker_feedback(true, &clips, Some(2));
        assert_eq!(other["detail"], "0:30.0");
        assert_eq!(
            picker_feedback(true, &queue(&[], None), None)["name"],
            "No clips"
        );
        assert_eq!(picker_feedback(false, &clips, Some(0))["name"], "QlipQ off");
    }
}
