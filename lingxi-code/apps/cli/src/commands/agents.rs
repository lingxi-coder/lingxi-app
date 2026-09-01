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
//!   ratatui list grouped into the fleet-view bands; `Enter` connects directly
//!   to a live worker's protocol-v2 PTY, or resumes a terminal historical job
//!   with the dispatch flags, and RETURNS TO THE VIEW when that session ends
//!   (2.1.198: leaving an attached session opens the agent view instead of
//!   exiting to shell);
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
                    Arc::new(platform_posix::PosixHttp::new()) as Arc<dyn platform_api::HttpTransport>,
                    Arc::new(platform_posix::PosixRuntime::new())
                        as Arc<dyn platform_api::RuntimeSpawner>,
                )
                .with_process_runner(
                    Arc::new(platform_posix::PosixProcess::new()) as Arc<dyn platform_api::ProcessRunner>,
                    Arc::new(platform_posix::PosixSandbox::new()) as Arc<dyn platform_api::Sandbox>,
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

/// Resolve the working directory a session was created in, so an interactive
/// attach/resume relaunch runs *in that directory*.
///
/// The resume loader derives the transcript path from the process cwd (Claude
/// encodes cwd into the `projects/<encoded-cwd>/<sessionId>.jsonl` path). The
/// agent view lists sessions from sibling git worktrees, each carrying its own
/// cwd — so relaunching a foreign-worktree session from the view's own cwd
/// would look up an empty/wrong transcript. Returning the recorded cwd lets the
/// caller chdir the respawned child to match (mirrors the binary respawning in
/// the session's cwd). Live sessions use `LiveSessionRecord.cwd`; background
/// jobs use the worker `cwd` (where the transcript was written), falling back to
/// `originCwd`.
fn session_origin_cwd(home: &Path, session_id: &str) -> Option<PathBuf> {
    use crate::agents_registry as reg;
    let live = reg::read_live_sessions(&reg::sessions_dir(home));
    let jobs = reg::read_jobs(&reg::jobs_dir(home));
    session_origin_cwd_from(&live, &jobs, session_id)
}

/// Pure resolver for [`session_origin_cwd`] (filesystem read split out so the
/// selection logic is unit-testable): a live record's cwd wins, else the
/// matching job's worker `cwd`, else its `originCwd`. Empty strings are ignored.
fn session_origin_cwd_from(
    live: &[crate::agents_registry::LiveSessionRecord],
    jobs: &[(String, crate::agents_registry::JobState)],
    session_id: &str,
) -> Option<PathBuf> {
    if let Some(rec) = live
        .iter()
        .find(|r| r.session_id.as_deref() == Some(session_id))
    {
        if !rec.cwd.is_empty() {
            return Some(PathBuf::from(&rec.cwd));
        }
    }
    jobs.iter()
        .find(|(_, job)| job.session_id.as_deref() == Some(session_id))
        .and_then(|(_, job)| job.cwd.as_deref().or(job.origin_cwd.as_deref()))
        .filter(|c| !c.is_empty())
        .map(PathBuf::from)
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

/// Delete the background session driving `session_id` (the FleetView Ctrl-X
/// two-press confirm): resolve its `jobs/<short>`, stop a live worker, and
/// remove the job state (+ a managed worktree, kept on failure). Emits
/// `tengu_bg_agent_action{action:"delete",source:"fleet"}` via
/// [`crate::commands::rm::perform_delete`]. Best-effort — a session with no
/// job row (e.g. a live-only interactive session) is left untouched.
fn delete_agent(home: &Path, session_id: &str) {
    use crate::agents_registry as reg;
    let jobs = reg::read_jobs(&reg::jobs_dir(home));
    if let Some((short, job)) = jobs
        .iter()
        .find(|(_, j)| j.session_id.as_deref() == Some(session_id))
    {
        if let Err(reason) = crate::commands::rm::perform_delete(home, short, job, "fleet") {
            eprintln!("{reason}");
        }
    }
}

/// Stop every running background worker (the FleetView `Ctrl+X Ctrl+K` chord):
/// `SIGTERM` each job whose recorded `workerPid` is still alive. Emits
/// `tengu_bg_agent_action{action:"stop",source:"fleet"}` per stopped worker.
/// Job state is NOT removed — stop-all only halts the live work.
fn stop_all_agents(home: &Path) {
    use crate::agents_registry as reg;
    let jobs = reg::read_jobs(&reg::jobs_dir(home));
    for (short, job) in &jobs {
        let probe = crate::daemon_roster::SystemProbe;
        let alive = job
            .worker_pid
            .is_some_and(|pid| crate::daemon_roster::ProcProbe::is_alive(&probe, pid));
        if !alive {
            continue;
        }
        let _ = crate::commands::rm::stop_worker(job);
        tracing::info!(
            event = "tengu_bg_agent_action",
            action = "stop",
            source = "fleet",
            short = short.as_str()
        );
    }
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
                // Live background attach: use the same job/session resolver as
                // the public `lingxi-cli attach` command instead of spawning a
                // second `--resume` writer against the same JSONL transcript.
                // If an old/stale worker has no endpoint metadata, keep the
                // single-writer guard and stay in the view.
                let home = crate::run::lingxi_home_dir();
                match crate::commands::attach::attach_target(&home, &session_id) {
                    Ok(crate::commands::attach::AttachDisposition::Attached) => {
                        watcher.observe();
                        state.reload(load_view_rows(cli));
                        continue;
                    }
                    Ok(crate::commands::attach::AttachDisposition::LiveEndpointUnavailable {
                        ..
                    }) => {
                        eprintln!(
                            "lingxi-cli agents: session {session_id} is still running in the background, but its live attach endpoint is unavailable; it keeps running."
                        );
                        watcher.observe();
                        state.reload(load_view_rows(cli));
                        continue;
                    }
                    Ok(
                        crate::commands::attach::AttachDisposition::NotFound
                        | crate::commands::attach::AttachDisposition::NotRunning { .. },
                    ) => {
                        // No live worker owns the transcript, so the legacy
                        // foreground resume path below is safe.
                    }
                    Err(e) => {
                        eprintln!("lingxi-cli agents: live attach failed: {e}");
                        watcher.observe();
                        state.reload(load_view_rows(cli));
                        continue;
                    }
                }
                // Attach = run the resumed session in the foreground; when it
                // ends, fall through and remount the view with fresh rows.
                // Route the self-respawn through the process wrapper so an
                // enterprise `CLAUDE_CODE_PROCESS_WRAPPER` launcher isn't
                // bypassed on interactive attach (mirrors the daemon/
                // background self-spawns).
                let exe = std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "lingxi-cli".to_string());
                let mut argv = vec![exe];
                argv.extend(attach_args(cli, &session_id));
                let argv = crate::process_wrapper::wrap_argv(argv);
                let Some((program, args)) = argv.split_first() else {
                    eprintln!("lingxi-cli agents: attach failed: empty attach argv");
                    return crate::exit_codes::RUNTIME_ERROR;
                };
                let mut command = std::process::Command::new(program);
                command.args(args);
                // Respawn in the session's own cwd so the resume loader finds the
                // right transcript when the session lives in a sibling worktree.
                if let Some(cwd) = session_origin_cwd(&home, &session_id) {
                    command.current_dir(cwd);
                }
                let status = command.status();
                if let Err(e) = status {
                    eprintln!("lingxi-cli agents: attach failed: {e}");
                    return crate::exit_codes::RUNTIME_ERROR;
                }
                watcher.observe();
                state.reload(load_view_rows(cli));
            }
            Ok(AgentsOutcome::Delete(session_id)) => {
                // Two-press Ctrl-X: kill the live worker (if any) and remove
                // the job state, then remount with fresh rows.
                delete_agent(&crate::run::lingxi_home_dir(), &session_id);
                watcher.observe();
                state.reload(load_view_rows(cli));
            }
            Ok(AgentsOutcome::StopAll) => {
                // Ctrl+X Ctrl+K: stop all running agents + background work.
                stop_all_agents(&crate::run::lingxi_home_dir());
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
    fn session_origin_cwd_resolves_from_live_then_job() {
        use crate::agents_registry::{JobState, LiveSessionRecord};
        use serde_json::json;

        let live: Vec<LiveSessionRecord> = serde_json::from_value(json!([
            {"pid": 1, "sessionId": "sid-live", "cwd": "/work/wt-a", "startedAt": 0, "kind": "interactive"}
        ]))
        .unwrap();
        let jobs = vec![(
            "bead0001".to_string(),
            JobState {
                session_id: Some("sid-job".to_string()),
                cwd: Some("/work/wt-b".to_string()),
                origin_cwd: Some("/work/main".to_string()),
                ..Default::default()
            },
        )];

        // Live record wins and its cwd is returned.
        assert_eq!(
            session_origin_cwd_from(&live, &jobs, "sid-live"),
            Some(PathBuf::from("/work/wt-a"))
        );
        // Job: the worker cwd (where the transcript was written) is preferred
        // over originCwd.
        assert_eq!(
            session_origin_cwd_from(&live, &jobs, "sid-job"),
            Some(PathBuf::from("/work/wt-b"))
        );
        // Unknown session → no cwd (relaunch keeps the caller's cwd).
        assert_eq!(session_origin_cwd_from(&live, &jobs, "sid-unknown"), None);

        // Job without a worker cwd falls back to originCwd.
        let jobs_origin_only = vec![(
            "bead0002".to_string(),
            JobState {
                session_id: Some("sid-origin".to_string()),
                cwd: None,
                origin_cwd: Some("/work/main".to_string()),
                ..Default::default()
            },
        )];
        assert_eq!(
            session_origin_cwd_from(&[], &jobs_origin_only, "sid-origin"),
            Some(PathBuf::from("/work/main"))
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
