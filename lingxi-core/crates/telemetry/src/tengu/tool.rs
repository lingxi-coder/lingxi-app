//! `tengu_tool_*` event schemas — 40 events emitted by the tool dispatcher,
//! permission gate, and per-tool implementations (M4 owner; M3-06 schema lock).
//!
//! Spec §7 line 768-779. Covers the generic tool lifecycle (started/completed/
//! failed/cancelled), the permission gate, and per-tool started/completed/
//! failed triads for bash, edit, read, write, grep, glob, `web_fetch`, task,
//! notebook, and MCP, plus a single skill-invocation event. User-derived
//! string identifiers are typed [`Verified`](crate::Verified); free-form user
//! content that may carry filepaths, secrets, or org-internal hostnames is
//! [`PiiTagged`](crate::PiiTagged).

use crate::pii::{PiiTagged, Verified};
use serde::{Deserialize, Serialize};

// -- Event name constants (byte-locked) ---------------------------------------

/// `tengu_tool_started` — generic per-invocation start (before dispatch).
pub const STARTED: &str = "tengu_tool_started";
/// `tengu_tool_completed` — generic per-invocation success.
pub const COMPLETED: &str = "tengu_tool_completed";
/// `tengu_tool_failed` — generic per-invocation failure.
pub const FAILED: &str = "tengu_tool_failed";
/// `tengu_tool_cancelled` — caller cancelled an in-flight tool call.
pub const CANCELLED: &str = "tengu_tool_cancelled";
/// `tengu_tool_permission_requested` — permission prompt presented to the user.
pub const PERMISSION_REQUESTED: &str = "tengu_tool_permission_requested";
/// `tengu_tool_permission_granted` — user (or remembered rule) allowed the action.
pub const PERMISSION_GRANTED: &str = "tengu_tool_permission_granted";
/// `tengu_tool_permission_denied` — user (or remembered rule) refused the action.
pub const PERMISSION_DENIED: &str = "tengu_tool_permission_denied";
/// `tengu_tool_permission_remembered` — decision was persisted for future calls.
pub const PERMISSION_REMEMBERED: &str = "tengu_tool_permission_remembered";
/// `tengu_tool_bash_started` — bash invocation about to spawn.
pub const BASH_STARTED: &str = "tengu_tool_bash_started";
/// `tengu_tool_bash_completed` — bash invocation exited.
pub const BASH_COMPLETED: &str = "tengu_tool_bash_completed";
/// `tengu_tool_bash_failed` — bash invocation errored before producing an exit code.
pub const BASH_FAILED: &str = "tengu_tool_bash_failed";
/// `tengu_tool_bash_timeout` — bash invocation hit the timeout watchdog.
pub const BASH_TIMEOUT: &str = "tengu_tool_bash_timeout";
/// `tengu_tool_edit_started` — Edit tool began applying a replacement.
pub const EDIT_STARTED: &str = "tengu_tool_edit_started";
/// `tengu_tool_edit_completed` — Edit tool wrote the modified file.
pub const EDIT_COMPLETED: &str = "tengu_tool_edit_completed";
/// `tengu_tool_edit_failed` — Edit tool errored (mismatch, IO, etc.).
pub const EDIT_FAILED: &str = "tengu_tool_edit_failed";
/// `tengu_tool_read_started` — Read tool began reading a file.
pub const READ_STARTED: &str = "tengu_tool_read_started";
/// `tengu_tool_read_completed` — Read tool returned file contents.
pub const READ_COMPLETED: &str = "tengu_tool_read_completed";
/// `tengu_tool_read_failed` — Read tool errored.
pub const READ_FAILED: &str = "tengu_tool_read_failed";
/// `tengu_tool_write_started` — Write tool began writing a file.
pub const WRITE_STARTED: &str = "tengu_tool_write_started";
/// `tengu_tool_write_completed` — Write tool finished writing a file.
pub const WRITE_COMPLETED: &str = "tengu_tool_write_completed";
/// `tengu_tool_write_failed` — Write tool errored.
pub const WRITE_FAILED: &str = "tengu_tool_write_failed";
/// `tengu_tool_grep_started` — Grep tool began scanning.
pub const GREP_STARTED: &str = "tengu_tool_grep_started";
/// `tengu_tool_grep_completed` — Grep tool returned match results.
pub const GREP_COMPLETED: &str = "tengu_tool_grep_completed";
/// `tengu_tool_grep_failed` — Grep tool errored (bad regex, IO).
pub const GREP_FAILED: &str = "tengu_tool_grep_failed";
/// `tengu_tool_glob_started` — Glob tool began expanding a pattern.
pub const GLOB_STARTED: &str = "tengu_tool_glob_started";
/// `tengu_tool_glob_completed` — Glob tool returned matched paths.
pub const GLOB_COMPLETED: &str = "tengu_tool_glob_completed";
/// `tengu_tool_glob_failed` — Glob tool errored.
pub const GLOB_FAILED: &str = "tengu_tool_glob_failed";
/// `tengu_tool_web_fetch_started` — `WebFetch` tool began an HTTP request.
pub const WEB_FETCH_STARTED: &str = "tengu_tool_web_fetch_started";
/// `tengu_tool_web_fetch_completed` — `WebFetch` tool received the response.
pub const WEB_FETCH_COMPLETED: &str = "tengu_tool_web_fetch_completed";
/// `tengu_tool_web_fetch_failed` — `WebFetch` tool errored (network, status code).
pub const WEB_FETCH_FAILED: &str = "tengu_tool_web_fetch_failed";
/// `tengu_tool_task_dispatched` — Task tool spawned a subagent.
pub const TASK_DISPATCHED: &str = "tengu_tool_task_dispatched";
/// `tengu_tool_task_completed` — Task subagent returned successfully.
pub const TASK_COMPLETED: &str = "tengu_tool_task_completed";
/// `tengu_tool_task_failed` — Task subagent errored.
pub const TASK_FAILED: &str = "tengu_tool_task_failed";
/// `tengu_tool_notebook_started` — `NotebookEdit` tool began editing a notebook.
pub const NOTEBOOK_STARTED: &str = "tengu_tool_notebook_started";
/// `tengu_tool_notebook_completed` — `NotebookEdit` tool wrote the modified notebook.
pub const NOTEBOOK_COMPLETED: &str = "tengu_tool_notebook_completed";
/// `tengu_tool_notebook_failed` — `NotebookEdit` tool errored.
pub const NOTEBOOK_FAILED: &str = "tengu_tool_notebook_failed";
/// `tengu_tool_mcp_invoked` — MCP tool call was dispatched to a server.
pub const MCP_INVOKED: &str = "tengu_tool_mcp_invoked";
/// `tengu_tool_mcp_completed` — MCP tool call returned a result.
pub const MCP_COMPLETED: &str = "tengu_tool_mcp_completed";
/// `tengu_tool_mcp_failed` — MCP tool call errored.
pub const MCP_FAILED: &str = "tengu_tool_mcp_failed";
/// `tengu_tool_skill_invoked` — Skill tool ran a registered skill.
pub const SKILL_INVOKED: &str = "tengu_tool_skill_invoked";
/// `tengu_tool_powershell_started` — PowerShell invocation about to spawn (M4-02).
pub const POWERSHELL_STARTED: &str = "tengu_tool_powershell_started";
/// `tengu_tool_powershell_completed` — PowerShell invocation exited (M4-02).
pub const POWERSHELL_COMPLETED: &str = "tengu_tool_powershell_completed";
/// `tengu_tool_powershell_failed` — PowerShell invocation errored (M4-02).
pub const POWERSHELL_FAILED: &str = "tengu_tool_powershell_failed";
/// `tengu_tool_repl_started` — REPL snippet about to execute (M4-02).
pub const REPL_STARTED: &str = "tengu_tool_repl_started";
/// `tengu_tool_repl_completed` — REPL snippet finished (M4-02).
pub const REPL_COMPLETED: &str = "tengu_tool_repl_completed";
/// `tengu_tool_repl_failed` — REPL snippet errored (M4-02).
pub const REPL_FAILED: &str = "tengu_tool_repl_failed";
/// `tengu_tool_sleep_started` — Sleep invocation began (M4-02).
pub const SLEEP_STARTED: &str = "tengu_tool_sleep_started";
/// `tengu_tool_sleep_completed` — Sleep invocation woke (M4-02).
pub const SLEEP_COMPLETED: &str = "tengu_tool_sleep_completed";
/// `tengu_tool_sleep_failed` — Sleep invocation rejected/errored (M4-02).
pub const SLEEP_FAILED: &str = "tengu_tool_sleep_failed";

