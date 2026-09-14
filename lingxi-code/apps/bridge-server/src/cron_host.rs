//! Authenticated host execution of scheduler-owned occurrences.
use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::events::ClientEvent;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{oneshot, Mutex};

type PendingRun = (
    cron::automation::AutomationRunRequest,
    oneshot::Sender<RunReply>,
);
type RunReply = Result<cron::automation::AutomationRunResult, String>;
pub struct HostCronFirer {
    sink: Arc<dyn ClientEventSink>,
    cwd: std::path::PathBuf,
    pending: Mutex<HashMap<String, PendingRun>>,
}
impl HostCronFirer {
    pub fn new(sink: Arc<dyn ClientEventSink>, cwd: std::path::PathBuf) -> Self {
        Self {
            sink,
            cwd,
            pending: Mutex::new(HashMap::new()),
        }
    }
    pub async fn started(&self, id: &str, session_id: &str) {
        let request = self
            .pending
            .lock()
            .await
            .get(id)
            .map(|(request, _)| request.clone());
        let result = if let Some(request) = request {
            let fs = platform_posix::PosixFileSystem::new(self.cwd.clone());
            cron::automation::bind_automation_run_session(&fs, &self.cwd, &request, session_id)
                .await
        } else {
            Err("Scheduled run is no longer awaiting execution".into())
        };
        self.sink
            .emit(ClientEvent::CronRunBound {
                run_id: id.into(),
                error: result.err(),
            })
            .await;
    }
    pub async fn complete(
        &self,
        id: &str,
        session: Option<String>,
        summary: Option<String>,
        error: Option<String>,
    ) {
        if let Some((_, sender)) = self.pending.lock().await.remove(id) {
            let result = match error {
                Some(error) => Err(error),
                None => session
                    .filter(|s| !s.is_empty())
                    .map(|session_id| cron::automation::AutomationRunResult {
                        session_id,
                        summary: summary.unwrap_or_default(),
                    })
                    .ok_or_else(|| "Host returned no execution session".into()),
            };
            let _ = sender.send(result);
        }
    }
}
#[async_trait]
impl cron::CronJobFirer for HostCronFirer {
    async fn fire(&self, _: &str, _: &str) -> Result<String, String> {
        Err("Host execution requires versioned automation settings".into())
    }
    async fn fire_automation(&self, request: &cron::automation::AutomationRunRequest) -> RunReply {
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .await
            .insert(request.run_id.clone(), (request.clone(), sender));
        self.sink
            .emit(ClientEvent::CronRunRequested {
                run_id: request.run_id.clone(),
                task: engine_desktop::cron_management::task_dto(request.task.clone()),
            })
            .await;
        let result = tokio::time::timeout(Duration::from_secs(24 * 60 * 60), receiver).await;
        self.pending.lock().await.remove(&request.run_id);
        match result {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("Scheduled execution host disconnected".into()),
            Err(_) => Err("Scheduled execution timed out".into()),
        }
    }
    /// The trait default answers `Ok(())` without releasing anything. The only
    /// cleanup this firer owns is the pending entry, and the line that removes
    /// it lives in the `fire_automation` future — which is exactly the future
    /// that was dropped when the scheduler decided to cancel. Drop it here so a
    /// later `cron_run_completed` cannot resolve into a dead receiver and the
    /// entry cannot outlive the run.
    async fn cancel_run(&self, run_id: &str) -> Result<(), String> {
        self.pending.lock().await.remove(run_id);
        Ok(())
    }
}
