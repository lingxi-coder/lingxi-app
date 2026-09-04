#![allow(clippy::unwrap_used)]

use super::*;
use platform_api::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
use platform_api::tool_invoker::{SubagentInvocationContext, ToolInvokerError};
use platform_api::{
    BudgetError, FusionDecision, FusionNeedsParentReason, FusionOrigin, FusionPreset,
    FusionRequest, FusionTiming, FusionUsage,
};
use serde_json::json;
use std::any::Any;
use std::collections::HashMap as StdHashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;
use tempfile::tempdir;
use test_harness::mocks::MockRuntimeSpawner;
use tokio::sync::{oneshot, Mutex as TokioMutex};

struct MockInvoker;

#[async_trait]
impl ToolInvoker for MockInvoker {
    async fn invoke(
        &self,
        _name: &str,
        _input: serde_json::Value,
        _ctx: SubagentInvocationContext,
    ) -> Result<serde_json::Value, ToolInvokerError> {
        Ok(json!(null))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct MockBudget;

#[async_trait]
impl BudgetEnforcerHandle for MockBudget {
    async fn check_and_charge(&self, _nano_usd: u64) -> Result<(), BudgetError> {
        Ok(())
    }

    async fn snapshot_total_nano_usd(&self) -> u64 {
        0
    }
}

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
        Ok(FileContent {
            total_lines: content.lines().count() as u64,
            content: trimmed,
            truncated,
        })
    }

    async fn write_file(&self, path: &str, body: &str) -> Result<(), FsError> {
        self.files
            .lock()
            .await
            .insert(path.to_string(), body.to_string());
        Ok(())
    }

    fn is_within_workspace(&self, _path: &str) -> bool {
        true
    }

