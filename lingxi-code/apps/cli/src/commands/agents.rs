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

    // (M8 cc2.1.198) The view fires the user's `Notification` hook on
    // background-agent band transitions (`agent_needs_input` /
    // `agent_completed`) — the binary's FleetView `hFc` diff (@222750113).
    // Built here (async context) so the sync view loop can fire through the
    // captured runtime handle.
    let watcher = NotificationWatcher::new(&crate::run::lingxi_home_dir()).await;
    run_agents_view(cli, watcher)
}

/// Fires the `Notification` hook for background-agent band transitions —
/// lingxi's counterpart of the binary's FleetView notification pipeline
/// (`hFc`/`$1f` @222691698 diff + `TQ` @219455460 hook fire).
///
/// The standalone agents view has no orchestrator, so the watcher loads the
/// settings-file hooks itself (user then project tier, project last so it
/// wins — the same standalone loader engine-desktop's composition root uses)
/// and executes them through a minimal `HookExecutorImpl`. When no
/// `Notification` hook is registered the watcher is INERT (`executor: None`)
/// — no polling work, mirroring the orchestrator's `has_notification_hook`
/// gate.
///
/// Depth note: the binary ALSO shows an OS notification (`QQ` → iTerm2 /
/// kitty / bell per `preferredNotifChannel`); lingxi has no OS-notifier port
/// yet, so only the hook side fires (the changelog surface for 2.1.198 is
/// the hook reasons).
pub(crate) struct NotificationWatcher {
    executor: Option<std::sync::Arc<hooks::HookExecutorImpl>>,
    prev: std::collections::HashMap<String, crate::agents_notify::AgentBand>,
    handle: Option<tokio::runtime::Handle>,
    home: PathBuf,
}

impl NotificationWatcher {
    /// Load settings hooks and arm the watcher when a `Notification` hook
    /// exists. Best-effort: malformed settings tiers are skipped exactly like
    /// the composition root's loader.
    pub(crate) async fn new(home: &Path) -> Self {
        use std::sync::Arc;

        let mut registry = hooks::HookRegistry::new();
        let user_settings = home.join("settings.json");
        let project_settings = std::env::current_dir()
            .unwrap_or_default()
            .join(branding::DOT_DIR)
            .join("settings.json");
        for (path, source) in [
            (user_settings, hooks::definition::HookSource::User),
            (project_settings, hooks::definition::HookSource::Project),
        ] {
            if let Ok(raw) = std::fs::read_to_string(&path) {
                if let Ok(defs) = hooks::parse_hooks_from_settings_json(&raw, source) {
                    for h in defs {
                        registry.register(h);
                    }
                }
            }
        }
        let armed = registry.has_hooks_for(&hooks::events::HookEventType::Notification);
        // H-BIN-12: no `.with_http_hook_policy(...)` here — this executor only
        // fires `Notification` hooks, and the HTTP arm supports PreToolUse /
        // PostToolUse only (`build_envelope_body`), so it never dispatches an
        // HTTP request. The default (no-restriction) policy is therefore correct.
        let executor = armed.then(|| {
            Arc::new(
                hooks::HookExecutorImpl::new(
                    Arc::new(tokio::sync::RwLock::new(registry)),
                    Arc::new(platform_posix::PosixHttp::new()) as Arc<dyn traits::HttpTransport>,
                    Arc::new(platform_posix::PosixRuntime::new())
                        as Arc<dyn traits::RuntimeSpawner>,
                )
                .with_process_runner(
                    Arc::new(platform_posix::PosixProcess::new()) as Arc<dyn traits::ProcessRunner>,
                    Arc::new(platform_posix::PosixSandbox::new()) as Arc<dyn traits::Sandbox>,
                ),
            )
        });
        Self {
            executor,
            prev: std::collections::HashMap::new(),
            handle: tokio::runtime::Handle::try_current().ok(),
            home: home.to_path_buf(),
        }
    }

    /// One observation: re-read the job store, diff bands, fire the hook for
    /// each raised notification. Called from the view's refresh tick and
    /// after each attach-remount.
    pub(crate) fn observe(&mut self) {
        let Some(executor) = &self.executor else {
            return;
        };
        let Some(handle) = &self.handle else {
            return;
        };
        use crate::agents_registry as reg;
        let jobs = reg::read_jobs(&reg::jobs_dir(&self.home));
        // `current_job_id = None`: the standalone view process is not itself
        // a background job (binary `t` = the CURRENT session's job id).
        let rows = crate::agents_notify::notify_rows(&jobs, None);
        let (next, notifications) = crate::agents_notify::detect_transitions(&self.prev, &rows);
        self.prev = next;
        for n in notifications {
            // `TQ`: `{...base, hook_event_name: "Notification", message,
            // title, notification_type}` with matchQuery = notification_type.
            // Best-effort fire-and-forget — a failing hook never disturbs the
            // view loop (the binary discards the aggregate too).
            let executor = executor.clone();
            let cwd = std::env::current_dir().unwrap_or_default();
            handle.spawn(async move {
                let _ = executor
                    .execute(
                        hooks::events::HookEvent::Notification {
                            message: n.message,
                            kind: n.notification_type.to_string(),
                        },
                        hooks::HookContext {
                            cwd,
                            ..Default::default()
                        },
                    )
                    .await;
            });
        }
    }

