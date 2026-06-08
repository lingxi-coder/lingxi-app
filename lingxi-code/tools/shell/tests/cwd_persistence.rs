//! BASH.4 — persistent shell working directory across `BashTool::call`s.
//!
//! Mirrors claude-code `STATE.cwd` semantics (Shell.ts / bashProvider.ts): a
//! foreground `cd` is observed via a `pwd -P` readback written to an internal
//! tracking temp file, so the *next* Bash call spawns under the new directory.
//! Background tasks and subagents must NOT mutate the shared cwd.
//!
//! The tests drive a recording `ProcessRunner` that (i) captures each spawn's
//! cwd + command string and (ii) simulates `pwd -P` by parsing the
//! ` && pwd -P >| '<file>'` readback redirect out of the spawned command and
//! writing a chosen path into that file — exactly what a real shell would do.

use async_trait::async_trait;
use hooks::{CwdChangedFire, CwdChangedFirer};
use serde_json::json;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
use tool_api::Tool;
use tool_shell::BashTool;
use traits::process::{ProcessError, ProcessHandle, ProcessOutput, ProcessRunner};
use traits::sandbox::SandboxedCommand;

/// One captured spawn: the working directory passed to the runner and the
/// assembled command string (the LAST arg — `bash -c -l <cmd>` after BASH.4's
/// login-shell flag, so the command is no longer at a fixed index).
#[derive(Clone)]
struct Spawn {
    cwd: Option<PathBuf>,
    command: String,
}

/// Recording `ProcessRunner`. Each foreground `run` (and background
/// `spawn_background`) is captured. Before returning, `run` pops the next
/// `sim_pwd` entry and, if `Some(path)`, writes that path into the readback
/// tracking file — simulating the shell's `pwd -P` after a `cd`.
struct RecordingRunner {
    fg: Mutex<Vec<Spawn>>,
    bg: Mutex<Vec<Spawn>>,
    sim_pwd: Mutex<VecDeque<Option<PathBuf>>>,
    output: ProcessOutput,
    bg_ok: bool,
}

impl RecordingRunner {
    fn new(sim_pwd: Vec<Option<PathBuf>>, bg_ok: bool) -> Arc<Self> {
        Arc::new(Self {
            fg: Mutex::new(Vec::new()),
            bg: Mutex::new(Vec::new()),
            sim_pwd: Mutex::new(sim_pwd.into_iter().collect()),
            output: ProcessOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
                timed_out: false,
            },
            bg_ok,
        })
    }
}

/// Parse the cwd tracking file out of a ` && pwd -P >| '<path>'` readback.
fn extract_cwd_file(command: &str) -> Option<PathBuf> {
    let marker = "pwd -P >| ";
    let idx = command.rfind(marker)?;
    let rest = command[idx + marker.len()..].trim();
    let inner = rest.strip_prefix('\'')?.strip_suffix('\'')?;
    // Undo the shell single-quote escaping `'\''` → `'`.
    Some(PathBuf::from(inner.replace("'\\''", "'")))
}

#[async_trait]
impl ProcessRunner for RecordingRunner {
    async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
        let inner = cmd.inner();
        // The command string is the LAST arg (`-c -l <cmd>` after BASH.4).
        let command = inner.args.last().cloned().unwrap_or_default();
        // Simulate `pwd -P` writing the post-`cd` physical cwd to the tracking
        // file the tool appended to the command.
        let sim = self.sim_pwd.lock().unwrap().pop_front().flatten();
        if let Some(target) = sim {
            if let Some(file) = extract_cwd_file(&command) {
                std::fs::write(&file, format!("{}\n", target.display())).unwrap();
            }
        }
        self.fg.lock().unwrap().push(Spawn {
            cwd: inner.cwd.clone(),
            command,
        });
        Ok(self.output.clone())
    }

    async fn spawn_background(
        &self,
        cmd: &SandboxedCommand,
    ) -> Result<ProcessHandle, ProcessError> {
        let inner = cmd.inner();
        self.bg.lock().unwrap().push(Spawn {
            cwd: inner.cwd.clone(),
            command: inner.args.last().cloned().unwrap_or_default(),
        });
        if self.bg_ok {
            Ok(ProcessHandle {
                task_id: "task-bg-1".into(),
                pid: 1234,
            })
        } else {
            Err(ProcessError::Unsupported)
        }
    }

    async fn kill(&self, _handle: &ProcessHandle) -> Result<(), ProcessError> {
        Ok(())
    }

    fn is_available(&self) -> bool {
        true
    }
}