    async fn watch(
        &self,
        _path: &str,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError> {
        Err(FsError::Io("not supported".into()))
    }

    async fn append_file(&self, path: &str, body: &str) -> Result<(), FsError> {
        let mut map = self.files.lock().await;
        map.entry(path.to_string()).or_default().push_str(body);
        Ok(())
    }

    async fn truncate(&self, path: &str, len: u64) -> Result<(), FsError> {
        let mut map = self.files.lock().await;
        if let Some(content) = map.get_mut(path) {
            content.truncate(usize::try_from(len).unwrap_or(usize::MAX));
        }
        Ok(())
    }

    async fn file_mtime(&self, _path: &str) -> Result<std::time::SystemTime, FsError> {
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

    async fn symlink(&self, _src: &str, _dst: &str) -> Result<(), FsError> {
        Ok(())
    }

    async fn flock_exclusive(&self, _path: &str) -> Result<Box<dyn FlockGuard>, FsError> {
        Err(FsError::Io("not supported".into()))
    }

    async fn fsync(&self, _path: &str) -> Result<(), FsError> {
        Ok(())
    }
}

/// An executor that blocks on the inherited cancel token instead of resolving
/// immediately — lets a test observe whether `kill` gives the run a real
/// chance to unwind cooperatively (F012) or hard-aborts it mid-flight.
struct CancelAwareExecutor {
    runs: AtomicUsize,
    observed_cancel: AtomicUsize,
}

impl CancelAwareExecutor {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            runs: AtomicUsize::new(0),
            observed_cancel: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl FusionExecutor for CancelAwareExecutor {
    async fn run(
        &self,
        _request: FusionRequest,
        inherit: FusionInheritance,
        _progress: Option<tokio::sync::mpsc::Sender<platform_api::FusionProgress>>,
    ) -> Result<FusionResult, FusionError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        inherit.cancel.cancelled().await;
        self.observed_cancel.fetch_add(1, Ordering::SeqCst);
        Err(FusionError::Cancelled)
    }
}

struct ImmediateExecutor {
    runs: AtomicUsize,
}

impl ImmediateExecutor {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            runs: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl FusionExecutor for ImmediateExecutor {
    async fn run(
        &self,
        _request: FusionRequest,
        _inherit: FusionInheritance,
        _progress: Option<tokio::sync::mpsc::Sender<platform_api::FusionProgress>>,
    ) -> Result<FusionResult, FusionError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(dummy_result())
    }
}

/// F005: sends two scripted `FusionProgress` events through whatever channel
/// `spawn` hands it before completing — the fixture under test is the
/// HANDLER's forwarding wire, not the orchestrator.
struct ProgressEmittingExecutor {
    runs: AtomicUsize,
}

impl ProgressEmittingExecutor {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            runs: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl FusionExecutor for ProgressEmittingExecutor {
    async fn run(
        &self,
        _request: FusionRequest,
        _inherit: FusionInheritance,
        progress: Option<tokio::sync::mpsc::Sender<platform_api::FusionProgress>>,
    ) -> Result<FusionResult, FusionError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        if let Some(tx) = progress {
            for stage in [
                platform_api::FusionStage::ResolvingModels,
                platform_api::FusionStage::RunningPanels {
                    completed: 2,
                    total: 3,
                },
            ] {
                let _ = tx
                    .send(platform_api::FusionProgress {
                        message: stage.label(),
                        stage,
                        panel_id: None,
                    realized_output_tokens: None,
                })
                    .await;
            }
        }
        Ok(dummy_result())
    }
}

/// Blocks in `run()` until the fusion inheritance's cancel token fires (the
/// same pattern `local_workflow_test::BlockingFusionExecutor` uses), then
/// returns `Err(Cancelled)`. Signals `started` once it has actually entered
/// `run()`, so a test can wait for the worker to be genuinely in-flight
/// before calling `kill`.
struct BlockingCancelAwareExecutor {
    started: StdMutex<Option<oneshot::Sender<()>>>,
    runs: AtomicUsize,
}

impl BlockingCancelAwareExecutor {
    fn new(started: oneshot::Sender<()>) -> Arc<Self> {
        Arc::new(Self {
            started: StdMutex::new(Some(started)),
            runs: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl FusionExecutor for BlockingCancelAwareExecutor {
    async fn run(
        &self,
        _request: FusionRequest,
        inherit: FusionInheritance,
        _progress: Option<tokio::sync::mpsc::Sender<platform_api::FusionProgress>>,
    ) -> Result<FusionResult, FusionError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        if let Some(tx) = self.started.lock().unwrap().take() {
            let _ = tx.send(());
        }
        inherit.cancel.cancelled().await;
        Err(FusionError::Cancelled)
    }
}

#[derive(Default)]
struct CountingCompletionSink(AtomicUsize);

#[async_trait]
impl FusionCompletionSink for CountingCompletionSink {
    async fn publish(&self, _conversation_id: &str, _result: &FusionResult) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct ActivationSink {
    statuses: StdMutex<Vec<(String, TaskStatus)>>,
    terminal: StdMutex<Vec<(String, String, String, TaskStatus)>>,
}

#[async_trait]
impl TaskStatusSink for ActivationSink {
    fn requires_explicit_activation(&self) -> bool {
        true
    }

    async fn set_status(&self, task_id: &str, status: TaskStatus) {
        self.statuses
            .lock()
            .unwrap()
            .push((task_id.to_string(), status));
    }

    async fn finish_fusion_terminal(
        &self,
        task_id: &str,
        run_id: String,
        final_text: String,
        status: TaskStatus,
    ) {
        self.terminal
            .lock()
            .unwrap()
            .push((task_id.to_string(), run_id, final_text, status));
        self.set_status(task_id, status).await;
    }

    async fn is_terminal(&self, task_id: &str) -> bool {
        self.statuses
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(id, _)| id == task_id)
            .is_some_and(|(_, status)| status.is_terminal())
    }
}

impl ActivationSink {
    fn last_status(&self) -> Option<TaskStatus> {
        self.statuses
            .lock()
            .unwrap()
            .last()
            .map(|(_, status)| *status)
    }

    fn statuses(&self) -> Vec<TaskStatus> {
        self.statuses
            .lock()
            .unwrap()
            .iter()
            .map(|(_, status)| *status)
            .collect()
    }
}

#[derive(Default)]
struct RecordingSink {
    statuses: StdMutex<Vec<(String, TaskStatus)>>,
    calls: StdMutex<Vec<&'static str>>,
    /// F005: every `set_fusion_stage` label, in call order.
    stages: StdMutex<Vec<String>>,
}

impl RecordingSink {
    fn last_status(&self) -> Option<TaskStatus> {
        self.statuses
            .lock()
            .unwrap()
            .last()
            .map(|(_, status)| *status)
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    fn count(&self, status: TaskStatus) -> usize {
        self.statuses
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, s)| *s == status)
            .count()
    }

    fn stages(&self) -> Vec<String> {
        self.stages.lock().unwrap().clone()
    }
}

#[async_trait]
impl TaskStatusSink for RecordingSink {
    async fn set_status(&self, task_id: &str, status: TaskStatus) {
        self.calls.lock().unwrap().push("status");
        self.statuses
            .lock()
            .unwrap()
            .push((task_id.to_string(), status));
    }

    async fn set_fusion_stage(&self, _task_id: &str, stage: String) {
        self.stages.lock().unwrap().push(stage);
    }

    async fn is_terminal(&self, task_id: &str) -> bool {
        self.statuses
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(id, _)| id == task_id)
            .is_some_and(|(_, status)| status.is_terminal())
    }
}

struct BlockingFusionTerminalSink {
    inner: RecordingSink,
    started_tx: StdMutex<Option<oneshot::Sender<()>>>,
    release_rx: TokioMutex<Option<oneshot::Receiver<()>>>,
}

impl BlockingFusionTerminalSink {
    fn new(started_tx: oneshot::Sender<()>, release_rx: oneshot::Receiver<()>) -> Self {
        Self {
            inner: RecordingSink::default(),
            started_tx: StdMutex::new(Some(started_tx)),
            release_rx: TokioMutex::new(Some(release_rx)),
        }
    }

    fn last_status(&self) -> Option<TaskStatus> {
        self.inner.last_status()
    }

    fn calls(&self) -> Vec<&'static str> {
        self.inner.calls()
    }
}

#[async_trait]
impl TaskStatusSink for BlockingFusionTerminalSink {
    async fn set_status(&self, task_id: &str, status: TaskStatus) {
        self.inner.calls.lock().unwrap().push("status");
        self.inner
            .statuses
            .lock()
            .unwrap()
            .push((task_id.to_string(), status));
    }

    async fn finish_fusion_terminal(
        &self,
        task_id: &str,
        _run_id: String,
        _final_text: String,
        status: TaskStatus,
    ) {
        self.inner.calls.lock().unwrap().push("outcome");
        if let Some(tx) = self.started_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
        if let Some(rx) = self.release_rx.lock().await.take() {
            let _ = rx.await;
        }
        self.set_status(task_id, status).await;
    }

    async fn is_terminal(&self, task_id: &str) -> bool {
        self.inner
            .statuses
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(id, _)| id == task_id)
            .is_some_and(|(_, status)| status.is_terminal())
    }
}

struct FailingExecutor {
    error: FusionError,
}

#[async_trait]
impl FusionExecutor for FailingExecutor {
    async fn run(
        &self,
        _request: FusionRequest,
        _inherit: FusionInheritance,
        _progress: Option<tokio::sync::mpsc::Sender<platform_api::FusionProgress>>,
    ) -> Result<FusionResult, FusionError> {
        Err(self.error.clone())
    }
}

/// Records every [`TaskStatusSink`] call in order (event tag first, so a
/// test can assert both the exact sequence and the payload of each call
/// rather than only that *some* status/outcome eventually landed).
#[derive(Default)]
struct RecordingStatusSink {
    events: Arc<StdMutex<Vec<String>>>,
    statuses: StdMutex<Vec<(String, TaskStatus)>>,
    errors: StdMutex<Vec<(String, String)>>,
    egress_and_usage: StdMutex<Vec<(String, Vec<String>, Option<platform_api::task_registry::AgentRunUsage>)>>,
}

impl RecordingStatusSink {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    /// Shares this sink's event log with a [`PublishOrderCompletionSink`] so
    /// a test can assert `publish` and `mark_fusion_result_published` land
    /// in the SAME ordered timeline as `finish_fusion_terminal` / the other
    /// status-sink calls, not just that both eventually happened.
    fn events_handle(&self) -> Arc<StdMutex<Vec<String>>> {
        self.events.clone()
    }

