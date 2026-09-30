//! Desktop [`AudioService`] proxy over the authenticated bridge connection.
//!
//! One request identity owns one pending request. Dropping a caller removes
//! that request and sends a targeted `AudioCancel`; late replies cannot resolve
//! another operation. Capability snapshots are shared with the service and
//! advance at hello/update boundaries.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use client::protocol::audio::{
    AudioCapabilitySnapshotDto, AudioErrorDto, AudioErrorKindDto, AudioInitiatorDto,
    AudioOperationDto, AudioOperationIdDto, AudioOperationKindDto, AudioOperationRequestDto,
    AudioOperationResultDto, AudioOwnerDto, AudioReadinessStateDto, MAX_AUDIO_BASE64_LENGTH,
    MAX_AUDIO_PAYLOAD_BYTES, MAX_AUDIO_SAMPLE_RATE_HZ,
};
use client::protocol::events::ClientEvent;
use lingxi_core::host::audio::{
    AudioCapabilitySnapshot, AudioError, AudioErrorKind, AudioInitiator, AudioOperation,
    AudioOperationContext, AudioOperationId, AudioOperationKind, AudioOperationReadiness,
    AudioOperationSuccess, AudioOwner, AudioReadinessState, AudioRecordingHandle, AudioService,
    AudioStatus,
};
use lingxi_core::host::{SttTranscript, TtsAudio, VoiceRecording};
use tokio::runtime::Handle;
use tokio::sync::{oneshot, Mutex};

/// Transport-supplied destination for audio requests and targeted cancels.
#[async_trait]
pub trait AudioRequestSink: Send + Sync {
    /// Forward one request or cancellation event; false means no client is attached.
    async fn emit_request(&self, request: ClientEvent) -> bool;
}

struct PendingEntry {
    identity: AudioOperationIdDto,
    response: oneshot::Sender<AudioOperationResultDto>,
}

type Pending = Arc<Mutex<HashMap<String, PendingEntry>>>;
type SharedCapabilities = Arc<RwLock<CapabilityState>>;

#[derive(Default)]
struct CapabilityState {
    snapshot: Option<AudioCapabilitySnapshotDto>,
    current_epoch: Option<u64>,
    retired_epochs: HashSet<u64>,
}

/// Desktop implementation of the shared device audio domain service.
pub struct AudioBridge {
    sink: Arc<dyn AudioRequestSink>,
    pending: Pending,
    capabilities: SharedCapabilities,
}

/// Response and capability update half of one connection's audio service.
#[derive(Clone)]
pub struct AudioResponder {
    pending: Pending,
    capabilities: SharedCapabilities,
}

impl AudioResponder {
    /// Resolve only the pending request with the exact UUID, generation and epoch.
    pub async fn resolve(
        &self,
        identity: AudioOperationIdDto,
        result: AudioOperationResultDto,
    ) -> bool {
        let mut pending = self.pending.lock().await;
        // Keep the snapshot read-locked through removal so a service-epoch
        // update cannot race between the check and accepting a stale reply.
        let capabilities = self
            .capabilities
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !capabilities
            .snapshot
            .as_ref()
            .is_some_and(|current| current.service_epoch == identity.service_epoch)
        {
            return false;
        }
        let Some(entry) = pending.get(&identity.id) else {
            return false;
        };
        if entry.identity != identity {
            return false;
        }
        let Some(entry) = pending.remove(&identity.id) else {
            return false;
        };
        drop(capabilities);
        drop(pending);
        entry.response.send(result).is_ok()
    }

    /// Apply the latest observed capability snapshot. Epochs are random
    /// instance identities, so an epoch seen before the current one is retired
    /// rather than ordered numerically. `None` retires the active epoch on
    /// disconnect.
    pub fn update_capabilities(&self, capabilities: Option<AudioCapabilitySnapshotDto>) {
        let mut state = self
            .capabilities
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(incoming) = capabilities {
            if state.current_epoch == Some(incoming.service_epoch) {
                if let Some(current) = state.snapshot.as_ref() {
                    if incoming.support_revision < current.support_revision
                        || (incoming.support_revision == current.support_revision
                            && incoming.supported_operations != current.supported_operations)
                    {
                        return;
                    }
                }
            } else {
                if state.retired_epochs.contains(&incoming.service_epoch) {
                    return;
                }
                if let Some(current_epoch) = state.current_epoch.replace(incoming.service_epoch) {
                    state.retired_epochs.insert(current_epoch);
                }
            }
            state.snapshot = Some(incoming);
        } else {
            state.snapshot = None;
            // The responder is connection-scoped. A reconnect can report the
            // same surviving AudioService epoch, so start fresh epoch history.
            state.current_epoch = None;
            state.retired_epochs.clear();
        }
    }

