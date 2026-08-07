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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use client_adapter::{
    AdapterOutputStream, AdapterPermissionGate, ClientEventListener, ListenerSink,
    PermissionRequestSink, TurnWrapper,
};
use client_protocol::commands::{
    ClientCommand, ListingKindDto as ProtocolListingKind, ProviderCredentialSecretDto,
};
use client_protocol::error::ClientError;
use client_protocol::events::{ClientEvent, ErrorKindDto, TurnOutcomeDto};
use client_protocol::local_apps::{AppCreateOriginDto, AppDesignPatchDto, AppEventDto};
use client_protocol::permission::{
    PermissionKindDto, PermissionRequest as PermissionRequestDto, PermissionResponseDto,
};
use command_api::model::BuiltinCommandHandler;
use command_api::parse_slash_command;
use command_api::RegistrySlashDispatcher;
use cron::CronJobFirer;
use local_apps::{AppError, AppGenerationCoordinator, AppService};
use mcp::{ConfigScope as McpConfigScope, McpRegistry, McpServerConfig};

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
use tokio::sync::{mpsc, Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;
use tool_api::AnthropicRequestBuilder;
use tool_api::BuiltinToolContext;
use tool_api::SessionCwd;
use traits::http::{
    HttpError, RawByteStream, RawByteStreamWithMeta, SseStream, SseStreamWithMeta,
    WebSocketConnectionWithMeta, WebSocketMessageStreamWithMeta,
};
use traits::{
    AuthHandle, Clock, FileSystem, HttpTransport, MobileLinuxCapability, MobileLinuxRuntime,
    MobileLinuxRuntimeMode, OrchestratorHandle, OutputStream, Platform, RootfsState, RootfsStatus,
    SlashCommandDispatcher,
};

use crate::{
    local_apps_generation::{lower_job, ClientGenerationJobObserver, MobileAppGenerationExecutor},
    local_apps_host::LocalAppsHostBroker,
    local_apps_llm::{ApiServiceModel, LocalAppsLlm},
    local_apps_mcp::{LocalAppsMcpTransport, LOCAL_APPS_REGISTRY_KEY},
    local_apps_profile::{profile_apps, ProfileApps, SharedLlm},
    mobile_command_registry, mobile_tool_registry_with_skill_loader,
    mobile_tool_registry_with_skill_loader_and_ask_resolver, register_android_ui_automation,
};

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
    async fn request_with_resolved_addrs(
        &self,
        req: protocol::HttpRequest,
        resolved: Option<traits::ResolvedAddressOverride>,
    ) -> Result<protocol::HttpResponse, HttpError> {
        self.0.request_with_resolved_addrs(req, resolved).await
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
    /// Whether the selected mobile workspace has passed the host trust flow.
    /// Defaults false so `/goal` and other hook-backed persistent behaviors fail
    /// closed until the Android/iOS host explicitly records trust.
    pub workspace_trusted: bool,
    /// Settings-declared `providers` block as raw JSON, fed verbatim to
    /// `llm_client::ClientConfig` via `build()`. `None` ⟶ built-in profiles only.
    pub provider_profiles: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    /// Settings-declared `routing` block as raw JSON, fed verbatim to
    /// `llm_client::ClientConfig`. `None` ⟶ the default (empty) routing config.
    pub routing: Option<serde_json::Value>,
    /// Stable compatibility carrier for the mobile `Shell` tool gate + prompt
    /// metadata. Existing Android call sites still populate this field; iOS can
    /// reuse the same carrier type once its runtime bridge enables shell/git.
    pub android_shell: Option<tool_api::AndroidShellToolCtx>,
    /// Stable compatibility carrier for the mobile structured `Git` tool gate +
    /// workspace metadata. Existing Android call sites still populate this
    /// field; iOS can reuse the same carrier type once its runtime bridge
    /// enables git.
    pub android_git: Option<tool_api::AndroidGitToolCtx>,
    /// Mobile Git network secret (HTTPS token + CA dir, spec §G3, P4). Held
    /// separately from the public [`MobileConfig::android_git`] carrier so the
    /// token never enters the broadly-cloned public ctx. `tool-git-mobile`
    /// reads it at call time.
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
    /// Whether local apps run through the fixed Next production server. Store
    /// builds keep this false and serve the static export instead.
    pub local_apps_full_runtime: bool,
    /// Host path containing the verified, read-only `node_modules` runtime
    /// bundle. Its `node_modules` child is mounted at the canonical read-only
    /// `/opt/lingxi/local-app-runtime/node_modules` path for build/run only.
    pub local_apps_runtime_root: Option<std::path::PathBuf>,
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
            .field("workspace_trusted", &self.workspace_trusted)
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
            .field("local_apps_full_runtime", &self.local_apps_full_runtime)
            .field("local_apps_runtime_root", &self.local_apps_runtime_root)
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
            workspace_trusted: false,
            provider_profiles: None,
            routing: None,
            android_shell: None,
            android_git: None,
            android_git_secret: None,
            // P0.2: default to NO memory provider (empty, deterministic). The
            // production FFI entry points inject `Some(real_provider())`.
            memory_provider: None,
            local_apps_full_runtime: false,
            local_apps_runtime_root: None,
        }
    }
}

impl MobileConfig {
    #[must_use]
    pub fn mobile_shell(&self) -> Option<&tool_api::MobileShellToolCtx> {
        self.android_shell.as_ref()
    }

    #[must_use]
    pub fn mobile_git(&self) -> Option<&tool_api::MobileGitToolCtx> {
        self.android_git.as_ref()
    }

    #[must_use]
    pub fn mobile_git_secret(&self) -> Option<&tool_api::MobileGitSecret> {
        self.android_git_secret.as_ref()
    }
}

/// Parse the non-secret provider configuration supplied by a mobile host.
///
/// Keeping JSON decoding in `engine-mobile` avoids making the Android/iOS
/// packager crates depend on `serde_json` on host builds. Secrets deliberately
/// travel through `SetProviderCredential` instead of either JSON document.
pub fn parse_mobile_provider_config_json(
    provider_profiles_json: &str,
    routing_json: Option<&str>,
) -> Result<
    (
        Option<std::collections::BTreeMap<String, serde_json::Value>>,
        Option<serde_json::Value>,
    ),
    MobileEngineError,
> {
    const MAX_CONFIG_BYTES: usize = 512 * 1024;
    if provider_profiles_json.len() > MAX_CONFIG_BYTES
        || routing_json.is_some_and(|value| value.len() > MAX_CONFIG_BYTES)
    {
        return Err(MobileEngineError::Internal(
            "invalid provider config: payload too large".to_string(),
        ));
    }

    let profiles = if provider_profiles_json.trim().is_empty() {
        None
    } else {
        Some(
            serde_json::from_str::<std::collections::BTreeMap<String, serde_json::Value>>(
                provider_profiles_json,
            )
            .map_err(|error| {
                MobileEngineError::Internal(format!("invalid provider profiles JSON: {error}"))
            })?,
        )
    };
    let routing = routing_json
        .filter(|value| !value.trim().is_empty())
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| {
            MobileEngineError::Internal(format!("invalid provider routing JSON: {error}"))
        })?;
    Ok((profiles, routing))
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
    /// The session transcript writer shared with the orchestrator.
    ///
    /// Mobile keeps one orchestrator alive while New/Resume changes the active
    /// session, so the command path retargets this writer to the new UUID before
    /// another turn can begin.
    pub session_writer: Arc<session::jsonl::writer::JsonlWriter>,
    /// Whether the wired secure-storage backend can actually PERSIST credentials
    /// (i.e. is a real OS Keychain/Keystore, `is_encrypted() == true`). Mobile
    /// currently wires the non-persisting `PlainTextSecureStorage` stub, so this
    /// is `false` and OAuth `/login` cannot persist its tokens — the Login arm
    /// short-circuits with a clear message instead of failing at the persist step
    /// with a cryptic `BackendUnavailable` (audit re-pass, secure-storage finding;
    /// the real native store is a §11 / Plan-17 follow-up). Becomes `true`
    /// automatically once a native Keychain/Keystore SecureStorage is injected.
    pub oauth_supported: bool,
    /// Shared provider credential manager used by the live LLM client, OAuth,
    /// and the mobile provider-settings commands. Retaining this handle is what
    /// lets a credential written after boot take effect on the next request
    /// without rebuilding the engine.
    pub credentials: Arc<CredentialManager>,
    /// Mobile-only Linux userspace runtime seam (Android PRoot / iOS iSH),
    /// when the platform wires one. `None` preserves the pre-migration state.
    pub mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    /// The sole mobile MCP registry. It contains exactly the built-in
    /// `local_apps` in-process provider; mobile never discovers project/user
    /// MCP configuration.
    pub mcp_registry: Arc<McpRegistry>,
    /// Transport retained so the engine handle can attach the AppService after
    /// the client event bridge has been constructed.
    local_apps_mcp: Arc<LocalAppsMcpTransport>,
    /// The local-app generator's LLM seam (Task 9): an [`ApiServiceModel`]
    /// over the SAME `api_service`/default model/profile the main
    /// conversation uses — no second routing table. Retained here so
    /// `build_mobile_engine_inner` can hand it to `profile_apps` after this
    /// function returns (the process-wide profile registry is loaded outside
    /// this per-connection builder).
    pub(crate) local_apps_llm: Arc<LocalAppsLlm>,
}

/// Non-secret result of testing one provider endpoint from the mobile engine.
///
/// The engine performs the request so an already-saved credential never has to
/// cross back into Swift/Kotlin. A caller may supply an unsaved draft credential
/// for a one-off test; it is used only for this request and is never persisted.
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConnectionTestDto {
    /// The endpoint authenticated successfully and the selected model is usable
    /// (or the provider returned no machine-readable model catalog).
    pub connected: bool,
    /// A server returned an HTTP response, even if authentication or the model
    /// check failed.
    pub reachable: bool,
    /// Authentication passed. This remains false when a rate limiter or proxy
    /// rejected the request before credentials could be verified.
    pub authenticated: bool,
    /// Whether the selected model appeared in a recognized model-list payload.
    pub model_available: bool,
    /// HTTP status when the provider responded.
    pub http_status: Option<u16>,
    /// End-to-end request duration, rounded down to milliseconds.
    pub latency_ms: u64,
    /// Log-safe user-facing detail. Provider response bodies and credentials are
    /// deliberately excluded.
    pub message: String,
    /// True when the credential came from the shared encrypted store; false for
    /// a one-off draft supplied by the settings form.
    pub used_stored_credential: bool,
}

/// Lowered rootfs lifecycle state for the foreign host.
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobileLinuxRootfsStateDto {
    Missing,
    Installing,
    Ready,
    Corrupt,
    Repairing,
    Resetting,
    Unsupported,
    BlockedByLicense,
}

/// Combined runtime + rootfs status for Android/iOS settings UIs.
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxStatusDto {
    /// Selected runtime mode.
    pub mode: String,
    /// Backend label for diagnostics / UI.
    pub backend: String,
    /// Whether the runtime can be used right now.
    pub available: bool,
    /// Human-readable availability / failure detail.
    pub reason: Option<String>,
    /// Rootfs lifecycle state.
    pub rootfs_state: MobileLinuxRootfsStateDto,
    /// Platform string (`android` / `ios`).
    pub platform: String,
    /// ABI / architecture string.
    pub abi: String,
    /// Active rootfs version, when present.
    pub version: Option<String>,
    /// Installed rootfs size in bytes, when known.
    pub installed_size_bytes: Option<u64>,
    /// Writable guest paths currently permitted.
    pub writable_guest_paths: Vec<String>,
    /// Capability flags.
    pub streaming_output: bool,
    pub background_processes: bool,
    pub pty: bool,
    pub bind_mounts: bool,
    pub rootfs_integrity: bool,
}

fn lower_mobile_linux_state(state: RootfsState) -> MobileLinuxRootfsStateDto {
    match state {
        RootfsState::Missing => MobileLinuxRootfsStateDto::Missing,
        RootfsState::Installing => MobileLinuxRootfsStateDto::Installing,
        RootfsState::Ready => MobileLinuxRootfsStateDto::Ready,
        RootfsState::Corrupt => MobileLinuxRootfsStateDto::Corrupt,
        RootfsState::Repairing => MobileLinuxRootfsStateDto::Repairing,
        RootfsState::Resetting => MobileLinuxRootfsStateDto::Resetting,
        RootfsState::Unsupported => MobileLinuxRootfsStateDto::Unsupported,
        RootfsState::BlockedByLicense => MobileLinuxRootfsStateDto::BlockedByLicense,
    }
}

fn lower_mobile_linux_mode(mode: MobileLinuxRuntimeMode) -> String {
    match mode {
        MobileLinuxRuntimeMode::Legacy => "legacy".to_string(),
        MobileLinuxRuntimeMode::MobileLinux => "mobile-linux".to_string(),
    }
}

fn lower_mobile_linux_backend(backend: traits::SandboxBackend) -> String {
    match backend {
        traits::SandboxBackend::LinuxNamespaces => "linux-namespaces",
        traits::SandboxBackend::LinuxFirejail => "linux-firejail",
        traits::SandboxBackend::MacOsSandboxExec => "macos-sandbox-exec",
        traits::SandboxBackend::WindowsJobObject => "windows-job-object",
        traits::SandboxBackend::AndroidMinijail => "android-minijail",
        traits::SandboxBackend::AndroidProot => "android-proot",
        traits::SandboxBackend::IosIsh => "ios-ish",
        traits::SandboxBackend::None => "none",
    }
    .to_string()
}

fn lower_mobile_linux_status(
    capability: MobileLinuxCapability,
    status: RootfsStatus,
) -> MobileLinuxStatusDto {
    MobileLinuxStatusDto {
        mode: lower_mobile_linux_mode(status.mode),
        backend: lower_mobile_linux_backend(status.backend),
        available: capability.available,
        reason: capability.reason.or(status.last_error),
        rootfs_state: lower_mobile_linux_state(status.state),
        platform: status.platform,
        abi: status.abi,
        version: status.version,
        installed_size_bytes: status.installed_size_bytes,
        writable_guest_paths: status.writable_guest_paths,
        streaming_output: capability.streaming_output,
        background_processes: capability.background_processes,
        pty: capability.pty,
        bind_mounts: capability.bind_mounts,
        rootfs_integrity: capability.rootfs_integrity,
    }
}

fn gate_mobile_shell_ctx(
    carrier: Option<tool_api::MobileShellToolCtx>,
    capability: Option<&MobileLinuxCapability>,
) -> Option<tool_api::MobileShellToolCtx> {
    let mut carrier = carrier?;
    if capability.is_some_and(|cap| {
        matches!(cap.mode, MobileLinuxRuntimeMode::MobileLinux) && !cap.available
    }) {
        carrier.enabled = false;
    }
    Some(carrier)
}

fn gate_mobile_git_ctx(
    carrier: Option<tool_api::MobileGitToolCtx>,
    capability: Option<&MobileLinuxCapability>,
) -> Option<tool_api::MobileGitToolCtx> {
    let mut carrier = carrier?;
    if capability.is_some_and(|cap| {
        matches!(cap.mode, MobileLinuxRuntimeMode::MobileLinux) && !cap.available
    }) {
        carrier.enabled = false;
    }
    Some(carrier)
}

#[cfg(test)]
mod mobile_tool_gate_tests {
    use super::*;

    fn unavailable_mobile_linux_capability() -> MobileLinuxCapability {
        MobileLinuxCapability {
            available: false,
            backend: traits::SandboxBackend::IosIsh,
            mode: MobileLinuxRuntimeMode::MobileLinux,
            reason: Some("runtime unavailable".into()),
            streaming_output: false,
            background_processes: false,
            pty: false,
            bind_mounts: false,
            rootfs_integrity: false,
        }
    }

    #[test]
    fn mobile_shell_gate_disables_selected_but_unavailable_runtime() {
        let gated = gate_mobile_shell_ctx(
            Some(tool_api::MobileShellToolCtx::mobile_linux_guest(
                true,
                vec!["sh".into()],
                None,
            )),
            Some(&unavailable_mobile_linux_capability()),
        )
        .expect("carrier present");
        assert!(!gated.enabled);
    }

