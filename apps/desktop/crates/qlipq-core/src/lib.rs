//! Pure domain logic for qlipq (queue, edit spec, media info, config, OBS filename parsing,
//! rename templating). No I/O — the single source of truth for the app's domain model.

pub mod config;
pub mod config_json;
pub mod datetimes;
pub mod detect;
pub mod edit_spec;
pub mod highlight;
pub mod ids;
pub mod media;
pub mod obs;
pub mod queue;
pub mod rename;

pub use config::*;
pub use detect::*;
pub use edit_spec::*;
pub use media::*;
pub use obs::*;
pub use queue::*;
pub use rename::*;
