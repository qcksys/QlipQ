//! Generate small deterministic, two-track recordings with libav. No CLI or downloaded fixtures.
use rsmpeg::avcodec::{AVCodec, AVCodecContext, AVPacket};
use rsmpeg::avformat::AVFormatContextOutput;
use rsmpeg::avutil::{AVChannelLayout, AVFrame};
use rsmpeg::ffi;

pub fn recording(path: &std::path::Path) {
    recording_sized(path, 160, 90);
}

pub fn recording_sized(path: &std::path::Path, width: i32, height: i32) {
    let path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let mut output = AVFormatContextOutput::create(&path).unwrap();
    let video_time = ffi::AVRational { num: 1, den: 10 };
    let audio_time = ffi::AVRational {
        num: 1,
        den: 48_000,
    };
    let codec = AVCodec::find_encoder(ffi::AV_CODEC_ID_MPEG4).unwrap();
    let mut video = AVCodecContext::new(&codec);
    video.set_width(width);
    video.set_height(height);
    video.set_pix_fmt(ffi::AV_PIX_FMT_YUV420P);
    video.set_time_base(video_time);
    video.set_flags(ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32);
    unsafe {
        (*video.as_mut_ptr()).gop_size = 1;
    }
    video.open(None).unwrap();
    {
        let mut stream = output.new_stream();
        stream.set_codecpar(video.extract_codecpar());
        stream.set_time_base(video_time);
    }
    for _ in 0..2 {
        let codec = AVCodec::find_encoder(ffi::AV_CODEC_ID_PCM_S16LE).unwrap();
        let mut audio = AVCodecContext::new(&codec);
        audio.set_sample_rate(48_000);
        audio.set_sample_fmt(ffi::AV_SAMPLE_FMT_S16);
        audio.set_ch_layout(*AVChannelLayout::from_nb_channels(2));
        audio.set_time_base(audio_time);
        audio.open(None).unwrap();
        let mut stream = output.new_stream();
        stream.set_codecpar(audio.extract_codecpar());
        stream.set_time_base(audio_time);
    }
    output.write_header(&mut None).unwrap();
    for n in 0..100 {
        let mut frame = AVFrame::new();
        frame.set_width(width);
        frame.set_height(height);
        frame.set_format(ffi::AV_PIX_FMT_YUV420P);
        frame.set_pts(n);
        frame.alloc_buffer().unwrap();
        for plane in 0..3 {
            let (w, h) = if plane == 0 {
                (width as usize, height as usize)
            } else {
                (width as usize / 2, height as usize / 2)
            };
            for y in 0..h {
                // Each plane has an allocated linesize * height buffer; fill only the visible row.
                unsafe {
                    std::ptr::write_bytes(
                        frame.data[plane].add(y * frame.linesize[plane] as usize),
                        if plane == 0 { 48 + n as u8 } else { 128 },
                        w,
                    );
                }
            }
        }
        video.send_frame(Some(&frame)).unwrap();
        while let Ok(mut packet) = video.receive_packet() {
            packet.set_stream_index(0);
            packet.set_duration(1);
            packet.rescale_ts(video_time, output.streams()[0].time_base);
            output.interleaved_write_frame(&mut packet).unwrap();
        }
        for track in 1..=2 {
            let mut packet = AVPacket::new();
            assert_eq!(
                unsafe { ffi::av_new_packet(packet.as_mut_ptr(), 4800 * 4) },
                0
            );
            // PCM packet is 4800 stereo samples; independent tones make audio mix tests meaningful.
            let samples =
                unsafe { std::slice::from_raw_parts_mut(packet.data.cast::<i16>(), 4800 * 2) };
            for i in 0..4800 {
                let t = (n * 4800 + i as i64) as f64 / 48_000.0;
                let sample =
                    ((t * std::f64::consts::TAU * 220.0 * track as f64).sin() * 2000.0) as i16;
                samples[2 * i] = sample;
                samples[2 * i + 1] = sample;
            }
            packet.set_stream_index(track);
            packet.set_pts(n * 4800);
            packet.set_dts(n * 4800);
            packet.set_duration(4800);
            packet.rescale_ts(audio_time, output.streams()[track as usize].time_base);
            output.interleaved_write_frame(&mut packet).unwrap();
        }
    }
    video.send_frame(None).unwrap();
    while let Ok(mut packet) = video.receive_packet() {
        packet.set_stream_index(0);
        packet.rescale_ts(video_time, output.streams()[0].time_base);
        output.interleaved_write_frame(&mut packet).unwrap();
    }
    output.write_trailer().unwrap();
}

pub fn audio_amplitudes(path: &str) -> [f64; 2] {
    let path = std::ffi::CString::new(path).unwrap();
    let mut input = rsmpeg::avformat::AVFormatContextInput::open(&path).unwrap();
    let index = input
        .streams()
        .iter()
        .position(|s| s.codecpar().codec_type == ffi::AVMEDIA_TYPE_AUDIO)
        .unwrap();
    let par = input.streams()[index].codecpar();
    let codec = AVCodec::find_decoder(par.codec_id).unwrap();
    let mut decoder = AVCodecContext::new(&codec);
    decoder.apply_codecpar(&par).unwrap();
    drop(par);
    decoder.open(None).unwrap();
    let mut samples = Vec::new();
    loop {
        let packet = input.read_packet().unwrap();
        if packet
            .as_ref()
            .is_some_and(|p| p.stream_index != index as i32)
        {
            continue;
        }
        decoder.send_packet(packet.as_ref()).unwrap();
        while let Ok(frame) = decoder.receive_frame() {
            assert_eq!(frame.format, ffi::AV_SAMPLE_FMT_FLTP);
            assert_eq!(frame.sample_rate, 48_000);
            // The decoder owns nb_samples planar f32 values per channel until this frame drops.
            samples.extend_from_slice(unsafe {
                std::slice::from_raw_parts(frame.data[0].cast::<f32>(), frame.nb_samples as usize)
            });
        }
        if packet.is_none() {
            break;
        }
    }
    // Skip codec priming, then measure both fixture tones with phase-independent correlation.
    let samples = &samples[24_000..72_000];
    [220.0, 440.0].map(|frequency| {
        let (mut sin, mut cos) = (0.0, 0.0);
        for (index, sample) in samples.iter().enumerate() {
            let phase = std::f64::consts::TAU * frequency * index as f64 / 48_000.0;
            sin += f64::from(*sample) * phase.sin();
            cos += f64::from(*sample) * phase.cos();
        }
        2.0 * sin.hypot(cos) / samples.len() as f64
    })
}
