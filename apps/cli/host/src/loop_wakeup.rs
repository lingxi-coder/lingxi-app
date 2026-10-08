//! Dynamic /loop delivery for the local terminal host.
use harness_runtime::desktop::loop_tools;
use lingxi_core::host::OrchestratorHandle;
use loop_tools::WakeupScheduler;
use std::sync::{Arc, Weak};
use tokio_util::sync::CancellationToken;

pub(crate) struct CliLoopHost {
    session_id: Arc<std::sync::Mutex<Option<lingxi_core::types::SessionId>>>,
    pub state: Arc<loop_tools::LoopRuntime>,
    pub scheduler: Arc<dyn WakeupScheduler>,
    session_cron: Option<Arc<loop_tools::SessionCronScheduler>>,
    orch: Weak<orchestrator::ConversationOrchestrator>,
    cwd: Arc<tool_api::SessionCwd>,
    reason: orchestrator::prompt::mid_turn_input::CancelReasonFlag,
}
impl CliLoopHost {
    pub async fn bind(
        runtime: &crate::init::Runtime,
        queue: Arc<msgqueue::MessageQueueManager>,
        tx: tokio::sync::mpsc::UnboundedSender<tui::TurnEvent>,
        reason: orchestrator::prompt::mid_turn_input::CancelReasonFlag,
    ) -> Arc<Self> {
        let state = Arc::new(loop_tools::LoopRuntime::default());
        let session_id = Arc::new(std::sync::Mutex::new(Some(
            runtime.orchestrator.current_session_id().await,
        )));
        let delivery = Arc::new(CliDelivery {
            session_id: session_id.clone(),
            queue,
            orch: Arc::downgrade(&runtime.orchestrator),
            tx,
            state: state.clone(),
        });
        let scheduler: Arc<dyn WakeupScheduler> =
            Arc::new(loop_tools::RuntimeWakeupScheduler::new(
                runtime.runtime_spawner.clone(),
                state.clone(),
                delivery.clone(),
            ));
        if let Some(cron) = &runtime.session_lifecycle.cron_scheduler {
            cron.set_session_delivery(delivery).await;
        }
        let scheduler = match runtime.wakeup_scheduler_cell.set(scheduler.clone()) {
            Ok(()) => scheduler,
            Err(_) => runtime.wakeup_scheduler_cell.get().unwrap().clone(),
        };
        let state = scheduler.loop_runtime().unwrap_or(state);
        Arc::new(Self {
            session_id,
            state,
            scheduler,
            session_cron: runtime.session_lifecycle.cron_scheduler.clone(),
            orch: Arc::downgrade(&runtime.orchestrator),
            cwd: runtime.session_cwd.clone(),
            reason,
        })
    }
    pub async fn refresh_session(&self) {
        let Some(orch) = self.orch.upgrade() else {
            return;
        };
        let current = orch.current_session_id().await;
        let changed = self
            .session_id
            .lock()
            .unwrap()
            .is_some_and(|prior| prior != current);
        if changed {
            self.scheduler.cancel_pending().await;
            self.state.reset();
            *self.session_id.lock().unwrap() = Some(current);
            if let Some(cron) = &self.session_cron {
                if let Err(error) = cron.set_session_id(current.as_uuid().to_string()).await {
                    tracing::warn!(%error, "cannot retarget session cron");
                }
            }
        }
        *self.session_id.lock().unwrap() = Some(current);
    }
    pub fn begin(&self, raw_loop: Option<&str>, human: bool) -> std::io::Result<Option<String>> {
        if let Some(orch) = self.orch.upgrade() {
            orch.turn_span().reset();
        }
        self.reason.reset();
        if let Some(raw) = raw_loop {
            let resolved = self.state.try_resolve_loop_default_fire(
                raw,
                &self.cwd.project_root(),
                &self.cwd.cwd(),
            )?;
            self.state.begin_tick(raw.to_owned());
            Ok(Some(resolved))
        } else {
            if human {
                self.state.invalidate_noop_streak();
            }
            Ok(None)
        }
    }
    pub fn resolve_scheduled(&self, raw: &str) -> std::io::Result<String> {
        self.state.take_in_flight_prompt();
        self.state.invalidate_noop_streak();
        self.state
            .try_resolve_loop_default_fire(raw, &self.cwd.project_root(), &self.cwd.cwd())
    }
    pub async fn run_scheduled(
        &self,
        prompt: &str,
        command: &msgqueue::QueuedCommand,
        cancel: CancellationToken,
    ) -> Result<(), String> {
        let orch = self
            .orch
            .upgrade()
            .ok_or_else(|| "session closed before scheduled delivery".to_string())?;
        orch.run_queued_prompt_batch(
            vec![orchestrator::QueuedPromptInput {
                goal_retry_id: command
                    .uuid
                    .starts_with("goal-retry-")
                    .then(|| command.uuid.clone()),
                text: prompt.to_string(),
                is_meta: true,
                mod_origin: Some(if command.uuid.starts_with("goal-retry-") {
                    serde_json::json!({"kind":"auto-continuation"})
                } else {
                    serde_json::json!({"kind":"scheduled-trigger"})
                }),
                message_id: None,
                transcript_row_token: None,
                queue_priority: Some("later".into()),
                scheduled_task_id: command.scheduled_task_id.clone(),
                scheduled_fire_id: command.scheduled_fire_id.clone(),
            }],
            cancel,
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }
    pub async fn finish(&self, cancel: &CancellationToken) {
        let aborted = cancel.is_cancelled()
            && self.reason.get()
                == orchestrator::prompt::mid_turn_input::CancelReason::UserInterrupt;
        if let Some(orch) = self.orch.upgrade() {
            let span = orch.turn_span().snapshot();
            if span.compactions > 0 {
                self.state.reset_autonomous_loop_delivered();
                self.state
                    .veto_tick(loop_tools::LoopFoldVeto::BlockingSystemInSpan);
            }
            if aborted || span.aborts > 0 {
                self.state.veto_tick(loop_tools::LoopFoldVeto::ToolAbort);
            }
            if span.denials > 0 {
                self.state.veto_tick(loop_tools::LoopFoldVeto::ToolDenial);
            }
            if cancel.is_cancelled() && !aborted {
                self.state
                    .veto_tick(loop_tools::LoopFoldVeto::QueuedCommand);
            }
            loop_tools::settle_loop_tick(
                &self.state,
                loop_tools::LoopSpanCounts {
                    tool_uses: span.tool_uses,
                    span_len: span.messages,
                },
            );
        }
        if aborted {
            loop_tools::cancel_dynamic_loop_on_user_abort(&self.scheduler).await;
        } else {
            loop_tools::maybe_arm_keepalive_with_runtime(&self.scheduler, &self.state).await;
        }
    }
    pub async fn shutdown(&self) {
        self.scheduler.cancel_pending().await;
    }
}
struct CliDelivery {
    session_id: Arc<std::sync::Mutex<Option<lingxi_core::types::SessionId>>>,
    queue: Arc<msgqueue::MessageQueueManager>,
    orch: Weak<orchestrator::ConversationOrchestrator>,
    tx: tokio::sync::mpsc::UnboundedSender<tui::TurnEvent>,
    state: Arc<loop_tools::LoopRuntime>,
}
#[async_trait::async_trait]
impl loop_tools::WakeupDelivery for CliDelivery {
    async fn deliver(
        &self,
        command_id: &str,
        prompt: String,
        _reason: String,
        task: loop_tools::WakeupTask,
    ) {
        while self.queue.has_active_turn().await {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let Some(orch) = self.orch.upgrade() else {
            return;
        };
        let current = orch.current_session_id().await;
        if self.session_id.lock().unwrap().as_ref() != Some(&current) {
            return;
        }
        let now = std::time::SystemTime::now();
        let now_ms = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let streak = task
            .task_kind_loop
            .then(|| self.state.noop_streak())
            .flatten();
        let (message, companion) = task.lines(now_ms, streak);
        let since_ms = streak.map_or(0, |(_, since)| {
            since
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        });
        if let Err(error) = orch
            .append_scheduled_loop_wakeup(
                message.clone(),
                companion.clone(),
                streak.map_or(0, |(n, _)| n),
                since_ms,
                orchestrator::ScheduledLoopFire {
                    fire_id: task.fire_id,
                    task_id: task.task_id.clone(),
                    cron: task.cron.clone(),
                    prompt: task.display_prompt.clone(),
                    task_kind_loop: task.task_kind_loop,
                },
            )
            .await
        {
            tracing::warn!(%error, "cannot persist loop wakeup");
        }
        let _ = self.tx.send(tui::TurnEvent::SystemNotice {
            body: message,
            is_error: false,
        });
        if let Some(body) = companion {
            let _ = self.tx.send(tui::TurnEvent::SystemNotice {
                body,
                is_error: false,
            });
        }
        self.queue
            .enqueue(msgqueue::QueuedCommand {
                scheduled_task_id: Some(task.task_id.clone()),
                scheduled_fire_id: Some(task.fire_id.as_uuid().to_string()),
                uuid: command_id.to_owned(),
                content: msgqueue::QueuedCommandContent::UserInput { text: prompt },
                priority: msgqueue::QueuePriority::Later,
                queued_at: now,
                source: msgqueue::QueueSource::Cron,
                agent_id: None,
                skip_slash_commands: true,
                is_meta: true,
            })
            .await;
    }
    async fn cancel_queued(&self) -> Vec<String> {
        let queued = self
            .queue
            .get_by_max_priority(msgqueue::QueuePriority::Later, |c| {
                c.uuid.starts_with("loop-wakeup-") && c.source == msgqueue::QueueSource::Cron
            })
            .await;
        self.queue
            .remove(
                &queued.iter().map(|c| c.uuid.clone()).collect::<Vec<_>>(),
                "dynamic loop cancelled",
            )
            .await;
        queued
            .iter()
            .filter_map(|c| c.text().map(str::to_owned))
            .collect()
    }
}

#[async_trait::async_trait]
impl loop_tools::SessionCronDelivery for CliDelivery {
    async fn clear_queued(&self) {
        let ids: Vec<_> = self
            .queue
            .snapshot()
            .await
            .into_iter()
            .filter(|command| {
                command.source == msgqueue::QueueSource::Cron
                    && command.uuid.starts_with("cron-fire-")
            })
            .map(|command| command.uuid)
            .collect();
        self.queue.remove(&ids, "session changed").await;
    }
    async fn is_loading(&self) -> bool {
        self.queue.has_active_turn().await
    }
    async fn enqueue(&self, fire: loop_tools::SessionCronFire) -> Result<(), String> {
        if fire.cron.is_empty() {
            self.queue
                .enqueue(msgqueue::QueuedCommand {
                    scheduled_task_id: None,
                    scheduled_fire_id: None,
                    uuid: format!("cron-fire-{}", fire.id),
                    content: msgqueue::QueuedCommandContent::UserInput { text: fire.prompt },
                    priority: msgqueue::QueuePriority::Later,
                    queued_at: std::time::SystemTime::now(),
                    source: msgqueue::QueueSource::Cron,
                    agent_id: None,
                    skip_slash_commands: true,
                    is_meta: true,
                })
                .await;
            return Ok(());
        }
        let task = loop_tools::WakeupTask::scheduled(&fire);
        let id = task.command_id();
        loop_tools::WakeupDelivery::deliver(self, &id, fire.prompt, String::new(), task).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Recorder {
        state: Arc<loop_tools::LoopRuntime>,
        scheduled: std::sync::atomic::AtomicUsize,
        cancelled: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl WakeupScheduler for Recorder {
        async fn schedule(&self, _: std::time::Duration, _: String, _: String) {
            self.scheduled
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        async fn cancel_pending(&self) -> Vec<String> {
            self.cancelled
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            vec![]
        }
        fn loop_runtime(&self) -> Option<Arc<loop_tools::LoopRuntime>> {
            Some(self.state.clone())
        }
    }
    fn host(cwd: std::path::PathBuf) -> (CliLoopHost, Arc<Recorder>) {
        let state = Arc::new(loop_tools::LoopRuntime::default());
        let rec = Arc::new(Recorder {
            state: state.clone(),
            scheduled: 0.into(),
            cancelled: 0.into(),
        });
        (
            CliLoopHost {
                session_id: Arc::new(std::sync::Mutex::new(None)),
                state,
                scheduler: rec.clone(),
                session_cron: None,
                orch: Weak::new(),
                cwd: tool_api::SessionCwd::new(cwd, vec![]),
                reason: orchestrator::prompt::mid_turn_input::CancelReasonFlag::new(),
            },
            rec,
        )
    }
    #[tokio::test]
    async fn dynamic_tick_keeps_raw_identity_and_fallback_ends_after_one_retry() {
        let tmp = tempfile::tempdir().unwrap();
        let (host, rec) = host(tmp.path().into());
        assert_eq!(
            host.begin(Some("  /check  "), false).unwrap().as_deref(),
            Some("  /check  ")
        );
        assert_eq!(host.state.in_flight_prompt().as_deref(), Some("  /check  "));
        host.finish(&CancellationToken::new()).await;
        assert_eq!(rec.scheduled.load(std::sync::atomic::Ordering::SeqCst), 1);
        host.begin(Some("  /check  "), false).unwrap();
        host.finish(&CancellationToken::new()).await;
        assert_eq!(rec.scheduled.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(host.state.loop_ended());
    }
    #[tokio::test]
    async fn user_abort_cancels_while_queue_now_preserves_keepalive() {
        let tmp = tempfile::tempdir().unwrap();
        let (host, rec) = host(tmp.path().into());
        host.begin(Some("p"), false).unwrap();
        host.reason
            .set(orchestrator::prompt::mid_turn_input::CancelReason::QueueNowCommand);
        let cancel = CancellationToken::new();
        cancel.cancel();
        host.finish(&cancel).await;
        assert_eq!(rec.scheduled.load(std::sync::atomic::Ordering::SeqCst), 1);
        host.begin(Some("p"), false).unwrap();
        host.finish(&cancel).await;
        assert!(host.state.loop_ended());
        assert!(host.state.in_flight_prompt().is_none());
    }
}
