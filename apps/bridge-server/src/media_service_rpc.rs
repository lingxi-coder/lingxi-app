//! Conversion and dispatch for authenticated host-only MediaService RPCs.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use client::protocol::media as dto;
use media_api::{
    MediaAsset, MediaContent, MediaError, MediaErrorKind, MediaEvent, MediaEventBatch,
    MediaEventCursor, MediaEventKind, MediaInitiator, MediaKind, MediaMetadata, MediaOperation,
    MediaOperationContext, MediaOperationId, MediaOwner, MediaRef, MediaResourceRef,
    MediaResourceState, MediaResourceStatus, MediaResult, MediaService, MediaSessionIntent,
    MediaSessionMode, MediaSessionRef, MediaSessionRequest, MediaSessionState, MediaSessionStatus,
    MediaSessionUpdate, MediaSource, MediaSourceKind, MediaSourceRef, MediaSourceUsage,
    RecordOptions, RecordingRef, TranscriptionInput,
};
use tokio::sync::{oneshot, Mutex, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

const MAX_IN_FLIGHT_REQUESTS: usize = 16;
const MAX_RETAINED_REQUEST_IDS: usize = 512;
const MAX_DUPLICATE_WAITERS_PER_REQUEST: usize = 8;
const MAX_CONTROL_REQUESTS: usize = 8;
const DEFAULT_MEDIA_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Owner-bound service RPC dispatcher. The BridgeConnection supplies the
/// trusted owner selected when it built the Desktop runtime.
pub struct MediaServiceRpc {
    service: Arc<dyn MediaService>,
    app_host: Option<Arc<audio_provider::media_host::AppMediaHost>>,
    owner: RwLock<MediaOwner>,
    owner_gate: RwLock<()>,
    owner_transition: RwLock<CancellationToken>,
    remote_connector: Option<Arc<remote_control_client::host_media::RemoteMediaConnector>>,
    media_connection_channel: Option<Arc<crate::media_bridge::BridgeMediaConnectionChannel>>,
    media_control_channel: Option<Arc<crate::media_bridge::BridgeMediaControlChannel>>,
    registered_routes: Mutex<HashSet<(MediaOwner, String)>>,
    generation: AtomicU64,
    active_operations: Arc<Mutex<HashSet<MediaOperationId>>>,
    closed: AtomicBool,
    closing: CancellationToken,
    request_state: Mutex<RequestState>,
    in_flight: Arc<Semaphore>,
    control_in_flight: Arc<Semaphore>,
}

#[derive(Default)]
struct RequestState {
    requests: HashMap<String, RequestEntry>,
    completed_order: VecDeque<String>,
}

struct RequestEntry {
    fingerprint: String,
    status: RequestStatus,
}

enum RequestStatus {
    Running(Vec<oneshot::Sender<dto::MediaServiceResponseDto>>),
    Completed(dto::MediaServiceResponseDto),
}

/// Cancels an operation if its dispatch future is dropped before it reaches a
/// terminal result. The connection close path also drains this identity set;
/// cancellation is idempotent at the shared MediaService boundary.
struct ActiveOperationGuard {
    service: Arc<dyn MediaService>,
    active_operations: Arc<Mutex<HashSet<MediaOperationId>>>,
    identity: Option<MediaOperationId>,
}

impl ActiveOperationGuard {
    fn new(
        service: Arc<dyn MediaService>,
        active_operations: Arc<Mutex<HashSet<MediaOperationId>>>,
        identity: MediaOperationId,
    ) -> Self {
        Self {
            service,
            active_operations,
            identity: Some(identity),
        }
    }

    async fn finish(mut self, cancel: bool) {
        if let Some(identity) = self.identity.take() {
            self.active_operations.lock().await.remove(&identity);
            if cancel {
                let _ = self.service.cancel(identity).await;
            }
        }
    }
}

impl Drop for ActiveOperationGuard {
    fn drop(&mut self) {
        let Some(identity) = self.identity.take() else {
            return;
        };
        let service = self.service.clone();
        let active_operations = self.active_operations.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                active_operations.lock().await.remove(&identity);
                let _ = service.cancel(identity).await;
            });
        }
    }
}

