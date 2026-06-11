//! Shared mobile session-host module (plan F3-03).
//!
//! This is the mobile sibling of `engine_desktop::build` (F2-01): the single
//! place that wires an off-device-buildable [`ConversationOrchestrator`] from a
//! deterministic [`MobileConfig`] + an `Arc<dyn Platform>`, binding the
//! transport-agnostic [`client_adapter::AdapterOutputStream`] and the id-keyed
//! [`client_adapter::AdapterPermissionGate`] as its sinks. The same lowering
//! pipeline therefore feeds the mobile [`ClientEventListener`] exactly as it
//! feeds the bridge-server WebSocket — governing decision §0.1 / §0.2.
//!
//! ## Why this lives in `engine-mobile` (not the FFI crates)
//!
//! The FFI packagers (`ios-framework` / `android-aar`) must NOT each re-derive
//! the runtime wiring — that would let iOS and Android drift. Instead they
//! re-export the shared host built here (F3-04 grows `MobileEngineHandle` to own
//! a [`MobileRuntime`]; F3-05 adds the async `submit`). F3-03 only builds the
//! orchestrator + binds the adapter sinks.
//!
//! ## Off-device determinism
//!
//! `build_mobile` reads **nothing** from `std::env` / argv: every input arrives
//! through [`MobileConfig`] and the OS handles arrive through `Arc<dyn
//! Platform>`. On the host (CI) a fake `Platform` shim (see the `tests` module)
//! supplies portable handles so the orchestrator is constructed and the adapter
//! sinks are exercised without a device — exactly the spec §8 "prove from a
//! Swift/Kotlin unit test" smoke path, runnable on the host. The real device
//! `Platform` (`platform-ios` / `platform-android`) is `cfg(target_os)`-gated in
//! `Cargo.toml`, so this module never names a device crate.

use std::collections::HashMap;
use std::sync::Arc;

use anthropic_oauth::client::ClaudeAiOAuthClient;
use anthropic_oauth::config::ClaudeAiOAuthConfig;
use anthropic_oauth::handle::OAuthHandle;
use tool_api::AnthropicRequestBuilder;
use llm_client::{DefaultLlmClient, Transport};
use orchestrator::model::user_agent::UserAgentEnv;
use orchestrator::provider_adapter::SubscriberState;
use platform_common::LlmTransportBridge;
use async_trait::async_trait;
use client_adapter::{
    AdapterOutputStream, AdapterPermissionGate, ClientEventListener, ListenerSink,
    PermissionRequestSink, TurnWrapper,
};
use client_protocol::commands::{ClientCommand, ListingKindDto as ProtocolListingKind};
use client_protocol::error::ClientError;
use client_protocol::events::ClientEvent;
use client_protocol::permission::{
    PermissionKindDto, PermissionRequest as PermissionRequestDto, PermissionResponseDto,
};
use command_api::RegistrySlashDispatcher;
use orchestrator::test_support::{noop_hook_executor, StaticMemoryProvider};
use orchestrator::{
    ConversationOrchestrator, OrchestratorApiClient, OrchestratorConfig, ProviderApiAdapter,
    StreamingApiClient,
};
use permission::gate::PermissionGate;
use permission::PermissionMode;
use sandbox::decision::ProjectTrustLevel;
use sandbox::runtime_config::{Platform as SandboxPlatform, SandboxRuntimeConfig};
use secret::CredentialManager;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tool_api::BuiltinToolContext;
use traits::http::{HttpError, RawByteStream, SseStream, SseStreamWithMeta};
use traits::{
    AuthHandle, HttpTransport, OrchestratorHandle, OutputStream, Platform, SlashCommandDispatcher,
};

use crate::{mobile_command_registry, mobile_tool_registry};

/// A sized newtype over the platform's `Arc<dyn HttpTransport>`.
///
/// [`LlmTransportBridge`] requires a `Sized` `HttpTransport` implementor.
/// Mobile reads its transport from the aggregate `Platform` as an
/// `Arc<dyn HttpTransport>` (unsized), so we wrap it in this thin delegating
/// newtype to satisfy the bound WITHOUT bypassing the device's HTTP backend —
/// every call forwards verbatim to the platform transport.
struct DynHttp(Arc<dyn HttpTransport>);

#[async_trait::async_trait]
impl HttpTransport for DynHttp {
    async fn request(
        &self,
        req: protocol::HttpRequest,
    ) -> Result<protocol::HttpResponse, HttpError> {
        self.0.request(req).await
    }
    async fn stream_sse(&self, req: protocol::HttpRequest) -> Result<SseStream, HttpError> {
        self.0.stream_sse(req).await
    }
    /// Forward to the inner transport so the device backend's real headers are
    /// preserved (the default would silently drop them via the `stream_sse` path).
    async fn stream_sse_with_meta(
        &self,
        req: protocol::HttpRequest,
    ) -> Result<SseStreamWithMeta, HttpError> {
        self.0.stream_sse_with_meta(req).await
    }
    async fn stream_raw_bytes(
        &self,
        req: protocol::HttpRequest,
    ) -> Result<RawByteStream, HttpError> {
        self.0.stream_raw_bytes(req).await
    }
}

/// Deterministic, env/argv-free recipe for building a mobile runtime.
///
/// The mobile analog of [`engine_desktop::DesktopConfig`]: every value the host
/// would otherwise read from the process environment becomes an explicit field,
/// so the FFI entry point (and the off-device host test) can build an identical
/// runtime without touching `std::env`. The OS handles themselves arrive
/// separately, through the `Arc<dyn Platform>` passed to [`build_mobile`].
///
/// Mobile deliberately omits the desktop-only `mcp_paths` and
/// `use_noop_permission_gate` knobs: there is no `.mcp.json` discovery on a
/// device, and a mobile client ALWAYS binds the connection-scoped
/// [`AdapterPermissionGate`] (a phone has no always-allow CLI mode).
#[derive(Clone, Debug)]
pub struct MobileConfig {
    /// API base URL (default `https://api.anthropic.com`).
    pub api_base: String,
    /// Anthropic API key. Empty string is valid — the orchestrator builds and
    /// only fails at `run_turn` with a 401, so slash-command dispatch still
    /// works with no key configured (mirrors the desktop config contract).
    pub api_key: String,
    /// Working directory the orchestrator + tool context are rooted at. On a
    /// device this is the app-sandbox container root.
    pub cwd: std::path::PathBuf,
    /// The `~/.claude`-equivalent root the settings / agents loaders walk. On a
    /// device this is inside the app sandbox.
    pub claude_home: std::path::PathBuf,
    /// Model id the build defaults to (`OrchestratorConfig.model`).
    pub default_model: String,
    /// Settings-declared `providers` block as raw JSON, fed verbatim to
    /// `llm_client::ClientConfig` via `build()`. `None` ⟶ built-in profiles only.
    pub provider_profiles: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    /// Settings-declared `routing` block as raw JSON, fed verbatim to
    /// `llm_client::ClientConfig`. `None` ⟶ the default (empty) routing config.
    pub routing: Option<serde_json::Value>,
}

impl Default for MobileConfig {
    fn default() -> Self {
        Self {
            api_base: "https://api.anthropic.com".to_string(),
            api_key: String::new(),
            cwd: std::path::PathBuf::from("."),
            claude_home: std::path::PathBuf::new(),
            default_model: crate::MobileEngineConfig::default().default_model,
            provider_profiles: None,
            routing: None,
        }
    }
}

/// Everything a mobile host needs to drive a conversation, built deterministically
/// by [`build_mobile`] from a [`MobileConfig`] + an `Arc<dyn Platform>`.
///
/// The mobile analog of `engine_desktop::DesktopRuntime`. F3-04 grows
/// `MobileEngineHandle` to OWN one of these (plus the handle-owned tokio
/// runtime); F3-05 adds the async `submit` that drives the orchestrator and
/// resolves the permission gate.
pub struct MobileRuntime {
    /// The fully-constructed orchestrator, bound to the adapter output stream
    /// and the id-keyed permission gate.
    pub orchestrator: Arc<ConversationOrchestrator>,
    /// Slash-command dispatcher seeded with the builtin + mobile handlers.
    pub dispatcher: RegistrySlashDispatcher,
    /// Auth handle for `/login` and `/logout`.
    pub auth: Arc<dyn AuthHandle>,
    /// The connection-scoped [`AdapterPermissionGate`] handle. Mobile ALWAYS
    /// binds the adapter gate (no always-allow mode), so unlike desktop this is
    /// never `None`: F3-05's `submit(ApprovePermission/DenyPermission)` calls
    /// [`AdapterPermissionGate::resolve`] on it to satisfy a parked `check()`.
    pub permission_gate: Arc<AdapterPermissionGate>,
    /// The registered foreign event listener. Held so F3-04's handle can own /
    /// re-surface it; the adapter already feeds it via a [`ListenerSink`].
    pub listener: Arc<dyn ClientEventListener>,
    /// The connection's [`client_adapter::ClientEventSink`] (a [`ListenerSink`]
    /// over `listener`). The orchestrator's [`AdapterOutputStream`] already pushes
    /// streamed turn events here; F3-05's `submit` reuses the SAME sink to
    /// synthesize boundary events (`TurnStarted` / `MessageComplete`) and emit
    /// listing replies, so everything rides one outbound channel.
    pub event_sink: Arc<dyn client_adapter::ClientEventSink>,
}

/// Errors surfaced while building a [`MobileRuntime`].
///
/// Mirrors `engine_desktop::BuildError`. Construction is effectively infallible
/// today (the orchestrator constructor cannot fail), but the typed error is kept
/// so a future real OAuth bootstrap can surface a cause without changing call
/// sites.
#[derive(Debug, thiserror::Error)]
pub enum MobileBuildError {
    /// api-client construction failed.
    #[error("api base resolution failed: {0}")]
    ApiBase(String),
    /// Orchestrator construction failed.
    #[error("orchestrator construction failed: {0}")]
    Orchestrator(String),
}

/// A [`PermissionRequestSink`] that records `request_id → tool_name` and then
/// forwards each request verbatim to the foreign sink (plan F3-05).
///
/// The mobile analog of bridge-server's `FramePermissionSink`: the inbound
/// `ApprovePermission`/`DenyPermission` command carries only a `request_id`, but
/// [`AdapterPermissionGate::resolve`] needs the tool name to append an
/// `AllowAlways` session rule. This wrapper captures the name as the request goes
/// out, so [`MobileEngineHandle::submit`] can look it back up on resolve. It
/// wraps (not replaces) the foreign listener-backed sink so the host still
/// receives every request.
struct RecordingPermissionSink {
    inner: Arc<dyn PermissionRequestSink>,
    tool_names: Arc<Mutex<HashMap<u64, String>>>,
}

