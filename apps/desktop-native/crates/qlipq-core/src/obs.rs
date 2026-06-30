use std::sync::LazyLock;

use chrono::{NaiveDate, NaiveDateTime};
use regex::Regex;

/// Metadata recovered from an OBS recording/replay-buffer filename.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedRecording {
    /// Local wall-clock timestamp parsed from the filename, if present.
    pub recorded_at: Option<NaiveDateTime>,
    /// A leading label (OBS scene/profile or game name) before the timestamp, if present.
    pub source: Option<String>,
    /// Whether the filename looks like a replay-buffer clip ("Replay ...").
    pub is_replay: bool,
}

// `[0-9]` keeps digits ASCII to match the source TypeScript regex semantics.
static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([0-9]{4})-([0-9]{2})-([0-9]{2})[ _T-]([0-9]{2})[-.:]([0-9]{2})[-.:]([0-9]{2})").unwrap()
});
static REPLAY_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\breplay\b").unwrap());
static TRAILING_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[_\-.]+$").unwrap());
static ONLY_REPLAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^replay$").unwrap());
static LEADING_REPLAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^replay[_\-. ]+").unwrap());
static STARTS_REPLAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^replay\b").unwrap());

/// Parse an OBS recording filename into a timestamp and optional source label.
pub fn parse_obs_filename(file_name: &str) -> ParsedRecording {
    let base = strip_extension(file_name);
    let mut is_replay = REPLAY_WORD.is_match(base);

    let Some(caps) = TIMESTAMP.captures(base) else {
        return ParsedRecording { recorded_at: None, source: None, is_replay };
    };

    let g = |i: usize| caps.get(i).unwrap().as_str().parse::<u32>().ok();
    let recorded_at = match (g(1), g(2), g(3), g(4), g(5), g(6)) {
        (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) => NaiveDate::from_ymd_opt(y as i32, mo, d)
            .and_then(|date| date.and_hms_opt(h, mi, s)),
        _ => None,
    };

    let m = caps.get(0).unwrap();
    let lead = base[..m.start()].trim();
    let cleaned_lead = TRAILING_SEP.replace(lead, "").trim().to_string();
    let mut source = None;
    if !cleaned_lead.is_empty() && !ONLY_REPLAY.is_match(&cleaned_lead) {
        let stripped = LEADING_REPLAY.replace(&cleaned_lead, "").trim().to_string();
        if !stripped.is_empty() {
            source = Some(stripped);
        }
    }
    if STARTS_REPLAY.is_match(lead) {
        is_replay = true;
    }

    ParsedRecording { recorded_at, source, is_replay }
}

/// Infer a game name from a recording's path relative to a watched root (NVIDIA per-game folders).
pub fn infer_game_from_path(root: &str, file_path: &str) -> Option<String> {
    let norm_root = root.replace('\\', "/");
    let norm_root = norm_root.trim_end_matches('/');
    let norm_file = file_path.replace('\\', "/");
    if norm_root.is_empty()
        || !norm_file
            .to_lowercase()
            .starts_with(&format!("{}/", norm_root.to_lowercase()))
    {
        return None;
    }
    let segments: Vec<&str> = norm_file[norm_root.len() + 1..]
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() >= 2 {
        Some(segments[0].to_string())
    } else {
        None
    }
}

fn strip_extension(file_name: &str) -> &str {
    match file_name.rfind('.') {
        Some(dot) if dot > 0 => &file_name[..dot],
        _ => file_name,
    }
}
