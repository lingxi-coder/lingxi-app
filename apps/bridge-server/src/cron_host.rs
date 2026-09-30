//! Authenticated host execution of scheduler-owned occurrences.
use async_trait::async_trait;
use client::adapter::ClientEventSink;
use client::protocol::events::ClientEvent;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{oneshot, Mutex};

pub(crate) const HOST_CANCEL_REQUESTED: &str = "lingxi-host-cancel-requested-v1";

/// Execution requests are retained through binding until host completion.
/// The connection holds its outbound lease while calling these methods, so a
/// close/reconnect cannot acknowledge delivery to the wrong socket.
#[derive(Default)]
pub(crate) struct HostCronRequests {
    pending: Mutex<HashMap<String, PendingHostRequest>>,
}

struct PendingHostRequest {
    event: ClientEvent,
    delivered: bool,
    handed_off: bool,
}

impl HostCronRequests {
    pub(crate) async fn enqueue(
        &self,
        event: ClientEvent,
        ready: bool,
        mut send: impl FnMut(ClientEvent) -> bool,
    ) {
        let ClientEvent::CronRunRequested { run_id, .. } = &event else {
            return;
        };
        let mut pending = self.pending.lock().await;
        let request = pending.entry(run_id.clone()).or_insert(PendingHostRequest {
            event,
            delivered: false,
            handed_off: false,
        });
        if ready && !request.delivered {
            request.delivered = send(request.event.clone());
            request.handed_off |= request.delivered;
        }
    }

    pub(crate) async fn replay(&self, mut send: impl FnMut(ClientEvent) -> bool) {
        let mut pending = self.pending.lock().await;
        for request in pending.values_mut().filter(|request| !request.delivered) {
            request.delivered = send(request.event.clone());
            request.handed_off |= request.delivered;
        }
    }

    async fn mark_started(&self, run_id: &str, session_id: &str) {
        let mut pending = self.pending.lock().await;
        if let Some(request) = pending.get_mut(run_id) {
            request.handed_off = true;
            if let ClientEvent::CronRunRequested { task, .. } = &mut request.event {
                if let Some(run) = task
                    .automation
                    .as_mut()
                    .and_then(|config| config.runs.first_mut())
                {
                    run.started_at = Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    );
                    run.session_id = Some(session_id.into());
                }
            }
        }
    }

    pub(crate) async fn disconnected(&self) {
        for request in self.pending.lock().await.values_mut() {
            request.delivered = false;
        }
    }

    pub(crate) async fn remove(&self, run_id: &str) {
        self.pending.lock().await.remove(run_id);
    }

    /// Cancel an untransmitted request outright. A possibly started request
    /// remains replayable only to recover its existing result: the host must
    /// use its claim cache, and refuse new execution when that cache is absent.
    async fn cancel(&self, run_id: &str) -> bool {
        let mut pending = self.pending.lock().await;
        let Some(request) = pending.get_mut(run_id) else {
            return false;
        };
        if !request.handed_off {
            pending.remove(run_id);
            return false;
        }
        if let ClientEvent::CronRunRequested { task, .. } = &mut request.event {
            if let Some(run) = task
                .automation
                .as_mut()
                .and_then(|config| config.runs.first_mut())
            {
                run.error = Some(HOST_CANCEL_REQUESTED.into());
            }
        }
        request.delivered = false;
        true
    }
}

pub(crate) struct PendingRun {
    request: cron::automation::AutomationRunRequest,
    sender: oneshot::Sender<RunReply>,
    host_may_be_running: bool,
    cancel_requested: bool,
}

/// The protocol treats run_id as opaque. Keep the durable occurrence identity
/// in the task metadata, and correlate acknowledgements to this exact claim.
/// Desktop decodes this prefix only when the tuple matches that metadata.
pub(crate) fn transport_run_id(request: &cron::automation::AutomationRunRequest) -> String {
    format!(
        "lingxi-cron-claim-v1:{}",
        serde_json::to_string(&(request.run_id.as_str(), request.claim_generation)).unwrap()
    )
}