impl MediaServiceRpc {
    /// Bind an exact app-scoped MediaService to this authenticated desktop
    /// session. The owner never comes from a renderer or RPC request body.
    #[must_use]
    pub fn new(service: Arc<dyn MediaService>, owner: MediaOwner) -> Arc<Self> {
        Arc::new(Self {
            service,
            app_host: None,
            owner: RwLock::new(owner),
            owner_gate: RwLock::new(()),
            owner_transition: RwLock::new(CancellationToken::new()),
            remote_connector: None,
            media_connection_channel: None,
            media_control_channel: None,
            registered_routes: Mutex::new(HashSet::new()),
            generation: std::sync::atomic::AtomicU64::new(0),
            active_operations: Arc::new(Mutex::new(HashSet::new())),
            closed: AtomicBool::new(false),
            closing: CancellationToken::new(),
            request_state: Mutex::new(RequestState::default()),
            in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS)),
            control_in_flight: Arc::new(Semaphore::new(MAX_CONTROL_REQUESTS)),
        })
    }

    /// Bind the exact app-scoped host when settings/route control actions are
    /// available on the authenticated Bridge connection.
    #[must_use]
    pub fn for_app_host(
        app_host: Arc<audio_provider::media_host::AppMediaHost>,
        owner: MediaOwner,
    ) -> Arc<Self> {
        Arc::new(Self {
            service: app_host.media_service(),
            app_host: Some(app_host),
            owner: RwLock::new(owner),
            owner_gate: RwLock::new(()),
            owner_transition: RwLock::new(CancellationToken::new()),
            remote_connector: None,
            media_connection_channel: None,
            media_control_channel: None,
            registered_routes: Mutex::new(HashSet::new()),
            generation: AtomicU64::new(0),
            active_operations: Arc::new(Mutex::new(HashSet::new())),
            closed: AtomicBool::new(false),
            closing: CancellationToken::new(),
            request_state: Mutex::new(RequestState::default()),
            in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS)),
            control_in_flight: Arc::new(Semaphore::new(MAX_CONTROL_REQUESTS)),
        })
    }

    /// Bind the exact app host and the single installed product connector to
    /// this authenticated Bridge connection. Route registration attaches only
    /// that connection's host-control channels to the aggregate.
    #[must_use]
    pub fn for_app_host_with_remote_connector(
        app_host: Arc<audio_provider::media_host::AppMediaHost>,
        owner: MediaOwner,
        remote_connector: Arc<remote_control_client::host_media::RemoteMediaConnector>,
        media_connection_channel: Arc<crate::media_bridge::BridgeMediaConnectionChannel>,
        media_control_channel: Arc<crate::media_bridge::BridgeMediaControlChannel>,
    ) -> Arc<Self> {
        Arc::new(Self {
            service: app_host.media_service(),
            app_host: Some(app_host),
            owner: RwLock::new(owner),
            owner_gate: RwLock::new(()),
            owner_transition: RwLock::new(CancellationToken::new()),
            remote_connector: Some(remote_connector),
            media_connection_channel: Some(media_connection_channel),
            media_control_channel: Some(media_control_channel),
            registered_routes: Mutex::new(HashSet::new()),
            generation: AtomicU64::new(0),
            active_operations: Arc::new(Mutex::new(HashSet::new())),
            closed: AtomicBool::new(false),
            closing: CancellationToken::new(),
            request_state: Mutex::new(RequestState::default()),
            in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS)),
            control_in_flight: Arc::new(Semaphore::new(MAX_CONTROL_REQUESTS)),
        })
    }

    /// Project the authenticated Desktop's explicit native media-session
    /// modes. These come from its shared MediaDeviceSession adapter, never from
    /// the legacy AudioService operation catalog.
    pub fn update_local_session_modes(&self, modes: &[dto::MediaSessionModeDto]) {
        let Some(app_host) = self.app_host.as_ref() else {
            return;
        };
        if modes.len() > 3
            || modes
                .iter()
                .enumerate()
                .any(|(index, mode)| modes[..index].contains(mode))
        {
            tracing::warn!("ignoring invalid native media-session mode inventory");
            return;
        }
        let modes = modes
            .iter()
            .map(|mode| match mode {
                dto::MediaSessionModeDto::Audio => MediaSessionMode::Audio,
                dto::MediaSessionModeDto::Video => MediaSessionMode::Video,
                dto::MediaSessionModeDto::AudioVideo => MediaSessionMode::AudioVideo,
            })
            .collect();
        if let Err(error) = app_host.update_local_session_modes(modes) {
            tracing::warn!(error = %error, "could not apply native media-session modes");
        }
    }

    /// Handle one host-only operation, read or cancellation request.
    pub async fn handle(
        &self,
        request: dto::MediaServiceRequestDto,
    ) -> dto::MediaServiceResponseDto {
        if let Err(message) = request.validate() {
            return service_error(
                request.request_id,
                MediaError::new(MediaErrorKind::InvalidRequest, message),
            );
        }
        // Keep the trusted owner stable across deduplication and the full
        // operation. Session transitions take the write side before changing
        // this owner, so no request can straddle two attached sessions.
        let _owner_gate = self.owner_gate.read().await;
        let owner = self.owner.read().await.clone();
        let owner_transition = self.owner_transition.read().await.clone();
        let request_id = request.request_id.clone();
        let fingerprint = match serde_json::to_string(&request.action) {
            Ok(fingerprint) => fingerprint,
            Err(_) => {
                return service_error(
                    request_id,
                    MediaError::new(
                        MediaErrorKind::InvalidRequest,
                        "media request could not be fingerprinted",
                    ),
                )
            }
        };
        let (tx, rx) = oneshot::channel();
        let leader = {
            let mut state = self.request_state.lock().await;
            if self.closed.load(Ordering::Acquire) {
                return service_error(
                    request_id,
                    MediaError::new(
                        MediaErrorKind::Unavailable,
                        "the media host connection is closing",
                    ),
                );
            }
            if let Some(existing) = state.requests.get_mut(&request_id) {
                if existing.fingerprint != fingerprint {
                    return service_error(
                        request_id,
                        MediaError::new(
                            MediaErrorKind::InvalidRequest,
                            "media request ID was replayed with different parameters",
                        ),
                    );
                }
                match &mut existing.status {
                    RequestStatus::Completed(response) => return response.clone(),
                    RequestStatus::Running(waiters) => {
                        if waiters.len() >= MAX_DUPLICATE_WAITERS_PER_REQUEST {
                            return service_error(
                                request_id,
                                MediaError::new(
                                    MediaErrorKind::Busy,
                                    "too many callers are waiting on this media request",
                                ),
                            );
                        }
                        waiters.push(tx);
                        false
                    }
                }
            } else {
                while state.requests.len() >= MAX_RETAINED_REQUEST_IDS {
                    let Some(oldest) = state.completed_order.pop_front() else {
                        return service_error(
                            request_id,
                            MediaError::new(
                                MediaErrorKind::Busy,
                                "media request correlation capacity is full",
                            ),
                        );
                    };
                    state.requests.remove(&oldest);
                }
                state.requests.insert(
                    request_id.clone(),
                    RequestEntry {
                        fingerprint,
                        status: RequestStatus::Running(vec![tx]),
                    },
                );
                true
            }
        };
        if !leader {
            return rx.await.unwrap_or_else(|_| {
                service_error(
                    request_id,
                    MediaError::new(
                        MediaErrorKind::Unavailable,
                        "media request owner closed before its response",
                    ),
                )
            });
        }
        let permit = if matches!(&request.action, dto::MediaServiceActionDto::Cancel { .. }) {
            match self.control_in_flight.clone().try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(_) => {
                    let response = service_error(
                        request_id.clone(),
                        MediaError::new(
                            MediaErrorKind::Busy,
                            "too many media control requests are active",
                        ),
                    );
                    self.complete_request(request_id, response.clone()).await;
                    return response;
                }
            }
        } else {
            match self.in_flight.clone().try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(_) => {
                    let response = service_error(
                        request_id.clone(),
                        MediaError::new(
                            MediaErrorKind::Busy,
                            "too many media service requests are active",
                        ),
                    );
                    self.complete_request(request_id, response.clone()).await;
                    return response;
                }
            }
        };
        let response = match request.action {
            dto::MediaServiceActionDto::Execute {
                operation,
                timeout_budget_ms,
                max_payload_bytes,
            } => {
                self.execute(
                    request_id.clone(),
                    owner.clone(),
                    owner_transition.clone(),
                    operation,
                    timeout_budget_ms,
                    max_payload_bytes,
                )
                .await
            }
            dto::MediaServiceActionDto::ReadMedia { media_ref } => {
                self.read_media(
                    request_id.clone(),
                    owner.clone(),
                    owner_transition.clone(),
                    media_ref,
                )
                .await
            }
            dto::MediaServiceActionDto::Cancel { operation_id } => {
                self.cancel(request_id.clone(), operation_id).await
            }
            dto::MediaServiceActionDto::UpdateAudioSettings { configuration_json } => {
                self.update_audio_settings(request_id.clone(), &configuration_json)
            }
            dto::MediaServiceActionDto::RegisterConnection {
                connection_id,
                label,
                supported_modes,
                source_kinds,
            } => {
                self.register_connection(
                    request_id.clone(),
                    owner,
                    connection_id,
                    label,
                    supported_modes,
                    source_kinds,
                )
                .await
            }
            dto::MediaServiceActionDto::UnregisterConnection { connection_id } => {
                self.unregister_connection(request_id.clone(), owner, connection_id)
                    .await
            }
        };
        drop(permit);
        let response = if response.validate().is_ok() {
            response
        } else {
            service_error(
                request_id.clone(),
                MediaError::new(
                    MediaErrorKind::NativeFailure,
                    "media host produced an invalid or oversized response",
                ),
            )
        };
        self.complete_request(request_id, response.clone()).await;
        response
    }

    fn update_audio_settings(
        &self,
        request_id: String,
        configuration_json: &str,
    ) -> dto::MediaServiceResponseDto {
        let Some(app_host) = self.app_host.as_ref() else {
            return service_error(
                request_id,
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "the app-scoped media host cannot update audio settings",
                ),
            );
        };
        match app_host.update_audio_routing_v4(configuration_json) {
            Ok(()) => dto::MediaServiceResponseDto {
                request_id,
                result: Some(dto::MediaServiceResultDto::SettingsUpdated),
                error: None,
            },
            Err(error) => service_error(
                request_id,
                MediaError::new(MediaErrorKind::InvalidRequest, error),
            ),
        }
    }

    async fn complete_request(&self, request_id: String, response: dto::MediaServiceResponseDto) {
        let waiters = {
            let mut state = self.request_state.lock().await;
            let Some(entry) = state.requests.get_mut(&request_id) else {
                return;
            };
            let waiters = match std::mem::replace(
                &mut entry.status,
                RequestStatus::Completed(response.clone()),
            ) {
                RequestStatus::Running(waiters) => waiters,
                RequestStatus::Completed(_) => Vec::new(),
            };
            state.completed_order.push_back(request_id);
            waiters
        };
        for waiter in waiters {
            let _ = waiter.send(response.clone());
        }
    }

    async fn register_connection(
        &self,
        request_id: String,
        owner: MediaOwner,
        connection_id: String,
        label: String,
        supported_modes: Vec<dto::MediaSessionModeDto>,
        source_kinds: Vec<dto::MediaSourceKindDto>,
    ) -> dto::MediaServiceResponseDto {
        let (Some(connector), Some(connection_channel), Some(control_channel)) = (
            self.remote_connector.as_ref(),
            self.media_connection_channel.as_ref(),
            self.media_control_channel.as_ref(),
        ) else {
            return service_error(
                request_id,
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "Remote Control media is not configured on this Bridge connection",
                ),
            );
        };
        let modes = supported_modes
            .into_iter()
            .map(session_mode_from_dto)
            .collect::<Vec<_>>();
        let channel = crate::media_bridge::BridgeMediaHostControlChannel::for_route(
            connection_id.clone(),
            connection_channel.clone(),
            control_channel.clone(),
        );
        let registered = connector
            .register_route(
                owner.clone(),
                connection_id.clone(),
                label,
                modes,
                source_kinds,
                channel,
            )
            .await;
        match registered {
            Ok(()) => {
                self.registered_routes
                    .lock()
                    .await
                    .insert((owner.clone(), connection_id));
                if let Some(app_host) = self.app_host.as_ref() {
                    app_host.update_remote_session_modes(connector.supported_modes(&owner).await);
                }
                dto::MediaServiceResponseDto {
                    request_id,
                    result: Some(dto::MediaServiceResultDto::ConnectionUpdated),
                    error: None,
                }
            }
            Err(error) => service_error(request_id, error),
        }
    }

    async fn unregister_connection(
        &self,
        request_id: String,
        owner: MediaOwner,
        connection_id: String,
    ) -> dto::MediaServiceResponseDto {
        let Some(connector) = self.remote_connector.as_ref() else {
            return service_error(
                request_id,
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "Remote Control media is not configured on this Bridge connection",
                ),
            );
        };
        let unregistered = connector.unregister_route(&owner, &connection_id).await;
        if let Err(error) = unregistered {
            return service_error(request_id, error);
        }
        self.registered_routes
            .lock()
            .await
            .remove(&(owner.clone(), connection_id));
        if let Some(app_host) = self.app_host.as_ref() {
            app_host.update_remote_session_modes(connector.supported_modes(&owner).await);
        }
        dto::MediaServiceResponseDto {
            request_id,
            result: Some(dto::MediaServiceResultDto::ConnectionUpdated),
            error: None,
        }
    }

    /// Replace the trusted session owner only after an admitted session
    /// transition completes. Existing owner resources and remote routes are
    /// stopped before requests can use the new owner snapshot.
    pub async fn update_owner(&self, owner: MediaOwner) {
        if self.owner.read().await.eq(&owner) {
            return;
        }
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        self.owner_transition.read().await.cancel();
        self.cancel_active_operations().await;
        let _owner_gate = self.owner_gate.write().await;
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let previous = self.owner.read().await.clone();
        if previous == owner {
            *self.owner_transition.write().await = CancellationToken::new();
            return;
        }
        self.stop_owner_resources(&previous).await;
        self.unregister_owner_routes(&previous).await;
        *self.owner.write().await = owner.clone();
        if let (Some(app_host), Some(connector)) =
            (self.app_host.as_ref(), self.remote_connector.as_ref())
        {
            app_host.update_remote_session_modes(connector.supported_modes(&owner).await);
        }
        *self.owner_transition.write().await = CancellationToken::new();
        let mut state = self.request_state.lock().await;
        state.requests.clear();
        state.completed_order.clear();
    }

    async fn unregister_owner_routes(&self, owner: &MediaOwner) {
        let Some(connector) = self.remote_connector.as_ref() else {
            return;
        };
        let route_ids = {
            let mut routes = self.registered_routes.lock().await;
            let ids = routes
                .iter()
                .filter_map(|(route_owner, connection_id)| {
                    (route_owner == owner).then(|| connection_id.clone())
                })
                .collect::<Vec<_>>();
            routes.retain(|(route_owner, _)| route_owner != owner);
            ids
        };
        for connection_id in route_ids {
            if let Err(error) = connector.unregister_route(owner, &connection_id).await {
                tracing::warn!(%error, %connection_id, "media route cleanup failed during session transition");
            }
        }
    }

    async fn cancel_active_operations(&self) {
        let identities = {
            let mut active = self.active_operations.lock().await;
            active.drain().collect::<Vec<_>>()
        };
        for identity in identities {
            if let Err(error) = self.service.cancel(identity).await {
                tracing::warn!(%error, "media operation cancellation failed during owner transition");
            }
        }
    }

    async fn stop_owner_resources(&self, owner: &MediaOwner) {
        let Some(generation) = self.next_generation() else {
            tracing::warn!("media generation exhausted before owner cleanup");
            return;
        };
        let caps = self.service.capabilities_for(owner);
        let context = MediaOperationContext {
            identity: MediaOperationId::new(generation, caps.service_epoch),
            owner: owner.clone(),
            initiator: None,
            timeout_budget_ms: Some(5_000),
            max_payload_bytes: caps.max_payload_bytes,
        };
        let statuses = tokio::time::timeout(
            Duration::from_secs(5),
            self.service
                .execute(context.clone(), MediaOperation::Status { target: None }),
        )
        .await;
        let Ok(Ok(MediaResult::Status { resources })) = statuses else {
            return;
        };
        for resource in resources.into_iter().filter(|resource| {
            matches!(
                resource.state,
                MediaResourceState::Active | MediaResourceState::Stopping
            )
        }) {
            let Some(generation) = self.next_generation() else {
                tracing::warn!("media generation exhausted while stopping owner resources");
                break;
            };
            let stop_context = MediaOperationContext {
                identity: MediaOperationId::new(generation, caps.service_epoch),
                owner: owner.clone(),
                initiator: None,
                timeout_budget_ms: Some(5_000),
                max_payload_bytes: caps.max_payload_bytes,
            };
            match tokio::time::timeout(
                Duration::from_secs(5),
                self.service.execute(
                    stop_context,
                    MediaOperation::Stop {
                        target: resource.resource,
                    },
                ),
            )
            .await
            {
                Ok(Ok(MediaResult::Stopped { .. })) => {}
                Ok(Ok(_)) => tracing::warn!(
                    "media resource stop returned an unexpected result during owner cleanup"
                ),
                Ok(Err(error)) => {
                    tracing::warn!(%error, "media resource stop failed during owner cleanup")
                }
                Err(_) => tracing::warn!("media resource stop timed out during owner cleanup"),
            }
        }
    }

    /// Cancel this attached session's in-flight requests when the authenticated
    /// Bridge client disconnects.
    pub async fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.closing.cancel();
        self.owner_transition.read().await.cancel();
        let _owner_gate = self.owner_gate.write().await;
        self.cancel_active_operations().await;
        let owner = self.owner.read().await.clone();
        self.stop_owner_resources(&owner).await;
        self.unregister_owner_routes(&owner).await;
        if let (Some(app_host), Some(connector)) =
            (self.app_host.as_ref(), self.remote_connector.as_ref())
        {
            app_host.update_remote_session_modes(Vec::new());
            let remaining = connector.supported_modes(&owner).await;
            app_host.update_remote_session_modes(remaining);
        }
    }

    async fn execute(
        &self,
        request_id: String,
        owner: MediaOwner,
        owner_transition: CancellationToken,
        operation_dto: dto::MediaOperationDto,
        timeout_budget_ms: Option<u64>,
        max_payload_bytes: Option<u64>,
    ) -> dto::MediaServiceResponseDto {
        let capabilities = self.service.capabilities_for(&owner);
        let payload_limit = max_payload_bytes.unwrap_or(capabilities.max_payload_bytes);
        if payload_limit > capabilities.max_payload_bytes {
            return service_error(
                request_id,
                MediaError::new(
                    MediaErrorKind::MediaTooLarge,
                    "requested media payload limit exceeds the host capability limit",
                ),
            );
        }
        let operation = match operation_from_dto(operation_dto) {
            Ok(operation) => operation,
            Err(error) => return service_error(request_id, error),
        };
        let generation = match self.next_generation() {
            Some(generation) => generation,
            None => {
                return service_error(
                    request_id,
                    MediaError::new(
                        MediaErrorKind::NativeFailure,
                        "media operation generation is exhausted",
                    ),
                )
            }
        };
        let identity = MediaOperationId::new(generation, capabilities.service_epoch);
        let context = MediaOperationContext {
            identity: identity.clone(),
            owner,
            initiator: Some(MediaInitiator {
                request_id: Some(request_id.clone()),
                ..Default::default()
            }),
            timeout_budget_ms,
            max_payload_bytes: payload_limit,
        };
        self.active_operations.lock().await.insert(identity.clone());
        let guard = ActiveOperationGuard::new(
            self.service.clone(),
            self.active_operations.clone(),
            identity.clone(),
        );
        let execute = self.service.execute(context, operation);
        let timeout = timeout_budget_ms
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_MEDIA_REQUEST_TIMEOUT);
        let result = tokio::select! {
            biased;
            _ = owner_transition.cancelled() => Err(MediaError::new(MediaErrorKind::Cancelled, "media owner changed during the operation")),
            _ = self.closing.cancelled() => Err(MediaError::new(MediaErrorKind::Cancelled, "media host connection closed")),
            result = tokio::time::timeout(timeout, execute) => match result {
                Ok(result) => result,
                Err(_) => Err(MediaError::new(MediaErrorKind::Timeout, "media operation exceeded its host-selected deadline")),
            },
        };
        let must_cancel = result.as_ref().is_err_and(|error| {
            matches!(
                error.kind,
                MediaErrorKind::Timeout | MediaErrorKind::Cancelled
            )
        });
        guard.finish(must_cancel).await;
        match result {
            Ok(result) => match result_to_dto(result) {
                Ok(result) => dto::MediaServiceResponseDto {
                    request_id,
                    result: Some(dto::MediaServiceResultDto::Operation {
                        operation_id: operation_id_to_dto(identity),
                        result,
                    }),
                    error: None,
                },
                Err(error) => service_error(request_id, error),
            },
            Err(error) => service_error(request_id, error),
        }
    }

    async fn read_media(
        &self,
        request_id: String,
        owner: MediaOwner,
        owner_transition: CancellationToken,
        media_ref: String,
    ) -> dto::MediaServiceResponseDto {
        let capabilities = self.service.capabilities_for(&owner);
        let Some(generation) = self.next_generation() else {
            return service_error(
                request_id,
                MediaError::new(
                    MediaErrorKind::NativeFailure,
                    "media operation generation is exhausted",
                ),
            );
        };
        let context = MediaOperationContext {
            identity: MediaOperationId::new(generation, capabilities.service_epoch),
            owner,
            initiator: Some(MediaInitiator {
                request_id: Some(request_id.clone()),
                ..Default::default()
            }),
            timeout_budget_ms: None,
            max_payload_bytes: capabilities.max_payload_bytes,
        };
        let identity = context.identity.clone();
        self.active_operations.lock().await.insert(identity.clone());
        let guard = ActiveOperationGuard::new(
            self.service.clone(),
            self.active_operations.clone(),
            identity,
        );
        let result = tokio::select! {
            biased;
            _ = owner_transition.cancelled() => Err(MediaError::new(MediaErrorKind::Cancelled, "media owner changed during the read")),
            _ = self.closing.cancelled() => Err(MediaError::new(MediaErrorKind::Cancelled, "media host connection closed")),
            result = tokio::time::timeout(DEFAULT_MEDIA_REQUEST_TIMEOUT, self.service.read_media(context, MediaRef(media_ref))) => match result {
                Ok(result) => result,
                Err(_) => Err(MediaError::new(MediaErrorKind::Timeout, "media read exceeded its deadline")),
            },
        };
        let must_cancel = result.as_ref().is_err_and(|error| {
            matches!(
                error.kind,
                MediaErrorKind::Timeout | MediaErrorKind::Cancelled
            )
        });
        guard.finish(must_cancel).await;
        match result {
            Ok(content) => dto::MediaServiceResponseDto {
                request_id,
                result: Some(dto::MediaServiceResultDto::Content {
                    content: content_to_dto(content),
                }),
                error: None,
            },
            Err(error) => service_error(request_id, error),
        }
    }

    async fn cancel(
        &self,
        request_id: String,
        operation_id: dto::MediaOperationIdDto,
    ) -> dto::MediaServiceResponseDto {
        let identity = operation_id_from_dto(operation_id);
        if !self.active_operations.lock().await.contains(&identity) {
            return service_error(
                request_id,
                MediaError::new(
                    MediaErrorKind::NotFound,
                    "media operation is not active for the attached session",
                ),
            );
        }
        match self.service.cancel(identity).await {
            Ok(()) => dto::MediaServiceResponseDto {
                request_id,
                result: Some(dto::MediaServiceResultDto::Cancelled),
                error: None,
            },
            Err(error) => service_error(request_id, error),
        }
    }

    fn next_generation(&self) -> Option<u64> {
        self.generation
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .ok()
            .and_then(|previous| previous.checked_add(1))
    }
}

