//! (M7 cc2.1.198) End-to-end locks for the `agents` subcommand surface.
//!
//! Oracle: the real 2.1.198 binary, verified live on 2026-07-02:
//! * `claude agents --help` → the captured fixture, exit 0;
//! * `claude agents --json` → pretty-printed array from the live-session
//!   registry (`sessions/<pid>.json`) + job store (`jobs/<short>/state.json`),
//!   exact key order `{pid?, id?, cwd, kind, startedAt, sessionId, name?,
//!   status?, state?}`, ascending `startedAt`, trailing newline; empty → `[]`;
//! * `claude agents` with non-TTY stdout → `'claude agents' requires an
//!   interactive terminal (stdout is not a TTY) — use 'claude agents --json'
//!   for a machine-readable listing.` on stderr, exit 1 (command name branded
//!   here);
//! * `--json --all` includes completed jobs; default hides workerless
//!   terminal jobs; `--cwd` filters by subtree.

use assert_cmd::Command;
use predicates::prelude::*;

const AGENTS_HELP_FIXTURE: &str =
    include_str!("../../../test-harness/src/parity/fixtures/cc_2_1_198_agents_help.txt");

/// `agents --help` / `-h` print the fixture byte-for-byte, exit 0.
#[test]
fn agents_help_matches_fixture_byte_for_byte() {
    for flag in ["--help", "-h"] {
        Command::cargo_bin("lingxi-cli")
            .unwrap()
            .args(["agents", flag])
            .assert()
            .code(0)
            .stdout(predicate::eq(AGENTS_HELP_FIXTURE));
    }
}

/// `agents --json` with an empty registry prints `[]` + newline, exit 0.
#[test]
fn agents_json_empty_registry_prints_empty_array() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(["agents", "--json"])
        .assert()
        .code(0)
        .stdout(predicate::eq("[]\n"));
}

/// Seed a config home with one live interactive session (this test process's
/// pid — alive while the child CLI runs) and one blocked background job.
fn seed_registry(home: &std::path::Path) -> i64 {
    let pid = std::process::id();
    let sessions = home.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{pid}.json")),
        format!(
            r#"{{"pid":{pid},"sessionId":"11111111-2222-3333-4444-555555555555","cwd":"/tmp/proj","startedAt":2000,"kind":"interactive","name":"proj","status":"busy"}}"#
        ),
    )
    .unwrap();
    let job = home.join("jobs/ad612c16");
    std::fs::create_dir_all(&job).unwrap();
    std::fs::write(
        job.join("state.json"),
        r#"{"state":"blocked","tempo":"blocked","name":"workflow byte-level alignment","sessionId":"ad612c16-1cb9-42fc-af8b-0805c55b9072","cwd":"/tmp/proj","createdAt":"1970-01-01T00:00:01.000Z"}"#,
    )
    .unwrap();
    i64::from(pid)
}

/// Populated registry: job row first (startedAt 1000 < 2000), binary key
/// order, byte-exact pretty-printed output.
#[test]
fn agents_json_lists_jobs_and_live_sessions_in_binary_shape() {
    let home = tempfile::tempdir().unwrap();
    let pid = seed_registry(home.path());
    let expected = format!(
        r#"[
  {{
    "id": "ad612c16",
    "cwd": "/tmp/proj",
    "kind": "background",
    "startedAt": 1000,
    "sessionId": "ad612c16-1cb9-42fc-af8b-0805c55b9072",
    "name": "workflow byte-level alignment",
    "state": "blocked"
  }},
  {{
    "pid": {pid},
    "cwd": "/tmp/proj",
    "kind": "interactive",
    "startedAt": 2000,
    "sessionId": "11111111-2222-3333-4444-555555555555",
    "name": "proj",
    "status": "busy"
  }}
]
"#
    );
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(["agents", "--json"])
        .assert()
        .code(0)
        .stdout(predicate::eq(expected));
}

