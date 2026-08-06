//! Listing / screen DTOs — the pull/reply payloads for every client screen
//! (sessions, models, MCP, hooks, agents, slash commands, memory, status,
//! settings, auth, doctor, tasks) (plan F1-05).
//!
//! This module defines the SUPPORTING row/payload structs and nested enums.
//! The listing *events* themselves (`SessionList`, `McpServers`, `Agents`, …)
//! are variants of [`crate::events::ClientEvent`] — the single outbound
//! envelope — and carry these structs.
//!
//! **Name reconciliation** (plan line 149): the design spec §4.1 says
//! `AgentList`, but the WIRE name is `Agents` ([`ClientEvent::Agents`]).
//! Reconciled here while the snapshot is still unfrozen.
//!
//! These DTOs are derived as mechanically as possible from the existing engine
//! info structs so parity is structural (the engine→DTO lowering itself lives
//! in `client-adapter`, F1-11/F1-15, NOT here). The engine source is noted per
//! DTO:
//! - [`SessionRowDto`] ← `session::jsonl::loader::SessionMetadata` (`.path` is
//!   mapped DIRECTLY — it exists at `session/src/jsonl/loader.rs:33`, decision
//!   per plan line 152 — do NOT synthesize).
//! - [`McpServerDto`]/[`McpStatusDto`] ← `traits::orchestrator::{McpServerInfo,
//!   McpStatus}` (`McpStatus::Error(String)` is lowered to the STRUCT variant
//!   [`McpStatusDto::Error`] for `UniFFI` flatness, plan line 154).
//! - [`HookDto`] ← `traits::orchestrator::HookInfo`.
//! - [`AgentDto`] ← `traits::orchestrator::AgentInfo`.
//! - [`SlashCommandDto`] ← `command_api::model::SlashCommand` (display fields).
//! - [`MemoryEntryDto`]/[`MemoryTierDto`] ← `protocol::{MemoryEntry,
//!   MemoryEntryTier}`.
//! - [`StatusSnapshotDto`] ← `traits::orchestrator::StatusSnapshot` (traits
//!   shape canonical; status-line fields appended OPTIONAL, plan line 155).
//! - [`AuthStateDto`] ← `Option<traits::auth::LoginInfo>`.
//! - [`DoctorReportDto`]/[`DoctorCheckDto`]/[`CheckStatusDto`]/[`DoctorSummaryDto`]
//!   ← `traits::orchestrator::{DoctorReport, DoctorCheck, CheckStatus,
//!   DoctorSummary}`.
//! - [`TaskRowDto`]/[`TaskStatusDto`] ← `traits::task_registry::TaskRecord` +
//!   `tasks::TaskStatus`. (`TaskOutputChunk` is carried inline by
//!   [`crate::events::ClientEvent::TaskOutputChunk`], mirroring
//!   `traits::task_registry::TaskOutputChunk`.)
//!
//! Frozen serde conventions (decision §0.1):
//! - internally tagged: `#[serde(tag = "type", rename_all = "snake_case")]`,
//! - reserved-extensible enums are `#[non_exhaustive]`,
//! - every optional field uses
//!   `#[serde(default, skip_serializing_if = "Option::is_none")]`.
//!
//! Settings payloads (`effective_json`/`provenance_json`) are JSON **Strings**
//! on the wire (decision §0.4); `serde_json::Value` never enters this crate.

use serde::{Deserialize, Serialize};

// ── Sessions ─────────────────────────────────────────────────────────────────

/// One resumable-session row — the lowered `SessionMetadata`
/// (`session/src/jsonl/loader.rs:23`). Carried by [`crate::events::ClientEvent::SessionList`].
///
/// `modified` (`SystemTime`) is lowered to an RFC 3339 `String`,
/// `message_count` (`usize`) to a `u32`, and `uuid`/`path` to `String`s. `path`
/// is mapped DIRECTLY (plan line 152 — it is a real field, not synthesized).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct SessionRowDto {
    /// The session UUID (parsed from the filename stem), as a stable string.
    pub uuid: String,
    /// The session title (≤ 50 chars + ellipsis).
    pub title: String,
    /// File mtime as an RFC 3339 timestamp (`SystemTime` lowered, decision §0.4).
    pub modified_rfc3339: String,
    /// Number of JSONL lines in the file (`usize` lowered to `u32`).
    pub message_count: u32,
    /// Absolute path to the `.jsonl` file — mapped directly from
    /// `SessionMetadata.path` so a client can request a re-load.
    pub path: String,
}