#[async_trait]
impl PermissionRequestSink for RecordingPermissionSink {
    async fn emit_request(&self, request: PermissionRequestDto) {
        // Record the tool name (only `ToolUseConfirm` has one — the reserved
        // kinds are never live-sourced in the foundation, decision §0.6).
        if let PermissionKindDto::ToolUseConfirm { tool_name, .. } = &request.kind {
            self.tool_names
                .lock()
                .await
                .insert(request.request_id, tool_name.clone());
        }
        self.inner.emit_request(request).await;
    }
}

// `builtin_anthropic_config` + `apply_settings_providers` live in
// `platform_common::llm_config` so both composition roots share the same
// model table and settings-wiring logic.
use platform_common::{apply_settings_providers, builtin_anthropic_config, parse_routing_overrides};

/// Build a fully-wired mobile [`MobileRuntime`] from a deterministic
/// [`MobileConfig`] + an `Arc<dyn Platform>` (plan F3-03 — the mobile sibling of
/// `engine_desktop::build`).
///
/// Off-device-deterministic: no `std::env` / argv reads. The OS handles
/// (filesystem / http / clock / process / sandbox / worktree) and the device
/// capabilities (camera / voice / share) are read from `platform`; everything
/// else arrives via `cfg`. The `listener` becomes the adapter's
/// [`client_adapter::ClientEventSink`] (wrapped in a [`ListenerSink`]) so every
/// translated [`client_protocol::events::ClientEvent`] is delivered to the
/// foreign host; `permission_sink` is where the [`AdapterPermissionGate`]'s
/// outbound permission requests go.
///
/// Engine behavior is preserved verbatim (spec §1 non-goal): only the *source*
/// of each input moved from env to `cfg`/`platform`, and the output / permission
/// sinks become connection-scoped parameters — mirroring the F2-01 desktop lift.
///
/// # Errors
///
/// Returns [`MobileBuildError`] if the api-client or orchestrator cannot be
/// constructed (effectively infallible in the current wiring).
pub async fn build_mobile(
    cfg: MobileConfig,
    platform: Arc<dyn Platform>,
    listener: Arc<dyn ClientEventListener>,
    permission_sink: Arc<dyn PermissionRequestSink>,
) -> Result<MobileRuntime, MobileBuildError> {
    build_mobile_inner(cfg, platform, listener, permission_sink, None).await
}

/// As [`build_mobile`], but allows a test to substitute the streaming client.
///
/// Production callers use [`build_mobile`] (`streaming_override == None`), which
/// wires the same router-backed [`ProviderApiAdapter`] for BOTH the batched and
/// the streaming paths — a mobile client always streams its turns. The off-device
/// walking-skeleton test (plan F3-06) passes a scripted
/// [`orchestrator::test_support_stream::MockStreamingApiClient`] here so the turn
/// is deterministic without a network — exactly the bridge-server e2e pattern,
/// which builds the orchestrator with the same mock. Engine behavior is unchanged
/// either way: only the *source* of the stream's bytes differs.
#[doc(hidden)]
// A cohesive composition root: the 8 numbered build steps below read as one
// linear sequence; splitting it only to satisfy the line cap would scatter them.
#[allow(clippy::too_many_lines)]
pub async fn build_mobile_inner(
    cfg: MobileConfig,
    platform: Arc<dyn Platform>,
    listener: Arc<dyn ClientEventListener>,
    permission_sink: Arc<dyn PermissionRequestSink>,
    streaming_override: Option<Arc<dyn StreamingApiClient>>,
) -> Result<MobileRuntime, MobileBuildError> {
    let cwd = cfg.cwd.clone();

    // (1) OS handles from the aggregate `Platform` (NOT a concrete posix type —
    //     the device supplies these; the host test supplies a portable shim).
    let http = platform.http();
    let clock = platform.clock();
    let fs = platform.filesystem();
    let process = platform.process();
    let sandbox = platform.sandbox();
    let worktree = platform.worktree();
    let storage = Arc::new(platform_posix_minimal::PlainTextSecureStorage::new());

    // (2a) Task 10: DefaultLlmClient over LlmTransportBridge.
    //      Mobile uses the platform's `Arc<dyn HttpTransport>` wrapped in `DynHttp`
    //      so the device backend is preserved; no desktop-only deps are pulled.
    //      OAuth is not yet wired on mobile (no credential-manager path exists here);
    //      the API-key path via ANTHROPIC_API_KEY covers the mobile use case.
    //
    //      3c-T2: apply settings `providers` / `routing` on top of the
    //      built-in Anthropic profile (same pattern as engine-desktop).
    let llm_transport: Arc<dyn Transport> =
        Arc::new(LlmTransportBridge::new(DynHttp(http.clone())));
    let (llm_client, routing_overrides, pricing_overrides) = {
        let mut cfg_obj = builtin_anthropic_config(&cfg.api_base, false);
        // Run whenever EITHER key is present: a routing-only settings file
        // (aliases onto builtin models, no custom providers) must still apply.
        if cfg.provider_profiles.is_some() || cfg.routing.is_some() {
            let empty = std::collections::BTreeMap::new();
            let providers = cfg.provider_profiles.as_ref().unwrap_or(&empty);
            if let Err(e) = apply_settings_providers(
                &mut cfg_obj,
                providers,
                cfg.routing.as_ref(),
            ) {
                tracing::warn!(error = %e, "settings providers/routing entry rejected; earlier entries and the built-in profile remain active");
            }
        }
        // Parse routing overrides after providers are applied.
        let routing_overrides = cfg.routing.as_ref().and_then(|r| {
            match parse_routing_overrides(r, &cfg_obj) {
                Ok(o) => Some(o),
                Err(e) => {
                    tracing::warn!(error = %e, "routing.fallback/retry overrides rejected; using defaults");
                    None
                }
            }
        }).unwrap_or_default();
        // Extract per-profile pricing overrides before cfg_obj is consumed.
        let pricing_overrides: Vec<(llm_client::ProviderId, String, llm_client::TokenPricing)> =
            cfg_obj.providers.iter().flat_map(|p| {
                p.pricing.overrides.iter().filter_map(|(model_id, tp)| {
                    p.models.iter()
                        .find(|m| m.display_model == *model_id)
                        .map(|m| (p.provider_id.clone(), m.billing_model.clone(), *tp))
                })
            }).collect();
        let client = Arc::new(
            DefaultLlmClient::from_config(cfg_obj)
                .map_err(|e| MobileBuildError::ApiBase(e.to_string()))?,
        );
        (client, routing_overrides, pricing_overrides)
    };
    let subscriber_state = SubscriberState { is_subscriber: false, is_enterprise: false };

    // 3c-T3: build the cost estimator from the builtin reference catalog.
    // T2: apply per-profile pricing overrides from settings.
    let cost_estimator = {
        use llm_client::{CostEstimator, PricingPolicy};
        use orchestrator::cost_wiring::llm_catalog_from_cost;
        let cost_cat = cost::pricing::PricingCatalog::builtin_reference();
        let mut llm_cat = llm_catalog_from_cost(&cost_cat);
        for (provider_id, billing_model, tp) in &pricing_overrides {
            llm_cat.add_override(provider_id.clone(), billing_model.clone(), *tp);
        }
        Arc::new(CostEstimator::new(llm_cat, PricingPolicy::MarkUnestimated))
    };

    // ONE adapter implements BOTH `OrchestratorApiClient` (batched) and
    // `StreamingApiClient` (the streaming turn path the mobile transport always
    // drives). Production wires it for both paths; a test may substitute the
    // streaming side via `streaming_override` (plan F3-06).
    let provider_adapter = Arc::new(ProviderApiAdapter::new_with_routing(
        llm_client,
        llm_transport,
        subscriber_state,
        UserAgentEnv::from_process_env(),
        env!("CARGO_PKG_VERSION"),
        None,
        None,
        Some(cost_estimator),
        routing_overrides.fallback,
        routing_overrides.max_retries,
        routing_overrides.backoff_ms,
    ));
    let api_client: Arc<dyn OrchestratorApiClient> = provider_adapter.clone();
    let streaming_api: Arc<dyn StreamingApiClient> =
        streaming_override.unwrap_or(provider_adapter as Arc<dyn StreamingApiClient>);
    // WebSearch builds Anthropic `POST /v1/messages` requests via its own
    // provider (server-side web search is Anthropic-only in v1).
    let tool_provider = Arc::new(AnthropicRequestBuilder::new(
        cfg.api_key.clone(),
        Some(cfg.api_base.clone()),
    ));

    // (3) Credential manager + OAuth client (used by /login, /logout).
    let credentials = Arc::new(CredentialManager::new(storage, clock.clone(), http.clone()));
    let oauth_cfg = ClaudeAiOAuthConfig::default_with_port(0);
    let oauth_client = Arc::new(ClaudeAiOAuthClient::new(oauth_cfg, http.clone(), credentials));
    let auth: Arc<dyn AuthHandle> = Arc::new(OAuthHandle::new(oauth_client));

    // (4) Orchestrator config from `cfg` (was a host env/arg read).
    let mut orch_cfg = OrchestratorConfig::default();
    orch_cfg.model.clone_from(&cfg.default_model);

    // (5) Connection-scoped sinks — the mobile transport's analog of the
    //     bridge-server's WS writer:
    //     - the `listener` becomes the `ClientEventSink` (via `ListenerSink`)
    //       the `AdapterOutputStream` pushes turn events to;
    //     - the `permission_sink` receives the gate's outbound requests.
    //     Mobile ALWAYS binds the `AdapterPermissionGate` (no always-allow mode).
    let event_sink = ListenerSink::arc(listener.clone());
    let output: Arc<dyn OutputStream> = Arc::new(AdapterOutputStream::new(event_sink.clone()));

    // (3c) No `.with_persist` on mobile: a device session has no project
    // `.claude/settings.local.json` convention to write back to, so AllowAlways
    // stays session-only here (the desktop transport gate persists; this does not).
    let adapter_gate = Arc::new(AdapterPermissionGate::new(permission_sink));
    let perms: Arc<dyn PermissionGate> = adapter_gate.clone();

    // (6) Hook / memory fillers (empty in the foundation, matching desktop).
    let hooks = noop_hook_executor();
    let memory: Arc<dyn orchestrator::prompt::MemoryHierarchyProvider> =
        Arc::new(StaticMemoryProvider::empty());

    // (7) Assemble the mobile tool registry through the composition root. The
    //     device capabilities (camera / voice / share) come from `platform`;
    //     desktop-only seams (subagent / mcp / lsp / team / worktree-tool) are
    //     absent because `engine-mobile` does not link those tool crates.
    let tool_ctx = BuiltinToolContext {
        // FILE.B: file tools share one read-state map (see engine-desktop note).
        read_file_state: tool_api::read_file_state::new_read_file_state_map(),
        fs,
        bus: Arc::new(telemetry::AnalyticsBus::new()),
        trusted_dirs: vec![cwd.clone()],
        process,
        sandbox,
        clock: clock.clone(),
        sandbox_runtime: SandboxRuntimeConfig::default(),
        sandbox_runner: tool_api::default_sandbox_runner(),
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
        default_model: orch_cfg.model.clone(),
        worktree,
        subagent_spawner: None,
        task_registry: None,
        mailbox_router: None,
        budget_enforcer: None,
        // (3b) No subagent spawner on mobile → AgentTool never builds an
        // invoker, so the dispatch gate is unused here. The main loop is still
        // gated via `perms` (passed to the orchestrator below).
        permission_gate: None,
        mcp_registry: None,
        lsp_registry: None,
        camera: platform.camera(),
        voice: platform.voice(),
        stt: platform.stt(),
        tts: platform.tts(),
        share: platform.share(),
        notifications: platform.notifications(),
        clipboard: platform.clipboard(),
        computer_control: platform.computer_control(),
    };
    let tools = Arc::new(mobile_tool_registry(tool_ctx));

    let orch = Arc::new(ConversationOrchestrator::new_with_streaming(
        orch_cfg,
        api_client,
        streaming_api,
        tools,
        hooks,
        perms,
        output,
        memory,
        cwd,
    ));

    // (8) Command registry through the mobile composition root.
    let handle: Arc<dyn OrchestratorHandle> = orch.clone();
    let reg = mobile_command_registry(handle, auth.clone());
    let dispatcher = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));

    Ok(MobileRuntime {
        orchestrator: orch,
        dispatcher,
        auth,
        permission_gate: adapter_gate,
        listener,
        event_sink,
    })
}