    /// Drop pending calls and invalidate the device snapshot on disconnect.
    pub async fn drain(&self) -> usize {
        self.update_capabilities(None);
        let mut pending = self.pending.lock().await;
        let count = pending.len();
        pending.clear();
        count
    }

    /// Number of currently parked operations, for tests and diagnostics.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }
}

/// Create one connection-scoped desktop audio service and its response side.
#[must_use]
pub fn new_audio_bridge(sink: Arc<dyn AudioRequestSink>) -> (Arc<AudioBridge>, AudioResponder) {
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let capabilities = Arc::new(RwLock::new(CapabilityState::default()));
    (
        Arc::new(AudioBridge {
            sink,
            pending: pending.clone(),
            capabilities: capabilities.clone(),
        }),
        AudioResponder {
            pending,
            capabilities,
        },
    )
}

#[async_trait]
impl AudioService for AudioBridge {
    fn capabilities(&self) -> AudioCapabilitySnapshot {
        self.capabilities
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshot
            .as_ref()
            .map(snapshot_from_dto)
            .unwrap_or(AudioCapabilitySnapshot {
                service_epoch: 0,
                support_revision: 0,
                supported_operations: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: MAX_AUDIO_PAYLOAD_BYTES as u64,
            })
    }

    async fn execute(
        &self,
        context: AudioOperationContext,
        operation: AudioOperation,
    ) -> Result<AudioOperationSuccess, AudioError> {
        let snapshot = self.capabilities();
        if context.identity.service_epoch != snapshot.service_epoch {
            return Err(AudioError::new(
                AudioErrorKind::Unavailable,
                "audio service changed before the operation could start",
            ));
        }
        if let Some(kind) = operation_kind(&operation) {
            if !snapshot.supported_operations.contains(&kind) {
                return Err(AudioError::new(
                    AudioErrorKind::Unsupported,
                    "the desktop audio service does not support this operation",
                ));
            }
        }

        let expected = expected_result(&operation);
        let default_budget = operation_deadline(&operation);
        let request_payload_limit = context
            .max_payload_bytes
            .min(snapshot.max_payload_bytes)
            .min(MAX_AUDIO_PAYLOAD_BYTES as u64);
        let identity = identity_to_dto(&context.identity);
        let request = AudioOperationRequestDto {
            identity: identity.clone(),
            owner: owner_to_dto(&context.owner),
            initiator: context.initiator.as_ref().map(initiator_to_dto),
            timeout_budget_ms: context.timeout_budget_ms,
            max_payload_bytes: request_payload_limit,
            operation: operation_to_dto(operation),
        };
        let (response_tx, response_rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.contains_key(&identity.id) {
                return Err(AudioError::new(
                    AudioErrorKind::InvalidRequest,
                    "audio operation identity was reused",
                ));
            }
            pending.insert(
                identity.id.clone(),
                PendingEntry {
                    identity: identity.clone(),
                    response: response_tx,
                },
            );
        }
        let mut cancel_on_drop = PendingCancelOnDrop {
            pending: self.pending.clone(),
            sink: self.sink.clone(),
            identity: identity.clone(),
            armed: true,
        };

        if !self
            .sink
            .emit_request(ClientEvent::AudioRequest { request })
            .await
        {
            cancel_on_drop.armed = false;
            self.pending.lock().await.remove(&identity.id);
            return Err(AudioError::new(
                AudioErrorKind::Unavailable,
                "no desktop client is connected to perform the audio operation",
            ));
        }

        let budget = context
            .timeout_budget_ms
            .map(Duration::from_millis)
            .unwrap_or(default_budget);
        let result = match tokio::time::timeout(budget, response_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                return Err(AudioError::new(
                    AudioErrorKind::NativeFailure,
                    "the desktop client disconnected before completing the audio operation",
                ));
            }
            Err(_) => {
                return Err(AudioError::new(
                    AudioErrorKind::Timeout,
                    "the desktop client did not complete the audio operation before its deadline",
                ));
            }
        };

