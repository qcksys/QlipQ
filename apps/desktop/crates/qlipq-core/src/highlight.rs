use serde::{Deserialize, Serialize};

use crate::edit_spec::TrimSpec;

pub const WINDOW_SECONDS: f64 = 30.0;
pub const OVERLAP_SECONDS: f64 = 5.0;
pub const LEAD_IN_SECONDS: f64 = 3.0;
pub const AFTERMATH_SECONDS: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalysisWindow {
    pub start_sec: f64,
    pub end_sec: f64,
}

pub fn analysis_windows(duration_sec: f64) -> Vec<AnalysisWindow> {
    if !duration_sec.is_finite() || duration_sec <= 0.0 {
        return Vec::new();
    }
    let mut windows = Vec::new();
    let mut start_sec = 0.0;
    loop {
        let end_sec = (start_sec + WINDOW_SECONDS).min(duration_sec);
        windows.push(AnalysisWindow { start_sec, end_sec });
        if end_sec >= duration_sec {
            return windows;
        }
        start_sec = end_sec - OVERLAP_SECONDS;
    }
}

impl AnalysisWindow {
    /// Sample once per second, including a frame near the end but never seeking to EOF.
    pub fn sample_times(self) -> Vec<f64> {
        let length = self.end_sec - self.start_sec;
        let count = length.ceil() as usize;
        (0..count)
            .map(|i| self.start_sec + (i as f64 + (i as f64 + 1.0).min(length)) / 2.0)
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HighlightDetection {
    /// Null when no noteworthy event is visible.
    #[serde(deserialize_with = "Option::deserialize")]
    pub event: Option<HighlightEvent>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HighlightEvent {
    /// Start of the event in seconds from the beginning of this analysis window.
    #[schemars(range(min = 0))]
    pub start_sec: f64,
    /// End of the event in seconds from the beginning of this analysis window.
    #[schemars(range(min = 0))]
    pub end_sec: f64,
    /// Relative interest from 1 (minor action) to 100 (exceptional play), not a probability.
    #[schemars(range(min = 1, max = 100))]
    pub score: u8,
    /// Brief description of the visible event.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HighlightSuggestion {
    pub trim: TrimSpec,
    pub score: u8,
    pub reason: String,
}

pub fn detection_schema() -> serde_json::Value {
    let schema = schemars::generate::SchemaSettings::default()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<HighlightDetection>();
    serde_json::to_value(schema).unwrap()
}

impl HighlightEvent {
    pub fn into_suggestion(
        self,
        window: AnalysisWindow,
        duration_sec: f64,
    ) -> Result<HighlightSuggestion, String> {
        if !duration_sec.is_finite()
            || !window.start_sec.is_finite()
            || !window.end_sec.is_finite()
            || window.start_sec < 0.0
            || window.end_sec > duration_sec
            || window.end_sec <= window.start_sec
            || !self.start_sec.is_finite()
            || !self.end_sec.is_finite()
            || self.start_sec < 0.0
            || self.end_sec < self.start_sec
            || self.end_sec > window.end_sec - window.start_sec
            || !(1..=100).contains(&self.score)
            || self.reason.trim().is_empty()
        {
            return Err(
                "The model returned an invalid highlight range. Try again or trim manually.".into(),
            );
        }
        Ok(HighlightSuggestion {
            trim: TrimSpec {
                start_sec: (window.start_sec + self.start_sec - LEAD_IN_SECONDS).max(0.0),
                end_sec: (window.start_sec + self.end_sec + AFTERMATH_SECONDS).min(duration_sec),
            },
            score: self.score,
            reason: self.reason.trim().chars().take(300).collect(),
        })
    }
}
