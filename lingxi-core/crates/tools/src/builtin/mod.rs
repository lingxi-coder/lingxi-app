//! Builtin tools — M4 sub-plans 01..08 land 40 implementations under this tree.
//!
//! M4-01 (this file) ships 6 foundation tools (file ops + search) and the
//! `register_all_builtin_tools` entrypoint that future sub-plans extend.

use crate::registry::ToolRegistry;
use lingxi_telemetry::AnalyticsBus;
use lingxi_traits::filesystem::FileSystem;
use std::path::PathBuf;
use std::sync::Arc;

pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod glob;
pub mod grep;
pub mod notebook_edit;

pub use file_edit::FileEditTool;
pub use file_read::FileReadTool;
pub use file_write::FileWriteTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use notebook_edit::NotebookEditTool;

/// Static surface every builtin tool needs at construction time.
///
/// Cloning is cheap — every field is `Arc` or a small owned vec.
#[derive(Clone)]
pub struct BuiltinToolContext {
    /// Sandboxed FS access (M1).
    pub fs: Arc<dyn FileSystem>,
    /// Telemetry bus (M3-06).
    pub bus: Arc<AnalyticsBus>,
    /// Canonicalised root directories the agent is allowed to read/write.
    pub trusted_dirs: Vec<PathBuf>,
}

/// Register every M4-01 foundation tool against `registry`.
///
/// Idempotent insertion: caller MUST pass an empty registry (or accept
/// duplicate registrations). Later sub-plans add Shell / Web / Workflow /
/// Agent / Team / MCP+LSP / System tools by extending this function.
///
/// NOTE (Task 1 skeleton): the tool impls land in Tasks 5/7/9/11/13/15.
/// Until those tasks wire the `Tool` trait, this function is intentionally
/// empty (placeholder structs cannot be cast to `Arc<dyn Tool>` yet). The
/// wiring is restored in Task 17 (registry wiring).
#[allow(unused_variables)]
pub fn register_all_builtin_tools(registry: &mut ToolRegistry, ctx: BuiltinToolContext) {
    // Wired in Tasks 5..16 when each tool's `impl Tool` lands.
}