// ── Models ───────────────────────────────────────────────────────────────────
//
// `ModelList`/`ModelChanged` carry provider-qualified `String` references, so
// they have no supporting struct here — they are plain `ClientEvent` variants
// (see `events.rs`). Keeping the existing wire shape avoids a protocol/UniFFI
// break while preserving provider identity for grouped client pickers.

// ── MCP ──────────────────────────────────────────────────────────────────────

/// One MCP server entry — the lowered `McpServerInfo`
/// (`traits/src/orchestrator.rs:122`). Carried by
/// [`crate::events::ClientEvent::McpServers`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct McpServerDto {
    /// Server name as registered in settings.
    pub name: String,
    /// Connection status at snapshot time.
    pub status: McpStatusDto,
    /// Transport kind: `"stdio"`, `"sse"`, or `"http"`.
    pub transport: String,
}

/// Connection status for an MCP server — the lowered `McpStatus`
/// (`traits/src/orchestrator.rs:133`).
///
/// The engine's `McpStatus::Error(String)` is a *tuple* variant; it is lowered
/// here to the STRUCT variant [`McpStatusDto::Error`] for `UniFFI` flatness
/// (decision §0.4 / plan line 154). Internally tagged on `type`, `snake_case`.
/// `#[non_exhaustive]` so a future status is additive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpStatusDto {
    /// Connected and healthy.
    Connected,
    /// Disconnected — either never connected or cleanly shut down.
    Disconnected,
    /// Connection failed with the wrapped reason. Lowered from the engine's
    /// tuple `McpStatus::Error(String)` to a STRUCT variant for `UniFFI`.
    Error {
        /// Human-readable failure reason.
        reason: String,
    },
}

// ── Hooks ────────────────────────────────────────────────────────────────────

/// One hook entry — the lowered `HookInfo` (`traits/src/orchestrator.rs:144`).
/// Carried by [`crate::events::ClientEvent::Hooks`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct HookDto {
    /// Hook identifier.
    pub name: String,
    /// Hook event (`"PreToolUse"`, `"PostToolUse"`, `"Stop"`, `"Notification"`).
    pub event: String,
    /// Optional matcher regex (tool-name pattern). Skipped when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// Timeout in milliseconds (default `60_000` if unset on the engine side).
    pub timeout_ms: u64,
}

// ── Agents ───────────────────────────────────────────────────────────────────

/// One subagent entry — the lowered `AgentInfo`
/// (`traits/src/orchestrator.rs:157`). Carried by
/// [`crate::events::ClientEvent::Agents`] (WIRE name; reconciled from spec
/// §4.1 `AgentList`, plan line 149).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AgentDto {
    /// Agent name (matches the markdown filename without extension).
    pub name: String,
    /// Human-readable description (may be truncated by callers).
    pub description: String,
    /// Tool allow-list (empty = all tools).
    pub tools_allowed: Vec<String>,
}

// ── Slash commands ───────────────────────────────────────────────────────────

/// One slash-command catalog entry — the display-relevant fields of
/// `command_api::model::SlashCommand` (`command-api/src/model.rs:12`). The rich
/// `SlashCommandKind` dispatch shape stays engine-side; the wire carries `name`,
/// `description`, and a `source` classification string. Carried by
/// [`crate::events::ClientEvent::SlashCommandCatalog`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct SlashCommandDto {
    /// Command name (without the leading `/`).
    pub name: String,
    /// Short user-facing description.
    pub description: String,
    /// Origin classification string (e.g. `"builtin"`, `"markdown"`,
    /// `"plugin"`, `"mcp"`) — the lowered `CommandSource`.
    pub source: String,
}

// ── Memory ───────────────────────────────────────────────────────────────────

