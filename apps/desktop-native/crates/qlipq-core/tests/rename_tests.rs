use chrono::NaiveDate;
use qlipq_core::rename::*;

fn recorded_at() -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(2024, 1, 31).unwrap().and_hms_opt(18, 9, 5).unwrap()
}

#[test]
fn expands_date_source_name_tokens() {
    let out = apply_naming_template(
        "{date}_{source}_{name}",
        &RenameVars { name: "raw".into(), ext: "mp4".into(), recorded_at: Some(recorded_at()), source: Some("Apex".into()), index: None },
    );
    assert_eq!(out, "2024-01-31_Apex_raw");
}

#[test]
fn collapses_separators_when_token_empty() {
    let out = apply_naming_template(
        "{date}_{source}_{name}",
        &RenameVars { name: "raw".into(), ext: "mp4".into(), recorded_at: Some(recorded_at()), source: None, index: None },
    );
    assert_eq!(out, "2024-01-31_raw");
}

#[test]
fn preserves_extension_when_building_filename() {
    let out = build_renamed_file_name(
        "{datetime}",
        &RenameVars { name: "raw".into(), ext: "MKV".into(), recorded_at: Some(recorded_at()), source: None, index: None },
    );
    assert_eq!(out, "2024-01-31_18-09-05.MKV");
}

#[test]
fn falls_back_to_clip() {
    let out = apply_naming_template("{source}", &RenameVars { name: "x".into(), ext: "mp4".into(), ..Default::default() });
    assert_eq!(out, "clip");
}

#[test]
fn sanitizes_illegal_chars_keeps_dashes() {
    assert_eq!(sanitize_file_name("a:b/c?d-2024-01-01"), "a_b_c_d-2024-01-01");
}

#[test]
fn split_file_name_separates_base_and_ext() {
    assert_eq!(split_file_name("clip.final.mp4"), ("clip.final".to_string(), "mp4".to_string()));
    assert_eq!(split_file_name("noext"), ("noext".to_string(), String::new()));
}

#[test]
fn index_token_renders_one_based() {
    let out = apply_naming_template("{name}-{index}", &RenameVars { name: "clip".into(), ext: "mp4".into(), index: Some(3), ..Default::default() });
    assert_eq!(out, "clip-3");
}
