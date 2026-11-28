//! The QlipQ keys and dials (`crate::qlipq`): what each shows, and the commands they send to QlipQ.

use std::collections::HashMap;

use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::App;
use crate::qlipq::{self, Action, DialKind, KeyKind, Line, QlipqEvent};
use crate::streamdeck;

struct Key {
    kind: KeyKind,
    settings: qlipq::KeySettings,
    shown: Option<qlipq::KeyLook>,
}

struct Dial {
    kind: DialKind,
    settings: qlipq::DialSettings,
    /// Clip picker: the queue entry the dial points at, by id.
    cursor: Option<String>,
    sent: Option<Value>,
}

/// The connection to QlipQ and the QlipQ actions on the deck.
#[derive(Default)]
pub(super) struct Qlipq {
    link: Option<UnboundedSender<String>>,
    state: qlipq::State,
    queue: qlipq::Queue,
    keys: HashMap<String, Key>,
    dials: HashMap<String, Dial>,
}

impl Qlipq {
    /// Where a clip picker's cursor sits in the queue, falling back to the open clip, then the first.
    fn cursor_index(&self, cursor: Option<&str>) -> Option<usize> {
        let position = |id: &str| self.queue.items.iter().position(|entry| entry.id == id);
        cursor
            .and_then(position)
            .or_else(|| self.queue.selected.as_deref().and_then(position))
            .or((!self.queue.items.is_empty()).then_some(0))
    }
}

impl App {
    pub fn on_qlipq(&mut self, event: QlipqEvent) {
        let qlipq = &mut self.qlipq;
        match event {
            QlipqEvent::Connected(link) => qlipq.link = Some(link),
            QlipqEvent::Disconnected => {
                qlipq.link = None;
                qlipq.state = qlipq::State::default();
                qlipq.queue = qlipq::Queue::default();
            }
            QlipqEvent::Line(value) => match qlipq::parse_line(&value) {
                Some(Line::State(state)) => qlipq.state = state,
                Some(Line::Queue(queue)) => {
                    // Cursors follow the open clip when it changes, and when their clip leaves the queue.
                    let selection_changed = queue.selected != qlipq.queue.selected;
                    for dial in qlipq.dials.values_mut() {
                        let gone = dial
                            .cursor
                            .as_ref()
                            .is_none_or(|id| !queue.items.iter().any(|entry| &entry.id == id));
                        if selection_changed || gone {
                            dial.cursor = queue.selected.clone();
                        }
                    }
                    qlipq.queue = queue;
                }
                None => {}
            },
        }
    }