/// Order-locked array of all 49 names; consumed by `tengu::ALL_EVENT_NAMES`.
pub(crate) const NAMES: &[&str] = &[
    STARTED,
    COMPLETED,
    FAILED,
    CANCELLED,
    PERMISSION_REQUESTED,
    PERMISSION_GRANTED,
    PERMISSION_DENIED,
    PERMISSION_REMEMBERED,
    BASH_STARTED,
    BASH_COMPLETED,
    BASH_FAILED,
    BASH_TIMEOUT,
    EDIT_STARTED,
    EDIT_COMPLETED,
    EDIT_FAILED,
    READ_STARTED,
    READ_COMPLETED,
    READ_FAILED,
    WRITE_STARTED,
    WRITE_COMPLETED,
    WRITE_FAILED,
    GREP_STARTED,
    GREP_COMPLETED,
    GREP_FAILED,
    GLOB_STARTED,
    GLOB_COMPLETED,
    GLOB_FAILED,
    WEB_FETCH_STARTED,
    WEB_FETCH_COMPLETED,
    WEB_FETCH_FAILED,
    TASK_DISPATCHED,
    TASK_COMPLETED,
    TASK_FAILED,
    NOTEBOOK_STARTED,
    NOTEBOOK_COMPLETED,
    NOTEBOOK_FAILED,
    MCP_INVOKED,
    MCP_COMPLETED,
    MCP_FAILED,
    SKILL_INVOKED,
    POWERSHELL_STARTED,
    POWERSHELL_COMPLETED,
    POWERSHELL_FAILED,
    REPL_STARTED,
    REPL_COMPLETED,
    REPL_FAILED,
    SLEEP_STARTED,
    SLEEP_COMPLETED,
    SLEEP_FAILED,
];

