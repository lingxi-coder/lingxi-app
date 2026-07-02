//! `lingxi-cli agents` — Manage background agents (M7 cc2.1.198).
//!
//! Byte-faithful surface for the flat `agents` command, ported from the real
//! 2.1.198 binary's `agentsCommandHandler` (`_Gf` @223856744) +
//! `printAgentsJson` (`pGf` @223853400):
//!
//! * `--help`/`-h` → the captured fixture text VERBATIM (commander's layout;
//!   clap cannot render it, so help is a manual flag printing the locked
//!   fixture — same idiom as `gateway.rs`).
//! * `--json` → the live-session registry (`~/.lingxi/sessions/<pid>.json`)
//!   merged with the background-job store (`~/.lingxi/jobs/<short>/
//!   state.json`) as a pretty-printed JSON array (key order, filters, and
//!   sort byte-matched to the binary — see `crate::agents_registry`).
//!   `--all` includes completed jobs; `--cwd` filters by directory subtree.
//! * no `--json`, stdout not a TTY → the binary's exact refusal on stderr,
//!   exit 1.
//! * no `--json`, TTY → the interactive agent view (minimal usable port):
//!   bypass gates first (root refusal + consent dialog, binary
//!   `refuseBypassUnderRoot`/`ensureAgentsBypassConsent` @223855350), then a
//!   ratatui list grouped into the fleet-view bands; `Enter` attaches
//!   (respawns `lingxi-cli --resume <sessionId>` with the dispatch flags) and
//!   RETURNS TO THE VIEW when the attached session ends (2.1.198: leaving an
//!   attached session opens the agent view instead of exiting to shell);
//!   `q`/`Esc`/`Ctrl-C` exits.

use clap::Args;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

/// The locked `claude agents --help` text — byte-identical to the captured
/// fixture (`parity_claude_2_1_198.rs` pins the same bytes from the fixture
/// side; `cli_subcommand_stubs.rs` asserts this output end-to-end).
pub const AGENTS_HELP: &str =
    include_str!("../../../../test-harness/src/parity/fixtures/cc_2_1_198_agents_help.txt");

/// `agents` args — byte-match `claude agents --help` (options only; no
/// children). Repeatable options (`--add-dir`, `--mcp-config`, `--plugin-dir`)
/// take exactly ONE value per occurrence (claude treats a second bare token as
/// a stray positional), so they are plain `Vec` fields (clap's default append,
/// `num_args = 1`) — deliberately NOT the parent `Argv`'s greedy `num_args =
/// 1..`. Help is a MANUAL flag (fixture-verbatim output, commander layout).
// The bool count mirrors the binary's flag surface 1:1 — collapsing flags
// into enums would break clap's byte-locked argv contract.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Args)]
#[command(disable_help_flag = true)]
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

    /// Display help for command (manual: prints the locked fixture text).
    #[arg(short = 'h', long = "help")]
    pub help: bool,

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

    /// Like --plugin-dir but the engine will not read this plugin's .mcp.json
    /// (hidden in the binary's help too — `.hideHelp()`).
    #[arg(long = "plugin-dir-no-mcp", value_name = "path", hide = true)]
    pub plugin_dir_no_mcp: Vec<PathBuf>,

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

impl Cli {
    /// Whether bypass-permissions is requested for dispatched sessions
    /// (binary `nis`: `permissionMode === "bypassPermissions" || allowBypass`;
    /// `--dangerously-skip-permissions` is the documented alias).
    #[must_use]
    pub fn bypass_requested(&self) -> bool {
        self.dangerously_skip_permissions
            || self.allow_dangerously_skip_permissions
            || self.permission_mode.as_deref() == Some("bypassPermissions")
    }
}

