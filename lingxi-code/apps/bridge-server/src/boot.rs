//! S2 — env/argv → [`DesktopConfig`] resolution and connection assembly for the
//! production bridge-server binary.
//!
//! This module is the env/argv-reading half of the binary (mirroring
//! `apps/cli/src/init.rs`'s `resolve_desktop_config`), kept separate from
//! `main.rs` so the resolution + a clear non-secret startup error are unit
//! testable without spawning a process or a runtime.
//!
//! The assembly mirrors the F2-06 e2e harness (`tests/e2e_permission_test.rs`)
//! and the desktop composition root:
//!
//! 1. [`BridgeConnection::new`] is created and its connection-scoped
//!    `event_sink()` + `permission_sink()` are pulled.
//! 2. `engine_desktop::build` assembles a real [`DesktopRuntime`] from the
//!    resolved [`DesktopConfig`], wiring the orchestrator's
//!    [`client_adapter::AdapterOutputStream`] to that event sink and binding the
//!    connection-scoped `AdapterPermissionGate` to that permission sink
//!    (`use_noop_permission_gate: false`).
//! 3. The runtime's handles back a production [`OrchestratorTurnDriver`] +
//!    [`EngineCommandRouter`], which are bound onto the connection.
//!
//! ## Single-client model
//!
//! Per the [`crate::server`] docs, the walking skeleton models ONE client per
//! server (the design's single Electron child): the per-connection state lives
//! directly on one [`BridgeConnection`], shared across the endpoint's
//! `Arc<dyn FramePump>`. The runtime is built once at boot and that one
//! connection is handed to the endpoint.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use bridge::lockfile::{IdeLockfile, LockfileGuard};
use bridge::McpEndpoint;
use engine_desktop::{build, DesktopConfig, DesktopRuntime};
use traits::{OrchestratorHandle, OutputStream, SlashCommandDispatcher};

use crate::driver::OrchestratorTurnDriver;
use crate::router::EngineCommandRouter;
use crate::server::BridgeConnection;

/// The env var the LLM API key is read from at runtime (NEVER hardcoded /
/// printed / committed — spec hard rule).
pub const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// Env override for the API base URL (mirrors `apps/cli`'s `resolve_api_base`).
pub const API_BASE_ENV: &str = "LINGXI_API_BASE_URL";
/// The canonical Anthropic endpoint used when [`API_BASE_ENV`] is unset.
pub const DEFAULT_API_BASE: &str = "https://api.anthropic.com";

/// Parsed command-line flags / env the binary accepts (a tiny hand-rolled
/// parser — `apps/bridge-server` deliberately avoids a clap dependency for the
/// two flags it needs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BridgeArgs {
    /// `--cwd <dir>`: working directory to root the engine at. The process
    /// `chdir`s into this before resolving the rest of the config so the
    /// settings / hook / `.mcp.json` loaders read the same dir.
    pub cwd: Option<PathBuf>,
    /// `--model <id>`: the default model id (overrides the desktop default).
    pub model: Option<String>,
    /// `--help` / `-h`: print usage and exit.
    pub help: bool,
}

/// One-line outcome of [`BridgeArgs::parse`]: either the parsed args, or a
/// non-secret usage error string.
pub type ParseResult = Result<BridgeArgs, String>;

