//! Adapter from the product's versioned native callback DTOs to shared
//! owner-bound media device leases.

use async_trait::async_trait;
use client::protocol::media as dto;
use media_api::{
    MediaDevicePorts, MediaDeviceSession, MediaError, MediaErrorKind, MediaFrame,
    MediaFramePayload, MediaInitiator, MediaKind, MediaOperationContext, MediaOperationId,
    MediaOwner, MediaPcmFrame, MediaRenderReceipt, MediaSessionIntent, MediaSessionMode,
    MediaSessionRequest, MediaSessionState, MediaSessionStatus, MediaSessionUpdate, MediaSourceId,
    MediaSourceRef, MediaVideoFrame,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex as StdMutex, OnceLock,
};
use std::time::Duration;
use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};

// Keep an opening callback alive after its caller is cancelled. Native callback
// implementations cannot all recover a lease that has been opened but whose
// response was dropped at the FFI/Bridge boundary. This executor is process
// scoped so dropping an individual host runtime cannot cancel that recovery.
// A permit remains held by each open or live lease, bounding stuck callbacks.
const MAX_NATIVE_MEDIA_LEASES: usize = 256;
const NATIVE_MEDIA_STOP_TIMEOUT: Duration = Duration::from_secs(30);
const NATIVE_MEDIA_STOP_ATTEMPTS: usize = 3;

struct NativeMediaExecutor {
    runtime: tokio::runtime::Runtime,
    leases: Arc<Semaphore>,
}

fn native_media_executor() -> Option<&'static NativeMediaExecutor> {
    static EXECUTOR: OnceLock<Option<NativeMediaExecutor>> = OnceLock::new();
    EXECUTOR
        .get_or_init(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("native-media-cleanup")
                .enable_all()
                .build()
                .ok()?;
            Some(NativeMediaExecutor {
                runtime,
                leases: Arc::new(Semaphore::new(MAX_NATIVE_MEDIA_LEASES)),
            })
        })
        .as_ref()
}

