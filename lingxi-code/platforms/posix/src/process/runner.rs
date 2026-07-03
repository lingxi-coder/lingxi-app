//! `tokio::process`-backed [`ProcessRunner`] for desktop hosts.
//!
//! Foreground `run` applies the claude-code spawn-env contract
//! (`LINGXI=1`, `GIT_EDITOR=true`, `AI_AGENT=<…>`, `SHELL=<bin>` for the
//! bash provider) plus a 30-minute
//! default timeout. `spawn_background` lands a real child with
//! file-mode stdio (POSIX `O_NOFOLLOW`) wired to a per-task output file
//! and a setsid call so [`super::kill_tree::kill_tree_force`] can later
//! terminate the descendant process group. `kill(handle)` delegates to
//! `kill_tree_force(handle.pid)` — immediate SIGKILL with no grace period,
//! matching `treeKill(pid, 'SIGKILL')` in `src/utils/ShellCommand.ts:337-343`.

use crate::process::kill_tree::kill_tree_force;
use crate::process::spawn_unsafe::attach_setsid;
use crate::process::wrap::{
    ai_agent_value, is_bash_provider_shell, task_output_path, DEFAULT_TIMEOUT, ENV_AI_AGENT,
    ENV_GIT_EDITOR, ENV_LINGXI_CHILD_SESSION, ENV_LINGXI_MARKER, ENV_LINGXI_SESSION_ID, ENV_SHELL,
};
use async_trait::async_trait;
use std::os::unix::fs::OpenOptionsExt;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use traits::{
    ProcessError, ProcessHandle, ProcessOutput, ProcessRunner, SandboxedCommand, SandboxedTag,
};

/// Audit reason stamped on a hook command by the hooks crate
/// (`hooks/src/executor.rs` — `bypass_with_audit(pcmd, "hook_command")`).
///
/// claude-code assembles a hook command's env as `{...WO(), ...Uot(o),
/// LINGXI_PROJECT_DIR}` with `o.source==="harness"` (BIN off 205727901 /
/// 199137330). For `source==="harness"`, `Uot` does NOT emit `AI_AGENT`
/// (that is gated `source==="agent"`), and the hook env carries no
/// `GIT_EDITOR` (that is a Bash-spawn-only var). The hooks crate already
/// folds the full `Uot(harness)` set (`LINGXI` / `LINGXI_SESSION_ID`
/// / `LINGXI_CHILD_SESSION` / `LINGXI_EFFORT`) into the command env, so
/// the runner must NOT layer the Bash-spawn `AI_AGENT` / `GIT_EDITOR` on top
/// of a hook child. We detect a hook command by this audit reason and skip
/// those two vars — every other command (Bash / REPL / PowerShell tool calls)
/// keeps the full Bash-spawn contract unchanged.
const HOOK_COMMAND_AUDIT_REASON: &str = "hook_command";

/// Auth / session / OTEL env keys claude-code STRIPS from a hook command's
/// environment. Mirrors `WO()` (claude-code BIN off ~195508064), which builds
/// the hook child env from `process.env` and then `delete`s each of these keys
/// (plus every `OTEL_*` key, handled separately below) so a hook script can
/// never read the user's OAuth token, subscription/rate-limit tier, the
/// background-session auth handles, or the resume/session bookkeeping. These
/// are EXACT key names — `WO()` deletes specific keys (NOT wildcard patterns)
/// for everything except the `OTEL_` prefix sweep. `AI_AGENT` / `GIT_EDITOR`
/// are deliberately NOT here: they are not in `WO()`'s denylist (they are
/// handled by the `is_hook_command` Bash-spawn gate above) and stripping them
/// is already correct via that path.
const HOOK_ENV_DENYLIST: &[&str] = &[
    "CLAUDE_CODE_OAUTH_TOKEN",
    "LINGXI_SUBSCRIPTION_TYPE",
    "LINGXI_RATE_LIMIT_TIER",
    "LINGXI_BG_AUTH_SNAPSHOT_PATH",
    "LINGXI_BG_SOCKET_TOKENS_PATH",
    "LINGXI_BG_RV_AUTH",
    "LINGXI_BG_PTY_AUTH",
    "LINGXI_SESSION_KIND",
    "LINGXI_BG_SOURCE",
    "LINGXI_BG_ISOLATION",
    "LINGXI_BG_BACKEND",
    "LINGXI_SESSION_NAME",
    "LINGXI_RESUME_INTERRUPTED_TURN",
    "LINGXI_RESUME_PROMPT",
    "LINGXI_BG_SESSION_PERMISSION_RULES",
    "LINGXI_BG_MEMORY_TOGGLED_OFF",
    "LINGXI_OTEL_DIAG_STDERR",
];

