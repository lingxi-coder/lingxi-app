//! `BuiltinToolContext` — the construction-time handle every builtin tool
//! takes in its `new()` constructor.
//!
//! Moved here from `tools/src/builtin/mod.rs` in M8-P5 so the per-category
//! tool crates (`tool-file`, `tool-shell`, …) can construct their tools
//! without depending on the `tools` monolith. The design §5.1 `ToolCtx`
//! reshape (individual `Arc<dyn Platform>` handles, no god-object) will
//! eventually slim this surface.

use api_client::AnthropicProvider;
use permission::PermissionMode;
use traits::permission_gate::PermissionGate;
use sandbox::decision::ProjectTrustLevel;
use sandbox::runtime_config::{Platform, SandboxRuntimeConfig};
use std::path::PathBuf;
use std::sync::Arc;
use telemetry::AnalyticsBus;
use traits::budget::BudgetEnforcerHandle;
use traits::camera::CameraControl;
use traits::clock::Clock;
use traits::computer_control::ComputerControl;
use traits::filesystem::FileSystem;
use traits::http::HttpTransport;
use traits::mailbox::MailboxRouterHandle;
use traits::notification::NotificationService;
use traits::process::ProcessRunner;
use traits::sandbox::Sandbox;
use traits::share::SharingService;
use traits::stt::SpeechToText;
use traits::tts::TextToSpeech;
use traits::subagent_spawn::SubagentSpawner;
use traits::task_registry::TaskRegistryHandle;
use traits::voice::VoiceRecorder;
use traits::worktree::WorktreeManager;

/// Static surface every builtin tool needs at construction time.
///
/// Cloning is cheap — every field is `Arc` or a small owned vec.
#[derive(Clone)]
pub struct BuiltinToolContext {
    /// Sandboxed FS access (M1).
    pub fs: Arc<dyn FileSystem>,
    /// Telemetry bus (M3-06).
    pub bus: Arc<AnalyticsBus>,
    /// Canonicalised root directories the agent is allowed to read/write.
    pub trusted_dirs: Vec<PathBuf>,
    /// Process runner — backs BashTool/PowerShellTool/REPLTool (M4-02).
    pub process: Arc<dyn ProcessRunner>,
    /// Sandbox seam — provides the `prepare`/`bypass_with_audit` constructors
    /// that turn `ProcessCommand` into `SandboxedCommand` (M4-02).
    pub sandbox: Arc<dyn Sandbox>,
    /// Wall-clock — backs SleepTool + duration measurement (M4-02).
    pub clock: Arc<dyn Clock>,
    /// Sandbox policy runtime config — drives `wrap_with_sandbox` (M4-02).
    pub sandbox_runtime: SandboxRuntimeConfig,
    /// Active permission mode (M4-02).
    pub permission_mode: PermissionMode,
    /// Whether the project workspace has been explicitly trusted (M4-02).
    pub project_trust: ProjectTrustLevel,
    /// Whether the host has a working sandbox backend right now (M4-02).
    pub sandbox_available: bool,
    /// Project workspace path (M4-02).
    pub workspace: PathBuf,
    /// Detected platform — drives `wrap_with_sandbox` branch (M4-02).
    pub platform: Platform,
    /// HTTP transport for web tools (WebFetch + WebSearch) (M4-03). M1 trait;
    /// tests inject `MockHttpTransport`.
    pub http: Arc<dyn HttpTransport>,
    /// Anthropic provider for assembling `POST /v1/messages` requests (M3-03).
    /// `WebSearchTool` uses it to build the HTTP request, then attaches a tool
    /// block + custom `anthropic-beta` header.
    pub provider: Arc<AnthropicProvider>,
    /// Model used by `WebSearch` when calling `POST /v1/messages` (M4-03).
    /// Sourced from the session's `coordinator_model` at registration time.
    pub default_model: String,
    /// Worktree manager (M2-01 trait) — backs `EnterWorktree` + `ExitWorktree`
    /// (M4-04). Tests inject `MockWorktreeManager`; production uses
    /// `platform_posix::PosixWorktreeManager`.
    pub worktree: Arc<dyn WorktreeManager>,

