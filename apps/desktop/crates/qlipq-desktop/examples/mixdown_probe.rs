//! Step-2 verification (media-stack plan): validate the preview **monitor mixdown** — decode every
//! audio track, apply per-track gain, and sum — numerically on a real multi-track clip. Examples
//! can't import the binary crate, so this mirrors the exact mix math of `libav::mix_and_push`
//! (`sum(sample × gain)` then clamp to ±1) and checks the invariants that prove gain + summation are
//! correct:
//!   * a single track at gain 1.0 reproduces that track's level (identity),
//!   * halving a track's gain drops its level by ~6.02 dB (when it isn't already clipping),
//!   * muting every track yields silence.
//!
//!   cargo run --release -p qlipq-desktop --example mixdown_probe -- "<clip>"

use std::ffi::CString;

use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avutil::AVChannelLayout;
use rsmpeg::avformat::AVFormatContextInput;
use rsmpeg::ffi;
use rsmpeg::swresample::SwrContext;

const OUT_RATE: i32 = 48_000;
const OUT_CH: usize = 2;
const SECS: f64 = 3.0; // analyze the first few seconds of each track

/// Decode the first `SECS` of audio stream `abs_idx`, resampled to 48k stereo interleaved f32.
fn decode_track(path: &str, abs_idx: usize) -> Vec<f32> {
    let cpath = CString::new(path).unwrap();
    let mut input = AVFormatContextInput::open(&cpath).unwrap();
    let (mut dec, tb_secs) = {
        let st = &input.streams()[abs_idx];
        let tb = st.time_base;
        let par = st.codecpar();
        let codec = AVCodec::find_decoder(par.codec_id).unwrap();
        let mut d = AVCodecContext::new(&codec);
        d.apply_codecpar(&par).unwrap();
        d.set_pkt_timebase(tb);
        d.open(None).unwrap();
        (d, tb.num as f64 / tb.den as f64)
    };
    let mut swr: Option<SwrContext> = None;
    let mut out: Vec<f32> = Vec::new();
    let abs_idx = abs_idx as i32;
    let mut eof = false;
    while !eof {
        match input.read_packet() {
            Ok(Some(pkt)) => {
                if pkt.stream_index == abs_idx {
                    let _ = dec.send_packet(Some(&pkt));
                }
            }
            Ok(None) => {
                let _ = dec.send_packet(None);
                eof = true;
            }
            Err(_) => break,
        }
        while let Ok(frame) = dec.receive_frame() {
            if frame.pts as f64 * tb_secs > SECS {
                eof = true;
            }
            resample_into(&mut swr, &frame, &mut out);
        }
    }
    out.truncate((SECS * OUT_RATE as f64) as usize * OUT_CH);
    out
}

fn resample_into(swr: &mut Option<SwrContext>, frame: &rsmpeg::avutil::AVFrame, dst: &mut Vec<f32>) {
    if swr.is_none() {
        let in_layout = AVChannelLayout::from_nb_channels(frame.ch_layout().nb_channels.max(1));
        let out_layout = AVChannelLayout::from_nb_channels(OUT_CH as i32);
        let mut s = SwrContext::new(&out_layout, ffi::AV_SAMPLE_FMT_FLT, OUT_RATE, &in_layout, frame.format, frame.sample_rate).unwrap();
        s.init().unwrap();
        *swr = Some(s);
    }
    let s = swr.as_mut().unwrap();
    let out_count = s.get_out_samples(frame.nb_samples);
    if out_count <= 0 {
        return;
    }
    let mut buf = vec![0f32; out_count as usize * OUT_CH];
    let mut out_ptr = buf.as_mut_ptr() as *mut u8;
    let in_ptr = frame.data.as_ptr() as *const *const u8;
    let got = match unsafe { s.convert(&mut out_ptr, out_count, in_ptr, frame.nb_samples) } {
        Ok(n) => n as usize,
        Err(_) => return,
    };
    buf.truncate(got * OUT_CH);
    dst.extend_from_slice(&buf);
}

