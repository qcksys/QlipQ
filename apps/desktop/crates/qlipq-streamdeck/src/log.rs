//! Plain append-only log next to Stream Deck's own logs; the plugin has no console.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Starts a fresh log for this run at `%APPDATA%\Elgato\StreamDeck\logs\com.qcksys.qlipq.meters.log`.
pub fn init() {
    let path = std::env::var_os("APPDATA").map(|appdata| {
        PathBuf::from(appdata).join("Elgato/StreamDeck/logs/com.qcksys.qlipq.meters.log")
    });
    if let Some(path) = &path {
        let _ = std::fs::write(path, "");
    }
    let _ = PATH.set(path);
}

pub fn write(message: &str) {
    let Some(Some(path)) = PATH.get() else {
        return;
    };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(
            file,
            "{} {message}",
            chrono::Local::now().format("%H:%M:%S")
        );
    }
}
