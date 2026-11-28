//! Local control pipe for the QlipQ Stream Deck actions (`crates/qlipq-streamdeck`).
//!
//! The plugin sends one JSON [`Command`] per line; QlipQ answers with [`StateMessage`] and
//! [`QueueMessage`] lines whenever what they describe changes. Only the signed-in user's own
//! programs can write to the pipe (Windows' default pipe security), and remote clients are refused.
//! Windows only; elsewhere the subscription does nothing.

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

/// Also hard-coded in the Stream Deck plugin (`qlipq-streamdeck/src/qlipq.rs`).
#[cfg_attr(not(windows), allow(dead_code))]
pub const PIPE_NAME: &str = r"\\.\pipe\qlipq-control";

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "cmd", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Command {
    TogglePlay,
    SetIn,
    SetOut,
    /// Move the playhead by whole frames (negative is back).
    StepFrames {
        frames: i32,
    },
    /// Move the playhead by seconds (negative is back).
    Skip {
        seconds: f64,
    },
    Export,
    CancelExport,
    Rescan,
    /// Open a queue item in the editor.
    Select {
        id: String,
    },
}

impl Command {
    pub fn parse(line: &str) -> Option<Self> {
        serde_json::from_str(line.trim()).ok()
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    /// A client connected; status lines sent here are written to it.
    Connected(UnboundedSender<String>),
    Command(Command),
    Disconnected,
}

/// The clip open in the editor.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    pub name: String,
    /// `None` until the clip has been probed.
    pub duration: Option<f64>,
    pub position: f64,
    pub trim_in: f64,
    pub trim_out: f64,
    pub playing: bool,
}