// -- Shared enums -------------------------------------------------------------

/// Generic tool-permission outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PermissionDecision {
    /// User explicitly allowed this single invocation.
    Allow,
    /// User explicitly denied this invocation.
    Deny,
    /// Allowed once with no persistence.
    AllowOnce,
    /// Allowed and remembered for the active scope.
    AllowAlways,
}

/// Where a remembered permission persists.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PermissionScope {
    /// Lives only for the active session.
    Session,
    /// Persists to the project-level settings.
    Project,
    /// Persists to the user-global settings.
    User,
}

/// Generic tool failure classification (no PII).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolFailureKind {
    /// Input failed validation before dispatch.
    InvalidInput,
    /// Permission gate refused the action.
    PermissionDenied,
    /// Tool ran past its timeout watchdog.
    Timeout,
    /// Underlying IO or syscall error.
    IoError,
    /// Sandbox runtime refused the operation.
    SandboxRefused,
    /// Catch-all bucket for everything else.
    Other,
}

// -- Generic lifecycle payloads -----------------------------------------------

/// Payload for [`STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted tool name (e.g. `bash`, `edit`, `mcp__server__method`).
    pub tool_name: Verified,
}

/// Payload for [`COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted tool name.
    pub tool_name: Verified,
    /// Wall-clock duration of the tool call in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted tool name.
    pub tool_name: Verified,
    /// Coarse failure classification.
    pub failure_kind: ToolFailureKind,
}

/// Payload for [`CANCELLED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelledPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted tool name.
    pub tool_name: Verified,
}

// -- Permission gate payloads -------------------------------------------------

/// Payload for [`PERMISSION_REQUESTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionRequestedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted tool name.
    pub tool_name: Verified,
    /// Whitelisted action (e.g. `read`, `write`, `network`).
    pub action: Verified,
}

/// Payload for [`PERMISSION_GRANTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionGrantedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Outcome of the prompt.
    pub decision: PermissionDecision,
}

/// Payload for [`PERMISSION_DENIED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionDeniedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted denial reason (no PII).
    pub reason: Verified,
}

/// Payload for [`PERMISSION_REMEMBERED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionRememberedPayload {
    /// Whitelisted tool name the rule applies to.
    pub tool_name: Verified,
    /// Whitelisted action the rule applies to.
    pub action: Verified,
    /// Where the decision was persisted.
    pub scope: PermissionScope,
}

// -- Bash payloads ------------------------------------------------------------

/// Payload for [`BASH_STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BashStartedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// The command string is PII-tagged (may include filepaths / secrets).
    pub command: PiiTagged,
    /// Configured timeout for this invocation, in milliseconds.
    pub timeout_ms: u64,
}

/// Payload for [`BASH_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BashCompletedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Process exit code (negative for signal-terminated).
    pub exit_code: i32,
    /// Wall-clock duration of the tool call in milliseconds.
    pub duration_ms: u64,
    /// Bytes captured on stdout.
    pub stdout_bytes: u64,
    /// Bytes captured on stderr.
    pub stderr_bytes: u64,
}

/// Payload for [`BASH_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BashFailedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted error description (no PII).
    pub error: Verified,
}

/// Payload for [`BASH_TIMEOUT`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BashTimeoutPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Configured timeout that was hit, in milliseconds.
    pub timeout_ms: u64,
}

// -- Edit payloads ------------------------------------------------------------

/// Payload for [`EDIT_STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditStartedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// PII-tagged: filepath (may carry user directory names).
    pub file_path: PiiTagged,
}

