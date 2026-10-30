//! App-scoped media facade shared by the Agent runtime, native UI, and
//! Remote Control adapter.

use async_trait::async_trait;
use media_api::audio::{
    AudioCapabilitySnapshot, AudioError, AudioErrorKind, AudioOperation, AudioOperationContext,
    AudioOperationSuccess, AudioService,
};
use media_api::{
    MediaAttachmentId, MediaAudioChannel, MediaCapabilities, MediaConnector, MediaConnectorId,
    MediaContent, MediaDevicePorts, MediaError, MediaErrorKind, MediaOperation,
    MediaOperationContext, MediaOperationId, MediaOperationIncarnation, MediaOwner, MediaRef,
    MediaResourceStore, MediaResult, MediaService, MediaSession, MediaSessionMode, MediaSessionRef,
    MediaSessionRequest, MediaSource, MediaSourceRef,
};
use media_runtime::{
    AudioRoutingSettings, MediaAttachmentHandle, MediaAttachmentHost, MediaAttachmentPhase,
    MediaOperationCleanup, OfflineAudioBackend, UnifiedMediaService,
};
use std::collections::{HashMap, HashSet};
use std::future::{poll_fn, Future};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock, RwLock, Weak,
};
use std::task::Poll;
use tokio::time::Duration;

const LOCAL_MICROPHONE_SOURCE_ID: &str = "media-runtime:microphone";
const LOCAL_CAMERA_SOURCE_ID: &str = "media-runtime:camera";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OperationRouteKey {
    identity: MediaOperationId,
    incarnation: MediaOperationIncarnation,
}

struct MediaOperationRoute {
    attachment: MediaAttachmentHandle,
    logical_owner: MediaOwner,
    context: MediaOperationContext,
    route_incarnation: Arc<()>,
    call_finished: bool,
}

struct MediaOperationRouteGuard<'a> {
    host: &'a AppMediaHost,
    attachment: MediaAttachmentHandle,
    context: MediaOperationContext,
    key: OperationRouteKey,
    incarnation: Arc<()>,
    succeeded: bool,
}

impl Drop for MediaOperationRouteGuard<'_> {
    fn drop(&mut self) {
        let confirmed = self.succeeded
            || !self.context.attachment_admitted()
            || self.attachment.operation_cleanup(&self.context) == MediaOperationCleanup::Confirmed;
        let mut routes = self
            .host
            .media_operation_routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(route) = routes.get_mut(&self.key) else {
            return;
        };
        if !Arc::ptr_eq(&route.route_incarnation, &self.incarnation) {
            return;
        }
        if confirmed {
            routes.remove(&self.key);
        } else {
            route.call_finished = true;
        }
    }
}

struct AudioAttachmentState {
    handle: MediaAttachmentHandle,
    shared_service: Arc<dyn AudioService>,
    service: Arc<dyn AudioService>,
}

#[derive(Clone)]
struct AudioOperationRoute {
    attachment: Arc<AudioAttachmentState>,
    context: AudioOperationContext,
    route_incarnation: Arc<()>,
    flight: Arc<AudioOperationFlight>,
    recording: Option<media_api::RecordingRef>,
    is_end_owner: bool,
}

#[derive(Default)]
struct AudioOperationFlight {
    reserved: AtomicBool,
    dispatched: AtomicBool,
    finished: AtomicBool,
    changed: tokio::sync::Notify,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct AudioOwnerRouteKey {
    attachment: MediaAttachmentId,
    owner: MediaOwner,
}

struct AudioOwnerEndFence {
    attachment: Arc<AudioAttachmentState>,
    flight: Arc<AudioOperationFlight>,
}

impl AudioOperationFlight {
    fn finish(&self) {
        self.finished.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }

    async fn wait_finished(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.finished.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Default)]
struct ProcessAudioAttachments {
    attachments: HashMap<MediaAttachmentId, Arc<AudioAttachmentState>>,
    operations: HashMap<OperationRouteKey, AudioOperationRoute>,
    ending_owners: HashMap<AudioOwnerRouteKey, AudioOwnerEndFence>,
}

fn process_audio_attachments() -> &'static Mutex<ProcessAudioAttachments> {
    static ATTACHMENTS: OnceLock<Mutex<ProcessAudioAttachments>> = OnceLock::new();
    ATTACHMENTS.get_or_init(|| Mutex::new(ProcessAudioAttachments::default()))
}

fn forget_audio_operation(key: &OperationRouteKey, incarnation: &Arc<()>) {
    let mut registry = process_audio_attachments()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if registry
        .operations
        .get(key)
        .is_some_and(|route| Arc::ptr_eq(&route.route_incarnation, incarnation))
    {
        registry.operations.remove(key);
    }
}

// The scoped facade may already be cancelling this exact operation. Keep the
// route until that flight has a receipt, including if this observer is dropped.
async fn cancel_audio_operation_route(
    key: OperationRouteKey,
    route: AudioOperationRoute,
) -> Result<(), AudioError> {
    let result = loop {
        let finished_before_cancel = route.flight.finished.load(Ordering::Acquire);
        match route.attachment.service.cancel(route.context.clone()).await {
            Ok(()) if !finished_before_cancel => {
                // A start can finish after an earlier cancellation receipt.
                // Reconfirm the exact context after the execute future has
                // settled (or been dropped) before declaring it closed.
                route.flight.wait_finished().await;
            }
            result => break result,
        }
    };
    if result.is_ok() {
        forget_audio_operation(&key, &route.route_incarnation);
    }
    result
}

fn spawn_audio_route_cancel(
    key: OperationRouteKey,
    route: AudioOperationRoute,
) -> Option<tokio::task::JoinHandle<Result<(), AudioError>>> {
    tokio::runtime::Handle::try_current()
        .ok()
        .map(|runtime| runtime.spawn(cancel_audio_operation_route(key, route)))
}

struct AudioOwnerEndCleanupGuard {
    key: AudioOwnerRouteKey,
    flight: Arc<AudioOperationFlight>,
    confirmed: bool,
}

impl Drop for AudioOwnerEndCleanupGuard {
    fn drop(&mut self) {
        if self.confirmed {
            let mut registry = process_audio_attachments()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if registry
                .ending_owners
                .get(&self.key)
                .is_some_and(|fence| Arc::ptr_eq(&fence.flight, &self.flight))
            {
                registry.ending_owners.remove(&self.key);
            }
        }
        self.flight.finish();
    }
}

fn spawn_audio_owner_cleanup(
    key: AudioOwnerRouteKey,
    flight: Arc<AudioOperationFlight>,
    origins: Vec<(OperationRouteKey, AudioOperationRoute)>,
) -> Option<tokio::task::JoinHandle<Result<(), AudioError>>> {
    let runtime = tokio::runtime::Handle::try_current().ok()?;
    Some(runtime.spawn(async move {
        let mut guard = AudioOwnerEndCleanupGuard {
            key,
            flight,
            confirmed: false,
        };
        let tasks = origins
            .into_iter()
            .map(|(key, route)| tokio::spawn(cancel_audio_operation_route(key, route)))
            .collect::<Vec<_>>();
        let mut confirmed = true;
        for task in tasks {
            confirmed &= matches!(task.await, Ok(Ok(())));
        }
        guard.confirmed = confirmed;
        if confirmed {
            Ok(())
        } else {
            Err(AudioError::new(
                AudioErrorKind::Unavailable,
                "audio owner cleanup has unconfirmed operation routes",
            ))
        }
    }))
}

struct AudioOperationRouteGuard {
    key: OperationRouteKey,
    route: AudioOperationRoute,
    armed: bool,
}

impl AudioOperationRouteGuard {
    fn schedule_cleanup(&mut self) -> Option<tokio::task::JoinHandle<Result<(), AudioError>>> {
        self.armed = false;
        spawn_audio_route_cancel(self.key.clone(), self.route.clone())
    }
}

impl Drop for AudioOperationRouteGuard {
    fn drop(&mut self) {
        if self.armed {
            // Any downstream future has already been dropped before this
            // guard, so a never-polled call cannot be dispatched afterward.
            self.route.flight.dispatched.store(true, Ordering::Release);
            self.route.flight.finish();
            let _ = spawn_audio_route_cancel(self.key.clone(), self.route.clone());
        }
    }
}

fn register_audio_attachment(
    handle: MediaAttachmentHandle,
    shared: Arc<dyn AudioService>,
) -> Result<(), MediaError> {
    if handle.phase() != MediaAttachmentPhase::Active {
        return Err(MediaError::new(
            MediaErrorKind::Unavailable,
            "cannot bind audio to a non-active media attachment",
        ));
    }
    let service = handle
        .audio_service(shared.clone())
        .map_err(media_error_from_audio)?;
    let mut registry = process_audio_attachments()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    registry
        .operations
        .retain(|_, route| route.attachment.handle.phase() != MediaAttachmentPhase::Revoked);
    registry
        .ending_owners
        .retain(|_, fence| fence.attachment.handle.phase() != MediaAttachmentPhase::Revoked);
    let operation_attachments = registry
        .operations
        .values()
        .map(|route| route.attachment.handle.id().clone())
        .collect::<HashSet<_>>();
    registry.attachments.retain(|id, state| {
        state.handle.phase() != MediaAttachmentPhase::Revoked || operation_attachments.contains(id)
    });
    if registry
        .attachments
        .values()
        .any(|state| state.handle.phase() != MediaAttachmentPhase::Revoked)
    {
        return Err(MediaError::new(
            MediaErrorKind::Unavailable,
            "a previous media/audio attachment remains Active or Unknown",
        ));
    }
    if registry.attachments.contains_key(handle.id()) {
        return Err(MediaError::new(
            MediaErrorKind::InvalidRequest,
            "media attachment already has an audio facade",
        ));
    }
    if registry.attachments.len() >= 256 {
        return Err(MediaError::new(
            MediaErrorKind::Unavailable,
            "unconfirmed audio attachment history is full",
        ));
    }
    registry.attachments.insert(
        handle.id().clone(),
        Arc::new(AudioAttachmentState {
            handle,
            shared_service: shared,
            service,
        }),
    );
    Ok(())
}

fn audio_attachment_state(id: &MediaAttachmentId) -> Option<Arc<AudioAttachmentState>> {
    process_audio_attachments()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .attachments
        .get(id)
        .cloned()
}

fn media_error_from_audio(error: AudioError) -> MediaError {
    MediaError::new(MediaErrorKind::Unavailable, error.to_string())
}

struct AppAudioService {
    host: Weak<AppMediaHost>,
}

impl AppAudioService {
    fn unavailable() -> AudioError {
        AudioError::new(
            AudioErrorKind::Unavailable,
            "the app media owner has no active audio attachment",
        )
    }

    fn unavailable_capabilities() -> AudioCapabilitySnapshot {
        AudioCapabilitySnapshot {
            service_epoch: 0,
            support_revision: 0,
            supported_operations: Vec::new(),
            readiness: Vec::new(),
            max_payload_bytes: 0,
        }
    }

    fn current_attachment(&self) -> Result<Arc<AudioAttachmentState>, AudioError> {
        let host = self.host.upgrade().ok_or_else(Self::unavailable)?;
        if host.closed.load(Ordering::Acquire) {
            return Err(Self::unavailable());
        }
        let handle = host
            .current_media_attachment()
            .ok_or_else(Self::unavailable)?;
        if handle.phase() != MediaAttachmentPhase::Active {
            return Err(Self::unavailable());
        }
        let state = audio_attachment_state(handle.id()).ok_or_else(Self::unavailable)?;
        if state.handle.id() != handle.id()
            || !Arc::ptr_eq(&state.shared_service, &host.audio_service)
        {
            return Err(Self::unavailable());
        }
        Ok(state)
    }