impl BridgeArgs {
    /// Parse from an argument iterator (excluding `argv[0]`). Accepts
    /// `--cwd <dir>`, `--model <id>`, `--help`/`-h`. Unknown flags are an error
    /// (returned as a non-secret string so the binary can print usage).
    ///
    /// # Errors
    /// Returns a human-readable usage error for an unknown flag or a flag that
    /// is missing its required value.
    pub fn parse<I, S>(args: I) -> ParseResult
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out = BridgeArgs::default();
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            match arg.as_ref() {
                "--help" | "-h" => out.help = true,
                "--cwd" => {
                    let v = it
                        .next()
                        .ok_or_else(|| "--cwd requires a directory argument".to_string())?;
                    out.cwd = Some(PathBuf::from(v.as_ref()));
                }
                "--model" => {
                    let v = it
                        .next()
                        .ok_or_else(|| "--model requires a model id argument".to_string())?;
                    out.model = Some(v.as_ref().to_string());
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(out)
    }
}

/// The usage text printed by `--help` (and on a parse error).
#[must_use]
pub fn usage() -> String {
    format!(
        "lingxi-bridge-server {}\n\
         \n\
         Serves one local conversation over a loopback WebSocket for the Electron/iOS shell.\n\
         The LLM API key is read from the {API_KEY_ENV} environment variable at runtime.\n\
         \n\
         USAGE:\n    \
             bridge-server [OPTIONS]\n\
         \n\
         OPTIONS:\n    \
             --cwd <DIR>      Working directory to root the engine at (default: current dir)\n    \
             --model <ID>     Default model id (default: the desktop build default)\n    \
             -h, --help       Print this help\n\
         \n\
         ENVIRONMENT:\n    \
             {API_KEY_ENV}   LLM API key (required for live turns; the server still boots without it)\n    \
             {API_BASE_ENV}  Override the API base URL (default: {DEFAULT_API_BASE})\n",
        env!("CARGO_PKG_VERSION")
    )
}

/// Resolve the API base URL: honours [`API_BASE_ENV`], else [`DEFAULT_API_BASE`].
#[must_use]
pub fn resolve_api_base() -> String {
    std::env::var(API_BASE_ENV).unwrap_or_else(|_| DEFAULT_API_BASE.to_string())
}

/// Load the merged settings `providers` / `routing` blocks from the layered
/// settings (project + user + env), rooted at the *current* working directory
/// (the process has already `chdir`'d into any `--cwd`). Returns `(None, None)`
/// on any load failure — callers then fall back to built-in profiles + the
/// default routing. Mirrors `apps/cli/src/init.rs`'s `load_provider_profiles`
/// + `load_routing`.
#[must_use]
fn load_settings_blocks() -> (Option<BTreeMap<String, serde_json::Value>>, Option<serde_json::Value>)
{
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let inputs = engine::settings::LoadInputs {
        env: &env,
        project_dir: &project_dir,
        defaults: engine::settings::schema::SettingsJson::default(),
    };
    match engine::settings::Settings::load(inputs) {
        Ok(eff) => (eff.settings.providers, eff.settings.routing),
        Err(_) => (None, None),
    }
}

/// Resolve a deterministic [`DesktopConfig`] from [`BridgeArgs`] + `std::env`.
///
/// Mirrors `apps/cli/src/init.rs`'s `resolve_desktop_config` field-for-field,
/// with the one production difference for a transport: `use_noop_permission_gate`
/// is `false`, so `engine_desktop::build` binds the connection-scoped
/// `AdapterPermissionGate` (the WS permission round-trip, F2-06).
///
/// `cwd` is `std::env::current_dir()` — the caller is expected to have already
/// `chdir`'d into any `--cwd` (so the settings / hook / `.mcp.json` loaders read
/// the same dir).
///
/// The `ANTHROPIC_API_KEY` env value is read here but is NEVER logged; an empty
/// key is a valid config (the server boots for transport testing and only a live
/// turn fails with a 401 — surfaced to the client as a terminal `Error` event).
/// User config-home: `$LINGXI_CONFIG_DIR` when set (claude-code `tr()` `??`: an
/// empty value is honored verbatim → cwd-relative), else `~/.claude`. Shared by
/// the bridge's desktop-config + lockfile resolution.
#[must_use]
pub fn lingxi_config_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(branding::CONFIG_DIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    dirs::home_dir().map(|h| h.join(branding::DOT_DIR))
}

#[must_use]
pub fn resolve_desktop_config(args: &BridgeArgs) -> DesktopConfig {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let lingxi_home = lingxi_config_home().unwrap_or_else(|| PathBuf::from("/dev/null"));
    let project_mcp_path = cwd.join(".mcp.json");
    // User/global-scope MCP servers live INSIDE `~/.lingxi.json` (top-level
    // `mcpServers`), exactly like claude-code — NOT a standalone file under the
    // OS config dir. The loader reads only that key
    // (`mcp::parse_global_config_mcp_servers`).
    let global_mcp_path = migrations::global_config::global_config_path()
        .unwrap_or_else(|| PathBuf::from("/dev/null"));

    let mut default_model = DesktopConfig::default().default_model;
    if let Some(m) = &args.model {
        default_model.clone_from(m);
    }

    let (provider_profiles, routing) = load_settings_blocks();

    DesktopConfig {
        api_base: resolve_api_base(),
        api_key: std::env::var(API_KEY_ENV).unwrap_or_default(),
        cwd,
        lingxi_home,
        default_model,
        // The Electron bridge does not expose --fallback-model (CLI --print
        // only); the Opus consecutive-529 fallback stays disabled here.
        fallback_model: None,
        provider_profiles,
        routing,
        mcp_paths: vec![project_mcp_path, global_mcp_path],
        // A transport binds the connection-scoped AdapterPermissionGate (F2-06).
        use_noop_permission_gate: false,
        // Transport host: the adapter gate IS the enforcement, so headless
        // deny-on-ask does not apply (only consulted when use_noop is true).
        deny_unresolved_ask: false,
        // Transport host: no injected interactive gate (that is the TUI's path).
        injected_permission_gate: None,
        // M10: the bridge-server does not start a coordinator session by
        // default (threading this from session metadata is a follow-up).
        session_started_as_coordinator: false,
        // Production memory: load the real `<cwd>/LINGXI.md` +
        // `~/.lingxi/LINGXI.md` hierarchy into the system prompt (claude-code
        // parity), which also makes the session-start
        // `fire_instructions_loaded()` fire over those files. Tests inject a
        // controlled provider (or `None`); only this real-host path reads the FS.
        memory_provider: Some(orchestrator::prompt::real_provider()),
        // The Electron bridge has no permission-mode CLI flag; default mode.
        permission_mode: permission::PermissionMode::Default,
        // Plan 3c: bridge has no interactive secure prompt; headless no-op.
        connect_prompt: None,
        // The Electron bridge exposes no --max-turns / --max-budget flags;
        // both stay unset (unbounded), matching the CLI defaults.
        max_turns: None,
        max_budget_usd: None,
        // The bridge has no structured-output flag; unconstrained turns.
        json_schema: None,
        // The bridge has no --system-prompt / --append-system-prompt CLI flags.
        system_prompt_override: None,
        append_system_prompt: None,
        // The Electron bridge has no --session-id flag (the SDK/bridge path mints
        // its own ids); always a fresh session id.
        session_id_override: None,
        // The Electron bridge has no --disable-slash-commands flag.
        disable_slash_commands: false,
        // The Electron bridge has no --add-dir flag.
        add_dir: Vec::new(),
        // The Electron bridge has no --mcp-config flag.
        cli_mcp_servers: Vec::new(),
        // The Electron bridge has no --exclude-dynamic-system-prompt-sections flag.
        exclude_dynamic_system_prompt_sections: false,
        // The bridge has no `--setting-sources` flag; load all tiers.
        setting_source_scope: (true, true),
        // The Electron bridge has no --safe-mode / --bare flags (CLI-only
        // reduced modes); all customizations load.
        customization_gates: engine_desktop::CustomizationGates::default(),
        // The Electron bridge has no --no-session-persistence flag; persist.
        session_persistence: true,
    }
}

/// True iff the config has neither an API key NOR any settings-configured
/// provider profile — i.e. no way to authenticate a live turn. The server still
/// boots (transport testing is valuable), but the caller logs a clear,
/// NON-SECRET warning so the operator knows turns will 401.
#[must_use]
pub fn has_no_credential_source(cfg: &DesktopConfig) -> bool {
    cfg.api_key.is_empty()
        && cfg
            .provider_profiles
            .as_ref()
            .is_none_or(BTreeMap::is_empty)
}

/// The assembled, ready-to-serve connection plus its auth-relevant facts.
///
/// The connection is the [`bridge::FramePump`] the endpoint drives; the runtime
/// is held so its background tasks (cost-persist drain, etc.) stay alive for the
/// process lifetime.
pub struct BoundServer {
    /// The fully-bound connection (driver + router attached).
    pub connection: BridgeConnection,
    /// The desktop runtime — held (not read) so its handles and the spawned
    /// background tasks it owns (the cost-persist drain, the orchestrator's
    /// registries) stay alive for as long as the server serves. Private so the
    /// `_` hold-alive intent is expressed without a `pub` dead-field lint.
    #[allow(dead_code)]
    runtime: DesktopRuntime,
}

/// Assemble a fully-bound [`BridgeConnection`] from a resolved [`DesktopConfig`].
///
/// Wires the connection-scoped sinks into a real [`DesktopRuntime`]
/// (`engine_desktop::build`), then binds the production
/// [`OrchestratorTurnDriver`] (with an error sink so a turn-level failure
/// reaches the client) and the [`EngineCommandRouter`] over the runtime handles.
///
/// # Errors
/// Returns the [`engine_desktop::BuildError`] string if the engine cannot be
/// assembled (effectively infallible today).
pub async fn assemble(cfg: DesktopConfig) -> Result<BoundServer, String> {
    let connection = BridgeConnection::new();

    // The orchestrator's output stream + the gate's request sink BOTH ride the
    // same connection-scoped outbound channel (the F2-06 contract).
    let event_sink = connection.event_sink();
    let output: Arc<dyn OutputStream> =
        Arc::new(client_adapter::AdapterOutputStream::new(event_sink.clone()));
    let permission_sink = connection.permission_sink();

    let runtime = build(cfg, output, permission_sink)
        .await
        .map_err(|e| e.to_string())?;

    // `use_noop_permission_gate: false` ⇒ build MUST surface the adapter gate.
    let gate = runtime.permission_gate.clone().ok_or_else(|| {
        "engine_desktop::build did not surface an AdapterPermissionGate despite \
         use_noop_permission_gate=false"
            .to_string()
    })?;

    // §27 mid-turn drain + Now-abort wiring. The per-connection queue is shared
    // (Arc) across THREE consumers: (1) the orchestrator's mid-turn input source
    // (injects queued `Next`/`Now` prompts WITHIN a running turn), (2) the turn
    // driver (registers each turn's cancel token so a `Now` enqueue aborts it),
    // and (3) the existing between-turn `drain_main_thread` loop. A
    // `CancelReasonFlag` is shared between the orchestrator (reads it to tell a
    // `Now`-command abort from a user interrupt) and the queue's now-abort hook
    // (sets it to `QueueNowCommand` right before firing the active-turn token).
    let queue = connection.queue_handle();
    let cancel_reason = orchestrator::prompt::mid_turn_input::CancelReasonFlag::new();
    runtime
        .orchestrator
        .set_mid_turn_input(Arc::new(crate::driver::MsgQueueMidTurnInput::new(
            queue.clone(),
        )));
    runtime.orchestrator.set_cancel_reason(cancel_reason.clone());
    {
        let reason = cancel_reason.clone();
        queue
            .set_now_abort_hook(Arc::new(move || {
                reason.set(orchestrator::prompt::mid_turn_input::CancelReason::QueueNowCommand);
            }))
            .await;
    }

    // Phase-2 /loop dynamic mode (ScheduleWakeup): fill the registered tool's
    // set-once wakeup cell now that the per-connection `queue` + a `RuntimeSpawner`
    // both exist. `MsgQueueWakeupScheduler` sleeps for the (clamped) delay, resolves
    // the `<<autonomous-loop-dynamic>>` sentinel, then enqueues the `/loop` input at
    // `Next` so the between-turn drain folds it back into the session. The cell was
    // threaded out of `engine_desktop::build` on `DesktopRuntime` precisely because
    // the tool is constructed before this seam. Setting it more than once is a no-op
    // (`OnceLock`); a fresh per-connection `assemble` builds a fresh runtime + cell.
    let wakeup_scheduler: Arc<dyn tool_cron::WakeupScheduler> =
        Arc::new(crate::driver::MsgQueueWakeupScheduler::new(
            queue.clone(),
            runtime.runtime_spawner.clone(),
        ));
    // The driver re-uses the SAME scheduler at its turn-completion edge to arm the
    // `/loop` keepalive fallback (binary `lKi`); clone before the cell consumes it.
    let driver_wakeup_scheduler = wakeup_scheduler.clone();
    if runtime.wakeup_scheduler_cell.set(wakeup_scheduler).is_err() {
        // Already filled — should not happen for a fresh runtime, but never panic
        // at the composition root over a benign double-wire.
    }

    // Production turn driver: errors surface as a terminal `ClientEvent::Error`
    // through the SAME connection-scoped event sink. Wired with the connection's
    // queue + the shared reason flag so each turn registers its cancel token, and
    // the wakeup scheduler so the turn-end edge can arm the keepalive fallback.
    let driver = Arc::new(
        OrchestratorTurnDriver::with_error_sink(runtime.orchestrator.clone(), event_sink)
            .with_queue(queue, cancel_reason)
            .with_wakeup_scheduler(driver_wakeup_scheduler),
    );

    // The full command-routing seam over the real engine handles.
    let handle: Arc<dyn OrchestratorHandle> = runtime.orchestrator.clone();
    let dispatcher: Arc<dyn SlashCommandDispatcher> =
        Arc::new(RegistrySlashDispatcherClone::wrap(&runtime));
    let router = Arc::new(EngineCommandRouter::new(
        handle,
        runtime.auth.clone(),
        runtime.task_registry.clone() as Arc<dyn traits::task_registry::TaskRegistryHandle>,
        Some(dispatcher),
    ));

    let connection = connection.bind(gate, driver).bind_router(router);
    Ok(BoundServer {
        connection,
        runtime,
    })
}

/// The `RegistrySlashDispatcher` returned by `engine_desktop::build` is owned by
/// value on the runtime, but the router needs an `Arc<dyn SlashCommandDispatcher>`
/// it can hold for the connection's lifetime. We cannot move the dispatcher out
/// of the runtime (we keep the runtime alive), so this thin newtype clones the
/// dispatcher's shared registry handle into a fresh dispatcher pointing at the
/// SAME registry — dispatching is identical.
struct RegistrySlashDispatcherClone {
    inner: command_api::RegistrySlashDispatcher,
}

impl RegistrySlashDispatcherClone {
    fn wrap(runtime: &DesktopRuntime) -> Self {
        Self {
            inner: runtime.dispatcher.clone_shared(),
        }
    }
}

#[async_trait::async_trait]
impl SlashCommandDispatcher for RegistrySlashDispatcherClone {
    async fn dispatch(&self, raw: &str) -> traits::SlashDispatchResult {
        self.inner.dispatch(raw).await
    }
}

/// A running endpoint paired with the Drop-guard that reaps its discovery
/// lockfile.
///
/// Returned by [`publish_lockfile`]: holding the [`LockfileGuard`] keeps the
/// `~/.lingxi/bridge/<port>.lock` file on disk for the server's lifetime; it is
/// removed when this struct is dropped (clean shutdown OR panic unwind).
pub struct ServedEndpoint {
    /// The bound loopback endpoint serving the connection.
    pub endpoint: McpEndpoint,
    /// Absolute path of the discovery lockfile this endpoint published.
    pub lockfile_path: PathBuf,
    /// Drop-guard that removes [`Self::lockfile_path`] when this struct drops.
    /// Kept as a field (not `_`) so callers can [`LockfileGuard::disarm`] it in
    /// tests; in the binary it simply lives for the process lifetime.
    pub lock_guard: LockfileGuard,
}

/// Write the F2-04 discovery lockfile for `endpoint`'s port and enforce its
/// `authToken` on the endpoint — the env-free half of the binary's step (4),
/// factored into the library so the headless serve-path test drives the SAME
/// code (keeping `main.rs` thin).
///
/// `lockfile_dir` is the directory the `<port>.lock` file is written into: the
/// binary passes its `~/.lingxi/bridge` dir (via [`IdeLockfile::for_bridge`]);
/// the test passes a `tempfile::TempDir` so no real `$HOME` is touched. The
/// `workspace_folders` are recorded verbatim in the lockfile body.
///
/// The freshly-generated token is enforced on the endpoint (so the published
/// `authToken` is the one a connecting client MUST present) and is NEVER logged.
/// The returned [`ServedEndpoint`] owns a [`LockfileGuard`] that reaps the file
/// on drop.
#[must_use]
pub fn publish_lockfile(
    endpoint: McpEndpoint,
    lockfile_dir: PathBuf,
    workspace_folders: Vec<PathBuf>,
) -> ServedEndpoint {
    let port = endpoint.port();
    let lockfile = IdeLockfile::new_for_bridge_dir(lockfile_dir, port, workspace_folders);
    // An I/O failure writing the discovery file is non-fatal to serving — the
    // endpoint is already bound — but we surface it via the guard's empty path
    // so a failed write still yields a coherent (file-absent) ServedEndpoint.
    let lockfile_path = lockfile.path();
    if let Err(e) = lockfile.write() {
        tracing::warn!(path = %lockfile_path.display(), error = %e, "failed to write bridge lockfile");
    }
    // The endpoint MUST enforce the SAME token the lockfile published.
    endpoint.set_auth_token(lockfile.auth_token().to_string());
    let lock_guard = LockfileGuard::new(lockfile_path.clone());
    ServedEndpoint {
        endpoint,
        lockfile_path,
        lock_guard,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_defaults_when_no_args() {
        let args = BridgeArgs::parse(Vec::<String>::new()).expect("empty parses");
        assert_eq!(args, BridgeArgs::default());
        assert!(!args.help);
    }

    #[test]
    fn parse_cwd_and_model() {
        let args = BridgeArgs::parse(["--cwd", "/tmp/p", "--model", "claude-x"])
            .expect("flags parse");
        assert_eq!(args.cwd, Some(PathBuf::from("/tmp/p")));
        assert_eq!(args.model.as_deref(), Some("claude-x"));
    }

    #[test]
    fn parse_help_flag() {
        assert!(BridgeArgs::parse(["--help"]).unwrap().help);
        assert!(BridgeArgs::parse(["-h"]).unwrap().help);
    }

    #[test]
    fn parse_missing_value_is_error() {
        assert!(BridgeArgs::parse(["--cwd"]).is_err());
        assert!(BridgeArgs::parse(["--model"]).is_err());
    }

    #[test]
    fn parse_unknown_flag_is_error() {
        let err = BridgeArgs::parse(["--frobnicate"]).unwrap_err();
        assert!(err.contains("--frobnicate") || err.contains("frobnicate") || err.contains("frobni") || err.contains("unknown"), "got: {err}");
    }

    #[test]
    fn usage_mentions_key_env_but_never_a_value() {
        let u = usage();
        assert!(u.contains(API_KEY_ENV), "usage must name the key env var");
        // The usage text is static — it can never carry a secret value.
        assert!(!u.contains("sk-"), "usage must not embed a key literal");
    }

    #[test]
    fn resolve_uses_model_override() {
        let args = BridgeArgs {
            model: Some("custom-model".to_string()),
            ..BridgeArgs::default()
        };
        let cfg = resolve_desktop_config(&args);
        assert_eq!(cfg.default_model, "custom-model");
        // A transport always binds the adapter gate.
        assert!(!cfg.use_noop_permission_gate);
    }

    #[test]
    fn has_no_credential_source_detects_empty() {
        let mut cfg = DesktopConfig {
            api_key: String::new(),
            provider_profiles: None,
            ..DesktopConfig::default()
        };
        assert!(has_no_credential_source(&cfg));

        cfg.api_key = "x".to_string();
        assert!(!has_no_credential_source(&cfg));

        cfg.api_key = String::new();
        cfg.provider_profiles = Some(BTreeMap::new());
        assert!(has_no_credential_source(&cfg), "empty profiles map is still no source");

        let mut profiles = BTreeMap::new();
        profiles.insert("p".to_string(), serde_json::json!({}));
        cfg.provider_profiles = Some(profiles);
        assert!(!has_no_credential_source(&cfg));
    }

    /// Assembly produces a fully-bound connection (gate + driver attached) from a
    /// deterministic config — no env reads, no live network. Proves the S2 wiring
    /// links end to end.
    #[tokio::test]
    async fn assemble_binds_connection() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().to_path_buf();
        let cfg = DesktopConfig {
            api_base: DEFAULT_API_BASE.to_string(),
            api_key: String::new(),
            cwd: cwd.clone(),
            lingxi_home: cwd.join(".lingxi"),
            default_model: "claude-sonnet-4-20250514".to_string(),
            fallback_model: None,
            provider_profiles: None,
            routing: None,
            mcp_paths: vec![cwd.join(".mcp.json")],
            use_noop_permission_gate: false,
            deny_unresolved_ask: false,
            injected_permission_gate: None,
            session_started_as_coordinator: false,
            // Deterministic test: empty memory, never the real FS.
            memory_provider: None,
            permission_mode: permission::PermissionMode::Default,
            connect_prompt: None,
            max_turns: None,
            max_budget_usd: None,
            json_schema: None,
            system_prompt_override: None,
            append_system_prompt: None,
            session_id_override: None,
            disable_slash_commands: false,
            add_dir: Vec::new(),
            cli_mcp_servers: Vec::new(),
            exclude_dynamic_system_prompt_sections: false,
            setting_source_scope: (true, true),
            customization_gates: engine_desktop::CustomizationGates::default(),
            session_persistence: true,
        };
        let bound = assemble(cfg).await.expect("assemble must succeed");
        // The gate handle is reachable only when bind() ran with a real gate.
        let gate = bound.connection.gate_handle();
        assert_eq!(gate.pending_count().await, 0, "fresh gate has no parked requests");
        // Phase-2 /loop wiring: assemble fills the ScheduleWakeup cell with the
        // msgqueue-backed scheduler (so the tool is no longer a no-op on the bridge).
        assert!(
            bound.runtime.wakeup_scheduler_cell.get().is_some(),
            "assemble must wire the ScheduleWakeup self-wakeup scheduler"
        );
    }
}
