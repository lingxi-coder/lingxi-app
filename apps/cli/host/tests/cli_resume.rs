//! Exit-code and fixed-Code profile coverage for `--resume <ID>` flows.
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
    // A valid UUID must still resolve to an existing session before startup.
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

#[test]
fn stdio_resume_recovers_chat_then_uses_code() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let cwd = root.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let session_id = uuid::Uuid::new_v4();
    let path = session::jsonl::path::session_path(&home, &cwd.to_string_lossy(), &session_id.to_string());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let rows = [
        serde_json::json!({"type":"user", "uuid":uuid::Uuid::new_v4(), "parentUuid":null,
            "sessionId":session_id, "cwd":cwd, "message":{"role":"user","content":"hello"}}),
        serde_json::json!({"type":"session-mode", "sessionId":session_id, "sessionMode":"chat",
            "contextRevision":2, "profileEpoch":2, "admittedMode":"chat", "admittedEpoch":2, "pending":false}),
    ];
    std::fs::write(&path, rows.iter().map(|row| format!("{row}\n")).collect::<String>()).unwrap();
    Command::cargo_bin("lingxi-cli").unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .env("LINGXI_CONFIG_DIR", &home)
        .current_dir(&cwd)
        .args(["--no-tui", "--resume", &session_id.to_string()])
        .write_stdin("/context manifest\n/exit\n")
        .assert().success()
        .stdout(predicate::str::contains("\"active\": \"code\""));
}
