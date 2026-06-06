//! Build the full orchestrator pipeline from an [`Argv`].
//!
//! **F2-01**: the ~270-line runtime wiring was lifted into the desktop
//! composition root — [`engine_desktop::build`] now assembles the orchestrator
//! pipeline from a deterministic [`engine_desktop::DesktopConfig`]. This module
//! keeps the *env/argv-reading* half: it resolves every `DesktopConfig` field
//! from `Argv` + `std::env` (the bridge-server fills the same config without
//! ever touching the process environment), then delegates the actual assembly.
//!
//! The wiring that now lives in `engine-desktop` (`apps/engine-desktop/src/lib.rs`):
//!
//! 1. `platform-posix-minimal` provides `HttpTransport` + `Clock` +
//!    `SecureStorage`.
//! 2. The api-client is built (via `ProviderRegistry`) from `cfg.api_base`
//!    (default `https://api.anthropic.com`) + `cfg.api_key`.
//! 3. `anthropic-oauth::ClaudeAiOAuthClient` wraps the credential manager so
//!    `/login` + `/logout` have a real handle.
//! 4. `orchestrator::ConversationOrchestrator` is constructed with
//!    `test_support` fillers for the hook/memory slots and — because the CLI
//!    sets `use_noop_permission_gate: true` — the always-allow
//!    `NoOpPermissionGate`.
//! 5. `command-api::RegistrySlashDispatcher` wraps the `CommandRegistry`
//!    populated through the desktop composition root.

use crate::argv::Argv;
use async_trait::async_trait;
use client_protocol::permission::PermissionRequest as PermissionRequestDto;
use command_api::RegistrySlashDispatcher;
use engine_desktop::{build, DesktopConfig};
use orchestrator::ConversationOrchestrator;
use std::sync::Arc;
use traits::{AuthHandle, OutputStream};

/// Bundle of everything `run_cli` needs to drive a conversation.
pub struct Runtime {
    /// The fully-constructed orchestrator.
    pub orchestrator: Arc<ConversationOrchestrator>,
    /// Slash-command dispatcher seeded with the 99 builtins + 18 wired core
    /// handlers (M5-09/M5-10/M5-11).
    pub dispatcher: RegistrySlashDispatcher,
    /// Auth handle for `/login` and `/logout`.
    pub auth: Arc<dyn AuthHandle>,
    /// (M9-05) The desktop task registry shared with the tool context. The TUI
    /// mount wraps it in a `PollerFeed` so the background-task footer + dialog
    /// read live state.
    pub task_registry: Arc<tasks::registry::TaskRegistry>,
    /// Live settings watcher firing `ConfigChange` hooks when settings files
    /// mutate on disk. Held here purely to keep the watcher alive for the
    /// session: dropping the `Runtime` (process teardown) aborts the watch
    /// tasks (RAII). If this field were dropped at `build_runtime` exit, the
    /// watcher would stop immediately after boot — so it must live on `Runtime`.
    pub settings_watcher: engine_desktop::settings_watch::SettingsWatcherHandle,
}

/// Build-result for the TUI startup path. (M6-03)
///
/// Returns the standard [`Runtime`] plus the bridge receiver the TUI
/// drains for streaming events. The orchestrator inside `runtime` is
/// constructed with a [`tui::BridgeOutputStream`] as its `output`,
/// so every `emit_text` / `emit_tool_call` / `emit_end_turn` lands on
/// `bridge_rx` as a [`tui::TurnEvent`].
pub struct TuiBuild {
    /// Standard runtime bundle.
    pub runtime: Runtime,
    /// Bridge receiver — the TUI render loop drains this into
    /// `tui::streaming::apply_event`.
    pub bridge_rx:
        tokio::sync::mpsc::UnboundedReceiver<tui::events::orchestrator_bridge::TurnEvent>,
}

