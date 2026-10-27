//! Product answerer for approved Remote Control media offers.
//!
//! Firebase/ICE policy is injected through `RemoteMediaConfigProvider`; one
//! owner-bound native device lease and an independent AV peer are created for
//! each host-issued media session reference.

use crate::media::{
    parse_remote_media_source_ref, RemoteIceServer, RemoteMediaAnswer, RemoteMediaCapabilities,
    RemoteMediaConfig, RemoteMediaConfigProvider, RemoteMediaConnection, RemoteMediaControlAction,
    RemoteMediaControlChannel, RemoteMediaControlRequest, RemoteMediaControlResult,
    RemoteMediaIntent, RemoteMediaMode, RemoteMediaOffer, RemoteMediaOfferHandler,
    RemoteMediaSourceKind,
};
use crate::rpc::{RemoteErrorCode, RpcError};
use async_trait::async_trait;
use lingxi_webrtc_core::{IceConfig, IceServer, RtcSession, SessionConfig, VideoDirection};
use lingxi_webrtc_media::{
    ports::DecodedAudioPlayout,
    video_ports::{pack_rgba, pack_yuv420, NativeVideo, Plane, VideoCapturePort},
    AudioProcessor,
};
use media_api::{
    MediaDevicePorts, MediaDeviceSession, MediaError, MediaErrorKind, MediaEvent, MediaEventBatch,
    MediaEventCursor, MediaEventKind, MediaFramePayload, MediaKind, MediaOperation,
    MediaOperationContext, MediaOperationId, MediaOwner, MediaPcmFrame, MediaRenderReceipt,
    MediaResourceRef, MediaResourceState, MediaSession, MediaSessionId, MediaSessionIntent,
    MediaSessionMode, MediaSessionRef, MediaSessionRequest, MediaSessionState, MediaSessionStatus,
    MediaSessionUpdate, MediaSource, MediaSourceKind, MediaSourceRef, MediaSourceUsage,
    MediaVideoFrame,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch, Notify, OwnedMutexGuard};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const MAX_EVENTS: usize = 256;
const OUTPUT_QUEUE: usize = 32;
const AUDIO_RATE_HZ: u32 = 48_000;
const AUDIO_CAPTURE_CHUNK: usize = 256;
const CONTROL_WAIT_MAX: Duration = Duration::from_secs(30);
const CLEANUP_ATTEMPT_MAX: Duration = Duration::from_secs(5);
const COMPLETED_CLOSE_RESULTS: usize = 64;

type SessionKey = (String, String);

struct SelectedNativeSources {
    remote_audio: Option<MediaSourceRef>,
    remote_video: Option<MediaSourceRef>,
    local_audio: Option<MediaSourceRef>,
    local_video: Option<MediaSourceRef>,
    video_source_kind: Option<MediaSourceKind>,
}

#[derive(Default)]
struct HandlerState {
    closed_connections: HashSet<String>,
    connection_cancellations: HashMap<String, CancellationToken>,
    sessions: HashMap<SessionKey, Arc<NativeRemoteMediaSession>>,
    closed_control_results: HashMap<SessionKey, RemoteMediaControlResult>,
    closed_control_order: VecDeque<SessionKey>,
    offer_locks: HashMap<SessionKey, Arc<tokio::sync::Mutex<()>>>,
}

/// Native media answerer injected into an approved viewer connection.
pub struct NativeMediaOfferHandler {
    service: Arc<dyn media_api::MediaService>,
    ports: Arc<dyn MediaDevicePorts>,
    config: Arc<dyn RemoteMediaConfigProvider>,
    state: Arc<tokio::sync::Mutex<HandlerState>>,
    next_generation: AtomicU64,
}

impl NativeMediaOfferHandler {
    /// Compose one answerer over the same app-scoped service and native ports
    /// used by Agent tools and the UI.
    #[must_use]
    pub fn new(
        service: Arc<dyn media_api::MediaService>,
        ports: Arc<dyn MediaDevicePorts>,
        config: Arc<dyn RemoteMediaConfigProvider>,
    ) -> Arc<Self> {
        Arc::new(Self {
            service,
            ports,
            config,
            state: Arc::new(tokio::sync::Mutex::new(HandlerState::default())),
            next_generation: AtomicU64::new(1),
        })
    }

    async fn offer_lock(&self, key: &SessionKey) -> OwnedMutexGuard<()> {
        let lock = {
            let mut state = self.state.lock().await;
            state
                .offer_locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        lock.lock_owned().await
    }

    fn context(
        &self,
        offer: &RemoteMediaOffer,
        config: &RemoteMediaConfig,
    ) -> MediaOperationContext {
        let epoch = self.service.capabilities().service_epoch.max(1);
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed).max(1);
        MediaOperationContext::new(
            MediaOperationId::new(generation, epoch),
            MediaOwner::Session {
                session_id: offer.session_id.clone(),
            },
        )
        .with_max_payload_bytes(config.max_payload_bytes)
    }

    async fn select_sources(
        &self,
        context: &MediaOperationContext,
        offer: &RemoteMediaOffer,
    ) -> Result<SelectedNativeSources, MediaError> {
        let remote_audio = offer
            .audio_source_ref
            .clone()
            .map(|id| MediaSourceRef(id.into()));
        let remote_video = offer
            .video_source_ref
            .clone()
            .map(|id| MediaSourceRef(id.into()));
        let mut local_audio = None;
        let mut local_video = None;
        let mut selected_video_kind = None;

        let audio_route = remote_audio
            .as_ref()
            .and_then(|source| parse_remote_media_source_ref(&source.0 .0));
        let video_route = remote_video
            .as_ref()
            .and_then(|source| parse_remote_media_source_ref(&source.0 .0));
        if audio_route
            .as_ref()
            .zip(video_route.as_ref())
            .is_some_and(|((audio_route, _), (video_route, _))| audio_route != video_route)
        {
            return Err(invalid_media(
                "media sources belong to different remote routes",
            ));
        }
        // Source refs carry the host-issued media route token, which is
        // intentionally distinct from the authenticated Remote Control
        // connection ID. The product gateway binds a route to its viewer
        // channel before forwarding this offer; here we only reject a
        // cross-route audio/video selection. `connection_id` remains the
        // owner of this session's lifecycle and control messages.

        if offer.mode.audio() {
            if !matches!(audio_route, Some((_, RemoteMediaSourceKind::Microphone))) {
                return Err(invalid_media(
                    "audio source is not an approved remote microphone",
                ));
            }
            let sources = sources_for(self.service.as_ref(), context, MediaKind::Audio).await?;
            let selected = select_source(&sources, None, MediaSourceKind::Microphone)?;
            local_audio = Some(selected.reference.clone());
        } else if remote_audio.is_some() {
            return Err(invalid_media(
                "audio source supplied for a video-only offer",
            ));
        }

        if offer.mode.video() {
            let video_kind = match video_route {
                Some((_, RemoteMediaSourceKind::Camera)) => MediaSourceKind::Camera,
                Some((_, RemoteMediaSourceKind::Screen)) => MediaSourceKind::Screen,
                _ => {
                    return Err(invalid_media(
                        "video source is not an approved remote camera or screen",
                    ));
                }
            };
            let sources = sources_for(self.service.as_ref(), context, MediaKind::Video).await?;
            let selected = select_source(&sources, None, video_kind)?;
            local_video = Some(selected.reference.clone());
            selected_video_kind = Some(selected.kind);
        } else if remote_video.is_some() {
            return Err(invalid_media(
                "video source supplied for an audio-only offer",
            ));
        }

        Ok(SelectedNativeSources {
            remote_audio,
            remote_video,
            local_audio,
            local_video,
            video_source_kind: selected_video_kind,
        })
    }
}

#[async_trait]
impl RemoteMediaOfferHandler for NativeMediaOfferHandler {
    fn capabilities(&self) -> Option<RemoteMediaCapabilities> {
        Some(RemoteMediaCapabilities {
            version: 1,
            modes: vec![
                RemoteMediaMode::Audio,
                RemoteMediaMode::Video,
                RemoteMediaMode::AudioVideo,
            ],
            source_kinds: vec![
                RemoteMediaSourceKind::Microphone,
                RemoteMediaSourceKind::Camera,
            ],
        })
    }

    async fn accept_offer(
        &self,
        connection: &RemoteMediaConnection,
        offer: RemoteMediaOffer,
        control: Arc<dyn RemoteMediaControlChannel>,
    ) -> Result<RemoteMediaAnswer, RpcError> {
        offer.validate()?;
        let key = (
            connection.connection_id.clone(),
            offer.media_session_ref.clone(),
        );
        let cancel = {
            let mut state = self.state.lock().await;
            if state.closed_connections.contains(&connection.connection_id) {
                return Err(RpcError::code(RemoteErrorCode::Closed));
            }
            state
                .connection_cancellations
                .entry(connection.connection_id.clone())
                .or_insert_with(CancellationToken::new)
                .clone()
        };
        let offer_guard = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(RpcError::code(RemoteErrorCode::Closed)),
            guard = self.offer_lock(&key) => guard,
        };
        let _offer_guard = offer_guard;
        {
            let state = self.state.lock().await;
            if cancel.is_cancelled() || state.closed_connections.contains(&connection.connection_id)
            {
                return Err(RpcError::code(RemoteErrorCode::Closed));
            }
            if state.closed_control_results.contains_key(&key) {
                return Err(RpcError::new(
                    RemoteErrorCode::InvalidParams,
                    "media session reference was reused after close",
                ));
            }
            if let Some(existing) = state.sessions.get(&key) {
                if existing.offer == offer {
                    return Ok(existing.answer.clone());
                }
                return Err(RpcError::new(
                    RemoteErrorCode::InvalidParams,
                    "media session reference was reused",
                ));
            }
        }