/// Schedule a bounded stop for an opened native lease whose response was
/// abandoned. The permit stays reserved until native cleanup is acknowledged;
/// an unrecoverable failure quarantines it for the lifetime of its semaphore.
pub fn schedule_native_media_stop(
    callback: Arc<dyn NativeMediaDeviceCallback>,
    lease_id: String,
    permit: OwnedSemaphorePermit,
) {
    let Some(executor) = native_media_executor() else {
        permit.forget();
        tracing::error!("native media cleanup executor is unavailable");
        return;
    };
    executor.runtime.spawn(async move {
        for attempt in 0..NATIVE_MEDIA_STOP_ATTEMPTS {
            let request_id = uuid::Uuid::new_v4().to_string();
            let request = dto::MediaDeviceRequestEnvelopeDto {
                request_id: request_id.clone(),
                request: dto::MediaDeviceRequestDto::Stop {
                    lease_id: lease_id.clone(),
                },
            };
            let result =
                tokio::time::timeout(NATIVE_MEDIA_STOP_TIMEOUT, callback.request(request)).await;
            if matches!(
                result,
                Ok(dto::MediaDeviceResponseDto {
                    request_id: ref response_id,
                    result: dto::MediaDeviceResultDto::Stopped
                        | dto::MediaDeviceResultDto::Error {
                            error: dto::MediaErrorDto {
                                kind: dto::MediaErrorKindDto::NotFound,
                                ..
                            }
                        },
                }) if response_id == &request_id
            ) {
                return;
            }
            if attempt + 1 < NATIVE_MEDIA_STOP_ATTEMPTS {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        // Do not admit another lease into a slot whose native cleanup has not
        // been acknowledged. The process-scoped permit stays quarantined.
        permit.forget();
        tracing::error!("abandoned native media lease cleanup was not acknowledged");
    });
}

/// Native product callback that owns the platform's shared mic, camera, and
/// renderer coordinator. One request always receives one correlated response.
#[async_trait]
pub trait NativeMediaDeviceCallback: Send + Sync {
    /// Freeze any connection-scoped routing for an open and its resulting
    /// lease. Platforms with an app-scoped callback may use the default.
    fn bind_for_open(&self) -> Option<Arc<dyn NativeMediaDeviceCallback>> {
        None
    }

    /// Execute one validated callback request against the app's current device
    /// coordinator. The callback must never create a parallel audio route.
    async fn request(
        &self,
        request: dto::MediaDeviceRequestEnvelopeDto,
    ) -> dto::MediaDeviceResponseDto;
}

/// Shared adapter used by Android, iOS, and Desktop callback implementations.
pub struct CallbackMediaDevicePorts {
    callback: Arc<dyn NativeMediaDeviceCallback>,
}

impl CallbackMediaDevicePorts {
    /// Bind the host to the platform's app-scoped media coordinator.
    #[must_use]
    pub fn new(callback: Arc<dyn NativeMediaDeviceCallback>) -> Self {
        Self { callback }
    }

    async fn call(
        &self,
        request: dto::MediaDeviceRequestDto,
        max_payload_bytes: u64,
    ) -> Result<dto::MediaDeviceResultDto, MediaError> {
        let envelope = dto::MediaDeviceRequestEnvelopeDto {
            request_id: uuid::Uuid::new_v4().to_string(),
            request,
        };
        envelope.validate().map_err(invalid_callback)?;
        let request_id = envelope.request_id.clone();
        let response = self.callback.request(envelope).await;
        response.validate().map_err(invalid_callback)?;
        if response.request_id != request_id {
            return Err(MediaError::new(
                MediaErrorKind::NativeFailure,
                "native media callback returned a mismatched request identity",
            ));
        }
        if let dto::MediaDeviceResultDto::Input { frame: Some(frame) } = &response.result {
            frame.validate().map_err(invalid_callback)?;
            if dto_frame_payload_bytes(frame) as u64 > max_payload_bytes {
                return Err(MediaError::new(
                    MediaErrorKind::MediaTooLarge,
                    "native media callback returned a frame over the operation payload limit",
                ));
            }
        }
        match response.result {
            dto::MediaDeviceResultDto::Error { error } => Err(error_from_dto(error)),
            result => Ok(result),
        }
    }

    async fn open_owned(
        &self,
        context: MediaOperationContext,
        request: MediaSessionRequest,
        permit: OwnedSemaphorePermit,
    ) -> Result<Arc<dyn MediaDeviceSession>, MediaError> {
        let result = self
            .call(
                dto::MediaDeviceRequestDto::Open {
                    request: dto::MediaDeviceOpenRequestDto {
                        identity: operation_id_to_dto(context.identity.clone()),
                        owner: owner_to_dto(context.owner.clone()),
                        initiator: context.initiator.clone().map(initiator_to_dto),
                        timeout_budget_ms: context.timeout_budget_ms,
                        max_payload_bytes: context.max_payload_bytes,
                        request: session_request_to_dto(&request),
                    },
                },
                context.max_payload_bytes,
            )
            .await?;
        let dto::MediaDeviceResultDto::Opened { lease } = result else {
            return Err(unexpected_result("open", &result));
        };
        let valid_lease_id = !lease.lease_id.is_empty() && lease.lease_id.len() <= 128;
        let session = Arc::new(CallbackMediaDeviceSession {
            callback: self.callback.clone(),
            lease_id: lease.lease_id,
            context,
            audio_source: request.audio_source,
            video_source: request.video_source,
            stopped: AtomicBool::new(false),
            stop_gate: tokio::sync::Mutex::new(()),
            permit: StdMutex::new(Some(permit)),
        });
        if !valid_lease_id {
            return Err(MediaError::new(
                MediaErrorKind::NativeFailure,
                "native media callback returned an invalid lease identity",
            ));
        }
        Ok(session)
    }
}

#[async_trait]
impl MediaDevicePorts for CallbackMediaDevicePorts {
    async fn open(
        &self,
        context: MediaOperationContext,
        request: MediaSessionRequest,
    ) -> Result<Arc<dyn MediaDeviceSession>, MediaError> {
        let executor = native_media_executor().ok_or_else(|| {
            MediaError::new(
                MediaErrorKind::Unavailable,
                "native media cleanup executor is unavailable",
            )
        })?;
        let permit = executor.leases.clone().try_acquire_owned().map_err(|_| {
            MediaError::new(MediaErrorKind::Busy, "native media lease capacity is full")
        })?;
        let callback = self
            .callback
            .bind_for_open()
            .unwrap_or_else(|| self.callback.clone());
        let ports = Self::new(callback);
        let (sender, receiver) = oneshot::channel();
        executor.runtime.spawn(async move {
            let result = ports.open_owned(context, request, permit).await;
            // If the receiver was dropped either before or after send, the
            // returned session's Drop still releases the unclaimed lease.
            let _ = sender.send(result);
        });
        receiver.await.map_err(|_| {
            MediaError::new(
                MediaErrorKind::Unavailable,
                "native media open task ended without a response",
            )
        })?
    }
}

struct CallbackMediaDeviceSession {
    callback: Arc<dyn NativeMediaDeviceCallback>,
    lease_id: String,
    context: MediaOperationContext,
    audio_source: Option<MediaSourceRef>,
    video_source: Option<MediaSourceRef>,
    stopped: AtomicBool,
    stop_gate: tokio::sync::Mutex<()>,
    permit: StdMutex<Option<OwnedSemaphorePermit>>,
}

impl Drop for CallbackMediaDeviceSession {
    fn drop(&mut self) {
        if !self.stopped.load(Ordering::Acquire) {
            if let Some(permit) = self.permit.get_mut().unwrap().take() {
                schedule_native_media_stop(self.callback.clone(), self.lease_id.clone(), permit);
            } else {
                tracing::error!("native media lease lost its cleanup reservation");
            }
        }
    }
}

impl CallbackMediaDeviceSession {
    async fn call(
        &self,
        request: dto::MediaDeviceRequestDto,
    ) -> Result<dto::MediaDeviceResultDto, MediaError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(MediaError::new(
                MediaErrorKind::NotFound,
                "native media lease has ended",
            ));
        }
        let envelope = dto::MediaDeviceRequestEnvelopeDto {
            request_id: uuid::Uuid::new_v4().to_string(),
            request,
        };
        let submitted_audio_frames = match &envelope.request {
            dto::MediaDeviceRequestDto::WriteAudio { request } => {
                let frame_bytes = request.frame.pcm16le.len();
                let bytes_per_sample_frame = usize::from(request.frame.channels).saturating_mul(2);
                (bytes_per_sample_frame > 0)
                    .then(|| frame_bytes / bytes_per_sample_frame)
                    .and_then(|frames| u32::try_from(frames).ok())
            }
            _ => None,
        };
        let is_audio_barrier = matches!(
            &envelope.request,
            dto::MediaDeviceRequestDto::FlushAudio { .. }
                | dto::MediaDeviceRequestDto::InterruptAudio { .. }
        );
        envelope.validate().map_err(invalid_callback)?;
        let request_id = envelope.request_id.clone();
        let response = self.callback.request(envelope).await;
        response.validate().map_err(invalid_callback)?;
        if response.request_id != request_id {
            return Err(MediaError::new(
                MediaErrorKind::NativeFailure,
                "native media callback returned a mismatched request identity",
            ));
        }
        if let dto::MediaDeviceResultDto::Input { frame: Some(frame) } = &response.result {
            frame.validate().map_err(invalid_callback)?;
            if dto_frame_payload_bytes(frame) as u64 > self.context.max_payload_bytes {
                return Err(MediaError::new(
                    MediaErrorKind::MediaTooLarge,
                    "native media callback returned a frame over the lease payload limit",
                ));
            }
            let expected = match frame.payload {
                dto::MediaFramePayloadDto::Audio(_) => self.audio_source.as_ref(),
                dto::MediaFramePayloadDto::Video(_) => self.video_source.as_ref(),
            };
            if !expected.is_some_and(|expected| expected.0 .0 == frame.source.id) {
                return Err(MediaError::new(
                    MediaErrorKind::NativeFailure,
                    "native media callback returned a frame from an unselected source",
                ));
            }
        }
        match &response.result {
            dto::MediaDeviceResultDto::AudioWritten { receipt } => {
                let Some(submitted_frames) = submitted_audio_frames else {
                    return Err(MediaError::new(
                        MediaErrorKind::NativeFailure,
                        "native media callback acknowledged audio that was not submitted",
                    ));
                };
                if receipt.accepted_sample_frames > submitted_frames
                    || receipt.played_sample_frames > receipt.accepted_sample_frames
                {
                    return Err(MediaError::new(
                        MediaErrorKind::NativeFailure,
                        "native media playback receipt exceeds its submitted frame",
                    ));
                }
            }
            dto::MediaDeviceResultDto::AudioFlushed { receipt }
            | dto::MediaDeviceResultDto::AudioInterrupted { receipt }
                if !is_audio_barrier || receipt.accepted_sample_frames != 0 =>
            {
                return Err(MediaError::new(
                    MediaErrorKind::NativeFailure,
                    "native media barrier receipt does not match the pending control",
                ));
            }
            _ => {}
        }
        match response.result {
            dto::MediaDeviceResultDto::Error { error } => Err(error_from_dto(error)),
            result => Ok(result),
        }
    }
}

