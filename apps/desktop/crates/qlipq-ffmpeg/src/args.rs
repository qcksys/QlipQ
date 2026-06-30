use qlipq_core::config::OutputSettings;
use qlipq_core::config::{QualityMode, QualityPreset};
use qlipq_core::edit_spec::EditSpec;
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

/// Resolved encoding choices, ready to feed [`build_export_args`].
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEncode {
    pub video: VideoEncodeOptions,
    pub audio: AudioEncodeOptions,
    /// Whether the chosen quality wants a re-encode (edits may force one regardless).
    pub reencode: bool,
}

pub struct BuildExportOptions {
    pub input_path: String,
    pub output_path: String,
    pub spec: EditSpec,
    /// Force a full re-encode even when a stream copy would suffice.
    pub reencode: bool,
    /// Append `-progress pipe:1 -nostats` for machine-readable progress on stdout.
    pub progress: bool,
    pub video: Option<VideoEncodeOptions>,
    pub audio: Option<AudioEncodeOptions>,
    /// Container metadata to stamp into the output (insertion-ordered).
    pub metadata: Vec<(String, String)>,
}

/// Format a number of seconds for ffmpeg's `-ss`/`-t` (millisecond precision).
pub fn format_seconds(sec: f64) -> String {
    format!("{:.3}", sec.max(0.0))
}

/// Minimal-decimal volume (up to 4 dp), matching `String(Number(v.toFixed(4)))`.
pub fn format_volume(volume: f64) -> String {
    let rounded = (volume * 10_000.0).round() / 10_000.0;
    format!("{}", rounded)
}

