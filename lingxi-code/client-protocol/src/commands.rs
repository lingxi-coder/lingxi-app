//! `ClientCommand` DTOs — inbound commands a client sends to the engine
//! (plan F1-06).
//!
//! This module freezes the FULL command set and ENCODES governing decision
//! §0.5: one transport connection owns ONE engine host that SWAPS its inner
//! `ConversationOrchestrator` in place on New/Resume. **`session_id` is a
//! CONNECTION ATTRIBUTE** — it is carried in the [`SessionStarted`] /
//! [`SessionResumed`] events ([`crate::events::ClientEvent`]), NOT as a
//! per-live-command param. The ONE allowed occurrence is
//! [`ClientCommand::ResumeSession`], which NAMES a target session to resume;
//! the `no_live_command_carries_session_id` test in
//! `tests/commands_test.rs` enforces this lock structurally.
//!
//! **Explicitly NOT commands** (client-local; the adapter does not back them,
//! decision per plan line 171) — these are intentionally absent from the
//! [`ClientCommand`] enum and are documented here, not encoded as variants:
//! - `SearchMessages` / `JumpToMessage` — scrollback navigation is a pure
//!   client-side view concern over the already-streamed message set.
//! - `ExportSession` — a client-side serialization of local state.
//! - `SetTheme` — a pure client UI preference.
//! - prompt-history (recall/up-arrow) — a client-local input buffer.
//!
//! Frozen serde conventions (decision §0.1):
//! - internally tagged: `#[serde(tag = "type", rename_all = "snake_case")]`
//!   (matches `protocol::ContentBlock` / api-client `StreamEvent`),
//! - the top-level enum is `#[non_exhaustive]` (mirrors `traits::OutputEvent`),
//! - every optional field uses
//!   `#[serde(default, skip_serializing_if = "Option::is_none")]`.
//!
//! Tool/image payloads stay UniFFI-flat (decision §0.4 / §0.8): inline image
//! bytes are a base64 [`String`] in [`ImageRefDto`]; `serde_json::Value` never
//! enters this crate.

use crate::computer_access::ComputerAccessResponseDto;
use crate::controls::ReasoningSelectionDto;
use crate::listings::TaskStatusDto;
use crate::local_apps::{AppAuthorizationDecisionDto, AppBridgeRequestDto, AppCreateOriginDto};
use crate::permission::PermissionResponseDto;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_git_version_control() -> bool {
    true
}

fn is_default_git_version_control(value: &bool) -> bool {
    *value
}

/// A provider credential carried over the authenticated local bridge.
///
/// The wire representation is a plain JSON string for TypeScript/UniFFI
/// compatibility, while the Rust `Debug` surface is always redacted so tracing
/// or assertion output cannot accidentally print the secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(transparent)]
pub struct ProviderCredentialSecretDto {
    /// Secret wire value. Callers should keep its lifetime as short as possible.
    pub value: String,
}

impl ProviderCredentialSecretDto {
    /// Wrap a provider credential for transport.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self { value }
    }

    /// Borrow the credential at the audited persistence boundary.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.value
    }
}

impl std::fmt::Debug for ProviderCredentialSecretDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderCredentialSecretDto(<redacted>)")
    }
}