/// A workerless terminal job is hidden by default and shown with `--all`
/// (`--all`: "include completed sessions (the full agent view list)").
#[test]
fn agents_json_all_includes_completed_jobs() {
    let home = tempfile::tempdir().unwrap();
    let job = home.path().join("jobs/bb00cc11");
    std::fs::create_dir_all(&job).unwrap();
    std::fs::write(
        job.join("state.json"),
        r#"{"state":"done","tempo":"idle","name":"finished thing","sessionId":"bb00cc11-0000-0000-0000-000000000000","cwd":"/tmp/proj","createdAt":"1970-01-01T00:00:05.000Z"}"#,
    )
    .unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(["agents", "--json"])
        .assert()
        .code(0)
        .stdout(predicate::eq("[]\n"));
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(["agents", "--json", "--all"])
        .assert()
        .code(0)
        .stdout(
            predicate::str::contains("\"state\": \"done\"")
                .and(predicate::str::contains("\"id\": \"bb00cc11\"")),
        );
}

/// `--cwd <path>` keeps only sessions under that subtree; a non-matching
/// filter prints `[]` (verified live: `claude agents --json --cwd /nonexistent`
/// → `[]`, exit 0).
#[test]
fn agents_json_cwd_filters_by_subtree() {
    let home = tempfile::tempdir().unwrap();
    seed_registry(home.path());
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(["agents", "--json", "--cwd", "/nonexistent-dir-xyz"])
        .assert()
        .code(0)
        .stdout(predicate::eq("[]\n"));
}

/// A dead pid's registration is ignored (and reaped) — the reader's liveness
/// probe, mirroring the binary's `bKe()` reaper.
#[test]
fn agents_json_skips_dead_pid_registrations() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    // A pid that cannot exist on test hosts (beyond pid_max).
    std::fs::write(
        sessions.join("2147483000.json"),
        r#"{"pid":2147483000,"sessionId":"dead-dead","cwd":"/tmp/proj","startedAt":1,"kind":"interactive"}"#,
    )
    .unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(["agents", "--json"])
        .assert()
        .code(0)
        .stdout(predicate::eq("[]\n"));
}

/// `agents` without `--json` and with a non-TTY stdout refuses with the
/// binary's exact message (branded command name) on stderr, exit 1.
#[test]
fn agents_without_json_non_tty_refuses() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("LINGXI_CONFIG_DIR", home.path())
        .arg("agents")
        .assert()
        .code(1)
        .stderr(predicate::eq(
            "'lingxi-cli agents' requires an interactive terminal (stdout is not a TTY) \u{2014} use 'lingxi-cli agents --json' for a machine-readable listing.\n",
        ))
        .stdout(predicate::eq(""));
}

/// The bypass flags still hit the non-TTY refusal first (the disclaimer is a
/// TTY dialog on the interactive path — binary order: json → TTY mount →
/// refusal).
#[test]
fn agents_bypass_flags_non_tty_still_refuse() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(["agents", "--dangerously-skip-permissions"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("requires an interactive terminal"));
}

/// The full fixture flag surface parses (parse-and-carry for
/// dispatch-affecting flags) — `--json` still works with all of them set.
#[test]
fn agents_accepts_full_fixture_flag_surface() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("LINGXI_CONFIG_DIR", home.path())
        .args([
            "agents",
            "--json",
            "--all",
            "--cwd",
            "/tmp",
            "--agent",
            "reviewer",
            "--model",
            "opus",
            "--effort",
            "high",
            "--add-dir",
            "/tmp/a",
            "--add-dir",
            "/tmp/b",
            "--mcp-config",
            "{}",
            "--permission-mode",
            "plan",
            "--plugin-dir",
            "/tmp/p",
            "--setting-sources",
            "user,project",
            "--settings",
            "{}",
            "--strict-mcp-config",
            "--allow-dangerously-skip-permissions",
        ])
        .assert()
        .code(0)
        .stdout(predicate::eq("[]\n"));
}
