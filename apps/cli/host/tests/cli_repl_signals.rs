//! Signal-driven REPL tests.  Unix-only because Windows lacks portable
//! signal injection in stable Rust.
//!
//! These tests are marked `#[ignore]` — run explicitly with:
//!   `cargo test -p lingxi-cli --test cli_repl_signals -- --ignored`
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` Task 7.

#![cfg(unix)]
// `libc::kill` is the only portable way to send SIGINT from a test to a
// child process on Unix.  The workspace lint is `deny`, not `forbid`, so we
// can allow it locally in this test file.
#![allow(unsafe_code)]

use std::io::Write;
use std::process::{Command, Stdio};

/// Helper: spawn lingxi-cli in REPL mode.
fn spawn_repl() -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
        .env("ANTHROPIC_API_KEY", "sk-ant-test-signals-fake")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn lingxi-cli")
}

#[test]
#[ignore = "signal-driven; slow + may be flaky in CI. Run explicitly with --ignored."]
fn double_sigint_at_idle_exits_130() {
    let mut child = spawn_repl();

    // Allow the binary to reach the `"> "` prompt.
    std::thread::sleep(std::time::Duration::from_millis(500));

    let pid = libc::pid_t::try_from(child.id()).expect("pid overflowed i32");

    // First SIGINT — arms the idle flag.
    unsafe {
        libc::kill(pid, libc::SIGINT);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Second SIGINT within the 2-second window — should exit 130.
    unsafe {
        libc::kill(pid, libc::SIGINT);
    }

    let status = child.wait().expect("wait failed");
    assert_eq!(
        status.code(),
        Some(130),
        "expected exit code 130 after double SIGINT, got {:?}",
        status.code()
    );
}

#[test]
#[ignore = "signal-driven; see comment on `double_sigint_at_idle_exits_130`."]
fn single_sigint_does_not_exit() {
    let mut child = spawn_repl();

    // Allow binary to settle at the prompt.
    std::thread::sleep(std::time::Duration::from_millis(500));

    let pid = libc::pid_t::try_from(child.id()).expect("pid overflowed i32");

    // First SIGINT — arms the idle flag, but should NOT exit.
    unsafe {
        libc::kill(pid, libc::SIGINT);
    }

    // Wait past the 2-second arming window — the flag should have been
    // cleared automatically.  The binary should still be alive.
    std::thread::sleep(std::time::Duration::from_secs(3));

    // Verify the process is still running, then send /exit to exit cleanly.
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"/exit\n")
        .expect("write failed");

    let status = child.wait().expect("wait failed");
    assert_eq!(
        status.code(),
        Some(0),
        "expected clean exit after single SIGINT + /exit, got {:?}",
        status.code()
    );
}