#[async_trait]
impl MediaDeviceSession for CallbackMediaDeviceSession {
    async fn next_input(&self) -> Result<Option<MediaFrame>, MediaError> {
        match self
            .call(dto::MediaDeviceRequestDto::NextInput {
                lease_id: self.lease_id.clone(),
            })
            .await?
        {
            dto::MediaDeviceResultDto::Input { frame } => frame.map(frame_from_dto).transpose(),
            result => Err(unexpected_result("next_input", &result)),
        }
    }

    async fn write_audio(&self, frame: MediaPcmFrame) -> Result<MediaRenderReceipt, MediaError> {
        frame.validate(self.context.max_payload_bytes)?;
        match self
            .call(dto::MediaDeviceRequestDto::WriteAudio {
                request: dto::MediaDeviceAudioWriteRequestDto {
                    lease_id: self.lease_id.clone(),
                    frame: pcm_to_dto(frame),
                },
            })
            .await?
        {
            dto::MediaDeviceResultDto::AudioWritten { receipt } => Ok(MediaRenderReceipt {
                accepted_sample_frames: receipt.accepted_sample_frames,
                played_sample_frames: receipt.played_sample_frames,
                rendered_at_unix_ms: receipt.rendered_at_unix_ms,
            }),
            result => Err(unexpected_result("write_audio", &result)),
        }
    }

