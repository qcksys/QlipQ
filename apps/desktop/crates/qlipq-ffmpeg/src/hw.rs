//! Hardware-encoder planning for the in-process export (the quality-model rework for Step 5).
//!
//! Pure logic, no libav: it maps the persisted [`OutputSettings`] quality model (designed around
//! x264's CRF/preset/bitrate) onto a **hardware** encoder plus its rate-control options, picking the
//! first encoder that actually opens on this machine (NVENC → AMF → QSV) via an injected `usable`
//! probe — so a vendor that's listed but unusable (e.g. QSV with no working iGPU) is skipped. This is
//! the HW analogue of [`crate::args::output_settings_to_encode`]; the desktop app supplies the runtime
//! `usable` probe and feeds the result to the libav encode pipeline.
//!
//! The CRF→constant-quality mapping is intentionally approximate — HW encoders don't share x264's CRF
//! scale, so we carry the user's CRF straight through as the encoder's CQ/QP and let per-vendor rate
//! control do the rest. It will not byte-match x264; that's expected for a HW re-encode.

use qlipq_core::config::{OutputSettings, QualityMode, QualityPreset, VideoCodecChoice};

/// A resolved hardware-encode plan: which encoder to open, its private options, and bitrate caps.
#[derive(Debug, Clone, PartialEq)]
pub struct HwVideo {
    /// libavcodec encoder name, e.g. `"hevc_nvenc"`.
    pub encoder: String,
    /// 10-bit pixel format (`p010le`) required — HDR or a 10-bit source — vs 8-bit (`nv12`).
    pub ten_bit: bool,
    /// Encoder-private options (an AVDictionary): preset, rate-control mode, constant-quality level.
    pub opts: Vec<(&'static str, String)>,
    /// Target average bitrate (kbps), set on the codec context when rate control uses a bitrate.
    pub bitrate_kbps: Option<i64>,
    /// Max bitrate cap (kbps) for constrained quality (CQ + a ceiling).
    pub maxrate_kbps: Option<i64>,
}

#[derive(Clone, Copy)]
enum Vendor {
    Nvenc,
    Amf,
    Qsv,
}

/// x264-style preset → NVENC `p1`..`p7` (p1 fastest … p7 slowest). The app exposes the x264 names.
fn nvenc_preset(x264: &str) -> &'static str {
    match x264 {
        "ultrafast" | "superfast" => "p1",
        "veryfast" => "p2",
        "faster" => "p3",
        "fast" => "p4",
        "medium" => "p5",
        "slow" => "p6",
        "slower" | "veryslow" => "p7",
        _ => "p5",
    }
}

/// x264-style preset → AMF `quality` usage bucket.
fn amf_quality(x264: &str) -> &'static str {
    match x264 {
        "ultrafast" | "superfast" | "veryfast" | "faster" => "speed",
        "slow" | "slower" | "veryslow" => "quality",
        _ => "balanced",
    }
}

fn preset_crf(p: QualityPreset) -> i64 {
    match p {
        QualityPreset::High => 18,
        QualityPreset::Balanced | QualityPreset::Original => 23,
        QualityPreset::Small => 28,
    }
}

/// Resolve the app's quality model to `(constant-quality, bitrate_kbps)`, either of which may be unset.
fn quality_intent(s: &OutputSettings) -> (Option<i64>, Option<i64>) {
    match s.quality_mode {
        QualityMode::Bitrate => (None, Some(s.video_bitrate_kbps)),
        QualityMode::Vbr => (Some(s.crf), Some(s.video_bitrate_kbps)), // quality + max bitrate
        QualityMode::Crf => (Some(s.crf), None),
        QualityMode::Preset => (Some(preset_crf(s.quality_preset)), None),
    }
}

