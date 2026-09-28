//! Public Claude-compatible remote-control surface.
//!
//! The transport is an Anthropic-operated relay and is not an open protocol.
//! LingXi deliberately exposes the discoverable command shape but fails before
//! creating a session instead of routing it to the unrelated local bridge.

use clap::{Args, ValueEnum};
use std::path::PathBuf;

/// How the proprietary relay would create sessions.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SpawnMode {
    /// Start sessions in the server's directory.
    SameDir,
    /// Create a worktree for each session.
    Worktree,
    /// Use the session's requested directory.
    Session,
}

/// `remote-control` command-line surface from Claude Code 2.1.216.
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// Human-readable remote-control server name.
    #[arg(long)]
    pub name: Option<String>,
    /// Prefix applied to remotely created session names.
    #[arg(long = "remote-control-session-name-prefix")]
    pub session_name_prefix: Option<String>,
    /// Continue the most recent local session.
    #[arg(short = 'c', long = "continue")]
    pub continue_session: bool,
    /// Resume a specific session.
    #[arg(long = "session-id", value_name = "uuid")]
    pub session_id: Option<String>,
    /// Initial permission mode for created sessions.
    #[arg(long = "permission-mode", value_name = "mode")]
    pub permission_mode: Option<String>,
    /// Write debug diagnostics to this path.
    #[arg(long = "debug-file", value_name = "path")]
    pub debug_file: Option<PathBuf>,
    /// Enable verbose diagnostics.
    #[arg(long)]
    pub verbose: bool,
    /// Session creation strategy.
    #[arg(long, value_enum, default_value = "same-dir")]
    pub spawn: SpawnMode,
    /// Maximum concurrently hosted sessions.
    #[arg(long, default_value_t = 32)]
    pub capacity: usize,
    /// Create requested session directories when absent.
    #[arg(
        long = "create-session-in-dir",
        overrides_with = "no_create_session_in_dir"
    )]
    pub create_session_in_dir: bool,
    /// Refuse to create requested session directories.
    #[arg(
        long = "no-create-session-in-dir",
        overrides_with = "create_session_in_dir"
    )]
    pub no_create_session_in_dir: bool,
}

/// Fail honestly: the relay/auth contract needed by this command is private.
pub async fn run(_cli: &Cli) -> i32 {
    eprintln!(
        "lingxi-cli remote-control: unavailable because the Anthropic relay/auth protocol is not provided."
    );
    eprintln!(
        "The local LingXi desktop bridge is intentionally not presented as Claude remote-control."
    );
    crate::exit_codes::NOT_IMPLEMENTED
}
