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
/// `tengu_tool_web_search_started` — `WebSearch` tool dispatched a query (M4-03).
pub const WEB_SEARCH_STARTED: &str = "tengu_tool_web_search_started";
/// `tengu_tool_web_search_completed` — `WebSearch` tool returned search results (M4-03).
pub const WEB_SEARCH_COMPLETED: &str = "tengu_tool_web_search_completed";
/// `tengu_tool_web_search_failed` — `WebSearch` tool errored (HTTP, parse, etc.) (M4-03).
pub const WEB_SEARCH_FAILED: &str = "tengu_tool_web_search_failed";
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
/// `tengu_tool_powershell_started` — `PowerShell` invocation about to spawn (M4-02).
pub const POWERSHELL_STARTED: &str = "tengu_tool_powershell_started";
/// `tengu_tool_powershell_completed` — `PowerShell` invocation exited (M4-02).
pub const POWERSHELL_COMPLETED: &str = "tengu_tool_powershell_completed";
/// `tengu_tool_powershell_failed` — `PowerShell` invocation errored (M4-02).
pub const POWERSHELL_FAILED: &str = "tengu_tool_powershell_failed";
/// `tengu_tool_repl_started` — `REPL` snippet about to execute (M4-02).
pub const REPL_STARTED: &str = "tengu_tool_repl_started";
/// `tengu_tool_repl_completed` — `REPL` snippet finished (M4-02).
pub const REPL_COMPLETED: &str = "tengu_tool_repl_completed";
/// `tengu_tool_repl_failed` — `REPL` snippet errored (M4-02).
pub const REPL_FAILED: &str = "tengu_tool_repl_failed";
/// `tengu_tool_sleep_started` — `Sleep` invocation began (M4-02).
pub const SLEEP_STARTED: &str = "tengu_tool_sleep_started";
/// `tengu_tool_sleep_completed` — `Sleep` invocation woke (M4-02).
pub const SLEEP_COMPLETED: &str = "tengu_tool_sleep_completed";
/// `tengu_tool_sleep_failed` — `Sleep` invocation rejected/errored (M4-02).
pub const SLEEP_FAILED: &str = "tengu_tool_sleep_failed";

// -- M4-04 Workflow tools (15 events) ------------------------------------------

/// `tengu_tool_todo_write_started` — `TodoWriteTool` began updating the session.
pub const TODO_WRITE_STARTED: &str = "tengu_tool_todo_write_started";
/// `tengu_tool_todo_write_completed` — `TodoWriteTool` finished updating.
pub const TODO_WRITE_COMPLETED: &str = "tengu_tool_todo_write_completed";
/// `tengu_tool_todo_write_failed` — `TodoWriteTool` errored (validation, etc.).
pub const TODO_WRITE_FAILED: &str = "tengu_tool_todo_write_failed";

/// `tengu_tool_enter_plan_mode_started` — `EnterPlanModeTool` began (M4-04).
pub const ENTER_PLAN_MODE_STARTED: &str = "tengu_tool_enter_plan_mode_started";
/// `tengu_tool_enter_plan_mode_completed` — `EnterPlanModeTool` flipped the flag (M4-04).
pub const ENTER_PLAN_MODE_COMPLETED: &str = "tengu_tool_enter_plan_mode_completed";
/// `tengu_tool_enter_plan_mode_failed` — `EnterPlanModeTool` errored (already in mode) (M4-04).
pub const ENTER_PLAN_MODE_FAILED: &str = "tengu_tool_enter_plan_mode_failed";

/// `tengu_tool_exit_plan_mode_started` — `ExitPlanModeTool` began (M4-04).
pub const EXIT_PLAN_MODE_STARTED: &str = "tengu_tool_exit_plan_mode_started";
/// `tengu_tool_exit_plan_mode_completed` — `ExitPlanModeTool` flipped the flag (M4-04).
pub const EXIT_PLAN_MODE_COMPLETED: &str = "tengu_tool_exit_plan_mode_completed";
/// `tengu_tool_exit_plan_mode_failed` — `ExitPlanModeTool` errored (not in mode) (M4-04).
pub const EXIT_PLAN_MODE_FAILED: &str = "tengu_tool_exit_plan_mode_failed";