/// Errors surfaced while building a [`Runtime`].
///
/// F2-01: the underlying assembly moved to [`engine_desktop::build`]; this enum
/// is a thin projection of [`engine_desktop::BuildError`] kept for source
/// compatibility with the CLI's existing call sites.
#[derive(Debug, thiserror::Error)]
pub enum InitError {
    /// API base URL resolution / construction failed.
    #[error("api base resolution failed: {0}")]
    ApiBase(String),
    /// Orchestrator construction failed (currently infallible).
    #[error("orchestrator construction failed: {0}")]
    Orchestrator(String),
}

impl From<engine_desktop::BuildError> for InitError {
    fn from(e: engine_desktop::BuildError) -> Self {
        match e {
            engine_desktop::BuildError::ApiBase(m) => Self::ApiBase(m),
            engine_desktop::BuildError::Orchestrator(m) => Self::Orchestrator(m),
        }
    }
}

/// A no-op [`PermissionRequestSink`] for the CLI path.
///
/// The CLI builds with `use_noop_permission_gate: true`, so
/// [`engine_desktop::build`] binds the always-allow `NoOpPermissionGate` and
/// NEVER constructs an `AdapterPermissionGate` — the sink is therefore never
/// invoked. It exists only to satisfy `build`'s signature (the bridge-server
/// passes a real WS-backed sink instead).
struct NoopPermissionRequestSink;

#[async_trait]
impl client_adapter::PermissionRequestSink for NoopPermissionRequestSink {
    async fn emit_request(&self, _request: PermissionRequestDto) {
        // Unreachable on the CLI path (NoOpPermissionGate never emits).
        debug_assert!(false, "CLI uses NoOpPermissionGate — no request is emitted");
    }
}

/// Resolve the API base URL: honours `LINGXI_API_BASE_URL` (used by tests
/// to inject a mock), otherwise the canonical Anthropic endpoint.
#[must_use]
pub fn resolve_api_base() -> String {
    std::env::var("LINGXI_API_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".to_string())
}

/// Load the merged settings `providers` object (project + user + env layers).
///
/// Resolves the project dir from the *current* working directory — the process
/// has already `chdir`'d into any `--cwd` before `build_runtime` runs, so this
/// reads the same dir as the hook loader. Returns `None` on any load failure or
/// when no `providers` block is set; callers then fall back to built-in
/// profiles only.
fn load_provider_profiles() -> Option<std::collections::BTreeMap<String, serde_json::Value>> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let inputs = engine::settings::LoadInputs {
        env: &env,
        project_dir: &project_dir,
        defaults: engine::settings::schema::SettingsJson::default(),
    };
    engine::settings::Settings::load(inputs)
        .ok()
        .and_then(|eff| eff.settings.providers)
}

/// Load the merged settings `routing` object (project + user + env layers).
///
/// Mirrors [`load_provider_profiles`] but reads the `routing` field. Returns
/// `None` on any load failure or when no `routing` block is set; callers then
/// fall back to the default (empty) routing config.
fn load_routing() -> Option<serde_json::Value> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let inputs = engine::settings::LoadInputs {
        env: &env,
        project_dir: &project_dir,
        defaults: engine::settings::schema::SettingsJson::default(),
    };
    engine::settings::Settings::load(inputs)
        .ok()
        .and_then(|eff| eff.settings.routing)
}