/// JS-style truthiness for an optional integer: present and non-zero.
fn truthy(value: Option<i64>) -> bool {
    matches!(value, Some(v) if v != 0)
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

/// Build the ffmpeg argument list to apply an [`EditSpec`] to a clip: fast-seek trim, crop+scale
/// filter chain, audio mixdown (per-track `volume` then `amix=normalize=0` to one track, summed at
/// the set levels), bitrate-vs-CRF rate control, constrained VBR, `-an` for no audio, and trailing
/// metadata/progress.
pub fn build_export_args(opts: &BuildExportOptions) -> Vec<String> {
    let spec = &opts.spec;
    let v = opts.video.clone().unwrap_or_default();
    let a = opts.audio.clone().unwrap_or_default();

    let video_codec = v.codec.unwrap_or_else(|| "libx264".to_string());
    let video_crf = v.crf.unwrap_or(20);
    let video_preset = v.preset.unwrap_or_else(|| "veryfast".to_string());
    let video_bitrate_kbps = v.bitrate_kbps;
    let video_maxrate_kbps = v.maxrate_kbps;
    let video_fps = v.fps;
    let video_scale_height = v.scale_height;
    let audio_codec = a.codec.unwrap_or_else(|| "aac".to_string());
    let audio_bitrate = a.bitrate.unwrap_or_else(|| "192k".to_string());

    let enabled_audio: Vec<&qlipq_core::edit_spec::AudioTrackSpec> =
        spec.audio_tracks.iter().filter(|t| t.enabled).collect();
    let needs_video_filter = spec.crop.is_some() || truthy(video_scale_height);
    // Two or more enabled tracks are mixed down to one (amix); a single track only needs a filter
    // when its volume isn't unity.
    let mix_audio = enabled_audio.len() >= 2;
    let needs_audio_filter = mix_audio || enabled_audio.iter().any(|t| t.volume != 1.0);
    let video_reencode = needs_video_filter || truthy(video_fps) || opts.reencode;
    let audio_reencode = needs_audio_filter;

    let mut args: Vec<String> = vec!["-y".to_string()];

    let mut duration: Option<f64> = None;
    if let Some(trim) = &spec.trim {
        args.push("-ss".to_string());
        args.push(format_seconds(trim.start_sec));
        duration = Some((trim.end_sec - trim.start_sec).max(0.0));
    }
    args.push("-i".to_string());
    args.push(opts.input_path.clone());
    if let Some(d) = duration {
        args.push("-t".to_string());
        args.push(format_seconds(d));
    }

    if needs_video_filter || needs_audio_filter {
        let mut filters: Vec<String> = Vec::new();
        let mut video_map = "0:v:0".to_string();

        let mut video_steps: Vec<String> = Vec::new();
        if let Some(c) = &spec.crop {
            video_steps.push(format!("crop={}:{}:{}:{}", c.width, c.height, c.x, c.y));
        }
        if truthy(video_scale_height) {
            video_steps.push(format!("scale=-2:{}", video_scale_height.unwrap()));
        }
        if !video_steps.is_empty() {
            filters.push(format!("[0:v:0]{}[vout]", video_steps.join(",")));
            video_map = "[vout]".to_string();
        }

        // Mix the enabled tracks into one: apply each track's volume, then sum with amix(normalize=0)
        // so the levels you set are preserved (this matches the preview monitor mix). A lone enabled
        // track skips amix — its volume filter, or a direct map when unity.
        let mut audio_maps: Vec<String> = Vec::new();
        match enabled_audio.as_slice() {
            [] => {}
            [t] if t.volume == 1.0 => audio_maps.push(format!("0:a:{}", t.index)),
            [t] => {
                filters.push(format!("[0:a:{}]volume={}[aout]", t.index, format_volume(t.volume)));
                audio_maps.push("[aout]".to_string());
            }
            tracks => {
                let mut inputs = String::new();
                for t in tracks {
                    if t.volume != 1.0 {
                        let label = format!("[a{}]", t.index);
                        filters.push(format!("[0:a:{}]volume={}{}", t.index, format_volume(t.volume), label));
                        inputs.push_str(&label);
                    } else {
                        inputs.push_str(&format!("[0:a:{}]", t.index));
                    }
                }
                filters.push(format!("{inputs}amix=inputs={}:normalize=0[aout]", tracks.len()));
                audio_maps.push("[aout]".to_string());
            }
        }
        args.push("-filter_complex".to_string());
        args.push(filters.join(";"));
        args.push("-map".to_string());
        args.push(video_map);
        for map in audio_maps {
            args.push("-map".to_string());
            args.push(map);
        }
    } else {
        args.push("-map".to_string());
        args.push("0:v:0".to_string());
        if enabled_audio.is_empty() {
            args.push("-an".to_string());
        } else {
            for track in &enabled_audio {
                args.push("-map".to_string());
                args.push(format!("0:a:{}", track.index));
            }
        }
    }

    if video_reencode {
        args.push("-c:v".to_string());
        args.push(video_codec);
        args.push("-preset".to_string());
        args.push(video_preset);
        if truthy(video_bitrate_kbps) {
            args.push("-b:v".to_string());
            args.push(format!("{}k", video_bitrate_kbps.unwrap()));
        } else {
            args.push("-crf".to_string());
            args.push(video_crf.to_string());
            if truthy(video_maxrate_kbps) {
                let m = video_maxrate_kbps.unwrap();
                args.push("-maxrate".to_string());
                args.push(format!("{}k", m));
                args.push("-bufsize".to_string());
                args.push(format!("{}k", m * 2));
            }
        }
        if truthy(video_fps) {
            args.push("-r".to_string());
            args.push(video_fps.unwrap().to_string());
        }
    } else {
        args.push("-c:v".to_string());
        args.push("copy".to_string());
    }

    if !enabled_audio.is_empty() {
        if audio_reencode {
            args.push("-c:a".to_string());
            args.push(audio_codec);
            args.push("-b:a".to_string());
            args.push(audio_bitrate);
        } else {
            args.push("-c:a".to_string());
            args.push("copy".to_string());
        }
    }

    for (key, value) in &opts.metadata {
        if !value.is_empty() {
            args.push("-metadata".to_string());
            args.push(format!("{}={}", key, value));
        }
    }

    if opts.progress {
        args.push("-progress".to_string());
        args.push("pipe:1".to_string());
        args.push("-nostats".to_string());
    }

    args.push(opts.output_path.clone());
    args
}
