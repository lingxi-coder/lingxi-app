//! M3-01 — 4-layer settings loader (env > user > project > defaults).
//!
//! Entry point: [`Settings::load`]. Per-field merge rules live in
//! [`merger`]; provenance for `/doctor` (M6) lives in [`tracer`].
//!
//! See spec §3 module layout and §4 Flow D for the full data flow.

use std::path::PathBuf;

pub mod env_parser;
pub mod loader;
pub mod merger;
pub mod schema;
pub mod tracer;

/// Errors returned by [`Settings::load`] and its sub-modules.
///
/// Mirrors spec §5 `SettingsError`.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// A settings file at this path could not be found.
    #[error("settings file not found: {0}")]
    Missing(PathBuf),
    /// JSON parse failure for the file at this path.
    #[error("settings file malformed at {path}: {source}")]
    ParseError {
        /// The file whose JSON failed to parse.
        path: PathBuf,
        /// The underlying serde error.
        #[source]
        source: serde_json::Error,
    },
    /// An env var holds a value that can't be coerced into its target type.
    #[error("env var {var} has invalid value {value:?}")]
    InvalidEnv {
        /// The env var name (e.g. `LINGXI_TRUSTED_DIRECTORIES`).
        var: String,
        /// The raw value as received from the process env.
        value: String,
    },
    /// Schema validation rejected the file (unknown field, type mismatch).
    #[error("schema validation failed: {0}")]
    SchemaViolation(String),
    /// Underlying IO failure (permission denied, etc.).
    #[error("io error reading {path}: {source}")]
    Io {
        /// The file the I/O happened on.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}
