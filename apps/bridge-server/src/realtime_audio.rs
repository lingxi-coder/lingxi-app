//! Connection-owned native audio. PCM and playback controls never take the chat turn lock.
use audio_provider::{realtime::NativeRealtimeSession, AudioProviderHost};
use base64::{engine::general_purpose::STANDARD, Engine};
use client::{adapter::ClientEventSink, protocol::events::ClientEvent};
use lingxi_core::host::orchestrator::OrchestratorHandle;
use orchestrator::ConversationOrchestrator;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_PCM_BYTES: usize = 256 * 1024;
const MAX_INPUT_BYTES: usize = 360 * 1024;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Input {
    Audio {
        #[serde(rename = "audioBase64")]
        audio_base64: String,
    },
    Commit {},
    Interrupt {
        #[serde(default, rename = "itemId")]
        item_id: Option<String>,
        #[serde(default, rename = "audioEndMs")]
        audio_end_ms: Option<u32>,
    },
    PlaybackCompleted {
        #[serde(default, rename = "itemId")]
        item_id: Option<String>,
    },
}

struct Active {
    epoch: u64,
    session_id: String,
    operation_id: String,
    cancel: CancellationToken,
    native: Option<Arc<NativeRealtimeSession>>,
    task: Option<JoinHandle<()>>,
    sink: Arc<dyn ClientEventSink>,
    execution: Option<Arc<crate::server::RealtimeExecutionOwner>>,
}

