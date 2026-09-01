//! Shell stdout event monitor (`monitor_ws`).

use crate::handlers::{NoopStatusSink, TaskStatusSink};
use crate::id::TaskType;
use crate::output_manager::TaskOutputManager;
use crate::state::TaskStatus;
use crate::task_trait::{Task, TaskContext, TaskError, TaskHandle, TaskSpawnInput};
use async_trait::async_trait;
use platform_api::{
    BackgroundTaskHandle, ProcessCommand, ProcessError, ProcessRunner, ProcessStreamSink,
    RuntimeSpawner, Sandbox,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

const HANDLER_NAME: &str = "monitor_ws";
const BYPASS_REASON: &str = "monitor_task";
const BATCH_WINDOW: Duration = Duration::from_millis(200);
const TOKEN_CAPACITY: f64 = 10.0;
const TOKEN_REFILL_SECS: f64 = 2.0;
const HIGH_VOLUME_STOP: Duration = Duration::from_secs(30);
const QUIET_RESET: Duration = Duration::from_secs(2);
/// Upper bound on how long the worker waits for `TaskRegistry::spawn` to publish
/// its registry row before giving up. Registration normally lands microseconds
/// after `spawn` returns; a wait this long means the spawn future was dropped
/// (turn/session teardown), so the worker exits instead of busy-polling forever.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_EVENT_CHARS: usize = 500;
const MAX_PENDING_LINES: usize = 256;

struct WorkerCancel {
    handle: BackgroundTaskHandle,
    flush_handle: BackgroundTaskHandle,
    runtime: Arc<dyn RuntimeSpawner>,
}

struct BatchState {
    pending: Vec<String>,
    flush_scheduled: bool,
    tokens: f64,
    last_refill: Instant,
    last_batch: Option<Instant>,
    high_volume_since: Option<Instant>,
    suppressed: usize,
    stop_reason: Option<String>,
}

impl Default for BatchState {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            flush_scheduled: false,
            tokens: TOKEN_CAPACITY,
            last_refill: Instant::now(),
            last_batch: None,
            high_volume_since: None,
            suppressed: 0,
            stop_reason: None,
        }
    }
}

/// What one flush tick decided to do with the batched output — the PURE result
/// of the token-bucket / suppression / high-volume-stop logic (no I/O), so it is
/// unit-testable in isolation.
#[derive(Debug, PartialEq, Eq)]
enum FlushOutcome {
    /// Deliver this event to the model via `notify_monitor_event`.
    Deliver(String),
    /// Deliver this stop message to the model, THEN cancel the task (the oracle
    /// `cvo` delivers via `dY(...)` before `killTask()` → terminal status Killed).
    Stop(String),
    /// Rate-limited with nothing to report this tick.
    Nothing,
}

impl BatchState {
    /// Pure token-bucket + suppression decision for one flush tick (mirrors the
    /// oracle `cvo` onData body). Refills the bucket, drains `pending`, and
    /// returns what to deliver. Caller has already confirmed `pending` is
    /// non-empty and the task is not cancelled.
    fn flush_decision(&mut self, now: Instant) -> FlushOutcome {
        // Reset the high-volume WINDOW after a quiet gap (oracle `i=void 0`).
        // The suppressed COUNT is NOT cleared here — the oracle only zeroes `o`
        // when it reports it on the next delivered event, so a quiet gap must
        // not silently drop an unreported count.
        if self
            .last_batch
            .is_some_and(|last| now.duration_since(last) >= QUIET_RESET)
        {
            self.high_volume_since = None;
        }
        self.last_batch = Some(now);
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed / TOKEN_REFILL_SECS).min(TOKEN_CAPACITY);
        self.last_refill = now;
        let batch = self.pending.join("\n");
        self.pending.clear();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            if self.suppressed > 0 {
                // Oracle suppression notice, reported on the next delivered event.
                let suppressed = std::mem::take(&mut self.suppressed);
                FlushOutcome::Deliver(format!(
                    "{batch}\n[{suppressed} events suppressed — output rate too high. Consider using TaskStop to restart this monitor with a more selective filter.]"
                ))
            } else {
                FlushOutcome::Deliver(batch)
            }
        } else {
            self.suppressed = self.suppressed.saturating_add(1);
            let started = *self.high_volume_since.get_or_insert(now);
            let window = now.duration_since(started);
            if window >= HIGH_VOLUME_STOP {
                // Oracle stop message, DELIVERED before the kill. `${s}` is
                // `Math.round((now - windowStart) / 1000)`.
                let secs = window.as_secs_f64().round() as u64;
                let stop = format!(
                    "[Monitor stopped — too much output ({} events suppressed over {secs}s). Restart with a more selective source.]",
                    self.suppressed
                );
                self.stop_reason = Some(stop.clone());
                FlushOutcome::Stop(stop)
            } else {
                FlushOutcome::Nothing
            }
        }
    }
}