    // ===== M4-05 wiring (Phase 3) =====
    /// Subagent spawner — `AgentTool` dispatches recursive subagent runs
    /// through this seam. `None` when the host has not wired a state-
    /// machine pool yet; in that case `AgentTool::call` surfaces a clear
    /// internal error. Production wires `agent::PoolSubagentSpawner`.
    pub subagent_spawner: Option<Arc<dyn SubagentSpawner>>,
    /// Task registry — the 6 `Task*` tools dispatch CRUD through this seam.
    /// Production wires `tasks::TaskRegistry`.
    pub task_registry: Option<Arc<dyn TaskRegistryHandle>>,
    /// Mailbox router — `SendMessageTool` dispatches teammate routing
    /// through this seam. Production wires `coordinator::MailboxRouter`.
    pub mailbox_router: Option<Arc<dyn MailboxRouterHandle>>,
    /// Budget enforcer — `AgentTool` gates spawn calls through this seam.
    /// Production wires `cost::BudgetEnforcer`.
    pub budget_enforcer: Option<Arc<dyn BudgetEnforcerHandle>>,
    /// Permission gate (enforcement 3b) — `AgentTool` threads this into the
    /// `RegistryToolInvoker` it hands the spawner, so a spawned subagent's tool
    /// calls are gated by the SAME policy as the main loop (closing the bypass
    /// where the inherited invoker dispatched any tool unconditionally). `None`
    /// = no enforcement wired (the default; legacy always-dispatch behavior).
    /// Production wires the boot gate (`PolicyPermissionGate` when
    /// `LINGXI_ENFORCE_PERMISSIONS` is set, else the no-op gate).
    pub permission_gate: Option<Arc<dyn PermissionGate>>,

    // ===== M4-07 wiring (Phase 7) =====
    /// MCP registry — the 4 MCP builtin tools (`MCPTool`, `McpAuthTool`,
    /// `ListMcpResourcesTool`, `ReadMcpResourceTool`) dispatch through this
    /// seam. `None` when the host has not wired an MCP layer; tools surface
    /// a "MCP registry not configured" error in that case. Production wires
    /// `mcp::McpRegistry` populated by the platform.
    pub mcp_registry: Option<Arc<::mcp::registry::McpRegistry>>,
    /// LSP registry — `LSPTool` dispatches through this seam. `None` when
    /// the host has not wired an LSP layer. Production wires
    /// `lsp::registry::LspRegistry` populated by plugin registration.
    pub lsp_registry: Option<Arc<::lsp::registry::LspRegistry>>,

    // ===== M8-P11 mobile / device-control capabilities =====
    /// Native camera — `tool-camera`'s `CameraTool` routes here. `None` on
    /// desktop; mobile composition roots wire `platform.camera()` (a Swift /
    /// Kotlin impl via UniFFI).
    pub camera: Option<Arc<dyn CameraControl>>,
    /// Native microphone recorder — `tool-voice`'s `VoiceTool` routes here.
    /// `None` on desktop.
    pub voice: Option<Arc<dyn VoiceRecorder>>,
    /// Native speech-to-text — `tool-speech`'s `SpeechTool` (`transcribe`)
    /// routes here. `None` on desktop; mobile wires `platform.stt()`.
    pub stt: Option<Arc<dyn SpeechToText>>,
    /// Native text-to-speech — `tool-speech`'s `SpeechTool` (`speak`) routes
    /// here. `None` on desktop; mobile wires `platform.tts()`.
    pub tts: Option<Arc<dyn TextToSpeech>>,
    /// Native share sheet — `tool-share`'s `ShareTool` routes here. `None` on
    /// desktop.
    pub share: Option<Arc<dyn SharingService>>,
    /// Native system notifications — `tool-notification`'s `NotificationTool`
    /// routes here. `None` on desktop; mobile wires `platform.notifications()`.
    pub notifications: Option<Arc<dyn NotificationService>>,
    /// Screen-capture + input automation — the device-control tools
    /// (`computer`/`android_use`/`ios_use`) route here. `None` unless a
    /// desktop automation backend or a mobile UniFFI impl is wired.
    pub computer_control: Option<Arc<dyn ComputerControl>>,
}