        if let AudioOperationResultDto::Failed { error } = &result {
            cancel_on_drop.armed = false;
            return Err(error_from_dto(error.clone()));
        }
        if !result_matches(expected, &result) {
            return Err(AudioError::new(
                AudioErrorKind::NativeFailure,
                "desktop client returned a result for a different audio operation",
            ));
        }
        let converted = result_from_dto(result, request_payload_limit);
        if converted.is_ok() {
            cancel_on_drop.armed = false;
        }
        converted
    }

    async fn cancel(&self, identity: AudioOperationId) -> Result<(), AudioError> {
        let identity = identity_to_dto(&identity);
        let mut pending = self.pending.lock().await;
        if pending
            .get(&identity.id)
            .is_some_and(|entry| entry.identity == identity)
        {
            pending.remove(&identity.id);
        }
        drop(pending);
        if self
            .sink
            .emit_request(ClientEvent::AudioCancel { identity })
            .await
        {
            Ok(())
        } else {
            Err(AudioError::new(
                AudioErrorKind::Unavailable,
                "no desktop client is connected to cancel the audio operation",
            ))
        }
    }
}

struct PendingCancelOnDrop {
    pending: Pending,
    sink: Arc<dyn AudioRequestSink>,
    identity: AudioOperationIdDto,
    armed: bool,
}

impl Drop for PendingCancelOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let pending = self.pending.clone();
        let sink = self.sink.clone();
        let identity = self.identity.clone();
        if let Ok(runtime) = Handle::try_current() {
            runtime.spawn(async move {
                let mut pending = pending.lock().await;
                if pending
                    .get(&identity.id)
                    .is_some_and(|entry| entry.identity == identity)
                {
                    pending.remove(&identity.id);
                }
                drop(pending);
                let _ = sink
                    .emit_request(ClientEvent::AudioCancel { identity })
                    .await;
            });
        }
    }
}

#[derive(Clone, Copy)]
enum ExpectedResult {
    RecordingStarted,
    Recording,
    Transcript,
    Synthesized,
    PlaybackCompleted,
    Status,
    OwnerEnded,
}

fn expected_result(operation: &AudioOperation) -> ExpectedResult {
    match operation {
        AudioOperation::StartRecording { .. } => ExpectedResult::RecordingStarted,
        AudioOperation::StopRecording { .. } => ExpectedResult::Recording,
        AudioOperation::Listen { .. } => ExpectedResult::Transcript,
        AudioOperation::Synthesize { .. } => ExpectedResult::Synthesized,
        AudioOperation::Speak { .. } => ExpectedResult::PlaybackCompleted,
        AudioOperation::Status { .. } => ExpectedResult::Status,
        AudioOperation::EndOwner => ExpectedResult::OwnerEnded,
    }
}

// Used only for the conservative deadline fallback if a caller omitted a budget.
fn operation_deadline(operation: &AudioOperation) -> Duration {
    match operation {
        AudioOperation::Listen { .. } => Duration::from_secs(180),
        AudioOperation::Speak { text, .. } | AudioOperation::Synthesize { text, .. } => {
            let chars = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
            Duration::from_secs(
                90u64
                    .saturating_add(chars.saturating_mul(200) / 1000)
                    .min(1200),
            )
        }
        _ => Duration::from_secs(30),
    }
}

fn result_matches(expected: ExpectedResult, result: &AudioOperationResultDto) -> bool {
    matches!(
        (expected, result),
        (
            ExpectedResult::RecordingStarted,
            AudioOperationResultDto::RecordingStarted { .. }
        ) | (
            ExpectedResult::Recording,
            AudioOperationResultDto::Recording { .. }
        ) | (
            ExpectedResult::Transcript,
            AudioOperationResultDto::Transcript { .. }
        ) | (
            ExpectedResult::Synthesized,
            AudioOperationResultDto::Synthesized { .. }
        ) | (
            ExpectedResult::PlaybackCompleted,
            AudioOperationResultDto::PlaybackCompleted { .. }
        ) | (
            ExpectedResult::Status,
            AudioOperationResultDto::Status { .. }
        ) | (
            ExpectedResult::OwnerEnded,
            AudioOperationResultDto::OwnerEnded
        )
    )
}

fn operation_kind(operation: &AudioOperation) -> Option<AudioOperationKind> {
    match operation {
        AudioOperation::StartRecording { .. } | AudioOperation::StopRecording { .. } => {
            Some(AudioOperationKind::Record)
        }
        AudioOperation::Listen { .. } => Some(AudioOperationKind::Listen),
        AudioOperation::Synthesize { .. } => Some(AudioOperationKind::Synthesize),
        AudioOperation::Speak { .. } => Some(AudioOperationKind::Speak),
        AudioOperation::Status { .. } | AudioOperation::EndOwner => None,
    }
}