/// Errors surfaced to the foreign (Swift / Kotlin) host across the FFI boundary.
///
/// This is the SINGLE shared error vocabulary both FFI packager crates re-export
/// (F3-04), folded from the legacy per-crate `MobileEngineError`: keeping it in
/// the shared host crate is what prevents iOS and Android from drifting. Under
/// the `uniffi` feature (F3-01) this becomes `#[derive(uniffi::Error)]`-able; it
/// is intentionally flat (no embedded engine types) so it marshals across the
/// boundary unchanged.
#[derive(Debug, Clone, thiserror::Error)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
pub enum MobileEngineError {
    /// The requested session id is not registered with this engine handle.
    #[error("session not found")]
    NotFound,
    /// The engine is in a state that does not allow the requested operation.
    #[error("invalid state")]
    InvalidState,
    /// `build_mobile_engine` was called off-device. The real device `Platform`
    /// (`platform-ios` / `platform-android`) is `cfg(target_os)`-gated, so the
    /// FFI constructor returns this on the host — but the shared session host
    /// itself is fully exercised off-device via the test shim.
    #[error("platform unavailable on this target")]
    PlatformUnavailable,
    /// Catch-all for engine-internal failures (the message is log-safe).
    #[error("internal: {0}")]
    Internal(String),
}

/// The real mobile session host (plan F3-04): the opaque handle the foreign
/// (Swift / Kotlin) side holds for the lifetime of one engine connection.
///
/// This is the grown-up form of the M8 stub (which held only an `Arc<dyn
/// Platform>` + a `skill_count` and whose `create_session` returned an
/// `Internal` error). It now OWNS, per governing decision §0.5 (one connection ⇒
/// one engine host):
///
/// - the **handle-owned tokio runtime** (`rt-multi-thread`) every turn / FFI
///   `submit` (F3-05) is driven on — so the engine never blocks the foreign UI
///   thread, and F3-07 registers this same runtime as `UniFFI`'s foreign async
///   executor;
/// - the fully-wired [`MobileRuntime`] from [`build_mobile`] (F3-03): the
///   orchestrator bound to the [`AdapterOutputStream`] + the id-keyed
///   [`AdapterPermissionGate`], the slash dispatcher, the auth handle;
/// - the registered foreign [`ClientEventListener`] (re-surfaced via
///   [`MobileRuntime::listener`]) that the adapter feeds every translated
///   [`client_protocol::events::ClientEvent`].
///
/// Both FFI packager crates (`ios-framework` / `android-aar`) RE-EXPORT this
/// shared host rather than each re-deriving it — that is what keeps iOS and
/// Android from drifting (plan F3-04). Under the `uniffi` feature this becomes
/// `#[derive(uniffi::Object)]`.
///
/// The connection-scoped adapter sinks (output stream / permission gate /
/// listener) survive an in-place orchestrator swap on New / Resume (§0.5); F3-05
/// adds the async `submit` that resolves the parked permission gate from inbound
/// commands and drives the turn on the owned runtime.
#[cfg_attr(feature = "uniffi", derive(uniffi::Object))]
pub struct MobileEngineHandle {
    /// The handle-owned multi-thread tokio runtime. Owned (not borrowed) so the
    /// engine outlives any single FFI call and F3-05's `submit(SendPrompt)` can
    /// `spawn` a streaming turn that returns promptly while results stream to the
    /// listener. F3-07 registers this runtime as the foreign async executor.
    runtime: tokio::runtime::Runtime,
    /// The fully-wired mobile runtime: orchestrator + dispatcher + auth + the
    /// connection-scoped [`AdapterPermissionGate`] + the registered listener.
    inner: MobileRuntime,
    /// The connection's event sink (a [`ListenerSink`] over the registered
    /// listener). Held so [`Self::submit`] can synthesize boundary events
    /// (`TurnStarted` / `MessageComplete`) and push listing replies to the SAME
    /// outbound channel the streamed turn events ride.
    event_sink: Arc<dyn client_adapter::ClientEventSink>,
    /// The cancellation token for the IN-FLIGHT turn, armed by
    /// `submit(SendPrompt)` and fired by `submit(Cancel)`. `None` when no turn is
    /// active. One connection ⇒ one in-flight turn (§0.5), so a single slot.
    active_cancel: Arc<Mutex<Option<CancellationToken>>>,
    /// `request_id → tool_name` recorded as each permission request is emitted, so
    /// an inbound `ApprovePermission`/`DenyPermission` (which carries only the
    /// `request_id`) can supply the tool name back to
    /// [`AdapterPermissionGate::resolve`] (needed for the `AllowAlways` rule
    /// append). The mobile analog of bridge-server's `FramePermissionSink` map.
    tool_names: Arc<Mutex<HashMap<u64, String>>>,
    /// Number of builtin mobile skills assembled (the M8 smoke signal, retained
    /// so the existing Swift/Kotlin smoke test keeps working).
    skill_count: usize,
    /// The `~/.claude`-equivalent root the session enumerator walks
    /// (`<claude_home>/projects/<sanitized cwd>/*.jsonl`). Captured from the
    /// `MobileConfig` so `submit(ListSessions)` can read the on-disk catalog
    /// without re-deriving it (SESSIONS/HISTORY).
    claude_home: std::path::PathBuf,
    /// The session enumerator's `cwd` key (its sanitized form selects the project
    /// subdir under `claude_home/projects/`). Captured from the `MobileConfig`.
    session_cwd: String,
    /// The platform filesystem handle the JSONL reader reads each session file
    /// through (`list_recent_sessions`' `Arc<dyn FileSystem>` argument). The SAME
    /// `fs` the orchestrator's tools use — captured from the `Platform` so the
    /// session listing reads through the device's real backend.
    fs: Arc<dyn traits::FileSystem>,
}

/// Default `ListSessions` row cap when the command omits an explicit `limit`
/// (SESSIONS/HISTORY). Mirrors the CLI `/resume` default (`apps/cli/src/run.rs`
/// passes `5`).
const DEFAULT_SESSION_LIST_LIMIT: usize = 5;

impl MobileEngineHandle {
    /// Number of builtin mobile skills assembled. (Under `uniffi`:
    /// `#[uniffi::export]`.)
    #[must_use]
    pub fn skill_count(&self) -> u32 {
        u32::try_from(self.skill_count).unwrap_or(u32::MAX)
    }

    /// Create a conversation session for `model`.
    ///
    /// No longer stubbed (F3-04): the handle now owns a real, fully-wired
    /// [`MobileRuntime`], so a session is a live attribute of THIS connection
    /// (§0.5 — `session_id` is a connection attribute, not a per-command param).
    /// Returns the connection's session ref. The full New/Resume orchestrator
    /// swap lands with the command path (F3-05); here we confirm the host is no
    /// longer a stub by returning the live session ref instead of an error.
    ///
    /// # Errors
    ///
    /// Returns [`MobileEngineError::InvalidState`] only if the handle were ever
    /// torn down mid-call (cannot happen with `&self`); kept typed so the FFI
    /// signature is stable for the F3-05 command path.
    // `_model` is a deliberately-unused, by-value FFI-shape placeholder (see the
    // doc above): the F3-05 command path keeps the owned `String` signature, so
    // it is not narrowed to `&str` just to satisfy the lint.
    #[allow(clippy::needless_pass_by_value)]
    pub fn create_session(&self, _model: String) -> Result<u64, MobileEngineError> {
        // One connection ⇒ one engine host ⇒ a single session ref (1). The
        // orchestrator is already constructed and bound; New/Resume swap it in
        // place (F3-05) without minting a new handle.
        Ok(1)
    }

    /// Borrow the owned tokio runtime (F3-05 spawns the streaming turn on it;
    /// F3-07 registers it as the foreign async executor).
    #[must_use]
    pub fn runtime(&self) -> &tokio::runtime::Runtime {
        &self.runtime
    }

