//! Real local transport and media-provider loopback integration tests.
#![cfg(feature = "native-media")]

use async_trait::async_trait;
use client::protocol::media::{
    MediaConnectionRequestDto, MediaConnectionResponseDto, MediaControlActionDto,
    MediaControlRequestDto, MediaControlResponseDto, MediaRenderReceiptDto,
    MediaSessionDescriptionDto, MediaSessionDescriptionTypeDto, MediaSessionIntentDto,
    MediaSessionModeDto, MediaSourceKindDto,
};
use media_api::{
    MediaCapabilities, MediaConnector, MediaDevicePorts, MediaDeviceSession, MediaError,
    MediaErrorKind, MediaEventKind, MediaFrame, MediaFramePayload, MediaKind, MediaOperation,
    MediaOperationContext, MediaOperationId, MediaOperationKind, MediaOperationReadiness,
    MediaOwner, MediaPcmFrame, MediaReadinessState, MediaRef, MediaRenderReceipt, MediaResourceRef,
    MediaResourceStore, MediaResult, MediaSession, MediaSessionIntent, MediaSessionMode,
    MediaSessionRequest, MediaSessionState, MediaSessionStatus, MediaSessionUpdate, MediaSource,
    MediaSourceKind, MediaSourceRef, MediaSourceUsage, MediaVideoFrame,
};
use remote_control_client::host_media::{
    RemoteMediaConnector, RemoteMediaHostConfigProvider, RemoteMediaHostControlChannel,
};
use remote_control_client::media::{
    remote_media_source_ref, RemoteIceServer, RemoteMediaConfig, RemoteMediaConfigProvider,
    RemoteMediaConnection, RemoteMediaControlAction, RemoteMediaControlChannel,
    RemoteMediaControlRequest, RemoteMediaIntent, RemoteMediaMode, RemoteMediaOffer,
    RemoteMediaOfferHandler, RemoteMediaSourceKind,
};
use remote_control_client::native_media::NativeMediaOfferHandler;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, Mutex as AsyncMutex, Notify};
use tokio::time::{sleep, timeout};

const ROUTE_ID: &str = "1e5d7a65-b37c-4e51-9ac8-33d2c2a98a61";
const SESSION_ID: &str = "12121212-3434-4567-8899-aaaaaaaaaaaa";
const LOCAL_MICROPHONE: &str = "media-runtime:microphone";
const LOCAL_CAMERA: &str = "media-runtime:camera";

fn media_config() -> RemoteMediaConfig {
    RemoteMediaConfig {
        ice_servers: Vec::<RemoteIceServer>::new(),
        relay_only: false,
        gather_timeout_ms: 1_000,
        connect_timeout_ms: 8_000,
        max_payload_bytes: 1024 * 1024,
    }
}

struct StaticConfig;

#[async_trait]
impl RemoteMediaHostConfigProvider for StaticConfig {
    async fn resolve(
        &self,
        _context: &MediaOperationContext,
        _route_id: &str,
        _request: &MediaSessionRequest,
    ) -> Result<RemoteMediaConfig, MediaError> {
        Ok(media_config())
    }
}

#[async_trait]
impl RemoteMediaConfigProvider for StaticConfig {
    async fn resolve(
        &self,
        _connection: &RemoteMediaConnection,
        _offer: &RemoteMediaOffer,
    ) -> Result<RemoteMediaConfig, remote_control_client::rpc::RpcError> {
        Ok(media_config())
    }
}

struct LoopbackChannel {
    handler: Arc<NativeMediaOfferHandler>,
    connection: RemoteMediaConnection,
    last_played_sample_frames: Mutex<u64>,
}

impl LoopbackChannel {
    fn from_rpc(error: remote_control_client::rpc::RpcError) -> MediaError {
        let kind = match error.code {
            remote_control_client::rpc::RemoteErrorCode::Capacity => MediaErrorKind::Busy,
            remote_control_client::rpc::RemoteErrorCode::InvalidParams => {
                MediaErrorKind::InvalidRequest
            }
            remote_control_client::rpc::RemoteErrorCode::UnknownSession => MediaErrorKind::NotFound,
            remote_control_client::rpc::RemoteErrorCode::Forbidden
            | remote_control_client::rpc::RemoteErrorCode::NotApproved => {
                MediaErrorKind::PermissionDenied
            }
            remote_control_client::rpc::RemoteErrorCode::Timeout => MediaErrorKind::Timeout,
            _ => MediaErrorKind::NativeFailure,
        };
        MediaError::new(kind, error.message)
    }
}

#[async_trait]
impl RemoteMediaHostControlChannel for LoopbackChannel {
    async fn negotiate(
        &self,
        request: MediaConnectionRequestDto,
    ) -> Result<MediaConnectionResponseDto, MediaError> {
        request
            .validate()
            .map_err(|_| MediaError::new(MediaErrorKind::InvalidRequest, "invalid test offer"))?;
        let offer = RemoteMediaOffer {
            v: 1,
            media_session_ref: request.media_session_ref.id.clone(),
            session_id: request.session_id.clone(),
            generation: request.operation_id.generation,
            mode: match request.mode {
                MediaSessionModeDto::Audio => RemoteMediaMode::Audio,
                MediaSessionModeDto::Video => RemoteMediaMode::Video,
                MediaSessionModeDto::AudioVideo => RemoteMediaMode::AudioVideo,
            },
            intent: match request.intent {
                MediaSessionIntentDto::Observation => RemoteMediaIntent::Observation,
                MediaSessionIntentDto::Conversation => RemoteMediaIntent::Conversation,
            },
            audio_source_ref: request
                .audio_source
                .as_ref()
                .map(|source| source.id.clone()),
            video_source_ref: request
                .video_source
                .as_ref()
                .map(|source| source.id.clone()),
            offer_sdp: request.description.sdp.clone(),
        };
        let answer = self
            .handler
            .accept_offer(&self.connection, offer, Arc::new(NoopPlaybackReceipt))
            .await
            .map_err(Self::from_rpc)?;
        let response = MediaConnectionResponseDto {
            request_id: request.request_id.clone(),
            connection_id: request.connection_id.clone(),
            media_session_ref: request.media_session_ref.clone(),
            description: Some(MediaSessionDescriptionDto {
                kind: MediaSessionDescriptionTypeDto::Answer,
                sdp: answer.answer_sdp,
            }),
            error: None,
        };
        response
            .validate()
            .map_err(|_| MediaError::new(MediaErrorKind::InvalidRequest, "invalid test answer"))?;
        if !response.matches_request(&request) {
            return Err(MediaError::new(
                MediaErrorKind::InvalidRequest,
                "test answer correlation mismatch",
            ));
        }
        Ok(response)
    }