fn service_error(request_id: String, error: MediaError) -> dto::MediaServiceResponseDto {
    dto::MediaServiceResponseDto {
        request_id,
        result: None,
        error: Some(error_to_dto(error)),
    }
}

fn session_mode_from_dto(mode: dto::MediaSessionModeDto) -> MediaSessionMode {
    match mode {
        dto::MediaSessionModeDto::Audio => MediaSessionMode::Audio,
        dto::MediaSessionModeDto::Video => MediaSessionMode::Video,
        dto::MediaSessionModeDto::AudioVideo => MediaSessionMode::AudioVideo,
    }
}

fn operation_from_dto(value: dto::MediaOperationDto) -> Result<MediaOperation, MediaError> {
    use dto::MediaOperationDto as D;
    Ok(match value {
        D::Sources { kind, usage } => MediaOperation::Sources {
            kind: kind.map(kind_from_dto),
            usage: usage.map(usage_from_dto),
        },
        D::Status { target } => MediaOperation::Status {
            target: target.map(resource_from_dto).transpose()?,
        },
        D::Capture { source, options } => MediaOperation::Capture {
            source: source_from_dto(source),
            options: media_api::CaptureOptions {
                max_duration_ms: options.max_duration_ms,
                max_dimension_px: options.max_dimension_px,
                jpeg_quality: options.jpeg_quality,
            },
        },
        D::Record { source, options } => MediaOperation::Record {
            source: source_from_dto(source),
            options: RecordOptions {
                max_duration_ms: options.max_duration_ms,
                mime_type: options.mime_type,
            },
        },
        D::Transcribe { input, language } => MediaOperation::Transcribe {
            input: match input {
                dto::MediaTranscriptionInputDto::LiveSource { source } => {
                    TranscriptionInput::LiveSource(source_from_dto(source))
                }
                dto::MediaTranscriptionInputDto::StoredMedia { media_ref } => {
                    TranscriptionInput::StoredMedia(MediaRef(media_ref))
                }
            },
            language,
        },
        D::Synthesize {
            text,
            language,
            rate,
            voice,
        } => MediaOperation::Synthesize {
            text,
            language,
            rate,
            voice,
        },
        D::Play { media_ref, source } => MediaOperation::Play {
            media: MediaRef(media_ref),
            source: source.map(source_from_dto),
        },
        D::Speak {
            text,
            language,
            rate,
            voice,
        } => MediaOperation::Speak {
            text,
            language,
            rate,
            voice,
        },
        D::SessionStart { request } => MediaOperation::SessionStart {
            request: MediaSessionRequest {
                mode: match request.mode {
                    dto::MediaSessionModeDto::Audio => MediaSessionMode::Audio,
                    dto::MediaSessionModeDto::Video => MediaSessionMode::Video,
                    dto::MediaSessionModeDto::AudioVideo => MediaSessionMode::AudioVideo,
                },
                intent: match request.intent {
                    dto::MediaSessionIntentDto::Observation => MediaSessionIntent::Observation,
                    dto::MediaSessionIntentDto::Conversation => MediaSessionIntent::Conversation,
                },
                audio_source: request.audio_source.map(source_from_dto),
                video_source: request.video_source.map(source_from_dto),
            },
        },
        D::SessionUpdate { session_id, update } => MediaOperation::SessionUpdate {
            session: MediaSessionRef(session_id.into()),
            update: update_from_dto(update)?,
        },
        D::Observe {
            target,
            after_sequence,
            limit,
        } => MediaOperation::Observe {
            target: resource_from_dto(target)?,
            after: after_sequence.map(|sequence| MediaEventCursor { sequence }),
            limit,
        },
        D::Wait {
            target,
            after_sequence,
            timeout_ms,
            limit,
        } => MediaOperation::Wait {
            target: resource_from_dto(target)?,
            after: after_sequence.map(|sequence| MediaEventCursor { sequence }),
            timeout_ms,
            limit,
        },
        D::Stop { target } => MediaOperation::Stop {
            target: resource_from_dto(target)?,
        },
    })
}