    async fn execute_bound(
        &self,
        context: AudioOperationContext,
        operation: AudioOperation,
        local_only: bool,
    ) -> Result<AudioOperationSuccess, AudioError> {
        let state = self.current_attachment()?;
        if context.owner.attachment_id().is_some() {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "Desktop audio callers cannot select a media attachment",
            ));
        }
        let key = OperationRouteKey {
            identity: context.identity.clone(),
            incarnation: context.operation_incarnation(),
        };
        let owner_key = AudioOwnerRouteKey {
            attachment: state.handle.id().clone(),
            owner: context.owner.clone(),
        };
        let is_end_owner = matches!(&operation, AudioOperation::EndOwner);
        let route_incarnation = Arc::new(());
        let route = AudioOperationRoute {
            attachment: state.clone(),
            context: context.clone(),
            route_incarnation: route_incarnation.clone(),
            flight: Arc::new(AudioOperationFlight::default()),
            recording: None,
            is_end_owner,
        };
        let (stopped_origin, ended_origins) = {
            let mut registry = process_audio_attachments()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.operations.retain(|_, route| {
                route.attachment.handle.phase() != MediaAttachmentPhase::Revoked
            });
            registry.ending_owners.retain(|_, fence| {
                fence.attachment.handle.phase() != MediaAttachmentPhase::Revoked
            });
            if let Some(fence) = registry.ending_owners.get(&owner_key) {
                if !is_end_owner || !fence.flight.finished.load(Ordering::Acquire) {
                    return Err(AudioError::new(
                        AudioErrorKind::Busy,
                        "audio owner teardown is unresolved",
                    ));
                }
            }
            if is_end_owner
                && registry.operations.values().any(|previous| {
                    previous.is_end_owner
                        && previous.attachment.handle.id() == state.handle.id()
                        && previous.context.owner == context.owner
                })
            {
                // A previous EndOwner has no exact terminal receipt. Retrying
                // must use that context; do not allocate another route for an
                // operation the scoped owner fence will reject anyway.
                return Err(AudioError::new(
                    AudioErrorKind::Unavailable,
                    "the previous audio owner teardown remains unconfirmed",
                ));
            }
            if registry.operations.contains_key(&key) {
                return Err(AudioError::new(
                    AudioErrorKind::InvalidRequest,
                    "audio operation incarnation was already admitted",
                ));
            }
            if registry.operations.len() >= 512 {
                return Err(AudioError::new(
                    AudioErrorKind::Busy,
                    "unresolved audio operation routes are full",
                ));
            }
            if is_end_owner
                && !registry.ending_owners.contains_key(&owner_key)
                && registry.ending_owners.len() >= 512
            {
                return Err(AudioError::new(
                    AudioErrorKind::Busy,
                    "unresolved audio owner teardowns are full",
                ));
            }
            // Capture only a unique pre-existing origin. A later start can
            // reuse the same handle while this stop is still in flight.
            let stopped_origin = if let AudioOperation::StopRecording { handle } = &operation {
                let mut origins = registry.operations.iter().filter(|(_, route)| {
                    route.attachment.handle.id() == state.handle.id()
                        && route.context.owner == context.owner
                        && route.recording.as_ref() == Some(handle)
                });
                let first = origins
                    .next()
                    .map(|(key, route)| (key.clone(), route.route_incarnation.clone()));
                if origins.next().is_none() {
                    first
                } else {
                    None
                }
            } else {
                None
            };
            // Hold admission under the same lock used to reserve dispatch.
            // Snapshot all earlier reservations, including pending starts.
            let ended_origins = if is_end_owner {
                registry.ending_owners.insert(
                    owner_key.clone(),
                    AudioOwnerEndFence {
                        attachment: state.clone(),
                        flight: route.flight.clone(),
                    },
                );
                registry
                    .operations
                    .iter()
                    .filter(|(_, route)| {
                        route.attachment.handle.id() == state.handle.id()
                            && route.context.owner == context.owner
                            && !route.is_end_owner
                            && route.flight.reserved.load(Ordering::Acquire)
                    })
                    .map(|(key, route)| (key.clone(), route.clone()))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            registry.operations.insert(key.clone(), route.clone());
            (stopped_origin, ended_origins)
        };
        let mut guard = AudioOperationRouteGuard {
            key: key.clone(),
            route: route.clone(),
            armed: true,
        };
        // Keep the caller's logical context here. The Harness facade created
        // from this trusted handle performs admission and injects the same
        // attachment scope used by MediaService; this wrapper only retains the
        // exact facade/context for later cancellation.
        let started = std::time::Instant::now();
        let budget = Duration::from_millis(context.timeout_budget_ms.unwrap_or(30_000));
        let mut downstream = Box::pin(async move {
            if local_only {
                state
                    .service
                    .execute_host_configured_local(context, operation)
                    .await
            } else {
                state.service.execute(context, operation).await
            }
        });
        let mut reserved = false;
        let mut rejected_before_dispatch = false;
        let result = poll_fn(|cx| {
            if !reserved {
                let registry = process_audio_attachments()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !is_end_owner && registry.ending_owners.contains_key(&owner_key) {
                    rejected_before_dispatch = true;
                    return Poll::Ready(Err(AudioError::new(
                        AudioErrorKind::Busy,
                        "audio owner teardown is unresolved",
                    )));
                }
                route.flight.reserved.store(true, Ordering::Release);
                reserved = true;
            }
            let result = downstream.as_mut().poll(cx);
            // The scoped service performs its admission before its first
            // suspension. A concurrent cancellation before this point must
            // not mistake an absent downstream lease for a close receipt.
            route.flight.dispatched.store(true, Ordering::Release);
            result
        })
        .await;
        drop(downstream);
        if rejected_before_dispatch {
            // The scoped future was never polled, so no native work exists.
            guard.armed = false;
            route.flight.finish();
            forget_audio_operation(&key, &route_incarnation);
            return result;
        }
        if let Ok(AudioOperationSuccess::RecordingStarted { handle }) = &result {
            let mut registry = process_audio_attachments()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(current) = registry.operations.get_mut(&key) {
                if Arc::ptr_eq(&current.route_incarnation, &route_incarnation) {
                    current.recording = Some(handle.clone());
                }
            }
            guard.armed = false;
            route.flight.finish();
        } else if result.is_ok() {
            guard.armed = false;
            forget_audio_operation(&key, &route_incarnation);
            // ScopedAudio only returns a completed stop after confirming the
            // matching start's exact cancellation. The snapshot prevents a
            // late stop from retiring a newer route with a reused handle.
            if matches!(&result, Ok(AudioOperationSuccess::Recording { .. })) {
                if let Some((origin_key, origin_incarnation)) = stopped_origin {
                    forget_audio_operation(&origin_key, &origin_incarnation);
                }
            }
            if is_end_owner && matches!(&result, Ok(AudioOperationSuccess::OwnerEnded)) {
                // EndOwner is not an origin receipt. Keep its admission fence
                // until every captured route confirms its own exact cancel.
                if let Some(cleanup) =
                    spawn_audio_owner_cleanup(owner_key, route.flight.clone(), ended_origins)
                {
                    let remaining = budget.saturating_sub(started.elapsed());
                    if remaining.is_zero() {
                        return Err(AudioError::new(
                            AudioErrorKind::Timeout,
                            "audio owner route cleanup exceeded its budget",
                        ));
                    }
                    match tokio::time::timeout(remaining, cleanup).await {
                        Ok(Ok(Ok(()))) => {}
                        Ok(Ok(Err(error))) => return Err(error),
                        Ok(Err(_)) => {
                            return Err(AudioError::new(
                                AudioErrorKind::NativeFailure,
                                "audio owner route cleanup ended without a receipt",
                            ));
                        }
                        Err(_) => {
                            return Err(AudioError::new(
                                AudioErrorKind::Timeout,
                                "audio owner route cleanup exceeded its budget",
                            ));
                        }
                    }
                } else {
                    // Without a runtime the fence remains closed and can be
                    // retried; no completion receipt has been observed.
                    route.flight.finish();
                    return Err(AudioError::new(
                        AudioErrorKind::Unavailable,
                        "audio owner route cleanup has no runtime",
                    ));
                }
            } else {
                route.flight.finish();
            }
        } else {
            route.flight.finish();
            if let Some(cleanup) = guard.schedule_cleanup() {
                // A dropped observer must not cancel this flight. Wait within
                // the caller's remaining budget so prompt receipts free capacity.
                let remaining = budget.saturating_sub(started.elapsed());
                if !remaining.is_zero() {
                    let _ = tokio::time::timeout(remaining, cleanup).await;
                }
            }
        }
        result
    }
}

#[async_trait]
impl AudioService for AppAudioService {
    fn capabilities(&self) -> AudioCapabilitySnapshot {
        self.current_attachment()
            .map(|state| state.service.capabilities())
            .unwrap_or_else(|_| Self::unavailable_capabilities())
    }

    fn capabilities_for(&self, owner: &MediaOwner) -> AudioCapabilitySnapshot {
        self.current_attachment()
            .map(|state| state.service.capabilities_for(owner))
            .unwrap_or_else(|_| Self::unavailable_capabilities())
    }

    async fn execute(
        &self,
        context: AudioOperationContext,
        operation: AudioOperation,
    ) -> Result<AudioOperationSuccess, AudioError> {
        self.execute_bound(context, operation, false).await
    }

    async fn execute_host_configured_local(
        &self,
        context: AudioOperationContext,
        operation: AudioOperation,
    ) -> Result<AudioOperationSuccess, AudioError> {
        self.execute_bound(context, operation, true).await
    }

    async fn cancel(&self, context: AudioOperationContext) -> Result<(), AudioError> {
        let key = OperationRouteKey {
            identity: context.identity.clone(),
            incarnation: context.operation_incarnation(),
        };
        if context.owner.attachment_id().is_some() {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "Desktop audio callers cannot select a media attachment",
            ));
        }
        let route = process_audio_attachments()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .operations
            .get(&key)
            .cloned();
        let Some(route) = route else {
            // The exact incarnation is already terminal or was never admitted.
            // Never resolve an absent context through the current attachment.
            return Ok(());
        };
        if context.owner != route.context.owner {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "audio cancellation owner does not match the admitted operation",
            ));
        }
        if !route.flight.dispatched.load(Ordering::Acquire) {
            return Err(AudioError::new(
                AudioErrorKind::Busy,
                "audio operation has not reached attachment admission",
            ));
        }
        if let Some(cleanup) = spawn_audio_route_cancel(key.clone(), route.clone()) {
            return match tokio::time::timeout(Duration::from_secs(3), cleanup).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(AudioError::new(
                    AudioErrorKind::NativeFailure,
                    "audio route cancellation ended without a completion receipt",
                )),
                Err(_) => Err(AudioError::new(
                    AudioErrorKind::Timeout,
                    "audio route cancellation has no completion receipt",
                )),
            };
        }
        cancel_audio_operation_route(key, route).await
    }
}

/// Keep unconfirmed attachments reachable for the lifetime of this app
/// process, even after one Bridge connection is dropped and replaced.
fn process_media_attachment_host() -> Arc<MediaAttachmentHost> {
    static HOST: OnceLock<Arc<MediaAttachmentHost>> = OnceLock::new();
    HOST.get_or_init(|| Arc::new(MediaAttachmentHost::new()))
        .clone()
}

struct RemoteConnectorState {
    connector: Option<Arc<dyn MediaConnector>>,
    supported_modes: Vec<MediaSessionMode>,
}

/// Dynamically installed product connector. The shared `MediaService` keeps
/// this one router for its lifetime while Remote Control installs or removes
/// the Firebase-backed adapter as the authenticated connection changes.
pub struct MediaConnectorRouter {
    local: Arc<media_runtime::LocalMediaConnector>,
    local_modes: RwLock<Vec<MediaSessionMode>>,
    remote: RwLock<RemoteConnectorState>,
    id: MediaConnectorId,
}

impl MediaConnectorRouter {
    /// Compose one always-available local connector with an optional aggregate
    /// product connector. Both share the exact app-scoped device/store inputs.
    #[must_use]
    pub fn new(
        local: Arc<media_runtime::LocalMediaConnector>,
        local_modes: Vec<MediaSessionMode>,
        id: impl Into<String>,
    ) -> Self {
        Self {
            local,
            local_modes: RwLock::new(local_modes),
            remote: RwLock::new(RemoteConnectorState {
                connector: None,
                supported_modes: Vec::new(),
            }),
            id: MediaConnectorId(id.into()),
        }
    }

    /// Install the product aggregate and its currently connected route modes.
    pub fn install_remote(
        &self,
        connector: Arc<dyn MediaConnector>,
        supported_modes: Vec<MediaSessionMode>,
    ) {
        *self.remote.write().unwrap_or_else(|e| e.into_inner()) = RemoteConnectorState {
            connector: Some(connector),
            supported_modes,
        };
    }

    /// Update authenticated remote route modes without replacing the aggregate.
    pub fn update_remote_modes(&self, supported_modes: Vec<MediaSessionMode>) {
        let mut remote = self.remote.write().unwrap_or_else(|e| e.into_inner());
        remote.supported_modes = if remote.connector.is_some() {
            supported_modes
        } else {
            Vec::new()
        };
    }

    /// Remove the product aggregate while preserving local device sessions.
    pub fn clear_remote(&self) {
        *self.remote.write().unwrap_or_else(|e| e.into_inner()) = RemoteConnectorState {
            connector: None,
            supported_modes: Vec::new(),
        };
    }

    /// Replace the structurally supported local session modes after the native
    /// audio capability snapshot changes.
    pub fn update_local_modes(&self, modes: Vec<MediaSessionMode>) -> Result<(), MediaError> {
        self.local.update_supported_modes(modes.clone())?;
        *self.local_modes.write().unwrap_or_else(|e| e.into_inner()) = modes;
        Ok(())
    }

    /// Current union used by the shared runtime's capability projection.
    pub fn supported_modes(&self) -> Vec<MediaSessionMode> {
        let mut modes = self
            .local_modes
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        modes.extend(
            self.remote
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .supported_modes
                .iter()
                .copied(),
        );
        deduplicate_modes(modes)
    }

    fn connectors(&self) -> (Arc<dyn MediaConnector>, Option<Arc<dyn MediaConnector>>) {
        (
            self.local.clone(),
            self.remote
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .connector
                .clone(),
        )
    }

    fn connector_matches_sources(sources: &[MediaSource], request: &MediaSessionRequest) -> bool {
        let selected = request
            .audio_source
            .iter()
            .chain(request.video_source.iter())
            .collect::<Vec<_>>();
        !selected.is_empty()
            && selected.iter().all(|reference| {
                sources.iter().any(|source| {
                    &source.reference == *reference
                        && source
                            .supported_operations
                            .contains(&media_api::MediaOperationKind::SessionStart)
                })
            })
    }

    fn local_matches_sources(
        additional_sources: &[MediaSource],
        request: &MediaSessionRequest,
    ) -> bool {
        let has_selection = request.audio_source.is_some() || request.video_source.is_some();
        has_selection
            && request
                .audio_source
                .as_ref()
                .is_none_or(|source| source.0 .0 == LOCAL_MICROPHONE_SOURCE_ID)
            && request.video_source.as_ref().is_none_or(|source| {
                source.0 .0 == LOCAL_CAMERA_SOURCE_ID
                    || additional_sources.iter().any(|candidate| {
                        candidate.reference == *source
                            && candidate
                                .supported_operations
                                .contains(&media_api::MediaOperationKind::SessionStart)
                    })
            })
    }

    fn selected_sources(request: &MediaSessionRequest) -> Vec<&MediaSourceRef> {
        request
            .audio_source
            .iter()
            .chain(request.video_source.iter())
            .collect()
    }
}

#[async_trait]
impl MediaConnector for MediaConnectorRouter {
    fn id(&self) -> MediaConnectorId {
        self.id.clone()
    }

    async fn sources(
        &self,
        context: MediaOperationContext,
    ) -> Result<Vec<MediaSource>, MediaError> {
        let (local, remote) = self.connectors();
        let mut sources = local.sources(context.clone()).await?;
        if let Some(remote) = remote {
            sources.extend(remote.sources(context).await?);
        }
        Ok(sources)
    }

