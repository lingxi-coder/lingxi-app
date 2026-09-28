//! Exit-code coverage for `--resume <ID>` flows.
//!
//! Full session-file replay requires a JSONL session on disk + the
//! orchestrator's `with_resume` constructor (M5-08) wired through —
//! deferred to M5-13. The tests below pin the surface behaviour the
//! M5-12 CLI is responsible for: argv parsing, UUID validation, and
//! the locked exit codes.
//!
//! Every test here gives the child a temp `$LINGXI_CONFIG_DIR`. That is what
//! keeps the spawned binary off the machine's native credential store — the
//! engine reads the credential root and picks the plaintext store for a
//! throwaway home. Without it, macOS's credential broker refuses the
//! unpackaged test binary and every case here dies at startup with
//! "secure storage init failed" instead of exercising what it names.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn resume_with_nonexistent_uuid_errors_with_not_found() {
    // SESSION.4: a syntactically-valid UUID with no session file on disk now
    // verifies existence and errors like claude-code ("No conversation found
    // with session ID: <id>", main.tsx:3681) with exit 1 — instead of falsely
    // reporting success. (Full transcript replay for an existing session is the
    // separately-deferred M5-13 REPL milestone.)
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(["--resume", "00000000-0000-0000-0000-000000000002"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "No conversation found with session ID: 00000000-0000-0000-0000-000000000002",
        ));
}

#[test]
fn resume_with_invalid_uuid_string_exits_1() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(["--resume", "garbage"])
        .assert()
        .code(1);
}