/// Build a `BashTool` whose workspace is `workspace` and whose process runner
/// is the supplied recorder. `workspace` seeds `shell_cwd` via `BashTool::new`.
fn tool_with(workspace: &std::path::Path, runner: Arc<RecordingRunner>) -> BashTool {
    let mut ctx = shell_test_ctx(ProcessOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        timed_out: false,
    });
    ctx.workspace = workspace.to_path_buf();
    ctx.process = runner;
    BashTool::new(ctx)
}

#[tokio::test]
async fn foreground_cd_persists_to_next_call() {
    let workspace = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let target_canon = std::fs::canonicalize(target.path()).unwrap();

    // First call simulates `cd <target>` (runner writes target into the
    // readback file); second call writes nothing.
    let runner = RecordingRunner::new(vec![Some(target.path().to_path_buf()), None], false);
    let tool = tool_with(workspace.path(), runner.clone());

    tool.call(
        json!({ "command": format!("cd {}", target.path().display()) }),
        fresh_ctx(),
        fresh_tx(),
    )
    .await
    .expect("first call ok");

    tool.call(json!({ "command": "pwd" }), fresh_ctx(), fresh_tx())
        .await
        .expect("second call ok");

    let fg = runner.fg.lock().unwrap();
    assert_eq!(fg.len(), 2, "two foreground spawns expected");
    // First spawn runs under the initial workspace.
    assert_eq!(fg[0].cwd.as_deref(), Some(workspace.path()));
    // Foreground command carries the readback redirect.
    assert!(
        fg[0].command.contains(" && pwd -P >| "),
        "foreground cmd should append pwd readback, got: {}",
        fg[0].command
    );
    // Second spawn inherits the canonicalized post-`cd` cwd.
    assert_eq!(
        fg[1].cwd.as_deref(),
        Some(target_canon.as_path()),
        "second call should spawn under the cd'd directory",
    );
}

#[tokio::test]
async fn background_does_not_mutate_shell_cwd_and_omits_readback() {
    let workspace = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();

    // sim_pwd would only fire on a foreground `run`; the bg call never reads
    // the file. Provide one entry for the trailing foreground probe (None).
    let runner = RecordingRunner::new(vec![None], true);
    let tool = tool_with(workspace.path(), runner.clone());

    // Background `cd` — must not change the shared cwd.
    tool.call(
        json!({
            "command": format!("cd {}", target.path().display()),
            "run_in_background": true,
        }),
        fresh_ctx(),
        fresh_tx(),
    )
    .await
    .expect("background call ok");

    // A later foreground call must still spawn under the original workspace.
    tool.call(json!({ "command": "pwd" }), fresh_ctx(), fresh_tx())
        .await
        .expect("foreground call ok");

    let bg = runner.bg.lock().unwrap();
    assert_eq!(bg.len(), 1, "one background spawn expected");
    assert_eq!(bg[0].cwd.as_deref(), Some(workspace.path()));
    assert!(
        !bg[0].command.contains("pwd -P >|"),
        "background cmd must NOT append the readback, got: {}",
        bg[0].command
    );

    let fg = runner.fg.lock().unwrap();
    assert_eq!(fg.len(), 1);
    assert_eq!(
        fg[0].cwd.as_deref(),
        Some(workspace.path()),
        "background call must not have mutated the shared cwd",
    );
}

