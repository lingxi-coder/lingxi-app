//! Exit-code coverage for `--resume <ID>` flows.
//!
//! Full session-file replay requires a JSONL session on disk + the
//! orchestrator's `with_resume` constructor (M5-08) wired through —
//! deferred to M5-13. The tests below pin the surface behaviour the
//! M5-12 CLI is responsible for: argv parsing, UUID validation, and
//! the locked exit codes.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn resume_with_valid_uuid_and_no_prompt_exits_64() {
    // Valid UUID syntax → progresses past validation → reports
    // "REPL not yet wired" and exits 64 (NOT_IMPLEMENTED).
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .args(["--resume", "00000000-0000-0000-0000-000000000002"])
        .assert()
        .code(64)
        .stderr(predicate::str::contains("resumed; REPL not yet wired"));
}

#[test]
fn resume_with_invalid_uuid_string_exits_1() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .args(["--resume", "garbage"])
        .assert()
        .code(1);
}