    async fn start_session(
        &self,
        context: MediaOperationContext,
        request: MediaSessionRequest,
    ) -> Result<Box<dyn MediaSession>, MediaError> {
        if Self::selected_sources(&request).is_empty() {
            return self.local.start_session(context, request).await;
        }
        let (local, remote) = self.connectors();
        let local_sources = local.sources(context.clone()).await?;
        let matches_local = Self::local_matches_sources(&local_sources, &request);
        let Some(remote) = remote else {
            return if matches_local {
                local.start_session(context, request).await
            } else {
                Err(MediaError::new(
                    MediaErrorKind::NotFound,
                    "the selected source is not available on a configured media connector",
                ))
            };
        };
        let remote_sources = remote.sources(context.clone()).await?;
        let matches_remote = Self::connector_matches_sources(&remote_sources, &request);
        match (matches_local, matches_remote) {
            (true, false) => local.start_session(context, request).await,
            (false, true) => remote.start_session(context, request).await,
            (true, true) => Err(MediaError::new(
                MediaErrorKind::InvalidRequest,
                "a media source reference is ambiguous across connectors",
            )),
            (false, false) => Err(MediaError::new(
                MediaErrorKind::NotFound,
                "the selected source is not available on a configured media connector",
            )),
        }
    }
}

fn deduplicate_modes(modes: Vec<MediaSessionMode>) -> Vec<MediaSessionMode> {
    let mut result = Vec::new();
    for mode in modes {
        if !result.contains(&mode) {
            result.push(mode);
        }
    }
    result
}

/// One app process's common media service, native device ports, and resource
/// store. Agent sessions, native UI calls, and the Remote Control adapter all
/// keep this same object.
pub struct AppMediaHost {
    service: Arc<UnifiedMediaService>,
    audio_service: Arc<dyn AudioService>,
    closed: AtomicBool,
    media_attachment_host: Arc<MediaAttachmentHost>,
    current_attachment: Arc<RwLock<Option<MediaAttachmentHandle>>>,
    media_operation_routes: Mutex<HashMap<OperationRouteKey, MediaOperationRoute>>,
    ports: Arc<dyn MediaDevicePorts>,
    resources: Arc<dyn MediaResourceStore>,
    connector: Arc<MediaConnectorRouter>,
    cloud_audio: Arc<super::media_cloud::AudioProviderCloudRegistry>,
    audio_configuration_json: RwLock<Option<String>>,
}

impl AppMediaHost {
    /// Compose the shared media facade around the app's existing device lanes.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        resources: Arc<dyn MediaResourceStore>,
        audio: Arc<dyn media_api::audio::AudioService>,
        camera: Option<Arc<dyn media_api::camera::CameraControl>>,
        ports: Arc<dyn MediaDevicePorts>,
        local_modes: Vec<MediaSessionMode>,
        cloud_audio: Arc<super::media_cloud::AudioProviderCloudRegistry>,
        offline_audio: Vec<OfflineAudioBackend>,
        audio_routing: AudioRoutingSettings,
        service_epoch: u64,
        max_payload_bytes: u64,
    ) -> Result<Arc<Self>, MediaError> {
        let local = Arc::new(media_runtime::LocalMediaConnector::new(
            ports.clone(),
            resources.clone(),
            Vec::new(),
            local_modes.clone(),
        )?);
        let connector = Arc::new(MediaConnectorRouter::new(
            local,
            local_modes.clone(),
            "lingxi.app-media",
        ));
        let service = MediaRuntimeBuilderProduct::build(
            resources.clone(),
            audio,
            camera,
            connector.clone(),
            local_modes,
            cloud_audio.clone(),
            offline_audio,
            audio_routing,
            service_epoch,
            max_payload_bytes,
        );
        let audio_service = service.audio_service();
        Ok(Arc::new(Self {
            service,
            media_attachment_host: process_media_attachment_host(),
            current_attachment: Arc::new(RwLock::new(None)),
            closed: AtomicBool::new(false),
            media_operation_routes: Mutex::new(HashMap::new()),
            audio_service,
            ports,
            resources,
            connector,
            cloud_audio,
            audio_configuration_json: RwLock::new(None),
        }))
    }

    /// The exact app-scoped service injected into Agent runtimes and UI calls.
    #[must_use]
    pub fn media_service(self: &Arc<Self>) -> Arc<dyn MediaService> {
        // The app host itself is the service facade, so every consumer keeps
        // the same app-scoped ports/store/router and session registry alive.
        // The runtime instance is delegated to below.
        //
        // This method's receiver is changed to `&Arc<Self>` so the Arc can be
        // coerced without constructing a second service view.
        self.clone()
    }

    /// Compose one mobile engine scope over this host's actual shared service.
    ///
    /// Only the host-issued scoped handle and the exact registry used by the
    /// service leave this assembly boundary. Mobile engines own separate
    /// attachments; they do not use the Desktop single-attachment audio slot.
    #[doc(hidden)]
    pub fn create_mobile_media_attachment(
        &self,
        attachment_host: &MediaAttachmentHost,
    ) -> Result<
        (
            MediaAttachmentHandle,
            Arc<super::media_cloud::AudioProviderCloudRegistry>,
        ),
        MediaError,
    > {
        if self.closed.load(Ordering::Acquire) {
            return Err(self.attachment_unavailable());
        }
        let attachment = attachment_host.attach(self.service.clone())?;
        Ok((attachment, self.cloud_audio.clone()))
    }

    /// Mint one host-owned media attachment over the exact app service. The
    /// process registry retains the handle through `Unknown` so a replacement
    /// connection cannot silently create a fresh owner scope.
    pub fn create_media_attachment(&self) -> Result<MediaAttachmentHandle, MediaError> {
        let attachment = self.media_attachment_host.attach(self.service.clone())?;
        if let Err(error) =
            register_audio_attachment(attachment.clone(), self.audio_service.clone())
        {
            attachment.begin_revoke();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = attachment.revoke(Duration::from_secs(1)).await;
                });
            }
            return Err(error);
        }
        Ok(attachment)
    }

    /// Publish the active trusted owner attachment to the Agent runtime and
    /// Bridge. The slot is replaced only after the prior attachment is revoked.
    pub fn set_current_media_attachment(
        &self,
        attachment: Option<MediaAttachmentHandle>,
    ) -> Result<(), MediaError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "the app media host is closed",
            ));
        }
        if attachment
            .as_ref()
            .is_some_and(|handle| handle.phase() != MediaAttachmentPhase::Active)
        {
            return Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "cannot publish a media attachment before it is active",
            ));
        }
        if attachment.as_ref().is_some_and(|handle| {
            audio_attachment_state(handle.id())
                .is_none_or(|state| !Arc::ptr_eq(&state.shared_service, &self.audio_service))
        }) {
            return Err(MediaError::new(
                MediaErrorKind::InvalidRequest,
                "media attachment was not issued for this app media/audio service",
            ));
        }
        *self
            .current_attachment
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = attachment;
        Ok(())
    }

    /// Snapshot the current host-owned attachment for a Bridge operation.
    #[must_use]
    pub fn current_media_attachment(&self) -> Option<MediaAttachmentHandle> {
        self.current_attachment
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Synchronously close media/audio admission before the async owner drain.
    pub fn begin_close(&self) {
        self.closed.store(true, Ordering::Release);
        if let Some(attachment) = self.current_media_attachment() {
            attachment.begin_revoke();
        }
    }

    /// Process-scoped host registry retaining Confirmed/Unknown attachment
    /// ownership across Bridge connection reconstruction.
    #[must_use]
    pub fn media_attachment_host(&self) -> Arc<MediaAttachmentHost> {
        self.media_attachment_host.clone()
    }

    /// Shared slot used by Bridge and the already-built Desktop runtime. Both
    /// resolve the same host-owned attachment after an owner transition.
    #[must_use]
    pub fn media_attachment_slot(&self) -> Arc<RwLock<Option<MediaAttachmentHandle>>> {
        self.current_attachment.clone()
    }

    fn active_media_service(&self) -> Option<Arc<dyn MediaService>> {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        self.current_media_attachment()
            .map(|attachment| attachment.service())
    }

    fn current_open_attachment(&self) -> Result<MediaAttachmentHandle, MediaError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(self.attachment_unavailable());
        }
        self.current_media_attachment()
            .filter(|attachment| attachment.phase() == MediaAttachmentPhase::Active)
            .ok_or_else(|| self.attachment_unavailable())
    }

    fn remember_media_operation(
        &self,
        attachment: &MediaAttachmentHandle,
        context: &MediaOperationContext,
    ) -> Result<(OperationRouteKey, Arc<()>), MediaError> {
        if context.owner.attachment_id().is_some() {
            return Err(MediaError::new(
                MediaErrorKind::InvalidRequest,
                "media callers cannot select an attachment",
            ));
        }
        let key = OperationRouteKey {
            identity: context.identity.clone(),
            incarnation: context.operation_incarnation(),
        };
        let mut routes = self
            .media_operation_routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        routes.retain(|_, route| {
            route.attachment.phase() != MediaAttachmentPhase::Revoked
                && !(route.call_finished
                    && (!route.context.attachment_admitted()
                        || route.attachment.operation_cleanup(&route.context)
                            == MediaOperationCleanup::Confirmed))
        });
        if routes.len() >= 512 {
            return Err(MediaError::new(
                MediaErrorKind::Busy,
                "unresolved media operation routes are full",
            ));
        }
        if routes.contains_key(&key) {
            return Err(MediaError::new(
                MediaErrorKind::InvalidRequest,
                "media operation incarnation was already admitted",
            ));
        }
        let mut bound_context = context.clone();
        bound_context.owner = context.owner.clone().in_attachment(attachment.id().clone());
        let route_incarnation = Arc::new(());
        routes.insert(
            key.clone(),
            MediaOperationRoute {
                attachment: attachment.clone(),
                logical_owner: context.owner.clone(),
                context: bound_context,
                route_incarnation: route_incarnation.clone(),
                call_finished: false,
            },
        );
        Ok((key, route_incarnation))
    }

    fn forget_media_operation(&self, key: &OperationRouteKey, expected: &Arc<()>) {
        let mut routes = self
            .media_operation_routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = routes.get(key).map(|route| route.route_incarnation.clone());
        if current.is_some_and(|current| Arc::ptr_eq(&current, expected)) {
            routes.remove(key);
        }
    }

    fn guard_media_operation<'a>(
        &'a self,
        attachment: &MediaAttachmentHandle,
        context: &MediaOperationContext,
    ) -> Result<MediaOperationRouteGuard<'a>, MediaError> {
        let (key, incarnation) = self.remember_media_operation(attachment, context)?;
        Ok(MediaOperationRouteGuard {
            host: self,
            attachment: attachment.clone(),
            context: context.clone(),
            key,
            incarnation,
            succeeded: false,
        })
    }

    /// Raw shared service for host assembly. Mobile Harness assembly wraps this
    /// with its engine-owned attachment before exposing it to Computer Use.
    #[must_use]
    pub fn audio_service(&self) -> Arc<dyn AudioService> {
        self.audio_service.clone()
    }

    /// Desktop Computer Use facade. Each call captures the current Bridge
    /// attachment; the Harness attachment facade owns scope and drain behavior.
    #[must_use]
    pub fn desktop_audio_service(self: &Arc<Self>) -> Arc<dyn AudioService> {
        Arc::new(AppAudioService {
            host: Arc::downgrade(self),
        })
    }

    /// Native transport ports used by the configured RTC connector.
    #[must_use]
    pub fn device_ports(&self) -> Arc<dyn MediaDevicePorts> {
        self.ports.clone()
    }

    /// Host store used by tools and connector events for opaque media refs.
    #[must_use]
    pub fn resource_store(&self) -> Arc<dyn MediaResourceStore> {
        self.resources.clone()
    }

    /// Stable connector slot which Remote Control may install or clear.
    #[must_use]
    pub fn connector_router(&self) -> Arc<MediaConnectorRouter> {
        self.connector.clone()
    }

    /// Install the product's aggregate connector and publish only the session
    /// modes that its currently authenticated routes can actually start.
    /// Existing sessions retain their own connector-owned handles.
    pub fn install_connector(
        &self,
        connector: Arc<dyn MediaConnector>,
        supported_modes: impl IntoIterator<Item = MediaSessionMode>,
    ) {
        self.connector
            .install_remote(connector, supported_modes.into_iter().collect());
        self.service.update_connector(
            Some(self.connector.clone()),
            self.connector.supported_modes(),
        );
    }

    /// Replace the active aggregate's route-derived mode union after a viewer
    /// route is registered or removed without replacing its connector Arc.
    pub fn update_remote_session_modes(&self, supported_modes: Vec<MediaSessionMode>) {
        self.connector.update_remote_modes(supported_modes);
        self.service.update_connector(
            Some(self.connector.clone()),
            self.connector.supported_modes(),
        );
    }

    /// Remove future source discovery and session starts while allowing any
    /// explicitly stopped or still-owned session handles to finish teardown.
    pub fn clear_connector(&self) {
        self.connector.clear_remote();
        self.service.update_connector(
            Some(self.connector.clone()),
            self.connector.supported_modes(),
        );
    }

    /// Apply new owner-independent local session support while retaining the
    /// authenticated product aggregate and its route-derived modes.
    pub fn update_local_session_modes(
        &self,
        modes: Vec<MediaSessionMode>,
    ) -> Result<(), MediaError> {
        self.connector.update_local_modes(modes)?;
        self.service.update_connector(
            Some(self.connector.clone()),
            self.connector.supported_modes(),
        );
        Ok(())
    }

    /// Trusted provider/session registry used by the common audio route policy.
    #[must_use]
    pub fn cloud_audio_registry(&self) -> Arc<super::media_cloud::AudioProviderCloudRegistry> {
        self.cloud_audio.clone()
    }

    /// Register an authenticated incoming session under its original owner.
    pub async fn attach_incoming_session(
        &self,
        context: MediaOperationContext,
        session: Box<dyn MediaSession>,
    ) -> Result<MediaSessionRef, MediaError> {
        let attachment = self.current_open_attachment()?;
        let mut route = self.guard_media_operation(&attachment, &context)?;
        let result = attachment.service().attach_session(context, session).await;
        route.succeeded = result.is_ok();
        result
    }

    /// Resolve an already attached session's owner-checked PCM bridge. This
    /// returns the existing RTC/device channel and never opens another lease.
    pub async fn session_audio(
        &self,
        context: MediaOperationContext,
        session: MediaSessionRef,
    ) -> Result<Arc<dyn MediaAudioChannel>, MediaError> {
        let attachment = self.current_open_attachment()?;
        let mut route = self.guard_media_operation(&attachment, &context)?;
        let result = attachment.service().session_audio(context, session).await;
        route.succeeded = result.is_ok();
        result
    }

    /// Change the active audio-v4 route snapshot without replacing the service
    /// or its resource/session registries.
    pub fn update_audio_routing(&self, settings: AudioRoutingSettings) {
        self.service.update_audio_routing(settings);
    }

    /// Parse and atomically apply one current v4 settings snapshot. Invalid
    /// JSON/schema never replaces the last active route table.
    pub fn update_audio_routing_v4(&self, configuration_json: &str) -> Result<(), String> {
        let settings = super::media_configuration::audio_routing_settings_from_v4_json_checked(
            configuration_json,
        )?;
        self.update_audio_routing(settings);
        *self
            .audio_configuration_json
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(configuration_json.to_owned());
        Ok(())
    }

    /// Current bare v4 settings snapshot used by the selected native realtime
    /// Agent adapter. It is app-owned state and contains no credentials.
    pub fn audio_configuration_json(&self) -> Result<String, String> {
        self.audio_configuration_json
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| "audio settings have not been received from the app host".into())
    }
}

