//! Trusted-directory enforcement — populated in Task 4.

use lingxi_telemetry::AnalyticsBus;
use std::path::{Path, PathBuf};

/// Placeholder — real value lands in Task 4.
pub const PATH_BLOCKED_EVENT: &str = "tengu_file_path_blocked";

/// Placeholder — real enum lands in Task 4.
#[derive(Debug, Clone)]
pub enum PathValidationError {
    /// Path resolves outside trusted directories.
    Outside {
        /// Path that was rejected.
        path: PathBuf,
    },
    /// I/O error during canonicalisation.
    Io(String),
}

impl std::fmt::Display for PathValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Outside { path } => {
                write!(f, "File path {} is outside trusted directories", path.display())
            }
            Self::Io(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for PathValidationError {}

/// Placeholder — real impl lands in Task 4.
pub fn canonicalize_and_validate(
    path: &Path,
    _trusted_dirs: &[PathBuf],
) -> Result<PathBuf, PathValidationError> {
    Ok(path.to_path_buf())
}

/// Placeholder — real impl lands in Task 4.
pub async fn emit_blocked_event(_bus: &AnalyticsBus, _tool_name: &str, _path: &Path) {}
