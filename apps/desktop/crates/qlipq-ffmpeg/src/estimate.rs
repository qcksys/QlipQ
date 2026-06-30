use std::sync::LazyLock;

use regex::Regex;

use qlipq_core::edit_spec::{effective_duration, EditSpec};
use qlipq_core::media::MediaInfo;

use crate::args::ResolvedEncode;

#[derive(Debug, Clone, PartialEq)]
pub struct SizeEstimate {
    pub bytes: f64,
    /// True when the figure is a quality-model ballpark (CRF/preset), not a hard target.
    pub approximate: bool,
}

static LEADING_INT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[+-]?[0-9]+").unwrap());

/// Baseline bits-per-pixel (per frame) at CRF 23; `bpp = base * 2^((23 - crf) / 6)`.
fn bpp_at_crf23(codec: &str) -> f64 {
    match codec {
        "libx264" => 0.095,
        "libx265" => 0.06,
        _ => 0.095,
    }
}

fn truthy(value: Option<i64>) -> bool {
    matches!(value, Some(v) if v != 0)
}

/// Estimate the exported file size for a clip under the resolved encode settings.
pub fn estimate_export_size(media: &MediaInfo, spec: &EditSpec, encode: &ResolvedEncode) -> SizeEstimate {
    let duration = effective_duration(spec, media);
    if duration <= 0.0 {
        return SizeEstimate { bytes: 0.0, approximate: false };
    }

    let video = &encode.video;
    let forced_reencode = spec.crop.is_some() || truthy(video.scale_height) || truthy(video.fps);
    let reencoding = encode.reencode || forced_reencode;

    // Pure stream-copy: output ≈ the source scaled by the fraction of duration kept.
    if !reencoding {
        let source_duration = if media.duration_sec != 0.0 { media.duration_sec } else { duration };
        let source_size = media.size_bytes.unwrap_or(0) as f64;
        return SizeEstimate { bytes: source_size * (duration / source_duration), approximate: false };
    }

    // Output frame dimensions after crop + downscale.
    let crop_w = spec.crop.as_ref().map(|c| c.width).unwrap_or(media.width) as f64;
    let crop_h = spec.crop.as_ref().map(|c| c.height).unwrap_or(media.height) as f64;
    let mut out_w = crop_w;
    let mut out_h = crop_h;
    if truthy(video.scale_height) && crop_h > 0.0 {
        let sh = video.scale_height.unwrap() as f64;
        out_h = sh;
        out_w = ((crop_w * (sh / crop_h)) / 2.0).round() * 2.0;
    }
    let out_fps = match video.fps {
        Some(f) if f > 0 => f as f64,
        _ => {
            if media.fps != 0.0 {
                media.fps
            } else {
                30.0
            }
        }
    };

    let audio_tracks = spec.audio_tracks.iter().filter(|t| t.enabled).count() as f64;
    let audio_kbps = audio_tracks * parse_int_leading(encode.audio.bitrate.as_deref().unwrap_or("0"));
    let audio_bytes = audio_kbps * 1000.0 * duration / 8.0;

    if truthy(video.bitrate_kbps) {
        let bitrate_bytes = video.bitrate_kbps.unwrap() as f64 * 1000.0 * duration / 8.0;
        return SizeEstimate { bytes: bitrate_bytes + audio_bytes, approximate: false };
    }

    let base = bpp_at_crf23(video.codec.as_deref().unwrap_or("libx264"));
    let bpp = base * 2_f64.powf((23 - video.crf.unwrap_or(20)) as f64 / 6.0);
    let mut video_bps = out_w * out_h * out_fps * bpp;
    if truthy(video.maxrate_kbps) {
        video_bps = video_bps.min(video.maxrate_kbps.unwrap() as f64 * 1000.0);
    }
    let video_bytes = video_bps * duration / 8.0;
    SizeEstimate { bytes: video_bytes + audio_bytes, approximate: true }
}

/// Mimics JS `parseInt(s, 10)`: leading integer, ignoring a trailing unit like `k`.
fn parse_int_leading(s: &str) -> f64 {
    LEADING_INT
        .find(s.trim())
        .and_then(|m| m.as_str().parse::<f64>().ok())
        .unwrap_or(f64::NAN)
}