fn identity_to_dto(identity: &AudioOperationId) -> AudioOperationIdDto {
    AudioOperationIdDto {
        id: identity.id.clone(),
        generation: identity.generation,
        service_epoch: identity.service_epoch,
    }
}

fn owner_to_dto(owner: &AudioOwner) -> AudioOwnerDto {
    match owner {
        AudioOwner::Session { session_id } => AudioOwnerDto::Session {
            session_id: session_id.clone(),
        },
        AudioOwner::LocalApp {
            app_id,
            runtime_generation,
        } => AudioOwnerDto::LocalApp {
            app_id: app_id.clone(),
            runtime_generation: *runtime_generation,
        },
        AudioOwner::Ui { instance_id } => AudioOwnerDto::Ui {
            instance_id: instance_id.clone(),
        },
        AudioOwner::System { instance_id } => AudioOwnerDto::System {
            instance_id: instance_id.clone(),
        },
    }
}

fn initiator_to_dto(initiator: &AudioInitiator) -> AudioInitiatorDto {
    AudioInitiatorDto {
        agent_id: initiator.agent_id.clone(),
        tool_use_id: initiator.tool_use_id.clone(),
        request_id: initiator.request_id.clone(),
    }
}

fn operation_to_dto(operation: AudioOperation) -> AudioOperationDto {
    match operation {
        AudioOperation::StartRecording {
            sample_rate_hz,
            format,
        } => AudioOperationDto::StartRecording {
            sample_rate_hz,
            format,
        },
        AudioOperation::StopRecording { handle } => {
            AudioOperationDto::StopRecording { handle: handle.0 }
        }
        AudioOperation::Listen { language } => AudioOperationDto::Listen { language },
        AudioOperation::Synthesize {
            text,
            language,
            rate,
            voice,
        } => AudioOperationDto::Synthesize {
            text,
            language,
            rate,
            voice,
        },
        AudioOperation::Speak {
            text,
            language,
            rate,
            voice,
        } => AudioOperationDto::Speak {
            text,
            language,
            rate,
            voice,
        },
        AudioOperation::Status { handle } => AudioOperationDto::Status {
            handle: handle.map(|handle| handle.0),
        },
        AudioOperation::EndOwner => AudioOperationDto::EndOwner,
    }
}

fn snapshot_from_dto(snapshot: &AudioCapabilitySnapshotDto) -> AudioCapabilitySnapshot {
    AudioCapabilitySnapshot {
        service_epoch: snapshot.service_epoch,
        support_revision: snapshot.support_revision,
        supported_operations: snapshot
            .supported_operations
            .iter()
            .copied()
            .map(kind_from_dto)
            .collect(),
        readiness: snapshot
            .readiness
            .iter()
            .map(|entry| AudioOperationReadiness {
                operation: kind_from_dto(entry.operation),
                state: readiness_from_dto(entry.state),
            })
            .collect(),
        max_payload_bytes: snapshot
            .max_payload_bytes
            .min(MAX_AUDIO_PAYLOAD_BYTES as u64),
    }
}

fn kind_from_dto(kind: AudioOperationKindDto) -> AudioOperationKind {
    match kind {
        AudioOperationKindDto::Record => AudioOperationKind::Record,
        AudioOperationKindDto::Listen => AudioOperationKind::Listen,
        AudioOperationKindDto::Synthesize => AudioOperationKind::Synthesize,
        AudioOperationKindDto::Speak => AudioOperationKind::Speak,
    }
}

fn kind_to_dto(kind: AudioOperationKind) -> AudioOperationKindDto {
    match kind {
        AudioOperationKind::Record => AudioOperationKindDto::Record,
        AudioOperationKind::Listen => AudioOperationKindDto::Listen,
        AudioOperationKind::Synthesize => AudioOperationKindDto::Synthesize,
        AudioOperationKind::Speak => AudioOperationKindDto::Speak,
    }
}

fn readiness_from_dto(state: AudioReadinessStateDto) -> AudioReadinessState {
    match state {
        AudioReadinessStateDto::Ready => AudioReadinessState::Ready,
        AudioReadinessStateDto::NeedsPermission => AudioReadinessState::NeedsPermission,
        AudioReadinessStateDto::Busy => AudioReadinessState::Busy,
        AudioReadinessStateDto::MissingModel => AudioReadinessState::MissingModel,
        AudioReadinessStateDto::Unavailable => AudioReadinessState::Unavailable,
    }
}