        let config = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(RpcError::code(RemoteErrorCode::Closed)),
            result = self.config.resolve(connection, &offer) => result?,
        };
        config.validate()?;
        // The pinned ICE agent supports UDP STUN/TURN only. Keep URI parsing
        // forward-compatible, but fail this configuration before source lookup
        // or opening the user's microphone/camera lease.
        config.validate_supported_transport()?;
        lingxi_webrtc_media::runtime::initialize_configured(offer.mode.video()).map_err(|_| {
            RpcError::new(
                RemoteErrorCode::Internal,
                "the media codec runtime is unavailable",
            )
        })?;
        if cancel.is_cancelled() {
            return Err(RpcError::code(RemoteErrorCode::Closed));
        }
        let context = self.context(&offer, &config);
        let selected = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(RpcError::code(RemoteErrorCode::Closed)),
            result = self.select_sources(&context, &offer) => {
                result.map_err(media_rpc_error)?
            },
        };
        let request = MediaSessionRequest {
            intent: match offer.intent {
                RemoteMediaIntent::Observation => MediaSessionIntent::Observation,
                RemoteMediaIntent::Conversation => MediaSessionIntent::Conversation,
            },
            mode: match offer.mode {
                RemoteMediaMode::Audio => MediaSessionMode::Audio,
                RemoteMediaMode::Video => MediaSessionMode::Video,
                RemoteMediaMode::AudioVideo => MediaSessionMode::AudioVideo,
            },
            audio_source: selected.remote_audio.clone(),
            video_source: selected.remote_video.clone(),
        };
        let device_request = MediaSessionRequest {
            audio_source: selected.local_audio.clone(),
            video_source: selected.local_video.clone(),
            ..request.clone()
        };
        let ports = self.ports.clone();
        let open_context = context.clone();
        let open_cancel = cancel.clone();
        let mut open_task = tokio::spawn(async move {
            let result = ports.open(open_context, device_request).await;
            match result {
                Ok(device) if open_cancel.is_cancelled() => {
                    let _ = device.stop().await;
                    Err(MediaError::new(
                        MediaErrorKind::Cancelled,
                        "media connection closed while opening its device lease",
                    ))
                }
                result => result,
            }
        });
        let device = tokio::select! {
            biased;
            opened = &mut open_task => match opened {
                Ok(Ok(device)) => device,
                Ok(Err(error)) => return Err(media_rpc_error(error)),
                Err(error) => return Err(RpcError::new(
                    RemoteErrorCode::Internal,
                    format!("media device open task failed: {error}"),
                )),
            },
            _ = cancel.cancelled() => {
                // Keep the opener task alive: if the native callback returns a
                // lease after close, that task sees cancellation and releases it.
                return Err(RpcError::code(RemoteErrorCode::Closed));
            }
        };
        if cancel.is_cancelled() {
            let _ = device.stop().await;
            return Err(RpcError::code(RemoteErrorCode::Closed));
        }

        let ice = IceConfig {
            servers: config
                .ice_servers
                .iter()
                .map(|server: &RemoteIceServer| IceServer {
                    urls: server.urls.clone(),
                    username: server.username.clone().unwrap_or_default(),
                    credential: server.credential.clone().unwrap_or_default(),
                })
                .collect(),
            relay_only: config.relay_only,
        };
        let rtc_config = SessionConfig {
            audio: offer.mode.audio(),
            video: offer.mode.video(),
            video_direction: VideoDirection::SendOnly,
            data_channel: false,
            answerer: true,
            ice,
            gather_timeout: Some(Duration::from_millis(config.gather_timeout_ms)),
            ..SessionConfig::default()
        };
        let mut rtc = match tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                let _ = device.stop().await;
                return Err(RpcError::code(RemoteErrorCode::Closed));
            }
            result = RtcSession::with_config(rtc_config) => result,
        } {
            Ok(rtc) => rtc,
            Err(error) => {
                let _ = device.stop().await;
                return Err(media_rpc_error(native_media_error(error)));
            }
        };
        if offer.mode.audio() {
            // The core starts incoming Opus suppressed until a product speaker
            // path is ready. Open its bounded RTP queue for the session pump;
            // Observation sessions keep `audio_playout` disabled and drain/drop
            // those packets without calling the capture-only native device.
            if let Err(error) = rtc.set_playback_suppressed(false) {
                let _ = rtc.close().await;
                let _ = device.stop().await;
                return Err(media_rpc_error(native_media_error(error)));
            }
        }
        let answer_result = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            result = timeout(
                Duration::from_millis(
                    config
                        .gather_timeout_ms
                        .saturating_add(config.connect_timeout_ms),
                ),
                rtc.answer(offer.offer_sdp.clone()),
            ) => Some(result),
        };
        let Some(answer_result) = answer_result else {
            let _ = rtc.close().await;
            let _ = device.stop().await;
            return Err(RpcError::code(RemoteErrorCode::Closed));
        };
        let answer_sdp = match answer_result {
            Ok(Ok(answer)) => answer,
            Ok(Err(error)) => {
                let _ = rtc.close().await;
                let _ = device.stop().await;
                return Err(media_rpc_error(native_media_error(error)));
            }
            Err(_) => {
                let _ = rtc.close().await;
                let _ = device.stop().await;
                return Err(RpcError::code(RemoteErrorCode::Timeout));
            }
        };
        let answer = RemoteMediaAnswer {
            v: 1,
            media_session_ref: offer.media_session_ref.clone(),
            answer_sdp,
        };
        if let Err(error) = answer.validate_for(&offer) {
            let _ = rtc.close().await;
            let _ = device.stop().await;
            return Err(error);
        }

        let session = Arc::new(NativeRemoteMediaSession::new(
            key.clone(),
            offer.clone(),
            answer.clone(),
            context.clone(),
            request.clone(),
            selected.local_audio,
            selected.local_video,
            device,
            selected.video_source_kind,
            control,
            Some(Arc::downgrade(&self.service)),
            Arc::downgrade(&self.state),
        ));
        let attached = match tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                let _ = rtc.close().await;
                let _ = session.stop_internal().await;
                return Err(RpcError::code(RemoteErrorCode::Closed));
            }
            result = self.service.attach_session(
                context,
                Box::new(AttachedSession(session.clone())),
            ) => result,
        } {
            Ok(reference) => reference,
            Err(error) => {
                let _ = rtc.close().await;
                if let Err(cleanup_error) = session.stop_internal().await {
                    return Err(media_rpc_error(cleanup_error));
                }
                return Err(media_rpc_error(error));
            }
        };
        if attached != session.reference {
            let _ = rtc.close().await;
            if let Err(cleanup_error) = session.stop_internal().await {
                return Err(media_rpc_error(cleanup_error));
            }
            return Err(RpcError::new(
                RemoteErrorCode::Internal,
                "media host changed the issued session reference",
            ));
        }

        {
            let mut state = self.state.lock().await;
            if cancel.is_cancelled() || state.closed_connections.contains(&connection.connection_id)
            {
                drop(state);
                let _ = rtc.close().await;
                if let Err(cleanup_error) = session.stop_internal().await {
                    return Err(media_rpc_error(cleanup_error));
                }
                return Err(RpcError::code(RemoteErrorCode::Closed));
            }
            if state
                .sessions
                .insert(key.clone(), session.clone())
                .is_some()
            {
                drop(state);
                if let Err(cleanup_error) = session.stop_internal().await {
                    return Err(media_rpc_error(cleanup_error));
                }
                return Err(RpcError::new(
                    RemoteErrorCode::Capacity,
                    "media session reference is already attached",
                ));
            }
        }
        if let Err(error) = session.start(rtc, config.connect_timeout_ms).await {
            if let Err(cleanup_error) = session.stop_internal().await {
                return Err(media_rpc_error(cleanup_error));
            }
            return Err(error);
        }
        if cancel.is_cancelled() {
            if let Err(cleanup_error) = session.stop_internal().await {
                return Err(media_rpc_error(cleanup_error));
            }
            return Err(RpcError::code(RemoteErrorCode::Closed));
        }
        Ok(answer)
    }

    async fn control(
        &self,
        connection: &RemoteMediaConnection,
        request: RemoteMediaControlRequest,
    ) -> Result<RemoteMediaControlResult, RpcError> {
        request.validate()?;
        let key = (
            connection.connection_id.clone(),
            request.media_session_ref.clone(),
        );
        let (session, closed_result) = {
            let state = self.state.lock().await;
            (
                state.sessions.get(&key).cloned(),
                state.closed_control_results.get(&key).cloned(),
            )
        };
        if let Some(session) = session {
            return session.control(request).await;
        }
        if request.action == RemoteMediaControlAction::Close {
            if let Some(result) = closed_result {
                result.validate_for(&request)?;
                return Ok(result);
            }
        }
        Err(RpcError::new(
            RemoteErrorCode::UnknownSession,
            "media session is not active",
        ))
    }

    async fn connection_closed(&self, connection: &RemoteMediaConnection) {
        let sessions = {
            let mut state = self.state.lock().await;
            state
                .closed_connections
                .insert(connection.connection_id.clone());
            if let Some(cancel) = state
                .connection_cancellations
                .get(&connection.connection_id)
            {
                cancel.cancel();
            }
            state
                .sessions
                .iter()
                .filter(|((connection_id, _), _)| connection_id == &connection.connection_id)
                .map(|(_, session)| session.clone())
                .collect::<Vec<_>>()
        };
        for session in sessions {
            let mut stopped = false;
            for attempt in 0..4 {
                let stop_result = timeout(CLEANUP_ATTEMPT_MAX, session.stop_internal())
                    .await
                    .map_err(|_| {
                        MediaError::new(
                            MediaErrorKind::Timeout,
                            "native media cleanup did not finish before its deadline",
                        )
                    })
                    .and_then(|result| result.map(|_| ()));
                match stop_result {
                    Ok(_) => {
                        session.schedule_service_deregistration();
                        stopped = true;
                        break;
                    }
                    Err(error) if attempt < 3 => {
                        tracing::debug!(
                            kind = error.kind.to_string(),
                            attempt = attempt + 1,
                            "retrying native media cleanup after connection close"
                        );
                        tokio::time::sleep(Duration::from_millis(50u64 << attempt)).await;
                    }
                    Err(error) => {
                        tracing::debug!(
                            kind = error.kind.to_string(),
                            "native media cleanup exhausted its bounded retries"
                        );
                    }
                }
            }
            if !stopped {
                tracing::debug!(
                    media_session_ref = %session.reference.0.0,
                    "native media connection cleanup remains pending after bounded retries"
                );
            }
        }
    }
}

