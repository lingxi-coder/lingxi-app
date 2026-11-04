//! Native realtime sessions share the current Harness Agent, tools and history.
use super::{failure, AudioProviderHost, AudioRequest, SessionContext};
use base64::{engine::general_purpose::STANDARD, Engine};
use bytes::Bytes;
use llm_runtime::services::sdk::{self, realtime::*};
use orchestrator::{
    ConversationOrchestrator, RealtimeAgentContext, RealtimeAgentInput, RealtimeAgentLimits,
    RealtimeAgentOutput, RealtimeCompletionBoundary,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

const INPUT_CAPACITY: usize = 32;
const EVENT_CAPACITY: usize = 32;
const MAX_PCM_CHUNK_BYTES: usize = 256 * 1024;

pub(crate) struct Registration {
    pending: Arc<Mutex<BTreeMap<String, CancellationToken>>>,
    id: String,
    removed: AtomicBool,
}
impl Registration {
    pub(crate) fn unregister(&self) {
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

struct RegistrationCompletion {
    registration: Arc<Registration>,
    remaining: AtomicUsize,
}
struct RegistrationPart(Arc<RegistrationCompletion>);
impl Drop for RegistrationPart {
    fn drop(&mut self) {
        if self.0.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.registration.unregister();
        }
    }
}

/// A connected provider conversation whose Agent loop remains owned by the
/// caller. The media attachment consumes this without starting a second Agent.
pub(crate) struct PreparedRealtimeProvider {
    pub(crate) request: AudioRequest,
    pub(crate) prepared: RealtimeAgentContext,
    pub(crate) control: RealtimeControl,
    pub(crate) events: RealtimeEvents,
    pub(crate) driver: RealtimeDriver,
    pub(crate) inputs: mpsc::Receiver<RealtimeAgentInput>,
    pub(crate) input: mpsc::Sender<RealtimeAgentInput>,
    pub(crate) input_format: RealtimeAudioFormat,
    pub(crate) output_format: RealtimeAudioFormat,
    pub(crate) max_chunk_bytes: usize,
    pub(crate) image_input_supported: bool,
    pub(crate) completion_boundary: RealtimeCompletionBoundary,
    pub(crate) session_ready: Value,
    pub(crate) usage_context: Value,
    pub(crate) cancel: CancellationToken,
    pub(crate) _registration: Arc<Registration>,
}

/// Host execution boundary: platforms reserve their ordinary Agent turn owner.
pub struct NativeRealtimeRun {
    /// Authoritative prompt, history, and tool snapshot prepared for this Agent session.
    pub prepared: orchestrator::RealtimeAgentContext,
    /// Provider control handle used by the Agent runner to close or interrupt the conversation.
    pub control: RealtimeControl,
    /// Provider events consumed by the Agent runtime while the conversation is active.
    pub events: RealtimeEvents,
    /// Host-supplied audio, commit, interruption, and playback-completion inputs for the Agent.
    pub inputs: mpsc::Receiver<RealtimeAgentInput>,
    /// Agent-produced realtime events returned to the host for client delivery.
    pub output: mpsc::Sender<RealtimeAgentOutput>,
    /// Per-session limits applied to Agent tool execution.
    pub limits: RealtimeAgentLimits,
    /// Whether the selected provider codec accepts typed image inputs.
    pub image_input_supported: bool,
    /// Cooperative cancellation token signaled when the host closes or aborts the session.
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
    forward_cancel: CancellationToken,
    input_format: RealtimeAudioFormat,
    max_chunk_bytes: usize,
    closing: AtomicBool,
    completion: Arc<Completion>,
}

/// Prepare the exact trusted provider route and return its transport to an
/// owner that already owns the Agent continuation. This is shared by the
/// existing UI realtime path and media Conversation handoff.
pub(crate) async fn prepare_provider_realtime(
    host: &AudioProviderHost,
    mut request: AudioRequest,
    mut prepared: RealtimeAgentContext,
) -> Result<PreparedRealtimeProvider, Value> {
    if request.kind != "realtime" {
        return Err(failure(
            "invalid_request",
            "native realtime requires realtime audio configuration",
        ));
    }
    bind_session(&mut request, &prepared)?;
    let audio_profile_name = requested_audio_profile(&request)?;
    let mut selected = host.select_audio(&request).await?;
    let route = selected.route.clone();
    let snapshot = selected.client.snapshot();
    prepared.audio_profile_name = audio_profile_name;
    prepared.profile_name = Some(route.profile_name.clone());
    prepared.account_scope = Some(route.account_scope.clone());
    if request.cloud.binding == "follow_session" {
        request.session = Some(SessionContext {
            session_id: prepared.session_id.clone(),
            profile_id: route.profile_name.clone(),
            account_scope: route.account_scope.clone(),
        });
    }
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
        instructions.push_str(&format!(
            "\nFor this audio conversation, use {locale} as the requested spoken-language preference."
        ));
    }
    let credential = selected.credential.clone();
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
            &snapshot,&route,AudioRealtimeConfig {model:request.cloud.model_id.clone(),voice:request.voice.clone(),instructions:Some(instructions)},tools,prepared.history.clone(),credential,transport,RealtimeLimits::default()
        ))=>result.map_err(|_|failure("timeout","native realtime connection deadline exceeded"))?.map_err(|_|failure("provider_error","native realtime connection failed"))?,
    };
    selected.account.succeeded().await;
    let input_format_json = format_json(&connected.input_format)?;
    let output_format_json = format_json(&connected.output_format)?;
    let model_id = connected.model.clone();
    let completion_boundary = match (caps.provider_id.as_str(), model_id.as_str()) {
        ("openai", _) => RealtimeCompletionBoundary::ExplicitResponse,
        ("google", "gemini-3.8-live") => RealtimeCompletionBoundary::Gemini38,
        _ => {
            return Err(failure(
                "unsupported",
                "this realtime provider or model has no verified Agent completion contract",
            ));
        }
    };
    let session_ready = json!({
        "type":"session_ready",
        "model":model_id,
        "profileId":route.profile_name,
        "accountScope":route.account_scope,
        "inputFormat":input_format_json,
        "outputFormat":output_format_json,
        "capabilities":connected.capabilities
    });
    let provider_id = caps.provider_id.clone();
    let image_input_supported = provider_id == "openai";
    let usage_context = json!({
        "operationId":request.operation_id.clone(),
        "profileId":route.profile_name.clone(),
        "accountScope":route.account_scope.clone(),
        "modelId":connected.model,
        "providerId":provider_id
    });
    let input_format = connected.input_format.clone();
    let output_format = connected.output_format.clone();
    let max_chunk_bytes = request.max_payload_bytes.min(MAX_PCM_CHUNK_BYTES);
    let (input, inputs) = mpsc::channel(INPUT_CAPACITY);
    Ok(PreparedRealtimeProvider {
        request,
        prepared,
        control: connected.control,
        events: connected.events,
        driver: connected.driver,
        inputs,
        input,
        input_format,
        output_format,
        max_chunk_bytes,
        image_input_supported,
        completion_boundary,
        session_ready,
        usage_context,
        cancel,
        _registration: registration,
    })
}