    /// Whether the watcher has a subscriber (drives the view's poll tick —
    /// no hook ⇒ pure blocking-read loop, zero idle wakeups).
    pub(crate) fn armed(&self) -> bool {
        self.executor.is_some() && self.handle.is_some()
    }
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
fn load_view_rows(cli: &Cli) -> Vec<tui::agents_screen::AgentRow> {
    use crate::agents_registry as reg;
    use tui::agents_screen::{extract_pr_number, AgentRow};

    let home = crate::run::lingxi_home_dir();
    let live = reg::read_live_sessions(&reg::sessions_dir(&home));
    let jobs = reg::read_jobs(&reg::jobs_dir(&home));
    let filter = resolve_cwd_filter(cli.cwd.as_deref());
    let self_pid = i32::try_from(std::process::id()).unwrap_or(i32::MAX);

    // Detail text per job (for the PR reference) — the JSON rows deliberately
    // omit it, so look it up by short id.
    let detail_by_short: std::collections::HashMap<&str, &str> = jobs
        .iter()
        .filter_map(|(short, job)| job.detail.as_deref().map(|d| (short.as_str(), d)))
        .collect();

    reg::build_agents_json(&live, &jobs, filter.as_deref(), true)
        .into_iter()
        .filter_map(|row| {
            if row.get("pid").and_then(serde_json::Value::as_i64) == Some(i64::from(self_pid)) {
                return None; // never list the view's own process
            }
            let get = |k: &str| {
                row.get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let name = {
                let n = get("name");
                if n.is_empty() {
                    get("sessionId")
                } else {
                    n
                }
            };
            let state = {
                // Job rows carry `state`; live-only rows only a status —
                // an interactive/live session is always "working".
                let s = get("state");
                if s.is_empty() {
                    "working".to_string()
                } else {
                    s
                }
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveAttachTarget {
    short: String,
    session_id: String,
    socket: PathBuf,
    auth: String,
}

/// Locate the authenticated live-attach socket for `session_id`. Pure over the
/// job store + roster + liveness probe so the "attach live worker, do not
/// double-`--resume`" decision is unit-testable.
fn live_attach_target_from(
    jobs: &[(String, crate::agents_registry::JobState)],
    roster: &crate::daemon_roster::Roster,
    session_id: &str,
    is_alive: &dyn Fn(i32) -> bool,
) -> Option<LiveAttachTarget> {
    for (short, job) in jobs {
        if job.session_id.as_deref() != Some(session_id) {
            continue;
        }
        let Some(worker_pid) = job.worker_pid else {
            continue;
        };
        if !is_alive(worker_pid) {
            continue;
        }
        let Some(record) = roster.workers.get(short) else {
            continue;
        };
        if !record.session_id.is_empty() && record.session_id != session_id {
            continue;
        }
        let socket = record
            .pty_sock
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                (!record.rendezvous_sock.is_empty()).then_some(record.rendezvous_sock.as_str())
            })?;
        let auth = record
            .pty_auth
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| record.rv_auth.as_deref().filter(|s| !s.is_empty()))?;
        return Some(LiveAttachTarget {
            short: short.clone(),
            session_id: session_id.to_string(),
            socket: PathBuf::from(socket),
            auth: auth.to_string(),
        });
    }
    None
}

/// Production wrapper over [`live_attach_target_from`]: read jobs + roster and
/// probe worker liveness via the system proc probe.
fn find_live_attach_target(home: &Path, session_id: &str) -> Option<LiveAttachTarget> {
    use crate::agents_registry as reg;
    let jobs = reg::read_jobs(&reg::jobs_dir(home));
    let roster = crate::daemon_roster::read_roster(
        home,
        i32::try_from(std::process::id()).unwrap_or(0),
        false,
    )
    .into_roster();
    let probe = crate::daemon_roster::SystemProbe;
    live_attach_target_from(&jobs, &roster, session_id, &|pid| {
        crate::daemon_roster::ProcProbe::is_alive(&probe, pid)
    })
}

fn attach_to_live_worker(target: &LiveAttachTarget) -> std::io::Result<()> {
    crate::bg_attach::attach_to_socket(&target.socket, &target.auth)
}

/// Whether any known job driving `session_id` is still executing under a LIVE
/// worker process. Pure over the job store + a liveness probe so it is
/// unit-testable.
///
/// Attaching a fresh `lingxi-cli --resume <sid>` to such a session opens a
/// SECOND writer against the same `<sessionId>.jsonl` transcript while the
/// daemon's `__bg-run` worker is still writing it — the concurrent-writer
/// hazard. The preferred path is [`find_live_attach_target`]; this guard remains
/// as a safe fallback for legacy/stale live workers that have no attach socket.
fn job_has_live_worker(
    jobs: &[(String, crate::agents_registry::JobState)],
    session_id: &str,
    is_alive: &dyn Fn(i32) -> bool,
) -> bool {
    jobs.iter().any(|(_short, job)| {
        job.session_id.as_deref() == Some(session_id) && job.worker_pid.is_some_and(is_alive)
    })
}

/// Production wrapper over [`job_has_live_worker`]: read the job store and probe
/// worker liveness via the system proc probe.
fn session_has_live_worker(home: &Path, session_id: &str) -> bool {
    use crate::agents_registry as reg;
    let jobs = reg::read_jobs(&reg::jobs_dir(home));
    let probe = crate::daemon_roster::SystemProbe;
    job_has_live_worker(&jobs, session_id, &|pid| {
        crate::daemon_roster::ProcProbe::is_alive(&probe, pid)
    })
}

/// Mount the agents view: draw/event loop on the alternate screen; `Enter`
/// attaches (terminal restored, `lingxi-cli --resume <sid>` runs to
/// completion, view remounts with FRESH registry rows — the 2.1.198 "return
/// to agent view, not shell" behavior); `q`/`Esc`/`Ctrl-C` exits.
///
/// (M8 cc2.1.198) When a `Notification` hook is registered, the loop runs a
/// periodic refresh tick: registry rows reload and the watcher diffs bands to
/// fire `agent_needs_input` / `agent_completed` (binary: the FleetView jobs
/// poll re-renders and `hFc` diffs per render; the 1s cadence here is the
/// host loop's choice, not a binary constant). With no hook the loop stays a
/// pure blocking read (zero idle wakeups — the M7 behavior, byte-identical).
///
/// Terminal IO only — every decision lives in the unit-tested
/// [`tui::agents_screen::AgentsScreenState`] +
/// [`crate::agents_notify::detect_transitions`]. Errors restore the terminal
/// and report on stderr.
fn run_agents_view(cli: &Cli, mut watcher: NotificationWatcher) -> i32 {
    use tui::agents_screen::{AgentsOutcome, AgentsScreenState};

    // Seed the band map BEFORE the first render so a view opened onto an
    // already-blocked job stays quiet ($1f: first observation records, never
    // notifies) — then observe on every refresh.
    watcher.observe();
    let tick = watcher
        .armed()
        .then(|| std::time::Duration::from_millis(1000));
    let mut state = AgentsScreenState::new(load_view_rows(cli));
    loop {
        let mut terminal = match tui::setup_terminal() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("lingxi-cli agents: terminal setup failed: {e}");
                return crate::exit_codes::RUNTIME_ERROR;
            }
        };
        let outcome = tui::agents_screen::run_view_loop_with_tick(
            &mut state,
            &mut terminal,
            tick,
            &mut |view| {
                watcher.observe();
                view.reload(load_view_rows(cli));
            },
        );
        let _ = tui::restore_terminal(&mut terminal);
        drop(terminal);
        match outcome {
            Ok(AgentsOutcome::Exit) => return crate::exit_codes::SUCCESS,
            Ok(AgentsOutcome::Attach(session_id)) => {
                // Live background attach: connect to the worker's authenticated
                // socket instead of spawning a second `--resume` writer against
                // the same JSONL transcript. If an old/stale live worker has no
                // socket metadata, keep the single-writer guard and stay in the
                // view.
                let home = crate::run::lingxi_home_dir();
                if let Some(target) = find_live_attach_target(&home, &session_id) {
                    if let Err(e) = attach_to_live_worker(&target) {
                        eprintln!("lingxi-cli agents: live attach failed: {e}");
                    }
                    watcher.observe();
                    state.reload(load_view_rows(cli));
                    continue;
                }
                if session_has_live_worker(&home, &session_id) {
                    eprintln!(
                        "lingxi-cli agents: session {session_id} is still running in the background, but its live attach socket is unavailable; it keeps running."
                    );
                    watcher.observe();
                    state.reload(load_view_rows(cli));
                    continue;
                }
                // Attach = run the resumed session in the foreground; when it
                // ends, fall through and remount the view with fresh rows.
                let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("lingxi-cli"));
                let status = std::process::Command::new(exe)
                    .args(attach_args(cli, &session_id))
                    .status();
                if let Err(e) = status {
                    eprintln!("lingxi-cli agents: attach failed: {e}");
                    return crate::exit_codes::RUNTIME_ERROR;
                }
                watcher.observe();
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

    fn roster_worker(
        session_id: &str,
        socket: &str,
        auth: &str,
    ) -> crate::daemon_roster::WorkerRecord {
        use crate::daemon_roster::{
            Dispatch, DispatchSource, Isolation, Launch, WorkerRecord, PROTO,
        };
        WorkerRecord {
            pid: 4321,
            proc_start: None,
            session_id: session_id.to_string(),
            rendezvous_sock: socket.to_string(),
            pty_sock: Some(socket.to_string()),
            messaging_sock: None,
            cli_version: Some("0.0.0".to_string()),
            started_at: 1_700_000_000_000,
            attempt: 0,
            cwd: "/work".to_string(),
            worktree_path: None,
            dispatch: Dispatch {
                proto: PROTO,
                short: "bead0001".to_string(),
                nonce: None,
                session_id: session_id.to_string(),
                created_at: 1_700_000_000_000,
                source: DispatchSource::Shell,
                cwd: "/work".to_string(),
                launch: Launch::Prompt {
                    args: vec!["hi".to_string()],
                },
                env: std::collections::BTreeMap::new(),
                reattach_env: None,
                worktree: None,
                isolation: Isolation::None,
                respawn_flags: Vec::new(),
                attach_stall_respawns: None,
                agent: None,
                routine: None,
                seed: None,
                cols: None,
                rows: None,
            },
            pending_respawn: None,
            dec_modes: None,
            rv_auth: Some(auth.to_string()),
            pty_auth: Some(auth.to_string()),
            extra: serde_json::Map::new(),
        }
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
    fn live_worker_guard_blocks_only_a_running_workers_session() {
        use crate::agents_registry::JobState;
        let jobs = vec![
            (
                "aaaa0001".to_string(),
                JobState {
                    session_id: Some("sid-live".to_string()),
                    worker_pid: Some(4321),
                    ..Default::default()
                },
            ),
            (
                "aaaa0002".to_string(),
                JobState {
                    session_id: Some("sid-dead".to_string()),
                    worker_pid: Some(9999),
                    ..Default::default()
                },
            ),
            (
                "aaaa0003".to_string(),
                JobState {
                    session_id: Some("sid-workerless".to_string()),
                    worker_pid: None,
                    ..Default::default()
                },
            ),
        ];
        // Only pid 4321 is alive.
        let alive = |pid: i32| pid == 4321;
        // Live worker → attach is blocked.
        assert!(job_has_live_worker(&jobs, "sid-live", &alive));
        // Recorded worker pid is dead (crashed/finished) → attach allowed.
        assert!(!job_has_live_worker(&jobs, "sid-dead", &alive));
        // Workerless "working"/terminal job → attach allowed (safe to --resume).
        assert!(!job_has_live_worker(&jobs, "sid-workerless", &alive));
        // Unknown session id → attach allowed.
        assert!(!job_has_live_worker(&jobs, "sid-unknown", &alive));
    }

    #[test]
    fn live_attach_target_uses_roster_socket_for_running_worker() {
        use crate::agents_registry::JobState;
        use crate::daemon_roster::empty_roster;

        let jobs = vec![(
            "bead0001".to_string(),
            JobState {
                session_id: Some("sid-live".to_string()),
                worker_pid: Some(4321),
                ..Default::default()
            },
        )];
        let mut roster = empty_roster(999);
        roster.workers.insert(
            "bead0001".to_string(),
            roster_worker("sid-live", "/tmp/live.sock", "token-1"),
        );

        let target =
            live_attach_target_from(&jobs, &roster, "sid-live", &|pid| pid == 4321).unwrap();
        assert_eq!(target.short, "bead0001");
        assert_eq!(target.session_id, "sid-live");
        assert_eq!(target.socket, PathBuf::from("/tmp/live.sock"));
        assert_eq!(target.auth, "token-1");

        assert!(
            live_attach_target_from(&jobs, &roster, "sid-live", &|_pid| false).is_none(),
            "dead worker pid must not produce a live attach target"
        );

        let mut roster_without_auth = empty_roster(999);
        let mut record = roster_worker("sid-live", "/tmp/live.sock", "token-1");
        record.pty_auth = None;
        record.rv_auth = None;
        roster_without_auth
            .workers
            .insert("bead0001".to_string(), record);
        assert!(
            live_attach_target_from(&jobs, &roster_without_auth, "sid-live", &|pid| pid == 4321)
                .is_none(),
            "auth is required before agents can attach to a live socket"
        );
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
