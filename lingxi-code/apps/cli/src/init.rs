//! Build the full orchestrator pipeline from an [`Argv`].
//!
//! M5-12 baseline wiring — assembles the existing crates into a runnable
//! [`Runtime`]:
//!
//! 1. `lingxi-platform-posix-minimal` provides `HttpTransport` + `Clock` +
//!    `SecureStorage`.
//! 2. `lingxi-api-client::AnthropicProvider` is built from the resolved
//!    `LINGXI_API_BASE_URL` (default `https://api.anthropic.com`) +
//!    `ANTHROPIC_API_KEY` (when present).
//! 3. `lingxi-anthropic-oauth::ClaudeAiOAuthClient` wraps the credential
//!    manager so `/login` + `/logout` (M5-11) have a real handle.
//! 4. `lingxi-orchestrator::ConversationOrchestrator` is constructed with
//!    `test_support` fillers for the hook/permission/memory slots that
//!    don't yet have production constructors — documented inherited gap
//!    from M5-10 / M5-11 `handle_impl` stubs.
//! 5. `lingxi-commands::RegistrySlashDispatcher` wraps a `CommandRegistry`
//!    populated by `register_all_builtin_commands` → `register_core_batch_1`
//!    → `register_core_batch_2` (the orchestrator implements both
//!    `OrchestratorHandle` via M5-10/M5-11).
//!
//! Future work (M5-13/M5-14): swap the `test_support` fillers for real
//! permission gate, hook executor, MCP/agent registries.

use crate::argv::Argv;
use anthropic_oauth::client::ClaudeAiOAuthClient;
use anthropic_oauth::config::ClaudeAiOAuthConfig;
use anthropic_oauth::handle::OAuthHandle;
use api_client::AnthropicProvider;
use command_api::RegistrySlashDispatcher;
use engine_desktop::{desktop_command_registry, desktop_tool_registry};
use orchestrator::test_support::{noop_hook_executor, NoOpPermissionGate, StaticMemoryProvider};
use orchestrator::{
    AnthropicProviderAdapter, ConversationOrchestrator, OrchestratorApiClient, OrchestratorConfig,
};
use permission::PermissionMode;
use platform_posix_minimal::{
    PlainTextSecureStorage, PosixClock, PosixFileSystem, PosixHttp, PosixMcp, PosixProcess,
    PosixSandbox, PosixWorktree,
};
use sandbox::decision::ProjectTrustLevel;
use sandbox::runtime_config::{Platform as SandboxPlatform, SandboxRuntimeConfig};
use secret::CredentialManager;
use std::sync::Arc;
use tokio::sync::RwLock;
use tool_api::BuiltinToolContext;
use traits::{AuthHandle, McpTransport, OrchestratorHandle, OutputStream};