    /// Handles a QlipQ action's Stream Deck event; returns false if `action` isn't a QlipQ action.
    pub(super) fn on_qlipq_action(
        &mut self,
        action: &str,
        event: &str,
        context: &str,
        payload: &Value,
    ) -> bool {
        let Some(action) = Action::from_uuid(action) else {
            return false;
        };
        let qlipq = &mut self.qlipq;
        match (action, event) {
            (Action::Key(kind), "willAppear" | "didReceiveSettings") => {
                qlipq.keys.insert(
                    context.to_string(),
                    Key {
                        kind,
                        settings: qlipq::KeySettings::from_payload(payload),
                        shown: None,
                    },
                );
            }
            (Action::Dial(kind), "willAppear" | "didReceiveSettings") => {
                let cursor = qlipq
                    .dials
                    .get(context)
                    .and_then(|dial| dial.cursor.clone())
                    .or_else(|| qlipq.queue.selected.clone());
                qlipq.dials.insert(
                    context.to_string(),
                    Dial {
                        kind,
                        settings: qlipq::DialSettings::from_payload(payload),
                        cursor,
                        sent: None,
                    },
                );
            }
            (_, "willDisappear") => {
                qlipq.keys.remove(context);
                qlipq.dials.remove(context);
            }
            (Action::Key(kind), "keyDown") => self.press_qlipq_key(context, kind),
            (Action::Dial(DialKind::Scrub), "dialRotate") => {
                let ticks = payload["ticks"].as_i64().unwrap_or(0) as i32;
                let command = qlipq
                    .dials
                    .get(context)
                    .map(|dial| dial.settings.scrub_command(ticks));
                if let Some(command) = command {
                    if qlipq.state.clip.is_some() {
                        self.send_qlipq(context, command);
                    }
                }
            }
            (Action::Dial(DialKind::Scrub), "dialDown" | "touchTap") => {
                let plays = qlipq
                    .dials
                    .get(context)
                    .is_some_and(|dial| dial.settings.plays_on_press());
                if plays {
                    if qlipq.state.clip.is_some() {
                        self.send_qlipq(context, qlipq::command("togglePlay"));
                    } else {
                        let _ = self.streamdeck.send(streamdeck::show_alert(context));
                    }
                }
            }
            (Action::Dial(DialKind::Picker), "dialRotate") => {
                let ticks = payload["ticks"].as_i64().unwrap_or(0) as i32;
                let count = qlipq.queue.items.len();
                let Some(dial) = qlipq.dials.get(context) else {
                    return true;
                };
                let Some(index) = qlipq.cursor_index(dial.cursor.as_deref()) else {
                    return true;
                };
                let next = qlipq::move_cursor(index, ticks, count, dial.settings.wrap);
                let id = qlipq.queue.items[next].id.clone();
                if let Some(dial) = qlipq.dials.get_mut(context) {
                    dial.cursor = Some(id);
                }
            }
            (Action::Dial(DialKind::Picker), "dialDown" | "touchTap") => {
                let id = qlipq.dials.get(context).and_then(|dial| {
                    let index = qlipq.cursor_index(dial.cursor.as_deref())?;
                    Some(qlipq.queue.items[index].id.clone())
                });
                match id {
                    Some(id) => {
                        self.send_qlipq(
                            context,
                            qlipq::command_with("select", json!({ "id": id })),
                        );
                    }
                    None => {
                        let _ = self.streamdeck.send(streamdeck::show_alert(context));
                    }
                }
            }
            _ => {}
        }
        true
    }

    /// Sends a command to QlipQ, or shows the warning triangle if QlipQ isn't connected.
    fn send_qlipq(&self, context: &str, command: String) -> bool {
        let sent = self
            .qlipq
            .link
            .as_ref()
            .is_some_and(|link| link.send(command).is_ok());
        if !sent {
            let _ = self.streamdeck.send(streamdeck::show_alert(context));
        }
        sent
    }

    fn press_qlipq_key(&mut self, context: &str, kind: KeyKind) {
        let Some(key) = self.qlipq.keys.get(context) else {
            return;
        };
        let state = &self.qlipq.state;
        let has_clip = state.clip.is_some();
        let command = match kind {
            KeyKind::Play if has_clip => "togglePlay",
            KeyKind::SetIn if has_clip => "setIn",
            KeyKind::SetOut if has_clip => "setOut",
            KeyKind::Export if state.exporting.is_some() => {
                if !key.settings.cancels_export() {
                    return;
                }
                "cancelExport"
            }
            KeyKind::Export if has_clip => "export",
            KeyKind::Rescan => "rescan",
            _ => {
                let _ = self.streamdeck.send(streamdeck::show_alert(context));
                return;
            }
        };
        if self.send_qlipq(context, qlipq::command(command)) && kind == KeyKind::Rescan {
            let _ = self.streamdeck.send(streamdeck::show_ok(context));
        }
    }

