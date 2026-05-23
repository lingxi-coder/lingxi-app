//! Stdio MCP transport helpers — types only in this task; real spawn lives
//! per-platform.

use std::collections::HashMap;
use std::path::PathBuf;

/// Configuration passed to `spawn_stdio` on each platform crate.
#[derive(Debug, Clone)]
pub struct StdioConfig {
    /// Executable path or name (resolved via PATH if not absolute).
    pub cmd: String,
    /// Arguments passed verbatim to the child.
    pub args: Vec<String>,
    /// Environment variables (merged with parent env minus filtered secrets).
    pub env: HashMap<String, String>,
    /// Working directory; if `None`, child inherits the parent's cwd.
    pub cwd: Option<PathBuf>,
}
