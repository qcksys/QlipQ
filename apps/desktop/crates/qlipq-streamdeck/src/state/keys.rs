use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use super::App;
use crate::{replay, streamdeck};

pub(super) const MUTE_UUID: &str = "com.qcksys.qlipq.meters.mute";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyAction {
    Scene,
    Record,
    Pause,
    Chapter,
    Buffer,
    Mute,
}

impl KeyAction {
    pub(super) fn from_uuid(uuid: &str) -> Option<Self> {
        match uuid.strip_prefix("com.qcksys.qlipq.meters.")? {
            "scene" => Some(Self::Scene),
            "record" => Some(Self::Record),
            "pause" => Some(Self::Pause),
            "chapter" => Some(Self::Chapter),
            "buffer" => Some(Self::Buffer),
            "mute" => Some(Self::Mute),
            _ => None,
        }
    }

    fn target_field(self) -> Option<&'static str> {
        match self {
            Self::Mute => Some("input"),
            Self::Scene => Some("scene"),
            _ => None,
        }
    }
}

struct Key {
    kind: KeyAction,
    target: String,
    shown: Option<(String, bool)>,
}

struct Scenes {
    program: String,
    preview: Option<String>,
}

#[derive(Default)]
pub(super) struct ObsKeys {
    visible: HashMap<String, Key>,
    // (active, paused) from GetRecordStatus or RecordStateChanged.
    recording: Option<(bool, bool)>,
    scenes: Option<Scenes>,
}

impl ObsKeys {
    pub(super) fn reset_status(&mut self) {
        self.recording = None;
        self.scenes = None;
    }

    pub(super) fn tracks_input(&self, name: &str) -> bool {
        self.visible
            .values()
            .any(|key| key.kind == KeyAction::Mute && key.target == name)
    }

    pub(super) fn is_mute_key(&self, context: &str) -> bool {
        self.visible
            .get(context)
            .is_some_and(|key| key.kind == KeyAction::Mute)
    }
}

impl App {
    pub(super) fn on_obs_key(
        &mut self,
        kind: KeyAction,
        event: &str,
        context: String,
        payload: &Value,
    ) {
        match event {
            "willAppear" | "didReceiveSettings" => {
                let target = kind
                    .target_field()
                    .and_then(|field| payload["settings"][field].as_str())
                    .unwrap_or_default()
                    .to_owned();
                self.keys.visible.insert(
                    context,
                    Key {
                        kind,
                        target: target.clone(),
                        shown: None,
                    },
                );
                match kind {
                    KeyAction::Buffer => self.request_replay_status(),
                    KeyAction::Record | KeyAction::Pause | KeyAction::Chapter => {
                        self.request_record_status()
                    }
                    KeyAction::Scene => self.request_scene_list("GetSceneList"),
                    KeyAction::Mute if !target.is_empty() => self.request_mute_status(&target),
                    _ => {}
                }
            }
            "willDisappear" => {
                self.keys.visible.remove(&context);
            }
            "sendToPlugin" if payload["listTargets"] == true => match kind {
                KeyAction::Scene => self.request_scene_list(&format!("scene-picker:{context}")),
                KeyAction::Mute => self.obs_request(
                    "GetInputList",
                    &format!("GetInputList:{context}"),
                    json!({}),
                ),
                _ => {}
            },
            "keyDown" => self.press_obs_key(&context),
            _ => {}
        }
    }

    fn press_obs_key(&self, context: &str) {
        let Some(key) = self.keys.visible.get(context) else {
            return;
        };
        let alert = || {
            let _ = self.streamdeck.send(streamdeck::show_alert(context));
        };
        if self.obs.is_none() {
            return alert();
        }
        let (request, data) = match key.kind {
            KeyAction::Record => ("ToggleRecord", json!({})),
            KeyAction::Pause => ("ToggleRecordPause", json!({})),
            // With no custom chapter name, OBS numbers markers in the current recording.
            KeyAction::Chapter => ("CreateRecordChapter", json!({})),
            KeyAction::Buffer => return self.replay_command(context, replay::Command::Toggle),
            KeyAction::Mute if !key.target.is_empty() => {
                ("ToggleInputMute", json!({ "inputName": key.target }))
            }
            KeyAction::Scene if !key.target.is_empty() => {
                let Some(scenes) = &self.keys.scenes else {
                    return alert();
                };
                let (request, current) = match &scenes.preview {
                    Some(preview) => ("SetCurrentPreviewScene", preview),
                    None => ("SetCurrentProgramScene", &scenes.program),
                };
                if current == &key.target {
                    return;
                }
                (request, json!({ "sceneName": key.target }))
            }
            _ => return alert(),
        };
        self.obs_request(request, &format!("key:{context}"), data);
    }