    /// Sends each QlipQ key and dial whatever changed.
    pub(super) fn draw_qlipq(&mut self) {
        let qlipq = &mut self.qlipq;
        let connected = qlipq.link.is_some();
        for (context, key) in &mut qlipq.keys {
            let look = qlipq::key_look(
                key.kind,
                &key.settings,
                connected,
                &qlipq.state,
                &qlipq.queue,
            );
            if key.shown.as_ref() != Some(&look) {
                let _ = self
                    .streamdeck
                    .send(streamdeck::set_image(context, &qlipq::key_image(&look)));
                key.shown = Some(look);
            }
        }
        let mut dials = std::mem::take(&mut qlipq.dials);
        for (context, dial) in &mut dials {
            let payload = match dial.kind {
                DialKind::Scrub => qlipq::scrub_feedback(connected, &qlipq.state),
                DialKind::Picker => qlipq::picker_feedback(
                    connected,
                    &qlipq.queue,
                    qlipq.cursor_index(dial.cursor.as_deref()),
                ),
            };
            if dial.sent.as_ref() != Some(&payload) {
                let _ = self
                    .streamdeck
                    .send(streamdeck::set_feedback(context, payload.clone()));
                dial.sent = Some(payload);
            }
        }
        qlipq.dials = dials;
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    fn connected_app() -> (
        App,
        tokio::sync::mpsc::UnboundedReceiver<String>,
        tokio::sync::mpsc::UnboundedReceiver<String>,
    ) {
        let (streamdeck, streamdeck_rx) = tokio::sync::mpsc::unbounded_channel();
        let (link, link_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(streamdeck);
        app.on_qlipq(QlipqEvent::Connected(link));
        (app, streamdeck_rx, link_rx)
    }

    fn sent_commands(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> Vec<Value> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|line| serde_json::from_str(&line).unwrap())
            .collect()
    }

    fn events(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|text| {
                let msg: Value = serde_json::from_str(&text).unwrap();
                msg["event"].as_str().unwrap().to_string()
            })
            .collect()
    }

    fn appear(app: &mut App, uuid: &str, context: &str, settings: Value) {
        app.on_streamdeck(json!({
            "event": "willAppear", "action": uuid, "context": context,
            "payload": { "settings": settings },
        }));
    }

    fn open_clip(app: &mut App, playing: bool, exporting: Option<f32>) {
        app.on_qlipq(QlipqEvent::Line(json!({
            "type": "state",
            "clip": { "id": "b.mp4", "name": "b.mp4", "duration": 60.0, "position": 6.0,
                      "trimIn": 1.0, "trimOut": 50.0, "playing": playing },
            "exporting": exporting,
        })));
    }

    fn queue(app: &mut App, selected: &str) {
        app.on_qlipq(QlipqEvent::Line(json!({
            "type": "queue",
            "items": [
                { "id": "a.mp4", "name": "a.mp4", "duration": 30.0 },
                { "id": "b.mp4", "name": "b.mp4", "duration": 60.0 },
                { "id": "c.mp4", "name": "c.mp4", "duration": null },
            ],
            "selected": selected,
        })));
    }

    #[test]
    fn keys_send_their_commands_only_when_they_can_act() {
        let (mut app, mut streamdeck_rx, mut link_rx) = connected_app();
        for (uuid, context) in [
            ("com.qcksys.qlipq.meters.qlipq-play", "play"),
            ("com.qcksys.qlipq.meters.qlipq-in", "in"),
            ("com.qcksys.qlipq.meters.qlipq-export", "export"),
            ("com.qcksys.qlipq.meters.qlipq-rescan", "rescan"),
        ] {
            appear(&mut app, uuid, context, json!({}));
        }
        let press = |app: &mut App, context: &str| {
            let uuid = format!("com.qcksys.qlipq.meters.qlipq-{context}");
            app.on_streamdeck(json!({ "event": "keyDown", "action": uuid, "context": context }));
        };

        press(&mut app, "play");
        assert!(sent_commands(&mut link_rx).is_empty(), "no clip open");
        assert_eq!(events(&mut streamdeck_rx), ["showAlert"]);

        open_clip(&mut app, false, None);
        press(&mut app, "play");
        press(&mut app, "in");
        press(&mut app, "export");
        press(&mut app, "rescan");
        let sent: Vec<Value> = sent_commands(&mut link_rx)
            .into_iter()
            .map(|command| command["cmd"].clone())
            .collect();
        assert_eq!(sent, ["togglePlay", "setIn", "export", "rescan"]);
        assert_eq!(events(&mut streamdeck_rx), ["showOk"], "rescan confirms");

        open_clip(&mut app, false, Some(0.3));
        press(&mut app, "export");
        assert_eq!(sent_commands(&mut link_rx)[0]["cmd"], "cancelExport");
    }

