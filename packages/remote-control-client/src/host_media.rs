//! Desktop offerer for approved Remote Control media routes.
//!
//! The connector owns no Firebase state and opens no physical desktop devices.
//! It receives a route-specific authenticated control channel from BridgeServer,
//! negotiates a separate WebRTC peer, and exposes decoded remote audio through
//! the shared MediaAudioChannel.

use crate::{
    encoding::random_id,
    media::{
        parse_remote_media_source_ref, remote_media_source_ref, RemoteMediaConfig,
        RemoteMediaSourceKind, MEDIA_AUDIO_SAMPLE_FRAMES_MAX, MEDIA_AUDIO_SEQUENCE_MAX,
    },
};
use async_trait::async_trait;
use client::protocol::media::{
    MediaConnectionRequestDto, MediaConnectionResponseDto, MediaControlActionDto,
    MediaControlRequestDto, MediaControlResponseDto, MediaErrorKindDto, MediaSessionDescriptionDto,
    MediaSessionDescriptionTypeDto, MediaSessionRefDto, MediaSourceKindDto,
};
use lingxi_webrtc_core::{
    IceConfig, IceServer, RtcSession, SessionConfig, VideoDirection, VideoSource,
};
use lingxi_webrtc_media::{
    ports::DecodedAudioPlayout,
    video_ports::{NativeVideo, RemoteVideoFrame},
    AudioProcessor,
};
use media_api::{
    MediaAudioChannel, MediaConnector, MediaConnectorId, MediaError, MediaErrorKind, MediaEvent,
    MediaEventBatch, MediaEventCursor, MediaEventKind, MediaFrame, MediaFramePayload, MediaKind,
    MediaOperation, MediaOperationContext, MediaOperationId, MediaOperationKind, MediaOwner,
    MediaPcmFrame, MediaRenderReceipt, MediaResourceRef, MediaResourceState, MediaResourceStore,
    MediaResult, MediaService, MediaSession, MediaSessionId, MediaSessionIntent, MediaSessionMode,
    MediaSessionRef, MediaSessionRequest, MediaSessionState, MediaSessionStatus,
    MediaSessionUpdate, MediaSource, MediaSourceKind, MediaSourceRef, MediaSourceUsage,
    MediaVideoFrame,
};
use media_runtime::MediaAttachmentHandle;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot, watch, Mutex as AsyncMutex, Notify, RwLock};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const MAX_ROUTES: usize = 64;
const MAX_SESSIONS_PER_ROUTE: usize = 16;
const MAX_EVENTS: usize = 256;
const MAX_AUDIO_FRAMES: usize = 32;
const AUDIO_RATE_HZ: u32 = 48_000;
const AUDIO_TICK: Duration = Duration::from_millis(5);
const AUDIO_CAPTURE_CHUNK: usize = 256;
const AUDIO_RTP_SAMPLES: u64 = 960;
const VIDEO_OBSERVATION_INTERVAL: Duration = Duration::from_secs(1);
const SERVICE_STOP_IDLE: u8 = 0;
const SERVICE_STOP_UNKNOWN: u8 = 1;
const SERVICE_STOP_CONFIRMED: u8 = 2;

/// Product-owned provider for Desktop's per-session ICE policy.
#[async_trait]
pub trait RemoteMediaHostConfigProvider: Send + Sync {
    /// Resolve transport credentials/policy for this owner and exact approved route.
    async fn resolve(
        &self,
        context: &MediaOperationContext,
        route_id: &str,
        request: &MediaSessionRequest,
    ) -> Result<RemoteMediaConfig, MediaError>;
}

/// BridgeServer adapter for one registered Remote Control route.
#[async_trait]
pub trait RemoteMediaHostControlChannel: Send + Sync {
    /// Offer a private, bounded SDP description to the exact approved viewer route.
    async fn negotiate(
        &self,
        request: MediaConnectionRequestDto,
    ) -> Result<MediaConnectionResponseDto, MediaError>;

    /// Flush or interrupt that route's actual native output path.
    async fn control(
        &self,
        request: MediaControlRequestDto,
    ) -> Result<MediaControlResponseDto, MediaError>;
}

struct Route {
    id: String,
    label: String,
    supported_modes: Vec<MediaSessionMode>,
    source_kinds: Vec<RemoteMediaSourceKind>,
    channel: Arc<dyn RemoteMediaHostControlChannel>,
    media_service: Option<std::sync::Weak<dyn MediaService>>,
    closed: AtomicBool,
    unregistering: AtomicBool,
    sessions: Mutex<HashMap<String, Arc<RemoteHostMediaSession>>>,
}

/// One stable aggregate used by the app's connector router and BridgeServer's
/// host-only route registration handler.
pub struct RemoteMediaConnector {
    config: Arc<dyn RemoteMediaHostConfigProvider>,
    resources: Arc<dyn MediaResourceStore>,
    media_service: std::sync::RwLock<Option<std::sync::Weak<dyn MediaService>>>,
    routes: RwLock<HashMap<MediaOwner, HashMap<String, Arc<Route>>>>,
}

impl RemoteMediaConnector {
    /// Create one product connector over the shared owner-scoped media store.
    #[must_use]
    pub fn new(
        config: Arc<dyn RemoteMediaHostConfigProvider>,
        resources: Arc<dyn MediaResourceStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            resources,
            media_service: std::sync::RwLock::new(None),
            routes: RwLock::new(HashMap::new()),
        })
    }

    /// Bind the shared service facade weakly so connector-owned terminal
    /// cleanup can deregister its attached session without creating a cycle.
    pub fn bind_media_service(&self, service: Arc<dyn MediaService>) {
        *self
            .media_service
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Some(Arc::downgrade(&service));
    }

    /// Register one authenticated viewer route for the trusted Bridge owner.
    /// Re-registration of an identical route is rejected as Busy so stale
    /// control channels cannot silently replace a live peer.
    pub async fn register_route(
        &self,
        owner: MediaOwner,
        connection_id: String,
        label: String,
        supported_modes: Vec<MediaSessionMode>,
        source_kinds: Vec<MediaSourceKindDto>,
        channel: Arc<dyn RemoteMediaHostControlChannel>,
    ) -> Result<(), MediaError> {
        if owner.attachment_id().is_some() {
            return Err(invalid(
                "attachment-scoped routes require their matching scoped media service",
            ));
        }
        self.register_route_inner(
            owner,
            connection_id,
            label,
            supported_modes,
            source_kinds,
            channel,
            None,
        )
        .await
    }

    /// Register an attachment-scoped route with the exact service wrapper that
    /// minted its owner namespace. Session Stop calls then pass through the
    /// same attachment admission/drain fence as media operations.
    pub async fn register_attachment_route(
        &self,
        owner: MediaOwner,
        connection_id: String,
        label: String,
        supported_modes: Vec<MediaSessionMode>,
        source_kinds: Vec<MediaSourceKindDto>,
        channel: Arc<dyn RemoteMediaHostControlChannel>,
        service: Arc<dyn MediaService>,
    ) -> Result<(), MediaError> {
        let Some(owner_attachment) = owner.attachment_id() else {
            return Err(invalid(
                "attachment-scoped route registration requires an attached owner",
            ));
        };
        if service.attachment_id().as_ref() != Some(owner_attachment) {
            return Err(invalid(
                "route owner and media service belong to different attachments",
            ));
        }
        self.register_route_inner(
            owner,
            connection_id,
            label,
            supported_modes,
            source_kinds,
            channel,
            Some(Arc::downgrade(&service)),
        )
        .await
    }

    async fn register_route_inner(
        &self,
        owner: MediaOwner,
        connection_id: String,
        label: String,
        supported_modes: Vec<MediaSessionMode>,
        source_kinds: Vec<MediaSourceKindDto>,
        channel: Arc<dyn RemoteMediaHostControlChannel>,
        media_service: Option<std::sync::Weak<dyn MediaService>>,
    ) -> Result<(), MediaError> {
        if logical_session_id(&owner).is_none()
            || !valid_route_uuid(&connection_id)
            || label.trim().is_empty()
            || label.len() > 256
            || label.chars().any(char::is_control)
            || supported_modes.is_empty()
            || supported_modes.len() > 3
            || source_kinds.is_empty()
            || source_kinds.len() > 3
            || has_duplicates(&supported_modes)
            || has_duplicates(&source_kinds)
        {
            return Err(invalid("media route registration is invalid"));
        }
        let source_kinds = source_kinds
            .into_iter()
            .map(|kind| match kind {
                MediaSourceKindDto::Microphone => Ok(RemoteMediaSourceKind::Microphone),
                MediaSourceKindDto::Camera => Ok(RemoteMediaSourceKind::Camera),
                MediaSourceKindDto::Screen => Ok(RemoteMediaSourceKind::Screen),
                _ => Err(invalid("remote media route source kind is unsupported")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let has_audio = source_kinds.contains(&RemoteMediaSourceKind::Microphone);
        let has_video = source_kinds.iter().any(|kind| {
            matches!(
                kind,
                RemoteMediaSourceKind::Camera | RemoteMediaSourceKind::Screen
            )
        });
        let audio_mode = supported_modes.iter().any(|mode| mode_has_audio(*mode));
        let video_mode = supported_modes.iter().any(|mode| mode_has_video(*mode));
        if audio_mode != has_audio || video_mode != has_video {
            return Err(invalid("media route modes and source inventory disagree"));
        }

        let mut routes = self.routes.write().await;
        let owner_routes = routes.entry(owner.clone()).or_default();
        if owner_routes.len() >= MAX_ROUTES || owner_routes.contains_key(&connection_id) {
            return Err(MediaError::new(
                MediaErrorKind::Busy,
                "media route is already registered or route capacity is full",
            ));
        }
        owner_routes.insert(
            connection_id.clone(),
            Arc::new(Route {
                id: connection_id,
                label,
                supported_modes,
                source_kinds,
                channel,
                media_service,
                closed: AtomicBool::new(false),
                unregistering: AtomicBool::new(false),
                sessions: Mutex::new(HashMap::new()),
            }),
        );
        Ok(())
    }

    /// Remove only the exact owner-bound route and stop its associated peers.
    pub async fn unregister_route(
        &self,
        owner: &MediaOwner,
        connection_id: &str,
    ) -> Result<(), MediaError> {
        self.unregister_route_inner(owner, connection_id, false)
            .await
    }

    /// Remove a route only after the matching attachment's media drain confirms.
    /// The host cleanup fence keeps the attachment Revoking until peer cleanup
    /// also confirms. The media drain has stopped all service-owned resources;
    /// this path closes only the peer transport and never resends MediaService
    /// Stop against a possibly unknown prior Stop result.
    pub async fn unregister_route_after_media_drain(
        &self,
        owner: &MediaOwner,
        connection_id: &str,
        attachment: &MediaAttachmentHandle,
    ) -> Result<(), MediaError> {
        if owner.attachment_id() != Some(attachment.id()) || !attachment.media_drain_confirmed() {
            return Err(invalid(
                "route cleanup requires confirmation for its exact media attachment",
            ));
        }
        self.unregister_route_inner(owner, connection_id, true)
            .await
    }

    async fn unregister_route_inner(
        &self,
        owner: &MediaOwner,
        connection_id: &str,
        attachment_media_drained: bool,
    ) -> Result<(), MediaError> {
        let route = self
            .routes
            .read()
            .await
            .get(owner)
            .and_then(|owner_routes| owner_routes.get(connection_id))
            .cloned();
        let Some(route) = route else {
            return Ok(());
        };
        route.unregistering.store(true, Ordering::Release);
        let sessions = route
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for session in sessions {
            let stopped = if attachment_media_drained {
                session.stop_after_media_drain().await
            } else {
                session.stop_from_service().await
            };
            if let Err(error) = stopped {
                return Err(error);
            }
        }
        let mut routes = self.routes.write().await;
        if let Some(owner_routes) = routes.get_mut(owner) {
            if owner_routes
                .get(connection_id)
                .is_some_and(|current| Arc::ptr_eq(current, &route))
            {
                owner_routes.remove(connection_id);
                route.closed.store(true, Ordering::Release);
            }
            if owner_routes.is_empty() {
                routes.remove(owner);
            }
        }
        Ok(())
    }

    /// Modes currently available from routes registered by this owner.
    /// The app host unions this with its LocalMediaConnector modes.
    pub async fn supported_modes(&self, owner: &MediaOwner) -> Vec<MediaSessionMode> {
        let routes = self.routes.read().await;
        let mut modes = Vec::new();
        for route in routes
            .get(owner)
            .into_iter()
            .flatten()
            .map(|(_, route)| route)
        {
            if route.unregistering.load(Ordering::Acquire) || route.closed.load(Ordering::Acquire) {
                continue;
            }
            for mode in &route.supported_modes {
                if !modes.contains(mode) {
                    modes.push(*mode);
                }
            }
        }
        modes
    }

    async fn route_for_sources(
        &self,
        owner: &MediaOwner,
        request: &MediaSessionRequest,
    ) -> Result<Arc<Route>, MediaError> {
        let route_id = selected_route_id(request)?;
        let routes = self.routes.read().await;
        let route = routes
            .get(owner)
            .and_then(|routes| routes.get(route_id))
            .cloned()
            .ok_or_else(|| {
                MediaError::new(
                    MediaErrorKind::NotFound,
                    "selected remote media route is unavailable",
                )
            })?;
        if route.closed.load(Ordering::Acquire)
            || route.unregistering.load(Ordering::Acquire)
            || !route.supported_modes.contains(&request.mode)
        {
            return Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "selected remote route does not support this media mode",
            ));
        }
        for source in request
            .audio_source
            .iter()
            .chain(request.video_source.iter())
        {
            let Some((source_route, kind)) = parse_remote_media_source_ref(&source.0 .0) else {
                return Err(invalid("selected remote media source reference is invalid"));
            };
            if source_route != route.id || !route.source_kinds.contains(&kind) {
                return Err(MediaError::new(
                    MediaErrorKind::NotFound,
                    "selected remote media source is not owned by this route",
                ));
            }
        }
        Ok(route)
    }
}