fn result_to_dto(value: MediaResult) -> Result<dto::MediaResultDto, MediaError> {
    use dto::MediaResultDto as D;
    Ok(match value {
        MediaResult::Sources { sources } => D::Sources {
            sources: sources.into_iter().map(source_to_dto).collect(),
        },
        MediaResult::Status { resources } => D::Status {
            resources: resources.into_iter().map(resource_status_to_dto).collect(),
        },
        MediaResult::Captured { asset } => D::Captured {
            asset: asset_to_dto(asset),
        },
        MediaResult::RecordingStarted { recording } => D::RecordingStarted {
            recording_id: recording.0 .0,
        },
        MediaResult::RecordingStopped { recording, asset } => D::RecordingStopped {
            recording_id: recording.0 .0,
            asset: asset_to_dto(asset),
        },
        MediaResult::Transcript { transcript } => D::Transcript {
            text: transcript.text,
            language: transcript.language,
            confidence: transcript.confidence,
        },
        MediaResult::Synthesized { asset } => D::Synthesized {
            asset: asset_to_dto(asset),
        },
        MediaResult::PlaybackCompleted { duration_ms } => D::PlaybackCompleted { duration_ms },
        MediaResult::SpeechCompleted { duration_ms } => D::SpeechCompleted { duration_ms },
        MediaResult::SessionStarted { session, status } => D::SessionStarted {
            session_id: session.0 .0,
            status: session_status_to_dto(status),
        },
        MediaResult::SessionUpdated { session, status } => D::SessionUpdated {
            session_id: session.0 .0,
            status: session_status_to_dto(status),
        },
        MediaResult::Events { batch } => D::Events {
            batch: event_batch_to_dto(batch)?,
        },
        MediaResult::Stopped { resource } => D::Stopped {
            resource: resource_to_dto(resource),
        },
    })
}