    fn last_status(&self) -> Option<TaskStatus> {
        self.statuses.lock().unwrap().last().map(|(_, s)| *s)
    }

    fn errors(&self) -> Vec<(String, String)> {
        self.errors.lock().unwrap().clone()
    }
}

#[async_trait]
impl TaskStatusSink for RecordingStatusSink {
    async fn set_status(&self, task_id: &str, status: TaskStatus) {
        self.events.lock().unwrap().push(format!("status:{status:?}"));
        self.statuses
            .lock()
            .unwrap()
            .push((task_id.to_string(), status));
    }

    async fn set_fusion_error(&self, task_id: &str, error: String) {
        self.events.lock().unwrap().push(format!("error:{error}"));
        self.errors
            .lock()
            .unwrap()
            .push((task_id.to_string(), error));
    }

    async fn set_fusion_egress_and_usage(
        &self,
        task_id: &str,
        egress_profiles: Vec<String>,
        usage: Option<platform_api::task_registry::AgentRunUsage>,
    ) {
        self.events.lock().unwrap().push(format!(
            "egress_and_usage:{}:{}",
            egress_profiles.len(),
            usage.is_some()
        ));
        self.egress_and_usage
            .lock()
            .unwrap()
            .push((task_id.to_string(), egress_profiles, usage));
    }

