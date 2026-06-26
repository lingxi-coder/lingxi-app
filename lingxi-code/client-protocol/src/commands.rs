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

use crate::listings::TaskStatusDto;
use crate::permission::PermissionResponseDto;
use serde::{Deserialize, Serialize};

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

    /// Run a raw slash-command line. **LOSSY at the dispatcher** — the reply is
    /// a [`CommandResultDto`] (display text + optional injected prompt), not a
    /// structured result.
    RunSlashCommand {
        /// The raw command line (e.g. `"/model opus"`), leading `/` included.
        raw: String,
    },

    // ── Listings ──────────────────────────────────────────────────────────

    /// Refresh a set of screen listings by kind. Each requested
    /// [`ListingKindDto`] produces its matching listing event
    /// ([`McpServers`](crate::events::ClientEvent::McpServers), etc.).
    RefreshListings {
        /// Which screen listings to (re)pull.
        which: Vec<ListingKindDto>,
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

    // ── Lifecycle ─────────────────────────────────────────────────────────

    /// Request a clean engine/connection shutdown.
    RequestExit,
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
