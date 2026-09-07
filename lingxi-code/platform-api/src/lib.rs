//! Platform abstraction traits (`platform-api` crate).
//!
//! Library crates depend on these traits; platform crates (`platforms/posix`,
//! `platforms/windows`, etc.) implement them. The conversation state machine
//! (`core`) never imports a concrete runtime or OS API directly.
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
/// (`claude-code_2-1-252_agent`) and the WebFetch `User-Agent`
/// (`claude-code/2.1.252`). LingXi is a 1:1 copy, so it presents the same string.
/// Single source of truth (R-V1) so the AI_AGENT and User-Agent stamps never drift.
///
/// Raised 2.1.241 → 2.1.245 on 2026-08-25, once the main query-loop identity
/// (`querySource` / `print` vs non-interactive) matched the 2.1.245 binary.
/// Raised 2.1.246 → 2.1.252 on 2026-08-30 after the mcp/plugin byte-alignment
/// backlog was implemented against the newer oracle.
///
/// The bump is deliberately LAST. It is what this session tells servers and child
/// processes it is, so raising it before the behaviour matched would overstate
/// the port — and the port has been burned by the opposite error too (it once
/// advertised 2.1.217 while implementing 2.1.220), which is why all three
/// identifiers derive from this one constant.
pub const CLAUDE_CODE_VERSION: &str = "2.1.252";

pub mod agent_name_registry;
pub mod agent_view;
pub mod android_ui;
pub mod auth;
pub mod backgrounding;
pub mod bg_session_forker;
pub mod bridge;
pub mod budget;
pub mod calendar;
pub mod camera;
pub mod clipboard;
pub mod clock;
pub mod commands;
pub mod computer_control;
pub mod contacts;
pub mod coordinator_mode;
pub mod deep_link;
pub mod device_status;
pub mod effect_handler;
pub mod env;
pub mod evidence;
pub mod file_history_sink;
pub mod filesystem;
pub mod fork_resume_gate;
pub mod fork_subagent;
pub mod fusion;
pub mod haptics;
pub mod http;
pub mod ide;
mod live_session_words;
pub mod live_sessions;
pub mod location;
pub mod lsp;
pub mod mailbox;
pub mod mcp;
pub mod mobile_linux;
pub mod mobile_runtime_environment;
pub mod model_attempt;
pub mod model_capabilities;
pub mod notification;
pub mod orchestrator;
pub mod panel_pool;
pub mod parked_agent_store;
pub mod permission_gate;
pub mod platform;
pub mod process;
pub mod prompting_gate;
pub mod repo_root_reload;
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
pub mod uds_inbox;
pub mod voice;
pub mod web_search;
pub mod workflow_output;
pub mod worktree;

