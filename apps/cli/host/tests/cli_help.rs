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
fn help_documents_the_attach_v2_detach_chord() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        // Clap right-pads every subcommand name to the longest one, so hard-coding
        // the gap makes this test fail whenever a longer subcommand is added
        // (WIZARD-06's 15-char `auto-mode-setup` displaced the 14-char
        // `remote-control` and did exactly that). Assert the name and its
        // description independently — the alignment is clap's business.
        // The subcommand and its description are asserted separately, and the
        // regex tolerates any run of spaces between them: clap right-pads every
        // name to the longest one, so a hard-coded gap breaks whenever a longer
        // subcommand appears (WIZARD-06's 15-char `auto-mode-setup` displaced
        // the 14-char `remote-control` and did exactly that). The alignment is
        // clap's business; what this test owns is that the row exists.
        .stdout(
            predicate::str::is_match(
                r"\battach\s+Open a background session here; Ctrl\+Z returns to the shell",
            )
            .unwrap(),
        );
}

#[test]
fn help_renders_public_aliases_in_the_option_heading() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "--allowedTools, --allowed-tools <tools>...",
        ))
        .stdout(predicate::str::contains(
            "--disallowedTools, --disallowed-tools <tools>...",
        ))
        .stdout(predicate::str::contains("--bg, --background"))
        .stdout(predicate::str::contains("[aliases: allowed-tools]").not())
        .stdout(predicate::str::contains("[aliases: disallowed-tools]").not())
        .stdout(predicate::str::contains("[aliases: background]").not());
}

#[test]
fn help_omits_leaked_engineering_notes() {
    // Regression lock: internal engineering/impl notes (plan-mission tags,
    // binary offsets, reference source paths) must never render in user-facing
    // `--help`. They live as plain `//` comments in argv.rs, not `///` docs.
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("cc2.1.198").not())
        .stdout(predicate::str::contains("RESIDUAL").not())
        .stdout(predicate::str::contains("binary @").not())
        .stdout(predicate::str::contains("main.tsx").not());
}

#[test]
fn help_bare_flag_lists_skipped_subsystems() {
    // FIX 2: the `--bare` help must name the skipped subsystems and the
    // strict-auth clause (2.1.199+ wording, LINGXI_SIMPLE naming).
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("auto-memory"))
        .stdout(predicate::str::contains("keychain"))
        .stdout(predicate::str::contains("LINGXI_SIMPLE=1"));
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
