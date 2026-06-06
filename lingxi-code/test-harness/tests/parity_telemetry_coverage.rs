//! M4-09 parity driver — every builtin tool has its 3 lifecycle events
//! (`tengu_tool_<snake>_{started,completed,failed}`) registered in
//! `telemetry::tengu::tool::NAMES`.
//!
//! This is a registration-coverage assertion (NOT a runtime dispatch test).
//! Each of the 40 tools is mapped here to its byte-locked snake name (the
//! same snake suffix used in production telemetry constants). The driver
//! looks each `tengu_tool_<snake>_{started,completed,failed}` string up in
//! `tengu::tool::NAMES` and fails fast if any are missing.
//!
//! Cardinality note: `NAMES.len()` is 134 (not 120). The 14 extra entries
//! are M3-06 baseline events (`tengu_tool_started`/`completed`/`failed`/
//! `cancelled`, 4 permission events, plus a handful of tool events that
//! pre-shipped in M3-06: `web_fetch_*`, `mcp_completed`/`failed`,
//! `task_dispatched`/`completed`/`failed`, `skill_invoked`). The
//! per-tool 3-event coverage requirement still holds on top of that
//! baseline.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use telemetry::tengu::ALL_EVENT_NAMES;

/// All tool-prefixed events in `ALL_EVENT_NAMES`. Mirrors what
/// `tengu::tool::NAMES` would expose if it were `pub`.
fn tool_event_names() -> Vec<&'static str> {
    ALL_EVENT_NAMES
        .iter()
        .copied()
        .filter(|n| n.starts_with("tengu_tool_"))
        .collect()
}

/// (tool display-name, telemetry snake suffix).
const TOOL_SNAKE: &[(&str, &str)] = &[
    // File (5)
    ("Read", "read"),
    ("Write", "write"),
    ("Edit", "edit"),
    ("NotebookEdit", "notebook"),
    ("Glob", "glob"),
    // Search (1)
    ("Grep", "grep"),
    // Shell (4)
    ("Bash", "bash"),
    ("PowerShell", "powershell"),
    ("REPL", "repl"),
    ("Sleep", "sleep"),
    // Web (2)
    ("WebFetch", "web_fetch"),
    ("WebSearch", "web_search"),
    // Workflow (5)
    ("TodoWrite", "todo_write"),
    ("EnterPlanMode", "enter_plan_mode"),
    ("ExitPlanMode", "exit_plan_mode"),
    ("EnterWorktree", "enter_worktree"),
    ("ExitWorktree", "exit_worktree"),
    // Agent + Task (8)
    ("Agent", "agent"),
    ("TaskCreate", "task_create"),
    ("TaskGet", "task_get"),
    ("TaskList", "task_list"),
    ("TaskUpdate", "task_update"),
    ("TaskStop", "task_stop"),
    ("TaskOutput", "task_output"),
    ("SendMessage", "send_message"),
    // Team (2)
    ("TeamCreate", "team_create"),
    ("TeamDelete", "team_delete"),
    // MCP + LSP (5)
    ("MCP", "mcp"),
    ("McpAuth", "mcp_auth"),
    ("ListMcpResourcesTool", "list_mcp_resources"),
    ("ReadMcpResourceTool", "read_mcp_resource"),
    ("LSP", "lsp"),
    // System (10)
    ("AskUserQuestion", "ask_user_question"),
    ("SendUserMessage", "brief"),
    ("Config", "config"),
    ("Skill", "skill"),
    ("CronCreate", "schedule_cron"),
    ("CronDelete", "cron_delete"),
    ("CronList", "cron_list"),
    ("ToolSearch", "tool_search"),
    ("RemoteTrigger", "remote_trigger"),
    ("StructuredOutput", "synthetic_output"),
];

#[test]
fn snake_table_covers_42_tools() {
    assert_eq!(
        TOOL_SNAKE.len(),
        42,
        "telemetry snake table must cover 42 tools (got {})",
        TOOL_SNAKE.len()
    );
    let unique: BTreeSet<&str> = TOOL_SNAKE.iter().map(|(n, _)| *n).collect();
    assert_eq!(unique.len(), 42, "tool display-names unique");
    let unique_snakes: BTreeSet<&str> = TOOL_SNAKE.iter().map(|(_, s)| *s).collect();
    assert_eq!(unique_snakes.len(), 42, "snake suffixes unique");
}

#[test]
fn every_tool_has_three_registered_events() {
    let registered: BTreeSet<&str> = tool_event_names().into_iter().collect();
    let mut missing: Vec<String> = Vec::new();
    for (display, snake) in TOOL_SNAKE {
        for suffix in ["started", "completed", "failed"] {
            let expected = format!("tengu_tool_{snake}_{suffix}");
            if !registered.contains(expected.as_str()) {
                missing.push(format!("{display}: {expected}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "tengu::tool::NAMES missing the following per-tool events:\n  - {}",
        missing.join("\n  - ")
    );
}

#[test]
fn names_cardinality_locked() {
    // M3-06 baseline + M4-02..08 deltas land at 134 entries.
    // (Plan-stated target was 120 = 3 × 40; reconciled to actual 134
    // because M3-06 pre-shipped some tool events that M4 sub-plans
    // counted as "their" additions. See M4-09 plan reconciliation note.)
    let count = tool_event_names().len();
    assert_eq!(
        count, 140,
        "tengu_tool_* events in ALL_EVENT_NAMES locked at 140 entries (134 + CronDelete/CronList 6)"
    );
}

#[test]
fn completed_suffix_lock_no_succeeded_drift() {
    // Tool events must use `_completed`, never `_succeeded`. (Non-tool
    // subsystems like api use `_succeeded` by design — only the
    // `tengu_tool_*` namespace is locked here.)
    for n in tool_event_names() {
        assert!(
            !n.contains("_succeeded"),
            "found _succeeded suffix in tool event {n}; must use _completed"
        );
    }
}