    async fn write_video(&self, frame: MediaVideoFrame) -> Result<(), MediaError> {
        frame.validate(self.context.max_payload_bytes)?;
        match self
            .call(dto::MediaDeviceRequestDto::WriteVideo {
                request: dto::MediaDeviceVideoWriteRequestDto {
                    lease_id: self.lease_id.clone(),
                    frame: video_to_dto(frame),
                },
            })
            .await?
        {
            dto::MediaDeviceResultDto::VideoWritten => Ok(()),
            result => Err(unexpected_result("write_video", &result)),
        }
    }

    async fn flush_audio(&self) -> Result<MediaRenderReceipt, MediaError> {
        match self
            .call(dto::MediaDeviceRequestDto::FlushAudio {
                request: dto::MediaDeviceAudioControlRequestDto {
                    lease_id: self.lease_id.clone(),
                },
            })
            .await?
        {
            dto::MediaDeviceResultDto::AudioFlushed { receipt } => Ok(MediaRenderReceipt {
                accepted_sample_frames: receipt.accepted_sample_frames,
                played_sample_frames: receipt.played_sample_frames,
                rendered_at_unix_ms: receipt.rendered_at_unix_ms,
            }),
            result => Err(unexpected_result("flush_audio", &result)),
        }
    }

    async fn interrupt_audio(&self) -> Result<MediaRenderReceipt, MediaError> {
        match self
            .call(dto::MediaDeviceRequestDto::InterruptAudio {
                request: dto::MediaDeviceAudioControlRequestDto {
                    lease_id: self.lease_id.clone(),
                },
            })
            .await?
        {
            dto::MediaDeviceResultDto::AudioInterrupted { receipt } => Ok(MediaRenderReceipt {
                accepted_sample_frames: receipt.accepted_sample_frames,
                played_sample_frames: receipt.played_sample_frames,
                rendered_at_unix_ms: receipt.rendered_at_unix_ms,
            }),
            result => Err(unexpected_result("interrupt_audio", &result)),
        }
    }

    async fn update(&self, update: MediaSessionUpdate) -> Result<MediaSessionStatus, MediaError> {
        match self
            .call(dto::MediaDeviceRequestDto::Update {
                request: dto::MediaDeviceUpdateRequestDto {
                    lease_id: self.lease_id.clone(),
                    update: update_to_dto(update),
                },
            })
            .await?
        {
            dto::MediaDeviceResultDto::Updated { status } => Ok(status_from_dto(status)),
            result => Err(unexpected_result("update", &result)),
        }
    }

    async fn stop(&self) -> Result<(), MediaError> {
        let _stop_guard = self.stop_gate.lock().await;
        if self.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        match self
            .call(dto::MediaDeviceRequestDto::Stop {
                lease_id: self.lease_id.clone(),
            })
            .await
        {
            Ok(dto::MediaDeviceResultDto::Stopped)
            | Err(MediaError {
                kind: MediaErrorKind::NotFound,
                ..
            }) => {
                self.stopped.store(true, Ordering::Release);
                drop(self.permit.lock().unwrap().take());
                Ok(())
            }
            Ok(result) => Err(unexpected_result("stop", &result)),
            Err(error) => Err(error),
        }
    }
}

fn operation_id_to_dto(value: MediaOperationId) -> dto::MediaOperationIdDto {
    dto::MediaOperationIdDto {
        id: value.id,
        generation: value.generation,
        service_epoch: value.service_epoch,
    }
}

fn owner_to_dto(value: MediaOwner) -> dto::MediaOwnerDto {
    match value {
        MediaOwner::Session { session_id } => dto::MediaOwnerDto::Session { session_id },
        MediaOwner::Ui { instance_id } => dto::MediaOwnerDto::Ui { instance_id },
        MediaOwner::System { instance_id } => dto::MediaOwnerDto::System { instance_id },
    }
}

fn initiator_to_dto(value: MediaInitiator) -> dto::MediaInitiatorDto {
    dto::MediaInitiatorDto {
        agent_id: value.agent_id,
        tool_use_id: value.tool_use_id,
        request_id: value.request_id,
    }
}

fn session_request_to_dto(value: &MediaSessionRequest) -> dto::MediaSessionRequestDto {
    dto::MediaSessionRequestDto {
        mode: match value.mode {
            MediaSessionMode::Audio => dto::MediaSessionModeDto::Audio,
            MediaSessionMode::Video => dto::MediaSessionModeDto::Video,
            MediaSessionMode::AudioVideo => dto::MediaSessionModeDto::AudioVideo,
        },
        intent: match value.intent {
            MediaSessionIntent::Observation => dto::MediaSessionIntentDto::Observation,
            MediaSessionIntent::Conversation => dto::MediaSessionIntentDto::Conversation,
        },
        audio_source: value.audio_source.as_ref().map(source_ref_to_dto),
        video_source: value.video_source.as_ref().map(source_ref_to_dto),
    }
}