#[tokio::test]
async fn subagent_does_not_mutate_shared_cwd() {
    let workspace = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();

    // Both calls simulate a successful `cd <target>` readback. The first runs
    // as a subagent (agent_id set) and must NOT update the shared cwd.
    let runner = RecordingRunner::new(
        vec![
            Some(target.path().to_path_buf()),
            Some(target.path().to_path_buf()),
        ],
        false,
    );
    let tool = tool_with(workspace.path(), runner.clone());

    let mut sub_ctx = fresh_ctx();
    sub_ctx.agent_id = Some(protocol::AgentId::new());

    tool.call(
        json!({ "command": format!("cd {}", target.path().display()) }),
        sub_ctx,
        fresh_tx(),
    )
    .await
    .expect("subagent call ok");

    // A subsequent main-thread call must still see the original workspace,
    // proving the subagent's `cd` did not leak into the shared cwd.
    tool.call(json!({ "command": "pwd" }), fresh_ctx(), fresh_tx())
        .await
        .expect("main call ok");

    let fg = runner.fg.lock().unwrap();
    assert_eq!(fg.len(), 2);
    assert_eq!(
        fg[1].cwd.as_deref(),
        Some(workspace.path()),
        "subagent cd must not persist to a later main-thread call",
    );
}

#[tokio::test]
async fn deleted_cwd_recovers_to_workspace() {
    let workspace = TempDir::new().unwrap();
    let gone = TempDir::new().unwrap();
    let gone_path = gone.path().to_path_buf();

    // First call moves the shell into `gone`; then `gone` is deleted before the
    // second call, forcing the deleted-cwd recovery path.
    let runner = RecordingRunner::new(vec![Some(gone_path.clone()), None], false);
    let tool = tool_with(workspace.path(), runner.clone());

    tool.call(
        json!({ "command": format!("cd {}", gone_path.display()) }),
        fresh_ctx(),
        fresh_tx(),
    )
    .await
    .expect("first call ok");

    // Delete the directory the shell cwd now points at.
    drop(gone);

    tool.call(json!({ "command": "pwd" }), fresh_ctx(), fresh_tx())
        .await
        .expect("second call recovers");

    let fg = runner.fg.lock().unwrap();
    assert_eq!(fg.len(), 2);
    assert_eq!(
        fg[1].cwd.as_deref(),
        Some(workspace.path()),
        "deleted cwd must recover to the workspace",
    );
}

#[tokio::test]
async fn deleted_cwd_and_workspace_yields_locked_error() {
    // Workspace itself is gone, so recovery has nowhere to fall back to.
    let dir = TempDir::new().unwrap();
    let ghost = dir.path().to_path_buf();
    drop(dir); // both shell_cwd (seeded from workspace) and workspace are gone

    let runner = RecordingRunner::new(vec![], false);
    let tool = tool_with(&ghost, runner.clone());

    let err = tool
        .call(json!({ "command": "pwd" }), fresh_ctx(), fresh_tx())
        .await
        .expect_err("missing workspace must error");

    let expected = format!(
        "Working directory \"{}\" no longer exists. Please restart Claude from an existing directory.",
        ghost.display()
    );
    assert!(
        err.to_string().contains(&expected),
        "expected locked recovery-failure string, got: {err}",
    );
    // No spawn should have happened.
    assert!(runner.fg.lock().unwrap().is_empty());
}

// ===== BASH.4 CwdChanged hook firer (Shell.ts:409 onCwdChangedForHooks) ======

/// Records every `(old, new)` it is fired with, so a test can assert the
/// `BashTool` fires `CwdChanged` exactly when the cwd moves.
struct RecordingFirer {
    fires: Mutex<Vec<(PathBuf, PathBuf)>>,
}

impl RecordingFirer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            fires: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl CwdChangedFirer for RecordingFirer {
    async fn fire(&self, fire: CwdChangedFire) {
        self.fires.lock().unwrap().push((fire.old, fire.new));
    }
}

/// `tool_with`, but additionally injecting a `CwdChanged` firer via the
/// optional builder (the desktop-composition wiring path).
fn tool_with_firer(
    workspace: &std::path::Path,
    runner: Arc<RecordingRunner>,
    firer: Arc<dyn CwdChangedFirer>,
) -> BashTool {
    let mut ctx = shell_test_ctx(ProcessOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        timed_out: false,
    });
    ctx.workspace = workspace.to_path_buf();
    ctx.process = runner;
    BashTool::new(ctx).with_cwd_changed_firer(firer)
}

