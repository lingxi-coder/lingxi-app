//! Three-way mode dispatch added in M6-01. Routes argv to one of:
//! - `Mode::Print(prompt)`: existing `run::run_oneshot` (v0.6.0, unchanged)
//! - `Mode::Tui`: new `tui::run_tui_session` (v0.7.0 default)
//! - `Mode::StdioRepl`: existing `repl::run_repl` (v0.6.0, kept as `--no-tui`
//!   and non-TTY fallback)
//!
//! TTY detection uses [`std::io::IsTerminal`] (stable since Rust 1.70). If
//! stdin OR stdout isn't a terminal, we fall back to `StdioRepl` (CI, pipes,
//! redirected I/O). The `--no-tui` flag forces `StdioRepl` regardless of
//! TTY state.

use crate::argv::Argv;
use std::io::IsTerminal;

/// The resolved execution mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// One-shot print mode: prompt is set, exit after first `end_turn`.
    /// Carries the prompt string so the caller doesn't re-clone argv.
    Print(String),
    /// Fullscreen iocraft TUI (default when no prompt + TTY + no `--no-tui`).
    Tui,
    /// Line-based stdio REPL from v0.6.0 (`--no-tui` or non-TTY).
    StdioRepl,
}

/// Pick a mode from argv + the current process's TTY state.
///
/// Decision tree:
/// 1. If `argv.prompt` has a non-empty value → `Print(prompt)`.
/// 2. Else if `argv.no_tui` is set OR stdin/stdout isn't a TTY → `StdioRepl`.
/// 3. Else → `Tui`.
#[must_use]
pub fn decide_mode(argv: &Argv) -> Mode {
    decide_mode_with(argv, is_full_tty())
}

/// Test seam: `is_tty` argument decouples the decision from the real
/// process TTY state.
#[must_use]
pub fn decide_mode_with(argv: &Argv, is_tty: bool) -> Mode {
    if let Some(p) = argv.prompt.as_deref() {
        if !p.trim().is_empty() {
            return Mode::Print(p.to_string());
        }
    }
    if argv.no_tui || !is_tty {
        return Mode::StdioRepl;
    }
    Mode::Tui
}

/// True iff BOTH stdin and stdout are terminals. (M7-12) Exposed
/// `pub(crate)` so `run::run_resume` can make the same TTY decision the mode
/// dispatcher uses — one source of truth for "are we interactive".
pub(crate) fn is_full_tty() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

use crate::exit_codes;
use crate::init::Runtime;
use crate::output::OutputSink;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use traits::OrchestratorHandle;

