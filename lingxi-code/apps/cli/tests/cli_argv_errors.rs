//! Exit-code coverage for argv parse errors + cwd validation + REPL entry.
//!
//! Locks per plan M5-12 Task 0 step 2.  M5-13 updated the REPL stub to real
//! behaviour: an empty stdin now exits 0 (EOF) instead of 64 (`NOT_IMPLEMENTED`).

use assert_cmd::Command;
use predicates::prelude::*;

/// Byte-parity with claude-code/commander: every usage error (unknown flag,
/// invalid choice, missing arg, cross-flag gate) exits 1, NOT the BSD
/// `EX_USAGE` 2. Flipped 2026-06-25 (ARGV_ERROR 2 → 1).
#[test]
fn unknown_flag_exits_1() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--nonexistent-flag")
        .assert()
        .code(1);
}

#[test]
fn cwd_to_nonexistent_path_exits_1() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .args(["--cwd", "/this/does/not/exist/at/all/zzz"])
        .arg("hello")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("--cwd path does not exist"));
}

/// M5-13: the REPL is real now.  Empty stdin → EOF → exits 0.
/// The `"> "` prompt appears on stderr; stdout gets the EOF newline.
#[test]
fn empty_prompt_with_no_resume_enters_repl_and_exits_0_on_eof() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-ant-test-fake")
        .write_stdin("") // immediate EOF
        .assert()
        .code(0)
        .stderr(predicate::str::contains("> "));
}

#[test]
fn resume_with_invalid_uuid_exits_1() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .args(["--resume", "not-a-uuid"])
        .assert()
        .code(1);
}

/// M7-12: `--resume` with no id is now wired. Under `--no-tui` it routes to
/// the M5-08 stdio picker (deterministic regardless of CI TTY state); with no
/// sessions in the cwd's project dir the picker prints the locked
/// "No conversations found to resume." line and exits 0. The test runs in a
/// fresh temp `$LINGXI_CONFIG_DIR` so the loader sees an empty project dir.
#[test]
fn resume_without_value_no_tui_empty_dir_exits_0() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .env("LINGXI_CONFIG_DIR", tmp.path())
        .args(["--resume", "--no-tui"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "No conversations found to resume.",
        ));
}

/// (M3 cc2.1.198) `--bg`/`--background` × `--print`/`-p` is rejected UP FRONT:
/// one byte-locked line on stderr (no `Error:` prefix — the binary's
/// `handleBgFlag` writes `${error}\n` directly), exit 1 (`process.exitCode=1`).
/// Note the em dash. The reject fires before any runtime/session work, so no
/// API key / config dir is needed.
#[test]
fn bg_with_print_rejected_up_front_exits_1() {
    let locked = "--bg and --print conflict: --print never starts the interactive session that `claude agents` attaches to, so the job would be unattachable. The prompt is the positional \u{2014} drop --print: `claude --bg '<task>'`.";
    for args in [
        vec!["--bg", "--print", "task"],
        vec!["--background", "-p", "task"],
    ] {
        Command::cargo_bin("lingxi-cli")
            .unwrap()
            .args(&args)
            .assert()
            .code(1)
            .stderr(predicate::eq(format!("{locked}\n")));
    }
}

/// (M3 cc2.1.198) `--no-session-persistence` outside `--print` hard-errors with
/// the byte-locked line (`Es("Error: --no-session-persistence can only be used
/// with --print mode.")`) and exit 1, before any turn runs.
#[test]
fn no_session_persistence_without_print_exits_1() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .args(["--no-session-persistence", "hi"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "Error: --no-session-persistence can only be used with --print mode.",
        ));
}