    /// The identity of the handle-owned tokio runtime (F3-07), as a stable
    /// string token.
    ///
    /// UniFFI's `#[uniffi::export(async_runtime = "tokio")]` drives every async
    /// export (`submit`, the async inspection helpers) on a tokio runtime via the
    /// `tokio` feature's foreign-executor scaffolding; the host registers THIS
    /// owned `rt-multi-thread` runtime as that executor (decision §0.5 — one
    /// connection ⇒ one engine host owning one runtime). The
    /// `async_submit_resolves_on_handle_runtime` test compares this token against
    /// the one an async export observes via [`Self::observed_runtime_id`] to PROVE
    /// the export awaits on this runtime — not a transient ambient one.
    /// (`tokio::runtime::Id` is not UniFFI-representable, so the token is its
    /// `Debug` form, which is stable for the lifetime of the runtime.)
    #[doc(hidden)]
    #[must_use]
    pub fn runtime_id(&self) -> String {
        format!("{:?}", self.runtime.handle().id())
    }

    /// Borrow the wired [`MobileRuntime`] (orchestrator / dispatcher / auth /
    /// gate / listener) the F3-05 command path drives.
    #[must_use]
    pub fn inner(&self) -> &MobileRuntime {
        &self.inner
    }

    /// The connection-scoped [`AdapterPermissionGate`] — F3-05's
    /// `submit(ApprovePermission/DenyPermission)` calls `resolve` on it to
    /// satisfy a parked `check()`.
    #[must_use]
    pub fn permission_gate(&self) -> Arc<AdapterPermissionGate> {
        self.inner.permission_gate.clone()
    }

    /// The registered foreign event listener the adapter feeds.
    #[must_use]
    pub fn listener(&self) -> Arc<dyn ClientEventListener> {
        self.inner.listener.clone()
    }

    /// Test/inspection helper: `true` iff the in-flight turn's cancellation token
    /// has been fired. Used by the F3-05 `submit_cancel_fires_token` test.
    #[doc(hidden)]
    pub async fn active_turn_is_cancelled(&self) -> bool {
        self.active_cancel
            .lock()
            .await
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    }
}

