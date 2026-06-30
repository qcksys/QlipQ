//! ISO-8601 timestamp helpers matching the web/C# persistence (`Date.toISOString()`), so the
//! cross-platform app reads/writes the same `edits.json` timestamps.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};

pub fn from_local(dt: NaiveDateTime) -> String {
    Local
        .from_local_datetime(&dt)
        .single()
        .map(|d| d.with_timezone(&Utc).to_rfc3339())
        .unwrap_or_default()
}

pub fn now() -> String {
    Utc::now().to_rfc3339()
}

pub fn from_unix_ms(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms).single().map(|d| d.to_rfc3339()).unwrap_or_default()
}

pub fn to_local(iso: &str) -> Option<NaiveDateTime> {
    DateTime::parse_from_rfc3339(iso).ok().map(|d| d.with_timezone(&Local).naive_local())
}