#[async_trait]
impl MediaConnector for RemoteMediaConnector {
    fn id(&self) -> MediaConnectorId {
        MediaConnectorId("lingxi.remote-control".into())
    }

    async fn sources(
        &self,
        context: MediaOperationContext,
    ) -> Result<Vec<MediaSource>, MediaError> {
        let routes = self.routes.read().await;
        let mut sources = Vec::new();
        for route in routes
            .get(&context.owner)
            .into_iter()
            .flatten()
            .map(|(_, route)| route)
        {
            if route.closed.load(Ordering::Acquire) || route.unregistering.load(Ordering::Acquire) {
                continue;
            }
            for kind in &route.source_kinds {
                let reference = remote_media_source_ref(&route.id, *kind)
                    .map_err(|_| invalid("registered media route identity is invalid"))?;
                let (media_kind, label) = match kind {
                    RemoteMediaSourceKind::Microphone => (MediaKind::Audio, "microphone"),
                    RemoteMediaSourceKind::Camera => (MediaKind::Video, "camera"),
                    RemoteMediaSourceKind::Screen => (MediaKind::Video, "screen"),
                };
                sources.push(MediaSource {
                    reference: MediaSourceRef(reference.into()),
                    kind: MediaSourceKind::Remote,
                    media_kind: Some(media_kind),
                    label: bounded_label(&route.label, label),
                    usages: vec![MediaSourceUsage::Session],
                    supported_operations: vec![MediaOperationKind::SessionStart],
                    is_default: false,
                });
            }
        }
        if sources.len() > 256 {
            return Err(MediaError::new(
                MediaErrorKind::MediaTooLarge,
                "remote source catalog exceeds its limit",
            ));
        }
        Ok(sources)
    }

    async fn start_session(
        &self,
        context: MediaOperationContext,
        request: MediaSessionRequest,
    ) -> Result<Box<dyn MediaSession>, MediaError> {
        let Some(owner_session_id) = logical_session_id(&context.owner).map(str::to_owned) else {
            return Err(MediaError::new(
                MediaErrorKind::InvalidRequest,
                "Remote Control media requires a session owner",
            ));
        };
        validate_selected_sources(&request)?;
        let route = self.route_for_sources(&context.owner, &request).await?;
        let config = self.config.resolve(&context, &route.id, &request).await?;
        config
            .validate()
            .map_err(|_| invalid("media transport configuration is invalid"))?;
        config.validate_supported_transport().map_err(|_| {
            MediaError::new(
                MediaErrorKind::Unsupported,
                "configured ICE transport is unsupported",
            )
        })?;
        lingxi_webrtc_media::runtime::initialize_configured(mode_has_video(request.mode)).map_err(
            |_| {
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "media codec runtime is unavailable",
                )
            },
        )?;
        if route.closed.load(Ordering::Acquire) || route.unregistering.load(Ordering::Acquire) {
            return Err(MediaError::new(
                MediaErrorKind::Cancelled,
                "media route was removed",
            ));
        }

        let reference = MediaSessionRef(MediaSessionId::from(format!("media_{}", random_id())));
        let generation = context.identity.generation.max(1);
        let session_ref = MediaSessionRefDto {
            id: reference.0 .0.clone(),
        };
        let ice = IceConfig {
            servers: config
                .ice_servers
                .iter()
                .map(|server| IceServer {
                    urls: server.urls.clone(),
                    username: server.username.clone().unwrap_or_default(),
                    credential: server.credential.clone().unwrap_or_default(),
                })
                .collect(),
            relay_only: config.relay_only,
        };
        let mut rtc = RtcSession::with_config(SessionConfig {
            audio: mode_has_audio(request.mode),
            video: mode_has_video(request.mode),
            video_direction: VideoDirection::ReceiveOnly,
            data_channel: false,
            answerer: false,
            ice,
            gather_timeout: Some(Duration::from_millis(config.gather_timeout_ms)),
            ..SessionConfig::default()
        })
        .await
        .map_err(native_media_error)?;
        if mode_has_audio(request.mode) {
            // Incoming RTP is intentionally suppressed by the core until the
            // product installs a live decoder/playout consumer. This session
            // pump is that consumer, so enable its bounded ingress queue before
            // sending the offer and receiving the answer.
            rtc.set_playback_suppressed(false)
                .map_err(native_media_error)?;
        }
        let offer_sdp = match rtc.offer().await {
            Ok(sdp) if sdp.len() <= client::protocol::media::MAX_REMOTE_MEDIA_SDP_BYTES => sdp,
            Ok(_) => {
                let _ = rtc.close().await;
                return Err(MediaError::new(
                    MediaErrorKind::MediaTooLarge,
                    "media offer exceeds its size limit",
                ));
            }
            Err(error) => {
                let _ = rtc.close().await;
                return Err(native_media_error(error));
            }
        };
        let offer_request = MediaConnectionRequestDto {
            request_id: random_id(),
            connection_id: route.id.clone(),
            session_id: owner_session_id,
            media_session_ref: session_ref,
            operation_id: dto_operation_id(&context.identity),
            owner: dto_owner(&context.owner)?,
            mode: dto_mode(request.mode),
            intent: dto_intent(request.intent),
            audio_source: request.audio_source.as_ref().map(dto_source_ref),
            video_source: request.video_source.as_ref().map(dto_source_ref),
            description: MediaSessionDescriptionDto {
                kind: MediaSessionDescriptionTypeDto::Offer,
                sdp: offer_sdp,
            },
        };
        offer_request
            .validate()
            .map_err(|_| invalid("media offer request is invalid"))?;
        let response = match route.channel.negotiate(offer_request.clone()).await {
            Ok(response) => response,
            Err(error) => {
                let _ = rtc.close().await;
                return Err(error);
            }
        };
        response
            .validate()
            .map_err(|_| invalid("media answer response is invalid"))?;
        if !response.matches_request(&offer_request) {
            let _ = rtc.close().await;
            return Err(invalid("media answer did not match the pending offer"));
        }
        if let Some(error) = response.error {
            let _ = rtc.close().await;
            return Err(dto_error(error.kind, error.message));
        }
        let answer = response
            .description
            .ok_or_else(|| invalid("media answer is missing"))?;
        if answer.kind != MediaSessionDescriptionTypeDto::Answer {
            let _ = rtc.close().await;
            return Err(invalid("media connection response was not an Answer"));
        }
        if let Err(error) = rtc.apply_answer(answer.sdp).await {
            let _ = rtc.close().await;
            return Err(native_media_error(error));
        }
        if route.closed.load(Ordering::Acquire) || route.unregistering.load(Ordering::Acquire) {
            let _ = rtc.close().await;
            return Err(MediaError::new(
                MediaErrorKind::Cancelled,
                "media route was removed",
            ));
        }

        let session = RemoteHostMediaSession::new(
            reference,
            context.clone(),
            request,
            route.clone(),
            generation,
            self.resources.clone(),
            route.media_service.clone().or_else(|| {
                self.media_service
                    .read()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone()
            }),
        );
        let route_full = {
            let mut sessions = route
                .sessions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if sessions.len() >= MAX_SESSIONS_PER_ROUTE {
                true
            } else {
                sessions.insert(session.reference.0 .0.clone(), session.clone());
                false
            }
        };
        if route_full {
            session.stop_internal().await?;
            return Err(MediaError::new(
                MediaErrorKind::Busy,
                "remote route has too many active media sessions",
            ));
        }
        if let Err(error) = session.start(rtc, config.connect_timeout_ms).await {
            session.stop_internal().await?;
            return Err(error);
        }
        Ok(Box::new(RemoteHostMediaSessionHandle(session)))
    }
}

fn validate_selected_sources(request: &MediaSessionRequest) -> Result<(), MediaError> {
    if mode_has_audio(request.mode) {
        let source = request
            .audio_source
            .as_ref()
            .ok_or_else(|| invalid("audio session requires a selected remote microphone"))?;
        if !matches!(
            parse_remote_media_source_ref(&source.0 .0),
            Some((_, RemoteMediaSourceKind::Microphone))
        ) {
            return Err(invalid("audio source is not a remote microphone"));
        }
    } else if request.audio_source.is_some() {
        return Err(invalid(
            "audio source supplied for an audio-disabled session",
        ));
    }
    if mode_has_video(request.mode) {
        let source = request
            .video_source
            .as_ref()
            .ok_or_else(|| invalid("video session requires a selected remote camera or screen"))?;
        if !matches!(
            parse_remote_media_source_ref(&source.0 .0),
            Some((
                _,
                RemoteMediaSourceKind::Camera | RemoteMediaSourceKind::Screen
            ))
        ) {
            return Err(invalid("video source is not a remote camera or screen"));
        }
    } else if request.video_source.is_some() {
        return Err(invalid(
            "video source supplied for a video-disabled session",
        ));
    }
    Ok(())
}

struct RemoteHostMediaSession {
    reference: MediaSessionRef,
    context: MediaOperationContext,
    request: MediaSessionRequest,
    route: Arc<Route>,
    generation: u64,
    resources: Arc<dyn MediaResourceStore>,
    media_service: Option<std::sync::Weak<dyn MediaService>>,
    status_tx: watch::Sender<MediaSessionStatus>,
    audio_channel: Option<Arc<RemoteHostMediaAudioChannel>>,
    audio_commands: AsyncMutex<Option<mpsc::Receiver<WorkerAudioCommand>>>,
    audio_frames_tx: Option<mpsc::Sender<MediaFrame>>,
    cancel: CancellationToken,
    pump: AsyncMutex<Option<JoinHandle<()>>>,
    event_sequence: AtomicU64,
    events: Mutex<VecDeque<MediaEvent>>,
    events_dropped: AtomicBool,
    event_notify: Notify,
    video_observation_last: Mutex<Option<Instant>>,
    enabled_audio: Arc<AtomicBool>,
    enabled_video: Arc<AtomicBool>,
    started: AtomicBool,
    stopping: Arc<AtomicBool>,
    stopped: AtomicBool,
    close_acknowledged: AtomicBool,
    stop_gate: AsyncMutex<()>,
    service_registered: AtomicBool,
    service_stop_state: Arc<AtomicU8>,
    next_service_stop_generation: AtomicU64,
}

/// The runtime owns a trait object while the route map retains an Arc until
/// remote device cleanup has been acknowledged.
struct RemoteHostMediaSessionHandle(Arc<RemoteHostMediaSession>);

struct RemoteHostMediaAudioChannel {
    commands: mpsc::Sender<WorkerAudioCommand>,
    input: AsyncMutex<mpsc::Receiver<MediaFrame>>,
    request_gate: AsyncMutex<()>,
    context: MediaOperationContext,
    reference: MediaSessionRef,
    generation: u64,
    route: Arc<Route>,
    enabled_audio: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    /// Interrupt/close invalidates an earlier flush without waiting for the
    /// viewer's output drain to finish.
    control_epoch: AtomicU64,
    last_barrier_target: Mutex<AudioTarget>,
    last_completed_target: Mutex<AudioTarget>,
    source_geometry: Mutex<Option<AudioSourceGeometry>>,
    accepted_source_sample_frames: AtomicU64,
    played_rtp_sample_frames: AtomicU64,
    played_source_sample_frames: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AudioSourceGeometry {
    sample_rate_hz: u32,
    channels: u16,
}

enum WorkerAudioCommand {
    Write {
        frame: MediaPcmFrame,
        reply: oneshot::Sender<Result<MediaRenderReceipt, MediaError>>,
    },
    Flush {
        reply: oneshot::Sender<Result<AudioTarget, MediaError>>,
    },
    Interrupt {
        reply: oneshot::Sender<Result<AudioTarget, MediaError>>,
    },
}

#[derive(Debug, Clone, Copy)]
struct AudioTarget {
    sequence: u64,
    sample_frames: u64,
}

fn rtp_sample_frames_to_source_frames(
    rtp_sample_frames: u64,
    source_rate_hz: u32,
    accepted_source_sample_frames: u64,
) -> u64 {
    u64::try_from(
        u128::from(rtp_sample_frames) * u128::from(source_rate_hz) / u128::from(AUDIO_RATE_HZ),
    )
    .unwrap_or(u64::MAX)
    .min(accepted_source_sample_frames)
}

#[derive(Default)]
struct AudioEncoderState {
    processor: Option<(u32, AudioProcessor)>,
    pending_sample_frames: u64,
    sequence: u64,
}

#[async_trait]
impl MediaSession for RemoteHostMediaSessionHandle {
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

