//! obs-websocket v5 client: reads OBS's own server config, authenticates, and relays messages.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::log;

const EVENT_SUB_SCENES: u64 = 1 << 2;
const EVENT_SUB_INPUTS: u64 = 1 << 3;
/// Replay buffer started, stopped and saved.
const EVENT_SUB_OUTPUTS: u64 = 1 << 6;
const EVENT_SUB_UI: u64 = 1 << 10;
const EVENT_SUB_INPUT_VOLUME_METERS: u64 = 1 << 16;
const RECONNECT_DELAY: Duration = Duration::from_secs(3);

pub enum ObsEvent {
    /// Identified; requests are sent as JSON text through the sender.
    Connected(mpsc::UnboundedSender<String>),
    Disconnected,
    /// An Event (op 5) or RequestResponse (op 7) message.
    Message(Value),
}

/// The subset of OBS's `plugin_config/obs-websocket/config.json` the plugin needs.
#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default)]
    pub server_enabled: bool,
    #[serde(default = "default_port")]
    pub server_port: u16,
    /// Whether a password is needed comes from OBS's Hello message, not this file.
    #[serde(default)]
    pub server_password: String,
}

fn default_port() -> u16 {
    4455
}

impl ServerConfig {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(serde_json::from_str(text)?)
    }

    fn path() -> Result<PathBuf> {
        let appdata = std::env::var_os("APPDATA").context("APPDATA is not set")?;
        Ok(PathBuf::from(appdata).join("obs-studio/plugin_config/obs-websocket/config.json"))
    }

    fn load() -> Result<Self> {
        let path = Self::path()?;
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text)
    }
}

/// The `authentication` string for Identify: base64(sha256(base64(sha256(password + salt)) + challenge)).
pub fn auth_string(password: &str, salt: &str, challenge: &str) -> String {
    let secret = BASE64.encode(&Sha256::digest(format!("{password}{salt}").as_bytes())[..]);
    BASE64.encode(&Sha256::digest(format!("{secret}{challenge}").as_bytes())[..])
}

pub fn request(request_type: &str, request_id: &str, data: Value) -> String {
    json!({
        "op": 6,
        "d": { "requestType": request_type, "requestId": request_id, "requestData": data }
    })
    .to_string()
}

/// Post-fader peak levels per input from an `InputVolumeMeters` event, as `[left, right]` dBFS.
/// Mono inputs repeat their one channel; inputs with no channels read as silence.
pub fn parse_meters(event_data: &Value) -> Vec<(String, [f32; 2])> {
    let Some(inputs) = event_data["inputs"].as_array() else {
        return Vec::new();
    };
    inputs
        .iter()
        .filter_map(|input| {
            let name = input["inputName"].as_str()?.to_string();
            let peaks: Vec<f32> = input["inputLevelsMul"]
                .as_array()
                .map(|channels| {
                    channels
                        .iter()
                        .map(|ch| crate::meter::mul_to_db(ch[1].as_f64().unwrap_or(0.0) as f32))
                        .collect()
                })
                .unwrap_or_default();
            let left = peaks.first().copied().unwrap_or(f32::NEG_INFINITY);
            let right = peaks.get(1).copied().unwrap_or(left);
            Some((name, [left, right]))
        })
        .collect()
}

