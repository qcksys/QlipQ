//! Plugin state: which OBS input each meter dial controls and the latest OBS levels, the replay
//! buffer behind the Save Replay keys, PC load for the stats dials, and what each one shows.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::audio::{self, Audio};
use crate::obs::{self, ObsEvent};
use crate::replay::{self, Press};
use crate::stats::{self, Sampler};
use crate::{log, meter, streamdeck};

mod keys;
mod qlipq_actions;
use keys::{KeyAction, ObsKeys};
use qlipq_actions::Qlipq;

pub const ACTION_UUID: &str = "com.qcksys.qlipq.meters.input";
pub const REPLAY_UUID: &str = "com.qcksys.qlipq.meters.replay";
pub const STATS_UUID: &str = "com.qcksys.qlipq.meters.stats";
pub const OUTPUT_UUID: &str = "com.qcksys.qlipq.meters.output";
const DEFAULT_STEP_DB: f32 = 0.2;
/// How often to re-ask OBS about state it hasn't reported yet (it answers "not ready" while starting).
const STATE_RETRY: Duration = Duration::from_secs(1);
/// Ticks are 66 ms apart, so a dial counts as due slightly early rather than a whole tick late.
const STATS_TOLERANCE: Duration = Duration::from_millis(100);
/// Sampling runs on the same thread as the meters, so a slower sample is worth logging.
const SLOW_SAMPLE: Duration = Duration::from_millis(50);
/// obs-websocket's NotReady status code.
const OBS_NOT_READY: i64 = 207;
const WHITE: &str = "#ffffff";
const GREY: &str = "#8a96a8";
const RED: &str = "#ff5f5f";

/// An OBS Input Meter dial's settings, as its property inspector (`pi/index.html`) saves them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// OBS input name, e.g. "Mic/Aux".
    pub input: String,
    /// dB per dial tick.
    pub step: f32,
    /// Shown on the strip instead of the input name; empty shows the input name.
    pub label: String,
    /// The quietest level the meter and fader show, in dB.
    pub floor: f32,
    /// How fast the peak bars fall back, in percent of the bar per second.
    pub falloff: f32,
    /// What pressing the dial or tapping the strip does: "mute" or "none".
    pub press: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input: String::new(),
            step: DEFAULT_STEP_DB,
            label: String::new(),
            floor: meter::FLOOR_DB,
            falloff: meter::FALL_PERCENT_PER_S,
            press: "mute".into(),
        }
    }
}

impl Settings {
    pub fn from_payload(payload: &Value) -> Self {
        let mut settings: Self =
            serde_json::from_value(payload["settings"].clone()).unwrap_or_default();
        let defaults = Self::default();
        settings.input = settings.input.trim().to_string();
        settings.label = settings.label.trim().to_string();
        if !(settings.step > 0.0 && settings.step <= 20.0) {
            settings.step = defaults.step;
        }
        if !(-96.0..=-10.0).contains(&settings.floor) {
            settings.floor = defaults.floor;
        }
        if !(10.0..=2000.0).contains(&settings.falloff) {
            settings.falloff = defaults.falloff;
        }
        if settings.press != "none" {
            settings.press = defaults.press;
        }
        settings
    }

    fn mutes_on_press(&self) -> bool {
        self.press == "mute"
    }
}

#[derive(Default)]
struct Input {
    /// `None` until OBS has reported it.
    volume_db: Option<f32>,
    muted: Option<bool>,
    /// Latest post-fader peaks, `[left, right]` dBFS.
    peaks_db: [f32; 2],
}

#[derive(Debug, PartialEq)]
struct View {
    title: String,
    value: String,
    value_color: &'static str,
    meters: [i32; 2],
    fader: i32,
}

