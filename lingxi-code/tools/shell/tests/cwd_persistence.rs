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
/// assembled `bash -c` command string (args[1]).
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
        let command = inner.args.get(1).cloned().unwrap_or_default();
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
            command: inner.args.get(1).cloned().unwrap_or_default(),
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