fn source_ref_to_dto(value: &MediaSourceRef) -> dto::MediaSourceRefDto {
    dto::MediaSourceRefDto {
        id: value.0 .0.clone(),
    }
}

fn source_ref_from_dto(value: dto::MediaSourceRefDto) -> MediaSourceRef {
    MediaSourceRef(MediaSourceId(value.id))
}

fn pcm_to_dto(value: MediaPcmFrame) -> dto::MediaPcmFrameDto {
    dto::MediaPcmFrameDto {
        sample_rate_hz: value.sample_rate_hz,
        channels: value.channels,
        pcm16le: value.pcm16le,
    }
}

fn pcm_from_dto(value: dto::MediaPcmFrameDto) -> MediaPcmFrame {
    MediaPcmFrame {
        sample_rate_hz: value.sample_rate_hz,
        channels: value.channels,
        pcm16le: value.pcm16le,
    }
}

fn video_to_dto(value: MediaVideoFrame) -> dto::MediaVideoFrameDto {
    match value {
        MediaVideoFrame::Rgba8 {
            width_px,
            height_px,
            stride_bytes,
            bytes,
        } => dto::MediaVideoFrameDto::Rgba8 {
            width_px,
            height_px,
            stride_bytes,
            bytes,
        },
        MediaVideoFrame::I420 {
            width_px,
            height_px,
            y_stride_bytes,
            u_stride_bytes,
            v_stride_bytes,
            y,
            u,
            v,
        } => dto::MediaVideoFrameDto::I420 {
            width_px,
            height_px,
            y_stride_bytes,
            u_stride_bytes,
            v_stride_bytes,
            y,
            u,
            v,
        },
    }
}

fn video_from_dto(value: dto::MediaVideoFrameDto) -> MediaVideoFrame {
    match value {
        dto::MediaVideoFrameDto::Rgba8 {
            width_px,
            height_px,
            stride_bytes,
            bytes,
        } => MediaVideoFrame::Rgba8 {
            width_px,
            height_px,
            stride_bytes,
            bytes,
        },
        dto::MediaVideoFrameDto::I420 {
            width_px,
            height_px,
            y_stride_bytes,
            u_stride_bytes,
            v_stride_bytes,
            y,
            u,
            v,
        } => MediaVideoFrame::I420 {
            width_px,
            height_px,
            y_stride_bytes,
            u_stride_bytes,
            v_stride_bytes,
            y,
            u,
            v,
        },
    }
}

fn frame_from_dto(value: dto::MediaFrameDto) -> Result<MediaFrame, MediaError> {
    let payload = match value.payload {
        dto::MediaFramePayloadDto::Audio(frame) => MediaFramePayload::Audio(pcm_from_dto(frame)),
        dto::MediaFramePayloadDto::Video(frame) => MediaFramePayload::Video(video_from_dto(frame)),
    };
    Ok(MediaFrame {
        source: source_ref_from_dto(value.source),
        sequence: value.sequence,
        captured_at_unix_ms: value.captured_at_unix_ms,
        payload,
    })
}

fn dto_frame_payload_bytes(frame: &dto::MediaFrameDto) -> usize {
    match &frame.payload {
        dto::MediaFramePayloadDto::Audio(frame) => frame.pcm16le.len(),
        dto::MediaFramePayloadDto::Video(dto::MediaVideoFrameDto::Rgba8 { bytes, .. }) => {
            bytes.len()
        }
        dto::MediaFramePayloadDto::Video(dto::MediaVideoFrameDto::I420 { y, u, v, .. }) => {
            y.len().saturating_add(u.len()).saturating_add(v.len())
        }
    }
}

fn update_to_dto(value: MediaSessionUpdate) -> dto::MediaSessionUpdateDto {
    match value {
        MediaSessionUpdate::SetEnabled { kind, enabled } => {
            dto::MediaSessionUpdateDto::SetEnabled {
                kind: media_kind_to_dto(kind),
                enabled,
            }
        }
        MediaSessionUpdate::SelectSource { kind, source } => {
            dto::MediaSessionUpdateDto::SelectSource {
                kind: media_kind_to_dto(kind),
                source: source_ref_to_dto(&source),
            }
        }
        MediaSessionUpdate::SetPlaybackGain { gain } => {
            dto::MediaSessionUpdateDto::SetPlaybackGain { gain }
        }
    }
}

fn media_kind_to_dto(value: MediaKind) -> dto::MediaKindDto {
    match value {
        MediaKind::Audio => dto::MediaKindDto::Audio,
        MediaKind::Image => dto::MediaKindDto::Image,
        MediaKind::Video => dto::MediaKindDto::Video,
    }
}

fn status_from_dto(value: dto::MediaSessionStatusDto) -> MediaSessionStatus {
    MediaSessionStatus {
        state: match value.state {
            dto::MediaSessionStateDto::Preparing => MediaSessionState::Preparing,
            dto::MediaSessionStateDto::Ready => MediaSessionState::Ready,
            dto::MediaSessionStateDto::Reconnecting => MediaSessionState::Reconnecting,
            dto::MediaSessionStateDto::Ended => MediaSessionState::Ended,
            dto::MediaSessionStateDto::Failed => MediaSessionState::Failed,
        },
        audio_enabled: value.audio_enabled,
        video_enabled: value.video_enabled,
    }
}