/// Prefix `WO()` sweeps from the hook child env: every key starting with
/// `OTEL_` is `delete`d (`for(let u of Object.keys(process.env))if(u
/// .startsWith("OTEL_"))delete c[u]`). Applied in addition to
/// [`HOOK_ENV_DENYLIST`].
const HOOK_ENV_DENY_PREFIX: &str = "OTEL_";

/// Env var gating the GHA subprocess secret-scrub (`subprocessEnv()`,
/// `utils/subprocessEnv.ts:86`). claude-code-action sets it when running with
/// untrusted content; truthy ⇒ scrub [`GHA_SUBPROCESS_SCRUB`] from EVERY
/// subprocess env (Bash AND hook children both spawn via `subprocessEnv()`).
const ENV_SUBPROCESS_ENV_SCRUB: &str = "LINGXI_SUBPROCESS_ENV_SCRUB";

/// Secret-bearing keys `subprocessEnv()` `delete`s from a child env when
/// [`ENV_SUBPROCESS_ENV_SCRUB`] is truthy (`subprocessEnv.ts:15-53`,
/// `GHA_SUBPROCESS_SCRUB`): Anthropic auth, OTLP exporter headers (carry bearer
/// tokens), cloud-provider creds, GitHub-Actions OIDC/runtime tokens, and
/// claude-code-action input duplicates. Each key's GitHub-Actions `INPUT_<KEY>`
/// twin is stripped too. Without the flag (the common case) this is inert.
const GHA_SUBPROCESS_SCRUB: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_CUSTOM_HEADERS",
    "OTEL_EXPORTER_OTLP_HEADERS",
    "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
    "OTEL_EXPORTER_OTLP_METRICS_HEADERS",
    "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_BEARER_TOKEN_BEDROCK",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "AZURE_CLIENT_SECRET",
    "AZURE_CLIENT_CERTIFICATE_PATH",
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
    "ACTIONS_RUNTIME_TOKEN",
    "ACTIONS_RUNTIME_URL",
    "ALL_INPUTS",
    "OVERRIDE_GITHUB_TOKEN",
    "DEFAULT_WORKFLOW_TOKEN",
    "SSH_SIGNING_KEY",
];

/// Production [`ProcessRunner`] using `tokio::process`.
#[derive(Default)]
pub struct PosixProcess;