#[derive(Clone)]
struct MonitorStreamSink {
    task_id: String,
    output_file: PathBuf,
    output_manager: Arc<TaskOutputManager>,
    status_sink: Arc<dyn TaskStatusSink>,
    cancel: CancellationToken,
    batch: Arc<Mutex<BatchState>>,
    flush_notify: Arc<Notify>,
}

impl MonitorStreamSink {
    fn truncate_line(line: &str) -> String {
        let mut chars = line.chars();
        let prefix: String = chars.by_ref().take(MAX_EVENT_CHARS).collect();
        if chars.next().is_some() {
            format!("{prefix}…")
        } else {
            prefix
        }
    }

    async fn flush(&self) {
        let now = Instant::now();
        let outcome = {
            let mut state = self.batch.lock().await;
            state.flush_scheduled = false;
            if state.pending.is_empty() || self.cancel.is_cancelled() {
                state.pending.clear();
                return;
            }
            state.flush_decision(now)
        };
        match outcome {
            FlushOutcome::Deliver(event) => {
                self.status_sink
                    .notify_monitor_event(&self.task_id, &event)
                    .await;
            }
            // High-volume auto-stop: deliver the stop message to the model FIRST,
            // then cancel — the terminal status becomes Killed (epilogue).
            FlushOutcome::Stop(event) => {
                self.status_sink
                    .notify_monitor_event(&self.task_id, &event)
                    .await;
                self.cancel.cancel();
            }
            FlushOutcome::Nothing => {}
        }
    }

    async fn stop_reason(&self) -> Option<String> {
        self.batch.lock().await.stop_reason.clone()
    }
}

#[async_trait]
impl ProcessStreamSink for MonitorStreamSink {
    async fn stdout_line(&self, line: String) -> Result<(), ProcessError> {
        let mut spool = line.clone();
        spool.push('\n');
        self.output_manager
            .append(&self.output_file, &spool)
            .await
            .map_err(|e| ProcessError::Io(e.to_string()))?;
        let mut state = self.batch.lock().await;
        if state.pending.len() < MAX_PENDING_LINES {
            state.pending.push(MonitorStreamSink::truncate_line(&line));
        } else {
            state.suppressed = state.suppressed.saturating_add(1);
            state.high_volume_since.get_or_insert_with(Instant::now);
        }
        let notify = !state.flush_scheduled;
        state.flush_scheduled = true;
        drop(state);
        if notify {
            self.flush_notify.notify_one();
        }
        Ok(())
    }

    async fn stderr_chunk(&self, chunk: Vec<u8>) -> Result<(), ProcessError> {
        self.output_manager
            .append(&self.output_file, &String::from_utf8_lossy(&chunk))
            .await
            .map_err(|e| ProcessError::Io(e.to_string()))
    }
}
/// Real handler for the public Monitor tool.
pub struct MonitorHandler {
    process: Arc<dyn ProcessRunner>,
    sandbox: Arc<dyn Sandbox>,
    output_manager: Arc<TaskOutputManager>,
    status_sink: Arc<dyn TaskStatusSink>,
    workers: Arc<Mutex<HashMap<String, WorkerCancel>>>,
}