fn event_batch_to_dto(value: MediaEventBatch) -> Result<dto::MediaEventBatchDto, MediaError> {
    value.validate()?;
    Ok(dto::MediaEventBatchDto {
        target: resource_to_dto(value.target),
        events: value.events.into_iter().map(event_to_dto).collect(),
        next_sequence: value.next_cursor.map(|cursor| cursor.sequence),
        dropped_before_cursor: value.dropped_before_cursor,
    })
}

fn event_to_dto(value: MediaEvent) -> dto::MediaEventDto {
    dto::MediaEventDto {
        sequence: value.sequence,
        occurred_at_unix_ms: value.occurred_at_unix_ms,
        target: resource_to_dto(value.target),
        operation_id: value.operation_id.map(operation_id_to_dto),
        kind: match value.kind {
            MediaEventKind::StateChanged { resource, state } => {
                dto::MediaEventKindDto::StateChanged {
                    resource: resource_to_dto(resource),
                    state: resource_state_to_dto(state),
                }
            }
            MediaEventKind::Frame { source, asset } => dto::MediaEventKindDto::Frame {
                source: source_to_ref_dto(source),
                asset: asset_to_dto(asset),
            },
            MediaEventKind::Transcript { transcript } => dto::MediaEventKindDto::Transcript {
                text: transcript.text,
                language: transcript.language,
                confidence: transcript.confidence,
            },
            MediaEventKind::Error { kind, message } => dto::MediaEventKindDto::Error {
                kind: error_kind_to_dto(kind),
                message,
            },
        },
    }
}

