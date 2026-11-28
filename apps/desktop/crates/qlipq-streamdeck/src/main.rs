//! QlipQ Live Meters: a Stream Deck + plugin that shows live OBS audio levels on the dials'
//! touch strip and turns each dial into that input's volume and mute control. It also has a
//! Save Replay key for OBS's replay buffer and a dial showing CPU, GPU and memory use.
#![cfg_attr(not(test), windows_subsystem = "windows")]

mod audio;
mod log;
mod meter;
mod obs;
mod qlipq;
mod replay;
mod state;
mod stats;
mod streamdeck;

use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

/// About 15 touch-strip updates a second.
const FRAME: Duration = Duration::from_millis(66);

fn main() -> Result<()> {
    log::init();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run());
    if let Err(err) = &result {
        log::write(&format!("plugin stopped: {err:#}"));
    }
    result
}

async fn run() -> Result<()> {
    let args = streamdeck::Args::parse(std::env::args().skip(1))?;
    let (streamdeck, mut streamdeck_events) = streamdeck::connect(&args).await?;
    log::write("registered with Stream Deck");

    let (obs_sender, mut obs_events) = mpsc::unbounded_channel();
    tokio::spawn(obs::run(obs_sender));

    let (qlipq_sender, mut qlipq_events) = mpsc::unbounded_channel();
    tokio::spawn(qlipq::run(qlipq_sender));

    let (audio_sender, mut audio_events) = mpsc::unbounded_channel();
    let mut app = state::App::new(streamdeck).with_audio_events(audio_sender);
    let mut frames = tokio::time::interval(FRAME);
    frames.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            event = streamdeck_events.recv() => match event {
                Some(event) => app.on_streamdeck(event),
                None => break,
            },
            Some(event) = obs_events.recv() => app.on_obs(event),
            Some(()) = audio_events.recv() => app.on_audio_changed(),
            Some(event) = qlipq_events.recv() => app.on_qlipq(event),
            _ = frames.tick() => app.tick(),
        }
    }
    log::write("Stream Deck closed the connection");
    Ok(())
}
