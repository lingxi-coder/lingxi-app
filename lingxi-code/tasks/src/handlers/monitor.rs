//! Shell stdout event monitor (`monitor_ws`).

use crate::handlers::{NoopStatusSink, TaskStatusSink};
use crate::id::TaskType;
use crate::output_manager::TaskOutputManager;
use crate::state::TaskStatus;
use crate::task_trait::{Task, TaskContext, TaskError, TaskHandle, TaskSpawnInput};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use traits::{
    BackgroundTaskHandle, ProcessCommand, ProcessError, ProcessRunner, ProcessStreamSink,
    RuntimeSpawner, Sandbox,
};

const HANDLER_NAME: &str = "monitor_ws";
const BYPASS_REASON: &str = "monitor_task";
const BATCH_WINDOW: Duration = Duration::from_millis(200);
const TOKEN_CAPACITY: f64 = 10.0;
const TOKEN_REFILL_SECS: f64 = 2.0;
const HIGH_VOLUME_STOP: Duration = Duration::from_secs(30);
const QUIET_RESET: Duration = Duration::from_secs(2);
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
        let event = {
            let mut state = self.batch.lock().await;
            state.flush_scheduled = false;
            if state.pending.is_empty() || self.cancel.is_cancelled() {
                state.pending.clear();
                return;
            }
            if state
                .last_batch
                .is_some_and(|last| now.duration_since(last) >= QUIET_RESET)
            {
                state.high_volume_since = None;
                state.suppressed = 0;
            }
            state.last_batch = Some(now);
            let elapsed = now.duration_since(state.last_refill).as_secs_f64();
            state.tokens = (state.tokens + elapsed / TOKEN_REFILL_SECS).min(TOKEN_CAPACITY);
            state.last_refill = now;
            let batch = state.pending.join("\n");
            state.pending.clear();
            if state.tokens >= 1.0 {
                state.tokens -= 1.0;
                if state.suppressed > 0 {
                    let suppressed = std::mem::take(&mut state.suppressed);
                    Some(format!("{batch}\n[{suppressed} monitor events suppressed]"))
                } else {
                    Some(batch)
                }
            } else {
                state.suppressed = state.suppressed.saturating_add(1);
                let started = *state.high_volume_since.get_or_insert(now);
                if now.duration_since(started) >= HIGH_VOLUME_STOP {
                    let reason = "Monitor stopped after 30s of excessive output; restart it with a more selective filter".to_string();
                    state.stop_reason = Some(reason);
                    self.cancel.cancel();
                }
                None
            }
        };
        if let Some(event) = event {
            self.status_sink
                .notify_monitor_event(&self.task_id, &event)
                .await;
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

    fn classify(result: &Result<traits::ProcessOutput, ProcessError>) -> TaskStatus {
        match result {
            Ok(output) if output.exit_code == 0 => TaskStatus::Completed,
            Ok(_) => TaskStatus::Failed,
            Err(ProcessError::Timeout) => TaskStatus::Killed,
            Err(_) => TaskStatus::Failed,
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
            // registry row to update.
            while !status_sink.is_registered(&worker_id).await {
                runtime.sleep(Duration::from_millis(1)).await;
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
            let status =
                if worker_cancel.is_cancelled() && worker_sink.stop_reason().await.is_none() {
                    TaskStatus::Killed
                } else {
                    MonitorHandler::classify(&result)
                };
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
        Ok(TaskHandle {
            task_id,
            cleanup: Some(cleanup),
        })
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
