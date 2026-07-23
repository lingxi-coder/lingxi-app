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
//! `build_mobile` takes every functional input through [`MobileConfig`] and the
//! OS handles through `Arc<dyn Platform>`. It reads a SMALL, fixed set of
//! `std::env` vars purely to mirror desktop parity behavior — the
//! `LINGXI_MEMDIR_PREFETCH` activation gate, the `ANTHROPIC_SMALL_FAST_MODEL`
//! / `ANTHROPIC_DEFAULT_HAIKU_MODEL` model ids a `prompt` hook may resolve to, and
//! `HOME` for the permission `FsRoots` (absent on a sandboxed device ⇒ `None`).
//! These are all unset on a real device, so on-device behavior stays
//! deterministic (prefetch off, no model override, no home root). No other env /
//! argv is read. On the host (CI) a fake `Platform` shim (see the `tests` module)
//! supplies portable handles so the orchestrator is constructed and the adapter
//! sinks are exercised without a device — exactly the spec §8 "prove from a
//! Swift/Kotlin unit test" smoke path, runnable on the host. The real device
//! `Platform` (`platform-ios` / `platform-android`) is `cfg(target_os)`-gated in
//! `Cargo.toml`, so this module never names a device crate.

use std::collections::HashMap;
use std::sync::Arc;

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
use command_api::model::BuiltinCommandHandler;
use command_api::parse_slash_command;
use command_api::RegistrySlashDispatcher;
use llm_client::oauth::anthropic::client::ClaudeAiOAuthClient;
use llm_client::oauth::anthropic::config::ClaudeAiOAuthConfig;
use llm_client::oauth::anthropic::handle::OAuthHandle;
use llm_client::LlmTransportBridge;
use llm_client::{DefaultLlmClient, Transport};
use orchestrator::model::user_agent::UserAgentEnv;
use orchestrator::provider_adapter::SubscriberState;
use orchestrator::test_support::StaticMemoryProvider;
use orchestrator::{
    ConversationOrchestrator, OrchestratorApiClient, OrchestratorConfig, ProviderApiAdapter,
    StreamingApiClient,
};
use permission::gate::PermissionGate;
use permission::PermissionMode;
use sandbox::runtime_config::{Platform as SandboxPlatform, SandboxRuntimeConfig};
use secret::CredentialManager;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tool_api::AnthropicRequestBuilder;
use tool_api::BuiltinToolContext;
use tool_api::SessionCwd;
use traits::http::{
    HttpError, RawByteStream, RawByteStreamWithMeta, SseStream, SseStreamWithMeta,
    WebSocketConnectionWithMeta, WebSocketMessageStreamWithMeta,
};
use traits::{
    AuthHandle, HttpTransport, OrchestratorHandle, OutputStream, Platform, SlashCommandDispatcher,
};

use crate::{mobile_command_registry, mobile_tool_registry_with_skill_loader};

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
    /// Forward to the inner transport so the device backend's real status and
    /// headers are preserved on binary (AWS event-stream) responses.
    async fn stream_raw_bytes_with_meta(
        &self,
        req: protocol::HttpRequest,
    ) -> Result<RawByteStreamWithMeta, HttpError> {
        self.0.stream_raw_bytes_with_meta(req).await
    }
    /// Forward WebSocket streaming so device transports that support Responses
    /// WebSocket are not hidden behind this sized wrapper.
    async fn stream_websocket_messages_with_meta(
        &self,
        req: protocol::HttpRequest,
    ) -> Result<WebSocketMessageStreamWithMeta, HttpError> {
        self.0.stream_websocket_messages_with_meta(req).await
    }
    /// Forward reusable WebSocket connections so Responses sessions can reuse
    /// the device backend connection inside a turn.
    async fn open_websocket_connection_with_meta(
        &self,
        req: protocol::HttpRequest,
    ) -> Result<WebSocketConnectionWithMeta, HttpError> {
        self.0.open_websocket_connection_with_meta(req).await
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
// P0.2: `Clone` only — `Debug` is implemented manually below because the new
// `memory_provider` field (`Arc<dyn MemoryHierarchyProvider>`) is not `Debug`.
// Mirrors the `DesktopConfig` pattern (engine-desktop/src/lib.rs:799-846).
#[derive(Clone)]
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
    pub lingxi_home: std::path::PathBuf,
    /// Model id the build defaults to (`OrchestratorConfig.model`).
    pub default_model: String,
    /// Settings-declared `providers` block as raw JSON, fed verbatim to
    /// `llm_client::ClientConfig` via `build()`. `None` ⟶ built-in profiles only.
    pub provider_profiles: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    /// Settings-declared `routing` block as raw JSON, fed verbatim to
    /// `llm_client::ClientConfig`. `None` ⟶ the default (empty) routing config.
    pub routing: Option<serde_json::Value>,
    /// Android-only Shell tool gate + prompt carrier (spec r3 §Registration gates,
    /// P3). `None` on iOS and desktop — the Shell tool is absent on those
    /// platforms. Built by `android-aar::build_android_engine` from the probed
    /// capability cache + the `AndroidShellConfig` gate; consumed by
    /// `tool_shell_mobile::register_all` in the composition root.
    pub android_shell: Option<tool_api::AndroidShellToolCtx>,
    /// Android-only Git tool gate + workspace carrier (spec §G5, P4). `None` on
    /// iOS and desktop — the Git tool is absent on those platforms. Built by
    /// `android-aar::build_android_engine` from the enable flag + workspace
    /// readiness + CA-store reachability; consumed by
    /// `tool_git_mobile::register_all` in the composition root.
    pub android_git: Option<tool_api::AndroidGitToolCtx>,
    /// Android-only Git network secret (HTTPS token + CA dir, spec §G3, P4).
    /// Held separately from the public [`MobileConfig::android_git`] carrier so
    /// the token never enters the broadly-cloned public ctx. `None` until Task 10
    /// wires it from `android-aar`; `tool-git-mobile` reads it at call time.
    pub android_git_secret: Option<tool_api::AndroidGitSecret>,
    /// P0.2 (mobile LINGXI.md hierarchy): the memory hierarchy provider the
    /// orchestrator loads its instruction files from. The production FFI entry
    /// points (`ios-framework` / `android-aar`) inject
    /// `Some(orchestrator::prompt::real_provider())` so the orchestrator loads
    /// the real `<cwd>/LINGXI.md` + `<lingxi_home>/LINGXI.md` hierarchy into the
    /// system prompt (claude-code parity) and the session-start
    /// `fire_instructions_loaded()` fires over those files. `None` (the default +
    /// every off-device host test) falls back to the empty
    /// [`StaticMemoryProvider`], so a default build loads NO memory and the host
    /// tests stay deterministic (they never touch the real filesystem). Mirrors
    /// `engine_desktop::DesktopConfig::memory_provider`.
    pub memory_provider: Option<Arc<dyn orchestrator::prompt::MemoryHierarchyProvider>>,
}