/// `tengu_tool_enter_worktree_started` — `EnterWorktreeTool` began (M4-04).
pub const ENTER_WORKTREE_STARTED: &str = "tengu_tool_enter_worktree_started";
/// `tengu_tool_enter_worktree_completed` — `EnterWorktreeTool` created a worktree (M4-04).
pub const ENTER_WORKTREE_COMPLETED: &str = "tengu_tool_enter_worktree_completed";
/// `tengu_tool_enter_worktree_failed` — `EnterWorktreeTool` errored (bad slug, git, IO) (M4-04).
pub const ENTER_WORKTREE_FAILED: &str = "tengu_tool_enter_worktree_failed";

/// `tengu_tool_exit_worktree_started` — `ExitWorktreeTool` began (M4-04).
pub const EXIT_WORKTREE_STARTED: &str = "tengu_tool_exit_worktree_started";
/// `tengu_tool_exit_worktree_completed` — `ExitWorktreeTool` removed a worktree (M4-04).
pub const EXIT_WORKTREE_COMPLETED: &str = "tengu_tool_exit_worktree_completed";
/// `tengu_tool_exit_worktree_failed` — `ExitWorktreeTool` errored (M4-04).
pub const EXIT_WORKTREE_FAILED: &str = "tengu_tool_exit_worktree_failed";

// ===== M4-05 Agent + Task tools (24 events, 8 tools × 3 lifecycle stages) =====

/// `tengu_tool_agent_started` — `AgentTool` began spawning a subagent (M4-05).
pub const AGENT_STARTED: &str = "tengu_tool_agent_started";
/// `tengu_tool_agent_completed` — `AgentTool` subagent returned Completed (M4-05).
///
/// Distinct from the legacy `TASK_COMPLETED = "tengu_tool_task_completed"` event
/// (M3-06 baseline, kept for back-compat); the M4-05 const is suffixed with
/// `_M4_05` to avoid identifier collision with the legacy constant.
pub const AGENT_COMPLETED_M4_05: &str = "tengu_tool_agent_completed";
/// `tengu_tool_agent_failed` — `AgentTool` subagent errored or was killed (M4-05).
pub const AGENT_FAILED: &str = "tengu_tool_agent_failed";

/// `tengu_tool_task_create_started` — `TaskCreateTool` validated input (M4-05).
pub const TASK_CREATE_STARTED: &str = "tengu_tool_task_create_started";
/// `tengu_tool_task_create_completed` — `TaskCreateTool` allocated spool + id (M4-05).
pub const TASK_CREATE_COMPLETED: &str = "tengu_tool_task_create_completed";
/// `tengu_tool_task_create_failed` — `TaskCreateTool` errored (M4-05).
pub const TASK_CREATE_FAILED: &str = "tengu_tool_task_create_failed";

/// `tengu_tool_task_get_started` — `TaskGetTool` accepted the request (M4-05).
pub const TASK_GET_STARTED: &str = "tengu_tool_task_get_started";
/// `tengu_tool_task_get_completed` — `TaskGetTool` returned a state row (M4-05).
pub const TASK_GET_COMPLETED: &str = "tengu_tool_task_get_completed";
/// `tengu_tool_task_get_failed` — `TaskGetTool` errored (M4-05).
pub const TASK_GET_FAILED: &str = "tengu_tool_task_get_failed";