/// What was last sent to a dial, so each tick only sends the items that changed.
#[derive(Default)]
struct Sent {
    title: Option<String>,
    value: Option<(String, &'static str)>,
    meters: Option<[i32; 2]>,
    fader: Option<i32>,
}

struct Dial {
    settings: Settings,
    sent: Sent,
    /// What the meter bars currently show, in percent; falls back towards the peaks.
    shown: [f32; 2],
}

struct ReplayKey {
    settings: replay::Settings,
    press: Option<Press>,
    /// The look and label last drawn.
    shown: Option<(replay::Look, String)>,
    /// A save this key asked for that OBS hasn't confirmed yet.
    awaiting_save: bool,
}

struct StatsDial {
    settings: stats::Settings,
    sent: Option<Value>,
    last_sent: Option<Instant>,
}

struct OutputKey {
    settings: audio::Settings,
    /// The chosen device as Windows last reported it.
    device: Option<audio::Device>,
    /// The look, label and glyph last drawn.
    shown: Option<(audio::Look, String, bool)>,
}

fn view(settings: &Settings, input: Option<&Input>, shown: [f32; 2], connected: bool) -> View {
    let title = if !settings.label.is_empty() {
        settings.label.clone()
    } else if settings.input.is_empty() {
        "Set an input".to_string()
    } else {
        settings.input.clone()
    };
    let idle = |value: &str| View {
        title: title.clone(),
        value: value.to_string(),
        value_color: GREY,
        meters: [0, 0],
        fader: 0,
    };
    if !connected {
        return idle("OBS off");
    }
    let Some(input) = input.filter(|input| input.volume_db.is_some()) else {
        return idle("");
    };
    let volume_db = input.volume_db.unwrap_or(f32::NEG_INFINITY);
    let (value, value_color) = if input.muted == Some(true) {
        ("Muted".to_string(), RED)
    } else {
        (meter::format_db(volume_db), WHITE)
    };
    View {
        title,
        value,
        value_color,
        meters: shown.map(|percent| percent.round() as i32),
        fader: meter::db_to_percent(volume_db, settings.floor).round() as i32,
    }
}

/// The `setFeedback` payload for the items that differ from what was last sent.
fn changes(view: &View, sent: &mut Sent) -> Option<Value> {
    let mut payload = Map::new();
    if sent.title.as_deref() != Some(view.title.as_str()) {
        payload.insert("title".into(), json!(view.title));
        sent.title = Some(view.title.clone());
    }
    let value = (view.value.clone(), view.value_color);
    if sent.value.as_ref() != Some(&value) {
        payload.insert(
            "value".into(),
            json!({ "value": view.value, "color": view.value_color }),
        );
        sent.value = Some(value);
    }
    if sent.meters != Some(view.meters) {
        payload.insert("meterL".into(), json!(view.meters[0]));
        payload.insert("meterR".into(), json!(view.meters[1]));
        sent.meters = Some(view.meters);
    }
    if sent.fader != Some(view.fader) {
        payload.insert("fader".into(), json!(view.fader));
        sent.fader = Some(view.fader);
    }
    (!payload.is_empty()).then_some(Value::Object(payload))
}

/// OBS reports `inputVolumeDb` as null for -inf.
fn read_db(value: &Value) -> f32 {
    value.as_f64().map_or(f32::NEG_INFINITY, |db| db as f32)
}

pub struct App {
    streamdeck: UnboundedSender<String>,
    obs: Option<UnboundedSender<String>>,
    dials: HashMap<String, Dial>,
    inputs: HashMap<String, Input>,
    replay_keys: HashMap<String, ReplayKey>,
    replay_status: replay::Status,
    keys: ObsKeys,
    qlipq: Qlipq,
    stats_dials: HashMap<String, StatsDial>,
    sampler: Option<Sampler>,
    sample: Option<stats::Sample>,
    output_keys: HashMap<String, OutputKey>,
    /// Opened with the first output key; needs `audio_events` to report device changes on.
    audio: Option<Audio>,
    audio_events: Option<UnboundedSender<()>>,
    output_defaults: audio::Defaults,
    /// A device or default role changed, so output keys need re-reading before they're drawn.
    audio_changed: bool,
    last_tick: Instant,
    last_state_retry: Instant,
    last_sample: Instant,
    /// Problems already logged, so a repeating one (a retried bad input name) logs once.
    logged_failures: HashSet<String>,
}

impl App {
    pub fn new(streamdeck: UnboundedSender<String>) -> Self {
        Self {
            streamdeck,
            obs: None,
            dials: HashMap::new(),
            inputs: HashMap::new(),
            replay_keys: HashMap::new(),
            replay_status: replay::Status::Unknown,
            keys: ObsKeys::default(),
            qlipq: Qlipq::default(),
            stats_dials: HashMap::new(),
            sampler: None,
            sample: None,
            output_keys: HashMap::new(),
            audio: None,
            audio_events: None,
            output_defaults: audio::Defaults::default(),
            audio_changed: false,
            last_tick: Instant::now(),
            last_state_retry: Instant::now(),
            last_sample: Instant::now(),
            logged_failures: HashSet::new(),
        }
    }

    /// Lets output keys hear about device changes: Windows' notifications send `()` here, and the
    /// main loop passes them on to `on_audio_changed`.
    pub fn with_audio_events(mut self, events: UnboundedSender<()>) -> Self {
        self.audio_events = Some(events);
        self
    }

    pub fn on_audio_changed(&mut self) {
        self.audio_changed = true;
    }

    pub fn on_streamdeck(&mut self, message: Value) {
        let event = message["event"].as_str().unwrap_or_default();
        let context = message["context"].as_str().unwrap_or_default().to_string();
        let payload = &message["payload"];
        let action = message["action"].as_str().unwrap_or_default();
        if let Some(kind) = KeyAction::from_uuid(action) {
            self.on_obs_key(kind, event, context, payload);
            return;
        }
        if self.on_qlipq_action(action, event, &context, payload) {
            return;
        }
        match action {
            REPLAY_UUID => self.on_replay_key(event, context, payload, Instant::now()),
            STATS_UUID => self.on_stats_dial(event, context, payload),
            OUTPUT_UUID => self.on_output_key(event, context, payload),
            _ => self.on_meter_dial(event, context, &message),
        }
    }

    fn on_output_key(&mut self, event: &str, context: String, payload: &Value) {
        match event {
            "willAppear" | "didReceiveSettings" => {
                if self.audio.is_none() {
                    self.audio = self.audio_events.clone().and_then(Audio::new);
                }
                let settings = audio::Settings::from_payload(payload);
                self.output_keys.insert(
                    context,
                    OutputKey {
                        settings,
                        device: None,
                        shown: None,
                    },
                );
                self.audio_changed = true;
            }
            "willDisappear" => {
                self.output_keys.remove(&context);
            }
            "keyDown" => {
                let Some(key) = self.output_keys.get(&context) else {
                    return;
                };
                let result = match (&self.audio, key.device.as_ref()) {
                    (Some(audio), Some(device)) if device.active => audio.set_output(&device.id),
                    _ => Err(anyhow::anyhow!("output device isn't connected")),
                };
                if let Err(err) = result {
                    log::write(&format!("{err:#}"));
                    let _ = self.streamdeck.send(streamdeck::show_alert(&context));
                }
                self.audio_changed = true;
            }
            "propertyInspectorDidAppear" => {
                let devices: Vec<Value> = self
                    .audio
                    .as_ref()
                    .map(Audio::devices)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|device| json!({ "id": device.id, "name": device.name }))
                    .collect();
                let _ = self.streamdeck.send(streamdeck::send_to_property_inspector(
                    OUTPUT_UUID,
                    &context,
                    json!({ "devices": devices }),
                ));
            }
            _ => {}
        }
    }