/// Payload for [`EDIT_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditCompletedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Number of replacement matches applied.
    pub replacements: u32,
    /// Wall-clock duration of the tool call in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`EDIT_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditFailedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Coarse failure classification.
    pub failure_kind: ToolFailureKind,
}

// -- Read payloads ------------------------------------------------------------

/// Payload for [`READ_STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadStartedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// PII-tagged: filepath.
    pub file_path: PiiTagged,
}

/// Payload for [`READ_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadCompletedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Total bytes read from the file.
    pub bytes_read: u64,
    /// Wall-clock duration of the tool call in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`READ_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadFailedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Coarse failure classification.
    pub failure_kind: ToolFailureKind,
}

// -- Write payloads -----------------------------------------------------------

/// Payload for [`WRITE_STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteStartedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// PII-tagged: filepath.
    pub file_path: PiiTagged,
}

/// Payload for [`WRITE_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteCompletedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Total bytes written to the file.
    pub bytes_written: u64,
    /// Wall-clock duration of the tool call in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`WRITE_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteFailedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Coarse failure classification.
    pub failure_kind: ToolFailureKind,
}

// -- Grep payloads ------------------------------------------------------------

/// Payload for [`GREP_STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrepStartedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// PII-tagged: regex pattern may include filepath fragments.
    pub pattern: PiiTagged,
}

/// Payload for [`GREP_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrepCompletedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Number of matching lines / hits.
    pub matches: u64,
    /// Number of files scanned.
    pub files_scanned: u64,
    /// Wall-clock duration of the tool call in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`GREP_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrepFailedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Coarse failure classification.
    pub failure_kind: ToolFailureKind,
}

// -- Glob payloads ------------------------------------------------------------

/// Payload for [`GLOB_STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobStartedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// PII-tagged: glob pattern may include filepath fragments.
    pub pattern: PiiTagged,
}

/// Payload for [`GLOB_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobCompletedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Number of files matched by the pattern.
    pub matches: u64,
    /// Wall-clock duration of the tool call in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`GLOB_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobFailedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Coarse failure classification.
    pub failure_kind: ToolFailureKind,
}

// -- WebFetch payloads --------------------------------------------------------

/// Payload for [`WEB_FETCH_STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebFetchStartedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// PII-tagged: URL may include tokens or org-internal hostnames.
    pub url: PiiTagged,
}

/// Payload for [`WEB_FETCH_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebFetchCompletedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// HTTP response status code.
    pub status: u32,
    /// Total bytes received in the response body.
    pub bytes_received: u64,
    /// Wall-clock duration of the tool call in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`WEB_FETCH_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebFetchFailedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Coarse failure classification.
    pub failure_kind: ToolFailureKind,
}

// -- Task payloads ------------------------------------------------------------

/// Payload for [`TASK_DISPATCHED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDispatchedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted subagent kind (`general-purpose`, `output-style-setup`, etc.).
    pub subagent_kind: Verified,
}

/// Payload for [`TASK_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskCompletedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Wall-clock duration of the subagent in milliseconds.
    pub duration_ms: u64,
    /// Turns the subagent ran before returning.
    pub turns: u32,
}

/// Payload for [`TASK_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskFailedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted error description (no PII).
    pub error: Verified,
}

// -- Notebook payloads --------------------------------------------------------

/// Payload for [`NOTEBOOK_STARTED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotebookStartedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// PII-tagged: notebook filepath.
    pub notebook_path: PiiTagged,
}

/// Payload for [`NOTEBOOK_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotebookCompletedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Number of cells modified.
    pub cells_edited: u32,
    /// Wall-clock duration of the tool call in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`NOTEBOOK_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotebookFailedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Coarse failure classification.
    pub failure_kind: ToolFailureKind,
}

// -- MCP payloads -------------------------------------------------------------

/// Payload for [`MCP_INVOKED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpInvokedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted MCP server name.
    pub server_name: Verified,
    /// Whitelisted MCP tool / method name.
    pub method: Verified,
}

/// Payload for [`MCP_COMPLETED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpCompletedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted MCP server name.
    pub server_name: Verified,
    /// Wall-clock duration of the tool call in milliseconds.
    pub duration_ms: u64,
}

/// Payload for [`MCP_FAILED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpFailedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted MCP server name.
    pub server_name: Verified,
    /// Coarse failure classification.
    pub failure_kind: ToolFailureKind,
}

// -- Skill payload ------------------------------------------------------------

/// Payload for [`SKILL_INVOKED`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillInvokedPayload {
    /// Stable identifier for this tool invocation.
    pub invocation_id: Verified,
    /// Whitelisted skill name.
    pub skill_name: Verified,
}
