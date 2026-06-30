use qlipq_core::config::OutputSettings;
use qlipq_core::config::{QualityMode, QualityPreset};
use qlipq_core::media::MediaInfo;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoEncodeOptions {
    pub codec: Option<String>,
    pub crf: Option<i64>,
    /// Target video bitrate in kbps. When set, takes precedence over `crf`.
    pub bitrate_kbps: Option<i64>,
    /// Max bitrate cap (kbps) for constrained-VBR: pairs with `crf` via -maxrate/-bufsize.
    pub maxrate_kbps: Option<i64>,
    pub preset: Option<String>,
    /// Output frame rate; when set, forces a re-encode.
    pub fps: Option<i64>,
    /// Downscale to this height (keeps aspect, even width); forces a re-encode.
    pub scale_height: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioEncodeOptions {
    pub codec: Option<String>,
    pub bitrate: Option<String>,
}

/// Resolved encoding choices the in-process export consumes to plan the encoder + rate control.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEncode {
    pub video: VideoEncodeOptions,
    pub audio: AudioEncodeOptions,
    /// Whether the chosen quality wants a re-encode (edits may force one regardless).
    pub reencode: bool,
}

fn preset_crf(preset: QualityPreset) -> i64 {
    match preset {
        QualityPreset::High => 18,
        QualityPreset::Balanced => 23,
        QualityPreset::Small => 28,
        QualityPreset::Original => 23,
    }
}

/// Resolve persisted [`OutputSettings`] into concrete encode options for a clip.
/// fps and downscale are clamped against the source so we never up-rate or up-scale.
pub fn output_settings_to_encode(output: &OutputSettings, media: &MediaInfo) -> ResolvedEncode {
    let fps = if output.fps > 0 && (output.fps as f64) < media.fps {
        Some(output.fps)
    } else {
        None
    };
    let scale_height = if output.max_height > 0 && output.max_height < media.height {
        Some(output.max_height)
    } else {
        None
    };

    let mut video = VideoEncodeOptions {
        codec: Some(output.video_codec.to_ffmpeg().to_string()),
        preset: Some(output.encoder_preset.clone()),
        fps,
        scale_height,
        ..Default::default()
    };

    let reencode = match output.quality_mode {
        QualityMode::Bitrate => {
            video.bitrate_kbps = Some(output.video_bitrate_kbps);
            true
        }
        QualityMode::Vbr => {
            video.crf = Some(output.crf);
            video.maxrate_kbps = Some(output.video_bitrate_kbps);
            true
        }
        QualityMode::Crf => {
            video.crf = Some(output.crf);
            true
        }
        QualityMode::Preset => {
            if output.quality_preset == QualityPreset::Original {
                // Stream-copy by default; this crf only applies if an edit forces a re-encode.
                video.crf = Some(18);
                false
            } else {
                video.crf = Some(preset_crf(output.quality_preset));
                true
            }
        }
    };

    ResolvedEncode {
        video,
        audio: AudioEncodeOptions {
            codec: Some("aac".to_string()),
            bitrate: Some(format!("{}k", output.audio_bitrate_kbps)),
        },
        reencode,
    }
}