/// The inbound command envelope a client sends to the engine.
///
/// `#[non_exhaustive]` (mirrors `traits::OutputEvent`) so adding a command is
/// additive (no major bump). Internally tagged on `type`, `snake_case`
/// (decision §0.1).
///
/// **Session-lifecycle lock (decision §0.5):** no variant carries a
/// `session_id` EXCEPT [`Self::ResumeSession`], which names a target to resume.
/// `session_id` otherwise travels as a CONNECTION ATTRIBUTE on the
/// [`SessionStarted`](crate::events::ClientEvent::SessionStarted) /
/// [`SessionResumed`](crate::events::ClientEvent::SessionResumed) events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ClientCommand {
    // ── Turn driving ──────────────────────────────────────────────────────
    /// Submit a user prompt to drive a turn. Carries the text, an optional
    /// [`PromptModeDto`], inline image bytes (decision §0.8), and an optional
    /// client turn correlator. Carries NO `session_id` (decision §0.5).
    SendPrompt {
        /// The user prompt text.
        text: String,
        /// Optional prompt-input mode. Skipped from the wire when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_mode: Option<PromptModeDto>,
        /// Inline image attachments. Uniform `{media_type, base64}` per
        /// decision §0.8 (the adapter writes inline bytes to a temp file on the
        /// engine host for the path-based engine entry). A non-optional `Vec`:
        /// always present (empty when no images).
        images: Vec<ImageRefDto>,
        /// Optional client-supplied turn correlator, echoed on
        /// [`TurnStarted`](crate::events::ClientEvent::TurnStarted). Skipped
        /// when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<u64>,
    },

    /// Cancel the in-flight turn. The optional `turn_id` narrows the cancel to
    /// a specific turn; `None` cancels the current one.
    Cancel {
        /// Optional turn correlator to cancel. Skipped when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<u64>,
    },

    // ── Permission resolution ─────────────────────────────────────────────
    /// Approve a parked permission request, correlated by `request_id`
    /// (the id-keyed gate, F1-14). The `response` distinguishes once / always.
    ApprovePermission {
        /// Correlator with the originating
        /// [`PermissionRequest`](crate::permission::PermissionRequest).
        request_id: u64,
        /// The approval kind (`AllowOnce` / `AllowAlways`).
        response: PermissionResponseDto,
    },

    /// Deny a parked permission request, correlated by `request_id`.
    DenyPermission {
        /// Correlator with the originating
        /// [`PermissionRequest`](crate::permission::PermissionRequest).
        request_id: u64,
    },

    // ── `computer` tool `request_access` resolution ──────────────────────────
    /// Approve a parked `computer`-tool `request_access` prompt, correlated by
    /// `request_id` (the connection-scoped
    /// `BridgeComputerAccessBroker`). `response` carries the granted app
    /// subset + capability flags.
    ApproveComputerAccess {
        /// Correlator with the originating
        /// [`ComputerAccessRequestDto`](crate::computer_access::ComputerAccessRequestDto).
        request_id: u64,
        /// The user's grant.
        response: ComputerAccessResponseDto,
    },

    /// Deny a parked `computer`-tool `request_access` prompt, correlated by
    /// `request_id`.
    DenyComputerAccess {
        /// Correlator with the originating
        /// [`ComputerAccessRequestDto`](crate::computer_access::ComputerAccessRequestDto).
        request_id: u64,
    },

    // ── `AskUserQuestion` resolution ──────────────────────────────────────
    /// Submit the answers for a parked interactive questionnaire.
    AnswerAskUserQuestion {
        /// Correlator from the originating
        /// [`AskUserQuestionRequestDto`](crate::ask_user_question::AskUserQuestionRequestDto).
        request_id: u64,
        /// Question text to the user's selected label(s) or free-text answer.
        answers: HashMap<String, String>,
    },

    /// Cancel a parked interactive questionnaire.
    CancelAskUserQuestion {
        /// Correlator from the originating request.
        request_id: u64,
    },

    /// Change the live permission mode for subsequent tool checks. Confirmed
    /// by a [`PermissionModeChanged`](crate::events::ClientEvent::PermissionModeChanged)
    /// event carrying the authoritative mode after engine-side validation.
    SetPermissionMode {
        /// Permission-mode wire id (`default`, `acceptEdits`, `plan`, `auto`,
        /// `dontAsk`, or `bypassPermissions`).
        mode: String,
    },

    // ── Provider credentials ─────────────────────────────────────────────
    /// Return non-secret availability for the requested provider ids.
    ListProviderCredentials {
        /// Main-process correlator echoed by `ProviderCredentialStatus`.
        operation_id: u64,
        /// Provider/keychain ids to inspect. Secret values are never returned.
        provider_ids: Vec<String>,
    },

    /// Persist a provider credential through the engine's shared secure store.
    SetProviderCredential {
        /// Main-process correlator echoed by `ProviderCredentialStatus`.
        operation_id: u64,
        /// Provider/keychain id used by CLI, TUI, and Desktop.
        provider_id: String,
        /// Secret received only over the authenticated loopback bridge.
        credential: ProviderCredentialSecretDto,
    },

    /// Delete a provider credential from the engine's shared secure store.
    DeleteProviderCredential {
        /// Main-process correlator echoed by `ProviderCredentialStatus`.
        operation_id: u64,
        /// Provider/keychain id used by CLI, TUI, and Desktop.
        provider_id: String,
    },

    // ── Model ─────────────────────────────────────────────────────────────
    /// Switch the active model. Confirmed by a
    /// [`ModelChanged`](crate::events::ClientEvent::ModelChanged) event.
    SetModel {
        /// The model name to activate.
        model: String,
    },

    /// Request the available-model catalog. Replied with a
    /// [`ModelList`](crate::events::ClientEvent::ModelList) event.
    ListModels,

    // ── Slash commands ────────────────────────────────────────────────────
    /// Run a raw slash-command line. Local/display commands reply with
    /// [`crate::events::ClientEvent::SlashCommandResult`]; prompt-expanding
    /// commands reuse `turn_id` for the ordinary turn stream.
    RunSlashCommand {
        /// The raw command line (e.g. `"/model opus"`), leading `/` included.
        raw: String,
        /// Optional client turn id used to correlate prompt-like slash commands
        /// with the normal streaming turn lifecycle.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<u64>,
    },

    // ── Listings ──────────────────────────────────────────────────────────
    /// Refresh a set of screen listings by kind. Each requested
    /// [`ListingKindDto`] produces its matching listing event
    /// ([`McpServers`](crate::events::ClientEvent::McpServers), etc.).
    RefreshListings {
        /// Which screen listings to (re)pull.
        which: Vec<ListingKindDto>,
    },

    /// List the agent instances attached to the current session. The reply is
    /// [`ClientEvent::SessionAgentList`](crate::events::ClientEvent::SessionAgentList)
    /// and is scoped to the connection's active session.
    ListSessionAgents,

    /// Load one session-agent transcript. `agent_id` accepts the stable
    /// `agent:<uuid>` form returned by [`SessionAgentSummaryDto`], or `main`
    /// for the parent conversation. Invalid ids are rejected by the engine.
    LoadSessionAgentTranscript {
        /// Agent instance to load.
        agent_id: String,
    },

    // ── Session lifecycle (decision §0.5) ─────────────────────────────────
    /// Start a fresh session on this connection (swaps the connection's inner
    /// orchestrator). Carries an optional working directory + model, but NO
    /// `session_id` — the new id is reported back via
    /// [`SessionStarted`](crate::events::ClientEvent::SessionStarted).
    NewSession {
        /// Optional working directory for the new session. Skipped when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// Optional starting model. Skipped when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },

    /// Resume a prior session on this connection. **The ONE allowed
    /// `session_id`-carrying command** (decision §0.5): it NAMES a target
    /// session to resume. Confirmed by a
    /// [`SessionResumed`](crate::events::ClientEvent::SessionResumed) event.
    ResumeSession {
        /// The id of the session to resume (the named target).
        session_id: String,
        /// Optional working directory override. Skipped when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },

    /// Request the resumable-session catalog. Replied with a
    /// [`SessionList`](crate::events::ClientEvent::SessionList) event.
    ListSessions {
        /// Optional cap on the number of rows. Skipped when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },

    // ── Auth + session control ────────────────────────────────────────────
    /// Begin the login flow. Auth state surfaces via
    /// [`AuthState`](crate::events::ClientEvent::AuthState).
    Login,

    /// Sign out the current user.
    Logout,

    /// Force a context compaction now. Reports via
    /// [`CompactionCompleted`](crate::events::ClientEvent::CompactionCompleted).
    ForceCompact,

    /// Clear the current session's history (rejected mid-turn, F2-08).
    ClearSession,

    // ── Tasks ─────────────────────────────────────────────────────────────
    /// List background tasks, optionally filtered by status. Each row arrives
    /// as a [`TaskRow`](crate::events::ClientEvent::TaskRow) event.
    TaskList {
        /// Optional status filter. Skipped from the wire when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status_filter: Option<TaskStatusDto>,
    },

    /// Pull a task's accumulated output spool from `offset`. Replied with a
    /// [`TaskOutputChunk`](crate::events::ClientEvent::TaskOutputChunk) event.
    TaskOutput {
        /// 9-char task id.
        task_id: String,
        /// Byte/line offset to read from.
        offset: u64,
    },

    /// Stop a running task by id.
    TaskStop {
        /// 9-char task id.
        task_id: String,
    },

    /// Resume a paused local workflow in the current session. The engine
    /// resolves the persisted script/checkpoint by task id; clients never
    /// receive or submit script paths or arguments directly.
    ResumeWorkflow {
        /// Paused workflow task id.
        task_id: String,
    },

    // ── Local apps ────────────────────────────────────────────────────────
    /// List the local apps. Replied with an
    /// [`AppsChanged`](crate::events::ClientEvent::AppsChanged) event carrying
    /// the full record set.
    ListApps,

    /// Request the complete detail snapshot for one app.
    GetAppDetails {
        /// App whose detail snapshot is requested.
        app_id: String,
    },

    /// Create a new local-app record and its workspace. Confirmed by an
    /// [`AppsChanged`](crate::events::ClientEvent::AppsChanged) event.
    CreateApp {
        /// User-facing display name.
        name: String,
        /// Where the creation originated (`chat` / `library`).
        origin: AppCreateOriginDto,
        /// One-line description of what the app should do — the seed the
        /// LLM authors the questionnaire from. Distinct from `name`: a
        /// display label is not a spec, and conflating the two used to leave
        /// the questionnaire authored from a bare app name.
        brief: String,
        /// Whether to keep Git-backed source versions. Defaults to enabled
        /// when omitted by an older client.
        #[serde(
            default = "default_git_version_control",
            skip_serializing_if = "is_default_git_version_control"
        )]
        git_enabled: bool,
        /// Provider-qualified model reference used by the app creation
        /// workflow. Skipped when the app follows the current session model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_model: Option<String>,
        /// Conversation the app was created from (`origin: chat`). Skipped
        /// from the wire when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
    },

    /// Start the app's dev-server runtime. Phase 1 validates the app exists,
    /// then fails typed with
    /// [`AppOperationFailed`](crate::events::ClientEvent::AppOperationFailed)
    /// `{ code: not_yet_available }` (runtime is phase 4).
    StartApp {
        /// App to start.
        app_id: String,
    },

    /// Stop the app's dev-server runtime (`not_yet_available` until phase 4,
    /// like [`Self::StartApp`]).
    StopApp {
        /// App to stop.
        app_id: String,
    },

    /// Restart the app's dev-server runtime (`not_yet_available` until phase
    /// 4, like [`Self::StartApp`]).
    RestartApp {
        /// App to restart.
        app_id: String,
    },

    /// Execute one data-only request from the versioned local-app bridge.
    ExecuteAppBridgeRequest {
        /// Data-only request to execute.
        request: AppBridgeRequestDto,
    },

    /// Resolve a permission-gated structured `WebView` action.
    ResolveAppUiRequest {
        /// Pending UI request correlator.
        request_id: String,
        /// User's scoped authorization decision.
        decision: AppAuthorizationDecisionDto,
        /// Structured `WebView` inspection/action result as JSON data.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_json: Option<String>,
        /// Host-side action failure, if the authorized action failed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Resolve a native app capability request.
    ResolveAppCapabilityRequest {
        /// Pending capability request correlator.
        request_id: String,
        /// User's scoped authorization decision.
        decision: AppAuthorizationDecisionDto,
    },

    /// Approve or reject a host-issued App Agent Profile proposal. The token
    /// is minted by the engine and is never accepted from the local-app page.
    ResolveAppProfileProposal {
        /// App whose profile is being changed.
        app_id: String,
        /// One-time token from [`AppProfileProposal`](crate::local_apps::AppEventDto).
        approval_token: String,
        /// `true` applies the exact proposal shown by the trusted client UI.
        approved: bool,
    },

    /// Revoke every session and durable capability grant for one app. Future
    /// gated operations prompt again; the design manifest is not changed.
    ResetAppPermissions {
        /// App whose saved grants should be cleared.
        app_id: String,
    },

    /// Page through one app's workspace-scoped session catalog. Replied with
    /// an [`AppSessionsChanged`](crate::events::ClientEvent::AppSessionsChanged)
    /// event. `offset`/`limit` page the modified-descending catalog
    /// (default limit 50, max 100); the reply's `next_offset` is `None` on
    /// the last page.
    ListAppSessions {
        /// App whose sessions to list.
        app_id: String,
        /// Zero-based row offset into the modified-descending catalog.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<u64>,
        /// Page size (default 50, max 100).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },

    /// List an app's restorable checkpoints. Phase 1 replies with an empty
    /// list (git wiring is phase 5).
    ListAppCheckpoints {
        /// App whose checkpoints to list.
        app_id: String,
    },

    /// Restore an app workspace to a checkpoint. Phase 1 validates the app
    /// exists, then fails typed with
    /// [`AppOperationFailed`](crate::events::ClientEvent::AppOperationFailed)
    /// `{ code: not_yet_available }` (git wiring is phase 5).
    RestoreAppCheckpoint {
        /// App to restore.
        app_id: String,
        /// Checkpoint to restore to.
        checkpoint_id: String,
    },

    /// Delete an app record and its workspace. Confirmed by an
    /// [`AppsChanged`](crate::events::ClientEvent::AppsChanged) event.
    DeleteApp {
        /// App to delete.
        app_id: String,
    },

    // ── Lifecycle ─────────────────────────────────────────────────────────
    /// Request a clean engine/connection shutdown.
    RequestExit,

    /// Request the authoritative conversation-controls snapshot for the
    /// active session/model.
    GetConversationControls,

    /// Change the live reasoning selection for subsequent provider requests.
    SetReasoningSelection {
        /// Provider-neutral reasoning selection.
        selection: ReasoningSelectionDto,
    },
}

/// Prompt-input mode for [`ClientCommand::SendPrompt`]. Internally tagged on
/// `type`, `snake_case`. `#[non_exhaustive]` so a future mode is additive.
///
/// Mirrors the TUI input modes (normal chat, `!` bash, `#` memory, plan mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PromptModeDto {
    /// Normal chat prompt.
    Normal,
    /// Bash-passthrough mode (`!`-prefixed input).
    Bash,
    /// Memory-edit mode (`#`-prefixed input).
    Memory,
    /// Plan mode (read-only planning before edits).
    Plan,
}

/// An inline image attachment for [`ClientCommand::SendPrompt`]. Uniform inline
/// `{media_type, base64}` on the wire (no transport-leaking enum, decision
/// §0.8): the adapter writes the inline bytes to a temp file on the engine host
/// for the path-based engine entry. Mobile inline image *input* is itself
/// DEFERRED (§5.12) — this DTO is the frozen shape, not a lit-up path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct ImageRefDto {
    /// MIME media type (e.g. `"image/png"`, `"image/jpeg"`).
    pub media_type: String,
    /// Base64-encoded image bytes.
    pub base64: String,
}