impl MonitorHandler {
    /// Construct a monitor handler.
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
            workers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Attach the registry-backed lifecycle sink.
    #[must_use]
    pub fn with_status_sink(mut self, sink: Arc<dyn TaskStatusSink>) -> Self {
        self.status_sink = sink;
        self
    }

    fn classify(result: &Result<platform_api::ProcessOutput, ProcessError>) -> TaskStatus {
        match result {
            Ok(output) if output.exit_code == 0 => TaskStatus::Completed,
            Ok(_) => TaskStatus::Failed,
            Err(ProcessError::Timeout) => TaskStatus::Killed,
            Err(_) => TaskStatus::Failed,
        }
    }

    /// Terminal status for a finished monitor worker. A cancellation — whether an
    /// explicit `kill()` or the high-volume auto-stop — goes through the oracle's
    /// `killTask()`, which sets the status to Killed (NOT Failed, even though the
    /// cancelled `run_streaming` resolves to an `Err`). Only a natural end is
    /// classified from the process result.
    fn terminal_status(
        cancelled: bool,
        result: &Result<platform_api::ProcessOutput, ProcessError>,
    ) -> TaskStatus {
        if cancelled {
            TaskStatus::Killed
        } else {
            Self::classify(result)
        }
    }
}

#[async_trait]
impl Task for MonitorHandler {
    fn name(&self) -> &str {
        HANDLER_NAME
    }

    fn task_type(&self) -> TaskType {
        TaskType::Monitor
    }