    #[test]
    fn export_key_can_ignore_presses_while_exporting() {
        let (mut app, _streamdeck_rx, mut link_rx) = connected_app();
        appear(
            &mut app,
            "com.qcksys.qlipq.meters.qlipq-export",
            "export",
            json!({ "whileExporting": "nothing" }),
        );
        open_clip(&mut app, false, Some(0.3));
        app.on_streamdeck(json!({
            "event": "keyDown", "action": "com.qcksys.qlipq.meters.qlipq-export", "context": "export",
        }));
        assert!(sent_commands(&mut link_rx).is_empty());
    }

    #[test]
    fn keys_redraw_when_qlipq_changes() {
        let (mut app, mut streamdeck_rx, _link_rx) = connected_app();
        appear(
            &mut app,
            "com.qcksys.qlipq.meters.qlipq-play",
            "play",
            json!({}),
        );
        app.draw_qlipq();
        assert_eq!(events(&mut streamdeck_rx), ["setImage"]);
        app.draw_qlipq();
        assert!(events(&mut streamdeck_rx).is_empty(), "unchanged");
        open_clip(&mut app, true, None);
        app.draw_qlipq();
        assert_eq!(events(&mut streamdeck_rx), ["setImage"]);
        app.on_qlipq(QlipqEvent::Disconnected);
        app.draw_qlipq();
        assert_eq!(events(&mut streamdeck_rx), ["setImage"]);
    }

    #[test]
    fn scrub_dial_steps_and_plays() {
        let (mut app, _streamdeck_rx, mut link_rx) = connected_app();
        appear(
            &mut app,
            "com.qcksys.qlipq.meters.qlipq-scrub",
            "scrub",
            json!({ "unit": "seconds", "amount": 2 }),
        );
        let rotate = |app: &mut App, ticks: i32| {
            app.on_streamdeck(json!({
                "event": "dialRotate", "action": "com.qcksys.qlipq.meters.qlipq-scrub",
                "context": "scrub", "payload": { "ticks": ticks },
            }));
        };
        rotate(&mut app, 3);
        assert!(sent_commands(&mut link_rx).is_empty(), "no clip open");
        open_clip(&mut app, false, None);
        rotate(&mut app, -2);
        app.on_streamdeck(json!({
            "event": "dialDown", "action": "com.qcksys.qlipq.meters.qlipq-scrub", "context": "scrub",
        }));
        assert_eq!(
            sent_commands(&mut link_rx),
            [
                json!({ "cmd": "skip", "seconds": -4.0 }),
                json!({ "cmd": "togglePlay" })
            ]
        );
    }

    #[test]
    fn picker_moves_a_cursor_and_opens_its_clip() {
        let (mut app, mut streamdeck_rx, mut link_rx) = connected_app();
        queue(&mut app, "b.mp4");
        appear(
            &mut app,
            "com.qcksys.qlipq.meters.qlipq-picker",
            "picker",
            json!({}),
        );
        let picker = "com.qcksys.qlipq.meters.qlipq-picker";
        app.draw_qlipq();
        let shown: Value = serde_json::from_str(&streamdeck_rx.try_recv().unwrap()).unwrap();
        assert_eq!(
            shown["payload"]["title"], "Clip 2 of 3",
            "starts on the open clip"
        );

        app.on_streamdeck(json!({
            "event": "dialRotate", "action": picker, "context": "picker", "payload": { "ticks": 5 },
        }));
        app.on_streamdeck(json!({ "event": "touchTap", "action": picker, "context": "picker" }));
        assert_eq!(
            sent_commands(&mut link_rx),
            [json!({ "cmd": "select", "id": "c.mp4" })],
            "clamped at the last clip"
        );

        queue(&mut app, "a.mp4");
        app.draw_qlipq();
        let shown: Value = serde_json::from_str(&streamdeck_rx.try_recv().unwrap()).unwrap();
        assert_eq!(
            shown["payload"]["title"], "Clip 1 of 3",
            "follows a new selection"
        );
    }
}