struct NativeRemoteMediaSession {
    key: SessionKey,
    offer: RemoteMediaOffer,
    answer: RemoteMediaAnswer,
    reference: MediaSessionRef,
    owner_session_id: String,
    context: MediaOperationContext,
    audio_source: Option<MediaSourceRef>,
    video_source: Option<MediaSourceRef>,
    native_audio_source: Option<MediaSourceRef>,
    native_video_source: Option<MediaSourceRef>,
    device: Arc<dyn MediaDeviceSession>,
    video_source_kind: Option<MediaSourceKind>,
    control: Arc<dyn RemoteMediaControlChannel>,
    service: Option<Weak<dyn media_api::MediaService>>,
    owner: Weak<tokio::sync::Mutex<HandlerState>>,
    status_tx: watch::Sender<MediaSessionStatus>,
    enabled_audio: AtomicBool,
    enabled_video: AtomicBool,
    event_sequence: AtomicU64,
    events: Mutex<VecDeque<MediaEvent>>,
    events_dropped: AtomicBool,
    event_notify: Notify,
    audio_sequence: AtomicU64,
    accepted_sample_frames: AtomicU64,
    played_sample_frames: AtomicU64,
    deferred_played_sample_frames: AtomicU64,
    rendered_at_unix_ms: AtomicU64,
    progress_tx: watch::Sender<AudioProgress>,
    playback_epoch: AtomicU64,
    playback_epoch_tx: watch::Sender<u64>,
    playback_result_gate: tokio::sync::Mutex<()>,
    cancel: CancellationToken,
    pump_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    render_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    control_gate: tokio::sync::Mutex<()>,
    stop_gate: tokio::sync::Mutex<()>,
    started: AtomicBool,
    stopping: AtomicBool,
    stopped: AtomicBool,
    stop_receipt: Mutex<Option<MediaRenderReceipt>>,
    service_registered: AtomicBool,
    service_stop_scheduled: AtomicBool,
    next_service_stop_generation: AtomicU64,
    connected_once: AtomicBool,
}

#[derive(Debug, Clone, Copy, Default)]
struct AudioProgress {
    sequence: u64,
    accepted: u64,
    played: u64,
    rendered_at: u64,
}

enum RenderCommand {
    Audio {
        frame: MediaPcmFrame,
        sequence: u64,
        epoch: u64,
    },
}

impl NativeRemoteMediaSession {
    #[allow(clippy::too_many_arguments)]
    fn new(
        key: SessionKey,
        offer: RemoteMediaOffer,
        answer: RemoteMediaAnswer,
        context: MediaOperationContext,
        request: MediaSessionRequest,
        native_audio_source: Option<MediaSourceRef>,
        native_video_source: Option<MediaSourceRef>,
        device: Arc<dyn MediaDeviceSession>,
        video_source_kind: Option<MediaSourceKind>,
        control: Arc<dyn RemoteMediaControlChannel>,
        service: Option<Weak<dyn media_api::MediaService>>,
        owner: Weak<tokio::sync::Mutex<HandlerState>>,
    ) -> Self {
        let reference = MediaSessionRef(MediaSessionId::from(offer.media_session_ref.clone()));
        let initial_status = MediaSessionStatus {
            state: MediaSessionState::Preparing,
            audio_enabled: offer.mode.audio(),
            video_enabled: offer.mode.video(),
        };
        let (status_tx, _) = watch::channel(initial_status);
        let (progress_tx, _) = watch::channel(AudioProgress::default());
        let (playback_epoch_tx, _) = watch::channel(1u64);
        let next_service_stop_generation = context.identity.generation.saturating_add(1).max(1);
        Self {
            key,
            offer,
            answer,
            reference,
            owner_session_id: context_owner_session(&context),
            context,
            audio_source: request.audio_source.clone(),
            video_source: request.video_source.clone(),
            native_audio_source,
            native_video_source,
            device,
            video_source_kind,
            control,
            service,
            owner,
            status_tx,
            enabled_audio: AtomicBool::new(true),
            enabled_video: AtomicBool::new(true),
            event_sequence: AtomicU64::new(0),
            events: Mutex::new(VecDeque::new()),
            events_dropped: AtomicBool::new(false),
            event_notify: Notify::new(),
            audio_sequence: AtomicU64::new(0),
            accepted_sample_frames: AtomicU64::new(0),
            played_sample_frames: AtomicU64::new(0),
            deferred_played_sample_frames: AtomicU64::new(0),
            rendered_at_unix_ms: AtomicU64::new(0),
            progress_tx,
            playback_epoch: AtomicU64::new(1),
            playback_epoch_tx,
            playback_result_gate: tokio::sync::Mutex::new(()),
            cancel: CancellationToken::new(),
            pump_task: tokio::sync::Mutex::new(None),
            render_task: tokio::sync::Mutex::new(None),
            control_gate: tokio::sync::Mutex::new(()),
            stop_gate: tokio::sync::Mutex::new(()),
            started: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            stop_receipt: Mutex::new(None),
            service_registered: AtomicBool::new(false),
            service_stop_scheduled: AtomicBool::new(false),
            next_service_stop_generation: AtomicU64::new(next_service_stop_generation),
            connected_once: AtomicBool::new(false),
        }
    }

    async fn start(
        self: &Arc<Self>,
        rtc: RtcSession,
        connect_timeout_ms: u64,
    ) -> Result<(), RpcError> {
        if self.started.swap(true, Ordering::AcqRel) {
            return Err(RpcError::new(
                RemoteErrorCode::Capacity,
                "media session already started",
            ));
        }
        let (output_tx, output_rx) = mpsc::channel(OUTPUT_QUEUE);
        let render_session = self.clone();
        let render_cancel = self.cancel.child_token();
        let render_task = tokio::spawn(async move {
            render_session.run_render(output_rx, render_cancel).await;
        });
        *self.render_task.lock().await = Some(render_task);

        let pump_session = self.clone();
        let cleanup_session = pump_session.clone();
        let pump_cancel = self.cancel.child_token();
        let pump_task = tokio::spawn(async move {
            let result = pump_session
                .run_pump(rtc, output_tx, connect_timeout_ms, pump_cancel.clone())
                .await;
            let terminal = match result {
                Ok(()) => MediaSessionState::Ended,
                Err(error) => {
                    tracing::debug!(
                        kind = error.kind.to_string(),
                        "native remote media pump ended"
                    );
                    MediaSessionState::Failed
                }
            };
            pump_cancel.cancel();
            if !cleanup_session.stopping.swap(true, Ordering::AcqRel) {
                let cleanup = cleanup_session.clone();
                tokio::spawn(async move {
                    if let Err(error) = cleanup.finish_from_pump(terminal).await {
                        tracing::debug!(
                            kind = error.kind.to_string(),
                            "native remote media cleanup failed"
                        );
                    }
                });
            }
        });
        *self.pump_task.lock().await = Some(pump_task);
        Ok(())
    }