// F3-05: the inbound command path — the async FFI entry point. Under the
// `uniffi` feature this impl block is a `#[uniffi::export(async_runtime =
// "tokio")]` so `submit` is exported as an async foreign method that resolves on
// the handle-owned tokio runtime (the runtime F3-07 registers as the foreign
// executor). Plain (non-FFI) on the host build so `cargo test` exercises the
// SAME method body.
#[cfg_attr(feature = "uniffi", uniffi::export(async_runtime = "tokio"))]
impl MobileEngineHandle {
    /// Submit one [`ClientCommand`] to the engine (plan F3-05).
    ///
    /// This is the mobile analog of the bridge-server's inbound frame dispatch
    /// (`apps/bridge-server/src/server.rs`): it never blocks the foreign UI
    /// thread for a whole turn. `SendPrompt` SPAWNS the streaming turn on the
    /// handle-owned runtime and returns promptly (results stream via the
    /// listener); the other commands resolve their engine entry and return.
    ///
    /// Command routing (decision §0.2 — the same lowering surface both transports
    /// share):
    /// - `SendPrompt` → arm a fresh [`CancellationToken`], synthesize
    ///   `TurnStarted`, SPAWN `run_turn_streaming_with_cancel` on the owned
    ///   runtime, return immediately. The orchestrator's [`AdapterOutputStream`]
    ///   streams `TextDelta` / `ToolUse*` / `CostUpdate` / `TurnEnded` to the
    ///   listener as side effects. Inline `images` are DEFERRED on mobile
    ///   (§0.8 / §5.12) — carried in the DTO, not fed to the engine.
    /// - `Cancel` → fire the in-flight cancellation token.
    /// - `ApprovePermission` / `DenyPermission` → resolve the parked oneshot on
    ///   the connection-scoped [`AdapterPermissionGate`] (F1-14), looking the
    ///   recorded tool name back up for an `AllowAlways` rule append.
    /// - `SetModel` → [`OrchestratorHandle::switch_model`], confirmed by a
    ///   `ModelChanged` event.
    /// - `RunSlashCommand` → the mobile slash dispatcher (LOSSY display surfaced
    ///   as a `TextDelta`, mirroring the bridge-server router).
    /// - `RefreshListings` / `ListModels` → the `list_*` handle reads, lowered to
    ///   their listing events through the shared `client_adapter::lowering` fns.
    /// - `ForceCompact` / `ClearSession` / `RequestExit` / `Login` / `Logout` →
    ///   their `OrchestratorHandle` / `AuthHandle` entries.
    ///
    /// - `ListSessions` → enumerate the on-disk JSONL catalog via
    ///   `session::jsonl::list_recent_sessions`, lower each row through the shared
    ///   `client_adapter::lower_session_metadata`, reply with `SessionList`.
    /// - `NewSession` → `clear_session` (mints a fresh `SessionId`) + optional
    ///   `switch_model`, confirmed by `SessionStarted` (SESSIONS/HISTORY).
    /// - `ResumeSession` → LIVE hot-restore (SESSIONS/HISTORY): reject mid-turn,
    ///   parse the `session_id` as a `Uuid`, load + validate the on-disk JSONL via
    ///   `orchestrator::replay_session_state`, adopt it into the running
    ///   orchestrator with `OrchestratorHandle::resume_session`, and confirm with a
    ///   `SessionResumed { session_id, messages }` carrying the full restored
    ///   transcript (lowered via `client_adapter::lowering::lower_transcript`). A
    ///   missing / corrupt / malformed session is honestly `Rejected` — we never
    ///   emit a false `SessionResumed`.
    ///
    /// Remaining host-driven / reserved commands (the task commands — mobile binds
    /// no `TaskRegistry`) are accepted and no-op'd (the `#[non_exhaustive]` enum
    /// also requires a catch-all); lighting them up is additive and does not
    /// change this seam.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when a command's engine entry fails synchronously
    /// (e.g. `SetModel` on an unknown model → [`ClientError::Internal`]). A turn
    /// failure is NOT surfaced here — it streams as a `ClientEvent::Error` to the
    /// listener (the turn is spawned, so `submit` has already returned `Ok`).
    // One match over the full command surface — the one-place-routes-everything
    // map this entry exists to be (same convention as `EngineCommandRouter::route`
    // and `engine_desktop::build`).
    #[allow(clippy::too_many_lines)]
    pub async fn submit(&self, command: ClientCommand) -> Result<(), ClientError> {
        match command {
            // ── Turn driving (SPAWN + return promptly) ─────────────────────
            ClientCommand::SendPrompt {
                text, turn_id, ..
            } => {
                // Arm a fresh cancellation token for this turn and record it so
                // a later `Cancel` can fire it (one in-flight turn per
                // connection, §0.5).
                let cancel = CancellationToken::new();
                *self.active_cancel.lock().await = Some(cancel.clone());

                // Synthesize `TurnStarted` (the engine never emits it) on the
                // shared event sink BEFORE spawning, so it precedes the streamed
                // turn events.
                let wrapper = TurnWrapper::new(self.event_sink.clone());
                wrapper.emit_turn_started(turn_id).await;

                // Spawn the streaming turn on the handle-owned runtime so this
                // FFI call returns promptly. The `AdapterOutputStream` streams
                // every turn event (`TextDelta` / `ToolUse*` / `CostUpdate` /
                // `TurnEnded`) to the listener as a side effect; on a turn
                // `Err(OrchestratorError)` we push the lowered `Error` event so
                // the foreign host learns the turn failed (the cancelable entry
                // returns `TurnOutcome`, not the `PumpedTurn` blocks, so there is
                // no `MessageComplete` to synthesize here — `TurnEnded` is the
                // boundary, matching the bridge-server turn driver).
                let orch = self.inner.orchestrator.clone();
                let sink = self.event_sink.clone();
                self.runtime.spawn(async move {
                    if let Err(err) = orch.run_turn_streaming_with_cancel(&text, cancel).await {
                        sink.emit(client_adapter::map_orchestrator_error(&err)).await;
                    }
                });
                Ok(())
            }

            ClientCommand::Cancel { .. } => {
                if let Some(token) = self.active_cancel.lock().await.as_ref() {
                    token.cancel();
                }
                Ok(())
            }

            // ── Permission resolution (resolve the parked oneshot, F1-14) ───
            ClientCommand::ApprovePermission {
                request_id,
                response,
            } => {
                self.resolve_permission(request_id, response).await;
                Ok(())
            }
            ClientCommand::DenyPermission { request_id } => {
                self.resolve_permission(request_id, PermissionResponseDto::Deny)
                    .await;
                Ok(())
            }

            // ── Model ──────────────────────────────────────────────────────
            ClientCommand::SetModel { model } => {
                let handle: Arc<dyn OrchestratorHandle> = self.inner.orchestrator.clone();
                handle.switch_model(&model).await.map_err(|e| {
                    ClientError::Internal {
                        message: format!("switch_model failed: {e}"),
                    }
                })?;
                self.event_sink
                    .emit(ClientEvent::ModelChanged { model })
                    .await;
                Ok(())
            }
            ClientCommand::ListModels => {
                self.emit_listing(ProtocolListingKind::Models).await;
                Ok(())
            }

            // ── Slash commands (LOSSY: display surfaced as a TextDelta) ─────
            ClientCommand::RunSlashCommand { raw } => {
                let display = match self.inner.dispatcher.dispatch(&raw).await {
                    traits::SlashDispatchResult::Handled { display }
                    | traits::SlashDispatchResult::Unknown { display, .. } => display,
                    traits::SlashDispatchResult::NotASlashCommand => {
                        format!("not a slash command: {raw}")
                    }
                };
                self.event_sink
                    .emit(ClientEvent::TextDelta { text: display })
                    .await;
                Ok(())
            }

            // ── Listings ────────────────────────────────────────────────────
            ClientCommand::RefreshListings { which } => {
                for kind in which {
                    self.emit_listing(kind).await;
                }
                Ok(())
            }

            // ── Auth ─────────────────────────────────────────────────────────
            ClientCommand::Login => {
                let state = match self.inner.auth.login().await {
                    Ok(li) => lower_auth_state(Some(li)),
                    Err(e) => {
                        self.event_sink
                            .emit(ClientEvent::Error {
                                kind: client_protocol::events::ErrorKindDto::Internal,
                                message: format!("login failed: {e}"),
                            })
                            .await;
                        lower_auth_state(self.inner.auth.current_user().await)
                    }
                };
                self.event_sink
                    .emit(ClientEvent::AuthState { state })
                    .await;
                Ok(())
            }
            ClientCommand::Logout => {
                if let Err(e) = self.inner.auth.logout().await {
                    self.event_sink
                        .emit(ClientEvent::Error {
                            kind: client_protocol::events::ErrorKindDto::Internal,
                            message: format!("logout failed: {e}"),
                        })
                        .await;
                }
                self.event_sink
                    .emit(ClientEvent::AuthState {
                        state: lower_auth_state(self.inner.auth.current_user().await),
                    })
                    .await;
                Ok(())
            }

            // ── Compaction ─────────────────────────────────────────────────
            ClientCommand::ForceCompact => {
                let handle: Arc<dyn OrchestratorHandle> = self.inner.orchestrator.clone();
                match handle.force_compact().await {
                    Ok(summary) => {
                        self.event_sink
                            .emit(ClientEvent::CompactionCompleted {
                                messages_before: summary.messages_before,
                                messages_after: summary.messages_after,
                                bytes_saved: summary.bytes_saved,
                            })
                            .await;
                        Ok(())
                    }
                    Err(e) => Err(ClientError::Internal {
                        message: format!("force_compact failed: {e}"),
                    }),
                }
            }

            // ── Session control ──────────────────────────────────────────────
            ClientCommand::ClearSession => {
                // Mid-turn semantics (plan §2): reject while a turn is in flight.
                let mid_turn = self
                    .active_cancel
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(|t| !t.is_cancelled());
                if mid_turn {
                    return Err(ClientError::Rejected {
                        message: "cannot clear the session while a turn is in flight".into(),
                    });
                }
                let handle: Arc<dyn OrchestratorHandle> = self.inner.orchestrator.clone();
                handle.clear_session().await.map_err(|e| ClientError::Internal {
                    message: format!("clear_session failed: {e}"),
                })?;
                self.event_sink.emit(ClientEvent::SessionEnded).await;
                Ok(())
            }
            ClientCommand::RequestExit => {
                let handle: Arc<dyn OrchestratorHandle> = self.inner.orchestrator.clone();
                handle.request_exit().await;
                Ok(())
            }

            // ── Sessions / history (SESSIONS/HISTORY) ────────────────────────
            //
            // `ListSessions` enumerates the on-disk JSONL catalog
            // (`<claude_home>/projects/<sanitized cwd>/*.jsonl`) via the shared
            // `session::jsonl::list_recent_sessions`, lowers each row through the
            // shared `client_adapter::lower_session_metadata`, and replies with a
            // `SessionList` event — the same listing surface the bridge-server
            // router uses (decision §0.2). An empty / missing catalog replies with
            // an empty list (the loader's `EmptyDirectory` is not an error here —
            // it is "no resumable sessions yet").
            ClientCommand::ListSessions { limit } => {
                let limit = limit
                    .map_or(DEFAULT_SESSION_LIST_LIMIT, |l| l as usize);
                self.emit_session_list(limit).await;
                Ok(())
            }

            // `NewSession` swaps the connection's orchestrator to a fresh session
            // (decision §0.5 — `session_id` is a connection attribute). The
            // orchestrator handle's `clear_session` mints a brand-new `SessionId`
            // and resets the in-memory history + JSONL parent chain; we then read
            // the new id back and confirm with `SessionStarted`. An optional
            // `model` override is applied via `switch_model` (the only New-session
            // knob the live orchestrator can honor); a `cwd` override is NOT
            // honored — the orchestrator is rooted at construction, so a true cwd
            // re-root would need a fresh build (DEFERRED, out of scope here).
            ClientCommand::NewSession { cwd: _, model } => {
                // Reject mid-turn (same contract as `ClearSession`): a new session
                // must not race an in-flight turn.
                let mid_turn = self
                    .active_cancel
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(|t| !t.is_cancelled());
                if mid_turn {
                    return Err(ClientError::Rejected {
                        message: "cannot start a new session while a turn is in flight".into(),
                    });
                }
                let handle: Arc<dyn OrchestratorHandle> = self.inner.orchestrator.clone();
                handle.clear_session().await.map_err(|e| ClientError::Internal {
                    message: format!("new session (clear_session) failed: {e}"),
                })?;
                if let Some(model) = model {
                    handle.switch_model(&model).await.map_err(|e| {
                        ClientError::Internal {
                            message: format!("new session model switch failed: {e}"),
                        }
                    })?;
                }
                let session_id = handle.current_session_id().await.to_string();
                self.event_sink
                    .emit(ClientEvent::SessionStarted { session_id })
                    .await;
                Ok(())
            }

            // `ResumeSession` names a prior session to hot-restore onto the live
            // orchestrator (SESSIONS/HISTORY). The orchestrator now exposes a real
            // rehydrate seam (`OrchestratorHandle::resume_session`, the symmetric
            // twin of `clear_session`): we load + validate the on-disk JSONL, adopt
            // it into the RUNNING orchestrator IN PLACE (named id + replayed
            // history + JSONL parent-uuid chain pointer), and emit a
            // `SessionResumed` carrying the full restored transcript so the client
            // renders the rehydrated conversation atomically. A missing / corrupt /
            // malformed session is honestly `Rejected` — a session that is not
            // resumable is genuinely not resumable, so we reject rather than emit a
            // FALSE `SessionResumed`.
            ClientCommand::ResumeSession { session_id, cwd } => {
                // (a) Reject mid-turn (same contract as `ClearSession` /
                // `NewSession`): a resume must not race an in-flight turn.
                let mid_turn = self
                    .active_cancel
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(|t| !t.is_cancelled());
                if mid_turn {
                    return Err(ClientError::Rejected {
                        message: "cannot resume while a turn is in flight".into(),
                    });
                }

                // (b) Parse the named session id as a `Uuid`. A malformed id is
                // honestly rejected (not faked) — there is no session to adopt.
                let uuid = match uuid::Uuid::parse_str(&session_id) {
                    Ok(u) => u,
                    Err(e) => {
                        return Err(ClientError::Rejected {
                            message: format!("resume: malformed session id {session_id:?}: {e}"),
                        });
                    }
                };

                // (c) cwd: use the command's override if Some, else the
                // connection's session cwd (the project-dir key the loader walks).
                let cwd = cwd.unwrap_or_else(|| self.session_cwd.clone());

                // (d) Load + validate the on-disk JSONL. A SessionNotFound /
                // ChainBroken / SessionIdMismatch / Io / Parse / EmptyDirectory is
                // genuinely not resumable — reject carrying the loader's message
                // (HONEST: we never emit a false SessionResumed for a missing /
                // corrupt session).
                let replayed = match orchestrator::replay_session_state(
                    &self.claude_home,
                    &cwd,
                    uuid,
                    self.fs.clone(),
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        return Err(ClientError::Rejected {
                            message: format!("resume: session {session_id} not resumable: {e}"),
                        });
                    }
                };

                // (e) Adopt the replayed session INTO the running orchestrator
                // (named id + history + JSONL chain pointer), then confirm with a
                // `SessionResumed` carrying the full restored transcript.
                let handle: Arc<dyn OrchestratorHandle> = self.inner.orchestrator.clone();
                handle
                    .resume_session(
                        protocol::SessionId::from_uuid(uuid),
                        replayed.state.history.clone(),
                        replayed.last_message_uuid.map(|u| u.to_string()),
                    )
                    .await
                    .map_err(|e| ClientError::Internal {
                        message: format!("resume_session failed: {e}"),
                    })?;

                let messages = client_adapter::lowering::lower_transcript(&replayed.state.history);
                self.event_sink
                    .emit(ClientEvent::SessionResumed {
                        session_id: uuid.to_string(),
                        messages,
                    })
                    .await;
                Ok(())
            }

            // ── Host-driven / reserved in the foundation ────────────────────
            //
            // The task commands have no engine handle on mobile (`build_mobile`
            // binds `task_registry: None`). They are accepted and no-op'd here —
            // lighting them up is additive and does not change this seam's shape.
            // The `#[non_exhaustive]` enum also requires a catch-all.
            other => {
                tracing::debug!(?other, "engine-mobile: command not routed by submit in the foundation");
                Ok(())
            }
        }
    }

    /// The tokio runtime [`tokio::runtime::Id`] (as a string token) this async
    /// export RESOLVES ON (plan F3-07).
    ///
    /// This is a genuine async `#[uniffi::export(async_runtime = "tokio")]`
    /// method, so polling it travels the EXACT foreign-executor path `submit`
    /// does: `UniFFI` hands the returned future to the registered tokio runtime —
    /// the handle-owned `rt-multi-thread` runtime — which polls it to completion.
    /// It reads `tokio::runtime::Handle::current()` (which panics outside a tokio
    /// context) and returns that runtime's id token. The
    /// `async_submit_resolves_on_handle_runtime` test asserts the returned token
    /// equals [`Self::runtime_id`], proving the async export awaits on the
    /// handle-owned runtime rather than a transient ambient one — i.e. that the
    /// foreign async executor is registered to THIS runtime.
    pub async fn observed_runtime_id(&self) -> String {
        // `Handle::current()` resolves the runtime the polling thread is bound to.
        // Yield once so the value is read AFTER an actual await point — the future
        // is genuinely driven by the executor, not resolved eagerly on the caller.
        tokio::task::yield_now().await;
        format!("{:?}", tokio::runtime::Handle::current().id())
    }
}

impl MobileEngineHandle {
    /// Resolve a parked permission request on the connection-scoped gate (the
    /// inbound side of the inverted handshake). Looks the recorded tool name back
    /// up so an `AllowAlways` can append the right session rule.
    async fn resolve_permission(&self, request_id: u64, response: PermissionResponseDto) {
        let tool_name = self
            .tool_names
            .lock()
            .await
            .remove(&request_id)
            .unwrap_or_default();
        let resolved = self
            .inner
            .permission_gate
            .resolve(request_id, response, &tool_name)
            .await;
        if !resolved {
            tracing::debug!(
                request_id,
                "engine-mobile: resolve for unknown / already-resolved permission id"
            );
        }
    }

