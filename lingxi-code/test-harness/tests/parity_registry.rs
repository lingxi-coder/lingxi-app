//! M4-09 parity driver — locks v0.5.0 registry cardinality.
//!
//! Asserts that the fixture `registry_40_tools.json` declares exactly 40
//! tools across 9 categories AND that each declared name is a known
//! `*_TOOL_NAME` constant in production (`tools::builtin`).
//!
//! This is a byte-lock parity driver in the M4-01..08 style: it does NOT
//! dispatch tools at runtime (that machinery lives behind `dummy_ctx` in
//! the `lingxi-tools` crate's `#[cfg(test)]` block — see
//! `register_all_inserts_forty_tools_after_m4_08`). Instead, this driver
//! cross-checks the fixture (the canonical 40-name list shipped with
//! v0.5.0) against the per-tool `TOOL_NAME` constants exposed by each
//! builtin module.

#![allow(clippy::unwrap_used)]

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use test_harness::parity::load_fixture;

#[derive(Deserialize)]
struct Fixture {
    total: usize,
    by_category: BTreeMap<String, Vec<String>>,
}

fn fx() -> Fixture {
    load_fixture::<Fixture>("registry_40_tools")
}

#[test]
fn registry_40_tools_fixture_totals_to_40() {
    let f = fx();
    assert_eq!(f.total, 40, "fixture declares total=40");
    let summed: usize = f.by_category.values().map(Vec::len).sum();
    assert_eq!(
        summed, 40,
        "sum of by_category lengths is 40 (got {summed})"
    );
    let unique: BTreeSet<&String> = f.by_category.values().flat_map(|v| v.iter()).collect();
    assert_eq!(unique.len(), 40, "by_category names are unique");
}

#[test]
fn registry_40_tools_fixture_categories_locked() {
    let f = fx();
    let cats: BTreeSet<&str> = f.by_category.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = [
        "file", "search", "shell", "web", "workflow", "agent", "team", "mcp_lsp", "system",
    ]
    .into_iter()
    .collect();
    assert_eq!(cats, expected, "9-category lock");

    let lens: BTreeMap<&str, usize> = f
        .by_category
        .iter()
        .map(|(k, v)| (k.as_str(), v.len()))
        .collect();
    assert_eq!(lens["file"], 5);
    assert_eq!(lens["search"], 1);
    assert_eq!(lens["shell"], 4);
    assert_eq!(lens["web"], 2);
    assert_eq!(lens["workflow"], 5);
    assert_eq!(lens["agent"], 8);
    assert_eq!(lens["team"], 2);
    assert_eq!(lens["mcp_lsp"], 5);
    assert_eq!(lens["system"], 8);
}

/// Cross-check the fixture's 40 names against the production
/// `*_TOOL_NAME` constants (when an explicit named constant exists) and
/// the in-module `TOOL_NAME` constants (single-tool modules).
#[test]
fn fixture_names_match_production_constants() {
    use tools::builtin;

    let f = fx();
    let names: BTreeSet<String> = f
        .by_category
        .values()
        .flat_map(|v| v.iter().cloned())
        .collect();

    // Each of the 40 names below must appear as a `pub const … &str`
    // in its module — byte-for-byte equal to the fixture entry.
    let pairs: &[(&str, &str)] = &[
        // File (5)
        ("Read", builtin::file_read::TOOL_NAME),
        ("Write", builtin::file_write::TOOL_NAME),
        ("Edit", builtin::file_edit::TOOL_NAME),
        ("NotebookEdit", builtin::notebook_edit::TOOL_NAME),
        ("Glob", builtin::glob::TOOL_NAME),
        // Search (1)
        ("Grep", builtin::grep::TOOL_NAME),
        // Shell (4)
        ("Bash", builtin::bash::TOOL_NAME),
        ("PowerShell", builtin::powershell::TOOL_NAME),
        ("REPL", builtin::repl::TOOL_NAME),
        ("Sleep", builtin::sleep::TOOL_NAME),
        // Web (2)
        ("WebFetch", builtin::web_fetch::TOOL_NAME),
        ("WebSearch", builtin::web_search::TOOL_NAME),
        // Workflow (5)
        ("TodoWrite", builtin::todo_write::TOOL_NAME),
        ("EnterPlanMode", builtin::plan_mode::ENTER_TOOL_NAME),
        ("ExitPlanMode", builtin::plan_mode::EXIT_TOOL_NAME),
        ("EnterWorktree", builtin::worktree::ENTER_TOOL_NAME),
        ("ExitWorktree", builtin::worktree::EXIT_TOOL_NAME),
        // Agent + Task (8)
        ("Agent", builtin::agent::AGENT_TOOL_NAME),
        ("TaskCreate", builtin::task::TASK_CREATE_TOOL_NAME),
        ("TaskGet", builtin::task::TASK_GET_TOOL_NAME),
        ("TaskList", builtin::task::TASK_LIST_TOOL_NAME),
        ("TaskUpdate", builtin::task::TASK_UPDATE_TOOL_NAME),
        ("TaskStop", builtin::task::TASK_STOP_TOOL_NAME),
        ("TaskOutput", builtin::task::TASK_OUTPUT_TOOL_NAME),
        ("SendMessage", builtin::send_message::SEND_MESSAGE_TOOL_NAME),
        // Team (2)
        ("TeamCreate", builtin::team::TEAM_CREATE_TOOL_NAME),
        ("TeamDelete", builtin::team::TEAM_DELETE_TOOL_NAME),
        // MCP + LSP (5)
        ("MCP", builtin::mcp::MCP_TOOL_NAME),
        ("McpAuth", builtin::mcp::MCP_AUTH_TOOL_NAME),
        (
            "ListMcpResources",
            builtin::mcp::LIST_MCP_RESOURCES_TOOL_NAME,
        ),
        ("ReadMcpResource", builtin::mcp::READ_MCP_RESOURCE_TOOL_NAME),
        ("LSP", builtin::lsp::LSP_TOOL_NAME),
        // System (8)
        (
            "AskUserQuestion",
            builtin::ask_user_question::ASK_USER_QUESTION_TOOL_NAME,
        ),
        ("Brief", builtin::brief::BRIEF_TOOL_NAME),
        ("Config", builtin::config::CONFIG_TOOL_NAME),
        ("Skill", builtin::skill::SKILL_TOOL_NAME),
        (
            "ScheduleCron",
            builtin::schedule_cron::SCHEDULE_CRON_TOOL_NAME,
        ),
        ("ToolSearch", builtin::tool_search::TOOL_SEARCH_TOOL_NAME),
        (
            "RemoteTrigger",
            builtin::remote_trigger::REMOTE_TRIGGER_TOOL_NAME,
        ),
        (
            "SyntheticOutput",
            builtin::synthetic_output::SYNTHETIC_OUTPUT_TOOL_NAME,
        ),
    ];

    assert_eq!(pairs.len(), 40, "production-constant lock covers 40 tools");

    for (fixture_name, production_const) in pairs {
        assert_eq!(
            production_const, fixture_name,
            "tool {fixture_name}: production constant ({production_const}) diverges from fixture"
        );
        assert!(
            names.contains(*fixture_name),
            "fixture missing expected name {fixture_name}"
        );
    }

    // Final sanity: the 40 paired names exactly match the fixture set.
    let paired: BTreeSet<String> = pairs.iter().map(|(n, _)| (*n).to_string()).collect();
    assert_eq!(paired, names, "fixture and production-constant sets agree");
}
