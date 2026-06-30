use std::sync::LazyLock;

use chrono::NaiveDateTime;
use regex::{Captures, Regex};

use crate::datetimes::{format_date, format_datetime, format_time};

/// Values available to a naming template when renaming a clip.
#[derive(Debug, Clone, Default)]
pub struct RenameVars {
    /// Original base name without extension.
    pub name: String,
    /// Original extension without the leading dot.
    pub ext: String,
    pub recorded_at: Option<NaiveDateTime>,
    pub source: Option<String>,
    /// 1-based position used by the `{index}` token.
    pub index: Option<i64>,
}

const FALLBACK_BASE: &str = "clip";

// Characters illegal in Windows filenames (the strictest common target). Dashes/spaces kept.
static ILLEGAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[<>:"/\\|?*]"#).unwrap());
static TRAILING_DOTS_SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ .]+$").unwrap());
static SEPARATOR_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[_.\s-]{2,}").unwrap());
static EDGE_SEPARATORS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[_.\s-]+|[_.\s-]+$").unwrap());
// `[A-Za-z0-9_]` matches JS `\w` (ASCII) exactly.
static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{([A-Za-z0-9_]+)\}").unwrap());

/// Replace illegal filename characters (incl. control chars) and trim trailing dots/spaces.
pub fn sanitize_file_name(name: &str) -> String {
    let without_controls: String = name
        .chars()
        .map(|c| if (c as u32) < 0x20 { '_' } else { c })
        .collect();
    let replaced = ILLEGAL.replace_all(&without_controls, "_");
    let trimmed = TRAILING_DOTS_SPACES.replace(&replaced, "");
    trimmed.trim().to_string()
}

fn tidy_separators(value: &str) -> String {
    let collapsed = SEPARATOR_RUN.replace_all(value, |caps: &Captures| {
        let run = &caps[0];
        if run.contains(' ') {
            " ".to_string()
        } else {
            run.chars().next().unwrap().to_string()
        }
    });
    EDGE_SEPARATORS.replace_all(&collapsed, "").to_string()
}

/// Expand a naming template into a base filename (no extension). Supported tokens:
/// `{name} {source} {date} {time} {datetime} {index} {ext}`. Unknown tokens expand to "".
pub fn apply_naming_template(template: &str, vars: &RenameVars) -> String {
    let expanded = TOKEN.replace_all(template, |caps: &Captures| match &caps[1] {
        "name" => vars.name.clone(),
        "source" => vars.source.clone().unwrap_or_default(),
        "date" => vars.recorded_at.as_ref().map(format_date).unwrap_or_default(),
        "time" => vars.recorded_at.as_ref().map(format_time).unwrap_or_default(),
        "datetime" => vars.recorded_at.as_ref().map(format_datetime).unwrap_or_default(),
        "index" => vars.index.map(|i| i.to_string()).unwrap_or_default(),
        "ext" => vars.ext.clone(),
        _ => String::new(),
    });
    let base = tidy_separators(&sanitize_file_name(&expanded));
    if base.is_empty() {
        FALLBACK_BASE.to_string()
    } else {
        base
    }
}

/// Build a full target filename (base + preserved extension) from a template.
pub fn build_renamed_file_name(template: &str, vars: &RenameVars) -> String {
    let base = apply_naming_template(template, vars);
    if vars.ext.is_empty() {
        base
    } else {
        format!("{}.{}", base, vars.ext)
    }
}

/// Split a filename into its base name and extension (without dot).
pub fn split_file_name(file_name: &str) -> (String, String) {
    match file_name.rfind('.') {
        Some(dot) if dot > 0 => (file_name[..dot].to_string(), file_name[dot + 1..].to_string()),
        _ => (file_name.to_string(), String::new()),
    }
}