fn content_to_dto(value: MediaContent) -> dto::MediaContentDto {
    dto::MediaContentDto {
        media_ref: value.reference.0,
        metadata: metadata_to_dto(value.metadata),
        bytes: value.bytes,
    }
}

fn asset_to_dto(value: MediaAsset) -> dto::MediaAssetDto {
    dto::MediaAssetDto {
        media_ref: value.reference.0,
        metadata: metadata_to_dto(value.metadata),
    }
}

fn metadata_to_dto(value: MediaMetadata) -> dto::MediaMetadataDto {
    dto::MediaMetadataDto {
        kind: kind_to_dto(value.kind),
        mime_type: value.mime_type,
        byte_len: value.byte_len,
        captured_at_unix_ms: value.captured_at_unix_ms,
        duration_ms: value.duration_ms,
        width_px: value.width_px,
        height_px: value.height_px,
        sample_rate_hz: value.sample_rate_hz,
        channels: value.channels,
    }
}

fn source_to_dto(value: MediaSource) -> dto::MediaSourceDto {
    dto::MediaSourceDto {
        reference: source_to_ref_dto(value.reference),
        kind: source_kind_to_dto(value.kind),
        media_kind: value.media_kind.map(kind_to_dto),
        label: value.label,
        usages: value.usages.into_iter().map(usage_to_dto).collect(),
        supported_operations: value
            .supported_operations
            .into_iter()
            .map(operation_kind_to_dto)
            .collect(),
        is_default: value.is_default,
    }
}

fn resource_status_to_dto(value: MediaResourceStatus) -> dto::MediaResourceStatusDto {
    dto::MediaResourceStatusDto {
        resource: resource_to_dto(value.resource),
        state: resource_state_to_dto(value.state),
        started_at_unix_ms: value.started_at_unix_ms,
        updated_at_unix_ms: value.updated_at_unix_ms,
    }
}

