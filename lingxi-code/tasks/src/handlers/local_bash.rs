//! Local-bash task handler — M2 implementation.
//!
//! Spawns a `bash -c "<command>"` child through the [`Sandbox`] +
//! [`ProcessRunner`] traits (the D2 / spec A1 sandbox-decision invariant) and
//! streams its captured stdout/stderr into the task's spool file, mirroring the
//! claude-code `LocalShellTask` lifecycle (`spawnShellTask` → `killTask`).
//!
//! ## Why `run()` and not `spawn_background()`
//!
//! [`crate::task_trait::Task`] hands the handler a [`TaskContext`] carrying only
//! `fs` + `runtime`; the engine must not call `tokio::spawn` directly (D17), so
//! the worker runs inside `ctx.runtime.spawn(..)`. Inside that future the child
//! is executed with [`ProcessRunner::run`], whose [`ProcessOutput`] is appended
//! to the spool via [`FileSystem::append_file`].
//!
//! We deliberately do *not* use [`ProcessRunner::spawn_background`] for the
//! primary execution path: a [`ProcessHandle`] exposes no live stdout/stderr
//! stream, and the platform `spawn_background` wires the child's fds to a
//! *runner-private* output file the handler cannot read back through the trait
//! (see `platforms/posix/src/process/runner.rs`). `run()` is the only way to
//! capture output for the spool that tests/UI read. The trade-off is that a
//! pure-`run()` worker has no OS handle to hand to [`ProcessRunner::kill`];
//! cancellation therefore rides on cooperatively dropping the runtime future
//! (the runtime's documented `cancel` contract) plus flipping status to
//! `Killed`. When a caller opts into background streaming
//! ([`LocalBashHandler::with_background_spawn`]) the handler records the real
//! [`ProcessHandle`] in `children` so [`Task::kill`] can issue a true
//! [`ProcessRunner::kill`].
//!
//! ## WARNING: background-streaming mode runs the command TWICE
//!
//! With `use_background_spawn=true` the handler launches **two** independent OS
//! children for one logical task: `spawn_background()` starts a detached
//! (`setsid`) child whose output goes to a runner-private file, and the worker
//! *also* calls `run()` to capture output for the spool. A side-effecting
//! command (writes a file, mutates state, sends a request) therefore executes
//! twice. The `run()` copy's exit drives status/spool; the detached copy is
//! only terminable via [`Task::kill`] — if `kill` is never called it runs to
//! completion regardless. Enable this mode only for commands that are
//! idempotent / observe-only, or accept the double-execution. The default
//! (`false`) executes the command exactly once via `run()`.

use crate::id::TaskType;
use crate::output_manager::TaskOutputManager;
use crate::state::TaskStatus;
use crate::task_trait::{Task, TaskContext, TaskError, TaskHandle, TaskSpawnInput};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use traits::{ProcessCommand, ProcessError, ProcessHandle, ProcessOutput, ProcessRunner, Sandbox};

/// Audit reason recorded on the sandboxed command when no policy is supplied,
/// mirroring the hooks Command arm's `"hook_command"` bypass tag
/// (`hooks/src/executor.rs:276`).
const BYPASS_REASON: &str = "local_bash_task";

/// Handler name reported by [`Task::name`] / used as the runtime task-name
/// prefix. Byte-aligned with the `"local_bash"` wire string in `handle.rs`.
const HANDLER_NAME: &str = "local_bash";

/// Sink for terminal-status / exit-code updates produced by the background
/// worker once the child completes (or is killed).
///
/// The handler does not own a [`crate::registry::TaskRegistry`] reference
/// (it would be a cyclic / over-broad dependency), so terminal transitions are
/// reported through this narrow seam instead. The wire-step that registers the
/// handler can plug in an adapter that calls `registry.set_status(..)` and
/// writes `LocalBashTaskState.exit_code`. The default [`NoopStatusSink`] makes
/// the handler usable standalone (and in unit tests).
#[async_trait]
pub trait TaskStatusSink: Send + Sync {
    /// Record the task's final status. Called exactly once per task on
    /// completion, failure, timeout, or kill.
    async fn set_status(&self, task_id: &str, status: TaskStatus);

    /// Record the child's exit code (the value from
    /// [`ProcessOutput::exit_code`]). Called once on natural completion.
    async fn set_exit_code(&self, _task_id: &str, _exit_code: i32) {}

