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

use crate::process::active_children;
use crate::process::kill_tree::kill_tree_force;
use crate::process::spawn_unsafe::attach_setsid;
use crate::process::wrap::{
    ai_agent_value, is_bash_provider_shell, task_output_path, DEFAULT_TIMEOUT, ENV_AI_AGENT,
    ENV_GIT_EDITOR, ENV_LINGXI_CHILD_SESSION, ENV_LINGXI_MARKER, ENV_LINGXI_SESSION_ID, ENV_SHELL,
};
use async_trait::async_trait;
use std::os::unix::fs::OpenOptionsExt;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use traits::process::ProcessStreamSink;
use traits::{
    HookRunOutcome, ProcessError, ProcessHandle, ProcessOutput, ProcessRunner, SandboxedCommand,
    SandboxedTag,
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

/// SH-07 — drain `r` to EOF into `buf`, pushing every chunk to `observer` as it
/// is read.
///
/// This is the live half of claude-code's `hook_progress` polling: upstream's
/// `tWi` reads a growing accumulator every second, which only works because the
/// child's `stdout`/`stderr` `data` listeners append to it as bytes arrive. A
/// `read_to_end` publishes nothing until EOF, so a progress poll layered over it
/// would emit exactly zero frames.
///
/// Chunks are handed over as raw bytes — decoding per chunk would corrupt a
/// multi-byte UTF-8 sequence that straddles a read boundary.
async fn drain_observed<R>(
    r: &mut R,
    buf: &mut Vec<u8>,
    observer: Option<&std::sync::Arc<dyn traits::HookOutputObserver>>,
    is_stderr: bool,
) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    // No observer ⇒ the cheap bulk read, byte-identical to the previous code.
    if observer.is_none() {
        r.read_to_end(buf).await?;
        return Ok(());
    }
    let mut chunk = vec![0u8; 8192];
    loop {
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(obs) = observer {
            if is_stderr {
                obs.on_chunk(&[], &chunk[..n]).await;
            } else {
                obs.on_chunk(&chunk[..n], &[]).await;
            }
        }
    }
}

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

/// RAII guard that drops a foreground child's pgid from the print/SDK-mode
/// [`active_children`] registry when its `run` completes or its future is dropped
/// (cancel). Paired with a `setsid` spawn + [`active_children::register`] so the
/// print/SDK signal handler tree-kills only children that are still running.
struct ChildRegistration(u32);

impl Drop for ChildRegistration {
    fn drop(&mut self) {
        active_children::unregister(self.0);
    }
}

/// Cancellation guard for streaming commands. Monitor workers are cancelled by
/// dropping their `run_streaming` future, so `Child::kill_on_drop` alone would
/// only kill the shell and could orphan grandchildren. Streaming children are
/// always session leaders; this guard synchronously kills their full process
/// group on every early-return/drop path and is disarmed after a clean reap.
struct StreamingProcessGroupGuard(Option<u32>);