fn session_status_to_dto(value: MediaSessionStatus) -> dto::MediaSessionStatusDto {
    dto::MediaSessionStatusDto {
        state: match value.state {
            MediaSessionState::Preparing => dto::MediaSessionStateDto::Preparing,
            MediaSessionState::Ready => dto::MediaSessionStateDto::Ready,
            MediaSessionState::Reconnecting => dto::MediaSessionStateDto::Reconnecting,
            MediaSessionState::Ended => dto::MediaSessionStateDto::Ended,
            MediaSessionState::Failed => dto::MediaSessionStateDto::Failed,
        },
        audio_enabled: value.audio_enabled,
        video_enabled: value.video_enabled,
    }
}

fn operation_id_to_dto(value: MediaOperationId) -> dto::MediaOperationIdDto {
    dto::MediaOperationIdDto {
        id: value.id,
        generation: value.generation,
        service_epoch: value.service_epoch,
    }
}

fn operation_id_from_dto(value: dto::MediaOperationIdDto) -> MediaOperationId {
    MediaOperationId {
        id: value.id,
        generation: value.generation,
        service_epoch: value.service_epoch,
    }
}

fn resource_from_dto(value: dto::MediaResourceRefDto) -> Result<MediaResourceRef, MediaError> {
    Ok(match value {
        dto::MediaResourceRefDto::Source { source } => {
            MediaResourceRef::Source(source_from_dto(source))
        }
        dto::MediaResourceRefDto::Recording { recording_id } => {
            MediaResourceRef::Recording(RecordingRef(recording_id.into()))
        }
        dto::MediaResourceRefDto::Session { session_id } => {
            MediaResourceRef::Session(MediaSessionRef(session_id.into()))
        }
        dto::MediaResourceRefDto::Media { media_ref } => {
            MediaResourceRef::Media(MediaRef(media_ref))
        }
        dto::MediaResourceRefDto::Operation { operation_id } => {
            MediaResourceRef::Operation(operation_id_from_dto(operation_id))
        }
    })
}

fn resource_to_dto(value: MediaResourceRef) -> dto::MediaResourceRefDto {
    match value {
        MediaResourceRef::Source(source) => dto::MediaResourceRefDto::Source {
            source: source_to_ref_dto(source),
        },
        MediaResourceRef::Recording(recording) => dto::MediaResourceRefDto::Recording {
            recording_id: recording.0 .0,
        },
        MediaResourceRef::Session(session) => dto::MediaResourceRefDto::Session {
            session_id: session.0 .0,
        },
        MediaResourceRef::Media(media) => dto::MediaResourceRefDto::Media { media_ref: media.0 },
        MediaResourceRef::Operation(operation) => dto::MediaResourceRefDto::Operation {
            operation_id: operation_id_to_dto(operation),
        },
    }
}

fn update_from_dto(value: dto::MediaSessionUpdateDto) -> Result<MediaSessionUpdate, MediaError> {
    Ok(match value {
        dto::MediaSessionUpdateDto::SetEnabled { kind, enabled } => {
            MediaSessionUpdate::SetEnabled {
                kind: kind_from_dto(kind),
                enabled,
            }
        }
        dto::MediaSessionUpdateDto::SelectSource { kind, source } => {
            MediaSessionUpdate::SelectSource {
                kind: kind_from_dto(kind),
                source: source_from_dto(source),
            }
        }
        dto::MediaSessionUpdateDto::SetPlaybackGain { gain } => {
            MediaSessionUpdate::SetPlaybackGain { gain }
        }
    })
}

fn source_from_dto(value: dto::MediaSourceRefDto) -> MediaSourceRef {
    MediaSourceRef(value.id.into())
}

fn source_to_ref_dto(value: MediaSourceRef) -> dto::MediaSourceRefDto {
    dto::MediaSourceRefDto { id: value.0 .0 }
}

fn kind_from_dto(value: dto::MediaKindDto) -> MediaKind {
    match value {
        dto::MediaKindDto::Audio => MediaKind::Audio,
        dto::MediaKindDto::Image => MediaKind::Image,
        dto::MediaKindDto::Video => MediaKind::Video,
    }
}

fn kind_to_dto(value: MediaKind) -> dto::MediaKindDto {
    match value {
        MediaKind::Audio => dto::MediaKindDto::Audio,
        MediaKind::Image => dto::MediaKindDto::Image,
        MediaKind::Video => dto::MediaKindDto::Video,
    }
}

fn usage_from_dto(value: dto::MediaSourceUsageDto) -> MediaSourceUsage {
    match value {
        dto::MediaSourceUsageDto::Capture => MediaSourceUsage::Capture,
        dto::MediaSourceUsageDto::Record => MediaSourceUsage::Record,
        dto::MediaSourceUsageDto::Transcribe => MediaSourceUsage::Transcribe,
        dto::MediaSourceUsageDto::Play => MediaSourceUsage::Play,
        dto::MediaSourceUsageDto::Speak => MediaSourceUsage::Speak,
        dto::MediaSourceUsageDto::Session => MediaSourceUsage::Session,
        dto::MediaSourceUsageDto::ReadImage => MediaSourceUsage::ReadImage,
    }
}

fn usage_to_dto(value: MediaSourceUsage) -> dto::MediaSourceUsageDto {
    match value {
        MediaSourceUsage::Capture => dto::MediaSourceUsageDto::Capture,
        MediaSourceUsage::Record => dto::MediaSourceUsageDto::Record,
        MediaSourceUsage::Transcribe => dto::MediaSourceUsageDto::Transcribe,
        MediaSourceUsage::Play => dto::MediaSourceUsageDto::Play,
        MediaSourceUsage::Speak => dto::MediaSourceUsageDto::Speak,
        MediaSourceUsage::Session => dto::MediaSourceUsageDto::Session,
        MediaSourceUsage::ReadImage => dto::MediaSourceUsageDto::ReadImage,
    }
}

fn source_kind_to_dto(value: MediaSourceKind) -> dto::MediaSourceKindDto {
    match value {
        MediaSourceKind::Microphone => dto::MediaSourceKindDto::Microphone,
        MediaSourceKind::Camera => dto::MediaSourceKindDto::Camera,
        MediaSourceKind::Screen => dto::MediaSourceKindDto::Screen,
        MediaSourceKind::Speaker => dto::MediaSourceKindDto::Speaker,
        MediaSourceKind::Library => dto::MediaSourceKindDto::Library,
        MediaSourceKind::Remote => dto::MediaSourceKindDto::Remote,
    }
}

