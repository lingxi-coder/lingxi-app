//! Platform abstraction traits.
//!
//! Engine crates depend on these traits; platform crates (`platforms/posix`,
//! `platforms/windows`, etc.) implement them. The engine never imports a
//! concrete runtime or OS API directly.
//!
//! See spec §4 (Trait System) and D17 (Runtime boundary).

#![forbid(unsafe_code)]

pub mod clock;
pub mod effect_handler;
pub mod filesystem;
pub mod http;
pub mod runtime;

pub use clock::Clock;
pub use effect_handler::EffectHandler;
pub use filesystem::{FileContent, FileSystem, FsError};
pub use http::{HttpError, HttpTransport};
pub use runtime::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};