/// Capture the logical route before account selection replaces it with an
/// execution profile. Explicit audio configuration may differ from the chat.
fn requested_audio_profile(request: &AudioRequest) -> Result<String, Value> {
    let profile = match request.cloud.binding.as_str() {
        "follow_session" => request.session.as_ref().map(|session| &session.profile_id),
        "explicit_profile" => request.cloud.profile_id.as_ref(),
        _ => None,
    }
    .filter(|profile| !profile.trim().is_empty())
    .ok_or_else(|| {
        failure(
            "needs_configuration",
            "native realtime audio profile is unavailable",
        )
    })?;
    Ok(profile.clone())
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
                    run.image_input_supported,
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
        let request = host.parse(request_json)?;
        let prepared = orchestrator.prepare_realtime_agent().await.map_err(|_| {
            failure(
                "unavailable",
                "current Agent session cannot prepare native realtime",
            )
        })?;
        let provider = prepare_provider_realtime(host, request, prepared).await?;
        let PreparedRealtimeProvider {
            request,
            prepared,
            control: provider_control,
            events: provider_events,
            driver,
            inputs: input_rx,
            input,
            input_format,
            output_format: _,
            max_chunk_bytes,
            image_input_supported,
            completion_boundary,
            session_ready,
            usage_context,
            cancel,
            _registration: registration,
        } = provider;
        let session_id = prepared.session_id.clone();
        let operation_id = request.operation_id.clone();
        let (events_tx, events_rx) = mpsc::channel(EVENT_CAPACITY);
        let (json_tx, json_rx) = mpsc::channel(EVENT_CAPACITY);
        // The operation token is held by AudioProviderHost for explicit
        // cancellation. Its children separate Agent cleanup from forwarding:
        // ordinary Agent shutdown cancels only its child, while an operation
        // cancellation stops both.
        let agent_cancel = cancel.child_token();
        let forward_cancel = cancel.child_token();
        let registration_completion = Arc::new(RegistrationCompletion {
            registration,
            remaining: AtomicUsize::new(2),
        });
        let agent_registration = RegistrationPart(registration_completion.clone());
        let forward_registration = RegistrationPart(registration_completion);
        let completion = Arc::new(Completion::default());
        let task_completion = completion.clone();
        let session = Arc::new(Self {
            input,
            control: provider_control.clone(),
            cancel: agent_cancel.clone(),
            forward_cancel: forward_cancel.clone(),
            input_format,
            max_chunk_bytes,
            closing: AtomicBool::new(false),
            completion,
        });
        let control = provider_control.clone();
        let driver = tokio::spawn(driver.run());
        let task_cancel = agent_cancel.clone();
        let agent = tokio::spawn(async move {
            let _completion = CompletionGuard(task_completion);
            let _registration = agent_registration;
            let result = runner(NativeRealtimeRun {
                prepared,
                control: provider_control,
                events: provider_events,
                inputs: input_rx,
                output: events_tx,
                limits: RealtimeAgentLimits {
                    completion_boundary,
                    ..Default::default()
                },
                image_input_supported,
                cancel: task_cancel.clone(),
            })
            .await;
            task_cancel.cancel();
            let _ = control
                .abort(RealtimeClose::normal("native Agent session ended"))
                .await;
            let driver = driver.await;
            match (result, driver) {
                (Err(error), _)
                    if error
                        .to_string()
                        .contains("Gemini completion cannot confirm interactionStatus IDLE") =>
                {
                    Err("native realtime provider did not confirm the Agent turn completed")
                }
                (Err(_), _) => Err("native Agent session failed"),
                (_, Ok(Err(_))) | (_, Err(_)) => Err("native realtime transport failed"),
                (Ok(end), _) => Ok(format!("{end:?}")),
            }
        });
        tokio::spawn(forward_realtime_events(
            events_rx,
            agent,
            json_tx,
            session_ready,
            usage_context,
            session_id,
            operation_id,
            agent_cancel,
            forward_cancel,
            forward_registration,
        ));
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
        if pcm.is_empty() || !pcm.len().is_multiple_of(2) || pcm.len() > self.max_chunk_bytes {
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
    /// Commit queued audio input and request the provider to continue its response.
    ///
    /// Both events are reserved and queued together; the continue event is a
    /// no-op for providers that start turns automatically.
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
    /// Acknowledge that the host finished playing an output item, if it has an identity.
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
    /// Gracefully close the provider conversation and wait for Agent ownership to release.
    ///
    /// If provider close does not complete within the close deadline, the session
    /// is aborted and an `unavailable` error is returned.
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
        self.forward_cancel.cancel();
        self.cancel.cancel();
    }

    /// Cancel the Agent immediately, abort the provider conversation, and await cleanup.
    pub async fn abort(&self) {
        self.closing.store(true, Ordering::Release);
        self.forward_cancel.cancel();
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
        self.forward_cancel.cancel();
        self.cancel.cancel();
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
        input_schema: sdk::protocol::JsonPayload::from_generated_value(
            value
                .get("parameters")
                .filter(|schema| schema.is_object())
                .ok_or_else(|| failure("unavailable", "Agent tool schema is invalid"))?,
        )
        .map_err(|_| failure("unavailable", "Agent tool schema is invalid"))?,
        strict: false,
        defer_loading: false,
        native_options: vec![],
        tool_type: None,
        extra: sdk::protocol::JsonPayload::default(),
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
async fn send_realtime_json(
    output: &mpsc::Sender<String>,
    value: String,
    cancel: &CancellationToken,
) -> bool {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => false,
        result = output.send(value) => result.is_ok(),
    }
}

/// Deliver every accepted Agent event and its terminal result in order. The
/// Agent's cleanup token is intentionally separate from explicit forwarding
/// cancellation: normal Agent completion also cancels the cleanup token.
async fn forward_realtime_events(
    mut events: mpsc::Receiver<RealtimeAgentOutput>,
    agent: tokio::task::JoinHandle<Result<String, &'static str>>,
    output: mpsc::Sender<String>,
    session_ready: Value,
    usage_context: Value,
    session_id: String,
    operation_id: String,
    agent_cancel: CancellationToken,
    forward_cancel: CancellationToken,
    registration: RegistrationPart,
) {
    // Keep operation-id cancellation available while terminal output drains.
    let _registration = registration;
    loop {
        let event = tokio::select! {
            biased;
            _ = forward_cancel.cancelled() => { agent_cancel.cancel(); return; },
            _ = output.closed() => { agent_cancel.cancel(); return; },
            event = events.recv() => event,
        };
        let Some(event) = event else { break };
        let mut event = match canonical_agent_event(event, &session_ready) {
            Ok(Some(event)) => event,
            Ok(None) => continue,
            Err(error) => {
                agent_cancel.cancel();
                error
            }
        };
        if event["type"] == "usage" {
            event["usageContext"] = usage_context.clone();
        }
        if !send_realtime_json(
            &output,
            stamp(event, &session_id, &operation_id),
            &forward_cancel,
        )
        .await
        {
            agent_cancel.cancel();
            return;
        }
    }
    // A closed/cancelled consumer must never make this task join protected
    // tool cleanup. Dropping the join handle detaches that cleanup owner.
    let result = tokio::select! {
        biased;
        _ = forward_cancel.cancelled() => { agent_cancel.cancel(); return; },
        _ = output.closed() => { agent_cancel.cancel(); return; },
        result = agent => result,
    };
    if !matches!(result, Ok(Ok(_))) {
        let message = match &result {
            Ok(Err(message)) => *message,
            _ => "native realtime session ended with an error",
        };
        if !send_realtime_json(
            &output,
            stamp(
                json!({"type":"error","kind":"provider_error","message":message}),
                &session_id,
                &operation_id,
            ),
            &forward_cancel,
        )
        .await
        {
            return;
        }
    }
    let reason = match result {
        Ok(Ok(reason)) => reason,
        _ => "failed".into(),
    };
    let _ = send_realtime_json(
        &output,
        stamp(
            json!({"type":"closed","reason":reason}),
            &session_id,
            &operation_id,
        ),
        &forward_cancel,
    )
    .await;
}

fn canonical_agent_event(
    event: RealtimeAgentOutput,
    ready: &Value,
) -> Result<Option<Value>, Value> {
    match event {
        RealtimeAgentOutput::Provider(event) => canonical_event(event, ready),
        RealtimeAgentOutput::AgentResponseStarted { turn_id } => Ok(Some(
            json!({"type":"agent_response_started","turnId":turn_id}),
        )),
        RealtimeAgentOutput::AgentTurnCompleted { turn_id } => Ok(Some(
            json!({"type":"agent_turn_completed","turnId":turn_id}),
        )),
    }
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

    fn test_registration(
        cancel: &CancellationToken,
    ) -> (Arc<Registration>, RegistrationPart, RegistrationPart) {
        let registration = Arc::new(Registration {
            pending: Arc::new(Mutex::new(BTreeMap::from([(
                "operation".into(),
                cancel.clone(),
            )]))),
            id: "operation".into(),
            removed: AtomicBool::new(false),
        });
        let completion = Arc::new(RegistrationCompletion {
            registration: registration.clone(),
            remaining: AtomicUsize::new(2),
        });
        (
            registration,
            RegistrationPart(completion.clone()),
            RegistrationPart(completion),
        )
    }

    #[tokio::test]
    async fn forwarder_drains_events_and_terminal_after_agent_cleanup_cancels_its_token() {
        for failed in [false, true] {
            let (events_tx, events_rx) = mpsc::channel(4);
            let (json_tx, mut json_rx) = mpsc::channel(1);
            json_tx.send("occupied".into()).await.unwrap();
            let operation_cancel = CancellationToken::new();
            let agent_cancel = operation_cancel.child_token();
            let forward_cancel = operation_cancel.child_token();
            let (registration, agent_registration, forward_registration) =
                test_registration(&operation_cancel);
            let runner_cancel = agent_cancel.clone();
            let agent = tokio::spawn(async move {
                let _registration = agent_registration;
                events_tx
                    .send(RealtimeAgentOutput::AgentResponseStarted {
                        turn_id: "turn".into(),
                    })
                    .await
                    .unwrap();
                events_tx
                    .send(RealtimeAgentOutput::AgentTurnCompleted {
                        turn_id: "turn".into(),
                    })
                    .await
                    .unwrap();
                // Harness cancels this token on both successful and failed
                // exits, even while the consumer's bounded queue is full.
                runner_cancel.cancel();
                drop(events_tx);
                if failed {
                    Err("fixture failed")
                } else {
                    Ok("completed".into())
                }
            });
            let forwarder = tokio::spawn(forward_realtime_events(
                events_rx,
                agent,
                json_tx,
                json!({"type":"session_ready"}),
                json!({}),
                "session".into(),
                "operation".into(),
                agent_cancel.clone(),
                forward_cancel.clone(),
                forward_registration,
            ));
            tokio::time::timeout(Duration::from_secs(2), agent_cancel.cancelled())
                .await
                .unwrap();
            assert!(!forward_cancel.is_cancelled());
            assert!(registration
                .pending
                .lock()
                .unwrap()
                .contains_key("operation"));
            assert_eq!(json_rx.recv().await.as_deref(), Some("occupied"));
            let mut delivered = Vec::new();
            while let Some(raw) = tokio::time::timeout(Duration::from_secs(2), json_rx.recv())
                .await
                .unwrap()
            {
                let event: Value = serde_json::from_str(&raw).unwrap();
                assert_eq!(event["sessionId"], "session");
                assert_eq!(event["operationId"], "operation");
                delivered.push(event);
            }
            forwarder.await.unwrap();
            assert!(!registration
                .pending
                .lock()
                .unwrap()
                .contains_key("operation"));
            let kinds: Vec<_> = delivered
                .iter()
                .map(|event| event["type"].as_str().unwrap())
                .collect();
            if failed {
                assert_eq!(
                    kinds,
                    [
                        "agent_response_started",
                        "agent_turn_completed",
                        "error",
                        "closed"
                    ]
                );
                assert_eq!(delivered[2]["message"], "fixture failed");
                assert_eq!(delivered[3]["reason"], "failed");
            } else {
                assert_eq!(
                    kinds,
                    ["agent_response_started", "agent_turn_completed", "closed"]
                );
                assert_eq!(delivered[2]["reason"], "completed");
            }
        }
    }

    #[tokio::test]
    async fn cancelled_or_dropped_consumer_never_waits_for_agent_cleanup() {
        for cause in ["explicit", "consumer", "operation"] {
            let (events_tx, events_rx) = mpsc::channel(2);
            let (json_tx, json_rx) = mpsc::channel(1);
            json_tx.send("occupied".into()).await.unwrap();
            events_tx
                .send(RealtimeAgentOutput::AgentResponseStarted {
                    turn_id: "turn".into(),
                })
                .await
                .unwrap();
            drop(events_tx);
            let operation_cancel = CancellationToken::new();
            let agent_cancel = operation_cancel.child_token();
            let forward_cancel = operation_cancel.child_token();
            let (registration, agent_registration, forward_registration) =
                test_registration(&operation_cancel);
            let (release, cleanup) = tokio::sync::oneshot::channel::<()>();
            let agent = tokio::spawn(async move {
                let _registration = agent_registration;
                let _ = cleanup.await;
                Ok("completed".into())
            });
            let forwarder = tokio::spawn(forward_realtime_events(
                events_rx,
                agent,
                json_tx,
                json!({}),
                json!({}),
                "session".into(),
                "operation".into(),
                agent_cancel.clone(),
                forward_cancel.clone(),
                forward_registration,
            ));
            tokio::task::yield_now().await;
            assert!(!forwarder.is_finished());
            match cause {
                "consumer" => drop(json_rx),
                "operation" => operation_cancel.cancel(),
                _ => forward_cancel.cancel(),
            }
            tokio::time::timeout(Duration::from_secs(2), forwarder)
                .await
                .unwrap()
                .unwrap();
            assert!(agent_cancel.is_cancelled());
            assert!(
                registration
                    .pending
                    .lock()
                    .unwrap()
                    .contains_key("operation"),
                "protected Agent cleanup still owns the operation"
            );
            release.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let still_registered = registration
                        .pending
                        .lock()
                        .unwrap()
                        .contains_key("operation");
                    if !still_registered {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn operation_can_cancel_a_full_forwarder_after_agent_cleanup_finishes() {
        let (events_tx, events_rx) = mpsc::channel(1);
        let (json_tx, _json_rx) = mpsc::channel(1);
        json_tx.send("occupied".into()).await.unwrap();
        events_tx
            .send(RealtimeAgentOutput::AgentTurnCompleted {
                turn_id: "turn".into(),
            })
            .await
            .unwrap();
        let operation_cancel = CancellationToken::new();
        let agent_cancel = operation_cancel.child_token();
        let forward_cancel = operation_cancel.child_token();
        let (registration, agent_registration, forward_registration) =
            test_registration(&operation_cancel);
        let completed = Arc::new(Completion::default());
        let completed_by_agent = completed.clone();
        let agent = tokio::spawn(async move {
            let _completion = CompletionGuard(completed_by_agent);
            let _registration = agent_registration;
            drop(events_tx);
            Ok("completed".into())
        });
        let forwarder = tokio::spawn(forward_realtime_events(
            events_rx,
            agent,
            json_tx,
            json!({}),
            json!({}),
            "session".into(),
            "operation".into(),
            agent_cancel,
            forward_cancel,
            forward_registration,
        ));
        tokio::time::timeout(Duration::from_secs(2), completed.wait())
            .await
            .unwrap();
        assert!(!forwarder.is_finished());
        let registered_cancel = registration
            .pending
            .lock()
            .unwrap()
            .get("operation")
            .unwrap()
            .clone();
        registered_cancel.cancel();
        tokio::time::timeout(Duration::from_secs(2), forwarder)
            .await
            .unwrap()
            .unwrap();
        assert!(!registration
            .pending
            .lock()
            .unwrap()
            .contains_key("operation"));
    }

    #[tokio::test]
    async fn slow_realtime_json_consumer_keeps_event_order() {
        let (output, mut input) = mpsc::channel(1);
        output.send("first".into()).await.unwrap();
        let cancel = CancellationToken::new();
        let producer =
            tokio::spawn(
                async move { send_realtime_json(&output, "second".into(), &cancel).await },
            );
        tokio::task::yield_now().await;
        assert!(!producer.is_finished());
        assert_eq!(input.recv().await.as_deref(), Some("first"));
        assert_eq!(input.recv().await.as_deref(), Some("second"));
        assert!(producer.await.unwrap());
    }

    #[tokio::test]
    async fn blocked_realtime_json_producer_is_cancellable() {
        let (output, mut input) = mpsc::channel(1);
        output.send("first".into()).await.unwrap();
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let producer = tokio::spawn(async move {
            send_realtime_json(&output, "second".into(), &task_cancel).await
        });
        tokio::task::yield_now().await;
        assert!(!producer.is_finished());
        cancel.cancel();
        assert!(
            !tokio::time::timeout(std::time::Duration::from_secs(2), producer)
                .await
                .unwrap()
                .unwrap()
        );
        assert_eq!(input.recv().await.as_deref(), Some("first"));
        assert!(input.recv().await.is_none());
    }

    #[test]
    fn only_local_agent_output_can_emit_agent_lifecycle_events() {
        let ready = json!({"type":"session_ready"});
        for name in ["agent_response_started", "agent_turn_completed"] {
            assert_eq!(
                canonical_agent_event(
                    RealtimeAgentOutput::Provider(RealtimeEvent::ProviderEvent {
                        name: name.into(),
                        native: json!({"type":name,"turnId":"forged"}),
                    }),
                    &ready
                )
                .unwrap(),
                None
            );
        }
        assert_eq!(
            canonical_agent_event(
                RealtimeAgentOutput::AgentTurnCompleted {
                    turn_id: "agent-1".into()
                },
                &ready
            )
            .unwrap(),
            Some(json!({"type":"agent_turn_completed","turnId":"agent-1"}))
        );
    }
    fn request(session_id: &str, profile_id: &str) -> AudioRequest {
        serde_json::from_value(json!({"operationId":"op","kind":"realtime","cloud":{"binding":"follow_session"},"session":{"sessionId":session_id,"profileId":profile_id,"accountScope":lingxi_core::host::provider_accounts::UNBOUND_ACCOUNT_SCOPE}})).unwrap()
    }
    fn prepared() -> orchestrator::RealtimeAgentContext {
        orchestrator::RealtimeAgentContext {
            session_id: "current-session".into(),
            attachment_id: None,
            profile_name: Some("current-profile".into()),
            session_profile_name: "current-profile".into(),
            audio_profile_name: "current-profile".into(),
            account_scope: Some(lingxi_core::host::provider_accounts::UNBOUND_ACCOUNT_SCOPE.into()),
            conversation_mode: lingxi_core::host::session_context::SessionModeSnapshot {
                session_id: "current-session".into(),
                mode: lingxi_core::host::session_context::ConversationMode::Code,
            },
            conversation_profile_epoch: 0,
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
        assert_eq!(
            requested_audio_profile(&current).unwrap(),
            "current-profile"
        );
        assert_eq!(current.session.unwrap().profile_id, "current-profile");
    }
    #[test]
    fn explicit_audio_override_keeps_its_profile_but_rejects_changed_agent_session() {
        let mut current = request("current-session", "current-profile");
        current.cloud.binding = "explicit_profile".into();
        current.cloud.profile_id = Some("audio-profile".into());
        bind_session(&mut current, &prepared()).unwrap();
        assert_eq!(current.cloud.profile_id.as_deref(), Some("audio-profile"));
        assert_eq!(requested_audio_profile(&current).unwrap(), "audio-profile");
        current.session.as_mut().unwrap().session_id = "old-session".into();
        assert!(bind_session(&mut current, &prepared()).is_err());
    }

    #[test]
    fn concrete_account_binding_survives_unbound_host_context_and_cannot_downgrade() {
        let scope = serde_json::to_string(&llm_runtime::accounts::AccountResourceScope {
            store_scope: "fixture-domain".into(),
            account_id: "b".into(),
            identity_generation: 4,
        })
        .unwrap();
        let mut bound = request("current-session", "account.b");
        bound.session.as_mut().unwrap().account_scope = scope.clone();
        bind_session(&mut bound, &prepared()).unwrap();
        assert_eq!(bound.session.as_ref().unwrap().account_scope, scope);
        // The logical binding selects the policy pool; concrete identity is
        // checked by its shared resolver before provider dispatch.
        assert_eq!(
            bound.session.as_ref().unwrap().profile_id,
            "current-profile"
        );
        let mut concrete = prepared();
        concrete.profile_name = Some("account.b".into());
        concrete.account_scope = Some(scope.clone());
        let mut downgraded = request("current-session", "account.b");
        assert!(bind_session(&mut downgraded, &concrete).is_err());
        let mut wrong_identity = request("current-session", "account.a");
        wrong_identity.session.as_mut().unwrap().account_scope = scope.clone();
        assert!(bind_session(&mut wrong_identity, &prepared()).is_err());
        let mut same = request("current-session", "account.b");
        same.session.as_mut().unwrap().account_scope = scope;
        bind_session(&mut same, &concrete).unwrap();
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
    if request
        .session
        .as_ref()
        .is_some_and(|supplied| supplied.session_id != prepared.session_id)
    {
        return Err(failure(
            "stale_session",
            "Agent session changed before native realtime admission",
        ));
    }
    if request.cloud.binding != "follow_session" {
        return Ok(());
    }
    let profile = prepared
        .profile_name
        .as_ref()
        .filter(|profile| !profile.trim().is_empty())
        .ok_or_else(|| {
            failure(
                "needs_configuration",
                "native realtime requires the current session provider profile",
            )
        })?;
    let prepared_scope = prepared.account_scope.as_ref().ok_or_else(|| {
        failure(
            "needs_configuration",
            "native realtime requires the current session account context",
        )
    })?;
    let intent = lingxi_core::host::provider_accounts::UNBOUND_ACCOUNT_SCOPE;
    let scope = if let Some(supplied) = &request.session {
        if prepared_scope != intent {
            if supplied.account_scope != *prepared_scope || supplied.profile_id != *profile {
                return Err(failure(
                    "stale_session",
                    "native realtime account binding changed",
                ));
            }
        } else if supplied.account_scope == intent {
            if supplied.profile_id != *profile || supplied.account_scope != intent {
                return Err(failure(
                    "stale_session",
                    "Agent audio profile changed before native realtime admission",
                ));
            }
        } else if let Ok(identity) = serde_json::from_str::<
            llm_runtime::accounts::AccountResourceScope,
        >(&supplied.account_scope)
        {
            if supplied.profile_id != format!("account.{}", identity.account_id) {
                return Err(failure(
                    "stale_session",
                    "native realtime account identity does not match its profile",
                ));
            }
        } else if !supplied.account_scope.starts_with("explicit-service.")
            || supplied.profile_id != *profile
        {
            return Err(failure(
                "stale_session",
                "native realtime account scope is invalid",
            ));
        }
        supplied.account_scope.clone()
    } else {
        prepared_scope.clone()
    };
    // The original Host binding chooses the eligible pool. A concrete supplied
    // resource scope is retained and checked by the shared resolver before I/O.
    request.session = Some(SessionContext {
        session_id: prepared.session_id.clone(),
        profile_id: profile.clone(),
        account_scope: scope,
    });
    Ok(())
}
