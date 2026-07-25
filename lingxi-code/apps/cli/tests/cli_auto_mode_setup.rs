//! WIZARD-06 end-to-end: drive the real `lingxi-cli auto-mode-setup --apply-file`
//! binary through the whole wired apply path (clap raw-arg capture → hand-rolled
//! grammar → path/read gate → secure read → sha256 hash-verify → parse → validate
//! → atomic settings write on disk), with the config home sandboxed to a temp dir
//! via `LINGXI_CONFIG_DIR`. This is the one thing the unit tests can't cover: the
//! real `run()` assembling its live deps from the environment.

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use sha2::{Digest, Sha256};

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Write `proposal` under the sandboxed config home (a containment root) and
/// return its path + sha256.
fn stage(home: &Path, proposal: &str) -> (std::path::PathBuf, String) {
    let path = home.join("proposal.json");
    std::fs::write(&path, proposal).unwrap();
    (path, sha256_hex(proposal.as_bytes()))
}

/// The happy path: a valid proposal with the correct `--expect-sha256` is applied
/// and the `autoMode` block lands in the sandboxed user `settings.json`.
#[test]
fn apply_file_writes_automode_settings_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    let proposal = r#"{"environment":["Solo dev on a laptop"],"allow":["Bash(ls:*)","$defaults"]}"#;
    let (path, sha) = stage(home.path(), proposal);

    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("LINGXI_CONFIG_DIR", home.path())
        .args([
            "auto-mode-setup",
            "--expect-sha256",
            &sha,
            "--apply-file",
            path.to_str().unwrap(),
        ])
        .assert()
        .code(0);

    let written = std::fs::read_to_string(home.path().join("settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(v["autoMode"]["environment"], serde_json::json!(["Solo dev on a laptop"]));
    assert_eq!(v["autoMode"]["allow"], serde_json::json!(["Bash(ls:*)", "$defaults"]));
}

/// A tampered digest is refused (`hash_mismatch`) and NOTHING is written.
#[test]
fn apply_file_rejects_hash_mismatch_and_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let (path, _sha) = stage(home.path(), r#"{"environment":["x"]}"#);

    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("LINGXI_CONFIG_DIR", home.path())
        .args([
            "auto-mode-setup",
            "--expect-sha256",
            &"f".repeat(64),
            "--apply-file",
            path.to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "do not match the reviewed digest",
        ));

    assert!(!home.path().join("settings.json").exists());
}

/// A flag-grammar error prints the byte-exact message on stderr and exits 1.
#[test]
fn apply_file_missing_path_is_grammar_error() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .args(["auto-mode-setup", "--expect-sha256", &"0".repeat(64), "--apply-file"])
        .assert()
        .code(1)
        .stderr(predicate::eq(
            "--apply-file needs a path to the reviewed proposal JSON.\n",
        ));
}
