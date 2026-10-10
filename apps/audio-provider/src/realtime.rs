//! Native realtime sessions share the current Harness Agent, tools and history.
use super::{failure, AudioProviderHost, AudioRequest, SessionContext};
use base64::{engine::general_purpose::STANDARD, Engine};
use bytes::Bytes;
use llm_runtime::services::sdk::{self, realtime::*};
use orchestrator::{ConversationOrchestrator, RealtimeAgentInput, RealtimeAgentLimits};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

const INPUT_CAPACITY: usize = 32;
const EVENT_CAPACITY: usize = 32;
const MAX_PCM_CHUNK_BYTES: usize = 256 * 1024;

struct Registration {
    pending: Arc<Mutex<BTreeMap<String, CancellationToken>>>,
    id: String,
    removed: AtomicBool,
}
impl Registration {
    fn unregister(&self) {
        if !self.removed.swap(true, Ordering::AcqRel) {
            self.pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.id);
        }
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.unregister();
    }
}

/// Host execution boundary: platforms reserve their ordinary Agent turn owner.
pub struct NativeRealtimeRun {
    pub prepared: orchestrator::RealtimeAgentContext,
    pub control: RealtimeControl,
    pub events: RealtimeEvents,
    pub inputs: mpsc::Receiver<RealtimeAgentInput>,
    pub output: mpsc::Sender<RealtimeEvent>,
    pub limits: RealtimeAgentLimits,
    pub cancel: CancellationToken,
}
#[derive(Default)]
struct Completion {
    done: AtomicBool,
    notify: Notify,
}
impl Completion {
    fn finish(&self) {
        self.done.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }
    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.done.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

struct CompletionGuard(Arc<Completion>);
impl Drop for CompletionGuard {
    fn drop(&mut self) {
        self.0.finish();
    }
}

/// One native capture/playback owner. Dropping it cancels tools and closes transport.
pub struct NativeRealtimeSession {
    input: mpsc::Sender<RealtimeAgentInput>,
    control: RealtimeControl,
    cancel: CancellationToken,
    registration: Arc<Registration>,
    input_format: RealtimeAudioFormat,
    max_chunk_bytes: usize,
    closing: AtomicBool,
    completion: Arc<Completion>,
}
impl NativeRealtimeSession {
    /// Connect once with an authoritative Agent snapshot. Credentials and profile
    /// configuration remain in the supplied app-scoped audio host.
    pub async fn start(
        orchestrator: Arc<ConversationOrchestrator>,
        host: &AudioProviderHost,
        request_json: &str,
    ) -> Result<(Arc<Self>, mpsc::Receiver<String>), Value> {
        let runner = orchestrator.clone();
        Self::start_with_runner(orchestrator, host, request_json, move |run| async move {
            runner
                .run_realtime_agent(
                    run.prepared,
                    run.control,
                    run.events,
                    run.inputs,
                    run.output,
                    run.limits,
                    run.cancel,
                )
                .await
        })
        .await
    }
    /// Mobile adapters supply the engine's actual connection-owned executor.
    pub async fn start_with_runner<F, Fut>(
        orchestrator: Arc<ConversationOrchestrator>,
        host: &AudioProviderHost,
        request_json: &str,
        runner: F,
    ) -> Result<(Arc<Self>, mpsc::Receiver<String>), Value>
    where
        F: FnOnce(NativeRealtimeRun) -> Fut + Send + 'static,
        Fut: std::future::Future<
                Output = Result<orchestrator::RealtimeAgentEnd, orchestrator::OrchestratorError>,
            > + Send
            + 'static,
    {
        let mut request = host.parse(request_json)?;
        if request.kind != "realtime" {
            return Err(failure(
                "invalid_request",
                "native realtime requires realtime audio configuration",
            ));
        }
        let prepared = orchestrator.prepare_realtime_agent().await.map_err(|_| {
            failure(
                "unavailable",
                "current Agent session cannot prepare native realtime",
            )
        })?;
        bind_session(&mut request, &prepared)?;
        let route = host.resolve(&request)?;
        let snapshot = host.client.snapshot();
        let caps = snapshot
            .audio()
            .capabilities(&route)
            .map_err(|_| failure("unsupported", "selected audio profile is unavailable"))?;
        if !caps.agent_conversation {
            return Err(failure(
                "unsupported",
                "selected profile lacks the native Agent realtime contract",
            ));
        }
        if request.interaction.as_deref() == Some("interruptible")
            && !caps
                .native_realtime_contract
                .is_some_and(|contract| contract.audio_truncation)
        {
            return Err(failure(
                "unsupported",
                "this provider requires turn-based capture because audio truncation is unavailable",
            ));
        }
        let model_descriptor = caps
            .model(
                sdk::audio::AudioOperation::NativeRealtime,
                request.cloud.model_id.as_deref(),
            )
            .map_err(|_| {
                failure(
                    "unsupported",
                    "selected native realtime audio model is unavailable",
                )
            })?;
        if let Some(voice) = request.voice.as_deref() {
            if !model_descriptor
                .voices
                .iter()
                .any(|known| known.id == voice)
            {
                return Err(failure(
                    "unsupported",
                    "native realtime voice is not declared for the selected audio model",
                ));
            }
        }
        request.voice = request
            .voice
            .or_else(|| model_descriptor.default_voice.clone());
        if request
            .rate
            .is_some_and(|rate| !rate.is_finite() || rate != 1.0)
        {
            return Err(failure(
                "unsupported",
                "native realtime does not support a speech rate override",
            ));
        }
        let mut instructions = prepared.instructions.clone();
        if let Some(locale) = request
            .language
            .as_deref()
            .filter(|value| !matches!(*value, "auto" | "default"))
        {
            if locale.is_empty()
                || locale.len() > 64
                || !locale
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err(failure(
                    "invalid_request",
                    "native realtime language preference must be a locale tag",
                ));
            }
            instructions.push_str(&format!("\nFor this audio conversation, use {locale} as the requested spoken-language preference."));
        }
        let credential = host.credential(&route.profile_name).await?;
        let tools = prepared
            .tools
            .iter()
            .map(|tool| tool_spec(tool))
            .collect::<Result<Vec<_>, _>>()?;
        let cancel = CancellationToken::new();
        {
            let mut pending = host
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pending.contains_key(&request.operation_id) {
                return Err(failure(
                    "busy",
                    "audio operation identity is already active",
                ));
            }
            pending.insert(request.operation_id.clone(), cancel.clone());
        }
        let registration = Arc::new(Registration {
            pending: host.pending.clone(),
            id: request.operation_id.clone(),
            removed: AtomicBool::new(false),
        });
        let transport = Arc::new(
            sdk::HttpTransport::new()
                .map_err(|_| failure("unavailable", "native realtime transport is unavailable"))?,
        );
        let connected = tokio::select! {biased;
            _=cancel.cancelled()=>return Err(failure("cancelled","native realtime was cancelled")),
            result=tokio::time::timeout(Duration::from_millis(request.timeout_ms),connect_audio_conversation(
                &snapshot,&route,AudioRealtimeConfig {model:request.cloud.model_id,voice:request.voice,instructions:Some(instructions)},tools,prepared.history.clone(),credential,transport,RealtimeLimits::default()
            ))=>result.map_err(|_|failure("timeout","native realtime connection deadline exceeded"))?.map_err(|_|failure("provider_error","native realtime connection failed"))?,
        };
        let input_format = format_json(&connected.input_format)?;
        let output_format = format_json(&connected.output_format)?;
        let session_ready = json!({"type":"session_ready","model":connected.model,"inputFormat":input_format,"outputFormat":output_format,"capabilities":connected.capabilities});
        let usage_context = json!({"operationId":request.operation_id,"profileId":route.profile_name,
            "accountScope":route.account_scope,"modelId":connected.model,"providerId":caps.provider_id});
        let session_id = prepared.session_id.clone();
        let operation_id = request.operation_id;
        let (input, input_rx) = mpsc::channel(INPUT_CAPACITY);
        let (events_tx, mut events_rx) = mpsc::channel(EVENT_CAPACITY);
        let (json_tx, json_rx) = mpsc::channel(EVENT_CAPACITY);
        let completion = Arc::new(Completion::default());
        let task_completion = completion.clone();
        let session = Arc::new(Self {
            input,
            control: connected.control.clone(),
            cancel: cancel.clone(),
            registration: registration.clone(),
            input_format: connected.input_format,
            max_chunk_bytes: request.max_payload_bytes.min(MAX_PCM_CHUNK_BYTES),
            closing: AtomicBool::new(false),
            completion,
        });
        let control = connected.control.clone();
        let driver = tokio::spawn(connected.driver.run());
        let task_cancel = cancel.clone();
        let agent = tokio::spawn(async move {
            let _completion = CompletionGuard(task_completion);
            let result = runner(NativeRealtimeRun {
                prepared,
                control: connected.control,
                events: connected.events,
                inputs: input_rx,
                output: events_tx,
                limits: RealtimeAgentLimits::default(),
                cancel: task_cancel.clone(),
            })
            .await;
            task_cancel.cancel();
            let _ = control
                .abort(RealtimeClose::normal("native Agent session ended"))
                .await;
            let driver = driver.await;
            registration.unregister();
            match (result, driver) {
                (Err(_), _) => Err("native Agent session failed"),
                (_, Ok(Err(_))) | (_, Err(_)) => Err("native realtime transport failed"),
                (Ok(end), _) => Ok(format!("{end:?}")),
            }
        });
        tokio::spawn(async move {
            while let Some(event) = events_rx.recv().await {
                let event = canonical_event(event, &session_ready);
                let mut event = match event {
                    Ok(Some(event)) => event,
                    Ok(None) => continue,
                    Err(error) => {
                        cancel.cancel();
                        error
                    }
                };
                if event["type"] == "usage" {
                    event["usageContext"] = usage_context.clone();
                }
                if json_tx
                    .try_send(stamp(event, &session_id, &operation_id))
                    .is_err()
                {
                    cancel.cancel();
                    break;
                }
            }
            let result = agent.await;
            if !matches!(result, Ok(Ok(_))) {
                let _=json_tx.try_send(stamp(json!({"type":"error","kind":"provider_error","message":"native realtime session ended with an error"}),&session_id,&operation_id));
            }
            let reason = match result {
                Ok(Ok(reason)) => reason,
                _ => "failed".into(),
            };
            let _ = json_tx.try_send(stamp(
                json!({"type":"closed","reason":reason}),
                &session_id,
                &operation_id,
            ));
        });
        Ok((session, json_rx))
    }
    fn active(&self) -> Result<(), Value> {
        if self.cancel.is_cancelled() || self.closing.load(Ordering::Acquire) {
            Err(failure("cancelled", "native realtime session is closed"))
        } else {
            Ok(())
        }
    }
    fn enqueue(&self, input: RealtimeAgentInput) -> Result<(), Value> {
        self.active()?;
        self.input
            .try_send(input)
            .map_err(|_| failure("busy", "native realtime input queue is full or closed"))
    }
    /// PCM16 little-endian, mono chunks in the rate declared by session_ready.
    pub fn send_audio(&self, pcm: Vec<u8>) -> Result<(), Value> {
        if pcm.is_empty() || pcm.len() % 2 != 0 || pcm.len() > self.max_chunk_bytes {
            return Err(failure(
                "invalid_request",
                "native realtime audio requires a bounded nonempty PCM16 chunk",
            ));
        }
        self.enqueue(RealtimeAgentInput::Provider(RealtimeInput::Audio {
            data: Bytes::from(pcm),
            format: self.input_format.clone(),
        }))
    }
    pub fn commit_input(&self) -> Result<(), Value> {
        self.active()?;
        let permits = self
            .input
            .try_reserve_many(2)
            .map_err(|_| failure("busy", "native realtime input queue is full or closed"))?;
        // ContinueResponse is an SDK no-op for providers with automatic turns.
        let items = [
            RealtimeAgentInput::Provider(RealtimeInput::CommitAudio),
            RealtimeAgentInput::Provider(RealtimeInput::ContinueResponse),
        ];
        for (permit, item) in permits.zip(items) {
            permit.send(item);
        }
        Ok(())
    }
    /// Truncation and interruption are admitted together; no partial queue mutation.
    pub fn interrupt(
        &self,
        item_id: Option<String>,
        audio_end_ms: Option<u32>,
    ) -> Result<(), Value> {
        self.active()?;
        match (item_id, audio_end_ms) {
            (Some(item_id), Some(audio_end_ms)) => {
                if !self.control.capabilities().audio_truncation {
                    return Err(failure(
                        "unsupported",
                        "this provider cannot truncate played audio; use turn-based capture",
                    ));
                }
                if item_id.trim().is_empty() {
                    return Err(failure(
                        "invalid_request",
                        "audio truncation requires an item identity",
                    ));
                }
                let permits = self.input.try_reserve_many(2).map_err(|_| {
                    failure("busy", "native realtime input queue is full or closed")
                })?;
                let items = [
                    RealtimeAgentInput::Provider(RealtimeInput::TruncateAudio {
                        item_id,
                        content_index: 0,
                        audio_end_ms,
                    }),
                    RealtimeAgentInput::Provider(RealtimeInput::Interrupt),
                ];
                for (permit, item) in permits.zip(items) {
                    permit.send(item);
                }
                Ok(())
            }
            (None, None) => self.enqueue(RealtimeAgentInput::Provider(RealtimeInput::Interrupt)),
            _ => Err(failure(
                "invalid_request",
                "audio truncation needs both item identity and played duration",
            )),
        }
    }
    pub fn playback_completed(&self, item_id: Option<String>) -> Result<(), Value> {
        if item_id.as_deref().is_some_and(|id| id.trim().is_empty()) {
            return Err(failure(
                "invalid_request",
                "playback acknowledgement item identity is empty",
            ));
        }
        self.enqueue(RealtimeAgentInput::PlaybackCompleted { item_id })
    }
    /// Resolves only after tool cancellation policy and Agent ownership release.
    pub async fn wait_closed(&self) {
        self.completion.wait().await;
    }
    pub async fn close(&self) -> Result<(), Value> {
        if self.closing.swap(true, Ordering::AcqRel) {
            self.wait_closed().await;
            return Ok(());
        }
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            self.control
                .close(RealtimeClose::normal("native audio closed")),
        )
        .await;
        self.cancel.cancel();
        self.wait_closed().await;
        match result {
            Ok(Ok(())) => Ok(()),
            _ => {
                let _ = self
                    .control
                    .abort(RealtimeClose::normal("native audio close interrupted"))
                    .await;
                Err(failure(
                    "unavailable",
                    "native realtime close did not complete",
                ))
            }
        }
    }
    /// Cancel immediately; the owner remains reserved until protected tools drain.
    pub fn cancel_now(&self) {
        self.closing.store(true, Ordering::Release);
        self.cancel.cancel();
    }

    pub async fn abort(&self) {
        self.closing.store(true, Ordering::Release);
        self.cancel.cancel();
        let _ = self
            .control
            .abort(RealtimeClose::normal("native audio aborted"))
            .await;
        self.wait_closed().await;
    }
}
impl Drop for NativeRealtimeSession {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.registration.unregister();
    }
}
fn tool_spec(value: &Value) -> Result<sdk::protocol::ToolSpec, Value> {
    Ok(sdk::protocol::ToolSpec {
        name: value
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| failure("unavailable", "Agent tool catalog is invalid"))?
            .into(),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        input_schema: value
            .get("parameters")
            .cloned()
            .filter(Value::is_object)
            .ok_or_else(|| failure("unavailable", "Agent tool schema is invalid"))?,
        input_schema_json: None,
        strict: false,
        defer_loading: false,
        native_options: vec![],
        tool_type: None,
        extra: Value::Null,
    })
}
fn format_json(format: &RealtimeAudioFormat) -> Result<Value, Value> {
    match format {
        RealtimeAudioFormat::Pcm16 { sample_rate_hz } => {
            Ok(json!({"encoding":"pcm16","sampleRateHz":sample_rate_hz,"channels":1}))
        }
        _ => Err(failure(
            "unsupported",
            "native playback requires PCM16 mono audio",
        )),
    }
}
fn stamp(mut event: Value, session_id: &str, operation_id: &str) -> String {
    if let Some(object) = event.as_object_mut() {
        object.insert("sessionId".into(), json!(session_id));
        object.insert("operationId".into(), json!(operation_id));
    }
    event.to_string()
}
fn canonical_event(event: RealtimeEvent, ready: &Value) -> Result<Option<Value>, Value> {
    let event = match event {
        RealtimeEvent::SessionReady => ready.clone(),
        RealtimeEvent::AudioDelta {
            data,
            format,
            item_id,
        } => {
            let format = format_json(&format).map_err(|_|json!({"type":"error","kind":"unsupported","message":"provider audio encoding is not playable PCM16"}))?;
            if data.len() % 2 != 0 {
                return Err(
                    json!({"type":"error","kind":"provider_error","message":"provider returned an incomplete PCM16 sample"}),
                );
            }
            json!({"type":"audio_delta","audioBase64":STANDARD.encode(data),"itemId":item_id,"sampleRateHz":format["sampleRateHz"],"channels":1,"encoding":"pcm16"})
        }
        RealtimeEvent::Transcript {
            direction,
            text,
            item_id,
            turn_id,
            final_chunk,
            update,
        } => {
            json!({"type":"transcript","update":match update {RealtimeTranscriptUpdate::Delta=>"delta",RealtimeTranscriptUpdate::Replace=>"replace"},"role":match direction {RealtimeTranscriptDirection::Input=>"user",RealtimeTranscriptDirection::Output=>"assistant"},"text":text,"final":final_chunk,"itemId":item_id,"turnId":turn_id})
        }
        RealtimeEvent::TextDelta { .. } => return Ok(None),
        RealtimeEvent::TurnStarted { turn_id } => json!({"type":"turn_started","turnId":turn_id}),
        RealtimeEvent::TurnCompleted { turn_id, status } => {
            json!({"type":"turn_completed","turnId":turn_id,"status":status})
        }
        RealtimeEvent::ToolCall {
            call_id,
            name,
            arguments,
        } => json!({"type":"tool_call","callId":call_id,"name":name,"arguments":arguments}),
        RealtimeEvent::ToolCancelled { call_ids } => {
            json!({"type":"tool_cancelled","callIds":call_ids})
        }
        RealtimeEvent::Usage {
            turn_id,
            input_tokens,
            output_tokens,
            total_tokens,
            native,
        } => {
            json!({"type":"usage","turnId":turn_id,"inputTokens":input_tokens,"outputTokens":output_tokens,"totalTokens":total_tokens,"native":native})
        }
        RealtimeEvent::SessionResumption { resumable, .. } => {
            json!({"type":"session_resumption","resumable":resumable})
        }
        RealtimeEvent::UserSpeechStarted => json!({"type":"user_speech_started"}),
        RealtimeEvent::Interrupted => json!({"type":"interrupted"}),
        RealtimeEvent::ProviderError { .. } | RealtimeEvent::ConnectionInterrupted { .. } => {
            json!({"type":"error","kind":"provider_error","message":"native realtime provider failed"})
        }
        RealtimeEvent::Closed { code, .. } => json!({"type":"closed","code":code}),
        RealtimeEvent::ProviderEvent { .. } => return Ok(None),
    };
    Ok(Some(event))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(session_id: &str, profile_id: &str) -> AudioRequest {
        serde_json::from_value(json!({"operationId":"op","kind":"realtime","cloud":{"binding":"follow_session"},"session":{"sessionId":session_id,"profileId":profile_id,"accountScope":format!("profile:{profile_id}")}})).unwrap()
    }
    fn prepared() -> orchestrator::RealtimeAgentContext {
        orchestrator::RealtimeAgentContext {
            session_id: "current-session".into(),
            profile_name: Some("current-profile".into()),
            instructions: String::new(),
            history: vec![],
            tools: vec![],
        }
    }
    #[test]
    fn stale_session_or_profile_is_rejected_before_rebinding() {
        let mut old = request("old-session", "current-profile");
        assert!(bind_session(&mut old, &prepared()).is_err());
        assert_eq!(old.session.unwrap().session_id, "old-session");
        let mut old = request("current-session", "old-profile");
        assert!(bind_session(&mut old, &prepared()).is_err());
        let mut current = request("current-session", "current-profile");
        bind_session(&mut current, &prepared()).unwrap();
        assert_eq!(current.session.unwrap().profile_id, "current-profile");
    }
    #[test]
    fn explicit_audio_override_keeps_its_profile_but_rejects_changed_agent_session() {
        let mut current = request("current-session", "current-profile");
        current.cloud.binding = "explicit_profile".into();
        current.cloud.profile_id = Some("audio-profile".into());
        bind_session(&mut current, &prepared()).unwrap();
        assert_eq!(current.cloud.profile_id.as_deref(), Some("audio-profile"));
        current.session.as_mut().unwrap().session_id = "old-session".into();
        assert!(bind_session(&mut current, &prepared()).is_err());
    }

    #[test]
    fn pcm_event_is_raw_and_carries_provider_rate() {
        let event = canonical_event(
            RealtimeEvent::AudioDelta {
                data: Bytes::from_static(&[0, 255, 1, 128]),
                format: RealtimeAudioFormat::Pcm16 {
                    sample_rate_hz: 24000,
                },
                item_id: Some("item-1".into()),
            },
            &Value::Null,
        )
        .unwrap()
        .unwrap();
        assert_eq!(event["audioBase64"], "AP8BgA==");
        assert_eq!(event["sampleRateHz"], 24000);
        assert_eq!(event["itemId"], "item-1");
    }
    #[test]
    fn generated_text_is_not_a_second_transcript_and_truncation_is_not_guessed() {
        assert!(canonical_event(
            RealtimeEvent::TextDelta {
                text: "duplicate".into(),
                item_id: None,
                final_chunk: true
            },
            &Value::Null
        )
        .unwrap()
        .is_none());
        assert!(format_json(&RealtimeAudioFormat::G711MuLaw).is_err());
    }
    #[test]
    fn transcript_completion_and_usage_keep_native_identity() {
        let event = canonical_event(
            RealtimeEvent::Transcript {
                direction: RealtimeTranscriptDirection::Output,
                update: RealtimeTranscriptUpdate::Replace,
                text: "heard text".into(),
                item_id: Some("item".into()),
                turn_id: Some("turn".into()),
                final_chunk: true,
            },
            &Value::Null,
        )
        .unwrap()
        .unwrap();
        assert_eq!(event["role"], "assistant");
        assert_eq!(event["final"], true);
        let event = canonical_event(
            RealtimeEvent::Usage {
                turn_id: Some("turn".into()),
                input_tokens: Some(3),
                output_tokens: Some(4),
                total_tokens: Some(7),
                native: json!({"input_tokens":3}),
            },
            &Value::Null,
        )
        .unwrap()
        .unwrap();
        assert_eq!(event["turnId"], "turn");
        assert_eq!(event["totalTokens"], 7);
    }
}

fn bind_session(
    request: &mut AudioRequest,
    prepared: &orchestrator::RealtimeAgentContext,
) -> Result<(), Value> {
    if let Some(supplied) = &request.session {
        if supplied.session_id != prepared.session_id {
            return Err(failure(
                "stale_session",
                "Agent session changed before native realtime admission",
            ));
        }
        if request.cloud.binding == "follow_session"
            && (prepared.profile_name.as_deref() != Some(supplied.profile_id.as_str())
                || supplied.account_scope != format!("profile:{}", supplied.profile_id))
        {
            return Err(failure(
                "stale_session",
                "Agent audio profile changed before native realtime admission",
            ));
        }
    }
    if request.cloud.binding == "follow_session" {
        let profile = prepared
            .profile_name
            .as_ref()
            .filter(|profile| !profile.trim().is_empty())
            .ok_or_else(|| {
                failure(
                    "needs_configuration",
                    "native realtime requires the current session's exact profile",
                )
            })?;
        request.session = Some(SessionContext {
            session_id: prepared.session_id.clone(),
            profile_id: profile.clone(),
            account_scope: format!("profile:{profile}"),
        });
    }
    Ok(())
}