/// Run the `agents` family.
pub async fn run(cli: &Cli) -> i32 {
    if cli.help {
        print!("{AGENTS_HELP}");
        return crate::exit_codes::SUCCESS;
    }
    if cli.json {
        return print_sessions_json(cli);
    }
    if !std::io::stdout().is_terminal() {
        // Binary `LIe("claude agents", "requires …")` — verified live
        // (stderr, exit 1). Command name branded, message bytes kept.
        eprintln!(
            "'lingxi-cli agents' requires an interactive terminal (stdout is not a TTY) \u{2014} use 'lingxi-cli agents --json' for a machine-readable listing."
        );
        return crate::exit_codes::RUNTIME_ERROR;
    }

    // (2.1.196) `claude agents --dangerously-skip-permissions` shows the
    // bypass disclaimer and applies bypass to dispatched sessions. Gates in
    // binary order: root refusal (`refuseBypassUnderRoot` — same message the
    // shared safety guard emits), then the consent dialog
    // (`ensureAgentsBypassConsent` — skipped when
    // `skipDangerousModePermissionPrompt` is already set, mirrored by
    // `mode::read_skip_dangerous_prompt`).
    if cli.bypass_requested() {
        if let Err(msg) =
            permission::enforce_bypass_safety(&crate::bypass_env::RealBypassEnv::new()).await
        {
            eprintln!("{msg}");
            return crate::exit_codes::RUNTIME_ERROR;
        }
        let skip_set = crate::mode::read_skip_dangerous_prompt();
        if tui::startup_bypass::should_show_bypass_dialog(true, skip_set) {
            match tui::startup_bypass::mount_bypass_dialog().await {
                Ok(tui::startup_bypass::BypassDialogOutcome::Accept) => {
                    crate::mode::persist_skip_dangerous_prompt();
                }
                Ok(tui::startup_bypass::BypassDialogOutcome::Decline) => {
                    return crate::exit_codes::RUNTIME_ERROR;
                }
                Err(e) => {
                    eprintln!("lingxi-cli agents: bypass dialog failed: {e}");
                    return crate::exit_codes::RUNTIME_ERROR;
                }
            }
        }
    }

    run_agents_view(cli)
}

