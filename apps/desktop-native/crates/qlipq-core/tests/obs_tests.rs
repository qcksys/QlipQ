use chrono::{Datelike, Timelike};
use qlipq_core::obs::{infer_game_from_path, parse_obs_filename};

#[test]
fn parses_obs_default_filename() {
    let r = parse_obs_filename("2024-01-31 18-09-05.mkv");
    let dt = r.recorded_at.unwrap();
    assert_eq!(dt.year(), 2024);
    assert_eq!(dt.month(), 1); // chrono months are 1-based (JS getMonth() == 0)
    assert_eq!(dt.day(), 31);
    assert_eq!(dt.hour(), 18);
    assert_eq!(dt.minute(), 9);
    assert_eq!(dt.second(), 5);
    assert_eq!(r.source, None);
    assert!(!r.is_replay);
}

#[test]
fn parses_replay_buffer_filename() {
    let r = parse_obs_filename("Replay 2024-12-01_07-30-00.mp4");
    assert!(r.is_replay);
    assert_eq!(r.recorded_at.unwrap().hour(), 7);
    assert_eq!(r.source, None);
}

#[test]
fn extracts_leading_source() {
    let r = parse_obs_filename("Apex Legends 2024-03-15 21-45-10.mkv");
    assert_eq!(r.source.as_deref(), Some("Apex Legends"));
    assert_eq!(r.recorded_at.unwrap().day(), 15);
}

#[test]
fn strips_replay_prefix_from_source() {
    let r = parse_obs_filename("Replay - Valorant - 2024-05-05 12-00-00.mp4");
    assert!(r.is_replay);
    assert_eq!(r.source.as_deref(), Some("Valorant"));
}

#[test]
fn supports_dotted_time_separators() {
    let r = parse_obs_filename("2024-06-28 14.02.59.mov");
    assert_eq!(r.recorded_at.unwrap().minute(), 2);
}

#[test]
fn returns_no_timestamp_for_unrecognised() {
    let r = parse_obs_filename("random-clip.mp4");
    assert!(r.recorded_at.is_none());
    assert!(!r.is_replay);
}

#[test]
fn infer_game_returns_subfolder() {
    assert_eq!(
        infer_game_from_path("E:/Shadowplay", "E:/Shadowplay/Counter-strike 2/clip.mp4").as_deref(),
        Some("Counter-strike 2")
    );
}

#[test]
fn infer_game_ignores_files_in_root() {
    assert_eq!(infer_game_from_path("E:/OBS Recordings", "E:/OBS Recordings/clip.mkv"), None);
}

#[test]
fn infer_game_tolerates_separators() {
    assert_eq!(infer_game_from_path("E:\\Shadowplay\\", "E:\\Shadowplay\\Deadlock\\a.mp4").as_deref(), Some("Deadlock"));
    assert_eq!(infer_game_from_path("E:/Shadowplay", "E:/Shadowplay/Deadlock\\a.mp4").as_deref(), Some("Deadlock"));
}

#[test]
fn infer_game_matches_root_case_insensitively() {
    assert_eq!(infer_game_from_path("e:/shadowplay", "E:/Shadowplay/Apex/a.mp4").as_deref(), Some("Apex"));
}

#[test]
fn infer_game_returns_none_when_not_under_root() {
    assert_eq!(infer_game_from_path("E:/Shadowplay", "D:/Other/x.mp4"), None);
}