    async fn audio_channel(&self) -> Result<Arc<dyn MediaAudioChannel>, MediaError> {
        self.0.audio_channel().await
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

impl RemoteHostMediaSession {
    fn new(
        reference: MediaSessionRef,
        context: MediaOperationContext,
        request: MediaSessionRequest,
        route: Arc<Route>,
        generation: u64,
        resources: Arc<dyn MediaResourceStore>,
        media_service: Option<std::sync::Weak<dyn MediaService>>,
    ) -> Arc<Self> {
        let mode = request.mode;
        let initial_status = MediaSessionStatus {
            state: MediaSessionState::Preparing,
            audio_enabled: mode_has_audio(mode),
            video_enabled: mode_has_video(mode),
        };
        let (status_tx, _) = watch::channel(initial_status);
        let enabled_audio = Arc::new(AtomicBool::new(mode_has_audio(mode)));
        let enabled_video = Arc::new(AtomicBool::new(mode_has_video(mode)));
        let stopping = Arc::new(AtomicBool::new(false));
        let (commands_tx, commands_rx) = if mode_has_audio(mode) {
            let (tx, rx) = mpsc::channel(16);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let (frames_tx, frames_rx) = if mode_has_audio(mode) {
            let (tx, rx) = mpsc::channel(MAX_AUDIO_FRAMES);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let next_service_stop_generation = context.identity.generation.saturating_add(1).max(1);
        let audio_channel = commands_tx.zip(frames_rx).map(|(commands, input)| {
            Arc::new(RemoteHostMediaAudioChannel {
                commands,
                input: AsyncMutex::new(input),
                request_gate: AsyncMutex::new(()),
                context: context.clone(),
                reference: reference.clone(),
                generation,
                route: route.clone(),
                enabled_audio: enabled_audio.clone(),
                stopping: stopping.clone(),
                control_epoch: AtomicU64::new(0),
                last_barrier_target: Mutex::new(AudioTarget {
                    sequence: 0,
                    sample_frames: 0,
                }),
                last_completed_target: Mutex::new(AudioTarget {
                    sequence: 0,
                    sample_frames: 0,
                }),
                source_geometry: Mutex::new(None),
                accepted_source_sample_frames: AtomicU64::new(0),
                played_rtp_sample_frames: AtomicU64::new(0),
                played_source_sample_frames: AtomicU64::new(0),
            })
        });
        Arc::new(Self {
            reference,
            context,
            request,
            route,
            generation,
            resources,
            media_service,
            status_tx,
            audio_channel,
            audio_commands: AsyncMutex::new(commands_rx),
            audio_frames_tx: frames_tx,
            cancel: CancellationToken::new(),
            pump: AsyncMutex::new(None),
            event_sequence: AtomicU64::new(0),
            events: Mutex::new(VecDeque::new()),
            events_dropped: AtomicBool::new(false),
            event_notify: Notify::new(),
            video_observation_last: Mutex::new(None),
            enabled_audio,
            enabled_video,
            started: AtomicBool::new(false),
            stopping,
            stopped: AtomicBool::new(false),
            close_acknowledged: AtomicBool::new(false),
            stop_gate: AsyncMutex::new(()),
            service_registered: AtomicBool::new(false),
            service_stop_state: Arc::new(AtomicU8::new(SERVICE_STOP_IDLE)),
            next_service_stop_generation: AtomicU64::new(next_service_stop_generation),
        })
    }

    async fn start(
        self: &Arc<Self>,
        mut rtc: RtcSession,
        connect_timeout_ms: u64,
    ) -> Result<(), MediaError> {
        if self.started.swap(true, Ordering::AcqRel) {
            let _ = rtc.close().await;
            return Err(MediaError::new(
                MediaErrorKind::Busy,
                "remote media session was already started",
            ));
        }
        if self.route.closed.load(Ordering::Acquire)
            || self.route.unregistering.load(Ordering::Acquire)
            || self.stopped.load(Ordering::Acquire)
        {
            let _ = rtc.close().await;
            return Err(MediaError::new(
                MediaErrorKind::Cancelled,
                "media route was removed",
            ));
        }
        let commands = self.audio_commands.lock().await.take();
        let session = self.clone();
        let cancel = self.cancel.child_token();
        let pump = tokio::spawn(async move {
            let result = session
                .run_pump(&mut rtc, commands, connect_timeout_ms, cancel.clone())
                .await;
            if let Err(error) = result {
                session.set_state(MediaSessionState::Failed);
                tracing::debug!(kind = error.kind.to_string(), "remote media peer stopped");
                cancel.cancel();
            }
            let _ = rtc.close().await;
            if !session.stopped.load(Ordering::Acquire) && !session.stopping.load(Ordering::Acquire)
            {
                let cleanup = session.clone();
                tokio::spawn(async move {
                    if let Err(error) = cleanup.stop_internal().await {
                        tracing::debug!(
                            kind = error.kind.to_string(),
                            "remote media cleanup needs retry"
                        );
                    }
                });
            }
        });
        *self.pump.lock().await = Some(pump);
        Ok(())
    }

    async fn run_pump(
        self: &Arc<Self>,
        rtc: &mut RtcSession,
        mut audio_commands: Option<mpsc::Receiver<WorkerAudioCommand>>,
        connect_timeout_ms: u64,
        cancel: CancellationToken,
    ) -> Result<(), MediaError> {
        let audio_source = self.request.audio_source.clone();
        let video_source = self.request.video_source.clone();
        let audio_time_anchor = Instant::now();
        let wall_time_anchor = unix_now_ms();
        let mut playout = if mode_has_audio(self.request.mode) {
            Some(DecodedAudioPlayout::new(AUDIO_RATE_HZ).map_err(native_media_error)?)
        } else {
            None
        };
        let video_wall_monotonic_anchor = Instant::now();
        let video_wall_anchor = unix_now_ms();
        let mut video = if mode_has_video(self.request.mode) {
            let (mut video, _, _) = NativeVideo::new();
            video
                .controls(false, false, true)
                .map_err(native_media_error)?;
            Some(video)
        } else {
            None
        };
        let video_sources = if mode_has_video(self.request.mode) {
            rtc.remote_video_sources()
                .await
                .map_err(native_media_error)?
        } else {
            HashMap::new()
        };
        let mut encoder = AudioEncoderState::default();
        let mut last_audio_sequence = None;
        let mut connection_state = rtc.subscribe_connection_state();
        let mut interval = tokio::time::interval(AUDIO_TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let connect_deadline =
            tokio::time::Instant::now() + Duration::from_millis(connect_timeout_ms);

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep_until(connect_deadline), if !self.connected_once() => {
                    return Err(MediaError::new(MediaErrorKind::Timeout, "remote media transport did not connect in time"));
                }
                state_changed = connection_state.changed() => {
                    if state_changed.is_err() { break; }
                    let state = *connection_state.borrow_and_update();
                    self.set_peer_state(state);
                    if state == webrtc::peer_connection::RTCPeerConnectionState::Failed {
                        return Err(MediaError::new(MediaErrorKind::NativeFailure, "remote media peer connection failed"));
                    }
                    if state == webrtc::peer_connection::RTCPeerConnectionState::Closed {
                        break;
                    }
                }
                command = async {
                    match audio_commands.as_mut() {
                        Some(commands) => commands.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let Some(command) = command else { audio_commands = None; continue; };
                    match command {
                        WorkerAudioCommand::Write { frame, reply } => {
                            let result = encode_audio(rtc, &mut encoder, frame, self.context.max_payload_bytes).await;
                            let _ = reply.send(result);
                        }
                        WorkerAudioCommand::Flush { reply } => {
                            let result = flush_encoder(rtc, &mut encoder).await;
                            let _ = reply.send(result);
                        }
                        WorkerAudioCommand::Interrupt { reply } => {
                            let result = interrupt_encoder(&mut encoder);
                            let _ = reply.send(Ok(result));
                        }
                    }
                }
                _ = interval.tick() => {
                    if let Some(playout) = playout.as_mut() {
                        for _ in 0..64 {
                            let packet = rtc.take_audio_packet().map_err(native_media_error)?;
                            let Some(packet) = packet else { break; };
                            playout.push(packet).map_err(native_media_error)?;
                        }
                        for _ in 0..8 {
                            let Some(frame) = playout.try_read().map_err(native_media_error)? else { break; };
                            if !self.enabled_audio.load(Ordering::Acquire) { continue; }
                            if last_audio_sequence.is_some_and(|previous| frame.sequence <= previous) {
                                return Err(invalid("remote audio sequence did not advance"));
                            }
                            last_audio_sequence = Some(frame.sequence);
                            let Some(source) = audio_source.clone() else {
                                return Err(invalid("remote audio source is unavailable"));
                            };
                            let media_frame = MediaFrame {
                                source,
                                sequence: frame.sequence,
                                // RTP does not carry the device's absolute capture clock. Preserve
                                // the decoded presentation/receive time instead of stamping decode-now.
                                captured_at_unix_ms: monotonic_to_unix_ms(
                                    audio_time_anchor,
                                    wall_time_anchor,
                                    frame.at,
                                ),
                                payload: MediaFramePayload::Audio(MediaPcmFrame {
                                    sample_rate_hz: frame.sample_rate_hz,
                                    channels: frame.channels,
                                    pcm16le: frame.pcm16le,
                                }),
                            };
                            if let Some(sender) = &self.audio_frames_tx {
                                sender.try_send(media_frame).map_err(|_| MediaError::new(MediaErrorKind::Busy, "remote audio reader queue is full"))?;
                            }
                        }
                    }
                    if let Some(video) = video.as_mut() {
                        video.controls(false, false, self.enabled_video.load(Ordering::Acquire)).map_err(native_media_error)?;
                        let frames = video.service(rtc).await.map_err(native_media_error)?;
                        for remote in frames {
                            let kind = video_sources.get(&remote.ssrc).copied().ok_or_else(|| invalid("remote video source is not in the negotiated SDP"))?;
                            let selected_kind = match parse_remote_media_source_ref(video_source.as_ref().map(|source| source.0.0.as_str()).unwrap_or_default()) {
                                Some((_, RemoteMediaSourceKind::Camera)) => VideoSource::Camera,
                                Some((_, RemoteMediaSourceKind::Screen)) => VideoSource::Screen,
                                _ => return Err(invalid("remote video source reference is invalid")),
                            };
                            if kind != selected_kind { return Err(invalid("remote video frame came from an unselected source")); }
                            self.store_video_frame(
                                remote,
                                video_source.clone().ok_or_else(|| invalid("remote video source is unavailable"))?,
                                video_wall_monotonic_anchor,
                                video_wall_anchor,
                            ).await?;
                        }
                    }
                }
            }
        }
        if let Some(playout) = playout.as_mut() {
            playout.interrupt().map_err(native_media_error)?;
        }
        Ok(())
    }

    async fn store_video_frame(
        &self,
        remote: RemoteVideoFrame,
        source: MediaSourceRef,
        wall_monotonic_anchor: Instant,
        wall_anchor_unix_ms: u64,
    ) -> Result<(), MediaError> {
        let presented_at = remote.at;
        {
            let mut last = self
                .video_observation_last
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !should_sample_video_frame(&mut last, presented_at) {
                return Ok(());
            }
        }
        if remote.frame.layout.width == 0 || remote.frame.layout.height == 0 {
            return Err(invalid("remote video frame has invalid geometry"));
        }
        let video_frame = decoded_video_frame(remote.frame)?;
        let timestamp =
            monotonic_to_unix_ms(wall_monotonic_anchor, wall_anchor_unix_ms, presented_at);
        let payload = media_runtime::local::encode_video_payload(
            &video_frame,
            timestamp,
            self.context.max_payload_bytes,
        )?;
        let asset = self.resources.put(&self.context, payload.clone()).await?;
        if asset.metadata != payload.metadata {
            let _ = self.resources.delete(&self.context, &asset.reference).await;
            return Err(MediaError::new(
                MediaErrorKind::StoreFailure,
                "media store returned mismatched video metadata",
            ));
        }
        self.push_event(MediaEventKind::Frame { source, asset }, timestamp);
        Ok(())
    }

    async fn stop_from_service(&self) -> Result<(), MediaError> {
        let Some(service) = self
            .media_service
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
        else {
            return self.stop_internal().await;
        };
        // Route cleanup must use the same logical owner through this route's
        // exact attachment-scoped service wrapper. The wrapper mints the
        // attachment namespace and tracks the Stop against its revoke fence.
        let owner = self.context.owner.logical_owner().clone();
        let capabilities = service.capabilities_for(&owner);
        if !capabilities.supports(MediaOperationKind::Stop) {
            return self.stop_internal().await;
        }
        match self.service_stop_state.load(Ordering::Acquire) {
            SERVICE_STOP_CONFIRMED => return self.stop_internal().await,
            SERVICE_STOP_UNKNOWN => {
                return Err(MediaError::new(
                    MediaErrorKind::Unavailable,
                    "remote media service Stop has an unconfirmed result; only attachment revoke can recover it",
                ));
            }
            SERVICE_STOP_IDLE => {}
            _ => {
                return Err(MediaError::new(
                    MediaErrorKind::NativeFailure,
                    "remote media service Stop state is invalid",
                ));
            }
        }
        if self
            .service_stop_state
            .compare_exchange(
                SERVICE_STOP_IDLE,
                SERVICE_STOP_UNKNOWN,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "another remote media service Stop has an unconfirmed result",
            ));
        }
        let generation = self
            .next_service_stop_generation
            .fetch_add(1, Ordering::Relaxed)
            .max(1);
        let context = MediaOperationContext::new(
            MediaOperationId::new(generation, capabilities.service_epoch),
            owner,
        )
        .with_timeout_budget_ms(5_000)
        .with_max_payload_bytes(
            self.context
                .max_payload_bytes
                .min(capabilities.max_payload_bytes),
        );
        match service
            .execute(
                context,
                MediaOperation::Stop {
                    target: MediaResourceRef::Session(self.reference.clone()),
                },
            )
            .await
        {
            Ok(MediaResult::Stopped { resource })
                if resource == MediaResourceRef::Session(self.reference.clone()) =>
            {
                self.service_stop_state
                    .store(SERVICE_STOP_CONFIRMED, Ordering::Release);
                self.stop_internal().await
            }
            Err(error) if error.kind == MediaErrorKind::NotFound => {
                self.service_stop_state
                    .store(SERVICE_STOP_CONFIRMED, Ordering::Release);
                self.stop_internal().await
            }
            Ok(_) => Err(MediaError::new(
                MediaErrorKind::NativeFailure,
                "media service did not confirm stopping the expected remote session",
            )),
            Err(error) => Err(error),
        }
    }

    async fn stop_after_media_drain(&self) -> Result<(), MediaError> {
        // The attachment's media drain receipt is the service cleanup evidence.
        // Do not schedule another MediaService Stop after its admission closed;
        // only retry the route's independently acknowledged peer close.
        self.service_registered.store(false, Ordering::Release);
        self.stop_internal().await
    }

    async fn stop_internal(&self) -> Result<(), MediaError> {
        let _stop_guard = self.stop_gate.lock().await;
        if self.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        self.stopping.store(true, Ordering::Release);
        if let Some(audio_channel) = &self.audio_channel {
            let _gate = audio_channel.request_gate.lock().await;
            audio_channel.control_epoch.fetch_add(1, Ordering::AcqRel);
        }

        // A local stop is not complete until the native peer confirms that its
        // device lease has been interrupted and released. Keep the RTC pump
        // alive if this acknowledgement fails so callers can retry safely.
        if !self.close_acknowledged.load(Ordering::Acquire) {
            let request = MediaControlRequestDto {
                request_id: random_id(),
                connection_id: self.route.id.clone(),
                media_session_ref: MediaSessionRefDto {
                    id: self.reference.0 .0.clone(),
                },
                generation: self.generation,
                action: MediaControlActionDto::Close,
            };
            request
                .validate()
                .map_err(|_| invalid("media close request is invalid"))?;
            let response = match self.route.channel.control(request.clone()).await {
                Ok(response) => response,
                Err(error) => {
                    self.set_state(MediaSessionState::Failed);
                    return Err(error);
                }
            };
            if let Err(error) = validate_close_response(&request, response) {
                self.set_state(MediaSessionState::Failed);
                return Err(error);
            }
            self.close_acknowledged.store(true, Ordering::Release);
        }

        self.cancel.cancel();
        let mut pump = self.pump.lock().await;
        if let Some(task) = pump.as_mut() {
            let result = join_media_task(task, "remote media pump").await;
            pump.take();
            if let Err(error) = result {
                self.set_state(MediaSessionState::Failed);
                return Err(error);
            }
        }
        drop(pump);
        let mut sessions = self
            .route
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if sessions.get(&self.reference.0 .0).is_some_and(|session| {
            session.reference == self.reference && session.generation == self.generation
        }) {
            sessions.remove(&self.reference.0 .0);
        }
        self.stopped.store(true, Ordering::Release);
        self.set_state(MediaSessionState::Ended);
        drop(sessions);
        drop(_stop_guard);
        self.schedule_service_deregistration();
        Ok(())
    }

    fn schedule_service_deregistration(&self) {
        if !self.service_registered.load(Ordering::Acquire) || !self.stopped.load(Ordering::Acquire)
        {
            return;
        }
        let Some(service) = self
            .media_service
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
        else {
            return;
        };
        if self
            .service_stop_state
            .compare_exchange(
                SERVICE_STOP_IDLE,
                SERVICE_STOP_UNKNOWN,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return;
        }
        let stop_state = self.service_stop_state.clone();
        let owner = self.context.owner.logical_owner().clone();
        let session = self.reference.clone();
        let max_payload_bytes = self.context.max_payload_bytes;
        let base_generation = self
            .next_service_stop_generation
            .fetch_add(4, Ordering::Relaxed)
            .max(1);
        tokio::spawn(async move {
            let capabilities = service.capabilities_for(&owner);
            if !capabilities.supports(MediaOperationKind::Stop) {
                tracing::warn!("media service cannot deregister a terminal remote session");
                return;
            }
            let context = MediaOperationContext::new(
                MediaOperationId::new(base_generation, capabilities.service_epoch),
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
                Ok(MediaResult::Stopped { resource })
                    if resource == MediaResourceRef::Session(session) =>
                {
                    stop_state.store(SERVICE_STOP_CONFIRMED, Ordering::Release);
                }
                Err(error) if error.kind == MediaErrorKind::NotFound => {
                    stop_state.store(SERVICE_STOP_CONFIRMED, Ordering::Release);
                }
                Ok(_) => tracing::warn!("media service returned an unexpected Stop result"),
                Err(error) => tracing::warn!(
                    kind = error.kind.to_string(),
                    "remote media session deregistration remains Unknown"
                ),
            }
        });
    }

    fn on_registered(&self) {
        self.service_registered.store(true, Ordering::Release);
        // A peer can terminate while its start operation is still being
        // committed into MediaRuntime. Defer the callback until this hook
        // confirms insertion, then emit it once if terminal already won.
        if self.stopped.load(Ordering::Acquire) {
            self.schedule_service_deregistration();
        }
    }

    fn connected_once(&self) -> bool {
        self.status_tx.borrow().state == MediaSessionState::Ready
            || self.status_tx.borrow().state == MediaSessionState::Reconnecting
    }

    fn set_peer_state(&self, state: webrtc::peer_connection::RTCPeerConnectionState) {
        use webrtc::peer_connection::RTCPeerConnectionState as Peer;
        let mapped = match state {
            Peer::Connected => MediaSessionState::Ready,
            Peer::Disconnected | Peer::Connecting if self.connected_once() => {
                MediaSessionState::Reconnecting
            }
            Peer::Failed => MediaSessionState::Failed,
            Peer::Closed if self.stopped.load(Ordering::Acquire) => MediaSessionState::Ended,
            Peer::Closed => MediaSessionState::Failed,
            Peer::New | Peer::Connecting | Peer::Disconnected | Peer::Unspecified => {
                MediaSessionState::Preparing
            }
            _ => MediaSessionState::Failed,
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
        let resource = MediaResourceRef::Session(self.reference.clone());
        let resource_state = match state {
            MediaSessionState::Preparing => MediaResourceState::Ready,
            MediaSessionState::Ready | MediaSessionState::Reconnecting => {
                MediaResourceState::Active
            }
            MediaSessionState::Ended => MediaResourceState::Completed,
            MediaSessionState::Failed => MediaResourceState::Failed,
        };
        self.push_event(
            MediaEventKind::StateChanged {
                resource: resource.clone(),
                state: resource_state,
            },
            unix_now_ms(),
        );
    }

    fn push_event(&self, kind: MediaEventKind, occurred_at_unix_ms: u64) {
        let sequence = self
            .event_sequence
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let target = MediaResourceRef::Session(self.reference.clone());
        let event = MediaEvent {
            sequence,
            occurred_at_unix_ms,
            target: target.clone(),
            operation_id: None,
            kind,
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

    fn validate_context(&self, context: &MediaOperationContext) -> Result<(), MediaError> {
        if context.owner != self.context.owner
            || context.identity.service_epoch != self.context.identity.service_epoch
        {
            return Err(MediaError::new(
                MediaErrorKind::NotFound,
                "media session owner does not match",
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl MediaSession for RemoteHostMediaSession {
    fn reference(&self) -> &MediaSessionRef {
        &self.reference
    }
    fn status(&self) -> MediaSessionStatus {
        self.status_tx.borrow().clone()
    }
    fn authorized_sources(&self) -> Vec<MediaSourceRef> {
        self.request
            .audio_source
            .iter()
            .chain(self.request.video_source.iter())
            .cloned()
            .collect()
    }

    async fn audio_channel(&self) -> Result<Arc<dyn MediaAudioChannel>, MediaError> {
        self.audio_channel
            .clone()
            .map(|channel| channel as Arc<dyn MediaAudioChannel>)
            .ok_or_else(|| {
                MediaError::new(
                    MediaErrorKind::Unsupported,
                    "remote media session has no audio channel",
                )
            })
    }

    async fn update(
        &self,
        context: MediaOperationContext,
        update: MediaSessionUpdate,
    ) -> Result<MediaSessionStatus, MediaError> {
        self.validate_context(&context)?;
        match &update {
            MediaSessionUpdate::SetEnabled {
                kind: MediaKind::Audio,
                enabled,
            } if !mode_has_audio(self.request.mode) && *enabled => {
                return Err(invalid("audio was not negotiated"));
            }
            MediaSessionUpdate::SetEnabled {
                kind: MediaKind::Video,
                enabled,
            } if !mode_has_video(self.request.mode) && *enabled => {
                return Err(invalid("video was not negotiated"));
            }
            MediaSessionUpdate::SelectSource {
                kind: MediaKind::Audio,
                source,
            } if self.request.audio_source.as_ref() != Some(source) => {
                return Err(invalid(
                    "audio source replacement requires a new media session",
                ));
            }
            MediaSessionUpdate::SelectSource {
                kind: MediaKind::Video,
                source,
            } if self.request.video_source.as_ref() != Some(source) => {
                return Err(invalid(
                    "video source replacement requires a new media session",
                ));
            }
            MediaSessionUpdate::SetPlaybackGain { .. } => {
                return Err(MediaError::new(
                    MediaErrorKind::Unsupported,
                    "remote playback gain is controlled by the connected device",
                ));
            }
            _ => {}
        }
        if let MediaSessionUpdate::SetEnabled {
            kind: MediaKind::Audio,
            enabled,
        } = &update
        {
            self.enabled_audio.store(*enabled, Ordering::Release);
        }
        if let MediaSessionUpdate::SetEnabled {
            kind: MediaKind::Video,
            enabled,
        } = &update
        {
            self.enabled_video.store(*enabled, Ordering::Release);
        }
        let mut status = self.status_tx.borrow().clone();
        if let MediaSessionUpdate::SetEnabled {
            kind: MediaKind::Audio,
            enabled,
        } = &update
        {
            status.audio_enabled = *enabled;
        }
        if let MediaSessionUpdate::SetEnabled {
            kind: MediaKind::Video,
            enabled,
        } = &update
        {
            status.video_enabled = *enabled;
        }
        self.status_tx.send_replace(status.clone());
        Ok(status)
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
            return Err(invalid("media observation bounds are invalid"));
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
            Ok((
                MediaEventBatch::bounded(
                    MediaResourceRef::Session(self.reference.clone()),
                    items,
                    dropped,
                )?,
                events.iter().all(|event| event.sequence <= after),
            ))
        };
        let (batch, empty) = collect()?;
        if !empty || wait_ms.unwrap_or(0) == 0 {
            return Ok(batch);
        }
        if timeout(
            Duration::from_millis(wait_ms.unwrap_or(0)),
            self.event_notify.notified(),
        )
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

#[async_trait]
impl MediaAudioChannel for RemoteHostMediaAudioChannel {
    async fn read(&self) -> Result<Option<MediaFrame>, MediaError> {
        Ok(self.input.lock().await.recv().await)
    }

    async fn write(&self, frame: MediaPcmFrame) -> Result<MediaRenderReceipt, MediaError> {
        let _guard = self.request_gate.lock().await;
        if self.stopping.load(Ordering::Acquire) {
            return Err(MediaError::new(
                MediaErrorKind::Cancelled,
                "remote media session is stopping",
            ));
        }
        if !self.enabled_audio.load(Ordering::Acquire) {
            return Err(MediaError::new(
                MediaErrorKind::InvalidRequest,
                "remote media audio is disabled",
            ));
        }
        frame.validate(self.context.max_payload_bytes)?;
        let geometry = AudioSourceGeometry {
            sample_rate_hz: frame.sample_rate_hz,
            channels: frame.channels,
        };
        {
            let mut source_geometry = self
                .source_geometry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match *source_geometry {
                Some(existing) if existing != geometry => {
                    return Err(invalid(
                        "audio source sample rate or channel count changed mid-session",
                    ));
                }
                Some(_) => {}
                None => *source_geometry = Some(geometry),
            }
        }
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(WorkerAudioCommand::Write { frame, reply: tx })
            .await
            .map_err(|_| {
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "remote media audio sender stopped",
                )
            })?;
        let receipt = rx.await.map_err(|_| {
            MediaError::new(
                MediaErrorKind::Unavailable,
                "remote media audio sender stopped",
            )
        })??;
        self.accepted_source_sample_frames
            .fetch_add(u64::from(receipt.accepted_sample_frames), Ordering::AcqRel);
        Ok(receipt)
    }

    async fn flush(&self) -> Result<MediaRenderReceipt, MediaError> {
        self.barrier(false).await
    }

    async fn interrupt(&self) -> Result<MediaRenderReceipt, MediaError> {
        self.barrier(true).await
    }
}

impl RemoteHostMediaAudioChannel {
    async fn barrier(&self, interrupt: bool) -> Result<MediaRenderReceipt, MediaError> {
        // Serialize only the local encoder snapshot. Holding this gate across
        // the remote acknowledgement would put barge-in behind a slow flush.
        let (target, epoch, previous_completed) = {
            let _guard = self.request_gate.lock().await;
            if self.stopping.load(Ordering::Acquire) && !interrupt {
                return Err(MediaError::new(
                    MediaErrorKind::Cancelled,
                    "remote media session is stopping",
                ));
            }
            let epoch = if interrupt {
                self.control_epoch.fetch_add(1, Ordering::AcqRel) + 1
            } else {
                self.control_epoch.load(Ordering::Acquire)
            };
            let (tx, rx) = oneshot::channel();
            let command = if interrupt {
                WorkerAudioCommand::Interrupt { reply: tx }
            } else {
                WorkerAudioCommand::Flush { reply: tx }
            };
            self.commands.send(command).await.map_err(|_| {
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "remote media audio sender stopped",
                )
            })?;
            let target = rx.await.map_err(|_| {
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "remote media audio sender stopped",
                )
            })??;
            let previous_target = *self
                .last_barrier_target
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            validate_audio_target(target, previous_target)?;
            *self
                .last_barrier_target
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = target;
            let previous_completed = *self
                .last_completed_target
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (target, epoch, previous_completed)
        };
        let action = if interrupt {
            MediaControlActionDto::InterruptAudio {
                target_sequence: target.sequence,
                target_sample_frames: target.sample_frames,
            }
        } else {
            MediaControlActionDto::FlushAudio {
                target_sequence: target.sequence,
                target_sample_frames: target.sample_frames,
            }
        };
        let request = MediaControlRequestDto {
            request_id: random_id(),
            connection_id: self.route.id.clone(),
            media_session_ref: MediaSessionRefDto {
                id: self.reference.0 .0.clone(),
            },
            generation: self.generation,
            action,
        };
        request
            .validate()
            .map_err(|_| invalid("media audio barrier request is invalid"))?;
        let response = self.route.channel.control(request.clone()).await?;
        // Linearize completion against interrupt/close. This is a short local
        // critical section; the network wait above remains cancellable.
        let _completion_gate = self.request_gate.lock().await;
        if self.control_epoch.load(Ordering::Acquire) != epoch {
            return Err(MediaError::new(
                MediaErrorKind::Cancelled,
                "media audio barrier was superseded by an interrupt or close",
            ));
        }
        let target_delta = target
            .sample_frames
            .saturating_sub(previous_completed.sample_frames);
        let receipt = control_receipt(&request, response, target_delta)?;
        let mut completed = self
            .last_completed_target
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if target.sample_frames >= completed.sample_frames {
            *completed = target;
        }
        drop(completed);
        let source_rate = {
            let geometry = self
                .source_geometry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (*geometry).map(|geometry| geometry.sample_rate_hz)
        };
        let played_source_sample_frames = if let Some(source_rate) = source_rate {
            let played_rtp_total = self
                .played_rtp_sample_frames
                .fetch_add(u64::from(receipt.played_sample_frames), Ordering::AcqRel)
                .saturating_add(u64::from(receipt.played_sample_frames));
            let accepted_source = self.accepted_source_sample_frames.load(Ordering::Acquire);
            let played_target =
                rtp_sample_frames_to_source_frames(played_rtp_total, source_rate, accepted_source);
            let previous = self
                .played_source_sample_frames
                .swap(played_target, Ordering::AcqRel);
            played_target.saturating_sub(previous)
        } else {
            0
        };
        Ok(MediaRenderReceipt {
            accepted_sample_frames: 0,
            played_sample_frames: u32::try_from(played_source_sample_frames).map_err(|_| {
                MediaError::new(
                    MediaErrorKind::MediaTooLarge,
                    "played source audio sample count exceeds its limit",
                )
            })?,
            rendered_at_unix_ms: receipt.rendered_at_unix_ms,
        })
    }
}

fn validate_close_response(
    request: &MediaControlRequestDto,
    response: MediaControlResponseDto,
) -> Result<(), MediaError> {
    response
        .validate()
        .map_err(|_| invalid("media close response is invalid"))?;
    if !response.matches_request(request) {
        return Err(invalid("media close response did not match its request"));
    }
    if let Some(error) = response.error {
        return Err(dto_error(error.kind, error.message));
    }
    let receipt = response
        .receipt
        .ok_or_else(|| invalid("media close acknowledgement is missing"))?;
    if receipt.accepted_sample_frames != 0
        || u64::from(receipt.played_sample_frames) > MEDIA_AUDIO_SAMPLE_FRAMES_MAX
    {
        return Err(invalid("media close acknowledgement is invalid"));
    }
    Ok(())
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

fn validate_audio_target(target: AudioTarget, previous: AudioTarget) -> Result<(), MediaError> {
    if target.sequence > MEDIA_AUDIO_SEQUENCE_MAX
        || target.sample_frames > MEDIA_AUDIO_SAMPLE_FRAMES_MAX
        || target.sample_frames != target.sequence.saturating_mul(AUDIO_RTP_SAMPLES)
        || target.sequence < previous.sequence
        || target.sample_frames < previous.sample_frames
    {
        return Err(invalid(
            "media audio barrier target is invalid or moved backwards",
        ));
    }
    Ok(())
}

fn control_receipt(
    request: &MediaControlRequestDto,
    response: MediaControlResponseDto,
    max_played_sample_frames: u64,
) -> Result<MediaRenderReceipt, MediaError> {
    response
        .validate()
        .map_err(|_| invalid("media audio barrier response is invalid"))?;
    if !response.matches_request(request) {
        return Err(invalid(
            "media audio barrier response did not match its request",
        ));
    }
    if let Some(error) = response.error {
        return Err(dto_error(error.kind, error.message));
    }
    let receipt = response
        .receipt
        .ok_or_else(|| invalid("media audio barrier receipt is missing"))?;
    if receipt.accepted_sample_frames != 0 {
        return Err(invalid("media audio barrier accepted new samples"));
    }
    if u64::from(receipt.played_sample_frames) > max_played_sample_frames {
        return Err(invalid(
            "media audio barrier exceeded its requested sample boundary",
        ));
    }
    Ok(MediaRenderReceipt {
        accepted_sample_frames: 0,
        played_sample_frames: receipt.played_sample_frames,
        rendered_at_unix_ms: receipt.rendered_at_unix_ms,
    })
}

async fn encode_audio(
    rtc: &mut RtcSession,
    state: &mut AudioEncoderState,
    frame: MediaPcmFrame,
    max_payload_bytes: u64,
) -> Result<MediaRenderReceipt, MediaError> {
    frame.validate(max_payload_bytes)?;
    let rate = frame.sample_rate_hz;
    if state
        .processor
        .as_ref()
        .is_none_or(|(current, _)| *current != rate)
    {
        state.processor = Some((
            rate,
            AudioProcessor::new(rate, rate).map_err(native_media_error)?,
        ));
        state.pending_sample_frames = 0;
    }
    let channels = usize::from(frame.channels);
    let samples = frame
        .pcm16le
        .chunks_exact(channels * 2)
        .map(|sample_frame| {
            sample_frame
                .chunks_exact(2)
                .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / i16::MAX as f32)
                .sum::<f32>()
                / channels as f32
        })
        .collect::<Vec<_>>();
    state.pending_sample_frames = state
        .pending_sample_frames
        .saturating_add(samples.len() as u64);
    process_audio_samples(rtc, state, &samples, rate).await?;
    Ok(MediaRenderReceipt {
        accepted_sample_frames: u32::try_from(samples.len()).map_err(|_| {
            MediaError::new(
                MediaErrorKind::MediaTooLarge,
                "audio frame sample count exceeds its limit",
            )
        })?,
        played_sample_frames: 0,
        rendered_at_unix_ms: None,
    })
}

async fn process_audio_samples(
    rtc: &mut RtcSession,
    state: &mut AudioEncoderState,
    samples: &[f32],
    rate: u32,
) -> Result<(), MediaError> {
    let processor = &mut state
        .processor
        .as_mut()
        .ok_or_else(|| invalid("audio processor is unavailable"))?
        .1;
    let packet_samples = u64::from(rate).div_ceil(50);
    for chunk in samples.chunks(AUDIO_CAPTURE_CHUNK) {
        for encoded in processor
            .capture(chunk, std::time::Instant::now())
            .map_err(native_media_error)?
        {
            rtc.send_opus(encoded.data, encoded.at)
                .await
                .map_err(native_media_error)?;
            state.sequence = state.sequence.checked_add(1).ok_or_else(|| {
                MediaError::new(
                    MediaErrorKind::MediaTooLarge,
                    "remote audio sequence exhausted",
                )
            })?;
            state.pending_sample_frames =
                state.pending_sample_frames.saturating_sub(packet_samples);
        }
    }
    Ok(())
}

async fn flush_encoder(
    rtc: &mut RtcSession,
    state: &mut AudioEncoderState,
) -> Result<AudioTarget, MediaError> {
    if state.pending_sample_frames > 0 {
        let Some((rate, _)) = state.processor.as_ref() else {
            return Err(invalid("audio processor is unavailable"));
        };
        let padding = audio_tail_padding_samples(state.pending_sample_frames, *rate);
        let zeros = vec![
            0.0;
            usize::try_from(padding).map_err(|_| MediaError::new(
                MediaErrorKind::MediaTooLarge,
                "audio padding exceeds its limit"
            ))?
        ];
        process_audio_samples(rtc, state, &zeros, *rate).await?;
    }
    Ok(AudioTarget {
        sequence: state.sequence,
        sample_frames: state.sequence.saturating_mul(AUDIO_RTP_SAMPLES),
    })
}

fn audio_tail_padding_samples(pending_sample_frames: u64, sample_rate_hz: u32) -> u64 {
    if pending_sample_frames == 0 {
        return 0;
    }
    u64::from(sample_rate_hz)
        .div_ceil(50)
        .saturating_sub(pending_sample_frames)
}

fn interrupt_encoder(state: &mut AudioEncoderState) -> AudioTarget {
    if let Some((_, processor)) = state.processor.as_mut() {
        let _ = processor.reset();
    }
    state.pending_sample_frames = 0;
    AudioTarget {
        sequence: state.sequence,
        sample_frames: state.sequence.saturating_mul(AUDIO_RTP_SAMPLES),
    }
}

fn decoded_video_frame(
    frame: lingxi_webrtc_media::video::VideoFrame,
) -> Result<MediaVideoFrame, MediaError> {
    let layout = frame.layout;
    if layout.strides.len() < 3 || layout.offsets.len() < 3 {
        return Err(invalid("decoded video plane layout is invalid"));
    }
    let plane = |index: usize, rows: u32| -> Result<Vec<u8>, MediaError> {
        let stride = usize::try_from(layout.strides[index])
            .map_err(|_| invalid("decoded video stride is invalid"))?;
        let offset = layout.offsets[index];
        let length = stride
            .checked_mul(rows as usize)
            .ok_or_else(|| invalid("decoded video plane size overflowed"))?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| invalid("decoded video plane offset overflowed"))?;
        if end > frame.pixels.len() {
            return Err(invalid("decoded video plane exceeds its buffer"));
        }
        Ok(frame.pixels[offset..end].to_vec())
    };
    let y_stride =
        u32::try_from(layout.strides[0]).map_err(|_| invalid("decoded video stride is invalid"))?;
    let u_stride =
        u32::try_from(layout.strides[1]).map_err(|_| invalid("decoded video stride is invalid"))?;
    let v_stride =
        u32::try_from(layout.strides[2]).map_err(|_| invalid("decoded video stride is invalid"))?;
    let chroma_height = layout.height.div_ceil(2);
    let video = MediaVideoFrame::I420 {
        width_px: layout.width,
        height_px: layout.height,
        y_stride_bytes: y_stride,
        u_stride_bytes: u_stride,
        v_stride_bytes: v_stride,
        y: plane(0, layout.height)?,
        u: plane(1, chroma_height)?,
        v: plane(2, chroma_height)?,
    };
    video.validate(media_api::DEFAULT_MAX_MEDIA_PAYLOAD_BYTES)?;
    Ok(video)
}

fn selected_route_id(request: &MediaSessionRequest) -> Result<&str, MediaError> {
    let mut route = None;
    for source in request
        .audio_source
        .iter()
        .chain(request.video_source.iter())
    {
        let Some((source_route, _)) = parse_remote_media_source_ref(&source.0 .0) else {
            return Err(invalid(
                "selected source is not a registered remote route source",
            ));
        };
        if route.is_some_and(|route| route != source_route) {
            return Err(invalid(
                "media session sources belong to different remote routes",
            ));
        }
        route = Some(source_route);
    }
    route.ok_or_else(|| invalid("remote media session requires a selected source"))
}

fn bounded_label(route: &str, source: &str) -> String {
    let label = format!("{route} {source}");
    label.chars().take(256).collect()
}

fn has_duplicates<T: PartialEq>(values: &[T]) -> bool {
    values
        .iter()
        .enumerate()
        .any(|(index, value)| values[..index].contains(value))
}

fn mode_has_audio(mode: MediaSessionMode) -> bool {
    matches!(mode, MediaSessionMode::Audio | MediaSessionMode::AudioVideo)
}
fn mode_has_video(mode: MediaSessionMode) -> bool {
    matches!(mode, MediaSessionMode::Video | MediaSessionMode::AudioVideo)
}

fn dto_mode(mode: MediaSessionMode) -> client::protocol::media::MediaSessionModeDto {
    match mode {
        MediaSessionMode::Audio => client::protocol::media::MediaSessionModeDto::Audio,
        MediaSessionMode::Video => client::protocol::media::MediaSessionModeDto::Video,
        MediaSessionMode::AudioVideo => client::protocol::media::MediaSessionModeDto::AudioVideo,
    }
}

fn dto_intent(intent: MediaSessionIntent) -> client::protocol::media::MediaSessionIntentDto {
    match intent {
        MediaSessionIntent::Observation => {
            client::protocol::media::MediaSessionIntentDto::Observation
        }
        MediaSessionIntent::Conversation => {
            client::protocol::media::MediaSessionIntentDto::Conversation
        }
    }
}

fn dto_source_ref(source: &MediaSourceRef) -> client::protocol::media::MediaSourceRefDto {
    client::protocol::media::MediaSourceRefDto {
        id: source.0 .0.clone(),
    }
}

fn dto_operation_id(identity: &MediaOperationId) -> client::protocol::media::MediaOperationIdDto {
    client::protocol::media::MediaOperationIdDto {
        id: identity.id.clone(),
        generation: identity.generation,
        service_epoch: identity.service_epoch,
    }
}

fn dto_owner(owner: &MediaOwner) -> Result<client::protocol::media::MediaOwnerDto, MediaError> {
    logical_session_id(owner)
        .map(
            |session_id| client::protocol::media::MediaOwnerDto::Session {
                session_id: session_id.to_owned(),
            },
        )
        .ok_or_else(|| invalid("remote media sessions require a conversation owner"))
}

/// Remote routes are keyed by the complete trusted owner, including an
/// attachment ID when present. Only the wire protocol's session field is
/// flattened to the logical session ID; route lookup, mode inventory, and
/// teardown continue to use the original `MediaOwner` value.
fn logical_session_id(owner: &MediaOwner) -> Option<&str> {
    match owner.logical_owner() {
        MediaOwner::Session { session_id } => Some(session_id),
        MediaOwner::Ui { .. } | MediaOwner::System { .. } | MediaOwner::Attachment { .. } => None,
    }
}

fn valid_route_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes[index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
}

fn dto_error(kind: MediaErrorKindDto, message: String) -> MediaError {
    let kind = match kind {
        MediaErrorKindDto::PermissionDenied => MediaErrorKind::PermissionDenied,
        MediaErrorKindDto::Busy => MediaErrorKind::Busy,
        MediaErrorKindDto::Cancelled => MediaErrorKind::Cancelled,
        MediaErrorKindDto::Timeout => MediaErrorKind::Timeout,
        MediaErrorKindDto::NoData => MediaErrorKind::NoData,
        MediaErrorKindDto::NotFound => MediaErrorKind::NotFound,
        MediaErrorKindDto::Unavailable => MediaErrorKind::Unavailable,
        MediaErrorKindDto::Unsupported => MediaErrorKind::Unsupported,
        MediaErrorKindDto::ModelMissing => MediaErrorKind::ModelMissing,
        MediaErrorKindDto::InvalidRequest => MediaErrorKind::InvalidRequest,
        MediaErrorKindDto::ProcessingFailed => MediaErrorKind::ProcessingFailed,
        MediaErrorKindDto::NativeFailure => MediaErrorKind::NativeFailure,
        MediaErrorKindDto::StoreFailure => MediaErrorKind::StoreFailure,
        MediaErrorKindDto::MediaTooLarge => MediaErrorKind::MediaTooLarge,
    };
    MediaError::new(kind, message)
}

fn native_media_error(error: impl std::fmt::Display) -> MediaError {
    MediaError::new(MediaErrorKind::NativeFailure, error.to_string())
}

fn invalid(message: &str) -> MediaError {
    MediaError::new(MediaErrorKind::InvalidRequest, message)
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u128::from(u64::MAX)) as u64
        })
}

fn monotonic_to_unix_ms(anchor: Instant, anchor_unix_ms: u64, at: Instant) -> u64 {
    if at >= anchor {
        anchor_unix_ms.saturating_add(
            at.duration_since(anchor)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        )
    } else {
        anchor_unix_ms.saturating_sub(
            anchor
                .duration_since(at)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        )
    }
}

fn should_sample_video_frame(last: &mut Option<Instant>, presented_at: Instant) -> bool {
    if last.is_some_and(|last| {
        presented_at <= last || presented_at.duration_since(last) < VIDEO_OBSERVATION_INTERVAL
    }) {
        return false;
    }
    *last = Some(presented_at);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::protocol::media::{MediaControlActionDto, MediaRenderReceiptDto};
    use std::collections::HashSet;

    const ROUTE_A: &str = "1e5d7a65-b37c-4e51-9ac8-33d2c2a98a61";
    const ROUTE_B: &str = "5f3802f1-6a08-42e3-877f-1e62b527c8ce";

    struct TestConfig;

    #[derive(Default)]
    struct TestMediaService {
        sessions: Mutex<HashMap<MediaSessionRef, (MediaOwner, Arc<dyn MediaSession>)>>,
        stop_calls: std::sync::atomic::AtomicUsize,
        fail_stop: AtomicBool,
    }

    impl TestMediaService {
        fn session_count(&self) -> usize {
            self.sessions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len()
        }
    }

    #[async_trait]
    impl MediaService for TestMediaService {
        fn capabilities(&self) -> media_api::MediaCapabilities {
            media_api::MediaCapabilities {
                service_epoch: 1,
                support_revision: 1,
                supported_operations: vec![MediaOperationKind::Status, MediaOperationKind::Stop],
                supported_media_kinds: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }

        async fn execute(
            &self,
            context: MediaOperationContext,
            operation: MediaOperation,
        ) -> Result<media_api::MediaResult, MediaError> {
            match operation {
                MediaOperation::Status { .. } => {
                    let sessions = self
                        .sessions
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let resources = sessions
                        .iter()
                        .filter(|(_, (owner, session))| {
                            owner == &context.owner
                                && session.status().state != MediaSessionState::Ended
                        })
                        .map(|(reference, _)| media_api::MediaResourceStatus {
                            resource: MediaResourceRef::Session(reference.clone()),
                            state: MediaResourceState::Active,
                            started_at_unix_ms: None,
                            updated_at_unix_ms: None,
                        })
                        .collect();
                    Ok(media_api::MediaResult::Status { resources })
                }
                MediaOperation::Stop {
                    target: MediaResourceRef::Session(reference),
                } => {
                    self.stop_calls.fetch_add(1, Ordering::Relaxed);
                    if self.fail_stop.load(Ordering::Acquire) {
                        return Err(MediaError::new(
                            MediaErrorKind::NativeFailure,
                            "test Stop failed with unknown side effects",
                        ));
                    }
                    let session = self
                        .sessions
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .get(&reference)
                        .filter(|(owner, _)| owner == &context.owner)
                        .map(|(_, session)| session.clone())
                        .ok_or_else(|| {
                            MediaError::new(MediaErrorKind::NotFound, "test session missing")
                        })?;
                    let status = session.stop(context.clone()).await?;
                    if status.state != MediaSessionState::Ended {
                        return Err(MediaError::new(
                            MediaErrorKind::NativeFailure,
                            "test session did not reach Ended",
                        ));
                    }
                    let mut sessions = self
                        .sessions
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if sessions
                        .get(&reference)
                        .is_some_and(|(_, current)| Arc::ptr_eq(current, &session))
                    {
                        sessions.remove(&reference);
                    }
                    Ok(media_api::MediaResult::Stopped {
                        resource: MediaResourceRef::Session(reference),
                    })
                }
                _ => Err(MediaError::new(
                    MediaErrorKind::Unsupported,
                    "test service supports session Stop only",
                )),
            }
        }

        async fn attach_session(
            &self,
            context: MediaOperationContext,
            session: Box<dyn MediaSession>,
        ) -> Result<MediaSessionRef, MediaError> {
            let reference = session.reference().clone();
            let session: Arc<dyn MediaSession> = Arc::from(session);
            {
                let mut sessions = self
                    .sessions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if sessions.len() >= 64 || sessions.contains_key(&reference) {
                    return Err(MediaError::new(
                        MediaErrorKind::Busy,
                        "test session already attached or capacity is full",
                    ));
                }
                sessions.insert(reference.clone(), (context.owner, session.clone()));
            }
            session.on_registered();
            Ok(reference)
        }

        async fn read_media(
            &self,
            _context: MediaOperationContext,
            _media: media_api::MediaRef,
        ) -> Result<media_api::MediaContent, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::NotFound,
                "test media not found",
            ))
        }

        async fn cancel(&self, _context: MediaOperationContext) -> Result<(), MediaError> {
            Ok(())
        }
    }

    #[async_trait]
    impl RemoteMediaHostConfigProvider for TestConfig {
        async fn resolve(
            &self,
            _context: &MediaOperationContext,
            _route_id: &str,
            _request: &MediaSessionRequest,
        ) -> Result<RemoteMediaConfig, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "test config is intentionally unavailable",
            ))
        }
    }