    async fn run_render(
        self: Arc<Self>,
        mut requests: mpsc::Receiver<RenderCommand>,
        cancel: CancellationToken,
    ) {
        loop {
            let command = tokio::select! {
                _ = cancel.cancelled() => break,
                command = requests.recv() => match command { Some(command) => command, None => break },
            };
            match command {
                RenderCommand::Audio {
                    frame,
                    sequence,
                    epoch,
                } => {
                    if epoch != self.playback_epoch.load(Ordering::Acquire)
                        || !self.enabled_audio.load(Ordering::Acquire)
                    {
                        continue;
                    }
                    let accepted_in = frame.pcm16le.len() as u64 / (u64::from(frame.channels) * 2);
                    let result = self.device.write_audio(frame).await;
                    let receipt = match result {
                        Ok(receipt) => receipt,
                        Err(error) => {
                            if error.kind == MediaErrorKind::Cancelled
                                && epoch != self.playback_epoch.load(Ordering::Acquire)
                            {
                                continue;
                            }
                            self.set_state(MediaSessionState::Failed);
                            tracing::debug!(
                                kind = error.kind.to_string(),
                                "native remote audio render failed"
                            );
                            cancel.cancel();
                            break;
                        }
                    };
                    let accepted = u64::from(receipt.accepted_sample_frames).min(accepted_in);
                    let played = u64::from(receipt.played_sample_frames).min(accepted);
                    let accepted_total = self
                        .accepted_sample_frames
                        .fetch_add(accepted, Ordering::AcqRel)
                        .saturating_add(accepted);
                    let played_total = self
                        .played_sample_frames
                        .fetch_add(played, Ordering::AcqRel)
                        .saturating_add(played);
                    let rendered_at = receipt.rendered_at_unix_ms.unwrap_or(0);
                    if rendered_at > 0 {
                        self.rendered_at_unix_ms
                            .store(rendered_at, Ordering::Release);
                    }
                    let progress = AudioProgress {
                        sequence,
                        accepted: accepted_total,
                        played: played_total,
                        rendered_at: self.rendered_at_unix_ms.load(Ordering::Acquire),
                    };
                    self.progress_tx.send_replace(progress);
                    if played > 0 {
                        if self.send_playback_receipt(progress).await.is_err() {
                            cancel.cancel();
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn run_pump(
        self: Arc<Self>,
        mut rtc: RtcSession,
        output_tx: mpsc::Sender<RenderCommand>,
        connect_timeout_ms: u64,
        cancel: CancellationToken,
    ) -> Result<(), MediaError> {
        let mut connection_state = rtc.subscribe_connection_state();
        let mut connect_deadline = Box::pin(tokio::time::sleep(Duration::from_millis(
            connect_timeout_ms,
        )));
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        let mut audio_playout =
            if self.offer.mode.audio() && self.offer.intent == RemoteMediaIntent::Conversation {
                Some(
                    DecodedAudioPlayout::new(AUDIO_RATE_HZ)
                        .map_err(|error| native_media_error(error))?,
                )
            } else {
                None
            };
        let mut audio_processor: Option<(u32, AudioProcessor)> = None;
        let (mut video, camera_port, screen_port) = if self.offer.mode.video() {
            let (video, camera, screen) = NativeVideo::new();
            (Some(video), Some(camera), Some(screen))
        } else {
            (None, None, None)
        };
        let mut last_sequence = HashMap::<MediaSourceRef, u64>::new();
        let video_source = self.native_video_source.clone();
        let audio_source = self.native_audio_source.clone();

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                _ = &mut connect_deadline, if !self.connected_once.load(Ordering::Acquire) => {
                    return Err(MediaError::new(MediaErrorKind::Timeout, "remote media transport did not connect in time"));
                }
                state_changed = connection_state.changed() => {
                    if state_changed.is_err() { break; }
                    let state = *connection_state.borrow_and_update();
                    self.set_peer_state(state);
                    if state == webrtc::peer_connection::RTCPeerConnectionState::Connected {
                        self.connected_once.store(true, Ordering::Release);
                    }
                    if matches!(state, webrtc::peer_connection::RTCPeerConnectionState::Failed | webrtc::peer_connection::RTCPeerConnectionState::Closed) {
                        break;
                    }
                }
                input = self.device.next_input() => {
                    let frame = input.map_err(|error| error)?;
                    let Some(frame) = frame else { break; };
                    frame.validate(self.context.max_payload_bytes)?;
                    if let Some(previous) = last_sequence.get(&frame.source) {
                        if frame.sequence <= *previous {
                            return Err(invalid_media("native input sequence did not advance"));
                        }
                    }
                    last_sequence.insert(frame.source.clone(), frame.sequence);
                    match frame.payload {
                        MediaFramePayload::Audio(pcm) => {
                            if !self.offer.mode.audio() || audio_source.as_ref().is_some_and(|selected| selected != &frame.source) {
                                return Err(invalid_media("native audio source does not match the negotiated session"));
                            }
                            if self.enabled_audio.load(Ordering::Acquire) {
                                encode_capture(&mut rtc, &mut audio_processor, &pcm, frame.captured_at_unix_ms).await?;
                            }
                        }
                        MediaFramePayload::Video(video_frame) => {
                            if !self.offer.mode.video() || video_source.as_ref().is_some_and(|selected| selected != &frame.source) {
                                return Err(invalid_media("native video source does not match the negotiated session"));
                            }
                            if self.enabled_video.load(Ordering::Acquire) {
                                submit_video(
                                    self.video_source_kind,
                                    video_frame,
                                    frame.captured_at_unix_ms,
                                    camera_port.as_ref(),
                                    screen_port.as_ref(),
                                )?;
                            }
                        }
                    }
                }
                _ = interval.tick() => {
                    if self.offer.mode.audio() {
                        for _ in 0..64 {
                            let packet = rtc.take_audio_packet().map_err(native_media_error)?;
                            let Some(packet) = packet else { break; };
                            if let Some(playout) = audio_playout.as_mut() {
                                playout.push(packet).map_err(native_media_error)?;
                            }
                        }
                    }
                    if let Some(playout) = audio_playout.as_mut() {
                        for _ in 0..8 {
                            let Some(frame) = playout.try_read().map_err(native_media_error)? else { break; };
                            if !self.enabled_audio.load(Ordering::Acquire) { continue; }
                            let epoch = self.playback_epoch.load(Ordering::Acquire);
                            let command = RenderCommand::Audio {
                                frame: MediaPcmFrame {
                                    sample_rate_hz: frame.sample_rate_hz,
                                    channels: frame.channels,
                                    pcm16le: frame.pcm16le,
                                },
                                sequence: frame.sequence,
                                epoch,
                            };
                            output_tx.try_send(command).map_err(|_| MediaError::new(MediaErrorKind::Busy, "native audio render queue is full"))?;
                        }
                    }
                    if let Some(video) = video.as_mut() {
                        let camera_enabled = self.enabled_video.load(Ordering::Acquire)
                            && self.video_source_kind == Some(MediaSourceKind::Camera);
                        let screen_enabled = self.enabled_video.load(Ordering::Acquire)
                            && self.video_source_kind == Some(MediaSourceKind::Screen);
                        video.controls(camera_enabled, screen_enabled, false).map_err(native_media_error)?;
                        video.service_sending(&mut rtc).await.map_err(native_media_error)?;
                    }
                }
            }
        }
        if let Some(playout) = audio_playout.as_mut() {
            let _ = playout.interrupt();
        }
        let _ = rtc.close().await;
        Ok(())
    }

    async fn control(
        &self,
        request: RemoteMediaControlRequest,
    ) -> Result<RemoteMediaControlResult, RpcError> {
        request.validate_for(&self.offer)?;
        if self.offer.intent == RemoteMediaIntent::Observation
            && request.action != RemoteMediaControlAction::Close
        {
            return Err(RpcError::new(
                RemoteErrorCode::NotRemoteEnabled,
                "audio playback controls are unsupported for observation sessions",
            ));
        }
        // Flush can wait for seconds while output drains. Interrupt and close
        // must remain able to enter during that wait so they can wake it.
        let _control_guard = match request.action {
            RemoteMediaControlAction::FlushAudio => None,
            RemoteMediaControlAction::InterruptAudio | RemoteMediaControlAction::Close => {
                Some(self.control_gate.lock().await)
            }
        };
        if self.stopped.load(Ordering::Acquire)
            || (self.stopping.load(Ordering::Acquire)
                && request.action != RemoteMediaControlAction::Close)
        {
            return Err(RpcError::code(RemoteErrorCode::Closed));
        }
        let mut progress = self.progress_tx.subscribe();
        let mut playback_epoch = self.playback_epoch_tx.subscribe();
        let request_epoch = *playback_epoch.borrow_and_update();
        if request.action == RemoteMediaControlAction::Close {
            self.stopping.store(true, Ordering::Release);
            {
                let _result_guard = self.playback_result_gate.lock().await;
                let next_epoch = self.playback_epoch.fetch_add(1, Ordering::AcqRel) + 1;
                self.playback_epoch_tx.send_replace(next_epoch);
            }
            self.stop_locked(MediaSessionState::Ended, true)
                .await
                .map_err(media_rpc_error)?;
            drop(_control_guard);
            self.schedule_service_deregistration();
            return self.control_result(&request);
        }
        if request.action == RemoteMediaControlAction::FlushAudio {
            let target = request.target_sample_frames;
            timeout(CONTROL_WAIT_MAX, async {
                tokio::select! {
                    _ = self.cancel.cancelled() => Err(RpcError::code(RemoteErrorCode::Closed)),
                    _ = playback_epoch.changed() => Err(RpcError::code(RemoteErrorCode::Closed)),
                    result = progress.wait_for(|value| value.accepted >= target) => {
                        result.map(|_| ()).map_err(|_| RpcError::code(RemoteErrorCode::Closed))
                    },
                }
            })
            .await
            .map_err(|_| RpcError::code(RemoteErrorCode::Timeout))??;
        }
        let barrier = match request.action {
            RemoteMediaControlAction::FlushAudio => timeout(CONTROL_WAIT_MAX, async {
                tokio::select! {
                    biased;
                    _ = self.cancel.cancelled() => Err(RpcError::code(RemoteErrorCode::Closed)),
                    _ = playback_epoch.changed() => Err(RpcError::code(RemoteErrorCode::Closed)),
                    result = self.device.flush_audio() => result.map_err(media_rpc_error),
                }
            })
            .await
            .map_err(|_| RpcError::code(RemoteErrorCode::Timeout))??,
            RemoteMediaControlAction::InterruptAudio => {
                {
                    let _result_guard = self.playback_result_gate.lock().await;
                    let next_epoch = self.playback_epoch.fetch_add(1, Ordering::AcqRel) + 1;
                    self.playback_epoch_tx.send_replace(next_epoch);
                }
                self.device
                    .interrupt_audio()
                    .await
                    .map_err(media_rpc_error)?
            }
            RemoteMediaControlAction::Close => unreachable!("close handled before audio barriers"),
        };
        let (played_before, progress) = {
            // This atomic section determines whether a completed flush won the
            // race with a later interrupt. A losing flush never advances the
            // cumulative counter or emits a playback-complete notification.
            let _result_guard = self.playback_result_gate.lock().await;
            if request.action == RemoteMediaControlAction::FlushAudio
                && self.playback_epoch.load(Ordering::Acquire) != request_epoch
            {
                return Err(RpcError::code(RemoteErrorCode::Closed));
            }
            let played_before = self.played_sample_frames.load(Ordering::Acquire);
            if barrier.played_sample_frames > 0 {
                self.played_sample_frames
                    .fetch_add(u64::from(barrier.played_sample_frames), Ordering::AcqRel);
            }
            if let Some(at) = barrier.rendered_at_unix_ms {
                self.rendered_at_unix_ms.store(at, Ordering::Release);
            }
            let prior_progress = *progress.borrow_and_update();
            let progress = AudioProgress {
                sequence: prior_progress.sequence,
                accepted: self.accepted_sample_frames.load(Ordering::Acquire),
                played: self.played_sample_frames.load(Ordering::Acquire),
                rendered_at: self.rendered_at_unix_ms.load(Ordering::Acquire),
            };
            (played_before, progress)
        };
        if progress.played > played_before {
            self.send_playback_receipt(progress).await?;
        }
        self.control_result(&request)
    }

    fn control_result(
        &self,
        request: &RemoteMediaControlRequest,
    ) -> Result<RemoteMediaControlResult, RpcError> {
        let progress = *self.progress_tx.borrow();
        let result = RemoteMediaControlResult {
            v: 1,
            media_session_ref: self.offer.media_session_ref.clone(),
            generation: self.offer.generation,
            audio_sequence: progress.sequence,
            accepted_sample_frames: self.accepted_sample_frames.load(Ordering::Acquire),
            played_sample_frames: self.played_sample_frames.load(Ordering::Acquire),
            rendered_at_unix_ms: (self.rendered_at_unix_ms.load(Ordering::Acquire) > 0)
                .then(|| self.rendered_at_unix_ms.load(Ordering::Acquire)),
        };
        result.validate_for(request)?;
        Ok(result)
    }

    async fn send_playback_receipt(&self, progress: AudioProgress) -> Result<(), RpcError> {
        if progress.sequence == 0 || progress.played == 0 {
            return Ok(());
        }
        self.control
            .playback_receipt(crate::media::RemoteMediaPlaybackReceipt {
                v: 1,
                media_session_ref: self.offer.media_session_ref.clone(),
                generation: self.offer.generation,
                audio_sequence: progress.sequence,
                accepted_sample_frames: progress.accepted,
                played_sample_frames: progress.played,
                rendered_at_unix_ms: (progress.rendered_at > 0).then_some(progress.rendered_at),
            })
            .await
    }

    async fn stop_internal(&self) -> Result<MediaRenderReceipt, MediaError> {
        self.stopping.store(true, Ordering::Release);
        {
            let _result_guard = self.playback_result_gate.lock().await;
            let next_epoch = self.playback_epoch.fetch_add(1, Ordering::AcqRel) + 1;
            self.playback_epoch_tx.send_replace(next_epoch);
        }
        self.cancel.cancel();
        let control_guard = self.control_gate.lock().await;
        let result = self.stop_locked(MediaSessionState::Ended, true).await;
        drop(control_guard);
        if result.is_ok() {
            self.schedule_service_deregistration();
        }
        result
    }

    async fn stop_locked(
        &self,
        terminal_state: MediaSessionState,
        join_pump: bool,
    ) -> Result<MediaRenderReceipt, MediaError> {
        let _stop_guard = self.stop_gate.lock().await;
        if self.stopped.load(Ordering::Acquire) {
            return Ok(self
                .stop_receipt
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .unwrap_or(MediaRenderReceipt {
                    accepted_sample_frames: 0,
                    played_sample_frames: 0,
                    rendered_at_unix_ms: None,
                }));
        }
        self.stopping.store(true, Ordering::Release);
        self.cancel.cancel();
        let mut cleanup_error = None;
        let interruption = if self.offer.mode.audio()
            && self.offer.intent == RemoteMediaIntent::Conversation
        {
            match self.device.interrupt_audio().await {
                Ok(receipt) => {
                    if receipt.played_sample_frames > 0 {
                        self.played_sample_frames
                            .fetch_add(u64::from(receipt.played_sample_frames), Ordering::AcqRel);
                    }
                    if let Some(rendered_at) = receipt.rendered_at_unix_ms {
                        self.rendered_at_unix_ms
                            .store(rendered_at, Ordering::Release);
                    }
                    Some(receipt)
                }
                Err(error) => {
                    cleanup_error = Some(error);
                    None
                }
            }
        } else {
            None
        };

        if join_pump {
            let mut pump = self.pump_task.lock().await;
            if let Some(task) = pump.as_mut() {
                let result = join_media_task(task, "native media pump").await;
                pump.take();
                if let Err(error) = result {
                    cleanup_error.get_or_insert(error);
                }
            }
            drop(pump);
        }
        let mut render = self.render_task.lock().await;
        if let Some(task) = render.as_mut() {
            let result = join_media_task(task, "native media renderer").await;
            render.take();
            if let Err(error) = result {
                cleanup_error.get_or_insert(error);
            }
        }
        drop(render);
        if let Err(error) = self.device.stop().await {
            cleanup_error.get_or_insert(error);
        }
        if let Some(error) = cleanup_error {
            self.set_state(MediaSessionState::Failed);
            return Err(error);
        }

        let receipt = interruption.unwrap_or(MediaRenderReceipt {
            accepted_sample_frames: 0,
            played_sample_frames: 0,
            rendered_at_unix_ms: None,
        });
        *self
            .stop_receipt
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(receipt);
        self.stopped.store(true, Ordering::Release);
        self.set_state(terminal_state);
        let close_request = RemoteMediaControlRequest {
            v: 1,
            media_session_ref: self.offer.media_session_ref.clone(),
            generation: self.offer.generation,
            action: RemoteMediaControlAction::Close,
            target_audio_sequence: 0,
            target_sample_frames: 0,
        };
        let close_result = self
            .control_result(&close_request)
            .map_err(rpc_media_error)?;
        if let Some(owner) = self.owner.upgrade() {
            let mut state = owner.lock().await;
            if state.sessions.get(&self.key).is_some_and(|session| {
                session.reference == self.reference
                    && session.offer.generation == self.offer.generation
            }) {
                state.sessions.remove(&self.key);
            }
            state
                .closed_control_results
                .insert(self.key.clone(), close_result);
            state.closed_control_order.retain(|key| key != &self.key);
            state.closed_control_order.push_back(self.key.clone());
            while state.closed_control_order.len() > COMPLETED_CLOSE_RESULTS {
                if let Some(oldest) = state.closed_control_order.pop_front() {
                    state.closed_control_results.remove(&oldest);
                }
            }
        }
        Ok(receipt)
    }

    async fn finish_from_pump(&self, state: MediaSessionState) -> Result<(), MediaError> {
        self.stopping.store(true, Ordering::Release);
        {
            let _result_guard = self.playback_result_gate.lock().await;
            let next_epoch = self.playback_epoch.fetch_add(1, Ordering::AcqRel) + 1;
            self.playback_epoch_tx.send_replace(next_epoch);
        }
        self.cancel.cancel();
        let control_guard = self.control_gate.lock().await;
        let result = self.stop_locked(state, true).await.map(|_| ());
        drop(control_guard);
        if result.is_ok() {
            self.schedule_service_deregistration();
        }
        result
    }

    /// Remove a terminal incoming session from the app-scoped service without
    /// holding native stop or handler registry locks across the service call.
    /// The weak reference avoids a cycle through the service's connector router.
    fn schedule_service_deregistration(&self) {
        if !self.service_registered.load(Ordering::Acquire) || !self.stopped.load(Ordering::Acquire)
        {
            return;
        }
        let Some(service) = self.service.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        if self.service_stop_scheduled.swap(true, Ordering::AcqRel) {
            return;
        }
        let owner = self.context.owner.clone();
        let session = self.reference.clone();
        let max_payload_bytes = self.context.max_payload_bytes;
        let base_generation = self
            .next_service_stop_generation
            .fetch_add(4, Ordering::Relaxed)
            .max(1);
        tokio::spawn(async move {
            for attempt in 0..4u32 {
                let capabilities = service.capabilities_for(&owner);
                if !capabilities.supports(media_api::MediaOperationKind::Stop) {
                    tracing::warn!("media service cannot deregister a terminal remote session");
                    return;
                }
                let generation = base_generation.saturating_add(u64::from(attempt)).max(1);
                let context = MediaOperationContext::new(
                    MediaOperationId::new(generation, capabilities.service_epoch),
                    owner.clone(),
                )
                .with_timeout_budget_ms(5_000)
                .with_max_payload_bytes(max_payload_bytes.min(capabilities.max_payload_bytes));
                match service
                    .execute(
                        context,
                        MediaOperation::Stop {
                            target: MediaResourceRef::Session(session.clone()),
                        },
                    )
                    .await
                {
                    Ok(_) => return,
                    Err(error) if error.kind == MediaErrorKind::NotFound => return,
                    Err(error) if attempt < 3 => {
                        tracing::debug!(
                            kind = error.kind.to_string(),
                            attempt = attempt + 1,
                            "retrying attached media session deregistration"
                        );
                        tokio::time::sleep(Duration::from_millis(50u64 << attempt)).await;
                    }
                    Err(error) => {
                        tracing::warn!(
                            kind = error.kind.to_string(),
                            "attached media session deregistration exhausted retries"
                        );
                    }
                }
            }
        });
    }

    fn on_registered(&self) {
        self.service_registered.store(true, Ordering::Release);
        // The peer pump may terminate between connector startup and service
        // insertion. Arm cleanup only after successful registration, then
        // retry the terminal notification if that race already occurred.
        if self.stopped.load(Ordering::Acquire) {
            self.schedule_service_deregistration();
        }
    }

    fn set_peer_state(&self, state: webrtc::peer_connection::RTCPeerConnectionState) {
        if state == webrtc::peer_connection::RTCPeerConnectionState::Connected {
            self.connected_once.store(true, Ordering::Release);
        }
        let mapped = match state {
            webrtc::peer_connection::RTCPeerConnectionState::Connected => MediaSessionState::Ready,
            webrtc::peer_connection::RTCPeerConnectionState::Disconnected
                if self.connected_once.load(Ordering::Acquire) =>
            {
                MediaSessionState::Reconnecting
            }
            webrtc::peer_connection::RTCPeerConnectionState::Connecting
            | webrtc::peer_connection::RTCPeerConnectionState::New
            | webrtc::peer_connection::RTCPeerConnectionState::Unspecified
            | webrtc::peer_connection::RTCPeerConnectionState::Disconnected => {
                MediaSessionState::Preparing
            }
            webrtc::peer_connection::RTCPeerConnectionState::Failed => MediaSessionState::Failed,
            webrtc::peer_connection::RTCPeerConnectionState::Closed
                if self.stopped.load(Ordering::Acquire) =>
            {
                MediaSessionState::Ended
            }
            webrtc::peer_connection::RTCPeerConnectionState::Closed => MediaSessionState::Failed,
        };
        self.set_state(mapped);
    }

    fn set_state(&self, state: MediaSessionState) {
        let mut status = self.status_tx.borrow().clone();
        if status.state == state {
            return;
        }
        status.state = state;
        self.status_tx.send_replace(status);
        let sequence = self
            .event_sequence
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let resource = MediaResourceRef::Session(self.reference.clone());
        let state = match state {
            MediaSessionState::Ready | MediaSessionState::Reconnecting => {
                MediaResourceState::Active
            }
            MediaSessionState::Ended => MediaResourceState::Completed,
            MediaSessionState::Failed => MediaResourceState::Failed,
            MediaSessionState::Preparing => MediaResourceState::Ready,
        };
        let event = MediaEvent {
            sequence,
            occurred_at_unix_ms: unix_now_ms(),
            target: resource.clone(),
            operation_id: None,
            kind: MediaEventKind::StateChanged { resource, state },
        };
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if events.len() >= MAX_EVENTS {
            events.pop_front();
            self.events_dropped.store(true, Ordering::Release);
        }
        events.push_back(event);
        self.event_notify.notify_waiters();
    }
}

#[async_trait]
impl MediaSession for NativeRemoteMediaSession {
    fn reference(&self) -> &MediaSessionRef {
        &self.reference
    }

    fn status(&self) -> MediaSessionStatus {
        self.status_tx.borrow().clone()
    }

    fn authorized_sources(&self) -> Vec<MediaSourceRef> {
        self.audio_source
            .iter()
            .chain(self.video_source.iter())
            .cloned()
            .collect()
    }

    async fn update(
        &self,
        context: MediaOperationContext,
        update: MediaSessionUpdate,
    ) -> Result<MediaSessionStatus, MediaError> {
        self.validate_context(&context)?;
        match &update {
            MediaSessionUpdate::SetPlaybackGain { .. }
                if self.offer.intent == RemoteMediaIntent::Observation =>
            {
                return Err(MediaError::new(
                    MediaErrorKind::Unsupported,
                    "playback gain is unsupported for observation sessions",
                ));
            }
            MediaSessionUpdate::SetEnabled {
                kind: MediaKind::Audio,
                enabled,
            } if !self.offer.mode.audio() && *enabled => {
                return Err(invalid_media("audio was not negotiated"));
            }
            MediaSessionUpdate::SetEnabled {
                kind: MediaKind::Video,
                enabled,
            } if !self.offer.mode.video() && *enabled => {
                return Err(invalid_media("video was not negotiated"));
            }
            MediaSessionUpdate::SelectSource {
                kind: MediaKind::Audio,
                source,
            } if self.audio_source.as_ref() != Some(source) => {
                return Err(invalid_media(
                    "audio source replacement requires a new media session",
                ));
            }
            MediaSessionUpdate::SelectSource {
                kind: MediaKind::Video,
                source,
            } if self.video_source.as_ref() != Some(source) => {
                return Err(invalid_media(
                    "video source replacement requires a new media session",
                ));
            }
            _ => {}
        }
        let device_update = match &update {
            MediaSessionUpdate::SelectSource {
                kind: MediaKind::Audio,
                source,
            } if self.audio_source.as_ref() == Some(source) => MediaSessionUpdate::SelectSource {
                kind: MediaKind::Audio,
                source: self
                    .native_audio_source
                    .clone()
                    .ok_or_else(|| invalid_media("native microphone source is unavailable"))?,
            },
            MediaSessionUpdate::SelectSource {
                kind: MediaKind::Video,
                source,
            } if self.video_source.as_ref() == Some(source) => MediaSessionUpdate::SelectSource {
                kind: MediaKind::Video,
                source: self
                    .native_video_source
                    .clone()
                    .ok_or_else(|| invalid_media("native video source is unavailable"))?,
            },
            _ => update.clone(),
        };
        let status = self.device.update(device_update).await?;
        if let MediaSessionUpdate::SetEnabled {
            kind: MediaKind::Audio,
            enabled,
        } = &update
        {
            self.enabled_audio.store(*enabled, Ordering::Release);
            let mut current = self.status_tx.borrow().clone();
            current.audio_enabled = *enabled;
            self.status_tx.send_replace(current);
        }
        if let MediaSessionUpdate::SetEnabled {
            kind: MediaKind::Video,
            enabled,
        } = &update
        {
            self.enabled_video.store(*enabled, Ordering::Release);
            let mut current = self.status_tx.borrow().clone();
            current.video_enabled = *enabled;
            self.status_tx.send_replace(current);
        }
        let mut current = self.status_tx.borrow().clone();
        current.audio_enabled = status.audio_enabled;
        current.video_enabled = status.video_enabled;
        self.status_tx.send_replace(current.clone());
        Ok(current)
    }

    async fn observe(
        &self,
        context: MediaOperationContext,
        after: Option<MediaEventCursor>,
        limit: u16,
        wait_ms: Option<u64>,
    ) -> Result<MediaEventBatch, MediaError> {
        self.validate_context(&context)?;
        if limit == 0
            || limit > media_api::MAX_MEDIA_EVENT_BATCH_SIZE
            || wait_ms.is_some_and(|ms| ms > 120_000)
        {
            return Err(invalid_media("media observation bounds are invalid"));
        }
        let after = after.map_or(0, |cursor| cursor.sequence);
        let collect = || -> Result<(MediaEventBatch, bool), MediaError> {
            let events = self
                .events
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let dropped = self.events_dropped.load(Ordering::Acquire)
                && events
                    .front()
                    .is_some_and(|event| event.sequence > after.saturating_add(1));
            let items = events
                .iter()
                .filter(|event| event.sequence > after)
                .take(usize::from(limit))
                .cloned()
                .collect::<Vec<_>>();
            let target = MediaResourceRef::Session(self.reference.clone());
            Ok((
                MediaEventBatch::bounded(target, items, dropped)?,
                events.is_empty() || events.iter().all(|event| event.sequence <= after),
            ))
        };
        let (batch, empty) = collect()?;
        if !empty || wait_ms.unwrap_or(0) == 0 {
            return Ok(batch);
        }
        let notified = self.event_notify.notified();
        if timeout(Duration::from_millis(wait_ms.unwrap_or(0)), notified)
            .await
            .is_err()
        {
            return Ok(collect()?.0);
        }
        Ok(collect()?.0)
    }

    async fn stop(&self, context: MediaOperationContext) -> Result<MediaSessionStatus, MediaError> {
        self.validate_context(&context)?;
        self.stop_internal().await?;
        Ok(self.status())
    }
}

impl NativeRemoteMediaSession {
    fn validate_context(&self, context: &MediaOperationContext) -> Result<(), MediaError> {
        if context.owner
            != (MediaOwner::Session {
                session_id: self.owner_session_id.clone(),
            })
        {
            return Err(MediaError::new(
                MediaErrorKind::NotFound,
                "media session owner does not match",
            ));
        }
        Ok(())
    }
}

struct AttachedSession(Arc<NativeRemoteMediaSession>);

#[async_trait]
impl MediaSession for AttachedSession {
    fn reference(&self) -> &MediaSessionRef {
        self.0.reference()
    }
    fn on_registered(&self) {
        self.0.on_registered();
    }
    fn status(&self) -> MediaSessionStatus {
        self.0.status()
    }
    fn authorized_sources(&self) -> Vec<MediaSourceRef> {
        self.0.authorized_sources()
    }
    async fn update(
        &self,
        context: MediaOperationContext,
        update: MediaSessionUpdate,
    ) -> Result<MediaSessionStatus, MediaError> {
        self.0.update(context, update).await
    }
    async fn observe(
        &self,
        context: MediaOperationContext,
        after: Option<MediaEventCursor>,
        limit: u16,
        wait_ms: Option<u64>,
    ) -> Result<MediaEventBatch, MediaError> {
        self.0.observe(context, after, limit, wait_ms).await
    }
    async fn stop(&self, context: MediaOperationContext) -> Result<MediaSessionStatus, MediaError> {
        self.0.stop(context).await
    }
}

async fn sources_for(
    service: &dyn media_api::MediaService,
    context: &MediaOperationContext,
    kind: MediaKind,
) -> Result<Vec<MediaSource>, MediaError> {
    match service
        .execute(
            context.clone(),
            MediaOperation::Sources {
                kind: Some(kind),
                usage: Some(MediaSourceUsage::Session),
            },
        )
        .await?
    {
        media_api::MediaResult::Sources { sources } => Ok(sources),
        _ => Err(invalid_media("source discovery returned an invalid result")),
    }
}

fn select_source(
    sources: &[MediaSource],
    selected: Option<&MediaSourceRef>,
    expected_kind: MediaSourceKind,
) -> Result<MediaSource, MediaError> {
    let source = if let Some(selected) = selected {
        sources.iter().find(|source| &source.reference == selected)
    } else {
        sources
            .iter()
            .find(|source| source.is_default && source.kind == expected_kind)
    }
    .ok_or_else(|| {
        MediaError::new(
            MediaErrorKind::NotFound,
            "selected media source is not available",
        )
    })?;
    if source.kind != expected_kind {
        return Err(invalid_media(
            "selected media source has the wrong device kind",
        ));
    }
    Ok(source.clone())
}

async fn encode_capture(
    rtc: &mut RtcSession,
    processor: &mut Option<(u32, AudioProcessor)>,
    pcm: &MediaPcmFrame,
    captured_at_unix_ms: u64,
) -> Result<(), MediaError> {
    pcm.validate(media_api::DEFAULT_MAX_MEDIA_PAYLOAD_BYTES)?;
    let rate = pcm.sample_rate_hz;
    let (_, current) = match processor {
        Some((current_rate, processor)) if *current_rate == rate => (*current_rate, processor),
        slot => {
            let new = AudioProcessor::new(rate, rate).map_err(native_media_error)?;
            *slot = Some((rate, new));
            let Some((current_rate, processor)) = slot.as_mut() else {
                unreachable!()
            };
            (*current_rate, processor)
        }
    };
    let channels = usize::from(pcm.channels);
    let samples = pcm
        .pcm16le
        .chunks_exact(channels * 2)
        .map(|frame| {
            frame
                .chunks_exact(2)
                .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / i16::MAX as f32)
                .sum::<f32>()
                / channels as f32
        })
        .collect::<Vec<_>>();
    let at = capture_instant(captured_at_unix_ms)?;
    for (offset, chunk) in samples.chunks(AUDIO_CAPTURE_CHUNK).enumerate() {
        let chunk_at =
            at + Duration::from_secs_f64((offset * AUDIO_CAPTURE_CHUNK) as f64 / f64::from(rate));
        let encoded_frames = current
            .capture(chunk, chunk_at)
            .map_err(native_media_error)?;
        for encoded in encoded_frames {
            rtc.send_opus(encoded.data, encoded.at)
                .await
                .map_err(native_media_error)?;
        }
    }
    Ok(())
}

fn submit_video(
    source_kind: Option<MediaSourceKind>,
    frame: MediaVideoFrame,
    captured_at_unix_ms: u64,
    camera: Option<&VideoCapturePort>,
    screen: Option<&VideoCapturePort>,
) -> Result<(), MediaError> {
    let at = capture_instant(captured_at_unix_ms)?;
    let (port, packed) = match (source_kind, frame) {
        (Some(MediaSourceKind::Camera), frame) => (camera, pack_video(frame)?),
        (Some(MediaSourceKind::Screen), frame) => (screen, pack_video(frame)?),
        _ => return Err(invalid_media("video source kind is unsupported")),
    };
    let port = port.ok_or_else(|| {
        MediaError::new(MediaErrorKind::Unsupported, "video source is unavailable")
    })?;
    port.submit(packed, at).map_err(native_media_error)
}

fn pack_video(
    frame: MediaVideoFrame,
) -> Result<lingxi_webrtc_media::video::VideoFrame, MediaError> {
    match frame {
        MediaVideoFrame::Rgba8 {
            width_px,
            height_px,
            stride_bytes,
            bytes,
        } => pack_rgba(
            width_px,
            height_px,
            Plane {
                bytes: &bytes,
                row_stride: stride_bytes as usize,
                pixel_stride: 4,
            },
            false,
        )
        .map_err(native_media_error),
        MediaVideoFrame::I420 {
            width_px,
            height_px,
            y_stride_bytes,
            u_stride_bytes,
            v_stride_bytes,
            y,
            u,
            v,
        } => pack_yuv420(
            width_px,
            height_px,
            [
                Plane {
                    bytes: &y,
                    row_stride: y_stride_bytes as usize,
                    pixel_stride: 1,
                },
                Plane {
                    bytes: &u,
                    row_stride: u_stride_bytes as usize,
                    pixel_stride: 1,
                },
                Plane {
                    bytes: &v,
                    row_stride: v_stride_bytes as usize,
                    pixel_stride: 1,
                },
            ],
        )
        .map_err(native_media_error),
    }
}

fn capture_instant(captured_at_unix_ms: u64) -> Result<Instant, MediaError> {
    let now_ms = unix_now_ms();
    let age = now_ms.saturating_sub(captured_at_unix_ms);
    if captured_at_unix_ms > now_ms.saturating_add(100) || age > 1_000 {
        return Err(invalid_media("native media capture timestamp is stale"));
    }
    Ok(Instant::now()
        .checked_sub(Duration::from_millis(age))
        .unwrap_or_else(Instant::now))
}

fn context_owner_session(context: &MediaOperationContext) -> String {
    match &context.owner {
        MediaOwner::Session { session_id } => session_id.clone(),
        _ => String::new(),
    }
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u128::from(u64::MAX)) as u64
        })
}