    async fn control(
        &self,
        request: MediaControlRequestDto,
    ) -> Result<MediaControlResponseDto, MediaError> {
        let (action, target_audio_sequence, target_sample_frames) = match request.action {
            MediaControlActionDto::FlushAudio {
                target_sequence,
                target_sample_frames,
            } => (
                RemoteMediaControlAction::FlushAudio,
                target_sequence,
                target_sample_frames,
            ),
            MediaControlActionDto::InterruptAudio {
                target_sequence,
                target_sample_frames,
            } => (
                RemoteMediaControlAction::InterruptAudio,
                target_sequence,
                target_sample_frames,
            ),
            MediaControlActionDto::Close => (RemoteMediaControlAction::Close, 0, 0),
        };
        let result = self
            .handler
            .control(
                &self.connection,
                RemoteMediaControlRequest {
                    v: 1,
                    media_session_ref: request.media_session_ref.id.clone(),
                    generation: request.generation,
                    action,
                    target_audio_sequence,
                    target_sample_frames,
                },
            )
            .await
            .map_err(Self::from_rpc)?;
        let played = {
            let mut last = self
                .last_played_sample_frames
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let delta = result
                .played_sample_frames
                .checked_sub(*last)
                .ok_or_else(|| {
                    MediaError::new(
                        MediaErrorKind::InvalidRequest,
                        "test peer playback progress moved backwards",
                    )
                })?;
            *last = result.played_sample_frames;
            u32::try_from(delta).map_err(|_| {
                MediaError::new(
                    MediaErrorKind::MediaTooLarge,
                    "test peer playback delta exceeds its bound",
                )
            })?
        };
        let response = MediaControlResponseDto {
            request_id: request.request_id.clone(),
            connection_id: request.connection_id.clone(),
            media_session_ref: request.media_session_ref.clone(),
            generation: request.generation,
            receipt: Some(MediaRenderReceiptDto {
                accepted_sample_frames: 0,
                played_sample_frames: played,
                rendered_at_unix_ms: result.rendered_at_unix_ms,
            }),
            error: None,
        };
        response.validate().map_err(|_| {
            MediaError::new(
                MediaErrorKind::InvalidRequest,
                "invalid test control response",
            )
        })?;
        if !response.matches_request(&request) {
            return Err(MediaError::new(
                MediaErrorKind::InvalidRequest,
                "test control correlation mismatch",
            ));
        }
        Ok(response)
    }
}

struct NoopPlaybackReceipt;

#[async_trait]
impl RemoteMediaControlChannel for NoopPlaybackReceipt {
    async fn playback_receipt(
        &self,
        _receipt: remote_control_client::media::RemoteMediaPlaybackReceipt,
    ) -> Result<(), remote_control_client::rpc::RpcError> {
        Ok(())
    }
}

struct LoopbackMediaService {
    sources: Vec<MediaSource>,
    attached: Mutex<Vec<(MediaOwner, Arc<dyn MediaSession>)>>,
}

#[async_trait]
impl media_api::MediaService for LoopbackMediaService {
    fn capabilities(&self) -> MediaCapabilities {
        MediaCapabilities {
            service_epoch: 1,
            support_revision: 1,
            supported_operations: vec![MediaOperationKind::Sources, MediaOperationKind::Stop],
            supported_media_kinds: vec![MediaKind::Audio, MediaKind::Video],
            readiness: vec![MediaOperationReadiness {
                operation: MediaOperationKind::Sources,
                state: MediaReadinessState::Ready,
            }],
            max_payload_bytes: 1024 * 1024,
        }
    }

    async fn execute(
        &self,
        context: MediaOperationContext,
        operation: MediaOperation,
    ) -> Result<MediaResult, MediaError> {
        match operation {
            MediaOperation::Sources {
                kind: Some(kind),
                usage: Some(MediaSourceUsage::Session),
            } => Ok(MediaResult::Sources {
                sources: self
                    .sources
                    .iter()
                    .filter(|source| source.media_kind == Some(kind))
                    .cloned()
                    .collect(),
            }),
            MediaOperation::Stop {
                target: MediaResourceRef::Session(reference),
            } => {
                let session = self
                    .attached
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .iter()
                    .find(|(owner, session)| {
                        owner == &context.owner && session.reference() == &reference
                    })
                    .map(|(_, session)| session.clone())
                    .ok_or_else(|| {
                        MediaError::new(MediaErrorKind::NotFound, "loopback session missing")
                    })?;
                let status = session.stop(context.clone()).await?;
                if status.state != MediaSessionState::Ended {
                    return Err(MediaError::new(
                        MediaErrorKind::NativeFailure,
                        "loopback session did not reach Ended",
                    ));
                }
                self.attached
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .retain(|(owner, existing)| {
                        owner != &context.owner || !Arc::ptr_eq(existing, &session)
                    });
                Ok(MediaResult::Stopped {
                    resource: MediaResourceRef::Session(reference),
                })
            }
            _ => Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "loopback supports only microphone source discovery",
            )),
        }
    }

    async fn attach_session(
        &self,
        context: MediaOperationContext,
        session: Box<dyn MediaSession>,
    ) -> Result<media_api::MediaSessionRef, MediaError> {
        let reference = session.reference().clone();
        let context_owner = context.owner;
        let session: Arc<dyn MediaSession> = Arc::from(session);
        {
            let mut attached = self
                .attached
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if attached.iter().any(|(owner, existing)| {
                owner == &context_owner && existing.reference() == &reference
            }) {
                return Err(MediaError::new(
                    MediaErrorKind::Busy,
                    "loopback session reference already exists",
                ));
            }
            attached.push((context_owner, session.clone()));
        }
        session.on_registered();
        Ok(reference)
    }

    async fn read_media(
        &self,
        _context: MediaOperationContext,
        _media: MediaRef,
    ) -> Result<media_api::MediaContent, MediaError> {
        Err(MediaError::new(
            MediaErrorKind::NotFound,
            "unused loopback store",
        ))
    }

    async fn cancel(&self, _identity: MediaOperationId) -> Result<(), MediaError> {
        Ok(())
    }
}