/// `tengu_tool_task_list_started` — `TaskListTool` began enumeration (M4-05).
pub const TASK_LIST_STARTED: &str = "tengu_tool_task_list_started";
/// `tengu_tool_task_list_completed` — `TaskListTool` returned the snapshot (M4-05).
pub const TASK_LIST_COMPLETED: &str = "tengu_tool_task_list_completed";
/// `tengu_tool_task_list_failed` — `TaskListTool` errored (M4-05).
pub const TASK_LIST_FAILED: &str = "tengu_tool_task_list_failed";

/// `tengu_tool_task_update_started` — `TaskUpdateTool` validated input (M4-05).
pub const TASK_UPDATE_STARTED: &str = "tengu_tool_task_update_started";
/// `tengu_tool_task_update_completed` — `TaskUpdateTool` mutated status (M4-05).
pub const TASK_UPDATE_COMPLETED: &str = "tengu_tool_task_update_completed";
/// `tengu_tool_task_update_failed` — `TaskUpdateTool` errored (M4-05).
pub const TASK_UPDATE_FAILED: &str = "tengu_tool_task_update_failed";

/// `tengu_tool_task_stop_started` — `TaskStopTool` validated input (M4-05).
pub const TASK_STOP_STARTED: &str = "tengu_tool_task_stop_started";
/// `tengu_tool_task_stop_completed` — `TaskStopTool` killed the task (M4-05).
pub const TASK_STOP_COMPLETED: &str = "tengu_tool_task_stop_completed";
/// `tengu_tool_task_stop_failed` — `TaskStopTool` errored (M4-05).
pub const TASK_STOP_FAILED: &str = "tengu_tool_task_stop_failed";

/// `tengu_tool_task_output_started` — `TaskOutputTool` began spool read (M4-05).
pub const TASK_OUTPUT_STARTED: &str = "tengu_tool_task_output_started";
/// `tengu_tool_task_output_completed` — `TaskOutputTool` returned content (M4-05).
pub const TASK_OUTPUT_COMPLETED: &str = "tengu_tool_task_output_completed";
/// `tengu_tool_task_output_failed` — `TaskOutputTool` errored (M4-05).
pub const TASK_OUTPUT_FAILED: &str = "tengu_tool_task_output_failed";

/// `tengu_tool_send_message_started` — `SendMessageTool` validated input (M4-05).
pub const SEND_MESSAGE_STARTED: &str = "tengu_tool_send_message_started";
/// `tengu_tool_send_message_completed` — `SendMessageTool` enqueued (M4-05).
pub const SEND_MESSAGE_COMPLETED: &str = "tengu_tool_send_message_completed";
/// `tengu_tool_send_message_failed` — `SendMessageTool` errored (M4-05).
pub const SEND_MESSAGE_FAILED: &str = "tengu_tool_send_message_failed";

// ===== M4-06 Team tools (6 events, 2 tools × 3 lifecycle stages) =============

/// `tengu_tool_team_create_started` — `TeamCreateTool` began (M4-06).
pub const TEAM_CREATE_STARTED: &str = "tengu_tool_team_create_started";
/// `tengu_tool_team_create_completed` — `TeamCreateTool` wrote config (M4-06).
pub const TEAM_CREATE_COMPLETED: &str = "tengu_tool_team_create_completed";
/// `tengu_tool_team_create_failed` — `TeamCreateTool` errored (M4-06).
pub const TEAM_CREATE_FAILED: &str = "tengu_tool_team_create_failed";

/// `tengu_tool_team_delete_started` — `TeamDeleteTool` began (M4-06).
pub const TEAM_DELETE_STARTED: &str = "tengu_tool_team_delete_started";
/// `tengu_tool_team_delete_completed` — `TeamDeleteTool` removed dir (M4-06).
pub const TEAM_DELETE_COMPLETED: &str = "tengu_tool_team_delete_completed";
/// `tengu_tool_team_delete_failed` — `TeamDeleteTool` errored (M4-06).
pub const TEAM_DELETE_FAILED: &str = "tengu_tool_team_delete_failed";