fn invalid_media(message: &str) -> MediaError {
    MediaError::new(MediaErrorKind::InvalidRequest, message)
}

fn native_media_error(error: impl std::fmt::Display) -> MediaError {
    MediaError::new(MediaErrorKind::NativeFailure, error.to_string())
}

async fn join_media_task(
    task: &mut JoinHandle<()>,
    task_name: &'static str,
) -> Result<(), MediaError> {
    match timeout(Duration::from_secs(2), &mut *task).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) if error.is_cancelled() => Ok(()),
        Ok(Err(error)) => Err(MediaError::new(
            MediaErrorKind::NativeFailure,
            format!("{task_name} task failed: {error}"),
        )),
        Err(_) => {
            task.abort();
            match (&mut *task).await {
                Ok(()) => Ok(()),
                Err(error) if error.is_cancelled() => Ok(()),
                Err(error) => Err(MediaError::new(
                    MediaErrorKind::NativeFailure,
                    format!("{task_name} task failed after cancellation: {error}"),
                )),
            }
        }
    }
}

fn media_rpc_error(error: MediaError) -> RpcError {
    let code = match error.kind {
        MediaErrorKind::PermissionDenied => RemoteErrorCode::Forbidden,
        MediaErrorKind::Busy => RemoteErrorCode::Capacity,
        MediaErrorKind::Timeout => RemoteErrorCode::Timeout,
        MediaErrorKind::Cancelled => RemoteErrorCode::Closed,
        MediaErrorKind::NotFound => RemoteErrorCode::UnknownSession,
        MediaErrorKind::InvalidRequest | MediaErrorKind::MediaTooLarge => {
            RemoteErrorCode::InvalidParams
        }
        _ => RemoteErrorCode::Internal,
    };
    RpcError::new(code, error.message)
}

