use std::sync::LazyLock;

use regex::Regex;

#[derive(Debug, Clone, PartialEq)]
pub struct ProgressUpdate {
    /// Output timestamp reached so far, in seconds, or `None` if unknown.
    pub out_time_sec: Option<f64>,
    /// True once ffmpeg reports `progress=end`.
    pub done: bool,
}

static TIMECODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9]+):([0-9]{2}):([0-9]{2}(?:\.[0-9]+)?)$").unwrap());

/// Parse a `HH:MM:SS.micro` timecode into seconds, or `None` if unparseable.
pub fn parse_timecode(value: &str) -> Option<f64> {
    let caps = TIMECODE.captures(value.trim())?;
    let h: f64 = caps[1].parse().ok()?;
    let m: f64 = caps[2].parse().ok()?;
    let s: f64 = caps[3].parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

/// Parse one or more `-progress pipe:1` chunks. ffmpeg's `out_time_ms` is actually microseconds —
/// both it and `out_time_us` are divided by 1,000,000 (do not "fix" this).
pub fn parse_progress(text: &str) -> ProgressUpdate {
    let mut out_time_sec: Option<f64> = None;
    let mut done = false;

    for raw in text.split('\n') {
        let line = raw.trim();
        let Some(eq) = line.find('=') else { continue };
        let key = &line[..eq];
        let value = &line[eq + 1..];

        match key {
            "out_time_us" | "out_time_ms" => {
                if let Ok(micros) = value.trim().parse::<f64>() {
                    if micros.is_finite() && micros >= 0.0 {
                        out_time_sec = Some(micros / 1_000_000.0);
                    }
                }
            }
            "out_time" => {
                if let Some(parsed) = parse_timecode(value) {
                    out_time_sec = Some(parsed);
                }
            }
            "progress" => {
                done = value == "end";
            }
            _ => {}
        }
    }

    ProgressUpdate { out_time_sec, done }
}

/// Clamp an export progress fraction (0..1) from the current and total seconds.
pub fn progress_fraction(out_time_sec: Option<f64>, duration_sec: f64) -> f64 {
    match out_time_sec {
        Some(t) if duration_sec > 0.0 => (t / duration_sec).clamp(0.0, 1.0),
        _ => 0.0,
    }
}