/// Order-locked array of all 97 names; consumed by `tengu::ALL_EVENT_NAMES`.
/// M3-06 locked the first 40; M4-02 appended 9 (powershell/repl/sleep);
/// M4-03 appended 3 (`web_search`); M4-04 appended 15 workflow events;
/// M4-05 appended 24 agent/task events; M4-06 appends 6 team events.
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
    WEB_SEARCH_STARTED,
    WEB_SEARCH_COMPLETED,
    WEB_SEARCH_FAILED,
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
    // M4-04 Workflow tools (15 events)
    TODO_WRITE_STARTED,
    TODO_WRITE_COMPLETED,
    TODO_WRITE_FAILED,
    ENTER_PLAN_MODE_STARTED,
    ENTER_PLAN_MODE_COMPLETED,
    ENTER_PLAN_MODE_FAILED,
    EXIT_PLAN_MODE_STARTED,
    EXIT_PLAN_MODE_COMPLETED,
    EXIT_PLAN_MODE_FAILED,
    ENTER_WORKTREE_STARTED,
    ENTER_WORKTREE_COMPLETED,
    ENTER_WORKTREE_FAILED,
    EXIT_WORKTREE_STARTED,
    EXIT_WORKTREE_COMPLETED,
    EXIT_WORKTREE_FAILED,
    // M4-05 Agent + Task tools (24 events, 8 tools × 3 lifecycle stages)
    AGENT_STARTED,
    AGENT_COMPLETED_M4_05,
    AGENT_FAILED,
    TASK_CREATE_STARTED,
    TASK_CREATE_COMPLETED,
    TASK_CREATE_FAILED,
    TASK_GET_STARTED,
    TASK_GET_COMPLETED,
    TASK_GET_FAILED,
    TASK_LIST_STARTED,
    TASK_LIST_COMPLETED,
    TASK_LIST_FAILED,
    TASK_UPDATE_STARTED,
    TASK_UPDATE_COMPLETED,
    TASK_UPDATE_FAILED,
    TASK_STOP_STARTED,
    TASK_STOP_COMPLETED,
    TASK_STOP_FAILED,
    TASK_OUTPUT_STARTED,
    TASK_OUTPUT_COMPLETED,
    TASK_OUTPUT_FAILED,
    SEND_MESSAGE_STARTED,
    SEND_MESSAGE_COMPLETED,
    SEND_MESSAGE_FAILED,
    // M4-06 Team tools (6 events, 2 tools × 3 lifecycle stages)
    TEAM_CREATE_STARTED,
    TEAM_CREATE_COMPLETED,
    TEAM_CREATE_FAILED,
    TEAM_DELETE_STARTED,
    TEAM_DELETE_COMPLETED,
    TEAM_DELETE_FAILED,
];

#[cfg(test)]
mod m4_04_workflow_event_tests {
    use super::*;

    #[test]
    fn todo_write_constants_are_locked() {
        assert_eq!(TODO_WRITE_STARTED, "tengu_tool_todo_write_started");
        assert_eq!(TODO_WRITE_COMPLETED, "tengu_tool_todo_write_completed");
        assert_eq!(TODO_WRITE_FAILED, "tengu_tool_todo_write_failed");
    }

    #[test]
    fn enter_plan_mode_constants_are_locked() {
        assert_eq!(
            ENTER_PLAN_MODE_STARTED,
            "tengu_tool_enter_plan_mode_started"
        );
        assert_eq!(
            ENTER_PLAN_MODE_COMPLETED,
            "tengu_tool_enter_plan_mode_completed"
        );
        assert_eq!(ENTER_PLAN_MODE_FAILED, "tengu_tool_enter_plan_mode_failed");
    }

    #[test]
    fn exit_plan_mode_constants_are_locked() {
        assert_eq!(EXIT_PLAN_MODE_STARTED, "tengu_tool_exit_plan_mode_started");
        assert_eq!(
            EXIT_PLAN_MODE_COMPLETED,
            "tengu_tool_exit_plan_mode_completed"
        );
        assert_eq!(EXIT_PLAN_MODE_FAILED, "tengu_tool_exit_plan_mode_failed");
    }