/// One LINGXI.md memory entry — the lowered `protocol::MemoryEntry`
/// (`protocol/src/messages.rs:201`). Carried by
/// [`crate::events::ClientEvent::MemoryEntries`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct MemoryEntryDto {
    /// Absolute path the entry was loaded from.
    pub path: String,
    /// Tier the entry belongs to.
    pub tier: MemoryTierDto,
    /// Raw markdown body (frontmatter stripped, secrets redacted).
    pub body: String,
    /// Age in whole days from the load `now` (`0` for a just-written file).
    pub age_days: u64,
    /// File size in bytes (post-redaction body length).
    pub size_bytes: u64,
}

/// Memory tier — the lowered `protocol::MemoryEntryTier`
/// (`protocol/src/messages.rs:218`). Internally tagged on `type`, `snake_case`.
/// `#[non_exhaustive]` so a future tier is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemoryTierDto {
    /// Session-scoped entry (highest tier weight).
    Session,
    /// Project-scoped entry.
    Project,
    /// Team-scoped entry.
    Team,
    /// User-scoped entry (lowest tier).
    User,
}

// ── Status ───────────────────────────────────────────────────────────────────

/// The `/status` panel snapshot — the lowered `StatusSnapshot`
/// (`traits/src/orchestrator.rs:214`). The traits-shape fields are canonical;
/// status-line fields are appended OPTIONAL (plan line 155). Carried by
/// [`crate::events::ClientEvent::StatusSnapshot`].
///
/// `Eq` is intentionally NOT derived (matches the engine struct) because
/// `total_cost_usd: f64` does not implement `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct StatusSnapshotDto {
    /// Current session id (as a stable string for display).
    pub session_id: String,
    /// Active model name (e.g. `"claude-opus-4-7"`).
    pub model: String,
    /// Total messages in the session history.
    pub n_messages: u32,
    /// Cumulative cost in USD.
    pub total_cost_usd: f64,
    /// Cumulative input tokens.
    pub input_tokens: u64,
    /// Cumulative output tokens.
    pub output_tokens: u64,
    /// MCP servers currently in `Connected` state.
    pub n_mcp_connected: u32,
    /// MCP servers configured (any state).
    pub n_mcp_total: u32,
    /// Hooks registered.
    pub n_hooks: u32,
    /// Subagents available.
    pub n_agents: u32,
    /// Session start time, RFC 3339 (UTC) — already a String in the engine.
    pub started_at: String,
    /// Working directory used to launch the session (`PathBuf` lowered).
    pub cwd: String,
    /// Optional pre-rendered status-line string — an APPENDED optional field
    /// (plan line 155). Skipped from the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_line: Option<String>,
    /// Active coordinator-team workers at snapshot time — an APPENDED OPTIONAL
    /// field (T21). `None` (skipped from the wire) for a non-coordinator
    /// session; `Some(n)` echoes the same scalar the PUSH
    /// [`crate::events::ClientEvent::CoordinatorStatus`] feed carries. Adding an
    /// optional field is a `Compatible` change (no `CLIENT_PROTOCOL_VERSION`
    /// major bump — version-guard `adding_optional_field_is_compatible`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_workers: Option<u32>,
}

// ── Auth ─────────────────────────────────────────────────────────────────────

/// Auth state — the lowered `Option<traits::auth::LoginInfo>`
/// (`traits/src/auth.rs:13`). Carried by
/// [`crate::events::ClientEvent::AuthState`]. Internally tagged on `type`,
/// `snake_case`. `#[non_exhaustive]` so a future state (e.g. `LoggingIn`) is
/// additive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuthStateDto {
    /// No stored credentials (`None` from `current_user`).
    SignedOut,
    /// A signed-in user (`Some(LoginInfo)` from `current_user`).
    SignedIn {
        /// Email address identifying the signed-in user.
        email: String,
        /// Anthropic organization id (`"org_..."` prefix).
        org_id: String,
    },
}

// ── Doctor ───────────────────────────────────────────────────────────────────