    async fn finish_fusion_terminal(
        &self,
        task_id: &str,
        _run_id: String,
        _final_text: String,
        status: TaskStatus,
    ) {
        self.events.lock().unwrap().push("outcome".to_string());
        self.set_status(task_id, status).await;
    }

    async fn mark_fusion_result_published(&self, _task_id: &str) {
        self.events.lock().unwrap().push("published".to_string());
    }

    async fn is_terminal(&self, task_id: &str) -> bool {
        self.statuses
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(id, _)| id == task_id)
            .is_some_and(|(_, status)| status.is_terminal())
    }
}

/// Pushes `"publish"` into a [`RecordingStatusSink`]'s shared event log
/// (via [`RecordingStatusSink::events_handle`]) so a test can assert the
/// exact interleaving of `FusionCompletionSink::publish` against
/// `TaskStatusSink` calls on the SAME timeline, not two separately-ordered
/// logs a test would have to correlate by hand.
struct PublishOrderCompletionSink(Arc<StdMutex<Vec<String>>>);

#[async_trait]
impl FusionCompletionSink for PublishOrderCompletionSink {
    async fn publish(&self, _conversation_id: &str, _result: &FusionResult) {
        self.0.lock().unwrap().push("publish".to_string());
    }
}

fn dummy_request() -> FusionRequest {
    FusionRequest {
        schema_version: 1,
        origin: FusionOrigin::Slash,
        prompt: "review this".into(),
        preset: FusionPreset::Quality,
        models: None,
        dimensions: vec!["coverage".into()],
        partial_ok: true,
        max_panel: None,
        cross_provider: true,
        parent_profile: "openai".into(),
        parent_model: "gpt-5.4".into(),
        conversation_id: Some("conv".into()),
        workflow_run_id: None,
    }
}

