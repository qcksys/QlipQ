//! The Save Replay key: a short press saves the OBS replay buffer, a long press starts or stops
//! it, and the key's image shows whether it is running.

use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_HOLD_S: f32 = 0.5;
const DEFAULT_LABEL: &str = "Save Replay";
/// Longer labels don't fit across the key at the smallest font size.
const MAX_LABEL_CHARS: usize = 18;

/// A Save Replay key's settings, as its property inspector (`pi/replay.html`) saves them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Seconds to hold the key before it starts or stops the buffer.
    pub hold: f32,
    /// What holding does: "toggle" starts or stops the buffer; "none" makes every press a save.
    pub hold_action: String,
    /// Text on the key image.
    pub label: String,
    /// Which saves get a checkmark: "all" (any save, including OBS's hotkey) or "own" (this key's).
    pub confirm: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hold: DEFAULT_HOLD_S,
            hold_action: "toggle".into(),
            label: DEFAULT_LABEL.into(),
            confirm: "all".into(),
        }
    }
}

impl Settings {
    pub fn from_payload(payload: &Value) -> Self {
        let mut settings: Self =
            serde_json::from_value(payload["settings"].clone()).unwrap_or_default();
        let defaults = Self::default();
        if !(0.2..=3.0).contains(&settings.hold) {
            settings.hold = defaults.hold;
        }
        if settings.hold_action != "none" {
            settings.hold_action = defaults.hold_action;
        }
        settings.label = settings
            .label
            .trim()
            .chars()
            .take(MAX_LABEL_CHARS)
            .collect();
        if settings.label.is_empty() {
            settings.label = defaults.label;
        }
        if settings.confirm != "own" {
            settings.confirm = defaults.confirm;
        }
        settings
    }

    /// How long a press must last to toggle the buffer, or `None` if holding doesn't toggle.
    pub fn hold_time(&self) -> Option<Duration> {
        (self.hold_action == "toggle").then(|| Duration::from_secs_f32(self.hold))
    }

    pub fn confirms_every_save(&self) -> bool {
        self.confirm == "all"
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Save,
    Toggle,
}

/// A key that is being held down.
pub struct Press {
    since: Instant,
    hold: Option<Duration>,
    toggled: bool,
}

impl Press {
    /// `hold` is how long the press must last to toggle; `None` never toggles.
    pub fn new(now: Instant, hold: Option<Duration>) -> Self {
        Self {
            since: now,
            hold,
            toggled: false,
        }
    }

    /// Returns `Toggle` once, as soon as the key has been held for the hold time.
    pub fn poll(&mut self, now: Instant) -> Option<Command> {
        let hold = self.hold?;
        if self.toggled || now.duration_since(self.since) < hold {
            return None;
        }
        self.toggled = true;
        Some(Command::Toggle)
    }