fn operation_kind_to_dto(value: media_api::MediaOperationKind) -> dto::MediaOperationKindDto {
    use dto::MediaOperationKindDto as D;
    match value {
        media_api::MediaOperationKind::Sources => D::Sources,
        media_api::MediaOperationKind::Status => D::Status,
        media_api::MediaOperationKind::Capture => D::Capture,
        media_api::MediaOperationKind::Record => D::Record,
        media_api::MediaOperationKind::Transcribe => D::Transcribe,
        media_api::MediaOperationKind::Synthesize => D::Synthesize,
        media_api::MediaOperationKind::Play => D::Play,
        media_api::MediaOperationKind::Speak => D::Speak,
        media_api::MediaOperationKind::SessionStart => D::SessionStart,
        media_api::MediaOperationKind::SessionUpdate => D::SessionUpdate,
        media_api::MediaOperationKind::Observe => D::Observe,
        media_api::MediaOperationKind::Wait => D::Wait,
        media_api::MediaOperationKind::Stop => D::Stop,
    }
}

fn resource_state_to_dto(value: MediaResourceState) -> dto::MediaResourceStateDto {
    match value {
        MediaResourceState::Ready => dto::MediaResourceStateDto::Ready,
        MediaResourceState::Active => dto::MediaResourceStateDto::Active,
        MediaResourceState::Stopping => dto::MediaResourceStateDto::Stopping,
        MediaResourceState::Completed => dto::MediaResourceStateDto::Completed,
        MediaResourceState::Failed => dto::MediaResourceStateDto::Failed,
    }
}

fn error_to_dto(value: MediaError) -> dto::MediaErrorDto {
    dto::MediaErrorDto {
        kind: error_kind_to_dto(value.kind),
        message: value.message,
    }
}

fn error_kind_to_dto(value: MediaErrorKind) -> dto::MediaErrorKindDto {
    use dto::MediaErrorKindDto as D;
    match value {
        MediaErrorKind::PermissionDenied => D::PermissionDenied,
        MediaErrorKind::Busy => D::Busy,
        MediaErrorKind::Cancelled => D::Cancelled,
        MediaErrorKind::Timeout => D::Timeout,
        MediaErrorKind::NoData => D::NoData,
        MediaErrorKind::NotFound => D::NotFound,
        MediaErrorKind::Unavailable => D::Unavailable,
        MediaErrorKind::Unsupported => D::Unsupported,
        MediaErrorKind::ModelMissing => D::ModelMissing,
        MediaErrorKind::InvalidRequest => D::InvalidRequest,
        MediaErrorKind::ProcessingFailed => D::ProcessingFailed,
        MediaErrorKind::NativeFailure => D::NativeFailure,
        MediaErrorKind::StoreFailure => D::StoreFailure,
        MediaErrorKind::MediaTooLarge => D::MediaTooLarge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use media_api::MediaCapabilities;
    use std::sync::Mutex as StdMutex;

    struct OwnerTrackingService {
        calls: StdMutex<Vec<(MediaOwner, MediaOperation)>>,
        old_owner: MediaOwner,
    }

    #[async_trait]
    impl MediaService for OwnerTrackingService {
        fn capabilities(&self) -> MediaCapabilities {
            MediaCapabilities {
                service_epoch: 7,
                support_revision: 1,
                supported_operations: vec![
                    media_api::MediaOperationKind::Status,
                    media_api::MediaOperationKind::Stop,
                ],
                supported_media_kinds: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }

        async fn execute(
            &self,
            context: MediaOperationContext,
            operation: MediaOperation,
        ) -> Result<MediaResult, MediaError> {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push((context.owner.clone(), operation.clone()));
            match operation {
                MediaOperation::Status { .. } => {
                    let resources = if context.owner == self.old_owner {
                        vec![MediaResourceStatus {
                            resource: MediaResourceRef::Session(MediaSessionRef(
                                media_api::MediaSessionId("media_old".into()),
                            )),
                            state: MediaResourceState::Active,
                            started_at_unix_ms: Some(1),
                            updated_at_unix_ms: Some(2),
                        }]
                    } else {
                        Vec::new()
                    };
                    Ok(MediaResult::Status { resources })
                }
                MediaOperation::Stop { target } => Ok(MediaResult::Stopped { resource: target }),
                _ => Err(MediaError::new(
                    MediaErrorKind::Unsupported,
                    "test service does not implement this operation",
                )),
            }
        }

        async fn read_media(
            &self,
            _context: MediaOperationContext,
            _media: MediaRef,
        ) -> Result<MediaContent, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::NotFound,
                "the test service has no stored media",
            ))
        }

        async fn cancel(&self, _identity: MediaOperationId) -> Result<(), MediaError> {
            Ok(())
        }
    }

    fn session_owner(session_id: &str) -> MediaOwner {
        MediaOwner::Session {
            session_id: session_id.into(),
        }
    }

    #[tokio::test]
    async fn owner_transition_stops_old_resources_before_accepting_new_owner_requests() {
        let old_owner = session_owner("11111111-1111-4111-8111-111111111111");
        let new_owner = session_owner("22222222-2222-4222-8222-222222222222");
        let service = Arc::new(OwnerTrackingService {
            calls: StdMutex::new(Vec::new()),
            old_owner: old_owner.clone(),
        });
        let rpc = MediaServiceRpc::new(service.clone(), old_owner.clone());

        rpc.update_owner(new_owner.clone()).await;
        let response = rpc
            .handle(dto::MediaServiceRequestDto {
                request_id: "new-owner-status".into(),
                action: dto::MediaServiceActionDto::Execute {
                    operation: dto::MediaOperationDto::Status { target: None },
                    timeout_budget_ms: Some(1_000),
                    max_payload_bytes: None,
                },
            })
            .await;

        assert!(matches!(
            response.result,
            Some(dto::MediaServiceResultDto::Operation {
                result: dto::MediaResultDto::Status { ref resources },
                ..
            }) if resources.is_empty()
        ));
        let calls = service
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(calls.iter().any(|(owner, operation)| {
            owner == &old_owner
                && matches!(
                    operation,
                    MediaOperation::Stop {
                        target: MediaResourceRef::Session(_)
                    }
                )
        }));
        assert!(calls.iter().any(|(owner, operation)| {
            owner == &new_owner && matches!(operation, MediaOperation::Status { .. })
        }));
    }
}