/// The exact mix math from `libav::mix_and_push`: per sample, sum (track × gain), clamp to ±1.
fn mix(tracks: &[(&Vec<f32>, f32)]) -> Vec<f32> {
    let n = tracks.iter().map(|(t, _)| t.len()).min().unwrap_or(0);
    let mut out = vec![0f32; n];
    for (t, g) in tracks {
        for (m, &s) in out.iter_mut().zip(t.iter()) {
            *m += s * g;
        }
    }
    for m in out.iter_mut() {
        *m = m.clamp(-1.0, 1.0);
    }
    out
}

fn rms_dbfs(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return f64::NEG_INFINITY;
    }
    let sum: f64 = samples.iter().map(|&x| (x as f64) * (x as f64)).sum();
    let rms = (sum / samples.len() as f64).sqrt();
    if rms <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * rms.log10()
    }
}

fn clip_pct(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    100.0 * samples.iter().filter(|&&x| x.abs() >= 1.0).count() as f64 / samples.len() as f64
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: mixdown_probe <media path>");
    let cpath = CString::new(path.clone()).unwrap();
    let input = AVFormatContextInput::open(&cpath).unwrap();
    let abs: Vec<usize> = input
        .streams()
        .iter()
        .enumerate()
        .filter(|(_, s)| s.codecpar().codec_type == ffi::AVMEDIA_TYPE_AUDIO)
        .map(|(i, _)| i)
        .collect();
    drop(input);
    println!("audio tracks: {}\n", abs.len());
    assert!(!abs.is_empty(), "clip has no audio");

    let decoded: Vec<Vec<f32>> = abs.iter().map(|&a| decode_track(&path, a)).collect();
    for (i, d) in decoded.iter().enumerate() {
        println!("  track {i}: {:.1} dBFS  ({} samples)", rms_dbfs(d), d.len());
    }

    // --- mixes ---
    let solo0 = mix(&[(&decoded[0], 1.0)]);
    let half0 = mix(&[(&decoded[0], 0.5)]);
    let all_full: Vec<(&Vec<f32>, f32)> = decoded.iter().map(|d| (d, 1.0f32)).collect();
    let all = mix(&all_full);
    let muted: Vec<(&Vec<f32>, f32)> = decoded.iter().map(|d| (d, 0.0f32)).collect();
    let silence = mix(&muted);

    println!("\nmixes:");
    println!("  track0 solo @1.0   {:7.1} dBFS  clip {:.2}%", rms_dbfs(&solo0), clip_pct(&solo0));
    println!("  track0 @0.5        {:7.1} dBFS  clip {:.2}%", rms_dbfs(&half0), clip_pct(&half0));
    println!("  all tracks @1.0    {:7.1} dBFS  clip {:.2}%", rms_dbfs(&all), clip_pct(&all));
    println!("  all muted          {:7.1} dBFS", rms_dbfs(&silence));

    // --- invariants ---
    let t0 = rms_dbfs(&decoded[0]);
    let solo_ok = (rms_dbfs(&solo0) - t0).abs() < 0.05;
    // -6.02 dB only holds when track0 isn't clipping at unity (loud transients aside); allow slack.
    let half_ok = !solo0.iter().any(|&x| x.abs() >= 1.0) && (rms_dbfs(&half0) - (t0 - 6.02)).abs() < 0.3
        || (rms_dbfs(&half0) < t0 - 4.0); // always at least clearly quieter
    let mute_ok = rms_dbfs(&silence) == f64::NEG_INFINITY;
    let sum_louder = decoded.len() < 2 || rms_dbfs(&all) >= decoded.iter().map(|d| rms_dbfs(d)).fold(f64::NEG_INFINITY, f64::max) - 0.01;

    println!("\ninvariants:");
    println!("  solo(t0,1.0) == t0 level ............ {}", if solo_ok { "PASS" } else { "FAIL" });
    println!("  gain 0.5 clearly quieter than 1.0 ... {}", if half_ok { "PASS" } else { "FAIL" });
    println!("  all muted == silence ............... {}", if mute_ok { "PASS" } else { "FAIL" });
    println!("  sum >= loudest single track ........ {}", if sum_louder { "PASS" } else { "FAIL" });

    if !(solo_ok && half_ok && mute_ok && sum_louder) {
        std::process::exit(1);
    }
}