impl PosixProcess {
    /// Construct a new `PosixProcess` runner.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Build a `tokio::process::Command` from a sandboxed command, applying
    /// the claude-code spawn-env contract.
    ///
    /// Env-var precedence (matches claude-code `Shell.ts:317-328`):
    /// 1. Caller-supplied env vars on the [`SandboxedCommand`].
    /// 2. `LINGXI=1`, `GIT_EDITOR=true`, `AI_AGENT=<Mer("agent")>`, and
    ///    `SHELL=<inner.command>` (the last only for the bash provider) —
    ///    overwritten on top so callers cannot accidentally clobber them.
    /// 3. `LINGXI_SESSION_ID` is propagated only when the caller has
    ///    explicitly injected it through the env map (the engine layer
    ///    decides whether to set it).
    ///
    /// `SHELL` (#6) follows claude-code's `SHELL: n==="bash"?S:void 0`: it is
    /// set to the resolved shell binary ONLY for the bash provider
    /// (`bash`/`zsh` spawns) and OMITTED for the powershell provider (whose
    /// binary is `pwsh`/`powershell.exe`).
    ///
    /// `AI_AGENT` (#7) mirrors `Uot`'s `t.AI_AGENT=Mer("agent")`, which is
    /// gated on `source==="agent"` — the Bash spawn site hardcodes that, so the
    /// Bash/REPL/PowerShell tool commands get it. A HOOK command runs with
    /// `source:"harness"` (#43), so it gets NEITHER `AI_AGENT` NOR `GIT_EDITOR`;
    /// the runner detects a hook by its [`HOOK_COMMAND_AUDIT_REASON`] tag and
    /// skips both. `SHELL` needs no special-casing: a hook's `command` is a
    /// script path (not `bash`/`zsh`), so `is_bash_provider_shell` is already
    /// false and the inherited `SHELL` is removed — matching the hook env's
    /// absence of `SHELL`.
    fn build_command(cmd: &SandboxedCommand) -> Command {
        let inner = cmd.inner();
        // A hook command (`source:"harness"`) must NOT receive the Bash-spawn
        // `AI_AGENT` / `GIT_EDITOR` vars — see [`HOOK_COMMAND_AUDIT_REASON`].
        // Every other command keeps the full Bash-spawn contract.
        let is_hook_command = matches!(
            cmd.tag(),
            SandboxedTag::BypassAuditedWithReason { reason }
                if reason == HOOK_COMMAND_AUDIT_REASON
        );
        let mut tcmd = Command::new(&inner.command);
        tcmd.args(&inner.args);
        if let Some(cwd) = &inner.cwd {
            tcmd.current_dir(cwd);
        }

        // 1. Caller env first.
        for (k, v) in &inner.env {
            tcmd.env(k, v);
        }
        // 2. Spawn-env contract (overrides anything the caller set).
        tcmd.env(ENV_LINGXI_MARKER.0, ENV_LINGXI_MARKER.1);
        // #7: claude-code `Uot` always marks child processes as a child session.
        tcmd.env(ENV_LINGXI_CHILD_SESSION.0, ENV_LINGXI_CHILD_SESSION.1);
        // #7: claude-code `Uot` injects `AI_AGENT=Mer("agent")` for the Bash
        // spawn (`source:"agent"`); a hook child (`source:"harness"`) gets
        // neither `AI_AGENT` nor `GIT_EDITOR` (#43 — the hook env is
        // `{...WO(), ...Uot(harness), LINGXI_PROJECT_DIR}`).
        if !is_hook_command {
            tcmd.env(ENV_AI_AGENT, ai_agent_value());
            tcmd.env(ENV_GIT_EDITOR.0, ENV_GIT_EDITOR.1);
        } else {
            // R-O4: strip claude-code's `WO()` auth/OTEL denylist from a hook
            // child's INHERITED env (BIN off ~195508064). A hook command runs
            // with `source:"harness"` and `WO()` `delete`s these keys from the
            // env it builds for the child, so the script can never read the
            // user's OAuth token / subscription / rate-limit tier, the
            // background-session auth handles, the resume/session bookkeeping,
            // or any OTEL telemetry config. `tokio::process::Command` inherits
            // the parent env by default (no `env_clear` here), so we
            // `env_remove` each denylisted key + sweep every inherited `OTEL_*`
            // key. A caller-supplied entry of the same name was applied above
            // (step 1) and is removed too — matching `WO()`, which deletes from
            // the FINAL merged env (`{...process.env,...}`), so a hook never
            // sees these regardless of source.
            for key in HOOK_ENV_DENYLIST {
                tcmd.env_remove(key);
            }
            // Sweep every inherited `OTEL_*` key from the parent env. We read
            // the keys (lossily, ignoring any non-UTF-8 name — an OTEL var is
            // always ASCII) and `env_remove` each match.
            for key in std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()) {
                if key.starts_with(HOOK_ENV_DENY_PREFIX) {
                    tcmd.env_remove(&key);
                }
            }
        }
        // #6: SHELL only for the bash provider; powershell omits it. claude-code
        // spreads `{...WO(), SHELL: n==="bash"?S:void 0}` — for powershell the
        // `void 0` overwrites any inherited `SHELL` to `undefined`, which Node
        // drops from the child env. We mirror that by REMOVING the inherited
        // `SHELL` rather than merely not setting it.
        if is_bash_provider_shell(&inner.command) {
            tcmd.env(ENV_SHELL, &inner.command);
        } else {
            tcmd.env_remove(ENV_SHELL);
        }
        // 3. LINGXI_SESSION_ID propagated only if explicitly provided.
        if let Some(sess) = inner.env.get(ENV_LINGXI_SESSION_ID) {
            tcmd.env(ENV_LINGXI_SESSION_ID, sess);
        }
        // 4. GHA subprocess secret-scrub (`subprocessEnv()`, subprocessEnv.ts:86-97):
        //    when `LINGXI_SUBPROCESS_ENV_SCRUB` is truthy (claude-code-action's
        //    untrusted-content mode), `delete` each secret-bearing key + its
        //    `INPUT_<KEY>` GitHub-Actions twin from the child env — for BOTH the
        //    Bash and the hook child (both spawn via `subprocessEnv()`), so a
        //    prompt-injected command can't read Anthropic/cloud/Actions creds in
        //    that CI mode. Inert (no-op) without the flag — the common case.
        if traits::env::is_env_truthy(std::env::var(ENV_SUBPROCESS_ENV_SCRUB).ok().as_deref()) {
            for key in GHA_SUBPROCESS_SCRUB {
                tcmd.env_remove(key);
                tcmd.env_remove(format!("INPUT_{key}"));
            }
        }
        tcmd
    }
}