    fn tick_output_keys(&mut self) {
        if self.audio_changed {
            self.audio_changed = false;
            if let Some(audio) = &self.audio {
                self.output_defaults = audio.defaults();
                for key in self.output_keys.values_mut() {
                    key.device = audio.device(&key.settings.device_id);
                }
            }
        }
        for (context, key) in &mut self.output_keys {
            let device = key.device.as_ref();
            let look = audio::look(&key.settings.device_id, device, &self.output_defaults);
            let label = audio::label(&key.settings, device);
            let headphones = audio::is_headphones(&label, device);
            let drawn = (look, label, headphones);
            if key.shown.as_ref() != Some(&drawn) {
                let _ = self.streamdeck.send(streamdeck::set_image(
                    context,
                    &audio::image(look, &drawn.1, headphones),
                ));
                key.shown = Some(drawn);
            }
        }
    }

    fn on_replay_key(&mut self, event: &str, context: String, payload: &Value, now: Instant) {
        match event {
            "willAppear" | "didReceiveSettings" => {
                let settings = replay::Settings::from_payload(payload);
                let key = self.replay_keys.entry(context).or_insert(ReplayKey {
                    settings: settings.clone(),
                    press: None,
                    shown: None,
                    awaiting_save: false,
                });
                key.settings = settings;
                key.shown = None;
                if event == "willAppear" && self.replay_status == replay::Status::Unknown {
                    self.request_replay_status();
                }
            }
            "willDisappear" => {
                self.replay_keys.remove(&context);
            }
            "keyDown" => {
                if let Some(key) = self.replay_keys.get_mut(&context) {
                    key.press = Some(Press::new(now, key.settings.hold_time()));
                }
            }
            "keyUp" => {
                let command = self
                    .replay_keys
                    .get_mut(&context)
                    .and_then(|key| key.press.take())
                    .and_then(|press| press.release(now));
                if let Some(command) = command {
                    self.replay_command(&context, command);
                }
            }
            _ => {}
        }
    }

    fn on_stats_dial(&mut self, event: &str, context: String, payload: &Value) {
        match event {
            "willAppear" | "didReceiveSettings" => {
                if self.sampler.is_none() {
                    self.sampler = Sampler::new();
                    self.last_sample = Instant::now();
                }
                let mut dial = StatsDial {
                    settings: stats::Settings::from_payload(payload),
                    sent: None,
                    last_sent: None,
                };
                // Show the last reading straight away rather than dashes until the next sample.
                self.send_stats(&context, &mut dial, Instant::now());
                self.stats_dials.insert(context, dial);
            }
            "willDisappear" => {
                self.stats_dials.remove(&context);
            }
            "propertyInspectorDidAppear" => {
                // Offer only GPUs the counters currently report, when there are any.
                let seen = self.sample.as_ref().map(|sample| &sample.gpus);
                let gpus: Vec<Value> = stats::gpu_names()
                    .into_iter()
                    .filter(|(id, _)| {
                        seen.is_none_or(|seen| seen.is_empty() || seen.contains_key(id))
                    })
                    .map(|(id, name)| json!({ "id": id, "name": name }))
                    .collect();
                let _ = self.streamdeck.send(streamdeck::send_to_property_inspector(
                    STATS_UUID,
                    &context,
                    json!({ "gpus": gpus }),
                ));
            }
            _ => {}
        }
    }

    fn send_stats(&self, context: &str, dial: &mut StatsDial, now: Instant) {
        let payload = stats::feedback(&self.sample.clone().unwrap_or_default(), &dial.settings);
        dial.last_sent = Some(now);
        if dial.sent.as_ref() != Some(&payload) {
            let _ = self
                .streamdeck
                .send(streamdeck::set_feedback(context, payload.clone()));
            dial.sent = Some(payload);
        }
    }

    fn on_meter_dial(&mut self, event: &str, context: String, message: &Value) {
        match event {
            "willAppear" | "didReceiveSettings" => {
                let settings = Settings::from_payload(&message["payload"]);
                let input = settings.input.clone();
                self.dials.insert(
                    context,
                    Dial {
                        settings,
                        sent: Sent::default(),
                        shown: [0.0, 0.0],
                    },
                );
                self.request_input_state(&input);
            }
            "willDisappear" => {
                self.dials.remove(&context);
            }
            "dialRotate" => {
                let ticks = message["payload"]["ticks"].as_i64().unwrap_or(0) as i32;
                self.rotate(&context, ticks);
            }
            "dialDown" | "touchTap" => {
                if self
                    .dials
                    .get(&context)
                    .is_some_and(|dial| dial.settings.mutes_on_press())
                {
                    self.toggle_mute(&context);
                }
            }
            "propertyInspectorDidAppear" | "sendToPlugin" => {
                self.obs_request(
                    "GetInputList",
                    &format!("GetInputList:{context}"),
                    json!({}),
                );
            }
            _ => {}
        }
    }

    pub fn on_obs(&mut self, event: ObsEvent) {
        match event {
            ObsEvent::Connected(requests) => {
                self.obs = Some(requests);
                self.inputs.clear();
                self.keys.reset_status();
                self.sync_obs_keys();
                let mut names: Vec<String> = self
                    .dials
                    .values()
                    .map(|dial| dial.settings.input.clone())
                    .collect();
                names.sort();
                names.dedup();
                for name in names {
                    self.request_input_state(&name);
                }
                self.replay_status = replay::Status::Unknown;
                if self.has_buffer_keys() {
                    self.request_replay_status();
                }
            }
            ObsEvent::Disconnected => {
                self.obs = None;
                self.keys.reset_status();
                self.inputs.clear();
                self.replay_status = replay::Status::Unknown;
            }
            ObsEvent::Message(message) => match message["op"].as_i64() {
                Some(5) => self.on_obs_event(&message["d"]),
                Some(7) => self.on_obs_response(&message["d"]),
                _ => {}
            },
        }
    }