/// Resolve the `--cwd` filter root: absolutize against the process cwd, then
/// canonicalize (binary `YE(resolve(cwd))` = realpath) with a lexical
/// fallback when the path does not exist (filters to nothing, like the
/// binary).
fn resolve_cwd_filter(cwd: Option<&Path>) -> Option<PathBuf> {
    let cwd = cwd?;
    let abs = std::path::absolute(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    Some(std::fs::canonicalize(&abs).unwrap_or(abs))
}

/// Print the live background/interactive sessions as a pretty JSON array
/// (exact `JSON.stringify(rows, null, 2)` bytes + trailing newline) and
/// return `SUCCESS`. An empty registry prints `[]`.
fn print_sessions_json(cli: &Cli) -> i32 {
    use crate::agents_registry as reg;
    let home = crate::run::lingxi_home_dir();
    let live = reg::read_live_sessions(&reg::sessions_dir(&home));
    let jobs = reg::read_jobs(&reg::jobs_dir(&home));
    let filter = resolve_cwd_filter(cli.cwd.as_deref());
    let rows = reg::build_agents_json(&live, &jobs, filter.as_deref(), cli.all);
    match serde_json::to_string_pretty(&rows) {
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

/// Build the interactive view's rows from the same registry + job store the
/// `--json` path reads (the view is the `--all` listing, minus this process's
/// own registration). PR references come from the job's detail/name text
/// (binary `Hon` token scan).
fn load_view_rows(cli: &Cli) -> Vec<tui_rata::agents_screen::AgentRow> {
    use crate::agents_registry as reg;
    use tui_rata::agents_screen::{extract_pr_number, AgentRow};

    let home = crate::run::lingxi_home_dir();
    let live = reg::read_live_sessions(&reg::sessions_dir(&home));
    let jobs = reg::read_jobs(&reg::jobs_dir(&home));
    let filter = resolve_cwd_filter(cli.cwd.as_deref());
    let self_pid = i32::try_from(std::process::id()).unwrap_or(i32::MAX);

    // Detail text per job (for the PR reference) — the JSON rows deliberately
    // omit it, so look it up by short id.
    let detail_by_short: std::collections::HashMap<&str, &str> = jobs
        .iter()
        .filter_map(|(short, job)| {
            job.detail.as_deref().map(|d| (short.as_str(), d))
        })
        .collect();

    reg::build_agents_json(&live, &jobs, filter.as_deref(), true)
        .into_iter()
        .filter_map(|row| {
            if row.get("pid").and_then(serde_json::Value::as_i64)
                == Some(i64::from(self_pid))
            {
                return None; // never list the view's own process
            }
            let get = |k: &str| row.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let name = {
                let n = get("name");
                if n.is_empty() { get("sessionId") } else { n }
            };
            let state = {
                // Job rows carry `state`; live-only rows only a status —
                // an interactive/live session is always "working".
                let s = get("state");
                if s.is_empty() { "working".to_string() } else { s }
            };
            let pr = row
                .get("id")
                .and_then(|v| v.as_str())
                .and_then(|short| detail_by_short.get(short))
                .and_then(|d| extract_pr_number(d))
                .or_else(|| extract_pr_number(&name));
            Some(AgentRow {
                session_id: get("sessionId"),
                name,
                state,
                kind: get("kind"),
                cwd: get("cwd"),
                pr,
            })
        })
        .collect()
}

/// The dispatch flags an attach forwards to the resumed session (subset of
/// the binary's respawn/dispatch defaults that lingxi's root argv accepts).
fn attach_args(cli: &Cli, session_id: &str) -> Vec<String> {
    let mut args = vec!["--resume".to_string(), session_id.to_string()];
    if let Some(mode) = &cli.permission_mode {
        args.extend(["--permission-mode".to_string(), mode.clone()]);
    }
    if cli.dangerously_skip_permissions {
        args.push("--dangerously-skip-permissions".to_string());
    }
    if let Some(model) = &cli.model {
        args.extend(["--model".to_string(), model.clone()]);
    }
    for dir in &cli.add_dir {
        args.extend(["--add-dir".to_string(), dir.display().to_string()]);
    }
    if let Some(settings) = &cli.settings {
        args.extend(["--settings".to_string(), settings.clone()]);
    }
    if let Some(sources) = &cli.setting_sources {
        args.extend(["--setting-sources".to_string(), sources.clone()]);
    }
    for cfg in &cli.mcp_config {
        args.extend(["--mcp-config".to_string(), cfg.clone()]);
    }
    if cli.strict_mcp_config {
        args.push("--strict-mcp-config".to_string());
    }
    args
}

/// Mount the agents view: draw/event loop on the alternate screen; `Enter`
/// attaches (terminal restored, `lingxi-cli --resume <sid>` runs to
/// completion, view remounts with FRESH registry rows — the 2.1.198 "return
/// to agent view, not shell" behavior); `q`/`Esc`/`Ctrl-C` exits.
///
/// Terminal IO only — every decision lives in the unit-tested
/// [`tui_rata::agents_screen::AgentsScreenState`]. Errors restore the
/// terminal and report on stderr.
fn run_agents_view(cli: &Cli) -> i32 {
    use tui_rata::agents_screen::{AgentsOutcome, AgentsScreenState};

    let mut state = AgentsScreenState::new(load_view_rows(cli));
    loop {
        let mut terminal = match tui_rata::setup_terminal() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("lingxi-cli agents: terminal setup failed: {e}");
                return crate::exit_codes::RUNTIME_ERROR;
            }
        };
        let outcome = tui_rata::agents_screen::run_view_loop(&mut state, &mut terminal);
        let _ = tui_rata::restore_terminal(&mut terminal);
        drop(terminal);
        match outcome {
            Ok(AgentsOutcome::Exit) => return crate::exit_codes::SUCCESS,
            Ok(AgentsOutcome::Attach(session_id)) => {
                // Attach = run the resumed session in the foreground; when it
                // ends, fall through and remount the view with fresh rows.
                let exe = std::env::current_exe()
                    .unwrap_or_else(|_| PathBuf::from("lingxi-cli"));
                let status = std::process::Command::new(exe)
                    .args(attach_args(cli, &session_id))
                    .status();
                if let Err(e) = status {
                    eprintln!("lingxi-cli agents: attach failed: {e}");
                    return crate::exit_codes::RUNTIME_ERROR;
                }
                state.reload(load_view_rows(cli));
            }
            Ok(AgentsOutcome::Stay) => unreachable!("view_loop only returns terminal outcomes"),
            Err(e) => {
                eprintln!("lingxi-cli agents: {e}");
                return crate::exit_codes::RUNTIME_ERROR;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, clap::Parser)]
    struct Harness {
        #[command(flatten)]
        agents: Cli,
    }

    fn parse(args: &[&str]) -> Cli {
        Harness::try_parse_from(std::iter::once("agents").chain(args.iter().copied()))
            .unwrap()
            .agents
    }

    #[test]
    fn bypass_requested_matches_binary_nis() {
        // nis: permissionMode === "bypassPermissions" || allowBypass; the
        // documented alias also counts.
        assert!(parse(&["--dangerously-skip-permissions"]).bypass_requested());
        assert!(parse(&["--allow-dangerously-skip-permissions"]).bypass_requested());
        assert!(parse(&["--permission-mode", "bypassPermissions"]).bypass_requested());
        assert!(!parse(&["--permission-mode", "plan"]).bypass_requested());
        assert!(!parse(&["--json"]).bypass_requested());
    }

    #[test]
    fn attach_forwards_bypass_to_dispatched_session() {
        // (2.1.196) "claude agents --dangerously-skip-permissions … applies
        // bypass to sessions dispatched from the agent view": the attach
        // respawn carries the bypass flag through.
        let args = attach_args(&parse(&["--dangerously-skip-permissions"]), "sid-1");
        assert_eq!(args[..2], ["--resume".to_string(), "sid-1".to_string()]);
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));

        let args = attach_args(
            &parse(&["--permission-mode", "bypassPermissions", "--model", "opus"]),
            "sid-2",
        );
        let joined = args.join(" ");
        assert!(joined.contains("--permission-mode bypassPermissions"));
        assert!(joined.contains("--model opus"));
    }
}
