//! Attribute libav's log output to the file being processed.
//!
//! libav's default `av_log` callback prints demuxer/decoder errors — e.g. a truncated or
//! still-recording clip's "missing mandatory atoms, broken header", "No sequence header available",
//! or "Invalid NAL unit size" — to stderr with no hint of *which* file triggered them. We install a
//! callback that formats each line exactly like the default one but prefixes it with the file the
//! current thread is decoding/probing/exporting, tracked by a thread-local that the decode entry
//! points set via [`enter`]. Decode threads are single-file for their lifetime, so the guard is just
//! held for the function body.

use std::cell::RefCell;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};

use rsmpeg::ffi;

thread_local! {
    static CURRENT_FILE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// While alive, libav log lines emitted on **this thread** are prefixed with the file it guards.
/// Dropping restores whatever file (if any) was active before, so nested scopes are safe.
pub struct FileScope(Option<String>);

impl Drop for FileScope {
    fn drop(&mut self) {
        CURRENT_FILE.with(|c| *c.borrow_mut() = self.0.take());
    }
}

/// Tag libav output on this thread as belonging to `path` until the returned guard drops.
#[must_use]
pub fn enter(path: &str) -> FileScope {
    FileScope(CURRENT_FILE.with(|c| c.borrow_mut().replace(path.to_owned())))
}

/// The file the current thread is working on, if a [`FileScope`] is active. Lets qlipq's own log
/// lines (e.g. the decoder-choice note) name the file too, without threading it through signatures.
#[must_use]
pub fn current() -> Option<String> {
    CURRENT_FILE.with(|c| c.borrow().clone())
}

/// Install the file-attributing libav log callback and quiet everything below `AV_LOG_ERROR`
/// (matching the prior `av_log_set_level` — keeps the harmless per-open INFO chatter out).
pub fn install() {
    unsafe {
        ffi::av_log_set_level(ffi::AV_LOG_ERROR as i32);
        ffi::av_log_set_callback(Some(log_callback));
    }
}

/// libav log sink: format the line like the default callback, then prefix the active file. `vl` is
/// forwarded straight into `av_log_format_line2` (no `va_arg` on our side); on Windows x86_64
/// `va_list` is a plain pointer, so this hand-off is a by-value copy.
unsafe extern "C" fn log_callback(
    avcl: *mut c_void,
    level: c_int,
    fmt: *const c_char,
    vl: ffi::va_list,
) {
    // Mirror the default callback's level gate — coloring/print flags live above the low byte.
    let severity = if level >= 0 { level & 0xff } else { level };
    if severity > ffi::av_log_get_level() {
        return;
    }
    let mut line = [0 as c_char; 1024];
    let mut print_prefix: c_int = 1;
    ffi::av_log_format_line2(
        avcl,
        level,
        fmt,
        vl,
        line.as_mut_ptr(),
        line.len() as c_int,
        &mut print_prefix,
    );
    let msg = CStr::from_ptr(line.as_ptr()).to_string_lossy();
    match current() {
        Some(path) => eprint!("qlipq[{path}]: {msg}"),
        None => eprint!("{msg}"),
    }
}