fn dummy_result() -> FusionResult {
    FusionResult {
        schema_version: 1,
        run_id: "fu_test".into(),
        status: FusionStatus::NeedsParent,
        decision: FusionDecision::NeedsParent {
            reason: FusionNeedsParentReason::LowConfidence,
        },
        final_text: "needs parent".into(),
        analysis: None,
        panels: vec![],
        usage: FusionUsage::default(),
        timing: FusionTiming::default(),
        egress_profiles: vec![],
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

fn make_handler(
    executor: Arc<dyn FusionExecutor>,
    output_manager: Arc<TaskOutputManager>,
    status_sink: Arc<dyn TaskStatusSink>,
    completion_sink: Arc<dyn FusionCompletionSink>,
) -> LocalFusionHandler {
    LocalFusionHandler::new(
        executor,
        completion_sink,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        output_manager,
    )
    .with_status_sink(status_sink)
}

#[tokio::test]
async fn handler_waits_for_activation_before_running_executor() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let (_dir, output_manager) = make_output_manager(fs.clone());
    let status_sink = Arc::new(ActivationSink::default());
    let completion_sink = Arc::new(CountingCompletionSink::default());
    let executor = ImmediateExecutor::new();
    let handler = make_handler(
        executor.clone(),
        output_manager,
        status_sink.clone(),
        completion_sink.clone(),
    );

    let mut handle = handler
        .spawn(
            TaskSpawnInput::LocalFusion {
                request: dummy_request(),
                conversation_id: "conv".into(),
            },
            make_ctx(fs),
        )
        .await
        .expect("spawn succeeds");

    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(status_sink.statuses().is_empty());
    assert_eq!(executor.runs.load(Ordering::SeqCst), 0);

    handle.activate();
    for _ in 0..200 {
        if status_sink
            .last_status()
            .is_some_and(TaskStatus::is_terminal)
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        status_sink.statuses(),
        vec![TaskStatus::Running, TaskStatus::Completed]
    );
    assert_eq!(executor.runs.load(Ordering::SeqCst), 1);
    assert_eq!(completion_sink.0.load(Ordering::SeqCst), 1);
}

/// F005: `LocalFusionHandler::spawn` must forward every `FusionProgress` the
/// executor emits into `TaskStatusSink::set_fusion_stage`, in order — before
/// this fix `spawn` always passed `None` for the progress channel, so a
/// `/fusion` task's stage never updated between `Running` and its terminal
/// status.
#[tokio::test]
async fn spawn_forwards_fusion_progress_into_set_fusion_stage() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let (_dir, output_manager) = make_output_manager(fs.clone());
    let status_sink = Arc::new(RecordingSink::default());
    let completion_sink = Arc::new(CountingCompletionSink::default());
    let executor = ProgressEmittingExecutor::new();
    let handler = make_handler(
        executor.clone(),
        output_manager,
        status_sink.clone(),
        completion_sink,
    );

    handler
        .spawn(
            TaskSpawnInput::LocalFusion {
                request: dummy_request(),
                conversation_id: "conv".into(),
            },
            make_ctx(fs),
        )
        .await
        .expect("spawn succeeds");

    for _ in 0..200 {
        if status_sink
            .last_status()
            .is_some_and(TaskStatus::is_terminal)
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(executor.runs.load(Ordering::SeqCst), 1);
    assert_eq!(
        status_sink.stages(),
        vec!["Resolving models".to_string(), "Running panels 2/3".to_string()],
        "expected the 2 scripted FusionProgress events forwarded in order"
    );
}

/// Review finding #17: a print-mode waiter that returns on terminal status
/// alone can race the still-in-flight `FusionCompletionSink::publish`
/// append and, on process exit, lose the durable `<fusion-result>` session
/// row. The fix is `TaskStatusSink::mark_fusion_result_published`, called
/// from the worker AFTER `publish` resolves — this pins that it fires, and
/// fires strictly AFTER `publish`, on the SAME shared timeline as the
/// terminal-status transition (never before it, since the notification
/// drain still needs `finish_fusion_terminal` first).
#[tokio::test]
async fn finalize_fusion_outcome_marks_result_published_after_the_completion_sink_publish() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let (_dir, output_manager) = make_output_manager(fs.clone());
    let status_sink = Arc::new(RecordingStatusSink::default());
    let completion_sink = Arc::new(PublishOrderCompletionSink(status_sink.events_handle()));
    let executor = ImmediateExecutor::new();
    let handler = make_handler(
        executor.clone(),
        output_manager,
        status_sink.clone(),
        completion_sink,
    );

    handler
        .spawn(
            TaskSpawnInput::LocalFusion {
                request: dummy_request(),
                conversation_id: "conv".into(),
            },
            make_ctx(fs),
        )
        .await
        .expect("spawn succeeds");

    for _ in 0..200 {
        if status_sink.events().contains(&"published".to_string()) {
            break;
        }
        tokio::task::yield_now().await;
    }

    assert_eq!(
        status_sink.events(),
        vec![
            "status:Running".to_string(),
            "egress_and_usage:0:true".to_string(),
            "outcome".to_string(),
            "status:Completed".to_string(),
            "publish".to_string(),
            "published".to_string(),
        ],
        "mark_fusion_result_published must fire, and strictly after both \
         finish_fusion_terminal (\"outcome\"/\"status:Completed\") and the \
         completion sink's own publish — never before either"
    );
}

#[tokio::test]
async fn kill_preserves_terminalizing_fusion_window() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let (_dir, output_manager) = make_output_manager(fs.clone());
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let status_sink = Arc::new(BlockingFusionTerminalSink::new(started_tx, release_rx));
    let completion_sink = Arc::new(CountingCompletionSink::default());
    let handler = make_handler(
        ImmediateExecutor::new(),
        output_manager,
        status_sink.clone(),
        completion_sink,
    );
    let ctx = make_ctx(fs);

    let handle = handler
        .spawn(
            TaskSpawnInput::LocalFusion {
                request: dummy_request(),
                conversation_id: "conv".into(),
            },
            ctx.clone(),
        )
        .await
        .expect("spawn succeeds");

    tokio::time::timeout(std::time::Duration::from_secs(2), started_rx)
        .await
        .expect("worker should enter terminal publish")
        .expect("terminal publish signal");

    handler
        .kill(&handle.task_id, ctx)
        .await
        .expect("kill should short-circuit during terminal publish");

    let _ = release_tx.send(());
    for _ in 0..200 {
        if status_sink
            .last_status()
            .is_some_and(TaskStatus::is_terminal)
        {
            break;
        }
        tokio::task::yield_now().await;
    }

    assert_eq!(status_sink.last_status(), Some(TaskStatus::Completed));
    assert_eq!(status_sink.calls(), vec!["status", "outcome", "status"]);
}

