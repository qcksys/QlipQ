/// Raw OBS config files, read by the host and parsed here (no I/O in this module).
/// Profiles are kept in insertion order so "first profile" fallback is deterministic.
#[derive(Debug, Clone, Default)]
pub struct ObsConfigFiles {
    /// Contents of `obs-studio/user.ini`, or `None` if absent.
    pub user_ini: Option<String>,
    /// Profile directory name → contents of its `basic.ini`.
    pub profiles: Vec<(String, String)>,
}

/// Read a single `key=value` from an INI section. Section and key match case-insensitively;
/// tolerant of a UTF-8 BOM and CRLF line endings, both of which OBS writes.
fn get_ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let section_lower = section.to_lowercase();
    let key_lower = key.to_lowercase();
    let mut current: Option<String> = None;
    for raw in text.split('\n') {
        let line = raw.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') && line.len() >= 3 {
            current = Some(line[1..line.len() - 1].to_lowercase());
            continue;
        }
        if current.as_deref() != Some(section_lower.as_str()) {
            continue;
        }
        if let Some(eq) = line.find('=') {
            if line[..eq].trim().to_lowercase() == key_lower {
                return Some(line[eq + 1..].trim().to_string());
            }
        }
    }
    None
}

/// Resolve the folder OBS records into, from its config files. Picks the active profile
/// (`user.ini` `[Basic] ProfileDir`, then `Profile`, then the first profile), reads that
/// profile's `basic.ini`, and returns `[AdvOut] RecFilePath` (Advanced mode) or
/// `[SimpleOutput] FilePath`. `None` when nothing usable is found.
pub fn detect_obs_recording_folder(files: &ObsConfigFiles) -> Option<String> {
    if files.profiles.is_empty() {
        return None;
    }

    let active = files.user_ini.as_ref().and_then(|ui| {
        get_ini_value(ui, "Basic", "ProfileDir")
            .filter(|s| !s.is_empty())
            .or_else(|| get_ini_value(ui, "Basic", "Profile").filter(|s| !s.is_empty()))
    });

    let basic_ini: Option<&str> = match &active {
        Some(name) => files
            .profiles
            .iter()
            .find(|(k, _)| k == name)
            .or_else(|| files.profiles.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)))
            .map(|(_, v)| v.as_str()),
        None => None,
    }
    .or_else(|| files.profiles.first().map(|(_, v)| v.as_str()));

    let basic_ini = basic_ini?;
    let mode = get_ini_value(basic_ini, "Output", "Mode");
    let folder = if mode.as_deref().map(|m| m.eq_ignore_ascii_case("advanced")).unwrap_or(false) {
        get_ini_value(basic_ini, "AdvOut", "RecFilePath")
    } else {
        get_ini_value(basic_ini, "SimpleOutput", "FilePath")
    };

    folder.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}