#[async_trait]
impl ProcessRunner for PosixProcess {
    async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
        let inner = cmd.inner();
        let mut tcmd = Self::build_command(cmd);
        tcmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // On the timeout path below we `return Err(..)` and drop the
            // `Child`. Tokio's default drop does NOT kill the OS process, so a
            // timed-out `bash -c` would otherwise be orphaned and keep running
            // (the handler reports `Killed` while the process is still alive).
            // `kill_on_drop` makes the drop send SIGKILL, so the timeout
            // actually terminates the child. (Descendants escaping the direct
            // child are the `spawn_background` setsid path's concern, not the
            // foreground capture path.)
            .kill_on_drop(true);

        let mut child = tcmd.spawn().map_err(|e| ProcessError::Io(e.to_string()))?;
        if let Some(stdin_text) = &inner.stdin {
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(stdin_text.as_bytes())
                    .await
                    .map_err(|e| ProcessError::Io(e.to_string()))?;
            }
        }

        let timeout = inner.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(r) => r.map_err(|e| ProcessError::Io(e.to_string()))?,
            Err(_) => return Err(ProcessError::Timeout),
        };

        Ok(ProcessOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
            timed_out: false,
        })
    }

    async fn spawn_background(
        &self,
        cmd: &SandboxedCommand,
    ) -> Result<ProcessHandle, ProcessError> {
        let task_id = generate_task_id();
        let out_path = task_output_path(&task_id);
        if let Some(parent) = out_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ProcessError::Io(format!("mkdir task-output: {e}")))?;
        }

        // Open the file with O_WRONLY | O_CREAT | O_APPEND | O_NOFOLLOW
        // to match claude-code's symlink-attack guard in Shell.ts:299-312.
        // `.append(true)` already implies write access on POSIX — clippy's
        // `ineffective_open_options` would flag a redundant `.write(true)`.
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&out_path)
            .map_err(|e| ProcessError::Io(format!("open task-output {out_path:?}: {e}")))?;
        // Duplicate the fd for stderr so both streams interleave atomically.
        let stderr_file = file
            .try_clone()
            .map_err(|e| ProcessError::Io(format!("clone fd: {e}")))?;

        let mut tcmd = Self::build_command(cmd);
        tcmd.stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(stderr_file));
        attach_setsid(&mut tcmd);

        let child = tcmd
            .spawn()
            .map_err(|e| ProcessError::Io(format!("spawn_background: {e}")))?;
        let pid = child
            .id()
            .ok_or_else(|| ProcessError::Io("spawn_background: child has no pid".into()))?;

        // Detach the JoinHandle — the child runs on its own; kill_tree
        // terminates it later. `tokio::process::Child` requires `.wait()` to
        // be called; spawn a small reaper to avoid zombies.
        tokio::spawn(async move {
            let mut child = child;
            let _ = child.wait().await;
        });

        Ok(ProcessHandle { task_id, pid })
    }

    async fn kill(&self, handle: &ProcessHandle) -> Result<(), ProcessError> {
        // Parity: claude-code's `#doKill` calls `treeKill(pid, 'SIGKILL')` directly
        // (ShellCommand.ts:337-343) — no SIGTERM grace period. Use kill_tree_force
        // (immediate SIGKILL) instead of kill_tree_unix (SIGTERM + 5 s + SIGKILL).
        kill_tree_force(handle.pid)
    }

    fn is_available(&self) -> bool {
        true
    }
}