    #[test]
    fn mobile_git_gate_disables_selected_but_unavailable_runtime() {
        let gated = gate_mobile_git_ctx(
            Some(tool_api::MobileGitToolCtx {
                enabled: true,
                has_token: true,
                workspace_root: "/workspace".into(),
            }),
            Some(&unavailable_mobile_linux_capability()),
        )
        .expect("carrier present");
        assert!(!gated.enabled);
    }
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
    match default_model.split_once('/') {
        Some(("anthropic", model)) if !model.is_empty() => ids.push(model.to_string()),
        Some(_) => {}
        None if !default_model.is_empty() => ids.push(default_model.to_string()),
        None => {}
    }
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

const MOBILE_ENABLED_PROFILES_KEY: &str = "mobileEnabledProfiles";

/// Apply the mobile host's explicit provider profile allowlist after shared
/// provider assembly and before any model catalog or client is constructed.
///
/// The reserved routing key is interpreted only in this mobile composition
/// root, so desktop assembly remains unchanged. Absence preserves the shared
/// catalog for backward-compatible hosts. Presence is fail-closed: a malformed
/// value is treated as an empty allowlist.
fn apply_mobile_profile_allowlist(
    assembled: &mut provider_config::Assembled,
    routing: Option<&serde_json::Value>,
) {
    let Some(value) = routing.and_then(|routing| routing.get(MOBILE_ENABLED_PROFILES_KEY)) else {
        return;
    };
    let enabled_profiles: std::collections::BTreeSet<String> = match value.as_array() {
        Some(items) => {
            let mut profiles = std::collections::BTreeSet::new();
            for item in items {
                let Some(profile) = item.as_str().filter(|profile| !profile.is_empty()) else {
                    profiles.clear();
                    break;
                };
                profiles.insert(profile.to_string());
            }
            profiles
        }
        None => std::collections::BTreeSet::new(),
    };

    let allowed_routes: Vec<(llm_client::ProviderId, String)> = assembled
        .client_config
        .providers
        .iter()
        .filter(|provider| enabled_profiles.contains(&provider.profile_name))
        .flat_map(|provider| {
            let provider_id = provider.provider_id.clone();
            provider
                .models
                .iter()
                .map(move |model| (provider_id.clone(), model.request_model.clone()))
        })
        .collect();

    assembled
        .client_config
        .providers
        .retain(|provider| enabled_profiles.contains(&provider.profile_name));
    assembled
        .credential_sources
        .retain(|source| enabled_profiles.contains(&source.profile_name));
    assembled.chains.aliases.retain(|_, target| {
        target
            .split_once('/')
            .is_some_and(|(profile, _)| enabled_profiles.contains(profile))
    });
    assembled.chains.chains.retain(|_, entries| {
        entries.retain(|entry| {
            allowed_routes.iter().any(|(provider_id, model)| {
                provider_id == &entry.provider_id && model == &entry.model
            })
        });
        !entries.is_empty()
    });
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
    build_mobile_inner_with_ask(
        cfg,
        platform,
        listener,
        permission_sink,
        streaming_override,
        None,
    )
    .await
}

#[allow(clippy::too_many_lines)]
async fn build_mobile_inner_with_ask(
    cfg: MobileConfig,
    platform: Arc<dyn Platform>,
    listener: Arc<dyn ClientEventListener>,
    permission_sink: Arc<dyn PermissionRequestSink>,
    streaming_override: Option<Arc<dyn StreamingApiClient>>,
    ask_user_question_tx: Option<tokio::sync::mpsc::Sender<tool_ui::AskUserQuestionExchange>>,
) -> Result<MobileRuntime, MobileBuildError> {
    let cwd = cfg.cwd.clone();
    let local_apps_mcp = Arc::new(LocalAppsMcpTransport::new(mobile_apps_data_root(&cfg)));
    let mcp_registry = Arc::new(McpRegistry::new(
        local_apps_mcp.clone() as Arc<dyn traits::McpTransport>
    ));
    mcp_registry
        .connect(McpServerConfig {
            name: LOCAL_APPS_REGISTRY_KEY.into(),
            spec: traits::McpTransportSpec::InProcess {
                registry_key: LOCAL_APPS_REGISTRY_KEY.into(),
            },
            scope: McpConfigScope::Managed,
            disabled: false,
            timeout_ms: Some(30_000),
            always_load: true,
            config_error: None,
        })
        .await
        .map_err(|error| {
            MobileBuildError::Orchestrator(format!("local apps MCP bootstrap failed: {error}"))
        })?;

    // (1) OS handles from the aggregate `Platform` (NOT a concrete posix type —
    //     the device supplies these; the host test supplies a portable shim).
    let http = platform.http();
    let clock = platform.clock();
    let fs = platform.filesystem();
    let main_session_id = protocol::SessionId::new();
    let main_session_uuid = main_session_id.as_uuid().to_string();
    let session_writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
        orchestrator::transcript_paths::main_transcript_path(
            &cfg.lingxi_home,
            &cwd.to_string_lossy(),
            &main_session_uuid,
        ),
        fs.clone(),
    ));
    let mobile_linux = platform.mobile_linux();
    let mobile_linux_capability = match mobile_linux.as_ref() {
        Some(runtime) => Some(runtime.probe_capability().await),
        None => None,
    };
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
    let mut assembled = provider_config::assemble(provider_config::AssembleInputs {
        anthropic_api_base: cfg.api_base.clone(),
        anthropic_models: anthropic_models(&cfg.default_model),
        anthropic_has_api_key: has_api_key,
        anthropic_has_oauth: false, // mobile inference is api-key-only (no OAuth)
        user_providers: cfg.provider_profiles.clone().unwrap_or_default(),
        routing: cfg.routing.clone(),
    });
    apply_mobile_profile_allowlist(&mut assembled, cfg.routing.as_ref());
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
    let profile_auto_mode_provider: std::collections::BTreeMap<String, String> = assembled
        .client_config
        .providers
        .iter()
        .map(|profile| {
            let provider = match &profile.provider_id {
                llm_client::ProviderId::AnthropicFirstParty => "firstParty",
                llm_client::ProviderId::BedrockClaude => "anthropicAws",
                llm_client::ProviderId::VertexClaude => "vertex",
                llm_client::ProviderId::FoundryClaude => "foundry",
                _ => "other",
            };
            (profile.profile_name.clone(), provider.to_string())
        })
        .collect();
    let model_provider_profiles: std::collections::BTreeMap<String, String> = assembled
        .client_config
        .providers
        .iter()
        .flat_map(|profile| {
            let profile_name = profile.profile_name.clone();
            profile
                .models
                .iter()
                .map(move |model| (model.request_model.clone(), profile_name.clone()))
        })
        .collect();
    let boot_auto_mode_provider = default_model_profile
        .as_ref()
        .or_else(|| model_provider_profiles.get(&default_model_id))
        .and_then(|profile| profile_auto_mode_provider.get(profile))
        .cloned()
        .unwrap_or_else(|| "firstParty".to_string());

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
    // Task 9: the local-app generator's three LLM calls (author/plan/write
    // source) ride the SAME `api_service` — routing, auth, retry — as the
    // main conversation, via `ApiService::messages_create_side_query`
    // (the same forced-tool-call mechanism `sidequery::ProviderSideQueryClient`
    // uses below). `default_model_id`/`default_model_profile` are the bare
    // model id and provider profile `orch_cfg.model` itself is set from a few
    // lines down — the local-app generator has no separate model selection of
    // its own.
    let local_apps_llm = Arc::new(LocalAppsLlm::new(Arc::new(ApiServiceModel::new(
        api_service.clone(),
        default_model_id.clone(),
        default_model_profile.clone(),
    ))));
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
        credentials.clone(),
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
    // `agentPushNotifEnabled` scalar override (user → project → local). The
    // feature flag is checked independently by the cron/tool consumers.
    let mut agent_push_notif_enabled = false;
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
                    if let Some(b) = v
                        .get("agentPushNotifEnabled")
                        .and_then(serde_json::Value::as_bool)
                    {
                        agent_push_notif_enabled = b;
                    }
                }
                additional_working_dirs
                    .extend(permission::additional_directories_from_settings_json(&raw));
            }
        }
        traits::session_flags::set_agent_push_notif_enabled(agent_push_notif_enabled);
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
                    model: default_model_id.clone(),
                    provider: boot_auto_mode_provider.clone(),
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
    let mut disable_all_hooks = false;
    let mut hooks_restricted = false;
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
                if let Some(value) = v
                    .get("disableAllHooks")
                    .and_then(serde_json::Value::as_bool)
                {
                    disable_all_hooks = value;
                }
                if let Some(value) = v
                    .get("allowManagedHooksOnly")
                    .and_then(serde_json::Value::as_bool)
                {
                    hooks_restricted = value;
                }
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
    // Transcript sink for the per-hook-run `attachment` records claude-code
    // persists (one line per hook run). Created empty because the hook
    // executor is built BEFORE the orchestrator that owns the JSONL writer;
    // `attach` fills the cell once `orch` exists (step 8 below).
    let hook_attachment_sink = Arc::new(orchestrator::JsonlHookAttachmentSink::new());
    let hooks: Arc<hooks::HookExecutorImpl> = Arc::new(
        hooks::HookExecutorImpl::new(
            hook_registry.clone(),
            http.clone(),
            Arc::new(platform_posix_minimal::PosixRuntime::new())
                as Arc<dyn traits::RuntimeSpawner>,
        )
        .with_policy_disable_all_hooks(disable_all_hooks)
        .with_process_runner(process.clone(), sandbox.clone())
        .with_prompt_runner(Arc::new(orchestrator::ApiClientHookPromptRunner::new(
            api_client.clone(),
        )))
        // (H-BIN-12) Gate outbound HTTP-hook URLs + intersect the per-hook env
        // allowlist from the merged settings; `(None, None)` = no restriction.
        .with_http_hook_policy(allowed_http_hook_urls, http_hook_allowed_env_vars)
        // One transcript `attachment` line per hook run, matching claude-code.
        .with_attachment_sink(hook_attachment_sink.clone() as Arc<dyn hooks::HookAttachmentSink>),
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
    // PathAtlas S2: guest paths translate onto the mount table's HOST roots
    // (workspace under `Application Support/workspaces/<id>`, persistent
    // `/root` under the managed rootfs dir) — SIBLINGS of the app-sandbox
    // cwd, not children. Without trusting them, every translated path would
    // pass translation and then die at `canonicalize_and_validate`
    // containment. Legacy/unavailable runtimes serve an empty table, so this
    // adds nothing off mobile-linux.
    let mut trusted_dirs = vec![cwd.clone()];
    let mobile_linux_mounts = mobile_linux
        .as_ref()
        .map(|runtime| runtime.current_mounts())
        .unwrap_or_default();
    trusted_dirs.extend(mobile_linux_mounts.iter().map(|m| m.host_path.clone()));
    // PathAtlas S3: the MODEL-VISIBLE cwd is the guest workspace when one is
    // mounted — the same coordinate the shell already uses, so file tools and
    // shell commands name the same files. Relative tool paths resolve against
    // it and come back through translate_model_path onto the host twin. The
    // engine-internal cwd (`cwd` — transcripts, .lingxi, memory files) stays
    // host.
    let model_cwd = mobile_linux_mounts
        .iter()
        .find(|m| matches!(m.purpose, traits::MountPurpose::Workspace))
        .map(|m| std::path::PathBuf::from(&m.guest_path))
        .unwrap_or_else(|| cwd.clone());
    let session_cwd = SessionCwd::new(model_cwd, trusted_dirs);
    let tool_ctx = BuiltinToolContext {
        // No session: this context never persists tool output.
        session_id: None,
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
        mcp_registry: Some(mcp_registry.clone()),
        lsp_registry: None,
        camera: platform.camera(),
        voice: platform.voice(),
        stt: platform.stt(),
        tts: platform.tts(),
        share: platform.share(),
        notifications: platform.notifications(),
        clipboard: platform.clipboard(),
        computer_control: platform.computer_control(),
        // Mobile shell/git registration is fail-closed when the host selected
        // mobile-linux but the runtime is blocked or unlinked. In that state the
        // tools stay ABSENT rather than silently falling back to the Android
        // legacy path.
        android_shell: gate_mobile_shell_ctx(
            cfg.mobile_shell().cloned(),
            mobile_linux_capability.as_ref(),
        ),
        android_git: gate_mobile_git_ctx(
            cfg.mobile_git().cloned(),
            mobile_linux_capability.as_ref(),
        ),
        // Secret carrier follows the same public-gate decision: if the public
        // git tool is gated off, keep the secret seam absent too.
        android_git_secret: gate_mobile_git_ctx(
            cfg.mobile_git().cloned(),
            mobile_linux_capability.as_ref(),
        )
        .and_then(|_| cfg.mobile_git_secret().cloned()),
        // Mobile uses the same blocking TaskCreated/TaskCompleted hook contract
        // as desktop. The transcript path is unavailable before the session is
        // mounted, so it remains empty; cwd and policy are still enforced.
        task_lifecycle_hooks: Some(Arc::new(
            orchestrator::OrchestratorTaskLifecycleHookFirer::new(
                hooks.clone(),
                cwd.clone(),
                std::path::PathBuf::new(),
            ),
        )),
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
    let mut tools = if let Some(tx) = ask_user_question_tx {
        let timeout = tool_ui::ask_user_question::AskUserQuestionTimeout::parse_or_default(
            tool_ctx.ask_user_question_timeout.as_deref(),
        );
        mobile_tool_registry_with_skill_loader_and_ask_resolver(
            tool_ctx.clone(),
            skill_loader,
            Arc::new(tool_ui::ask_user_question::TuiBridgeResolver::new(
                timeout, tx,
            )),
        )
    } else {
        mobile_tool_registry_with_skill_loader(tool_ctx.clone(), skill_loader)
    };
    register_android_ui_automation(&mut tools, platform.android_ui_automation());
    for (connection_id, mcp_tools) in
        tool_mcp::build_registered_mcp_tools(&mcp_registry, tool_ctx).await
    {
        tools.register_mcp_tools(connection_id, mcp_tools);
    }
    let tools = Arc::new(tools);

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
    .with_session_id(main_session_id)
    .with_jsonl_writer(session_writer.clone())
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
    .with_workspace_trusted(cfg.workspace_trusted)
    .with_hooks_restricted(hooks_restricted || disable_all_hooks)
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
    // PathAtlas S3: prompt probes (memory hierarchy, git status, file tree)
    // must read the HOST directory backing the guest session cwd while the
    // env block displays the guest path itself. Live table: external mounts
    // added later still resolve.
    let orchestrator = if let Some(runtime) = mobile_linux.clone() {
        orchestrator.with_prompt_probe_cwd_resolver(std::sync::Arc::new(move |path| {
            traits::mobile_linux::map_guest_path_to_host(
                &path.to_string_lossy(),
                &runtime.current_mounts(),
            )
            .unwrap_or_else(|| path.to_path_buf())
        }))
    } else {
        orchestrator
    };
    orch_inner = orch_inner.with_mcp_registry(mcp_registry.clone());
    // P0.1 (gated): attach the memdir prefetch when enabled above.
    if let Some(prefetch) = memdir_prefetch {
        orch_inner = orch_inner.with_memory_prefetch(prefetch);
    }
    let orch = Arc::new(orch_inner);

    // Fill the hook-attachment sink's cell now that the orchestrator (and its
    // JSONL writer) exists. The sink holds a `Weak`, so this does not create an
    // orchestrator↔hook-executor reference cycle.
    hook_attachment_sink.attach(&orch);

    // H-CHG-02: wire the enforcing gate's live `set_permission_mode` auto gate to
    // the LIVE `session.model` (mutated by `/model` switches / resume), so a
    // runtime switch to `auto` on an auto-unsupported model is rejected
    // (`dUe(wi())` — claude-code `Nle`). Non-blocking `try_lock`; a contended read
    // returns `None` and the model check is skipped (fail-open). Desktop mirror.
    if let Some(cell) = live_model_provider_cell.as_ref() {
        let session = orch.session();
        let model_provider_profiles = model_provider_profiles.clone();
        let profile_auto_mode_provider = profile_auto_mode_provider.clone();
        let _ = cell.set(std::sync::Arc::new(move || {
            session.try_lock().ok().map(|state| {
                let profile = state
                    .model_profile
                    .as_ref()
                    .or_else(|| model_provider_profiles.get(&state.model));
                let provider = profile
                    .and_then(|profile| profile_auto_mode_provider.get(profile))
                    .cloned()
                    .unwrap_or_else(|| "firstParty".to_string());
                permission::LiveModelContext {
                    model: state.model.clone(),
                    provider,
                }
            })
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
        handle.clone(),
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
    let background_command_handle = handle.clone();
    let dispatcher = RegistrySlashDispatcher::new(shared_command_registry.clone())
        .with_skill_usage_home(cfg.lingxi_home.clone())
        .with_background_prompt_launcher(Arc::new(move |prompt| {
            let handle = background_command_handle.clone();
            Box::pin(async move {
                handle
                    .fork_conversation(&prompt)
                    .await
                    .map(|outcome| {
                        let tail = &outcome.agent_id[outcome.agent_id.len().saturating_sub(4)..];
                        format!(
                            "\u{2442} started code-review in background as {} ({tail})",
                            outcome.name
                        )
                    })
                    .map_err(|error| error.to_string())
            })
        }))
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
        session_writer,
        oauth_supported,
        credentials,
        mobile_linux,
        mcp_registry,
        local_apps_mcp,
        local_apps_llm,
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
/// Test-only bookkeeping for the fire-and-forget authoring/planning tasks
/// [`MobileEngineHandle::spawn_authoring`] / [`MobileEngineHandle::spawn_planning`]
/// launch. Production drops their `JoinHandle` outright — the whole point of
/// those triggers is that `submit(CreateApp)` etc. return before the LLM round
/// trip lands — but a test needs a deterministic way to wait for that round
/// trip to settle without sleeping (a poll loop would work but is exactly the
/// kind of flake-prone timing dependency §0's ban on sleeps exists to avoid).
/// Compiles to a true no-op outside `cfg(test)`, so this can never accumulate
/// handles in a long-running process.
#[cfg(test)]
#[derive(Default)]
struct LocalAppsBackgroundTracker(StdMutex<Vec<tokio::task::JoinHandle<()>>>);

#[cfg(test)]
impl LocalAppsBackgroundTracker {
    fn track(&self, handle: tokio::task::JoinHandle<()>) {
        self.0
            .lock()
            .expect("local-apps background tracker poisoned")
            .push(handle);
    }

    /// Await every handle queued so far, INCLUDING ones a just-awaited task
    /// itself queued (e.g. `update_brief` re-triggering authoring) — loops
    /// until a full pass finds nothing new.
    async fn settle(&self) {
        loop {
            let handles: Vec<_> = {
                let mut guard = self
                    .0
                    .lock()
                    .expect("local-apps background tracker poisoned");
                std::mem::take(&mut *guard)
            };
            if handles.is_empty() {
                break;
            }
            for handle in handles {
                let _ = handle.await;
            }
        }
    }
}

#[cfg(not(test))]
#[derive(Default)]
struct LocalAppsBackgroundTracker;

#[cfg(not(test))]
impl LocalAppsBackgroundTracker {
    fn track(&self, _handle: tokio::task::JoinHandle<()>) {}
}

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
    active_cancel: Arc<Mutex<Option<Arc<ActiveTurn>>>>,
    /// `request_id → tool_name` recorded as each permission request is emitted, so
    /// an inbound `ApprovePermission`/`DenyPermission` (which carries only the
    /// `request_id`) can supply the tool name back to
    /// [`AdapterPermissionGate::resolve`] (needed for the `AllowAlways` rule
    /// append). The mobile analog of bridge-server's `FramePermissionSink` map.
    tool_names: Arc<Mutex<HashMap<u64, String>>>,
    /// Correlates interactive `AskUserQuestion` events with inbound answers.
    ask_user_question_broker: Arc<client_adapter::BridgeAskUserQuestionBroker>,
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
    /// LOCAL-APPS (phase 1): the engine-owned [`AppService`] — the single
    /// source of truth for the on-device "Apps" capability, rebuilt from disk
    /// alone at every boot and rooted at the per-profile data root
    /// (`<app_files_root>/apps/…`, see [`mobile_apps_data_root`]). Held as a
    /// `Result` so a corrupt on-disk store degrades to typed
    /// `AppOperationFailed` replies on every app command instead of bricking
    /// engine construction.
    local_apps: Result<Arc<AppService>, AppError>,
    /// LOCAL-APPS: the bridge-owned ordered emission channel every app-surface
    /// client event rides to the sink — domain events via the installed
    /// `SinkAppEventObserver`, engine-synthesized events (`AppOperationFailed`,
    /// checkpoint reply rows) via the handlers here. One channel ⇒ one total
    /// order (channel order = commit order), and the forwarder task awaits the
    /// sink with NO service lock held (see `local_apps_bridge`).
    app_emissions: crate::local_apps_bridge::AppEmissionQueue,
    /// Host-owned trust boundary for local-app data, runtime, capability and
    /// structured WebView operations.  The MCP provider and native command
    /// surface share this exact broker.
    local_apps_host: Arc<LocalAppsHostBroker>,
    /// Durable single-worker generation queue shared with the AppService
    /// continuation sink.
    app_generation: Arc<AppGenerationCoordinator>,
    /// Keeps the process-wide profile service and fanouts alive.
    profile_apps: Option<Arc<ProfileApps>>,
    app_client_subscription: Option<u64>,
    app_domain_subscription: Option<local_apps::AppEventSubscription>,
    app_domain_observer: Option<Arc<crate::local_apps_bridge::SinkAppEventObserver>>,
    /// See [`LocalAppsBackgroundTracker`] — a true no-op outside `cfg(test)`.
    local_apps_background: LocalAppsBackgroundTracker,
}

impl Drop for MobileEngineHandle {
    fn drop(&mut self) {
        if let Some(profile) = &self.profile_apps {
            if let Some(subscription) = self.app_client_subscription.take() {
                profile.client_events.unsubscribe(subscription);
            }
            if let Some(subscription) = self.app_domain_subscription.take() {
                profile.domain_events.unsubscribe(subscription);
            }
        }
        // Drop the strong observer after unregistering its weak fanout entry.
        self.app_domain_observer.take();
    }
}

/// Default `ListSessions` row cap when the command omits an explicit `limit`
/// (SESSIONS/HISTORY). Mirrors the CLI `/resume` default (`apps/cli/src/run.rs`
/// passes `5`).
const DEFAULT_SESSION_LIST_LIMIT: usize = 5;

/// Connection-scoped ownership record for one streamed turn.
struct ActiveTurn {
    /// Optional client correlator supplied by `SendPrompt`. `Cancel(Some(id))`
    /// may only affect the owner carrying the same id; Android's legacy
    /// `Cancel(None)` intentionally targets whichever turn is current.
    turn_id: Option<u64>,
    cancel: CancellationToken,
    task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
    completed: AtomicBool,
    /// Set before the terminal event is forwarded to the foreign listener.
    /// Any subsequent live-turn event is stale and must be discarded.
    terminal_emitted: AtomicBool,
    completion: Notify,
}

impl ActiveTurn {
    fn new(turn_id: Option<u64>) -> Self {
        Self {
            turn_id,
            cancel: CancellationToken::new(),
            task: StdMutex::new(None),
            completed: AtomicBool::new(false),
            terminal_emitted: AtomicBool::new(false),
            completion: Notify::new(),
        }
    }

    fn set_task_handle(&self, handle: tokio::task::JoinHandle<()>) {
        let mut task = self
            .task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *task = Some(handle);
    }

    fn take_task_handle(&self) -> Option<tokio::task::JoinHandle<()>> {
        let task = self
            .task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        task
    }

    async fn wait_completed(&self) {
        loop {
            if self.completed.load(Ordering::Acquire) {
                return;
            }
            let notified = self.completion.notified();
            tokio::pin!(notified);
            // Register before the second state check so completion cannot be
            // lost between checking and awaiting.
            notified.as_mut().enable();
            if self.completed.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    fn mark_completed(&self) {
        self.completed.store(true, Ordering::Release);
        self.completion.notify_waiters();
    }

    fn matches_cancel(&self, requested_turn_id: Option<u64>) -> bool {
        requested_turn_id.is_none() || self.turn_id == requested_turn_id
    }
}

/// Mobile-only lifecycle guard around the foreign listener. The core protocol
/// intentionally keeps its frozen event shapes, so the host enforces the
/// single-owner invariant at the delivery boundary: one terminal event closes
/// the turn, cancellation rewrites that terminal outcome, and live-turn events
/// emitted after terminal/slot release are discarded.
struct TurnLifecycleListener {
    inner: Arc<dyn ClientEventListener>,
    active_turn: Arc<Mutex<Option<Arc<ActiveTurn>>>>,
}

impl TurnLifecycleListener {
    fn new(
        inner: Arc<dyn ClientEventListener>,
        active_turn: Arc<Mutex<Option<Arc<ActiveTurn>>>>,
    ) -> Self {
        Self { inner, active_turn }
    }

    fn is_live_turn_payload(event: &ClientEvent) -> bool {
        matches!(
            event,
            ClientEvent::AskUserQuestion { .. }
                | ClientEvent::SystemNotice { .. }
                | ClientEvent::TextDelta { .. }
                | ClientEvent::ToolUseStarted { .. }
                | ClientEvent::ToolHeartbeat { .. }
                | ClientEvent::ToolUseResult { .. }
                | ClientEvent::MessageComplete { .. }
                | ClientEvent::CostUpdate { .. }
                | ClientEvent::CompactionCompleted { .. }
                | ClientEvent::CoordinatorStatus { .. }
                | ClientEvent::CoordinatorWorker { .. }
                | ClientEvent::ThinkingDelta { .. }
                | ClientEvent::UsageUpdate { .. }
                | ClientEvent::Attachment { .. }
                | ClientEvent::ApiRetry { .. }
        )
    }
}

#[async_trait]
impl ClientEventListener for TurnLifecycleListener {
    async fn on_event(&self, mut event: ClientEvent) {
        let active = self.active_turn.lock().await.clone();
        let is_live_turn_payload = Self::is_live_turn_payload(&event);
        let should_forward = match &mut event {
            ClientEvent::TurnEnded {
                outcome,
                stop_reason,
                ..
            } => {
                if let Some(turn) = active.as_ref() {
                    if turn.terminal_emitted.swap(true, Ordering::AcqRel) {
                        return;
                    }
                    if turn.cancel.is_cancelled() {
                        *outcome = TurnOutcomeDto::Cancelled;
                        *stop_reason = Some("cancelled".to_string());
                    }
                    true
                } else {
                    false
                }
            }
            ClientEvent::Error { .. } => {
                if let Some(turn) = active.as_ref() {
                    !turn.terminal_emitted.swap(true, Ordering::AcqRel)
                } else {
                    true
                }
            }
            ClientEvent::TurnStarted { turn_id } => active.as_ref().is_some_and(|turn| {
                turn.turn_id == *turn_id && !turn.terminal_emitted.load(Ordering::Acquire)
            }),
            _ if is_live_turn_payload => active
                .as_ref()
                .is_some_and(|turn| !turn.terminal_emitted.load(Ordering::Acquire)),
            // Listing, session, app, task and explicit resolution events are
            // connection-scoped rather than owned by a live conversation turn.
            _ => true,
        };

        if should_forward {
            self.inner.on_event(event).await;
        } else {
            tracing::debug!("mobile: dropped stale live-turn event");
        }
    }
}

impl MobileEngineHandle {
    /// Return a credential-free view over this handle's validated cron store.
    pub async fn cron_store(&self) -> Arc<MobileCronStoreHandle> {
        Arc::new(MobileCronStoreHandle::new(
            self.firer_cfg.cwd.clone(),
            self.firer_platform.filesystem(),
            self.firer_platform.clock(),
        ))
    }

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
    /// has been fired.
    #[doc(hidden)]
    pub async fn active_turn_is_cancelled(&self) -> bool {
        self.active_cancel
            .lock()
            .await
            .as_ref()
            .is_some_and(|turn| turn.cancel.is_cancelled())
    }

    async fn retarget_session_writer(&self, session_id: protocol::SessionId, cwd: &str) {
        let path = orchestrator::transcript_paths::main_transcript_path(
            &self.lingxi_home,
            cwd,
            &session_id.as_uuid().to_string(),
        );
        self.inner.session_writer.retarget(path).await;
    }

    async fn has_mobile_empty_session_anchor(&self, session_id: uuid::Uuid, cwd: &str) -> bool {
        let path = session::jsonl::session_path(&self.lingxi_home, cwd, &session_id.to_string());
        let Some(path) = path.to_str() else {
            return false;
        };
        let Ok(file) = self.fs.read_file(path, None, None).await else {
            return false;
        };
        let expected_session_id = session_id.to_string();
        file.content.lines().any(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .is_some_and(|value| {
                    value.get("type").and_then(serde_json::Value::as_str) == Some("custom-title")
                        && value.get("sessionId").and_then(serde_json::Value::as_str)
                            == Some(expected_session_id.as_str())
                        && value
                            .get("mobileEmptySession")
                            .and_then(serde_json::Value::as_u64)
                            == Some(1)
                })
        })
    }

    async fn persist_mobile_empty_session_anchor(
        &self,
        session_id: uuid::Uuid,
        cwd: &str,
        title: &str,
    ) -> Result<(), ClientError> {
        let path = session::jsonl::session_path(&self.lingxi_home, cwd, &session_id.to_string());
        let writer = session::jsonl::JsonlWriter::new(path, self.fs.clone());
        writer
            .append_mobile_empty_session(
                &session_id.to_string(),
                if title.is_empty() { "新对话" } else { title },
            )
            .await
            .map_err(|error| ClientError::Internal {
                message: format!("persist empty session failed: {error}"),
            })
    }

    async fn resume_session_impl(
        &self,
        session_id: String,
        cwd: Option<String>,
        empty_bootstrap_title: Option<String>,
    ) -> Result<(), ClientError> {
        if self.active_cancel.lock().await.is_some() {
            return Err(ClientError::Rejected {
                message: "cannot resume while a turn is in flight".into(),
            });
        }

        let canonical_session_id = session_id.strip_prefix("sess:").unwrap_or(&session_id);
        let uuid =
            uuid::Uuid::parse_str(canonical_session_id).map_err(|error| ClientError::Rejected {
                message: format!("resume: malformed session id {session_id:?}: {error}"),
            })?;
        let cwd = cwd.unwrap_or_else(|| self.session_cwd.clone());

        match orchestrator::replay_session_state(&self.lingxi_home, &cwd, uuid, self.fs.clone())
            .await
        {
            Ok(replayed) => {
                let handle: Arc<dyn OrchestratorHandle> = self.inner.orchestrator.clone();
                handle
                    .resume_session(
                        protocol::SessionId::from_uuid(uuid),
                        replayed.state.history.clone(),
                        replayed.last_message_uuid.map(|id| id.to_string()),
                        replayed
                            .state
                            .active_goal
                            .clone()
                            .map(|goal| traits::ActiveGoalSnapshot {
                                condition: goal.condition,
                                set_at: goal.set_at,
                                last_reason: goal.last_reason,
                                iterations: goal.iterations,
                                tokens_at_start: goal.tokens_at_start,
                            }),
                        replayed.handle_runtime_snapshot(),
                    )
                    .await
                    .map_err(|error| ClientError::Internal {
                        message: format!("resume_session failed: {error}"),
                    })?;
                self.retarget_session_writer(protocol::SessionId::from_uuid(uuid), &cwd)
                    .await;
                let messages = client_adapter::lowering::lower_transcript(&replayed.state.history);
                self.event_sink
                    .emit(ClientEvent::SessionResumed {
                        session_id: uuid.to_string(),
                        messages,
                    })
                    .await;
                Ok(())
            }
            Err(error) => {
                let is_empty = matches!(
                    &error,
                    orchestrator::resume::ResumeError::Loader(
                        session::jsonl::LoaderError::EmptyDirectory
                    )
                );
                let is_missing = matches!(
                    &error,
                    orchestrator::resume::ResumeError::Loader(
                        session::jsonl::LoaderError::SessionNotFound { .. }
                    )
                );
                let has_anchor = is_empty && self.has_mobile_empty_session_anchor(uuid, &cwd).await;
                let may_bootstrap = empty_bootstrap_title.is_some() && (is_empty || is_missing);
                if !has_anchor && !may_bootstrap {
                    return Err(ClientError::Rejected {
                        message: format!("resume: session {session_id} not resumable: {error}"),
                    });
                }

                if !has_anchor {
                    self.persist_mobile_empty_session_anchor(
                        uuid,
                        &cwd,
                        empty_bootstrap_title.as_deref().unwrap_or("新对话"),
                    )
                    .await?;
                }

                let handle: Arc<dyn OrchestratorHandle> = self.inner.orchestrator.clone();
                handle
                    .resume_session(
                        protocol::SessionId::from_uuid(uuid),
                        Vec::new(),
                        None,
                        None,
                        traits::ResumeRuntimeSnapshot::default(),
                    )
                    .await
                    .map_err(|resume_error| ClientError::Internal {
                        message: format!("resume empty session failed: {resume_error}"),
                    })?;
                self.retarget_session_writer(protocol::SessionId::from_uuid(uuid), &cwd)
                    .await;
                self.event_sink
                    .emit(ClientEvent::SessionResumed {
                        session_id: uuid.to_string(),
                        messages: Vec::new(),
                    })
                    .await;
                Ok(())
            }
        }
    }

    /// Reserve the connection's single turn slot without replacing its owner.
    async fn reserve_turn(&self, turn_id: Option<u64>) -> Result<Arc<ActiveTurn>, ClientError> {
        let mut active = self.active_cancel.lock().await;
        if active.is_some() {
            return Err(ClientError::Rejected {
                message: "a turn is already in flight".into(),
            });
        }
        let turn = Arc::new(ActiveTurn::new(turn_id));
        *active = Some(turn.clone());
        Ok(turn)
    }

    /// Cooperatively cancel exactly the requested turn and wait until every
    /// owned tool/event producer has unwound. `Block` tools deliberately finish
    /// naturally; force-aborting the outer task would violate their mutation
    /// safety contract. A stale specific id is a no-op and cannot drain or
    /// otherwise disturb the current turn.
    async fn cancel_active_turn(&self, requested_turn_id: Option<u64>) -> Result<(), ClientError> {
        let active = self.active_cancel.lock().await.clone();
        let Some(turn) = active else {
            tracing::debug!(
                requested_turn_id,
                "mobile: ignored cancel without an active turn"
            );
            return Ok(());
        };
        if !turn.matches_cancel(requested_turn_id) {
            tracing::debug!(
                requested_turn_id,
                active_turn_id = turn.turn_id,
                "mobile: ignored stale turn cancellation"
            );
            return Ok(());
        }

        let permission_count = self.inner.permission_gate.drain().await;
        let question_count = self.ask_user_question_broker.drain().await;
        self.tool_names.lock().await.clear();
        turn.cancel.cancel();
        tracing::debug!(
            requested_turn_id,
            active_turn_id = turn.turn_id,
            permission_count,
            question_count,
            "mobile: waiting for cancelled turn to release its owner slot"
        );

        if let Some(task) = turn.take_task_handle() {
            if task.await.is_err() {
                let mut active = self.active_cancel.lock().await;
                if active
                    .as_ref()
                    .is_some_and(|owner| Arc::ptr_eq(owner, &turn))
                {
                    *active = None;
                }
                drop(active);
                turn.mark_completed();
                self.event_sink
                    .emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: "turn task terminated unexpectedly".to_string(),
                    })
                    .await;
            }
        } else {
            // Another concurrent Cancel may own the JoinHandle. The completion
            // notification still gives every caller the same release guarantee.
            turn.wait_completed().await;
        }
        tracing::debug!(active_turn_id = turn.turn_id, "mobile: cancel completed");
        Ok(())
    }

    /// Start one streamed turn and release its slot on every normal return path
    /// (success, orchestrator error, or cancellation). Pointer ownership keeps a
    /// finishing task from clearing a newer reservation.
    async fn start_streaming_turn(
        &self,
        text: String,
        turn_id: Option<u64>,
    ) -> Result<(), ClientError> {
        let turn = self.reserve_turn(turn_id).await?;

        #[cfg(debug_assertions)]
        eprintln!(
            "[turn-diagnostic] turn started client_turn_id={}",
            turn_id.map_or_else(|| "none".to_string(), |id| id.to_string())
        );

        let wrapper = TurnWrapper::new(self.event_sink.clone());
        wrapper.emit_turn_started(turn_id).await;

        let orch = self.inner.orchestrator.clone();
        let sink = self.event_sink.clone();
        let active_cancel = self.active_cancel.clone();
        let task_turn = turn.clone();
        let task = self.runtime.spawn(async move {
            let result = orch
                .run_turn_streaming_with_cancel(&text, task_turn.cancel.clone())
                .await;
            if let Err(err) = &result {
                sink.emit(client_adapter::map_orchestrator_error(err)).await;
            }

            #[cfg(debug_assertions)]
            eprintln!(
                "[turn-diagnostic] turn future returned cancelled={} result_ok={}",
                task_turn.cancel.is_cancelled(),
                result.is_ok()
            );

            let mut active = active_cancel.lock().await;
            if active
                .as_ref()
                .is_some_and(|owner| Arc::ptr_eq(owner, &task_turn))
            {
                *active = None;
            }
            drop(active);
            // Notify only after the slot is released: Cancel returning is the
            // guarantee that New/Resume/Clear can no longer observe this turn.
            task_turn.mark_completed();
        });
        turn.set_task_handle(task);
        Ok(())
    }

    async fn read_mobile_linux_status(
        runtime: &dyn MobileLinuxRuntime,
    ) -> Result<MobileLinuxStatusDto, MobileEngineError> {
        let capability = runtime.probe_capability().await;
        let status = runtime
            .rootfs_status()
            .await
            .map_err(|e| MobileEngineError::Internal(format!("mobile_linux_status failed: {e}")))?;
        Ok(lower_mobile_linux_status(capability, status))
    }

    // ── Local apps (phase 1) ────────────────────────────────────────────────
    //
    // The `submit` arms below delegate here. Failures are DOMAIN outcomes, not
    // transport errors: every arm resolves `Ok(())` and surfaces its failure as
    // a typed `AppOperationFailed { code, message }` event, the single failure
    // channel the spec gives app clients. Successful mutations additionally
    // announce the new record set via `AppsChanged` (create/delete already ride
    // the service's own `AppsChanged` domain event, so only the other mutating
    // arms re-emit it here).

    /// The engine-owned local-apps service, or the boot-time load error
    /// (surfaced by [`Self::local_apps_or_report`] as `AppOperationFailed` on
    /// every app command). Exposed for tests and the phase-3 generator, which
    /// drive the generation/validation transitions the command surface does
    /// not carry.
    ///
    /// # Errors
    ///
    /// The boot-time [`AppError`] when the on-disk store failed to load.
    pub fn local_apps(&self) -> Result<Arc<AppService>, AppError> {
        self.local_apps.clone()
    }

    /// Test-only seam: swap the profile's [`LocalAppsLlm`] for a scripted
    /// double, exercising the SAME [`SharedLlm::replace`] path a real
    /// reconnect / `/model` switch takes (Task 11's `profile_apps` fix), so
    /// authoring/planning tests are deterministic without a network.
    #[cfg(test)]
    fn set_local_apps_model(&self, model: Arc<dyn crate::local_apps_llm::LocalAppsModel>) {
        if let Some(profile) = &self.profile_apps {
            profile.llm.replace(Arc::new(LocalAppsLlm::new(model)));
        }
    }

    /// Test-only: await every authoring/planning task spawned so far. See
    /// [`LocalAppsBackgroundTracker`].
    #[cfg(test)]
    async fn settle_local_apps(&self) {
        self.local_apps_background.settle().await;
    }

    /// The live service, or emit the boot-time load failure and yield `None`.
    async fn local_apps_or_report(&self, app_id: Option<&str>) -> Option<Arc<AppService>> {
        match &self.local_apps {
            Ok(service) => Some(service.clone()),
            Err(error) => {
                self.emit_app_failure(app_id.map(str::to_string), error)
                    .await;
                None
            }
        }
    }

    /// Lower one typed [`AppError`] onto the `AppOperationFailed` event,
    /// routed through the bridge's ordered emission channel so every
    /// app-surface event shares one total order. When the service is alive
    /// the queue flushes its emission tasks first, so the synthesized failure
    /// can never overtake the domain events of its own cause (e.g.
    /// `AppDesignConflict` always precedes the `revision_conflict` failure).
    async fn emit_app_failure(&self, app_id: Option<String>, error: &AppError) {
        let service = self.local_apps.as_ref().ok().cloned();
        self.app_emissions
            .emit_failure(service.as_deref(), app_id, error)
            .await;
    }

    fn emit_app_event(&self, event: AppEventDto) {
        self.app_emissions
            .enqueue_engine(ClientEvent::AppEvent { event });
    }

    /// Post-mutation `AppsChanged` snapshot: every successful mutation
    /// announces the full record set (records carry `workflow_state` /
    /// `updated_at_ms`, so any mutation changes the set). Delegated to
    /// [`AppService::announce_apps`] — the snapshot and its emission ride the
    /// service's emission-order lock (through the installed
    /// `SinkAppEventObserver`), so a concurrent mutation on another `submit`
    /// can never get its events overtaken by a stale snapshot.
    async fn emit_apps_snapshot(service: &AppService) {
        service.announce_apps().await;
    }

    /// Await one detached app-mutation task (the seven mutate-then-announce
    /// handlers below). The body — service mutation AND its post-mutation
    /// snapshot announce — runs in a task spawned on the engine runtime, so
    /// dropping the `submit` future that awaits here abandons the AWAIT, not
    /// the work: a mutation that commits always gets its `AppsChanged`
    /// (contract C8), mirroring the core's dropped-caller completion-task
    /// hardening. A panic inside the body is resumed on the caller (the same
    /// observable behavior as running the body inline); a cancelled join can
    /// only mean the runtime itself is shutting down, where nothing is left
    /// to do.
    async fn join_app_mutation(task: tokio::task::JoinHandle<()>) {
        if let Err(error) = task.await {
            if error.is_panic() {
                std::panic::resume_unwind(error.into_panic());
            }
        }
    }

    /// As [`Self::join_app_mutation`], but the task also reports ITS OWN
    /// mutation's outcome — used by the four handlers below that gate a
    /// SEPARATE, un-joined background LLM round trip (authoring/planning) on
    /// it. `T` is `Option<u64>` at every call site: `Some(epoch)` on success
    /// (the `llm_round` the mutation just bumped to, handed straight to the
    /// freshly spawned task), `None` on failure. A cancelled join (runtime
    /// shutting down) reports `T::default()` (`None`): there is nothing left
    /// to trigger.
    async fn join_app_mutation_outcome<T: Default>(task: tokio::task::JoinHandle<T>) -> T {
        match task.await {
            Ok(outcome) => outcome,
            Err(error) => {
                if error.is_panic() {
                    std::panic::resume_unwind(error.into_panic());
                }
                T::default()
            }
        }
    }

    async fn handle_list_apps(&self) {
        let Some(service) = self.local_apps_or_report(None).await else {
            return;
        };
        Self::emit_apps_snapshot(&service).await;
    }

    async fn handle_get_app_details(&self, app_id: String) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let root = mobile_apps_data_root(&self.firer_cfg);
        let result = async {
            let record = service.record(&app_id).await?;
            let draft = service.draft(&app_id).await?;
            let runtime = service.runtime_record(&app_id).await?;
            let checkpoints = service.list_checkpoints(&app_id).await?;
            let mut details = crate::local_apps_bridge::lower_details(
                &root,
                &record,
                &draft,
                &runtime,
                &checkpoints,
            )?;
            details.generation_job = self
                .app_generation
                .jobs_for_app(&app_id)
                .await?
                .into_iter()
                .next()
                .map(lower_job);
            Ok(details)
        }
        .await;
        match result {
            Ok(details) => self.emit_app_event(AppEventDto::AppDetailsChanged { details }),
            Err(error) => self.emit_app_failure(Some(app_id), &error).await,
        }
    }

    async fn handle_create_app(
        &self,
        name: &str,
        origin: AppCreateOriginDto,
        brief: &str,
        conversation_id: Option<String>,
    ) {
        let Some(service) = self.local_apps_or_report(None).await else {
            return;
        };
        // Raising the origin is fallible like every other inbound DTO raise
        // (W1): an unknown `#[non_exhaustive]` future origin must fail typed
        // instead of silently laundering into a library create.
        let origin = match crate::local_apps_bridge::raise_origin(origin) {
            Ok(origin) => origin,
            Err(error) => {
                self.emit_app_failure(None, &error).await;
                return;
            }
        };
        // The record keeps a conversation binding only for chat-origin creates
        // (`AppRecord.conversation_id` doc: "origin: chat"); a library create
        // never binds one. Derived from the RAISED origin (an exhaustive
        // match — see `AppCreateOrigin::conversation_binding`).
        let conversation_id = origin.conversation_binding(conversation_id);
        // Success needs no extra emit: `create_app` announces the new record
        // set via its own `AppsChanged` domain event. `brief` is the LLM's
        // real seed now (Task 11) — `name` is a display label, never the
        // spec the questionnaire gets authored from; see
        // `create_app_persists_the_caller_supplied_brief_and_does_not_overwrite_a_supplied_name`.
        match service.create_app(Some(name), brief, conversation_id).await {
            Ok(record) => {
                let epoch = record.llm_round;
                self.trigger_authoring(&service, record.id, epoch);
            }
            Err(error) => self.emit_app_failure(None, &error).await,
        }
    }

    async fn handle_update_app_brief(&self, app_id: String, brief: String) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let emissions = self.app_emissions.clone();
        let task_service = service.clone();
        let task_app_id = app_id.clone();
        let epoch: Option<u64> =
            Self::join_app_mutation_outcome(self.runtime.handle().spawn(async move {
                match task_service.update_brief(&task_app_id, &brief).await {
                    Ok(epoch) => {
                        Self::emit_apps_snapshot(&task_service).await;
                        Some(epoch)
                    }
                    Err(error) => {
                        emissions
                            .emit_failure(Some(&task_service), Some(task_app_id), &error)
                            .await;
                        None
                    }
                }
            }))
            .await;
        if let Some(epoch) = epoch {
            self.trigger_authoring(&service, app_id, epoch);
        }
    }

    async fn handle_retry_app_questionnaire(&self, app_id: String) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let emissions = self.app_emissions.clone();
        let task_service = service.clone();
        let task_app_id = app_id.clone();
        let epoch: Option<u64> =
            Self::join_app_mutation_outcome(self.runtime.handle().spawn(async move {
                match task_service.retry_questionnaire(&task_app_id).await {
                    Ok(epoch) => {
                        Self::emit_apps_snapshot(&task_service).await;
                        Some(epoch)
                    }
                    Err(error) => {
                        emissions
                            .emit_failure(Some(&task_service), Some(task_app_id), &error)
                            .await;
                        None
                    }
                }
            }))
            .await;
        if let Some(epoch) = epoch {
            self.trigger_authoring(&service, app_id, epoch);
        }
    }

    async fn handle_begin_app_planning(&self, app_id: String) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let emissions = self.app_emissions.clone();
        let task_service = service.clone();
        let task_app_id = app_id.clone();
        // `begin_planning` validates the collected answers are self-consistent
        // BEFORE flipping to `planning` — a failure here never starts the
        // background plan round trip.
        let epoch: Option<u64> =
            Self::join_app_mutation_outcome(self.runtime.handle().spawn(async move {
                match task_service.begin_planning(&task_app_id).await {
                    Ok(epoch) => {
                        Self::emit_apps_snapshot(&task_service).await;
                        Some(epoch)
                    }
                    Err(error) => {
                        emissions
                            .emit_failure(Some(&task_service), Some(task_app_id), &error)
                            .await;
                        None
                    }
                }
            }))
            .await;
        if let Some(epoch) = epoch {
            self.trigger_planning(&service, app_id, epoch);
        }
    }

    async fn handle_retry_app_plan(&self, app_id: String) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let emissions = self.app_emissions.clone();
        let task_service = service.clone();
        let task_app_id = app_id.clone();
        let epoch: Option<u64> =
            Self::join_app_mutation_outcome(self.runtime.handle().spawn(async move {
                match task_service.retry_plan(&task_app_id).await {
                    Ok(epoch) => {
                        Self::emit_apps_snapshot(&task_service).await;
                        Some(epoch)
                    }
                    Err(error) => {
                        emissions
                            .emit_failure(Some(&task_service), Some(task_app_id), &error)
                            .await;
                        None
                    }
                }
            }))
            .await;
        if let Some(epoch) = epoch {
            self.trigger_planning(&service, app_id, epoch);
        }
    }

    /// Kick off background questionnaire authoring for `app_id`, fire-and-
    /// forget from the caller's perspective: `submit` has already returned
    /// (or is about to) by the time the LLM round trip lands. The eventual
    /// `questionnaire_ready` / `questionnaire_failed` transition and its
    /// `AppsChanged` snapshot ride the same app-emission channel as every
    /// other app event. Delegates to the shared
    /// [`crate::local_apps_profile::spawn_authoring`] — see its doc for why
    /// this is a free function and why it runs on
    /// [`crate::local_apps_profile::worker_runtime`] rather than
    /// `self.runtime`.
    fn trigger_authoring(&self, service: &Arc<AppService>, app_id: String, epoch: u64) {
        // `self.local_apps` was `Ok` (checked by every caller via
        // `local_apps_or_report`) iff `self.profile_apps` is `Some` — both are
        // set together from the same `loaded_profile` match at build time.
        // `debug_assert!` because a violation here is the SAME permanent
        // hang this whole task exists to close, just via a different door —
        // cheap enough to check even in release (a `tracing::error!` fires
        // there too), since silently returning is exactly the failure mode
        // under review.
        let Some(profile) = &self.profile_apps else {
            debug_assert!(
                false,
                "trigger_authoring called with local_apps Ok but profile_apps None"
            );
            tracing::error!(
                app_id,
                "local-apps profile unavailable; authoring was not triggered — the app is \
                 stuck in authoring_questionnaire with no recovery until an engine restart"
            );
            return;
        };
        let notifier: Arc<dyn crate::local_apps_profile::AppFailureNotifier> =
            Arc::new(self.app_emissions.clone());
        let handle = crate::local_apps_profile::spawn_authoring(
            service.clone(),
            profile.llm.current(),
            notifier,
            app_id,
            epoch,
        );
        self.local_apps_background.track(handle);
    }

    /// As [`Self::trigger_authoring`], for background plan derivation.
    fn trigger_planning(&self, service: &Arc<AppService>, app_id: String, epoch: u64) {
        let Some(profile) = &self.profile_apps else {
            debug_assert!(
                false,
                "trigger_planning called with local_apps Ok but profile_apps None"
            );
            tracing::error!(
                app_id,
                "local-apps profile unavailable; planning was not triggered — the app is \
                 stuck in planning with no recovery until an engine restart"
            );
            return;
        };
        let notifier: Arc<dyn crate::local_apps_profile::AppFailureNotifier> =
            Arc::new(self.app_emissions.clone());
        let handle = crate::local_apps_profile::spawn_planning(
            service.clone(),
            profile.llm.current(),
            notifier,
            app_id,
            epoch,
        );
        self.local_apps_background.track(handle);
    }

    // The seven mutate-then-announce handlers below are cancellation-atomic:
    // each body runs in a detached task joined via `join_app_mutation`, so a
    // caller dropped after the mutation commits cannot lose the mutation's
    // only `AppsChanged` snapshot. (`CreateApp` / `DeleteApp` need no such
    // wrap for their announce: it rides the core's own completion task.)

    async fn handle_open_app_designer(&self, app_id: String) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let emissions = self.app_emissions.clone();
        Self::join_app_mutation(self.runtime.handle().spawn(async move {
            match service.open_designer(&app_id).await {
                Ok(_interaction) => Self::emit_apps_snapshot(&service).await,
                Err(error) => {
                    emissions
                        .emit_failure(Some(&service), Some(app_id), &error)
                        .await;
                }
            }
        }))
        .await;
    }

    async fn handle_update_app_design_draft(
        &self,
        app_id: String,
        expected_revision: u64,
        patch: AppDesignPatchDto,
    ) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let patch = match crate::local_apps_bridge::raise_patch(patch) {
            Ok(patch) => patch,
            Err(error) => {
                self.emit_app_failure(Some(app_id), &error).await;
                return;
            }
        };
        let emissions = self.app_emissions.clone();
        Self::join_app_mutation(self.runtime.handle().spawn(async move {
            match service
                .update_draft(&app_id, expected_revision, &patch)
                .await
            {
                Ok(_revision) => Self::emit_apps_snapshot(&service).await,
                // A stale revision already emitted `AppDesignConflict` from
                // the service; the typed failure rides BEHIND it on the
                // ordered emission channel.
                Err(error) => {
                    emissions
                        .emit_failure(Some(&service), Some(app_id), &error)
                        .await;
                }
            }
        }))
        .await;
    }

    async fn handle_apply_agent_design_suggestion(
        &self,
        app_id: String,
        suggestion_id: &str,
        expected_revision: u64,
    ) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let emissions = self.app_emissions.clone();
        let suggestion_id = suggestion_id.to_string();
        Self::join_app_mutation(self.runtime.handle().spawn(async move {
            match service
                .apply_suggestion(&app_id, &suggestion_id, expected_revision)
                .await
            {
                Ok(_revision) => Self::emit_apps_snapshot(&service).await,
                Err(error) => {
                    emissions
                        .emit_failure(Some(&service), Some(app_id), &error)
                        .await;
                }
            }
        }))
        .await;
    }

    async fn handle_request_app_design_suggestion(
        &self,
        app_id: String,
        _expected_revision: u64,
        _prompt: Option<String>,
    ) {
        // TODO(local-apps#questionnaire, Task 8): this used to build a
        // suggested patch (`build_design_suggestion`, deleted here) by
        // looking up the app's `AppTemplateKind` in the static built-in
        // template catalog — the core no longer has a per-app template at
        // all (Task 2), so that lookup has no input anymore. Task 8 replaces
        // it with a real LLM-driven suggestion call. No existing test
        // exercises this command (`RequestAppDesignSuggestion` is dispatched
        // only from here; the design-suggestion tests in this file drive
        // `AppService::store_suggestion` directly, bypassing this handler
        // entirely), so failing loudly here is a pure gap-close, not a
        // behavior regression.
        if self.local_apps_or_report(Some(&app_id)).await.is_none() {
            return;
        }
        self.emit_app_failure(
            Some(app_id),
            &AppError::NotYetAvailable(
                "agent design suggestions are not yet wired to the LLM (Task 8 replaces the \
                 template-driven suggester)"
                    .into(),
            ),
        )
        .await;
    }

    async fn handle_dismiss_app_design_suggestion(&self, app_id: String, suggestion_id: String) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let emissions = self.app_emissions.clone();
        Self::join_app_mutation(self.runtime.handle().spawn(async move {
            match service.dismiss_suggestion(&app_id, &suggestion_id).await {
                Ok(()) => Self::emit_apps_snapshot(&service).await,
                Err(error) => {
                    emissions
                        .emit_failure(Some(&service), Some(app_id), &error)
                        .await;
                }
            }
        }))
        .await;
    }

    async fn handle_confirm_app_design(&self, app_id: String, interaction_id: &str, revision: u64) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let emissions = self.app_emissions.clone();
        let interaction_id = interaction_id.to_string();
        Self::join_app_mutation(self.runtime.handle().spawn(async move {
            match service
                .confirm_design(&app_id, &interaction_id, revision)
                .await
            {
                Ok(()) => Self::emit_apps_snapshot(&service).await,
                Err(error) => {
                    emissions
                        .emit_failure(Some(&service), Some(app_id), &error)
                        .await;
                }
            }
        }))
        .await;
    }

    async fn handle_cancel_app_design(&self, app_id: String) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let emissions = self.app_emissions.clone();
        Self::join_app_mutation(self.runtime.handle().spawn(async move {
            match service.cancel_design(&app_id).await {
                Ok(()) => Self::emit_apps_snapshot(&service).await,
                Err(error) => {
                    emissions
                        .emit_failure(Some(&service), Some(app_id), &error)
                        .await;
                }
            }
        }))
        .await;
    }

    async fn handle_confirm_app_preview(
        &self,
        app_id: String,
        interaction_id: &str,
        revision: u64,
    ) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let emissions = self.app_emissions.clone();
        let interaction_id = interaction_id.to_string();
        Self::join_app_mutation(self.runtime.handle().spawn(async move {
            match service
                .confirm_preview(&app_id, &interaction_id, revision)
                .await
            {
                Ok(()) => Self::emit_apps_snapshot(&service).await,
                Err(error) => {
                    emissions
                        .emit_failure(Some(&service), Some(app_id), &error)
                        .await;
                }
            }
        }))
        .await;
    }

    async fn handle_request_app_revision(&self, app_id: String, prompt: &str) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        let emissions = self.app_emissions.clone();
        let prompt = prompt.to_string();
        Self::join_app_mutation(self.runtime.handle().spawn(async move {
            match service.request_revision(&app_id, &prompt).await {
                Ok(()) => Self::emit_apps_snapshot(&service).await,
                Err(error) => {
                    emissions
                        .emit_failure(Some(&service), Some(app_id), &error)
                        .await;
                }
            }
        }))
        .await;
    }

    async fn handle_retry_app_generation(&self, app_id: String) {
        let Some(_service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        if let Err(error) = self.app_generation.retry_app(&app_id).await {
            self.emit_app_failure(Some(app_id), &error).await;
        }
    }

    async fn handle_list_app_checkpoints(&self, app_id: String) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        match service.list_checkpoints(&app_id).await {
            Ok(checkpoints) => {
                self.emit_app_event(AppEventDto::AppCheckpointsChanged {
                    app_id,
                    checkpoints: checkpoints
                        .iter()
                        .map(crate::local_apps_bridge::lower_checkpoint)
                        .collect(),
                });
            }
            Err(error) => self.emit_app_failure(Some(app_id), &error).await,
        }
    }

    async fn handle_app_runtime_action(&self, app_id: String, action: &str) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        if let Err(error) = service.record(&app_id).await {
            self.emit_app_failure(Some(app_id), &error).await;
            return;
        }
        let input = serde_json::json!({ "app_id": app_id.clone(), "action": action });
        if let Err(message) = self.local_apps_host.manage_runtime_value(input).await {
            self.emit_app_failure(
                Some(app_id),
                &AppError::Io(format!("local app runtime {action} failed: {message}")),
            )
            .await;
        }
    }

    async fn handle_restore_app_checkpoint(&self, app_id: String, checkpoint_id: String) {
        let input = serde_json::json!({
            "app_id": app_id.clone(),
            "checkpoint_id": checkpoint_id,
        });
        match self.local_apps_host.restore_checkpoint_value(input).await {
            Ok(_) => self.handle_list_app_checkpoints(app_id).await,
            Err(message) => {
                self.emit_app_failure(
                    Some(app_id),
                    &AppError::Io(format!("restore checkpoint failed: {message}")),
                )
                .await;
            }
        }
    }

    async fn handle_delete_app(&self, app_id: String) {
        let Some(service) = self.local_apps_or_report(Some(&app_id)).await else {
            return;
        };
        if let Err(message) = self
            .local_apps_host
            .manage_runtime_value(serde_json::json!({
                "app_id": app_id.clone(),
                "action": "stop",
            }))
            .await
        {
            self.emit_app_failure(
                Some(app_id),
                &AppError::Io(format!("stop local app before delete failed: {message}")),
            )
            .await;
            return;
        }
        // Success needs no extra emit: `delete_app` announces the shrunken
        // record set via its own `AppsChanged` domain event.
        if let Err(error) = service.delete_app(&app_id).await {
            self.emit_app_failure(Some(app_id), &error).await;
        }
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
    /// Resume a confirmed zero-message mobile session without changing its UUID.
    ///
    /// Android persists `SessionStarted` immediately in its Project index. Older
    /// versions did so before the engine wrote any JSONL file, so this explicit
    /// entrypoint is the migration-safe proof that a missing transcript is an
    /// intended empty session rather than lost conversation data. If a valid
    /// transcript now exists, it is replayed normally instead of being cleared.
    pub async fn resume_empty_session(
        &self,
        session_id: String,
        title: String,
    ) -> Result<(), ClientError> {
        self.resume_session_impl(session_id, None, Some(title))
            .await
    }

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
    ///   transcript (lowered via `client_adapter::lowering::lower_transcript`).
    ///   An explicitly anchored mobile zero-message session restores with an empty
    ///   transcript and the same UUID. Other missing, corrupt, or malformed
    ///   sessions are honestly `Rejected` — we never emit a false
    ///   `SessionResumed`.
    ///
    /// - The local-apps commands (`ListApps` / `CreateApp` / … / `DeleteApp`)
    ///   → the engine-owned [`AppService`] (LOCAL-APPS phase 1): domain events
    ///   lower onto the `App*` client events, every failure surfaces as a
    ///   typed `AppOperationFailed { code, message }`, and the runtime /
    ///   checkpoint commands honestly fail `not_yet_available` (runtime is
    ///   phase 4, git checkpoints are phase 5).
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
                self.start_streaming_turn(text, turn_id).await
            }

            ClientCommand::Cancel { turn_id } => self.cancel_active_turn(turn_id).await,

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
            ClientCommand::AnswerAskUserQuestion {
                request_id,
                answers,
            } => {
                if !self
                    .ask_user_question_broker
                    .resolve(request_id, answers)
                    .await
                {
                    tracing::debug!(
                        request_id,
                        "mobile: resolve for unknown / already-resolved AskUserQuestion id"
                    );
                }
                Ok(())
            }
            ClientCommand::CancelAskUserQuestion { request_id } => {
                if !self.ask_user_question_broker.cancel(request_id).await {
                    tracing::debug!(
                        request_id,
                        "mobile: cancel for unknown / already-resolved AskUserQuestion id"
                    );
                }
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

            // ── Provider credentials ────────────────────────────────────────
            // Mobile uses the same CredentialManager instance as the live
            // multi-provider client. Settings writes therefore become visible
            // to the next request immediately; secret values are never echoed.
            ClientCommand::ListProviderCredentials {
                operation_id,
                provider_ids,
            } => {
                let validation_error = if provider_ids.len() > 32
                    || provider_ids.iter().any(|id| !provider_id_is_valid(id))
                {
                    Some("invalid provider credential query".to_string())
                } else {
                    None
                };
                self.emit_provider_credential_status(operation_id, &provider_ids, validation_error)
                    .await;
                Ok(())
            }
            ClientCommand::SetProviderCredential {
                operation_id,
                provider_id,
                credential,
            } => {
                let error = if !provider_id_is_valid(&provider_id)
                    || credential.expose_secret().is_empty()
                    || credential.expose_secret().len() > 16_384
                    || credential.expose_secret().contains('\0')
                {
                    Some("invalid provider credential".to_string())
                } else {
                    self.inner
                        .credentials
                        .set_provider_key(&provider_id, credential.expose_secret())
                        .await
                        .err()
                        .map(|failure| format!("failed to store provider credential: {failure}"))
                };
                let applied = error.is_none();
                self.event_sink
                    .emit(ClientEvent::ProviderCredentialStatus {
                        operation_id,
                        configured_provider_ids: applied
                            .then_some(provider_id.clone())
                            .into_iter()
                            .collect(),
                        unavailable_provider_ids: (!applied)
                            .then_some(provider_id)
                            .into_iter()
                            .collect(),
                        storage_encrypted: self
                            .inner
                            .credentials
                            .provider_key_storage_is_encrypted(),
                        error,
                    })
                    .await;
                Ok(())
            }
            ClientCommand::DeleteProviderCredential {
                operation_id,
                provider_id,
            } => {
                let error = if !provider_id_is_valid(&provider_id) {
                    Some("invalid provider id".to_string())
                } else {
                    self.inner
                        .credentials
                        .delete_provider_key(&provider_id)
                        .await
                        .err()
                        .map(|failure| format!("failed to delete provider credential: {failure}"))
                };
                let applied = error.is_none();
                self.event_sink
                    .emit(ClientEvent::ProviderCredentialStatus {
                        operation_id,
                        configured_provider_ids: Vec::new(),
                        unavailable_provider_ids: (!applied)
                            .then_some(provider_id)
                            .into_iter()
                            .collect(),
                        storage_encrypted: self
                            .inner
                            .credentials
                            .provider_key_storage_is_encrypted(),
                        error,
                    })
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
                // The local-app LLM stages (author/plan/write-source) ride
                // their OWN `ApiServiceModel`, not the orchestrator's model
                // selection — without this they would stay silently pinned
                // to whatever was live at engine build time even after a
                // `/model` switch. `local_apps_llm` is stable for this
                // connection's whole lifetime (only a reconnect gets a new
                // one, via `profile_apps`'s `SharedLlm::replace`), so
                // mutating it in place here is exactly the model every
                // future authoring/planning/generation call will read.
                self.inner
                    .local_apps_llm
                    .set_model(model_id.clone(), profile.clone());
                let snapshot = handle.get_status_snapshot().await;
                let selected =
                    traits::qualified_model_ref(&snapshot.model, snapshot.model_profile.as_deref());
                self.event_sink
                    .emit(ClientEvent::ModelChanged { model: selected })
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
                        self.start_streaming_turn(prompt, None).await?;
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
                let mid_turn = self.active_cancel.lock().await.is_some();
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
                self.retarget_session_writer(handle.current_session_id().await, &self.session_cwd)
                    .await;
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
                let mid_turn = self.active_cancel.lock().await.is_some();
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
                let new_session_id = handle.current_session_id().await;
                self.retarget_session_writer(new_session_id, &self.session_cwd)
                    .await;
                self.inner
                    .session_writer
                    .append_mobile_empty_session(&new_session_id.as_uuid().to_string(), "新对话")
                    .await
                    .map_err(|error| ClientError::Internal {
                        message: format!("new session anchor failed: {error}"),
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
                // Mobile clients persist this value as the resumable catalog key.
                // `SessionId::Display` is presentation-oriented (`sess:<uuid>`),
                // while the JSONL filename and ResumeSession contract use the
                // bare UUID. Never leak the display prefix into persisted state.
                let session_id = new_session_id.as_uuid().to_string();
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
            // renders the rehydrated conversation atomically. An explicitly
            // anchored mobile zero-message session restores with an empty
            // transcript and the same UUID. Other missing, corrupt, or malformed
            // sessions are honestly `Rejected`, so we never emit a FALSE
            // `SessionResumed`.
            ClientCommand::ResumeSession { session_id, cwd } => {
                self.resume_session_impl(session_id, cwd, None).await
            }

            // ── Local apps (LOCAL-APPS phase 1) ─────────────────────────────
            //
            // The 15 app commands route to the engine-owned `AppService` (the
            // single source of truth for the on-device "Apps" capability).
            // Failures are domain outcomes, not transport errors: each arm
            // resolves `Ok(())` and surfaces its failure as a typed
            // `AppOperationFailed { code, message }` event (see the handler
            // section in the plain impl block above).
            ClientCommand::ListApps => {
                self.handle_list_apps().await;
                Ok(())
            }
            ClientCommand::GetAppDetails { app_id } => {
                self.handle_get_app_details(app_id).await;
                Ok(())
            }
            ClientCommand::CreateApp {
                name,
                origin,
                brief,
                conversation_id,
            } => {
                self.handle_create_app(&name, origin, &brief, conversation_id)
                    .await;
                Ok(())
            }
            ClientCommand::UpdateAppBrief { app_id, brief } => {
                self.handle_update_app_brief(app_id, brief).await;
                Ok(())
            }
            ClientCommand::RetryAppQuestionnaire { app_id } => {
                self.handle_retry_app_questionnaire(app_id).await;
                Ok(())
            }
            ClientCommand::BeginAppPlanning { app_id } => {
                self.handle_begin_app_planning(app_id).await;
                Ok(())
            }
            ClientCommand::RetryAppPlan { app_id } => {
                self.handle_retry_app_plan(app_id).await;
                Ok(())
            }
            ClientCommand::OpenAppDesigner { app_id } => {
                self.handle_open_app_designer(app_id).await;
                Ok(())
            }
            ClientCommand::UpdateAppDesignDraft {
                app_id,
                expected_revision,
                patch,
            } => {
                self.handle_update_app_design_draft(app_id, expected_revision, patch)
                    .await;
                Ok(())
            }
            ClientCommand::ApplyAgentDesignSuggestion {
                app_id,
                suggestion_id,
                expected_revision,
            } => {
                self.handle_apply_agent_design_suggestion(
                    app_id,
                    &suggestion_id,
                    expected_revision,
                )
                .await;
                Ok(())
            }
            ClientCommand::RequestAppDesignSuggestion {
                app_id,
                expected_revision,
                prompt,
            } => {
                self.handle_request_app_design_suggestion(app_id, expected_revision, prompt)
                    .await;
                Ok(())
            }
            ClientCommand::DismissAppDesignSuggestion {
                app_id,
                suggestion_id,
            } => {
                self.handle_dismiss_app_design_suggestion(app_id, suggestion_id)
                    .await;
                Ok(())
            }
            ClientCommand::ConfirmAppDesign {
                app_id,
                revision,
                interaction_id,
            } => {
                self.handle_confirm_app_design(app_id, &interaction_id, revision)
                    .await;
                Ok(())
            }
            ClientCommand::CancelAppDesign { app_id } => {
                self.handle_cancel_app_design(app_id).await;
                Ok(())
            }
            ClientCommand::StartApp { app_id } => {
                self.handle_app_runtime_action(app_id, "start").await;
                Ok(())
            }
            ClientCommand::StopApp { app_id } => {
                self.handle_app_runtime_action(app_id, "stop").await;
                Ok(())
            }
            ClientCommand::RestartApp { app_id } => {
                self.handle_app_runtime_action(app_id, "restart").await;
                Ok(())
            }
            ClientCommand::ConfirmAppPreview {
                app_id,
                revision,
                interaction_id,
            } => {
                self.handle_confirm_app_preview(app_id, &interaction_id, revision)
                    .await;
                Ok(())
            }
            ClientCommand::RequestAppRevision { app_id, prompt } => {
                self.handle_request_app_revision(app_id, &prompt).await;
                Ok(())
            }
            ClientCommand::RetryAppGeneration { app_id } => {
                self.handle_retry_app_generation(app_id).await;
                Ok(())
            }
            ClientCommand::ExecuteAppBridgeRequest { request } => {
                self.local_apps_host.execute_bridge(request).await;
                Ok(())
            }
            ClientCommand::ResolveAppUiRequest {
                request_id,
                decision,
                result_json,
                error,
            } => {
                if !self
                    .local_apps_host
                    .resolve_ui(&request_id, decision, result_json, error)
                    .await
                {
                    tracing::debug!(request_id, "unknown or completed local-app UI request");
                }
                Ok(())
            }
            ClientCommand::ResolveAppCapabilityRequest {
                request_id,
                decision,
            } => {
                if !self
                    .local_apps_host
                    .resolve_capability(&request_id, decision)
                    .await
                {
                    tracing::debug!(
                        request_id,
                        "unknown or completed local-app capability request"
                    );
                }
                Ok(())
            }
            ClientCommand::ResetAppPermissions { app_id } => {
                if let Err(message) = self.local_apps_host.reset_permissions(&app_id).await {
                    self.emit_app_failure(
                        Some(app_id),
                        &AppError::Io(format!("reset app permissions failed: {message}")),
                    )
                    .await;
                }
                Ok(())
            }
            ClientCommand::ListAppCheckpoints { app_id } => {
                self.handle_list_app_checkpoints(app_id).await;
                Ok(())
            }
            ClientCommand::RestoreAppCheckpoint {
                app_id,
                checkpoint_id,
            } => {
                self.handle_restore_app_checkpoint(app_id, checkpoint_id)
                    .await;
                Ok(())
            }
            ClientCommand::DeleteApp { app_id } => {
                self.handle_delete_app(app_id).await;
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

    /// Test a provider endpoint without exposing a stored credential to the
    /// foreign host or mutating the live engine configuration.
    ///
    /// The request uses each provider's model-list endpoint because it verifies
    /// DNS/TLS, authentication, and the selected model without consuming
    /// inference tokens. An optional draft credential takes precedence over the
    /// secure-store value and is never persisted.
    pub async fn test_provider_connection(
        &self,
        provider_id: String,
        provider_preset: String,
        api_base: String,
        model: String,
        credential_override: Option<ProviderCredentialSecretDto>,
    ) -> ProviderConnectionTestDto {
        if !provider_id_is_valid(&provider_id) {
            return provider_connection_failure("Provider 标识无效", false, false, None, 0, false);
        }

        let draft = credential_override
            .as_ref()
            .map(ProviderCredentialSecretDto::expose_secret)
            .filter(|value| !value.trim().is_empty());
        let used_stored_credential = draft.is_none();
        let credential = if let Some(value) = draft {
            value.to_string()
        } else {
            match self.inner.credentials.get_provider_key(&provider_id).await {
                Ok(Some(secret)) => secret.expose_secret().clone(),
                Ok(None) => {
                    return provider_connection_failure(
                        "请先输入或保存 API Key",
                        false,
                        false,
                        None,
                        0,
                        true,
                    );
                }
                Err(_) => {
                    return provider_connection_failure(
                        "无法读取本机安全存储中的 API Key",
                        false,
                        false,
                        None,
                        0,
                        true,
                    );
                }
            }
        };
        if credential.len() > 16_384 || credential.contains('\0') {
            return provider_connection_failure(
                "API Key 格式无效",
                false,
                false,
                None,
                0,
                used_stored_credential,
            );
        }

        let endpoint = match provider_models_endpoint(&api_base, &provider_preset) {
            Ok(endpoint) => endpoint,
            Err(message) => {
                return provider_connection_failure(
                    message,
                    false,
                    false,
                    None,
                    0,
                    used_stored_credential,
                );
            }
        };
        let request = protocol::HttpRequest {
            method: protocol::HttpMethod::Get,
            url: endpoint,
            headers: provider_connection_headers(&provider_preset, &credential),
            body: None,
            body_bytes: None,
            timeout: Some(PROVIDER_CONNECTION_TIMEOUT),
        };
        let started = std::time::Instant::now();
        let response = self.firer_platform.http().request(request).await;
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        classify_provider_connection_response(
            response,
            model.trim(),
            latency_ms,
            used_stored_credential,
        )
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

    /// Inspect the mobile Linux runtime/rootfs status exposed by the current
    /// mobile platform backend.
    pub async fn mobile_linux_status(&self) -> Result<MobileLinuxStatusDto, MobileEngineError> {
        let Some(runtime) = self.inner.mobile_linux.as_ref() else {
            return Err(MobileEngineError::PlatformUnavailable);
        };
        Self::read_mobile_linux_status(runtime.as_ref()).await
    }

    /// Re-run rootfs verification and return the updated status.
    pub async fn verify_mobile_linux_rootfs(
        &self,
    ) -> Result<MobileLinuxStatusDto, MobileEngineError> {
        let Some(runtime) = self.inner.mobile_linux.as_ref() else {
            return Err(MobileEngineError::PlatformUnavailable);
        };
        let status = runtime.verify_rootfs().await.map_err(|e| {
            MobileEngineError::Internal(format!("verify_mobile_linux_rootfs failed: {e}"))
        })?;
        let capability = runtime.probe_capability().await;
        Ok(lower_mobile_linux_status(capability, status))
    }

    /// Attempt a non-destructive rootfs repair and return the updated status.
    pub async fn repair_mobile_linux_rootfs(
        &self,
    ) -> Result<MobileLinuxStatusDto, MobileEngineError> {
        let Some(runtime) = self.inner.mobile_linux.as_ref() else {
            return Err(MobileEngineError::PlatformUnavailable);
        };
        let status = runtime.repair_rootfs().await.map_err(|e| {
            MobileEngineError::Internal(format!("repair_mobile_linux_rootfs failed: {e}"))
        })?;
        let capability = runtime.probe_capability().await;
        Ok(lower_mobile_linux_status(capability, status))
    }

    /// Reset the managed rootfs state and return the updated status.
    pub async fn reset_mobile_linux_rootfs(
        &self,
    ) -> Result<MobileLinuxStatusDto, MobileEngineError> {
        let Some(runtime) = self.inner.mobile_linux.as_ref() else {
            return Err(MobileEngineError::PlatformUnavailable);
        };
        let status = runtime.reset_rootfs().await.map_err(|e| {
            MobileEngineError::Internal(format!("reset_mobile_linux_rootfs failed: {e}"))
        })?;
        let capability = runtime.probe_capability().await;
        Ok(lower_mobile_linux_status(capability, status))
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

    async fn emit_provider_credential_status(
        &self,
        operation_id: u64,
        provider_ids: &[String],
        operation_error: Option<String>,
    ) {
        if let Some(error) = operation_error {
            self.event_sink
                .emit(ClientEvent::ProviderCredentialStatus {
                    operation_id,
                    configured_provider_ids: Vec::new(),
                    unavailable_provider_ids: provider_ids.to_vec(),
                    storage_encrypted: self.inner.credentials.provider_key_storage_is_encrypted(),
                    error: Some(error),
                })
                .await;
            return;
        }

        let mut configured_provider_ids = Vec::new();
        let mut unavailable_provider_ids = Vec::new();
        let mut failures = Vec::new();
        for provider_id in provider_ids {
            match self.inner.credentials.get_provider_key(provider_id).await {
                Ok(Some(_)) => configured_provider_ids.push(provider_id.clone()),
                Ok(None) => {}
                Err(failure) => {
                    unavailable_provider_ids.push(provider_id.clone());
                    failures.push(format!("{provider_id}: {failure}"));
                }
            }
        }
        let error = (!failures.is_empty()).then(|| {
            format!(
                "provider credential storage is unavailable ({})",
                failures.join("; ")
            )
        });
        self.event_sink
            .emit(ClientEvent::ProviderCredentialStatus {
                operation_id,
                configured_provider_ids,
                unavailable_provider_ids,
                storage_encrypted: self.inner.credentials.provider_key_storage_is_encrypted(),
                error,
            })
            .await;
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
                let snapshot = handle.get_status_snapshot().await;
                let models = traits::curated_model_refs(
                    &listings,
                    &available,
                    &snapshot.model,
                    snapshot.model_profile.as_deref(),
                );
                let current =
                    traits::qualified_model_ref(&snapshot.model, snapshot.model_profile.as_deref());
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

fn provider_id_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || (index > 0 && matches!(ch, '-' | '_' | '.'))
        })
}

const PROVIDER_CONNECTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

fn provider_models_endpoint(api_base: &str, provider_preset: &str) -> Result<String, &'static str> {
    let base = api_base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("请填写 API 地址");
    }
    if !(base.starts_with("https://") || base.starts_with("http://")) {
        return Err("API 地址必须以 https:// 或 http:// 开头");
    }
    if base
        .chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, '#' | '?'))
        || base.split_once("://").is_some_and(|(_, authority)| {
            authority
                .split('/')
                .next()
                .is_some_and(|host| host.contains('@'))
        })
    {
        return Err("API 地址格式无效");
    }

    if base.ends_with("/models") {
        return Ok(base.to_string());
    }
    if let Some(prefix) = base.strip_suffix("/chat/completions") {
        return Ok(format!("{prefix}/models"));
    }
    if provider_preset == "anthropic" && !base.ends_with("/v1") {
        return Ok(format!("{base}/v1/models"));
    }
    Ok(format!("{base}/models"))
}

fn provider_connection_headers(provider_preset: &str, credential: &str) -> Vec<(String, String)> {
    let mut headers = vec![("accept".to_string(), "application/json".to_string())];
    match provider_preset {
        "anthropic" => {
            headers.push(("x-api-key".to_string(), credential.to_string()));
            headers.push(("anthropic-version".to_string(), "2023-06-01".to_string()));
        }
        "google" => {
            headers.push(("x-goog-api-key".to_string(), credential.to_string()));
        }
        _ => {
            headers.push(("authorization".to_string(), format!("Bearer {credential}")));
        }
    }
    headers
}

fn provider_connection_failure(
    message: impl Into<String>,
    reachable: bool,
    authenticated: bool,
    http_status: Option<u16>,
    latency_ms: u64,
    used_stored_credential: bool,
) -> ProviderConnectionTestDto {
    ProviderConnectionTestDto {
        connected: false,
        reachable,
        authenticated,
        model_available: false,
        http_status,
        latency_ms,
        message: message.into(),
        used_stored_credential,
    }
}

fn provider_model_ids(body: &str) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let entries = value
        .get("data")
        .or_else(|| value.get("models"))?
        .as_array()?;
    Some(
        entries
            .iter()
            .filter_map(|entry| {
                entry
                    .get("id")
                    .or_else(|| entry.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(|id| id.strip_prefix("models/").unwrap_or(id).to_string())
            })
            .collect(),
    )
}

fn classify_provider_connection_response(
    response: Result<protocol::HttpResponse, HttpError>,
    model: &str,
    latency_ms: u64,
    used_stored_credential: bool,
) -> ProviderConnectionTestDto {
    match response {
        Ok(response) if (200..300).contains(&response.status) => {
            let model_ids = provider_model_ids(&response.body);
            let model_available = model.is_empty()
                || model_ids
                    .as_ref()
                    .is_some_and(|ids| ids.iter().any(|id| id == model));
            match model_ids {
                Some(_) if !model_available => provider_connection_failure(
                    format!("连接与认证成功，但模型 `{model}` 不在可用列表中"),
                    true,
                    true,
                    Some(response.status),
                    latency_ms,
                    used_stored_credential,
                ),
                Some(_) => ProviderConnectionTestDto {
                    connected: true,
                    reachable: true,
                    authenticated: true,
                    model_available: true,
                    http_status: Some(response.status),
                    latency_ms,
                    message: format!("连接成功 · {latency_ms} ms"),
                    used_stored_credential,
                },
                None => ProviderConnectionTestDto {
                    connected: true,
                    reachable: true,
                    authenticated: true,
                    model_available: false,
                    http_status: Some(response.status),
                    latency_ms,
                    message: format!("连接与认证成功 · {latency_ms} ms（未能校验模型列表）"),
                    used_stored_credential,
                },
            }
        }
        Ok(response) => {
            classify_provider_connection_status(response.status, latency_ms, used_stored_credential)
        }
        Err(HttpError::Status { status, .. }) => {
            classify_provider_connection_status(status, latency_ms, used_stored_credential)
        }
        Err(HttpError::Timeout(_)) => provider_connection_failure(
            "连接超时，请检查网络或 API 地址",
            false,
            false,
            None,
            latency_ms,
            used_stored_credential,
        ),
        Err(HttpError::Connection(_)) => provider_connection_failure(
            "无法连接服务，请检查网络、DNS、TLS 或 API 地址",
            false,
            false,
            None,
            latency_ms,
            used_stored_credential,
        ),
        Err(HttpError::InvalidRequest(_)) => provider_connection_failure(
            "API 地址或请求配置无效",
            false,
            false,
            None,
            latency_ms,
            used_stored_credential,
        ),
        Err(HttpError::InvalidResponse(_)) => provider_connection_failure(
            "服务响应格式无效",
            true,
            false,
            None,
            latency_ms,
            used_stored_credential,
        ),
        Err(HttpError::Cancelled) => provider_connection_failure(
            "连接测试已取消",
            false,
            false,
            None,
            latency_ms,
            used_stored_credential,
        ),
    }
}

fn classify_provider_connection_status(
    status: u16,
    latency_ms: u64,
    used_stored_credential: bool,
) -> ProviderConnectionTestDto {
    let (message, authenticated) = match status {
        400 | 422 => ("服务可达，但请求格式不受支持", false),
        401 => ("认证失败，请检查 API Key", false),
        402 => ("认证成功，但账户余额不足", true),
        403 => ("服务拒绝访问，请检查 Key 权限", false),
        404 => ("服务可达，但模型列表端点不存在；请检查 API 地址", false),
        429 => ("服务可达，但请求频率已达上限，请稍后重试", false),
        500..=599 => ("Provider 服务暂时不可用，请稍后重试", false),
        _ => ("Provider 返回了无法识别的响应", false),
    };
    provider_connection_failure(
        message,
        true,
        authenticated,
        Some(status),
        latency_ms,
        used_stored_credential,
    )
}

// ───────────────────────────────────────────────────────────────────────────
// Cron firing — the Android background-scheduler bridge.
//
// The desktop `cron::CronScheduler` 60s tick loop is unavailable on mobile (no
// long-lived daemon, and the mobile engine binds no `TaskRegistry` / subagent
// spawner). Android WorkManager calls the single-occurrence methods below after
// AlarmManager or the 15-minute watchdog wakes it. Due-detection and
// bookkeeping remain in the shared cron store; firing is a fresh, throwaway
// orchestrator turn. Durable allow rules still apply, while any permission that
// would require foreground interaction is denied immediately.
// ───────────────────────────────────────────────────────────────────────────

/// Per-job wall-clock budget for a fired cron turn. This remains a second
/// safety limit beneath WorkManager's outer lifecycle budget.
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
    /// Whether the failure is safe to retry automatically (HTTP 429/5xx and
    /// transport failures). Successful runs always report `false`.
    pub retryable: bool,
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
    /// Whether Android may schedule this task. Recurring schedules must have a
    /// minimum interval of 15 minutes; one-shot schedules are exempt.
    pub mobile_supported: bool,
    /// Stable explanation when [`Self::mobile_supported`] is false.
    pub unsupported_reason: Option<String>,
}