fn error_from_dto(value: dto::MediaErrorDto) -> MediaError {
    let kind = match value.kind {
        dto::MediaErrorKindDto::PermissionDenied => MediaErrorKind::PermissionDenied,
        dto::MediaErrorKindDto::Busy => MediaErrorKind::Busy,
        dto::MediaErrorKindDto::Cancelled => MediaErrorKind::Cancelled,
        dto::MediaErrorKindDto::Timeout => MediaErrorKind::Timeout,
        dto::MediaErrorKindDto::NoData => MediaErrorKind::NoData,
        dto::MediaErrorKindDto::NotFound => MediaErrorKind::NotFound,
        dto::MediaErrorKindDto::Unavailable => MediaErrorKind::Unavailable,
        dto::MediaErrorKindDto::Unsupported => MediaErrorKind::Unsupported,
        dto::MediaErrorKindDto::ModelMissing => MediaErrorKind::ModelMissing,
        dto::MediaErrorKindDto::InvalidRequest => MediaErrorKind::InvalidRequest,
        dto::MediaErrorKindDto::ProcessingFailed => MediaErrorKind::ProcessingFailed,
        dto::MediaErrorKindDto::NativeFailure => MediaErrorKind::NativeFailure,
        dto::MediaErrorKindDto::StoreFailure => MediaErrorKind::StoreFailure,
        dto::MediaErrorKindDto::MediaTooLarge => MediaErrorKind::MediaTooLarge,
    };
    MediaError::new(kind, value.message)
}

fn invalid_callback(message: String) -> MediaError {
    MediaError::new(MediaErrorKind::NativeFailure, message)
}