/// Generate a unique task id of the form `local_bash_<nanos-hex>`.
fn generate_task_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("local_bash_{nanos:x}")
}

#[cfg(test)]
mod hook_env_tests {
    use super::*;
    use std::collections::HashMap;
    use traits::ProcessCommand;

    /// A sentinel pre-seeded into the caller env for `AI_AGENT` / `GIT_EDITOR`.
    /// The runner OVERWRITES these for a non-hook (Bash-spawn) command but
    /// LEAVES them for a hook command — so the child observing the sentinel vs.
    /// the runner's value tells us exactly which path ran, independently of the
    /// (claude-code-spawned) test process's own inherited env.
    const SENTINEL: &str = "__sentinel_caller_value__";

    /// Spawn `/usr/bin/env` through the runner with the given audit `reason`,
    /// pre-seeding `AI_AGENT` / `GIT_EDITOR` with [`SENTINEL`]. Returns the
    /// child's env dump.
    async fn run_env_dump(reason: &str) -> String {
        let mut env = HashMap::new();
        env.insert("AI_AGENT".to_string(), SENTINEL.to_string());
        env.insert("GIT_EDITOR".to_string(), SENTINEL.to_string());
        let pcmd = ProcessCommand {
            command: "/usr/bin/env".to_string(),
            args: vec![],
            cwd: None,
            env,
            timeout: None,
            stdin: None,
        };
        let cmd = SandboxedCommand::__new_sandboxed(
            pcmd,
            SandboxedTag::BypassAuditedWithReason {
                reason: reason.to_string(),
            },
        );
        let out = PosixProcess::new().run(&cmd).await.expect("env runs");
        out.stdout
    }

    /// A hook command (`source:"harness"`) gets neither `AI_AGENT` nor
    /// `GIT_EDITOR` from the runner — matching claude-code's `{...WO(),
    /// ...Uot(harness), LINGXI_PROJECT_DIR}` hook env, which carries neither.
    /// The pre-seeded sentinel therefore survives untouched.
    #[tokio::test]
    async fn hook_command_omits_ai_agent_and_git_editor() {
        let dump = run_env_dump(HOOK_COMMAND_AUDIT_REASON).await;
        let lines: Vec<&str> = dump.lines().collect();
        assert!(
            lines.iter().any(|l| *l == format!("AI_AGENT={SENTINEL}")),
            "runner must NOT overwrite AI_AGENT on a hook child; env was:\n{dump}",
        );
        assert!(
            lines.iter().any(|l| *l == format!("GIT_EDITOR={SENTINEL}")),
            "runner must NOT overwrite GIT_EDITOR on a hook child; env was:\n{dump}",
        );
        // The always-present `Uot` markers are still set on a hook child.
        assert!(lines.iter().any(|l| *l == "LINGXI=1"));
        assert!(lines.iter().any(|l| *l == "LINGXI_CHILD_SESSION=1"));
    }

    /// A non-hook command (Bash/REPL/PowerShell tool call, `source:"agent"`)
    /// keeps the full Bash-spawn contract: the runner OVERWRITES the sentinel
    /// with `AI_AGENT=<Mer("agent")>` + `GIT_EDITOR=true`.
    #[tokio::test]
    async fn tool_command_overwrites_ai_agent_and_git_editor() {
        let dump = run_env_dump("bash_tool_call").await;
        let lines: Vec<&str> = dump.lines().collect();
        assert!(
            !lines.iter().any(|l| *l == format!("AI_AGENT={SENTINEL}")),
            "runner overwrites AI_AGENT on a non-hook child; env was:\n{dump}",
        );
        assert!(
            lines
                .iter()
                .any(|l| *l == format!("AI_AGENT={}", ai_agent_value())),
            "non-hook child carries the runner's AI_AGENT value; env was:\n{dump}",
        );
        assert!(
            lines.iter().any(|l| *l == "GIT_EDITOR=true"),
            "non-hook child carries GIT_EDITOR=true; env was:\n{dump}",
        );
    }
}