/// One due task occurrence lowered for WorkManager dispatch.
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronDueOccurrenceDto {
    /// Stable persisted task id.
    pub task_id: String,
    /// The task's computed (possibly missed) fire instant in epoch milliseconds.
    pub scheduled_at_ms: u64,
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

/// A headless cron turn has no foreground answerer. Requests are forwarded to a
/// local channel and resolved `Deny` immediately, so a background job never
/// parks for the normal five-minute interactive timeout. Existing durable
/// allow-rules still short-circuit before a request is emitted.
struct ImmediateDenyPermissionSink {
    sender: mpsc::UnboundedSender<PermissionRequestDto>,
}

#[async_trait]
impl PermissionRequestSink for ImmediateDenyPermissionSink {
    async fn emit_request(&self, request: PermissionRequestDto) {
        let _ = self.sender.send(request);
    }
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
        let (permission_tx, mut permission_rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn PermissionRequestSink> = Arc::new(ImmediateDenyPermissionSink {
            sender: permission_tx,
        });
        let rt = build_mobile_inner(
            self.cfg.clone(),
            self.platform.clone(),
            listener,
            sink,
            None,
        )
        .await
        .map_err(|e| e.to_string())?;

        let gate = rt.permission_gate.clone();
        let deny_requests = tokio::spawn(async move {
            while let Some(request) = permission_rx.recv().await {
                let tool_name = match &request.kind {
                    PermissionKindDto::ToolUseConfirm { tool_name, .. } => tool_name.as_str(),
                    _ => "",
                };
                let _ = gate
                    .resolve(request.request_id, PermissionResponseDto::Deny, tool_name)
                    .await;
            }
        });
        let run = rt.orchestrator.run_turn_streaming(prompt);
        let result = match tokio::time::timeout(CRON_TURN_TIMEOUT, run).await {
            Ok(Ok(_outcome)) => Ok(captured.lock().await.clone()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("cron turn timed out".to_string()),
        };
        deny_requests.abort();
        // `rt` drops here → the throwaway session + its permission gate tear down.
        result
    }
}

/// Compute a task's next fire (epoch ms) from its cron string + anchor
/// (`lastFiredAt ?? createdAt ?? now`). `None` for an unparseable / impossible
/// expression.
fn task_next_fire_ms(
    id: &str,
    cron: &str,
    created_at_ms: u64,
    last_fired_at_ms: Option<u64>,
    recurring: bool,
    now: std::time::SystemTime,
) -> Option<u64> {
    // Delegate to the SAME jittered scheduler computation the Android alarm arms
    // from (`next_cron_fire_time` → `cron::next_fire_epoch_ms`), so the per-task
    // next fire the management UI shows is the instant the job will ACTUALLY
    // fire. A raw `next_match_after` here omitted Claude Code's recurring jitter,
    // making the displayed time disagree with the armed alarm by up to 30 min.
    cron::next_fire_epoch_ms_for_task(id, cron, created_at_ms, last_fired_at_ms, recurring, now)
}

const MOBILE_MIN_RECURRING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

fn cron_field_values(field: &cron::CronField, min: u32, max: u32) -> Option<Vec<u32>> {
    let values = match field {
        cron::CronField::Any => (min..=max).collect(),
        cron::CronField::Exact(value) => vec![*value],
        cron::CronField::Step(step) if *step > 0 => {
            (min..=max).filter(|value| value % step == 0).collect()
        }
        cron::CronField::Step(_) => return None,
        cron::CronField::Range(start, end) if start <= end => (*start..=*end).collect(),
        cron::CronField::Range(_, _) => return None,
        cron::CronField::List(values) if !values.is_empty() => values.clone(),
        cron::CronField::List(_) => return None,
    };
    if values.iter().all(|value| (min..=max).contains(value)) {
        Some(values)
    } else {
        None
    }
}

/// Android's recurring-work contract: a valid five-field cron expression whose
/// closest two wall-clock occurrences are at least fifteen minutes apart.
/// One-shot tasks still validate field ranges but are not interval-limited.
fn mobile_cron_schedule_error(cron_expr: &str, recurring: bool) -> Option<String> {
    let expression = match cron::parse_cron(cron_expr) {
        Ok(expression) => expression,
        Err(error) => return Some(format!("invalid cron expression: {error}")),
    };
    let invalid_field = || Some("cron expression contains an out-of-range field".to_string());
    let Some(minutes) = cron_field_values(&expression.minute, 0, 59) else {
        return invalid_field();
    };
    let Some(hours) = cron_field_values(&expression.hour, 0, 23) else {
        return invalid_field();
    };
    if cron_field_values(&expression.dom, 1, 31).is_none()
        || cron_field_values(&expression.month, 1, 12).is_none()
        || cron_field_values(&expression.dow, 0, 6).is_none()
    {
        return invalid_field();
    }
    if !recurring {
        return None;
    }

    let mut minute_of_day = Vec::with_capacity(minutes.len() * hours.len());
    for hour in hours {
        for minute in &minutes {
            minute_of_day.push(hour * 60 + minute);
        }
    }
    minute_of_day.sort_unstable();
    minute_of_day.dedup();
    if minute_of_day.is_empty() {
        return Some("cron expression has no valid fire time".to_string());
    }
    if minute_of_day.len() > 1 {
        let min_gap = minute_of_day
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .chain(std::iter::once(
                24 * 60 - minute_of_day[minute_of_day.len() - 1] + minute_of_day[0],
            ))
            .min()
            .unwrap_or(24 * 60);
        if std::time::Duration::from_secs(u64::from(min_gap) * 60) < MOBILE_MIN_RECURRING_INTERVAL {
            return Some("Android recurring tasks must be at least 15 minutes apart".to_string());
        }
    }
    None
}

fn cron_task_dto(task: cron::CronTask, now: std::time::SystemTime) -> CronTaskDto {
    let recurring = task.recurring.unwrap_or(false);
    let unsupported_reason = mobile_cron_schedule_error(&task.cron, recurring);
    CronTaskDto {
        human: tool_cron::schedule_cron::cron_to_human(&task.cron),
        next_fire_ms: task_next_fire_ms(
            &task.id,
            &task.cron,
            task.created_at,
            task.last_fired_at,
            recurring,
            now,
        ),
        id: task.id,
        cron: task.cron,
        prompt: task.prompt,
        created_at_ms: task.created_at,
        last_fired_at_ms: task.last_fired_at,
        recurring,
        mobile_supported: unsupported_reason.is_none(),
        unsupported_reason,
    }
}

fn cron_failure_is_retryable(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "429",
        "rate limit",
        "transport error",
        "connection failed",
        "temporarily unavailable",
        "timed out",
        "timeout",
        "dns",
        "http 5",
        "status 5",
        "server error",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn fired_cron_dto(task: &cron::CronTask, result: Result<String, String>) -> FiredCronJobDto {
    match result {
        Ok(text) => FiredCronJobDto {
            id: task.id.clone(),
            prompt: task.prompt.clone(),
            result_text: Some(text),
            status: CronFireStatusDto::Ok,
            retryable: false,
        },
        Err(message) => FiredCronJobDto {
            id: task.id.clone(),
            prompt: task.prompt.clone(),
            result_text: None,
            retryable: cron_failure_is_retryable(&message),
            status: CronFireStatusDto::Failed { message },
        },
    }
}

async fn read_cron_tasks(fs: &dyn FileSystem, cwd: &std::path::Path) -> cron::ScheduledTasks {
    cron::read_tasks_body(fs, cwd)
        .await
        .map(|body| cron::parse_tasks(&body))
        .unwrap_or_default()
}

/// Lightweight, credential-free scheduled-task store used by Android UI and
/// reconciliation workers. It owns only the validated workspace root plus the
/// platform filesystem/clock; constructing it never builds an LLM client.
#[cfg_attr(feature = "uniffi", derive(uniffi::Object))]
pub struct MobileCronStoreHandle {
    cwd: std::path::PathBuf,
    fs: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
}

impl MobileCronStoreHandle {
    #[must_use]
    pub fn new(cwd: std::path::PathBuf, fs: Arc<dyn FileSystem>, clock: Arc<dyn Clock>) -> Self {
        Self { cwd, fs, clock }
    }
}

#[cfg_attr(feature = "uniffi", uniffi::export(async_runtime = "tokio"))]
impl MobileCronStoreHandle {
    pub async fn list(&self) -> Vec<CronTaskDto> {
        let now = self.clock.now();
        read_cron_tasks(self.fs.as_ref(), &self.cwd)
            .await
            .tasks
            .into_iter()
            .map(|task| cron_task_dto(task, now))
            .collect()
    }

    pub async fn create(
        &self,
        cron_expr: String,
        prompt: String,
        recurring: bool,
    ) -> Result<CronTaskDto, MobileEngineError> {
        if let Some(error) = mobile_cron_schedule_error(&cron_expr, recurring) {
            return Err(MobileEngineError::Internal(error));
        }
        let _process_guard = cron::lock_cron_file().await;
        let _file_guard = cron::lock_scheduled_tasks(self.fs.as_ref(), &self.cwd)
            .await
            .map_err(|error| {
                MobileEngineError::Internal(format!("lock scheduled_tasks.json: {error}"))
            })?;
        let mut document = read_cron_tasks(self.fs.as_ref(), &self.cwd).await;
        let now = self.clock.now();
        let now_ms = now
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let task = cron::CronTask {
            id: tool_cron::schedule_cron::generate_cron_task_id(),
            cron: cron_expr,
            prompt,
            created_at: now_ms,
            last_fired_at: None,
            recurring: Some(recurring),
            permanent: None,
        };
        document.tasks.push(task.clone());
        cron::write_tasks_body(
            self.fs.as_ref(),
            &self.cwd,
            &cron::serialize_tasks(&document),
        )
        .await
        .map_err(|error| {
            MobileEngineError::Internal(format!("write scheduled_tasks.json: {error}"))
        })?;
        Ok(cron_task_dto(task, now))
    }

    pub async fn update(
        &self,
        id: String,
        cron_expr: String,
        prompt: String,
        recurring: bool,
    ) -> Result<CronTaskDto, MobileEngineError> {
        if let Some(error) = mobile_cron_schedule_error(&cron_expr, recurring) {
            return Err(MobileEngineError::Internal(error));
        }
        let _process_guard = cron::lock_cron_file().await;
        let _file_guard = cron::lock_scheduled_tasks(self.fs.as_ref(), &self.cwd)
            .await
            .map_err(|error| {
                MobileEngineError::Internal(format!("lock scheduled_tasks.json: {error}"))
            })?;
        let mut document = read_cron_tasks(self.fs.as_ref(), &self.cwd).await;
        let now = self.clock.now();
        let now_ms = now
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let task = document
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or(MobileEngineError::NotFound)?;
        task.cron = cron_expr;
        task.prompt = prompt;
        task.recurring = Some(recurring);
        task.created_at = now_ms;
        task.last_fired_at = None;
        let updated = task.clone();
        cron::write_tasks_body(
            self.fs.as_ref(),
            &self.cwd,
            &cron::serialize_tasks(&document),
        )
        .await
        .map_err(|error| {
            MobileEngineError::Internal(format!("write scheduled_tasks.json: {error}"))
        })?;
        Ok(cron_task_dto(updated, now))
    }

    pub async fn delete(&self, id: String) -> bool {
        let _process_guard = cron::lock_cron_file().await;
        let Ok(_file_guard) = cron::lock_scheduled_tasks(self.fs.as_ref(), &self.cwd).await else {
            return false;
        };
        let mut document = read_cron_tasks(self.fs.as_ref(), &self.cwd).await;
        let previous_len = document.tasks.len();
        document.tasks.retain(|task| task.id != id);
        previous_len != document.tasks.len()
            && cron::write_tasks_body(
                self.fs.as_ref(),
                &self.cwd,
                &cron::serialize_tasks(&document),
            )
            .await
            .is_ok()
    }

    pub async fn next_fire_time(&self) -> Option<u64> {
        self.list()
            .await
            .into_iter()
            .filter(|task| task.mobile_supported)
            .filter_map(|task| task.next_fire_ms)
            .min()
    }

    pub async fn due_occurrences(&self, now_ms: u64) -> Vec<CronDueOccurrenceDto> {
        self.list()
            .await
            .into_iter()
            .filter(|task| task.mobile_supported)
            .filter_map(|task| {
                task.next_fire_ms
                    .filter(|scheduled_at_ms| *scheduled_at_ms <= now_ms)
                    .map(|scheduled_at_ms| CronDueOccurrenceDto {
                        task_id: task.id,
                        scheduled_at_ms,
                    })
            })
            .collect()
    }

    /// Mark a due occurrence complete after the host exhausts retries. This is
    /// intentionally available on the lightweight store so iOS background
    /// reconciliation never needs to construct an LLM engine just to advance
    /// durable schedule bookkeeping.
    pub async fn acknowledge_occurrence(&self, task_id: String, scheduled_at_ms: u64) -> bool {
        let _process_guard = cron::lock_cron_file().await;
        let Ok(_file_guard) = cron::lock_scheduled_tasks(self.fs.as_ref(), &self.cwd).await else {
            return false;
        };
        let mut document = read_cron_tasks(self.fs.as_ref(), &self.cwd).await;
        let now = self.clock.now();
        let now_ms = now
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let Some(task) = document.tasks.iter().find(|task| task.id == task_id) else {
            return false;
        };
        let expected = task_next_fire_ms(
            &task.id,
            &task.cron,
            task.created_at,
            task.last_fired_at,
            task.recurring.unwrap_or(false),
            now,
        );
        if expected != Some(scheduled_at_ms) || scheduled_at_ms > now_ms {
            return false;
        }
        finalize_cron_occurrence(&mut document, &task_id, now_ms);
        cron::write_tasks_body(
            self.fs.as_ref(),
            &self.cwd,
            &cron::serialize_tasks(&document),
        )
        .await
        .is_ok()
    }

    pub fn validate_schedule(&self, cron_expr: String, recurring: bool) -> Option<String> {
        mobile_cron_schedule_error(&cron_expr, recurring)
    }
}

fn finalize_cron_occurrence(
    document: &mut cron::ScheduledTasks,
    task_id: &str,
    completed_at_ms: u64,
) {
    let remove = document
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .is_some_and(|task| !task.recurring.unwrap_or(false));
    if remove {
        document.tasks.retain(|task| task.id != task_id);
    } else if let Some(task) = document.tasks.iter_mut().find(|task| task.id == task_id) {
        task.last_fired_at = Some(completed_at_ms);
    }
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
            retryable: matches!(
                &f.status,
                cron::FireStatus::Failed(message) if cron_failure_is_retryable(message)
            ),
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
        MobileCronStoreHandle::new(
            self.firer_cfg.cwd.clone(),
            self.firer_platform.filesystem(),
            self.firer_platform.clock(),
        )
        .next_fire_time()
        .await
    }

    /// List the persisted cron jobs for the management UI (each with its computed
    /// next fire + human schedule). A missing / unparseable file lists nothing.
    pub async fn cron_list(&self) -> Vec<CronTaskDto> {
        MobileCronStoreHandle::new(
            self.firer_cfg.cwd.clone(),
            self.firer_platform.filesystem(),
            self.firer_platform.clock(),
        )
        .list()
        .await
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
        MobileCronStoreHandle::new(
            self.firer_cfg.cwd.clone(),
            self.firer_platform.filesystem(),
            self.firer_platform.clock(),
        )
        .create(cron_expr, prompt, recurring)
        .await
    }

    /// Edit a task in place while preserving its stable id.
    pub async fn cron_update(
        &self,
        id: String,
        cron_expr: String,
        prompt: String,
        recurring: bool,
    ) -> Result<CronTaskDto, MobileEngineError> {
        MobileCronStoreHandle::new(
            self.firer_cfg.cwd.clone(),
            self.firer_platform.filesystem(),
            self.firer_platform.clock(),
        )
        .update(id, cron_expr, prompt, recurring)
        .await
    }

    /// Delete a cron job by id. Returns `true` iff a job was removed.
    pub async fn cron_delete(&self, id: String) -> bool {
        MobileCronStoreHandle::new(
            self.firer_cfg.cwd.clone(),
            self.firer_platform.filesystem(),
            self.firer_platform.clock(),
        )
        .delete(id)
        .await
    }

    /// Run one scheduled occurrence exactly once across duplicate alarm/worker
    /// deliveries. A retryable transport failure intentionally leaves the
    /// occurrence unacknowledged so WorkManager can retry it.
    pub async fn run_cron_task_if_due(
        &self,
        task_id: String,
        scheduled_at_ms: u64,
    ) -> Option<FiredCronJobDto> {
        let fs = self.firer_platform.filesystem();
        let _process_guard = cron::lock_cron_file().await;
        let _file_guard = cron::lock_scheduled_tasks(fs.as_ref(), &self.firer_cfg.cwd)
            .await
            .ok()?;
        let mut document = read_cron_tasks(fs.as_ref(), &self.firer_cfg.cwd).await;
        let now = self.firer_platform.clock().now();
        let now_ms = now
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let task = document
            .tasks
            .iter()
            .find(|task| task.id == task_id)?
            .clone();
        let recurring = task.recurring.unwrap_or(false);
        if mobile_cron_schedule_error(&task.cron, recurring).is_some() {
            return None;
        }
        let expected = task_next_fire_ms(
            &task.id,
            &task.cron,
            task.created_at,
            task.last_fired_at,
            recurring,
            now,
        )?;
        if expected != scheduled_at_ms || scheduled_at_ms > now_ms {
            return None;
        }

        let firer = MobileTurnFirer {
            cfg: self.firer_cfg.clone(),
            platform: self.firer_platform.clone(),
        };
        let fired = fired_cron_dto(&task, firer.fire(&task.id, &task.prompt).await);
        if !fired.retryable {
            finalize_cron_occurrence(&mut document, &task.id, now_ms);
            let _ = cron::write_tasks_body(
                fs.as_ref(),
                &self.firer_cfg.cwd,
                &cron::serialize_tasks(&document),
            )
            .await;
        }
        Some(fired)
    }

    /// Mark a retry-exhausted occurrence complete without running it again.
    pub async fn acknowledge_cron_occurrence(&self, task_id: String, scheduled_at_ms: u64) -> bool {
        let fs = self.firer_platform.filesystem();
        let _process_guard = cron::lock_cron_file().await;
        let Ok(_file_guard) = cron::lock_scheduled_tasks(fs.as_ref(), &self.firer_cfg.cwd).await
        else {
            return false;
        };
        let mut document = read_cron_tasks(fs.as_ref(), &self.firer_cfg.cwd).await;
        let now = self.firer_platform.clock().now();
        let now_ms = now
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let Some(task) = document.tasks.iter().find(|task| task.id == task_id) else {
            return false;
        };
        let expected = task_next_fire_ms(
            &task.id,
            &task.cron,
            task.created_at,
            task.last_fired_at,
            task.recurring.unwrap_or(false),
            now,
        );
        if expected != Some(scheduled_at_ms) || scheduled_at_ms > now_ms {
            return false;
        }
        finalize_cron_occurrence(&mut document, &task_id, now_ms);
        cron::write_tasks_body(
            fs.as_ref(),
            &self.firer_cfg.cwd,
            &cron::serialize_tasks(&document),
        )
        .await
        .is_ok()
    }

    /// Execute a task immediately without moving its recurring/one-shot anchor.
    pub async fn run_cron_task_now(&self, task_id: String) -> Option<FiredCronJobDto> {
        let fs = self.firer_platform.filesystem();
        let _process_guard = cron::lock_cron_file().await;
        let _file_guard = cron::lock_scheduled_tasks(fs.as_ref(), &self.firer_cfg.cwd)
            .await
            .ok()?;
        let task = read_cron_tasks(fs.as_ref(), &self.firer_cfg.cwd)
            .await
            .tasks
            .into_iter()
            .find(|task| task.id == task_id)?;
        let firer = MobileTurnFirer {
            cfg: self.firer_cfg.clone(),
            platform: self.firer_platform.clone(),
        };
        Some(fired_cron_dto(
            &task,
            firer.fire(&task.id, &task.prompt).await,
        ))
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
/// LOCAL-APPS (phase 1): the per-profile data root the apps store lives under
/// (`<root>/apps/index.json`, `<root>/apps/<id>/…`).
///
/// The engine's per-profile data dir is the app-files root: every production
/// path sets `lingxi_home = <app_files_root>/<DOT_DIR>` (android-aar
/// `build_android_engine*`; the host `test_config` mirrors it under a temp
/// root), so its parent IS the profile root — deliberately independent of
/// `cwd`, which may point at a per-project workspace while apps are a
/// profile-global capability. A degenerate `lingxi_home` (empty / no parent,
/// only reachable through a hand-rolled `MobileConfig`) falls back to `cwd`,
/// which equals the app-files root whenever no project workspace is selected.
fn mobile_apps_data_root(cfg: &MobileConfig) -> std::path::PathBuf {
    cfg.lingxi_home
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| cfg.cwd.clone(), std::path::Path::to_path_buf)
}

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
    // Construct turn ownership before the runtime so every adapter-originated
    // event is filtered/reclassified by the same connection-scoped lifecycle
    // listener used by the eventual handle.
    let active_cancel: Arc<Mutex<Option<Arc<ActiveTurn>>>> = Arc::new(Mutex::new(None));
    let lifecycle_listener: Arc<dyn ClientEventListener> =
        Arc::new(TurnLifecycleListener::new(listener, active_cancel.clone()));

    // `build_mobile` is async; drive it on the owned runtime so any spawned work
    // it does is owned by this handle's runtime, not an ambient one.
    let (ask_user_question_tx, ask_user_question_rx) =
        tokio::sync::mpsc::channel::<tool_ui::AskUserQuestionExchange>(8);
    let inner = runtime
        .block_on(build_mobile_inner_with_ask(
            cfg,
            platform,
            lifecycle_listener,
            recording_sink,
            streaming_override,
            Some(ask_user_question_tx),
        ))
        .map_err(|e| MobileEngineError::Internal(e.to_string()))?;

    let skill_count = crate::mobile_skill_registry().len();
    let event_sink = inner.event_sink.clone();
    let ask_user_question_broker = Arc::new(client_adapter::BridgeAskUserQuestionBroker::new(
        event_sink.clone(),
    ));
    {
        let broker = ask_user_question_broker.clone();
        runtime.spawn(async move { broker.run(ask_user_question_rx).await });
    }

    // LOCAL-APPS: one process-wide service per profile root. Conversation or
    // provider source changes only add/remove event subscribers; they do not
    // open a second SQLite/Git/generation owner for the same application data.
    let app_emissions =
        crate::local_apps_bridge::AppEmissionQueue::spawn(runtime.handle(), event_sink.clone());
    let loaded_profile = runtime.block_on(profile_apps(
        mobile_apps_data_root(&firer_cfg),
        firer_platform.clock(),
        inner.mobile_linux.clone(),
        firer_cfg.local_apps_full_runtime,
        firer_cfg.local_apps_runtime_root.clone(),
        inner.local_apps_llm.clone(),
    ));
    let (
        local_apps,
        local_apps_host,
        app_generation,
        retained_profile,
        app_client_subscription,
        app_domain_subscription,
        app_domain_observer,
    ) = match loaded_profile {
        Ok(profile) => {
            let client_subscription = profile.client_events.subscribe(event_sink.clone());
            let observer = Arc::new(crate::local_apps_bridge::SinkAppEventObserver::new(
                app_emissions.clone(),
            ));
            let domain_subscription = profile.domain_events.subscribe(observer.clone());
            // Only now do the observers exist. AppService::load already
            // announced pending gates, but into a fanout nobody had subscribed
            // to yet, so those events were dropped — leaving a relaunched
            // client with no `interaction_id` for an armed designer gate.
            runtime.block_on(profile.service.resync_pending_gates());
            (
                Ok(profile.service.clone()),
                profile.host.clone(),
                profile.generation.clone(),
                Some(profile),
                Some(client_subscription),
                Some(domain_subscription),
                Some(observer),
            )
        }
        Err(error) => {
            // Preserve the established failure contract: a corrupt store does
            // not brick the conversation engine; every app command returns the
            // typed load error. These unattached fallbacks can only report that
            // same unavailable state and never mutate data.
            let host = LocalAppsHostBroker::new(
                mobile_apps_data_root(&firer_cfg),
                event_sink.clone(),
                inner.mobile_linux.clone(),
                firer_cfg.local_apps_full_runtime,
                firer_cfg.local_apps_runtime_root.clone(),
            );
            let executor = MobileAppGenerationExecutor::new(
                inner.mobile_linux.clone(),
                host.clone(),
                Arc::new(SharedLlm::new(inner.local_apps_llm.clone())),
            );
            let generation = AppGenerationCoordinator::new_with_observer(
                mobile_apps_data_root(&firer_cfg),
                firer_platform.clock(),
                executor,
                ClientGenerationJobObserver::new(
                    mobile_apps_data_root(&firer_cfg),
                    event_sink.clone(),
                ),
            );
            let _ = host.attach_generation(generation.clone());
            (Err(error), host, generation, None, None, None, None)
        }
    };
    if inner
        .local_apps_mcp
        .attach_host(local_apps_host.clone())
        .is_err()
    {
        tracing::warn!("local-apps MCP host was already attached");
    }
    match &local_apps {
        Ok(service) => {
            if inner
                .local_apps_mcp
                .attach_service(service.clone())
                .is_err()
            {
                tracing::warn!("local-apps MCP service was already attached");
            }
            // Startup redelivery is idempotent; the generation coordinator
            // deduplicates the durable continuation tuple.
            let service = service.clone();
            runtime.spawn(async move {
                if let Err(error) = service.redeliver_all_undelivered().await {
                    tracing::warn!(
                        error = %error,
                        "local-apps startup continuation sweep failed; entries stay queued"
                    );
                }
            });
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "local-apps store failed to load; app commands will report the failure"
            );
        }
    }

    Ok(Arc::new(MobileEngineHandle {
        runtime,
        inner,
        event_sink,
        active_cancel,
        tool_names,
        ask_user_question_broker,
        skill_count,
        lingxi_home,
        session_cwd,
        fs,
        firer_cfg,
        firer_platform,
        local_apps,
        app_emissions,
        local_apps_host,
        app_generation,
        profile_apps: retained_profile,
        app_client_subscription,
        app_domain_subscription,
        app_domain_observer,
        local_apps_background: LocalAppsBackgroundTracker::default(),
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex as StdMutex};

    use async_trait::async_trait;
    use client_adapter::{ClientEventListener, PermissionRequestSink};

    use super::{
        build_mobile, classify_provider_connection_response, mobile_cron_schedule_error,
        provider_models_endpoint, MobileConfig, MobileCronStoreHandle,
    };
    // F3-06: the off-device host shim now lives in `crate::test_support` (the
    // single, non-drifting definition shared with the `skeleton_test.rs`
    // integration test). The in-crate F3-03/F3-05 unit tests reuse it. The
    // collecting permission sink is aliased to the legacy name these test bodies
    // already use.
    use crate::test_support::{
        test_config, CollectingPermissionSink as RecordingPermissionSink, FakeListener,
        HostFakePlatform,
    };

    #[test]
    fn android_recurring_schedule_enforces_fifteen_minute_floor() {
        assert!(mobile_cron_schedule_error("*/5 * * * *", true).is_some());
        assert!(mobile_cron_schedule_error("0,10 * * * *", true).is_some());
        assert!(mobile_cron_schedule_error("*/15 * * * *", true).is_none());
        assert!(mobile_cron_schedule_error("0 * * * *", true).is_none());
        assert!(mobile_cron_schedule_error("* * * * *", false).is_none());
        assert!(mobile_cron_schedule_error("61 * * * *", false).is_some());
    }

    #[test]
    fn provider_connection_uses_provider_specific_model_endpoint() {
        assert_eq!(
            Ok("https://api.deepseek.com/models".to_string()),
            provider_models_endpoint("https://api.deepseek.com/", "deepseek"),
        );
        assert_eq!(
            Ok("https://api.openai.com/v1/models".to_string()),
            provider_models_endpoint("https://api.openai.com/v1", "openai"),
        );
        assert_eq!(
            Ok("https://api.anthropic.com/v1/models".to_string()),
            provider_models_endpoint("https://api.anthropic.com", "anthropic"),
        );
        assert!(provider_models_endpoint("file:///tmp/provider", "custom").is_err());
        assert!(provider_models_endpoint("https://key@example.com/v1", "custom").is_err());
    }

    #[test]
    fn provider_connection_requires_selected_model_in_recognized_catalog() {
        let connected = classify_provider_connection_response(
            Ok(protocol::HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: r#"{"object":"list","data":[{"id":"deepseek-v4-flash"}]}"#.to_string(),
                body_bytes: Vec::new(),
            }),
            "deepseek-v4-flash",
            42,
            true,
        );
        assert!(connected.connected);
        assert!(connected.authenticated);
        assert!(connected.model_available);
        assert_eq!(Some(200), connected.http_status);
        assert!(connected.used_stored_credential);

        let missing = classify_provider_connection_response(
            Ok(protocol::HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: r#"{"models":[{"name":"models/gemini-2.5-flash"}]}"#.to_string(),
                body_bytes: Vec::new(),
            }),
            "gemini-2.5-pro",
            9,
            false,
        );
        assert!(!missing.connected);
        assert!(missing.authenticated);
        assert!(!missing.model_available);
        assert!(!missing.used_stored_credential);
    }

    #[test]
    fn provider_connection_maps_auth_failure_without_echoing_response_body() {
        let result = classify_provider_connection_response(
            Err(traits::HttpError::Status {
                status: 401,
                body: "secret-bearing upstream response".to_string(),
            }),
            "deepseek-v4-flash",
            18,
            true,
        );

        assert!(!result.connected);
        assert!(result.reachable);
        assert!(!result.authenticated);
        assert_eq!(Some(401), result.http_status);
        assert!(!result.message.contains("upstream"));
        assert!(!result.message.contains("secret"));
    }

    #[tokio::test]
    async fn lightweight_cron_store_crud_and_due_occurrence_need_no_engine() {
        use platform_posix_minimal::{PosixClock, PosixFileSystem};

        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(branding::DOT_DIR)).expect("state dir");
        let store = MobileCronStoreHandle::new(
            temp.path().to_path_buf(),
            Arc::new(PosixFileSystem::new(temp.path().to_path_buf())),
            Arc::new(PosixClock::new()),
        );

        let created = store
            .create("* * * * *".to_string(), "hello".to_string(), false)
            .await
            .expect("one-shot creation");
        assert_eq!(1, store.list().await.len());
        let updated = store
            .update(
                created.id.clone(),
                "*/15 * * * *".to_string(),
                "updated".to_string(),
                true,
            )
            .await
            .expect("recurring update");
        assert_eq!("updated", updated.prompt);
        let due = store
            .due_occurrences(updated.next_fire_ms.expect("next fire").saturating_add(1))
            .await;
        assert_eq!(created.id, due[0].task_id);
        assert!(store.delete(created.id).await);
        assert!(store.list().await.is_empty());
    }

    /// In-memory encrypted store used to exercise post-boot provider credential
    /// writes without involving a platform keychain.
    #[derive(Default)]
    struct FakeEncryptedStore {
        map: StdMutex<HashMap<(String, String), protocol::SecureStorageData>>,
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
        ) -> Result<Option<protocol::SecureStorageData>, traits::SecureStorageError> {
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
                .filter(|(stored_service, _)| stored_service == service)
                .map(|(_, account)| account.clone())
                .collect())
        }

        fn is_encrypted(&self) -> bool {
            true
        }

        fn backend(&self) -> traits::SecureStorageBackend {
            traits::SecureStorageBackend::EncryptedFile
        }
    }

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
            // claude-code cron id = `randomUUID().slice(0,8)` → 8 lowercase hex
            // chars (NOT a `[bartwmdks]`-prefixed task id, and NOT deterministically
            // 'd'-prefixed — the previous `starts_with('d')` assertion passed only
            // ~1/16 of the time).
            assert_eq!(created.id.len(), 8, "cron id is 8 chars: {}", created.id);
            assert!(
                created
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
                "cron id is lowercase hex: {}",
                created.id
            );
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
            raw_content: memory_body.to_string(),
            content_differs_from_disk: false,
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
    use crate::local_apps_llm::test_support::ScriptedModel;
    use client_protocol::commands::ClientCommand;
    use client_protocol::error::ClientError;
    use client_protocol::events::ClientEvent as Ev;
    use client_protocol::permission::PermissionResponseDto;

    /// Build a real, fully-wired [`MobileEngineHandle`] off-device (host fake
    /// `Platform`) so the F3-05 `submit` path is exercised on CI. Returns the
    /// handle plus the recording listener so a test can read back delivered
    /// events.
    ///
    /// Task 11: `CreateApp` (and friends) now trigger a REAL background
    /// authoring/planning round trip. Off-device tests have no network, so
    /// this installs a deterministic, always-fails-fast local-apps model
    /// (zero scripted responses ⇒ an immediate, in-process
    /// `AppError::Io`, no I/O) instead of leaving every caller race a real
    /// `api.anthropic.com` request — a test that wants a scripted success
    /// overrides it via `set_local_apps_model` before triggering.
    fn build_submit_handle(root: &std::path::Path) -> (Arc<MobileEngineHandle>, Arc<FakeListener>) {
        let platform: Arc<dyn traits::Platform> =
            Arc::new(HostFakePlatform::new(root.to_path_buf()));
        let listener = Arc::new(FakeListener::default());
        let listener_dyn: Arc<dyn ClientEventListener> = listener.clone();
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let handle = build_mobile_engine(test_config(root), platform, listener_dyn, perm_sink)
            .expect("build_mobile_engine failed");
        handle.set_local_apps_model(ScriptedModel::new(Vec::new()));
        (handle, listener)
    }

    fn build_submit_handle_with_secure_store(
        root: &std::path::Path,
    ) -> (Arc<MobileEngineHandle>, Arc<FakeListener>) {
        let platform: Arc<dyn traits::Platform> = Arc::new(
            HostFakePlatform::new(root.to_path_buf())
                .with_secure_storage(Arc::new(FakeEncryptedStore::default())),
        );
        let listener = Arc::new(FakeListener::default());
        let listener_dyn: Arc<dyn ClientEventListener> = listener.clone();
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let handle = build_mobile_engine(test_config(root), platform, listener_dyn, perm_sink)
            .expect("build_mobile_engine failed");
        handle.set_local_apps_model(ScriptedModel::new(Vec::new()));
        (handle, listener)
    }

    #[test]
    fn provider_credentials_round_trip_through_mobile_submit() {
        use client_protocol::commands::ProviderCredentialSecretDto;

        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle_with_secure_store(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::SetProviderCredential {
                    operation_id: 11,
                    provider_id: "openai".into(),
                    credential: ProviderCredentialSecretDto::new("sk-test-secret".into()),
                })
                .await
                .expect("set provider credential");
            handle
                .submit(ClientCommand::ListProviderCredentials {
                    operation_id: 12,
                    provider_ids: vec!["openai".into()],
                })
                .await
                .expect("list provider credentials");
            handle
                .submit(ClientCommand::DeleteProviderCredential {
                    operation_id: 13,
                    provider_id: "openai".into(),
                })
                .await
                .expect("delete provider credential");

            let events = listener.received.lock().await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::ProviderCredentialStatus {
                    operation_id: 11,
                    configured_provider_ids,
                    storage_encrypted: true,
                    error: None,
                    ..
                } if configured_provider_ids == &vec!["openai".to_string()]
            )));
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::ProviderCredentialStatus {
                    operation_id: 12,
                    configured_provider_ids,
                    storage_encrypted: true,
                    error: None,
                    ..
                } if configured_provider_ids == &vec!["openai".to_string()]
            )));
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::ProviderCredentialStatus {
                    operation_id: 13,
                    configured_provider_ids,
                    storage_encrypted: true,
                    error: None,
                    ..
                } if configured_provider_ids.is_empty()
            )));
        });
    }

    /// Mobile's flat model event must retain the provider profile in both the
    /// catalog and the active selection so identical ids from different
    /// providers remain independently selectable.
    #[test]
    fn submit_model_events_use_provider_qualified_ids() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::ListModels)
                .await
                .expect("submit(ListModels) ok");

            let events = listener.received.lock().await.clone();
            let models = events
                .iter()
                .find_map(|event| match event {
                    Ev::ModelList { models, .. } => Some(models),
                    _ => None,
                })
                .expect("ModelList must be emitted");
            assert!(
                models.iter().any(|model| model == "openai/gpt-5.5"),
                "OpenAI's shared model id must stay qualified: {models:?}"
            );
            assert!(
                models.iter().any(|model| model == "github-copilot/gpt-5.5"),
                "Copilot's shared model id must stay qualified: {models:?}"
            );

            handle
                .submit(ClientCommand::SetModel {
                    model: "github-copilot/gpt-5.5".into(),
                })
                .await
                .expect("submit(SetModel) ok");

            let events = listener.received.lock().await;
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    Ev::ModelChanged { model } if model == "github-copilot/gpt-5.5"
                )),
                "ModelChanged must preserve the selected provider profile: {events:?}"
            );
        });
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

    /// `submit(Cancel)` does not return at token-fire time: it waits until the
    /// owned turn has unwound and released the slot, so an immediate session
    /// transition cannot race the cancelled task.
    #[test]
    fn submit_cancel_waits_for_cleanup_before_new_session() {
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
                handle.active_cancel.lock().await.is_none(),
                "Cancel must not return until the cancelled turn releases its slot"
            );
            handle
                .submit(ClientCommand::NewSession {
                    cwd: None,
                    model: None,
                })
                .await
                .expect("NewSession immediately after Cancel must not see an in-flight turn");
        });
    }

    /// A Block-behavior tool owns its mutation boundary until natural
    /// completion. Cancel must wait instead of aborting the outer turn.
    #[test]
    fn submit_cancel_waits_for_blocking_owner_to_finish() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, _listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            let turn = handle.reserve_turn(Some(7)).await.expect("reserve turn");
            let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
            let active = handle.active_cancel.clone();
            let task_turn = turn.clone();
            let parked = handle.runtime().spawn(async move {
                let _ = release_rx.await;
                let mut owner = active.lock().await;
                if owner
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &task_turn))
                {
                    *owner = None;
                }
                drop(owner);
                task_turn.mark_completed();
            });
            turn.set_task_handle(parked);

            let cancelling_handle = handle.clone();
            let cancel_task =
                tokio::spawn(async move { cancelling_handle.cancel_active_turn(Some(7)).await });
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while !turn.cancel.is_cancelled() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("cancel task must fire the matching token");
            assert!(
                !cancel_task.is_finished(),
                "Cancel must remain pending while the Block owner is active"
            );

            release_tx.send(()).expect("release Block owner");
            cancel_task
                .await
                .expect("cancel task joined")
                .expect("cancel completed");

            assert!(
                handle.active_cancel.lock().await.is_none(),
                "natural completion must release the single-turn slot"
            );
            assert!(turn.completed.load(std::sync::atomic::Ordering::Acquire));
        });
    }

    #[test]
    fn stale_specific_cancel_does_not_touch_current_turn() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, _listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            let turn = handle.reserve_turn(Some(8)).await.expect("reserve turn");
            handle
                .cancel_active_turn(Some(7))
                .await
                .expect("stale cancel is a no-op");

            assert!(!turn.cancel.is_cancelled());
            assert!(
                handle
                    .active_cancel
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(|owner| Arc::ptr_eq(owner, &turn)),
                "stale cancellation must retain the current owner"
            );

            *handle.active_cancel.lock().await = None;
            turn.mark_completed();
        });
    }

    #[tokio::test]
    async fn lifecycle_listener_rewrites_cancelled_terminal_and_drops_late_events() {
        let inner = Arc::new(FakeListener::default());
        let active = Arc::new(tokio::sync::Mutex::new(None));
        let turn = Arc::new(super::ActiveTurn::new(Some(12)));
        *active.lock().await = Some(turn.clone());
        let listener = super::TurnLifecycleListener::new(inner.clone(), active.clone());

        turn.cancel.cancel();
        listener
            .on_event(Ev::TurnEnded {
                outcome: client_protocol::events::TurnOutcomeDto::EndTurn,
                stop_reason: Some("end_turn".to_string()),
                cost: client_protocol::events::CostDto {
                    total_usd: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    api_calls: 0,
                    session_duration_secs: 0,
                    formatted: "$0.00".to_string(),
                },
            })
            .await;
        listener
            .on_event(Ev::ToolHeartbeat {
                id: "tool-1".to_string(),
                tool: "Bash".to_string(),
                elapsed_ms: 2_000,
            })
            .await;
        listener
            .on_event(Ev::ThinkingDelta {
                thinking: "late".to_string(),
                signature: None,
            })
            .await;
        listener
            .on_event(Ev::SystemNotice {
                message: "late notice".to_string(),
                is_error: false,
            })
            .await;

        let events = inner.received.lock().await;
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events.first(),
            Some(Ev::TurnEnded {
                outcome: client_protocol::events::TurnOutcomeDto::Cancelled,
                stop_reason: Some(reason),
                ..
            }) if reason == "cancelled"
        ));
    }

    #[tokio::test]
    async fn lifecycle_listener_drops_unowned_live_turn_payloads() {
        let inner = Arc::new(FakeListener::default());
        let active = Arc::new(tokio::sync::Mutex::new(None));
        let listener = super::TurnLifecycleListener::new(inner.clone(), active);

        listener
            .on_event(Ev::ToolUseStarted {
                id: "stale-tool".to_string(),
                tool: "Read".to_string(),
                input_json: "{}".to_string(),
            })
            .await;
        listener
            .on_event(Ev::ApiRetry {
                message: "late retry".to_string(),
                attempt: 1,
                max_retries: 3,
                delay_ms: 100,
            })
            .await;
        listener
            .on_event(Ev::SystemNotice {
                message: "unowned notice".to_string(),
                is_error: false,
            })
            .await;

        assert!(inner.received.lock().await.is_empty());
    }

    /// A connection owns at most one live turn. A second `SendPrompt` must be
    /// rejected instead of replacing the first turn's cancellation token,
    /// otherwise Cancel and session guards start controlling the wrong task.
    #[test]
    fn submit_send_prompt_rejects_overlapping_turn() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, _listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            *handle.active_cancel.lock().await = Some(Arc::new(super::ActiveTurn::new(None)));

            let result = handle
                .submit(ClientCommand::SendPrompt {
                    text: "must be rejected".into(),
                    prompt_mode: None,
                    images: Vec::new(),
                    turn_id: Some(99),
                })
                .await;

            assert!(
                matches!(result, Err(ClientError::Rejected { .. })),
                "overlapping SendPrompt must be rejected, got {result:?}"
            );
        });
    }

    /// A provider/model failure is terminal for the connection slot just like a
    /// successful or cancelled turn. The orchestrator surfaces authentication
    /// failure as a `model_error` turn, then session control becomes available.
    #[test]
    fn submit_model_error_releases_slot() {
        use crate::test_support::new_engine_with_streaming;
        use orchestrator::test_support_stream::MockStreamingApiClient;

        let tmp = tempfile::tempdir().expect("tempdir");
        let platform: Arc<dyn traits::Platform> =
            Arc::new(HostFakePlatform::new(tmp.path().to_path_buf()));
        let listener = Arc::new(FakeListener::default());
        let listener_dyn: Arc<dyn ClientEventListener> = listener.clone();
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let streaming: Arc<dyn orchestrator::StreamingApiClient> =
            Arc::new(MockStreamingApiClient::with_open_error(
                llm_client::LlmError::Authentication {
                    message: String::new(),
                },
                Vec::new(),
            ));
        let handle = new_engine_with_streaming(
            test_config(tmp.path()),
            platform,
            listener_dyn,
            perm_sink,
            Some(streaming),
        )
        .expect("build_mobile_engine failed");

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::SendPrompt {
                    text: "fail deterministically".into(),
                    prompt_mode: None,
                    images: Vec::new(),
                    turn_id: Some(100),
                })
                .await
                .expect("submit(SendPrompt) ok");

            for _ in 0..2000 {
                let failed = listener.received.lock().await.iter().any(|event| {
                    matches!(
                        event,
                        Ev::TurnEnded { stop_reason, .. }
                            if stop_reason.as_deref() == Some("model_error")
                    )
                });
                if failed && handle.active_cancel.lock().await.is_none() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }

            let events = listener.received.lock().await.clone();
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    Ev::TurnEnded { stop_reason, .. }
                        if stop_reason.as_deref() == Some("model_error")
                )),
                "a provider failure must terminate as model_error: {events:?}"
            );
            assert!(
                handle.active_cancel.lock().await.is_none(),
                "a failed turn must release its connection slot"
            );
            handle
                .submit(ClientCommand::ClearSession)
                .await
                .expect("session control must work after a failed turn");
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
            let after_uuid = oh.current_session_id().await.as_uuid().to_string();
            assert_ne!(before, after, "NewSession must mint a fresh session id");

            let events = drained(&listener).await;
            let started = events.iter().find_map(|e| match e {
                Ev::SessionStarted { session_id } => Some(session_id.clone()),
                _ => None,
            });
            assert_eq!(
                started.expect("a SessionStarted event must be emitted"),
                after_uuid,
                "SessionStarted must carry the bare resumable UUID"
            );

            let path =
                session::jsonl::session_path(&handle.lingxi_home, &handle.session_cwd, &after_uuid);
            let raw = std::fs::read_to_string(path).expect("new session anchor exists");
            let anchor: serde_json::Value =
                serde_json::from_str(raw.trim()).expect("anchor is valid json");
            assert_eq!(anchor["sessionId"], after_uuid);
            assert_eq!(anchor["mobileEmptySession"], 1);

            handle
                .submit(ClientCommand::ListSessions { limit: None })
                .await
                .expect("list anchored empty session");
            let events = drained(&listener).await;
            let row = events.iter().rev().find_map(|event| match event {
                Ev::SessionList { sessions } => {
                    sessions.iter().find(|row| row.uuid == after_uuid).cloned()
                }
                _ => None,
            });
            let row = row.expect("anchored empty session must be listed");
            assert_eq!(row.message_count, 0);
        });
    }

    #[test]
    fn submit_resume_session_restores_anchored_empty_session_with_same_uuid() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            use traits::OrchestratorHandle;

            handle
                .submit(ClientCommand::NewSession {
                    cwd: None,
                    model: None,
                })
                .await
                .expect("create anchored empty session");
            let expected = handle
                .inner()
                .orchestrator
                .current_session_id()
                .await
                .as_uuid()
                .to_string();

            handle
                .submit(ClientCommand::ResumeSession {
                    session_id: expected.clone(),
                    cwd: None,
                })
                .await
                .expect("anchored empty session resumes");

            assert_eq!(
                handle
                    .inner()
                    .orchestrator
                    .current_session_id()
                    .await
                    .as_uuid()
                    .to_string(),
                expected,
            );
            let events = drained(&listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::SessionResumed {
                    session_id,
                    messages,
                } if session_id == &expected && messages.is_empty()
            )));
        });
    }

    #[test]
    fn resume_empty_session_bootstraps_legacy_project_index_uuid() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());
        let expected = "dddddddd-4444-4444-8444-dddddddddddd";

        handle.runtime().block_on(async {
            use traits::OrchestratorHandle;

            handle
                .resume_empty_session(expected.into(), "旧空会话".into())
                .await
                .expect("legacy indexed empty session resumes");

            assert_eq!(
                handle
                    .inner()
                    .orchestrator
                    .current_session_id()
                    .await
                    .as_uuid()
                    .to_string(),
                expected,
            );
            let path =
                session::jsonl::session_path(&handle.lingxi_home, &handle.session_cwd, expected);
            let raw = std::fs::read_to_string(path).expect("migration anchor exists");
            assert!(raw.contains("\"mobileEmptySession\":1"));
            assert!(raw.contains("旧空会话"));

            let events = drained(&listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::SessionResumed {
                    session_id,
                    messages,
                } if session_id == expected && messages.is_empty()
            )));
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

    /// Older Android builds persisted `SessionId::Display` (`sess:<uuid>`) in
    /// their Project session index. The mobile resume boundary accepts that
    /// legacy spelling once, but emits the canonical bare UUID so the client can
    /// rewrite its cache without carrying the prefix forward.
    #[test]
    fn submit_resume_session_accepts_legacy_display_prefix() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (file_uuid, seeded_count) = seed_replay_valid_session(tmp.path());
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::ResumeSession {
                    session_id: format!("sess:{file_uuid}"),
                    cwd: None,
                })
                .await
                .expect("legacy prefixed session id must resume");

            let events = drained(&listener).await;
            let resumed = events.iter().find_map(|event| match event {
                Ev::SessionResumed {
                    session_id,
                    messages,
                } => Some((session_id.clone(), messages.len())),
                _ => None,
            });
            assert_eq!(
                resumed,
                Some((file_uuid, seeded_count)),
                "legacy input must be confirmed with a bare UUID"
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

    #[test]
    fn mobile_model_refs_keep_duplicate_provider_models_distinct() {
        let listings = vec![
            traits::ModelListing {
                display_model: "gpt-5.5".into(),
                request_model: "gpt-5.5".into(),
                provider_id: "openai".into(),
                provider_label: "OpenAI".into(),
                description: None,
                supports_reasoning: true,
            },
            traits::ModelListing {
                display_model: "gpt-5.5".into(),
                request_model: "gpt-5.5".into(),
                provider_id: "github-copilot".into(),
                provider_label: "GitHub Copilot".into(),
                description: None,
                supports_reasoning: true,
            },
        ];

        let refs = traits::curated_model_refs(
            &listings,
            &["gpt-5.5".into()],
            "gpt-5.5",
            Some("github-copilot"),
        );

        assert_eq!(refs[0], "github-copilot/gpt-5.5");
        assert!(refs.iter().any(|model| model == "openai/gpt-5.5"));
        assert_eq!(
            refs.iter()
                .filter(|model| model.as_str() == "github-copilot/gpt-5.5")
                .count(),
            1,
            "the active model and catalog row must de-duplicate by qualified id"
        );
        assert!(
            !refs.iter().any(|model| model == "gpt-5.5"),
            "ambiguous bare ids must not leak into the mobile picker"
        );
    }

    // ── LOCAL-APPS (phase 1): handler-level tests (command in → state +
    //    events out) over the real engine handle ─────────────────────────────

    use client_protocol::local_apps::{
        AppCreateOriginDto, AppDesignPatchDto, AppDesignPatchOpDto, AppErrorCodeDto, AppEventDto,
        AppRuntimeStateDto, AppWorkflowStateDto, DesignValueDto,
    };

    /// Drain and return every event delivered to the fake listener so far.
    /// Event delivery is asynchronous, in two ordered stages: `AppService`
    /// hands events to spawned emission tasks (commit → enqueue onto the
    /// bridge's emission channel), and the bridge's single forwarder drains
    /// that channel to the sink (enqueue → deliver). A bare take races both,
    /// so the barrier is two-stage too: `flush_events` waits until everything
    /// committed is ENQUEUED, then the queue flush waits until everything
    /// enqueued is DELIVERED.
    async fn drain_events(handle: &MobileEngineHandle, listener: &FakeListener) -> Vec<Ev> {
        if let Ok(service) = handle.local_apps() {
            service.flush_events().await;
        }
        handle.app_emissions.flush().await;
        std::mem::take(&mut *listener.received.lock().await)
    }

    fn title_patch(value: &str) -> AppDesignPatchDto {
        AppDesignPatchDto {
            ops: vec![AppDesignPatchOpDto::Set {
                field_id: "title".into(),
                value: DesignValueDto::ShortText {
                    value: value.into(),
                },
            }],
            note: None,
        }
    }

    fn apps_changed_rows(events: &[Ev]) -> Option<Vec<client_protocol::local_apps::AppRecordDto>> {
        events.iter().rev().find_map(|event| match event {
            Ev::AppsChanged { apps } => Some(apps.clone()),
            _ => None,
        })
    }

    /// Task 11: `CreateApp` triggers a REAL background authoring round trip,
    /// which `build_submit_handle`'s default test double fails immediately
    /// (no scripted response) — deterministically, but concurrently with the
    /// caller. Many tests below need `collecting_spec` for scaffolding
    /// unrelated to authoring itself; this settles that background failure
    /// FIRST (so nothing races the reset), resets to
    /// `authoring_questionnaire` directly on the service (bypassing
    /// `submit`, so no SECOND background round trip triggers), then splices
    /// the fixture questionnaire exactly as `advance_to_collecting_spec`
    /// always has.
    async fn seed_collecting_spec(
        handle: &MobileEngineHandle,
        service: &local_apps::AppService,
        app_id: &str,
    ) -> local_apps::AppRecord {
        handle.settle_local_apps().await;
        service
            .retry_questionnaire(app_id)
            .await
            .expect("reset to authoring_questionnaire for test scaffolding");
        local_apps::test_support::advance_to_collecting_spec(service, app_id).await
    }

    /// (local-apps#questionnaire, Task 5, coordinator ruling: total removal of
    /// the static template catalog): this test used to also cover
    /// `ListAppTemplates` → `AppTemplatesChanged` before the FIRST assertion
    /// below; that command/event pair is deleted along with the catalog, so
    /// the test is renamed to describe what it still covers — `CreateApp` →
    /// `AppsChanged` and `GetAppDetails` → `AppDetailsChanged`.
    #[test]
    fn local_apps_create_and_details_round_trip_through_submit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: "Tracker".into(),
                    origin: AppCreateOriginDto::Library,
                    brief: "a test app".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            let events = drain_events(&handle, &listener).await;
            let app_id = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged")
                [0]
            .id
            .clone();

            handle
                .submit(ClientCommand::GetAppDetails {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(GetAppDetails)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppEvent {
                    event: AppEventDto::AppDetailsChanged { details }
                } if details.app.id == app_id
                    && details.design_revision == 0
                    && matches!(details.runtime.state, AppRuntimeStateDto::Stopped)
            )));
        });
    }

    /// PINS the Task 11 fix: `ClientCommand::CreateApp` now carries a real
    /// `brief` field, and `handle_create_app` persists the CALLER-SUPPLIED
    /// brief — not `name` doubling as the brief (the deliberate placeholder
    /// this test used to pin, `create_app_persists_name_as_brief_until_
    /// task_11_adds_a_real_one`, until this task landed). All three LLM
    /// stages read `AppRecord.brief`, so a client-created app must author
    /// its questionnaire from the caller's real spec, not a bare display
    /// name.
    ///
    /// Mirrors `create_persists_the_caller_supplied_brief_and_does_not_
    /// overwrite_a_supplied_name` in `local_apps_mcp.rs` (Task 10's side of
    /// this same fix): `name` and `brief` are asserted UNEQUAL and both
    /// checked, so this cannot pass by conflating them back together. The
    /// `NAME` fixture stays longer than `AppService::create_app`'s 24-char
    /// placeholder cut so a regression to `create_app(None, brief, ..)`
    /// (`name` silently dropped) is visible too: it would come back as the
    /// brief's own 24-char prefix instead of `NAME`.
    #[test]
    fn create_app_persists_the_caller_supplied_brief_and_does_not_overwrite_a_supplied_name() {
        const NAME: &str = "Habit Tracker Deluxe Edition";
        const BRIEF: &str = "一个记事本 app，用来跟踪每天的习惯打卡";
        assert!(
            NAME.chars().count() > 24,
            "test fixture must exceed the placeholder cut to be meaningful"
        );
        assert_ne!(
            NAME, BRIEF,
            "name and brief must be distinct fixtures so the test cannot pass by \
             conflating them"
        );
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: NAME.into(),
                    origin: AppCreateOriginDto::Library,
                    brief: BRIEF.into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            let events = drain_events(&handle, &listener).await;
            let app_id = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged")
                [0]
            .id
            .clone();

            let service = handle.local_apps().expect("local-apps service");
            let record = service.record(&app_id).await.expect("record");
            assert_eq!(
                record.name, NAME,
                "a caller-supplied name must not be silently overwritten"
            );
            assert_eq!(
                record.brief, BRIEF,
                "the brief the caller supplied is the brief that gets stored — not the \
                 name, not empty, not anything else"
            );
        });
    }

    // ── Task 11: host-orchestrated authoring / planning triggers ────────────

    /// A valid `emit_questionnaire` tool payload — one step, one deferrable
    /// multiple-choice field, matching `local_apps_llm.rs`'s own fixture
    /// shape (already exercised there against the real validator).
    fn good_questionnaire() -> serde_json::Value {
        serde_json::json!({
            "suggestedName": "记事本",
            "steps": [{
                "id": "basics", "order": 0, "title": "功能",
                "fields": [{
                    "id": "features", "label": "需要哪些功能",
                    "fieldType": "multiple_choice", "required": true,
                    "allowsCustom": true, "allowsDefer": true,
                    "options": [{"value": "list", "label": "笔记列表"}]
                }]
            }]
        })
    }

    /// A second, DIFFERENT valid questionnaire (distinct step id) — proves a
    /// re-authoring round trip actually replaced the old questionnaire
    /// rather than coincidentally matching it.
    fn other_questionnaire() -> serde_json::Value {
        serde_json::json!({
            "suggestedName": "待办清单",
            "steps": [{
                "id": "todo_basics", "order": 0, "title": "任务",
                "fields": [{
                    "id": "priority", "label": "需要区分优先级吗",
                    "fieldType": "boolean", "required": true,
                    "allowsCustom": false, "allowsDefer": true,
                    "options": []
                }]
            }]
        })
    }

    /// A valid `emit_plan` tool payload, matching `local_apps_llm.rs`'s own
    /// fixture shape.
    fn good_plan() -> serde_json::Value {
        serde_json::json!({
            "summary": "一个记事本，帮你记录日常想法。",
            "collections": [{
                "id": "notes", "name": "笔记",
                "fields": [{"id": "title", "label": "标题", "kind": "text", "required": true}]
            }],
            "capabilities": ["data_mutation"],
            "domains": []
        })
    }

    /// `CreateApp` starts authoring in the background; once it settles, the
    /// app has moved past `authoring_questionnaire` to `collecting_spec` and
    /// the LLM's suggested name has replaced the create-time placeholder.
    #[test]
    fn creating_an_app_drives_authoring_to_collecting_spec() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());
        handle.set_local_apps_model(ScriptedModel::new(vec![Ok(good_questionnaire())]));

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: String::new(),
                    origin: AppCreateOriginDto::Library,
                    brief: "一个记事本".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            handle.settle_local_apps().await;
            let events = drain_events(&handle, &listener).await;
            let app_id = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged")
                [0]
            .id
            .clone();

            let service = handle.local_apps().expect("local-apps service");
            let record = service.record(&app_id).await.expect("record");
            assert_eq!(
                record.workflow_state,
                local_apps::AppWorkflowState::CollectingSpec
            );
            assert_eq!(
                record.name, "记事本",
                "the suggested name replaced the create-time placeholder"
            );
        });
    }

    /// A model failure during authoring fails closed — `questionnaire_failed`,
    /// never a silent fallback — and `RetryAppQuestionnaire` recovers from
    /// there once the model is available again.
    #[test]
    fn a_model_failure_lands_in_questionnaire_failed_and_stays_retryable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());
        handle.set_local_apps_model(ScriptedModel::new(vec![
            Err(local_apps::AppError::LlmUnavailable("offline".into())),
            Ok(good_questionnaire()),
        ]));

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: String::new(),
                    origin: AppCreateOriginDto::Library,
                    brief: "一个记事本".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            handle.settle_local_apps().await;
            let events = drain_events(&handle, &listener).await;
            let app_id = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged")
                [0]
            .id
            .clone();

            let service = handle.local_apps().expect("local-apps service");
            assert_eq!(
                service.record(&app_id).await.expect("record").workflow_state,
                local_apps::AppWorkflowState::QuestionnaireFailed
            );

            handle
                .submit(ClientCommand::RetryAppQuestionnaire {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(RetryAppQuestionnaire)");
            handle.settle_local_apps().await;
            assert_eq!(
                service.record(&app_id).await.expect("record").workflow_state,
                local_apps::AppWorkflowState::CollectingSpec
            );
        });
    }

    /// `BeginAppPlanning` validates the answers, starts planning in the
    /// background, and — once it settles — opens the spec-confirmation gate
    /// with a plan stamped against the current draft revision.
    #[test]
    fn beginning_planning_drives_through_to_the_confirmation_gate() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());
        handle.set_local_apps_model(ScriptedModel::new(vec![
            Ok(good_questionnaire()),
            Ok(good_plan()),
        ]));

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: String::new(),
                    origin: AppCreateOriginDto::Library,
                    brief: "一个记事本".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            handle.settle_local_apps().await;
            let events = drain_events(&handle, &listener).await;
            let app_id = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged")
                [0]
            .id
            .clone();

            handle
                .submit(ClientCommand::UpdateAppDesignDraft {
                    app_id: app_id.clone(),
                    expected_revision: 0,
                    patch: AppDesignPatchDto {
                        ops: vec![AppDesignPatchOpDto::Set {
                            field_id: "features".into(),
                            value: DesignValueDto::MultipleChoice {
                                value: vec!["list".into()],
                            },
                        }],
                        note: None,
                    },
                })
                .await
                .expect("submit(UpdateAppDesignDraft)");

            handle
                .submit(ClientCommand::BeginAppPlanning {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(BeginAppPlanning)");
            handle.settle_local_apps().await;

            let service = handle.local_apps().expect("local-apps service");
            let record = service.record(&app_id).await.expect("record");
            assert_eq!(
                record.workflow_state,
                local_apps::AppWorkflowState::AwaitingSpecConfirmation
            );
            let draft = service.draft(&app_id).await.expect("draft");
            assert_eq!(draft.plan_for_revision, Some(draft.revision));
        });
    }

    /// `UpdateAppBrief` discards the old questionnaire/answers and
    /// re-authors from scratch — once it settles, the draft carries the
    /// FRESH questionnaire, not the one authored from the original brief.
    #[test]
    fn changing_the_brief_reauthors_the_questionnaire() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());
        handle.set_local_apps_model(ScriptedModel::new(vec![
            Ok(good_questionnaire()),
            Ok(other_questionnaire()),
        ]));

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: String::new(),
                    origin: AppCreateOriginDto::Library,
                    brief: "一个记事本".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            handle.settle_local_apps().await;
            let events = drain_events(&handle, &listener).await;
            let app_id = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged")
                [0]
            .id
            .clone();

            handle
                .submit(ClientCommand::UpdateAppBrief {
                    app_id: app_id.clone(),
                    brief: "改成一个待办清单".into(),
                })
                .await
                .expect("submit(UpdateAppBrief)");
            handle.settle_local_apps().await;

            let service = handle.local_apps().expect("local-apps service");
            let draft = service.draft(&app_id).await.expect("draft");
            assert_eq!(
                draft.questionnaire[0].id, "todo_basics",
                "a fresh questionnaire replaced the old one"
            );
            assert!(draft.fields.is_empty());
        });
    }

    /// Index of the first event matching `pred`, or a panic naming what was
    /// expected and the whole batch. The app surface delivers through ONE
    /// ordered channel (channel order = commit order), so multi-event batches
    /// assert relative POSITIONS, not mere membership (W5).
    fn position_of(events: &[Ev], what: &str, pred: impl Fn(&Ev) -> bool) -> usize {
        events
            .iter()
            .position(pred)
            .unwrap_or_else(|| panic!("{what} not found in {events:?}"))
    }

    #[test]
    fn local_apps_designer_flow_round_trips_through_submit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            // Create (library origin ⇒ no conversation binding).
            handle
                .submit(ClientCommand::CreateApp {
                    name: "Habit Tracker".into(),
                    origin: AppCreateOriginDto::Library,
                    brief: "a test app".into(),
                    conversation_id: Some("conv-ignored".into()),
                })
                .await
                .expect("submit(CreateApp)");
            // Task 11: `CreateApp` triggers a background authoring round trip.
            // `build_submit_handle`'s default model has no scripted response,
            // so it fails immediately and deterministically —
            // `settle_local_apps` waits for that to land BEFORE any event/
            // state read below, so `workflow_state` is read once settled
            // rather than raced against the still-running background task.
            handle.settle_local_apps().await;
            let events = drain_events(&handle, &listener).await;
            let apps = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged");
            assert_eq!(apps.len(), 1);
            assert_eq!(apps[0].name, "Habit Tracker");
            assert_eq!(
                apps[0].workflow_state,
                AppWorkflowStateDto::QuestionnaireFailed,
                "the default test double has no scripted response, so background \
                 authoring fails immediately"
            );
            assert_eq!(
                apps[0].conversation_id, None,
                "a library-origin create binds no conversation"
            );
            let app_id = apps[0].id.clone();
            assert_eq!(apps[0].workspace_rel, format!("apps/{app_id}/workspace"));

            // `OpenAppDesigner` needs `collecting_spec`; stand in for Task
            // 4/8's not-yet-wired questionnaire-authoring LLM round trip.
            let service = handle.local_apps().expect("local-apps service");
            seed_collecting_spec(&handle, &service, &app_id).await;

            // Open the designer gate; the interaction id reaches the client
            // ONLY through this event (spec §I gating).
            handle
                .submit(ClientCommand::OpenAppDesigner {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(OpenAppDesigner)");
            let events = drain_events(&handle, &listener).await;
            let (designer_interaction, designer_revision) = events
                .iter()
                .find_map(|event| match event {
                    Ev::AppDesignerRequested {
                        app_id: id,
                        interaction_id,
                        revision,
                    } if *id == app_id => Some((interaction_id.clone(), *revision)),
                    _ => None,
                })
                .expect("AppDesignerRequested must be emitted");
            assert_eq!(designer_revision, 0);
            // Order-sensitive (W5): the gate-opening batch is
            // state-change-first (`WorkflowChanged` before the gate
            // announcement), and domain events precede the trailing
            // `AppsChanged` snapshot.
            let workflow_at = position_of(&events, "AppWorkflowChanged(awaiting_spec)", |event| {
                matches!(
                    event,
                    Ev::AppWorkflowChanged {
                        state: AppWorkflowStateDto::AwaitingSpecConfirmation,
                        ..
                    }
                )
            });
            let designer_at = position_of(&events, "AppDesignerRequested", |event| {
                matches!(event, Ev::AppDesignerRequested { .. })
            });
            let snapshot_at = position_of(&events, "AppsChanged snapshot", |event| {
                matches!(event, Ev::AppsChanged { .. })
            });
            assert!(
                workflow_at < designer_at,
                "state-change-first: WorkflowChanged must precede DesignerRequested \
                 (got {events:?})"
            );
            assert!(
                designer_at < snapshot_at,
                "domain events must precede the trailing AppsChanged snapshot \
                 (got {events:?})"
            );

            // A conflicting update is rejected: conflict event + typed
            // failure, and the stale value is NOT applied.
            handle
                .submit(ClientCommand::UpdateAppDesignDraft {
                    app_id: app_id.clone(),
                    expected_revision: 7,
                    patch: title_patch("Stale"),
                })
                .await
                .expect("submit(UpdateAppDesignDraft stale)");
            let events = drain_events(&handle, &listener).await;
            // Order-sensitive (W5): the spec-required cause precedes the
            // synthesized failure on the wire.
            let conflict_at = position_of(&events, "AppDesignConflict", |event| {
                matches!(
                    event,
                    Ev::AppDesignConflict {
                        expected_revision: 7,
                        actual_revision: 0,
                        ..
                    }
                )
            });
            let failed_at =
                position_of(&events, "AppOperationFailed(revision_conflict)", |event| {
                    matches!(
                        event,
                        Ev::AppOperationFailed {
                            code: AppErrorCodeDto::RevisionConflict,
                            ..
                        }
                    )
                });
            assert!(
                conflict_at < failed_at,
                "AppDesignConflict (the cause) must precede AppOperationFailed \
                 (got {events:?})"
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Ev::AppDesignDraftChanged { .. })),
                "a conflicting edit must not change the draft"
            );

            // A valid update bumps the revision and carries the field map.
            handle
                .submit(ClientCommand::UpdateAppDesignDraft {
                    app_id: app_id.clone(),
                    expected_revision: 0,
                    patch: title_patch("Mine"),
                })
                .await
                .expect("submit(UpdateAppDesignDraft)");
            let events = drain_events(&handle, &listener).await;
            let fields = events
                .iter()
                .find_map(|event| match event {
                    Ev::AppDesignDraftChanged {
                        revision: 1,
                        fields,
                        ..
                    } => Some(fields.clone()),
                    _ => None,
                })
                .expect("AppDesignDraftChanged at revision 1");
            assert!(matches!(
                fields.get("title"),
                Some(DesignValueDto::ShortText { value }) if value == "Mine"
            ));

            // Confirm gating (spec §I): a guessed interaction id fails…
            handle
                .submit(ClientCommand::ConfirmAppDesign {
                    app_id: app_id.clone(),
                    revision: 1,
                    interaction_id: "int-guessed".into(),
                })
                .await
                .expect("submit(ConfirmAppDesign guessed)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppOperationFailed {
                    code: AppErrorCodeDto::InteractionInvalid,
                    ..
                }
            )));
            // …and the right id with a stale revision fails too.
            handle
                .submit(ClientCommand::ConfirmAppDesign {
                    app_id: app_id.clone(),
                    revision: 0,
                    interaction_id: designer_interaction.clone(),
                })
                .await
                .expect("submit(ConfirmAppDesign stale)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppOperationFailed {
                    code: AppErrorCodeDto::RevisionConflict,
                    ..
                }
            )));
            let service = handle.local_apps().expect("local-apps service");
            assert_eq!(
                service.record(&app_id).await.unwrap().workflow_state,
                local_apps::AppWorkflowState::AwaitingSpecConfirmation,
                "failed confirms must not advance the workflow"
            );

            local_apps::test_support::stamp_fresh_plan(&service, &app_id).await;
            // The exact pending id + the current revision confirms → generating.
            handle
                .submit(ClientCommand::ConfirmAppDesign {
                    app_id: app_id.clone(),
                    revision: 1,
                    interaction_id: designer_interaction,
                })
                .await
                .expect("submit(ConfirmAppDesign)");
            let events = drain_events(&handle, &listener).await;
            // Order-sensitive (W5): the workflow move precedes its trailing
            // snapshot.
            let generating_at = position_of(&events, "AppWorkflowChanged(generating)", |event| {
                matches!(
                    event,
                    Ev::AppWorkflowChanged {
                        state: AppWorkflowStateDto::Generating,
                        ..
                    }
                )
            });
            let snapshot_at = position_of(&events, "AppsChanged snapshot", |event| {
                matches!(event, Ev::AppsChanged { .. })
            });
            assert!(
                generating_at < snapshot_at,
                "WorkflowChanged(generating) must precede the AppsChanged snapshot \
                 (got {events:?})"
            );

            // Generation/validation transitions are AppService seams (the
            // phase-3 generator drives them); their domain events must ride
            // the SAME outbound sink as the command replies — and in commit
            // order: Validating first, then the gate-opening pair
            // state-change-first (WorkflowChanged before PreviewReady).
            service
                .generation_complete(&app_id)
                .await
                .expect("generation_complete");
            service
                .validation_passed(&app_id)
                .await
                .expect("validation_passed");
            let events = drain_events(&handle, &listener).await;
            let validating_at = position_of(&events, "AppWorkflowChanged(validating)", |event| {
                matches!(
                    event,
                    Ev::AppWorkflowChanged {
                        state: AppWorkflowStateDto::Validating,
                        ..
                    }
                )
            });
            let awaiting_preview_at = position_of(
                &events,
                "AppWorkflowChanged(awaiting_preview_confirmation)",
                |event| {
                    matches!(
                        event,
                        Ev::AppWorkflowChanged {
                            state: AppWorkflowStateDto::AwaitingPreviewConfirmation,
                            ..
                        }
                    )
                },
            );
            let preview_at = position_of(&events, "AppPreviewReady", |event| {
                matches!(event, Ev::AppPreviewReady { .. })
            });
            assert!(
                validating_at < awaiting_preview_at && awaiting_preview_at < preview_at,
                "seam events must arrive in commit order, state-change-first \
                 (got {events:?})"
            );
            let (preview_interaction, preview_revision) = events
                .iter()
                .find_map(|event| match event {
                    Ev::AppPreviewReady {
                        interaction_id,
                        revision,
                        url: None,
                        ..
                    } => Some((interaction_id.clone(), *revision)),
                    _ => None,
                })
                .expect("AppPreviewReady must be emitted with no url in phase 1");
            assert_eq!(preview_revision, 1);

            // Confirm the preview through the UI command path → ready.
            handle
                .submit(ClientCommand::ConfirmAppPreview {
                    app_id: app_id.clone(),
                    revision: preview_revision,
                    interaction_id: preview_interaction,
                })
                .await
                .expect("submit(ConfirmAppPreview)");
            let events = drain_events(&handle, &listener).await;
            // Order-sensitive (W5): the workflow move precedes its trailing
            // snapshot.
            let ready_at = position_of(&events, "AppWorkflowChanged(ready)", |event| {
                matches!(
                    event,
                    Ev::AppWorkflowChanged {
                        state: AppWorkflowStateDto::Ready,
                        ..
                    }
                )
            });
            let snapshot_at = position_of(&events, "AppsChanged snapshot", |event| {
                matches!(event, Ev::AppsChanged { .. })
            });
            assert!(
                ready_at < snapshot_at,
                "WorkflowChanged(ready) must precede the AppsChanged snapshot \
                 (got {events:?})"
            );
            assert_eq!(
                service.record(&app_id).await.unwrap().workflow_state,
                local_apps::AppWorkflowState::Ready
            );
        });
    }

    #[test]
    fn local_apps_runtime_and_checkpoint_commands_report_real_state() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: "Board".into(),
                    origin: AppCreateOriginDto::Chat,
                    brief: "a test app".into(),
                    conversation_id: Some("conv-7".into()),
                })
                .await
                .expect("submit(CreateApp)");
            let events = drain_events(&handle, &listener).await;
            let apps = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged");
            assert_eq!(
                apps[0].conversation_id.as_deref(),
                Some("conv-7"),
                "a chat-origin create keeps the conversation binding"
            );
            let app_id = apps[0].id.clone();

            // Persisted capability grants are revocable from the native
            // permissions page without exposing permissions.json to clients.
            let layout = local_apps::AppLayout::new(tmp.path(), app_id.clone()).unwrap();
            let mut permissions = local_apps::AppPermissions::default();
            permissions.grant(local_apps::AppCapability::DataMutation);
            permissions.grant_domain("api.example.com").unwrap();
            local_apps::save_permissions(&layout, &permissions).unwrap();
            handle
                .submit(ClientCommand::ResetAppPermissions {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(ResetAppPermissions)");
            assert_eq!(
                local_apps::load_permissions(&layout).unwrap(),
                local_apps::AppPermissions::default()
            );

            // A record without generated build output fails honestly and never
            // reports a false running state.
            handle
                .submit(ClientCommand::StartApp {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(StartApp)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppOperationFailed { app_id: Some(id), .. } if *id == app_id
            )));
            let service = handle.local_apps().expect("local-apps service");
            assert_ne!(
                service.runtime_record(&app_id).await.unwrap().state,
                local_apps::AppRuntimeState::Running,
                "a missing build must never be surfaced as running"
            );

            // A missing app is not_found, not not_yet_available.
            handle
                .submit(ClientCommand::StartApp {
                    app_id: "beadfeed".into(),
                })
                .await
                .expect("submit(StartApp missing)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppOperationFailed {
                    code: AppErrorCodeDto::NotFound,
                    ..
                }
            )));

            // Checkpoint lists now use the explicit snapshot event, including
            // an empty list for a new app.
            handle
                .submit(ClientCommand::ListAppCheckpoints {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(ListAppCheckpoints)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppEvent {
                    event: AppEventDto::AppCheckpointsChanged { app_id: id, checkpoints }
                } if *id == app_id && checkpoints.is_empty()
            )));
            handle
                .submit(ClientCommand::ListAppCheckpoints {
                    app_id: "beadfeed".into(),
                })
                .await
                .expect("submit(ListAppCheckpoints missing)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppOperationFailed {
                    code: AppErrorCodeDto::NotFound,
                    ..
                }
            )));
        });
    }

    #[test]
    fn local_apps_state_survives_engine_rebuild_and_delete_removes_the_app() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());
        let app_id = handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: "Persist".into(),
                    origin: AppCreateOriginDto::Library,
                    brief: "a test app".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            let events = drain_events(&handle, &listener).await;
            let apps = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged");
            let app_id = apps[0].id.clone();
            let service = handle.local_apps().expect("local-apps service");
            seed_collecting_spec(&handle, &service, &app_id).await;
            handle
                .submit(ClientCommand::UpdateAppDesignDraft {
                    app_id: app_id.clone(),
                    expected_revision: 0,
                    patch: title_patch("Kept"),
                })
                .await
                .expect("submit(UpdateAppDesignDraft)");
            app_id
        });
        // The store lives at the per-profile data root (`<root>/apps/…`) —
        // `test_config` roots `lingxi_home` under the temp dir, so its parent
        // (the temp root) is the profile root.
        assert!(tmp
            .path()
            .join("apps")
            .join(&app_id)
            .join("workspace")
            .join(branding::DOT_DIR)
            .join("design-spec.json")
            .is_file());
        drop(handle);

        // A brand-new engine over the same root rebuilds from disk alone.
        let (handle, listener) = build_submit_handle(tmp.path());
        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::ListApps)
                .await
                .expect("submit(ListApps)");
            let events = drain_events(&handle, &listener).await;
            let apps = apps_changed_rows(&events).expect("ListApps replies with AppsChanged");
            assert_eq!(apps.len(), 1);
            assert_eq!(apps[0].id, app_id);
            let service = handle.local_apps().expect("local-apps service");
            assert_eq!(
                service.draft(&app_id).await.unwrap().revision,
                1,
                "the draft revision survives the engine rebuild"
            );

            handle
                .submit(ClientCommand::DeleteApp {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(DeleteApp)");
            let events = drain_events(&handle, &listener).await;
            assert!(events
                .iter()
                .any(|event| matches!(event, Ev::AppsChanged { apps } if apps.is_empty())));
        });
        assert!(
            !tmp.path().join("apps").join(&app_id).exists(),
            "DeleteApp must remove the app directory"
        );
    }

    #[test]
    fn local_apps_suggestion_and_cancel_commands_round_trip_through_submit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: "Moodboard".into(),
                    origin: AppCreateOriginDto::Library,
                    brief: "a test app".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            let events = drain_events(&handle, &listener).await;
            let app_id = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged")
                [0]
            .id
            .clone();

            // `OpenAppDesigner` needs `collecting_spec`; stand in for Task
            // 4/8's not-yet-wired questionnaire-authoring LLM round trip.
            let service = handle.local_apps().expect("local-apps service");
            seed_collecting_spec(&handle, &service, &app_id).await;

            // The designer gate opens at revision 0; its interaction id
            // reaches the client only through this event.
            handle
                .submit(ClientCommand::OpenAppDesigner {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(OpenAppDesigner)");
            let events = drain_events(&handle, &listener).await;
            let (designer_interaction, designer_revision) = events
                .iter()
                .find_map(|event| match event {
                    Ev::AppDesignerRequested {
                        app_id: id,
                        interaction_id,
                        revision,
                    } if *id == app_id => Some((interaction_id.clone(), *revision)),
                    _ => None,
                })
                .expect("AppDesignerRequested must be emitted");
            assert_eq!(designer_revision, 0);

            // A user edit AFTER the confirmation request: the gate survives,
            // the draft revision moves past the one the gate was opened at.
            handle
                .submit(ClientCommand::UpdateAppDesignDraft {
                    app_id: app_id.clone(),
                    expected_revision: 0,
                    patch: title_patch("Mood"),
                })
                .await
                .expect("submit(UpdateAppDesignDraft)");
            let events = drain_events(&handle, &listener).await;
            assert!(events
                .iter()
                .any(|event| matches!(event, Ev::AppDesignDraftChanged { revision: 1, .. })));

            // An agent-side suggestion (AppService seam — the phase-3 designer
            // agent drives this) is announced with the id the apply command
            // must echo.
            let suggestion = service
                .store_suggestion(
                    &app_id,
                    local_apps::AppDesignPatch {
                        ops: vec![local_apps::AppDesignPatchOp::Set {
                            field_id: "accent".into(),
                            value: local_apps::DesignValue::Color("#3366ff".into()),
                        }],
                        note: None,
                    },
                )
                .await
                .expect("store_suggestion");
            let events = drain_events(&handle, &listener).await;
            let mut saw_suggestion = false;
            for event in &events {
                if let Ev::AppDesignSuggestionAvailable {
                    app_id: id,
                    suggestion_id,
                    based_on_revision,
                    patch,
                } = event
                {
                    assert_eq!(id, &app_id);
                    assert_eq!(suggestion_id, &suggestion.suggestion_id);
                    assert_eq!(*based_on_revision, 1);
                    assert!(matches!(
                        &patch.ops[..],
                        [AppDesignPatchOpDto::Set {
                            field_id,
                            value: DesignValueDto::Color { value },
                        }] if field_id == "accent" && value == "#3366ff"
                    ));
                    saw_suggestion = true;
                }
            }
            assert!(
                saw_suggestion,
                "AppDesignSuggestionAvailable must be emitted"
            );

            // A guessed suggestion id cannot apply…
            handle
                .submit(ClientCommand::ApplyAgentDesignSuggestion {
                    app_id: app_id.clone(),
                    suggestion_id: "sugg-guessed".into(),
                    expected_revision: 1,
                })
                .await
                .expect("submit(ApplyAgentDesignSuggestion guessed)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppOperationFailed {
                    code: AppErrorCodeDto::InteractionInvalid,
                    ..
                }
            )));
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Ev::AppDesignDraftChanged { .. })),
                "a failed apply must not change the draft"
            );

            // …and a stale expected_revision conflicts (conflict event + typed
            // failure, suggestion left pending).
            handle
                .submit(ClientCommand::ApplyAgentDesignSuggestion {
                    app_id: app_id.clone(),
                    suggestion_id: suggestion.suggestion_id.clone(),
                    expected_revision: 0,
                })
                .await
                .expect("submit(ApplyAgentDesignSuggestion stale)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppDesignConflict {
                    expected_revision: 0,
                    actual_revision: 1,
                    ..
                }
            )));
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppOperationFailed {
                    code: AppErrorCodeDto::RevisionConflict,
                    ..
                }
            )));

            // The exact id + current revision applies the patch (revision 2).
            handle
                .submit(ClientCommand::ApplyAgentDesignSuggestion {
                    app_id: app_id.clone(),
                    suggestion_id: suggestion.suggestion_id.clone(),
                    expected_revision: 1,
                })
                .await
                .expect("submit(ApplyAgentDesignSuggestion)");
            let events = drain_events(&handle, &listener).await;
            let fields = events
                .iter()
                .find_map(|event| match event {
                    Ev::AppDesignDraftChanged {
                        revision: 2,
                        fields,
                        ..
                    } => Some(fields.clone()),
                    _ => None,
                })
                .expect("AppDesignDraftChanged at revision 2");
            assert!(matches!(
                fields.get("accent"),
                Some(DesignValueDto::Color { value }) if value == "#3366ff"
            ));

            // Confirming with the revision the gate was OPENED at — after the
            // post-request edits — must fail with revision_conflict and leave
            // the gate pending (spec §B: confirm requires the CURRENT
            // revision).
            handle
                .submit(ClientCommand::ConfirmAppDesign {
                    app_id: app_id.clone(),
                    revision: designer_revision,
                    interaction_id: designer_interaction.clone(),
                })
                .await
                .expect("submit(ConfirmAppDesign post-edit stale)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppOperationFailed {
                    code: AppErrorCodeDto::RevisionConflict,
                    ..
                }
            )));
            assert_eq!(
                service.record(&app_id).await.unwrap().workflow_state,
                local_apps::AppWorkflowState::AwaitingSpecConfirmation,
                "the failed confirm must not consume the gate or move the workflow"
            );

            // Cancelling through the UI command voids the gate → collecting_spec…
            handle
                .submit(ClientCommand::CancelAppDesign {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(CancelAppDesign)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppWorkflowChanged {
                    state: AppWorkflowStateDto::CollectingSpec,
                    ..
                }
            )));

            // …and the voided gate can never confirm again, even with the
            // current revision echoed correctly.
            handle
                .submit(ClientCommand::ConfirmAppDesign {
                    app_id: app_id.clone(),
                    revision: 2,
                    interaction_id: designer_interaction,
                })
                .await
                .expect("submit(ConfirmAppDesign voided)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppOperationFailed {
                    code: AppErrorCodeDto::WorkflowStateInvalid,
                    ..
                }
            )));
        });
    }

    #[test]
    fn local_apps_revision_requests_round_trip_through_submit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: "Gallery".into(),
                    origin: AppCreateOriginDto::Library,
                    brief: "a test app".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            let events = drain_events(&handle, &listener).await;
            let app_id = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged")
                [0]
            .id
            .clone();
            let service = handle.local_apps().expect("local-apps service");
            seed_collecting_spec(&handle, &service, &app_id).await;

            handle
                .submit(ClientCommand::OpenAppDesigner {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(OpenAppDesigner)");
            let events = drain_events(&handle, &listener).await;
            let designer_interaction = events
                .iter()
                .find_map(|event| match event {
                    Ev::AppDesignerRequested { interaction_id, .. } => Some(interaction_id.clone()),
                    _ => None,
                })
                .expect("AppDesignerRequested must be emitted");
            local_apps::test_support::stamp_fresh_plan(&service, &app_id).await;
            handle
                .submit(ClientCommand::ConfirmAppDesign {
                    app_id: app_id.clone(),
                    revision: 0,
                    interaction_id: designer_interaction,
                })
                .await
                .expect("submit(ConfirmAppDesign)");
            drain_events(&handle, &listener).await;

            // The generation/validation seams (phase 3 drives them) open the
            // preview gate.
            service
                .generation_complete(&app_id)
                .await
                .expect("generation_complete");
            service
                .validation_passed(&app_id)
                .await
                .expect("validation_passed");
            let events = drain_events(&handle, &listener).await;
            let first_preview = events
                .iter()
                .find_map(|event| match event {
                    Ev::AppPreviewReady { interaction_id, .. } => Some(interaction_id.clone()),
                    _ => None,
                })
                .expect("AppPreviewReady must be emitted");

            // From the preview gate, feedback through the UI command voids the
            // gate → revising, with no failure.
            handle
                .submit(ClientCommand::RequestAppRevision {
                    app_id: app_id.clone(),
                    prompt: "use a darker header".into(),
                })
                .await
                .expect("submit(RequestAppRevision)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppWorkflowChanged {
                    state: AppWorkflowStateDto::Revising,
                    ..
                }
            )));
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Ev::AppOperationFailed { .. })),
                "a legal revision request must not fail"
            );

            // The voided preview gate can no longer confirm.
            handle
                .submit(ClientCommand::ConfirmAppPreview {
                    app_id: app_id.clone(),
                    revision: 0,
                    interaction_id: first_preview.clone(),
                })
                .await
                .expect("submit(ConfirmAppPreview voided)");
            let events = drain_events(&handle, &listener).await;
            assert!(events.iter().any(|event| matches!(
                event,
                Ev::AppOperationFailed {
                    code: AppErrorCodeDto::WorkflowStateInvalid,
                    ..
                }
            )));

            // The persistent coordinator owns the revision pass now. By the
            // time the UI event drain completes it may already have advanced
            // from revising to validation (or a retryable validation failure
            // in this off-device test, where no Node runtime is installed).
            // The coordinator's own tests cover minting the fresh preview gate.
            assert!(matches!(
                service.record(&app_id).await.unwrap().workflow_state,
                local_apps::AppWorkflowState::Revising
                    | local_apps::AppWorkflowState::Validating
                    | local_apps::AppWorkflowState::ValidationFailed
            ));
        });
    }

    /// W3 regression listener: reacts to `AppDesignerRequested` by driving
    /// ANOTHER submit on the same engine handle from inside `on_event`,
    /// hopping through the runtime the way a real Swift/Kotlin callback
    /// would (a separate task the callback then awaits). Pre-W3 the observer
    /// awaited the sink INSIDE the service's emission-order guard, so the
    /// inner submit's emission-order acquisition waited (cross-task, so the
    /// core's task-local reentrancy panic could not see it) on the very
    /// guard whose release waited on this callback — a silent deadlock of
    /// the whole app surface.
    #[derive(Default)]
    struct ResubmittingListener {
        received: tokio::sync::Mutex<Vec<Ev>>,
        engine: StdMutex<Option<Arc<MobileEngineHandle>>>,
        resubmitted: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl ClientEventListener for ResubmittingListener {
        async fn on_event(&self, event: Ev) {
            if let Ev::AppDesignerRequested { app_id, .. } = &event {
                if !self
                    .resubmitted
                    .swap(true, std::sync::atomic::Ordering::SeqCst)
                {
                    let engine = self
                        .engine
                        .lock()
                        .unwrap()
                        .clone()
                        .expect("engine registered before the designer opens");
                    let app_id = app_id.clone();
                    let inner = tokio::spawn(async move {
                        engine
                            .submit(ClientCommand::UpdateAppDesignDraft {
                                app_id,
                                expected_revision: 0,
                                patch: title_patch("From listener"),
                            })
                            .await
                    });
                    inner
                        .await
                        .expect("inner submit task")
                        .expect("inner submit");
                }
            }
            self.received.lock().await.push(event);
        }
    }

    /// `drain_events` twin for [`ResubmittingListener`] (same two-stage
    /// barrier: commit → enqueue, then enqueue → deliver).
    async fn drain_resubmitting(
        handle: &MobileEngineHandle,
        listener: &ResubmittingListener,
    ) -> Vec<Ev> {
        if let Ok(service) = handle.local_apps() {
            service.flush_events().await;
        }
        handle.app_emissions.flush().await;
        std::mem::take(&mut *listener.received.lock().await)
    }

    #[test]
    fn local_apps_listener_driving_a_submit_from_on_event_cannot_deadlock() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let platform: Arc<dyn traits::Platform> =
            Arc::new(HostFakePlatform::new(tmp.path().to_path_buf()));
        let listener = Arc::new(ResubmittingListener::default());
        let listener_dyn: Arc<dyn ClientEventListener> = listener.clone();
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let handle =
            build_mobile_engine(test_config(tmp.path()), platform, listener_dyn, perm_sink)
                .expect("build_mobile_engine failed");
        // See `build_submit_handle`'s doc: a deterministic, no-network local-
        // apps model so `seed_collecting_spec` below never races (or hangs
        // on) a real `api.anthropic.com` request.
        handle.set_local_apps_model(ScriptedModel::new(Vec::new()));
        *listener.engine.lock().unwrap() = Some(handle.clone());

        handle.runtime().block_on(async {
            // Bounded so a reintroduced lock-across-listener-code regression
            // FAILS fast instead of hanging CI.
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                handle
                    .submit(ClientCommand::CreateApp {
                        name: "Reentrant".into(),
                        origin: AppCreateOriginDto::Library,
                        brief: "a test app".into(),
                        conversation_id: None,
                    })
                    .await
                    .expect("submit(CreateApp)");
                let events = drain_resubmitting(&handle, &listener).await;
                let app_id = apps_changed_rows(&events)
                    .expect("CreateApp must announce AppsChanged")[0]
                    .id
                    .clone();
                let service = handle.local_apps().expect("local-apps service");
                seed_collecting_spec(&handle, &service, &app_id).await;

                // Delivering AppDesignerRequested makes the listener drive
                // the draft edit from inside `on_event`; both the outer and
                // the inner command must complete.
                handle
                    .submit(ClientCommand::OpenAppDesigner {
                        app_id: app_id.clone(),
                    })
                    .await
                    .expect("submit(OpenAppDesigner)");
                let mut events = drain_resubmitting(&handle, &listener).await;
                // The inner submit finished inside the designer event's
                // delivery, so its own events land behind the first flush
                // barrier; a second barrier round collects them.
                events.extend(drain_resubmitting(&handle, &listener).await);
                assert!(
                    listener
                        .resubmitted
                        .load(std::sync::atomic::Ordering::SeqCst),
                    "the listener never saw AppDesignerRequested"
                );
                assert!(
                    events.iter().any(|event| matches!(
                        event,
                        Ev::AppDesignDraftChanged { revision: 1, .. }
                    )),
                    "the listener-driven draft edit must complete and deliver (got {events:?})"
                );
                let service = handle.local_apps().expect("local-apps service");
                assert_eq!(
                    service.draft(&app_id).await.expect("draft").revision,
                    1,
                    "the listener-driven edit must have committed"
                );
            })
            .await
            .expect(
                "app surface deadlocked: a listener driving a submit from on_event \
                 must complete now that no lock is held while listener code runs",
            );
        });
        // Break the listener ↔ engine strong-reference cycle so the engine
        // (and its runtime) can drop with the test.
        *listener.engine.lock().unwrap() = None;
    }

    /// W2 regression, mirroring the core's dropped-caller completion-task
    /// tests: abort the mutating `submit` at a sweep of yield offsets. Any
    /// mutation that actually COMMITTED (probe: the draft revision bump)
    /// must still deliver its `DesignDraftChanged` FOLLOWED by an
    /// `AppsChanged` snapshot — the announce lives in a detached task, not
    /// in the droppable caller future (contract C8).
    #[test]
    fn local_apps_dropped_submit_caller_never_loses_a_committed_mutations_snapshot() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: "Sweep".into(),
                    origin: AppCreateOriginDto::Library,
                    brief: "a test app".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            let events = drain_events(&handle, &listener).await;
            let app_id = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged")
                [0]
            .id
            .clone();
            let service = handle.local_apps().expect("local-apps service");
            seed_collecting_spec(&handle, &service, &app_id).await;
            handle
                .submit(ClientCommand::OpenAppDesigner {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(OpenAppDesigner)");
            drain_events(&handle, &listener).await;

            /// Highest yield offset in the sweep; this one is the deterministic
            /// anchor (see below) rather than another timing probe.
            const LAST_SWEEP_YIELD: u32 = 15;

            let mut committed_iterations = 0usize;
            for yields in 0..=LAST_SWEEP_YIELD {
                let revision = service.draft(&app_id).await.expect("draft").revision;
                let submit_handle = Arc::clone(&handle);
                let submit_app_id = app_id.clone();
                let submit = tokio::spawn(async move {
                    submit_handle
                        .submit(ClientCommand::UpdateAppDesignDraft {
                            app_id: submit_app_id,
                            expected_revision: revision,
                            patch: title_patch(&format!("v{revision}")),
                        })
                        .await
                });
                if yields == LAST_SWEEP_YIELD {
                    // Determinism anchor. A fixed yield budget is only a PROXY
                    // for "the spawned handler got far enough to commit"; under
                    // load the runtime can starve that task for all 16 offsets,
                    // and then the sweep reports "the probe is broken" when in
                    // fact nothing ever ran. The final offset therefore waits
                    // for the commit and THEN drops the caller — still exactly
                    // the drop-after-commit case this test exists to pin, but
                    // guaranteed to occur at least once on any machine.
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                    while service.draft(&app_id).await.expect("draft").revision == revision
                        && std::time::Instant::now() < deadline
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                } else {
                    for _ in 0..yields {
                        tokio::task::yield_now().await;
                    }
                }
                submit.abort();
                let _ = submit.await;

                // Classify by the committed-state probe: the detached
                // mutation (if the handler reached its spawn) runs to
                // completion regardless of the abort, so poll briefly.
                let mut events: Vec<Ev> = Vec::new();
                let settle = std::time::Instant::now() + std::time::Duration::from_secs(2);
                let committed = loop {
                    events.extend(drain_events(&handle, &listener).await);
                    if service.draft(&app_id).await.expect("draft").revision == revision + 1 {
                        break true;
                    }
                    if std::time::Instant::now() > settle {
                        break false;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                };
                if !committed {
                    continue;
                }
                committed_iterations += 1;
                // The committed edit's domain event (which rides the core's
                // own completion task) must be FOLLOWED by the announce-side
                // AppsChanged — exactly the event a droppable caller future
                // used to lose. Bounded poll: a regression fails loudly.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    let draft_at = events.iter().position(|event| {
                        matches!(
                            event,
                            Ev::AppDesignDraftChanged { revision: r, .. } if *r == revision + 1
                        )
                    });
                    if draft_at.is_some_and(|at| {
                        events[at..]
                            .iter()
                            .any(|event| matches!(event, Ev::AppsChanged { .. }))
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "committed draft revision {} lost its AppsChanged snapshot \
                         (yields={yields}; got {events:?})",
                        revision + 1
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    events.extend(drain_events(&handle, &listener).await);
                }
            }
            assert!(
                committed_iterations > 0,
                "the abort sweep never committed a mutation; the probe (or the \
                 sweep width) is broken"
            );
        });
    }

    /// W4: the generation-progress and runtime seams (the phase-3 generator /
    /// phase-4 runtime drive them) must reach the client through the same
    /// lowered wire path as command replies — field-exact.
    #[test]
    fn local_apps_progress_and_runtime_seams_deliver_field_exact_events() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: "Telemetry".into(),
                    origin: AppCreateOriginDto::Library,
                    brief: "a test app".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            let events = drain_events(&handle, &listener).await;
            let app_id = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged")
                [0]
            .id
            .clone();
            let service = handle.local_apps().expect("local-apps service");

            service
                .report_generation_progress(local_apps::AppGenerationProgress {
                    app_id: app_id.clone(),
                    stage: "pages".into(),
                    percent: Some(42),
                    detail: Some("3/7 screens".into()),
                })
                .await
                .expect("report_generation_progress");
            let events = drain_events(&handle, &listener).await;
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    Ev::AppGenerationProgress {
                        app_id: id,
                        stage,
                        percent: Some(42),
                        detail: Some(detail),
                    } if *id == app_id && stage == "pages" && detail == "3/7 screens"
                )),
                "AppGenerationProgress must arrive field-exact (got {events:?})"
            );

            // Runtime record: stopped -> starting (pins the port)…
            service
                .update_runtime_record(
                    &app_id,
                    local_apps::AppRuntimeState::Starting,
                    Some(3001),
                    Some(77),
                    None,
                )
                .await
                .expect("update_runtime_record(starting)");
            // …then starting -> failed, carrying the failure detail.
            service
                .update_runtime_record(
                    &app_id,
                    local_apps::AppRuntimeState::Failed,
                    None,
                    None,
                    Some("dev server exited: code 1".into()),
                )
                .await
                .expect("update_runtime_record(failed)");
            let events = drain_events(&handle, &listener).await;
            let starting_at = position_of(&events, "AppRuntimeChanged(starting)", |event| {
                matches!(
                    event,
                    Ev::AppRuntimeChanged {
                        app_id: id,
                        state: AppRuntimeStateDto::Starting,
                        last_error: None,
                        ..
                    } if *id == app_id
                )
            });
            let failed_at = position_of(&events, "AppRuntimeChanged(failed)", |event| {
                matches!(
                    event,
                    Ev::AppRuntimeChanged {
                        app_id: id,
                        state: AppRuntimeStateDto::Failed,
                        last_error: Some(last_error),
                        ..
                    } if *id == app_id && last_error == "dev server exited: code 1"
                )
            });
            assert!(
                starting_at < failed_at,
                "runtime transitions must arrive in commit order (got {events:?})"
            );
        });
    }

    /// W5: failure-vs-cause order on the suggestion conflict path. The
    /// suggestion announcement is consumed EARLIER; the stale apply's batch
    /// must then deliver `AppDesignConflict` (the spec-required cause)
    /// BEFORE the synthesized `AppOperationFailed { revision_conflict }`.
    #[test]
    fn local_apps_stale_suggestion_apply_orders_conflict_before_failure() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: "Conflicted".into(),
                    origin: AppCreateOriginDto::Library,
                    brief: "a test app".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            let events = drain_events(&handle, &listener).await;
            let app_id = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged")
                [0]
            .id
            .clone();
            let service = handle.local_apps().expect("local-apps service");
            seed_collecting_spec(&handle, &service, &app_id).await;
            handle
                .submit(ClientCommand::OpenAppDesigner {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(OpenAppDesigner)");
            handle
                .submit(ClientCommand::UpdateAppDesignDraft {
                    app_id: app_id.clone(),
                    expected_revision: 0,
                    patch: title_patch("Base"),
                })
                .await
                .expect("submit(UpdateAppDesignDraft)");
            let suggestion = service
                .store_suggestion(
                    &app_id,
                    local_apps::AppDesignPatch {
                        ops: vec![local_apps::AppDesignPatchOp::Set {
                            field_id: "accent".into(),
                            value: local_apps::DesignValue::Color("#3366ff".into()),
                        }],
                        note: None,
                    },
                )
                .await
                .expect("store_suggestion");
            // Consume everything so far — the suggestion announcement rode an
            // earlier batch by design.
            let events = drain_events(&handle, &listener).await;
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, Ev::AppDesignSuggestionAvailable { .. })),
                "setup must have delivered the suggestion announcement"
            );

            // The stale apply: cause first, failure second — deterministic
            // now that both ride the single ordered emission channel.
            handle
                .submit(ClientCommand::ApplyAgentDesignSuggestion {
                    app_id: app_id.clone(),
                    suggestion_id: suggestion.suggestion_id.clone(),
                    expected_revision: 0,
                })
                .await
                .expect("submit(ApplyAgentDesignSuggestion stale)");
            let events = drain_events(&handle, &listener).await;
            let conflict_at = position_of(&events, "AppDesignConflict", |event| {
                matches!(
                    event,
                    Ev::AppDesignConflict {
                        expected_revision: 0,
                        actual_revision: 1,
                        ..
                    }
                )
            });
            let failed_at =
                position_of(&events, "AppOperationFailed(revision_conflict)", |event| {
                    matches!(
                        event,
                        Ev::AppOperationFailed {
                            code: AppErrorCodeDto::RevisionConflict,
                            ..
                        }
                    )
                });
            assert!(
                conflict_at < failed_at,
                "AppDesignConflict (the cause) must precede AppOperationFailed \
                 (got {events:?})"
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Ev::AppDesignDraftChanged { .. })),
                "a stale apply must not change the draft"
            );
        });
    }

    /// W5: cross-batch FIFO. Two rapid mutations with no intermediate flush
    /// must deliver the full interleaved sequence in commit order — each
    /// batch's domain event, then ITS trailing snapshot, then the next
    /// batch. Asserted as the EXACT sequence, so any reordering (or a
    /// snapshot jumping its batch) fails.
    #[test]
    fn local_apps_two_rapid_mutations_deliver_the_exact_commit_ordered_sequence() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (handle, listener) = build_submit_handle(tmp.path());

        handle.runtime().block_on(async {
            handle
                .submit(ClientCommand::CreateApp {
                    name: "Fifo".into(),
                    origin: AppCreateOriginDto::Library,
                    brief: "a test app".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp)");
            let events = drain_events(&handle, &listener).await;
            let app_id = apps_changed_rows(&events).expect("CreateApp must announce AppsChanged")
                [0]
            .id
            .clone();
            let service = handle.local_apps().expect("local-apps service");
            seed_collecting_spec(&handle, &service, &app_id).await;
            handle
                .submit(ClientCommand::OpenAppDesigner {
                    app_id: app_id.clone(),
                })
                .await
                .expect("submit(OpenAppDesigner)");
            drain_events(&handle, &listener).await;

            // Two mutations, no flush in between.
            handle
                .submit(ClientCommand::UpdateAppDesignDraft {
                    app_id: app_id.clone(),
                    expected_revision: 0,
                    patch: title_patch("One"),
                })
                .await
                .expect("submit(UpdateAppDesignDraft #1)");
            handle
                .submit(ClientCommand::UpdateAppDesignDraft {
                    app_id: app_id.clone(),
                    expected_revision: 1,
                    patch: title_patch("Two"),
                })
                .await
                .expect("submit(UpdateAppDesignDraft #2)");

            let events = drain_events(&handle, &listener).await;
            let sequence: Vec<String> = events
                .iter()
                .map(|event| match event {
                    Ev::AppDesignDraftChanged { revision, .. } => format!("draft@{revision}"),
                    Ev::AppsChanged { .. } => "apps".into(),
                    other => format!("unexpected({other:?})"),
                })
                .collect();
            assert_eq!(
                sequence,
                vec!["draft@1", "apps", "draft@2", "apps"],
                "the interleaved two-command sequence must be exactly commit-ordered"
            );
        });
    }

    #[test]
    fn local_apps_startup_sweep_consumes_continuations_queued_by_a_previous_run() {
        let tmp = tempfile::tempdir().expect("tempdir");

        // Seed the on-disk store (at the same `<root>/apps` the engine roots
        // its service at) with a gate outcome whose delivery FAILED — the
        // state a crash mid-delivery leaves behind.
        let seeded_app_id = {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("seed runtime");
            rt.block_on(async {
                let sink = Arc::new(local_apps::RecordingContinuationSink::new());
                sink.set_fail(true);
                let service = local_apps::AppService::load(
                    tmp.path(),
                    Arc::new(local_apps::test_support::FixedClock::new(1_753_900_000_000)),
                    Arc::clone(&sink) as Arc<dyn local_apps::ContinuationSink>,
                    Arc::new(local_apps::NoopAppEventObserver),
                )
                .await
                .expect("seed service");
                let record = service
                    .create_app(Some("Queued"), "a test app", None)
                    .await
                    .expect("create app");
                local_apps::test_support::advance_to_collecting_spec(&service, &record.id).await;
                let gate = service
                    .open_designer(&record.id)
                    .await
                    .expect("open designer");
                local_apps::test_support::stamp_fresh_plan(&service, &record.id).await;
                service
                    .confirm_design(&record.id, &gate.interaction_id, 0)
                    .await
                    .expect("confirm design");
                let queued = service
                    .interactions(&record.id)
                    .await
                    .expect("interactions");
                assert_eq!(
                    queued.undelivered.len(),
                    1,
                    "failed delivery must stay queued on disk"
                );
                assert_eq!(queued.last_delivered_seq, 0);
                record.id
            })
        };

        // A fresh engine over the same root spawns the startup redelivery
        // sweep (spec §E at-least-once); with the phase-1 Noop sink the queued
        // continuation is drained and marked delivered — never dropped with a
        // stale `last_delivered_seq`.
        let (handle, _listener) = build_submit_handle(tmp.path());
        handle.runtime().block_on(async {
            let service = handle.local_apps().expect("local-apps service");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let interactions = service
                    .interactions(&seeded_app_id)
                    .await
                    .expect("interactions");
                if interactions.undelivered.is_empty() {
                    assert_eq!(
                        interactions.last_delivered_seq, 1,
                        "the sweep must mark the continuation delivered"
                    );
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "startup sweep did not consume the queued continuation in time"
                );
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        });
    }

    /// A corrupt on-disk apps store must DEGRADE, never brick: the engine
    /// still builds (chat is unaffected), `local_apps()` returns the boot
    /// error, and every app command surfaces it as a typed
    /// `AppOperationFailed { code: storage_corrupt }` — the Err branch of the
    /// handle's `local_apps` Result and `local_apps_or_report`'s emit path.
    #[test]
    fn local_apps_corrupt_store_degrades_but_never_bricks_the_engine() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("apps")).expect("apps dir");
        std::fs::write(tmp.path().join("apps/index.json"), "{ not json")
            .expect("plant corrupt index");

        // The engine still builds over the corrupt store…
        let (handle, listener) = build_submit_handle(tmp.path());
        handle.runtime().block_on(async {
            // …the boot-time load error is held on the handle…
            let err = match handle.local_apps() {
                Err(error) => error,
                Ok(_) => panic!("a corrupt store must surface as the boot error"),
            };
            assert_eq!(err.code(), local_apps::AppErrorCode::StorageCorrupt);

            // …and every app command reports it typed instead of hanging or
            // pretending an empty store.
            handle
                .submit(ClientCommand::ListApps)
                .await
                .expect("submit(ListApps) must not be a transport error");
            handle
                .submit(ClientCommand::CreateApp {
                    name: "Habit Tracker".into(),
                    origin: AppCreateOriginDto::Library,
                    brief: "a test app".into(),
                    conversation_id: None,
                })
                .await
                .expect("submit(CreateApp) must not be a transport error");
            let events = drain_events(&handle, &listener).await;
            let failures = events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        Ev::AppOperationFailed {
                            code: AppErrorCodeDto::StorageCorrupt,
                            ..
                        }
                    )
                })
                .count();
            assert_eq!(
                failures, 2,
                "each app command must emit the typed boot failure: {events:?}"
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Ev::AppsChanged { .. })),
                "a corrupt store must never masquerade as an (empty) app list"
            );
        });
    }
}

