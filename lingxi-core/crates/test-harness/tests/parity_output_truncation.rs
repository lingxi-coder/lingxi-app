//! M4-09 parity driver — every builtin tool that produces variable-length
//! output routes it through `lingxi_tools::shared::truncate` (or the
//! tool_trait-level `OutputTruncated` enforcement), with the
//! `MAX_TOOL_OUTPUT_LENGTH = 30_000` and the locked suffix.
//!
//! Two assertions:
//!   1. Constant lock — the two literals are exactly the spec values.
//!   2. Source-level lock — every per-tool source file either references
//!      `MAX_TOOL_OUTPUT_LENGTH` / `truncate` / `truncate_default`, or
//!      carries an explicit `// no-truncation: <reason>` opt-out for tools
//!      whose return is bounded by construction (e.g. `{ ok: bool }`).

#![allow(clippy::unwrap_used)]

use lingxi_tools::shared::{MAX_TOOL_OUTPUT_LENGTH, TRUNCATION_SUFFIX};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

/// Each entry MUST carry a one-line justification.
fn exempt_tools() -> HashSet<&'static str> {
    [
        // Pure side-effect or fixed-shape return:
        "exit_plan_mode",    // returns empty object (handled in plan_mode.rs alongside enter)
        "exit_worktree",     // returns empty object (handled in worktree.rs alongside enter)
        "sleep",             // returns { slept_ms: u64 }
        "schedule_cron",     // returns { next_fire_unix_secs, task_id }
        "task_stop",         // returns { stopped: bool, task_id }
        "task_update",       // returns { task_id, status }
        "team_delete",       // returns { deleted: bool }
        "mcp_auth",          // returns { authenticated: bool }
        "remote_trigger",    // returns { stub: true, ... }
    ]
    .into_iter()
    .collect()
}

/// Mapping: tool display-name → source file under `builtin/`. Some tool
/// modules host multiple display-names (e.g. `task.rs` hosts 6 task tools,
/// `plan_mode.rs` hosts Enter+Exit). The grep is at file granularity.
fn tool_files() -> Vec<&'static str> {
    vec![
        "file_read.rs",
        "file_write.rs",
        "file_edit.rs",
        "notebook_edit.rs",
        "glob.rs",
        "grep.rs",
        "bash.rs",
        "powershell.rs",
        "repl.rs",
        "sleep.rs",
        "web_fetch.rs",
        "web_search.rs",
        "todo_write.rs",
        "plan_mode.rs",  // Enter+Exit
        "worktree.rs",   // Enter+Exit
        "agent.rs",
        "task.rs",       // 6 task tools
        "send_message.rs",
        "team.rs",
        "mcp.rs",        // MCP + McpAuth + ListMcpResources + ReadMcpResource
        "lsp.rs",
        "ask_user_question.rs",
        "brief.rs",
        "config.rs",
        "skill.rs",
        "schedule_cron.rs",
        "tool_search.rs",
        "remote_trigger.rs",
        "synthetic_output.rs",
    ]
}

#[test]
fn output_truncation_constants_locked() {
    assert_eq!(MAX_TOOL_OUTPUT_LENGTH, 30_000, "spec §7: 30_000 chars");
    assert_eq!(
        TRUNCATION_SUFFIX,
        "\n\n[Output truncated due to length]",
        "spec §7 locked suffix"
    );
}

#[test]
fn truncation_lib_source_contains_literals() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let lib_src = repo_root.join("crates/tools/src/shared/output_truncation.rs");
    let body = fs::read_to_string(&lib_src)
        .unwrap_or_else(|e| panic!("read {}: {e}", lib_src.display()));
    assert!(
        body.contains("30_000"),
        "MAX_TOOL_OUTPUT_LENGTH literal 30_000 missing from {}",
        lib_src.display()
    );
    assert!(
        body.contains("[Output truncated due to length]"),
        "TRUNCATION_SUFFIX literal missing from {}",
        lib_src.display()
    );
}

#[test]
fn every_tool_with_output_calls_truncate_or_opts_out() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let builtin_dir = repo_root.join("crates/tools/src/builtin");
    let exempt = exempt_tools();

    let mut violations: Vec<String> = Vec::new();
    for file in tool_files() {
        let src = builtin_dir.join(file);
        let body = fs::read_to_string(&src)
            .unwrap_or_else(|e| panic!("read {}: {e}", src.display()));
        let has_call = body.contains("MAX_TOOL_OUTPUT_LENGTH")
            || body.contains("output_truncation::truncate")
            || body.contains("shared::truncate")
            || body.contains("truncate_default")
            || body.contains("OutputTruncated");
        // Files in the exempt set are allowed without a call (their tools
        // return bounded outputs). Their stem (without `.rs`) appears in
        // `exempt_tools()`.
        let stem = file.trim_end_matches(".rs");
        let is_exempt = exempt.contains(stem);
        let has_optout_marker =
            body.contains("// no-truncation:") || body.contains("//! no-truncation:");
        if !has_call && !is_exempt && !has_optout_marker {
            violations.push(format!(
                "{file}: no truncation call AND not in exempt set AND no `// no-truncation:` opt-out"
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "output-truncation coverage violations:\n  - {}",
        violations.join("\n  - ")
    );
}
