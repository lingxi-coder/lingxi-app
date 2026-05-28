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
use lingxi_anthropic_oauth::client::ClaudeAiOAuthClient;
use lingxi_anthropic_oauth::config::ClaudeAiOAuthConfig;
use lingxi_anthropic_oauth::handle::OAuthHandle;
use lingxi_api_client::AnthropicProvider;
use lingxi_commands::dispatcher::RegistrySlashDispatcher;
use lingxi_commands::registry::{
    register_all_builtin_commands, register_core_batch_1, register_core_batch_2, CommandRegistry,
};
use lingxi_orchestrator::test_support::{
    noop_hook_executor, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use lingxi_orchestrator::{
    AnthropicProviderAdapter, ConversationOrchestrator, OrchestratorApiClient, OrchestratorConfig,
};
use lingxi_platform_posix_minimal::{PlainTextSecureStorage, PosixClock, PosixHttp};
use lingxi_secret::CredentialManager;
use lingxi_tools::registry::ToolRegistry;
use lingxi_traits::{AuthHandle, OrchestratorHandle, OutputStream};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Bundle of everything `run_cli` needs to drive a conversation.
pub struct Runtime {
    /// The fully-constructed orchestrator.
    pub orchestrator: Arc<ConversationOrchestrator>,
    /// Slash-command dispatcher seeded with the 99 builtins + 18 wired core
    /// handlers (M5-09/M5-10/M5-11).
    pub dispatcher: RegistrySlashDispatcher,
    /// Auth handle for `/login` and `/logout`.
    pub auth: Arc<dyn AuthHandle>,
    /// Output stream the orchestrator pushes turn events to. Held so the
    /// CLI can attach a replacement sink (M5-12 Task 9).
    pub output: Arc<dyn OutputStream>,
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

/// Build the full runtime from parsed argv.
///
/// Currently no `.await` is needed inside the constructor, but the signature
/// remains `async` so future iterations (real OAuth token bootstrap, MCP
/// server connect) can plug in without changing every call site.
#[allow(clippy::unused_async)]
pub async fn build_runtime(argv: &Argv) -> Result<Runtime, InitError> {
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
    let provider = AnthropicProvider::new(api_key, Some(api_base.clone()));
    let api_client: Arc<dyn OrchestratorApiClient> =
        Arc::new(AnthropicProviderAdapter::new(provider, http.clone()));

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

    // (5) Build the orchestrator using test_support fillers for the
    //     hook/permission/memory slots. These are the documented inherited
    //     M5-10/M5-11 gaps — production constructors land in M5-13+.
    let tools = Arc::new(ToolRegistry::new());
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output: Arc<dyn OutputStream> = Arc::new(MockOutputStream::new());
    let memory: Arc<dyn lingxi_orchestrator::prompt::MemoryHierarchyProvider> =
        Arc::new(StaticMemoryProvider::empty());
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let orch = Arc::new(ConversationOrchestrator::new(
        cfg,
        api_client,
        tools,
        hooks,
        perms,
        output.clone(),
        memory,
        cwd,
    ));

    // (6) Build the command registry. The orchestrator implements
    //     `OrchestratorHandle` via M5-10 + M5-11.
    let handle: Arc<dyn OrchestratorHandle> = orch.clone();
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    register_core_batch_1(&mut reg, handle.clone());
    register_core_batch_2(&mut reg, handle, auth.clone());
    let dispatcher = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));

    Ok(Runtime {
        orchestrator: orch,
        dispatcher,
        auth,
        output,
    })
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
        };
        let r = build_runtime(&argv).await;
        assert!(r.is_ok(), "build_runtime failed: {:?}", r.err());
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