fn rpc_media_error(error: RpcError) -> MediaError {
    let kind = match error.code {
        RemoteErrorCode::Capacity => MediaErrorKind::Busy,
        RemoteErrorCode::InvalidParams => MediaErrorKind::InvalidRequest,
        RemoteErrorCode::UnknownSession => MediaErrorKind::NotFound,
        RemoteErrorCode::Forbidden | RemoteErrorCode::NotApproved => {
            MediaErrorKind::PermissionDenied
        }
        RemoteErrorCode::NotRemoteEnabled | RemoteErrorCode::UpgradeRequired => {
            MediaErrorKind::Unsupported
        }
        RemoteErrorCode::Timeout => MediaErrorKind::Timeout,
        RemoteErrorCode::Closed => MediaErrorKind::Cancelled,
        RemoteErrorCode::NotPending | RemoteErrorCode::Internal => MediaErrorKind::NativeFailure,
    };
    MediaError::new(kind, error.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::AtomicUsize;

    struct RetryStopDevice {
        stop_calls: AtomicUsize,
    }

    #[async_trait]
    impl MediaDeviceSession for RetryStopDevice {
        async fn next_input(&self) -> Result<Option<media_api::MediaFrame>, MediaError> {
            Ok(None)
        }

        async fn write_audio(
            &self,
            frame: MediaPcmFrame,
        ) -> Result<MediaRenderReceipt, MediaError> {
            Ok(MediaRenderReceipt {
                accepted_sample_frames: u32::try_from(
                    frame.pcm16le.len() / (usize::from(frame.channels) * 2),
                )
                .unwrap_or(u32::MAX),
                played_sample_frames: 0,
                rendered_at_unix_ms: None,
            })
        }

        async fn write_video(&self, _frame: MediaVideoFrame) -> Result<(), MediaError> {
            Ok(())
        }

        async fn update(
            &self,
            _update: MediaSessionUpdate,
        ) -> Result<MediaSessionStatus, MediaError> {
            Ok(MediaSessionStatus {
                state: MediaSessionState::Ready,
                audio_enabled: true,
                video_enabled: false,
            })
        }

        async fn stop(&self) -> Result<(), MediaError> {
            if self.stop_calls.fetch_add(1, Ordering::AcqRel) == 0 {
                Err(MediaError::new(
                    MediaErrorKind::Unavailable,
                    "first native stop attempt is unavailable",
                ))
            } else {
                Ok(())
            }
        }

        async fn flush_audio(&self) -> Result<MediaRenderReceipt, MediaError> {
            Ok(MediaRenderReceipt {
                accepted_sample_frames: 0,
                played_sample_frames: 0,
                rendered_at_unix_ms: None,
            })
        }

        async fn interrupt_audio(&self) -> Result<MediaRenderReceipt, MediaError> {
            Ok(MediaRenderReceipt {
                accepted_sample_frames: 0,
                played_sample_frames: 0,
                rendered_at_unix_ms: None,
            })
        }
    }

    struct NoopControl;

    #[async_trait]
    impl RemoteMediaControlChannel for NoopControl {
        async fn playback_receipt(
            &self,
            _receipt: crate::media::RemoteMediaPlaybackReceipt,
        ) -> Result<(), RpcError> {
            Ok(())
        }
    }

    struct PreemptibleFlushDevice {
        flush_started: watch::Sender<bool>,
        interrupted: watch::Sender<bool>,
    }

    #[async_trait]
    impl MediaDeviceSession for PreemptibleFlushDevice {
        async fn next_input(&self) -> Result<Option<media_api::MediaFrame>, MediaError> {
            Ok(None)
        }

        async fn write_audio(
            &self,
            _frame: MediaPcmFrame,
        ) -> Result<MediaRenderReceipt, MediaError> {
            Ok(MediaRenderReceipt {
                accepted_sample_frames: 0,
                played_sample_frames: 0,
                rendered_at_unix_ms: None,
            })
        }

        async fn write_video(&self, _frame: MediaVideoFrame) -> Result<(), MediaError> {
            Ok(())
        }

        async fn update(
            &self,
            _update: MediaSessionUpdate,
        ) -> Result<MediaSessionStatus, MediaError> {
            Ok(MediaSessionStatus {
                state: MediaSessionState::Ready,
                audio_enabled: true,
                video_enabled: false,
            })
        }

        async fn stop(&self) -> Result<(), MediaError> {
            Ok(())
        }

        async fn flush_audio(&self) -> Result<MediaRenderReceipt, MediaError> {
            self.flush_started.send_replace(true);
            let mut interrupted = self.interrupted.subscribe();
            if !*interrupted.borrow_and_update() {
                interrupted.changed().await.map_err(|_| {
                    MediaError::new(MediaErrorKind::Unavailable, "interrupt signal closed")
                })?;
            }
            Err(MediaError::new(
                MediaErrorKind::Cancelled,
                "flush was interrupted before playback completed",
            ))
        }

        async fn interrupt_audio(&self) -> Result<MediaRenderReceipt, MediaError> {
            self.interrupted.send_replace(true);
            Ok(MediaRenderReceipt {
                accepted_sample_frames: 0,
                played_sample_frames: 480,
                rendered_at_unix_ms: Some(1_800_000_000_000),
            })
        }
    }

    #[tokio::test]
    async fn native_stop_failure_keeps_the_attached_session_for_retry() {
        const SESSION_ID: &str = "12121212-3434-4567-8899-aaaaaaaaaaaa";
        const MEDIA_ID: &str = "media_0123456789abcdef";
        let owner = Arc::new(tokio::sync::Mutex::new(HandlerState::default()));
        let device = Arc::new(RetryStopDevice {
            stop_calls: AtomicUsize::new(0),
        });
        let remote_audio =
            MediaSourceRef("remote_01234567-89ab-cdef-0123-456789abcdef_microphone".into());
        let local_audio = MediaSourceRef("media-runtime:microphone".into());
        let offer = RemoteMediaOffer {
            v: 1,
            media_session_ref: MEDIA_ID.to_owned(),
            session_id: SESSION_ID.to_owned(),
            generation: 1,
            mode: RemoteMediaMode::Audio,
            intent: RemoteMediaIntent::Conversation,
            audio_source_ref: Some(remote_audio.0 .0.clone()),
            video_source_ref: None,
            offer_sdp: "bounded offer".to_owned(),
        };
        let answer = RemoteMediaAnswer {
            v: 1,
            media_session_ref: MEDIA_ID.to_owned(),
            answer_sdp: "bounded answer".to_owned(),
        };
        let context = MediaOperationContext::new(
            MediaOperationId::new(1, 1),
            MediaOwner::Session {
                session_id: SESSION_ID.to_owned(),
            },
        );
        let request = MediaSessionRequest {
            intent: MediaSessionIntent::Conversation,
            mode: MediaSessionMode::Audio,
            audio_source: Some(remote_audio),
            video_source: None,
        };
        let key = ("connection-test".to_owned(), MEDIA_ID.to_owned());
        let session = Arc::new(NativeRemoteMediaSession::new(
            key.clone(),
            offer,
            answer,
            context,
            request,
            Some(local_audio),
            None,
            device.clone(),
            None,
            Arc::new(NoopControl),
            None,
            Arc::downgrade(&owner),
        ));
        owner.lock().await.sessions.insert(key, session.clone());

        assert_eq!(
            session.stop_internal().await.unwrap_err().kind,
            MediaErrorKind::Unavailable,
        );
        assert_eq!(session.status().state, MediaSessionState::Failed);
        assert!(!session.stopped.load(Ordering::Acquire));
        assert!(owner.lock().await.sessions.contains_key(&session.key));

        session.stop_internal().await.unwrap();
        assert_eq!(session.status().state, MediaSessionState::Ended);
        assert!(session.stopped.load(Ordering::Acquire));
        let owner_state = owner.lock().await;
        assert!(!owner_state.sessions.contains_key(&session.key));
        let cached_close = owner_state
            .closed_control_results
            .get(&session.key)
            .expect("successful native cleanup retains a correlated close acknowledgement");
        cached_close
            .validate_for(&RemoteMediaControlRequest {
                v: 1,
                media_session_ref: MEDIA_ID.to_owned(),
                generation: 1,
                action: RemoteMediaControlAction::Close,
                target_audio_sequence: 0,
                target_sample_frames: 0,
            })
            .unwrap();
        drop(owner_state);
        assert_eq!(device.stop_calls.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn native_interrupt_preempts_a_pending_flush_without_a_false_completion() {
        const SESSION_ID: &str = "12121212-3434-4567-8899-aaaaaaaaaaaa";
        const MEDIA_ID: &str = "media_0123456789abcdef";
        let owner = Arc::new(tokio::sync::Mutex::new(HandlerState::default()));
        let (flush_started, _) = watch::channel(false);
        let (interrupted, _) = watch::channel(false);
        let device = Arc::new(PreemptibleFlushDevice {
            flush_started,
            interrupted,
        });
        let remote_audio =
            MediaSourceRef("remote_01234567-89ab-cdef-0123-456789abcdef_microphone".into());
        let offer = RemoteMediaOffer {
            v: 1,
            media_session_ref: MEDIA_ID.to_owned(),
            session_id: SESSION_ID.to_owned(),
            generation: 1,
            mode: RemoteMediaMode::Audio,
            intent: RemoteMediaIntent::Conversation,
            audio_source_ref: Some(remote_audio.0 .0.clone()),
            video_source_ref: None,
            offer_sdp: "bounded offer".to_owned(),
        };
        let answer = RemoteMediaAnswer {
            v: 1,
            media_session_ref: MEDIA_ID.to_owned(),
            answer_sdp: "bounded answer".to_owned(),
        };
        let context = MediaOperationContext::new(
            MediaOperationId::new(1, 1),
            MediaOwner::Session {
                session_id: SESSION_ID.to_owned(),
            },
        );
        let request = MediaSessionRequest {
            intent: MediaSessionIntent::Conversation,
            mode: MediaSessionMode::Audio,
            audio_source: Some(remote_audio),
            video_source: None,
        };
        let session = Arc::new(NativeRemoteMediaSession::new(
            ("connection-test".to_owned(), MEDIA_ID.to_owned()),
            offer,
            answer,
            context,
            request,
            Some(MediaSourceRef("media-runtime:microphone".into())),
            None,
            device.clone(),
            None,
            Arc::new(NoopControl),
            None,
            Arc::downgrade(&owner),
        ));
        session.accepted_sample_frames.store(960, Ordering::Release);
        session.progress_tx.send_replace(AudioProgress {
            sequence: 1,
            accepted: 960,
            played: 0,
            rendered_at: 0,
        });

        let mut flush_started = device.flush_started.subscribe();
        let flush_session = session.clone();
        let flush = tokio::spawn(async move {
            flush_session
                .control(RemoteMediaControlRequest {
                    v: 1,
                    media_session_ref: MEDIA_ID.to_owned(),
                    generation: 1,
                    action: RemoteMediaControlAction::FlushAudio,
                    target_audio_sequence: 1,
                    target_sample_frames: 960,
                })
                .await
        });
        timeout(
            Duration::from_secs(1),
            flush_started.wait_for(|started| *started),
        )
        .await
        .expect("flush must reach the native output barrier")
        .unwrap();

        let interrupt_session = session.clone();
        let interrupt = tokio::spawn(async move {
            interrupt_session
                .control(RemoteMediaControlRequest {
                    v: 1,
                    media_session_ref: MEDIA_ID.to_owned(),
                    generation: 1,
                    action: RemoteMediaControlAction::InterruptAudio,
                    target_audio_sequence: 1,
                    target_sample_frames: 960,
                })
                .await
        });
        let interrupt = timeout(Duration::from_secs(1), interrupt)
            .await
            .expect("interrupt must not queue behind the pending flush")
            .unwrap()
            .unwrap();
        assert_eq!(interrupt.played_sample_frames, 480);

        let flush = flush.await.unwrap();
        assert_eq!(flush.unwrap_err().code, RemoteErrorCode::Closed);
        assert_eq!(session.played_sample_frames.load(Ordering::Acquire), 480);
    }

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[tokio::test]
    async fn timed_out_media_tasks_are_aborted_and_joined_before_cleanup_returns() {
        let dropped = Arc::new(AtomicBool::new(false));
        let task_dropped = dropped.clone();
        let task = tokio::spawn(async move {
            let _drop_flag = DropFlag(task_dropped);
            std::future::pending::<()>().await;
        });
        let mut task = task;
        join_media_task(&mut task, "test media pump").await.unwrap();
        assert!(dropped.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn cancelled_native_pump_join_keeps_handle_for_cleanup_retry() {
        let release = Arc::new(Notify::new());
        let completed = Arc::new(AtomicBool::new(false));
        let task_release = release.clone();
        let task_completed = completed.clone();
        let pump = tokio::spawn(async move {
            task_release.notified().await;
            task_completed.store(true, Ordering::Release);
        });
        let pump_slot = Arc::new(tokio::sync::Mutex::new(Some(pump)));
        let joining = Arc::new(Notify::new());
        let join_slot = pump_slot.clone();
        let join_started = joining.clone();
        let stop_attempt = tokio::spawn(async move {
            let mut slot = join_slot.lock().await;
            if let Some(task) = slot.as_mut() {
                join_started.notify_one();
                let result = join_media_task(task, "test native media pump").await;
                slot.take();
                result
            } else {
                Ok(())
            }
        });
        timeout(Duration::from_secs(1), joining.notified())
            .await
            .expect("first cleanup attempt should reach the pump join");
        tokio::task::yield_now().await;
        stop_attempt.abort();
        assert!(stop_attempt.await.unwrap_err().is_cancelled());
        assert!(pump_slot.lock().await.is_some());
        assert!(!completed.load(Ordering::Acquire));

        release.notify_one();
        let mut slot = pump_slot.lock().await;
        let task = slot
            .as_mut()
            .expect("retry must retain the original pump handle");
        join_media_task(task, "test native media pump")
            .await
            .unwrap();
        slot.take();
        assert!(completed.load(Ordering::Acquire));
        assert!(slot.is_none());
    }
}
