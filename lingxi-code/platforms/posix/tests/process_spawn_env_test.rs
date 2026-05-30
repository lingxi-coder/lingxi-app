//! Foreground `run` injects `CLAUDECODE`/`GIT_EDITOR`/`SHELL` into the child env.

#![cfg(unix)]

use lingxi_platform_posix::process::PosixProcess;
use lingxi_traits::{
    ProcessCommand, ProcessRunner, SandboxBackend, SandboxedCommand, SandboxedTag,
};
use std::collections::HashMap;

fn mk(command: &str, args: Vec<&str>, env: HashMap<String, String>) -> SandboxedCommand {
    SandboxedCommand::__new_sandboxed(
        ProcessCommand {
            command: command.into(),
            args: args.into_iter().map(String::from).collect(),
            cwd: None,
            env,
            timeout: None,
            stdin: None,
        },
        SandboxedTag::Wrapped {
            backend: SandboxBackend::None,
        },
    )
}

#[tokio::test]
async fn run_injects_spawn_env_contract() {
    let proc = PosixProcess::new();
    let out = proc
        .run(&mk(
            "/bin/sh",
            vec![
                "-c",
                "echo CC=$CLAUDECODE GE=$GIT_EDITOR SH=$SHELL SESS=$CLAUDE_CODE_SESSION_ID",
            ],
            HashMap::from([(
                "CLAUDE_CODE_SESSION_ID".to_string(),
                "session-abc".to_string(),
            )]),
        ))
        .await
        .expect("run");
    assert!(out.stdout.contains("CC=1"), "missing CLAUDECODE=1: {out:?}");
    assert!(
        out.stdout.contains("GE=true"),
        "missing GIT_EDITOR=true: {out:?}"
    );
    assert!(
        out.stdout.contains("SH=/bin/sh"),
        "missing SHELL=/bin/sh: {out:?}"
    );
    assert!(
        out.stdout.contains("SESS=session-abc"),
        "missing session id: {out:?}"
    );
}