struct LoopbackPorts {
    open_count: AtomicUsize,
    input_audio_frames: Arc<AtomicUsize>,
    input_video_frames: Arc<AtomicUsize>,
    render_attempts: Arc<AtomicUsize>,
    playback_control_attempts: Arc<AtomicUsize>,
    stop_count: Arc<AtomicUsize>,
    stop_attempts: Arc<AtomicUsize>,
    stop_failures_remaining: Arc<AtomicUsize>,
    open_started: Option<Arc<Notify>>,
    release_open: Option<Arc<Notify>>,
    input_sender: Mutex<Option<mpsc::Sender<MediaFrame>>>,
    rendered_sender: mpsc::UnboundedSender<MediaPcmFrame>,
    rendered_receiver: AsyncMutex<mpsc::UnboundedReceiver<MediaPcmFrame>>,
}

impl LoopbackPorts {
    fn new() -> Arc<Self> {
        let (rendered_sender, rendered_receiver) = mpsc::unbounded_channel();
        Arc::new(Self {
            open_count: AtomicUsize::new(0),
            input_audio_frames: Arc::new(AtomicUsize::new(0)),
            input_video_frames: Arc::new(AtomicUsize::new(0)),
            render_attempts: Arc::new(AtomicUsize::new(0)),
            playback_control_attempts: Arc::new(AtomicUsize::new(0)),
            stop_count: Arc::new(AtomicUsize::new(0)),
            stop_attempts: Arc::new(AtomicUsize::new(0)),
            stop_failures_remaining: Arc::new(AtomicUsize::new(0)),
            open_started: None,
            release_open: None,
            input_sender: Mutex::new(None),
            rendered_sender,
            rendered_receiver: AsyncMutex::new(rendered_receiver),
        })
    }

    async fn send_capture(&self, frame: MediaFrame) {
        let sender = self
            .input_sender
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .expect("native lease has been opened");
        sender
            .send(frame)
            .await
            .expect("loopback capture consumer is alive");
    }

    async fn next_rendered(&self) -> Option<MediaPcmFrame> {
        self.rendered_receiver.lock().await.recv().await
    }
}

struct LoopbackDeviceSession {
    input: AsyncMutex<mpsc::Receiver<MediaFrame>>,
    input_audio_frames: Arc<AtomicUsize>,
    input_video_frames: Arc<AtomicUsize>,
    rendered: mpsc::UnboundedSender<MediaPcmFrame>,
    render_attempts: Arc<AtomicUsize>,
    playback_control_attempts: Arc<AtomicUsize>,
    stop_count: Arc<AtomicUsize>,
    stop_attempts: Arc<AtomicUsize>,
    stop_failures_remaining: Arc<AtomicUsize>,
    mode: MediaSessionMode,
    intent: MediaSessionIntent,
}

#[async_trait]
impl MediaDeviceSession for LoopbackDeviceSession {
    async fn next_input(&self) -> Result<Option<MediaFrame>, MediaError> {
        let frame = self.input.lock().await.recv().await;
        if let Some(frame) = &frame {
            match &frame.payload {
                MediaFramePayload::Audio(_) => {
                    self.input_audio_frames.fetch_add(1, Ordering::AcqRel);
                }
                MediaFramePayload::Video(_) => {
                    self.input_video_frames.fetch_add(1, Ordering::AcqRel);
                }
            }
        }
        Ok(frame)
    }

    async fn write_audio(&self, frame: MediaPcmFrame) -> Result<MediaRenderReceipt, MediaError> {
        self.render_attempts.fetch_add(1, Ordering::AcqRel);
        if self.intent == MediaSessionIntent::Observation {
            return Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "observation native lease is capture-only",
            ));
        }
        let sample_frames = u32::try_from(frame.pcm16le.len() / (usize::from(frame.channels) * 2))
            .map_err(|_| {
                MediaError::new(MediaErrorKind::MediaTooLarge, "test audio is too large")
            })?;
        self.rendered.send(frame).map_err(|_| {
            MediaError::new(
                MediaErrorKind::Unavailable,
                "loopback render observer closed",
            )
        })?;
        Ok(MediaRenderReceipt {
            accepted_sample_frames: sample_frames,
            played_sample_frames: sample_frames,
            rendered_at_unix_ms: Some(unix_now_ms()),
        })
    }

    async fn flush_audio(&self) -> Result<MediaRenderReceipt, MediaError> {
        self.playback_control_attempts
            .fetch_add(1, Ordering::AcqRel);
        if self.intent == MediaSessionIntent::Observation {
            return Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "observation native lease has no playback barrier",
            ));
        }
        Ok(MediaRenderReceipt {
            accepted_sample_frames: 0,
            played_sample_frames: 0,
            rendered_at_unix_ms: Some(unix_now_ms()),
        })
    }

    async fn interrupt_audio(&self) -> Result<MediaRenderReceipt, MediaError> {
        self.playback_control_attempts
            .fetch_add(1, Ordering::AcqRel);
        if self.intent == MediaSessionIntent::Observation {
            return Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "observation native lease has no playback barrier",
            ));
        }
        Ok(MediaRenderReceipt {
            accepted_sample_frames: 0,
            played_sample_frames: 0,
            rendered_at_unix_ms: Some(unix_now_ms()),
        })
    }

    async fn write_video(&self, _frame: MediaVideoFrame) -> Result<(), MediaError> {
        Err(MediaError::new(
            MediaErrorKind::Unsupported,
            "audio loopback has no video renderer",
        ))
    }

    async fn update(&self, _update: MediaSessionUpdate) -> Result<MediaSessionStatus, MediaError> {
        if self.intent == MediaSessionIntent::Observation
            && matches!(_update, MediaSessionUpdate::SetPlaybackGain { .. })
        {
            self.playback_control_attempts
                .fetch_add(1, Ordering::AcqRel);
            return Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "observation native lease has no playback gain",
            ));
        }
        Ok(MediaSessionStatus {
            state: MediaSessionState::Ready,
            audio_enabled: matches!(
                self.mode,
                MediaSessionMode::Audio | MediaSessionMode::AudioVideo
            ),
            video_enabled: matches!(
                self.mode,
                MediaSessionMode::Video | MediaSessionMode::AudioVideo
            ),
        })
    }

    async fn stop(&self) -> Result<(), MediaError> {
        self.stop_attempts.fetch_add(1, Ordering::AcqRel);
        if self
            .stop_failures_remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                (remaining > 0).then_some(remaining.saturating_sub(1))
            })
            .is_ok()
        {
            return Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "injected transient native stop failure",
            ));
        }
        self.stop_count.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