/// Per-vendor rate-control: returns `(private opts, avg bitrate, max bitrate)`.
fn rate_control(vendor: Vendor, s: &OutputSettings) -> (Vec<(&'static str, String)>, Option<i64>, Option<i64>) {
    let (cq, bitrate) = quality_intent(s);
    let mut opts: Vec<(&'static str, String)> = Vec::new();
    let (mut br, mut maxr) = (None, None);
    match vendor {
        Vendor::Nvenc => {
            opts.push(("preset", nvenc_preset(&s.encoder_preset).to_string()));
            match (cq, bitrate) {
                (Some(c), Some(b)) => {
                    opts.push(("rc", "vbr".into()));
                    opts.push(("cq", c.to_string()));
                    maxr = Some(b); // constrained: quality target with a bitrate ceiling
                }
                (Some(c), None) => {
                    opts.push(("rc", "vbr".into()));
                    opts.push(("cq", c.to_string()));
                }
                (None, Some(b)) => {
                    opts.push(("rc", "vbr".into()));
                    br = Some(b);
                }
                (None, None) => {}
            }
        }
        Vendor::Amf => {
            opts.push(("quality", amf_quality(&s.encoder_preset).to_string()));
            match (cq, bitrate) {
                (Some(c), _) => {
                    opts.push(("rc", "cqp".into()));
                    opts.push(("qp_i", c.to_string()));
                    opts.push(("qp_p", c.to_string()));
                    maxr = bitrate;
                }
                (None, Some(b)) => {
                    opts.push(("rc", "vbr_peak".into()));
                    br = Some(b);
                }
                (None, None) => {}
            }
        }
        Vendor::Qsv => match (cq, bitrate) {
            (Some(c), _) => {
                opts.push(("global_quality", c.to_string()));
                maxr = bitrate;
            }
            (None, Some(b)) => br = Some(b),
            (None, None) => {}
        },
    }
    (opts, br, maxr)
}

/// Plan a hardware video encode for these settings. `ten_bit` (HDR or a 10-bit source) forces HEVC —
/// NVENC H.264 has no 10-bit profile. `usable(name)` reports whether an encoder actually opens on this
/// machine, so unusable vendors are skipped (NVENC → AMF → QSV). Returns `None` when no HW encoder is
/// usable, so the caller can keep the CLI/x264 path.
pub fn plan_hw_video(s: &OutputSettings, ten_bit: bool, usable: impl Fn(&str) -> bool) -> Option<HwVideo> {
    let want_hevc = ten_bit || s.video_codec == VideoCodecChoice::Libx265;
    let candidates: &[(&str, Vendor)] = if want_hevc {
        &[("hevc_nvenc", Vendor::Nvenc), ("hevc_amf", Vendor::Amf), ("hevc_qsv", Vendor::Qsv)]
    } else {
        &[("h264_nvenc", Vendor::Nvenc), ("h264_amf", Vendor::Amf), ("h264_qsv", Vendor::Qsv)]
    };
    let &(encoder, vendor) = candidates.iter().find(|(name, _)| usable(name))?;
    let (opts, bitrate_kbps, maxrate_kbps) = rate_control(vendor, s);
    Some(HwVideo { encoder: encoder.to_string(), ten_bit, opts, bitrate_kbps, maxrate_kbps })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(configure: impl FnOnce(&mut OutputSettings)) -> OutputSettings {
        let mut s = OutputSettings::default();
        configure(&mut s);
        s
    }

    #[test]
    fn ten_bit_forces_hevc_even_for_h264_choice() {
        let p = plan_hw_video(&settings(|s| s.video_codec = VideoCodecChoice::Libx264), true, |_| true).unwrap();
        assert_eq!(p.encoder, "hevc_nvenc");
        assert!(p.ten_bit);
    }

    #[test]
    fn h265_choice_picks_hevc() {
        let p = plan_hw_video(&settings(|s| s.video_codec = VideoCodecChoice::Libx265), false, |_| true).unwrap();
        assert_eq!(p.encoder, "hevc_nvenc");
    }

    #[test]
    fn falls_back_past_unusable_vendors() {
        // NVENC unusable (e.g. no NVIDIA GPU) → AMF; both unusable → QSV.
        let amf = plan_hw_video(&settings(|_| {}), false, |n| !n.contains("nvenc")).unwrap();
        assert_eq!(amf.encoder, "h264_amf");
        let qsv = plan_hw_video(&settings(|_| {}), false, |n| n.contains("qsv")).unwrap();
        assert_eq!(qsv.encoder, "h264_qsv");
    }

    #[test]
    fn none_when_no_hw_usable() {
        assert!(plan_hw_video(&settings(|_| {}), false, |_| false).is_none());
    }

    #[test]
    fn nvenc_crf_maps_to_cq_and_preset() {
        let p = plan_hw_video(
            &settings(|s| {
                s.quality_mode = QualityMode::Crf;
                s.crf = 20;
                s.encoder_preset = "slow".into();
            }),
            false,
            |_| true,
        )
        .unwrap();
        assert!(p.opts.contains(&("rc", "vbr".to_string())));
        assert!(p.opts.contains(&("cq", "20".to_string())));
        assert!(p.opts.contains(&("preset", "p6".to_string())));
        assert_eq!(p.bitrate_kbps, None);
    }

    #[test]
    fn nvenc_bitrate_mode_sets_bitrate() {
        let p = plan_hw_video(
            &settings(|s| {
                s.quality_mode = QualityMode::Bitrate;
                s.video_bitrate_kbps = 8000;
            }),
            false,
            |_| true,
        )
        .unwrap();
        assert_eq!(p.bitrate_kbps, Some(8000));
        assert!(p.opts.iter().all(|(k, _)| *k != "cq"));
    }

    #[test]
    fn nvenc_vbr_is_constrained_quality() {
        // VBR = CQ quality with a max-bitrate ceiling.
        let p = plan_hw_video(
            &settings(|s| {
                s.quality_mode = QualityMode::Vbr;
                s.crf = 22;
                s.video_bitrate_kbps = 9000;
            }),
            false,
            |_| true,
        )
        .unwrap();
        assert!(p.opts.contains(&("cq", "22".to_string())));
        assert_eq!(p.maxrate_kbps, Some(9000));
    }

    #[test]
    fn preset_quality_maps_named_preset_to_cq() {
        let p = plan_hw_video(&settings(|s| s.quality_preset = QualityPreset::High), false, |_| true).unwrap();
        assert!(p.opts.contains(&("cq", "18".to_string()))); // High → 18
    }
}