#[tokio::test]
async fn drain_pending_kills_preserves_terminalizing_fusion_window() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let (_dir, output_manager) = make_output_manager(fs.clone());
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let status_sink = Arc::new(BlockingFusionTerminalSink::new(started_tx, release_rx));
    let completion_sink = Arc::new(CountingCompletionSink::default());
    let handler = make_handler(
        ImmediateExecutor::new(),
        output_manager,
        status_sink.clone(),
        completion_sink,
    );

    let handle = handler
        .spawn(
            TaskSpawnInput::LocalFusion {
                request: dummy_request(),
                conversation_id: "conv".into(),
            },
            make_ctx(fs),
        )
        .await
        .expect("spawn succeeds");

    tokio::time::timeout(std::time::Duration::from_secs(2), started_rx)
        .await
        .expect("worker should enter terminal publish")
        .expect("terminal publish signal");

    (handle.cleanup.as_ref().expect("cleanup seam"))();
    handler.drain_pending_kills().await;

    let _ = release_tx.send(());
    for _ in 0..200 {
        if status_sink
            .last_status()
            .is_some_and(TaskStatus::is_terminal)
        {
            break;
        }
        tokio::task::yield_now().await;
    }

    assert_eq!(status_sink.last_status(), Some(TaskStatus::Completed));
    assert_eq!(status_sink.calls(), vec!["status", "outcome", "status"]);
}

/// F012: `kill` on a run that is genuinely still executing (not already in
/// the terminalizing window) must let the executor observe cooperative
/// cancellation through its own inherited token before falling back to a
/// hard `runtime.cancel` abort, and must reach `Killed` exactly once —
/// never once from the worker's own `Err(FusionError::Cancelled)` arm AND
/// again from `kill`'s fallback.
#[tokio::test]
async fn kill_while_running_lets_executor_observe_cancellation_and_status_killed_once() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let (_dir, output_manager) = make_output_manager(fs.clone());
    let status_sink = Arc::new(RecordingSink::default());
    let completion_sink = Arc::new(CountingCompletionSink::default());
    let executor = CancelAwareExecutor::new();
    let handler = make_handler(
        executor.clone(),
        output_manager,
        status_sink.clone(),
        completion_sink,
    );
    let ctx = make_ctx(fs);

    let handle = handler
        .spawn(
            TaskSpawnInput::LocalFusion {
                request: dummy_request(),
                conversation_id: "conv".into(),
            },
            ctx.clone(),
        )
        .await
        .expect("spawn succeeds");

    // Wait until the worker has genuinely entered the executor and is
    // blocked on the inherited cancel token — not yet cancelled.
    for _ in 0..200 {
        if executor.runs.load(Ordering::SeqCst) >= 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        executor.runs.load(Ordering::SeqCst),
        1,
        "executor must have started"
    );
    assert_eq!(
        executor.observed_cancel.load(Ordering::SeqCst),
        0,
        "executor must still be genuinely blocked before kill"
    );

    handler
        .kill(&handle.task_id, ctx)
        .await
        .expect("kill succeeds");

    assert_eq!(
        executor.observed_cancel.load(Ordering::SeqCst),
        1,
        "kill must let the executor observe cooperative cancellation through its own \
         inherited token, not get hard-aborted mid-await"
    );
    assert_eq!(
        status_sink.count(TaskStatus::Killed),
        1,
        "status must reach Killed exactly once; got: {:?}",
        status_sink.calls()
    );
}

// ---- WP6 item 7: Err -> Failed with a recorded error, spool content,
// egress/usage recorded before the terminal transition ----------------------

