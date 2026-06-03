//! Platform abstraction traits.
//!
//! Engine crates depend on these traits; platform crates (`platforms/posix`,
//! `platforms/windows`, etc.) implement them. The engine never imports a
//! concrete runtime or OS API directly.
//!
//! See spec §4 (Trait System) and D17 (Runtime boundary).

#![forbid(unsafe_code)]

pub mod auth;
pub mod bridge;
pub mod budget;
pub mod camera;
pub mod clock;
pub mod commands;
pub mod computer_control;
pub mod effect_handler;
pub mod filesystem;
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
pub mod runtime;
pub mod sandbox;
pub mod secure_storage;
pub mod share;
pub mod stt;
pub mod subagent_spawn;
pub mod swarm;
pub mod task_registry;
pub mod team_registry;
pub mod team_spawn;
pub mod tool_invoker;
pub mod tts;
pub mod voice;
pub mod worktree;

pub use auth::{AuthError, AuthHandle, LoginInfo};
pub use bridge::{BridgeConfig, BridgeConnection, BridgeError, BridgeTransport};
pub use budget::{BudgetEnforcerHandle, BudgetError};
pub use camera::{CameraControl, CameraError, CameraPosition, CapturePhotoOpts, CapturedImage};
pub use clock::Clock;
pub use commands::{SlashCommandDispatcher, SlashDispatchResult};
pub use computer_control::{ComputerControl, ComputerError, Screenshot};
pub use effect_handler::EffectHandler;
pub use filesystem::{FileContent, FileEvent, FileEventKind, FileSystem, FlockGuard, FsError};
pub use http::{HttpError, HttpTransport};
pub use lsp::{LspError, LspRawConnection, LspServerCapabilities, LspServerConfig, LspTransport};
pub use mailbox::{
    MailboxError as RouterMailboxError, MailboxMessage, MailboxRouterHandle, RouteAck,
};
pub use mcp::*;
pub use notification::{NotificationError, NotificationRequest, NotificationService};
pub use orchestrator::{
    AgentInfo, CheckStatus, CompactionSummary, CostSnapshot, DoctorCheck, DoctorReport,
    DoctorSummary, HandleError, HookInfo, McpServerInfo, McpStatus, MemoryEditorOutcome,
    OrchestratorHandle, OutputEvent, OutputStream, StatusSnapshot, TurnOutcome,
};
pub use permission_gate::{PermissionDecision, PermissionGate};
pub use platform::Platform;
pub use process::{ProcessError, ProcessHandle, ProcessOutput, ProcessRunner};
pub use prompting_gate::{
    PermissionRequest, PromptDecision, PromptDefault, PromptError, PromptingGate,
};
pub use runtime::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};
pub use sandbox::{
    NetworkPolicy, ProcessCommand, ResourceLimits, Sandbox, SandboxBackend, SandboxCapability,
    SandboxError, SandboxFeatures, SandboxPolicy, SandboxedCommand, SandboxedTag,
};
pub use secure_storage::{SecureStorage, SecureStorageBackend, SecureStorageError};
pub use share::{ShareError, SharePayload, ShareResult, SharingService};
pub use stt::{SpeechToText, SttError, SttOpts, SttTranscript};
pub use tts::{TextToSpeech, TtsAudio, TtsError, TtsOpts};
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
pub use voice::{VoiceError, VoiceRecorder, VoiceRecording, VoiceRecordingOpts};
#[allow(unused_imports)]
pub use worktree::*;
pub use worktree::{WorktreeError, WorktreeHandle, WorktreeInfo, WorktreeManager};