/// Resolve a deterministic [`DesktopConfig`] from `Argv` + `std::env`.
///
/// **F2-01**: this is the *only* half of the old `build_runtime` that remains
/// in `apps/cli` — every env / argv / `dirs` read that the engine-tier
/// `engine_desktop::build` must NOT perform (so the bridge-server can build an
/// identical runtime without touching the process environment). Each field
/// mirrors the concrete read the pre-lift `build_runtime` made:
///
/// - `api_base` ← `resolve_api_base()` (env `LINGXI_API_BASE_URL`).
/// - `api_key` ← env `ANTHROPIC_API_KEY` (empty string is valid).
/// - `cwd` ← `std::env::current_dir()` (the process has already `chdir`'d into
///   any `--cwd`).
/// - `claude_home` ← `~/.claude` (the hook / agents / settings loader root).
/// - `default_model` ← `Argv::model`, else the desktop default.
/// - `fallback_model` ← `Argv::fallback_model`, but ONLY in `--print` mode
///   (claude-code restricts `--fallback-model` to non-interactive runs); the
///   interactive TUI path resolves it to `None`.
/// - `provider_profiles` ← settings `providers` block (`load_provider_profiles`).
/// - `routing` ← settings `routing` block (`load_routing`).
/// - `mcp_paths` ← `[<cwd>/.mcp.json, <config_dir>/lingxi/mcp.json]` (project
///   preferred over global), matching the precedence the old loader used.
/// - `use_noop_permission_gate` ← `true` (the CLI always binds the always-allow
///   `NoOpPermissionGate`; a transport binds `AdapterPermissionGate`).
#[must_use]
fn resolve_desktop_config(argv: &Argv) -> DesktopConfig {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let claude_home = dirs::home_dir().map_or_else(
        || std::path::PathBuf::from("/dev/null"),
        |h| h.join(".claude"),
    );
    let project_mcp_path = cwd.join(".mcp.json");
    let global_mcp_path = dirs::config_dir().map_or_else(
        || std::path::PathBuf::from("/dev/null"),
        |d| d.join("lingxi").join("mcp.json"),
    );

    let mut default_model = DesktopConfig::default().default_model;
    if let Some(m) = &argv.model {
        default_model.clone_from(m);
    }
    // Opus-fallback: `--fallback-model` parses unconditionally (`argv.rs`) but
    // claude-code only HONORS it in `--print`/non-interactive mode
    // (`main.tsx:1000` "only works with --print"). Mirror that SOFT restriction
    // here — the interactive TUI path leaves it `None`, so the turn_loop's
    // 529-overload interception stays a no-op for interactive sessions.
    let fallback_model = if argv.print {
        argv.fallback_model.clone()
    } else {
        None
    };
    // `--no-stream` is always honoured in the baseline pipeline (only the
    // batched constructor is wired). Read the flag to silence the unused-field
    // warning and preserve the pre-lift behavior.
    let _ = argv.no_stream;

    DesktopConfig {
        api_base: resolve_api_base(),
        api_key: std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
        cwd,
        claude_home,
        default_model,
        fallback_model,
        provider_profiles: load_provider_profiles(),
        routing: load_routing(),
        mcp_paths: vec![project_mcp_path, global_mcp_path],
        use_noop_permission_gate: true,
        // M10: the CLI does not start a coordinator session (threading this
        // from session metadata is a follow-up; the default is byte-identical
        // to the pre-M10 build).
        session_started_as_coordinator: false,
        // Production memory: load the real `<cwd>/CLAUDE.md` +
        // `~/.claude/CLAUDE.md` hierarchy into the system prompt (claude-code
        // parity), which also makes the session-start
        // `fire_instructions_loaded()` fire over those files. Tests inject a
        // controlled provider (or `None`); only this real-host path reads the FS.
        memory_provider: Some(orchestrator::prompt::real_provider()),
    }
}

/// Build the full runtime from parsed argv + the chosen output stream.
///
/// `output` is the sink the orchestrator will push turn events to (plain
/// stdout or NDJSON, projected from `crate::output::OutputSink` through
/// `crate::output_adapter::SinkAdapter`). M5-12 Task 9 wires this end-to-end
/// so `--json` produces NDJSON `text` / `tool_call` / `turn_end` lines.
///
/// **F2-01**: the actual assembly is delegated to [`engine_desktop::build`].
/// This function only resolves a [`DesktopConfig`] from `Argv`/env
/// ([`resolve_desktop_config`]) and projects the returned
/// [`engine_desktop::DesktopRuntime`] into the CLI's [`Runtime`]. The CLI sets
/// `use_noop_permission_gate: true`, so the supplied [`NoopPermissionRequestSink`]
/// is never invoked (the no-op gate never emits a request) and the returned
/// `permission_gate` handle is always `None`.
pub async fn build_runtime(
    argv: &Argv,
    output: Arc<dyn OutputStream>,
) -> Result<Runtime, InitError> {
    let cfg = resolve_desktop_config(argv);
    let permission_sink: Arc<dyn client_adapter::PermissionRequestSink> =
        Arc::new(NoopPermissionRequestSink);
    let rt = build(cfg, output, permission_sink).await?;
    Ok(Runtime {
        orchestrator: rt.orchestrator,
        dispatcher: rt.dispatcher,
        auth: rt.auth,
        task_registry: rt.task_registry,
        settings_watcher: rt.settings_watcher,
    })
}