    fn request_mute_status(&self, name: &str) {
        self.obs_request(
            "GetInputMute",
            &format!("GetInputMute:{name}"),
            json!({ "inputName": name }),
        );
    }

    fn request_record_status(&self) {
        self.obs_request("GetRecordStatus", "GetRecordStatus", json!({}));
    }

    fn request_scene_list(&self, id: &str) {
        self.obs_request("GetSceneList", id, json!({}));
    }

    pub(super) fn has_buffer_keys(&self) -> bool {
        !self.replay_keys.is_empty()
            || self
                .keys
                .visible
                .values()
                .any(|key| key.kind == KeyAction::Buffer)
    }

    pub(super) fn sync_obs_keys(&self) {
        let keys = self.keys.visible.values();
        if self.keys.recording.is_none()
            && keys.clone().any(|k| {
                matches!(
                    k.kind,
                    KeyAction::Record | KeyAction::Pause | KeyAction::Chapter
                )
            })
        {
            self.request_record_status();
        }
        if self.keys.scenes.is_none() && keys.clone().any(|k| k.kind == KeyAction::Scene) {
            self.request_scene_list("GetSceneList");
        }
        let missing: HashSet<_> = keys
            .filter(|k| k.kind == KeyAction::Mute && !k.target.is_empty())
            .map(|k| k.target.as_str())
            .filter(|name| self.inputs.get(*name).and_then(|i| i.muted).is_none())
            .collect();
        for name in missing {
            self.request_mute_status(name);
        }
    }

    pub(super) fn rename_key_targets(&mut self, kind: KeyAction, old: &str, new: &str) {
        if old.is_empty() || new.is_empty() {
            return;
        }
        for (context, key) in &mut self.keys.visible {
            if key.kind == kind && key.target == old {
                key.target = new.into();
                let field = kind.target_field().expect("scene or input");
                let _ = self
                    .streamdeck
                    .send(streamdeck::set_settings(context, json!({(field): new})));
            }
        }
    }

    pub(super) fn on_key_status_event(&mut self, event: &str, data: &Value) {
        match event {
            "RecordStateChanged" => {
                self.keys.recording = data["outputActive"]
                    .as_bool()
                    .map(|active| (active, data["outputState"] == "OBS_WEBSOCKET_OUTPUT_PAUSED"));
            }
            "SceneNameChanged" => {
                self.rename_key_targets(
                    KeyAction::Scene,
                    data["oldSceneName"].as_str().unwrap_or_default(),
                    data["sceneName"].as_str().unwrap_or_default(),
                );
                self.refresh_scenes();
            }
            "StudioModeStateChanged" | "SceneListChanged" => self.refresh_scenes(),
            "CurrentProgramSceneChanged" | "CurrentPreviewSceneChanged" => {
                if let (Some(scenes), Some(name)) =
                    (&mut self.keys.scenes, data["sceneName"].as_str())
                {
                    if event == "CurrentProgramSceneChanged" {
                        scenes.program = name.into();
                    } else {
                        scenes.preview = Some(name.into());
                    }
                }
            }
            _ => {}
        }
    }

    fn refresh_scenes(&mut self) {
        self.keys.scenes = None;
        if self
            .keys
            .visible
            .values()
            .any(|k| k.kind == KeyAction::Scene)
        {
            self.request_scene_list("GetSceneList");
        }
    }