#[async_trait]
impl MediaDevicePorts for LoopbackPorts {
    async fn open(
        &self,
        _context: MediaOperationContext,
        request: MediaSessionRequest,
    ) -> Result<Arc<dyn MediaDeviceSession>, MediaError> {
        let audio_valid = if matches!(
            request.mode,
            MediaSessionMode::Audio | MediaSessionMode::AudioVideo
        ) {
            request
                .audio_source
                .as_ref()
                .map(|source| source.0 .0.as_str())
                == Some(LOCAL_MICROPHONE)
        } else {
            request.audio_source.is_none()
        };
        let video_valid = if matches!(
            request.mode,
            MediaSessionMode::Video | MediaSessionMode::AudioVideo
        ) {
            request
                .video_source
                .as_ref()
                .map(|source| source.0 .0.as_str())
                == Some(LOCAL_CAMERA)
        } else {
            request.video_source.is_none()
        };
        if !audio_valid || !video_valid {
            return Err(MediaError::new(
                MediaErrorKind::InvalidRequest,
                "loopback native port received an unexpected source",
            ));
        }
        if let (Some(started), Some(release)) = (&self.open_started, &self.release_open) {
            started.notify_one();
            release.notified().await;
        }
        if self.open_count.fetch_add(1, Ordering::AcqRel) != 0 {
            return Err(MediaError::new(
                MediaErrorKind::Busy,
                "loopback allows only one physical lease",
            ));
        }
        let (input_sender, input_receiver) = mpsc::channel(16);
        *self
            .input_sender
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(input_sender);
        Ok(Arc::new(LoopbackDeviceSession {
            input: AsyncMutex::new(input_receiver),
            input_audio_frames: self.input_audio_frames.clone(),
            input_video_frames: self.input_video_frames.clone(),
            rendered: self.rendered_sender.clone(),
            render_attempts: self.render_attempts.clone(),
            playback_control_attempts: self.playback_control_attempts.clone(),
            stop_count: self.stop_count.clone(),
            stop_attempts: self.stop_attempts.clone(),
            stop_failures_remaining: self.stop_failures_remaining.clone(),
            mode: request.mode,
            intent: request.intent,
        }))
    }
}