    /// Record the child's OS pid once known. Called when a background handle
    /// is obtained (streaming mode only).
    async fn set_pid(&self, _task_id: &str, _pid: u32) {}
}

/// No-op [`TaskStatusSink`] — the default when the handler is constructed
/// without an explicit sink. State transitions become observable only through
/// the spool file in this mode.
pub struct NoopStatusSink;

#[async_trait]
impl TaskStatusSink for NoopStatusSink {
    async fn set_status(&self, _task_id: &str, _status: TaskStatus) {}
}

/// Local-bash task handler.
///
/// Holds the constructor-injected execution dependencies that are *not*
/// available on [`TaskContext`] ([`ProcessRunner`] + [`Sandbox`]), the spool
/// manager, the terminal-status sink, and the shared child-handle map keyed by
/// `task_id` so [`Task::kill`] can recover a [`ProcessHandle`].
pub struct LocalBashHandler {
    /// Runs the sandboxed bash command (and, in streaming mode, kills it).
    process: Arc<dyn ProcessRunner>,
    /// Mints the [`traits::SandboxedCommand`] the runner accepts (D2 / A1).
    sandbox: Arc<dyn Sandbox>,
    /// Owns the spool directory + path allocation for stdout/stderr.
    output_manager: Arc<TaskOutputManager>,
    /// Where terminal status / exit-code transitions are reported.
    status_sink: Arc<dyn TaskStatusSink>,
    /// `task_id` → live child handle. Only populated in background-streaming
    /// mode; [`Task::kill`] removes + kills the handle if present.
    children: Arc<Mutex<HashMap<String, ProcessHandle>>>,
    /// `task_id` → handle queued for teardown by the synchronous
    /// [`TaskHandle::cleanup`] closure (which cannot await). Drained by
    /// [`LocalBashHandler::drain_pending_kills`], the async seam the
    /// registry/cleanup-registry calls on agent exit.
    pending_kill: Arc<Mutex<HashMap<String, ProcessHandle>>>,
    /// When `true`, also call [`ProcessRunner::spawn_background`] to obtain a
    /// real [`ProcessHandle`] for the `children` map (enabling a true
    /// `ProcessRunner::kill`). Off by default to keep the child executed
    /// exactly once via `run()`.
    use_background_spawn: bool,
}

impl LocalBashHandler {
    /// Construct a handler with the injected execution dependencies.
    ///
    /// `fs` and `runtime` arrive per-call via [`TaskContext`] and are
    /// deliberately *not* injected here. `process` + `sandbox` are required
    /// because they are absent from [`TaskContext`] yet the handler cannot run
    /// a command without them — exactly as
    /// `HookExecutorImpl::with_process_runner` wires them.
    #[must_use]
    pub fn new(
        process: Arc<dyn ProcessRunner>,
        sandbox: Arc<dyn Sandbox>,
        output_manager: Arc<TaskOutputManager>,
    ) -> Self {
        Self {
            process,
            sandbox,
            output_manager,
            status_sink: Arc::new(NoopStatusSink),
            children: Arc::new(Mutex::new(HashMap::new())),
            pending_kill: Arc::new(Mutex::new(HashMap::new())),
            use_background_spawn: false,
        }
    }

    /// Attach a [`TaskStatusSink`] so terminal transitions / exit codes are
    /// reported (e.g. an adapter over [`crate::registry::TaskRegistry`]).
    #[must_use]
    pub fn with_status_sink(mut self, sink: Arc<dyn TaskStatusSink>) -> Self {
        self.status_sink = sink;
        self
    }

    /// Opt into background-streaming mode: in addition to `run()`, obtain a
    /// real [`ProcessHandle`] via [`ProcessRunner::spawn_background`] and record
    /// it in `children` so [`Task::kill`] can issue a true
    /// [`ProcessRunner::kill`]. The streamed child's output lands in the
    /// runner's own file (see module docs), so the spool is still populated
    /// from the `run()` capture.
    ///
    /// WARNING: this executes the command **twice** (one detached child via
    /// `spawn_background`, one captured child via `run()`). See the module-level
    /// "background-streaming mode runs the command TWICE" warning. Leave this
    /// `false` (the default) for commands with side effects.
    #[must_use]
    pub fn with_background_spawn(mut self, enabled: bool) -> Self {
        self.use_background_spawn = enabled;
        self
    }