fn error_from_dto(error: AudioErrorDto) -> AudioError {
    let kind = match error.kind {
        AudioErrorKindDto::PermissionDenied => AudioErrorKind::PermissionDenied,
        AudioErrorKindDto::Busy => AudioErrorKind::Busy,
        AudioErrorKindDto::Cancelled => AudioErrorKind::Cancelled,
        AudioErrorKindDto::Timeout => AudioErrorKind::Timeout,
        AudioErrorKindDto::NoSpeech => AudioErrorKind::NoSpeech,
        AudioErrorKindDto::NotRecording => AudioErrorKind::NotRecording,
        AudioErrorKindDto::Unavailable => AudioErrorKind::Unavailable,
        AudioErrorKindDto::Unsupported => AudioErrorKind::Unsupported,
        AudioErrorKindDto::ModelMissing => AudioErrorKind::ModelMissing,
        AudioErrorKindDto::VoiceMissing => AudioErrorKind::VoiceMissing,
        AudioErrorKindDto::InvalidRequest => AudioErrorKind::InvalidRequest,
        AudioErrorKindDto::SynthesisFailed => AudioErrorKind::SynthesisFailed,
        AudioErrorKindDto::NativeFailure => AudioErrorKind::NativeFailure,
        AudioErrorKindDto::MediaTooLarge => AudioErrorKind::MediaTooLarge,
        _ => AudioErrorKind::NativeFailure,
    };
    AudioError::new(kind, error.message)
}

fn result_from_dto(
    result: AudioOperationResultDto,
    max_payload_bytes: u64,
) -> Result<AudioOperationSuccess, AudioError> {
    match result {
        AudioOperationResultDto::RecordingStarted { handle } => {
            if handle.trim().is_empty() {
                return Err(AudioError::new(
                    AudioErrorKind::NativeFailure,
                    "desktop client returned an empty recording handle",
                ));
            }
            Ok(AudioOperationSuccess::RecordingStarted {
                handle: AudioRecordingHandle(handle),
            })
        }
        AudioOperationResultDto::Recording {
            audio_base64,
            mime_type,
        } => {
            let audio_bytes = decode_bounded(&audio_base64, max_payload_bytes)?;
            if audio_bytes.is_empty() {
                return Err(AudioError::new(
                    AudioErrorKind::NativeFailure,
                    "desktop client returned an empty recording",
                ));
            }
            Ok(AudioOperationSuccess::Recording {
                recording: VoiceRecording {
                    audio_bytes,
                    mime_type,
                },
            })
        }
        AudioOperationResultDto::Transcript {
            text,
            language,
            confidence,
        } => {
            if confidence.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
                return Err(AudioError::new(
                    AudioErrorKind::NativeFailure,
                    "desktop client returned invalid transcript confidence",
                ));
            }
            Ok(AudioOperationSuccess::Transcript {
                transcript: SttTranscript {
                    text,
                    language,
                    confidence,
                },
            })
        }
        AudioOperationResultDto::Synthesized {
            pcm_base64,
            sample_rate_hz,
        } => {
            if sample_rate_hz == 0 || sample_rate_hz > MAX_AUDIO_SAMPLE_RATE_HZ {
                return Err(AudioError::new(
                    AudioErrorKind::NativeFailure,
                    "native PCM sample rate is outside the supported range",
                ));
            }
            let pcm = decode_bounded(&pcm_base64, max_payload_bytes)?;
            if pcm.is_empty() || pcm.len() % 2 != 0 {
                return Err(AudioError::new(
                    AudioErrorKind::NativeFailure,
                    "native synthesis returned empty or unaligned PCM16 audio",
                ));
            }
            Ok(AudioOperationSuccess::Synthesized {
                audio: TtsAudio {
                    pcm,
                    sample_rate_hz,
                },
            })
        }
        AudioOperationResultDto::PlaybackCompleted { duration_ms } => {
            Ok(AudioOperationSuccess::PlaybackCompleted { duration_ms })
        }
        AudioOperationResultDto::Status { status } => Ok(AudioOperationSuccess::Status {
            status: AudioStatus {
                recording: status.recording,
                playing: status.playing,
            },
        }),
        AudioOperationResultDto::OwnerEnded => Ok(AudioOperationSuccess::OwnerEnded),
        AudioOperationResultDto::Failed { error } => Err(error_from_dto(error)),
        _ => Err(AudioError::new(
            AudioErrorKind::NativeFailure,
            "desktop client returned an unknown audio result",
        )),
    }
}

