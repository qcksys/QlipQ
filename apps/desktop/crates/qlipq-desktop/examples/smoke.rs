//! Phase-1 smoke test for the in-process libav (rsmpeg) toolchain. Opens a media file and prints
//! its duration + per-stream codec/dims. Success = it links, loads the shared FFmpeg DLLs, and
//! prints without a missing-DLL error (0xc0000135).
//!
//!   cargo run -p qlipq-iced --example smoke --features libav-preview -- "<path-to-clip>"

use std::ffi::CString;

use rsmpeg::avformat::AVFormatContextInput;

fn main() {
    let arg = std::env::args().nth(1).expect("usage: smoke <media path>");
    let path = CString::new(arg).expect("path has interior NUL");

    let ctx = AVFormatContextInput::open(&path).expect("failed to open input");

    let dur = ctx.duration as f64 / rsmpeg::ffi::AV_TIME_BASE as f64;
    println!("opened OK — duration {dur:.3}s, {} stream(s)", ctx.nb_streams);

    for (i, stream) in ctx.streams().iter().enumerate() {
        let par = stream.codecpar();
        let codec = rsmpeg::avcodec::AVCodec::find_decoder(par.codec_id)
            .map(|c| c.name().to_string_lossy().into_owned())
            .unwrap_or_else(|| "<no decoder>".into());
        println!(
            "  stream {i}: codec_type={} codec={codec} {}x{}",
            par.codec_type, par.width, par.height
        );
    }
}
