//! ffmpeg/ffprobe argument building & output parsing — a Rust port of `@qcksys/qlipq-ffmpeg`.
//! This is the single source of truth for the ffmpeg command line; the host only spawns it.

pub mod args;
pub mod estimate;
pub mod probe;
pub mod progress;

pub use args::*;
pub use estimate::*;
pub use probe::*;
pub use progress::*;