    /// Share the same `children` map with an external owner (e.g. the registry
    /// wiring) so a [`TaskHandle::cleanup`] closure and [`Task::kill`] observe
    /// the same handles.
    #[must_use]
    pub fn children_map(&self) -> Arc<Mutex<HashMap<String, ProcessHandle>>> {
        self.children.clone()
    }

    /// Drain handles queued by [`TaskHandle::cleanup`] and kill each child for
    /// real. This is the async counterpart of the synchronous cleanup closure:
    /// the registry/cleanup-registry calls it on agent teardown so a child
    /// outliving its agent is terminated (claude-code `killTask`-on-cleanup
    /// parity). A queued handle with `pid == 0` is a pure-`run()` task that had
    /// no OS child — it only flips status to `Killed`.
    pub async fn drain_pending_kills(&self) {
        let pending: Vec<(String, ProcessHandle)> =
            self.pending_kill.lock().await.drain().collect();
        for (task_id, handle) in pending {
            if handle.pid != 0 {
                let _ = self.process.kill(&handle).await;
            }
            self.status_sink.set_status(&task_id, TaskStatus::Killed).await;
        }
    }

    /// Build the `bash -c "<command>"` [`ProcessCommand`], carrying the
    /// optional timeout (the runner enforces it and reports
    /// [`ProcessError::Timeout`] / [`ProcessOutput::timed_out`]).
    fn build_process_command(
        command: String,
        timeout: Option<std::time::Duration>,
    ) -> ProcessCommand {
        ProcessCommand {
            command: "bash".into(),
            args: vec!["-c".into(), command],
            cwd: None,
            env: HashMap::new(),
            timeout,
            stdin: None,
        }
    }

    /// Map a [`ProcessRunner::run`] result onto the terminal status to report,
    /// honoring `timed_out` (→ `Killed`) and the exit code (`0` → `Completed`,
    /// non-zero → `Failed`). Returns `(status, exit_code)`.
    fn classify(result: &Result<ProcessOutput, ProcessError>) -> (TaskStatus, Option<i32>) {
        match result {
            Ok(o) if o.timed_out => (TaskStatus::Killed, Some(o.exit_code)),
            Ok(o) if o.exit_code == 0 => (TaskStatus::Completed, Some(o.exit_code)),
            Ok(o) => (TaskStatus::Failed, Some(o.exit_code)),
            Err(ProcessError::Timeout) => (TaskStatus::Killed, None),
            Err(_) => (TaskStatus::Failed, None),
        }
    }
}

#[async_trait]
impl Task for LocalBashHandler {
    fn name(&self) -> &str {
        HANDLER_NAME
    }

    fn task_type(&self) -> TaskType {
        TaskType::LocalBash
    }