impl Drop for StreamingProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            let _ = kill_tree_force(pid);
        }
    }
}

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

        // PARITY 2.1.212 (print/SDK SIGTERM cleanup): in print/SDK mode, spawn the
        // foreground child in its OWN process group (`setsid`, like claude-code's
        // `detached: true`) and register its pgid so a process-level
        // SIGTERM/SIGHUP/SIGINT handler can `killpg` the whole subtree before exit
        // — `kill_on_drop` alone never fires on an abrupt signal. Interactive mode
        // leaves this off, so its spawn path is byte-identical to before.
        let print_mode_cleanup = active_children::print_mode_child_cleanup_enabled();
        if print_mode_cleanup {
            attach_setsid(&mut tcmd);
        }

        let mut child = tcmd.spawn().map_err(|e| ProcessError::Io(e.to_string()))?;
        // Register the setsid child's pgid (== pid) for the duration of this run;
        // the guard unregisters it on completion OR on future-drop (cancel).
        let _child_registration = if print_mode_cleanup {
            child.id().map(|pid| {
                active_children::register(pid);
                ChildRegistration(pid)
            })
        } else {
            None
        };
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

    async fn run_streaming(
        &self,
        cmd: &SandboxedCommand,
        sink: std::sync::Arc<dyn ProcessStreamSink>,
    ) -> Result<ProcessOutput, ProcessError> {
        let inner = cmd.inner();
        let mut tcmd = Self::build_command(cmd);
        tcmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // A Monitor/streaming worker can be cancelled at any await point. Give
        // it an independent process group so the drop guard removes descendants
        // as well as the direct shell.
        attach_setsid(&mut tcmd);

        let mut child = tcmd.spawn().map_err(|e| ProcessError::Io(e.to_string()))?;
        let pid = child
            .id()
            .ok_or_else(|| ProcessError::Io("streaming child has no pid".into()))?;
        let mut group_guard = StreamingProcessGroupGuard(Some(pid));

        if let Some(stdin_text) = &inner.stdin {
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(stdin_text.as_bytes())
                    .await
                    .map_err(|e| ProcessError::Io(e.to_string()))?;
            }
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProcessError::Io("streaming child has no stdout pipe".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| ProcessError::Io("streaming child has no stderr pipe".into()))?;
        let stdout_sink = sink.clone();
        let stderr_sink = sink;

        let stdout_task = async move {
            let mut reader = BufReader::new(stdout);
            let mut captured = Vec::new();
            loop {
                let mut line = Vec::new();
                let read = reader
                    .read_until(b'\n', &mut line)
                    .await
                    .map_err(|e| ProcessError::Io(e.to_string()))?;
                if read == 0 {
                    break;
                }
                captured.extend_from_slice(&line);
                while matches!(line.last(), Some(b'\n' | b'\r')) {
                    line.pop();
                }
                stdout_sink
                    .stdout_line(String::from_utf8_lossy(&line).into_owned())
                    .await?;
            }
            Ok::<Vec<u8>, ProcessError>(captured)
        };
        let stderr_task = async move {
            let mut captured = Vec::new();
            let mut chunk = vec![0_u8; 8 * 1024];
            loop {
                let read = stderr
                    .read(&mut chunk)
                    .await
                    .map_err(|e| ProcessError::Io(e.to_string()))?;
                if read == 0 {
                    break;
                }
                let bytes = chunk[..read].to_vec();
                captured.extend_from_slice(&bytes);
                stderr_sink.stderr_chunk(bytes).await?;
            }
            Ok::<Vec<u8>, ProcessError>(captured)
        };
        let execution = async {
            let (stdout, stderr, status) = tokio::try_join!(stdout_task, stderr_task, async {
                child
                    .wait()
                    .await
                    .map_err(|e| ProcessError::Io(e.to_string()))
            })?;
            Ok::<_, ProcessError>((stdout, stderr, status))
        };

        let timeout = inner.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let result = tokio::time::timeout(timeout, execution).await;
        match result {
            Ok(Ok((stdout, stderr, status))) => {
                group_guard.0 = None;
                Ok(ProcessOutput {
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    exit_code: status.code().unwrap_or(-1),
                    timed_out: false,
                })
            }
            Ok(Err(error)) => {
                let _ = kill_tree_force(pid);
                group_guard.0 = None;
                let _ = child.wait().await;
                Err(error)
            }
            Err(_) => {
                let _ = kill_tree_force(pid);
                group_guard.0 = None;
                let _ = child.wait().await;
                Err(ProcessError::Timeout)
            }
        }
    }

    // PARITY 2.1.210 (timeout → move-to-background): a foreground Bash command
    // that exceeds its timeout is NOT killed — the still-running child is handed
    // off to a detached reaper that keeps draining its output into a per-task
    // file, and `MovedToBackground` is returned so the tool layer surfaces the
    // "…did not complete within its Ns timeout and was moved to the background"
    // note. The spawn preamble mirrors `run` exactly (identical env, the same
    // print/SDK-mode setsid + active-children registration) so the
    // finished-in-time path is byte-identical to `run`.
    async fn run_foreground(
        &self,
        cmd: &SandboxedCommand,
    ) -> Result<traits::ForegroundOutcome, ProcessError> {
        let inner = cmd.inner();
        let mut tcmd = Self::build_command(cmd);
        tcmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // As in `run`: a dropped `Child` (cancellation / early return)
            // SIGKILLs the process. On the timeout→background handoff below the
            // child is MOVED into a detached reaper before this future returns,
            // so it is NOT dropped and survives; only cancellation still kills.
            .kill_on_drop(true);

        let print_mode_cleanup = active_children::print_mode_child_cleanup_enabled();
        if print_mode_cleanup {
            attach_setsid(&mut tcmd);
        }

        let mut child = tcmd.spawn().map_err(|e| ProcessError::Io(e.to_string()))?;
        // Capture this before polling `wait()`: Tokio clears `Child::id()` once
        // the direct child has been reaped. A busy executor can observe the
        // timeout and the already-completed child in the same poll, so looking
        // the id up only during the background handoff is racy.
        let spawned_pid = child
            .id()
            .ok_or_else(|| ProcessError::Io("spawned child has no pid".into()))?;
        let child_registration = if print_mode_cleanup {
            active_children::register(spawned_pid);
            Some(ChildRegistration(spawned_pid))
        } else {
            None
        };
        if let Some(stdin_text) = &inner.stdin {
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(stdin_text.as_bytes())
                    .await
                    .map_err(|e| ProcessError::Io(e.to_string()))?;
            }
        }

        let timeout = inner.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let mut sout = child
            .stdout
            .take()
            .ok_or_else(|| ProcessError::Io("child has no stdout pipe".into()))?;
        let mut serr = child
            .stderr
            .take()
            .ok_or_else(|| ProcessError::Io("child has no stderr pipe".into()))?;
        let mut out_buf: Vec<u8> = Vec::new();
        let mut err_buf: Vec<u8> = Vec::new();
        let mut out_done = false;
        let mut err_done = false;
        let mut exit_status: Option<std::process::ExitStatus> = None;

        let sleep = tokio::time::sleep(timeout);
        tokio::pin!(sleep);

        // Drive both pipes and the child's exit concurrently, bounded by the
        // deadline. Reading into buffers (rather than `wait_with_output`) keeps
        // the child + its still-open pipes in hand if the deadline fires.
        let timed_out = loop {
            if out_done && err_done && exit_status.is_some() {
                break false;
            }
            tokio::select! {
                biased;
                () = &mut sleep => break true,
                r = sout.read_buf(&mut out_buf), if !out_done => {
                    match r { Ok(0) | Err(_) => out_done = true, Ok(_) => {} }
                }
                r = serr.read_buf(&mut err_buf), if !err_done => {
                    match r { Ok(0) | Err(_) => err_done = true, Ok(_) => {} }
                }
                s = child.wait(), if exit_status.is_none() => {
                    exit_status = Some(s.map_err(|e| ProcessError::Io(e.to_string()))?);
                }
            }
        };

        if !timed_out {
            let status = exit_status.expect("loop breaks with a status when not timed out");
            return Ok(traits::ForegroundOutcome::Completed(ProcessOutput {
                stdout: String::from_utf8_lossy(&out_buf).into_owned(),
                stderr: String::from_utf8_lossy(&err_buf).into_owned(),
                exit_code: status.code().unwrap_or(-1),
                timed_out: false,
            }));
        }

        // The timer may become ready while this future is starved by other
        // workspace work, after the OS process has already exited. Because the
        // select is deliberately timer-biased, that used to enter the
        // background branch and then fail (`Child::id()` is `None` after
        // `wait()`). An exited process cannot be moved to the background. Drain
        // any immediately available tail from its pipes, with a small bounded
        // grace period, and report the real completion instead.
        let completed_status = match exit_status.take() {
            Some(status) => Some(status),
            None => child
                .try_wait()
                .map_err(|e| ProcessError::Io(e.to_string()))?,
        };
        if let Some(status) = completed_status {
            const MAX_POST_EXIT_READS: usize = 256;
            let grace = tokio::time::sleep(std::time::Duration::from_millis(50));
            tokio::pin!(grace);
            let mut reads = 0usize;
            while !(out_done && err_done) && reads < MAX_POST_EXIT_READS {
                tokio::select! {
                    // Prefer already-buffered output/EOF over an elapsed grace
                    // timer. The read-count cap prevents a descendant that
                    // inherited the pipes from starving this loop indefinitely.
                    biased;
                    r = sout.read_buf(&mut out_buf), if !out_done => {
                        reads += 1;
                        match r { Ok(0) | Err(_) => out_done = true, Ok(_) => {} }
                    }
                    r = serr.read_buf(&mut err_buf), if !err_done => {
                        reads += 1;
                        match r { Ok(0) | Err(_) => err_done = true, Ok(_) => {} }
                    }
                    () = &mut grace => break,
                }
            }
            return Ok(traits::ForegroundOutcome::Completed(ProcessOutput {
                stdout: String::from_utf8_lossy(&out_buf).into_owned(),
                stderr: String::from_utf8_lossy(&err_buf).into_owned(),
                exit_code: status.code().unwrap_or(-1),
                timed_out: false,
            }));
        }

        // ===== Timeout → move to background =====
        // Land a per-task output file (same O_NOFOLLOW symlink guard as
        // `spawn_background`) and hand the live child to a detached reaper. The
        // partial output captured before the deadline is flushed first so a
        // `Read` on the file shows everything from the start; the reaper then
        // keeps copying both pipes to the file until EOF and reaps the child.
        let task_id = generate_task_id();
        let out_path = task_output_path(&task_id);
        if let Some(parent) = out_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ProcessError::Io(format!("mkdir task-output: {e}")))?;
        }
        let std_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&out_path)
            .map_err(|e| ProcessError::Io(format!("open task-output {out_path:?}: {e}")))?;
        let std_file2 = std_file
            .try_clone()
            .map_err(|e| ProcessError::Io(format!("clone fd: {e}")))?;
        let pid = spawned_pid;

        tokio::spawn(async move {
            // Hold the print-mode registration for the child's remaining life so
            // an abrupt process SIGTERM can still `killpg` the backgrounded tree.
            let _child_registration = child_registration;
            let mut child = child;
            let mut file = tokio::fs::File::from_std(std_file);
            let mut file2 = tokio::fs::File::from_std(std_file2);
            let _ = file.write_all(&out_buf).await;
            let _ = file.write_all(&err_buf).await;
            let _ = file.flush().await;
            let copy_out = tokio::io::copy(&mut sout, &mut file);
            let copy_err = tokio::io::copy(&mut serr, &mut file2);
            let _ = tokio::join!(copy_out, copy_err);
            let _ = file.flush().await;
            let _ = file2.flush().await;
            let _ = child.wait().await;
        });

        Ok(traits::ForegroundOutcome::MovedToBackground(
            ProcessHandle { task_id, pid },
        ))
    }

    async fn run_hook_with_async_detection(
        &self,
        cmd: &SandboxedCommand,
        default_async_timeout: std::time::Duration,
    ) -> Result<HookRunOutcome, ProcessError> {
        self.run_hook_with_async_detection_observed(cmd, default_async_timeout, None)
            .await
    }

    /// SH-07 — the observed variant. Identical to the buffered one except that
    /// every chunk read off the child's stdout/stderr is pushed to `observer` as
    /// it arrives, which is what lets the hook layer run claude-code's `tWi`
    /// progress poll (oracle 2.1.238 @ 296463298) over a live accumulator.
    ///
    /// RESIDUAL: the BACKGROUNDED (`{"async":true}` first line) arm detaches the
    /// child into a drain task; upstream keeps polling it through the pending-
    /// async-hook registry (`n3m`). The port's detached drain carries no
    /// observer, so a backgrounded hook emits no further `hook_progress` frames.
    async fn run_hook_with_async_detection_observed(
        &self,
        cmd: &SandboxedCommand,
        default_async_timeout: std::time::Duration,
        observer: Option<std::sync::Arc<dyn traits::HookOutputObserver>>,
    ) -> Result<HookRunOutcome, ProcessError> {
        let inner = cmd.inner();
        let mut tcmd = Self::build_command(cmd);
        tcmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // As in `run`: a dropped `Child` (timeout / early return) must SIGKILL
            // the process so a timed-out hook is not orphaned.
            .kill_on_drop(true);

        let mut child = tcmd.spawn().map_err(|e| ProcessError::Io(e.to_string()))?;
        if let Some(stdin_text) = &inner.stdin {
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(stdin_text.as_bytes())
                    .await
                    .map_err(|e| ProcessError::Io(e.to_string()))?;
            }
            // `stdin` drops here → the child sees EOF on stdin.
        }

        let timeout = inner.timeout.unwrap_or(DEFAULT_TIMEOUT);
        // A single deadline bounds the whole operation, matching `run`'s single
        // `timeout(wait_with_output)` budget.
        let deadline = tokio::time::Instant::now() + timeout;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProcessError::Io("hook child has no stdout pipe".into()))?;
        let mut reader = BufReader::new(stdout);
        let mut first_line: Vec<u8> = Vec::new();
        match tokio::time::timeout_at(deadline, reader.read_until(b'\n', &mut first_line)).await {
            Err(_) => return Err(ProcessError::Timeout),
            Ok(Err(e)) => return Err(ProcessError::Io(e.to_string())),
            Ok(Ok(_)) => {}
        }
        // Upstream attaches its `data` listeners BEFORE writing stdin, so the
        // first line is part of the observed output too.
        if let (Some(obs), false) = (observer.as_ref(), first_line.is_empty()) {
            obs.on_chunk(&first_line, &[]).await;
        }

        // Runtime async detection: a first line of `{"async":true,...}` backgrounds
        // the hook (claude-code `hooks.ts:1117-1166`).
        if let Some(async_timeout) = parse_async_first_line(&first_line, default_async_timeout) {
            let stderr = child.stderr.take();
            // The eventual (post-marker) drained output is delivered once through
            // this channel so the hook layer can fold it back as an
            // `async_hook_response` (claude-code `registerPendingAsyncHook`).
            let (output_tx, output_rx) = tokio::sync::oneshot::channel::<ProcessOutput>();
            // Detach: drain remaining output (so the pipe never blocks the child)
            // and reap it, bounded by the async timeout; on timeout the child is
            // dropped → `kill_on_drop` SIGKILLs it.
            tokio::spawn(async move {
                let mut child = child;
                let drain_and_wait = async {
                    let mut stdout_buf = Vec::new();
                    let mut stderr_buf = Vec::new();
                    let stderr_read = async {
                        if let Some(mut se) = stderr {
                            se.read_to_end(&mut stderr_buf).await
                        } else {
                            Ok(0)
                        }
                    };
                    let (_stdout_res, _stderr_res) =
                        tokio::join!(reader.read_to_end(&mut stdout_buf), stderr_read);
                    let status = child.wait().await;
                    (stdout_buf, stderr_buf, status)
                };
                // The drained stdout is the hook's output AFTER the consumed
                // `{"async":true}` marker line — exactly the payload the fold-back
                // maps through the command-hook contract (`map_command_output`).
                let output = match tokio::time::timeout(async_timeout, drain_and_wait).await {
                    Ok((stdout_buf, stderr_buf, status)) => ProcessOutput {
                        stdout: String::from_utf8_lossy(&stdout_buf).into_owned(),
                        stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
                        exit_code: status.map_or(-1, |s| s.code().unwrap_or(-1)),
                        timed_out: false,
                    },
                    Err(_) => ProcessOutput {
                        stdout: String::new(),
                        stderr: "async hook timed out; process was terminated".to_string(),
                        exit_code: -1,
                        timed_out: true,
                    },
                };
                // `child` drops at end of scope → `kill_on_drop` SIGKILLs a
                // still-running (timed-out) child.
                let _ = output_tx.send(output);
            });
            return Ok(HookRunOutcome::Backgrounded {
                async_timeout,
                output: Some(output_rx),
            });
        }

        // Not async: read the rest of stdout and all of stderr CONCURRENTLY (as
        // `wait_with_output` does, so a large stderr can't deadlock the stdout
        // read), then wait — all bounded by the same deadline.
        let mut stderr_pipe = child.stderr.take();
        let obs_out = observer.clone();
        let obs_err = observer.clone();
        let complete = async {
            let mut rest: Vec<u8> = Vec::new();
            let mut stderr_buf: Vec<u8> = Vec::new();
            let stderr_read = async {
                if let Some(se) = stderr_pipe.as_mut() {
                    drain_observed(se, &mut stderr_buf, obs_err.as_ref(), true).await
                } else {
                    Ok(())
                }
            };
            let (rest_res, stderr_res) = tokio::join!(
                drain_observed(&mut reader, &mut rest, obs_out.as_ref(), false),
                stderr_read
            );
            rest_res.map_err(|e| ProcessError::Io(e.to_string()))?;
            stderr_res.map_err(|e| ProcessError::Io(e.to_string()))?;
            let status = child
                .wait()
                .await
                .map_err(|e| ProcessError::Io(e.to_string()))?;
            Ok::<_, ProcessError>((rest, stderr_buf, status))
        };

        match tokio::time::timeout_at(deadline, complete).await {
            Err(_) => Err(ProcessError::Timeout),
            Ok(Err(e)) => Err(e),
            Ok(Ok((rest, stderr_buf, status))) => {
                // Reconstruct stdout exactly: consumed first line + the rest.
                let mut stdout_bytes = first_line;
                stdout_bytes.extend_from_slice(&rest);
                Ok(HookRunOutcome::Completed(ProcessOutput {
                    stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
                    exit_code: status.code().unwrap_or(-1),
                    timed_out: false,
                }))
            }
        }
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