pub(crate) fn requested_task(
    request: &cron::automation::AutomationRunRequest,
) -> client::protocol::events::CronJobDto {
    let mut task = request.task.clone();
    if let Some(automation) = task.automation.as_mut() {
        // The SDK intentionally clears historical runs in the claimed input.
        // Restore only this claim's identity for host dedupe/history lookup.
        automation.runs = vec![cron::automation::AutomationRun {
            id: request.run_id.clone(),
            task_id: task.id.clone(),
            owner_pid: Some(std::process::id()),
            claim_generation: Some(request.claim_generation),
            manual_occurrence_at: None,
            scheduled_at: 0,
            started_at: None,
            finished_at: None,
            status: cron::automation::AutomationRunStatus::Running,
            model: automation.model.clone(),
            reasoning: automation.reasoning.clone(),
            session_id: None,
            summary: None,
            error: None,
        }];
    }
    harness_runtime::desktop::cron_management::task_dto(task)
}

type RunReply = Result<cron::automation::AutomationRunResult, String>;
pub struct HostCronFirer {
    sink: Arc<dyn ClientEventSink>,
    cwd: std::path::PathBuf,
    pub(crate) pending: Mutex<HashMap<String, PendingRun>>,
    requests: Arc<HostCronRequests>,
}
impl HostCronFirer {
    pub(crate) fn new(
        sink: Arc<dyn ClientEventSink>,
        cwd: std::path::PathBuf,
        requests: Arc<HostCronRequests>,
    ) -> Self {
        Self {
            sink,
            cwd,
            pending: Mutex::new(HashMap::new()),
            requests,
        }
    }
    pub async fn started(&self, id: &str, session_id: &str) {
        // Bind and publish its acknowledgement under the cancellation lock.
        // A cancellation that wins first must reject a later bind; one that
        // follows an accepted bind must retain ownership until host completion.
        let mut pending = self.pending.lock().await;
        let result = match pending.get_mut(id) {
            Some(run) if run.cancel_requested => {
                Err("cancelled: Scheduled execution was revoked before binding".into())
            }
            Some(run) => {
                run.host_may_be_running = true;
                // The same host can recover the original promise/result after
                // reconnect. A host without that claim cache sees the bound
                // marker and must refuse to execute this attempt a second time.
                self.requests.mark_started(id, session_id).await;
                let fs = crate::HostFileSystem::new(self.cwd.clone());
                cron::automation::bind_automation_run_session(
                    &fs,
                    &self.cwd,
                    &run.request,
                    session_id,
                )
                .await
            }
            None => Err("Scheduled run is no longer awaiting execution".into()),
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
        let mut pending = self.pending.lock().await;
        self.requests.remove(id).await;
        if let Some(run) = pending.remove(id) {
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
            let _ = run.sender.send(result);
        }
    }
}
#[async_trait]
impl cron::CronJobFirer for HostCronFirer {
    async fn fire(&self, _: &str, _: &str) -> Result<String, String> {
        Err("Host execution requires versioned automation settings".into())
    }
    async fn fire_automation(&self, request: &cron::automation::AutomationRunRequest) -> RunReply {
        let id = transport_run_id(request);
        let (sender, receiver) = oneshot::channel();
        {
            // Publication and cancellation share this lock. A cancelled entry
            // cannot be republished by a firing future paused before emit.
            let mut pending = self.pending.lock().await;
            pending.insert(
                id.clone(),
                PendingRun {
                    request: request.clone(),
                    sender,
                    host_may_be_running: false,
                    cancel_requested: false,
                },
            );
            self.sink
                .emit(ClientEvent::CronRunRequested {
                    run_id: id.clone(),
                    task: requested_task(request),
                })
                .await;
        }
        let result = tokio::time::timeout(Duration::from_secs(24 * 60 * 60), receiver).await;
        self.pending.lock().await.remove(&id);
        self.requests.remove(&id).await;
        match result {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("Scheduled execution host disconnected".into()),
            Err(_) => Err("Scheduled execution timed out".into()),
        }
    }
    async fn cancel_run(&self, run_id: &str) -> Result<(), String> {
        let mut pending = self.pending.lock().await;
        let attempts = pending
            .iter()
            .filter(|(_, run)| run.request.run_id == run_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut unconfirmed = false;
        for id in attempts {
            // A successful send may have started execution even if its started
            // acknowledgement was lost. Limit replay to result recovery, but
            // never falsely claim that a remote writer has been joined.
            let handed_off = self.requests.cancel(&id).await;
            let Some(run) = pending.get_mut(&id) else {
                continue;
            };
            run.host_may_be_running |= handed_off;
            run.cancel_requested = true;
            if run.host_may_be_running {
                unconfirmed = true;
            } else {
                pending.remove(&id);
            }
        }
        if unconfirmed {
            Err("interrupted: Host execution has not confirmed termination".into())
        } else {
            Ok(())
        }
    }
}