fn delayed_loopback_ports() -> (Arc<LoopbackPorts>, Arc<Notify>, Arc<Notify>) {
    let (rendered_sender, rendered_receiver) = mpsc::unbounded_channel();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let ports = Arc::new(LoopbackPorts {
        open_count: AtomicUsize::new(0),
        input_audio_frames: Arc::new(AtomicUsize::new(0)),
        input_video_frames: Arc::new(AtomicUsize::new(0)),
        render_attempts: Arc::new(AtomicUsize::new(0)),
        playback_control_attempts: Arc::new(AtomicUsize::new(0)),
        stop_count: Arc::new(AtomicUsize::new(0)),
        stop_attempts: Arc::new(AtomicUsize::new(0)),
        stop_failures_remaining: Arc::new(AtomicUsize::new(0)),
        open_started: Some(started.clone()),
        release_open: Some(release.clone()),
        input_sender: Mutex::new(None),
        rendered_sender,
        rendered_receiver: AsyncMutex::new(rendered_receiver),
    });
    (ports, started, release)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_offer_rejects_mixed_host_source_routes_before_open() {
    let service = Arc::new(LoopbackMediaService {
        sources: media_sources(),
        attached: Mutex::new(Vec::new()),
    });
    let ports = LoopbackPorts::new();
    let handler =
        NativeMediaOfferHandler::new(service.clone(), ports.clone(), Arc::new(StaticConfig));
    let connection = RemoteMediaConnection {
        account_uid: "loopback-account".to_owned(),
        host_id: "loopback-host".to_owned(),
        device_id: "loopback-device".to_owned(),
        connection_id: "authenticated-remote-control-connection".to_owned(),
    };
    let other_route = "5f3802f1-6a08-42e3-877f-1e62b527c8ce";
    let offer = RemoteMediaOffer {
        v: 1,
        media_session_ref: "media_session_wrong_route".to_owned(),
        session_id: SESSION_ID.to_owned(),
        generation: 1,
        mode: RemoteMediaMode::AudioVideo,
        intent: RemoteMediaIntent::Observation,
        audio_source_ref: Some(
            remote_media_source_ref(ROUTE_ID, RemoteMediaSourceKind::Microphone).unwrap(),
        ),
        video_source_ref: Some(
            remote_media_source_ref(other_route, RemoteMediaSourceKind::Camera).unwrap(),
        ),
        offer_sdp: "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n"
            .to_owned(),
    };

    let error = handler
        .accept_offer(&connection, offer, Arc::new(NoopPlaybackReceipt))
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        remote_control_client::rpc::RemoteErrorCode::InvalidParams
    );
    assert_eq!(
        error.message,
        "media sources belong to different remote routes"
    );
    assert_eq!(ports.open_count.load(Ordering::Acquire), 0);
    assert!(service
        .attached
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_connection_during_delayed_open_stops_the_late_native_lease() {
    let service = Arc::new(LoopbackMediaService {
        sources: media_sources(),
        attached: Mutex::new(Vec::new()),
    });
    let (ports, open_started, release_open) = delayed_loopback_ports();
    let handler =
        NativeMediaOfferHandler::new(service.clone(), ports.clone(), Arc::new(StaticConfig));
    let connection = RemoteMediaConnection {
        account_uid: "loopback-account".to_owned(),
        host_id: "loopback-host".to_owned(),
        device_id: "loopback-device".to_owned(),
        connection_id: "authenticated-delayed-open-connection".to_owned(),
    };
    let offer = RemoteMediaOffer {
        v: 1,
        media_session_ref: "media_session_delayed_open".to_owned(),
        session_id: SESSION_ID.to_owned(),
        generation: 1,
        mode: RemoteMediaMode::Audio,
        intent: RemoteMediaIntent::Observation,
        audio_source_ref: Some(
            remote_media_source_ref(ROUTE_ID, RemoteMediaSourceKind::Microphone).unwrap(),
        ),
        video_source_ref: None,
        offer_sdp: "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n".to_owned(),
    };
    let accept_handler = handler.clone();
    let accept_connection = connection.clone();
    let accept = tokio::spawn(async move {
        accept_handler
            .accept_offer(&accept_connection, offer, Arc::new(NoopPlaybackReceipt))
            .await
    });
    timeout(Duration::from_secs(2), open_started.notified())
        .await
        .expect("native lease open must start");

    handler.connection_closed(&connection).await;
    let error = accept
        .await
        .expect("offer task exits after connection close")
        .expect_err("closed connection must not receive an SDP answer");
    assert_eq!(
        error.code,
        remote_control_client::rpc::RemoteErrorCode::Closed
    );

    release_open.notify_one();
    timeout(Duration::from_secs(2), async {
        while ports.stop_count.load(Ordering::Acquire) == 0 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("a native lease returned after close must be stopped");
    assert_eq!(ports.open_count.load(Ordering::Acquire), 1);
    assert_eq!(ports.stop_count.load(Ordering::Acquire), 1);
    assert!(service
        .attached
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_empty());
}

struct LoopbackResourceStore {
    next_id: AtomicUsize,
    payloads: Mutex<HashMap<String, (String, media_api::MediaPayload)>>,
}

impl LoopbackResourceStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            next_id: AtomicUsize::new(0),
            payloads: Mutex::new(HashMap::new()),
        })
    }
}

#[async_trait]
impl MediaResourceStore for LoopbackResourceStore {
    async fn put(
        &self,
        context: &MediaOperationContext,
        payload: media_api::MediaPayload,
    ) -> Result<media_api::MediaAsset, MediaError> {
        payload.validate(context.max_payload_bytes)?;
        let id = format!(
            "loopback-media-{}",
            self.next_id.fetch_add(1, Ordering::AcqRel)
        );
        let owner = owner_id(&context.owner);
        let asset = media_api::MediaAsset {
            reference: MediaRef::from(id.clone()),
            metadata: payload.metadata.clone(),
        };
        self.payloads
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id, (owner, payload));
        Ok(asset)
    }

    async fn get(
        &self,
        context: &MediaOperationContext,
        media: &MediaRef,
        max_bytes: u64,
    ) -> Result<media_api::MediaPayload, MediaError> {
        let payloads = self
            .payloads
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (owner, payload) = payloads.get(&media.0).ok_or_else(|| {
            MediaError::new(
                MediaErrorKind::NotFound,
                "loopback media reference is missing",
            )
        })?;
        if owner != &owner_id(&context.owner) {
            return Err(MediaError::new(
                MediaErrorKind::NotFound,
                "loopback media reference belongs to another owner",
            ));
        }
        if payload.bytes.len() as u64 > max_bytes {
            return Err(MediaError::new(
                MediaErrorKind::MediaTooLarge,
                "loopback media read exceeds its byte cap",
            ));
        }
        Ok(payload.clone())
    }

    async fn delete(
        &self,
        context: &MediaOperationContext,
        media: &MediaRef,
    ) -> Result<(), MediaError> {
        let mut payloads = self
            .payloads
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match payloads.get(&media.0) {
            Some((owner, _)) if owner == &owner_id(&context.owner) => {
                payloads.remove(&media.0);
                Ok(())
            }
            _ => Err(MediaError::new(
                MediaErrorKind::NotFound,
                "loopback media reference is missing",
            )),
        }
    }
}

fn owner_id(owner: &MediaOwner) -> String {
    match owner {
        MediaOwner::Session { session_id } => session_id.clone(),
        MediaOwner::Ui { instance_id } | MediaOwner::System { instance_id } => instance_id.clone(),
    }
}

fn microphone_pcm(sequence: u64) -> Vec<u8> {
    let mut pcm16le = Vec::with_capacity(480 * 2);
    for index in 0..480 {
        let phase = (index + sequence as usize * 480) % 80;
        let sample = ((phase as i32 - 40) * 300) as i16;
        pcm16le.extend_from_slice(&sample.to_le_bytes());
    }
    pcm16le
}

fn microphone_frame(sequence: u64) -> MediaFrame {
    MediaFrame {
        source: MediaSourceRef(LOCAL_MICROPHONE.into()),
        sequence,
        captured_at_unix_ms: unix_now_ms(),
        payload: MediaFramePayload::Audio(MediaPcmFrame {
            sample_rate_hz: 24_000,
            channels: 1,
            pcm16le: microphone_pcm(sequence),
        }),
    }
}