    async fn spawn(
        &self,
        input: TaskSpawnInput,
        ctx: TaskContext,
    ) -> Result<TaskHandle, TaskError> {
        let TaskSpawnInput::Monitor {
            command,
            timeout,
            cwd,
            tool_use_id: _,
            creator_teammate_name: _,
            creator_team_name: _,
            creator_agent_id: _,
        } = input
        else {
            return Err(TaskError::Internal(
                "monitor handler received a non-Monitor input".into(),
            ));
        };
        let task_id = crate::id::generate_task_id(TaskType::Monitor);
        let output_file = self
            .output_manager
            .allocate(&task_id)
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;
        let sandboxed = self.sandbox.bypass_with_audit(
            ProcessCommand {
                command: "bash".into(),
                args: vec!["-c".into(), command],
                cwd,
                env: HashMap::new(),
                timeout,
                stdin: None,
            },
            BYPASS_REASON,
        );
        let cancel = CancellationToken::new();
        let sink = Arc::new(MonitorStreamSink {
            task_id: task_id.clone(),
            output_file,
            output_manager: self.output_manager.clone(),
            status_sink: self.status_sink.clone(),
            cancel: cancel.clone(),
            batch: Arc::new(Mutex::new(BatchState::default())),
            flush_notify: Arc::new(Notify::new()),
        });
        // One batching worker per monitor. A per-batch RuntimeSpawner task would
        // retain a completed JoinHandle every 200ms for the lifetime of the
        // session; this single worker is cancelled on every exit path.
        let flush_sink = sink.clone();
        let flush_runtime = ctx.runtime.clone();
        let flush_cancel = CancellationToken::new();
        let flush_worker_cancel = flush_cancel.clone();
        let flush_handle = ctx
            .runtime
            .spawn(
                &format!("{HANDLER_NAME}:flush:{task_id}"),
                Box::pin(async move {
                    loop {
                        tokio::select! {
                            () = flush_worker_cancel.cancelled() => break,
                            () = flush_sink.flush_notify.notified() => {
                                flush_runtime.sleep(BATCH_WINDOW).await;
                                flush_sink.flush().await;
                            }
                        }
                    }
                }),
            )
            .await
            .map_err(|e| TaskError::Internal(e.to_string()))?;
        let process = self.process.clone();
        let status_sink = self.status_sink.clone();
        let workers = self.workers.clone();
        let worker_id = task_id.clone();
        let worker_sink = sink.clone();
        let worker_cancel = cancel.clone();
        let runtime = ctx.runtime.clone();
        let worker_runtime = ctx.runtime.clone();
        let worker_flush_handle = flush_handle.clone();
        let worker = Box::pin(async move {
            // `TaskRegistry::spawn` can only insert the handler-generated id
            // after this method returns. Wait for that publication so an
            // `echo ready` monitor cannot finish before its event/status has a
            // registry row to update — but BOUND the wait and honor cancellation
            // so a dropped `TaskRegistry::spawn` future (turn/session teardown)
            // cannot leave this detached worker busy-polling the registry every
            // 1ms forever.
            let registered = {
                let wait = async {
                    while !status_sink.is_registered(&worker_id).await {
                        runtime.sleep(Duration::from_millis(1)).await;
                    }
                };
                tokio::select! {
                    () = wait => true,
                    () = worker_cancel.cancelled() => false,
                    () = runtime.sleep(REGISTRATION_TIMEOUT) => false,
                }
            };
            if !registered {
                // Never published (cancelled or timed out). Tear down the paired
                // flush worker, drop our map entry, and exit. The monitored
                // command was never started, so nothing runs unwatched.
                flush_cancel.cancel();
                let _ = worker_runtime.cancel(&worker_flush_handle).await;
                workers.lock().await.remove(&worker_id);
                return;
            }
            status_sink
                .set_status(&worker_id, TaskStatus::Running)
                .await;
            let stream_sink: Arc<dyn ProcessStreamSink> = worker_sink.clone();
            let result = tokio::select! {
                result = process.run_streaming(&sandboxed, stream_sink) => result,
                () = worker_cancel.cancelled() => {
                    Err(ProcessError::Io(worker_sink.stop_reason().await.unwrap_or_else(|| "monitor cancelled".into())))
                }
            };
            worker_sink.flush().await;
            if let Ok(output) = &result {
                status_sink
                    .set_exit_code(&worker_id, output.exit_code)
                    .await;
            }
            let status = MonitorHandler::terminal_status(worker_cancel.is_cancelled(), &result);
            status_sink.set_status(&worker_id, status).await;
            flush_cancel.cancel();
            let _ = worker_runtime.cancel(&worker_flush_handle).await;
            workers.lock().await.remove(&worker_id);
        });
        let mut workers = self.workers.lock().await;
        let handle = match ctx
            .runtime
            .spawn(&format!("{HANDLER_NAME}:{task_id}"), worker)
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                let _ = ctx.runtime.cancel(&flush_handle).await;
                return Err(TaskError::Internal(error.to_string()));
            }
        };
        workers.insert(
            task_id.clone(),
            WorkerCancel {
                handle,
                flush_handle,
                runtime: ctx.runtime.clone(),
            },
        );
        drop(workers);

        let cleanup_cancel = cancel;
        let cleanup: Arc<dyn Fn() + Send + Sync> = Arc::new(move || cleanup_cancel.cancel());
        Ok(TaskHandle::new(task_id, Some(cleanup)))
    }

    async fn kill(&self, task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
        // Drop the map guard before awaiting cancellation. The main worker's
        // epilogue removes this same entry, so retaining the guard across
        // `RuntimeSpawner::cancel` can deadlock while cancel waits for the
        // worker future to finish.
        let worker = { self.workers.lock().await.remove(task_id) };
        if let Some(worker) = worker {
            let main_result = worker.runtime.cancel(&worker.handle).await;
            let flush_result = worker.runtime.cancel(&worker.flush_handle).await;
            if let Err(error) = main_result.or(flush_result) {
                return Err(TaskError::Io(error.to_string()));
            }
        }
        if !self.status_sink.is_terminal(task_id).await {
            self.status_sink
                .set_status(task_id, TaskStatus::Killed)
                .await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
    use platform_api::sandbox::{SandboxBackend, SandboxCapability, SandboxedTag};
    use platform_api::{
        ProcessCommand, ProcessHandle, ProcessOutput, SandboxError, SandboxPolicy, SandboxedCommand,
    };
    use std::collections::HashMap as StdHashMap;
    use std::sync::Mutex as StdMutex;
    use test_harness::mocks::MockRuntimeSpawner;
    use tokio::sync::Mutex as TokioMutex;

    // ── Pure token-bucket / suppression / high-volume-stop logic ────────────

    /// A `BatchState` at `now` with a given token count and suppressed count,
    /// `last_refill`/`last_batch` pinned to `now` (no incidental refill or quiet
    /// reset unless the test overrides them).
    fn state(now: Instant, tokens: f64, suppressed: usize) -> BatchState {
        let mut s = BatchState::default();
        s.tokens = tokens;
        s.last_refill = now;
        s.last_batch = Some(now);
        s.suppressed = suppressed;
        s
    }

    #[test]
    fn flush_decision_batches_pending_into_one_event() {
        let now = Instant::now();
        let mut s = state(now, 10.0, 0);
        s.pending = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(
            s.flush_decision(now),
            FlushOutcome::Deliver("a\nb\nc".to_string())
        );
        assert!(s.pending.is_empty(), "pending is drained");
    }

    #[test]
    fn flush_decision_reports_suppression_notice_byte_exact() {
        let now = Instant::now();
        let mut s = state(now, 5.0, 5);
        s.pending = vec!["out".into()];
        assert_eq!(
            s.flush_decision(now),
            FlushOutcome::Deliver(
                "out\n[5 events suppressed \u{2014} output rate too high. Consider using TaskStop to restart this monitor with a more selective filter.]"
                    .to_string()
            )
        );
        assert_eq!(s.suppressed, 0, "count is zeroed ONLY when reported");
    }

    #[test]
    fn flush_decision_rate_limited_suppresses_and_starts_window() {
        let now = Instant::now();
        let mut s = state(now, 0.0, 0);
        s.high_volume_since = None;
        s.pending = vec!["x".into()];
        assert_eq!(s.flush_decision(now), FlushOutcome::Nothing);
        assert_eq!(s.suppressed, 1);
        assert!(s.high_volume_since.is_some(), "high-volume window started");
    }

    #[test]
    fn flush_decision_high_volume_stop_delivers_byte_exact_message() {
        // `now` is 100s after `base`; the window opened at base+69s → 31s wide.
        let base = Instant::now();
        let now = base + Duration::from_secs(100);
        let mut s = state(now, 0.0, 42);
        s.high_volume_since = Some(base + Duration::from_secs(69));
        s.pending = vec!["x".into()];
        assert_eq!(
            s.flush_decision(now),
            FlushOutcome::Stop(
                "[Monitor stopped \u{2014} too much output (43 events suppressed over 31s). Restart with a more selective source.]"
                    .to_string()
            )
        );
        assert!(s.stop_reason.is_some());
    }

    #[test]
    fn flush_decision_quiet_gap_resets_window_but_keeps_the_unreported_count() {
        let base = Instant::now();
        let now = base + Duration::from_secs(100);
        let mut s = state(now, 0.0, 7);
        // Last batch was 3s ago (> QUIET_RESET = 2s); the window opened long ago.
        s.last_batch = Some(base + Duration::from_secs(97));
        s.high_volume_since = Some(base + Duration::from_secs(50));
        s.pending = vec!["x".into()];
        assert_eq!(s.flush_decision(now), FlushOutcome::Nothing);
        assert_eq!(
            s.suppressed, 8,
            "a quiet gap must NOT silently drop the unreported suppressed count"
        );
        assert_eq!(
            s.high_volume_since,
            Some(now),
            "the 30s window is reset by the quiet gap, then restarted at `now`"
        );
    }

    #[test]
    fn truncate_line_caps_at_max_event_chars() {
        let long = "x".repeat(MAX_EVENT_CHARS + 50);
        let t = MonitorStreamSink::truncate_line(&long);
        assert_eq!(t.chars().count(), MAX_EVENT_CHARS + 1);
        assert!(t.ends_with('\u{2026}'));
        assert_eq!(MonitorStreamSink::truncate_line("hello"), "hello");
    }

    #[test]
    fn terminal_status_treats_any_cancellation_as_killed() {
        // The high-volume auto-stop cancels with an Err result — it must be
        // Killed (NOT Failed, the pre-fix bug).
        assert_eq!(
            MonitorHandler::terminal_status(true, &Err(ProcessError::Io("stopped".into()))),
            TaskStatus::Killed
        );
        // A natural end is classified from the exit code.
        let ok0 = Ok(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        assert_eq!(
            MonitorHandler::terminal_status(false, &ok0),
            TaskStatus::Completed
        );
        let ok1 = Ok(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
            timed_out: false,
        });
        assert_eq!(
            MonitorHandler::terminal_status(false, &ok1),
            TaskStatus::Failed
        );
    }

    // ── Handler-level (spawn → run → terminal status) ───────────────────────

    struct MockRunner {
        output: ProcessOutput,
    }
    impl MockRunner {
        fn new(stdout: &str, exit_code: i32) -> Arc<Self> {
            Arc::new(Self {
                output: ProcessOutput {
                    stdout: stdout.to_string(),
                    stderr: String::new(),
                    exit_code,
                    timed_out: false,
                },
            })
        }
    }
    #[async_trait]
    impl ProcessRunner for MockRunner {
        async fn run(&self, _cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            Ok(self.output.clone())
        }
        async fn spawn_background(
            &self,
            _cmd: &SandboxedCommand,
        ) -> Result<ProcessHandle, ProcessError> {
            Err(ProcessError::Unsupported)
        }
        async fn kill(&self, _handle: &ProcessHandle) -> Result<(), ProcessError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    /// A `run()` that parks forever — stands in for a long-running monitored
    /// command until the future is cancelled.
    struct BlockingRunner;
    impl BlockingRunner {
        fn new() -> Arc<Self> {
            Arc::new(Self)
        }
    }
    #[async_trait]
    impl ProcessRunner for BlockingRunner {
        async fn run(&self, _cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            std::future::pending::<()>().await;
            unreachable!()
        }
        async fn spawn_background(
            &self,
            _cmd: &SandboxedCommand,
        ) -> Result<ProcessHandle, ProcessError> {
            Err(ProcessError::Unsupported)
        }
        async fn kill(&self, _handle: &ProcessHandle) -> Result<(), ProcessError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    struct StubSandbox;
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
                features: platform_api::SandboxFeatures::default(),
            }
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        statuses: StdMutex<Vec<TaskStatus>>,
        events: StdMutex<Vec<String>>,
    }
    #[async_trait]
    impl TaskStatusSink for RecordingSink {
        async fn set_status(&self, _task_id: &str, status: TaskStatus) {
            self.statuses.lock().unwrap().push(status);
        }
        async fn notify_monitor_event(&self, _task_id: &str, event: &str) {
            self.events.lock().unwrap().push(event.to_string());
        }
    }
    impl RecordingSink {
        fn last_status(&self) -> Option<TaskStatus> {
            self.statuses.lock().unwrap().last().copied()
        }
        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    // Minimal in-memory FileSystem (the spool target); output_manager only
    // exercises create/append here.
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
            _offset: Option<u64>,
            _limit: Option<u64>,
        ) -> Result<FileContent, FsError> {
            let map = self.files.lock().await;
            let content = map.get(path).cloned().unwrap_or_default();
            let total_lines = content.lines().count() as u64;
            Ok(FileContent {
                content,
                truncated: false,
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
            self.files
                .lock()
                .await
                .entry(path.to_string())
                .or_default()
                .push_str(body);
            Ok(())
        }
        async fn truncate(&self, _: &str, _: u64) -> Result<(), FsError> {
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

    /// A status sink whose task row is NEVER published — exercises the worker's
    /// bounded/cancellable registration wait.
    #[derive(Default)]
    struct NeverRegisteredSink;
    #[async_trait]
    impl TaskStatusSink for NeverRegisteredSink {
        async fn set_status(&self, _task_id: &str, _status: TaskStatus) {}
        async fn is_registered(&self, _task_id: &str) -> bool {
            false
        }
    }

    fn make_handler(
        process: Arc<dyn ProcessRunner>,
        sink: Arc<dyn TaskStatusSink>,
    ) -> (MonitorHandler, TaskContext) {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from("/spool"), fs.clone()));
        let handler =
            MonitorHandler::new(process, Arc::new(StubSandbox), mgr).with_status_sink(sink);
        let ctx = TaskContext {
            fs,
            runtime: Arc::new(MockRuntimeSpawner::default()),
        };
        (handler, ctx)
    }

    fn monitor_input() -> TaskSpawnInput {
        TaskSpawnInput::Monitor {
            command: "echo hi".into(),
            timeout: None,
            cwd: None,
            tool_use_id: None,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        }
    }

    async fn await_terminal(sink: &Arc<RecordingSink>) -> TaskStatus {
        for _ in 0..5000 {
            if let Some(s) = sink.last_status() {
                if s.is_terminal() {
                    return s;
                }
            }
            tokio::task::yield_now().await;
        }
        panic!("worker never reported a terminal status");
    }

    #[tokio::test]
    async fn natural_completion_reports_completed_and_delivers_events() {
        let sink = Arc::new(RecordingSink::default());
        let (handler, ctx) = make_handler(MockRunner::new("hello\nworld\n", 0), sink.clone());
        let _handle = handler.spawn(monitor_input(), ctx).await.unwrap();
        assert_eq!(await_terminal(&sink).await, TaskStatus::Completed);
        let events = sink.events();
        assert!(
            events
                .iter()
                .any(|e| e.contains("hello") && e.contains("world")),
            "batched output must reach the model; got {events:?}"
        );
    }

    #[tokio::test]
    async fn nonzero_exit_reports_failed() {
        let sink = Arc::new(RecordingSink::default());
        let (handler, ctx) = make_handler(MockRunner::new("", 1), sink.clone());
        let _handle = handler.spawn(monitor_input(), ctx).await.unwrap();
        assert_eq!(await_terminal(&sink).await, TaskStatus::Failed);
    }

    #[tokio::test]
    async fn cancellation_reports_killed_not_failed() {
        let sink = Arc::new(RecordingSink::default());
        let (handler, ctx) = make_handler(BlockingRunner::new(), sink.clone());
        let handle = handler.spawn(monitor_input(), ctx).await.unwrap();
        // Let the worker register, mark Running, and park in run_streaming.
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        // The cleanup closure cancels the token; the worker's cancelled arm runs
        // its epilogue → Killed (the pre-fix code reported Failed once a
        // stop_reason was set).
        (handle.cleanup.expect("cleanup closure"))();
        assert_eq!(await_terminal(&sink).await, TaskStatus::Killed);
    }

    #[tokio::test]
    async fn worker_exits_when_registration_never_occurs() {
        // The registry row is never published, so the worker's registration wait
        // would busy-poll forever (the #47 bug). With the bounded/cancellable
        // wait it must observe the cancellation and EXIT — proven by the worker
        // dropping its own entry from the handler's `workers` map.
        let (handler, ctx) = make_handler(BlockingRunner::new(), Arc::new(NeverRegisteredSink));
        let handle = handler.spawn(monitor_input(), ctx).await.unwrap();
        assert_eq!(
            handler.workers.lock().await.len(),
            1,
            "the worker is registered in the handler map while it waits"
        );
        // Let the worker reach its registration wait, then cancel it.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        (handle.cleanup.expect("cleanup closure"))();
        for _ in 0..5000 {
            if handler.workers.lock().await.is_empty() {
                return; // the worker exited the wait and tore itself down
            }
            tokio::task::yield_now().await;
        }
        panic!("worker never exited the registration wait after cancellation");
    }
}
