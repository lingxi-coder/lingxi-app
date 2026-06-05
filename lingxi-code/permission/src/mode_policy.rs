//! Plan-mode tool policy — the read-only / planning-safe allowlist.
//!
//! Ports the EXTERNAL subset of claude-code's `SAFE_YOLO_ALLOWLISTED_TOOLS`
//! (`classifierDecision.ts:56-98`) — the set of tools the auto-mode classifier
//! treats as safe enough to skip a classifier round-trip because they are
//! read-only or only touch agent-local planning state. The same set is reused
//! here as the Plan-mode mutation-block allowlist: in `Plan` mode every tool
//! NOT on this list is treated as a potential state mutation and asked about
//! (see [`crate::policy::PermissionPolicy::authorize`]).
//!
//! ## Divergence from TS
//! - The TS set is built from per-tool `*_TOOL_NAME` constants imported across
//!   the tool modules. This crate has no dependency on the tools crate (and
//!   must not add one), so the wire names are inlined as string literals that
//!   match the names this port advertises (see
//!   [`crate::defaults_per_tool`]). These now match the TS constant values
//!   byte-for-byte, including the MCP resource tools' `Tool` suffix
//!   (`ListMcpResourcesTool` / `ReadMcpResourceTool`, per
//!   `classifierDecision.ts:56-98`).
//! - The `ant`-only safe tools (`TerminalCapture`, `OverflowTest`,
//!   `VerifyPlanExecution`) and the internal `YoloClassifier` tool are OMITTED
//!   — they are gated behind `USER_TYPE==='ant'` / feature flags in TS and are
//!   absent from external builds, so the external allowlist is byte-faithful
//!   without them.
//! - `Workflow` is conditional on the `WORKFLOW_SCRIPTS` feature in TS and is
//!   not part of this port's tool set, so it is omitted.

/// External read-only / planning-safe allowlist (the `Plan`-mode pass set).
///
/// Mirrors `SAFE_YOLO_ALLOWLISTED_TOOLS` (`classifierDecision.ts:56-94`),
/// external subset. A tool on this list never trips the Plan-mode mutation
/// backstop; everything else does. Read-only file/search operations, MCP
/// resource reads, task/todo metadata, plan-mode UI tools, the team mailbox
/// tools, and `Sleep` are all safe.
const PLAN_SAFE_TOOLS: &[&str] = &[
    // Read-only file operations.
    "Read",
    // Search / read-only.
    "Grep",
    "Glob",
    "LSP",
    "ToolSearch",
    "ListMcpResourcesTool",
    "ReadMcpResourceTool",
    // Task management (metadata only).
    "TodoWrite",
    "TaskCreate",
    "TaskGet",
    "TaskUpdate",
    "TaskList",
    "TaskStop",
    "TaskOutput",
    // Plan mode / UI.
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    // Swarm coordination (internal mailbox / team state only — teammates run
    // their own permission checks, so no actual security bypass).
    "TeamCreate",
    // Agent cleanup.
    "TeamDelete",
    "SendMessage",
    // Misc safe.
    "Sleep",
];

/// Is `tool_name` on the read-only / planning-safe allowlist?
///
/// Used by the Plan-mode backstop in [`crate::policy::PermissionPolicy::authorize`]:
/// a tool that is NOT plan-safe is treated as a state mutation and asked about
/// when no allow rule already matched. Returns `true` for the read-only /
/// planning-safe tools enumerated in [`PLAN_SAFE_TOOLS`], `false` otherwise
/// (mutating tools like `Edit`/`Write`/`Bash`, and unknown tools, fail closed).
#[must_use]
pub fn is_plan_safe_tool(tool_name: &str) -> bool {
    PLAN_SAFE_TOOLS.contains(&tool_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_and_search_tools_are_plan_safe() {
        for t in ["Read", "Grep", "Glob", "LSP", "ToolSearch"] {
            assert!(is_plan_safe_tool(t), "{t} should be plan-safe");
        }
    }

    #[test]
    fn mcp_resource_reads_are_plan_safe() {
        assert!(is_plan_safe_tool("ListMcpResourcesTool"));
        assert!(is_plan_safe_tool("ReadMcpResourceTool"));
    }

    #[test]
    fn task_and_todo_metadata_tools_are_plan_safe() {
        for t in [
            "TodoWrite",
            "TaskCreate",
            "TaskGet",
            "TaskUpdate",
            "TaskList",
            "TaskStop",
            "TaskOutput",
        ] {
            assert!(is_plan_safe_tool(t), "{t} should be plan-safe");
        }
    }

    #[test]
    fn plan_ui_tools_are_plan_safe() {
        assert!(is_plan_safe_tool("AskUserQuestion"));
        assert!(is_plan_safe_tool("EnterPlanMode"));
        assert!(is_plan_safe_tool("ExitPlanMode"));
    }

    #[test]
    fn team_and_send_message_tools_are_plan_safe() {
        assert!(is_plan_safe_tool("TeamCreate"));
        assert!(is_plan_safe_tool("TeamDelete"));
        assert!(is_plan_safe_tool("SendMessage"));
    }

    #[test]
    fn sleep_is_plan_safe() {
        assert!(is_plan_safe_tool("Sleep"));
    }

    #[test]
    fn mutating_tools_are_not_plan_safe() {
        // Editors, shell, and external side-effect tools must trip the block.
        for t in [
            "Edit",
            "Write",
            "NotebookEdit",
            "Bash",
            "PowerShell",
            "WebFetch",
            "WebSearch",
        ] {
            assert!(!is_plan_safe_tool(t), "{t} must NOT be plan-safe");
        }
    }

    #[test]
    fn ant_only_and_classifier_tools_are_omitted() {
        // External builds never advertise these — they must not be plan-safe.
        for t in [
            "TerminalCapture",
            "OverflowTest",
            "VerifyPlanExecution",
            "YoloClassifier",
            "Workflow",
        ] {
            assert!(!is_plan_safe_tool(t), "{t} is ant/feature-gated, omitted");
        }
    }

    #[test]
    fn unknown_tool_is_not_plan_safe() {
        assert!(!is_plan_safe_tool("DoesNotExist"));
        assert!(!is_plan_safe_tool(""));
    }
}