#[cfg(test)]
mod mobile_provider_allowlist_tests {
    use std::collections::BTreeMap;

    use serde_json::{json, Value};

    use super::{anthropic_models, apply_mobile_profile_allowlist};

    fn assembled(routing: Option<Value>) -> provider_config::Assembled {
        let user_providers = BTreeMap::from([
            (
                "alpha".to_string(),
                json!({
                    "type": "openai",
                    "baseUrl": "https://alpha.example/v1",
                    "apiKeyEnv": "ALPHA_API_KEY",
                    "models": [{"id": "model-a"}]
                }),
            ),
            (
                "beta".to_string(),
                json!({
                    "type": "openai",
                    "baseUrl": "https://beta.example/v1",
                    "apiKeyEnv": "BETA_API_KEY",
                    "models": [{"id": "model-b"}]
                }),
            ),
        ]);
        provider_config::assemble(provider_config::AssembleInputs {
            anthropic_api_base: "https://api.anthropic.com".to_string(),
            anthropic_models: anthropic_models("claude-sonnet-4-20250514"),
            anthropic_has_api_key: false,
            anthropic_has_oauth: false,
            user_providers,
            routing,
        })
    }

    #[test]
    fn absent_mobile_allowlist_preserves_full_catalog() {
        let mut assembled = assembled(None);
        let provider_count = assembled.client_config.providers.len();
        let credential_count = assembled.credential_sources.len();

        apply_mobile_profile_allowlist(&mut assembled, None);

        assert_eq!(provider_count, assembled.client_config.providers.len());
        assert_eq!(credential_count, assembled.credential_sources.len());
    }