/// Aggregate diagnostic report — the lowered `DoctorReport`
/// (`traits/src/orchestrator.rs:168`). Carried by
/// [`crate::events::ClientEvent::DoctorReport`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct DoctorReportDto {
    /// Individual check results in execution order.
    pub checks: Vec<DoctorCheckDto>,
    /// Summary tallies (pass/warn/fail counts).
    pub summary: DoctorSummaryDto,
}

/// One `/doctor` check result — the lowered `DoctorCheck`
/// (`traits/src/orchestrator.rs:177`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct DoctorCheckDto {
    /// Check identifier (`"config-dir"`, `"api-key"`, etc.).
    pub name: String,
    /// Pass/warn/fail outcome.
    pub status: CheckStatusDto,
    /// Optional detail string. Skipped from the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Outcome of a single [`DoctorCheckDto`] — the lowered `CheckStatus`
/// (`traits/src/orchestrator.rs:188`). Internally tagged on `type`,
/// `snake_case`. `#[non_exhaustive]` so a future outcome is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CheckStatusDto {
    /// Check succeeded.
    Pass,
    /// Check produced a warning (non-fatal anomaly).
    Warn,
    /// Check failed.
    Fail,
}

/// Pass/warn/fail tallies in a [`DoctorReportDto`] — the lowered
/// `DoctorSummary` (`traits/src/orchestrator.rs:198`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct DoctorSummaryDto {
    /// Number of checks that returned [`CheckStatusDto::Pass`].
    pub passed: u32,
    /// Number of checks that returned [`CheckStatusDto::Warn`].
    pub warnings: u32,
    /// Number of checks that returned [`CheckStatusDto::Fail`].
    pub failed: u32,
}

// ── Tasks ────────────────────────────────────────────────────────────────────

/// One task row — the lowered `TaskRecord` (`traits/src/task_registry.rs:36`).
/// The engine's `status` wire `String` is lowered to a [`TaskStatusDto`] enum.
/// Carried by [`crate::events::ClientEvent::TaskRow`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct TaskRowDto {
    /// 9-char `[bartwmdks][0-9a-z]{8}` task id.
    pub task_id: String,
    /// Task type wire string (one of the 7 byte-locked variants).
    pub task_type: String,
    /// Task status.
    pub status: TaskStatusDto,
    /// Human-readable description.
    pub description: String,
}

/// Task status — the lowered `tasks::TaskStatus` (`tasks/src/state.rs:11`), the
/// 5 byte-locked statuses. Internally tagged on `type`, `snake_case`.
/// `#[non_exhaustive]` so a future status is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskStatusDto {
    /// Created but not yet started.
    Pending,
    /// Currently executing.
    Running,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
    /// Stopped by the user before completion.
    Cancelled,
}

// ── Coordinator (T18 — per-worker roster) ─────────────────────────────────────

/// One coordinator-team worker row — the lowered `traits::team_registry::WorkerInfo`
/// (itself the POD projection of the coordinator's `WorkerAgent`). Field-shaped
/// to lower 1:1 onto the TUI `WorkerRow` (`tui/src/multiagent/state.rs:25`):
/// `agent_id` / `name` / `agent_type` / `status`. Carried by
/// [`crate::events::ClientEvent::CoordinatorWorker`] (one per worker, the same
/// shape as [`TaskRowDto`] / [`crate::events::ClientEvent::TaskRow`]).
///
/// This is DISTINCT from `permission::WorkerInfoDto` (`{name, color, team}`,
/// reserved for sub-agent permission requests) — that shape does not match the
/// `WorkerRow` roster row.
///
/// `status` is a simplified label `String` (the same convention as
/// [`TaskRowDto::status`]'s source `TaskRecord.status`) so this crate need not
/// import the concrete `WorkerStatus` enum.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct CoordinatorWorkerDto {
    /// Worker agent id, stringified (the `AgentId` display / wire form).
    pub agent_id: String,
    /// Display name (rendered in the roster).
    pub name: String,
    /// Agent-type string (e.g. `"explorer"`, `"writer"`).
    pub agent_type: String,
    /// Simplified status label (e.g. `"idle"`, `"working"`, `"failed"`).
    pub status: String,
}