/// Parse a hook's first stdout line for the async-response marker
/// `{"async": true, "asyncTimeout"?: <ms>}` (claude-code's `FZe` schema).
/// Returns the effective background timeout (`asyncTimeout` ms, or
/// `default_async_timeout` when absent/zero — claude's `asyncTimeout || 15000`),
/// or `None` when the line is not that marker.
fn parse_async_first_line(
    line: &[u8],
    default_async_timeout: std::time::Duration,
) -> Option<std::time::Duration> {
    let text = std::str::from_utf8(line).ok()?.trim();
    if text.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("async") != Some(&serde_json::Value::Bool(true)) {
        return None;
    }
    let timeout = value
        .get("asyncTimeout")
        .and_then(serde_json::Value::as_u64)
        .filter(|&ms| ms > 0)
        .map_or(default_async_timeout, std::time::Duration::from_millis);
    Some(timeout)
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
mod async_hook_tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;
    use traits::ProcessCommand;

    fn sh(script: &str) -> SandboxedCommand {
        let pcmd = ProcessCommand {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            cwd: None,
            env: HashMap::new(),
            timeout: Some(Duration::from_secs(5)),
            stdin: None,
        };
        SandboxedCommand::__new_sandboxed(
            pcmd,
            SandboxedTag::BypassAuditedWithReason {
                reason: HOOK_COMMAND_AUDIT_REASON.to_string(),
            },
        )
    }

    #[test]
    fn parse_async_first_line_recognizes_marker_and_timeout() {
        let def = Duration::from_millis(15_000);
        // Not the marker.
        assert!(parse_async_first_line(b"hello\n", def).is_none());
        assert!(parse_async_first_line(b"{\"async\":false}\n", def).is_none());
        assert!(parse_async_first_line(b"", def).is_none());
        // Marker without timeout → default; with 0 → default; with N → N.
        assert_eq!(
            parse_async_first_line(b"{\"async\":true}\n", def),
            Some(def)
        );
        assert_eq!(
            parse_async_first_line(b"{\"async\":true,\"asyncTimeout\":0}", def),
            Some(def)
        );
        assert_eq!(
            parse_async_first_line(b"{\"async\":true,\"asyncTimeout\":250}\n", def),
            Some(Duration::from_millis(250))
        );
    }

    #[tokio::test]
    async fn normal_hook_output_is_reconstructed_exactly() {
        // A multi-line, non-marker hook: stdout/stderr/exit must match `run`.
        let cmd = sh("printf 'line1\\nline2\\n'; printf 'err1\\n' 1>&2; exit 3");
        let outcome = PosixProcess::new()
            .run_hook_with_async_detection(&cmd, Duration::from_millis(15_000))
            .await
            .expect("runs");
        match outcome {
            HookRunOutcome::Completed(o) => {
                assert_eq!(o.stdout, "line1\nline2\n");
                assert_eq!(o.stderr, "err1\n");
                assert_eq!(o.exit_code, 3);
            }
            HookRunOutcome::Backgrounded { .. } => panic!("normal hook must not background"),
        }
    }

    #[tokio::test]
    async fn empty_output_hook_completes() {
        let cmd = sh("exit 0");
        let outcome = PosixProcess::new()
            .run_hook_with_async_detection(&cmd, Duration::from_millis(15_000))
            .await
            .expect("runs");
        match outcome {
            HookRunOutcome::Completed(o) => {
                assert_eq!(o.stdout, "");
                assert_eq!(o.exit_code, 0);
            }
            HookRunOutcome::Backgrounded { .. } => panic!("empty hook must not background"),
        }
    }

    #[tokio::test]
    async fn async_marker_backgrounds_without_blocking() {
        // Prints the marker, then sleeps far longer than the async timeout. The
        // call must return Backgrounded immediately (well under the sleep).
        let cmd = sh("echo '{\"async\":true,\"asyncTimeout\":100}'; sleep 30");
        let start = tokio::time::Instant::now();
        let outcome = PosixProcess::new()
            .run_hook_with_async_detection(&cmd, Duration::from_millis(15_000))
            .await
            .expect("runs");
        let HookRunOutcome::Backgrounded {
            async_timeout,
            output: Some(_output_rx),
        } = outcome
        else {
            panic!("async hook must background with an eventual-output handle");
        };
        // The marker's `asyncTimeout` (100ms) overrides the caller default.
        assert_eq!(async_timeout, Duration::from_millis(100));
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "async hook must not block the turn (took {:?})",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn async_marker_retains_eventual_stdout_and_stderr() {
        // The post-marker stdout/stderr + exit are drained and delivered once
        // through the eventual-output channel (the fold-back payload).
        // asyncTimeout is generous ON PURPOSE. This test asserts the FOLD-BACK
        // PAYLOAD, not the timeout, and the marker's timeout is a hard kill
        // deadline for the backgrounded process. At 1000ms it raced the
        // scheduler: under a saturated run three `echo`s can exceed a second of
        // wall clock, the hook was terminated, and the test failed with an
        // empty stdout and `timed_out: true`. A test that asserts on output
        // must not also be a wall-clock benchmark.
        let cmd =
            sh("echo '{\"async\":true,\"asyncTimeout\":60000}'; echo after; echo err 1>&2; exit 0");
        let outcome = PosixProcess::new()
            .run_hook_with_async_detection(&cmd, Duration::from_millis(15_000))
            .await
            .expect("runs");
        let HookRunOutcome::Backgrounded {
            output: Some(output_rx),
            ..
        } = outcome
        else {
            panic!("async hook must background with an eventual-output handle");
        };

        let out = tokio::time::timeout(Duration::from_secs(60), output_rx)
            .await
            .expect("eventual output must arrive before the async timeout")
            .expect("sender must not drop");
        // The consumed marker line is NOT part of the eventual output; only the
        // post-marker content is.
        assert_eq!(out.stdout, "after\n", "eventual stdout: {out:?}");
        assert_eq!(out.stderr, "err\n", "eventual stderr: {out:?}");
        assert_eq!(out.exit_code, 0);
        assert!(!out.timed_out);
    }
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

#[cfg(test)]
mod streaming_tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::Notify;
    use traits::ProcessCommand;

    #[derive(Default)]
    struct RecordingSink {
        lines: Mutex<Vec<String>>,
        stderr: Mutex<Vec<u8>>,
        line_ready: Notify,
    }

    #[async_trait]
    impl ProcessStreamSink for RecordingSink {
        async fn stdout_line(&self, line: String) -> Result<(), ProcessError> {
            self.lines.lock().expect("lines lock").push(line);
            self.line_ready.notify_one();
            Ok(())
        }

        async fn stderr_chunk(&self, chunk: Vec<u8>) -> Result<(), ProcessError> {
            self.stderr.lock().expect("stderr lock").extend(chunk);
            Ok(())
        }
    }

    fn stream_sh(script: &str, timeout: Duration) -> SandboxedCommand {
        SandboxedCommand::__new_sandboxed(
            ProcessCommand {
                command: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), script.to_string()],
                cwd: None,
                env: HashMap::new(),
                timeout: Some(timeout),
                stdin: None,
            },
            SandboxedTag::BypassAuditedWithReason {
                reason: "monitor_task_test".to_string(),
            },
        )
    }

    #[tokio::test]
    async fn streaming_delivers_a_line_before_process_exit() {
        let sink = Arc::new(RecordingSink::default());
        // The child sleeps long enough that the `!task.is_finished()` assertion
        // below cannot lose a race with the scheduler. At 0.4s it did: this test
        // passes in isolation but failed alongside its siblings, because a
        // descheduled test thread let the whole process finish before the
        // assertion ran. The window is what makes the assertion meaningful, so
        // it has to dwarf jitter rather than merely exceed it.
        let command = stream_sh(
            "printf 'ready\\n'; sleep 3; printf 'done\\n'",
            Duration::from_secs(60),
        );
        let task_sink: Arc<dyn ProcessStreamSink> = sink.clone();
        let task =
            tokio::spawn(
                async move { PosixProcess::new().run_streaming(&command, task_sink).await },
            );

        tokio::time::timeout(Duration::from_secs(30), sink.line_ready.notified())
            .await
            .expect("first line is delivered live");
        assert!(
            !task.is_finished(),
            "streaming must not wait for process exit"
        );
        let output = task
            .await
            .expect("runner task joins")
            .expect("command succeeds");
        assert_eq!(output.stdout, "ready\ndone\n");
        assert_eq!(
            sink.lines.lock().expect("lines lock").as_slice(),
            ["ready", "done"]
        );
    }

    #[tokio::test]
    async fn streaming_drains_large_stderr_without_deadlock() {
        let sink = Arc::new(RecordingSink::default());
        let command = stream_sh(
            "dd if=/dev/zero bs=1024 count=256 1>&2 2>/dev/null; printf 'ok\\n'",
            Duration::from_secs(5),
        );
        let output = PosixProcess::new()
            .run_streaming(&command, sink.clone())
            .await
            .expect("large stderr is drained concurrently");
        assert_eq!(output.stdout, "ok\n");
        assert_eq!(output.stderr.len(), 256 * 1024);
        assert_eq!(sink.stderr.lock().expect("stderr lock").len(), 256 * 1024);
    }

    #[tokio::test]
    async fn streaming_timeout_kills_the_command() {
        let sink: Arc<dyn ProcessStreamSink> = Arc::new(RecordingSink::default());
        let command = stream_sh("sleep 30", Duration::from_millis(50));
        let error = PosixProcess::new()
            .run_streaming(&command, sink)
            .await
            .expect_err("deadline must stop the command");
        assert!(matches!(error, ProcessError::Timeout));
    }

    #[tokio::test]
    async fn dropping_streaming_future_kills_process_group() {
        let sink = Arc::new(RecordingSink::default());
        let command = stream_sh("echo $$; sleep 30", Duration::from_secs(60));
        let task_sink: Arc<dyn ProcessStreamSink> = sink.clone();
        let task =
            tokio::spawn(
                async move { PosixProcess::new().run_streaming(&command, task_sink).await },
            );
        tokio::time::timeout(Duration::from_secs(2), sink.line_ready.notified())
            .await
            .expect("shell pid is emitted");
        let pid = sink.lines.lock().expect("lines lock")[0].clone();

        task.abort();
        let _ = task.await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let alive = std::process::Command::new("/bin/kill")
            .args(["-0", &pid])
            .status()
            .expect("kill -0 runs")
            .success();
        assert!(!alive, "cancelled streaming shell {pid} must not survive");
    }
}