fn decode_bounded(encoded: &str, max_payload_bytes: u64) -> Result<Vec<u8>, AudioError> {
    if encoded.len() > MAX_AUDIO_BASE64_LENGTH {
        return Err(AudioError::new(
            AudioErrorKind::MediaTooLarge,
            "native audio output exceeds the bridge frame limit",
        ));
    }
    let raw_limit = max_payload_bytes.min(MAX_AUDIO_PAYLOAD_BYTES as u64) as usize;
    if encoded.len() > raw_limit.div_ceil(3) * 4 {
        return Err(AudioError::new(
            AudioErrorKind::MediaTooLarge,
            "native audio output exceeds the requested byte limit",
        ));
    }
    let decoded = STANDARD.decode(encoded).map_err(|_| {
        AudioError::new(
            AudioErrorKind::NativeFailure,
            "native audio output is not valid base64",
        )
    })?;
    if decoded.len() > raw_limit {
        return Err(AudioError::new(
            AudioErrorKind::MediaTooLarge,
            "native audio output exceeds the requested byte limit",
        ));
    }
    Ok(decoded)
}

/// Encode finished recording bytes within the common bridge payload limit.
pub fn encode_audio_payload(bytes: &[u8], max_payload_bytes: u64) -> Result<String, AudioError> {
    if bytes.len() as u64 > max_payload_bytes.min(MAX_AUDIO_PAYLOAD_BYTES as u64) {
        return Err(AudioError::new(
            AudioErrorKind::MediaTooLarge,
            "audio recording exceeds the requested byte limit",
        ));
    }
    Ok(STANDARD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::mpsc;

    struct MockSink {
        tx: mpsc::UnboundedSender<ClientEvent>,
        connected: AtomicBool,
    }

    #[async_trait]
    impl AudioRequestSink for MockSink {
        async fn emit_request(&self, request: ClientEvent) -> bool {
            self.connected.load(Ordering::SeqCst) && self.tx.send(request).is_ok()
        }
    }

    fn service() -> (
        Arc<AudioBridge>,
        AudioResponder,
        mpsc::UnboundedReceiver<ClientEvent>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let sink = Arc::new(MockSink {
            tx,
            connected: AtomicBool::new(true),
        });
        let (bridge, responder) = new_audio_bridge(sink);
        responder.update_capabilities(Some(snapshot(&[
            AudioOperationKindDto::Record,
            AudioOperationKindDto::Listen,
            AudioOperationKindDto::Synthesize,
            AudioOperationKindDto::Speak,
        ])));
        (bridge, responder, rx)
    }

    fn snapshot(supported_operations: &[AudioOperationKindDto]) -> AudioCapabilitySnapshotDto {
        snapshot_at(7, 4, supported_operations)
    }

    fn snapshot_at(
        service_epoch: u64,
        support_revision: u64,
        supported_operations: &[AudioOperationKindDto],
    ) -> AudioCapabilitySnapshotDto {
        AudioCapabilitySnapshotDto {
            service_epoch,
            support_revision,
            supported_operations: supported_operations.to_vec(),
            readiness: Vec::new(),
            max_payload_bytes: 1024,
        }
    }

    fn context(kind: &str) -> AudioOperationContext {
        AudioOperationContext {
            identity: AudioOperationId {
                id: format!("op-{kind}"),
                generation: 2,
                service_epoch: 7,
            },
            owner: AudioOwner::Session {
                session_id: "session-a".into(),
            },
            initiator: Some(AudioInitiator {
                agent_id: Some("agent-a".into()),
                tool_use_id: Some("tool-9".into()),
                request_id: None,
            }),
            timeout_budget_ms: Some(1_000),
            max_payload_bytes: 1024,
        }
    }

    async fn next_event(rx: &mut mpsc::UnboundedReceiver<ClientEvent>) -> ClientEvent {
        tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("audio event is emitted")
            .expect("audio event channel remains open")
    }

    #[tokio::test]
    async fn live_capability_updates_are_visible_and_gate_unsupported_operations() {
        let (bridge, responder, mut rx) = service();
        let before = bridge.capabilities();
        assert!(before
            .supported_operations
            .contains(&AudioOperationKind::Speak));

        responder.update_capabilities(Some(snapshot_at(7, 5, &[AudioOperationKindDto::Record])));
        let after = bridge.capabilities();
        assert_eq!(after.support_revision, 5);
        assert_eq!(after.supported_operations, vec![AudioOperationKind::Record]);
        let error = bridge
            .execute(
                context("unsupported"),
                AudioOperation::Speak {
                    text: "hello".into(),
                    language: None,
                    rate: None,
                    voice: None,
                },
            )
            .await
            .expect_err("unsupported playback must not be sent to the client");
        assert_eq!(error.kind, AudioErrorKind::Unsupported);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn retired_service_epoch_snapshot_cannot_replace_current_capabilities() {
        let (bridge, responder, _) = service();
        responder.update_capabilities(Some(snapshot_at(3, 1, &[AudioOperationKindDto::Speak])));
        responder.update_capabilities(Some(snapshot_at(7, 99, &[AudioOperationKindDto::Record])));

        let current = bridge.capabilities();
        assert_eq!(current.service_epoch, 3);
        assert_eq!(current.support_revision, 1);
        assert_eq!(
            current.supported_operations,
            vec![AudioOperationKind::Speak]
        );
    }

    #[tokio::test]
    async fn older_support_revision_cannot_replace_a_newer_snapshot_in_the_same_epoch() {
        let (bridge, responder, _) = service();
        responder.update_capabilities(Some(snapshot_at(7, 5, &[AudioOperationKindDto::Record])));
        responder.update_capabilities(Some(snapshot_at(7, 4, &[AudioOperationKindDto::Speak])));

        let current = bridge.capabilities();
        assert_eq!(current.service_epoch, 7);
        assert_eq!(current.support_revision, 5);
        assert_eq!(
            current.supported_operations,
            vec![AudioOperationKind::Record]
        );
    }

    #[tokio::test]
    async fn reconnect_can_reuse_the_surviving_audio_service_epoch() {
        let (bridge, responder, _) = service();
        responder.update_capabilities(None);
        responder.update_capabilities(Some(snapshot(&[AudioOperationKindDto::Speak])));

        let current = bridge.capabilities();
        assert_eq!(current.service_epoch, 7);
        assert_eq!(
            current.supported_operations,
            vec![AudioOperationKind::Speak]
        );
    }

    #[tokio::test]
    async fn response_requires_exact_identity_and_matching_operation() {
        let (bridge, responder, mut rx) = service();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .execute(
                        context("listen"),
                        AudioOperation::Listen {
                            language: Some("en-US".into()),
                        },
                    )
                    .await
            }
        });
        let request = match next_event(&mut rx).await {
            ClientEvent::AudioRequest { request } => request,
            other => panic!("expected audio request, got {other:?}"),
        };
        assert_eq!(
            request.owner,
            AudioOwnerDto::Session {
                session_id: "session-a".into()
            }
        );
        assert_eq!(
            request.initiator.as_ref().unwrap().tool_use_id.as_deref(),
            Some("tool-9")
        );
        let mut wrong_identity = request.identity.clone();
        wrong_identity.generation += 1;
        assert!(
            !responder
                .resolve(
                    wrong_identity,
                    AudioOperationResultDto::Transcript {
                        text: "wrong".into(),
                        language: None,
                        confidence: None
                    },
                )
                .await
        );
        assert!(
            responder
                .resolve(
                    request.identity,
                    AudioOperationResultDto::Transcript {
                        text: "hello".into(),
                        language: Some("en-US".into()),
                        confidence: Some(0.9)
                    },
                )
                .await
        );
        let result = task.await.unwrap().unwrap();
        assert!(
            matches!(result, AudioOperationSuccess::Transcript { transcript } if transcript.text == "hello")
        );
        assert_eq!(responder.pending_count().await, 0);
    }

    #[tokio::test]
    async fn old_service_epoch_response_is_rejected_after_capabilities_advance() {
        let (bridge, responder, mut rx) = service();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .execute(
                        context("stale-epoch"),
                        AudioOperation::Listen { language: None },
                    )
                    .await
            }
        });
        let request = match next_event(&mut rx).await {
            ClientEvent::AudioRequest { request } => request,
            other => panic!("expected audio request, got {other:?}"),
        };
        responder.update_capabilities(Some(snapshot_at(1, 1, &[AudioOperationKindDto::Listen])));

        assert!(
            !responder
                .resolve(
                    request.identity.clone(),
                    AudioOperationResultDto::Transcript {
                        text: "stale transcript".into(),
                        language: None,
                        confidence: None,
                    },
                )
                .await
        );
        assert_eq!(responder.pending_count().await, 1);

        task.abort();
        let _ = task.await;
        match next_event(&mut rx).await {
            ClientEvent::AudioCancel { identity } => assert_eq!(identity, request.identity),
            other => panic!("expected targeted audio cancel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dropped_request_clears_pending_and_sends_its_targeted_cancel() {
        let (bridge, responder, mut rx) = service();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .execute(
                        context("speak"),
                        AudioOperation::Speak {
                            text: "hello".into(),
                            language: None,
                            rate: None,
                            voice: None,
                        },
                    )
                    .await
            }
        });
        let request = match next_event(&mut rx).await {
            ClientEvent::AudioRequest { request } => request,
            other => panic!("expected audio request, got {other:?}"),
        };
        assert_eq!(responder.pending_count().await, 1);
        task.abort();
        let _ = task.await;
        match next_event(&mut rx).await {
            ClientEvent::AudioCancel { identity } => assert_eq!(identity, request.identity),
            other => panic!("expected targeted audio cancel, got {other:?}"),
        }
        assert_eq!(responder.pending_count().await, 0);
    }

    #[tokio::test]
    async fn dropped_start_after_client_completion_still_rolls_back_by_origin_identity() {
        let (bridge, responder, mut rx) = service();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .execute(
                        context("start-recording"),
                        AudioOperation::StartRecording {
                            sample_rate_hz: 16_000,
                            format: "wav".into(),
                        },
                    )
                    .await
            }
        });
        let request = match next_event(&mut rx).await {
            ClientEvent::AudioRequest { request } => request,
            other => panic!("expected audio request, got {other:?}"),
        };

        // The client has locally completed capture and removed its pending op,
        // but this current-thread test drops the caller before it can observe
        // the oneshot. Targeted cancellation must still reach the client so it
        // can find and stop the lease retained under this start identity.
        assert!(
            responder
                .resolve(
                    request.identity.clone(),
                    AudioOperationResultDto::RecordingStarted {
                        handle: "recording-1".into(),
                    },
                )
                .await
        );
        task.abort();
        let _ = task.await;
        match next_event(&mut rx).await {
            ClientEvent::AudioCancel { identity } => assert_eq!(identity, request.identity),
            other => panic!("expected origin-identity rollback cancel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wrong_result_and_invalid_pcm_cancel_the_operation() {
        let (bridge, _responder, mut rx) = service();
        let wrong = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .execute(
                        context("wrong-result"),
                        AudioOperation::Listen { language: None },
                    )
                    .await
            }
        });
        let request = match next_event(&mut rx).await {
            ClientEvent::AudioRequest { request } => request,
            other => panic!("expected audio request, got {other:?}"),
        };
        _responder
            .resolve(
                request.identity.clone(),
                AudioOperationResultDto::PlaybackCompleted { duration_ms: 2 },
            )
            .await;
        assert_eq!(
            wrong.await.unwrap().unwrap_err().kind,
            AudioErrorKind::NativeFailure
        );
        match next_event(&mut rx).await {
            ClientEvent::AudioCancel { identity } => assert_eq!(identity, request.identity),
            other => panic!("expected targeted cancel after mismatched result, got {other:?}"),
        }

        let invalid = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .execute(
                        context("invalid-pcm"),
                        AudioOperation::Synthesize {
                            text: "hello".into(),
                            language: None,
                            rate: None,
                            voice: None,
                        },
                    )
                    .await
            }
        });
        let request = match next_event(&mut rx).await {
            ClientEvent::AudioRequest { request } => request,
            other => panic!("expected audio request, got {other:?}"),
        };
        _responder
            .resolve(
                request.identity.clone(),
                AudioOperationResultDto::Synthesized {
                    pcm_base64: STANDARD.encode([1_u8, 2, 3]),
                    sample_rate_hz: 16_000,
                },
            )
            .await;
        assert_eq!(
            invalid.await.unwrap().unwrap_err().kind,
            AudioErrorKind::NativeFailure
        );
        match next_event(&mut rx).await {
            ClientEvent::AudioCancel { identity } => assert_eq!(identity, request.identity),
            other => panic!("expected targeted cancel after invalid PCM, got {other:?}"),
        }
    }
}