/// TUI variant of [`build_runtime`]. (M6-03)
///
/// Constructs the orchestrator with [`tui::BridgeOutputStream`] as
/// its `output` so streaming `emit_text` calls route into the bridge
/// channel returned alongside the runtime. The TUI render loop drains
/// this channel through `tui::streaming::apply_event`.
pub async fn build_runtime_for_tui(argv: &Argv) -> Result<TuiBuild, InitError> {
    let (bridge_tx, bridge_rx) = tokio::sync::mpsc::unbounded_channel();
    let bridge: Arc<dyn OutputStream> = Arc::new(tui::BridgeOutputStream::new(bridge_tx));
    let runtime = build_runtime(argv, bridge).await?;
    Ok(TuiBuild { runtime, bridge_rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn build_runtime_with_defaults() {
        let argv = Argv {
            prompt: Some("hi".into()),
            print: true,
            resume: None,
            model: None,
            fallback_model: None,
            cwd: None,
            no_stream: true,
            json: false,
            debug: false,
            no_tui: false,
        };
        let output: Arc<dyn OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let r = build_runtime(&argv, output).await;
        let r = r.expect("build_runtime failed");
        // M6-06: cost tracker must be wired.
        assert!(
            r.orchestrator.has_cost_tracker(),
            "build_runtime did not wire CostTracker"
        );
        // M6-07: three registries must be wired.
        assert!(
            r.orchestrator.has_mcp_registry(),
            "build_runtime did not wire McpRegistry"
        );
        assert!(
            r.orchestrator.has_hook_registry(),
            "build_runtime did not wire HookRegistry"
        );
        assert!(
            r.orchestrator.has_agent_catalog(),
            "build_runtime did not wire agent catalog"
        );
        // M6-08: compactor must be wired.
        assert!(
            r.orchestrator.has_compaction(),
            "build_runtime did not wire CompactionOrchestrator"
        );
    }

    /// Opus-fallback hop: `--fallback-model` threads into
    /// `DesktopConfig.fallback_model` in `--print` mode, and is dropped (left
    /// `None`) in interactive mode — mirroring claude-code's "only works with
    /// --print" restriction.
    #[test]
    fn fallback_model_threads_through_in_print_mode() {
        let base = Argv {
            prompt: Some("hi".into()),
            print: true,
            resume: None,
            model: None,
            fallback_model: Some("claude-opus-4-20250514".into()),
            cwd: None,
            no_stream: true,
            json: false,
            debug: false,
            no_tui: false,
        };

        // `--print` ⟶ honored.
        let cfg = resolve_desktop_config(&base);
        assert_eq!(
            cfg.fallback_model.as_deref(),
            Some("claude-opus-4-20250514")
        );

        // Interactive (no `--print`) ⟶ dropped to `None`.
        let interactive = Argv {
            print: false,
            ..base.clone()
        };
        let cfg = resolve_desktop_config(&interactive);
        assert!(cfg.fallback_model.is_none());

        // No flag at all ⟶ `None` even in print mode.
        let absent = Argv {
            fallback_model: None,
            ..base
        };
        let cfg = resolve_desktop_config(&absent);
        assert!(cfg.fallback_model.is_none());
    }

    #[test]
    fn resolve_api_base_default() {
        // Snapshot the env first to avoid clobbering other tests.
        let prior = std::env::var("LINGXI_API_BASE_URL").ok();
        std::env::remove_var("LINGXI_API_BASE_URL");
        let base = resolve_api_base();
        assert_eq!(base, "https://api.anthropic.com");
        if let Some(v) = prior {
            std::env::set_var("LINGXI_API_BASE_URL", v);
        }
    }
}