#[async_trait]
impl MediaService for AppMediaHost {
    fn capabilities(&self) -> MediaCapabilities {
        self.active_media_service().map_or_else(
            || self.unavailable_media_capabilities(),
            |service| service.capabilities(),
        )
    }

    fn attachment_id(&self) -> Option<MediaAttachmentId> {
        self.current_media_attachment()
            .map(|attachment| attachment.id().clone())
    }

    fn capabilities_for(&self, owner: &MediaOwner) -> MediaCapabilities {
        self.active_media_service().map_or_else(
            || self.unavailable_media_capabilities(),
            |service| service.capabilities_for(owner),
        )
    }

    async fn execute(
        &self,
        context: MediaOperationContext,
        operation: MediaOperation,
    ) -> Result<MediaResult, MediaError> {
        let attachment = self.current_open_attachment()?;
        let mut route = self.guard_media_operation(&attachment, &context)?;
        let result = attachment.service().execute(context, operation).await;
        route.succeeded = result.is_ok();
        result
    }

    async fn attach_session(
        &self,
        context: MediaOperationContext,
        session: Box<dyn MediaSession>,
    ) -> Result<MediaSessionRef, MediaError> {
        let attachment = self.current_open_attachment()?;
        let mut route = self.guard_media_operation(&attachment, &context)?;
        let result = attachment.service().attach_session(context, session).await;
        route.succeeded = result.is_ok();
        result
    }

    async fn session_audio(
        &self,
        context: MediaOperationContext,
        session: MediaSessionRef,
    ) -> Result<Arc<dyn MediaAudioChannel>, MediaError> {
        let attachment = self.current_open_attachment()?;
        let mut route = self.guard_media_operation(&attachment, &context)?;
        let result = attachment.service().session_audio(context, session).await;
        route.succeeded = result.is_ok();
        result
    }

    async fn read_media(
        &self,
        context: MediaOperationContext,
        media: MediaRef,
    ) -> Result<MediaContent, MediaError> {
        let attachment = self.current_open_attachment()?;
        let mut route = self.guard_media_operation(&attachment, &context)?;
        let result = attachment.service().read_media(context, media).await;
        route.succeeded = result.is_ok();
        result
    }

    async fn cancel(&self, context: MediaOperationContext) -> Result<(), MediaError> {
        let key = OperationRouteKey {
            identity: context.identity.clone(),
            incarnation: context.operation_incarnation(),
        };
        let route = self
            .media_operation_routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .map(|route| MediaOperationRoute {
                attachment: route.attachment.clone(),
                logical_owner: route.logical_owner.clone(),
                context: route.context.clone(),
                route_incarnation: route.route_incarnation.clone(),
                call_finished: route.call_finished,
            });
        let Some(route) = route else {
            // Never reinterpret an absent old context through the new current
            // attachment. The exact operation is already terminal or unknown.
            return Ok(());
        };
        if context.owner != route.logical_owner {
            return Err(MediaError::new(
                MediaErrorKind::InvalidRequest,
                "media cancellation owner does not match its admitted operation",
            ));
        }
        // The cancellation request may be reconstructed from a wire handle.
        // Only the retained admitted context carries this call's work witness.
        let cleanup_context = route.context.clone();
        let result = if route.attachment.phase() == MediaAttachmentPhase::Active {
            let mut unscoped = route.context.clone();
            unscoped.owner = route.logical_owner;
            route.attachment.service().cancel(unscoped).await
        } else {
            self.service.cancel(route.context).await
        };
        let cleanup = if cleanup_context.attachment_admitted() {
            route.attachment.operation_cleanup(&cleanup_context)
        } else if !route.call_finished {
            // The caller may still cross attachment admission on another
            // thread after this cancellation snapshot.
            MediaOperationCleanup::Pending
        } else {
            MediaOperationCleanup::Confirmed
        };
        if result.is_ok() && cleanup == MediaOperationCleanup::Confirmed {
            self.forget_media_operation(&key, &route.route_incarnation);
        }
        match (result, cleanup) {
            (Ok(()), MediaOperationCleanup::Pending | MediaOperationCleanup::Unknown) => {
                Err(MediaError::new(
                    MediaErrorKind::Busy,
                    "media cancellation cleanup remains unconfirmed",
                ))
            }
            (result, _) => result,
        }
    }
}

impl AppMediaHost {
    fn attachment_unavailable(&self) -> MediaError {
        MediaError::new(
            MediaErrorKind::Unavailable,
            "the app media owner has no active attachment",
        )
    }

    fn unavailable_media_capabilities(&self) -> MediaCapabilities {
        let mut capabilities = self.service.capabilities();
        capabilities.supported_operations.clear();
        capabilities.supported_media_kinds.clear();
        capabilities.readiness.clear();
        capabilities
    }
}

struct MediaRuntimeBuilderProduct;

impl MediaRuntimeBuilderProduct {
    #[allow(clippy::too_many_arguments)]
    fn build(
        resources: Arc<dyn MediaResourceStore>,
        audio: Arc<dyn media_api::audio::AudioService>,
        camera: Option<Arc<dyn media_api::camera::CameraControl>>,
        connector: Arc<MediaConnectorRouter>,
        supported_modes: Vec<MediaSessionMode>,
        cloud_audio: Arc<super::media_cloud::AudioProviderCloudRegistry>,
        offline_audio: Vec<OfflineAudioBackend>,
        audio_routing: AudioRoutingSettings,
        service_epoch: u64,
        max_payload_bytes: u64,
    ) -> Arc<UnifiedMediaService> {
        let builder = media_runtime::MediaRuntimeBuilder::new(resources).audio(audio);
        let builder = if let Some(camera) = camera {
            builder.camera(camera)
        } else {
            builder
        };
        builder
            .connector(connector, supported_modes)
            .offline_audio(offline_audio)
            .audio_routing(audio_routing)
            .service_epoch(service_epoch)
            .max_payload_bytes(max_payload_bytes)
            .cloud_audio_host(cloud_audio)
            .build()
    }
}