    async fn spawn(
        &self,
        input: TaskSpawnInput,
        ctx: TaskContext,
    ) -> Result<TaskHandle, TaskError> {
        // 1. Only the LocalBash variant is accepted; reject the other six.
        let TaskSpawnInput::LocalBash { command, timeout } = input else {
            return Err(TaskError::Internal(
                "local_bash handler received a non-LocalBash spawn input".into(),
            ));
        };

        // 2. Generate the task id and allocate its spool file.
        let task_id = crate::id::generate_task_id(TaskType::LocalBash);
        let spool_path = self
            .output_manager
            .allocate(&task_id)
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;
        let spool_path_str = spool_path
            .to_str()
            .ok_or_else(|| TaskError::Internal("spool path is not valid UTF-8".into()))?
            .to_owned();

        // 3. Build the bash command.
        let pcmd = Self::build_process_command(command, timeout);

        // 4. Mint the SandboxedCommand. Mirror the hooks Command arm: with no
        //    policy in scope, an audited bypass is the parity-honest construction
        //    (hooks/src/executor.rs:276).
        let sandboxed = self.sandbox.bypass_with_audit(pcmd, BYPASS_REASON);

        // 4b. Optional background handle for a real ProcessRunner::kill seam.
        if self.use_background_spawn {
            match self.process.spawn_background(&sandboxed).await {
                Ok(handle) => {
                    self.status_sink.set_pid(&task_id, handle.pid).await;
                    self.children.lock().await.insert(task_id.clone(), handle);
                }
                Err(e) => {
                    self.status_sink.set_status(&task_id, TaskStatus::Failed).await;
                    return Err(TaskError::Io(e.to_string()));
                }
            }
        }

        // 5. Drive the child to completion inside a runtime-spawned worker
        //    (engine code must not call tokio::spawn directly — D17). The worker
        //    captures output via run(), appends it to the spool, and reports the
        //    terminal status / exit code.
        let process = self.process.clone();
        let status_sink = self.status_sink.clone();
        let children = self.children.clone();
        let fs = ctx.fs.clone();
        let worker_task_id = task_id.clone();
        let worker = Box::pin(async move {
            status_sink
                .set_status(&worker_task_id, TaskStatus::Running)
                .await;

            let result = process.run(&sandboxed).await;

            // Append captured output to the spool (best effort — spool I/O
            // failure must not mask the command result). Append with O_NOFOLLOW
            // (claude-code `diskOutput.ts`) so a symlink planted at the spool
            // path from inside the sandbox cannot redirect the write (T18).
            if let Ok(out) = &result {
                if !out.stdout.is_empty() {
                    let _ = fs.append_file_no_follow(&spool_path_str, &out.stdout).await;
                }
                if !out.stderr.is_empty() {
                    let _ = fs.append_file_no_follow(&spool_path_str, &out.stderr).await;
                }
            }

            let (status, exit_code) = LocalBashHandler::classify(&result);
            if let Some(code) = exit_code {
                status_sink.set_exit_code(&worker_task_id, code).await;
            }
            status_sink.set_status(&worker_task_id, status).await;

            // The child has exited; drop any recorded handle so a late kill is
            // a graceful no-op (claude-code `status !== 'running'` early return).
            children.lock().await.remove(&worker_task_id);
        });

        ctx.runtime
            .spawn(&format!("{HANDLER_NAME}:{task_id}"), worker)
            .await
            .map_err(|e| TaskError::Internal(e.to_string()))?;

        // 6. Build the cleanup seam (claude-code `registerCleanup` parity).
        //    The closure is *synchronous* (the `TaskHandle::cleanup` type is
        //    `Fn()`), while the only OS-kill primitive — `ProcessRunner::kill`
        //    — is async, and engine code must not spawn its own runtime tasks
        //    (D17). So cleanup cannot itself await a kill. Its honest job is to
        //    flag the child for teardown by moving any live handle into the
        //    shared `pending_kill` queue, which the registry/cleanup-registry
        //    drains by calling the async `Task::kill`. Authoritative OS
        //    termination therefore always flows through `Task::kill`.
        let cleanup_children = self.children.clone();
        let cleanup_pending = self.pending_kill.clone();
        let cleanup_task_id = task_id.clone();
        let cleanup: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let (Ok(mut children), Ok(mut pending)) =
                (cleanup_children.try_lock(), cleanup_pending.try_lock())
            {
                if let Some(handle) = children.remove(&cleanup_task_id) {
                    pending.insert(cleanup_task_id.clone(), handle);
                } else {
                    // Even with no live handle, record intent so a later drain
                    // flips status to Killed for a still-pending task.
                    pending
                        .entry(cleanup_task_id.clone())
                        .or_insert_with(|| ProcessHandle {
                            task_id: cleanup_task_id.clone(),
                            pid: 0,
                        });
                }
            }
        });

        Ok(TaskHandle {
            task_id,
            cleanup: Some(cleanup),
        })
    }

    async fn kill(&self, task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
        // Recover the live handle (if any) and issue a real kill. An absent
        // handle ⇒ the child already exited (or pure-run mode) ⇒ graceful no-op,
        // mirroring claude-code's `status !== 'running'` early return.
        let handle = self.children.lock().await.remove(task_id);
        if let Some(h) = handle {
            self.process
                .kill(&h)
                .await
                .map_err(|e| TaskError::Io(e.to_string()))?;
        }
        // Flip status to Killed regardless.
        //
        // NOTE on the in-flight worker future: this handler discards the
        // `BackgroundTaskHandle` that `ctx.runtime.spawn` returns in `spawn()`
        // (it has no map to record it in that `kill` can reach), so `kill` does
        // NOT abort the worker future — the `run()` it is awaiting drives to
        // completion. In pure-`run()` mode that is harmless (no OS child handle
        // exists to leak; the foreground runner's `kill_on_drop` covers the
        // timeout path). In background-streaming mode the `ProcessRunner::kill`
        // above is what authoritatively terminates the OS child; the worker's
        // own `run()` copy then exits naturally. Authoritative worker-future
        // cancellation (handing this handle to the registry's
        // `RuntimeSpawner::cancel`) is a follow-up once the registry drives
        // `Task::spawn` and records the returned handle.
        self.status_sink.set_status(task_id, TaskStatus::Killed).await;
        Ok(())
    }

    fn supports_messages(&self) -> bool {
        false
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::state::TaskStatus;
    use std::collections::HashMap as StdHashMap;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;
    use test_harness::mocks::MockRuntimeSpawner;
    use tokio::sync::Mutex as TokioMutex;
    use traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
    use traits::sandbox::{SandboxBackend, SandboxCapability, SandboxedTag};
    use traits::{ProcessCommand, SandboxError, SandboxPolicy, SandboxedCommand};

    // ---- In-memory FileSystem (mirrors handle.rs InMemoryFs) ---------------

    struct InMemoryFs {
        files: TokioMutex<StdHashMap<String, String>>,
    }
    impl InMemoryFs {
        fn new() -> Self {
            Self {
                files: TokioMutex::new(StdHashMap::new()),
            }
        }
    }
    #[async_trait]
    impl FileSystem for InMemoryFs {
        async fn read_file(
            &self,
            path: &str,
            offset: Option<u64>,
            limit: Option<u64>,
        ) -> Result<FileContent, FsError> {
            let map = self.files.lock().await;
            let content = map.get(path).cloned().unwrap_or_default();
            let off = usize::try_from(offset.unwrap_or(0)).unwrap_or(usize::MAX);
            let body: String = content.chars().skip(off).collect();
            let truncated = limit.is_some_and(|lim| body.len() as u64 > lim);
            let trimmed = match limit {
                Some(lim) => body
                    .chars()
                    .take(usize::try_from(lim).unwrap_or(usize::MAX))
                    .collect(),
                None => body,
            };
            let total_lines = content.lines().count() as u64;
            Ok(FileContent {
                content: trimmed,
                truncated,
                total_lines,
            })
        }
        async fn write_file(&self, path: &str, body: &str) -> Result<(), FsError> {
            self.files
                .lock()
                .await
                .insert(path.to_string(), body.to_string());
            Ok(())
        }
        fn is_within_workspace(&self, _: &str) -> bool {
            true
        }
        async fn watch(
            &self,
            _: &str,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError>
        {
            Err(FsError::Io("not supported".into()))
        }
        async fn append_file(&self, path: &str, body: &str) -> Result<(), FsError> {
            let mut map = self.files.lock().await;
            map.entry(path.to_string()).or_default().push_str(body);
            Ok(())
        }
        async fn truncate(&self, path: &str, len: u64) -> Result<(), FsError> {
            let mut map = self.files.lock().await;
            if let Some(s) = map.get_mut(path) {
                s.truncate(usize::try_from(len).unwrap_or(usize::MAX));
            }
            Ok(())
        }
        async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
            Ok(std::time::SystemTime::UNIX_EPOCH)
        }
        async fn file_size(&self, path: &str) -> Result<u64, FsError> {
            let map = self.files.lock().await;
            Ok(map.get(path).map_or(0, |s| s.len() as u64))
        }
        async fn delete_file(&self, path: &str) -> Result<(), FsError> {
            self.files.lock().await.remove(path);
            Ok(())
        }
        async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> {
            Ok(())
        }
        async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
            Err(FsError::Io("not supported".into()))
        }
        async fn fsync(&self, _: &str) -> Result<(), FsError> {
            Ok(())
        }
    }

    // ---- Mock ProcessRunner (records the command it ran / killed) ----------

    struct MockRunner {
        output: ProcessOutput,
        ran_command: StdMutex<Option<Vec<String>>>,
        spawned_background: StdMutex<bool>,
        killed: StdMutex<Vec<u32>>,
        bg_pid: u32,
    }
    impl MockRunner {
        fn new(output: ProcessOutput) -> Arc<Self> {
            Arc::new(Self {
                output,
                ran_command: StdMutex::new(None),
                spawned_background: StdMutex::new(false),
                killed: StdMutex::new(Vec::new()),
                bg_pid: 4242,
            })
        }
        fn ran_args(&self) -> Option<Vec<String>> {
            self.ran_command.lock().unwrap().clone()
        }
        fn killed_pids(&self) -> Vec<u32> {
            self.killed.lock().unwrap().clone()
        }
        fn did_spawn_background(&self) -> bool {
            *self.spawned_background.lock().unwrap()
        }
    }
    #[async_trait]
    impl ProcessRunner for MockRunner {
        async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            let inner = cmd.inner();
            let mut recorded = vec![inner.command.clone()];
            recorded.extend(inner.args.clone());
            *self.ran_command.lock().unwrap() = Some(recorded);
            Ok(self.output.clone())
        }
        async fn spawn_background(
            &self,
            _cmd: &SandboxedCommand,
        ) -> Result<ProcessHandle, ProcessError> {
            *self.spawned_background.lock().unwrap() = true;
            Ok(ProcessHandle {
                task_id: "bg".into(),
                pid: self.bg_pid,
            })
        }
        async fn kill(&self, handle: &ProcessHandle) -> Result<(), ProcessError> {
            self.killed.lock().unwrap().push(handle.pid);
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    // ---- Stub Sandbox (mints via the documented external-impl seam) --------

    struct StubSandbox {
        last_reason: StdMutex<Option<String>>,
    }
    impl StubSandbox {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                last_reason: StdMutex::new(None),
            })
        }
        fn reason(&self) -> Option<String> {
            self.last_reason.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl Sandbox for StubSandbox {
        fn is_available(&self) -> bool {
            true
        }
        fn backend(&self) -> SandboxBackend {
            SandboxBackend::None
        }
        fn prepare(
            &self,
            cmd: ProcessCommand,
            _policy: &SandboxPolicy,
        ) -> Result<SandboxedCommand, SandboxError> {
            Ok(SandboxedCommand::__new_sandboxed(
                cmd,
                SandboxedTag::BypassAuditedWithReason {
                    reason: "test".into(),
                },
            ))
        }
        fn bypass_with_audit(&self, cmd: ProcessCommand, reason: &str) -> SandboxedCommand {
            *self.last_reason.lock().unwrap() = Some(reason.to_string());
            SandboxedCommand::__new_sandboxed(
                cmd,
                SandboxedTag::BypassAuditedWithReason {
                    reason: reason.into(),
                },
            )
        }
        async fn probe_capability(&self) -> SandboxCapability {
            SandboxCapability {
                available: true,
                reason: None,
                features: traits::SandboxFeatures::default(),
            }
        }
    }

    // ---- Recording status sink ---------------------------------------------

    #[derive(Default)]
    struct RecordingSink {
        statuses: StdMutex<Vec<(String, TaskStatus)>>,
        exit_codes: StdMutex<Vec<(String, i32)>>,
        pids: StdMutex<Vec<(String, u32)>>,
    }
    #[async_trait]
    impl TaskStatusSink for RecordingSink {
        async fn set_status(&self, task_id: &str, status: TaskStatus) {
            self.statuses
                .lock()
                .unwrap()
                .push((task_id.to_string(), status));
        }
        async fn set_exit_code(&self, task_id: &str, exit_code: i32) {
            self.exit_codes
                .lock()
                .unwrap()
                .push((task_id.to_string(), exit_code));
        }
        async fn set_pid(&self, task_id: &str, pid: u32) {
            self.pids.lock().unwrap().push((task_id.to_string(), pid));
        }
    }
    impl RecordingSink {
        fn last_status(&self) -> Option<TaskStatus> {
            self.statuses.lock().unwrap().last().map(|(_, s)| *s)
        }
        fn exit_codes(&self) -> Vec<(String, i32)> {
            self.exit_codes.lock().unwrap().clone()
        }
        fn pids(&self) -> Vec<(String, u32)> {
            self.pids.lock().unwrap().clone()
        }
    }

    // ---- Helpers ------------------------------------------------------------

    fn output(stdout: &str, stderr: &str, exit_code: i32, timed_out: bool) -> ProcessOutput {
        ProcessOutput {
            stdout: stdout.into(),
            stderr: stderr.into(),
            exit_code,
            timed_out,
        }
    }

    fn make_ctx(fs: Arc<dyn FileSystem>) -> TaskContext {
        TaskContext {
            fs,
            runtime: Arc::new(MockRuntimeSpawner::default()),
        }
    }

    fn make_output_manager(fs: Arc<dyn FileSystem>) -> (tempfile::TempDir, Arc<TaskOutputManager>) {
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs));
        (dir, mgr)
    }

    /// Poll the sink until it has recorded a terminal status (or the budget is
    /// exhausted). The worker runs on the `MockRuntimeSpawner`'s tokio task, so a
    /// few yields let it finish deterministically without a fixed sleep.
    async fn await_terminal(sink: &Arc<RecordingSink>) -> TaskStatus {
        for _ in 0..200 {
            if let Some(s) = sink.last_status() {
                if s.is_terminal() {
                    return s;
                }
            }
            tokio::task::yield_now().await;
        }
        sink.last_status().expect("worker never reported a status")
    }

    // ---- Tests --------------------------------------------------------------

    #[tokio::test]
    async fn spawn_returns_handle_and_runs_bash_dash_c() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runner = MockRunner::new(output("hello\n", "", 0, false));
        let sandbox = StubSandbox::new();
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());

        let handler = LocalBashHandler::new(runner.clone(), sandbox.clone(), mgr)
            .with_status_sink(sink.clone());

        let handle = handler
            .spawn(
                TaskSpawnInput::LocalBash {
                    command: "echo hello".into(),
                    timeout: None,
                },
                make_ctx(fs),
            )
            .await
            .expect("spawn should succeed");

        assert!(handle.task_id.starts_with('b'), "LocalBash id prefix");
        assert!(handle.cleanup.is_some(), "cleanup seam is present");

        let status = await_terminal(&sink).await;
        assert_eq!(status, TaskStatus::Completed, "exit 0 ⇒ Completed");

        // Command ran as `bash -c "echo hello"`.
        let args = runner.ran_args().expect("run() must have been called");
        assert_eq!(args, vec!["bash", "-c", "echo hello"]);

        // Audited bypass reason matches the hooks-arm parity tag.
        assert_eq!(sandbox.reason().as_deref(), Some(BYPASS_REASON));

        // Exit code recorded.
        let codes = sink.exit_codes();
        assert_eq!(codes.len(), 1);
        assert_eq!(codes[0].1, 0);
    }

    #[tokio::test]
    async fn spawn_spools_stdout_and_stderr() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runner = MockRunner::new(output("out-line\n", "err-line\n", 0, false));
        let sandbox = StubSandbox::new();
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let sink = Arc::new(RecordingSink::default());

        let handler = LocalBashHandler::new(runner, sandbox, mgr.clone())
            .with_status_sink(sink.clone());

        let handle = handler
            .spawn(
                TaskSpawnInput::LocalBash {
                    command: "do stuff".into(),
                    timeout: None,
                },
                make_ctx(fs),
            )
            .await
            .unwrap();

        await_terminal(&sink).await;

        // The handler allocates `{task_id}.txt` under the manager's dir
        // (the tempdir). Read it straight back.
        let spool_path = dir.path().join(format!("{}.txt", handle.task_id));
        let read = mgr
            .read(&spool_path, crate::output_manager::OutputOptions::default())
            .await
            .unwrap();
        assert!(read.content.contains("out-line"), "stdout spooled");
        assert!(read.content.contains("err-line"), "stderr spooled");
    }

    #[tokio::test]
    async fn nonzero_exit_maps_to_failed() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runner = MockRunner::new(output("", "boom\n", 1, false));
        let sandbox = StubSandbox::new();
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());

        let handler =
            LocalBashHandler::new(runner, sandbox, mgr).with_status_sink(sink.clone());

        handler
            .spawn(
                TaskSpawnInput::LocalBash {
                    command: "false".into(),
                    timeout: None,
                },
                make_ctx(fs),
            )
            .await
            .unwrap();

        assert_eq!(await_terminal(&sink).await, TaskStatus::Failed);
    }

    #[tokio::test]
    async fn timed_out_output_maps_to_killed() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runner = MockRunner::new(output("", "", -1, true));
        let sandbox = StubSandbox::new();
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());

        let handler =
            LocalBashHandler::new(runner, sandbox, mgr).with_status_sink(sink.clone());

        handler
            .spawn(
                TaskSpawnInput::LocalBash {
                    command: "sleep 100".into(),
                    timeout: Some(std::time::Duration::from_millis(1)),
                },
                make_ctx(fs),
            )
            .await
            .unwrap();

        assert_eq!(await_terminal(&sink).await, TaskStatus::Killed);
    }

    #[tokio::test]
    async fn background_spawn_records_pid_and_kill_terminates_child() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        // Keep the run() future from finishing instantly so the handle survives
        // until kill: a never-timed-out long output is fine — the worker still
        // completes promptly with the mock, so we kill via the children map
        // before the worker's remove() lands by inspecting the recorded handle.
        let runner = MockRunner::new(output("", "", 0, false));
        let sandbox = StubSandbox::new();
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());

        let handler = LocalBashHandler::new(runner.clone(), sandbox, mgr)
            .with_status_sink(sink.clone())
            .with_background_spawn(true);

        // Grab the shared children map BEFORE spawn so we can re-insert a handle
        // deterministically for the kill assertion (the worker may have already
        // removed it after run() completed).
        let children = handler.children_map();

        let handle = handler
            .spawn(
                TaskSpawnInput::LocalBash {
                    command: "long".into(),
                    timeout: None,
                },
                make_ctx(fs),
            )
            .await
            .unwrap();

        assert!(runner.did_spawn_background(), "background spawn issued");

        // pid recorded into the sink.
        let pids = sink.pids();
        assert_eq!(pids.len(), 1);
        assert_eq!(pids[0].1, 4242);

        // Ensure a handle is present for the kill seam regardless of worker
        // timing (re-insert the same handle the runner hands out).
        children.lock().await.insert(
            handle.task_id.clone(),
            ProcessHandle {
                task_id: "bg".into(),
                pid: 4242,
            },
        );

        handler
            .kill(&handle.task_id, make_ctx(Arc::new(InMemoryFs::new())))
            .await
            .expect("kill should succeed");

        assert_eq!(runner.killed_pids(), vec![4242], "ProcessRunner::kill fired");
        assert_eq!(sink.last_status(), Some(TaskStatus::Killed));
    }

    #[tokio::test]
    async fn kill_absent_handle_is_graceful_noop() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runner = MockRunner::new(output("", "", 0, false));
        let sandbox = StubSandbox::new();
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());

        let handler =
            LocalBashHandler::new(runner.clone(), sandbox, mgr).with_status_sink(sink.clone());

        // No spawn ⇒ no handle. kill must still succeed and flip to Killed.
        handler
            .kill("bdeadbeef", make_ctx(fs))
            .await
            .expect("kill of unknown task is a no-op success");
        assert!(runner.killed_pids().is_empty(), "no child to kill");
        assert_eq!(sink.last_status(), Some(TaskStatus::Killed));
    }

    #[tokio::test]
    async fn non_localbash_input_is_rejected() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runner = MockRunner::new(output("", "", 0, false));
        let sandbox = StubSandbox::new();
        let (_dir, mgr) = make_output_manager(fs.clone());

        let handler = LocalBashHandler::new(runner, sandbox, mgr);

        let result = handler
            .spawn(
                TaskSpawnInput::LocalWorkflow {
                    workflow_id: "wf".into(),
                },
                make_ctx(fs),
            )
            .await;
        match result {
            Err(TaskError::Internal(_)) => {}
            Err(other) => panic!("expected Internal, got {other:?}"),
            Ok(_) => panic!("non-LocalBash input must be rejected"),
        }
    }

    #[tokio::test]
    async fn cleanup_then_drain_kills_streaming_child() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runner = MockRunner::new(output("", "", 0, false));
        let sandbox = StubSandbox::new();
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());

        let handler = LocalBashHandler::new(runner.clone(), sandbox, mgr)
            .with_status_sink(sink.clone())
            .with_background_spawn(true);

        let children = handler.children_map();
        let handle = handler
            .spawn(
                TaskSpawnInput::LocalBash {
                    command: "long".into(),
                    timeout: None,
                },
                make_ctx(fs),
            )
            .await
            .unwrap();

        // Re-seat a live handle so cleanup has something to enqueue regardless
        // of the worker's removal timing.
        children.lock().await.insert(
            handle.task_id.clone(),
            ProcessHandle {
                task_id: "bg".into(),
                pid: 4242,
            },
        );

        // Fire the synchronous cleanup closure (moves handle to pending_kill).
        (handle.cleanup.as_ref().unwrap())();

        // Drain performs the real async kill.
        handler.drain_pending_kills().await;

        assert_eq!(runner.killed_pids(), vec![4242], "drain killed the child");
        assert_eq!(sink.last_status(), Some(TaskStatus::Killed));
    }

    #[test]
    fn name_and_type_are_local_bash() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runner = MockRunner::new(output("", "", 0, false));
        let sandbox = StubSandbox::new();
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs));
        let handler = LocalBashHandler::new(runner, sandbox, mgr);
        assert_eq!(handler.name(), "local_bash");
        assert_eq!(handler.task_type(), TaskType::LocalBash);
        assert!(!handler.supports_messages());
    }
}