/// Sent whenever the open clip, its playhead or the export changes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename = "state", rename_all = "camelCase")]
pub struct StateMessage {
    pub clip: Option<Clip>,
    /// Export progress from 0 to 1 while an export runs.
    pub exporting: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueEntry {
    pub id: String,
    pub name: String,
    pub duration: Option<f64>,
}

/// Sent whenever the visible queue (in its current filter and sort order) or the selection changes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename = "queue", rename_all = "camelCase")]
pub struct QueueMessage {
    pub items: Vec<QueueEntry>,
    pub selected: Option<String>,
}

/// One status line, without the trailing newline.
pub fn line<T: Serialize>(message: &T) -> String {
    serde_json::to_string(message).unwrap_or_default()
}

#[cfg(windows)]
pub fn subscription() -> iced::Subscription<Event> {
    iced::Subscription::run(serve)
}

#[cfg(not(windows))]
pub fn subscription() -> iced::Subscription<Event> {
    iced::Subscription::none()
}

#[cfg(windows)]
fn serve() -> impl iced::futures::Stream<Item = Event> {
    serve_at(PIPE_NAME.to_string())
}

/// Serves one client at a time on the pipe `name` for the life of the app.
#[cfg(windows)]
fn serve_at(name: String) -> impl iced::futures::Stream<Item = Event> {
    use iced::futures::SinkExt;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::ServerOptions;

    iced::stream::channel(32, async move |mut output| {
        loop {
            // `first_pipe_instance` refuses a pipe another program (or a second QlipQ) already owns.
            let server = match ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(&name)
            {
                Ok(server) => server,
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            if server.connect().await.is_err() {
                continue;
            }
            let (reader, mut writer) = tokio::io::split(server);
            let (status, mut outgoing) = tokio::sync::mpsc::unbounded_channel::<String>();
            if output.send(Event::Connected(status)).await.is_err() {
                return;
            }
            let mut lines = BufReader::new(reader).lines();
            loop {
                tokio::select! {
                    line = lines.next_line() => match line {
                        Ok(Some(line)) => {
                            if let Some(command) = Command::parse(&line) {
                                if output.send(Event::Command(command)).await.is_err() {
                                    return;
                                }
                            }
                        }
                        _ => break,
                    },
                    text = outgoing.recv() => match text {
                        Some(text) => {
                            if writer.write_all(format!("{text}\n").as_bytes()).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    },
                }
            }
            if output.send(Event::Disconnected).await.is_err() {
                return;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(
            Command::parse(r#"{"cmd":"togglePlay"}"#),
            Some(Command::TogglePlay)
        );
        assert_eq!(
            Command::parse(r#"{"cmd":"stepFrames","frames":-3}"#),
            Some(Command::StepFrames { frames: -3 })
        );
        assert_eq!(
            Command::parse(r#" {"cmd":"skip","seconds":1.5} "#),
            Some(Command::Skip { seconds: 1.5 })
        );
        assert_eq!(
            Command::parse(r#"{"cmd":"select","id":"abc"}"#),
            Some(Command::Select { id: "abc".into() })
        );
        assert_eq!(
            Command::parse(r#"{"cmd":"cancelExport"}"#),
            Some(Command::CancelExport)
        );
        assert_eq!(Command::parse(r#"{"cmd":"formatDisk"}"#), None);
        assert_eq!(Command::parse("not json"), None);
    }

    /// A real pipe round trip: a client's command arrives as an event, and a status line sent
    /// through the `Connected` sender reaches the client.
    #[cfg(windows)]
    #[tokio::test]
    async fn pipe_carries_commands_in_and_status_out() {
        use iced::futures::StreamExt;
        use std::time::Duration;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::windows::named_pipe::ClientOptions;

        let name = format!(r"\\.\pipe\qlipq-control-test-{}", std::process::id());
        let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        let server = serve_at(name.clone());
        tokio::spawn(async move {
            let mut server = std::pin::pin!(server);
            while let Some(event) = server.next().await {
                if events_tx.send(event).is_err() {
                    return;
                }
            }
        });

        let client = loop {
            match ClientOptions::new().open(&name) {
                Ok(client) => break client,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        };
        let (reader, mut writer) = tokio::io::split(client);
        writer
            .write_all(b"{\"cmd\":\"setIn\"}\nnonsense\n{\"cmd\":\"rescan\"}\n")
            .await
            .unwrap();

        let timeout = Duration::from_secs(5);
        let Some(Event::Connected(status)) =
            tokio::time::timeout(timeout, events.recv()).await.unwrap()
        else {
            panic!("expected Connected first");
        };
        for expected in [Command::SetIn, Command::Rescan] {
            match tokio::time::timeout(timeout, events.recv()).await.unwrap() {
                Some(Event::Command(command)) => assert_eq!(command, expected),
                _ => panic!("expected {expected:?}"),
            }
        }

        status.send(r#"{"type":"state"}"#.to_string()).unwrap();
        let mut lines = BufReader::new(reader).lines();
        let line = tokio::time::timeout(timeout, lines.next_line())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(line.as_deref(), Some(r#"{"type":"state"}"#));

        drop(lines);
        drop(writer);
        assert!(matches!(
            tokio::time::timeout(timeout, events.recv()).await.unwrap(),
            Some(Event::Disconnected)
        ));
    }

    #[test]
    fn status_lines_carry_their_type() {
        let state = line(&StateMessage {
            clip: Some(Clip {
                id: "a".into(),
                name: "a.mp4".into(),
                duration: Some(10.0),
                position: 2.5,
                trim_in: 1.0,
                trim_out: 9.0,
                playing: true,
            }),
            exporting: None,
        });
        let state: serde_json::Value = serde_json::from_str(&state).unwrap();
        assert_eq!(state["type"], "state");
        assert_eq!(state["clip"]["trimIn"], 1.0);
        assert_eq!(state["exporting"], serde_json::Value::Null);

        let queue = line(&QueueMessage {
            items: vec![QueueEntry {
                id: "a".into(),
                name: "a.mp4".into(),
                duration: None,
            }],
            selected: Some("a".into()),
        });
        let queue: serde_json::Value = serde_json::from_str(&queue).unwrap();
        assert_eq!(queue["type"], "queue");
        assert_eq!(queue["items"][0]["name"], "a.mp4");
        assert_eq!(queue["selected"], "a");
    }
}
