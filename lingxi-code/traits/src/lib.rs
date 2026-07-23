//! Platform abstraction traits.
//!
//! Engine crates depend on these traits; platform crates (`platforms/posix`,
//! `platforms/windows`, etc.) implement them. The engine never imports a
//! concrete runtime or OS API directly.
//!
//! See spec §4 (Trait System) and D17 (Runtime boundary).

// Platform filesystem primitives are safe by default. The Windows rooted-file
// implementation is the sole exception: the standard library has no
// handle-relative open/rename API, so that module wraps a small audited set of
// `ntdll` calls. Keeping this at `deny` lets the exception remain item-scoped.
#![deny(unsafe_code)]

/// The claude-code version LingXi replicates byte-for-byte (the parity target),
/// distinct from this workspace's own `CARGO_PKG_VERSION`. claude-code embeds its
/// `VERSION` in outward-facing identifiers — the `AI_AGENT` child-env value
/// (`claude-code_2-1-217_agent`) and the WebFetch `User-Agent`
/// (`claude-code/2.1.217`). LingXi is a 1:1 copy, so it presents the same string.
/// Single source of truth (R-V1) so the AI_AGENT and User-Agent stamps never drift.
pub const CLAUDE_CODE_VERSION: &str = "2.1.217";

pub mod agent_name_registry;
pub mod agent_view;
pub mod auth;
pub mod bg_session_forker;
pub mod bridge;
pub mod budget;
pub mod camera;
pub mod clipboard;
pub mod clock;
pub mod commands;
pub mod computer_control;
pub mod coordinator_mode;
pub mod effect_handler;
pub mod env;
pub mod file_history_sink;
pub mod filesystem;
pub mod fork_subagent;
pub mod http;
pub mod lsp;
pub mod mailbox;
pub mod mcp;
pub mod notification;
pub mod orchestrator;
pub mod permission_gate;
pub mod platform;
pub mod process;
pub mod prompting_gate;
#[cfg_attr(windows, allow(unsafe_code))]
pub mod rooted_fs;
pub mod runtime;
pub mod sandbox;
pub mod secure_storage;
pub mod session_flags;
pub mod share;
pub mod skill_loader;
pub mod stt;
pub mod subagent_output_guard;
pub mod subagent_spawn;
pub mod subscription;
pub mod swarm;
pub mod task_registry;
pub mod team_registry;
pub mod team_spawn;
pub mod tool_invoker;
pub mod traffic_mode;
pub mod tts;
pub mod voice;
pub mod web_search;
pub mod worktree;

pub use auth::{AuthError, AuthHandle, LoginInfo};
pub use bridge::{BridgeConfig, BridgeConnection, BridgeError, BridgeTransport};
pub use budget::{BudgetEnforcerHandle, BudgetError};
pub use camera::{CameraControl, CameraError, CameraPosition, CapturePhotoOpts, CapturedImage};
pub use clipboard::{Clipboard, ClipboardError};
pub use clock::Clock;
pub use commands::{SlashCommandDispatcher, SlashDispatchResult};
pub use computer_control::{ComputerControl, ComputerError, Screenshot};
pub use effect_handler::EffectHandler;
pub use file_history_sink::FileHistorySink;
pub use filesystem::{FileContent, FileEvent, FileEventKind, FileSystem, FlockGuard, FsError};
pub use http::{
    HttpError, HttpTransport, RawByteStreamWithMeta, WebSocketConnection,
    WebSocketConnectionWithMeta, WebSocketMessageStream, WebSocketMessageStreamWithMeta,
};
pub use lsp::{
    LspError, LspRawConnection, LspServerCapabilities, LspServerConfig, LspTransport,
    NewDiagnosticsSource,
};
pub use mailbox::{
    MailboxError as RouterMailboxError, MailboxMessage, MailboxRouterHandle, RouteAck,
};
pub use mcp::*;
pub use notification::{NotificationError, NotificationRequest, NotificationService};
pub use orchestrator::{
    curated_model_names, is_curated_model, parse_model_ref, provider_default_model,
    provider_fallback_order, provider_has_curated_list, ActiveGoalSnapshot, AgentInfo, CheckStatus,
    CompactionSummary, ContextPressureBanner, ContextPressureLevel, CostSnapshot, DoctorCheck,
    DoctorReport, DoctorSummary, ForkOutcome, HandleError, HookInfo, McpActionState, McpServerInfo,
    McpStatus, MemoryEditorOutcome, ModelListing, ModelUsageRow, OrchestratorHandle, OutputEvent,
    OutputStream, PlanSnapshot, RateLimitSnapshot, RecapOutcome, ResumeRuntimeSnapshot,
    RewindRowData, StatusSnapshot, TurnOutcome,
};
pub use permission_gate::{PermissionDecision, PermissionGate};
pub use platform::Platform;
pub use process::{
    ForegroundOutcome, HookRunOutcome, ProcessError, ProcessHandle, ProcessOutput, ProcessRunner,
    ProcessStreamSink,
};
pub use prompting_gate::{
    PermissionRequest, PromptDecision, PromptDefault, PromptError, PromptingGate,
};
pub use rooted_fs::{AtomicWriteOptions, RootedFileLock};
pub use runtime::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};
pub use sandbox::{
    BackendPlanHandle, NetworkPolicy, ProcessCommand, ResourceLimits, Sandbox, SandboxBackend,
    SandboxCapability, SandboxError, SandboxFeatures, SandboxPolicy, SandboxedCommand,
    SandboxedTag,
};
pub use secure_storage::{SecureStorage, SecureStorageBackend, SecureStorageError};
pub use share::{ShareError, SharePayload, ShareResult, SharingService};
pub use skill_loader::{SkillLoad, SkillLoader};
pub use stt::{SpeechToText, SttError, SttOpts, SttTranscript};
pub use subagent_spawn::{
    SubagentInheritance, SubagentResult, SubagentSpawnError, SubagentSpawnRequest, SubagentSpawner,
    SubagentUsage,
};
pub use swarm::{PaneId, PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};
pub use task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};
pub use team_registry::{TeamRegistryHandle, WorkerInfo};
pub use team_spawn::{TeamSpawnError, TeamSpawnSeam};
pub use tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};
pub use tts::{TextToSpeech, TtsAudio, TtsError, TtsOpts};
pub use voice::{VoiceError, VoiceRecorder, VoiceRecording, VoiceRecordingOpts};
pub use web_search::{WebSearchConfigProvider, WebSearchRuntimeConfig};
#[allow(unused_imports)]
pub use worktree::*;
pub use worktree::{WorktreeError, WorktreeHandle, WorktreeInfo, WorktreeManager};
