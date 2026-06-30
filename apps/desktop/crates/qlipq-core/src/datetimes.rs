use chrono::{NaiveDateTime, Timelike, Datelike};

/// `YYYY-MM-DD` (from a local wall-clock timestamp).
pub fn format_date(dt: &NaiveDateTime) -> String {
    format!("{:04}-{:02}-{:02}", dt.year(), dt.month(), dt.day())
}

/// `HH-MM-SS` (dashes are filesystem-safe, unlike colons).
pub fn format_time(dt: &NaiveDateTime) -> String {
    format!("{:02}-{:02}-{:02}", dt.hour(), dt.minute(), dt.second())
}

/// `YYYY-MM-DD_HH-MM-SS`.
pub fn format_datetime(dt: &NaiveDateTime) -> String {
    format!("{}_{}", format_date(dt), format_time(dt))
}

/// Human duration like `1:02:03` or `2:05` from a number of seconds.
pub fn format_duration(total_seconds: f64) -> String {
    let safe = total_seconds.max(0.0).floor() as i64;
    let hours = safe / 3600;
    let minutes = (safe % 3600) / 60;
    let seconds = safe % 60;
    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{}:{:02}", minutes, seconds)
    }
}