    /// Enumerate the on-disk resumable-session catalog and emit a `SessionList`
    /// event (SESSIONS/HISTORY).
    ///
    /// Reads `<claude_home>/projects/<sanitized cwd>/*.jsonl` via the shared
    /// `session::jsonl::list_recent_sessions` (the SAME enumerator the CLI
    /// `/resume` picker uses), capped at `limit`, then lowers each
    /// `SessionMetadata` row through the shared
    /// `client_adapter::lower_session_metadata` (decision §0.2). A missing /
    /// empty catalog (`LoaderError::EmptyDirectory`) is NOT an error here — it
    /// replies with an empty list ("no resumable sessions yet"); a real I/O
    /// failure is logged and also yields an empty list so the client always gets
    /// a reply.
    async fn emit_session_list(&self, limit: usize) {
        use session::jsonl::list_recent_sessions;
        let sessions = match list_recent_sessions(
            &self.claude_home,
            &self.session_cwd,
            limit,
            self.fs.clone(),
        )
        .await
        {
            Ok(rows) => rows
                .iter()
                .map(client_adapter::lowering::lower_session_metadata)
                .collect(),
            Err(session::jsonl::LoaderError::EmptyDirectory) => Vec::new(),
            Err(e) => {
                tracing::debug!(error = %e, "engine-mobile: list_recent_sessions failed; replying empty");
                Vec::new()
            }
        };
        self.event_sink
            .emit(ClientEvent::SessionList { sessions })
            .await;
    }

    /// Pull a single listing kind and emit its listing event through the
    /// connection's event sink, reusing the shared `client_adapter::lowering`
    /// parity fns (decision §0.2). Listing kinds with no engine handle on mobile
    /// (`Sessions` / `Memory` / `Settings` / `SlashCommands` / `Tasks`) are
    /// skipped — the same foundation reality as the bridge-server router.
    async fn emit_listing(&self, kind: ProtocolListingKind) {
        use client_adapter::lowering::{
            lower_agent_info, lower_doctor_report, lower_hook_info, lower_mcp_server_info,
            lower_status_snapshot,
        };
        let handle: Arc<dyn OrchestratorHandle> = self.inner.orchestrator.clone();
        match kind {
            ProtocolListingKind::Models => {
                let models = handle.list_available_models().await;
                let current = handle.get_status_snapshot().await.model;
                self.event_sink
                    .emit(ClientEvent::ModelList { models, current })
                    .await;
            }
            ProtocolListingKind::Mcp => {
                let servers = handle
                    .list_mcp_servers()
                    .await
                    .iter()
                    .map(lower_mcp_server_info)
                    .collect();
                self.event_sink
                    .emit(ClientEvent::McpServers { servers })
                    .await;
            }
            ProtocolListingKind::Hooks => {
                let hooks = handle.list_hooks().await.iter().map(lower_hook_info).collect();
                self.event_sink.emit(ClientEvent::Hooks { hooks }).await;
            }
            ProtocolListingKind::Agents => {
                let agents = handle.list_agents().await.iter().map(lower_agent_info).collect();
                self.event_sink.emit(ClientEvent::Agents { agents }).await;
            }
            ProtocolListingKind::Status => {
                let snapshot = lower_status_snapshot(&handle.get_status_snapshot().await);
                self.event_sink
                    .emit(ClientEvent::StatusSnapshot { snapshot })
                    .await;
            }
            ProtocolListingKind::Doctor => {
                let report = lower_doctor_report(&handle.run_doctor_checks().await);
                self.event_sink
                    .emit(ClientEvent::DoctorReport { report })
                    .await;
            }
            ProtocolListingKind::Auth => {
                let state = lower_auth_state(self.inner.auth.current_user().await);
                self.event_sink.emit(ClientEvent::AuthState { state }).await;
            }
            // No engine handle on mobile in the foundation — additive to wire.
            _ => {
                tracing::debug!(?kind, "engine-mobile: listing kind unhandled in the foundation");
            }
        }
    }
}

/// Lower an `Option<LoginInfo>` to the auth-state DTO (the inverse copy of the
/// bridge-server router's helper — kept private to the shared host so iOS /
/// Android cannot drift).
fn lower_auth_state(info: Option<traits::auth::LoginInfo>) -> client_protocol::listings::AuthStateDto {
    match info {
        Some(li) => client_protocol::listings::AuthStateDto::SignedIn {
            email: li.email,
            org_id: li.org_id,
        },
        None => client_protocol::listings::AuthStateDto::SignedOut,
    }
}

/// Build the shared mobile session host (plan F3-04): construct the
/// handle-owned tokio runtime, build the [`MobileRuntime`] on it, and return the
/// opaque [`MobileEngineHandle`] both FFI crates re-export.
///
/// The FFI packager crates (`ios-framework` / `android-aar`) call THIS after
/// constructing the device `Platform` from the foreign callbacks — so the
/// runtime / adapter / listener wiring lives in exactly one place and iOS /
/// Android cannot drift. `listener` is the foreign [`ClientEventListener`] the
/// host registers; it is stored on the runtime and fed by the adapter.
/// `permission_sink` is where the gate's outbound permission requests go (on
/// mobile, also the listener's transport).
///
/// Off-device-deterministic: the heavy lifting is [`build_mobile`], which reads
/// nothing from `std::env`. The owned runtime is a fresh `rt-multi-thread`
/// runtime; `build_mobile` (async) is driven to completion on it via
/// `block_on`, after which it owns the orchestrator's spawned work.
///
/// # Errors
///
/// Returns [`MobileEngineError::Internal`] if the tokio runtime cannot be built
/// or [`build_mobile`] fails (effectively infallible in the current wiring).
pub fn build_mobile_engine(
    cfg: MobileConfig,
    platform: Arc<dyn Platform>,
    listener: Arc<dyn ClientEventListener>,
    permission_sink: Arc<dyn PermissionRequestSink>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    build_mobile_engine_inner(cfg, platform, listener, permission_sink, None)
}