pub struct RealtimeAudioController {
    orchestrator: Arc<ConversationOrchestrator>,
    host: Arc<AudioProviderHost>,
    next_epoch: AtomicU64,
    shutdown: AtomicBool,
    admission: tokio::sync::Mutex<()>,
    execution_owners: Option<crate::server::RealtimeExecutionOwners>,
    active: Arc<Mutex<Option<Active>>>,
}
impl RealtimeAudioController {
    pub fn new(orchestrator: Arc<ConversationOrchestrator>, host: Arc<AudioProviderHost>) -> Self {
        Self {
            orchestrator,
            host,
            next_epoch: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            admission: tokio::sync::Mutex::new(()),
            execution_owners: None,
            active: Arc::new(Mutex::new(None)),
        }
    }
    pub(crate) fn with_execution_owners(
        mut self,
        owners: crate::server::RealtimeExecutionOwners,
    ) -> Self {
        self.execution_owners = Some(owners);
        self
    }
    pub fn is_active(&self) -> bool {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }
    pub async fn session_context(&self) -> Result<(String, String), String> {
        self.orchestrator
            .current_audio_binding()
            .await
            .map_err(|error| error.to_string())
    }
    fn take_active(&self) -> Option<Active> {
        self.active.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
    /// Stop admission synchronously, including startup. The shared session's
    /// Drop releases its cancellation lease when the last owner disappears.
    pub fn abort_now(&self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(active) = self.take_active() {
            active.cancel.cancel();
            let execution = active.execution;
            if let Some(execution) = &execution {
                execution.cancellation_token().cancel();
            }
            if let Some(task) = active.task {
                task.abort();
            }
            let native = active.native;
            if let Some(native) = &native {
                native.cancel_now();
            }
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if let Some(native) = native {
                        native.abort().await;
                    }
                    if let Some(execution) = execution {
                        execution.retire().await;
                    }
                });
            }
        }
    }
    pub async fn stop(&self) {
        let _admission = self.admission.lock().await;
        self.stop_inner().await;
    }
    async fn stop_inner(&self) {
        if let Some(active) = self.take_active() {
            active.cancel.cancel();
            if let Some(native) = active.native {
                native.cancel_now();
                native.abort().await;
            }
            if let Some(task) = active.task {
                task.abort();
                let _ = task.await;
            }
            if let Some(execution) = active.execution {
                execution.retire().await;
            }
            emit_closed(&*active.sink, &active.session_id, &active.operation_id).await;
        }
    }
    pub async fn start(self: &Arc<Self>, request_json: String, sink: Arc<dyn ClientEventSink>) {
        let _admission = self.admission.lock().await;
        let session_id = self
            .orchestrator
            .current_session_id()
            .await
            .as_uuid()
            .to_string();
        let mut value: Value = match bounded_request(&request_json) {
            Ok(value) => value,
            Err(message) => {
                emit_error(&*sink, &session_id, "", "invalid_request", message).await;
                return;
            }
        };
        let operation_id = value["operationId"].as_str().unwrap_or_default().to_owned();
        // Current Harness context is authoritative; renderer metadata cannot
        // replace session history, tools, permissions or credential ownership.
        value
            .as_object_mut()
            .expect("validated object")
            .remove("session");
        self.stop_inner().await;
        let execution = match self
            .execution_owners
            .as_ref()
            .map(|owners| owners.begin(&session_id))
            .transpose()
        {
            Ok(owner) => owner,
            Err(error) => {
                emit_error(&*sink, &session_id, &operation_id, "busy", &error).await;
                return;
            }
        };
        let execution_cancel = execution
            .as_ref()
            .map(|owner| owner.cancellation_token())
            .unwrap_or_else(CancellationToken::new);
        let task_execution = execution.clone();
        // Reserve ownership before publishing Active. The guard moves with the
        // factory into the spawned core runner, or drops if startup is cancelled.
        let runner_guard = execution.as_ref().map(|owner| owner.reserve_runner());
        let epoch = self.next_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let cancel = CancellationToken::new();
        let active_state = self.active.clone();
        let orchestrator = self.orchestrator.clone();
        let host = self.host.clone();
        let task_cancel = cancel.clone();
        let task_session = session_id.clone();
        let task_operation = operation_id.clone();
        // Install the owner before the task can complete startup.
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if self.shutdown.load(Ordering::Acquire) {
            return;
        }
        *active = Some(Active {
            epoch,
            session_id,
            operation_id,
            cancel,
            native: None,
            task: None,
            sink: sink.clone(),
            execution,
        });
        let task = tokio::spawn(async move {
            let request = value.to_string();
            let runner = orchestrator.clone();
            let runner_execution = task_execution.clone();
            let startup = NativeRealtimeSession::start_with_runner(
                orchestrator.clone(),
                &host,
                &request,
                move |run| async move {
                    let result = run_owned_agent(runner, runner_execution, run).await;
                    drop(runner_guard);
                    result
                },
            );
            let started = tokio::select! {
                biased;
                _ = task_cancel.cancelled() => None,
                _ = execution_cancel.cancelled() => None,
                result = startup => Some(result),
            };
            let Some(started) = started else {
                if let Some(execution) = &task_execution {
                    execution.retire().await;
                }
                if owns(&active_state, epoch) {
                    emit_closed(&*sink, &task_session, &task_operation).await;
                }
                clear(&active_state, epoch);
                return;
            };
            let (native, mut events) = match started {
                Ok(started) => started,
                Err(error) => {
                    if owns(&active_state, epoch) {
                        emit_failure(&*sink, &task_session, &task_operation, error).await;
                        if let Some(execution) = &task_execution {
                            execution.retire().await;
                        }
                        clear(&active_state, epoch);
                    }
                    return;
                }
            };
            // Retain startup's native cancellation handle before any await,
            // so forced connection teardown can await its tool settlement.
            let installed = {
                let mut active = active_state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(active) = active.as_mut().filter(|active| active.epoch == epoch) {
                    active.native = Some(native.clone());
                    true
                } else {
                    false
                }
            };
            if !installed {
                native.abort().await;
                return;
            }
            if orchestrator
                .current_session_id()
                .await
                .as_uuid()
                .to_string()
                != task_session
            {
                native.abort().await;
                if let Some(execution) = &task_execution {
                    execution.retire().await;
                }
                clear(&active_state, epoch);
                return;
            }
            loop {
                let event = tokio::select! {biased;_ =task_cancel.cancelled()=>break,_=execution_cancel.cancelled()=>break,event=events.recv()=>event};
                let Some(event_json) = event else {
                    break;
                };
                if !owns(&active_state, epoch)
                    || orchestrator
                        .current_session_id()
                        .await
                        .as_uuid()
                        .to_string()
                        != task_session
                {
                    break;
                }
                // Forward the exact shared parser event; identity is checked
                // against the current owner rather than accepted from input.
                let valid = serde_json::from_str::<Value>(&event_json)
                    .ok()
                    .is_some_and(|event| {
                        event["sessionId"].as_str() == Some(&task_session)
                            && event["operationId"].as_str() == Some(&task_operation)
                    });
                if !valid {
                    emit_error(
                        &*sink,
                        &task_session,
                        &task_operation,
                        "native_failure",
                        "realtime session emitted an invalid owner identity",
                    )
                    .await;
                    break;
                }
                sink.emit(ClientEvent::RealtimeAudioEvent {
                    session_id: task_session.clone(),
                    event_json,
                })
                .await;
            }
            native.abort().await;
            if let Some(execution) = &task_execution {
                execution.retire().await;
            }
            if execution_cancel.is_cancelled() && owns(&active_state, epoch) {
                emit_closed(&*sink, &task_session, &task_operation).await;
            }
            clear(&active_state, epoch);
        });
        active.as_mut().expect("installed owner").task = Some(task);
    }
    pub async fn input(&self, input_json: String, sink: Arc<dyn ClientEventSink>) {
        let owner = {
            self.active
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map(|owner| {
                    (
                        owner.session_id.clone(),
                        owner.operation_id.clone(),
                        owner.native.clone(),
                        owner
                            .execution
                            .as_ref()
                            .is_some_and(|execution| execution.cancellation_token().is_cancelled()),
                    )
                })
        };
        let Some((session_id, operation_id, native, execution_cancelled)) = owner else {
            let session_id = self
                .orchestrator
                .current_session_id()
                .await
                .as_uuid()
                .to_string();
            emit_error(
                &*sink,
                &session_id,
                "",
                "unavailable",
                "native realtime session is not active",
            )
            .await;
            return;
        };
        if execution_cancelled {
            self.stop().await;
            return;
        }
        let (input_operation, input) = match parse_owned_input(&input_json) {
            Ok(input) => input,
            Err(error) => {
                emit_failure(&*sink, &session_id, &operation_id, error).await;
                return;
            }
        };
        // A stopped device's late chunks must never enter a replacement owner.
        if input_operation != operation_id {
            return;
        }
        if self
            .orchestrator
            .current_session_id()
            .await
            .as_uuid()
            .to_string()
            != session_id
        {
            self.stop().await;
            return;
        }
        let Some(native) = native else {
            emit_error(
                &*sink,
                &session_id,
                &operation_id,
                "busy",
                "native realtime session is starting",
            )
            .await;
            return;
        };
        let result = (|| match input {
            Input::Audio { audio_base64 } => {
                let pcm = STANDARD.decode(audio_base64).map_err(|_| {
                    audio_provider::failure("invalid_request", "invalid realtime PCM encoding")
                })?;
                if pcm.len() > MAX_PCM_BYTES {
                    return Err(audio_provider::failure(
                        "media_too_large",
                        "realtime PCM chunk is too large",
                    ));
                }
                native.send_audio(pcm)
            }
            Input::Commit {} => native.commit_input(),
            Input::Interrupt {
                item_id,
                audio_end_ms,
            } => native.interrupt(item_id, audio_end_ms),
            Input::PlaybackCompleted { item_id } => native.playback_completed(item_id),
        })();
        if let Err(error) = result {
            emit_failure(&*sink, &session_id, &operation_id, error).await;
        }
    }
}
impl Drop for RealtimeAudioController {
    fn drop(&mut self) {
        self.abort_now();
    }
}
async fn run_owned_agent(
    orchestrator: Arc<ConversationOrchestrator>,
    execution: Option<Arc<crate::server::RealtimeExecutionOwner>>,
    run: audio_provider::realtime::NativeRealtimeRun,
) -> Result<orchestrator::RealtimeAgentEnd, orchestrator::OrchestratorError> {
    let cancel = run.cancel.clone();
    let execution_cancel = execution
        .as_ref()
        .map(|owner| owner.cancellation_token())
        .unwrap_or_else(CancellationToken::new);
    let core = orchestrator.run_realtime_agent(
        run.prepared,
        run.control,
        run.events,
        run.inputs,
        run.output,
        run.limits,
        run.cancel,
    );
    tokio::pin!(core);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            if let Some(owner) = &execution {
                owner.cancel_permissions().await;
            }
            core.await
        },
        _ = execution_cancel.cancelled() => {
            cancel.cancel();
            if let Some(owner) = &execution {
                owner.cancel_permissions().await;
            }
            core.await
        },
        result = &mut core => result,
    }
}
fn owns(state: &Mutex<Option<Active>>, epoch: u64) -> bool {
    state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|owner| owner.epoch == epoch && !owner.cancel.is_cancelled())
}
fn clear(state: &Mutex<Option<Active>>, epoch: u64) {
    let mut active = state.lock().unwrap_or_else(|e| e.into_inner());
    if active.as_ref().is_some_and(|active| active.epoch == epoch) {
        active.take();
    }
}
fn bounded_request(request: &str) -> Result<Value, &'static str> {
    if request.len() > MAX_REQUEST_BYTES {
        return Err("realtime request is too large");
    }
    let value: Value = serde_json::from_str(request).map_err(|_| "invalid realtime request")?;
    if !value.is_object()
        || value["kind"] != "realtime"
        || !value["operationId"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && id.len() <= 128)
    {
        return Err("realtime request requires kind and operation identity");
    }
    Ok(value)
}
fn parse_input(input: &str) -> Result<Input, Value> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(audio_provider::failure(
            "media_too_large",
            "realtime input is too large",
        ));
    }
    serde_json::from_str(input)
        .map_err(|_| audio_provider::failure("invalid_request", "invalid realtime input control"))
}
fn parse_owned_input(input: &str) -> Result<(String, Input), Value> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(audio_provider::failure(
            "media_too_large",
            "realtime input is too large",
        ));
    }
    let mut value: Value = serde_json::from_str(input).map_err(|_| {
        audio_provider::failure("invalid_request", "invalid realtime input control")
    })?;
    let operation_id = value
        .as_object_mut()
        .and_then(|object| object.remove("operationId"))
        .and_then(|value| value.as_str().map(str::to_owned))
        .filter(|id| !id.is_empty() && id.len() <= 128)
        .ok_or_else(|| {
            audio_provider::failure(
                "invalid_request",
                "realtime input requires its operation identity",
            )
        })?;
    let control: Input = serde_json::from_value(value).map_err(|_| {
        audio_provider::failure("invalid_request", "invalid realtime input control")
    })?;
    Ok((operation_id, control))
}
async fn emit_error(
    sink: &dyn ClientEventSink,
    session: &str,
    operation: &str,
    kind: &str,
    message: &str,
) {
    let event_json=json!({"type":"error","kind":kind,"message":message,"sessionId":session,"operationId":operation}).to_string();
    sink.emit(ClientEvent::RealtimeAudioEvent {
        session_id: session.into(),
        event_json,
    })
    .await;
}
async fn emit_closed(sink: &dyn ClientEventSink, session: &str, operation: &str) {
    let event_json =
        json!({"type":"closed","reason":"cancelled","sessionId":session,"operationId":operation})
            .to_string();
    sink.emit(ClientEvent::RealtimeAudioEvent {
        session_id: session.into(),
        event_json,
    })
    .await;
}
async fn emit_failure(sink: &dyn ClientEventSink, session: &str, operation: &str, failure: Value) {
    let error = failure.get("error").unwrap_or(&failure);
    let kind = error
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("native_failure");
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("native realtime operation failed");
    emit_error(sink, session, operation, kind, message).await;
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn microphone_lane_accepts_only_bounded_capture_and_playback_controls() {
        assert!(matches!(
            parse_input(r#"{"type":"audio","audioBase64":"AAA="}"#),
            Ok(Input::Audio { .. })
        ));
        assert!(parse_input(r#"{"type":"tool_result","callId":"forged","output":{}}"#).is_err());
        assert!(parse_input(r#"{"type":"import_history","items":[]}"#).is_err());
        assert!(parse_input(r#"{"type":"commit","sessionId":"forged"}"#).is_err());
        assert!(parse_input(&" ".repeat(MAX_INPUT_BYTES + 1)).is_err());
    }
    #[test]
    fn every_input_carries_its_operation_fence() {
        assert!(parse_owned_input(r#"{"operationId":"owner","type":"commit"}"#).is_ok());
        assert!(parse_owned_input(r#"{"type":"commit"}"#).is_err());
        assert!(
            parse_owned_input(r#"{"operationId":"owner","type":"tool_result","output":{}}"#)
                .is_err()
        );
    }
    #[test]
    fn stale_generation_does_not_clear_replacement_owner() {
        struct NoopSink;
        #[async_trait::async_trait]
        impl ClientEventSink for NoopSink {
            async fn emit(&self, _: ClientEvent) {}
        }
        let cancel = CancellationToken::new();
        let state = Mutex::new(Some(Active {
            epoch: 2,
            session_id: "replacement".into(),
            operation_id: "new-owner".into(),
            cancel: cancel.clone(),
            native: None,
            task: None,
            sink: Arc::new(NoopSink),
            execution: None,
        }));
        assert!(owns(&state, 2));
        clear(&state, 1);
        assert_eq!(
            state.lock().unwrap().as_ref().unwrap().operation_id,
            "new-owner"
        );
        cancel.cancel();
        assert!(!owns(&state, 2));
        clear(&state, 2);
        assert!(state.lock().unwrap().is_none());
    }
    #[test]
    fn startup_requires_owner_and_does_not_accept_unbounded_request() {
        assert!(bounded_request(r#"{"kind":"realtime","operationId":"operation"}"#).is_ok());
        assert!(bounded_request(r#"{"kind":"realtime"}"#).is_err());
        assert!(bounded_request(&" ".repeat(MAX_REQUEST_BYTES + 1)).is_err());
    }
}
