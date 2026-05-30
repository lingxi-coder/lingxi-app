//! End-to-end: `lingxi-cli --help` produces help text with the locked
//! first-line snippets per plan M5-12 T0 step 5.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn help_contains_top_level_doc() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "AI coding assistant — runs a single turn or REPL",
        ));
}

#[test]
fn help_lists_print_flag() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("-p, --print"))
        .stdout(predicate::str::contains(
            "Print mode: exit after first end_turn",
        ));
}

#[test]
fn help_lists_resume_flag() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Resume a previous session by UUID",
        ));
}

#[test]
fn help_lists_model_flag() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Override the active model"));
}

#[test]
fn help_lists_cwd_flag() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Change to this directory"));
}

#[test]
fn help_lists_no_stream_flag() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Disable streaming SSE"));
}

#[test]
fn help_lists_json_flag() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Emit machine-readable NDJSON"));
}

#[test]
fn help_lists_debug_flag() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Enable verbose logging"));
}

#[test]
fn version_flag_exits_zero() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("lingxi-cli "));
}