    #[test]
    fn explicit_empty_mobile_allowlist_filters_every_profile_and_route() {
        let routing = json!({
            "mobileEnabledProfiles": [],
            "fallback": {
                "primary": ["alpha/model-a", "beta/model-b"]
            }
        });
        let mut assembled = assembled(Some(routing.clone()));
        assert!(!assembled.client_config.providers.is_empty());
        assert!(!assembled.chains.chains.is_empty());

        apply_mobile_profile_allowlist(&mut assembled, Some(&routing));

        assert!(assembled.client_config.providers.is_empty());
        assert!(assembled.credential_sources.is_empty());
        assert!(assembled.chains.aliases.is_empty());
        assert!(assembled.chains.chains.is_empty());
    }

    #[test]
    fn mobile_allowlist_filters_providers_credentials_aliases_and_fallbacks() {
        let routing = json!({
            "mobileEnabledProfiles": ["alpha"],
            "aliases": {
                "allowed": "alpha/model-a",
                "blocked": "beta/model-b"
            },
            "fallback": {
                "mixed": ["alpha/model-a", "beta/model-b"],
                "blocked": ["beta/model-b"]
            }
        });
        let mut assembled = assembled(Some(routing.clone()));

        apply_mobile_profile_allowlist(&mut assembled, Some(&routing));

        let profiles: Vec<_> = assembled
            .client_config
            .providers
            .iter()
            .map(|provider| provider.profile_name.as_str())
            .collect();
        assert_eq!(vec!["alpha"], profiles);
        assert_eq!(1, assembled.credential_sources.len());
        assert_eq!("alpha", assembled.credential_sources[0].profile_name);
        assert_eq!(
            Some(&"alpha/model-a".to_string()),
            assembled.chains.aliases.get("allowed")
        );
        assert!(!assembled.chains.aliases.contains_key("blocked"));
        assert_eq!(1, assembled.chains.chains["mixed"].len());
        assert!(!assembled.chains.chains.contains_key("blocked"));
    }
}
