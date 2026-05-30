//! `UniFFI`-shaped Rust façade for the engine.
//!
//! M1.22 ships the Rust-side types only — `EngineHandle` and `SessionHandle`
//! are opaque handles that mirror the shape `UniFFI` will eventually expose to
//! Kotlin and Swift. The bindings themselves are deferred to M2 once
//! `edition2024`-clean `uniffi` releases are available workspace-wide; see the
//! design doc §D14 and the plan §Task 1 for the rationale.
//!
//! Today this crate is consumed only by host-side Rust code (cli-demo,
//! tests). All public types here MUST remain serializable / `Send + Sync` so
//! the future `UniFFI` layer can wrap them without changes.

#![forbid(unsafe_code)]

mod dto_conversions;
mod engine_handle;
mod session_handle;

pub use engine_handle::{EngineError, EngineHandle};
pub use session_handle::SessionHandle;