fn unexpected_result(operation: &str, result: &dto::MediaDeviceResultDto) -> MediaError {
    MediaError::new(
        MediaErrorKind::NativeFailure,
        format!("native media callback returned an invalid {operation} result: {result:?}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use tokio::sync::Notify;

    struct OpeningCallback {
        opened: Notify,
        release: Semaphore,
        stopped: mpsc::Sender<String>,
        fail_open: AtomicBool,
        fail_stops: AtomicUsize,
        stop_not_found: AtomicBool,
        hold_failed_stop: AtomicBool,
        release_failed_stop: Semaphore,
    }

    impl OpeningCallback {
        fn new() -> (Arc<Self>, mpsc::Receiver<String>) {
            let (stopped, receiver) = mpsc::channel();
            (
                Arc::new(Self {
                    opened: Notify::new(),
                    release: Semaphore::new(0),
                    stopped,
                    fail_open: AtomicBool::new(false),
                    fail_stops: AtomicUsize::new(0),
                    stop_not_found: AtomicBool::new(false),
                    hold_failed_stop: AtomicBool::new(false),
                    release_failed_stop: Semaphore::new(0),
                }),
                receiver,
            )
        }
    }

    #[async_trait]
    impl NativeMediaDeviceCallback for OpeningCallback {
        async fn request(
            &self,
            envelope: dto::MediaDeviceRequestEnvelopeDto,
        ) -> dto::MediaDeviceResponseDto {
            let result = match envelope.request {
                dto::MediaDeviceRequestDto::Open { .. } => {
                    if self.fail_open.load(Ordering::Acquire) {
                        return dto::MediaDeviceResponseDto {
                            request_id: envelope.request_id,
                            result: dto::MediaDeviceResultDto::Error {
                                error: dto::MediaErrorDto {
                                    kind: dto::MediaErrorKindDto::NativeFailure,
                                    message: "open failed".into(),
                                },
                            },
                        };
                    }
                    // The native lease exists before the response is released.
                    self.opened.notify_one();
                    self.release.acquire().await.unwrap().forget();
                    dto::MediaDeviceResultDto::Opened {
                        lease: dto::MediaDeviceLeaseDto {
                            lease_id: "test-native-lease".into(),
                        },
                    }
                }
                dto::MediaDeviceRequestDto::Stop { lease_id } => {
                    self.stopped.send(lease_id).unwrap();
                    if self
                        .fail_stops
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                            remaining.checked_sub(1)
                        })
                        .is_ok()
                    {
                        if self.hold_failed_stop.load(Ordering::Acquire) {
                            self.release_failed_stop.acquire().await.unwrap().forget();
                        }
                        dto::MediaDeviceResultDto::Error {
                            error: dto::MediaErrorDto {
                                kind: dto::MediaErrorKindDto::NativeFailure,
                                message: "stop failed".into(),
                            },
                        }
                    } else if self.stop_not_found.load(Ordering::Acquire) {
                        dto::MediaDeviceResultDto::Error {
                            error: dto::MediaErrorDto {
                                kind: dto::MediaErrorKindDto::NotFound,
                                message: "lease is already absent".into(),
                            },
                        }
                    } else {
                        dto::MediaDeviceResultDto::Stopped
                    }
                }
                other => panic!("unexpected callback: {other:?}"),
            };
            dto::MediaDeviceResponseDto {
                request_id: envelope.request_id,
                result,
            }
        }
    }

    struct MismatchedStopCallback {
        inner: Arc<OpeningCallback>,
        mismatches: AtomicUsize,
    }

    #[async_trait]
    impl NativeMediaDeviceCallback for MismatchedStopCallback {
        async fn request(
            &self,
            envelope: dto::MediaDeviceRequestEnvelopeDto,
        ) -> dto::MediaDeviceResponseDto {
            let is_stop = matches!(&envelope.request, dto::MediaDeviceRequestDto::Stop { .. });
            let mut response = self.inner.request(envelope).await;
            if is_stop
                && self
                    .mismatches
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok()
            {
                response.request_id = "wrong-request".into();
            }
            response
        }
    }

    fn open_args() -> (MediaOperationContext, MediaSessionRequest) {
        (
            MediaOperationContext::new(
                MediaOperationId::new(1, 1),
                MediaOwner::Session {
                    session_id: "session".into(),
                },
            ),
            MediaSessionRequest {
                intent: MediaSessionIntent::Observation,
                mode: MediaSessionMode::Audio,
                audio_source: Some(MediaSourceRef(MediaSourceId::from(
                    "media-runtime:microphone",
                ))),
                video_source: None,
            },
        )
    }

    fn assert_stopped_once(stopped: &mpsc::Receiver<String>) {
        assert_eq!(
            stopped.recv_timeout(Duration::from_secs(2)).unwrap(),
            "test-native-lease"
        );
        assert!(stopped.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[tokio::test]
    async fn cancelled_open_stops_the_late_native_lease() {
        let (callback, stopped) = OpeningCallback::new();
        let ports = Arc::new(CallbackMediaDevicePorts::new(callback.clone()));
        let (context, request) = open_args();
        let open = tokio::spawn(async move { ports.open(context, request).await });
        callback.opened.notified().await;
        open.abort();
        assert!(matches!(open.await, Err(error) if error.is_cancelled()));
        callback.release.add_permits(1);
        assert_stopped_once(&stopped);
    }

    #[tokio::test]
    async fn dropping_a_delivered_but_unpolled_lease_stops_it() {
        let (callback, stopped) = OpeningCallback::new();
        callback.release.add_permits(1);
        let ports = CallbackMediaDevicePorts::new(callback);
        let (context, request) = open_args();
        let session = ports.open(context, request).await.unwrap();
        let (sender, receiver) = oneshot::channel();
        assert!(sender.send(session).is_ok());
        drop(receiver);
        assert_stopped_once(&stopped);
    }

    #[tokio::test]
    async fn successful_open_remains_live_until_explicit_stop() {
        let (callback, stopped) = OpeningCallback::new();
        callback.release.add_permits(1);
        let ports = CallbackMediaDevicePorts::new(callback);
        let (context, request) = open_args();
        let session = ports.open(context, request).await.unwrap();
        assert!(stopped.try_recv().is_err());
        session.stop().await.unwrap();
        drop(session);
        assert_stopped_once(&stopped);
    }

    #[tokio::test]
    async fn failed_open_releases_its_capacity() {
        let (callback, _) = OpeningCallback::new();
        callback.fail_open.store(true, Ordering::Release);
        let ports = CallbackMediaDevicePorts::new(callback);
        let capacity = Arc::new(Semaphore::new(1));
        let permit = capacity.clone().try_acquire_owned().unwrap();
        let (context, request) = open_args();
        assert!(ports.open_owned(context, request, permit).await.is_err());
        assert_eq!(capacity.available_permits(), 1);
    }

    #[tokio::test]
    async fn failed_explicit_stop_retains_capacity_until_a_successful_retry() {
        let (callback, stopped) = OpeningCallback::new();
        callback.release.add_permits(1);
        callback.fail_stops.store(1, Ordering::Release);
        let ports = CallbackMediaDevicePorts::new(callback);
        let capacity = Arc::new(Semaphore::new(1));
        let permit = capacity.clone().try_acquire_owned().unwrap();
        let (context, request) = open_args();
        let session = ports.open_owned(context, request, permit).await.unwrap();
        assert!(session.stop().await.is_err());
        assert_eq!(capacity.available_permits(), 0);
        session.stop().await.unwrap();
        assert_eq!(capacity.available_permits(), 1);
        drop(session);
        assert_eq!(
            stopped.recv_timeout(Duration::from_secs(2)).unwrap(),
            "test-native-lease"
        );
        assert_stopped_once(&stopped);
    }

    #[tokio::test]
    async fn explicit_stop_accepts_an_already_absent_lease() {
        let (callback, stopped) = OpeningCallback::new();
        callback.release.add_permits(1);
        callback.stop_not_found.store(true, Ordering::Release);
        let ports = CallbackMediaDevicePorts::new(callback);
        let capacity = Arc::new(Semaphore::new(1));
        let permit = capacity.clone().try_acquire_owned().unwrap();
        let (context, request) = open_args();
        let session = ports.open_owned(context, request, permit).await.unwrap();
        session.stop().await.unwrap();
        assert_eq!(capacity.available_permits(), 1);
        drop(session);
        assert_stopped_once(&stopped);
    }

    #[tokio::test]
    async fn abandoned_lease_retries_stop_before_releasing_capacity() {
        let (callback, stopped) = OpeningCallback::new();
        callback.release.add_permits(1);
        callback.fail_stops.store(1, Ordering::Release);
        callback.hold_failed_stop.store(true, Ordering::Release);
        let ports = CallbackMediaDevicePorts::new(callback.clone());
        let capacity = Arc::new(Semaphore::new(1));
        let permit = capacity.clone().try_acquire_owned().unwrap();
        let (context, request) = open_args();
        let session = ports.open_owned(context, request, permit).await.unwrap();
        drop(session);
        assert_eq!(
            stopped.recv_timeout(Duration::from_secs(2)).unwrap(),
            "test-native-lease"
        );
        assert_eq!(capacity.available_permits(), 0);
        callback.release_failed_stop.add_permits(1);
        assert_eq!(
            stopped.recv_timeout(Duration::from_secs(2)).unwrap(),
            "test-native-lease"
        );
        let permit = tokio::time::timeout(Duration::from_secs(2), capacity.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(permit);
        assert!(stopped.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[tokio::test]
    async fn abandoned_lease_requires_a_correlated_stop_acknowledgement() {
        let (inner, stopped) = OpeningCallback::new();
        inner.release.add_permits(1);
        let callback = Arc::new(MismatchedStopCallback {
            inner,
            mismatches: AtomicUsize::new(1),
        });
        let ports = CallbackMediaDevicePorts::new(callback);
        let capacity = Arc::new(Semaphore::new(1));
        let permit = capacity.clone().try_acquire_owned().unwrap();
        let (context, request) = open_args();
        let session = ports.open_owned(context, request, permit).await.unwrap();
        drop(session);
        assert_eq!(
            stopped.recv_timeout(Duration::from_secs(2)).unwrap(),
            "test-native-lease"
        );
        assert_eq!(
            stopped.recv_timeout(Duration::from_secs(2)).unwrap(),
            "test-native-lease"
        );
        let permit = tokio::time::timeout(Duration::from_secs(2), capacity.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(permit);
        assert!(stopped.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[tokio::test]
    async fn abandoned_lease_accepts_a_correlated_already_absent_response() {
        let (callback, stopped) = OpeningCallback::new();
        callback.release.add_permits(1);
        callback.stop_not_found.store(true, Ordering::Release);
        let ports = CallbackMediaDevicePorts::new(callback);
        let capacity = Arc::new(Semaphore::new(1));
        let permit = capacity.clone().try_acquire_owned().unwrap();
        let (context, request) = open_args();
        let session = ports.open_owned(context, request, permit).await.unwrap();
        drop(session);
        assert_eq!(
            stopped.recv_timeout(Duration::from_secs(2)).unwrap(),
            "test-native-lease"
        );
        let permit = tokio::time::timeout(Duration::from_secs(2), capacity.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(permit);
        assert!(stopped.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[tokio::test]
    async fn repeatedly_failed_cleanup_quarantines_capacity() {
        let (callback, stopped) = OpeningCallback::new();
        callback.release.add_permits(1);
        callback
            .fail_stops
            .store(NATIVE_MEDIA_STOP_ATTEMPTS, Ordering::Release);
        let ports = CallbackMediaDevicePorts::new(callback);
        let capacity = Arc::new(Semaphore::new(1));
        let permit = capacity.clone().try_acquire_owned().unwrap();
        let (context, request) = open_args();
        let session = ports.open_owned(context, request, permit).await.unwrap();
        drop(session);
        for _ in 0..NATIVE_MEDIA_STOP_ATTEMPTS {
            assert_eq!(
                stopped.recv_timeout(Duration::from_secs(2)).unwrap(),
                "test-native-lease"
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(250), capacity.acquire())
                .await
                .is_err()
        );
        assert!(stopped.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn abandoned_open_cleanup_survives_its_host_runtime() {
        let (callback, stopped) = OpeningCallback::new();
        let ports = Arc::new(CallbackMediaDevicePorts::new(callback.clone()));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (context, request) = open_args();
            let open = tokio::spawn(async move { ports.open(context, request).await });
            callback.opened.notified().await;
            open.abort();
            assert!(matches!(open.await, Err(error) if error.is_cancelled()));
        });
        drop(runtime);
        callback.release.add_permits(1);
        assert_stopped_once(&stopped);
    }
}
