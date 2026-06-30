use qlipq_core::detect::{detect_obs_recording_folder, ObsConfigFiles};

const USER_INI: &str = "\u{feff}[General]\r\nFirstRun=true\r\n\r\n[Basic]\r\nProfile=Default\r\nProfileDir=Default\r\n";

const ADVANCED_BASIC: &str = "\u{feff}[General]\r\nName=Default\r\n\r\n[Output]\r\nMode=Advanced\r\n\r\n[SimpleOutput]\r\nFilePath=E:/Simple Path\r\n\r\n[AdvOut]\r\nRecType=Standard\r\nRecFilePath=E:/OBS Recordings\r\nRecFormat2=mp4\r\n";

const SIMPLE_BASIC: &str = "\u{feff}[Output]\r\nMode=Simple\r\n\r\n[SimpleOutput]\r\nFilePath=D:/Clips\r\n";

fn files(user_ini: Option<&str>, profiles: Vec<(&str, &str)>) -> ObsConfigFiles {
    ObsConfigFiles {
        user_ini: user_ini.map(|s| s.to_string()),
        profiles: profiles.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    }
}

#[test]
fn advanced_mode_uses_advout_rec_file_path() {
    let f = files(Some(USER_INI), vec![("Default", ADVANCED_BASIC)]);
    assert_eq!(detect_obs_recording_folder(&f).as_deref(), Some("E:/OBS Recordings"));
}

#[test]
fn simple_mode_uses_simpleoutput_file_path() {
    let f = files(Some(USER_INI), vec![("Default", SIMPLE_BASIC)]);
    assert_eq!(detect_obs_recording_folder(&f).as_deref(), Some("D:/Clips"));
}

#[test]
fn missing_mode_falls_back_to_simple_output() {
    let no_mode = "\u{feff}[SimpleOutput]\r\nFilePath=C:/Recordings\r\n";
    let f = files(None, vec![("Default", no_mode)]);
    assert_eq!(detect_obs_recording_folder(&f).as_deref(), Some("C:/Recordings"));
}

#[test]
fn active_profile_selected_by_profile_dir() {
    let f = files(
        Some("[Basic]\nProfileDir=Gaming\n"),
        vec![
            ("Default", ADVANCED_BASIC),
            ("Gaming", "[Output]\nMode=Simple\n[SimpleOutput]\nFilePath=G:/Gaming\n"),
        ],
    );
    assert_eq!(detect_obs_recording_folder(&f).as_deref(), Some("G:/Gaming"));
}

#[test]
fn profile_dir_matched_case_insensitively() {
    let f = files(Some("[Basic]\nProfileDir=default\n"), vec![("Default", ADVANCED_BASIC)]);
    assert_eq!(detect_obs_recording_folder(&f).as_deref(), Some("E:/OBS Recordings"));
}

#[test]
fn falls_back_to_first_profile_when_user_ini_absent() {
    let f = files(None, vec![("Default", SIMPLE_BASIC)]);
    assert_eq!(detect_obs_recording_folder(&f).as_deref(), Some("D:/Clips"));
}

#[test]
fn no_profiles_returns_none() {
    let f = files(Some(USER_INI), vec![]);
    assert_eq!(detect_obs_recording_folder(&f), None);
}

#[test]
fn empty_recording_path_returns_none() {
    let empty = "[Output]\nMode=Advanced\n[AdvOut]\nRecFilePath=\n";
    let f = files(None, vec![("Default", empty)]);
    assert_eq!(detect_obs_recording_folder(&f), None);
}
