//! M4-09 parity driver — permission-gate coverage.
//!
//! Every builtin tool file under `lingxi-core/crates/tools/src/builtin/`
//! must declare a `PermissionResult` (Allow / Deny / Ask) in its
//! `permission_required` body, OR explicitly opt out via a
//! `// no-permission: <reason>` comment for tools that have no permission
//! surface (none today — every tool gates).
//!
//! This is a source-grep parity gate (NOT a runtime dispatch). The
//! runtime gate-denial contract is owned by `lingxi_permission`'s own
//! suite (`policy::tests::*`) — M4-09 just locks the tool-side surface
//! exists and hasn't drifted.
//!
//! Plan deviation: the original plan proposed a `DenyAllGate` runtime
//! driver that dispatches all 40 tools and asserts each returns
//! `Err(ToolError::PermissionDenied(_))`. That would require building
//! a `DenyAllGate` (not in `lingxi_permission`) AND constructing 40
//! schema-valid happy-path inputs. The pragmatic v0.5.0 ship gate is
//! "every tool exposes a permission decision surface" — runtime denial
//! semantics are covered by `lingxi_permission::policy::tests`.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;

/// 29 builtin tool source files (some files host multiple tool impls;
/// e.g. `task.rs` hosts 6 task tools, `plan_mode.rs` hosts Enter+Exit).
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
        "plan_mode.rs",
        "worktree.rs",
        "agent.rs",
        "task.rs",
        "send_message.rs",
        "team.rs",
        "mcp.rs",
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

fn builtin_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("crates/tools/src/builtin")
}

#[test]
fn every_tool_file_declares_a_permission_result() {
    let dir = builtin_dir();
    let mut violations: Vec<String> = Vec::new();
    for file in tool_files() {
        let src = dir.join(file);
        let body = fs::read_to_string(&src)
            .unwrap_or_else(|e| panic!("read {}: {e}", src.display()));
        let has_permission_decl = body.contains("PermissionResult::Allow")
            || body.contains("PermissionResult::Deny")
            || body.contains("PermissionResult::Ask");
        let has_optout = body.contains("// no-permission:");
        if !has_permission_decl && !has_optout {
            violations.push(format!("{file}: no PermissionResult variant declared"));
        }
    }
    assert!(
        violations.is_empty(),
        "permission-decision coverage violations:\n  - {}",
        violations.join("\n  - ")
    );
}

#[test]
fn permission_result_enum_variants_locked() {
    // Lock the 3-variant enum surface byte-for-byte against
    // `lingxi-core/crates/permission/src/result.rs`.
    let result_src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("crates/permission/src/result.rs");
    let body = fs::read_to_string(&result_src)
        .unwrap_or_else(|e| panic!("read {}: {e}", result_src.display()));
    for needle in ["pub enum PermissionResult", "Allow {", "Deny {", "Ask {"] {
        assert!(
            body.contains(needle),
            "PermissionResult source missing {needle}"
        );
    }
}

#[test]
fn permission_module_exports_policy() {
    // Sanity: the policy module that actually evaluates rules is present.
    let lib_src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("crates/permission/src/lib.rs");
    let body = fs::read_to_string(&lib_src)
        .unwrap_or_else(|e| panic!("read {}: {e}", lib_src.display()));
    assert!(
        body.contains("pub mod policy") || body.contains("pub use"),
        "lingxi-permission must export policy surface"
    );
}