fn camera_frame(sequence: u64) -> MediaFrame {
    let (width_px, height_px) = (320u32, 240u32);
    let stride_bytes = width_px * 4;
    let mut bytes = Vec::with_capacity((stride_bytes * height_px) as usize);
    for y in 0..height_px {
        for x in 0..width_px {
            bytes.extend_from_slice(&[
                ((x + sequence as u32 * 11) % 255) as u8,
                ((y + sequence as u32 * 7) % 255) as u8,
                96,
                255,
            ]);
        }
    }
    MediaFrame {
        source: MediaSourceRef(LOCAL_CAMERA.into()),
        sequence,
        captured_at_unix_ms: unix_now_ms(),
        payload: MediaFramePayload::Video(MediaVideoFrame::Rgba8 {
            width_px,
            height_px,
            stride_bytes,
            bytes,
        }),
    }
}

fn media_sources() -> Vec<MediaSource> {
    vec![
        MediaSource {
            reference: MediaSourceRef(LOCAL_MICROPHONE.into()),
            kind: MediaSourceKind::Microphone,
            media_kind: Some(MediaKind::Audio),
            label: "Loopback microphone".to_owned(),
            usages: vec![MediaSourceUsage::Session],
            supported_operations: vec![MediaOperationKind::SessionStart],
            is_default: true,
        },
        MediaSource {
            reference: MediaSourceRef(LOCAL_CAMERA.into()),
            kind: MediaSourceKind::Camera,
            media_kind: Some(MediaKind::Video),
            label: "Loopback camera".to_owned(),
            usages: vec![MediaSourceUsage::Session],
            supported_operations: vec![MediaOperationKind::SessionStart],
            is_default: true,
        },
    ]
}

