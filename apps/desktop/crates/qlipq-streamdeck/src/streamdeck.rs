//! Stream Deck plugin protocol: launch arguments, registration, and the message pump.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::log;

/// The arguments Stream Deck launches a plugin with.
#[derive(Debug, PartialEq)]
pub struct Args {
    pub port: u16,
    pub plugin_uuid: String,
    pub register_event: String,
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut port = None;
        let mut plugin_uuid = None;
        let mut register_event = None;
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let target = match flag.as_str() {
                "-port" => &mut port,
                "-pluginUUID" => &mut plugin_uuid,
                "-registerEvent" => &mut register_event,
                _ => continue,
            };
            *target = args.next();
        }
        Ok(Self {
            port: port
                .context("missing -port")?
                .parse()
                .context("invalid -port")?,
            plugin_uuid: plugin_uuid.context("missing -pluginUUID")?,
            register_event: register_event.context("missing -registerEvent")?,
        })
    }
}

/// Connects and registers. Returns a sender for outgoing messages and a receiver of incoming
/// events; the receiver closes when Stream Deck drops the connection.
pub async fn connect(
    args: &Args,
) -> Result<(
    mpsc::UnboundedSender<String>,
    mpsc::UnboundedReceiver<Value>,
)> {
    let url = format!("ws://127.0.0.1:{}", args.port);
    let (socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .with_context(|| format!("connecting to Stream Deck at {url}"))?;
    let (mut write, mut read) = socket.split();
    write
        .send(Message::text(
            json!({ "event": args.register_event, "uuid": args.plugin_uuid }).to_string(),
        ))
        .await?;

    let (outgoing, mut to_send) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(text) = to_send.recv().await {
            if let Err(err) = write.send(Message::text(text)).await {
                log::write(&format!("Stream Deck send failed: {err}"));
                return;
            }
        }
    });

    let (events, incoming) = mpsc::unbounded_channel::<Value>();
    tokio::spawn(async move {
        while let Some(message) = read.next().await {
            match message {
                Ok(Message::Text(text)) => match serde_json::from_str(text.as_str()) {
                    Ok(value) => {
                        if events.send(value).is_err() {
                            return;
                        }
                    }
                    Err(err) => log::write(&format!("bad message from Stream Deck: {err}")),
                },
                Ok(Message::Close(_)) => return,
                Ok(_) => {}
                Err(err) => {
                    log::write(&format!("Stream Deck connection error: {err}"));
                    return;
                }
            }
        }
    });

    Ok((outgoing, incoming))
}

pub fn set_feedback(context: &str, payload: Value) -> String {
    json!({ "event": "setFeedback", "context": context, "payload": payload }).to_string()
}

pub fn set_settings(context: &str, settings: Value) -> String {
    json!({ "event": "setSettings", "context": context, "payload": settings }).to_string()
}

/// Sets a key's image; `image` is a data URL.
pub fn set_image(context: &str, image: &str) -> String {
    json!({ "event": "setImage", "context": context, "payload": { "image": image, "target": 0 } })
        .to_string()
}

/// Briefly shows Stream Deck's checkmark on a key.
pub fn show_ok(context: &str) -> String {
    json!({ "event": "showOk", "context": context }).to_string()
}

/// Briefly shows Stream Deck's warning triangle on a key.
pub fn show_alert(context: &str) -> String {
    json!({ "event": "showAlert", "context": context }).to_string()
}

pub fn send_to_property_inspector(action: &str, context: &str, payload: Value) -> String {
    json!({
        "event": "sendToPropertyInspector",
        "action": action,
        "context": context,
        "payload": payload,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_launch_arguments_in_any_order() {
        let args = Args::parse(
            [
                "-info",
                "{}",
                "-registerEvent",
                "registerPlugin",
                "-port",
                "28196",
                "-pluginUUID",
                "abc123",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(
            args,
            Args {
                port: 28196,
                plugin_uuid: "abc123".into(),
                register_event: "registerPlugin".into()
            }
        );
    }

    #[test]
    fn missing_port_is_an_error() {
        assert!(
            Args::parse(["-pluginUUID", "x", "-registerEvent", "r"].map(String::from)).is_err()
        );
    }
}
