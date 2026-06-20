//! Foreground `run` injects the claude-code spawn-env contract into the child:
//! `CLAUDECODE`/`CLAUDE_CODE_CHILD_SESSION`/`GIT_EDITOR`/`AI_AGENT` always, plus
//! `SHELL` for the bash provider only (claude-code `SHELL: n==="bash"?S:void 0`).

#![cfg(unix)]

use platform_posix::process::PosixProcess;
use std::collections::HashMap;
use traits::{ProcessCommand, ProcessRunner, SandboxBackend, SandboxedCommand, SandboxedTag};

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

/// Pick a real bash-provider shell available on the host (so the spawn binary
/// contains `bash`/`zsh`, matching claude-code's resolved `shellPath`).
fn bash_provider_shell() -> &'static str {
    for candidate in ["/bin/bash", "/bin/zsh", "/usr/bin/bash", "/usr/bin/zsh"] {
        if std::path::Path::new(candidate).exists() {
            return candidate;
        }
    }
    // Fallback: every supported host ships at least one of the above.
    "/bin/bash"
}

#[tokio::test]
async fn run_injects_spawn_env_contract_for_bash_provider() {
    let shell = bash_provider_shell();
    let proc = PosixProcess::new();
    let out = proc
        .run(&mk(
            shell,
            vec![
                "-c",
                "echo CC=$CLAUDECODE CS=$CLAUDE_CODE_CHILD_SESSION GE=$GIT_EDITOR \
                 AA=$AI_AGENT SH=$SHELL SESS=$CLAUDE_CODE_SESSION_ID",
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
        out.stdout.contains("CS=1"),
        "missing CLAUDE_CODE_CHILD_SESSION=1: {out:?}"
    );
    assert!(
        out.stdout.contains("GE=true"),
        "missing GIT_EDITOR=true: {out:?}"
    );
    // #7: AI_AGENT is always injected, value `claude-code_<ver>_agent`.
    assert!(
        out.stdout.contains("AA=claude-code_") && out.stdout.contains("_agent"),
        "missing AI_AGENT=claude-code_<ver>_agent: {out:?}"
    );
    // #6: SHELL set to the resolved bash-provider binary.
    assert!(
        out.stdout.contains(&format!("SH={shell}")),
        "missing SHELL={shell}: {out:?}"
    );
    assert!(
        out.stdout.contains("SESS=session-abc"),
        "missing session id: {out:?}"
    );
}

/// R-O4: a HOOK command (tagged `BypassAuditedWithReason { reason:
/// "hook_command" }`) must have claude-code's `WO()` auth/OTEL denylist
/// STRIPPED from its inherited env — the script can never read the OAuth
/// token, subscription/rate-limit tier, background-session auth handles,
/// resume/session bookkeeping, or any `OTEL_*` telemetry config. A
/// NON-denylisted var still passes through; and a NON-hook command keeps the
/// denylist (the strip is hook-specific). BOTH directions live in ONE test
/// because they mutate `std::env` (process-global) — splitting them would race
/// under the parallel test runner.
#[tokio::test]
async fn run_strips_wo_denylist_from_hook_command_env() {
    // Seed the PARENT process env with denylisted keys + one survivor. The
    // runner inherits the parent env (tokio default), so these reach `env`
    // unless stripped.
    std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", "secret-oauth");
    std::env::set_var("CLAUDE_CODE_SUBSCRIPTION_TYPE", "max");
    std::env::set_var("CLAUDE_CODE_RATE_LIMIT_TIER", "tier-9");
    std::env::set_var("CLAUDE_BG_RV_AUTH", "bg-rv");
    std::env::set_var("CLAUDE_CODE_RESUME_PROMPT", "resume-me");
    std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://otel");
    std::env::set_var("CLAUDE_CODE_OTEL_DIAG_STDERR", "1");
    std::env::set_var("LX_HOOK_SURVIVOR", "i-survive");

    let env_bin = ["/usr/bin/env", "/bin/env"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap_or("/usr/bin/env");

    // A hook command carries the `hook_command` bypass-audit tag.
    let cmd = SandboxedCommand::__new_sandboxed(
        ProcessCommand {
            command: env_bin.into(),
            args: vec![],
            cwd: None,
            env: HashMap::new(),
            timeout: None,
            stdin: None,
        },
        SandboxedTag::BypassAuditedWithReason {
            reason: "hook_command".to_string(),
        },
    );
    let out = PosixProcess::new().run(&cmd).await.expect("run");

    for denied in [
        "CLAUDE_CODE_OAUTH_TOKEN=",
        "CLAUDE_CODE_SUBSCRIPTION_TYPE=",
        "CLAUDE_CODE_RATE_LIMIT_TIER=",
        "CLAUDE_BG_RV_AUTH=",
        "CLAUDE_CODE_RESUME_PROMPT=",
        "OTEL_EXPORTER_OTLP_ENDPOINT=",
        "CLAUDE_CODE_OTEL_DIAG_STDERR=",
    ] {
        assert!(
            !out.stdout.lines().any(|l| l.starts_with(denied)),
            "denylisted hook env var leaked: {denied} in {out:?}"
        );
    }
    // A non-denylisted custom var survives.
    assert!(
        out.stdout.lines().any(|l| l == "LX_HOOK_SURVIVOR=i-survive"),
        "non-denylisted hook env var was wrongly stripped: {out:?}"
    );

    // NEGATIVE direction: a NON-hook command (no `hook_command` tag) KEEPS the
    // denylisted vars — the strip is hook-specific (claude-code runs `WO()`
    // only for the hook-command env, NOT a normal Bash/REPL tool spawn). Reuse
    // the parent env still carrying `CLAUDE_CODE_OAUTH_TOKEN` from above.
    let out_non_hook = PosixProcess::new()
        .run(&mk(env_bin, vec![], HashMap::new()))
        .await
        .expect("run");
    assert!(
        out_non_hook
            .stdout
            .lines()
            .any(|l| l == "CLAUDE_CODE_OAUTH_TOKEN=secret-oauth"),
        "non-hook command must NOT strip the denylist: {out_non_hook:?}"
    );

    for k in [
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_SUBSCRIPTION_TYPE",
        "CLAUDE_CODE_RATE_LIMIT_TIER",
        "CLAUDE_BG_RV_AUTH",
        "CLAUDE_CODE_RESUME_PROMPT",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "CLAUDE_CODE_OTEL_DIAG_STDERR",
        "LX_HOOK_SURVIVOR",
    ] {
        std::env::remove_var(k);
    }
}

/// #6: the powershell provider OMITS `SHELL` (`void 0`). A non-bash-provider
/// spawn must not carry a `SHELL` value in the env we pass to the child.
///
/// We spawn `/usr/bin/env` (which prints the literal passed environment and,
/// unlike a real shell, does NOT self-repopulate `SHELL` from the passwd
/// entry) so the assertion observes exactly what the runner injected. A real
/// `pwsh` would likewise receive no `SHELL` from us.
#[tokio::test]
async fn run_omits_shell_for_non_bash_provider() {
    let env_bin = ["/usr/bin/env", "/bin/env"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap_or("/usr/bin/env");
    let proc = PosixProcess::new();
    let out = proc
        .run(&mk(
            env_bin,
            vec![],
            // Seed an inherited-looking SHELL; the contract must drop it.
            HashMap::from([("SHELL".to_string(), "/bin/zsh".to_string())]),
        ))
        .await
        .expect("run");
    assert!(
        !out.stdout.lines().any(|l| l.starts_with("SHELL=")),
        "non-bash-provider spawn must not pass SHELL: {out:?}"
    );
    // The unconditional contract vars are still present.
    assert!(
        out.stdout.lines().any(|l| l == "CLAUDECODE=1"),
        "missing CLAUDECODE=1: {out:?}"
    );
    assert!(
        out.stdout
            .lines()
            .any(|l| l.starts_with("AI_AGENT=claude-code_") && l.ends_with("_agent")),
        "missing AI_AGENT: {out:?}"
    );
}