    #[test]
    fn enter_worktree_constants_are_locked() {
        assert_eq!(ENTER_WORKTREE_STARTED, "tengu_tool_enter_worktree_started");
        assert_eq!(
            ENTER_WORKTREE_COMPLETED,
            "tengu_tool_enter_worktree_completed"
        );
        assert_eq!(ENTER_WORKTREE_FAILED, "tengu_tool_enter_worktree_failed");
    }

    #[test]
    fn exit_worktree_constants_are_locked() {
        assert_eq!(EXIT_WORKTREE_STARTED, "tengu_tool_exit_worktree_started");
        assert_eq!(
            EXIT_WORKTREE_COMPLETED,
            "tengu_tool_exit_worktree_completed"
        );
        assert_eq!(EXIT_WORKTREE_FAILED, "tengu_tool_exit_worktree_failed");
    }

    #[test]
    fn names_array_contains_all_15_workflow_events() {
        let workflow = [
            TODO_WRITE_STARTED,
            TODO_WRITE_COMPLETED,
            TODO_WRITE_FAILED,
            ENTER_PLAN_MODE_STARTED,
            ENTER_PLAN_MODE_COMPLETED,
            ENTER_PLAN_MODE_FAILED,
            EXIT_PLAN_MODE_STARTED,
            EXIT_PLAN_MODE_COMPLETED,
            EXIT_PLAN_MODE_FAILED,
            ENTER_WORKTREE_STARTED,
            ENTER_WORKTREE_COMPLETED,
            ENTER_WORKTREE_FAILED,
            EXIT_WORKTREE_STARTED,
            EXIT_WORKTREE_COMPLETED,
            EXIT_WORKTREE_FAILED,
        ];
        for name in workflow {
            assert!(
                NAMES.contains(&name),
                "NAMES array missing workflow event: {name}"
            );
        }
        assert_eq!(
            NAMES.len(),
            97,
            "M3-06 40 + M4-02 9 + M4-03 3 + M4-04 15 + M4-05 24 + M4-06 6 = 97"
        );
    }
}

#[cfg(test)]
mod m4_05_agent_task_event_tests {
    use super::*;

    #[test]
    fn agent_constants_are_locked() {
        assert_eq!(AGENT_STARTED, "tengu_tool_agent_started");
        assert_eq!(AGENT_COMPLETED_M4_05, "tengu_tool_agent_completed");
        assert_eq!(AGENT_FAILED, "tengu_tool_agent_failed");
    }

    #[test]
    fn task_create_constants_are_locked() {
        assert_eq!(TASK_CREATE_STARTED, "tengu_tool_task_create_started");
        assert_eq!(TASK_CREATE_COMPLETED, "tengu_tool_task_create_completed");
        assert_eq!(TASK_CREATE_FAILED, "tengu_tool_task_create_failed");
    }

    #[test]
    fn task_get_list_update_stop_output_constants_are_locked() {
        assert_eq!(TASK_GET_STARTED, "tengu_tool_task_get_started");
        assert_eq!(TASK_GET_COMPLETED, "tengu_tool_task_get_completed");
        assert_eq!(TASK_GET_FAILED, "tengu_tool_task_get_failed");
        assert_eq!(TASK_LIST_STARTED, "tengu_tool_task_list_started");
        assert_eq!(TASK_LIST_COMPLETED, "tengu_tool_task_list_completed");
        assert_eq!(TASK_LIST_FAILED, "tengu_tool_task_list_failed");
        assert_eq!(TASK_UPDATE_STARTED, "tengu_tool_task_update_started");
        assert_eq!(TASK_UPDATE_COMPLETED, "tengu_tool_task_update_completed");
        assert_eq!(TASK_UPDATE_FAILED, "tengu_tool_task_update_failed");
        assert_eq!(TASK_STOP_STARTED, "tengu_tool_task_stop_started");
        assert_eq!(TASK_STOP_COMPLETED, "tengu_tool_task_stop_completed");
        assert_eq!(TASK_STOP_FAILED, "tengu_tool_task_stop_failed");
        assert_eq!(TASK_OUTPUT_STARTED, "tengu_tool_task_output_started");
        assert_eq!(TASK_OUTPUT_COMPLETED, "tengu_tool_task_output_completed");
        assert_eq!(TASK_OUTPUT_FAILED, "tengu_tool_task_output_failed");
    }