/// Keeps a session with OBS open for the life of the plugin, retrying every few seconds.
pub async fn run(events: mpsc::UnboundedSender<ObsEvent>) {
    let mut last_error = String::new();
    loop {
        match session(&events).await {
            Ok(()) => log::write("OBS connection closed"),
            Err(err) => {
                // Log each distinct failure once, not every retry.
                let text = format!("{err:#}");
                if text != last_error {
                    log::write(&format!("OBS connection failed: {text}"));
                    last_error = text;
                }
            }
        }
        if events.send(ObsEvent::Disconnected).is_err() {
            return;
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn session(events: &mpsc::UnboundedSender<ObsEvent>) -> Result<()> {
    let config = ServerConfig::load()?;
    if !config.server_enabled {
        bail!("the OBS WebSocket server is switched off");
    }
    let url = format!("ws://127.0.0.1:{}", config.server_port);
    let (socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .with_context(|| format!("connecting to {url}"))?;
    let (mut write, mut read) = socket.split();

    let hello = next_json(&mut read).await?;
    if hello["op"] != 0 {
        bail!("expected Hello, got {hello}");
    }
    let mut identify = json!({
        "rpcVersion": 1,
        "eventSubscriptions": EVENT_SUB_SCENES | EVENT_SUB_INPUTS | EVENT_SUB_OUTPUTS | EVENT_SUB_UI | EVENT_SUB_INPUT_VOLUME_METERS,
    });
    if let Some(auth) = hello["d"]["authentication"].as_object() {
        if config.server_password.is_empty() {
            bail!("OBS requires a WebSocket password but none is set in its config");
        }
        identify["authentication"] = json!(auth_string(
            &config.server_password,
            auth["salt"].as_str().unwrap_or_default(),
            auth["challenge"].as_str().unwrap_or_default(),
        ));
    }
    write
        .send(Message::text(json!({ "op": 1, "d": identify }).to_string()))
        .await?;
    let identified = next_json(&mut read).await?;
    if identified["op"] != 2 {
        bail!("OBS did not accept the connection: {identified}");
    }
    log::write("connected to OBS");

    let (requests, mut outgoing) = mpsc::unbounded_channel::<String>();
    events.send(ObsEvent::Connected(requests))?;
    loop {
        tokio::select! {
            text = outgoing.recv() => match text {
                Some(text) => write.send(Message::text(text)).await?,
                None => return Ok(()),
            },
            incoming = read.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    events.send(ObsEvent::Message(serde_json::from_str(text.as_str())?))?;
                }
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(err.into()),
            },
        }
    }
}

async fn next_json<S>(read: &mut S) -> Result<Value>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match read.next().await {
            Some(Ok(Message::Text(text))) => return Ok(serde_json::from_str(text.as_str())?),
            Some(Ok(Message::Close(frame))) => bail!("OBS closed the connection: {frame:?}"),
            Some(Ok(_)) => {}
            Some(Err(err)) => return Err(err.into()),
            None => bail!("OBS closed the connection"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_string_matches_reference_vector() {
        // Password, salt and challenge from the obs-websocket docs; expected value computed
        // independently with .NET's SHA256 and Base64.
        assert_eq!(
            auth_string(
                "supersecretpassword",
                "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=",
                "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY=",
            ),
            "1Ct943GAT+6YQUUX47Ia/ncufilbe6+oD6lY+5kaCu4="
        );
    }

    #[test]
    fn server_config_reads_obs_fields_and_defaults() {
        let config = ServerConfig::parse(
            r#"{"alerts_enabled":false,"auth_required":true,"first_load":false,
                "server_enabled":true,"server_password":"pw","server_port":4466}"#,
        )
        .unwrap();
        assert!(config.server_enabled);
        assert_eq!(config.server_port, 4466);
        assert_eq!(config.server_password, "pw");

        let defaults = ServerConfig::parse("{}").unwrap();
        assert!(!defaults.server_enabled);
        assert_eq!(defaults.server_port, 4455);
    }

    #[test]
    fn parse_meters_reads_post_fader_peaks() {
        let data = json!({ "inputs": [
            { "inputName": "Mic/Aux", "inputLevelsMul": [[0.1, 0.5, 0.6], [0.1, 1.0, 1.0]] },
            { "inputName": "Mono", "inputLevelsMul": [[0.1, 0.1, 0.1]] },
            { "inputName": "Silent", "inputLevelsMul": [] },
        ]});
        let meters = parse_meters(&data);
        assert_eq!(meters.len(), 3);
        assert_eq!(meters[0].0, "Mic/Aux");
        assert!((meters[0].1[0] - -6.0206).abs() < 0.001);
        assert_eq!(meters[0].1[1], 0.0);
        assert_eq!(meters[1].1[0], meters[1].1[1]);
        assert_eq!(meters[2].1, [f32::NEG_INFINITY, f32::NEG_INFINITY]);
    }

    #[test]
    fn request_wraps_in_op_6() {
        let msg: Value = serde_json::from_str(&request(
            "GetInputMute",
            "id-1",
            json!({"inputName": "Mic"}),
        ))
        .unwrap();
        assert_eq!(msg["op"], 6);
        assert_eq!(msg["d"]["requestType"], "GetInputMute");
        assert_eq!(msg["d"]["requestData"]["inputName"], "Mic");
    }
}