/// The reply for [`ClientCommand::RunSlashCommand`] — slash dispatch is LOSSY,
/// so the result is a display string plus an optional prompt the command wants
/// injected as the next turn's input. (The rich `SlashCommandKind` dispatch
/// shape stays engine-side; this is what crosses the wire.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct CommandResultDto {
    /// User-facing display text produced by the command.
    pub display: String,
    /// Optional prompt the command injects as the next turn input. Skipped from
    /// the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub injected: Option<String>,
}

/// Which screen listing to (re)pull via [`ClientCommand::RefreshListings`].
/// Internally tagged on `type`, `snake_case`. `#[non_exhaustive]` so a future
/// screen kind is additive. Each kind maps to a listing event in
/// [`crate::events::ClientEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ListingKindDto {
    /// Resumable-session catalog → `SessionList`.
    Sessions,
    /// Available-model catalog → `ModelList`.
    Models,
    /// MCP server listing → `McpServers`.
    Mcp,
    /// Hook listing → `Hooks`.
    Hooks,
    /// Subagent listing → `Agents`.
    Agents,
    /// Slash-command catalog → `SlashCommandCatalog`.
    SlashCommands,
    /// LINGXI.md memory listing → `MemoryEntries`.
    Memory,
    /// `/status` panel snapshot → `StatusSnapshot`.
    Status,
    /// Effective settings + provenance → `SettingsSnapshot`.
    Settings,
    /// Auth state → `AuthState`.
    Auth,
    /// `/doctor` report → `DoctorReport`.
    Doctor,
    /// Task listing → `TaskRow` (one per task).
    Tasks,
    /// Coordinator-team roster → `CoordinatorWorker` (one per worker) (T18).
    Coordinator,
}