    #[test]
    fn send_message_constants_are_locked() {
        assert_eq!(SEND_MESSAGE_STARTED, "tengu_tool_send_message_started");
        assert_eq!(SEND_MESSAGE_COMPLETED, "tengu_tool_send_message_completed");
        assert_eq!(SEND_MESSAGE_FAILED, "tengu_tool_send_message_failed");
    }

    #[test]
    fn names_array_contains_all_24_agent_task_events() {
        let agent_task = [
            AGENT_STARTED,
            AGENT_COMPLETED_M4_05,
            AGENT_FAILED,
            TASK_CREATE_STARTED,
            TASK_CREATE_COMPLETED,
            TASK_CREATE_FAILED,
            TASK_GET_STARTED,
            TASK_GET_COMPLETED,
            TASK_GET_FAILED,
            TASK_LIST_STARTED,
            TASK_LIST_COMPLETED,
            TASK_LIST_FAILED,
            TASK_UPDATE_STARTED,
            TASK_UPDATE_COMPLETED,
            TASK_UPDATE_FAILED,
            TASK_STOP_STARTED,
            TASK_STOP_COMPLETED,
            TASK_STOP_FAILED,
            TASK_OUTPUT_STARTED,
            TASK_OUTPUT_COMPLETED,
            TASK_OUTPUT_FAILED,
            SEND_MESSAGE_STARTED,
            SEND_MESSAGE_COMPLETED,
            SEND_MESSAGE_FAILED,
        ];
        for name in agent_task {
            assert!(
                NAMES.contains(&name),
                "NAMES array missing M4-05 event: {name}"
            );
        }
        // Distinctness across the entire NAMES array.
        let mut seen = std::collections::HashSet::new();
        for n in NAMES {
            assert!(seen.insert(*n), "duplicate event name in NAMES: {n}");
        }
    }
}

#[cfg(test)]
mod m4_06_team_event_tests {
    use super::*;

    #[test]
    fn team_constants_are_locked() {
        assert_eq!(TEAM_CREATE_STARTED, "tengu_tool_team_create_started");
        assert_eq!(TEAM_CREATE_COMPLETED, "tengu_tool_team_create_completed");
        assert_eq!(TEAM_CREATE_FAILED, "tengu_tool_team_create_failed");
        assert_eq!(TEAM_DELETE_STARTED, "tengu_tool_team_delete_started");
        assert_eq!(TEAM_DELETE_COMPLETED, "tengu_tool_team_delete_completed");
        assert_eq!(TEAM_DELETE_FAILED, "tengu_tool_team_delete_failed");
    }

    #[test]
    fn names_array_contains_exactly_6_team_events() {
        let team_events = [
            TEAM_CREATE_STARTED,
            TEAM_CREATE_COMPLETED,
            TEAM_CREATE_FAILED,
            TEAM_DELETE_STARTED,
            TEAM_DELETE_COMPLETED,
            TEAM_DELETE_FAILED,
        ];
        for n in team_events {
            assert!(NAMES.contains(&n), "NAMES missing M4-06 event: {n}");
        }
        let count = NAMES
            .iter()
            .filter(|n| n.starts_with("tengu_tool_team_"))
            .count();
        assert_eq!(count, 6, "expected exactly 6 tengu_tool_team_* events");
    }
}

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