#[tokio::test]
async fn failing_executor_records_error_before_failed_status_and_spools_it() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let (_dir, output_manager) = make_output_manager(fs.clone());
    let status_sink = Arc::new(RecordingStatusSink::default());
    let completion_sink = Arc::new(CountingCompletionSink::default());
    let error = FusionError::TooFewModels {
        eligible: 1,
        required: 2,
        same_provider_only: false,
        parent_profile: "anthropic".into(),
    };
    let executor = Arc::new(FailingExecutor {
        error: error.clone(),
    });
    let handler = make_handler(
        executor,
        output_manager.clone(),
        status_sink.clone(),
        completion_sink.clone(),
    );

    let handle = handler
        .spawn(
            TaskSpawnInput::LocalFusion {
                request: dummy_request(),
                conversation_id: "conv".into(),
            },
            make_ctx(fs.clone()),
        )
        .await
        .expect("spawn succeeds");

    for _ in 0..200 {
        if status_sink
            .last_status()
            .is_some_and(TaskStatus::is_terminal)
        {
            break;
        }
        tokio::task::yield_now().await;
    }

    // Status is Failed, exactly one error was recorded, and it carries the
    // real error text (not folded away into a generic "failed").
    assert_eq!(status_sink.last_status(), Some(TaskStatus::Failed));
    let errors = status_sink.errors();
    assert_eq!(errors.len(), 1, "exactly one error recorded: {errors:?}");
    assert_eq!(errors[0].1, error.to_string());
    // Ordering: the error must land BEFORE the terminal status flip so a
    // concurrent notification drain can never observe `Failed` with no error.
    assert_eq!(
        status_sink.events(),
        vec![
            "status:Running".to_string(),
            format!("error:{error}"),
            "status:Failed".to_string(),
        ]
    );
    // The Fusion run never published a completion (it failed).
    assert_eq!(completion_sink.0.load(Ordering::SeqCst), 0);

    // Spool contains the raw error text.
    let path = output_manager.path_for(&handle.task_id).unwrap();
    let spooled = fs
        .read_file(&path.to_string_lossy(), None, None)
        .await
        .expect("spool readable");
    assert_eq!(spooled.content, error.to_string());
}

#[tokio::test]
async fn ok_executor_records_egress_and_usage_before_completed_status() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let (_dir, output_manager) = make_output_manager(fs.clone());
    let status_sink = Arc::new(RecordingStatusSink::default());
    let completion_sink = Arc::new(CountingCompletionSink::default());
    let executor = ImmediateExecutor::new();
    let handler = make_handler(
        executor.clone(),
        output_manager,
        status_sink.clone(),
        completion_sink.clone(),
    );

    handler
        .spawn(
            TaskSpawnInput::LocalFusion {
                request: dummy_request(),
                conversation_id: "conv".into(),
            },
            make_ctx(fs),
        )
        .await
        .expect("spawn succeeds");

    for _ in 0..200 {
        if status_sink
            .last_status()
            .is_some_and(TaskStatus::is_terminal)
        {
            break;
        }
        tokio::task::yield_now().await;
    }

    assert_eq!(status_sink.last_status(), Some(TaskStatus::Completed));
    let egress_and_usage = status_sink.egress_and_usage.lock().unwrap().clone();
    assert_eq!(
        egress_and_usage.len(),
        1,
        "egress/usage recorded exactly once: {egress_and_usage:?}"
    );
    // `dummy_result()` carries no egress profiles and zeroed usage — assert
    // the concrete content the handler forwarded, not just "some value".
    assert_eq!(egress_and_usage[0].1, Vec::<String>::new());
    let usage = egress_and_usage[0].2.as_ref().expect("usage summary set");
    assert_eq!(usage.subagent_tokens, 0);
    assert_eq!(usage.tool_uses, 0);
    assert_eq!(usage.duration_ms, 0);
    // Ordering: egress/usage lands before the terminal `outcome` publish,
    // and `mark_fusion_result_published` (review finding #17) fires last —
    // after the completion sink's own publish, which for `CountingCompletionSink`
    // has already run by the time we observe a terminal status (there is no
    // real await point between them here), so it is on this same log too.
    assert_eq!(
        status_sink.events(),
        vec![
            "status:Running".to_string(),
            "egress_and_usage:0:true".to_string(),
            "outcome".to_string(),
            "status:Completed".to_string(),
            "published".to_string(),
        ]
    );
    assert_eq!(completion_sink.0.load(Ordering::SeqCst), 1);
}

// ---- WP6 item 7: kill-while-running and a sink that never gets to publish
// must not corrupt the terminal status ---------------------------------