/// Bundle of everything `run_cli` needs to drive a conversation.
pub struct Runtime {
    /// The fully-constructed orchestrator.
    pub orchestrator: Arc<ConversationOrchestrator>,
    /// Slash-command dispatcher seeded with the 99 builtins + 18 wired core
    /// handlers (M5-09/M5-10/M5-11).
    pub dispatcher: RegistrySlashDispatcher,
    /// Auth handle for `/login` and `/logout`.
    pub auth: Arc<dyn AuthHandle>,
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
#[derive(Debug, thiserror::Error)]
pub enum InitError {
    /// API base URL resolution / construction failed.
    #[error("api base resolution failed: {0}")]
    ApiBase(String),
    /// Orchestrator construction failed (currently infallible).
    #[error("orchestrator construction failed: {0}")]
    Orchestrator(String),
}

/// Resolve the API base URL: honours `LINGXI_API_BASE_URL` (used by tests
/// to inject a mock), otherwise the canonical Anthropic endpoint.
#[must_use]
pub fn resolve_api_base() -> String {
    std::env::var("LINGXI_API_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".to_string())
}

/// Build the full runtime from parsed argv + the chosen output stream.
///
/// `output` is the sink the orchestrator will push turn events to (plain
/// stdout or NDJSON, projected from `crate::output::OutputSink` through
/// `crate::output_adapter::SinkAdapter`). M5-12 Task 9 wires this end-to-end
/// so `--json` produces NDJSON `text` / `tool_call` / `turn_end` lines.
///
/// Currently no `.await` is needed inside the constructor, but the signature
/// remains `async` so future iterations (real OAuth token bootstrap, MCP
/// server connect) can plug in without changing every call site.
#[allow(clippy::unused_async, clippy::too_many_lines)]
pub async fn build_runtime(
    argv: &Argv,
    output: Arc<dyn OutputStream>,
) -> Result<Runtime, InitError> {
    let api_base = resolve_api_base();
    let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();

    // (1) Platform-minimal façade (http + clock + storage). These exist in
    //     production today; nothing experimental here.
    let http = Arc::new(PosixHttp::new());
    let clock = Arc::new(PosixClock::new());
    let storage = Arc::new(PlainTextSecureStorage::new());

    // (2) Build the api-client (AnthropicProvider). Note that an empty
    //     api_key is accepted — the orchestrator's `run_turn` will fail
    //     with a 401 if no real key is configured, but the CLI binary
    //     itself constructs successfully so slash-command dispatch still
    //     works without an API key.
    let provider = AnthropicProvider::new(api_key.clone(), Some(api_base.clone()));
    let api_client: Arc<dyn OrchestratorApiClient> =
        Arc::new(AnthropicProviderAdapter::new(provider, http.clone()));
    // M8-P6: the tool context's WebSearch tool builds `POST /v1/messages`
    // requests through its own `Arc<AnthropicProvider>`. `AnthropicProvider`
    // isn't `Clone`, so build a second cheap instance (it only stores the
    // api key + base URL).
    let tool_provider = Arc::new(AnthropicProvider::new(api_key, Some(api_base.clone())));

    // (3) Credential manager + OAuth client (used by /login, /logout).
    let credentials = Arc::new(CredentialManager::new(storage, clock.clone(), http.clone()));
    let oauth_cfg = ClaudeAiOAuthConfig::default_with_port(0);
    let oauth_client = Arc::new(ClaudeAiOAuthClient::new(
        oauth_cfg,
        http.clone(),
        credentials,
    ));
    let auth: Arc<dyn AuthHandle> = Arc::new(OAuthHandle::new(oauth_client));

    // (4) Orchestrator config from argv. `OrchestratorConfig` does not
    //     carry a streaming toggle directly — streaming vs. batched is
    //     picked by which constructor the caller uses (`new` = batched
    //     only, `new_with_streaming` = both). M5-12 baseline always
    //     constructs via `new` (batched), honouring `--no-stream` by
    //     default. Live `--stream` lands when `new_with_streaming` is
    //     wired in a future plan.
    let mut cfg = OrchestratorConfig::default();
    if let Some(m) = &argv.model {
        cfg.model.clone_from(m);
    }
    // `--no-stream` is always honoured in M5-12 because the baseline
    // pipeline only wires the batched constructor. Read the flag (silence
    // unused-field warning) and proceed.
    let _ = argv.no_stream;

    // (4.5) M6-06: Construct one CostTracker per process. The persist
    //       channel drains into a fire-and-forget task that discards
    //       snapshots in v0.7.0 — on-disk cost-state persistence is M7
    //       work. Channel depth 64 absorbs short bursts without blocking
    //       record_api_response_v2.
    let (cost_persist_tx, mut cost_persist_rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move {
        // Discard snapshots — v0.7.0 does not persist cost.
        while cost_persist_rx.recv().await.is_some() {}
    });
    let cost_tracker = Arc::new(cost::CostTracker::new(
        protocol::SessionId::new(),
        Arc::new(cost::PricingCatalog::builtin_reference()),
        cost_persist_tx,
    ));

    // (5) Build the orchestrator using test_support fillers for the
    //     hook/permission/memory slots. These are the documented inherited
    //     M5-10/M5-11 gaps — production constructors land in M5-13+.
    //
    //     M8-P6: the tool registry is no longer constructed empty here — it is
    //     assembled below (after the MCP registry exists) through the desktop
    //     composition root `engine_desktop::desktop_tool_registry`.
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let memory: Arc<dyn orchestrator::prompt::MemoryHierarchyProvider> =
        Arc::new(StaticMemoryProvider::empty());
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // (5.1) M6-07: Load `.mcp.json` (project preferred over user-global) and
    //       pre-populate the McpRegistry with `Disconnected` state entries
    //       so `/mcp` can list them. Real connect / health-check is M7 work.
    let global_mcp_path = dirs::config_dir().map_or_else(
        || std::path::PathBuf::from("/dev/null"),
        |d| d.join("lingxi").join("mcp.json"),
    );
    let project_mcp_path = cwd.join(".mcp.json");
    let mcp_configs = mcp::load_mcp_json_with_precedence(&project_mcp_path, &global_mcp_path);
    let mcp_transport: Arc<dyn McpTransport> = Arc::new(PosixMcp::new());
    let mcp_registry = Arc::new(mcp::McpRegistry::new(mcp_transport));
    {
        let mut conns = mcp_registry.connections.write().await;
        for cfg_entry in mcp_configs {
            let name = cfg_entry.name.clone();
            conns.insert(
                name,
                mcp::McpConnectionState::Disconnected {
                    config: cfg_entry,
                    last_error: None,
                },
            );
        }
    }

    // (5.2) M6-07: HookRegistry — read settings.json hooks block from
    //       project (cwd/.claude/settings.json) and user (~/.claude/settings.json
    //       or platform config_dir equivalent), in that order so project
    //       wins on identical command registration (the registry currently
    //       de-dupes by HookId, not name — both register; /hooks lists both).
    let mut hook_registry = hooks::HookRegistry::new();
    let project_settings_path = cwd.join(".claude").join("settings.json");
    let user_settings_path = dirs::config_dir().map_or_else(
        || std::path::PathBuf::from("/dev/null"),
        |d| d.join("claude").join("settings.json"),
    );
    for (path, source) in [
        (user_settings_path, hooks::definition::HookSource::User),
        (
            project_settings_path,
            hooks::definition::HookSource::Project,
        ),
    ] {
        if let Ok(raw) = tokio::fs::read_to_string(&path).await {
            match hooks::parse_hooks_from_settings_json(&raw, source) {
                Ok(hooks_vec) => {
                    for h in hooks_vec {
                        hook_registry.register(h);
                    }
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "skipping malformed settings hooks"
                ),
            }
        }
    }
    let hook_registry = Arc::new(tokio::sync::RwLock::new(hook_registry));

    // (5.3) M6-07: Agent catalog — load from project + user agents/.
    //       Project wins on agent_type collision because it is passed
    //       SECOND to load_agents_from_dirs (later paths win).
    let project_agents_dir = cwd.join(".claude").join("agents");
    let user_agents_dir = dirs::home_dir().map_or_else(
        || std::path::PathBuf::from("/dev/null"),
        |h| h.join(".claude").join("agents"),
    );
    let agents = agent::load_agents_from_dirs(&[
        (user_agents_dir, agent::definition::AgentSource::UserDefined),
        (project_agents_dir, agent::definition::AgentSource::Project),
    ])
    .await;
    let agent_catalog = Arc::new(tokio::sync::RwLock::new(agents));

    // (5.4) M6-08: Real compaction. Threshold defaults to 150_000 tokens —
    //       matches M3's design lock for the Anthropic prod context
    //       window. Default Autocompactor (no `with_forked_runner`)
    //       returns a stub summary string; real LLM summarization lands
    //       in M7 when the ForkedAgentRunner pool is wired.
    let compactor = Arc::new(compaction::CompactionOrchestrator::new(150_000));

    // (5.5) M8-P6: assemble the desktop tool registry through the composition
    //       root. The orchestrator previously received an empty
    //       `ToolRegistry::new()`; `engine-desktop` now owns the desktop tool
    //       set (14 tool crates) and we build the `BuiltinToolContext` from the
    //       posix platform handles + session policy. MCP tools share the
    //       orchestrator's `McpRegistry`; subagent/task/mailbox/budget/LSP seams
    //       stay `None` until their production pools are wired (M9+).
    let tool_ctx = BuiltinToolContext {
        fs: Arc::new(PosixFileSystem::new(cwd.clone())),
        bus: Arc::new(telemetry::AnalyticsBus::new()),
        trusted_dirs: vec![cwd.clone()],
        process: Arc::new(PosixProcess::new()),
        sandbox: Arc::new(PosixSandbox::new()),
        clock: clock.clone(),
        sandbox_runtime: SandboxRuntimeConfig::default(),
        permission_mode: PermissionMode::Default,
        project_trust: ProjectTrustLevel::Trusted,
        sandbox_available: false,
        workspace: cwd.clone(),
        platform: if cfg!(target_os = "macos") {
            SandboxPlatform::Mac
        } else {
            SandboxPlatform::Linux
        },
        http: http.clone(),
        provider: tool_provider,
        default_model: cfg.model.clone(),
        worktree: Arc::new(PosixWorktree::new()),
        subagent_spawner: None,
        task_registry: None,
        mailbox_router: None,
        budget_enforcer: None,
        mcp_registry: Some(mcp_registry.clone()),
        lsp_registry: None,
        // Mobile / device-control capabilities are not wired on desktop (M8).
        camera: None,
        voice: None,
        share: None,
        computer_control: None,
    };
    let tools = Arc::new(desktop_tool_registry(tool_ctx));

    let orch = Arc::new(
        ConversationOrchestrator::new(cfg, api_client, tools, hooks, perms, output, memory, cwd)
            .with_cost_tracker(cost_tracker)
            .with_mcp_registry(mcp_registry)
            .with_hook_registry(hook_registry)
            .with_agent_catalog(agent_catalog)
            .with_compaction(compactor),
    );

    // (6) Build the command registry through the desktop composition root.
    //     The orchestrator implements `OrchestratorHandle` via M5-10 + M5-11;
    //     `engine_desktop::desktop_command_registry` owns the seed →
    //     batch-1 → batch-2 sequence that was previously inlined here.
    let handle: Arc<dyn OrchestratorHandle> = orch.clone();
    let reg = desktop_command_registry(handle, auth.clone());
    let dispatcher = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));

    Ok(Runtime {
        orchestrator: orch,
        dispatcher,
        auth,
    })
}

/// TUI variant of [`build_runtime`]. (M6-03)
///
/// Constructs the orchestrator with [`tui::BridgeOutputStream`] as
/// its `output` so streaming `emit_text` calls route into the bridge
/// channel returned alongside the runtime. The TUI render loop drains
/// this channel through `tui::streaming::apply_event`.
#[allow(clippy::unused_async)]
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