    pub(super) fn on_key_response(
        &mut self,
        request: &str,
        id: &str,
        ok: bool,
        data: &Value,
    ) -> bool {
        if let Some(context) = id.strip_prefix("key:") {
            if self.keys.visible.contains_key(context) {
                if !ok {
                    let _ = self.streamdeck.send(streamdeck::show_alert(context));
                } else if request == "CreateRecordChapter" {
                    let _ = self.streamdeck.send(streamdeck::show_ok(context));
                }
            }
            return true;
        }
        if !ok {
            return false;
        }
        match request {
            "GetRecordStatus" => {
                self.keys.recording = data["outputActive"]
                    .as_bool()
                    .zip(data["outputPaused"].as_bool());
            }
            "GetSceneList" => {
                self.keys.scenes = data["currentProgramSceneName"]
                    .as_str()
                    .map(|program| Scenes {
                        program: program.into(),
                        preview: data["currentPreviewSceneName"].as_str().map(String::from),
                    });
                if let Some(context) = id.strip_prefix("scene-picker:") {
                    let names: Vec<_> = data["scenes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s["sceneName"].as_str())
                        .collect();
                    let _ = self.streamdeck.send(streamdeck::send_to_property_inspector(
                        "com.qcksys.qlipq.meters.scene",
                        context,
                        json!({ "scenes": names }),
                    ));
                }
            }
            _ => return false,
        }
        true
    }

    pub(super) fn draw_obs_keys(&mut self) {
        for (context, key) in &mut self.keys.visible {
            let (recording, paused) = self.keys.recording.unwrap_or_default();
            let (title, active) = match key.kind {
                KeyAction::Record => (if recording { "Recording" } else { "Record" }, recording),
                KeyAction::Pause => (if paused { "Resume" } else { "Pause" }, paused),
                KeyAction::Chapter => ("Mark", recording),
                KeyAction::Buffer => ("Buffer", self.replay_status == replay::Status::Active(true)),
                KeyAction::Scene => {
                    let current = self
                        .keys
                        .scenes
                        .as_ref()
                        .map(|s| s.preview.as_ref().unwrap_or(&s.program));
                    (
                        if key.target.is_empty() {
                            "Set scene"
                        } else {
                            &key.target[..]
                        },
                        current == Some(&key.target),
                    )
                }
                KeyAction::Mute => {
                    let muted = self.inputs.get(&key.target).and_then(|i| i.muted);
                    (
                        if key.target.is_empty() {
                            "Set input"
                        } else {
                            &key.target[..]
                        },
                        muted == Some(true),
                    )
                }
            };
            let (title, active) = if self.obs.is_none() {
                ("OBS off", false)
            } else {
                (title, active)
            };
            if key
                .shown
                .as_ref()
                .is_some_and(|(old, state)| old == title && *state == active)
            {
                continue;
            }
            key.shown = Some((title.to_owned(), active));
            for (event, payload) in [
                ("setState", json!({"state": u8::from(active)})),
                ("setTitle", json!({"title": title, "target": 0})),
            ] {
                let _ = self
                    .streamdeck
                    .send(json!({"event":event,"context":context,"payload":payload}).to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::ObsEvent;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    struct Rig {
        app: App,
        obs: UnboundedReceiver<String>,
        deck: UnboundedReceiver<String>,
    }

    fn messages(rx: &mut UnboundedReceiver<String>) -> Vec<Value> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|s| serde_json::from_str(&s).unwrap())
            .collect()
    }

    impl Rig {
        fn new() -> Self {
            let (sd, deck) = unbounded_channel();
            let (tx, obs) = unbounded_channel();
            let mut app = App::new(sd);
            app.on_obs(ObsEvent::Connected(tx));
            Self { app, obs, deck }
        }

        fn event(&mut self, key: &str, action: &str, event: &str, settings: Value) {
            self.app.on_streamdeck(json!({
                "action":format!("com.qcksys.qlipq.meters.{action}"),"event":event,
                "context":key, "payload":{"settings": settings}
            }));
        }

        fn response(&mut self, request: &str, id: &str, result: bool, data: Value) {
            self.app.on_obs(ObsEvent::Message(json!({"op":7,"d":{
                "requestType":request, "requestId":id, "requestStatus":{"result":result}, "responseData":data
            }})));
        }

        fn obs_event(&mut self, event: &str, data: Value) {
            self.app.on_obs(ObsEvent::Message(
                json!({"op":5,"d":{"eventType":event,"eventData":data}}),
            ));
        }

        fn press(&mut self, key: &str, action: &str) -> Vec<Value> {
            messages(&mut self.obs);
            for event in ["keyDown", "keyUp"] {
                self.event(key, action, event, Value::Null);
            }
            messages(&mut self.obs)
                .into_iter()
                .map(|m| m["d"].clone())
                .collect()
        }

        fn states(&mut self) -> HashMap<String, Value> {
            self.app.tick();
            messages(&mut self.deck)
                .into_iter()
                .filter(|m| m["event"] == "setState")
                .map(|m| {
                    (
                        m["context"].as_str().unwrap().into(),
                        m["payload"]["state"].clone(),
                    )
                })
                .collect()
        }
    }

    #[test]
    fn record_pause_chapter_and_buffer_each_send_one_command_per_press() {
        let mut rig = Rig::new();
        for (key, request) in [
            ("record", "ToggleRecord"),
            ("pause", "ToggleRecordPause"),
            ("chapter", "CreateRecordChapter"),
            ("buffer", "ToggleReplayBuffer"),
        ] {
            rig.event(key, key, "willAppear", json!({}));
            let commands = rig.press(key, key);
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0]["requestType"], request);
            assert_eq!(commands[0]["requestData"], json!({}));
        }
        rig.response("CreateRecordChapter", "key:chapter", true, json!({}));
        rig.response("ToggleRecordPause", "key:pause", false, json!({}));
        let feedback = messages(&mut rig.deck);
        assert_eq!(feedback[0]["event"], "showOk");
        assert_eq!(feedback[0]["context"], "chapter");
        assert_eq!(feedback[1]["event"], "showAlert");
        assert_eq!(feedback[1]["context"], "pause");
        rig.obs_event(
            "RecordStateChanged",
            json!({"outputActive":true,"outputState":"OBS_WEBSOCKET_OUTPUT_PAUSED"}),
        );
        rig.obs_event("ReplayBufferStateChanged", json!({"outputActive":true}));
        let states = rig.states();
        for key in ["record", "pause", "chapter", "buffer"] {
            assert_eq!(states[key], 1);
        }
        rig.app.on_obs(ObsEvent::Disconnected);
        assert!(rig.states().values().all(|v| *v == 0));
        assert!(rig.press("record", "record").is_empty());
        assert_eq!(messages(&mut rig.deck)[0]["event"], "showAlert");
        let (new_obs, mut requests) = unbounded_channel();
        rig.app.on_obs(ObsEvent::Connected(new_obs));
        let queries = messages(&mut requests);
        assert!(queries
            .iter()
            .any(|q| q["d"]["requestType"] == "GetRecordStatus"));
        assert!(queries
            .iter()
            .any(|q| q["d"]["requestType"] == "GetReplayBufferStatus"));
    }

    #[test]
    fn scene_keys_follow_preview_and_only_switch_program_outside_studio_mode() {
        let mut rig = Rig::new();
        for name in ["Streaming", "Recording"] {
            rig.event(name, "scene", "willAppear", json!({"scene":name}));
        }
        assert!(
            rig.press("Recording", "scene").is_empty(),
            "wait for the studio mode snapshot"
        );
        rig.response(
            "GetSceneList",
            "GetSceneList",
            true,
            json!({"currentProgramSceneName":"Recording","currentPreviewSceneName":"Streaming"}),
        );
        assert_eq!(rig.states()["Streaming"], 1);
        let commands = rig.press("Recording", "scene");
        assert_eq!(commands[0]["requestType"], "SetCurrentPreviewScene");
        assert_eq!(commands[0]["requestData"], json!({"sceneName":"Recording"}));
        assert!(rig.press("Streaming", "scene").is_empty());
        rig.obs_event(
            "CurrentPreviewSceneChanged",
            json!({"sceneName":"Recording"}),
        );
        assert_eq!(rig.states()["Recording"], 1);
        rig.obs_event("StudioModeStateChanged", json!({"studioModeEnabled":false}));
        assert!(rig.press("Streaming", "scene").is_empty());
        rig.response(
            "GetSceneList",
            "GetSceneList",
            true,
            json!({"currentProgramSceneName":"Recording","currentPreviewSceneName":null}),
        );
        assert_eq!(
            rig.press("Streaming", "scene")[0]["requestType"],
            "SetCurrentProgramScene"
        );
    }

    #[test]
    fn mute_keys_track_their_own_inputs_and_follow_obs_renames() {
        let mut rig = Rig::new();
        rig.event("mic", "mute", "willAppear", json!({"input":"Mic/Aux"}));
        rig.event("pc", "mute", "willAppear", json!({"input":"Desktop Audio"}));
        assert_eq!(
            rig.press("mic", "mute")[0]["requestData"],
            json!({"inputName":"Mic/Aux"})
        );
        rig.response(
            "GetInputMute",
            "GetInputMute:Desktop Audio",
            true,
            json!({"inputMuted":true}),
        );
        rig.obs_event(
            "InputMuteStateChanged",
            json!({"inputName":"Mic/Aux","inputMuted":false}),
        );
        let states = rig.states();
        assert_eq!(states["mic"], 0);
        assert_eq!(states["pc"], 1);
        rig.obs_event(
            "InputNameChanged",
            json!({"oldInputName":"Mic/Aux","inputName":"Headset"}),
        );
        assert_eq!(
            rig.press("mic", "mute")[0]["requestData"],
            json!({"inputName":"Headset"})
        );
        rig.event("mic", "mute", "willDisappear", json!({}));
        assert!(rig.press("mic", "mute").is_empty());
    }
}