    /// Advances the meter falloff, handles held Save Replay keys, samples PC load once a second,
    /// and sends each dial and key whatever changed.
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.retry_unknown_state(now);
        self.tick_replay_keys(now);
        self.draw_obs_keys();
        self.tick_stats(now);
        self.tick_output_keys();
        self.draw_qlipq();
        let elapsed = now.duration_since(self.last_tick).as_secs_f32().min(0.5);
        self.last_tick = now;
        let connected = self.obs.is_some();
        for (context, dial) in &mut self.dials {
            let input = self.inputs.get(&dial.settings.input);
            let peaks = input.map_or([f32::NEG_INFINITY; 2], |input| input.peaks_db);
            for (shown, peak) in dial.shown.iter_mut().zip(peaks) {
                let target = meter::db_to_percent(peak, dial.settings.floor);
                *shown = meter::decay(*shown, target, elapsed, dial.settings.falloff);
            }
            let view = view(&dial.settings, input, dial.shown, connected);
            if let Some(payload) = changes(&view, &mut dial.sent) {
                let _ = self
                    .streamdeck
                    .send(streamdeck::set_feedback(context, payload));
            }
        }
    }

    fn tick_replay_keys(&mut self, now: Instant) {
        let held: Vec<String> = self
            .replay_keys
            .iter_mut()
            .filter_map(|(context, key)| {
                key.press.as_mut()?.poll(now)?;
                Some(context.clone())
            })
            .collect();
        for context in held {
            self.replay_command(&context, replay::Command::Toggle);
        }
        let look = replay::look(self.obs.is_some(), self.replay_status);
        for (context, key) in &mut self.replay_keys {
            let drawn = (look, key.settings.label.clone());
            if key.shown.as_ref() != Some(&drawn) {
                let _ = self.streamdeck.send(streamdeck::set_image(
                    context,
                    &replay::image(look, &key.settings.label),
                ));
                key.shown = Some(drawn);
            }
        }
    }

    fn tick_stats(&mut self, now: Instant) {
        // Sample as often as the fastest dial wants; each dial then updates on its own interval.
        let Some(interval) = self
            .stats_dials
            .values()
            .map(|dial| dial.settings.interval())
            .min()
        else {
            return;
        };
        if now.duration_since(self.last_sample) < interval {
            return;
        }
        let Some(sampler) = self.sampler.as_mut() else {
            return;
        };
        self.last_sample = now;
        let started = Instant::now();
        let sample = sampler.sample();
        let took = started.elapsed();
        if took > SLOW_SAMPLE && self.logged_failures.insert("slow PC stats sample".into()) {
            log::write(&format!(
                "PC stats sample took {} ms, which delays the meters",
                took.as_millis()
            ));
        }
        self.sample = Some(sample);
        let mut dials = std::mem::take(&mut self.stats_dials);
        for (context, dial) in &mut dials {
            let due = dial.last_sent.is_none_or(|sent| {
                now.duration_since(sent) + STATS_TOLERANCE >= dial.settings.interval()
            });
            if due {
                self.send_stats(context, dial, now);
            }
        }
        self.stats_dials = dials;
    }

    fn replay_command(&self, context: &str, command: replay::Command) {
        if self.obs.is_none() {
            let _ = self.streamdeck.send(streamdeck::show_alert(context));
            return;
        }
        let request = match command {
            replay::Command::Save => "SaveReplayBuffer",
            replay::Command::Toggle => "ToggleReplayBuffer",
        };
        self.obs_request(request, &format!("{request}:{context}"), json!({}));
    }

    fn request_replay_status(&self) {
        self.obs_request("GetReplayBufferStatus", "GetReplayBufferStatus", json!({}));
    }

    /// Re-requests dial inputs and replay buffer state OBS hasn't reported yet, at most once per
    /// `STATE_RETRY`.
    fn retry_unknown_state(&mut self, now: Instant) {
        if self.obs.is_none() || now.duration_since(self.last_state_retry) < STATE_RETRY {
            return;
        }
        self.last_state_retry = now;
        self.sync_obs_keys();
        if self.replay_status == replay::Status::Unknown && self.has_buffer_keys() {
            self.request_replay_status();
        }
        let mut unknown: Vec<String> = self
            .dials
            .values()
            .map(|dial| dial.settings.input.clone())
            .filter(|name| {
                self.inputs
                    .get(name)
                    .and_then(|input| input.volume_db)
                    .is_none()
            })
            .collect();
        unknown.sort();
        unknown.dedup();
        for name in unknown {
            self.request_input_state(&name);
        }
    }

    fn is_tracked(&self, input: &str) -> bool {
        self.dials.values().any(|dial| dial.settings.input == input)
            || self.keys.tracks_input(input)
    }

    fn obs_request(&self, request_type: &str, request_id: &str, data: Value) {
        if let Some(obs) = &self.obs {
            let _ = obs.send(obs::request(request_type, request_id, data));
        }
    }

    fn request_input_state(&self, input: &str) {
        if input.is_empty() {
            return;
        }
        let data = json!({ "inputName": input });
        self.obs_request(
            "GetInputVolume",
            &format!("GetInputVolume:{input}"),
            data.clone(),
        );
        self.obs_request("GetInputMute", &format!("GetInputMute:{input}"), data);
    }

    fn rotate(&mut self, context: &str, ticks: i32) {
        let Some(dial) = self.dials.get(context) else {
            return;
        };
        let (name, step) = (dial.settings.input.clone(), dial.settings.step);
        let Some(current) = self.inputs.get(&name).and_then(|input| input.volume_db) else {
            return;
        };
        let next = meter::step_volume(current, ticks, step);
        if let Some(input) = self.inputs.get_mut(&name) {
            input.volume_db = Some(next);
        }
        self.obs_request(
            "SetInputVolume",
            "SetInputVolume",
            json!({ "inputName": name, "inputVolumeDb": next }),
        );
    }

    fn toggle_mute(&mut self, context: &str) {
        let Some(name) = self
            .dials
            .get(context)
            .map(|dial| dial.settings.input.clone())
        else {
            return;
        };
        if let Some(input) = self.inputs.get_mut(&name) {
            input.muted = input.muted.map(|muted| !muted);
        }
        self.obs_request(
            "ToggleInputMute",
            "ToggleInputMute",
            json!({ "inputName": name }),
        );
    }

    fn on_obs_event(&mut self, event: &Value) {
        let data = &event["eventData"];
        self.on_key_status_event(event["eventType"].as_str().unwrap_or_default(), data);
        let name = data["inputName"].as_str().unwrap_or_default().to_string();
        match event["eventType"].as_str().unwrap_or_default() {
            "InputVolumeMeters" => {
                for (name, peaks) in obs::parse_meters(data) {
                    if let Some(input) = self.inputs.get_mut(&name) {
                        input.peaks_db = peaks;
                    }
                }
            }
            "InputVolumeChanged" if self.is_tracked(&name) => {
                self.inputs.entry(name).or_default().volume_db =
                    Some(read_db(&data["inputVolumeDb"]));
            }
            "InputMuteStateChanged" if self.is_tracked(&name) => {
                self.inputs.entry(name).or_default().muted = data["inputMuted"].as_bool();
            }
            "ReplayBufferStateChanged" => {
                self.replay_status =
                    replay::Status::Active(data["outputActive"].as_bool().unwrap_or(false));
            }
            // Saved from any key or hotkey: confirm it on keys set to confirm every save, and on
            // keys that asked for this one.
            "ReplayBufferSaved" => {
                for (context, key) in &mut self.replay_keys {
                    if key.settings.confirms_every_save() || key.awaiting_save {
                        let _ = self.streamdeck.send(streamdeck::show_ok(context));
                    }
                    key.awaiting_save = false;
                }
            }
            "InputNameChanged" => {
                let old = data["oldInputName"].as_str().unwrap_or_default();
                self.rename_key_targets(KeyAction::Mute, old, &name);
                for (context, dial) in &mut self.dials {
                    if dial.settings.input == old {
                        dial.settings.input = name.clone();
                        let _ = self
                            .streamdeck
                            .send(streamdeck::set_settings(context, json!(dial.settings)));
                    }
                }
                if let Some(input) = self.inputs.remove(old) {
                    self.inputs.insert(name, input);
                }
            }
            _ => {}
        }
    }

    fn on_obs_response(&mut self, response: &Value) {
        let request_type = response["requestType"].as_str().unwrap_or_default();
        let request_id = response["requestId"].as_str().unwrap_or_default();
        let data = &response["responseData"];
        let status = &response["requestStatus"];
        if self.on_key_response(request_type, request_id, status["result"] == true, data) {
            return;
        }
        let subject = request_id.split_once(':').map(|(_, rest)| rest.to_string());
        if status["result"] != true {
            // NotReady is expected while OBS starts up; the next retry asks again.
            let not_ready = status["code"] == OBS_NOT_READY;
            match (request_type, subject) {
                ("SaveReplayBuffer" | "ToggleReplayBuffer", Some(context)) => {
                    let _ = self.streamdeck.send(streamdeck::show_alert(&context));
                }
                ("GetReplayBufferStatus", _) if !not_ready => {
                    self.replay_status = replay::Status::Unavailable;
                }
                _ => {}
            }
            let failure = format!("OBS {request_type} failed: {status}");
            if !not_ready && self.logged_failures.insert(failure.clone()) {
                log::write(&failure);
            }
            return;
        }
        match (request_type, subject) {
            ("GetReplayBufferStatus", _) => {
                self.replay_status =
                    replay::Status::Active(data["outputActive"].as_bool().unwrap_or(false));
            }
            ("SaveReplayBuffer", Some(context)) => {
                if let Some(key) = self.replay_keys.get_mut(&context) {
                    key.awaiting_save = true;
                }
            }
            ("GetInputVolume", Some(name)) => {
                self.inputs.entry(name).or_default().volume_db =
                    Some(read_db(&data["inputVolumeDb"]));
            }
            ("GetInputMute", Some(name)) => {
                self.inputs.entry(name).or_default().muted = data["inputMuted"].as_bool();
            }
            ("GetInputList", Some(context)) => {
                let mut names: Vec<&str> = data["inputs"]
                    .as_array()
                    .map(|inputs| {
                        inputs
                            .iter()
                            .filter_map(|input| input["inputName"].as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                names.sort_unstable();
                let action = if self.keys.is_mute_key(&context) {
                    keys::MUTE_UUID
                } else {
                    ACTION_UUID
                };
                let _ = self.streamdeck.send(streamdeck::send_to_property_inspector(
                    action,
                    &context,
                    json!({ "inputs": names }),
                ));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(input: &str) -> Settings {
        Settings {
            input: input.into(),
            ..Settings::default()
        }
    }

    #[test]
    fn settings_default_and_sanitise() {
        assert_eq!(Settings::from_payload(&json!({})), Settings::default());
        let parsed = Settings::from_payload(&json!({ "settings": {
            "input": " Mic/Aux ", "step": -1, "label": "  ", "floor": 0, "falloff": 1, "press": "explode",
        }}));
        assert_eq!(parsed, settings("Mic/Aux"));
        let parsed = Settings::from_payload(&json!({ "settings": {
            "input": "Desktop Audio", "step": 1.5, "label": " Game ", "floor": -40,
            "falloff": 300, "press": "none",
        }}));
        assert_eq!(parsed.step, 1.5);
        assert_eq!(parsed.label, "Game");
        assert_eq!(parsed.floor, -40.0);
        assert_eq!(parsed.falloff, 300.0);
        assert!(!parsed.mutes_on_press());
        assert!(Settings::default().mutes_on_press());
    }

    #[test]
    fn view_when_offline_or_unconfigured() {
        let offline = view(&settings("Mic/Aux"), None, [0.0; 2], false);
        assert_eq!(
            (offline.title.as_str(), offline.value.as_str()),
            ("Mic/Aux", "OBS off")
        );
        assert_eq!(
            view(&Settings::default(), None, [0.0; 2], true).title,
            "Set an input"
        );
    }

    #[test]
    fn view_shows_level_volume_and_mute() {
        let mut input = Input {
            volume_db: Some(-30.0),
            ..Input::default()
        };
        let live = view(&settings("Mic/Aux"), Some(&input), [50.4, 49.6], true);
        assert_eq!(
            live,
            View {
                title: "Mic/Aux".into(),
                value: "-30.0 dB".into(),
                value_color: WHITE,
                meters: [50, 50],
                fader: 50
            }
        );
        input.muted = Some(true);
        let muted = view(&settings("Mic/Aux"), Some(&input), [0.0; 2], true);
        assert_eq!((muted.value.as_str(), muted.value_color), ("Muted", RED));
    }

    #[test]
    fn view_uses_the_label_and_floor() {
        let input = Input {
            volume_db: Some(-20.0),
            ..Input::default()
        };
        let dial = Settings {
            label: "Mic".into(),
            floor: -40.0,
            ..settings("Mic/Aux")
        };
        let shown = view(&dial, Some(&input), [0.0; 2], true);
        assert_eq!(shown.title, "Mic");
        assert_eq!(shown.fader, 50);
    }

    #[test]
    fn meter_dial_press_mutes_unless_set_to_nothing() {
        let (streamdeck, _streamdeck_rx) = tokio::sync::mpsc::unbounded_channel();
        let (obs_tx, mut obs_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(streamdeck);
        app.on_obs(ObsEvent::Connected(obs_tx));
        for (context, press) in [("mutes", "mute"), ("ignores", "none")] {
            app.on_streamdeck(json!({
                "event": "willAppear",
                "context": context,
                "payload": { "settings": { "input": "Mic/Aux", "press": press } },
            }));
        }
        request_types(&mut obs_rx);

        app.on_streamdeck(json!({ "event": "dialDown", "context": "ignores" }));
        assert!(request_types(&mut obs_rx).is_empty());
        app.on_streamdeck(json!({ "event": "touchTap", "context": "mutes" }));
        assert_eq!(request_types(&mut obs_rx), ["ToggleInputMute"]);
    }

    #[test]
    fn changes_only_sends_what_moved() {
        let mut sent = Sent::default();
        let first = View {
            title: "Mic".into(),
            value: "-3.0 dB".into(),
            value_color: WHITE,
            meters: [10, 12],
            fader: 95,
        };
        let payload = changes(&first, &mut sent).unwrap();
        assert_eq!(payload["value"]["value"], "-3.0 dB");
        assert_eq!(payload["meterR"], 12);

        assert!(changes(&first, &mut sent).is_none());

        let moved = View {
            meters: [40, 12],
            ..first
        };
        let payload = changes(&moved, &mut sent).unwrap();
        assert_eq!(payload.as_object().unwrap().len(), 2);
        assert_eq!(payload["meterL"], 40);
    }

    #[test]
    fn read_db_treats_null_as_silence() {
        assert_eq!(read_db(&json!(-6.5)), -6.5);
        assert_eq!(read_db(&Value::Null), f32::NEG_INFINITY);
    }

    fn request_types(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|text| {
                let msg: Value = serde_json::from_str(&text).unwrap();
                msg["d"]["requestId"].as_str().unwrap().to_string()
            })
            .collect()
    }

    fn response(request_type: &str, request_id: &str, code: i64, data: Value) -> ObsEvent {
        ObsEvent::Message(json!({ "op": 7, "d": {
            "requestType": request_type,
            "requestId": request_id,
            "requestStatus": { "result": code == 100, "code": code },
            "responseData": data,
        }}))
    }

    #[test]
    fn retries_inputs_obs_was_not_ready_for() {
        let (streamdeck, _streamdeck_rx) = tokio::sync::mpsc::unbounded_channel();
        let (obs_tx, mut obs_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(streamdeck);
        app.on_streamdeck(json!({
            "event": "willAppear",
            "context": "dial-1",
            "payload": { "settings": { "input": "Mic/Aux" } },
        }));
        app.on_obs(ObsEvent::Connected(obs_tx));
        assert_eq!(
            request_types(&mut obs_rx),
            ["GetInputVolume:Mic/Aux", "GetInputMute:Mic/Aux"]
        );

        // OBS is still starting: both requests come back NotReady.
        app.on_obs(response(
            "GetInputVolume",
            "GetInputVolume:Mic/Aux",
            207,
            Value::Null,
        ));
        app.on_obs(response(
            "GetInputMute",
            "GetInputMute:Mic/Aux",
            207,
            Value::Null,
        ));

        let start = Instant::now();
        app.retry_unknown_state(start);
        assert!(
            request_types(&mut obs_rx).is_empty(),
            "waits for the retry interval"
        );

        app.retry_unknown_state(start + Duration::from_secs(2));
        assert_eq!(
            request_types(&mut obs_rx),
            ["GetInputVolume:Mic/Aux", "GetInputMute:Mic/Aux"]
        );

        app.on_obs(response(
            "GetInputVolume",
            "GetInputVolume:Mic/Aux",
            100,
            json!({ "inputVolumeDb": -4.0 }),
        ));
        app.retry_unknown_state(start + Duration::from_secs(4));
        assert!(
            request_types(&mut obs_rx).is_empty(),
            "stops once the volume is known"
        );
    }

    /// `(event, context)` of each message sent to Stream Deck.
    fn streamdeck_events(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    ) -> Vec<(String, String)> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|text| {
                let msg: Value = serde_json::from_str(&text).unwrap();
                (
                    msg["event"].as_str().unwrap().to_string(),
                    msg["context"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    fn replay_key_app() -> (
        App,
        tokio::sync::mpsc::UnboundedReceiver<String>,
        tokio::sync::mpsc::UnboundedReceiver<String>,
    ) {
        let (streamdeck, streamdeck_rx) = tokio::sync::mpsc::unbounded_channel();
        let (obs_tx, mut obs_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(streamdeck);
        app.on_streamdeck(
            json!({ "event": "willAppear", "action": REPLAY_UUID, "context": "key-1" }),
        );
        app.on_obs(ObsEvent::Connected(obs_tx));
        assert_eq!(request_types(&mut obs_rx), ["GetReplayBufferStatus"]);
        (app, streamdeck_rx, obs_rx)
    }

    #[test]
    fn replay_key_saves_on_tap_and_toggles_on_hold() {
        let (mut app, _streamdeck_rx, mut obs_rx) = replay_key_app();
        let start = Instant::now();

        app.on_replay_key("keyDown", "key-1".into(), &Value::Null, start);
        app.on_replay_key(
            "keyUp",
            "key-1".into(),
            &Value::Null,
            start + Duration::from_millis(200),
        );
        assert_eq!(request_types(&mut obs_rx), ["SaveReplayBuffer:key-1"]);

        app.on_replay_key("keyDown", "key-1".into(), &Value::Null, start);
        app.tick_replay_keys(start + Duration::from_millis(300));
        assert!(
            request_types(&mut obs_rx).is_empty(),
            "not held long enough yet"
        );
        app.tick_replay_keys(start + Duration::from_millis(550));
        assert_eq!(request_types(&mut obs_rx), ["ToggleReplayBuffer:key-1"]);
        app.on_replay_key(
            "keyUp",
            "key-1".into(),
            &Value::Null,
            start + Duration::from_secs(1),
        );
        assert!(
            request_types(&mut obs_rx).is_empty(),
            "release after a hold does nothing"
        );
    }

    #[test]
    fn replay_key_image_follows_the_buffer() {
        let (mut app, mut streamdeck_rx, _obs_rx) = replay_key_app();
        let image = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>| {
            std::iter::from_fn(|| rx.try_recv().ok())
                .map(|text| serde_json::from_str::<Value>(&text).unwrap())
                .filter(|msg| msg["event"] == "setImage")
                .map(|msg| msg["payload"]["image"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        let now = Instant::now();

        app.on_obs(response(
            "GetReplayBufferStatus",
            "GetReplayBufferStatus",
            100,
            json!({ "outputActive": true }),
        ));
        app.tick_replay_keys(now);
        assert_eq!(
            image(&mut streamdeck_rx),
            [replay::image(replay::Look::BufferOn, "Save Replay")]
        );
        app.tick_replay_keys(now);
        assert!(
            image(&mut streamdeck_rx).is_empty(),
            "unchanged image isn't resent"
        );

        app.on_obs(ObsEvent::Message(json!({ "op": 5, "d": {
            "eventType": "ReplayBufferStateChanged",
            "eventData": { "outputActive": false, "outputState": "OBS_WEBSOCKET_OUTPUT_STOPPED" },
        }})));
        app.tick_replay_keys(now);
        assert_eq!(
            image(&mut streamdeck_rx),
            [replay::image(replay::Look::BufferOff, "Save Replay")]
        );

        app.on_obs(ObsEvent::Disconnected);
        app.tick_replay_keys(now);
        assert_eq!(
            image(&mut streamdeck_rx),
            [replay::image(replay::Look::ObsOff, "Save Replay")]
        );
    }

    #[test]
    fn replay_key_confirms_saves_and_flags_failures() {
        let (mut app, mut streamdeck_rx, _obs_rx) = replay_key_app();
        app.on_obs(response(
            "SaveReplayBuffer",
            "SaveReplayBuffer:key-1",
            501,
            Value::Null,
        ));
        app.on_obs(ObsEvent::Message(json!({ "op": 5, "d": {
            "eventType": "ReplayBufferSaved",
            "eventData": { "savedReplayPath": "E:/OBS Recordings/Replay.mkv" },
        }})));
        assert_eq!(
            streamdeck_events(&mut streamdeck_rx),
            [
                ("showAlert".to_string(), "key-1".to_string()),
                ("showOk".to_string(), "key-1".to_string())
            ]
        );
    }

    #[test]
    fn replay_status_retries_while_obs_starts_and_stops_if_unavailable() {
        let (mut app, _streamdeck_rx, mut obs_rx) = replay_key_app();
        let start = Instant::now();
        app.on_obs(response(
            "GetReplayBufferStatus",
            "GetReplayBufferStatus",
            207,
            Value::Null,
        ));
        app.retry_unknown_state(start + Duration::from_secs(2));
        assert_eq!(request_types(&mut obs_rx), ["GetReplayBufferStatus"]);

        // 604 InvalidResourceState: the replay buffer is switched off in OBS's settings.
        app.on_obs(response(
            "GetReplayBufferStatus",
            "GetReplayBufferStatus",
            604,
            Value::Null,
        ));
        assert_eq!(app.replay_status, replay::Status::Unavailable);
        app.retry_unknown_state(start + Duration::from_secs(4));
        assert!(request_types(&mut obs_rx).is_empty());
    }

    #[test]
    fn replay_key_alerts_when_obs_is_closed() {
        let (streamdeck, mut streamdeck_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(streamdeck);
        app.on_streamdeck(
            json!({ "event": "willAppear", "action": REPLAY_UUID, "context": "key-1" }),
        );
        let start = Instant::now();
        app.on_replay_key("keyDown", "key-1".into(), &Value::Null, start);
        app.on_replay_key(
            "keyUp",
            "key-1".into(),
            &Value::Null,
            start + Duration::from_millis(100),
        );
        assert_eq!(
            streamdeck_events(&mut streamdeck_rx),
            [("showAlert".to_string(), "key-1".to_string())]
        );
    }

    #[test]
    fn replay_key_settings_change_the_hold_and_label() {
        let (mut app, mut streamdeck_rx, mut obs_rx) = replay_key_app();
        app.on_streamdeck(json!({
            "event": "didReceiveSettings", "action": REPLAY_UUID, "context": "key-1",
            "payload": { "settings": { "holdAction": "none", "label": "Clip it" } },
        }));
        let start = Instant::now();
        app.on_replay_key("keyDown", "key-1".into(), &Value::Null, start);
        app.tick_replay_keys(start + Duration::from_secs(2));
        app.on_replay_key(
            "keyUp",
            "key-1".into(),
            &Value::Null,
            start + Duration::from_secs(2),
        );
        assert_eq!(
            request_types(&mut obs_rx),
            ["SaveReplayBuffer:key-1"],
            "with no hold action a long press still saves"
        );
        let images: Vec<String> = std::iter::from_fn(|| streamdeck_rx.try_recv().ok())
            .map(|text| serde_json::from_str::<Value>(&text).unwrap())
            .filter(|msg| msg["event"] == "setImage")
            .map(|msg| msg["payload"]["image"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(images, [replay::image(replay::Look::BufferOff, "Clip it")]);
    }

    #[test]
    fn replay_key_set_to_own_saves_confirms_only_its_own() {
        let (mut app, mut streamdeck_rx, _obs_rx) = replay_key_app();
        app.on_streamdeck(json!({
            "event": "didReceiveSettings", "action": REPLAY_UUID, "context": "key-1",
            "payload": { "settings": { "confirm": "own" } },
        }));
        let saved = || {
            ObsEvent::Message(json!({ "op": 5, "d": {
                "eventType": "ReplayBufferSaved", "eventData": { "savedReplayPath": "x.mkv" },
            }}))
        };
        app.on_obs(saved());
        assert!(
            streamdeck_events(&mut streamdeck_rx).is_empty(),
            "a hotkey save isn't confirmed"
        );

        app.on_obs(response(
            "SaveReplayBuffer",
            "SaveReplayBuffer:key-1",
            100,
            json!({}),
        ));
        app.on_obs(saved());
        assert_eq!(
            streamdeck_events(&mut streamdeck_rx),
            [("showOk".to_string(), "key-1".to_string())]
        );
        app.on_obs(saved());
        assert!(streamdeck_events(&mut streamdeck_rx).is_empty());
    }

    fn sample() -> stats::Sample {
        stats::Sample {
            cpu: Some(12.0),
            gpus: HashMap::from([("luid_0x00000000_0x00012d26".to_string(), 80.0)]),
            memory: Some(40.0),
        }
    }

    #[test]
    fn stats_dial_shows_the_last_sample_when_it_appears() {
        let (streamdeck, mut streamdeck_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(streamdeck);
        app.sample = Some(sample());
        app.on_streamdeck(
            json!({ "event": "willAppear", "action": STATS_UUID, "context": "dial-4" }),
        );
        let sent: Value = serde_json::from_str(&streamdeck_rx.try_recv().unwrap()).unwrap();
        assert_eq!(sent["event"], "setFeedback");
        assert_eq!(sent["context"], "dial-4");
        assert_eq!(sent["payload"]["row2Value"]["value"], "80%");

        app.on_streamdeck(
            json!({ "event": "willDisappear", "action": STATS_UUID, "context": "dial-4" }),
        );
        assert!(app.stats_dials.is_empty());
    }

    #[test]
    fn stats_dials_update_on_their_own_interval() {
        let (streamdeck, mut streamdeck_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(streamdeck);
        app.sample = Some(sample());
        for (context, interval) in [("fast", 1.0), ("slow", 3.0)] {
            app.on_streamdeck(json!({
                "event": "willAppear", "action": STATS_UUID, "context": context,
                "payload": { "settings": { "interval": interval } },
            }));
        }
        streamdeck_events(&mut streamdeck_rx);
        let start = Instant::now();
        for dial in app.stats_dials.values_mut() {
            dial.last_sent = Some(start);
            dial.sent = None;
        }
        let due = |app: &mut App, at: Instant| {
            let mut contexts: Vec<String> = app
                .stats_dials
                .iter_mut()
                .filter_map(|(context, dial)| {
                    let due = dial.last_sent.is_none_or(|sent| {
                        at.duration_since(sent) + STATS_TOLERANCE >= dial.settings.interval()
                    });
                    due.then(|| context.clone())
                })
                .collect();
            contexts.sort();
            contexts
        };
        assert!(due(&mut app, start + Duration::from_millis(500)).is_empty());
        assert_eq!(due(&mut app, start + Duration::from_millis(950)), ["fast"]);
        assert_eq!(
            due(&mut app, start + Duration::from_secs(3)),
            ["fast", "slow"]
        );
    }

    #[test]
    fn output_key_without_a_device_shows_not_connected_and_alerts() {
        let (streamdeck, mut streamdeck_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(streamdeck);
        app.on_streamdeck(json!({
            "event": "willAppear", "action": OUTPUT_UUID, "context": "out-1",
            "payload": { "settings": { "deviceId": "{0.0.0.00000000}.{x}", "label": "EVO16" } },
        }));
        app.tick_output_keys();
        let sent: Value = serde_json::from_str(&streamdeck_rx.try_recv().unwrap()).unwrap();
        assert_eq!(sent["event"], "setImage");
        assert_eq!(
            sent["payload"]["image"],
            audio::image(audio::Look::Missing, "EVO16", false)
        );
        app.tick_output_keys();
        assert!(
            streamdeck_rx.try_recv().is_err(),
            "unchanged image isn't resent"
        );

        app.on_streamdeck(json!({ "event": "keyDown", "action": OUTPUT_UUID, "context": "out-1" }));
        assert_eq!(
            streamdeck_events(&mut streamdeck_rx),
            [("showAlert".to_string(), "out-1".to_string())]
        );
    }

    #[test]
    fn stats_property_inspector_gets_the_gpu_list() {
        let (streamdeck, mut streamdeck_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(streamdeck);
        app.on_streamdeck(json!({
            "event": "propertyInspectorDidAppear", "action": STATS_UUID, "context": "dial-4",
        }));
        let sent: Value = serde_json::from_str(&streamdeck_rx.try_recv().unwrap()).unwrap();
        assert_eq!(sent["event"], "sendToPropertyInspector");
        assert!(sent["payload"]["gpus"].is_array());
    }
}