    /// A short press saves. A long press has already toggled, unless polling missed it.
    pub fn release(mut self, now: Instant) -> Option<Command> {
        if self.toggled {
            return None;
        }
        self.poll(now).or(Some(Command::Save))
    }
}

/// What OBS last said about its replay buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    /// Not asked yet, or OBS was still starting.
    Unknown,
    /// The replay buffer is switched off in OBS's output settings.
    Unavailable,
    Active(bool),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Look {
    ObsOff,
    BufferOff,
    BufferOn,
}

pub fn look(connected: bool, status: Status) -> Look {
    match (connected, status) {
        (false, _) => Look::ObsOff,
        (true, Status::Active(true)) => Look::BufferOn,
        _ => Look::BufferOff,
    }
}

/// The key image for a look and label, as a data URL for `setImage`.
pub fn image(look: Look, label: &str) -> String {
    format!(
        "data:image/svg+xml;base64,{}",
        BASE64.encode(key_svg(look, label))
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The Tom Custom deck's key style: a dark key with an accent bar, or the accent as a gradient
/// when the thing it controls is on. `imgs/replay-off.svg` is the off look with the default label.
fn key_svg(look: Look, label: &str) -> String {
    const SAVE_GLYPH: &str = r#"<path d="M14 12 H72 L88 28 V88 H14 Z"/><path d="M30 12 V34 H66 V12"/><rect x="30" y="54" width="40" height="34" rx="3"/>"#;
    let (gradient, bar, glyph, label_color) = match look {
        Look::BufferOn => (
            r##"<stop offset="0" stop-color="#ff5f5f" stop-opacity="0.55"/><stop offset="1" stop-color="#ff5f5f" stop-opacity="0.22"/>"##,
            "#ff5f5f",
            "#ffffff",
            "#f2f5f8",
        ),
        Look::BufferOff => (
            r##"<stop offset="0" stop-color="#202835"/><stop offset="1" stop-color="#0f141b"/>"##,
            "#ff5f5f",
            "#ff5f5f",
            "#f2f5f8",
        ),
        Look::ObsOff => (
            r##"<stop offset="0" stop-color="#202835"/><stop offset="1" stop-color="#0f141b"/>"##,
            "#3d4858",
            "#5c6878",
            "#9fabbb",
        ),
    };
    let status = match look {
        Look::BufferOn => r##"<circle cx="38" cy="125" r="5" fill="#ffffff"/><text x="49" y="130" font-family="Segoe UI" font-weight="600" font-size="15" fill="#ffffff">Buffer on</text>"##.to_string(),
        Look::BufferOff => status_text("Buffer off"),
        Look::ObsOff => status_text("OBS off"),
    };
    let size = match label.chars().count() {
        0..=10 => 19,
        11..=13 => 17,
        _ => 15,
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144"><defs><linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">{gradient}</linearGradient></defs><rect width="144" height="144" fill="#0f141b"/><rect width="144" height="144" fill="url(#bg)"/><rect width="144" height="6" fill="{bar}"/><g transform="translate(39 14) scale(0.66)"><g fill="none" stroke="{glyph}" stroke-width="7" stroke-linecap="round" stroke-linejoin="round">{SAVE_GLYPH}</g></g><text x="72" y="103" text-anchor="middle" font-family="Segoe UI" font-weight="600" font-size="{size}" fill="{label_color}">{label}</text>{status}</svg>"##,
        label = escape(label),
    )
}

fn status_text(text: &str) -> String {
    format!(
        r##"<text x="72" y="130" text-anchor="middle" font-family="Segoe UI" font-size="15" fill="#9fabbb">{text}</text>"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HOLD: Option<Duration> = Some(Duration::from_millis(500));

    #[test]
    fn short_press_saves() {
        let start = Instant::now();
        let mut press = Press::new(start, HOLD);
        assert_eq!(press.poll(start + Duration::from_millis(200)), None);
        assert_eq!(
            press.release(start + Duration::from_millis(300)),
            Some(Command::Save)
        );
    }

    #[test]
    fn long_press_toggles_once_while_held() {
        let start = Instant::now();
        let mut press = Press::new(start, HOLD);
        assert_eq!(
            press.poll(start + Duration::from_millis(500)),
            Some(Command::Toggle)
        );
        assert_eq!(press.poll(start + Duration::from_secs(2)), None);
        assert_eq!(press.release(start + Duration::from_secs(3)), None);
    }

    #[test]
    fn long_press_released_before_a_poll_still_toggles() {
        let start = Instant::now();
        let press = Press::new(start, HOLD);
        assert_eq!(
            press.release(start + Duration::from_millis(700)),
            Some(Command::Toggle)
        );
    }

    #[test]
    fn with_no_hold_action_every_press_saves() {
        let start = Instant::now();
        let mut press = Press::new(start, None);
        assert_eq!(press.poll(start + Duration::from_secs(5)), None);
        assert_eq!(
            press.release(start + Duration::from_secs(5)),
            Some(Command::Save)
        );
    }

    #[test]
    fn settings_default_and_sanitise() {
        assert_eq!(Settings::from_payload(&json!({})), Settings::default());
        let parsed = Settings::from_payload(&json!({ "settings": {
            "hold": 9, "holdAction": "explode", "label": "   ", "confirm": "nobody",
        }}));
        assert_eq!(parsed, Settings::default());
        let parsed = Settings::from_payload(&json!({ "settings": {
            "hold": 1.2, "holdAction": "none", "label": " Clip it ", "confirm": "own",
        }}));
        assert_eq!(parsed.hold, 1.2);
        assert_eq!(parsed.hold_time(), None);
        assert_eq!(parsed.label, "Clip it");
        assert!(!parsed.confirms_every_save());
        assert_eq!(
            Settings::default().hold_time(),
            Some(Duration::from_millis(500))
        );
    }

    #[test]
    fn settings_cut_long_labels() {
        let parsed = Settings::from_payload(
            &json!({ "settings": { "label": "A very long label for one key" } }),
        );
        assert_eq!(parsed.label.chars().count(), MAX_LABEL_CHARS);
    }

    #[test]
    fn look_follows_obs_and_buffer() {
        assert_eq!(look(false, Status::Active(true)), Look::ObsOff);
        assert_eq!(look(true, Status::Active(true)), Look::BufferOn);
        assert_eq!(look(true, Status::Active(false)), Look::BufferOff);
        assert_eq!(look(true, Status::Unknown), Look::BufferOff);
        assert_eq!(look(true, Status::Unavailable), Look::BufferOff);
    }

    fn decoded(image: &str) -> String {
        let svg = image
            .strip_prefix("data:image/svg+xml;base64,")
            .expect("an SVG data URL");
        String::from_utf8(BASE64.decode(svg).unwrap()).unwrap()
    }

    #[test]
    fn images_show_the_label_and_state() {
        let on = decoded(&image(Look::BufferOn, "Save Replay"));
        assert!(
            on.starts_with("<svg") && on.contains(">Save Replay<") && on.contains(">Buffer on<")
        );
        assert!(decoded(&image(Look::BufferOff, "Save Replay")).contains(">Buffer off<"));
        assert!(decoded(&image(Look::ObsOff, "Save Replay")).contains(">OBS off<"));
    }

    #[test]
    fn images_escape_labels_and_shrink_long_ones() {
        let svg = decoded(&image(Look::BufferOff, "Tom's <clips> & co"));
        assert!(svg.contains(">Tom&apos;s &lt;clips&gt; &amp; co<"));
        assert!(svg.contains(r##"font-size="15" fill="#f2f5f8""##));
        assert!(decoded(&image(Look::BufferOff, "Clip")).contains(r#"font-size="19""#));
    }
}