impl std::fmt::Debug for MobileConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn MemoryHierarchyProvider` is not `Debug`, so render the
        // `memory_provider` field as a Some(<provider>)/None presence marker.
        // Every other field is printed verbatim so `{cfg:?}` stays useful for
        // host logging (copies the `DesktopConfig` Debug pattern at
        // engine-desktop/src/lib.rs:799-846).
        f.debug_struct("MobileConfig")
            .field("api_base", &self.api_base)
            .field("api_key", &self.api_key)
            .field("cwd", &self.cwd)
            .field("lingxi_home", &self.lingxi_home)
            .field("default_model", &self.default_model)
            .field("provider_profiles", &self.provider_profiles)
            .field("routing", &self.routing)
            .field("android_shell", &self.android_shell)
            .field("android_git", &self.android_git)
            .field("android_git_secret", &self.android_git_secret)
            .field(
                "memory_provider",
                if self.memory_provider.is_some() {
                    &"Some(<provider>)"
                } else {
                    &"None"
                },
            )
            .finish()
    }
}

impl Default for MobileConfig {
    fn default() -> Self {
        Self {
            api_base: "https://api.anthropic.com".to_string(),
            api_key: String::new(),
            cwd: std::path::PathBuf::from("."),
            lingxi_home: std::path::PathBuf::new(),
            default_model: crate::MobileEngineConfig::default().default_model,
            provider_profiles: None,
            routing: None,
            android_shell: None,
            android_git: None,
            android_git_secret: None,
            // P0.2: default to NO memory provider (empty, deterministic). The
            // production FFI entry points inject `Some(real_provider())`.
            memory_provider: None,
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
    /// Whether the wired secure-storage backend can actually PERSIST credentials
    /// (i.e. is a real OS Keychain/Keystore, `is_encrypted() == true`). Mobile
    /// currently wires the non-persisting `PlainTextSecureStorage` stub, so this
    /// is `false` and OAuth `/login` cannot persist its tokens — the Login arm
    /// short-circuits with a clear message instead of failing at the persist step
    /// with a cryptic `BackendUnavailable` (audit re-pass, secure-storage finding;
    /// the real native store is a §11 / Plan-17 follow-up). Becomes `true`
    /// automatically once a native Keychain/Keystore SecureStorage is injected.
    pub oauth_supported: bool,
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

/// First-party Anthropic models the mobile engine routes by default, plus the
/// configured `default_model` and any env-configured small-fast / haiku model a
/// `prompt` hook may resolve to. The `llm_client` registry resolves a request
/// model by exact id, so every model the host may request must appear here
/// (Phase 2a-mobile: this becomes the Anthropic profile `assemble` declares).
///
/// The mobile sibling of `engine_desktop::anthropic_models_for`, minus the
/// `fallback_model` (mobile has no fallback-model config knob). The env small-
/// fast / haiku ids are read inline because main has no public
/// `small_fast_model_env_ids` helper.
fn anthropic_models(default_model: &str) -> Vec<llm_client::ModelProfile> {
    let caps = llm_client::Capabilities {
        streaming: true,
        tools: true,
        vision: true,
        documents: true,
        reasoning: true,
        structured_output: true,
    };
    let mut ids: Vec<String> = vec![
        "claude-opus-4-6".to_string(),
        "claude-sonnet-4-6".to_string(),
        "claude-haiku-4-5".to_string(),
    ];
    ids.push(default_model.to_string());
    // Env-configured small-fast / haiku model a `prompt` hook may resolve to
    // (matching `hook_prompt_runner::resolve_model`'s precedence:
    // `ANTHROPIC_SMALL_FAST_MODEL` > `ANTHROPIC_DEFAULT_HAIKU_MODEL` > default
    // Haiku), so such a request resolves instead of failing `ModelUnavailable`.
    for var in [
        "ANTHROPIC_SMALL_FAST_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    ] {
        if let Ok(m) = std::env::var(var) {
            if !m.is_empty() {
                ids.push(m);
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .map(|id| llm_client::ModelProfile {
            display_model: id.clone(),
            request_model: id.clone(),
            billing_model: id,
            aliases: Vec::new(),
            description: None,
            capabilities: caps,
        })
        .collect()
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

// Phase 2a-mobile: the multi-provider client config / chains / credential
// sources / pricing catalog are now assembled by `provider_config::assemble`
// (which owns the byte-equivalent Anthropic profile + the builtin catalog
// presets + the settings-`providers` merge). The old single-Anthropic
// `builtin_anthropic_config` / `apply_settings_providers` /
// `parse_routing_overrides` helpers from `platform_common::llm_config` are no
// longer wired here; they remain in `platform_common` (the desktop e2e tests
// still reach them via fully-qualified paths). `LlmTransportBridge` is still
// imported at the top of the module.

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
    // Secure storage: prefer the platform's NATIVE store (iOS Keychain / Android
    // Keystore) when the device layer injects one; otherwise fall back to the
    // non-persisting development stub. The stub cannot persist secrets, so OAuth
    // `/login` is short-circuited with a clear message below (it cannot store
    // tokens); a real injected store flips `oauth_supported` true and enables
    // subscription login. Computed before `storage` moves into CredentialManager.
    let storage: Arc<dyn traits::SecureStorage> = platform
        .secure_storage()
        .unwrap_or_else(|| Arc::new(platform_posix_minimal::PlainTextSecureStorage::new()));
    let oauth_supported = traits::SecureStorage::is_encrypted(storage.as_ref());

    // Audit fix (telemetry parity): ONE shared AnalyticsBus drives the whole
    // pipeline — the `ApiService` (so `tengu_api_*` events are not dropped), the
    // tool context (`tool_ctx.bus`), and the orchestrator (`.with_analytics_bus`)
    // — instead of the prior split where a private bus served only the tools and
    // the ApiService got `None`. Mirrors the desktop root's single logEvent sink.
    let analytics_bus = Arc::new(telemetry::AnalyticsBus::new());

    // (2a) Task 10: DefaultLlmClient over LlmTransportBridge.
    //      Mobile uses the platform's `Arc<dyn HttpTransport>` wrapped in `DynHttp`
    //      so the device backend is preserved; no desktop-only deps are pulled.
    //
    //      Phase 2a-mobile: assemble the FULL multi-provider client config
    //      (Anthropic + builtin catalog presets + settings `providers`) + chains
    //      + credential sources + pricing catalog via `provider_config::assemble`,
    //      mirroring `engine_desktop::build`. Mobile is api-key + env only (no
    //      OAuth, no availability map, no CostTracker / picker), so
    //      `anthropic_has_oauth = false` and there is no OAuth credential delegate
    //      — the interactive `/connect` / picker is a §11 follow-up. A bad
    //      settings entry only emits a warning; the engine still boots with every
    //      well-formed profile (incl. the built-in Anthropic one).
    let llm_transport: Arc<dyn Transport> =
        Arc::new(LlmTransportBridge::new(DynHttp(http.clone())));
    let has_api_key = !cfg.api_key.is_empty();
    let assembled = provider_config::assemble(provider_config::AssembleInputs {
        anthropic_api_base: cfg.api_base.clone(),
        anthropic_models: anthropic_models(&cfg.default_model),
        anthropic_has_api_key: has_api_key,
        anthropic_has_oauth: false, // mobile inference is api-key-only (no OAuth)
        user_providers: cfg.provider_profiles.clone().unwrap_or_default(),
        routing: cfg.routing.clone(),
    });
    for w in &assembled.warnings {
        tracing::warn!(warning = %w, "provider-config assembly (mobile)");
    }

    // TPM-C (mobile): resolve an optional `profile/model` qualifier in the
    // configured default_model so a shared id routes deterministically on the
    // first turn (mirror of engine-desktop). Must run while
    // `assembled.client_config.providers` is still owned (before `from_config`
    // moves it). `display_model`/`provider_label` are immaterial to parsing, so
    // we reuse `request_model` / the profile name for both fields.
    let default_listings: Vec<traits::ModelListing> = assembled
        .client_config
        .providers
        .iter()
        .flat_map(|p| {
            let profile = p.profile_name.clone();
            p.models.iter().map(move |m| traits::ModelListing {
                display_model: m.request_model.clone(),
                request_model: m.request_model.clone(),
                provider_id: profile.clone(),
                provider_label: profile.clone(),
                description: m.description.clone(),
                supports_reasoning: m.capabilities.reasoning,
            })
        })
        .collect();
    let (default_model_id, default_model_profile) =
        traits::parse_model_ref(&cfg.default_model, &default_listings);

    // (3) Credential manager — built BEFORE the client so the same `Arc` serves
    //     BOTH the composite credential provider (below) and the OAuth client
    //     (used by /login, /logout, step (3b)). One store, no second keychain.
    let credentials = Arc::new(CredentialManager::new(storage, clock.clone(), http.clone()));

    let mut client = DefaultLlmClient::from_config(assembled.client_config)
        .map_err(|e| MobileBuildError::ApiBase(format!("llm-client config: {e}")))?;
    // §6.1: ONE composite credential slot for ALL providers — the Anthropic api
    // key is served directly; every other provider resolves keychain → env. The
    // composite preserves the env fallback, so api-key-via-`ANTHROPIC_API_KEY`
    // still works exactly as before. Mobile has no OAuth delegate.
    let composite = provider_config::MultiCredentialProvider::new(
        credentials.clone(),
        assembled.credential_sources.clone(),
        if has_api_key {
            Some(cfg.api_key.clone())
        } else {
            None
        },
        None,
        std::collections::BTreeMap::new(),
    );
    client = client.with_credential_provider(Arc::new(composite));
    let llm_client = Arc::new(client);

    // No live subscription slot on mobile (no OAuth profile fetch) — static state stands.
    let subscriber_state = SubscriberState {
        is_subscriber: false,
        is_enterprise: false,
    };

    // 3c-T3: build the cost estimator from the assembled pricing catalog so
    // LlmResponse.cost is populated on every successful decode. The catalog
    // already carries the built-in reference tiers + non-Anthropic preset rows +
    // any settings per-profile pricing overrides folded in by `assemble`. Unpriced
    // / unknown models leave cost = None (never an error).
    let cost_estimator = {
        use llm_client::{CostEstimator, PricingPolicy};
        use orchestrator::cost_wiring::llm_catalog_from_cost;
        let llm_cat = llm_catalog_from_cost(&assembled.pricing);
        Arc::new(CostEstimator::new(llm_cat, PricingPolicy::MarkUnestimated))
    };

    // Audit #15: session CostTracker (desktop parity). The `cost_estimator` above
    // populates per-response `LlmResponse.cost`; the CostTracker accumulates the
    // running SESSION total the orchestrator records each turn. The persist
    // channel is DRAINED by a spawned recv-loop that discards each `CostState` —
    // byte-for-byte mirroring engine-desktop (which also just drains it): mobile
    // has no on-disk cost persistence / `/cost` UI consumer yet, but wiring the
    // tracker keeps the accounting path 1:1 with desktop. A fresh build-time
    // SessionId is used (the per-connection session swaps in later, as on desktop).
    let cost_tracker = {
        let (cost_persist_tx, mut cost_persist_rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move { while cost_persist_rx.recv().await.is_some() {} });
        Arc::new(cost::CostTracker::new(
            protocol::SessionId::new(),
            Arc::new(assembled.pricing),
            cost_persist_tx,
        ))
    };

    // Phase 2a-mobile CHAINS BRIDGE: translate the assembled `ChainConfig` into
    // main's richer adapter's `fallback_overrides` shape (same as engine-desktop —
    // we reuse main's `ProviderApiAdapter::new_with_routing`, NOT parity's leaner
    // `new`). `assemble` keys each chain by the request/display model id with an
    // ordered list of `ChainEntry`; main's adapter routes by model-id through the
    // multi-provider registry, so the informational `ChainEntry.provider_id` is
    // dropped here — the per-entry `model` ids are the fallback chain. Cross-
    // provider routing still resolves because every provider's models are
    // registered in the assembled `ClientConfig`. The adapter's own alias map is
    // rebuilt from the client's `available_models()` (whose aliases `assemble`
    // already populated from `chains.aliases`), so no separate alias pass here.
    let fallback_overrides: std::collections::BTreeMap<String, Vec<String>> = assembled
        .chains
        .chains
        .iter()
        .map(|(key, entries)| {
            (
                key.clone(),
                entries.iter().map(|e| e.model.clone()).collect(),
            )
        })
        .collect();
    // Retry override → main's scalar settings_max_retries / settings_backoff_ms.
    let settings_max_retries = assembled.chains.retry.as_ref().map(|r| r.max_attempts);
    let settings_backoff_ms = assembled.chains.retry.as_ref().map(|r| r.backoff_ms);

    // ONE adapter implements BOTH `OrchestratorApiClient` (batched) and
    // `StreamingApiClient` (the streaming turn path the mobile transport always
    // drives). Production wires it for both paths; a test may substitute the
    // streaming side via `streaming_override` (plan F3-06). Mobile is NOT a
    // subscriber (`SubscriberState::default()` — api-key-only inference), and
    // binds no live subscription slot / availability map / CostTracker (out of
    // scope; mobile parity did not).
    let api_service = Arc::new(llm_client::ApiService::new_with_routing(
        llm_client,
        llm_transport,
        subscriber_state,
        UserAgentEnv::from_process_env(),
        env!("CARGO_PKG_VERSION"),
        Some(analytics_bus.clone()), // audit fix: API events share the one bus
        None,
        Some(cost_estimator),
        fallback_overrides,
        settings_max_retries,
        settings_backoff_ms,
    ));
    let provider_adapter = Arc::new(ProviderApiAdapter::new(api_service.clone()));
    let api_client: Arc<dyn OrchestratorApiClient> = provider_adapter.clone();
    let streaming_api: Arc<dyn StreamingApiClient> =
        streaming_override.unwrap_or(provider_adapter.clone() as Arc<dyn StreamingApiClient>);
    // WebSearch builds Anthropic `POST /v1/messages` requests via its own
    // provider (server-side web search is Anthropic-only in v1).
    let tool_provider = Arc::new(
        AnthropicRequestBuilder::new(cfg.api_key.clone(), Some(cfg.api_base.clone()))
            .with_mcp_token_counter(provider_adapter.clone()),
    );

    // (3b) OAuth client (used by /login, /logout). Reuses the SAME `credentials`
    //      manager built above for the composite credential provider — one
    //      keychain-backed store, not a second one.
    let oauth_cfg = ClaudeAiOAuthConfig::default_with_port(0);
    let oauth_client = Arc::new(ClaudeAiOAuthClient::new(
        oauth_cfg,
        http.clone(),
        credentials,
    ));
    let auth: Arc<dyn AuthHandle> = Arc::new(OAuthHandle::new(oauth_client));

    // (4) Orchestrator config from `cfg` (was a host env/arg read).
    let mut orch_cfg = OrchestratorConfig::default();
    // TPM-C: use the bare id produced by parse_model_ref (strips a profile/
    // prefix when present, passes through unchanged for bare ids).
    orch_cfg.model.clone_from(&default_model_id);

    // (5) Connection-scoped sinks — the mobile transport's analog of the
    //     bridge-server's WS writer:
    //     - the `listener` becomes the `ClientEventSink` (via `ListenerSink`)
    //       the `AdapterOutputStream` pushes turn events to;
    //     - the `permission_sink` receives the gate's outbound requests.
    //     Mobile binds the `AdapterPermissionGate` (no always-allow mode), then
    //     wraps it with a local `PolicyPermissionGate` so the core policy binds.
    let event_sink = ListenerSink::arc(listener.clone());
    let output: Arc<dyn OutputStream> = Arc::new(AdapterOutputStream::new(event_sink.clone()));

    // (3c) No `.with_persist` on mobile: a device session has no project
    // `.lingxi/settings.local.json` convention to write back to, so AllowAlways
    // stays session-only here (the desktop transport gate persists; this does not).
    let adapter_gate = Arc::new(AdapterPermissionGate::new(permission_sink));
    // Wrap the adapter gate with a local `PolicyPermissionGate` so the CORE
    // allow/deny/ask/defaultMode semantics bind on mobile too — claude-code
    // enforces ONE core policy on every host, not "the remote client is the
    // enforcement". The adapter gate stays the Ask-delegation transport: an
    // unresolved mutating Ask still forwards to the remote client, but local deny/
    // allow rules + defaultMode are honored regardless of what the client
    // replicates. Rules are loaded from the SAME project + user settings.json the
    // hook loader reads below (dedup-aware: on mobile `lingxi_home` can equal
    // `<cwd>/.claude`, so a colliding path is read once to avoid doubling rules).
    // Read(deny) → Grep/Glob search-exclude globs, resolved from the policy below
    // (empty when no Read-deny rule ⇒ unchanged default).
    let mut read_deny_exclude_globs: Vec<String> = Vec::new();
    // (P2-14) `settings.skipWebFetchPreflight` → WebFetch skips the domain-blocklist
    // preflight. Mobile has no `engine::settings::Settings::load` seam (no `engine`
    // dep), so it reads the key directly from the SAME settings.json tiers the perms
    // loop below reads, scalar-override (later tier wins). `false` by default.
    let mut skip_web_fetch_preflight = false;
    // (M-15) `settings.askUserQuestionTimeout` (`60s`/`5m`/`10m`/`never`) → the
    // AskUserQuestion resolver idle window. Read from the SAME settings.json tiers
    // as the perms loop below, scalar-override (later tier wins). `None` by default
    // (⇒ `never`, block on the user). Parsed into `AskUserQuestionTimeout` at
    // `tool_ui` registration.
    let mut ask_user_question_timeout: Option<String> = None;
    // (M-03) `settings.disableAgentView` → the agent-view fork/subtask surface is
    // disabled exactly like `CLAUDE_CODE_DISABLE_AGENT_VIEW=1` (binary `I2i()`).
    // Read from the SAME settings.json tiers as the perms loop below,
    // scalar-override (later tier wins). `false` by default (agent view enabled;
    // the env half still applies independently). Threaded into
    // `register_core_batch_8` via `traits::agent_view::is_enabled_with_setting`.
    let mut disable_agent_view = false;
    // (#3 shell-expansion) Capture the boot `Arc<PermissionPolicy>` before it is
    // consumed by `PolicyPermissionGate::new`, so `tool_ctx.permission_policy`
    // shares the SAME base policy the model-facing gate enforces (the prompt
    // shell-expansion provider reads it as the base for embedded `!`cmd`` bodies).
    let mut boot_permission_policy: Option<Arc<permission::PermissionPolicy>> = None;
    // H-CHG-02: capture the enforcing gate's set-once LIVE-model cell (cycle-break),
    // filled once the orchestrator (owner of the live `session.model`) exists, so
    // the live `set_permission_mode` auto gate evaluates `dUe(wi())` against the
    // CURRENT model — mirrors the desktop composition root.
    let mut live_model_provider_cell: Option<
        Arc<std::sync::OnceLock<permission::LiveModelProvider>>,
    > = None;
    let perms: Arc<dyn PermissionGate> = {
        let mut rules = Vec::new();
        let mut mode = PermissionMode::Default;
        // Audit fix (#1): the project/enterprise bypassPermissions KILLSWITCH
        // (`disableBypassPermissionsMode`), sticky across tiers — mirrors desktop
        // (engine-desktop sets `policy.bypass_killswitch_active`). Without it a
        // settings `defaultMode:bypassPermissions` becomes an unguarded allow-all
        // on mobile, which has no interactive bypass-safety guard either.
        let mut bypass_disabled = false;
        // Auto-mode killswitch (`Bpa()`), sticky across tiers — mirrors desktop
        // (`policy.auto_mode_disabled`). Feeds both the boot mode-load downgrade
        // and the live `set_permission_mode` auto rejection.
        let mut auto_mode_disabled = false;
        // Audit fix (#12): `permissions.additionalDirectories`, unioned across
        // tiers, so an AcceptEdits write under a settings-declared extra dir
        // auto-allows (mirrors desktop's `.with_working_dirs`); empty ⇒ unchanged.
        let mut additional_working_dirs: Vec<std::path::PathBuf> = Vec::new();
        let proj = cwd.join(branding::DOT_DIR).join("settings.json");
        let user = cfg.lingxi_home.join("settings.json");
        // Audit fix (#6): also read the LocalSettings tier (`settings.local.json`),
        // LAST so its rules/defaultMode win — mirrors desktop. Mobile does not
        // PERSIST to it (no `.with_persist`), but a synced/checked-in
        // settings.local.json's deny/allow rules + defaultMode are now honored.
        let local = cwd.join(branding::DOT_DIR).join("settings.local.json");
        let mut sources: Vec<(std::path::PathBuf, permission::PermissionRuleSource)> = Vec::new();
        if user != proj {
            sources.push((user, permission::PermissionRuleSource::UserSettings));
        }
        sources.push((proj, permission::PermissionRuleSource::ProjectSettings));
        // `settings.local.json` is a distinct filename from both `settings.json`
        // paths, so it never collides with the dedup above — always read it last.
        sources.push((local, permission::PermissionRuleSource::LocalSettings));
        for (path, source) in sources {
            if let Ok(raw) = tokio::fs::read_to_string(&path).await {
                match permission::permission_rules_from_settings_json(&raw, source) {
                    Ok(mut r) => rules.append(&mut r),
                    Err(e) => tracing::warn!(
                        error = %e,
                        path = %path.display(),
                        "engine-mobile: skipping malformed settings permissions"
                    ),
                }
                if let Some(m) = permission::default_mode_from_settings_json(&raw) {
                    mode = m; // local settings read last → its defaultMode wins
                }
                if permission::bypass_permissions_disabled_from_settings_json(&raw) {
                    bypass_disabled = true; // sticky: any tier disabling wins
                }
                if permission::auto_mode_disabled_from_settings_json(&raw) {
                    auto_mode_disabled = true; // sticky: any tier disabling wins (Bpa)
                }
                // (P2-14) Scalar-override: a tier that declares the key overrides
                // (sources are ordered user → project → local, so local wins).
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if let Some(b) = v
                        .get("skipWebFetchPreflight")
                        .and_then(serde_json::Value::as_bool)
                    {
                        skip_web_fetch_preflight = b;
                    }
                    // (M-15) Scalar-override: a tier that declares the key overrides
                    // (user → project → local, so local wins).
                    if let Some(s) = v
                        .get("askUserQuestionTimeout")
                        .and_then(serde_json::Value::as_str)
                    {
                        ask_user_question_timeout = Some(s.to_string());
                    }
                    // (M-03) Scalar-override: a tier that declares the key
                    // overrides (user → project → local, so local wins).
                    if let Some(b) = v
                        .get("disableAgentView")
                        .and_then(serde_json::Value::as_bool)
                    {
                        disable_agent_view = b;
                    }
                }
                additional_working_dirs
                    .extend(permission::additional_directories_from_settings_json(&raw));
            }
        }
        // Filesystem roots so file-path CONTENT rules (`Edit(src/**)`,
        // `Read(./secrets/**)`) match the call's path. `dirs` is not a mobile dep,
        // so HOME comes from the env (absent on a sandboxed device ⇒ `None`).
        let roots = permission::FsRoots {
            cwd: cwd.clone(),
            home: std::env::var_os("HOME").map(std::path::PathBuf::from),
            lingxi_home: cfg.lingxi_home.clone(),
        };
        // Auto-mode availability gate — claude-code `xms` mode-load downgrade:
        // a resolved `auto` mode downgrades to `default` when unavailable (the
        // `disableAutoMode` killswitch or an auto-unsupported boot model). Local
        // breaker fresh at boot; provider `"firstParty"` (multi-provider mapping
        // deferred — see `permission::auto_gate`).
        if mode == PermissionMode::Auto {
            let (gated, _reason) = permission::apply_auto_mode_gate(
                mode,
                &permission::AutoGateInputs {
                    disabled_by_settings: auto_mode_disabled,
                    circuit_broken: false,
                    model: cfg.default_model.clone(),
                    provider: "firstParty".to_string(),
                },
            );
            mode = gated;
        }
        let mut policy = permission::PermissionPolicy::from_rules(mode, rules)
            .with_roots(roots)
            .with_working_dirs(additional_working_dirs);
        // Audit fix (#1): honor the bypassPermissions killswitch resolved above.
        policy.bypass_killswitch_active = bypass_disabled;
        // Auto-mode killswitch (`Bpa()`): the live `set_permission_mode` gate
        // refuses `auto` when any tier set `disableAutoMode: "disable"`.
        policy.auto_mode_disabled = auto_mode_disabled;
        let policy = Arc::new(policy);
        // Resolve active Read(deny) rules to search-exclude globs before the
        // policy moves into the gate (same as the desktop composition root).
        read_deny_exclude_globs = permission::read_deny_exclude_globs(&policy, &cwd);
        // Share the boot policy into `tool_ctx` for the prompt shell-expansion
        // gate (clone the `Arc` BEFORE `policy` moves into the gate below).
        boot_permission_policy = Some(policy.clone());
        // Grab the LIVE-model cell BEFORE coercing to `Arc<dyn PermissionGate>`;
        // filled once the orchestrator exists (below).
        let enforcing = permission::PolicyPermissionGate::new(policy, adapter_gate.clone());
        live_model_provider_cell = Some(enforcing.live_model_provider_handle());
        Arc::new(enforcing)
    };

    // (6) Hook executor + memory provider.
    //
    // P0.2: the hook executor is no longer the `noop_hook_executor()` stub — it
    // is the REAL `HookExecutorImpl` (mobile sibling of `engine_desktop::build`
    // §5.2 / §5.25), built from the settings hooks below so the Command / Prompt
    // hook arms run for real and the session-start `SessionStart` /
    // `InstructionsLoaded` lifecycle fires (step (9) below) dispatch against the
    // loaded hooks.
    //
    // (6a) HookRegistry — read settings.json hooks from project
    //      (`<cwd>/.lingxi/settings.json`) then user
    //      (`<lingxi_home>/settings.json`), project last so it wins on identical
    //      command registration (same precedence as desktop). The files are read
    //      via `tokio::fs` (NOT the workspace-constrained `FileSystem::read_file`)
    //      because `lingxi_home` may sit outside the orchestrator's cwd, exactly
    //      as desktop reads them. A missing or malformed file is skipped, never an
    //      error — the common (no-hooks) case registers nothing and stays a no-op.
    let mut hook_registry = hooks::HookRegistry::new();
    let project_settings_path = cwd.join(branding::DOT_DIR).join("settings.json");
    let user_settings_path = cfg.lingxi_home.join("settings.json");
    // On mobile `lingxi_home` is commonly `<cwd>/.claude`, so the user- and
    // project-settings paths can resolve to the SAME file. Desktop never
    // collides (lingxi_home = `~/.claude` ≠ cwd) and so has no dedup. Reading a
    // colliding path twice would `register()` every declared hook twice, so it
    // would fire twice per event — a parity divergence. De-dup to read each
    // distinct path ONCE; when they collide keep the Project tag (project is
    // read last so it wins precedence on differing paths, matching desktop).
    let mut settings_sources: Vec<(std::path::PathBuf, hooks::definition::HookSource)> = Vec::new();
    if user_settings_path != project_settings_path {
        settings_sources.push((user_settings_path, hooks::definition::HookSource::User));
    }
    settings_sources.push((
        project_settings_path,
        hooks::definition::HookSource::Project,
    ));
    // (H-BIN-12) Accumulate the CC 2.1.207 HTTP-hook security allowlists across
    // the SAME settings tiers, concat-deduped (CC merges these arrays across
    // sources). Stay `None` until a tier declares the key (⇒ no restriction); an
    // explicit `[]` sets `Some(empty)` (⇒ block ALL HTTP hooks for
    // allowedHttpHookUrls). Mobile has no `engine::settings::Settings::load`
    // seam, so read the keys directly like the `skipWebFetchPreflight` path.
    let mut allowed_http_hook_urls: Option<Vec<String>> = None;
    let mut http_hook_allowed_env_vars: Option<Vec<String>> = None;
    let concat_dedup_str_array =
        |acc: &mut Option<Vec<String>>, val: Option<&serde_json::Value>| {
            let Some(arr) = val.and_then(serde_json::Value::as_array) else {
                return;
            };
            let out = acc.get_or_insert_with(Vec::new);
            for s in arr.iter().filter_map(|x| x.as_str().map(String::from)) {
                if !out.contains(&s) {
                    out.push(s);
                }
            }
        };
    for (path, source) in settings_sources {
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
                    "engine-mobile: skipping malformed settings hooks"
                ),
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                concat_dedup_str_array(&mut allowed_http_hook_urls, v.get("allowedHttpHookUrls"));
                concat_dedup_str_array(
                    &mut http_hook_allowed_env_vars,
                    v.get("httpHookAllowedEnvVars"),
                );
            }
        }
    }
    let hook_registry = Arc::new(RwLock::new(hook_registry));

    // (6b) Build the real executor. Per the mobile runner-sourcing decision:
    //      - `with_process_runner(process, sandbox)` makes the Command arm run
    //        child processes through the SAME platform-sourced runner + sandbox
    //        the tools use (a device-jailed runner on Android, the host runner on
    //        iOS/CI). Both are required — the runner only accepts a
    //        `traits::SandboxedCommand`, which only the sandbox can mint.
    //      - `with_prompt_runner(ApiClientHookPromptRunner)` evaluates inline
    //        single-turn `prompt` hooks over the SAME `api_client` the
    //        orchestrator drives (shared provider routing / auth / telemetry).
    //      The `with_async_registry` + `with_agent_spawner` builders are
    //      DELIBERATELY OMITTED: mobile has no subagent spawner, and with no
    //      async/agent hook configured this is behavior-neutral (a non-blocking
    //      hook falls back to running synchronously; an `agent` hook returns a
    //      structured "not wired" error rather than spawning). The
    //      `RuntimeSpawner` is the posix-minimal `PosixRuntime` (only the omitted
    //      Agent/async arms consult it; the Command arm uses `process`).
    let hooks: Arc<hooks::HookExecutorImpl> = Arc::new(
        hooks::HookExecutorImpl::new(
            hook_registry.clone(),
            http.clone(),
            Arc::new(platform_posix_minimal::PosixRuntime::new())
                as Arc<dyn traits::RuntimeSpawner>,
        )
        .with_process_runner(process.clone(), sandbox.clone())
        .with_prompt_runner(Arc::new(orchestrator::ApiClientHookPromptRunner::new(
            api_client.clone(),
        )))
        // (H-BIN-12) Gate outbound HTTP-hook URLs + intersect the per-hook env
        // allowlist from the merged settings; `(None, None)` = no restriction.
        .with_http_hook_policy(allowed_http_hook_urls, http_hook_allowed_env_vars),
    );

    // P0.2: the production FFI entry points inject
    // `cfg.memory_provider = Some(orchestrator::prompt::real_provider())` so the
    // orchestrator loads the real `<cwd>/LINGXI.md` + `<lingxi_home>/LINGXI.md`
    // hierarchy into its system prompt (claude-code parity) and step (9)'s
    // `fire_instructions_loaded()` fires over those files. `None` (the default +
    // every host test) falls back to the empty `StaticMemoryProvider`, so a
    // default build loads NO memory and the host tests stay deterministic.
    let memory: Arc<dyn orchestrator::prompt::MemoryHierarchyProvider> = cfg
        .memory_provider
        .clone()
        .unwrap_or_else(|| Arc::new(StaticMemoryProvider::empty()));

    // (7) Assemble the mobile tool registry through the composition root. The
    //     device capabilities (camera / voice / share) come from `platform`;
    //     desktop-only seams (subagent / mcp / lsp / team / worktree-tool) are
    //     absent because `engine-mobile` does not link those tool crates.
    // P1-06: ONE per-session read-file-state registry (see engine-desktop
    // note) — cloned into the file tools' `BuiltinToolContext` and the SAME
    // `Arc` handed to the orchestrator via `.with_read_state_map(...)` below.
    let read_state_map = tool_api::read_file_state::new_read_file_state_map();
    // Bound (not inlined) so the SAME `Arc<SessionCwd>` can also be handed to
    // the orchestrator below via `.with_session_cwd(...)` (Task 5 — worktree
    // 206 session-cwd plumbing). Mobile never registers the worktree tool
    // (see above), so this cell never actually swaps today; wiring it keeps
    // the orchestrator's cwd source consistent with desktop and future-proofs
    // a mobile worktree tool without a second staleness bug to fix later.
    let session_cwd = SessionCwd::new(cwd.clone(), vec![cwd.clone()]);
    let tool_ctx = BuiltinToolContext {
        // FILE.B / P1-06: file tools share the ONE per-session read-state map
        // (see engine-desktop note).
        read_file_state: read_state_map.clone(),
        // Read(deny) → Grep/Glob search excludes, resolved from the local
        // `PermissionPolicy` built above (empty when no Read-deny rule ⇒
        // unchanged default).
        read_deny_exclude_globs,
        fs,
        bus: analytics_bus.clone(),
        process,
        sandbox,
        clock: clock.clone(),
        sandbox_runtime: SandboxRuntimeConfig::default(),
        // Mobile has no interactive `/sandbox` toggle (no live TUI); the frozen
        // `sandbox_runtime` above governs — full Android/iOS sandboxing intact.
        sandbox_enabled_override: None,
        // (P2-14) `settings.skipWebFetchPreflight`, read from the settings.json
        // tiers in the perms loop above (scalar-override, local wins).
        skip_web_fetch_preflight,
        // (M-15) `settings.askUserQuestionTimeout`, read from the settings.json
        // tiers in the perms loop above (scalar-override, local wins).
        ask_user_question_timeout,
        // RUNNER ↔ AVAILABILITY COUPLING (#5): the live `SandboxRuntimeRunner`
        // (domain/proxy/policy enforcement) requires host forward proxies +
        // bwrap/seatbelt — desktop-OS primitives a phone (iOS/Android,
        // `platform-posix-minimal`) does NOT have. So mobile keeps the legacy
        // wrap AND reports `sandbox_available: false`, which makes
        // `should_use_sandbox` short-circuit to `NoSandbox`
        // (`sandbox/decision.rs:64`) BEFORE the runner is ever consulted — the
        // legacy wrap is therefore inert here, not an under-enforcement gap. The
        // `debug_assert!` below pins the invariant: if a future capable host flips
        // `sandbox_available` to `true`, it MUST also inject a live runner (the
        // legacy wrap can only express `--unshare-net`/`--share-net`, never the
        // domain/proxy enforcement the desktop runtime provides).
        sandbox_runner: tool_api::default_sandbox_runner(),
        permission_mode: PermissionMode::Default,
        // (#3 shell-expansion) Share the SAME boot policy the model-facing gate
        // enforces as the base for embedded `!`cmd`` bodies in prompt commands.
        // Always `Some` here (the `perms` block above is unconditional).
        permission_policy: boot_permission_policy
            .clone()
            .expect("boot permission policy is built unconditionally above"),
        sandbox_available: false,
        session_cwd: session_cwd.clone(),
        // Worktree 206 parity (Task 8): a fresh, empty (`None`) session
        // record. Mobile never registers the worktree tool (see the
        // `session_cwd` note above), so this cell stays inert in production —
        // wired for shape-consistency with desktop.
        worktree_session: tool_api::worktree_session::new_worktree_session_cell(),
        platform: if cfg!(target_os = "macos") {
            SandboxPlatform::Mac
        } else {
            SandboxPlatform::Linux
        },
        http: http.clone(),
        provider: tool_provider,
        default_model: orch_cfg.model.clone(),
        // Mobile has no settings.json-backed WebSearch config provider (desktop
        // injects `DesktopWebSearchConfigProvider`); WebSearch falls back to its
        // built-in defaults here. `None` matches the tool-api test-support host.
        web_search_config: None,
        worktree,
        subagent_spawner: None,
        agent_name_registry: None,
        task_registry: None,
        mailbox_router: None,
        budget_enforcer: None,
        // Mobile has no coordinator runtime; fork-subagent gate sees non-coordinator.
        coordinator_mode: None,
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
        // P3: thread the Android Shell gate + prompt carrier from MobileConfig.
        // `None` on iOS and desktop (cfg.android_shell defaults to None).
        android_shell: cfg.android_shell.clone(),
        // P4: thread the Android Git gate + workspace carrier from MobileConfig.
        // `None` on iOS and desktop (cfg.android_git defaults to None).
        android_git: cfg.android_git.clone(),
        // P4: thread the Android Git network secret (token + CA dir) from
        // MobileConfig. `None` on iOS and desktop; T10 populates from android-aar.
        android_git_secret: cfg.android_git_secret.clone(),
        // The V2 task tools' BLOCKING TaskCreated/TaskCompleted hooks are a
        // desktop composition-root wiring; mobile leaves them unwired (the tool
        // path is then non-blocking, matching the registry firer behavior).
        task_lifecycle_hooks: None,
    };
    // #5 invariant: mobile has no live sandbox runtime, so sandboxing must stay
    // unavailable — otherwise `should_use_sandbox` would route commands through
    // the under-enforcing legacy wrap. Enabling sandboxing on a future capable
    // host REQUIRES injecting a live runner alongside flipping this flag.
    debug_assert!(
        !tool_ctx.sandbox_available,
        "mobile sets sandbox_available=false because it has no live SandboxRuntimeRunner; \
         enabling sandboxing requires injecting one (see the sandbox_runner coupling note)"
    );
    // Audit fix (#14): build a disk-backed Skill loader so the mobile Skill tool
    // resolves on-disk `.lingxi/commands` / `.lingxi/skills` under the device's
    // app-private root (`lingxi_home` = `<app_files_root>/.claude`). `home` = cwd
    // so `home/.claude` resolves to the same app-private `.claude` as lingxi_home
    // (the loaders dedup by name across project/user/managed layers). No
    // session id at build time on mobile (the session is per-connection), so
    // `${LINGXI_SESSION_ID}` is left un-substituted — matching the loader's None
    // path. The loader owns its own registry, so this needs no reordering of the
    // composition below.
    let skill_loader: Arc<dyn tool_skill::skill::SkillLoader> = Arc::new(
        crate::skill_loader::MobileDiskSkillLoader::load_from_disk(
            &cwd,
            &cfg.lingxi_home,
            &cwd,
            None,
        )
        .await,
    );
    // (#3 shell-expansion) Build the shared prompt shell-expansion provider from
    // `tool_ctx` (carrying the base `permission_policy` + process/sandbox seams)
    // BEFORE `tool_ctx` is moved into the tool registry below, then chain it onto
    // the dispatcher so mobile `/commit` … expand their embedded `!`git …``
    // bodies identically to desktop. Mobile reports `sandbox_available:false`, so
    // `should_use_sandbox` short-circuits to `NoSandbox` and the expansion runs
    // via the plain `ProcessRunner` — consistent with mobile's own Bash tool.
    let shell_expansion_provider = tool_skill::build_prompt_shell_provider(&tool_ctx);
    let tools = Arc::new(mobile_tool_registry_with_skill_loader(
        tool_ctx,
        skill_loader,
    ));

    // P0.1 ACTIVATION on mobile (gated, default OFF) — the same gate as desktop,
    // `LINGXI_MEMDIR_PREFETCH`. When truthy, wire the memdir-backed memory
    // selector so relevant `<lingxi_home>/memdir` entries surface each turn via a
    // Haiku-class side query (a `ProviderSideQueryClient` over the device HTTP
    // transport + `cfg.api_key`, independent of the multi-provider turn client).
    // Unset/false ⇒ no prefetch ⇒ surfacing inert ⇒ the locked mobile fixtures
    // stay byte-identical. On a device the env var is typically unset, so this is
    // off unless the host app explicitly sets it. A missing/unusable key makes the
    // side query fail → empty surfaced set (never breaks a turn).
    let memdir_prefetch =
        if traits::env::is_env_truthy(std::env::var("LINGXI_MEMDIR_PREFETCH").ok().as_deref()) {
            // `cfg.lingxi_home` is the device `.claude` dir; the helper re-appends
            // `.lingxi/memdir`, so pass its PARENT as `home` ⇒ `<lingxi_home>/memdir`.
            let home = cfg
                .lingxi_home
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| cwd.clone());
            Some(orchestrator::prompt::build_memdir_prefetch_from_anthropic(
                cfg.api_key.clone(),
                Some(cfg.api_base.clone()),
                http.clone(),
                Arc::new(platform_posix_minimal::runtime::PosixRuntime::new())
                    as Arc<dyn traits::RuntimeSpawner>,
                &home,
            ))
        } else {
            None
        };

    // Audit fix (#3): autocompaction parity with desktop. Build the real
    // CompactionOrchestrator (threshold 150_000 tokens — the M3 Anthropic prod
    // context lock) backed by a forked summary side-query over the SAME device
    // HTTP transport + cfg.api_key the memdir-prefetch uses; nothing here needs a
    // desktop-only primitive. Without this the orchestrator's compaction stays
    // None, the proactive `maybe_compact_before_call` is a strict no-op, and a
    // long mobile session fails at the wire on context-window overflow with no
    // summary recovery. The same `cache_safe_slot` is handed to BOTH the forked
    // summarizer and the orchestrator so the turn loop's per-call snapshot is what
    // the summary call replays. (This is the ordinary M3 autocompaction layer, NOT
    // the flag-gated CONTEXT_COLLAPSE/REACTIVE_COMPACT path.) Built before
    // `orch_cfg` is moved into the orchestrator so it can read `orch_cfg.model`.
    let cache_safe_slot = Arc::new(sidequery::CacheSafeParamsSlot::new());
    let compaction_side_query: Arc<dyn sidequery::SideQueryClient> = Arc::new(
        sidequery::ProviderSideQueryClient::from_service(api_service.clone()),
    );
    let forked_runner = Arc::new(
        sidequery::ForkedAgentRunner::new()
            .with_side_query_client(compaction_side_query, orch_cfg.model.clone()),
    );
    let compactor = Arc::new(compaction::CompactionOrchestrator::with_autocompactor(
        compaction::Autocompactor::with_forked_runner(forked_runner, cache_safe_slot.clone()),
        150_000,
    ));

    let mut orch_inner = ConversationOrchestrator::new_with_streaming(
        orch_cfg,
        api_client,
        streaming_api,
        tools,
        hooks,
        perms,
        output,
        memory,
        // `cwd` is reused below by the batch-8 registration, so clone here.
        cwd.clone(),
    )
    // P0.2: attach the SAME `HookRegistry` the executor reads so `list_hooks`
    // reports the loaded settings hooks (the executor fires against it; this
    // exposes it for inspection — mobile sibling of desktop's
    // `.with_hook_registry(hook_registry)`).
    .with_hook_registry(hook_registry)
    // FIX A: hand the orchestrator the resolved claude-home so its hook payloads
    // carry a deterministically-computed `transcript_path` (claude-code
    // `getTranscriptPathForSession`) even though no `JsonlWriter` is wired —
    // mobile sibling of desktop's `.with_config_home(cfg.lingxi_home.clone())`.
    .with_config_home(cfg.lingxi_home.clone())
    // Audit fix (#15): the orchestrator shares the ONE AnalyticsBus (so its
    // events ride the same sink as the ApiService + tools) + the session
    // CostTracker (desktop parity; accumulates the running session cost total).
    .with_analytics_bus(analytics_bus)
    .with_cost_tracker(cost_tracker)
    // Audit fix (#3): attach the compactor + the shared cache-safe slot so the
    // turn loop autocompacts before context-window overflow (desktop parity).
    .with_compaction(compactor)
    .with_cache_safe_slot(cache_safe_slot)
    // Audit fix (#13): per-turn V2 `<task-reminder>` over the file-backed
    // TodoStore (tool_task IS registered on mobile) — mirror of desktop.
    .with_todo_reminder_tasks(Arc::new(orchestrator::TodoStoreReminderTasks::new()))
    // P1-06: share the ONE `readFileState` map with the file tools (created
    // above) so post-compact file restore + staleness consumers see a tool's
    // `readFileState.set` — mirror of desktop.
    .with_read_state_map(read_state_map)
    // Task 5 (worktree 206 session-cwd plumbing): share the SAME
    // `Arc<SessionCwd>` the tool context reads, so the system prompt's env
    // block and the conditional-rules memory cache stay consistent with the
    // tool-facing cwd source (mobile mirror of desktop's
    // `.with_session_cwd(session_cwd)`; inert today — see the binding note
    // above).
    .with_session_cwd(session_cwd);
    // P0.1 (gated): attach the memdir prefetch when enabled above.
    if let Some(prefetch) = memdir_prefetch {
        orch_inner = orch_inner.with_memory_prefetch(prefetch);
    }
    let orch = Arc::new(orch_inner);

    // H-CHG-02: wire the enforcing gate's live `set_permission_mode` auto gate to
    // the LIVE `session.model` (mutated by `/model` switches / resume), so a
    // runtime switch to `auto` on an auto-unsupported model is rejected
    // (`dUe(wi())` — claude-code `Nle`). Non-blocking `try_lock`; a contended read
    // returns `None` and the model check is skipped (fail-open). Desktop mirror.
    if let Some(cell) = live_model_provider_cell.as_ref() {
        let session = orch.session();
        let _ = cell.set(std::sync::Arc::new(move || {
            session.try_lock().ok().map(|s| s.model.clone())
        }));
    }

    // (8) Command registry through the mobile composition root.
    let handle: Arc<dyn OrchestratorHandle> = orch.clone();
    // TPM-C (mobile step 2): seed the initial model_profile from a
    // profile-qualified default_model.  SessionState::empty starts model_profile
    // at None; this is a no-op when default_model is a bare id.
    if let Some(profile) = default_model_profile.as_deref() {
        if let Err(e) = handle.switch_model(&default_model_id, Some(profile)).await {
            tracing::warn!(error = %e, "failed to seed default model profile");
        }
    }
    orch.spawn_startup_responses_websocket_prewarm();
    // Pre-create the shared registry slot so batch-8's `/reload-skills` handler
    // and the dispatcher observe ONE command set; fill it once the builtins are
    // assembled, then hand the SAME `Arc` to the dispatcher.
    let shared_command_registry: Arc<RwLock<command_api::CommandRegistry>> =
        Arc::new(RwLock::new(command_api::CommandRegistry::new()));
    let mut reg = mobile_command_registry(handle.clone(), auth.clone());
    // Batch 8 (`/fork`, `/goal`, `/recap`, `/reload-skills`, `/skill-doctor`,
    // `/stop`): wired here in the uniffi composition root because it needs the
    // shared `Arc<tokio::sync::RwLock<CommandRegistry>>` (tokio is uniffi-only in
    // this crate's default lib build). Mobile has no on-disk custom-skill
    // discovery layer, so no managed dir / no additional dirs / safe-mode off.
    command_core::register_core_batch_8(
        &mut reg,
        handle,
        shared_command_registry.clone(),
        cwd.clone(),
        cfg.lingxi_home.clone(),
        None,
        cwd.clone(),
        Vec::new(),
        false,
        disable_agent_view,
    );
    *shared_command_registry.write().await = reg;
    let dispatcher = RegistrySlashDispatcher::new(shared_command_registry.clone())
        .with_skill_usage_home(cfg.lingxi_home.clone())
        // (#3) Real embedded-shell expansion for markdown/plugin + builtin
        // `InjectMessage` prompts. Non-MCP only.
        .with_shell_expansion(shell_expansion_provider);

    // (9) Session lifecycle fires (P0.2 — mobile sibling of `engine_desktop::build`
    //     §7 / §7.1). Fire `SessionStart` then `InstructionsLoaded` now that the
    //     orchestrator + the real hook registry are fully wired:
    //     - `fire_session_start("startup")`: `build_mobile` assembles exactly one
    //       fresh session per call, so the byte-faithful `source` is `"startup"`
    //       (claude-code `utils/hooks.ts` SessionStart path).
    //     - `fire_instructions_loaded()`: fires once per eager LINGXI.md /
    //       `LINGXI.local.md` the memory provider yields (load_reason
    //       `session_start`), exactly as desktop. With the default empty provider
    //       this is a no-op over zero files; with the injected `real_provider()`
    //       it fires over the real hierarchy.
    //     Both are best-effort — each discards the hook aggregate, so a failing /
    //     malformed lifecycle hook never breaks boot, and each is a strict no-op
    //     when no matching hook is registered (the common case). No matching
    //     `SessionEnd` is fired here: like desktop, `build_mobile` returns the
    //     runtime and the FFI host drops it with no hook-capable teardown seam.
    let session_start = orch.fire_session_start("startup").await;
    if session_start.reload_skills {
        let handler = command_core::reload_skills::ReloadSkillsHandler::with_all_roots(
            shared_command_registry.clone(),
            cwd.clone(),
            cfg.lingxi_home.clone(),
            None,
            cwd.clone(),
            Vec::new(),
            false,
        );
        if let Some(parsed) = parse_slash_command("/reload-skills") {
            let _ = handler.handle(&parsed).await;
        }
    }
    orch.fire_instructions_loaded().await;

    Ok(MobileRuntime {
        orchestrator: orch,
        dispatcher,
        auth,
        permission_gate: adapter_gate,
        listener,
        event_sink,
        oauth_supported,
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
    /// (`<lingxi_home>/projects/<sanitized cwd>/*.jsonl`). Captured from the
    /// `MobileConfig` so `submit(ListSessions)` can read the on-disk catalog
    /// without re-deriving it (SESSIONS/HISTORY).
    lingxi_home: std::path::PathBuf,
    /// The session enumerator's `cwd` key (its sanitized form selects the project
    /// subdir under `lingxi_home/projects/`). Captured from the `MobileConfig`.
    session_cwd: String,
    /// The platform filesystem handle the JSONL reader reads each session file
    /// through (`list_recent_sessions`' `Arc<dyn FileSystem>` argument). The SAME
    /// `fs` the orchestrator's tools use — captured from the `Platform` so the
    /// session listing reads through the device's real backend.
    fs: Arc<dyn traits::FileSystem>,
    /// The deterministic build recipe, captured so the cron firing path
    /// ([`Self::run_due_cron_now`]) can rebuild a FRESH, throwaway
    /// [`MobileRuntime`] per fired job (an isolated session that never pollutes
    /// the user's live conversation). Also carries `cwd`, which resolves the
    /// `<cwd>/.lingxi/scheduled_tasks.json` the cron FFI reads/writes.
    firer_cfg: MobileConfig,
    /// The aggregate device `Platform`, captured alongside `firer_cfg` so the
    /// cron firing path can call `build_mobile_inner` (and reach `filesystem()` /
    /// `clock()`) without re-deriving the device handles.
    firer_platform: Arc<dyn Platform>,
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
            ClientCommand::SendPrompt { text, turn_id, .. } => {
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
                        sink.emit(client_adapter::map_orchestrator_error(&err))
                            .await;
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

            ClientCommand::SetPermissionMode { mode } => {
                let handle: Arc<dyn OrchestratorHandle> = self.inner.orchestrator.clone();
                handle
                    .set_permission_mode(&mode)
                    .await
                    .map_err(|e| ClientError::Rejected {
                        message: format!("set_permission_mode failed: {e}"),
                    })?;
                let active = handle.permission_mode().await.unwrap_or(mode);
                self.event_sink
                    .emit(ClientEvent::PermissionModeChanged { mode: active })
                    .await;
                Ok(())
            }

            // ── Model ──────────────────────────────────────────────────────
            ClientCommand::SetModel { model } => {
                let handle: Arc<dyn OrchestratorHandle> = self.inner.orchestrator.clone();
                let listings = handle.list_model_listings().await;
                let (model_id, profile) = traits::parse_model_ref(&model, &listings);
                handle
                    .switch_model(&model_id, profile.as_deref())
                    .await
                    .map_err(|e| ClientError::Internal {
                        message: format!("switch_model failed: {e}"),
                    })?;
                self.event_sink
                    .emit(ClientEvent::ModelChanged { model: model_id })
                    .await;
                Ok(())
            }
            ClientCommand::ListModels => {
                self.emit_listing(ProtocolListingKind::Models).await;
                Ok(())
            }

            // ── Slash commands ──────────────────────────────────────────────
            // Display-only (`type: "local"`) commands surface as a TextDelta; a
            // `type: "prompt"` command (`/loop`, Markdown/Plugin) runs its
            // expanded prompt AS a turn through the SAME streaming path as
            // `SendPrompt` (claude-code injects the expanded prompt as the user
            // message), so a typed `/loop` actually schedules + executes.
            ClientCommand::RunSlashCommand { raw } => {
                match self.inner.dispatcher.dispatch(&raw).await {
                    traits::SlashDispatchResult::RunAsTurn { prompt } => {
                        let cancel = CancellationToken::new();
                        *self.active_cancel.lock().await = Some(cancel.clone());
                        let wrapper = TurnWrapper::new(self.event_sink.clone());
                        wrapper.emit_turn_started(None).await;
                        let orch = self.inner.orchestrator.clone();
                        let sink = self.event_sink.clone();
                        self.runtime.spawn(async move {
                            if let Err(err) =
                                orch.run_turn_streaming_with_cancel(&prompt, cancel).await
                            {
                                sink.emit(client_adapter::map_orchestrator_error(&err))
                                    .await;
                            }
                        });
                    }
                    traits::SlashDispatchResult::Handled { display }
                    | traits::SlashDispatchResult::Unknown { display, .. } => {
                        self.event_sink
                            .emit(ClientEvent::TextDelta { text: display })
                            .await;
                    }
                    traits::SlashDispatchResult::NotASlashCommand => {
                        self.event_sink
                            .emit(ClientEvent::TextDelta {
                                text: format!("not a slash command: {raw}"),
                            })
                            .await;
                    }
                }
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
                // Audit (secure-storage): on a build with no persisting secure
                // store (mobile currently wires the PlainTextSecureStorage stub),
                // an OAuth exchange would authenticate but fail to persist its
                // tokens with a cryptic `BackendUnavailable`. Short-circuit with a
                // clear, actionable message instead. API-key auth needs no /login.
                // Lifts automatically once a native Keychain/Keystore store is
                // injected (then `oauth_supported` is true). §11 / Plan-17 follow-up.
                if !self.inner.oauth_supported {
                    self.event_sink
                        .emit(ClientEvent::Error {
                            kind: client_protocol::events::ErrorKindDto::Internal,
                            message: "OAuth login is not yet supported on this platform \
                                      (no secure credential store); configure an API key instead."
                                .to_string(),
                        })
                        .await;
                    self.event_sink
                        .emit(ClientEvent::AuthState {
                            state: lower_auth_state(self.inner.auth.current_user().await),
                        })
                        .await;
                    return Ok(());
                }
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
                self.event_sink.emit(ClientEvent::AuthState { state }).await;
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
                handle
                    .clear_session()
                    .await
                    .map_err(|e| ClientError::Internal {
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
            // (`<lingxi_home>/projects/<sanitized cwd>/*.jsonl`) via the shared
            // `session::jsonl::list_recent_sessions`, lowers each row through the
            // shared `client_adapter::lower_session_metadata`, and replies with a
            // `SessionList` event — the same listing surface the bridge-server
            // router uses (decision §0.2). An empty / missing catalog replies with
            // an empty list (the loader's `EmptyDirectory` is not an error here —
            // it is "no resumable sessions yet").
            ClientCommand::ListSessions { limit } => {
                let limit = limit.map_or(DEFAULT_SESSION_LIST_LIMIT, |l| l as usize);
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
                handle
                    .clear_session()
                    .await
                    .map_err(|e| ClientError::Internal {
                        message: format!("new session (clear_session) failed: {e}"),
                    })?;
                if let Some(model) = model {
                    let listings = handle.list_model_listings().await;
                    let (model_id, profile) = traits::parse_model_ref(&model, &listings);
                    handle
                        .switch_model(&model_id, profile.as_deref())
                        .await
                        .map_err(|e| ClientError::Internal {
                            message: format!("new session model switch failed: {e}"),
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
                    &self.lingxi_home,
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
                        replayed
                            .state
                            .active_goal
                            .clone()
                            .map(|goal| traits::ActiveGoalSnapshot {
                                condition: goal.condition,
                                set_at: goal.set_at,
                                last_reason: goal.last_reason,
                            }),
                        replayed.handle_runtime_snapshot(),
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
                tracing::debug!(
                    ?other,
                    "engine-mobile: command not routed by submit in the foundation"
                );
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
    /// Reads `<lingxi_home>/projects/<sanitized cwd>/*.jsonl` via the shared
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
            &self.lingxi_home,
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
                // Curate to the "latest few" per provider instead of flooding the
                // client with the full assembled catalog (~hundreds of ids — every
                // preset is injected into the live config by `provider_config::assemble`).
                // Mobile lacks the TUI's availability maps, so this trims to the
                // shared `is_curated_model` whitelist (keeping the current model);
                // `[Connect]` gating + grouping stays a TUI/structured-DTO concern.
                let available = handle.list_available_models().await;
                let listings = handle.list_model_listings().await;
                let current = handle.get_status_snapshot().await.model;
                let models = traits::curated_model_names(&listings, &available, &current);
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
                let hooks = handle
                    .list_hooks()
                    .await
                    .iter()
                    .map(lower_hook_info)
                    .collect();
                self.event_sink.emit(ClientEvent::Hooks { hooks }).await;
            }
            ProtocolListingKind::Agents => {
                let agents = handle
                    .list_agents()
                    .await
                    .iter()
                    .map(lower_agent_info)
                    .collect();
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
                tracing::debug!(
                    ?kind,
                    "engine-mobile: listing kind unhandled in the foundation"
                );
            }
        }
    }
}

/// Lower an `Option<LoginInfo>` to the auth-state DTO (the inverse copy of the
/// bridge-server router's helper — kept private to the shared host so iOS /
/// Android cannot drift).
fn lower_auth_state(
    info: Option<traits::auth::LoginInfo>,
) -> client_protocol::listings::AuthStateDto {
    match info {
        Some(li) => client_protocol::listings::AuthStateDto::SignedIn {
            email: li.email,
            org_id: li.org_id,
        },
        None => client_protocol::listings::AuthStateDto::SignedOut,
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Cron firing — the Android background-scheduler bridge.
//
// The desktop `cron::CronScheduler` 60s tick loop is unavailable on mobile (no
// long-lived daemon, and the mobile engine binds no `TaskRegistry` / subagent
// spawner). Instead the Android foreground service — woken by an exact
// `AlarmManager` alarm — calls `run_due_cron_now()` to evaluate
// `<cwd>/.lingxi/scheduled_tasks.json` ONCE and fire whatever is due, then
// `next_cron_fire_time()` to arm the next alarm. Due-detection + bookkeeping is
// `cron::run_due` (1:1 with the desktop tick loop); firing is a fresh, throwaway
// orchestrator turn. Permission strategy is claude-code parity: the fired turn
// inherits the session permission context via the orchestrator's existing
// `PolicyPermissionGate` (settings rules + defaultMode) — no special escalation.
// ───────────────────────────────────────────────────────────────────────────

/// Per-job wall-clock budget for a fired cron turn. A turn that parks (e.g. on a
/// permission `Ask` with no interactive answerer in a headless run) is abandoned
/// after this so the firing pass makes progress — faithful to claude-code's
/// "never silently escalate for cron" stance (a prompting tool ends the job).
const CRON_TURN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Terminal status of one fired cron job, lowered for the foreign host.
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone)]
pub enum CronFireStatusDto {
    /// The job's turn completed.
    Ok,
    /// The job's turn failed (or timed out); carries a log-safe message.
    Failed {
        /// Human-readable failure detail.
        message: String,
    },
}

/// One fired-job record the Android service turns into a result notification.
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct FiredCronJobDto {
    /// The cron job id that fired.
    pub id: String,
    /// The prompt that was run.
    pub prompt: String,
    /// The final assistant text, if the turn produced any.
    pub result_text: Option<String>,
    /// Terminal status.
    pub status: CronFireStatusDto,
}

/// A persisted cron job lowered for the Android management UI.
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct CronTaskDto {
    /// Stable 9-char job id.
    pub id: String,
    /// 5-field cron expression (local time).
    pub cron: String,
    /// Prompt run at each fire.
    pub prompt: String,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: u64,
    /// Last fire time, epoch milliseconds (absent until the job first fires).
    pub last_fired_at_ms: Option<u64>,
    /// `true` = recurring; `false` = one-shot.
    pub recurring: bool,
    /// Next fire, epoch milliseconds (absent for an impossible expression).
    pub next_fire_ms: Option<u64>,
    /// Human-readable schedule (e.g. "every day at 9:00am").
    pub human: String,
}

/// A discarding [`ClientEventListener`] that captures only assistant text, so a
/// headless cron turn's final message can be surfaced in a notification without
/// streaming anything to the user's live UI.
struct CapturingListener {
    text: Arc<Mutex<String>>,
}

#[async_trait]
impl ClientEventListener for CapturingListener {
    async fn on_event(&self, event: ClientEvent) {
        if let ClientEvent::TextDelta { text } = event {
            self.text.lock().await.push_str(&text);
        }
    }
}

/// A no-op [`PermissionRequestSink`]: a headless cron turn has no interactive
/// answerer, so an outbound request is dropped (it parks until the throwaway gate
/// is dropped, which fail-closed resolves it `Deny`). The core
/// allow/deny/defaultMode policy still binds via the orchestrator's
/// `PolicyPermissionGate` — pre-allowed tools run (claude-code parity).
struct NoopPermissionSink;

#[async_trait]
impl PermissionRequestSink for NoopPermissionSink {
    async fn emit_request(&self, _request: PermissionRequestDto) {}
}

/// A [`cron::CronJobFirer`] that runs a due job as a FRESH, throwaway
/// orchestrator turn. Each fire builds an isolated [`MobileRuntime`] (its own
/// empty session) from the captured build recipe, runs the prompt to completion
/// capturing the assistant text, then drops the runtime — so a cron run never
/// appends to the user's live transcript nor streams to their UI.
struct MobileTurnFirer {
    cfg: MobileConfig,
    platform: Arc<dyn Platform>,
}

#[async_trait]
impl cron::CronJobFirer for MobileTurnFirer {
    async fn fire(&self, _id: &str, prompt: &str) -> Result<String, String> {
        let captured = Arc::new(Mutex::new(String::new()));
        let listener: Arc<dyn ClientEventListener> = Arc::new(CapturingListener {
            text: captured.clone(),
        });
        let sink: Arc<dyn PermissionRequestSink> = Arc::new(NoopPermissionSink);
        let rt = build_mobile_inner(
            self.cfg.clone(),
            self.platform.clone(),
            listener,
            sink,
            None,
        )
        .await
        .map_err(|e| e.to_string())?;

        let run = rt.orchestrator.run_turn_streaming(prompt);
        let result = match tokio::time::timeout(CRON_TURN_TIMEOUT, run).await {
            Ok(Ok(_outcome)) => Ok(captured.lock().await.clone()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("cron turn timed out".to_string()),
        };
        // `rt` drops here → the throwaway session + its permission gate tear down.
        result
    }
}

/// Compute a task's next fire (epoch ms) from its cron string + anchor
/// (`lastFiredAt ?? createdAt ?? now`). `None` for an unparseable / impossible
/// expression.
fn task_next_fire_ms(
    cron: &str,
    created_at_ms: u64,
    last_fired_at_ms: Option<u64>,
    now: std::time::SystemTime,
) -> Option<u64> {
    let schedule = cron::parse_cron(cron).ok()?;
    let anchor = last_fired_at_ms
        .filter(|ms| *ms > 0)
        .map(|ms| std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms))
        .unwrap_or_else(|| {
            if created_at_ms > 0 {
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(created_at_ms)
            } else {
                now
            }
        });
    schedule
        .next_match_after(anchor)
        .and_then(|st| st.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

// The cron FFI surface — async UniFFI exports driven on the handle-owned runtime
// (same `async_runtime = "tokio"` contract as `submit`). A separate impl block so
// the cron methods read as one unit; UniFFI supports multiple exported blocks.
#[cfg_attr(feature = "uniffi", uniffi::export(async_runtime = "tokio"))]
impl MobileEngineHandle {
    /// Evaluate the persisted cron tasks file ONCE and fire every due job — the
    /// Android foreground-service entry. Reuses `cron::run_due_jobs` (desktop-1:1
    /// due-detection + bookkeeping) with a [`MobileTurnFirer`]. Returns one row
    /// per fired job for the service's result notifications.
    pub async fn run_due_cron_now(&self) -> Vec<FiredCronJobDto> {
        let path = cron::tasks_file::scheduled_tasks_path(&self.firer_cfg.cwd);
        let fs = self.firer_platform.filesystem();
        let clock = self.firer_platform.clock();
        let firer = MobileTurnFirer {
            cfg: self.firer_cfg.clone(),
            platform: self.firer_platform.clone(),
        };
        cron::run_due_jobs(
            &path,
            fs,
            clock,
            &firer,
            Some(cron::default_recurring_max_age()),
        )
        .await
        .into_iter()
        .map(|f| FiredCronJobDto {
            id: f.id,
            prompt: f.prompt,
            result_text: f.result_text,
            status: match f.status {
                cron::FireStatus::Ok => CronFireStatusDto::Ok,
                cron::FireStatus::Failed(message) => CronFireStatusDto::Failed { message },
            },
        })
        .collect()
    }

    /// Earliest next fire across all persisted enabled jobs, epoch milliseconds,
    /// or `None` if there are no jobs / none ever fire again. The Android
    /// scheduler arms its next exact alarm at this instant.
    pub async fn next_cron_fire_time(&self) -> Option<u64> {
        let path = cron::tasks_file::scheduled_tasks_path(&self.firer_cfg.cwd);
        cron::next_fire_epoch_ms(
            &path,
            self.firer_platform.filesystem(),
            self.firer_platform.clock(),
        )
        .await
    }

    /// List the persisted cron jobs for the management UI (each with its computed
    /// next fire + human schedule). A missing / unparseable file lists nothing.
    pub async fn cron_list(&self) -> Vec<CronTaskDto> {
        let fs = self.firer_platform.filesystem();
        let Ok(content) = cron::tasks_file::read_tasks_body(fs.as_ref(), &self.firer_cfg.cwd).await
        else {
            return Vec::new();
        };
        let now = self.firer_platform.clock().now();
        cron::tasks_file::parse_tasks(&content)
            .tasks
            .into_iter()
            .map(|t| CronTaskDto {
                human: tool_cron::schedule_cron::cron_to_human(&t.cron),
                next_fire_ms: task_next_fire_ms(&t.cron, t.created_at, t.last_fired_at, now),
                id: t.id,
                cron: t.cron,
                prompt: t.prompt,
                created_at_ms: t.created_at,
                last_fired_at_ms: t.last_fired_at,
                recurring: t.recurring.unwrap_or(false),
            })
            .collect()
    }

    /// Create a durable cron job from the UI: validate the expression, mint a
    /// `d`+base36 id (the SAME format as the `CronCreate` tool, no on-disk drift),
    /// and append it to `scheduled_tasks.json`. Returns the created row.
    ///
    /// # Errors
    /// [`MobileEngineError::Internal`] on an invalid cron expression or a write
    /// failure.
    pub async fn cron_create(
        &self,
        cron_expr: String,
        prompt: String,
        recurring: bool,
    ) -> Result<CronTaskDto, MobileEngineError> {
        cron::parse_cron(&cron_expr)
            .map_err(|e| MobileEngineError::Internal(format!("invalid cron expression: {e}")))?;
        let fs = self.firer_platform.filesystem();
        // Serialize against a concurrent firing pass's write-back (lost-update guard).
        let _process_guard = cron::lock_cron_file().await;
        let _file_guard = cron::tasks_file::lock_scheduled_tasks(fs.as_ref(), &self.firer_cfg.cwd)
            .await
            .map_err(|e| MobileEngineError::Internal(format!("lock scheduled_tasks.json: {e}")))?;
        let mut doc =
            match cron::tasks_file::read_tasks_body(fs.as_ref(), &self.firer_cfg.cwd).await {
                Ok(body) => cron::tasks_file::parse_tasks(&body),
                Err(_) => cron::tasks_file::ScheduledTasks::default(),
            };
        let now = self.firer_platform.clock().now();
        let now_ms = now
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let task = cron::tasks_file::CronTask {
            id: tool_cron::schedule_cron::generate_cron_task_id(),
            cron: cron_expr,
            prompt,
            created_at: now_ms,
            last_fired_at: None,
            recurring: Some(recurring),
            permanent: None,
        };
        doc.tasks.push(task.clone());
        cron::tasks_file::write_tasks_body(
            fs.as_ref(),
            &self.firer_cfg.cwd,
            &cron::tasks_file::serialize_tasks(&doc),
        )
        .await
        .map_err(|e| MobileEngineError::Internal(format!("write scheduled_tasks.json: {e}")))?;
        Ok(CronTaskDto {
            human: tool_cron::schedule_cron::cron_to_human(&task.cron),
            next_fire_ms: task_next_fire_ms(&task.cron, now_ms, None, now),
            id: task.id,
            cron: task.cron,
            prompt: task.prompt,
            created_at_ms: task.created_at,
            last_fired_at_ms: None,
            recurring,
        })
    }

    /// Delete a cron job by id. Returns `true` iff a job was removed.
    pub async fn cron_delete(&self, id: String) -> bool {
        let fs = self.firer_platform.filesystem();
        // Serialize against a concurrent firing pass's write-back (lost-update guard).
        let _process_guard = cron::lock_cron_file().await;
        let Ok(_file_guard) =
            cron::tasks_file::lock_scheduled_tasks(fs.as_ref(), &self.firer_cfg.cwd).await
        else {
            return false;
        };
        let Ok(content) = cron::tasks_file::read_tasks_body(fs.as_ref(), &self.firer_cfg.cwd).await
        else {
            return false;
        };
        let mut doc = cron::tasks_file::parse_tasks(&content);
        let before = doc.tasks.len();
        doc.tasks.retain(|t| t.id != id);
        if doc.tasks.len() == before {
            return false;
        }
        cron::tasks_file::write_tasks_body(
            fs.as_ref(),
            &self.firer_cfg.cwd,
            &cron::tasks_file::serialize_tasks(&doc),
        )
        .await
        .is_ok()
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
    let lingxi_home = cfg.lingxi_home.clone();
    let session_cwd = cfg.cwd.to_string_lossy().into_owned();
    let fs = platform.filesystem();
    // Capture the build recipe + platform BEFORE they move into
    // `build_mobile_inner`, so the cron firing path can rebuild a fresh throwaway
    // runtime per fired job (the same pattern as `lingxi_home`/`session_cwd`/`fs`).
    let firer_cfg = cfg.clone();
    let firer_platform = platform.clone();

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
        lingxi_home,
        session_cwd,
        fs,
        firer_cfg,
        firer_platform,
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
        // P0.2: the injectable memory provider defaults to None (empty,
        // deterministic — production injects `Some(real_provider())`).
        assert!(cfg.memory_provider.is_none());
        let _clone = cfg.clone();
        // Exercises the manual `Debug` impl that renders `memory_provider` as a
        // presence marker (`Arc<dyn MemoryHierarchyProvider>` is not `Debug`).
        let _ = format!("{cfg:?}");
    }

    /// Cron FFI (Android background-scheduler bridge): `cron_create` / `cron_list`
    /// / `cron_delete` / `next_cron_fire_time` round-trip through a real handle and
    /// the shared `scheduled_tasks.json` contract, and `run_due_cron_now` fires
    /// NOTHING (so makes no network call) for a job that is not yet due. The firing
    /// CORE (`cron::run_due_jobs` due-detection + bookkeeping) is unit-tested in the
    /// `cron` crate; here we prove the handle wiring (captured `firer_cfg` /
    /// `firer_platform`) reaches the on-disk file deterministically off-device.
    #[test]
    fn cron_ffi_create_list_delete_round_trips() {
        use crate::test_support::{
            new_engine_with_streaming, test_config, CollectingPermissionSink, FakeListener,
            HostFakePlatform,
        };

        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join(".lingxi")).expect("mk .lingxi");
        let platform: Arc<dyn traits::Platform> =
            Arc::new(HostFakePlatform::new(tmp.path().to_path_buf()));
        let handle = new_engine_with_streaming(
            test_config(tmp.path()),
            platform,
            Arc::new(FakeListener::default()),
            Arc::new(CollectingPermissionSink::default()),
            None,
        )
        .expect("build handle");

        handle.runtime().block_on(async {
            // Empty file → empty list, no next fire, nothing fires.
            assert!(handle.cron_list().await.is_empty());
            assert_eq!(handle.next_cron_fire_time().await, None);
            assert!(handle.run_due_cron_now().await.is_empty());

            // Create a far-future job (Jan 1 00:00) → present, with a computed next
            // fire, and NOT due now (so `run_due_cron_now` fires nothing / no net).
            let created = handle
                .cron_create("0 0 1 1 *".to_string(), "happy new year".to_string(), true)
                .await
                .expect("create succeeds");
            assert!(created.id.starts_with('d'), "claude-code id format");
            assert!(created.recurring);
            assert!(created.next_fire_ms.is_some());

            let list = handle.cron_list().await;
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].prompt, "happy new year");
            assert_eq!(list[0].cron, "0 0 1 1 *");
            // The list's per-task next fire matches the scheduler's earliest.
            assert_eq!(handle.next_cron_fire_time().await, list[0].next_fire_ms);
            assert!(
                handle.run_due_cron_now().await.is_empty(),
                "a far-future job is not due, so nothing fires"
            );

            // An invalid expression is rejected and does NOT persist.
            assert!(handle
                .cron_create("not a cron".to_string(), "x".to_string(), false)
                .await
                .is_err());
            assert_eq!(handle.cron_list().await.len(), 1);

            // Delete removes it; a second delete is a no-op `false`.
            assert!(handle.cron_delete(created.id.clone()).await);
            assert!(handle.cron_list().await.is_empty());
            assert!(!handle.cron_delete(created.id).await);
        });
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

    /// Audit (secure-storage): the runtime's `oauth_supported` reflects the
    /// platform's injected secure store — `false` with the non-persisting stub
    /// (so `/login` is gated off), `true` once a real encrypted store is injected
    /// (enabling the OAuth persist path). Proves the native secure-storage
    /// injection seam (`Platform::secure_storage()` → `build_mobile_inner`)
    /// end-to-end, off-device.
    #[tokio::test]
    async fn injected_encrypted_store_enables_oauth() {
        use async_trait::async_trait;
        use std::collections::HashMap;
        use std::sync::Mutex;

        /// In-memory `SecureStorage` that reports itself as encrypted — the
        /// off-device stand-in for a real Keychain/Keystore bridge.
        #[derive(Default)]
        struct FakeEncryptedStore {
            map: Mutex<HashMap<(String, String), protocol::SecureStorageData>>,
        }
        #[async_trait]
        impl traits::SecureStorage for FakeEncryptedStore {
            async fn store(
                &self,
                service: &str,
                account: &str,
                data: protocol::SecureStorageData,
            ) -> Result<(), traits::SecureStorageError> {
                self.map
                    .lock()
                    .unwrap()
                    .insert((service.to_string(), account.to_string()), data);
                Ok(())
            }
            async fn retrieve(
                &self,
                service: &str,
                account: &str,
            ) -> Result<Option<protocol::SecureStorageData>, traits::SecureStorageError>
            {
                Ok(self
                    .map
                    .lock()
                    .unwrap()
                    .get(&(service.to_string(), account.to_string()))
                    .cloned())
            }
            async fn delete(
                &self,
                service: &str,
                account: &str,
            ) -> Result<(), traits::SecureStorageError> {
                self.map
                    .lock()
                    .unwrap()
                    .remove(&(service.to_string(), account.to_string()));
                Ok(())
            }
            async fn list(&self, service: &str) -> Result<Vec<String>, traits::SecureStorageError> {
                Ok(self
                    .map
                    .lock()
                    .unwrap()
                    .keys()
                    .filter(|(s, _)| s == service)
                    .map(|(_, a)| a.clone())
                    .collect())
            }
            fn is_encrypted(&self) -> bool {
                true
            }
            fn backend(&self) -> traits::SecureStorageBackend {
                traits::SecureStorageBackend::EncryptedFile
            }
        }

        let tmp = tempfile::tempdir().expect("tempdir");

        // No store injected → the non-persisting stub → OAuth /login gated off.
        let rt_stub = build_mobile(
            test_config(tmp.path()),
            Arc::new(HostFakePlatform::new(tmp.path().to_path_buf())),
            Arc::new(FakeListener::default()),
            Arc::new(RecordingPermissionSink::default()),
        )
        .await
        .expect("build_mobile (stub) failed");
        assert!(
            !rt_stub.oauth_supported,
            "the non-persisting stub store must gate OAuth /login off"
        );

        // Inject an encrypted store → OAuth /login enabled.
        let platform: Arc<dyn traits::Platform> = Arc::new(
            HostFakePlatform::new(tmp.path().to_path_buf())
                .with_secure_storage(Arc::new(FakeEncryptedStore::default())),
        );
        let rt_real = build_mobile(
            test_config(tmp.path()),
            platform,
            Arc::new(FakeListener::default()),
            Arc::new(RecordingPermissionSink::default()),
        )
        .await
        .expect("build_mobile (encrypted) failed");
        assert!(
            rt_real.oauth_supported,
            "an injected encrypted secure store must enable OAuth /login"
        );
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
            gate.resolve(
                1,
                client_protocol::permission::PermissionResponseDto::Deny,
                "Bash"
            )
            .await
        );
        let _ = task.await.unwrap();
    }

    // ── P0.2: mobile hook lifecycle (real HookExecutorImpl + lifecycle fires) ─
    //
    // The mobile composition root now builds the REAL `HookExecutorImpl` (loaded
    // from `cwd/.lingxi/settings.json` + `lingxi_home/settings.json`) in place of
    // the `noop_hook_executor()` stub, and fires `SessionStart` (source=startup)
    // + `InstructionsLoaded` (once per loaded LINGXI.md) at boot — exactly the
    // desktop `build()` lifecycle (engine-desktop §7 / §7.1). These mirror the
    // desktop `build_fires_session_start_against_a_registered_hook` /
    // `build_fires_instructions_loaded_against_a_registered_hook` /
    // `build_with_injected_memory_reaches_system_prompt` tests.

    /// Write a single command hook for `event` into the project settings the
    /// mobile hook loader reads at boot (`<cwd>/.lingxi/settings.json`). The
    /// `"true"` command is a side-effect-free no-op (the in-build lifecycle fire
    /// is best-effort), so this asserts hook *registration*, not the command's
    /// effect.
    fn write_project_hook(cwd: &std::path::Path, event: &str) {
        let lingxi_dir = cwd.join(".lingxi");
        std::fs::create_dir_all(&lingxi_dir).expect("mk .lingxi");
        std::fs::write(
            lingxi_dir.join("settings.json"),
            format!(
                r#"{{ "hooks": {{ "{event}": [ {{ "hooks": [
                {{ "type": "command", "command": "true" }}
            ] }} ] }} }}"#
            ),
        )
        .expect("write settings.json");
    }

    /// P0.2: the boot path fires `SessionStart` (source=startup) once the
    /// orchestrator + hook registry are wired, best-effort. We register a
    /// `SessionStart` command hook in the project settings; `build_mobile` must
    /// (a) succeed even though the wired `fire_session_start("startup")` ran a
    /// (no-op) command hook, and (b) surface the loaded hook via `list_hooks` —
    /// proving the boot path loaded the session-lifecycle hook the in-build fire
    /// dispatched against (mobile sibling of the desktop test).
    #[tokio::test]
    async fn build_mobile_fires_session_start_against_a_registered_hook() {
        use traits::OrchestratorHandle as _;

        let tmp = tempfile::tempdir().expect("tempdir");
        write_project_hook(tmp.path(), "SessionStart");

        let platform: Arc<dyn traits::Platform> =
            Arc::new(HostFakePlatform::new(tmp.path().to_path_buf()));
        let listener: Arc<dyn ClientEventListener> = Arc::new(FakeListener::default());
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build_mobile(test_config(tmp.path()), platform, listener, perm_sink)
            .await
            .expect("build_mobile must succeed even with a (no-op) SessionStart hook registered");

        let hooks = rt.orchestrator.list_hooks().await;
        // Exactly ONE registration. On mobile `lingxi_home` == `<cwd>/.claude`, so
        // the user- and project-settings paths resolve to the SAME file; before the
        // settings-path de-dup this hook registered (and therefore fired) TWICE. A
        // plain `.any()` masked that — assert count == 1 to catch the regression.
        let session_start_count = hooks.iter().filter(|h| h.event == "SessionStart").count();
        assert_eq!(
            session_start_count, 1,
            "boot must load the SessionStart hook EXACTLY once (no settings-path double-registration): {hooks:?}"
        );
    }

    /// P0.2: the boot path fires `InstructionsLoaded` (load_reason=session_start)
    /// right after `SessionStart`, best-effort. We register an
    /// `InstructionsLoaded` command hook in the project settings; `build_mobile`
    /// must succeed and surface the loaded hook via `list_hooks`. (With the
    /// default empty memory provider no instruction file actually fires, exactly
    /// like the desktop sibling — the assertion pins the boot-fire seam + the
    /// real registry wiring.)
    #[tokio::test]
    async fn build_mobile_fires_instructions_loaded_against_a_registered_hook() {
        use traits::OrchestratorHandle as _;

        let tmp = tempfile::tempdir().expect("tempdir");
        write_project_hook(tmp.path(), "InstructionsLoaded");

        let platform: Arc<dyn traits::Platform> =
            Arc::new(HostFakePlatform::new(tmp.path().to_path_buf()));
        let listener: Arc<dyn ClientEventListener> = Arc::new(FakeListener::default());
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build_mobile(test_config(tmp.path()), platform, listener, perm_sink)
            .await
            .expect("build_mobile must succeed even with a (no-op) InstructionsLoaded hook");

        let hooks = rt.orchestrator.list_hooks().await;
        assert!(
            hooks.iter().any(|h| h.event == "InstructionsLoaded"),
            "boot must load the InstructionsLoaded hook the lifecycle fire dispatches against: {hooks:?}"
        );
    }

    /// P0.2: the injectable `cfg.memory_provider` seam (production wires
    /// `orchestrator::prompt::real_provider()`). We inject a CONTROLLED in-memory
    /// provider (NOT the real FS) carrying one project LINGXI.md and prove it
    /// flows through `build_mobile` into the orchestrator's system prompt (the
    /// GAP-3 memory section — preamble + tier-tagged `Contents of …:` + body).
    /// The default-empty sibling elides the memory section, so its presence is
    /// the load-bearing difference the injected provider makes.
    #[tokio::test]
    async fn build_mobile_with_injected_memory_reaches_system_prompt() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut cfg = test_config(tmp.path());

        let memory_path = cfg.cwd.join("LINGXI.md");
        let memory_body = "PROJECT MEMORY: always be terse.";
        let memory_file = orchestrator::prompt::MemoryFile {
            path: memory_path.clone(),
            body: memory_body.to_string(),
            is_local_override: false,
            tier: orchestrator::prompt::LingxiMdTier::Project,
            globs: None,
        };
        cfg.memory_provider = Some(Arc::new(
            orchestrator::test_support::StaticMemoryProvider::with_files(vec![memory_file]),
        ));

        let platform: Arc<dyn traits::Platform> =
            Arc::new(HostFakePlatform::new(tmp.path().to_path_buf()));
        let listener: Arc<dyn ClientEventListener> = Arc::new(FakeListener::default());
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build_mobile(cfg, platform, listener, perm_sink)
            .await
            .expect("build_mobile with an injected memory provider must succeed");

        // R-P1: claudeMd lives in the leading additional-context `<system-reminder>`
        // meta now (same `memory_block::format`), NOT the system prompt.
        let ctx =
            rt.orchestrator.additional_context_preview().await.expect(
                "an additional-context meta must be present (currentDate is unconditional)",
            );
        assert!(
            ctx.contains(
                "Codebase and user instructions are shown below. Be sure to adhere to these instructions."
            ),
            "injected memory must emit the memory preamble in the additional-context meta: {ctx}"
        );
        assert!(
            ctx.contains(&format!(
                "Contents of {} (project instructions, checked into the codebase):",
                memory_path.display()
            )),
            "the injected LINGXI.md must emit a tier-tagged `Contents of …:` marker: {ctx}"
        );
        assert!(
            ctx.contains(memory_body),
            "the injected LINGXI.md body must appear in the additional-context meta: {ctx}"
        );
    }

    /// P0.2 determinism guard: a default-config `build_mobile`
    /// (`cfg.memory_provider == None`) loads NO memory, so the system prompt
    /// emits NO memory section — pinning that the existing boot tests stay
    /// deterministic (they never read the real `~/.lingxi/LINGXI.md`).
    #[tokio::test]
    async fn build_mobile_default_loads_no_memory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = test_config(tmp.path());
        assert!(
            cfg.memory_provider.is_none(),
            "default config must leave memory_provider None (empty, deterministic)"
        );

        let platform: Arc<dyn traits::Platform> =
            Arc::new(HostFakePlatform::new(tmp.path().to_path_buf()));
        let listener: Arc<dyn ClientEventListener> = Arc::new(FakeListener::default());
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());

        let rt = build_mobile(cfg, platform, listener, perm_sink)
            .await
            .expect("default build_mobile must succeed");

        let sys = rt.orchestrator.assemble_system_prompt_preview().await;
        assert!(
            !sys.contains(
                "Codebase and user instructions are shown below. Be sure to adhere to these instructions."
            ),
            "a default build must emit NO memory preamble: {sys}"
        );
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
    fn build_submit_handle(root: &std::path::Path) -> (Arc<MobileEngineHandle>, Arc<FakeListener>) {
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
        assert!(
            result.is_ok(),
            "submit(SendPrompt) returned an error: {result:?}"
        );

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

    /// Seed one valid session JSONL under `<lingxi_home>/projects/<sanitize(cwd)>/`
    /// so `submit(ListSessions)` has a real on-disk catalog to enumerate. Mirrors
    /// the `session` crate's own `list_recent_test` fixture (the enumerator reads
    /// the dir via `tokio::fs` and each file via the injected `fs`). Returns the
    /// seeded session UUID string.
    fn seed_session_file(root: &std::path::Path) -> String {
        let cfg = test_config(root);
        let cwd = cfg.cwd.to_string_lossy().into_owned();
        let project_dir = cfg
            .lingxi_home
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
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&line).unwrap()),
        )
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
                .submit(ClientCommand::NewSession {
                    cwd: None,
                    model: None,
                })
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
    /// `<lingxi_home>/projects/<sanitize(cwd)>/<uuid>.jsonl` — a user+assistant
    /// pair with a proper `parentUuid` chain (first msg parent=null, the second's
    /// parent = the first's uuid, both `sessionId == <file uuid>`) so it PASSES
    /// the loader's `validate_chain`. Returns `(file_uuid, user_assistant_count)`.
    fn seed_replay_valid_session(root: &std::path::Path) -> (String, usize) {
        let cfg = test_config(root);
        let cwd = cfg.cwd.to_string_lossy().into_owned();
        let project_dir = cfg
            .lingxi_home
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
                    Ev::SessionResumed {
                        session_id,
                        messages,
                    } => {
                        assert_eq!(
                            session_id, &file_uuid,
                            "resumed id must be the named session"
                        );
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
                !events
                    .iter()
                    .any(|e| matches!(e, Ev::SessionResumed { .. })),
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
                !events
                    .iter()
                    .any(|e| matches!(e, Ev::SessionResumed { .. })),
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

    // ── TPM-C (mobile): default_model profile/model parsing ──────────────────

    /// Verifies the listings-building + `parse_model_ref` logic the mobile
    /// composition root uses at build time: a qualified `profile/model`
    /// default_model splits into the bare id (written to `orch_cfg.model`) and
    /// `Some(profile)` (used to seed `switch_model`), while a bare id passes
    /// through unchanged with `None` profile (no-op seed path).
    ///
    /// This is a pure unit test of the parser + listing shape — no I/O, no
    /// tokio runtime — mirroring `default_model_parse_qualified_and_bare` in
    /// engine-desktop.
    #[test]
    fn mobile_default_model_parse_qualified_and_bare() {
        // Construct the same listing shape the mobile composition root builds
        // from `assembled.client_config.providers` (display_model == request_model
        // on mobile; provider_label == profile_name).
        let listings = vec![
            traits::ModelListing {
                display_model: "gpt-5.2".to_string(),
                request_model: "gpt-5.2".to_string(),
                provider_id: "openai".to_string(),
                provider_label: "openai".to_string(),
                description: None,
                supports_reasoning: false,
            },
            traits::ModelListing {
                display_model: "gpt-5.2".to_string(),
                request_model: "gpt-5.2".to_string(),
                provider_id: "github-copilot".to_string(),
                provider_label: "github-copilot".to_string(),
                description: None,
                supports_reasoning: false,
            },
            traits::ModelListing {
                display_model: "claude-sonnet-4-20250514".to_string(),
                request_model: "claude-sonnet-4-20250514".to_string(),
                provider_id: "anthropic".to_string(),
                provider_label: "anthropic".to_string(),
                description: None,
                supports_reasoning: true,
            },
        ];

        // Qualified: "openai/gpt-5.2" → bare id "gpt-5.2" + profile "openai"
        let (id, profile) = traits::parse_model_ref("openai/gpt-5.2", &listings);
        assert_eq!(id, "gpt-5.2", "qualified ref must strip the profile prefix");
        assert_eq!(
            profile.as_deref(),
            Some("openai"),
            "qualified ref must extract the profile"
        );

        // Bare: "claude-sonnet-4-20250514" → same id, no profile (no-op seed path)
        let (id2, profile2) = traits::parse_model_ref("claude-sonnet-4-20250514", &listings);
        assert_eq!(
            id2, "claude-sonnet-4-20250514",
            "bare model id must pass through"
        );
        assert!(profile2.is_none(), "bare model must yield None profile");

        // Shared id with two providers and explicit profile qualifier
        let (id3, profile3) = traits::parse_model_ref("github-copilot/gpt-5.2", &listings);
        assert_eq!(id3, "gpt-5.2");
        assert_eq!(profile3.as_deref(), Some("github-copilot"));
    }
}
