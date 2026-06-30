//! Pure encode planning for qlipq: resolve persisted output settings into concrete encode options,
//! plan the hardware encoder + rate control, and estimate output size. No I/O — the in-process libav
//! export ([`qlipq-desktop`]'s `export` module) consumes these.

pub mod args;
pub mod estimate;
pub mod hw;

pub use args::*;
pub use estimate::*;
pub use hw::*;
