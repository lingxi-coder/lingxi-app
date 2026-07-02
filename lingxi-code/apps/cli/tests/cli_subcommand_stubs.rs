//! (M4 cc2.1.198) Exit-code + byte locks for the `gateway` subcommand surface
//! and the FIXED `ultrareview` unsupported behavior.
//!
//! `gateway` oracle (verified live against the real 2.1.198 binary):
//!   * `claude gateway --help`            → fixture text, exit 0;
//!   * `claude gateway`                   → `error: required option '--config
//!     <path>' not specified` on stderr, exit 1;
//!   * `claude gateway --config /missing` → `claude gateway: ENOENT: no such
//!     file or directory, open '/missing'`, exit 1;
//!   * with a readable config the binary starts the enterprise gateway —
//!     lingxi-cli reports it unsupported instead (exit `NOT_IMPLEMENTED`).

use assert_cmd::Command;
use predicates::prelude::*;

/// The captured `claude gateway --help` fixture — the same bytes
/// `test-harness/tests/parity_claude_2_1_198.rs` pins from the fixture side.
const GATEWAY_HELP_FIXTURE: &str =
    include_str!("../../../test-harness/src/parity/fixtures/cc_2_1_198_gateway_help.txt");

/// `gateway --help` (and `-h`) prints the fixture BYTE-FOR-BYTE, exit 0.
#[test]
fn gateway_help_matches_fixture_byte_for_byte() {
    for flag in ["--help", "-h"] {
        Command::cargo_bin("lingxi-cli")
            .unwrap()
            .args(["gateway", flag])
            .assert()
            .code(0)
            .stdout(predicate::eq(GATEWAY_HELP_FIXTURE));
    }
}

/// Bare `gateway` → commander's missing-required-option line, exit 1
/// (byte-verified live: `error: required option '--config <path>' not
/// specified`).
#[test]
fn gateway_without_config_matches_commander_error() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("gateway")
        .assert()
        .code(1)
        .stderr(predicate::eq(
            "error: required option '--config <path>' not specified\n",
        ));
}

/// `gateway --config <missing>` → the ENOENT line (binary prefix `claude
/// gateway:` → branded `lingxi-cli gateway:`), exit 1.
#[test]
fn gateway_with_missing_config_reports_enoent() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .args(["gateway", "--config", "/this/does/not/exist/gw.yaml"])
        .assert()
        .code(1)
        .stderr(predicate::eq(
            "lingxi-cli gateway: ENOENT: no such file or directory, open '/this/does/not/exist/gw.yaml'\n",
        ));
}

/// `gateway --config <existing>` → clear unsupported notice, non-zero exit
/// (the enterprise gateway runtime is not part of lingxi-cli); never serves.
#[test]
fn gateway_with_existing_config_reports_unsupported() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("gw.yaml");
    std::fs::write(&cfg, "").unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .args(["gateway", "--config"])
        .arg(&cfg)
        .assert()
        .code(64) // exit_codes::NOT_IMPLEMENTED
        .stderr(predicate::str::contains(
            "lingxi-cli gateway: the enterprise auth/telemetry gateway is not available in lingxi-cli.",
        ));
}

/// `ultrareview` stays FIXED: unsupported notice on stderr + exit 64
/// (`NOT_IMPLEMENTED`), never a billable turn — locked here so surface drift
/// is caught (cc 2.1.198 M4 item 8).
#[test]
fn ultrareview_unsupported_is_stable() {
    for args in [vec!["ultrareview"], vec!["ultrareview", "123", "--json"]] {
        Command::cargo_bin("lingxi-cli")
            .unwrap()
            .args(&args)
            .assert()
            .code(64) // exit_codes::NOT_IMPLEMENTED
            .stderr(predicate::str::contains(
                "lingxi-cli ultrareview: the cloud-hosted multi-agent reviewer is not available in lingxi-cli.",
            ))
            .stderr(predicate::str::contains(
                "Use the local equivalent instead: the interactive `/code-review ultra` slash command.",
            ));
    }
}