/// Execute the chosen mode. Returns the process exit code.
///
/// `Mode::Print` and `Mode::StdioRepl` call into the existing v0.6.0
/// code paths unchanged. `Mode::Tui` calls into the new `lingxi-tui`
/// entry point.
pub async fn dispatch(
    mode: Mode,
    argv: &Argv,
    runtime: &Runtime,
    sink: Arc<dyn OutputSink>,
) -> i32 {
    match mode {
        Mode::Print(_) => {
            // run_oneshot reads the prompt directly from argv.prompt;
            // the captured-prompt copy in Mode::Print(prompt) exists for
            // test introspection only.
            crate::run::run_oneshot(argv, runtime, sink.as_ref()).await
        }
        Mode::StdioRepl => {
            // v0.6.0 REPL path. `repl::run_repl` rebuilds the runtime
            // internally (it owns its own sink/orchestrator construction
            // for the streaming-stdout case). Pass argv through and
            // ignore the `runtime` arg in this arm.
            let _ = runtime; // intentionally unused in this arm
            let _ = sink; // intentionally unused (repl mints its own)
            crate::repl::run_repl(argv).await
        }
        Mode::Tui => {
            // M6-03 path: rebuild the runtime with `BridgeOutputStream` as
            // the orchestrator's output, then pass the bridge_rx into
            // `run_tui_session` so streaming events route into AppState.
            //
            // The `runtime` arg here was built with the standard
            // sink-adapter output (for one-shot / NDJSON modes); we
            // discard it and construct a TUI-specific build. The original
            // sink is therefore unused in this arm.
            let _ = runtime; // intentionally unused — we mint a TUI build
            let _ = sink; // intentionally unused
            let tui_build = match crate::init::build_runtime_for_tui(argv).await {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("lingxi-cli: tui init failed: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
            };
            // (Task 3) Startup project-trust dialog (`TrustDialog`, shown by
            // `showSetupScreens` BEFORE the REPL/session and BEFORE any
            // tool/hook/plugin runs). TTY-only — this `Mode::Tui` arm is only
            // reached when stdin+stdout are terminals (`mode::decide_mode_with`
            // falls back to `StdioRepl` otherwise), so the raw-mode mount is
            // safe AND "non-interactive ⇒ no dialog" is satisfied for free.
            // Placed BEFORE the bypass gate: trust is the outermost "may I
            // touch this folder at all" gate. Already-accepted (incl.
            // parent-walk-trusted) ⇒ skip ⇒ byte-identical to today.
            match trust_gate().await {
                TrustGateOutcome::Proceed => {}
                TrustGateOutcome::Decline => {
                    // "No, exit" / Esc → exit 1 (`TrustDialog.tsx:158-160`
                    // `gracefulShutdownSync(1)`; matches the REPL gate).
                    return exit_codes::RUNTIME_ERROR;
                }
            }
            // (Task 8) Startup bypass-permissions confirmation dialog
            // (`BypassPermissionsModeDialog`, shown by `showSetupScreens` BEFORE
            // the REPL). TTY-only — this arm is only reached when stdin+stdout
            // are terminals (`mode::decide_mode`), so the raw-mode mount is safe
            // here. Gated on bypass-mode resolved AND no settings tier already
            // carrying `skipDangerousModePermissionPrompt`.
            let (bypass_mode, _) = crate::resolve_permission_mode(argv);
            let is_bypass = bypass_mode == permission::PermissionMode::BypassPermissions;
            let skip_set = read_skip_dangerous_prompt();
            if tui::startup_bypass::should_show_bypass_dialog(is_bypass, skip_set) {
                match tui::startup_bypass::mount_bypass_dialog().await {
                    Ok(tui::startup_bypass::BypassDialogOutcome::Accept) => {
                        // Persist so subsequent launches skip the prompt
                        // (`onConfirm` → `saveCurrentProjectConfig`). Best-effort.
                        persist_skip_dangerous_prompt();
                        // TELEMETRY (`tengu_bypass_permissions_mode_dialog_accept`,
                        // registered in Task 6): DEFERRED. There is no pre-session
                        // analytics sink wired at this seam (same situation as the
                        // startup migrations, which emit with `bus: None`). Emitting
                        // here would buffer onto a bus that is never flushed pre-REPL
                        // — an honest no-op is preferable to a call that looks wired
                        // but silently drops. The event name is registered and the
                        // emit lands once a pre-session sink exists.
                    }
                    Ok(tui::startup_bypass::BypassDialogOutcome::Decline) => {
                        // User declined (or pressed Esc): exit 1 (TS `process.exit(1)`).
                        return exit_codes::RUNTIME_ERROR;
                    }
                    Err(e) => {
                        eprintln!("lingxi-cli: bypass dialog failed: {e}");
                        return exit_codes::RUNTIME_ERROR;
                    }
                }
            }
            // FRESH launch: no replayed scrollback. `build_tui_runtime` with an
            // empty `resumed_messages` vec is byte-identical to the pre-refactor
            // inline assembly — `Runtime::with_resumed_messages([])` is a no-op
            // and `run_tui_session` skips the seed when the vec is empty.
            let tui_runtime = build_tui_runtime(tui_build, argv, Vec::new()).await;
            mount_tui_runtime(tui_runtime).await
        }
    }
}

/// Assemble the [`tui::session::Runtime`] from a [`crate::init::TuiBuild`].
///
/// (M5-13) Extracted from the `Mode::Tui` arm so the `--resume <uuid>` mount
/// (`run::run_resume_by_id`) reuses the EXACT same wiring — orchestrator handle,
/// bridge, status snapshot, multi-agent `PollerFeed`, turn-spawn sender, and the
/// command registry — instead of duplicating it. The ONLY resume-specific input
/// is `resumed_messages`: the prior conversation, already mapped to TUI
/// scrollback rows via `tui::replay::rebuild_from_jsonl`. A FRESH launch passes
/// an empty vec, so `with_resumed_messages([])` is a no-op and the fresh mount
/// stays byte-identical to the pre-extraction inline code.
pub(crate) async fn build_tui_runtime(
    tui_build: crate::init::TuiBuild,
    argv: &Argv,
    resumed_messages: Vec<tui::state::RenderedMessage>,
) -> tui::session::Runtime {
    // (M7-13 review) Coerce the concrete orchestrator to the
    // `OrchestratorHandle` trait object so it can drive both the session
    // id read and the Settings open pump inside the TUI mount.
    let orchestrator: Arc<dyn OrchestratorHandle> = tui_build.runtime.orchestrator.clone();
    let session_id = orchestrator.current_session_id().await;
    let bridge = tui::session::TuiBridge {
        rx: tui_build.bridge_rx,
    };
    // Status snapshot — model from argv (if set), cwd from current
    // dir, cost placeholder. Full status wiring lands in M6-06.
    let mut status = tui::state::StatusSnapshot::default();
    if let Some(m) = &argv.model {
        status.model.clone_from(m);
    }
    if let Ok(cwd) = std::env::current_dir() {
        status.cwd = cwd;
    }
    // (M9-05) Wrap the desktop TaskRegistry in a `PollerFeed` so the TUI
    // background-task footer + dialog read live state. The registry is
    // the SAME one wired into the tool context (tools that spawn tasks
    // update it; the feed polls it). Coerced to the narrow `traits`
    // handle at the seam.
    let task_feed: Arc<dyn tui::multiagent::MultiAgentFeed> = Arc::new(
        tui::multiagent::PollerFeed::new(tui_build.runtime.task_registry.clone()
            as Arc<dyn traits::task_registry::TaskRegistryHandle>),
    );
    // (MULTIMODAL.1) Thread the bridge sender clone so the TUI live-key
    // loop can spawn streaming turns (`tui::root::pump_turn`). Moved out
    // of `tui_build` (a disjoint field from the already-taken `bridge_rx`).
    // (ARGS.3) Thread the dispatcher's shared command registry so the TUI
    // populates its progressive argument-hint map at init. `.registry()`
    // only clones the inner `Arc<RwLock<CommandRegistry>>`, leaving the
    // dispatcher in place on `tui_build.runtime` (read before `turn_tx`,
    // a disjoint field, is moved out below — no borrow/move conflict).
    let command_registry = tui_build.runtime.dispatcher.registry();
    // Thread the SAME fully-wired slash dispatcher (incl. its UserPromptExpansion
    // hooks) into the TUI so the live submit path can expand a typed `/loop` (and
    // Markdown/Plugin prompt commands) and run it as a turn — matching the CLI
    // repl. `.registry()` above already cloned the shared registry Arc; the
    // dispatcher field is otherwise unused in TUI mode, so move it out here.
    let slash_dispatcher: Arc<dyn traits::SlashCommandDispatcher> =
        Arc::new(tui_build.runtime.dispatcher);
    // (Plan 3c §8 / I1/I2) Project the engine-computed provider maps off the
    // runtime (disjoint fields, read before `turn_tx` is moved out below) so the
    // `/model` picker can badge unconfigured providers + resolve a bare
    // USER-provider model id to its own group + availability gate. Empty (the
    // default) keeps every row available — byte-identical to the historical path.
    let provider_availability = tui_build.runtime.provider_availability.clone();
    let model_providers = tui_build.runtime.model_providers.clone();
    // (Plan 3c C1) Project the shared engine credential store off the runtime so
    // the `/connect` screen's `pump_store_provider_key` persists a collected key
    // via `CredentialManager::set_provider_key`.
    let provider_key_store = tui_build.runtime.provider_key_store.clone();
    // (M5-13) Seed the prior conversation last so a resumed session paints its
    // existing history on the first frame. For a fresh launch this is `[]`.
    tui::session::Runtime::with_bridge(session_id, bridge, status)
        .with_orchestrator(orchestrator)
        .with_multiagent_feed(task_feed)
        .with_turn_tx(tui_build.turn_tx)
        .with_permission_rx(tui_build.permission_rx)
        .with_command_registry(command_registry)
        .with_dispatcher(slash_dispatcher)
        .with_provider_availability(provider_availability)
        .with_model_providers(model_providers)
        .with_provider_key_store(provider_key_store)
        // (B4 Task 5) Thread the composition root's shared subscription slot so
        // the TUI rate-limit composer reads the live snapshot at compose time.
        .with_subscription(tui_build.runtime.subscription.clone())
        // (A6 batch-6 Task 2) Read + merge the `statusLine` setting (User+Local,
        // Local-over-User) and thread it through so the TUI's debounced
        // statusline pump runs the configured command. `None` (no setting)
        // leaves the built-in status row in place.
        .with_status_line_config(read_status_line_config())
        .with_resumed_messages(resumed_messages)
}

/// Drive the TUI to a clean shutdown and map its result to a process exit code.
/// Shared by the fresh `Mode::Tui` arm and the `--resume <uuid>` mount.
pub(crate) async fn mount_tui_runtime(tui_runtime: tui::session::Runtime) -> i32 {
    let cancel = CancellationToken::new();
    match tui::run_tui_session(tui_runtime, cancel).await {
        Ok(()) => exit_codes::SUCCESS,
        Err(e) => {
            eprintln!("lingxi-cli: tui session failed: {e}");
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// Resolve `(claude_home, project_dir)` the settings reader/writer address.
///
/// `claude_home = ~/.claude` (the user settings root; `/dev/null` when no home
/// dir, matching `init::resolve_desktop_config`'s degrade); `project_dir =
/// std::env::current_dir()`, read AFTER `cwd::apply_cwd` so it reflects any
/// `--cwd`. These feed `migrations::settings_update::settings_path`.
fn settings_dirs() -> (std::path::PathBuf, std::path::PathBuf) {
    let claude_home = crate::run::claude_home_dir();
    let project_dir =
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    (claude_home, project_dir)
}

/// (A6 batch-6 Task 2) Read + merge the `statusLine` setting from the USER
/// (`~/.lingxi/settings.json`) and LOCAL (`<proj>/.lingxi/settings.local.json`)
/// tiers, Local-over-User, and parse it into a [`StatusLineConfig`]. `None` when
/// neither tier carries a `command`-shaped `statusLine` (then the built-in row
/// renders). Pure over the two settings roots so it is unit-testable; the live
/// caller [`read_status_line_config`] resolves them via [`settings_dirs`].
///
/// `statusLine` is NOT a typed `SettingsJson` field (`Settings::load` cannot
/// carry it), so this reads the raw per-tier maps directly via
/// `read_settings_map`. A broken/unreadable tier degrades to "no value" for that
/// tier (read error → treated as absent), matching the TS warn-and-continue
/// settings stance.
///
/// DIVERGENCES from claude-code (documented, intentional): (1) TS resolves
/// `statusLine` from the fully-merged settings across User → Project
/// (`.lingxi/settings.json`) → Local → flag → policy (`constants.ts`
/// `SETTING_SOURCES`); the Rust `migrations::settings_update::SettingsSource`
/// has only `User`/`Local` substrate (same limit as [`read_skip_dangerous_prompt`]),
/// so a `statusLine` committed in project `.lingxi/settings.json` is silently
/// dropped — recorded in spec rev2.11's remaining list. (2) TS deep-merges the
/// `statusLine` OBJECT across tiers (lodash default merge); this does a whole-
/// object replace (Local's `statusLine` wholly replaces User's), so a config
/// SPLIT across tiers (e.g. `{type,command}` in User + `{padding}` in Local)
/// diverges — real configs carry the whole object in one tier, so this is
/// acceptable.
fn read_status_line_config_from(
    claude_home: &std::path::Path,
    project_dir: &std::path::Path,
) -> Option<tui::components::status_line_command::StatusLineConfig> {
    use migrations::settings_update::{read_settings_map, settings_path, SettingsSource};
    // Local-over-User: read User first, then let Local's `statusLine` override.
    let mut status_line: Option<serde_json::Value> = None;
    for source in [SettingsSource::User, SettingsSource::Local] {
        let p = settings_path(source, claude_home, project_dir);
        if let Ok(map) = read_settings_map(&p) {
            if let Some(v) = map.get("statusLine") {
                status_line = Some(v.clone());
            }
        }
    }
    status_line
        .as_ref()
        .and_then(tui::components::status_line_command::StatusLineConfig::from_settings_value)
}

/// (A6 batch-6 Task 2) Live wrapper over [`read_status_line_config_from`],
/// resolving the User+Local settings roots via [`settings_dirs`].
fn read_status_line_config() -> Option<tui::components::status_line_command::StatusLineConfig> {
    let (claude_home, project_dir) = settings_dirs();
    read_status_line_config_from(&claude_home, &project_dir)
}

/// True iff `skipDangerousModePermissionPrompt` is truthy in EITHER the user
/// (`~/.lingxi/settings.json`) OR local (`<cwd>/.lingxi/settings.local.json`)
/// settings — the `hasSkipDangerousModePermissionPrompt` user+local check
/// (claude-code `settings.ts:882-889`; the flag/policy tiers have no Rust
/// substrate). On any read failure the tier degrades to `false`.
fn read_skip_dangerous_prompt() -> bool {
    use migrations::settings_update::{read_settings_map, settings_path, SettingsSource};
    let (claude_home, project_dir) = settings_dirs();
    [SettingsSource::User, SettingsSource::Local].iter().any(|s| {
        let p = settings_path(*s, &claude_home, &project_dir);
        read_settings_map(&p)
            .ok()
            .and_then(|m| {
                m.get("skipDangerousModePermissionPrompt")
                    .map(migrations::context::js_truthy)
            })
            .unwrap_or(false)
    })
}

/// Persist `skipDangerousModePermissionPrompt = true` to the USER
/// `settings.json` (`onConfirm` → save in claude-code). Best-effort: a write
/// failure warns and is otherwise ignored (TS `updateSettingsForSource` never
/// throws; the migration port follows the same warn-and-continue contract).
fn persist_skip_dangerous_prompt() {
    use migrations::settings_update::{settings_path, update_settings, SettingsSource};
    let (claude_home, project_dir) = settings_dirs();
    let path = settings_path(SettingsSource::User, &claude_home, &project_dir);
    if let Err(e) = update_settings(
        &path,
        vec![(
            "skipDangerousModePermissionPrompt".into(),
            Some(serde_json::json!(true)),
        )],
    ) {
        tracing::warn!(error = %e, "persist skipDangerousModePermissionPrompt failed (ignored)");
    }
}

/// Outcome of the startup trust gate ([`trust_gate`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustGateOutcome {
    /// Already trusted (incl. parent-walk), no config path, or the user
    /// accepted the dialog ⇒ continue into the session (trusted).
    Proceed,
    /// The user declined (or pressed Esc) ⇒ exit 1.
    Decline,
}

/// Whether the un-trusted cwd should mount the trust dialog, and the resolved
/// `(cwd, config_path)` to mount + persist against. `None` ⇒ proceed with NO
/// dialog (already trusted via parent-walk, OR no global config path to
/// consult/persist — degrade exactly as today's hardcoded `Trusted`).
///
/// Pure decision (no terminal I/O): factored out of [`trust_gate`] so the
/// gate's branch logic is unit-testable without a TTY (the mount itself, like
/// `run_tui_session`, can't be driven headless). Mirrors
/// `TrustDialog.tsx:199-202` — `if (hasTrustDialogAccepted) { onDone() }` skips
/// the dialog.
fn trust_gate_should_prompt(
    cwd: &std::path::Path,
    config_path: Option<&std::path::Path>,
) -> bool {
    match config_path {
        // No config path to consult/persist ⇒ proceed without prompting
        // (no-home degrade, same as `init::resolve_desktop_config`).
        None => false,
        // Already trusted (cwd or any ancestor) ⇒ proceed without prompting.
        Some(p) => !migrations::global_config::check_has_trust_dialog_accepted(p, cwd),
    }
}

/// Startup project-trust GATE (design doc §3; parity
/// `components/TrustDialog/TrustDialog.tsx` + `showSetupScreens`).
///
/// Resolves the cwd + the global config path, and:
/// - already-trusted (parent-walk) / no config path ⇒ [`TrustGateOutcome::Proceed`]
///   with NO dialog (byte-identical to today's hardcoded `Trusted`);
/// - otherwise mounts the TTY trust dialog; Accept ⇒ persist
///   `hasTrustDialogAccepted` (best-effort) + `Proceed`; Decline/Esc ⇒
///   [`TrustGateOutcome::Decline`] (exit 1); a mount I/O error ⇒ `Decline`
///   (fail-closed — never silently proceed on a broken prompt).
async fn trust_gate() -> TrustGateOutcome {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let config_path = migrations::global_config::global_config_path();
    if !trust_gate_should_prompt(&cwd, config_path.as_deref()) {
        return TrustGateOutcome::Proceed;
    }
    // `trust_gate_should_prompt` only returns true with `Some(config_path)`.
    let config_path = config_path.expect("prompt implies a config path");
    match tui::startup_trust::mount_trust_dialog(&cwd).await {
        Ok(tui::startup_trust::TrustDialogOutcome::Accept) => {
            // "Yes, I trust this folder" → record acceptance via the shared
            // accept branch (`TrustDialog.tsx:162,174-177`): SESSION-ONLY
            // in-memory when `cwd == $HOME`, else persisted to disk best-effort.
            migrations::global_config::record_trust_accept(&config_path, &cwd);
            TrustGateOutcome::Proceed
        }
        Ok(tui::startup_trust::TrustDialogOutcome::Decline) => TrustGateOutcome::Decline,
        Err(e) => {
            eprintln!("lingxi-cli: trust dialog failed: {e}");
            TrustGateOutcome::Decline
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(prompt: Option<&str>, no_tui: bool) -> Argv {
        Argv {
            prompt: prompt.map(String::from),
            no_tui,
            ..Argv::default()
        }
    }

    #[test]
    fn prompt_routes_to_print() {
        let a = argv(Some("fix it"), false);
        assert_eq!(decide_mode_with(&a, true), Mode::Print("fix it".into()));
    }

    #[test]
    fn empty_prompt_does_not_route_to_print() {
        let a = argv(Some("   "), false);
        assert_eq!(decide_mode_with(&a, true), Mode::Tui);
    }

    #[test]
    fn no_prompt_tty_routes_to_tui() {
        let a = argv(None, false);
        assert_eq!(decide_mode_with(&a, true), Mode::Tui);
    }

    #[test]
    fn no_tui_flag_forces_stdio_repl() {
        let a = argv(None, true);
        assert_eq!(decide_mode_with(&a, true), Mode::StdioRepl);
    }

    #[test]
    fn non_tty_routes_to_stdio_repl() {
        let a = argv(None, false);
        assert_eq!(decide_mode_with(&a, false), Mode::StdioRepl);
    }

    #[test]
    fn non_tty_with_prompt_still_prints() {
        // -p with piped input should still print one-shot (matches v0.6.0).
        let a = argv(Some("hi"), false);
        assert_eq!(decide_mode_with(&a, false), Mode::Print("hi".into()));
    }

    // ── (A6 batch-6 Task 2) statusLine settings merge ─────────────────────

    fn write_settings(path: &std::path::Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    /// A real `{"statusLine":{"type":"command","command":"echo hi"}}` in USER
    /// settings reaches a `Some(StatusLineConfig)` via the merge helper. This is
    /// the B2 RED — nothing parsed `statusLine` into the runtime before.
    #[test]
    fn status_line_config_read_from_user_settings() {
        let tmp = std::env::temp_dir().join(format!("slc-user-{}", std::process::id()));
        let claude_home = tmp.join("home");
        let project_dir = tmp.join("proj");
        write_settings(
            &claude_home.join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"echo hi"}}"#,
        );
        let cfg = read_status_line_config_from(&claude_home, &project_dir);
        let cfg = cfg.expect("user statusLine parses");
        assert_eq!(cfg.command, "echo hi");
        assert_eq!(cfg.kind, "command");
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Local settings (`<proj>/.lingxi/settings.local.json`) WIN over User for
    /// the `statusLine` key (Local-over-User precedence).
    #[test]
    fn status_line_config_local_overrides_user() {
        let tmp = std::env::temp_dir().join(format!("slc-prec-{}", std::process::id()));
        let claude_home = tmp.join("home");
        let project_dir = tmp.join("proj");
        write_settings(
            &claude_home.join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"user-cmd"}}"#,
        );
        write_settings(
            &project_dir.join(".lingxi").join("settings.local.json"),
            r#"{"statusLine":{"type":"command","command":"local-cmd"}}"#,
        );
        let cfg = read_status_line_config_from(&claude_home, &project_dir)
            .expect("merged statusLine parses");
        assert_eq!(cfg.command, "local-cmd", "Local wins over User");
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// No `statusLine` key anywhere → `None` (built-in row renders).
    #[test]
    fn status_line_config_absent_is_none() {
        let tmp = std::env::temp_dir().join(format!("slc-none-{}", std::process::id()));
        let claude_home = tmp.join("home");
        let project_dir = tmp.join("proj");
        write_settings(&claude_home.join("settings.json"), r#"{"theme":"dark"}"#);
        assert!(read_status_line_config_from(&claude_home, &project_dir).is_none());
        std::fs::remove_dir_all(&tmp).ok();
    }

    // ── (Task 3) startup trust gate ───────────────────────────────────────
    //
    // The mount itself (`mount_trust_dialog`) can't be driven headless (no TTY
    // — same caveat documented on `run_tui_session` / `mount_bypass_dialog`),
    // so the gate DECISION is tested here via the store seam
    // (`trust_gate_should_prompt`, against a real `migrations::global_config`
    // temp config). The dialog's pure key→outcome map (Accept/Decline/Esc) is
    // covered by `tui::startup_trust`'s own unit tests; `apps/cli` does not
    // depend on `crossterm`, so the gate's branch logic is exercised here at
    // the predicate seam — mirroring how the bypass gate is covered (pure
    // predicate, thin I/O wrapper excluded).

    use std::path::PathBuf;

    /// An un-trusted cwd ⇒ the gate WOULD mount the dialog
    /// (`trust_gate_should_prompt == true`); persisting (the Accept path)
    /// makes a subsequent `check_` true and the gate no longer re-prompts
    /// (continue path / store marked).
    #[test]
    fn untrusted_cwd_prompts_then_accept_marks_and_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(".lingxi.json");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        // Un-trusted ⇒ dialog shown.
        assert!(
            trust_gate_should_prompt(&cwd, Some(config_path.as_path())),
            "un-trusted cwd must mount the trust dialog"
        );

        // Accept persists ⇒ subsequent check is true (and the gate would NOT
        // re-prompt) — the "continue, store marked" path.
        migrations::global_config::mark_trust_dialog_accepted(&config_path, &cwd).unwrap();
        assert!(
            migrations::global_config::check_has_trust_dialog_accepted(&config_path, &cwd),
            "accept must persist trust"
        );
        assert!(
            !trust_gate_should_prompt(&cwd, Some(config_path.as_path())),
            "after accept the gate must not re-prompt"
        );
    }

    /// A trusted cwd ⇒ NO dialog (straight to the main screen / session).
    #[test]
    fn trusted_cwd_shows_no_dialog() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(".lingxi.json");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        // Pre-trust the cwd.
        migrations::global_config::mark_trust_dialog_accepted(&config_path, &cwd).unwrap();
        assert!(
            !trust_gate_should_prompt(&cwd, Some(config_path.as_path())),
            "trusted cwd must NOT mount the trust dialog"
        );
    }

    /// An ancestor-trusted cwd ⇒ NO dialog (parent-walk; trusting a parent
    /// trusts children — `checkHasTrustDialogAccepted`).
    #[test]
    fn ancestor_trusted_cwd_shows_no_dialog() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(".lingxi.json");
        let parent = tmp.path().join("workspace");
        let child = parent.join("sub").join("project");
        std::fs::create_dir_all(&child).unwrap();
        // Trust the PARENT only.
        migrations::global_config::mark_trust_dialog_accepted(&config_path, &parent).unwrap();
        assert!(
            !trust_gate_should_prompt(&child, Some(config_path.as_path())),
            "ancestor-trusted cwd must NOT mount the trust dialog (parent-walk)"
        );
    }

    /// No global config path (no-home degrade) ⇒ NO dialog, proceed as today.
    #[test]
    fn no_config_path_shows_no_dialog() {
        let cwd = PathBuf::from("/some/untracked/dir");
        assert!(
            !trust_gate_should_prompt(&cwd, None),
            "no config path must proceed without a dialog (degrade)"
        );
    }
}
