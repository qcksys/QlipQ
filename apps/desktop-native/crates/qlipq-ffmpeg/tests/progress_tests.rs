use qlipq_ffmpeg::progress::*;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn parse_timecode_to_seconds() {
    assert!(close(parse_timecode("00:00:01.500000").unwrap(), 1.5));
    assert!(close(parse_timecode("01:02:03").unwrap(), 3723.0));
    assert_eq!(parse_timecode("nope"), None);
}

#[test]
fn parse_progress_reads_out_time_us_and_continue() {
    let chunk = "frame=120\nout_time_us=2500000\nout_time=00:00:02.500000\nprogress=continue";
    let r = parse_progress(chunk);
    assert!(close(r.out_time_sec.unwrap(), 2.5));
    assert!(!r.done);
}

#[test]
fn parse_progress_detects_end() {
    let r = parse_progress("out_time_us=10000000\nprogress=end\n");
    assert!(close(r.out_time_sec.unwrap(), 10.0));
    assert!(r.done);
}

#[test]
fn parse_progress_falls_back_to_timecode() {
    let r = parse_progress("out_time=00:00:04.000000\nprogress=continue");
    assert!(close(r.out_time_sec.unwrap(), 4.0));
}

#[test]
fn progress_fraction_clamps() {
    assert!(close(progress_fraction(Some(5.0), 10.0), 0.5));
    assert!(close(progress_fraction(Some(20.0), 10.0), 1.0));
    assert!(close(progress_fraction(None, 10.0), 0.0));
    assert!(close(progress_fraction(Some(5.0), 0.0), 0.0));
}
