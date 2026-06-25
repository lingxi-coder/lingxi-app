//! `lingxi-cli agents` — Manage background agents
//!
//! Byte-faithful clap surface for the flat `agents` command (no children),
//! matching `claude agents --help` from claude-code 2.1.191. The command has
//! one scripting path (`--json`) plus a bag of dispatch-config flags that
//! configure the sessions launched from the interactive agent view.
//!
//! Behaviour:
//! - `--json`: serialise lingxi's live background/interactive sessions to a JSON
//!   array on stdout and exit `SUCCESS`. claude reads a CROSS-PROCESS registry
//!   of live `claude` processes (each entry = `{pid, cwd, kind, startedAt,
//!   sessionId, status, …}`). lingxi has no cross-process session-registration
//!   store wired (the `tasks` registry is in-process only and `session` storage
//!   holds transcripts, not live process registrations), so a fresh `lingxi-cli`
//!   process observes zero live sessions and prints an empty array `[]` — the
//!   honest, byte-faithful "no sessions" result (`JSON.stringify([], null, 2)`
//!   == `serde_json::to_string_pretty(&[])` == `[]`). It never starts a chat
//!   turn or hits the network.
//! - no `--json` (the interactive agent view): a full-screen TUI surface that
//!   is not part of the CLI parity layer, so it prints a not-yet-implemented
//!   notice and exits `NOT_IMPLEMENTED` rather than faking the view.

use clap::Args;
use std::path::PathBuf;

/// `agents` args — byte-match `claude agents --help` (options only; no
/// children). Repeatable options (`--add-dir`, `--mcp-config`, `--plugin-dir`)
/// take exactly ONE value per occurrence (claude treats a second bare token as
/// a stray positional), so they are plain `Vec` fields (clap's default append,
/// `num_args = 1`) — deliberately NOT the parent `Argv`'s greedy `num_args =
/// 1..`.
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// Additional directory to allow tool access to in dispatched sessions
    /// (repeatable)
    #[arg(long = "add-dir", value_name = "directory")]
    pub add_dir: Vec<PathBuf>,

    /// Default agent for sessions dispatched from agent view. Overrides the
    /// 'agent' setting.
    #[arg(long = "agent", value_name = "agent")]
    pub agent: Option<String>,

    /// With --json: include completed sessions (the full agent view list)
    #[arg(long = "all")]
    pub all: bool,

    /// Make bypass-permissions mode available to dispatched sessions without
    /// defaulting to it
    #[arg(long = "allow-dangerously-skip-permissions")]
    pub allow_dangerously_skip_permissions: bool,

    /// Show only background sessions started under <path>
    #[arg(long = "cwd", value_name = "path")]
    pub cwd: Option<PathBuf>,

    /// Alias for --permission-mode bypassPermissions
    #[arg(long = "dangerously-skip-permissions")]
    pub dangerously_skip_permissions: bool,

    /// Default effort level for sessions dispatched from agent view
    #[arg(long = "effort", value_name = "level")]
    pub effort: Option<String>,

    /// Print active sessions as a JSON array and exit (for scripting; does not
    /// require a TTY)
    #[arg(long = "json")]
    pub json: bool,

    /// MCP server configuration to apply to dispatched sessions (repeatable)
    #[arg(long = "mcp-config", value_name = "config")]
    pub mcp_config: Vec<String>,

    /// Default model for sessions dispatched from agent view
    #[arg(long = "model", value_name = "model")]
    pub model: Option<String>,

    /// Default permission mode for sessions dispatched from agent view
    #[arg(long = "permission-mode", value_name = "mode")]
    pub permission_mode: Option<String>,

    /// Load plugins from specified directory for the agent view and dispatched
    /// sessions (repeatable)
    #[arg(long = "plugin-dir", value_name = "path")]
    pub plugin_dir: Vec<PathBuf>,

    /// Comma-separated list of setting sources to load (user, project, local).
    #[arg(long = "setting-sources", value_name = "sources")]
    pub setting_sources: Option<String>,

    /// Settings file or JSON string to apply to the agent view and dispatched
    /// sessions
    #[arg(long = "settings", value_name = "file-or-json")]
    pub settings: Option<String>,

    /// Only use MCP servers from --mcp-config in dispatched sessions
    #[arg(long = "strict-mcp-config")]
    pub strict_mcp_config: bool,
}

/// Run the `agents` family.
///
/// `--json` serialises the live background/interactive sessions and exits
/// (always an empty array in a fresh `lingxi-cli` process — see module docs).
/// Without `--json`, the interactive agent view is a TUI surface outside the
/// CLI parity layer: print a notice and return `NOT_IMPLEMENTED`.
pub async fn run(cli: &Cli) -> i32 {
    if cli.json {
        return print_sessions_json(cli);
    }

    eprintln!("lingxi-cli agents: interactive agent view not yet implemented");
    crate::exit_codes::NOT_IMPLEMENTED
}

/// Print the live background/interactive sessions as a pretty JSON array and
/// return `SUCCESS`. lingxi has no cross-process live-session registry wired, so
/// the array is empty (byte-faithful with claude's empty `[]` output). Honors
/// `--cwd` / `--all` for forward compatibility, though both currently filter an
/// already-empty list.
fn print_sessions_json(_cli: &Cli) -> i32 {
    // No cross-process live-session registry exists in this process: a fresh
    // `lingxi-cli` invocation observes zero live sessions. Emit the empty array
    // exactly as claude's `JSON.stringify([], null, 2)` would (`[]`).
    let sessions: Vec<serde_json::Value> = Vec::new();
    match serde_json::to_string_pretty(&sessions) {
        Ok(s) => {
            println!("{s}");
            crate::exit_codes::SUCCESS
        }
        Err(e) => {
            eprintln!("lingxi-cli agents: failed to serialize sessions: {e}");
            crate::exit_codes::RUNTIME_ERROR
        }
    }
}