#[tokio::test]
async fn cwd_change_fires_cwd_changed_hook_with_old_and_new() {
    // ONE tempdir, and the `cd` target is a real subdirectory inside it. Both
    // `old` and `new` derive from the SAME canonical root, so there is no second
    // independent `TempDir` whose creation/canonicalization can drift under the
    // macOS fseventsd/APFS churn the harness is prone to (a known non-code flake).
    let workspace = TempDir::new().unwrap();
    let workspace_canon = std::fs::canonicalize(workspace.path()).unwrap();
    let sub_canon = workspace_canon.join("sub");
    std::fs::create_dir(&sub_canon).unwrap();

    // Seed the shell cwd from the CANONICAL workspace so `old` is deterministic
    // (no `/var` vs `/private/var` raw-vs-canonical ambiguity). Feed the runner
    // the exact canonical target as the `pwd -P` readback.
    let runner = RecordingRunner::new(vec![Some(sub_canon.clone())], false);
    let firer = RecordingFirer::new();
    let tool = tool_with_firer(&workspace_canon, runner, firer.clone());

    tool.call(
        json!({ "command": format!("cd {}", sub_canon.display()) }),
        fresh_ctx(),
        fresh_tx(),
    )
    .await
    .expect("call ok");

    let recorded = firer.fires.lock().unwrap();
    assert_eq!(recorded.len(), 1, "exactly one CwdChanged fire expected");
    // 1:1 onCwdChangedForHooks(cwd, newCwd): `old` = prior shell cwd (canonical
    // workspace), `new` = canonicalized post-`cd` cwd (the `sub` directory).
    assert_eq!(
        recorded[0],
        (workspace_canon, sub_canon),
        "CwdChanged must carry (old=workspace, new=sub)"
    );
}

#[tokio::test]
async fn no_cwd_change_does_not_fire() {
    let workspace = TempDir::new().unwrap();

    // No `cd`: the readback writes nothing, so the cwd is unchanged and the
    // firer must NOT be invoked (claude-code's `oldCwd !== newCwd` guard).
    let runner = RecordingRunner::new(vec![None], false);
    let firer = RecordingFirer::new();
    let tool = tool_with_firer(workspace.path(), runner, firer.clone());

    tool.call(json!({ "command": "echo hi" }), fresh_ctx(), fresh_tx())
        .await
        .expect("call ok");

    assert!(
        firer.fires.lock().unwrap().is_empty(),
        "no cwd change => no CwdChanged fire",
    );
}

#[tokio::test]
async fn no_firer_registered_is_a_silent_noop() {
    // ONE tempdir + a real `sub` subdir (same churn-robust shape as the firing
    // test) so the canonical paths can't drift under the harness FS flake.
    let workspace = TempDir::new().unwrap();
    let workspace_canon = std::fs::canonicalize(workspace.path()).unwrap();
    let sub_canon = workspace_canon.join("sub");
    std::fs::create_dir(&sub_canon).unwrap();

    // `tool_with` builds the plain `BashTool::new(ctx)` (no firer) — the mobile /
    // plain-caller path. A `cd` still updates the persistent cwd (proving the
    // fire is purely additive), but no firer is invoked (nothing to assert about
    // it — the point is no panic and identical cwd behavior).
    let runner = RecordingRunner::new(vec![Some(sub_canon.clone()), None], false);
    let tool = tool_with(&workspace_canon, runner.clone());

    tool.call(
        json!({ "command": format!("cd {}", sub_canon.display()) }),
        fresh_ctx(),
        fresh_tx(),
    )
    .await
    .expect("first call ok (no firer registered)");

    tool.call(json!({ "command": "pwd" }), fresh_ctx(), fresh_tx())
        .await
        .expect("second call ok");

    // The cwd still moved despite no firer — the fire is best-effort/no-op only.
    let fg = runner.fg.lock().unwrap();
    assert_eq!(fg.len(), 2);
    assert_eq!(
        fg[1].cwd.as_deref(),
        Some(sub_canon.as_path()),
        "cwd update is unchanged when no firer is registered",
    );
}
