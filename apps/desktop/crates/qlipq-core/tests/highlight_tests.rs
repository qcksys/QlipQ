use qlipq_core::highlight::*;

fn event(start_sec: f64, end_sec: f64) -> HighlightEvent {
    HighlightEvent {
        start_sec,
        end_sec,
        score: 80,
        reason: "A multi-kill".into(),
    }
}

#[test]
fn windows_cover_long_recordings_with_overlap_and_no_redundant_tail() {
    assert_eq!(
        analysis_windows(30.0),
        vec![AnalysisWindow {
            start_sec: 0.0,
            end_sec: 30.0
        }]
    );
    assert_eq!(
        analysis_windows(61.0),
        vec![
            AnalysisWindow {
                start_sec: 0.0,
                end_sec: 30.0
            },
            AnalysisWindow {
                start_sec: 25.0,
                end_sec: 55.0
            },
            AnalysisWindow {
                start_sec: 50.0,
                end_sec: 61.0
            },
        ]
    );
    for duration in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(analysis_windows(duration).is_empty());
    }
}

#[test]
fn sampling_stays_inside_each_window_including_short_clips() {
    for duration in [0.01, 0.5, 1.0, 1.1, 29.9, 30.0, 150.1] {
        for window in analysis_windows(duration) {
            let times = window.sample_times();
            assert!(!times.is_empty());
            assert!(times.len() <= 30);
            assert!(times
                .iter()
                .all(|&t| t >= window.start_sec && t < window.end_sec));
            assert!(times.windows(2).all(|pair| pair[1] > pair[0]));
        }
    }
}

#[test]
fn converts_window_relative_times_and_adds_context() {
    let s = event(10.0, 14.0)
        .into_suggestion(
            AnalysisWindow {
                start_sec: 25.0,
                end_sec: 55.0,
            },
            80.0,
        )
        .unwrap();
    assert_eq!((s.trim.start_sec, s.trim.end_sec), (32.0, 41.0));
    assert_eq!(s.reason, "A multi-kill");
}

#[test]
fn context_is_clamped_to_source_boundaries() {
    let window = AnalysisWindow {
        start_sec: 0.0,
        end_sec: 8.0,
    };
    let s = event(1.0, 7.5).into_suggestion(window, 8.0).unwrap();
    assert_eq!((s.trim.start_sec, s.trim.end_sec), (0.0, 8.0));
}

#[test]
fn single_instant_events_receive_a_non_empty_trim_with_context() {
    for (window, duration, time, expected) in [
        (
            AnalysisWindow {
                start_sec: 25.0,
                end_sec: 55.0,
            },
            80.0,
            10.5,
            (32.5, 37.5),
        ),
        (
            AnalysisWindow {
                start_sec: 0.0,
                end_sec: 8.0,
            },
            8.0,
            0.0,
            (0.0, 2.0),
        ),
        (
            AnalysisWindow {
                start_sec: 0.0,
                end_sec: 8.0,
            },
            8.0,
            8.0,
            (5.0, 8.0),
        ),
        (
            AnalysisWindow {
                start_sec: 0.0,
                end_sec: 0.05,
            },
            0.05,
            0.0,
            (0.0, 0.05),
        ),
    ] {
        let suggestion = event(time, time).into_suggestion(window, duration).unwrap();
        assert_eq!(
            (suggestion.trim.start_sec, suggestion.trim.end_sec),
            expected
        );
        assert!(suggestion.trim.start_sec < suggestion.trim.end_sec);
    }
}

#[test]
fn rejects_hallucinated_or_invalid_ranges_before_applying_padding() {
    let window = AnalysisWindow {
        start_sec: 25.0,
        end_sec: 55.0,
    };
    for (start, end) in [
        (-1.0, 5.0),
        (31.0, 31.0),
        (5.0, 4.0),
        (29.0, 31.0),
        (f64::NAN, 5.0),
        (1.0, f64::INFINITY),
    ] {
        assert!(event(start, end).into_suggestion(window, 80.0).is_err());
    }
    let mut invalid = event(1.0, 2.0);
    invalid.score = 101;
    assert!(invalid.into_suggestion(window, 80.0).is_err());
}

#[test]
fn model_can_report_no_event_and_malformed_json_is_rejected() {
    assert!(serde_json::from_str::<HighlightDetection>("{}").is_err());
    assert!(
        serde_json::from_str::<HighlightDetection>(r#"{"event":null}"#)
            .unwrap()
            .event
            .is_none()
    );
    assert!(
        serde_json::from_str::<HighlightDetection>(r#"{"event":{"startSec":"10 seconds"}}"#)
            .is_err()
    );
    let schema = detection_schema();
    assert_eq!(schema["required"], serde_json::json!(["event"]));
    assert!(schema["properties"]["event"]["anyOf"]
        .as_array()
        .unwrap()
        .iter()
        .any(|variant| variant["type"] == "null"));
}