async fn wait_for_attached_sessions_to_drain(service: &LoopbackMediaService) {
    timeout(Duration::from_secs(2), async {
        loop {
            if service
                .attached
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty()
            {
                return;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("terminal remote media sessions must leave the shared service registry");
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_media_offer_and_answer_stream_pcm_and_close_one_native_lease() {
    timeout(Duration::from_secs(30), async {
        let session_id = SESSION_ID.to_owned();
        let service = Arc::new(LoopbackMediaService {
            sources: media_sources(),
            attached: Mutex::new(Vec::new()),
        });
        let ports = LoopbackPorts::new();
        // Connection teardown retries the native stop once after this
        // deliberately transient failure, then deregisters the service row.
        ports.stop_failures_remaining.store(1, Ordering::Release);
        let handler =
            NativeMediaOfferHandler::new(service.clone(), ports.clone(), Arc::new(StaticConfig));
        let connection = RemoteMediaConnection {
            account_uid: "loopback-account".to_owned(),
            host_id: "loopback-host".to_owned(),
            device_id: "loopback-device".to_owned(),
            connection_id: "loopback-control-connection".to_owned(),
        };
        let route_channel = Arc::new(LoopbackChannel {
            handler: handler.clone(),
            connection: connection.clone(),
            last_played_sample_frames: Mutex::new(0),
        });
        let owner = MediaOwner::Session {
            session_id: session_id.clone(),
        };
        let resources = LoopbackResourceStore::new();
        let connector = RemoteMediaConnector::new(Arc::new(StaticConfig), resources);
        connector.bind_media_service(service.clone());
        connector
            .register_route(
                owner.clone(),
                ROUTE_ID.to_owned(),
                "Loopback phone".to_owned(),
                vec![MediaSessionMode::Audio],
                vec![MediaSourceKindDto::Microphone],
                route_channel,
            )
            .await
            .unwrap();
        let remote_microphone =
            remote_media_source_ref(ROUTE_ID, RemoteMediaSourceKind::Microphone).unwrap();
        let context = MediaOperationContext::new(MediaOperationId::new(1, 1), owner.clone())
            .with_max_payload_bytes(1024 * 1024);
        let session = connector
            .start_session(
                context.clone(),
                MediaSessionRequest {
                    intent: MediaSessionIntent::Conversation,
                    mode: MediaSessionMode::Audio,
                    audio_source: Some(MediaSourceRef(remote_microphone.clone().into())),
                    video_source: None,
                },
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "conversation audio session failed to start; device open count={}, error={error:?}",
                    ports.open_count.load(Ordering::Acquire),
                )
            });
        let audio = session.audio_channel().await.unwrap();

        timeout(Duration::from_secs(10), async {
            loop {
                if session.status().state == MediaSessionState::Ready {
                    break;
                }
                if matches!(
                    session.status().state,
                    MediaSessionState::Failed | MediaSessionState::Ended
                ) {
                    panic!(
                        "media connection failed before becoming ready: {:?}",
                        session.status()
                    );
                }
                sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("loopback peer should reach Ready");

        for sequence in 1..=6 {
            ports.send_capture(microphone_frame(sequence)).await;
            sleep(Duration::from_millis(20)).await;
        }
        let captured = timeout(Duration::from_secs(10), audio.read())
            .await
            .expect("native microphone PCM should decode on the host")
            .unwrap()
            .expect("microphone stream should remain active");
        assert_eq!(captured.source.0 .0.as_str(), remote_microphone);
        assert!(matches!(captured.payload, MediaFramePayload::Audio(_)));

        for _ in 0..4 {
            audio
                .write(MediaPcmFrame {
                    sample_rate_hz: 24_000,
                    channels: 1,
                    pcm16le: microphone_pcm(1),
                })
                .await
                .unwrap();
        }
        let rendered = timeout(Duration::from_secs(10), ports.next_rendered())
            .await
            .expect("generated peer PCM should render through the shared native lease")
            .expect("native renderer should receive an audio frame");
        assert_eq!(rendered.sample_rate_hz, 48_000);
        assert!(rendered.pcm16le.len() >= 960);
        let barrier = timeout(Duration::from_secs(10), audio.flush())
            .await
            .expect("flush should receive actual native playback progress")
            .unwrap();
        assert!(barrier.played_sample_frames > 0);
        assert_eq!(ports.open_count.load(Ordering::Acquire), 1);

        handler.connection_closed(&connection).await;
        let ended = session.stop(context).await.unwrap();
        assert_eq!(ended.state, MediaSessionState::Ended);
        assert_eq!(ports.stop_count.load(Ordering::Acquire), 1);
        assert_eq!(ports.stop_attempts.load(Ordering::Acquire), 2);
        wait_for_attached_sessions_to_drain(&service).await;
    })
    .await
    .expect("local WebRTC media loopback must finish within its offline deadline");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_audio_video_observation_keeps_uplink_without_native_playback() {
    timeout(Duration::from_secs(30), async {
        let service = Arc::new(LoopbackMediaService {
            sources: media_sources(),
            attached: Mutex::new(Vec::new()),
        });
        let ports = LoopbackPorts::new();
        let handler =
            NativeMediaOfferHandler::new(service.clone(), ports.clone(), Arc::new(StaticConfig));
        let connection = RemoteMediaConnection {
            account_uid: "loopback-account".to_owned(),
            host_id: "loopback-host".to_owned(),
            device_id: "loopback-observation-microphone".to_owned(),
            connection_id: "loopback-observation-connection".to_owned(),
        };
        let route_channel = Arc::new(LoopbackChannel {
            handler: handler.clone(),
            connection: connection.clone(),
            last_played_sample_frames: Mutex::new(0),
        });
        let owner = MediaOwner::Session {
            session_id: SESSION_ID.to_owned(),
        };
        let resources = LoopbackResourceStore::new();
        let connector = RemoteMediaConnector::new(Arc::new(StaticConfig), resources.clone());
        connector.bind_media_service(service.clone());
        connector
            .register_route(
                owner.clone(),
                ROUTE_ID.to_owned(),
                "Loopback observation AV".to_owned(),
                vec![MediaSessionMode::AudioVideo],
                vec![MediaSourceKindDto::Microphone, MediaSourceKindDto::Camera],
                route_channel,
            )
            .await
            .unwrap();
        let remote_microphone =
            remote_media_source_ref(ROUTE_ID, RemoteMediaSourceKind::Microphone).unwrap();
        let remote_camera =
            remote_media_source_ref(ROUTE_ID, RemoteMediaSourceKind::Camera).unwrap();
        let context = MediaOperationContext::new(MediaOperationId::new(3, 1), owner)
            .with_max_payload_bytes(1024 * 1024);
        let session = connector
            .start_session(
                context.clone(),
                MediaSessionRequest {
                    intent: MediaSessionIntent::Observation,
                    mode: MediaSessionMode::AudioVideo,
                    audio_source: Some(MediaSourceRef(remote_microphone.clone().into())),
                    video_source: Some(MediaSourceRef(remote_camera.clone().into())),
                },
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "observation audio/video session failed to start; device open count={}, error={error:?}",
                    ports.open_count.load(Ordering::Acquire),
                )
            });
        let audio = session.audio_channel().await.unwrap();

        timeout(Duration::from_secs(10), async {
            loop {
                match session.status().state {
                    MediaSessionState::Ready => break,
                    MediaSessionState::Failed | MediaSessionState::Ended => {
                        panic!(
                            "observation audio peer failed before becoming Ready: {:?}",
                            session.status()
                        );
                    }
                    MediaSessionState::Preparing | MediaSessionState::Reconnecting => {
                        sleep(Duration::from_millis(25)).await;
                    }
                }
            }
        })
        .await
        .expect("observation audio peer should reach Ready");

        let control = |action| RemoteMediaControlRequest {
            v: 1,
            media_session_ref: session.reference().0 .0.clone(),
            generation: context.identity.generation,
            action,
            target_audio_sequence: 0,
            target_sample_frames: 0,
        };
        for action in [
            RemoteMediaControlAction::FlushAudio,
            RemoteMediaControlAction::InterruptAudio,
        ] {
            let error = handler
                .control(&connection, control(action))
                .await
                .unwrap_err();
            assert_eq!(
                error.code,
                remote_control_client::rpc::RemoteErrorCode::NotRemoteEnabled
            );
        }
        let gain_error = session
            .update(
                context.clone(),
                MediaSessionUpdate::SetPlaybackGain { gain: 0.5 },
            )
            .await
            .unwrap_err();
        assert_eq!(gain_error.kind, MediaErrorKind::Unsupported);
        assert_eq!(session.status().state, MediaSessionState::Ready);

        // Incoming remote PCM must not invoke a speaker callback on a
        // capture-only observation lease. Local microphone capture still has
        // to travel back over the same peer.
        for _ in 0..4 {
            audio
                .write(MediaPcmFrame {
                    sample_rate_hz: 24_000,
                    channels: 1,
                    pcm16le: microphone_pcm(1),
                })
                .await
                .unwrap();
        }
        // Preserve the native microphone's expected 20 ms cadence; video has
        // its own one-frame-per-100 ms capture schedule.
        for sequence in 1..=6 {
            ports.send_capture(microphone_frame(sequence)).await;
            sleep(Duration::from_millis(20)).await;
        }
        for sequence in 1..=6 {
            ports.send_capture(camera_frame(sequence)).await;
            sleep(Duration::from_millis(100)).await;
        }
        let captured_audio = timeout(Duration::from_secs(10), audio.read())
            .await
            .unwrap_or_else(|_| {
                let native_status = service
                    .attached
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .first()
                    .map(|(_, session)| session.status());
                panic!(
                    "observation microphone input should remain active; host={:?}, native={native_status:?}, input audio/video={}/{}, render attempts={}",
                    session.status(),
                    ports.input_audio_frames.load(Ordering::Acquire),
                    ports.input_video_frames.load(Ordering::Acquire),
                    ports.render_attempts.load(Ordering::Acquire),
                )
            })
            .unwrap()
            .expect("the remote peer should receive microphone PCM");
        assert_eq!(captured_audio.source.0 .0, remote_microphone);
        assert!(matches!(
            captured_audio.payload,
            MediaFramePayload::Audio(_)
        ));
        let asset = timeout(Duration::from_secs(15), async {
            let mut after = None;
            loop {
                let batch = session
                    .observe(context.clone(), after, 32, Some(500))
                    .await
                    .unwrap();
                after = batch.next_cursor;
                if let Some(asset) = batch.events.into_iter().find_map(|event| match event.kind {
                    MediaEventKind::Frame { source, asset } if source.0 .0 == remote_camera => {
                        Some(asset)
                    }
                    _ => None,
                }) {
                    break asset;
                }
            }
        })
        .await
        .expect("observation camera input should produce a persisted host frame");
        assert_eq!(asset.metadata.mime_type, "image/png");
        assert_eq!(asset.metadata.width_px, Some(320));
        assert_eq!(asset.metadata.height_px, Some(240));
        let stored = resources
            .get(&context, &asset.reference, context.max_payload_bytes)
            .await
            .unwrap();
        assert!(stored.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(ports.render_attempts.load(Ordering::Acquire), 0);
        assert_eq!(
            ports.playback_control_attempts.load(Ordering::Acquire),
            0
        );
        assert_eq!(session.status().state, MediaSessionState::Ready);

        let ended = session.stop(context).await.unwrap();
        assert_eq!(ended.state, MediaSessionState::Ended);
        assert_eq!(ports.open_count.load(Ordering::Acquire), 1);
        assert_eq!(ports.stop_count.load(Ordering::Acquire), 1);
        assert_eq!(
            ports.playback_control_attempts.load(Ordering::Acquire),
            0
        );
        wait_for_attached_sessions_to_drain(&service).await;
    })
    .await
    .expect("local WebRTC observation-audio loopback must finish within its offline deadline");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_camera_peer_is_observed_and_persisted_as_png_on_the_host() {
    timeout(Duration::from_secs(30), async {
        let service = Arc::new(LoopbackMediaService {
            sources: media_sources(),
            attached: Mutex::new(Vec::new()),
        });
        let ports = LoopbackPorts::new();
        let handler =
            NativeMediaOfferHandler::new(service.clone(), ports.clone(), Arc::new(StaticConfig));
        let route_channel = Arc::new(LoopbackChannel {
            handler,
            connection: RemoteMediaConnection {
                account_uid: "loopback-account".to_owned(),
                host_id: "loopback-host".to_owned(),
                device_id: "loopback-camera".to_owned(),
                connection_id: "loopback-camera-connection".to_owned(),
            },
            last_played_sample_frames: Mutex::new(0),
        });
        let owner = MediaOwner::Session {
            session_id: SESSION_ID.to_owned(),
        };
        let resources = LoopbackResourceStore::new();
        let connector = RemoteMediaConnector::new(Arc::new(StaticConfig), resources.clone());
        connector.bind_media_service(service.clone());
        connector
            .register_route(
                owner.clone(),
                ROUTE_ID.to_owned(),
                "Loopback camera".to_owned(),
                vec![MediaSessionMode::Video],
                vec![MediaSourceKindDto::Camera],
                route_channel,
            )
            .await
            .unwrap();
        let remote_camera =
            remote_media_source_ref(ROUTE_ID, RemoteMediaSourceKind::Camera).unwrap();
        let context = MediaOperationContext::new(MediaOperationId::new(2, 1), owner)
            .with_max_payload_bytes(1024 * 1024);
        let session = connector
            .start_session(
                context.clone(),
                MediaSessionRequest {
                    intent: MediaSessionIntent::Observation,
                    mode: MediaSessionMode::Video,
                    audio_source: None,
                    video_source: Some(MediaSourceRef(remote_camera.clone().into())),
                },
            )
            .await
            .unwrap();

        timeout(Duration::from_secs(10), async {
            loop {
                match session.status().state {
                    MediaSessionState::Ready => break,
                    MediaSessionState::Failed | MediaSessionState::Ended => {
                        panic!(
                            "camera peer failed before becoming ready: {:?}",
                            session.status()
                        );
                    }
                    MediaSessionState::Preparing | MediaSessionState::Reconnecting => {
                        sleep(Duration::from_millis(25)).await;
                    }
                }
            }
        })
        .await
        .expect("camera loopback peer should reach Ready");

        for sequence in 1..=6 {
            ports.send_capture(camera_frame(sequence)).await;
            sleep(Duration::from_millis(100)).await;
        }
        let asset = timeout(Duration::from_secs(15), async {
            let mut after = None;
            loop {
                let batch = session
                    .observe(context.clone(), after, 32, Some(500))
                    .await
                    .unwrap();
                after = batch.next_cursor;
                if let Some(asset) = batch.events.into_iter().find_map(|event| match event.kind {
                    MediaEventKind::Frame { source, asset } if source.0 .0 == remote_camera => {
                        Some(asset)
                    }
                    _ => None,
                }) {
                    break asset;
                }
            }
        })
        .await
        .expect("real VP8 camera ingress should produce an observed host frame");
        assert_eq!(asset.metadata.mime_type, "image/png");
        assert_eq!(asset.metadata.width_px, Some(320));
        assert_eq!(asset.metadata.height_px, Some(240));

        let stored = resources
            .get(&context, &asset.reference, context.max_payload_bytes)
            .await
            .unwrap();
        assert!(stored.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(ports.open_count.load(Ordering::Acquire), 1);
        assert_eq!(session.status().state, MediaSessionState::Ready);

        let ended = session.stop(context).await.unwrap();
        assert_eq!(ended.state, MediaSessionState::Ended);
        assert_eq!(ports.stop_count.load(Ordering::Acquire), 1);
        wait_for_attached_sessions_to_drain(&service).await;
    })
    .await
    .expect("local WebRTC camera loopback must finish within its offline deadline");
}