pub use android_ui::{
    AndroidAccessRequest, AndroidAccessTier, AndroidAction, AndroidActionResult, AndroidAppInfo,
    AndroidAudioListenRequest, AndroidAudioSpeakRequest, AndroidAudioSpeakResult,
    AndroidAudioTranscript, AndroidAutomationError, AndroidAutomationSessionState,
    AndroidAutomationStatus, AndroidCaptureMode, AndroidGlobalAction, AndroidNodeQuery,
    AndroidRect, AndroidScreenshot, AndroidUiAutomation, AndroidUiNode, AndroidUiSnapshot,
    AndroidWaitCondition, MAX_ANDROID_AUDIO_LISTEN_MS, MAX_ANDROID_AUDIO_SPEAK_CHARS,
    MAX_ANDROID_UI_BATCH, MAX_ANDROID_UI_DEPTH, MAX_ANDROID_UI_NODES, MAX_ANDROID_UI_WAIT_MS,
};
pub use auth::{AuthError, AuthHandle, LoginInfo};
pub use backgrounding::{
    classify_backgrounding, BackgroundingDecision, BackgroundingSnapshot,
    DEFAULT_BACKGROUND_DEFER_MS,
};
pub use bridge::{BridgeConfig, BridgeConnection, BridgeError, BridgeTransport};
pub use budget::{
    BudgetCommitReceipt, BudgetEnforcerHandle, BudgetError, BudgetReservationId,
    BudgetSettlementReceipt,
};
pub use calendar::{CalendarError, CalendarEvent, CalendarProvider, CalendarQuery};
pub use camera::{CameraControl, CameraError, CameraPosition, CapturePhotoOpts, CapturedImage};
pub use clipboard::{Clipboard, ClipboardError};
pub use clock::Clock;
pub use commands::{SlashCommandDispatcher, SlashDispatchResult};
pub use computer_control::{ComputerControl, ComputerError, Screenshot};
pub use contacts::{Contact, ContactsError, ContactsProvider, ContactsQuery};
pub use deep_link::{DeepLinkError, DeepLinkOpener};
pub use device_status::{DeviceStatus, DeviceStatusError, DeviceStatusProvider};
pub use effect_handler::EffectHandler;
pub use evidence::{
    EvidenceBlockRef, EvidenceCapability, EvidenceContext, EvidenceReceipt, EvidenceReceiptRef,
    EvidenceRun, EvidenceSource, MAX_EVIDENCE_BYTES_PER_PANEL, MAX_EVIDENCE_BYTES_PER_RECEIPT,
    MAX_EVIDENCE_BYTES_PER_RUN,
};
pub use file_history_sink::FileHistorySink;
pub use filesystem::{
    apply_line_window, file_content_from_prefix_bytes, FileContent, FileEvent, FileEventKind,
    FileSystem, FileSystemCacheIdentity, FlockGuard, FsError,
};
pub use fusion::{
    normalize_dimensions, panel_never_dispatched, parse_fusion_model_ref, parse_fusion_models,
    validate_panel_report, DurableFusionOutboxRecord, DurableFusionTerminalRecord, EvidenceKind,
    FusionActivation, FusionAgentSurface, FusionAnalysis, FusionAttemptSettlementStatus, FusionCompletionSink,
    FusionContradiction, FusionCostClass, FusionDecision, FusionError, FusionExecutor,
    FusionInheritance, FusionLatencyClass, FusionModelHints, FusionModelRef,
    FusionNeedsParentReason, FusionOrigin, FusionPreparedSummary, FusionPreset, FusionProgress,
    FusionPublicationReceipt, FusionPublicationState, FusionPublicationStatus,
    FusionRecommendation, FusionRequest, FusionResult, FusionRunControl, FusionRunFacts,
    FusionRunFactsRecorder, FusionRunId, FusionRunIdentity, FusionRunOutcome, FusionRunRecorder,
    FusionRunRecorderFactory, FusionSlashPublicationTarget, FusionStage, FusionStatus,
    FusionSubmission, FusionTerminalCapability, FusionTiming, FusionUniqueInsight, FusionUsage,
    NoopFusionCompletionSink, PanelClaim, PanelEvidence, PanelOutcome, PanelPosition, PanelReport,
    PanelRisk, PanelRunStatus, PreparedFusionRun, RiskSeverity, DEFAULT_FUSION_DIMENSIONS,
    DEFAULT_FUSION_DIMENSION_DESCRIPTIONS, FUSION_MAX_PANEL, FUSION_MIN_PANEL, FUSION_PANEL_TYPE,
    FUSION_SCHEMA_VERSION, FUSION_WORKFLOW_CALL_CAP_HARD_LIMIT,
};
pub use haptics::{HapticError, HapticService, HapticStyle};
pub use http::{
    HttpError, HttpTransport, RawByteStreamWithMeta, ResolvedAddressOverride, WebSocketConnection,
    WebSocketConnectionWithMeta, WebSocketMessageStream, WebSocketMessageStreamWithMeta,
};
pub use ide::{IdeEndpointInfo, IdeHandle, IdeStatus, IdeTransport};
pub use location::{LocationError, LocationFix, LocationProvider};
pub use live_sessions::{SessionIdClaim, SessionWriterLease, SharedSessionWriterLease};
pub use lsp::{
    LspError, LspRawConnection, LspServerCapabilities, LspServerConfig, LspTransport,
    NewDiagnosticsSource,
};
pub use mailbox::{
    MailboxError as RouterMailboxError, MailboxMessage, MailboxRouterHandle, RouteAck,
};
pub use mcp::*;
pub use mobile_linux::{
    LinuxCommandRequest, LinuxCommandResult, LinuxEnforcementReceipt, LinuxProcessHandle,
    MobileLinuxCapability, MobileLinuxError, MobileLinuxEvent, MobileLinuxEventKind,
    MobileLinuxRuntime, MobileLinuxRuntimeMode, MobileLinuxSandboxPlan, MobileLinuxTaskSnapshot,
    MobileLinuxTaskStatus, MountPurpose, MountSpec, PtyOpenRequest, PtySessionHandle, PtySize,
    RawStdioOpenRequest, RawStdioReadResult, RawStdioSessionHandle, RootfsState, RootfsStatus,
    UnavailableMobileLinuxRuntime,
};
pub use mobile_runtime_environment::{
    MobileDeviceClass, MobileExecutionTarget, MobileHostEnvironment, MobileHostOs,
    MobileLaunchMode, MobileLifecyclePolicy, MobileNetworkPolicy, MobileRuntimeEnvironment,
    MobileToolRuntime, MOBILE_RUNTIME_ENVIRONMENT_VERSION,
};
pub use model_attempt::{
    ModelAttemptBillingMode, ModelAttemptContext, ModelAttemptContextError,
    ModelAttemptRegistrationId, ModelAttemptRun, ModelAttemptStage,
};
pub use notification::{NotificationError, NotificationRequest, NotificationService};
pub use orchestrator::{
    curated_model_listings, curated_model_names, curated_model_refs, is_curated_model,
    parse_model_ref, provider_default_model, provider_fallback_order, provider_has_curated_list,
    provider_model_catalog, provider_model_catalog_listings, qualified_model_ref,
    reasoning_control_spec_for_model, validated_reasoning_selection_for_model, ActiveGoalSnapshot,
    AgentInfo, AttachmentKind, CheckStatus, CompactionSummary, ContextPressureBanner,
    ContextPressureLevel, ContextUsageCategory, ContextUsageCategoryKind, ContextUsageSnapshot,
    ConversationControls, CostSnapshot, CurrentUsageSnapshot, DeferredToolReplay,
    DirectoryAddedHookSummary, DoctorCheck, DoctorReport, DoctorSummary, ForkOutcome,
    GoalStatusAttachment, GoalStatusKind, HandleError, HookInfo, McpActionState, McpServerInfo,
    McpStatus, McpToggleOutcome, MemoryEditorOutcome, ModelBillingMode, ModelCapabilities,
    ModelListing, ModelMetadata, ModelPricing, ModelPricingTier, ModelProvenance, ModelUsageRow,
    OrchestratorHandle, OutputEvent, OutputStream, PermissionControlState,
    PermissionModeAvailability, PlanSnapshot, PromptSnapshot, PromptToolDescription,
    RateLimitSnapshot, ReasoningBudgetRange, ReasoningControlSpec, ReasoningSelection,
    RecapOutcome, RegisterRepoRootOutcome, RegisterRepoRootRequest, ResumeRuntimeSnapshot,
    RewindRowData, SkillInfo, StatusSnapshot, TurnOutcome,
};
pub use panel_pool::{PanelPoolLease, PanelPoolPermit};
pub use permission_gate::{
    AutoModePrompt, PermissionDecision, PermissionGate, PermissionRequestSource,
};
pub use platform::Platform;
pub use process::{
    ForegroundOutcome, ForegroundRunResult, HookOutputObserver, HookRunOutcome, ProcessError,
    ProcessHandle, ProcessOutput, ProcessOutputFile, ProcessRunner, ProcessStreamSink,
};
pub use prompting_gate::{
    PermissionRequest, PromptDecision, PromptDefault, PromptError, PromptingGate,
};
pub use repo_root_reload::{RepoRootReloadOutcome, RepoRootReloadRequest, RepoRootReloader};
pub use rooted_fs::{
    atomic_write_pinned, lock_exclusive_pinned, open_read_file_pinned, sync_parent_pinned,
    truncate_file_pinned, AtomicWriteOptions, RootedFileLock, RootIdentity,
};
pub use runtime::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};
pub use sandbox::{
    BackendPlanHandle, NetworkPolicy, ProcessCommand, ResourceLimits, Sandbox, SandboxBackend,
    SandboxCapability, SandboxError, SandboxFeatures, SandboxPolicy, SandboxedCommand,
    SandboxedTag,
};
pub use secure_storage::{
    CredentialStoragePolicy, InMemorySecureStorage, SecureStorage, SecureStorageBackend,
    SecureStorageError,
};
pub use share::{ShareError, SharePayload, ShareResult, SharingService};
pub use skill_loader::{SkillLoad, SkillLoader};
pub use stt::{SpeechToText, SttError, SttOpts, SttTranscript};
pub use subagent_spawn::{
    StructuredOutputMode, SubagentInheritance, SubagentObservation, SubagentResult,
    SubagentSpawnError, SubagentSpawnObserver, SubagentSpawnRequest, SubagentSpawner,
    SubagentUsage, WorkflowQueryWatchdog,
};
pub use swarm::{PaneId, PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};
pub use task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};
pub use team_registry::{TeamRegistryHandle, WorkerInfo};
pub use team_spawn::{TeamSpawnError, TeamSpawnSeam};
pub use tool_invoker::{
    SubagentInvocationContext, ToolExecutionPolicy, ToolInvoker, ToolInvokerError,
};
pub use tts::{TextToSpeech, TtsAudio, TtsError, TtsOpts};
pub use voice::{VoiceError, VoiceRecorder, VoiceRecording, VoiceRecordingOpts};
pub use web_search::{WebSearchConfigProvider, WebSearchRuntimeConfig};
pub use workflow_output::{
    WorkflowOutputAccount, WorkflowOutputEventId, WorkflowOutputScope, WorkflowOutputScopes,
};
#[allow(unused_imports)]
pub use worktree::*;
pub use worktree::{WorktreeError, WorktreeHandle, WorktreeInfo, WorktreeManager};