#[tokio::test]
async fn kill_while_running_ends_status_killed_exactly_once() {
    // Full cooperative-cancellation OBSERVABILITY by the executor (proving
    // `inherit.cancel.cancelled()` resolves before the worker task is
    // aborted) is F012/WP8's kill-path fix, landing on this same file from a
    // different work package/lane — not re-verified here to avoid
    // conflicting with that in-flight change. This test covers what WP6 owns:
    // killing a task whose executor is genuinely still in-flight must still
    // converge on exactly one `Killed` transition, not hang and not leave
    // `workers` holding a stale entry a second `kill` could double-fire on.
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let (_dir, output_manager) = make_output_manager(fs.clone());
    let (started_tx, started_rx) = oneshot::channel();
    let status_sink = Arc::new(RecordingStatusSink::default());
    let completion_sink = Arc::new(CountingCompletionSink::default());
    let executor = BlockingCancelAwareExecutor::new(started_tx);
    let handler = make_handler(
        executor.clone(),
        output_manager,
        status_sink.clone(),
        completion_sink.clone(),
    );
    let ctx = make_ctx(fs);

    let handle = handler
        .spawn(
            TaskSpawnInput::LocalFusion {
                request: dummy_request(),
                conversation_id: "conv".into(),
            },
            ctx.clone(),
        )
        .await
        .expect("spawn succeeds");

    tokio::time::timeout(std::time::Duration::from_secs(2), started_rx)
        .await
        .expect("worker should enter the blocking executor")
        .expect("started signal");
    assert_eq!(
        executor.runs.load(Ordering::SeqCst),
        1,
        "the executor was genuinely invoked before kill"
    );

    handler
        .kill(&handle.task_id, ctx.clone())
        .await
        .expect("kill succeeds while the executor is in-flight");

    assert_eq!(status_sink.last_status(), Some(TaskStatus::Killed));
    let terminal_transitions = status_sink
        .statuses
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, s)| s.is_terminal())
        .count();
    assert_eq!(
        terminal_transitions, 1,
        "exactly one terminal transition: {:?}",
        status_sink.statuses.lock().unwrap()
    );
    assert_eq!(
        completion_sink.0.load(Ordering::SeqCst),
        0,
        "a killed run must never publish a completion"
    );

    // A second kill on the now-terminal task is a harmless no-op, not a
    // double transition.
    handler
        .kill(&handle.task_id, ctx)
        .await
        .expect("kill is idempotent once terminal");
    assert_eq!(
        status_sink
            .statuses
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, s)| s.is_terminal())
            .count(),
        1,
        "kill after terminal must not add a second transition"
    );
}

#[tokio::test]
async fn a_sink_that_never_publishes_does_not_revert_a_completed_run_to_failed() {
    // `LocalFusionHandler`'s own doc comment (top of this file) states the
    // invariant: "Sink failures must not rewrite the task's terminal
    // status." This pins it with a sink that captures the status AT THE
    // MOMENT it is invoked (proving the terminal transition already
    // happened) and then does nothing further — the run must still read
    // back as Completed.
    struct FailingCompletionSink {
        calls: AtomicUsize,
        status_at_publish: StdMutex<Option<TaskStatus>>,
        status_sink: Arc<RecordingStatusSink>,
    }

    #[async_trait]
    impl FusionCompletionSink for FailingCompletionSink {
        async fn publish(&self, _conversation_id: &str, _result: &FusionResult) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.status_at_publish.lock().unwrap() = self.status_sink.last_status();
            // Simulate a sink that fails to reach the client (e.g.
            // `DesktopFusionCompletionSink::publish`'s real `append_meta_..`
            // erroring) — it logs and returns, touching nothing else.
        }
    }

    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let (_dir, output_manager) = make_output_manager(fs.clone());
    let status_sink = Arc::new(RecordingStatusSink::default());
    let completion_sink = Arc::new(FailingCompletionSink {
        calls: AtomicUsize::new(0),
        status_at_publish: StdMutex::new(None),
        status_sink: status_sink.clone(),
    });
    let executor = ImmediateExecutor::new();
    let handler = make_handler(
        executor,
        output_manager,
        status_sink.clone(),
        completion_sink.clone(),
    );

    handler
        .spawn(
            TaskSpawnInput::LocalFusion {
                request: dummy_request(),
                conversation_id: "conv".into(),
            },
            make_ctx(fs),
        )
        .await
        .expect("spawn succeeds");

    for _ in 0..200 {
        if status_sink
            .last_status()
            .is_some_and(TaskStatus::is_terminal)
        {
            break;
        }
        tokio::task::yield_now().await;
    }

    assert_eq!(completion_sink.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *completion_sink.status_at_publish.lock().unwrap(),
        Some(TaskStatus::Completed),
        "the terminal status must already be Completed by the time publish runs"
    );
    // The "failing" sink changed nothing: status stays Completed, not Failed.
    assert_eq!(status_sink.last_status(), Some(TaskStatus::Completed));
}