impl std::fmt::Debug for AppMediaHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AppMediaHost(<app-scoped>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use media_api::audio::{
        AudioCapabilitySnapshot, AudioError, AudioErrorKind, AudioOperation, AudioOperationContext,
        AudioOperationId, AudioOperationKind, AudioOperationSuccess, AudioService,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    static AUDIO_ATTACHMENT_TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct UnsupportedAudio;

    #[async_trait]
    impl AudioService for UnsupportedAudio {
        fn capabilities(&self) -> AudioCapabilitySnapshot {
            AudioCapabilitySnapshot {
                service_epoch: 1,
                support_revision: 1,
                supported_operations: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }

        async fn execute(
            &self,
            _context: AudioOperationContext,
            _operation: AudioOperation,
        ) -> Result<AudioOperationSuccess, AudioError> {
            Err(AudioError::new(
                AudioErrorKind::Unsupported,
                "the capability projection test does not execute audio",
            ))
        }

        async fn cancel(&self, _context: AudioOperationContext) -> Result<(), AudioError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingAudio {
        executed: Mutex<Vec<AudioOperationContext>>,
        cancelled: Mutex<Vec<AudioOperationContext>>,
        cancelled_operations: Mutex<HashSet<OperationRouteKey>>,
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        block_end_owner: AtomicBool,
        end_owner_entered: tokio::sync::Notify,
        release_end_owner: tokio::sync::Notify,
        end_owner_calls: AtomicUsize,
    }

    #[async_trait]
    impl AudioService for RecordingAudio {
        fn capabilities(&self) -> AudioCapabilitySnapshot {
            AudioCapabilitySnapshot {
                service_epoch: 7,
                support_revision: 1,
                supported_operations: vec![AudioOperationKind::Speak],
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }

        async fn execute(
            &self,
            context: AudioOperationContext,
            operation: AudioOperation,
        ) -> Result<AudioOperationSuccess, AudioError> {
            let key = OperationRouteKey {
                identity: context.identity.clone(),
                incarnation: context.operation_incarnation(),
            };
            self.executed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(context);
            match operation {
                AudioOperation::Speak { .. } => {
                    self.entered.notify_one();
                    self.release.notified().await;
                    let cancelled = self
                        .cancelled_operations
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .contains(&key);
                    if cancelled {
                        Err(AudioError::new(
                            AudioErrorKind::Cancelled,
                            "test audio operation was cancelled",
                        ))
                    } else {
                        Ok(AudioOperationSuccess::PlaybackCompleted { duration_ms: 1 })
                    }
                }
                AudioOperation::EndOwner => {
                    self.end_owner_calls.fetch_add(1, Ordering::SeqCst);
                    if self.block_end_owner.load(Ordering::SeqCst) {
                        self.end_owner_entered.notify_one();
                        self.release_end_owner.notified().await;
                    }
                    Ok(AudioOperationSuccess::OwnerEnded)
                }
                AudioOperation::Status { .. } => Ok(AudioOperationSuccess::Status {
                    status: media_api::audio::AudioStatus::default(),
                }),
                _ => Err(AudioError::new(
                    AudioErrorKind::Unsupported,
                    "test audio operation is unsupported",
                )),
            }
        }

        async fn cancel(&self, context: AudioOperationContext) -> Result<(), AudioError> {
            self.cancelled_operations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(OperationRouteKey {
                    identity: context.identity.clone(),
                    incarnation: context.operation_incarnation(),
                });
            self.cancelled
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(context);
            self.release.notify_one();
            Ok(())
        }
    }

    struct UnopenedDevicePorts;

    #[async_trait]
    impl MediaDevicePorts for UnopenedDevicePorts {
        async fn open(
            &self,
            _context: MediaOperationContext,
            _request: MediaSessionRequest,
        ) -> Result<Arc<dyn media_api::MediaDeviceSession>, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "the capability projection test does not open devices",
            ))
        }
    }

    #[tokio::test]
    async fn mobile_engine_scopes_use_the_real_host_service_and_shared_isolated_routes() {
        use media_api::{MediaOperationKind, MediaReadinessState};
        use media_runtime::{AudioRouteMode, CloudBinding, MediaAttachmentRevocation};

        let mut routing = AudioRoutingSettings::default();
        routing.operations.insert(
            AudioOperationKind::Transcribe,
            AudioRouteMode::Cloud {
                binding: CloudBinding::FollowSession,
                model: None,
            },
        );
        let registry = Arc::new(super::super::media_cloud::AudioProviderCloudRegistry::new(
            "wav",
        ));
        let host = AppMediaHost::new(
            Arc::new(super::super::media_store::BoundedMediaResourceStore::default()),
            Arc::new(RecordingAudio::default()),
            None,
            Arc::new(UnopenedDevicePorts),
            Vec::new(),
            registry.clone(),
            Vec::new(),
            routing,
            1,
            1024,
        )
        .unwrap();
        let attachment_host = MediaAttachmentHost::new();
        let (first, first_registry) = host
            .create_mobile_media_attachment(&attachment_host)
            .unwrap();
        let (second, second_registry) = host
            .create_mobile_media_attachment(&attachment_host)
            .unwrap();
        assert_ne!(first.id(), second.id());
        assert!(Arc::ptr_eq(&first_registry, &registry));
        assert!(Arc::ptr_eq(&second_registry, &registry));
        assert!(host.current_media_attachment().is_none());

        let owner = MediaOwner::Session {
            session_id: "same-session".into(),
        };
        for (generation, attachment) in [(1, &first), (2, &second)] {
            let service = attachment.service();
            let capabilities = service.capabilities_for(&owner);
            assert!(capabilities.supports(MediaOperationKind::Sources));
            let context = MediaOperationContext::new(
                MediaOperationId::new(generation, capabilities.service_epoch),
                owner.clone(),
            );
            assert!(matches!(
                service
                    .execute(
                        context,
                        MediaOperation::Sources {
                            kind: None,
                            usage: None
                        }
                    )
                    .await,
                Ok(MediaResult::Sources { .. })
            ));
        }
        let readiness = |attachment: &MediaAttachmentHandle| {
            attachment
                .service()
                .capabilities_for(&owner)
                .readiness
                .into_iter()
                .find(|entry| entry.operation == MediaOperationKind::Transcribe)
                .map(|entry| entry.state)
        };
        assert_eq!(readiness(&first), Some(MediaReadinessState::Unavailable));
        assert_eq!(readiness(&second), Some(MediaReadinessState::Unavailable));

        let provider = || {
            Arc::new(
                crate::AudioProviderHost::new(
                    "{}",
                    "international",
                    None,
                    std::collections::BTreeMap::new(),
                )
                .unwrap(),
            )
        };
        let first_provider = provider();
        let second_provider = provider();
        for (attachment, provider) in [(&first, &first_provider), (&second, &second_provider)] {
            let permit = attachment.authorize_owner(&owner).unwrap();
            registry
                .register_attached_profile(&permit, "openai", provider.clone())
                .unwrap();
            registry
                .bind_attached_session_to_profile(&permit, "openai", "profile:openai")
                .unwrap();
        }
        assert!(Arc::ptr_eq(
            &registry
                .provider_for_attached_session(
                    first.id(),
                    "same-session",
                    "openai",
                    "profile:openai"
                )
                .unwrap(),
            &first_provider,
        ));
        assert!(Arc::ptr_eq(
            &registry
                .provider_for_attached_session(
                    second.id(),
                    "same-session",
                    "openai",
                    "profile:openai"
                )
                .unwrap(),
            &second_provider,
        ));
        assert_eq!(readiness(&first), Some(MediaReadinessState::Ready));
        assert_eq!(readiness(&second), Some(MediaReadinessState::Ready));
        assert!(registry
            .provider_for_session("same-session", "openai", "profile:openai")
            .is_err());

        let (idle, _) = host
            .create_mobile_media_attachment(&attachment_host)
            .unwrap();
        assert_eq!(
            idle.revoke(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        assert_eq!(
            first.revoke(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        registry.unregister_attachment(first.id());
        assert!(registry
            .provider_for_attached_session(first.id(), "same-session", "openai", "profile:openai")
            .is_err());
        assert!(Arc::ptr_eq(
            &registry
                .provider_for_attached_session(
                    second.id(),
                    "same-session",
                    "openai",
                    "profile:openai"
                )
                .unwrap(),
            &second_provider,
        ));
        assert_eq!(readiness(&second), Some(MediaReadinessState::Ready));
        assert_eq!(
            second.revoke(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        registry.unregister_attachment(second.id());
    }

    #[tokio::test]
    async fn local_audio_session_is_advertised_only_after_explicit_device_modes_arrive() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let host = AppMediaHost::new(
            Arc::new(super::super::media_store::BoundedMediaResourceStore::default()),
            Arc::new(UnsupportedAudio),
            None,
            Arc::new(UnopenedDevicePorts),
            Vec::new(),
            Arc::new(super::super::media_cloud::AudioProviderCloudRegistry::new(
                "wav",
            )),
            Vec::new(),
            AudioRoutingSettings::default(),
            1,
            1024,
        )
        .expect("an app host may start before desktop helper capabilities arrive");
        let attachment = host
            .create_media_attachment()
            .expect("the host issues the media owner attachment");
        host.set_current_media_attachment(Some(attachment))
            .expect("the initial owner attachment is active");
        let owner = MediaOwner::Session {
            session_id: "desktop-session".into(),
        };
        assert!(
            !host
                .media_service()
                .capabilities_for(&owner)
                .supports(media_api::MediaOperationKind::SessionStart),
            "absence of a native media-session capability must not advertise local audio"
        );

        host.update_local_session_modes(vec![MediaSessionMode::Audio])
            .expect("the helper's explicit duplex mode is applied");
        assert!(
            host.media_service()
                .capabilities_for(&owner)
                .supports(media_api::MediaOperationKind::SessionStart),
            "the exact helper mode must make local audio sessions available"
        );

        host.update_local_session_modes(Vec::new())
            .expect("helper loss clears its local mode inventory");
        assert!(
            !host
                .media_service()
                .capabilities_for(&owner)
                .supports(media_api::MediaOperationKind::SessionStart),
            "a disconnected native helper must remove local audio support"
        );

        let attachment = host
            .current_media_attachment()
            .expect("attachment published");
        assert_eq!(
            attachment.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    #[tokio::test]
    async fn media_cancel_after_slot_rotation_uses_captured_attachment_and_incarnation() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let host = AppMediaHost::new(
            Arc::new(super::super::media_store::BoundedMediaResourceStore::default()),
            Arc::new(UnsupportedAudio),
            None,
            Arc::new(UnopenedDevicePorts),
            Vec::new(),
            Arc::new(super::super::media_cloud::AudioProviderCloudRegistry::new(
                "wav",
            )),
            Vec::new(),
            AudioRoutingSettings::default(),
            1,
            1024,
        )
        .expect("the test app media host composes");
        let original = host
            .create_media_attachment()
            .expect("the host creates its initial attachment");
        host.set_current_media_attachment(Some(original.clone()))
            .expect("the initial attachment is published");
        let owner = MediaOwner::Session {
            session_id: "media-owner-rotation".into(),
        };
        let identity = MediaOperationId::new(51, 1);
        let old_context = MediaOperationContext::new(identity.clone(), owner.clone());
        let (old_key, _) = host
            .remember_media_operation(&original, &old_context)
            .expect("the old operation is routed to its admitted attachment");

        original.begin_revoke();
        let routes = host
            .media_operation_routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let old_route = routes
            .get(&old_key)
            .expect("the old cancellation route stays retained");
        assert_eq!(old_route.attachment.id(), original.id());
        drop(routes);
        assert_eq!(
            original.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );

        let replacement = host
            .create_media_attachment()
            .expect("the old attachment is confirmed before replacement");
        host.set_current_media_attachment(Some(replacement.clone()))
            .expect("the replacement attachment is published");
        let reused_context = MediaOperationContext::new(identity, owner.clone());
        let (new_key, _) = host
            .remember_media_operation(&replacement, &reused_context)
            .expect("a reused public ID has a distinct opaque incarnation");
        assert_ne!(old_key.incarnation, new_key.incarnation);

        host.cancel(old_context)
            .await
            .expect("late old cancellation is a no-op, not forwarded to the current slot");
        let routes = host
            .media_operation_routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(!routes.contains_key(&old_key));
        assert_eq!(
            routes
                .get(&new_key)
                .expect("late old cancellation must retain the reused ID's new route")
                .attachment
                .id(),
            replacement.id()
        );
        drop(routes);

        assert_eq!(
            replacement.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    #[tokio::test]
    async fn late_media_completion_cannot_remove_a_reused_route_incarnation() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let host = AppMediaHost::new(
            Arc::new(super::super::media_store::BoundedMediaResourceStore::default()),
            Arc::new(UnsupportedAudio),
            None,
            Arc::new(UnopenedDevicePorts),
            Vec::new(),
            Arc::new(super::super::media_cloud::AudioProviderCloudRegistry::new(
                "wav",
            )),
            Vec::new(),
            AudioRoutingSettings::default(),
            1,
            1024,
        )
        .expect("the test app media host composes");
        let attachment = host
            .create_media_attachment()
            .expect("the host creates its attachment");
        host.set_current_media_attachment(Some(attachment.clone()))
            .expect("the attachment is published");
        let context = MediaOperationContext::new(
            MediaOperationId::new(71, 1),
            MediaOwner::Session {
                session_id: "route-incarnation-replay".into(),
            },
        );
        let (old_key, old_route_incarnation) = host
            .remember_media_operation(&attachment, &context)
            .expect("the first operation route is admitted");
        host.forget_media_operation(&old_key, &old_route_incarnation);
        let (replayed_key, replayed_route_incarnation) = host
            .remember_media_operation(&attachment, &context)
            .expect("test replays the exact context after its earlier completion");
        assert_eq!(old_key, replayed_key);
        assert!(!Arc::ptr_eq(
            &old_route_incarnation,
            &replayed_route_incarnation
        ));

        // A late completion from the old route must compare its exact route
        // incarnation before deleting anything under the reused operation key.
        host.forget_media_operation(&old_key, &old_route_incarnation);
        let routes = host
            .media_operation_routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(routes.get(&replayed_key).is_some_and(|route| Arc::ptr_eq(
            &route.route_incarnation,
            &replayed_route_incarnation
        )));
        drop(routes);

        assert_eq!(
            attachment.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    #[tokio::test]
    async fn computer_use_cancel_after_slot_rotation_stays_on_the_original_attachment() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(RecordingAudio::default());
        let host = AppMediaHost::new(
            Arc::new(super::super::media_store::BoundedMediaResourceStore::default()),
            native.clone(),
            None,
            Arc::new(UnopenedDevicePorts),
            Vec::new(),
            Arc::new(super::super::media_cloud::AudioProviderCloudRegistry::new(
                "wav",
            )),
            Vec::new(),
            AudioRoutingSettings::default(),
            1,
            1024,
        )
        .expect("the test app media host composes");
        let original = host
            .create_media_attachment()
            .expect("the host creates its initial attachment");
        host.set_current_media_attachment(Some(original.clone()))
            .expect("the initial attachment is published");
        let service = host.desktop_audio_service();
        let owner = MediaOwner::Session {
            session_id: "computer-use-owner-rotation".into(),
        };
        let service_epoch = service.capabilities().service_epoch;
        let context =
            AudioOperationContext::new(AudioOperationId::new(41, service_epoch), owner.clone());
        let caller_selected_context = AudioOperationContext::new(
            AudioOperationId::new(42, service_epoch),
            owner.clone().in_attachment(original.id().clone()),
        );
        let caller_selected_error = service
            .execute(
                caller_selected_context,
                AudioOperation::Speak {
                    text: "must not start".into(),
                    language: None,
                    rate: None,
                    voice: None,
                },
            )
            .await
            .expect_err("Desktop callers cannot choose an attachment scope");
        assert_eq!(caller_selected_error.kind, AudioErrorKind::InvalidRequest);
        assert!(native
            .executed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());
        let incarnation = context.operation_incarnation();
        let mut call = tokio::spawn({
            let service = service.clone();
            let context = context.clone();
            async move {
                service
                    .execute(
                        context,
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
        tokio::select! {
            _ = native.entered.notified() => {}
            result = &mut call => panic!("audio call completed before native admission: {result:?}"),
            _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("audio call did not reach native admission"),
        }

        // Revocation closes external admission and owns cancellation of the
        // exact old operation. A caller cannot issue a second cancel through
        // the closed facade; the drain must still cancel the original route.
        original.begin_revoke();
        let rejected = service
            .cancel(context.clone())
            .await
            .expect_err("a revoking attachment rejects a new cancel admission");
        assert_eq!(rejected.kind, AudioErrorKind::Cancelled);
        assert_eq!(
            original.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed,
            "the drain must cancel and close the original owner"
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), call)
                .await
                .expect("the attachment drain must cancel the original audio call")
                .unwrap()
                .unwrap_err()
                .kind,
            AudioErrorKind::Cancelled
        );
        let cancelled = native
            .cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(cancelled.len(), 1);
        assert_eq!(
            cancelled[0].owner,
            owner.clone().in_attachment(original.id().clone())
        );
        assert!(cancelled[0]
            .operation_incarnation()
            .same_instance(&incarnation));
        drop(cancelled);

        let replacement = host
            .create_media_attachment()
            .expect("the old attachment is fully closed before replacement");
        host.set_current_media_attachment(Some(replacement.clone()))
            .expect("the replacement attachment is published");
        service
            .cancel(context)
            .await
            .expect("a terminal old context must be a no-op, not use the new slot");
        assert_eq!(
            native
                .cancelled
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1,
            "the old cancellation must not be forwarded to the replacement attachment"
        );
        assert_eq!(
            replacement.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    #[tokio::test]
    async fn audio_owner_cleanup_timeout_keeps_one_flight_and_closes_admission() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(RecordingAudio::default());
        let host = AppMediaHost::new(
            Arc::new(super::super::media_store::BoundedMediaResourceStore::default()),
            native.clone(),
            None,
            Arc::new(UnopenedDevicePorts),
            Vec::new(),
            Arc::new(super::super::media_cloud::AudioProviderCloudRegistry::new(
                "wav",
            )),
            Vec::new(),
            AudioRoutingSettings::default(),
            1,
            1024,
        )
        .expect("the test app media host composes");
        let attachment = host
            .create_media_attachment()
            .expect("the host creates its attachment");
        host.set_current_media_attachment(Some(attachment.clone()))
            .expect("the attachment is published");
        let service = host.desktop_audio_service();
        let service_epoch = service.capabilities().service_epoch;
        let context = AudioOperationContext::new(
            AudioOperationId::new(61, service_epoch),
            MediaOwner::Session {
                session_id: "bounded-audio-cleanup".into(),
            },
        );
        let mut operation = tokio::spawn({
            let service = service.clone();
            let context = context.clone();
            async move {
                service
                    .execute(
                        context,
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
        tokio::select! {
            _ = native.entered.notified() => {}
            result = &mut operation => panic!("audio call completed before native admission: {result:?}"),
            _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("audio call did not reach native admission"),
        }
        native.block_end_owner.store(true, Ordering::SeqCst);
        attachment.begin_revoke();

        let observed = attachment.revoke(Duration::from_millis(10)).await;
        assert!(matches!(
            observed,
            media_runtime::MediaAttachmentRevocation::Unknown { .. }
        ));
        tokio::time::timeout(Duration::from_secs(2), native.end_owner_entered.notified())
            .await
            .expect("attachment drain must reach native owner teardown");
        let key = OperationRouteKey {
            identity: context.identity.clone(),
            incarnation: context.operation_incarnation(),
        };
        let routes = process_audio_attachments()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending = routes
            .operations
            .get(&key)
            .expect("Unknown drain must retain the original audio route");
        assert_eq!(pending.attachment.handle.id(), attachment.id());
        assert_eq!(pending.context.owner, context.owner);
        drop(routes);

        let rejected = service
            .execute(
                AudioOperationContext::new(
                    AudioOperationId::new(62, service_epoch),
                    MediaOwner::Session {
                        session_id: "bounded-audio-cleanup".into(),
                    },
                ),
                AudioOperation::Speak {
                    text: "must not start".into(),
                    language: None,
                    rate: None,
                    voice: None,
                },
            )
            .await
            .expect_err("Revoking attachment rejects new Desktop audio");
        assert_eq!(rejected.kind, AudioErrorKind::Unavailable);

        native.release_end_owner.notify_one();
        assert_eq!(
            attachment.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed,
            "retry observes the original EndOwner flight"
        );
        assert_eq!(native.end_owner_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            operation.await.unwrap().unwrap_err().kind,
            AudioErrorKind::Cancelled
        );
        let replacement = host
            .create_media_attachment()
            .expect("Confirmed drain permits a new trusted attachment");
        host.set_current_media_attachment(Some(replacement.clone()))
            .expect("replacement is published after the confirmed drain");
        service
            .cancel(context)
            .await
            .expect("the retired incarnation cannot be resolved through the new slot");
        assert_eq!(
            native
                .cancelled
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1,
            "late cancellation stays on the original attachment and does not replay"
        );
        assert_eq!(
            replacement.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    struct NativeOpenFixture {
        opened: tokio::sync::Notify,
        open_gate: tokio::sync::Semaphore,
        stop_started: tokio::sync::Notify,
        stop_gate: tokio::sync::Semaphore,
        fail_open: AtomicBool,
        confirmed_open_failure: AtomicBool,
        hold_stop: AtomicBool,
        stop_failures: AtomicUsize,
    }

    impl Default for NativeOpenFixture {
        fn default() -> Self {
            Self {
                opened: tokio::sync::Notify::new(),
                open_gate: tokio::sync::Semaphore::new(0),
                stop_started: tokio::sync::Notify::new(),
                stop_gate: tokio::sync::Semaphore::new(0),
                fail_open: AtomicBool::new(false),
                confirmed_open_failure: AtomicBool::new(false),
                hold_stop: AtomicBool::new(false),
                stop_failures: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl super::super::media_native::NativeMediaDeviceCallback for NativeOpenFixture {
        async fn request(
            &self,
            request: client::protocol::media::MediaDeviceRequestEnvelopeDto,
        ) -> client::protocol::media::MediaDeviceResponseDto {
            use client::protocol::media as dto;
            let result = match request.request {
                dto::MediaDeviceRequestDto::Open { .. }
                    if self.fail_open.load(Ordering::Acquire) =>
                {
                    let error = dto::MediaErrorDto {
                        kind: dto::MediaErrorKindDto::NativeFailure,
                        message: "fixture native open failed".into(),
                    };
                    if self.confirmed_open_failure.load(Ordering::Acquire) {
                        dto::MediaDeviceResultDto::OpenFailedWithoutResource { error }
                    } else {
                        dto::MediaDeviceResultDto::Error { error }
                    }
                }
                dto::MediaDeviceRequestDto::Open { .. } => {
                    // A real native lease exists before the response is released.
                    self.opened.notify_one();
                    self.open_gate.acquire().await.unwrap().forget();
                    dto::MediaDeviceResultDto::Opened {
                        lease: dto::MediaDeviceLeaseDto {
                            lease_id: "held-native-lease".into(),
                        },
                    }
                }
                dto::MediaDeviceRequestDto::Stop { .. } => {
                    self.stop_started.notify_one();
                    if self.hold_stop.load(Ordering::Acquire) {
                        self.stop_gate.acquire().await.unwrap().forget();
                    }
                    if self
                        .stop_failures
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
                        .is_ok()
                    {
                        dto::MediaDeviceResultDto::Error {
                            error: dto::MediaErrorDto {
                                kind: dto::MediaErrorKindDto::NativeFailure,
                                message: "fixture native stop failed".into(),
                            },
                        }
                    } else {
                        dto::MediaDeviceResultDto::Stopped
                    }
                }
                dto::MediaDeviceRequestDto::NextInput { .. } => std::future::pending().await,
                other => panic!("unexpected native callback: {other:?}"),
            };
            dto::MediaDeviceResponseDto {
                request_id: request.request_id,
                result,
            }
        }
    }

    fn native_open_host() -> (
        Arc<AppMediaHost>,
        MediaAttachmentHandle,
        Arc<NativeOpenFixture>,
    ) {
        let callback = Arc::new(NativeOpenFixture::default());
        let ports = Arc::new(super::super::media_native::CallbackMediaDevicePorts::new(
            callback.clone(),
        ));
        let host = AppMediaHost::new(
            Arc::new(super::super::media_store::BoundedMediaResourceStore::default()),
            Arc::new(UnsupportedAudio),
            None,
            ports,
            vec![MediaSessionMode::Audio],
            Arc::new(super::super::media_cloud::AudioProviderCloudRegistry::new(
                "wav",
            )),
            Vec::new(),
            AudioRoutingSettings::default(),
            1,
            1024,
        )
        .unwrap();
        // Keep deliberately unknown outcomes local to this fixture. The media
        // execution path uses this slot directly; audio binding has separate
        // tests and its process-wide registry must not be poisoned here.
        let issuer = MediaAttachmentHost::new();
        let attachment = issuer.attach(host.service.clone()).unwrap();
        *host.current_attachment.write().unwrap() = Some(attachment.clone());
        (host, attachment, callback)
    }

    fn native_start(generation: u64) -> (MediaOperationContext, MediaOperation) {
        let context = MediaOperationContext::new(
            MediaOperationId::new(generation, 1),
            MediaOwner::Session {
                session_id: "native-open-test".into(),
            },
        );
        let request = MediaSessionRequest {
            intent: media_api::MediaSessionIntent::Observation,
            mode: MediaSessionMode::Audio,
            audio_source: Some(MediaSourceRef(media_api::MediaSourceId::from(
                "media-runtime:microphone",
            ))),
            video_source: None,
        };
        (context, MediaOperation::SessionStart { request })
    }

    #[tokio::test]
    async fn cancellation_before_attachment_admission_cannot_forget_the_live_route() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let (host, attachment, _) = native_open_host();
        let (context, _) = native_start(3000);
        let route = host.guard_media_operation(&attachment, &context).unwrap();
        assert!(!context.attachment_admitted());
        assert_eq!(
            host.cancel(context).await.unwrap_err().kind,
            MediaErrorKind::Busy
        );
        assert_eq!(host.media_operation_routes.lock().unwrap().len(), 1);
        drop(route);
        assert!(host.media_operation_routes.lock().unwrap().is_empty());
        assert_eq!(
            attachment.drain_media(Duration::from_secs(2)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    #[tokio::test]
    async fn native_open_and_late_stop_block_attachment_media_drain() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let (host, attachment, callback) = native_open_host();
        let mut fence = attachment.retain_for_host_cleanup().unwrap();
        callback.hold_stop.store(true, Ordering::Release);
        let (context, operation) = native_start(3001);
        let call_host = host.clone();
        let call_context = context.clone();
        let call = tokio::spawn(async move { call_host.execute(call_context, operation).await });
        tokio::time::timeout(Duration::from_secs(2), callback.opened.notified())
            .await
            .unwrap();
        call.abort();
        assert!(matches!(call.await, Err(error) if error.is_cancelled()));
        let cancellation =
            MediaOperationContext::new(context.identity.clone(), context.owner.clone())
                .with_operation_handle(context.operation_incarnation());
        assert_eq!(
            host.cancel(cancellation).await.unwrap_err().kind,
            MediaErrorKind::Busy
        );
        assert!(matches!(
            attachment.drain_media(Duration::from_millis(20)).await,
            media_runtime::MediaAttachmentRevocation::Unknown { .. }
        ));
        assert_eq!(attachment.phase(), MediaAttachmentPhase::Revoking);
        assert!(!attachment.media_drain_confirmed());
        callback.open_gate.add_permits(1);
        tokio::time::timeout(Duration::from_secs(2), callback.stop_started.notified())
            .await
            .unwrap();
        assert!(matches!(
            attachment.drain_media(Duration::from_millis(20)).await,
            media_runtime::MediaAttachmentRevocation::Unknown { .. }
        ));
        callback.stop_gate.add_permits(1);
        assert_eq!(
            attachment.drain_media(Duration::from_secs(2)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
        assert_eq!(attachment.phase(), MediaAttachmentPhase::Revoking);
        fence.confirm().unwrap();
        assert_eq!(attachment.phase(), MediaAttachmentPhase::Revoked);
        host.cancel(context).await.unwrap();
        assert!(host.media_operation_routes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unconfirmed_native_open_failure_keeps_attachment_and_route() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let (host, attachment, callback) = native_open_host();
        callback.fail_open.store(true, Ordering::Release);
        let (context, operation) = native_start(3002);
        assert_eq!(
            host.execute(context.clone(), operation)
                .await
                .unwrap_err()
                .kind,
            MediaErrorKind::NativeFailure
        );
        assert_eq!(
            attachment.operation_cleanup(&context),
            MediaOperationCleanup::Unknown
        );
        assert_eq!(host.media_operation_routes.lock().unwrap().len(), 1);
        assert!(matches!(
            attachment.drain_media(Duration::from_secs(2)).await,
            media_runtime::MediaAttachmentRevocation::Unknown { .. }
        ));
        assert_eq!(attachment.phase(), MediaAttachmentPhase::Unknown);
        assert_eq!(host.media_operation_routes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn failed_late_native_stop_keeps_attachment_and_route_unknown() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let (host, attachment, callback) = native_open_host();
        callback.stop_failures.store(3, Ordering::Release);
        let (context, operation) = native_start(3004);
        let call_host = host.clone();
        let call_context = context.clone();
        let call = tokio::spawn(async move { call_host.execute(call_context, operation).await });
        tokio::time::timeout(Duration::from_secs(2), callback.opened.notified())
            .await
            .unwrap();
        call.abort();
        assert!(matches!(call.await, Err(error) if error.is_cancelled()));
        callback.open_gate.add_permits(1);
        assert!(matches!(
            attachment.drain_media(Duration::from_secs(2)).await,
            media_runtime::MediaAttachmentRevocation::Unknown { .. }
        ));
        assert_eq!(attachment.phase(), MediaAttachmentPhase::Unknown);
        assert_eq!(
            attachment.operation_cleanup(&context),
            MediaOperationCleanup::Unknown
        );
        assert_eq!(
            host.cancel(context).await.unwrap_err().kind,
            MediaErrorKind::Busy
        );
        assert_eq!(host.media_operation_routes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn confirmed_open_failures_do_not_exhaust_operation_routes() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let (host, attachment, callback) = native_open_host();
        callback.fail_open.store(true, Ordering::Release);
        callback
            .confirmed_open_failure
            .store(true, Ordering::Release);
        for generation in 4000..4513 {
            let (context, operation) = native_start(generation);
            assert_eq!(
                host.execute(context, operation).await.unwrap_err().kind,
                MediaErrorKind::NativeFailure
            );
        }
        assert!(host.media_operation_routes.lock().unwrap().is_empty());
        assert_eq!(
            attachment.drain_media(Duration::from_secs(2)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    #[tokio::test]
    async fn live_session_is_stopped_before_waiting_for_native_witness() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let (host, attachment, callback) = native_open_host();
        callback.open_gate.add_permits(1);
        let (context, operation) = native_start(3003);
        assert!(matches!(
            host.execute(context, operation).await.unwrap(),
            MediaResult::SessionStarted { .. }
        ));
        assert_eq!(
            attachment.drain_media(Duration::from_secs(2)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FacadeNativeState {
        Held,
        Stopped,
        NoResource,
        Unknown,
    }

    struct FacadeNativeOperation {
        owner: MediaOwner,
        state: FacadeNativeState,
    }

    #[derive(Default)]
    struct FacadeNativeAudio {
        // 0 = started, 1 = confirmed no resource, 2 = unknown, 3 = pending,
        // 4 = hold the native service's first poll; 5 = hold dropping its
        // pending start after cancellation on another runtime thread.
        mode: AtomicUsize,
        fail_cancel: AtomicBool,
        fail_end_owner: AtomicBool,
        operations: Mutex<HashMap<OperationRouteKey, FacadeNativeOperation>>,
        confirmed: Mutex<HashSet<OperationRouteKey>>,
        cancellations: Mutex<Vec<AudioOperationContext>>,
        entered: tokio::sync::Notify,
        cancel_entered: tokio::sync::Notify,
        cancel_confirmed: tokio::sync::Notify,
        drop_entered: tokio::sync::Notify,
        cancel_gate: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
        stop_entered: tokio::sync::Notify,
        stop_gate: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
        end_entered: tokio::sync::Notify,
        end_gate: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
        first_poll_gate: (Mutex<bool>, std::sync::Condvar),
    }

    struct HoldNativeStartDrop<'a>(&'a FacadeNativeAudio);

    impl Drop for HoldNativeStartDrop<'_> {
        fn drop(&mut self) {
            self.0.drop_entered.notify_one();
            let (lock, released) = &self.0.first_poll_gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = released.wait(open).unwrap();
            }
        }
    }

    impl FacadeNativeAudio {
        fn handle() -> media_api::RecordingRef {
            media_api::RecordingRef(media_api::RecordingId::from("shared-handle"))
        }

        fn has_held(&self, context: &AudioOperationContext) -> bool {
            self.operations
                .lock()
                .unwrap()
                .iter()
                .any(|(key, operation)| {
                    key.incarnation == context.operation_incarnation()
                        && matches!(
                            operation.state,
                            FacadeNativeState::Held | FacadeNativeState::Unknown
                        )
                })
        }
    }

    #[async_trait]
    impl AudioService for FacadeNativeAudio {
        fn capabilities(&self) -> AudioCapabilitySnapshot {
            AudioCapabilitySnapshot {
                service_epoch: 7,
                support_revision: 1,
                supported_operations: vec![AudioOperationKind::Record],
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }

        async fn execute(
            &self,
            context: AudioOperationContext,
            operation: AudioOperation,
        ) -> Result<AudioOperationSuccess, AudioError> {
            match operation {
                AudioOperation::StartRecording { .. } => {
                    let mode = self.mode.load(Ordering::SeqCst);
                    let state = match mode {
                        1 => FacadeNativeState::NoResource,
                        2 => FacadeNativeState::Unknown,
                        _ => FacadeNativeState::Held,
                    };
                    self.operations.lock().unwrap().insert(
                        OperationRouteKey {
                            identity: context.identity.clone(),
                            incarnation: context.operation_incarnation(),
                        },
                        FacadeNativeOperation {
                            owner: context.owner,
                            state,
                        },
                    );
                    if mode == 3 {
                        self.entered.notify_one();
                        std::future::pending::<()>().await;
                    }
                    if mode == 4 {
                        self.entered.notify_one();
                        let (lock, released) = &self.first_poll_gate;
                        let mut open = lock.lock().unwrap();
                        while !*open {
                            open = released.wait(open).unwrap();
                        }
                    }
                    if mode == 5 {
                        let hold_drop = HoldNativeStartDrop(self);
                        self.entered.notify_one();
                        std::future::pending::<()>().await;
                        drop(hold_drop);
                    }
                    if mode == 1 || mode == 2 {
                        Err(AudioError::new(
                            AudioErrorKind::NativeFailure,
                            "native start failed",
                        ))
                    } else {
                        Ok(AudioOperationSuccess::RecordingStarted {
                            handle: Self::handle(),
                        })
                    }
                }
                AudioOperation::StopRecording { handle } => {
                    {
                        let mut operations = self.operations.lock().unwrap();
                        let Some(operation) = operations.values_mut().find(|operation| {
                            operation.owner == context.owner
                                && operation.state == FacadeNativeState::Held
                                && handle == Self::handle()
                        }) else {
                            return Err(AudioError::new(
                                AudioErrorKind::NotRecording,
                                "not recording",
                            ));
                        };
                        operation.state = FacadeNativeState::Stopped;
                    }
                    let gate = self.stop_gate.lock().unwrap().clone();
                    if let Some(gate) = gate {
                        self.stop_entered.notify_one();
                        gate.acquire().await.unwrap().forget();
                    }
                    Ok(AudioOperationSuccess::Recording {
                        recording: media_api::voice::VoiceRecording {
                            audio_bytes: vec![1, 2],
                            mime_type: "audio/wav".into(),
                        },
                    })
                }
                AudioOperation::Status { .. } => {
                    let recording = self.operations.lock().unwrap().values().any(|operation| {
                        operation.owner == context.owner
                            && matches!(
                                operation.state,
                                FacadeNativeState::Held | FacadeNativeState::Unknown
                            )
                    });
                    Ok(AudioOperationSuccess::Status {
                        status: media_api::audio::AudioStatus {
                            recording,
                            playing: false,
                        },
                    })
                }
                AudioOperation::EndOwner => {
                    if self.fail_end_owner.load(Ordering::SeqCst) {
                        return Err(AudioError::new(
                            AudioErrorKind::NativeFailure,
                            "native owner teardown failed",
                        ));
                    }
                    let gate = self.end_gate.lock().unwrap().clone();
                    if let Some(gate) = gate {
                        self.end_entered.notify_one();
                        gate.acquire().await.unwrap().forget();
                    }
                    self.operations
                        .lock()
                        .unwrap()
                        .retain(|_, operation| operation.owner != context.owner);
                    Ok(AudioOperationSuccess::OwnerEnded)
                }
                _ => Err(AudioError::new(
                    AudioErrorKind::Unsupported,
                    "unused fixture operation",
                )),
            }
        }

        async fn cancel(&self, context: AudioOperationContext) -> Result<(), AudioError> {
            self.cancellations.lock().unwrap().push(context.clone());
            let gate = self.cancel_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                self.cancel_entered.notify_one();
                gate.acquire().await.unwrap().forget();
            }
            if self.fail_cancel.load(Ordering::SeqCst) {
                return Err(AudioError::new(
                    AudioErrorKind::NativeFailure,
                    "no close receipt",
                ));
            }
            let key = OperationRouteKey {
                identity: context.identity.clone(),
                incarnation: context.operation_incarnation(),
            };
            let result = if self.operations.lock().unwrap().remove(&key).is_some() {
                self.confirmed.lock().unwrap().insert(key);
                Ok(())
            } else if self.confirmed.lock().unwrap().contains(&key) {
                Ok(())
            } else {
                Err(AudioError::new(
                    AudioErrorKind::NativeFailure,
                    "no correlated native receipt",
                ))
            };
            if result.is_ok() {
                self.cancel_confirmed.notify_one();
            }
            result
        }
    }

    fn desktop_audio_host(
        native: Arc<FacadeNativeAudio>,
    ) -> (
        Arc<AppMediaHost>,
        MediaAttachmentHandle,
        Arc<dyn AudioService>,
    ) {
        let host = AppMediaHost::new(
            Arc::new(super::super::media_store::BoundedMediaResourceStore::default()),
            native,
            None,
            Arc::new(UnopenedDevicePorts),
            Vec::new(),
            Arc::new(super::super::media_cloud::AudioProviderCloudRegistry::new(
                "wav",
            )),
            Vec::new(),
            AudioRoutingSettings::default(),
            1,
            1024,
        )
        .unwrap();
        let attachment = host.create_media_attachment().unwrap();
        host.set_current_media_attachment(Some(attachment.clone()))
            .unwrap();
        let facade = host.desktop_audio_service();
        (host, attachment, facade)
    }

    fn desktop_audio_context(
        service: &dyn AudioService,
        generation: u64,
        owner: MediaOwner,
    ) -> AudioOperationContext {
        AudioOperationContext::new(
            AudioOperationId::new(generation, service.capabilities().service_epoch),
            owner,
        )
    }

    fn desktop_audio_route_exists(context: &AudioOperationContext) -> bool {
        process_audio_attachments()
            .lock()
            .unwrap()
            .operations
            .contains_key(&OperationRouteKey {
                identity: context.identity.clone(),
                incarnation: context.operation_incarnation(),
            })
    }

    fn start_recording() -> AudioOperation {
        AudioOperation::StartRecording {
            sample_rate_hz: 24_000,
            format: "wav".into(),
        }
    }

    async fn finish_desktop_audio_test(attachment: &MediaAttachmentHandle) {
        assert_eq!(
            attachment.revoke(Duration::from_secs(3)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    #[tokio::test]
    async fn desktop_recording_cancel_requires_the_original_native_receipt() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner = MediaOwner::Session {
            session_id: "facade-recording".into(),
        };
        let context = desktop_audio_context(facade.as_ref(), 5001, owner);
        assert!(matches!(
            facade
                .execute(context.clone(), start_recording())
                .await
                .unwrap(),
            AudioOperationSuccess::RecordingStarted { .. }
        ));
        assert!(native.has_held(&context));
        assert!(desktop_audio_route_exists(&context));

        native.fail_cancel.store(true, Ordering::SeqCst);
        assert_eq!(
            facade.cancel(context.clone()).await.unwrap_err().kind,
            AudioErrorKind::NativeFailure
        );
        assert!(
            native.has_held(&context),
            "an unconfirmed native stop must stay live"
        );
        assert!(desktop_audio_route_exists(&context));
        native.fail_cancel.store(false, Ordering::SeqCst);
        facade.cancel(context.clone()).await.unwrap();
        assert!(!native.has_held(&context));
        assert!(!desktop_audio_route_exists(&context));
        let cancelled = native.cancellations.lock().unwrap();
        assert!(cancelled
            .iter()
            .all(|observed| observed.operation_incarnation() == context.operation_incarnation()));
        assert!(cancelled.iter().all(|observed| observed.owner
            == context.owner.clone().in_attachment(attachment.id().clone())));
        drop(cancelled);
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test]
    async fn desktop_confirmed_no_resource_failures_do_not_exhaust_routes() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        native.mode.store(1, Ordering::SeqCst);
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner = MediaOwner::Session {
            session_id: "facade-confirmed-failure".into(),
        };
        for generation in 6000..6513 {
            let context = desktop_audio_context(facade.as_ref(), generation, owner.clone());
            assert_eq!(
                facade
                    .execute(context.clone(), start_recording())
                    .await
                    .unwrap_err()
                    .kind,
                AudioErrorKind::NativeFailure
            );
            assert!(
                !desktop_audio_route_exists(&context),
                "a confirmed failure must free its exact route"
            );
        }
        assert!(native.operations.lock().unwrap().is_empty());
        assert!(native.cancellations.lock().unwrap().len() >= 513);
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test]
    async fn desktop_unknown_failure_keeps_its_route_until_exact_retry() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        native.mode.store(2, Ordering::SeqCst);
        native.fail_cancel.store(true, Ordering::SeqCst);
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner = MediaOwner::Session {
            session_id: "facade-unknown".into(),
        };
        let context = desktop_audio_context(facade.as_ref(), 7000, owner);
        assert_eq!(
            facade
                .execute(context.clone(), start_recording())
                .await
                .unwrap_err()
                .kind,
            AudioErrorKind::NativeFailure
        );
        assert!(desktop_audio_route_exists(&context));
        assert!(native.has_held(&context));
        assert!(facade.cancel(context.clone()).await.is_err());
        assert!(desktop_audio_route_exists(&context));
        native.fail_cancel.store(false, Ordering::SeqCst);
        facade.cancel(context.clone()).await.unwrap();
        assert!(!desktop_audio_route_exists(&context));
        assert!(!native.has_held(&context));
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test]
    async fn desktop_late_failure_receipt_retires_route_after_observer_timeout() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        native.mode.store(1, Ordering::SeqCst);
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        *native.cancel_gate.lock().unwrap() = Some(gate.clone());
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner = MediaOwner::Session {
            session_id: "facade-late-receipt".into(),
        };
        let context =
            desktop_audio_context(facade.as_ref(), 7500, owner).with_timeout_budget_ms(50);
        let task = tokio::spawn({
            let facade = facade.clone();
            let context = context.clone();
            async move { facade.execute(context, start_recording()).await }
        });
        tokio::time::timeout(Duration::from_secs(2), native.cancel_entered.notified())
            .await
            .unwrap();
        assert_eq!(
            task.await.unwrap().unwrap_err().kind,
            AudioErrorKind::NativeFailure
        );
        assert!(desktop_audio_route_exists(&context));
        *native.cancel_gate.lock().unwrap() = None;
        gate.add_permits(1);
        tokio::time::timeout(Duration::from_secs(2), async {
            while desktop_audio_route_exists(&context) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the late receipt must retire the same route");
        assert!(native.operations.lock().unwrap().is_empty());
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test]
    async fn desktop_dropped_start_retires_only_after_native_cancel() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        native.mode.store(3, Ordering::SeqCst);
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner = MediaOwner::Session {
            session_id: "facade-dropped".into(),
        };
        let context = desktop_audio_context(facade.as_ref(), 8000, owner);
        let task = tokio::spawn({
            let facade = facade.clone();
            let context = context.clone();
            async move { facade.execute(context, start_recording()).await }
        });
        tokio::time::timeout(Duration::from_secs(2), native.entered.notified())
            .await
            .unwrap();
        assert!(desktop_audio_route_exists(&context));
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(3), async {
            while desktop_audio_route_exists(&context) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the detached exact cancellation must retire its route");
        assert!(!native.has_held(&context));
        assert!(native
            .cancellations
            .lock()
            .unwrap()
            .iter()
            .any(|observed| observed.operation_incarnation() == context.operation_incarnation()));
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn desktop_cancel_before_first_poll_cannot_claim_a_close_receipt() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        native.mode.store(4, Ordering::SeqCst);
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner = MediaOwner::Session {
            session_id: "facade-first-poll".into(),
        };
        let context = desktop_audio_context(facade.as_ref(), 8500, owner);
        let task = tokio::spawn({
            let facade = facade.clone();
            let context = context.clone();
            async move { facade.execute(context, start_recording()).await }
        });
        tokio::time::timeout(Duration::from_secs(2), native.entered.notified())
            .await
            .unwrap();
        let cancel =
            tokio::time::timeout(Duration::from_secs(1), facade.cancel(context.clone())).await;
        let cancelled_before_poll = native.cancellations.lock().unwrap().len();
        {
            let (lock, released) = &native.first_poll_gate;
            *lock.lock().unwrap() = true;
            released.notify_all();
        }
        assert_eq!(cancel.unwrap().unwrap_err().kind, AudioErrorKind::Busy);
        assert_eq!(cancelled_before_poll, 0);
        assert!(matches!(
            task.await.unwrap().unwrap(),
            AudioOperationSuccess::RecordingStarted { .. }
        ));
        facade.cancel(context.clone()).await.unwrap();
        assert!(!native.has_held(&context));
        assert!(!desktop_audio_route_exists(&context));
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn desktop_cancel_waits_until_the_start_future_has_really_settled() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        native.mode.store(5, Ordering::SeqCst);
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner = MediaOwner::Session {
            session_id: "facade-cancel-before-settlement".into(),
        };
        let context = desktop_audio_context(facade.as_ref(), 8600, owner);
        let start = tokio::spawn({
            let facade = facade.clone();
            let context = context.clone();
            async move { facade.execute(context, start_recording()).await }
        });
        tokio::time::timeout(Duration::from_secs(2), native.entered.notified())
            .await
            .unwrap();
        let mut cancel = tokio::spawn({
            let facade = facade.clone();
            let context = context.clone();
            async move { facade.cancel(context).await }
        });
        let drop_entered =
            tokio::time::timeout(Duration::from_secs(2), native.drop_entered.notified()).await;
        let receipt =
            tokio::time::timeout(Duration::from_secs(2), native.cancel_confirmed.notified()).await;
        let cancel_waited = tokio::time::timeout(Duration::from_millis(50), &mut cancel)
            .await
            .is_err();
        let retained = desktop_audio_route_exists(&context);
        {
            let (lock, released) = &native.first_poll_gate;
            *lock.lock().unwrap() = true;
            released.notify_all();
        }
        assert!(drop_entered.is_ok() && receipt.is_ok());
        assert!(
            cancel_waited,
            "the earlier native receipt cannot finish an unsettled start"
        );
        assert!(retained);
        assert_eq!(
            start.await.unwrap().unwrap_err().kind,
            AudioErrorKind::Cancelled
        );
        cancel.await.unwrap().unwrap();
        assert!(!desktop_audio_route_exists(&context));
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test]
    async fn desktop_confirmed_stops_do_not_exhaust_recording_origins() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner = MediaOwner::Session {
            session_id: "facade-repeated-stop".into(),
        };
        for generation in 10_000..10_513 {
            let start = desktop_audio_context(facade.as_ref(), generation, owner.clone());
            let AudioOperationSuccess::RecordingStarted { handle } = facade
                .execute(start.clone(), start_recording())
                .await
                .unwrap()
            else {
                panic!("start must return a recording");
            };
            let stop = desktop_audio_context(facade.as_ref(), generation + 10_000, owner.clone());
            assert!(matches!(
                facade
                    .execute(stop, AudioOperation::StopRecording { handle })
                    .await
                    .unwrap(),
                AudioOperationSuccess::Recording { .. }
            ));
            assert!(!desktop_audio_route_exists(&start));
            assert!(!native.has_held(&start));
        }
        assert!(native.operations.lock().unwrap().is_empty());
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test]
    async fn desktop_confirmed_owner_ends_do_not_exhaust_recording_origins() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner = MediaOwner::Session {
            session_id: "facade-repeated-owner-end".into(),
        };
        for generation in 40_000..40_513 {
            let start = desktop_audio_context(facade.as_ref(), generation, owner.clone());
            assert!(matches!(
                facade
                    .execute(start.clone(), start_recording())
                    .await
                    .unwrap(),
                AudioOperationSuccess::RecordingStarted { .. }
            ));
            let end = desktop_audio_context(facade.as_ref(), generation + 10_000, owner.clone());
            assert!(matches!(
                facade.execute(end, AudioOperation::EndOwner).await.unwrap(),
                AudioOperationSuccess::OwnerEnded
            ));
            assert!(!desktop_audio_route_exists(&start));
            assert!(!native.has_held(&start));
        }
        assert!(native.operations.lock().unwrap().is_empty());
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test]
    async fn desktop_owner_end_fences_fresh_starts_and_reclaims_pending_origin() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        native.mode.store(3, Ordering::SeqCst);
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        *native.end_gate.lock().unwrap() = Some(gate.clone());
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner = MediaOwner::Session {
            session_id: "facade-end-pending".into(),
        };
        let pending_context = desktop_audio_context(facade.as_ref(), 60_000, owner.clone());
        let pending = tokio::spawn({
            let facade = facade.clone();
            let context = pending_context.clone();
            async move { facade.execute(context, start_recording()).await }
        });
        tokio::time::timeout(Duration::from_secs(2), native.entered.notified())
            .await
            .unwrap();
        let end_context = desktop_audio_context(facade.as_ref(), 60_001, owner.clone());
        let ending = tokio::spawn({
            let facade = facade.clone();
            async move { facade.execute(end_context, AudioOperation::EndOwner).await }
        });
        tokio::time::timeout(Duration::from_secs(2), native.end_entered.notified())
            .await
            .unwrap();
        assert_eq!(
            pending.await.unwrap().unwrap_err().kind,
            AudioErrorKind::Cancelled
        );
        assert!(
            desktop_audio_route_exists(&pending_context),
            "initial exact cancel met the active EndOwner fence"
        );
        let rejected = desktop_audio_context(facade.as_ref(), 60_002, owner.clone());
        assert_eq!(
            facade
                .execute(rejected.clone(), start_recording())
                .await
                .unwrap_err()
                .kind,
            AudioErrorKind::Busy
        );
        assert!(!desktop_audio_route_exists(&rejected));

        native.mode.store(0, Ordering::SeqCst);
        let other_owner = MediaOwner::Session {
            session_id: "facade-end-other".into(),
        };
        let other = desktop_audio_context(facade.as_ref(), 60_003, other_owner);
        assert!(matches!(
            facade
                .execute(other.clone(), start_recording())
                .await
                .unwrap(),
            AudioOperationSuccess::RecordingStarted { .. }
        ));
        *native.end_gate.lock().unwrap() = None;
        gate.add_permits(1);
        assert!(matches!(
            ending.await.unwrap().unwrap(),
            AudioOperationSuccess::OwnerEnded
        ));
        assert!(!desktop_audio_route_exists(&pending_context));
        assert!(desktop_audio_route_exists(&other));
        assert!(native.has_held(&other));

        let fresh = desktop_audio_context(facade.as_ref(), 60_004, owner);
        assert!(matches!(
            facade
                .execute(fresh.clone(), start_recording())
                .await
                .unwrap(),
            AudioOperationSuccess::RecordingStarted { .. }
        ));
        facade.cancel(fresh.clone()).await.unwrap();
        facade.cancel(other.clone()).await.unwrap();
        assert!(!desktop_audio_route_exists(&fresh));
        assert!(!desktop_audio_route_exists(&other));
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test]
    async fn desktop_failed_owner_end_keeps_the_fence_and_can_be_retried() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner = MediaOwner::Session {
            session_id: "facade-end-retry".into(),
        };
        let already_admitted = desktop_audio_context(facade.as_ref(), 61_000, owner.clone());
        assert!(already_admitted.claim_operation_admission());
        assert_eq!(
            facade
                .execute(already_admitted, AudioOperation::EndOwner)
                .await
                .unwrap_err()
                .kind,
            AudioErrorKind::InvalidRequest
        );
        let rejected = desktop_audio_context(facade.as_ref(), 61_001, owner.clone());
        assert_eq!(
            facade
                .execute(rejected, start_recording())
                .await
                .unwrap_err()
                .kind,
            AudioErrorKind::Busy
        );
        let retry = desktop_audio_context(facade.as_ref(), 61_002, owner.clone());
        assert!(matches!(
            facade
                .execute(retry, AudioOperation::EndOwner)
                .await
                .unwrap(),
            AudioOperationSuccess::OwnerEnded
        ));
        let fresh = desktop_audio_context(facade.as_ref(), 61_003, owner);
        assert!(matches!(
            facade
                .execute(fresh.clone(), start_recording())
                .await
                .unwrap(),
            AudioOperationSuccess::RecordingStarted { .. }
        ));
        facade.cancel(fresh).await.unwrap();
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test]
    async fn desktop_unconfirmed_owner_fences_are_bounded_and_retryable() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        let (_host, attachment, facade) = desktop_audio_host(native);
        for index in 0..512 {
            let owner = MediaOwner::Session {
                session_id: format!("facade-bounded-{index}"),
            };
            let context = desktop_audio_context(facade.as_ref(), 70_000 + index, owner);
            assert!(context.claim_operation_admission());
            assert_eq!(
                facade
                    .execute(context, AudioOperation::EndOwner)
                    .await
                    .unwrap_err()
                    .kind,
                AudioErrorKind::InvalidRequest
            );
        }
        let overflow_owner = MediaOwner::Session {
            session_id: "facade-bounded-overflow".into(),
        };
        let overflow = desktop_audio_context(facade.as_ref(), 80_000, overflow_owner.clone());
        assert_eq!(
            facade
                .execute(overflow, AudioOperation::EndOwner)
                .await
                .unwrap_err()
                .kind,
            AudioErrorKind::Busy
        );

        for index in 0..512 {
            let owner = MediaOwner::Session {
                session_id: format!("facade-bounded-{index}"),
            };
            let retry = desktop_audio_context(facade.as_ref(), 90_000 + index, owner);
            assert!(matches!(
                facade
                    .execute(retry, AudioOperation::EndOwner)
                    .await
                    .unwrap(),
                AudioOperationSuccess::OwnerEnded
            ));
        }
        let retry_overflow = desktop_audio_context(facade.as_ref(), 100_000, overflow_owner);
        assert!(matches!(
            facade
                .execute(retry_overflow, AudioOperation::EndOwner)
                .await
                .unwrap(),
            AudioOperationSuccess::OwnerEnded
        ));
        assert!(process_audio_attachments()
            .lock()
            .unwrap()
            .ending_owners
            .is_empty());
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test]
    async fn desktop_unknown_owner_end_retries_do_not_exhaust_routes() {
        // A genuinely unknown owner cannot be reset in-process without
        // inventing a receipt. Isolate this test in its own test process.
        const CHILD: &str = "MEDIA_AUDIO_UNKNOWN_OWNER_RETRY_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("media_host::tests::desktop_unknown_owner_end_retries_do_not_exhaust_routes")
                .arg("--nocapture")
                .env(CHILD, "1")
                .output()
                .expect("run the isolated unknown-owner test");
            assert!(
                output.status.success(),
                "isolated unknown-owner test failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        native.fail_end_owner.store(true, Ordering::SeqCst);
        let (_host, _attachment, facade) = desktop_audio_host(native);
        let owner = MediaOwner::Session {
            session_id: "facade-unknown-owner-end".into(),
        };
        let original = desktop_audio_context(facade.as_ref(), 110_000, owner.clone());
        assert_eq!(
            facade
                .execute(original.clone(), AudioOperation::EndOwner)
                .await
                .unwrap_err()
                .kind,
            AudioErrorKind::NativeFailure
        );
        assert!(desktop_audio_route_exists(&original));
        for generation in 110_001..110_514 {
            let retry = desktop_audio_context(facade.as_ref(), generation, owner.clone());
            assert_eq!(
                facade
                    .execute(retry.clone(), AudioOperation::EndOwner)
                    .await
                    .unwrap_err()
                    .kind,
                AudioErrorKind::Unavailable
            );
            assert!(!desktop_audio_route_exists(&retry));
        }
        assert!(desktop_audio_route_exists(&original));
        let other = MediaOwner::Session {
            session_id: "facade-unaffected-owner".into(),
        };
        let status = desktop_audio_context(facade.as_ref(), 120_000, other);
        assert!(matches!(
            facade
                .execute(status, AudioOperation::Status { handle: None })
                .await
                .unwrap(),
            AudioOperationSuccess::Status { .. }
        ));
        // The isolated process ends with the Unknown route and fence intact.
    }

    #[tokio::test]
    async fn desktop_stop_and_cancel_never_cross_owners_with_the_same_handle() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner_a = MediaOwner::Session {
            session_id: "facade-owner-a".into(),
        };
        let owner_b = MediaOwner::Session {
            session_id: "facade-owner-b".into(),
        };
        let start_a = desktop_audio_context(facade.as_ref(), 9000, owner_a.clone());
        let start_b = desktop_audio_context(facade.as_ref(), 9001, owner_b.clone());
        let AudioOperationSuccess::RecordingStarted { handle: handle_a } = facade
            .execute(start_a.clone(), start_recording())
            .await
            .unwrap()
        else {
            panic!("start A");
        };
        let AudioOperationSuccess::RecordingStarted { handle: handle_b } = facade
            .execute(start_b.clone(), start_recording())
            .await
            .unwrap()
        else {
            panic!("start B");
        };
        assert_eq!(handle_a, handle_b);
        let wrong_owner = AudioOperationContext::new(start_a.identity.clone(), owner_b.clone())
            .with_operation_handle(start_a.operation_incarnation());
        assert_eq!(
            facade.cancel(wrong_owner).await.unwrap_err().kind,
            AudioErrorKind::InvalidRequest
        );
        let wrong_incarnation =
            AudioOperationContext::new(start_a.identity.clone(), owner_a.clone());
        facade.cancel(wrong_incarnation).await.unwrap();
        assert!(native.has_held(&start_a) && native.has_held(&start_b));

        let stop_a = desktop_audio_context(facade.as_ref(), 9010, owner_a);
        assert!(matches!(
            facade
                .execute(stop_a, AudioOperation::StopRecording { handle: handle_a })
                .await
                .unwrap(),
            AudioOperationSuccess::Recording { .. }
        ));
        assert!(!desktop_audio_route_exists(&start_a));
        assert!(desktop_audio_route_exists(&start_b));
        assert!(!native.has_held(&start_a));
        assert!(native.has_held(&start_b));
        let stop_b = desktop_audio_context(facade.as_ref(), 9011, owner_b);
        assert!(matches!(
            facade
                .execute(stop_b, AudioOperation::StopRecording { handle: handle_b })
                .await
                .unwrap(),
            AudioOperationSuccess::Recording { .. }
        ));
        assert!(!desktop_audio_route_exists(&start_b));
        assert!(!native.has_held(&start_b));
        finish_desktop_audio_test(&attachment).await;
    }

    #[tokio::test]
    async fn desktop_late_stop_does_not_retire_a_new_start_with_the_same_handle() {
        let _serial = AUDIO_ATTACHMENT_TEST_SERIAL.lock().await;
        let native = Arc::new(FacadeNativeAudio::default());
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        *native.stop_gate.lock().unwrap() = Some(gate.clone());
        let (_host, attachment, facade) = desktop_audio_host(native.clone());
        let owner = MediaOwner::Session {
            session_id: "facade-stop-reuse".into(),
        };
        let first = desktop_audio_context(facade.as_ref(), 30_000, owner.clone());
        let AudioOperationSuccess::RecordingStarted { handle } = facade
            .execute(first.clone(), start_recording())
            .await
            .unwrap()
        else {
            panic!("first recording did not start");
        };
        let stop = desktop_audio_context(facade.as_ref(), 30_001, owner.clone());
        let stopping = tokio::spawn({
            let facade = facade.clone();
            let handle = handle.clone();
            async move {
                facade
                    .execute(stop, AudioOperation::StopRecording { handle })
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), native.stop_entered.notified())
            .await
            .unwrap();
        let second = desktop_audio_context(facade.as_ref(), 30_002, owner);
        let AudioOperationSuccess::RecordingStarted { handle: reused } = facade
            .execute(second.clone(), start_recording())
            .await
            .unwrap()
        else {
            panic!("replacement recording did not start");
        };
        assert_eq!(handle, reused);
        *native.stop_gate.lock().unwrap() = None;
        gate.add_permits(1);
        assert!(matches!(
            stopping.await.unwrap().unwrap(),
            AudioOperationSuccess::Recording { .. }
        ));
        assert!(!desktop_audio_route_exists(&first));
        assert!(desktop_audio_route_exists(&second));
        assert!(native.has_held(&second));
        facade.cancel(second.clone()).await.unwrap();
        assert!(!desktop_audio_route_exists(&second));
        assert!(!native.has_held(&second));
        finish_desktop_audio_test(&attachment).await;
    }
}