    struct TestChannel;

    #[async_trait]
    impl RemoteMediaHostControlChannel for TestChannel {
        async fn negotiate(
            &self,
            _request: MediaConnectionRequestDto,
        ) -> Result<MediaConnectionResponseDto, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "test channel does not negotiate",
            ))
        }

        async fn control(
            &self,
            request: MediaControlRequestDto,
        ) -> Result<MediaControlResponseDto, MediaError> {
            Ok(control_close_response(&request))
        }
    }

    struct RetryCloseChannel {
        close_calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl RemoteMediaHostControlChannel for RetryCloseChannel {
        async fn negotiate(
            &self,
            _request: MediaConnectionRequestDto,
        ) -> Result<MediaConnectionResponseDto, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "test channel does not negotiate",
            ))
        }

        async fn control(
            &self,
            request: MediaControlRequestDto,
        ) -> Result<MediaControlResponseDto, MediaError> {
            if self.close_calls.fetch_add(1, Ordering::AcqRel) == 0 {
                return Err(MediaError::new(
                    MediaErrorKind::Unavailable,
                    "first close attempt is unavailable",
                ));
            }
            Ok(control_close_response(&request))
        }
    }

    struct PendingBarrierChannel {
        requests: tokio::sync::mpsc::UnboundedSender<(
            MediaControlRequestDto,
            oneshot::Sender<Result<MediaControlResponseDto, MediaError>>,
        )>,
    }

    #[async_trait]
    impl RemoteMediaHostControlChannel for PendingBarrierChannel {
        async fn negotiate(
            &self,
            _request: MediaConnectionRequestDto,
        ) -> Result<MediaConnectionResponseDto, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "test channel does not negotiate",
            ))
        }

        async fn control(
            &self,
            request: MediaControlRequestDto,
        ) -> Result<MediaControlResponseDto, MediaError> {
            let (reply, response) = oneshot::channel();
            self.requests.send((request, reply)).map_err(|_| {
                MediaError::new(MediaErrorKind::Unavailable, "control test is closed")
            })?;
            response.await.map_err(|_| {
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "control test response was dropped",
                )
            })?
        }
    }

    struct TestStore;

    #[async_trait]
    impl MediaResourceStore for TestStore {
        async fn put(
            &self,
            _context: &MediaOperationContext,
            _payload: media_api::MediaPayload,
        ) -> Result<media_api::MediaAsset, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "unused test store",
            ))
        }

        async fn get(
            &self,
            _context: &MediaOperationContext,
            _media: &media_api::MediaRef,
            _max_bytes: u64,
        ) -> Result<media_api::MediaPayload, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "unused test store",
            ))
        }

        async fn delete(
            &self,
            _context: &MediaOperationContext,
            _media: &media_api::MediaRef,
        ) -> Result<(), MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "unused test store",
            ))
        }
    }

    fn owner(id: &str) -> MediaOwner {
        MediaOwner::Session {
            session_id: id.to_owned(),
        }
    }

    fn context(owner: MediaOwner) -> MediaOperationContext {
        MediaOperationContext::new(MediaOperationId::new(1, 1), owner)
    }

    fn connector() -> Arc<RemoteMediaConnector> {
        RemoteMediaConnector::new(Arc::new(TestConfig), Arc::new(TestStore))
    }

    fn audio_video_request(audio_route: &str, video_route: &str) -> MediaSessionRequest {
        MediaSessionRequest {
            intent: MediaSessionIntent::Conversation,
            mode: MediaSessionMode::AudioVideo,
            audio_source: Some(MediaSourceRef(
                remote_media_source_ref(audio_route, RemoteMediaSourceKind::Microphone)
                    .unwrap()
                    .into(),
            )),
            video_source: Some(MediaSourceRef(
                remote_media_source_ref(video_route, RemoteMediaSourceKind::Camera)
                    .unwrap()
                    .into(),
            )),
        }
    }

    async fn register_av(connector: &RemoteMediaConnector, owner: MediaOwner, route: &str) {
        connector
            .register_route(
                owner,
                route.to_owned(),
                "Viewer phone".to_owned(),
                vec![
                    MediaSessionMode::Audio,
                    MediaSessionMode::Video,
                    MediaSessionMode::AudioVideo,
                ],
                vec![MediaSourceKindDto::Microphone, MediaSourceKindDto::Camera],
                Arc::new(TestChannel),
            )
            .await
            .unwrap();
    }

    async fn register_av_with_service(
        connector: &RemoteMediaConnector,
        owner: MediaOwner,
        route: &str,
        service: Arc<dyn MediaService>,
    ) {
        connector
            .register_attachment_route(
                owner,
                route.to_owned(),
                "Viewer phone".to_owned(),
                vec![
                    MediaSessionMode::Audio,
                    MediaSessionMode::Video,
                    MediaSessionMode::AudioVideo,
                ],
                vec![MediaSourceKindDto::Microphone, MediaSourceKindDto::Camera],
                Arc::new(TestChannel),
                service,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn source_catalog_is_owner_scoped_and_matches_declared_kinds() {
        let connector = connector();
        let session_owner = owner("session-a");
        register_av(&connector, session_owner.clone(), ROUTE_A).await;

        let sources = connector
            .sources(context(session_owner.clone()))
            .await
            .unwrap();
        let references = sources
            .iter()
            .map(|source| source.reference.0 .0.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(references.len(), 2);
        assert!(references.contains(
            remote_media_source_ref(ROUTE_A, RemoteMediaSourceKind::Microphone)
                .unwrap()
                .as_str()
        ));
        assert!(references.contains(
            remote_media_source_ref(ROUTE_A, RemoteMediaSourceKind::Camera)
                .unwrap()
                .as_str()
        ));
        assert!(sources
            .iter()
            .all(|source| source.kind == MediaSourceKind::Remote && !source.is_default));
        assert!(!references.contains(
            remote_media_source_ref(ROUTE_A, RemoteMediaSourceKind::Screen)
                .unwrap()
                .as_str()
        ));
        assert!(connector
            .sources(context(owner("session-b")))
            .await
            .unwrap()
            .is_empty());

        let route = connector
            .route_for_sources(&session_owner, &audio_video_request(ROUTE_A, ROUTE_A))
            .await
            .unwrap();
        assert_eq!(route.id, ROUTE_A);
        let missing_owner = connector
            .route_for_sources(&owner("session-b"), &audio_video_request(ROUTE_A, ROUTE_A))
            .await;
        let missing_owner = match missing_owner {
            Ok(_) => panic!("a route must not cross media owners"),
            Err(error) => error,
        };
        assert_eq!(missing_owner.kind, MediaErrorKind::NotFound);
    }

    #[tokio::test]
    async fn attachment_owner_is_preserved_for_route_keys_modes_and_unregistration() {
        let connector = connector();
        let host = media_runtime::MediaAttachmentHost::new();
        let attachment_a = host
            .attach(Arc::new(TestMediaService::default()))
            .expect("first attachment handle");
        let attachment_b = host
            .attach(Arc::new(TestMediaService::default()))
            .expect("second attachment handle");
        let owner_a = owner("same-session").in_attachment(attachment_a.id().clone());
        let owner_b = owner("same-session").in_attachment(attachment_b.id().clone());

        register_av_with_service(&connector, owner_a.clone(), ROUTE_A, attachment_a.service())
            .await;
        register_av_with_service(&connector, owner_b.clone(), ROUTE_A, attachment_b.service())
            .await;
        let route = connector
            .routes
            .read()
            .await
            .get(&owner_a)
            .and_then(|routes| routes.get(ROUTE_A))
            .expect("route is stored under the full attachment owner")
            .clone();
        let replacement_route = connector
            .routes
            .read()
            .await
            .get(&owner_b)
            .and_then(|routes| routes.get(ROUTE_A))
            .expect("same route ID is independently stored under the new attachment")
            .clone();
        assert_eq!(
            route
                .media_service
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
                .and_then(|service| service.attachment_id()),
            owner_a.attachment_id().cloned(),
            "route Stop must use the exact service attachment"
        );
        assert_eq!(
            replacement_route
                .media_service
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
                .and_then(|service| service.attachment_id()),
            owner_b.attachment_id().cloned(),
            "replacement route Stop must not retain the old attachment service"
        );
        assert!(matches!(
            dto_owner(&owner_a),
            Ok(client::protocol::media::MediaOwnerDto::Session { ref session_id })
                if session_id == "same-session"
        ));
        assert_eq!(
            connector.supported_modes(&owner_a).await,
            vec![
                MediaSessionMode::Audio,
                MediaSessionMode::Video,
                MediaSessionMode::AudioVideo,
            ]
        );
        assert_eq!(connector.supported_modes(&owner_b).await.len(), 3);
        assert!(connector
            .route_for_sources(&owner_b, &audio_video_request(ROUTE_A, ROUTE_A))
            .await
            .is_ok());

        assert_eq!(
            attachment_a.revoke(Duration::from_millis(250)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
        assert_eq!(
            attachment_b.revoke(Duration::from_millis(250)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
        let wrong_attachment = connector
            .unregister_route_after_media_drain(&owner_a, ROUTE_A, &attachment_b)
            .await
            .expect_err("a different revoked attachment cannot clean this route");
        assert_eq!(wrong_attachment.kind, MediaErrorKind::InvalidRequest);
        assert_eq!(connector.supported_modes(&owner_a).await.len(), 3);
        assert_eq!(connector.supported_modes(&owner_b).await.len(), 3);

        connector
            .unregister_route_after_media_drain(&owner_a, ROUTE_A, &attachment_a)
            .await
            .expect("the matching confirmed attachment owns route cleanup");
        assert!(connector.supported_modes(&owner_a).await.is_empty());
        assert_eq!(
            connector.supported_modes(&owner_b).await.len(),
            3,
            "removing the old attachment route must preserve the same-ID replacement route"
        );
        connector
            .unregister_route_after_media_drain(&owner_b, ROUTE_A, &attachment_b)
            .await
            .expect("the replacement attachment owns its separate route");
        assert!(connector.supported_modes(&owner_b).await.is_empty());
    }

    #[tokio::test]
    async fn nested_or_non_session_attachment_owners_cannot_register_remote_routes() {
        let connector = connector();
        let host = media_runtime::MediaAttachmentHost::new();
        let attachment = host
            .attach(Arc::new(TestMediaService::default()))
            .expect("test attachment");
        let other_attachment = host
            .attach(Arc::new(TestMediaService::default()))
            .expect("second test attachment");
        let inner = owner("same-session").in_attachment(attachment.id().clone());
        let nested = MediaOwner::Attachment {
            attachment_id: attachment.id().clone(),
            owner: Box::new(inner),
        };
        let error = connector
            .register_attachment_route(
                nested,
                ROUTE_A.to_owned(),
                "Viewer phone".to_owned(),
                vec![MediaSessionMode::Audio],
                vec![MediaSourceKindDto::Microphone],
                Arc::new(TestChannel),
                attachment.service(),
            )
            .await
            .expect_err("nested attachment owners are not logical session owners");
        assert_eq!(error.kind, MediaErrorKind::InvalidRequest);

        let wrong_service_owner = owner("same-session").in_attachment(attachment.id().clone());
        let error = connector
            .register_attachment_route(
                wrong_service_owner,
                ROUTE_A.to_owned(),
                "Viewer phone".to_owned(),
                vec![MediaSessionMode::Audio],
                vec![MediaSourceKindDto::Microphone],
                Arc::new(TestChannel),
                other_attachment.service(),
            )
            .await
            .expect_err("the route owner and service must share the exact attachment");
        assert_eq!(error.kind, MediaErrorKind::InvalidRequest);

        let attached_ui = MediaOwner::Ui {
            instance_id: "ui-instance".into(),
        }
        .in_attachment(attachment.id().clone());
        let error = connector
            .register_attachment_route(
                attached_ui,
                ROUTE_A.to_owned(),
                "Viewer phone".to_owned(),
                vec![MediaSessionMode::Audio],
                vec![MediaSourceKindDto::Microphone],
                Arc::new(TestChannel),
                attachment.service(),
            )
            .await
            .expect_err("remote routes require a logical session owner");
        assert_eq!(error.kind, MediaErrorKind::InvalidRequest);

        assert_eq!(
            attachment.revoke(Duration::from_millis(250)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
        assert_eq!(
            other_attachment.revoke(Duration::from_millis(250)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    #[tokio::test]
    async fn unknown_service_stop_is_not_redispatched_by_route_cleanup() {
        let connector = connector();
        let service = Arc::new(TestMediaService::default());
        service.fail_stop.store(true, Ordering::Release);
        let service_trait: Arc<dyn MediaService> = service.clone();
        let owner = owner("session-a");
        register_av(&connector, owner.clone(), ROUTE_A).await;
        let route = connector
            .routes
            .read()
            .await
            .get(&owner)
            .and_then(|routes| routes.get(ROUTE_A))
            .expect("route was registered")
            .clone();
        let session = RemoteHostMediaSession::new(
            MediaSessionRef(MediaSessionId::from("media_unknown_stop")),
            context(owner),
            MediaSessionRequest {
                intent: MediaSessionIntent::Conversation,
                mode: MediaSessionMode::Audio,
                audio_source: Some(MediaSourceRef(
                    remote_media_source_ref(ROUTE_A, RemoteMediaSourceKind::Microphone)
                        .unwrap()
                        .into(),
                )),
                video_source: None,
            },
            route,
            1,
            connector.resources.clone(),
            Some(Arc::downgrade(&service_trait)),
        );

        let first = session
            .stop_from_service()
            .await
            .expect_err("the fake service reports an unknown Stop result");
        assert_eq!(first.kind, MediaErrorKind::NativeFailure);
        let retry = session
            .stop_from_service()
            .await
            .expect_err("an unknown Stop remains fenced without redispatch");
        assert_eq!(retry.kind, MediaErrorKind::Unavailable);
        assert_eq!(service.stop_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn attachment_revoke_drains_service_session_before_peer_route_removal() {
        let connector = connector();
        let host = media_runtime::MediaAttachmentHost::new();
        let service = Arc::new(TestMediaService::default());
        let attachment = host.attach(service.clone()).expect("test attachment");
        let logical_owner = owner("same-session");
        let attached_owner = logical_owner.clone().in_attachment(attachment.id().clone());
        register_av_with_service(
            &connector,
            attached_owner.clone(),
            ROUTE_A,
            attachment.service(),
        )
        .await;
        let route = connector
            .routes
            .read()
            .await
            .get(&attached_owner)
            .and_then(|routes| routes.get(ROUTE_A))
            .expect("attachment route is registered")
            .clone();
        let reference = MediaSessionRef(MediaSessionId::from("media_attachment_session"));
        let request = MediaSessionRequest {
            intent: MediaSessionIntent::Conversation,
            mode: MediaSessionMode::Audio,
            audio_source: Some(MediaSourceRef(
                remote_media_source_ref(ROUTE_A, RemoteMediaSourceKind::Microphone)
                    .unwrap()
                    .into(),
            )),
            video_source: None,
        };
        let route_service = route
            .media_service
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .expect("route retains the exact attachment-scoped service");
        let session = RemoteHostMediaSession::new(
            reference.clone(),
            MediaOperationContext::new(MediaOperationId::new(1, 1), attached_owner.clone()),
            request,
            route.clone(),
            1,
            connector.resources.clone(),
            Some(Arc::downgrade(&route_service)),
        );
        route
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(reference.0 .0.clone(), session.clone());
        attachment
            .service()
            .attach_session(
                MediaOperationContext::new(MediaOperationId::new(2, 1), logical_owner),
                Box::new(RemoteHostMediaSessionHandle(session.clone())),
            )
            .await
            .expect("service session should be admitted through the attachment");

        assert_eq!(service.session_count(), 1);
        assert_eq!(
            attachment.revoke(Duration::from_millis(500)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
        assert_eq!(service.session_count(), 0);
        assert_eq!(session.status().state, MediaSessionState::Ended);
        assert!(connector.supported_modes(&attached_owner).await.len() == 3);

        connector
            .unregister_route_after_media_drain(&attached_owner, ROUTE_A, &attachment)
            .await
            .expect("peer route closes only after its attachment is confirmed revoked");
        assert!(connector.supported_modes(&attached_owner).await.is_empty());
    }

    #[tokio::test]
    async fn selected_sources_cannot_cross_routes_or_invent_screen_sharing() {
        let connector = connector();
        let owner = owner("session-a");
        register_av(&connector, owner.clone(), ROUTE_A).await;
        register_av(&connector, owner.clone(), ROUTE_B).await;

        assert_eq!(
            selected_route_id(&audio_video_request(ROUTE_A, ROUTE_B))
                .unwrap_err()
                .kind,
            MediaErrorKind::InvalidRequest
        );
        let screen_request = MediaSessionRequest {
            intent: MediaSessionIntent::Observation,
            mode: MediaSessionMode::Video,
            audio_source: None,
            video_source: Some(MediaSourceRef(
                remote_media_source_ref(ROUTE_A, RemoteMediaSourceKind::Screen)
                    .unwrap()
                    .into(),
            )),
        };
        let unadvertised_screen = connector.route_for_sources(&owner, &screen_request).await;
        let unadvertised_screen = match unadvertised_screen {
            Ok(_) => panic!("a route must not invent an unregistered screen source"),
            Err(error) => error,
        };
        assert_eq!(unadvertised_screen.kind, MediaErrorKind::NotFound);
    }

    #[tokio::test]
    async fn route_registration_rejects_modes_without_their_real_sources() {
        let connector = connector();
        let result = connector
            .register_route(
                owner("session-a"),
                ROUTE_A.to_owned(),
                "viewer".to_owned(),
                vec![MediaSessionMode::AudioVideo],
                vec![MediaSourceKindDto::Microphone],
                Arc::new(TestChannel),
            )
            .await;
        assert_eq!(result.unwrap_err().kind, MediaErrorKind::InvalidRequest);
        assert!(connector
            .sources(context(owner("session-a")))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn unregister_is_owner_scoped_and_stops_only_its_route_sessions() {
        let connector = connector();
        let owner_a = owner("session-a");
        let owner_b = owner("session-b");
        register_av(&connector, owner_a.clone(), ROUTE_A).await;
        register_av(&connector, owner_b.clone(), ROUTE_A).await;
        let route_a = connector
            .routes
            .read()
            .await
            .get(&owner_a)
            .and_then(|routes| routes.get(ROUTE_A))
            .unwrap()
            .clone();
        let request = MediaSessionRequest {
            intent: MediaSessionIntent::Conversation,
            mode: MediaSessionMode::Audio,
            audio_source: Some(MediaSourceRef(
                remote_media_source_ref(ROUTE_A, RemoteMediaSourceKind::Microphone)
                    .unwrap()
                    .into(),
            )),
            video_source: None,
        };
        let session = RemoteHostMediaSession::new(
            MediaSessionRef(MediaSessionId::from("media_session_test")),
            context(owner_a.clone()),
            request,
            route_a.clone(),
            7,
            connector.resources.clone(),
            None,
        );
        route_a
            .sessions
            .lock()
            .unwrap()
            .insert(session.reference.0 .0.clone(), session.clone());

        connector.unregister_route(&owner_a, ROUTE_A).await.unwrap();
        assert_eq!(session.status().state, MediaSessionState::Ended);
        assert!(connector
            .sources(context(owner_a))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(connector.sources(context(owner_b)).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn unregistered_duplicate_cleanup_cannot_stop_registered_session() {
        let connector = connector();
        let service = Arc::new(TestMediaService::default());
        let service_trait: Arc<dyn MediaService> = service.clone();
        connector.bind_media_service(service_trait.clone());
        let owner = owner("session-a");
        register_av(&connector, owner.clone(), ROUTE_A).await;
        let route = connector
            .routes
            .read()
            .await
            .get(&owner)
            .and_then(|routes| routes.get(ROUTE_A))
            .expect("route was registered")
            .clone();
        let reference = MediaSessionRef(MediaSessionId::from("media_session_duplicate"));
        let request = MediaSessionRequest {
            intent: MediaSessionIntent::Observation,
            mode: MediaSessionMode::Video,
            audio_source: None,
            video_source: Some(MediaSourceRef(
                remote_media_source_ref(ROUTE_A, RemoteMediaSourceKind::Camera)
                    .unwrap()
                    .into(),
            )),
        };
        let session_context = context(owner.clone());
        let registered = RemoteHostMediaSession::new(
            reference.clone(),
            session_context.clone(),
            request.clone(),
            route.clone(),
            1,
            connector.resources.clone(),
            Some(Arc::downgrade(&service_trait)),
        );
        service
            .attach_session(
                session_context.clone(),
                Box::new(RemoteHostMediaSessionHandle(registered.clone())),
            )
            .await
            .expect("first session registers");
        route
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(reference.0 .0.clone(), registered.clone());

        let unregistered = RemoteHostMediaSession::new(
            reference,
            session_context.clone(),
            request,
            route.clone(),
            2,
            connector.resources.clone(),
            Some(Arc::downgrade(&service_trait)),
        );
        // This models the cleanup lease after service attach rejects a
        // duplicate reference. It must stop only the new native peer.
        unregistered.stop_internal().await.unwrap();
        tokio::task::yield_now().await;
        assert_eq!(service.session_count(), 1);
        assert_eq!(registered.status().state, MediaSessionState::Preparing);

        connector.unregister_route(&owner, ROUTE_A).await.unwrap();
        assert_eq!(service.session_count(), 0);
    }

    #[tokio::test]
    async fn cancelled_remote_pump_join_keeps_handle_for_cleanup_retry() {
        let release = Arc::new(Notify::new());
        let completed = Arc::new(AtomicBool::new(false));
        let task_release = release.clone();
        let task_completed = completed.clone();
        let pump = tokio::spawn(async move {
            task_release.notified().await;
            task_completed.store(true, Ordering::Release);
        });
        let pump_slot = Arc::new(AsyncMutex::new(Some(pump)));
        let joining = Arc::new(Notify::new());
        let join_slot = pump_slot.clone();
        let join_started = joining.clone();
        let stop_attempt = tokio::spawn(async move {
            let mut slot = join_slot.lock().await;
            if let Some(task) = slot.as_mut() {
                join_started.notify_one();
                let result = join_media_task(task, "test remote media pump").await;
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
        join_media_task(task, "test remote media pump")
            .await
            .unwrap();
        slot.take();
        assert!(completed.load(Ordering::Acquire));
        assert!(slot.is_none());
    }

    #[tokio::test]
    async fn repeated_route_teardown_deregisters_attached_sessions_past_service_capacity() {
        let connector = connector();
        let service = Arc::new(TestMediaService::default());
        let service_trait: Arc<dyn MediaService> = service.clone();
        connector.bind_media_service(service_trait.clone());
        let owner = owner("session-a");

        for index in 0..65usize {
            register_av(&connector, owner.clone(), ROUTE_A).await;
            let route = connector
                .routes
                .read()
                .await
                .get(&owner)
                .and_then(|routes| routes.get(ROUTE_A))
                .expect("route was registered")
                .clone();
            let request = MediaSessionRequest {
                intent: MediaSessionIntent::Observation,
                mode: MediaSessionMode::Video,
                audio_source: None,
                video_source: Some(MediaSourceRef(
                    remote_media_source_ref(ROUTE_A, RemoteMediaSourceKind::Camera)
                        .unwrap()
                        .into(),
                )),
            };
            let context = MediaOperationContext::new(
                MediaOperationId::new(u64::try_from(index + 1).unwrap(), 1),
                owner.clone(),
            );
            let reference =
                MediaSessionRef(MediaSessionId::from(format!("media_session_cycle_{index}")));
            let session = RemoteHostMediaSession::new(
                reference.clone(),
                context.clone(),
                request,
                route.clone(),
                u64::try_from(index + 1).unwrap(),
                connector.resources.clone(),
                Some(Arc::downgrade(&service_trait)),
            );
            service
                .attach_session(
                    context,
                    Box::new(RemoteHostMediaSessionHandle(session.clone())),
                )
                .await
                .expect("incoming session is tracked by the shared service");
            route
                .sessions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(reference.0 .0.clone(), session);

            connector.unregister_route(&owner, ROUTE_A).await.unwrap();
            assert_eq!(service.session_count(), 0, "cycle {index} leaked a session");
            assert!(route
                .sessions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty());
        }
    }

    #[tokio::test]
    async fn failed_remote_close_retains_session_for_a_real_retry() {
        let connector = connector();
        let owner = owner("session-a");
        let channel = Arc::new(RetryCloseChannel {
            close_calls: std::sync::atomic::AtomicUsize::new(0),
        });
        connector
            .register_route(
                owner.clone(),
                ROUTE_A.to_owned(),
                "viewer".to_owned(),
                vec![MediaSessionMode::Audio],
                vec![MediaSourceKindDto::Microphone],
                channel.clone(),
            )
            .await
            .unwrap();
        let route = connector
            .routes
            .read()
            .await
            .get(&owner)
            .and_then(|routes| routes.get(ROUTE_A))
            .unwrap()
            .clone();
        let request = MediaSessionRequest {
            intent: MediaSessionIntent::Conversation,
            mode: MediaSessionMode::Audio,
            audio_source: Some(MediaSourceRef(
                remote_media_source_ref(ROUTE_A, RemoteMediaSourceKind::Microphone)
                    .unwrap()
                    .into(),
            )),
            video_source: None,
        };
        let session = RemoteHostMediaSession::new(
            MediaSessionRef(MediaSessionId::from("media_retry_test")),
            context(owner),
            request,
            route.clone(),
            7,
            connector.resources.clone(),
            None,
        );
        route
            .sessions
            .lock()
            .unwrap()
            .insert(session.reference.0 .0.clone(), session.clone());

        assert_eq!(
            session.stop_internal().await.unwrap_err().kind,
            MediaErrorKind::Unavailable
        );
        assert_eq!(session.status().state, MediaSessionState::Failed);
        assert!(route
            .sessions
            .lock()
            .unwrap()
            .contains_key(&session.reference.0 .0));

        session.stop_internal().await.unwrap();
        assert_eq!(session.status().state, MediaSessionState::Ended);
        assert!(route.sessions.lock().unwrap().is_empty());
        assert_eq!(channel.close_calls.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn interrupt_control_bypasses_pending_flush_and_discards_its_late_receipt() {
        let (control_tx, mut control_rx) = tokio::sync::mpsc::unbounded_channel();
        let route = Arc::new(Route {
            id: ROUTE_A.to_owned(),
            label: "viewer".to_owned(),
            supported_modes: vec![MediaSessionMode::Audio],
            source_kinds: vec![RemoteMediaSourceKind::Microphone],
            channel: Arc::new(PendingBarrierChannel {
                requests: control_tx,
            }),
            media_service: None,
            closed: AtomicBool::new(false),
            unregistering: AtomicBool::new(false),
            sessions: Mutex::new(HashMap::new()),
        });
        let (commands, mut command_rx) = mpsc::channel(8);
        let (input_tx, input) = mpsc::channel(1);
        drop(input_tx);
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                match command {
                    WorkerAudioCommand::Write { reply, .. } => {
                        let _ = reply.send(Ok(MediaRenderReceipt {
                            accepted_sample_frames: 480,
                            played_sample_frames: 0,
                            rendered_at_unix_ms: None,
                        }));
                    }
                    WorkerAudioCommand::Flush { reply }
                    | WorkerAudioCommand::Interrupt { reply } => {
                        let _ = reply.send(Ok(AudioTarget {
                            sequence: 1,
                            sample_frames: 960,
                        }));
                    }
                }
            }
        });
        let channel = Arc::new(RemoteHostMediaAudioChannel {
            commands,
            input: AsyncMutex::new(input),
            request_gate: AsyncMutex::new(()),
            context: context(owner("session-a")),
            reference: MediaSessionRef(MediaSessionId::from("media_barrier_test")),
            generation: 9,
            route,
            enabled_audio: Arc::new(AtomicBool::new(true)),
            stopping: Arc::new(AtomicBool::new(false)),
            control_epoch: AtomicU64::new(0),
            last_barrier_target: Mutex::new(AudioTarget {
                sequence: 0,
                sample_frames: 0,
            }),
            last_completed_target: Mutex::new(AudioTarget {
                sequence: 0,
                sample_frames: 0,
            }),
            source_geometry: Mutex::new(Some(AudioSourceGeometry {
                sample_rate_hz: 24_000,
                channels: 1,
            })),
            accepted_source_sample_frames: AtomicU64::new(480),
            played_rtp_sample_frames: AtomicU64::new(0),
            played_source_sample_frames: AtomicU64::new(0),
        });

        let pending_flush_channel = channel.clone();
        let flush = tokio::spawn(async move { pending_flush_channel.flush().await });
        let (flush_request, flush_reply) = control_rx.recv().await.unwrap();
        assert!(matches!(
            flush_request.action,
            MediaControlActionDto::FlushAudio { .. }
        ));

        let interrupt_channel = channel.clone();
        let interrupt = tokio::spawn(async move { interrupt_channel.interrupt().await });
        let (interrupt_request, interrupt_reply) =
            tokio::time::timeout(Duration::from_millis(100), control_rx.recv())
                .await
                .expect("interrupt must reach the remote peer while flush is pending")
                .unwrap();
        assert!(matches!(
            interrupt_request.action,
            MediaControlActionDto::InterruptAudio { .. }
        ));
        interrupt_reply
            .send(Ok(control_response(&interrupt_request, 480)))
            .unwrap();
        let receipt = interrupt.await.unwrap().unwrap();
        assert_eq!(
            receipt.played_sample_frames, 240,
            "480 played RTP frames map to source-rate frames"
        );

        flush_reply
            .send(Ok(control_response(&flush_request, 960)))
            .unwrap();
        assert_eq!(
            flush.await.unwrap().unwrap_err().kind,
            MediaErrorKind::Cancelled
        );
    }

    fn control_request(generation: u64) -> MediaControlRequestDto {
        MediaControlRequestDto {
            request_id: "control-request-1".to_owned(),
            connection_id: ROUTE_A.to_owned(),
            media_session_ref: MediaSessionRefDto {
                id: "media_session_test".to_owned(),
            },
            generation,
            action: MediaControlActionDto::FlushAudio {
                target_sequence: 2,
                target_sample_frames: 1_920,
            },
        }
    }

    fn control_response(request: &MediaControlRequestDto, played: u32) -> MediaControlResponseDto {
        MediaControlResponseDto {
            request_id: request.request_id.clone(),
            connection_id: request.connection_id.clone(),
            media_session_ref: request.media_session_ref.clone(),
            generation: request.generation,
            receipt: Some(MediaRenderReceiptDto {
                accepted_sample_frames: 0,
                played_sample_frames: played,
                rendered_at_unix_ms: Some(1_800_000_000_000),
            }),
            error: None,
        }
    }

    fn control_close_response(request: &MediaControlRequestDto) -> MediaControlResponseDto {
        MediaControlResponseDto {
            request_id: request.request_id.clone(),
            connection_id: request.connection_id.clone(),
            media_session_ref: request.media_session_ref.clone(),
            generation: request.generation,
            receipt: Some(MediaRenderReceiptDto {
                accepted_sample_frames: 0,
                played_sample_frames: 0,
                rendered_at_unix_ms: None,
            }),
            error: None,
        }
    }

    #[test]
    fn control_receipts_require_exact_generation_and_actual_bounded_progress() {
        let request = control_request(7);
        let receipt = control_receipt(&request, control_response(&request, 960), 1_920).unwrap();
        assert_eq!(receipt.accepted_sample_frames, 0);
        assert_eq!(receipt.played_sample_frames, 960);
        assert_eq!(receipt.rendered_at_unix_ms, Some(1_800_000_000_000));

        let mut stale = control_response(&request, 960);
        stale.generation = 6;
        assert_eq!(
            control_receipt(&request, stale, 1_920).unwrap_err().kind,
            MediaErrorKind::InvalidRequest
        );
        assert_eq!(
            control_receipt(&request, control_response(&request, 1_921), 1_920)
                .unwrap_err()
                .kind,
            MediaErrorKind::InvalidRequest
        );

        let invalid_target = AudioTarget {
            sequence: 2,
            sample_frames: 1_000,
        };
        assert_eq!(
            validate_audio_target(
                invalid_target,
                AudioTarget {
                    sequence: 0,
                    sample_frames: 0
                }
            )
            .unwrap_err()
            .kind,
            MediaErrorKind::InvalidRequest
        );
        assert!(validate_audio_target(
            AudioTarget {
                sequence: 2,
                sample_frames: 1_920
            },
            AudioTarget {
                sequence: 1,
                sample_frames: 960
            },
        )
        .is_ok());
        assert!(validate_audio_target(
            AudioTarget {
                sequence: 1,
                sample_frames: 960
            },
            AudioTarget {
                sequence: 2,
                sample_frames: 1_920
            },
        )
        .is_err());
    }

    #[test]
    fn video_observation_samples_over_time_without_a_lifetime_frame_cap() {
        let start = Instant::now();
        let mut last = None;
        let mut observed = 0;
        for frame in 0..360 {
            let at = start + Duration::from_millis(frame * 33);
            if should_sample_video_frame(&mut last, at) {
                observed += 1;
            }
        }
        assert!((10..=12).contains(&observed));
        assert!(!should_sample_video_frame(
            &mut last,
            start + Duration::from_secs(10),
        ));
        assert!(should_sample_video_frame(
            &mut last,
            start + Duration::from_secs(13),
        ));
    }

    #[test]
    fn audio_barrier_receipts_map_rtp_frames_back_to_the_fixed_source_geometry() {
        let partial_interrupt = rtp_sample_frames_to_source_frames(960, 24_000, 500);
        assert_eq!(partial_interrupt, 480);
        let padded_flush_tail = rtp_sample_frames_to_source_frames(1_920, 24_000, 500);
        assert_eq!(
            padded_flush_tail.saturating_sub(partial_interrupt),
            20,
            "the padded tail is clamped to only 20 source samples beyond the partial interrupt",
        );
        assert_eq!(rtp_sample_frames_to_source_frames(1_440, 16_000, 800), 480);
        assert_eq!(rtp_sample_frames_to_source_frames(1, 44_100, 8), 0);
        assert_eq!(rtp_sample_frames_to_source_frames(2, 44_100, 8), 1);
        assert_eq!(audio_tail_padding_samples(20, 24_000), 460);
        assert_eq!(audio_tail_padding_samples(480, 24_000), 0);
    }
}