/// As [`build_mobile_engine`], but allows a test to substitute the streaming
/// client (plan F3-06 — the off-device walking skeleton). Production callers use
/// [`build_mobile_engine`] (`streaming_override == None`); the host skeleton test
/// passes a scripted
/// [`orchestrator::test_support_stream::MockStreamingApiClient`] so
/// `submit(SendPrompt)` drives a deterministic turn without a network. The whole
/// session-host wiring (the runtime, the recording permission sink, the adapter
/// sinks) is identical to production — only the stream's source differs.
#[doc(hidden)]
pub fn build_mobile_engine_inner(
    cfg: MobileConfig,
    platform: Arc<dyn Platform>,
    listener: Arc<dyn ClientEventListener>,
    permission_sink: Arc<dyn PermissionRequestSink>,
    streaming_override: Option<Arc<dyn StreamingApiClient>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    // The handle OWNS its runtime (§0.5). A multi-thread runtime so a streaming
    // turn spawned by F3-05 runs concurrently with the FFI read path.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| MobileEngineError::Internal(format!("tokio runtime build failed: {e}")))?;

    // F3-05: interpose a recording sink so the inbound `ApprovePermission` /
    // `DenyPermission` command path (which carries only the `request_id`) can
    // recover the tool name for an `AllowAlways` rule append. The foreign sink
    // still receives every request — the recorder wraps, never replaces it.
    let tool_names: Arc<Mutex<HashMap<u64, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let recording_sink: Arc<dyn PermissionRequestSink> = Arc::new(RecordingPermissionSink {
        inner: permission_sink,
        tool_names: tool_names.clone(),
    });

    // SESSIONS/HISTORY: capture the session-enumerator inputs BEFORE `cfg` /
    // `platform` are moved into `build_mobile_inner`. `submit(ListSessions)`
    // reads the on-disk catalog with these (the SAME `fs` the tools use).
    let claude_home = cfg.claude_home.clone();
    let session_cwd = cfg.cwd.to_string_lossy().into_owned();
    let fs = platform.filesystem();

    // `build_mobile` is async; drive it on the owned runtime so any spawned work
    // it does is owned by this handle's runtime, not an ambient one.
    let inner = runtime
        .block_on(build_mobile_inner(
            cfg,
            platform,
            listener,
            recording_sink,
            streaming_override,
        ))
        .map_err(|e| MobileEngineError::Internal(e.to_string()))?;

    let skill_count = crate::mobile_skill_registry().len();
    let event_sink = inner.event_sink.clone();

    Ok(Arc::new(MobileEngineHandle {
        runtime,
        inner,
        event_sink,
        active_cancel: Arc::new(Mutex::new(None)),
        tool_names,
        skill_count,
        claude_home,
        session_cwd,
        fs,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use client_adapter::{ClientEventListener, PermissionRequestSink};

    use super::{build_mobile, MobileConfig};
    // F3-06: the off-device host shim now lives in `crate::test_support` (the
    // single, non-drifting definition shared with the `skeleton_test.rs`
    // integration test). The in-crate F3-03/F3-05 unit tests reuse it. The
    // collecting permission sink is aliased to the legacy name these test bodies
    // already use.
    use crate::test_support::{
        test_config, CollectingPermissionSink as RecordingPermissionSink, FakeListener,
        HostFakePlatform,
    };

    /// F3-03: `MobileConfig::default` is constructible and its frozen field set
    /// is reachable — the mobile analog of `desktop_config_default_is_constructible`.
    #[test]
    fn mobile_config_default_is_constructible() {
        let cfg = MobileConfig::default();
        assert_eq!(cfg.api_base, "https://api.anthropic.com");
        assert!(cfg.api_key.is_empty());
        assert_eq!(cfg.cwd, std::path::PathBuf::from("."));
        assert_eq!(cfg.default_model, "claude-sonnet-4-20250514");
        assert!(cfg.provider_profiles.is_none());
        assert!(cfg.routing.is_none());
        let _clone = cfg.clone();
        let _ = format!("{cfg:?}");
    }

    /// F3-03: `build_mobile` constructs a real `ConversationOrchestrator`
    /// off-device, from a `MobileConfig` + a host fake `Platform` alone — no
    /// `std::env`, no device. This is the mobile sibling of
    /// `build_constructs_runtime_deterministically`.
    #[tokio::test]
    async fn build_mobile_constructs_orchestrator() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let platform: Arc<dyn traits::Platform> =
            Arc::new(HostFakePlatform::new(tmp.path().to_path_buf()));
        let listener: Arc<dyn ClientEventListener> = Arc::new(FakeListener::default());
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build_mobile(test_config(tmp.path()), platform, listener, perm_sink)
            .await
            .expect("build_mobile failed");

        // The orchestrator exists and exposes the `OrchestratorHandle` surface
        // the command registry binds to. Constructing it at all proves the full
        // mobile assembly (tool registry + command registry + adapter sinks).
        let _handle: Arc<dyn traits::OrchestratorHandle> = rt.orchestrator.clone();
    }

    /// F3-03: the built runtime binds the adapter sinks — the
    /// `AdapterPermissionGate` (proven by parking a real `check()` that lands on
    /// the recording sink) and the listener-backed output stream (proven by the
    /// returned listener being the one we registered).
    #[tokio::test]
    async fn mobile_runtime_binds_adapter_sinks() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let platform: Arc<dyn traits::Platform> =
            Arc::new(HostFakePlatform::new(tmp.path().to_path_buf()));
        let listener: Arc<dyn ClientEventListener> = Arc::new(FakeListener::default());
        let sink = Arc::new(RecordingPermissionSink::default());
        let perm_sink: Arc<dyn PermissionRequestSink> = sink.clone();

        let rt = build_mobile(test_config(tmp.path()), platform, listener, perm_sink)
            .await
            .expect("build_mobile failed");

        // Drive a `check()` on a spawned task; a deny-by-default tool parks a
        // request on the sink (proving the adapter gate is bound, not a no-op).
        let gate = rt.permission_gate.clone();
        let g = gate.clone();
        let task = tokio::spawn(async move {
            use permission::gate::PermissionGate;
            g.check("Bash", &serde_json::json!({"command": "ls"})).await
        });

        for _ in 0..2000 {
            if sink.count.load(std::sync::atomic::Ordering::SeqCst) == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            sink.count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "adapter gate must emit a PermissionRequest"
        );

        // Resolve so the parked future returns (request id starts at 1).
        assert!(
            gate.resolve(1, client_protocol::permission::PermissionResponseDto::Deny, "Bash")
                .await
        );
        let _ = task.await.unwrap();
    }

    // ── F3-05: the async `submit` FFI entry point ───────────────────────────

    use super::{build_mobile_engine, MobileEngineHandle};
    use client_protocol::commands::ClientCommand;
    use client_protocol::error::ClientError;
    use client_protocol::events::ClientEvent as Ev;
    use client_protocol::permission::PermissionResponseDto;

    /// Build a real, fully-wired [`MobileEngineHandle`] off-device (host fake
    /// `Platform`) so the F3-05 `submit` path is exercised on CI. Returns the
    /// handle plus the recording listener so a test can read back delivered
    /// events.
    fn build_submit_handle(
        root: &std::path::Path,
    ) -> (Arc<MobileEngineHandle>, Arc<FakeListener>) {
        let platform: Arc<dyn traits::Platform> =
            Arc::new(HostFakePlatform::new(root.to_path_buf()));
        let listener = Arc::new(FakeListener::default());
        let listener_dyn: Arc<dyn ClientEventListener> = listener.clone();
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let handle = build_mobile_engine(test_config(root), platform, listener_dyn, perm_sink)
            .expect("build_mobile_engine failed");
        (handle, listener)
    }

    /// F3-05: `submit(SendPrompt)` MUST spawn the streaming turn on the
    /// handle-owned runtime and RETURN PROMPTLY — it must not block for the
    /// whole turn (results stream via the listener). We prove the call resolves
    /// `Ok(())` without a `TurnEnded` having been delivered yet (the spawned turn
    /// against the host fake `Platform` does not complete synchronously inside
    /// the `submit` call).
    #[test]
    fn submit_send_prompt_returns_promptly() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        let result = handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::SendPrompt {
                    text: "hello".into(),
                    prompt_mode: None,
                    images: Vec::new(),
                    turn_id: Some(7),
                })
                .await
        });
        assert!(result.is_ok(), "submit(SendPrompt) returned an error: {result:?}");

        // The turn was SPAWNED, so `submit` returned before any `TurnEnded` was
        // delivered to the listener. (A `TurnStarted` may have been synthesized
        // synchronously, but the terminal `TurnEnded` must not have fired.)
        let saw_turn_ended = handle.runtime().block_on(async {
            listener
                .received
                .lock()
                .await
                .iter()
                .any(|e| matches!(e, Ev::TurnEnded { .. }))
        });
        assert!(
            !saw_turn_ended,
            "submit must return promptly — TurnEnded must not fire inside the call"
        );
    }

    /// F3-05: `submit(Cancel)` fires the connection-scoped cancellation token so
    /// an in-flight streaming turn unwinds. We arm a turn (`SendPrompt`), then
    /// `submit(Cancel)`, and assert the handle's active cancel token is now
    /// cancelled.
    #[test]
    fn submit_cancel_fires_token() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, _listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            // Arm a turn so a cancel token is in flight.
            handle
                .submit(ClientCommand::SendPrompt {
                    text: "drive a turn".into(),
                    prompt_mode: None,
                    images: Vec::new(),
                    turn_id: None,
                })
                .await
                .expect("submit(SendPrompt) ok");

            assert!(
                !handle.active_turn_is_cancelled().await,
                "the freshly-armed turn token must not be cancelled yet"
            );

            handle
                .submit(ClientCommand::Cancel { turn_id: None })
                .await
                .expect("submit(Cancel) ok");

            assert!(
                handle.active_turn_is_cancelled().await,
                "submit(Cancel) must fire the in-flight cancellation token"
            );
        });
    }

    /// F3-05: `submit(ApprovePermission)` resolves a parked `check()` oneshot on
    /// the connection-scoped [`AdapterPermissionGate`] (F1-14) — the inbound
    /// command side of the inverted permission handshake. A parked `check()`
    /// returns `Allow` once the approval arrives via `submit`.
    #[test]
    fn submit_approve_resolves_oneshot() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, _listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            // Park a real `check()` on the gate from a spawned task (the engine
            // turn side); it blocks on the oneshot until `submit` resolves it.
            let gate = handle.permission_gate();
            let g = gate.clone();
            let parked = handle.runtime().spawn(async move {
                use permission::gate::PermissionGate;
                g.check("Bash", &serde_json::json!({"command": "ls"})).await
            });

            // Spin until the gate has parked exactly one request (id starts at
            // 1). A short async sleep (not a bare `yield_now`) lets the spawned
            // `check()` make progress even if the worker pool is momentarily busy.
            for _ in 0..2000 {
                if gate.pending_count().await == 1 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            assert_eq!(gate.pending_count().await, 1, "check() must park a request");

            // Approve via the FFI command path — resolves the parked oneshot.
            handle
                .submit(ClientCommand::ApprovePermission {
                    request_id: 1,
                    response: PermissionResponseDto::AllowOnce,
                })
                .await
                .expect("submit(ApprovePermission) ok");

            let decision = parked.await.expect("parked check joined");
            assert_eq!(decision, permission::gate::PermissionDecision::Allow);
        });
    }

    // ── F3-07: async-over-FFI runtime registration ──────────────────────────

    /// F3-07: the async FFI exports resolve on the HANDLE-OWNED tokio runtime.
    ///
    /// `UniFFI`'s `#[uniffi::export(async_runtime = "tokio")]` (plus the workspace
    /// `uniffi` dep's `tokio` feature, pinned in F3-00) registers a tokio runtime
    /// as the foreign async executor; the mobile host registers the
    /// handle-owned `rt-multi-thread` runtime (§0.5 — one connection ⇒ one engine
    /// host owning one runtime). This proves the registration actually takes: the
    /// async `observed_runtime_id` export — driven through the SAME executor path
    /// `submit` uses — resolves on the runtime whose id matches the handle's owned
    /// runtime, NOT a transient ambient one.
    #[test]
    fn async_submit_resolves_on_handle_runtime() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, _listener) = build_submit_handle(tmp.path());

        // The handle-owned runtime's identity token (the runtime F3-07 registers
        // as the foreign async executor).
        let owned_id = handle.runtime_id();

        // Drive the async export on the handle-owned runtime — exactly what the
        // UniFFI tokio foreign executor does for a foreign caller. The future
        // awaits (a real yield point) and then reads the runtime it is bound to.
        let observed_id = handle
            .runtime()
            .block_on(async { handle.observed_runtime_id().await });

        assert_eq!(
            observed_id, owned_id,
            "the async FFI export must resolve on the handle-owned tokio runtime \
             (foreign async executor = the handle's runtime), got {observed_id} vs owned {owned_id}"
        );

        // And the inbound `submit` async export resolves on that same runtime: a
        // `Cancel` (no in-flight turn) drives the full `submit` future through the
        // executor and returns `Ok` — proving the async entry point itself awaits
        // on the registered runtime, not just the inspection helper.
        let cancel_result = handle
            .runtime()
            .block_on(async { handle.submit(ClientCommand::Cancel { turn_id: None }).await });
        assert!(
            cancel_result.is_ok(),
            "submit must resolve on the handle-owned runtime: {cancel_result:?}"
        );
    }

    // ── SESSIONS/HISTORY: ListSessions / NewSession / ResumeSession ──────────

    /// Seed one valid session JSONL under `<claude_home>/projects/<sanitize(cwd)>/`
    /// so `submit(ListSessions)` has a real on-disk catalog to enumerate. Mirrors
    /// the `session` crate's own `list_recent_test` fixture (the enumerator reads
    /// the dir via `tokio::fs` and each file via the injected `fs`). Returns the
    /// seeded session UUID string.
    fn seed_session_file(root: &std::path::Path) -> String {
        let cfg = test_config(root);
        let cwd = cfg.cwd.to_string_lossy().into_owned();
        let project_dir = cfg
            .claude_home
            .join("projects")
            .join(session::jsonl::project_dir_name(&cwd));
        std::fs::create_dir_all(&project_dir).expect("create project dir");
        // A fixed, valid UUID literal (the loader parses the filename stem with
        // `Uuid::parse_str`; engine-mobile does not depend on the `uuid` crate, so
        // we use a literal instead of minting one). Deterministic by design.
        let uuid = "11111111-2222-3333-4444-555555555555".to_string();
        let path = project_dir.join(format!("{uuid}.jsonl"));
        let line = serde_json::json!({
            "type": "user",
            "uuid": uuid,
            "parentUuid": serde_json::Value::Null,
            "sessionId": uuid,
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": cwd,
            "version": "0.6.0",
            "isSidechain": false,
            "userType": "external",
            "message": {"role": "user", "content": "hello from a prior session"}
        });
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&line).unwrap()))
            .expect("write session file");
        uuid
    }

    /// Drain every event the listener received during a blocked closure.
    async fn drained(listener: &FakeListener) -> Vec<Ev> {
        listener.received.lock().await.clone()
    }

    /// SESSIONS/HISTORY: `submit(ListSessions)` enumerates the on-disk catalog and
    /// emits a `SessionList` carrying the seeded row (proving the engine actually
    /// reads the store — not a no-op catch-all). The row's `uuid` matches the
    /// seeded file's stem, lowered via the shared `lower_session_metadata`.
    #[test]
    fn submit_list_sessions_emits_seeded_row() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let seeded_uuid = seed_session_file(tmp.path());
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::ListSessions { limit: None })
                .await
                .expect("submit(ListSessions) ok");

            let events = drained(&listener).await;
            let row = events.iter().find_map(|e| match e {
                Ev::SessionList { sessions } => Some(sessions.clone()),
                _ => None,
            });
            let sessions = row.expect("a SessionList event must be emitted");
            assert_eq!(sessions.len(), 1, "exactly one seeded session expected");
            assert_eq!(
                sessions[0].uuid, seeded_uuid,
                "the listed row must be the seeded session"
            );
        });
    }

    /// SESSIONS/HISTORY: `submit(ListSessions)` on a connection with NO on-disk
    /// catalog (empty / missing project dir) still replies with a `SessionList`
    /// carrying an EMPTY vec — the loader's `EmptyDirectory` is "no sessions yet",
    /// not an error, and the client must always get a reply.
    #[test]
    fn submit_list_sessions_empty_when_no_catalog() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::ListSessions { limit: Some(5) })
                .await
                .expect("submit(ListSessions) ok");

            let events = drained(&listener).await;
            let sessions = events
                .iter()
                .find_map(|e| match e {
                    Ev::SessionList { sessions } => Some(sessions.clone()),
                    _ => None,
                })
                .expect("a SessionList event must be emitted even with no catalog");
            assert!(
                sessions.is_empty(),
                "no on-disk catalog must yield an empty SessionList, got {sessions:?}"
            );
        });
    }

    /// SESSIONS/HISTORY: `submit(NewSession)` clears the session (minting a fresh
    /// id) and confirms with a `SessionStarted` carrying the new connection
    /// session id — proving the command drives the real orchestrator handle, not
    /// the no-op catch-all. The reported id matches the orchestrator's
    /// `current_session_id` after the swap.
    #[test]
    fn submit_new_session_emits_session_started() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            use traits::OrchestratorHandle;
            let oh: Arc<dyn OrchestratorHandle> = handle.inner().orchestrator.clone();
            let before = oh.current_session_id().await.to_string();

            handle
                .submit(ClientCommand::NewSession { cwd: None, model: None })
                .await
                .expect("submit(NewSession) ok");

            let after = oh.current_session_id().await.to_string();
            assert_ne!(before, after, "NewSession must mint a fresh session id");

            let events = drained(&listener).await;
            let started = events.iter().find_map(|e| match e {
                Ev::SessionStarted { session_id } => Some(session_id.clone()),
                _ => None,
            });
            assert_eq!(
                started.expect("a SessionStarted event must be emitted"),
                after,
                "SessionStarted must carry the new connection session id"
            );
        });
    }

    /// Seed a REPLAY-VALID session JSONL under
    /// `<claude_home>/projects/<sanitize(cwd)>/<uuid>.jsonl` — a user+assistant
    /// pair with a proper `parentUuid` chain (first msg parent=null, the second's
    /// parent = the first's uuid, both `sessionId == <file uuid>`) so it PASSES
    /// the loader's `validate_chain`. Returns `(file_uuid, user_assistant_count)`.
    fn seed_replay_valid_session(root: &std::path::Path) -> (String, usize) {
        let cfg = test_config(root);
        let cwd = cfg.cwd.to_string_lossy().into_owned();
        let project_dir = cfg
            .claude_home
            .join("projects")
            .join(session::jsonl::project_dir_name(&cwd));
        std::fs::create_dir_all(&project_dir).expect("create project dir");

        // Fixed, valid UUID literals (engine-mobile parses, never mints, in the
        // test). The file stem IS the sessionId; the two messages carry DISTINCT
        // `uuid`s forming a one-link parent chain.
        let file_uuid = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa".to_string();
        let user_uuid = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb".to_string();
        let asst_uuid = "cccccccc-3333-4333-8333-cccccccccccc".to_string();
        let path = project_dir.join(format!("{file_uuid}.jsonl"));

        let user_line = serde_json::json!({
            "type": "user",
            "uuid": user_uuid,
            "parentUuid": serde_json::Value::Null,
            "sessionId": file_uuid,
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": cwd,
            "version": "0.6.0",
            "isSidechain": false,
            "userType": "external",
            "message": {"role": "user", "content": "resume me from disk"}
        });
        let asst_line = serde_json::json!({
            "type": "assistant",
            "uuid": asst_uuid,
            "parentUuid": user_uuid,
            "sessionId": file_uuid,
            "timestamp": "2026-05-25T12:00:01.000Z",
            "cwd": cwd,
            "version": "0.6.0",
            "isSidechain": false,
            "message": {"role": "assistant", "content": [{"type": "text", "text": "resumed!"}]}
        });
        let body = format!(
            "{}\n{}\n",
            serde_json::to_string(&user_line).unwrap(),
            serde_json::to_string(&asst_line).unwrap()
        );
        std::fs::write(&path, body).expect("write replay-valid session file");
        (file_uuid, 2)
    }

    /// SESSIONS/HISTORY (live ResumeSession): `submit(ResumeSession)` against a
    /// REPLAY-VALID on-disk session hot-restores it into the running orchestrator.
    /// Asserts a `SessionResumed` event is emitted whose `messages.len()` equals
    /// the seeded user+assistant count, AND the orchestrator's live
    /// `conversation_transcript` equals the restored history (proving the model
    /// will see prior context on the next turn).
    #[test]
    fn submit_resume_session_rehydrates_and_emits() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (file_uuid, seeded_count) = seed_replay_valid_session(tmp.path());
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            use traits::OrchestratorHandle;

            let result = handle
                .submit(ClientCommand::ResumeSession {
                    session_id: file_uuid.clone(),
                    cwd: None,
                })
                .await;
            assert!(
                result.is_ok(),
                "ResumeSession against a replay-valid session must succeed, got {result:?}"
            );

            // A SessionResumed carrying the full restored transcript was emitted.
            let events = drained(&listener).await;
            let messages = events
                .iter()
                .find_map(|e| match e {
                    Ev::SessionResumed { session_id, messages } => {
                        assert_eq!(session_id, &file_uuid, "resumed id must be the named session");
                        Some(messages.clone())
                    }
                    _ => None,
                })
                .expect("a SessionResumed event must be emitted on a successful resume");
            assert_eq!(
                messages.len(),
                seeded_count,
                "SessionResumed.messages must carry every replayed user/assistant message"
            );

            // The RUNNING orchestrator adopted the restored history — the next
            // turn will see the prior context.
            let oh: Arc<dyn OrchestratorHandle> = handle.inner().orchestrator.clone();
            let transcript = oh.conversation_transcript().await;
            assert_eq!(
                transcript.len(),
                seeded_count,
                "the live orchestrator must hold the restored transcript after resume"
            );
            // The adopted id is the named session (resume does NOT mint a fresh one).
            assert_eq!(
                oh.current_session_id().await.as_uuid().to_string(),
                file_uuid,
                "resume must adopt the named session id on the live orchestrator"
            );
        });
    }

    /// SESSIONS/HISTORY (live ResumeSession): an UNKNOWN session uuid (no on-disk
    /// file) is honestly REJECTED — a missing session is genuinely not resumable,
    /// so we return `ClientError::Rejected` rather than emit a false `SessionResumed`.
    #[test]
    fn submit_resume_session_missing_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            let result = handle
                .submit(ClientCommand::ResumeSession {
                    // A well-formed uuid that names no on-disk session.
                    session_id: "dddddddd-4444-4444-8444-dddddddddddd".into(),
                    cwd: None,
                })
                .await;
            assert!(
                matches!(result, Err(ClientError::Rejected { .. })),
                "an unknown session must be Rejected, got {result:?}"
            );
            let events = drained(&listener).await;
            assert!(
                !events.iter().any(|e| matches!(e, Ev::SessionResumed { .. })),
                "a rejected ResumeSession must NOT emit a (false) SessionResumed event"
            );
        });
    }

    /// SESSIONS/HISTORY (live ResumeSession): a MALFORMED session id (not a uuid)
    /// is honestly REJECTED — we parse the id as a `Uuid` first and reject a
    /// non-uuid rather than fake a confirmation.
    #[test]
    fn submit_resume_session_malformed_id_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            let result = handle
                .submit(ClientCommand::ResumeSession {
                    session_id: "not-a-uuid".into(),
                    cwd: None,
                })
                .await;
            assert!(
                matches!(result, Err(ClientError::Rejected { .. })),
                "a malformed session id must be Rejected, got {result:?}"
            );
            let events = drained(&listener).await;
            assert!(
                !events.iter().any(|e| matches!(e, Ev::SessionResumed { .. })),
                "a rejected ResumeSession must NOT emit a SessionResumed event"
            );
        });
    }

    /// SESSIONS/HISTORY (live ResumeSession): resume is REJECTED while a turn is in
    /// flight (mirror of `ClearSession` / `NewSession` mid-turn guards). We arm a
    /// live (un-cancelled) cancel token via `SendPrompt`, then submit Resume.
    #[test]
    fn submit_resume_session_mid_turn_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (file_uuid, _count) = seed_replay_valid_session(tmp.path());
        let (handle, _listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            // Arm an in-flight turn so a live cancel token is recorded.
            handle
                .submit(ClientCommand::SendPrompt {
                    text: "drive a turn".into(),
                    prompt_mode: None,
                    images: Vec::new(),
                    turn_id: None,
                })
                .await
                .expect("submit(SendPrompt) ok");
            assert!(
                !handle.active_turn_is_cancelled().await,
                "the freshly-armed turn token must not be cancelled yet"
            );

            let result = handle
                .submit(ClientCommand::ResumeSession {
                    session_id: file_uuid,
                    cwd: None,
                })
                .await;
            assert!(
                matches!(result, Err(ClientError::Rejected { .. })),
                "resume must be Rejected while a turn is in flight, got {result:?}"
            );
        });
    }
}
