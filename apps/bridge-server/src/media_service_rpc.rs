//! Conversion and dispatch for authenticated host-only MediaService RPCs.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use client::protocol::media as dto;
use media_api::{
    MediaAsset, MediaAttachmentId, MediaContent, MediaError, MediaErrorKind, MediaEvent,
    MediaEventBatch, MediaEventCursor, MediaEventKind, MediaInitiator, MediaKind, MediaMetadata,
    MediaOperation, MediaOperationContext, MediaOperationId, MediaOperationIncarnation, MediaOwner,
    MediaRef, MediaResourceRef, MediaResourceState, MediaResourceStatus, MediaResult, MediaService,
    MediaSessionIntent, MediaSessionMode, MediaSessionRef, MediaSessionRequest, MediaSessionState,
    MediaSessionStatus, MediaSessionUpdate, MediaSource, MediaSourceKind, MediaSourceRef,
    MediaSourceUsage, RecordOptions, RecordingRef, TranscriptionInput,
};
use media_runtime::{
    MediaAttachmentCleanupFence, MediaAttachmentHandle, MediaAttachmentHost, MediaAttachmentPhase,
    MediaAttachmentRevocation, MediaOperationCleanup,
};
use tokio::sync::{oneshot, watch, Mutex, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

const MAX_IN_FLIGHT_REQUESTS: usize = 16;
const MAX_RETAINED_REQUEST_IDS: usize = 512;
const MAX_DUPLICATE_WAITERS_PER_REQUEST: usize = 8;
const MAX_CONTROL_REQUESTS: usize = 8;
const DEFAULT_MEDIA_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CANCEL_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(1);

/// Owner-bound service RPC dispatcher. The BridgeConnection supplies the
/// trusted owner selected when it built the Desktop runtime.
pub struct MediaServiceRpc {
    shared_service: Option<Arc<dyn MediaService>>,
    attachments: Arc<std::sync::RwLock<Option<MediaAttachmentHandle>>>,
    attachment_host: Arc<MediaAttachmentHost>,
    app_host: Option<Arc<audio_provider::media_host::AppMediaHost>>,
    owner: RwLock<MediaOwner>,
    owner_gate: RwLock<()>,
    owner_transition_gate: Mutex<()>,
    pending_owner: Mutex<Option<PendingOwnerTransition>>,
    transition_pending: AtomicBool,
    owner_transition: RwLock<CancellationToken>,
    remote_connector: Option<Arc<remote_control_client::host_media::RemoteMediaConnector>>,
    media_connection_channel: Option<Arc<crate::media_bridge::BridgeMediaConnectionChannel>>,
    media_control_channel: Option<Arc<crate::media_bridge::BridgeMediaControlChannel>>,
    registered_routes: Mutex<HashMap<(MediaOwner, String), u64>>,
    cleanup_fences: std::sync::Mutex<HashMap<MediaAttachmentId, MediaAttachmentCleanupFence>>,
    route_cleanup_flights: Mutex<HashMap<RouteCleanupKey, RouteCleanupFlight>>,
    generation: AtomicU64,
    route_generation: AtomicU64,
    active_operations: Arc<ActiveOperationRegistry>,
    closed: AtomicBool,
    closing: std::sync::Mutex<CancellationToken>,
    request_state: Mutex<RequestState>,
    in_flight: Arc<Semaphore>,
    control_in_flight: Arc<Semaphore>,
}

#[cfg(test)]
#[path = "media_service_rpc/bridge_teardown_test.rs"]
mod bridge_teardown_test;

#[derive(Default)]
struct RequestState {
    requests: HashMap<String, RequestEntry>,
    completed_order: VecDeque<String>,
}

struct PendingOwnerTransition {
    target: MediaOwner,
    replacement: Option<MediaAttachmentHandle>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct RouteCleanupKey {
    owner: MediaOwner,
    connection_id: String,
    route_generation: Option<u64>,
    drained_attachment: Option<MediaAttachmentId>,
}

/// Completion retained independently of any waiter. An observer timeout or a
/// settled error never drops/replays a route cleanup that may have side effects.
#[derive(Clone)]
struct RouteCleanupFlight {
    incarnation: Arc<()>,
    completion: watch::Receiver<Option<Result<(), MediaError>>>,
}

impl RouteCleanupFlight {
    async fn wait(&self) -> Result<(), MediaError> {
        let mut completion = self.completion.clone();
        loop {
            if let Some(result) = { completion.borrow().clone() } {
                return result;
            }
            if completion.changed().await.is_err() {
                return Err(MediaError::new(
                    MediaErrorKind::NativeFailure,
                    "remote route cleanup worker stopped without a result",
                ));
            }
        }
    }
}

struct RequestEntry {
    fingerprint: String,
    status: RequestStatus,
}

enum RequestStatus {
    Running(Vec<oneshot::Sender<dto::MediaServiceResponseDto>>),
    Completed(dto::MediaServiceResponseDto),
}

/// One identity registration and its exact host-minted context. Keeping a
/// cancellation in flight in this entry prevents an identity from being reused
/// while an older cancel call could still reach the service.
struct ActiveOperationEntry {
    context: MediaOperationContext,
    service: Arc<dyn MediaService>,
    // Captured with `service`, never re-read from the replaceable owner slot.
    attachment: Option<MediaAttachmentHandle>,
    // Only a local Wait(Operation) is proven to have no remote or native
    // side effect after its execution future has settled.
    allow_cleanup_receipt: bool,
    incarnation: Arc<()>,
    terminal: bool,
    cancel_requested: bool,
    cancel_confirmed: bool,
    confirmed_by_cleanup: bool,
    cancellation: Option<CancellationFlight>,
    #[cfg(test)]
    cleanup_read_test_hook: Option<Arc<dyn Fn(MediaOperationCleanup) + Send + Sync>>,
}

#[derive(Clone)]
struct ActiveOperationLease {
    context: MediaOperationContext,
    service: Arc<dyn MediaService>,
    incarnation: Arc<()>,
}

/// A detached service cancellation with a shareable completion signal. Dropping
/// an observer never drops the service future or releases its operation fence.
#[derive(Clone)]
struct CancellationFlight {
    incarnation: Arc<()>,
    completion: watch::Receiver<Option<Result<(), MediaError>>>,
}

enum CancellationObservation {
    Completed(Result<(), MediaError>),
    TimedOut,
}

impl CancellationFlight {
    fn confirmed() -> Self {
        let (_, completion) = watch::channel(Some(Ok(())));
        Self {
            incarnation: Arc::new(()),
            completion,
        }
    }

    async fn wait(&self) -> Result<(), MediaError> {
        let mut completion = self.completion.clone();
        loop {
            if let Some(result) = { completion.borrow().clone() } {
                return result;
            }
            if completion.changed().await.is_err() {
                return Err(MediaError::new(
                    MediaErrorKind::NativeFailure,
                    "media cancellation worker stopped before reporting a result",
                ));
            }
        }
    }

    async fn observe(&self, deadline: Duration) -> CancellationObservation {
        match tokio::time::timeout(deadline, self.wait()).await {
            Ok(result) => CancellationObservation::Completed(result),
            Err(_) => CancellationObservation::TimedOut,
        }
    }
}

#[derive(Default)]
struct ActiveOperationRegistry {
    entries: Mutex<HashMap<MediaOperationId, ActiveOperationEntry>>,
    // Bounded receipts permit a new request ID to observe cleanup after the
    // original terminal entry is released. No service or attachment is held.
    receipts: std::sync::Mutex<ConfirmedCancellationReceipts>,
    cleanup_observer_running: AtomicBool,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct CancellationReceiptKey {
    identity: MediaOperationId,
    owner: MediaOwner,
    incarnation: MediaOperationIncarnation,
}

#[derive(Default)]
struct ConfirmedCancellationReceipts {
    values: HashMap<CancellationReceiptKey, MediaAttachmentId>,
    order: VecDeque<CancellationReceiptKey>,
}

fn allows_cleanup_receipt(operation: &MediaOperation, unified_app_host: bool) -> bool {
    unified_app_host
        && matches!(
            operation,
            MediaOperation::Wait {
                target: MediaResourceRef::Operation { .. },
                ..
            }
        )
}

impl ConfirmedCancellationReceipts {
    fn insert(&mut self, key: CancellationReceiptKey, attachment: MediaAttachmentId) {
        if self.values.insert(key.clone(), attachment).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > MAX_RETAINED_REQUEST_IDS {
            if let Some(expired) = self.order.pop_front() {
                self.values.remove(&expired);
            }
        }
    }

    fn forget_attachment(&mut self, attachment: &MediaAttachmentId) {
        self.values.retain(|_, id| id != attachment);
        let order = std::mem::take(&mut self.order);
        self.order = order
            .into_iter()
            .filter(|key| self.values.contains_key(key))
            .collect();
    }
}

impl ActiveOperationRegistry {
    async fn register(
        &self,
        context: MediaOperationContext,
        service: Arc<dyn MediaService>,
        attachment: Option<MediaAttachmentHandle>,
        allow_cleanup_receipt: bool,
    ) -> Result<ActiveOperationLease, ()> {
        let identity = context.identity.clone();
        let mut entries = self.entries.lock().await;
        if entries.contains_key(&identity) {
            return Err(());
        }
        let incarnation = Arc::new(());
        let lease = ActiveOperationLease {
            context: context.clone(),
            service: service.clone(),
            incarnation: incarnation.clone(),
        };
        entries.insert(
            identity,
            ActiveOperationEntry {
                context,
                service,
                attachment,
                allow_cleanup_receipt,
                incarnation,
                terminal: false,
                cancel_requested: false,
                cancel_confirmed: false,
                confirmed_by_cleanup: false,
                cancellation: None,
                #[cfg(test)]
                cleanup_read_test_hook: None,
            },
        );
        Ok(lease)
    }

    async fn cancel_identity(
        self: &Arc<Self>,
        identity: &MediaOperationId,
        owner: &MediaOwner,
        incarnation: &MediaOperationIncarnation,
    ) -> Option<CancellationFlight> {
        let mut entries = self.entries.lock().await;
        let entry = entries.get_mut(identity).filter(|entry| {
            &entry.context.owner == owner
                && entry
                    .context
                    .operation_incarnation()
                    .same_instance(incarnation)
        });
        let Some(entry) = entry else {
            let key = CancellationReceiptKey {
                identity: identity.clone(),
                owner: owner.clone(),
                incarnation: *incarnation,
            };
            return self
                .receipts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values
                .contains_key(&key)
                .then(CancellationFlight::confirmed);
        };
        entry.cancel_requested = true;
        entry.promote_completed_busy();
        if entry.cancel_confirmed {
            if entry.terminal {
                self.remember_cleanup_receipt(entry);
                entries.remove(identity);
            }
            return Some(CancellationFlight::confirmed());
        }
        if entry.needs_cleanup_observation() {
            self.start_cleanup_observer();
        }
        self.ensure_cancellation(entry)
    }

    async fn cancel_lease(
        self: &Arc<Self>,
        operation: &ActiveOperationLease,
    ) -> Option<CancellationFlight> {
        let mut entries = self.entries.lock().await;
        let entry = entries.get_mut(&operation.context.identity)?;
        if !Arc::ptr_eq(&entry.incarnation, &operation.incarnation) {
            return None;
        }
        entry.cancel_requested = true;
        entry.promote_completed_busy();
        if entry.cancel_confirmed {
            if entry.terminal {
                self.remember_cleanup_receipt(entry);
                entries.remove(&operation.context.identity);
            }
            return Some(CancellationFlight::confirmed());
        }
        if entry.needs_cleanup_observation() {
            self.start_cleanup_observer();
        }
        self.ensure_cancellation(entry)
    }

    async fn reserve_all_cancels(&self) -> Vec<ActiveOperationLease> {
        let mut entries = self.entries.lock().await;
        entries
            .values_mut()
            .filter_map(|entry| {
                if entry.cancel_confirmed {
                    return None;
                }
                Some(entry.lease())
            })
            .collect()
    }

    /// A Confirmed attachment revoke is stronger evidence than any individual
    /// cancel result. Once the owner gate is drained, discard only operation
    /// fences whose captured service belongs to this exact revoked attachment.
    async fn forget_confirmed_attachment(&self, attachment_id: &MediaAttachmentId) {
        let mut entries = self.entries.lock().await;
        entries.retain(|_, entry| entry.service.attachment_id().as_ref() != Some(attachment_id));
        self.receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .forget_attachment(attachment_id);
    }

    /// Marks an operation terminal. If cancellation was requested, retain the
    /// incarnation fence until the service confirms the shared cancel flight.
    async fn finish(
        self: &Arc<Self>,
        operation: &ActiveOperationLease,
        cancel: bool,
    ) -> Option<CancellationFlight> {
        let identity = &operation.context.identity;
        let mut entries = self.entries.lock().await;
        let Some(entry) = entries.get_mut(identity) else {
            return None;
        };
        if !Arc::ptr_eq(&entry.incarnation, &operation.incarnation) {
            return None;
        }
        entry.terminal = true;
        if cancel {
            entry.cancel_requested = true;
        }
        entry.promote_completed_busy();
        if entry.cancel_confirmed {
            self.remember_cleanup_receipt(entry);
            entries.remove(identity);
            return cancel.then(CancellationFlight::confirmed);
        }
        if entry.needs_cleanup_observation() {
            self.start_cleanup_observer();
        }
        if cancel {
            return self.ensure_cancellation(entry);
        }
        let remove = !entry.cancel_requested || entry.cancel_confirmed;
        if remove {
            entries.remove(identity);
        }
        None
    }

    fn ensure_cancellation(
        self: &Arc<Self>,
        entry: &mut ActiveOperationEntry,
    ) -> Option<CancellationFlight> {
        if let Some(flight) = &entry.cancellation {
            return Some(flight.clone());
        }
        if entry.cancel_confirmed {
            return None;
        }

        let context = entry.context.clone();
        let service = entry.service.clone();
        let operation_incarnation = entry.incarnation.clone();
        let flight_incarnation = Arc::new(());
        let (sender, completion) = watch::channel(None);
        let flight = CancellationFlight {
            incarnation: flight_incarnation.clone(),
            completion,
        };
        entry.cancellation = Some(flight.clone());

        let registry = Arc::downgrade(self);
        let task_context = context.clone();
        let task_service = service.clone();
        let task_operation_incarnation = operation_incarnation.clone();
        let task_flight_incarnation = flight_incarnation.clone();
        tokio::spawn(async move {
            let result = task_service.cancel(task_context.clone()).await;
            // Make the settled result visible before updating the registry:
            // finish() and this callback must both be able to observe Busy.
            sender.send_replace(Some(result.clone()));
            if let Some(registry) = registry.upgrade() {
                registry
                    .cancellation_finished(
                        &task_context,
                        &task_operation_incarnation,
                        &task_flight_incarnation,
                        &result,
                    )
                    .await;
            }
        });
        Some(flight)
    }

    async fn cancellation_finished(
        self: &Arc<Self>,
        context: &MediaOperationContext,
        operation_incarnation: &Arc<()>,
        flight_incarnation: &Arc<()>,
        result: &Result<(), MediaError>,
    ) {
        let identity = &context.identity;
        let mut entries = self.entries.lock().await;
        let Some(entry) = entries.get_mut(identity) else {
            return;
        };
        if !Arc::ptr_eq(&entry.incarnation, operation_incarnation) {
            return;
        }
        let Some(flight) = entry.cancellation.as_ref() else {
            return;
        };
        if !Arc::ptr_eq(&flight.incarnation, flight_incarnation) {
            return;
        }
        if result.is_ok() {
            entry.cancel_confirmed = true;
        } else if result
            .as_ref()
            .is_err_and(|error| error.kind == MediaErrorKind::Busy)
            && entry.cleanup_confirmed()
        {
            entry.cancel_confirmed = true;
            entry.confirmed_by_cleanup = true;
        }
        let remove = entry.terminal && entry.cancel_confirmed;
        if remove {
            self.remember_cleanup_receipt(entry);
            entries.remove(identity);
        } else if entry.needs_cleanup_observation() {
            self.start_cleanup_observer();
        }
    }

    // At most one weak observer per connection. It performs only exact
    // lifecycle reads: it never reissues a native cancel or treats a timeout
    // as confirmation. An Unknown entry is retained but not polled forever.
    fn start_cleanup_observer(self: &Arc<Self>) {
        if self.cleanup_observer_running.swap(true, Ordering::AcqRel) {
            return;
        }
        let registry = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut delay = Duration::from_millis(10);
            loop {
                tokio::time::sleep(delay).await;
                let Some(registry) = registry.upgrade() else {
                    return;
                };
                let mut entries = registry.entries.lock().await;
                let mut still_pending = false;
                entries.retain(|_, entry| {
                    if entry.needs_cleanup_observation() {
                        // Read the exact lifecycle once. If Pending changes to
                        // Confirmed immediately afterward, `still_pending`
                        // keeps this observer alive for the next pass.
                        match entry.cleanup_status() {
                            MediaOperationCleanup::Confirmed => {
                                entry.cancel_confirmed = true;
                                entry.confirmed_by_cleanup = true;
                                registry.remember_cleanup_receipt(entry);
                                return false;
                            }
                            MediaOperationCleanup::Pending => still_pending = true,
                            MediaOperationCleanup::Unknown => {}
                        }
                    } else if entry.terminal && entry.confirmed_by_cleanup {
                        registry.remember_cleanup_receipt(entry);
                        return false;
                    }
                    true
                });
                if !still_pending {
                    // Producers use the same entries lock, so no Pending entry
                    // can be inserted between this check and clearing the flag.
                    registry
                        .cleanup_observer_running
                        .store(false, Ordering::Release);
                    return;
                }
                drop(entries);
                drop(registry);
                delay = (delay * 2).min(Duration::from_secs(1));
            }
        });
    }

    // Call only with the entries lock held, before removing the exact entry.
    fn remember_cleanup_receipt(&self, entry: &ActiveOperationEntry) {
        if !entry.confirmed_by_cleanup {
            return;
        }
        let Some(attachment) = entry.attachment.as_ref() else {
            return;
        };
        let key = CancellationReceiptKey {
            identity: entry.context.identity.clone(),
            owner: entry.context.owner.clone(),
            incarnation: entry.context.operation_incarnation(),
        };
        self.receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, attachment.id().clone());
    }

    #[cfg(test)]
    async fn context(&self, identity: &MediaOperationId) -> Option<MediaOperationContext> {
        self.entries
            .lock()
            .await
            .get(identity)
            .map(|entry| entry.context.clone())
    }
}

impl ActiveOperationEntry {
    fn cleanup_status(&self) -> MediaOperationCleanup {
        if !self.allow_cleanup_receipt {
            return MediaOperationCleanup::Unknown;
        }
        let status = self
            .attachment
            .as_ref()
            .map_or(MediaOperationCleanup::Unknown, |attachment| {
                attachment.operation_cleanup(&self.context)
            });
        #[cfg(test)]
        if self.terminal {
            if let Some(hook) = &self.cleanup_read_test_hook {
                hook(status);
            }
        }
        status
    }

    fn has_completed_busy(&self) -> bool {
        self.cancellation.as_ref().is_some_and(|flight| {
            matches!(&*flight.completion.borrow(), Some(Err(error)) if error.kind == MediaErrorKind::Busy)
        })
    }

    fn needs_cleanup_observation(&self) -> bool {
        self.allow_cleanup_receipt
            && self.terminal
            && !self.cancel_confirmed
            && self.has_completed_busy()
    }

    fn cleanup_confirmed(&self) -> bool {
        self.cleanup_status() == MediaOperationCleanup::Confirmed
    }

    fn promote_completed_busy(&mut self) {
        if self.has_completed_busy() && self.cleanup_confirmed() {
            self.cancel_confirmed = true;
            self.confirmed_by_cleanup = true;
        }
    }

    fn lease(&self) -> ActiveOperationLease {
        ActiveOperationLease {
            context: self.context.clone(),
            service: self.service.clone(),
            incarnation: self.incarnation.clone(),
        }
    }
}

/// Cancels an operation if its dispatch future is dropped before it reaches a
/// terminal result. The guard owns the original trusted operation context and
/// incarnation so a late finish cannot target a later registration that reuses
/// the same identity.
struct ActiveOperationGuard {
    active_operations: Arc<ActiveOperationRegistry>,
    operation: Option<ActiveOperationLease>,
}

impl ActiveOperationGuard {
    fn new(
        active_operations: Arc<ActiveOperationRegistry>,
        operation: ActiveOperationLease,
    ) -> Self {
        Self {
            active_operations,
            operation: Some(operation),
        }
    }

    async fn finish(mut self, cancel: bool) {
        if let Some(operation) = self.operation.take() {
            if let Some(flight) = self.active_operations.finish(&operation, cancel).await {
                report_cancellation_observation(
                    flight.observe(CANCEL_OBSERVATION_TIMEOUT).await,
                    "media operation completion",
                );
            }
        }
    }
}

impl Drop for ActiveOperationGuard {
    fn drop(&mut self) {
        let Some(operation) = self.operation.take() else {
            return;
        };
        let active_operations = self.active_operations.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Some(flight) = active_operations.finish(&operation, true).await {
                    report_cancellation_observation(
                        flight.observe(CANCEL_OBSERVATION_TIMEOUT).await,
                        "dropped media operation",
                    );
                }
            });
        }
    }
}

fn report_cancellation_observation(observation: CancellationObservation, reason: &'static str) {
    match observation {
        CancellationObservation::Completed(Ok(())) => {}
        CancellationObservation::Completed(Err(error)) => {
            tracing::warn!(%error, reason, "media cancellation failed");
        }
        CancellationObservation::TimedOut => {
            tracing::warn!(
                ?CANCEL_OBSERVATION_TIMEOUT,
                reason,
                "media cancellation remains in flight after the observation deadline"
            );
        }
    }
}

impl MediaServiceRpc {
    fn current_attachment(&self) -> Result<MediaAttachmentHandle, MediaError> {
        self.attachments
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "the Bridge connection has no active media attachment",
                )
            })
    }

    fn create_replacement_attachment(&self) -> Result<MediaAttachmentHandle, MediaError> {
        let attachment = if let Some(app_host) = self.app_host.as_ref() {
            app_host.create_media_attachment()?
        } else {
            let service = self.shared_service.clone().ok_or_else(|| {
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "the Bridge media host cannot recreate its local test attachment",
                )
            })?;
            self.attachment_host.attach(service)?
        };
        let fence = attachment.retain_for_host_cleanup()?;
        self.cleanup_fences
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(attachment.id().clone(), fence);
        Ok(attachment)
    }

    fn release_cleanup_fence(&self, attachment: &MediaAttachmentHandle) -> Result<(), MediaError> {
        let mut fences = self
            .cleanup_fences
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match fences.get_mut(attachment.id()) {
            Some(fence) => {
                fence.confirm()?;
                fences.remove(attachment.id());
                Ok(())
            }
            None if attachment.phase() == MediaAttachmentPhase::Revoked => Ok(()),
            None => Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "attachment cleanup fence is missing",
            )),
        }
    }

    async fn finish_attachment_cleanup(
        &self,
        owner: &MediaOwner,
        attachment: &MediaAttachmentHandle,
    ) -> Result<(), MediaError> {
        self.unregister_owner_routes(owner, attachment).await?;
        if let Some(app_host) = self.app_host.as_ref() {
            app_host
                .cloud_audio_registry()
                .unregister_attachment(attachment.id());
            if let Some(connector) = self.remote_connector.as_ref() {
                let remaining = tokio::time::timeout(
                    CANCEL_OBSERVATION_TIMEOUT,
                    connector.supported_modes(owner),
                )
                .await
                .map_err(|_| {
                    MediaError::new(
                        MediaErrorKind::Timeout,
                        "remote media modes remain unconfirmed during cleanup",
                    )
                })?;
                if self
                    .current_attachment()
                    .is_ok_and(|current| current.id() == attachment.id())
                {
                    app_host.update_remote_session_modes(remaining);
                }
            }
        }
        self.release_cleanup_fence(attachment)
    }

    fn clear_closed_attachment(&self, attachment: &MediaAttachmentHandle) {
        if !self.closed.load(Ordering::Acquire)
            || attachment.phase() != MediaAttachmentPhase::Revoked
        {
            return;
        }
        let mut current = self
            .attachments
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current
            .as_ref()
            .is_some_and(|handle| handle.id() == attachment.id())
        {
            *current = None;
        }
    }

    /// Fence admission immediately when the owning transport disconnects.
    pub fn begin_close(&self) {
        self.closed.store(true, Ordering::Release);
        self.transition_pending.store(true, Ordering::Release);
        self.closing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancel();
        if let Ok(token) = self.owner_transition.try_read() {
            token.cancel();
        }
        if let Ok(attachment) = self.current_attachment() {
            attachment.begin_revoke();
        }
    }

    /// Reattach this connection only after the previous exact scope finished.
    /// Returns whether a replacement was installed.
    pub async fn reopen(
        self: &Arc<Self>,
        router: Option<&Arc<dyn crate::router::CommandRouter>>,
    ) -> Result<bool, MediaError> {
        let _serial = self.owner_transition_gate.lock().await;
        if !self.closed.load(Ordering::Acquire) {
            return Ok(false);
        }
        if let Ok(previous) = self.current_attachment() {
            if previous.phase() != MediaAttachmentPhase::Revoked {
                return Err(MediaError::new(
                    MediaErrorKind::Unavailable,
                    "previous media cleanup is unconfirmed",
                ));
            }
            self.clear_closed_attachment(&previous);
        }
        if self.pending_owner.lock().await.is_some() {
            return Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "previous media owner transition is unconfirmed",
            ));
        }
        let replacement = self.create_replacement_attachment()?;
        let owner = self.owner.read().await.clone();
        if let Some(router) = router {
            if let Err(message) = router.prepare_media_attachment(&replacement, &owner).await {
                replacement.begin_revoke();
                let attached_owner = owner.in_attachment(replacement.id().clone());
                if matches!(
                    replacement.drain_media(CANCEL_OBSERVATION_TIMEOUT).await,
                    MediaAttachmentRevocation::Confirmed
                ) {
                    if let Err(error) = self
                        .finish_attachment_cleanup(&attached_owner, &replacement)
                        .await
                    {
                        tracing::warn!(%error, "unpublished reconnect attachment cleanup is unconfirmed");
                    }
                } else {
                    self.watch_late_confirmed_route_cleanup(attached_owner, replacement);
                }
                return Err(MediaError::new(MediaErrorKind::Unavailable, message));
            }
        }
        if let Some(app_host) = self.app_host.as_ref() {
            app_host.set_current_media_attachment(Some(replacement.clone()))?;
        } else {
            *self
                .attachments
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(replacement);
        }
        *self
            .closing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = CancellationToken::new();
        *self.owner_transition.write().await = CancellationToken::new();
        let mut requests = self.request_state.lock().await;
        requests.requests.clear();
        requests.completed_order.clear();
        self.transition_pending.store(false, Ordering::Release);
        self.closed.store(false, Ordering::Release);
        Ok(true)
    }

    fn next_route_generation(&self) -> Option<u64> {
        self.route_generation
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .ok()
            .and_then(|previous| previous.checked_add(1))
    }

    /// Close admission on the currently published scope before the session
    /// driver starts changing owners. This synchronous pre-fence is important:
    /// the later async `revoke()` call must not be the first point where direct
    /// Desktop MediaService callers stop entering the old attachment.
    pub fn begin_owner_transition(&self) -> Result<(), MediaError> {
        self.transition_pending.store(true, Ordering::Release);
        let attachment = self.current_attachment()?;
        attachment.begin_revoke();
        if let Ok(token) = self.owner_transition.try_read() {
            token.cancel();
        }
        Ok(())
    }

    /// Fence new admission and wait for already accepted request handlers
    /// before the session router starts changing the logical owner.
    pub async fn prepare_owner_transition(&self) -> Result<(), MediaError> {
        self.begin_owner_transition()?;
        let _gate = tokio::time::timeout(CANCEL_OBSERVATION_TIMEOUT, self.owner_gate.write())
            .await
            .map_err(|_| {
                MediaError::new(
                    MediaErrorKind::Timeout,
                    "media request handlers remain active before session transition",
                )
            })?;
        Ok(())
    }

    /// Bind an exact app-scoped MediaService to this authenticated desktop
    /// session. The owner never comes from a renderer or RPC request body.
    #[must_use]
    pub fn new(service: Arc<dyn MediaService>, owner: MediaOwner) -> Result<Arc<Self>, MediaError> {
        let attachment_host = Arc::new(MediaAttachmentHost::new());
        let shared_service = Some(service.clone());
        let attachment = attachment_host.attach(service)?;
        let cleanup_fence = attachment.retain_for_host_cleanup()?;
        let cleanup_fences = HashMap::from([(attachment.id().clone(), cleanup_fence)]);
        let attachments = Arc::new(std::sync::RwLock::new(Some(attachment)));
        Ok(Arc::new(Self {
            shared_service,
            attachments,
            attachment_host,
            app_host: None,
            owner: RwLock::new(owner),
            owner_gate: RwLock::new(()),
            owner_transition_gate: Mutex::new(()),
            pending_owner: Mutex::new(None),
            transition_pending: AtomicBool::new(false),
            owner_transition: RwLock::new(CancellationToken::new()),
            remote_connector: None,
            media_connection_channel: None,
            media_control_channel: None,
            registered_routes: Mutex::new(HashMap::new()),
            cleanup_fences: std::sync::Mutex::new(cleanup_fences),
            route_cleanup_flights: Mutex::new(HashMap::new()),
            generation: std::sync::atomic::AtomicU64::new(0),
            route_generation: AtomicU64::new(0),
            active_operations: Arc::new(ActiveOperationRegistry::default()),
            closed: AtomicBool::new(false),
            closing: std::sync::Mutex::new(CancellationToken::new()),
            request_state: Mutex::new(RequestState::default()),
            in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS)),
            control_in_flight: Arc::new(Semaphore::new(MAX_CONTROL_REQUESTS)),
        }))
    }

    /// Bind the exact app-scoped host when settings/route control actions are
    /// available on the authenticated Bridge connection.
    #[must_use]
    pub fn for_app_host(
        app_host: Arc<audio_provider::media_host::AppMediaHost>,
        owner: MediaOwner,
    ) -> Result<Arc<Self>, MediaError> {
        let attachments = app_host.media_attachment_slot();
        let attachment = attachments
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "the app host has no active media attachment",
                )
            })?;
        let cleanup_fence = attachment.retain_for_host_cleanup()?;
        let cleanup_fences = HashMap::from([(attachment.id().clone(), cleanup_fence)]);
        Ok(Arc::new(Self {
            shared_service: None,
            attachments,
            attachment_host: app_host.media_attachment_host(),
            app_host: Some(app_host),
            owner: RwLock::new(owner),
            owner_gate: RwLock::new(()),
            owner_transition_gate: Mutex::new(()),
            pending_owner: Mutex::new(None),
            transition_pending: AtomicBool::new(false),
            owner_transition: RwLock::new(CancellationToken::new()),
            remote_connector: None,
            media_connection_channel: None,
            media_control_channel: None,
            registered_routes: Mutex::new(HashMap::new()),
            cleanup_fences: std::sync::Mutex::new(cleanup_fences),
            route_cleanup_flights: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
            route_generation: AtomicU64::new(0),
            active_operations: Arc::new(ActiveOperationRegistry::default()),
            closed: AtomicBool::new(false),
            closing: std::sync::Mutex::new(CancellationToken::new()),
            request_state: Mutex::new(RequestState::default()),
            in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS)),
            control_in_flight: Arc::new(Semaphore::new(MAX_CONTROL_REQUESTS)),
        }))
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
    ) -> Result<Arc<Self>, MediaError> {
        let attachments = app_host.media_attachment_slot();
        let attachment = attachments
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "the app host has no active media attachment",
                )
            })?;
        let cleanup_fence = attachment.retain_for_host_cleanup()?;
        let cleanup_fences = HashMap::from([(attachment.id().clone(), cleanup_fence)]);
        Ok(Arc::new(Self {
            shared_service: None,
            attachments,
            attachment_host: app_host.media_attachment_host(),
            app_host: Some(app_host),
            owner: RwLock::new(owner),
            owner_gate: RwLock::new(()),
            owner_transition_gate: Mutex::new(()),
            pending_owner: Mutex::new(None),
            transition_pending: AtomicBool::new(false),
            owner_transition: RwLock::new(CancellationToken::new()),
            remote_connector: Some(remote_connector),
            media_connection_channel: Some(media_connection_channel),
            media_control_channel: Some(media_control_channel),
            registered_routes: Mutex::new(HashMap::new()),
            cleanup_fences: std::sync::Mutex::new(cleanup_fences),
            route_cleanup_flights: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
            route_generation: AtomicU64::new(0),
            active_operations: Arc::new(ActiveOperationRegistry::default()),
            closed: AtomicBool::new(false),
            closing: std::sync::Mutex::new(CancellationToken::new()),
            request_state: Mutex::new(RequestState::default()),
            in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS)),
            control_in_flight: Arc::new(Semaphore::new(MAX_CONTROL_REQUESTS)),
        }))
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

    /// Snapshot the exact active session owner for the authenticated context response.
    pub(crate) async fn media_owner_for_session(
        &self,
        session_id: &str,
    ) -> Option<dto::MediaOwnerDto> {
        let _owner_gate = self.owner_gate.read().await;
        if self.closed.load(Ordering::Acquire) || self.transition_pending.load(Ordering::Acquire) {
            return None;
        }
        let owner = self.owner.read().await;
        if !matches!(&*owner, MediaOwner::Session { session_id: current } if current == session_id)
        {
            return None;
        }
        let attachment = self.current_attachment().ok()?;
        if attachment.phase() != MediaAttachmentPhase::Active {
            return None;
        }
        Some(dto::MediaOwnerDto::Attached {
            attachment_host_id: attachment.id().host_id(),
            attachment_generation: attachment.id().generation(),
            logical_kind: dto::MediaOwnerKindDto::Session,
            logical_owner_id: session_id.to_owned(),
        })
    }

    /// Snapshot the exact transport epoch before a request is spawned.
    pub(crate) fn connection_token(&self) -> CancellationToken {
        self.closing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Capture the host-issued attachment before a transport request is spawned.
    /// A task delayed across an owner switch must not acquire the new owner.
    pub(crate) fn request_attachment_epoch(&self) -> Option<MediaAttachmentId> {
        if self.closed.load(Ordering::Acquire) || self.transition_pending.load(Ordering::Acquire) {
            return None;
        }
        let attachment = self.current_attachment().ok()?;
        if attachment.phase() != MediaAttachmentPhase::Active
            || self.closed.load(Ordering::Acquire)
            || self.transition_pending.load(Ordering::Acquire)
        {
            return None;
        }
        Some(attachment.id().clone())
    }

    /// Handle one host-only operation, read or cancellation request.
    pub async fn handle(
        &self,
        request: dto::MediaServiceRequestDto,
    ) -> dto::MediaServiceResponseDto {
        let attachment_epoch = self.request_attachment_epoch();
        self.handle_inner(request, None, attachment_epoch).await
    }

    pub(crate) async fn handle_for_connection(
        &self,
        request: dto::MediaServiceRequestDto,
        connection: CancellationToken,
        attachment_epoch: Option<MediaAttachmentId>,
    ) -> dto::MediaServiceResponseDto {
        self.handle_inner(request, Some(connection), attachment_epoch)
            .await
    }

    async fn handle_inner(
        &self,
        request: dto::MediaServiceRequestDto,
        connection: Option<CancellationToken>,
        attachment_epoch: Option<MediaAttachmentId>,
    ) -> dto::MediaServiceResponseDto {
        if let Err(message) = request.validate() {
            return service_error(
                request.request_id,
                MediaError::new(MediaErrorKind::InvalidRequest, message),
            );
        }
        // Reject closed admission before waiting on a transition's write lock.
        // Otherwise a client waiting for this response may itself hold up the
        // resource stop that the transition is waiting to observe.
        if attachment_epoch.is_none()
            || self.closed.load(Ordering::Acquire)
            || self.transition_pending.load(Ordering::Acquire)
        {
            return service_error(
                request.request_id,
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "the media owner transition is pending; owner-bound requests are closed",
                ),
            );
        }
        // Keep the trusted owner stable across deduplication and the full
        // operation. Session transitions take the write side before changing
        // this owner, so no request can straddle two attached sessions.
        let _owner_gate = self.owner_gate.read().await;
        if connection
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return service_error(
                request.request_id,
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "the originating Bridge connection is closed",
                ),
            );
        }
        if self.transition_pending.load(Ordering::Acquire) {
            return service_error(
                request.request_id,
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "the media owner transition is pending; owner-bound requests are closed",
                ),
            );
        }
        if self.closed.load(Ordering::Acquire)
            || self.current_attachment().ok().is_none_or(|attachment| {
                attachment.phase() != MediaAttachmentPhase::Active
                    || Some(attachment.id()) != attachment_epoch.as_ref()
            })
        {
            return service_error(
                request.request_id,
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "the originating media attachment is no longer active",
                ),
            );
        }
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
            dto::MediaServiceActionDto::Cancel {
                operation_id,
                incarnation,
            } => {
                self.cancel(request_id.clone(), owner, operation_id, incarnation)
                    .await
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
        let attachment = match self.current_attachment() {
            Ok(attachment) => attachment,
            Err(error) => return service_error(request_id, error),
        };
        let permit = match attachment.authorize_owner(&owner) {
            Ok(permit) => permit,
            Err(error) => return service_error(request_id, error),
        };
        let attached_owner = permit.owner().clone();
        let Some(route_generation) = self.next_route_generation() else {
            return service_error(
                request_id,
                MediaError::new(
                    MediaErrorKind::NativeFailure,
                    "media route incarnation generation is exhausted",
                ),
            );
        };
        let channel = crate::media_bridge::BridgeMediaHostControlChannel::for_route(
            connection_id.clone(),
            connection_channel.clone(),
            control_channel.clone(),
        );
        let route_service = attachment.service();
        let registered = connector
            .register_attachment_route(
                attached_owner.clone(),
                connection_id.clone(),
                label,
                modes,
                source_kinds,
                channel,
                route_service,
            )
            .await;
        match registered {
            Ok(()) => {
                self.registered_routes
                    .lock()
                    .await
                    .insert((attached_owner.clone(), connection_id), route_generation);
                if let Err(error) = self.refresh_remote_session_modes(&attached_owner).await {
                    return service_error(request_id, error);
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
        if self.remote_connector.is_none() {
            return service_error(
                request_id,
                MediaError::new(
                    MediaErrorKind::Unavailable,
                    "Remote Control media is not configured on this Bridge connection",
                ),
            );
        }
        let attachment = match self.current_attachment() {
            Ok(attachment) => attachment,
            Err(error) => return service_error(request_id, error),
        };
        let permit = match attachment.authorize_owner(&owner) {
            Ok(permit) => permit,
            Err(error) => return service_error(request_id, error),
        };
        let attached_owner = permit.owner().clone();
        let route_generation = self
            .registered_routes
            .lock()
            .await
            .get(&(attached_owner.clone(), connection_id.clone()))
            .copied();
        if let Err(error) = self
            .unregister_route_bounded(
                attached_owner.clone(),
                connection_id.clone(),
                tokio::time::Instant::now() + CANCEL_OBSERVATION_TIMEOUT,
                route_generation,
                None,
            )
            .await
        {
            return service_error(request_id, error);
        }
        if let Err(error) = self.refresh_remote_session_modes(&attached_owner).await {
            return service_error(request_id, error);
        }
        dto::MediaServiceResponseDto {
            request_id,
            result: Some(dto::MediaServiceResultDto::ConnectionUpdated),
            error: None,
        }
    }

    async fn refresh_remote_session_modes(&self, owner: &MediaOwner) -> Result<(), MediaError> {
        let (Some(app_host), Some(connector)) =
            (self.app_host.as_ref(), self.remote_connector.as_ref())
        else {
            return Ok(());
        };
        let modes =
            tokio::time::timeout(CANCEL_OBSERVATION_TIMEOUT, connector.supported_modes(owner))
                .await
                .map_err(|_| {
                    MediaError::new(
                        MediaErrorKind::Timeout,
                        "remote media mode snapshot did not settle before the deadline",
                    )
                })?;
        app_host.update_remote_session_modes(modes);
        Ok(())
    }

    /// Rotate the trusted logical owner only after the old attachment drain is
    /// confirmed and every old route is removed. Unknown leaves the original
    /// attachment and route records in place and closes all RPC admission.
    pub async fn update_owner(self: &Arc<Self>, owner: MediaOwner) -> Result<(), MediaError> {
        self.update_owner_with_router(owner, None).await
    }

    pub(crate) async fn update_owner_with_router(
        self: &Arc<Self>,
        owner: MediaOwner,
        router: Option<&Arc<dyn crate::router::CommandRouter>>,
    ) -> Result<(), MediaError> {
        let _transition_serial = tokio::time::timeout(
            CANCEL_OBSERVATION_TIMEOUT,
            self.owner_transition_gate.lock(),
        )
        .await
        .map_err(|_| {
            MediaError::new(
                MediaErrorKind::Timeout,
                "another media owner transition has not settled",
            )
        })?;
        if self.closed.load(Ordering::Acquire) {
            return Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "the media host connection is closing",
            ));
        }

        let mut pending = self.pending_owner.lock().await;
        let current_owner = self.owner.read().await.clone();
        match pending.as_ref() {
            Some(transition) if transition.target != owner => {
                return Err(MediaError::new(
                    MediaErrorKind::Busy,
                    "a different media owner transition is awaiting recovery",
                ));
            }
            Some(_) => {}
            None if current_owner == owner && !self.transition_pending.load(Ordering::Acquire) => {
                return Ok(())
            }
            None => {
                *pending = Some(PendingOwnerTransition {
                    target: owner.clone(),
                    replacement: None,
                });
            }
        }
        let previous = self.owner.read().await.clone();
        let old_attachment = self.current_attachment()?;
        self.transition_pending.store(true, Ordering::Release);
        // `begin_revoke` synchronously closes admission. `revoke` below starts
        // and observes the shared drain; timeout/Unknown retains this handle
        // and route set.
        old_attachment.begin_revoke();
        self.owner_transition.read().await.cancel();
        // Admission is already synchronously closed. Join accepted request
        // handlers before taking the media drain snapshot, including handlers
        // that have not yet acquired their attachment registration permit.
        let _owner_gate = tokio::time::timeout(CANCEL_OBSERVATION_TIMEOUT, self.owner_gate.write())
            .await
            .map_err(|_| {
                MediaError::new(
                MediaErrorKind::Timeout,
                "media requests did not leave the old owner gate before the transition deadline",
            )
            })?;
        match old_attachment.drain_media(CANCEL_OBSERVATION_TIMEOUT).await {
            MediaAttachmentRevocation::Confirmed => {}
            MediaAttachmentRevocation::Unknown { reason } => {
                self.watch_late_confirmed_route_cleanup(
                    previous.clone().in_attachment(old_attachment.id().clone()),
                    old_attachment.clone(),
                );
                return Err(MediaError::new(
                    MediaErrorKind::Unavailable,
                    format!("old media attachment remains Unknown: {reason}"),
                ));
            }
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "the media host connection is closing",
            ));
        }
        self.active_operations
            .forget_confirmed_attachment(old_attachment.id())
            .await;

        let old_attached_owner = previous.clone().in_attachment(old_attachment.id().clone());
        self.finish_attachment_cleanup(&old_attached_owner, &old_attachment)
            .await?;
        if old_attachment.phase() != MediaAttachmentPhase::Revoked {
            return Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "another host cleanup is unconfirmed",
            ));
        }

        let pending_transition = pending
            .as_mut()
            .expect("the transition target is retained while the owner gate is closed");
        if pending_transition.replacement.is_none() {
            pending_transition.replacement = Some(self.create_replacement_attachment()?);
        }
        let replacement = pending_transition
            .replacement
            .clone()
            .expect("a replacement attachment was just stored");
        if replacement.phase() != MediaAttachmentPhase::Active {
            return Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "the replacement media attachment is not Active",
            ));
        }
        if let Some(router) = router {
            router
                .prepare_media_attachment(&replacement, &owner)
                .await
                .map_err(|message| MediaError::new(MediaErrorKind::Unavailable, message))?;
        }
        let replacement_owner = owner.clone().in_attachment(replacement.id().clone());
        let new_modes = if let Some(connector) = self.remote_connector.as_ref() {
            tokio::time::timeout(
                CANCEL_OBSERVATION_TIMEOUT,
                connector.supported_modes(&replacement_owner),
            )
            .await
            .map_err(|_| {
                MediaError::new(
                    MediaErrorKind::Timeout,
                    "remote media mode snapshot did not settle before the owner transition deadline",
                )
            })?
        } else {
            Vec::new()
        };

        // Acquire every async commit lock before publishing the new handle.
        // No await follows the slot/owner swap, so readers see either the old
        // revoked scope or the complete new owner+attachment pair.
        let mut owner_state = self.owner.write().await;
        let mut transition_token = self.owner_transition.write().await;
        let mut request_state = self.request_state.lock().await;
        let mut attachment_slot = self
            .attachments
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.closed.load(Ordering::Acquire) {
            return Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "the media host connection is closing",
            ));
        }
        *owner_state = owner;
        *attachment_slot = Some(replacement);
        if let Some(app_host) = self.app_host.as_ref() {
            app_host.update_remote_session_modes(new_modes);
        }
        *transition_token = CancellationToken::new();
        request_state.requests.clear();
        request_state.completed_order.clear();
        *pending = None;
        self.transition_pending.store(false, Ordering::Release);
        Ok(())
    }

    async fn unregister_route_bounded(
        &self,
        owner: MediaOwner,
        connection_id: String,
        deadline: tokio::time::Instant,
        route_generation: Option<u64>,
        drained_attachment: Option<MediaAttachmentHandle>,
    ) -> Result<(), MediaError> {
        let Some(connector) = self.remote_connector.as_ref() else {
            let key = (owner, connection_id);
            let mut routes = self.registered_routes.lock().await;
            if route_generation.is_some_and(|generation| routes.get(&key) == Some(&generation)) {
                routes.remove(&key);
            }
            return Ok(());
        };
        let key = RouteCleanupKey {
            owner: owner.clone(),
            connection_id: connection_id.clone(),
            route_generation,
            drained_attachment: drained_attachment
                .as_ref()
                .map(|attachment| attachment.id().clone()),
        };
        let peer_close_retry_is_safe = drained_attachment.is_some();
        let (flight, incarnation) = {
            let mut flights = self.route_cleanup_flights.lock().await;
            let connector = connector.clone();
            let route_owner = owner.clone();
            let route_id = connection_id.clone();
            let drained_attachment = drained_attachment.clone();
            let flight = flights.entry(key.clone()).or_insert_with(move || {
                let incarnation = Arc::new(());
                let flight_incarnation = incarnation.clone();
                let (sender, completion) = watch::channel(None);
                tokio::spawn(async move {
                    let result = if let Some(attachment) = drained_attachment {
                        connector
                            .unregister_route_after_media_drain(
                                &route_owner,
                                &route_id,
                                &attachment,
                            )
                            .await
                    } else {
                        connector.unregister_route(&route_owner, &route_id).await
                    };
                    sender.send_replace(Some(result));
                });
                RouteCleanupFlight {
                    incarnation: flight_incarnation,
                    completion,
                }
            });
            (flight.clone(), flight.incarnation.clone())
        };
        match tokio::time::timeout_at(deadline, flight.wait()).await {
            Ok(Ok(())) => {
                let mut flights = self.route_cleanup_flights.lock().await;
                if flights
                    .get(&key)
                    .is_some_and(|current| Arc::ptr_eq(&current.incarnation, &incarnation))
                {
                    flights.remove(&key);
                }
                drop(flights);
                let route_key = (owner, connection_id);
                let mut routes = self.registered_routes.lock().await;
                if route_generation
                    .is_some_and(|generation| routes.get(&route_key) == Some(&generation))
                {
                    routes.remove(&route_key);
                }
                Ok(())
            }
            Ok(Err(error)) => {
                // Err is an unknown side-effect result. Keep the completed
                // flight and route key as an incarnation fence. After the
                // attachment media drain is confirmed, the distinct cleanup
                // key may retry only `stop_internal`, whose close-ack and
                // stop-gate contract explicitly supports safe retries.
                if peer_close_retry_is_safe {
                    let mut flights = self.route_cleanup_flights.lock().await;
                    if flights
                        .get(&key)
                        .is_some_and(|current| Arc::ptr_eq(&current.incarnation, &incarnation))
                    {
                        flights.remove(&key);
                    }
                }
                Err(error)
            }
            Err(_) => Err(MediaError::new(
                MediaErrorKind::Timeout,
                format!("remote route {connection_id} remains registered while cleanup runs"),
            )),
        }
    }

    async fn unregister_owner_routes(
        &self,
        owner: &MediaOwner,
        attachment: &MediaAttachmentHandle,
    ) -> Result<(), MediaError> {
        let route_ids = self
            .registered_routes
            .lock()
            .await
            .iter()
            .filter_map(|((route_owner, connection_id), generation)| {
                (route_owner == owner).then(|| (connection_id.clone(), *generation))
            })
            .collect::<Vec<_>>();
        let deadline = tokio::time::Instant::now() + CANCEL_OBSERVATION_TIMEOUT;
        for (connection_id, route_generation) in route_ids {
            self.unregister_route_bounded(
                owner.clone(),
                connection_id,
                deadline,
                Some(route_generation),
                Some(attachment.clone()),
            )
            .await?;
        }
        Ok(())
    }

    /// Keep the exact old route scope alive after a bounded revoke observer
    /// times out. The Harness drain has its own finite budget; a late
    /// confirmation can therefore finish this route cleanup, while stable
    /// `Unknown` drops the watcher without claiming or removing anything.
    fn watch_late_confirmed_route_cleanup(
        self: &Arc<Self>,
        owner: MediaOwner,
        attachment: MediaAttachmentHandle,
    ) {
        match attachment.phase() {
            MediaAttachmentPhase::Revoking | MediaAttachmentPhase::Revoked => {}
            MediaAttachmentPhase::Active | MediaAttachmentPhase::Unknown => return,
        }
        let rpc = self.clone();
        tokio::spawn(async move {
            if !matches!(
                attachment.drain_media(Duration::from_secs(31)).await,
                MediaAttachmentRevocation::Confirmed
            ) {
                return;
            }
            if let Err(error) = rpc.finish_attachment_cleanup(&owner, &attachment).await {
                tracing::warn!(
                    %error,
                    attachment_id = ?attachment.id(),
                    "late-confirmed media attachment left routes unconfirmed"
                );
                return;
            }
            rpc.clear_closed_attachment(&attachment);
        });
    }

    /// Cancel this attached session's in-flight requests when the authenticated
    /// Bridge client disconnects.
    pub async fn close(self: &Arc<Self>) {
        self.begin_close();
        // Serialize close against the final owner/attachment swap. If the
        // transition is still observing cleanup, the old attachment has
        // already been synchronously fenced above and remains unreachable.
        let _transition_serial = match tokio::time::timeout(
            CANCEL_OBSERVATION_TIMEOUT * 3,
            self.owner_transition_gate.lock(),
        )
        .await
        {
            Ok(guard) => guard,
            Err(_) => {
                tracing::warn!(
                    ?CANCEL_OBSERVATION_TIMEOUT,
                    "media owner transition remains in flight during Bridge close"
                );
                return;
            }
        };
        let _owner_gate =
            match tokio::time::timeout(CANCEL_OBSERVATION_TIMEOUT * 3, self.owner_gate.write())
                .await
            {
                Ok(guard) => guard,
                Err(_) => {
                    tracing::warn!("media request handlers remain in flight during Bridge close");
                    return;
                }
            };
        let pending = self.pending_owner.lock().await.take();
        if let Some(pending) = pending {
            if let Some(replacement) = pending.replacement {
                replacement.begin_revoke();
                if matches!(
                    replacement.drain_media(CANCEL_OBSERVATION_TIMEOUT).await,
                    MediaAttachmentRevocation::Confirmed
                ) {
                    let owner = pending.target.in_attachment(replacement.id().clone());
                    if let Err(error) = self.finish_attachment_cleanup(&owner, &replacement).await {
                        tracing::warn!(%error, "unpublished replacement media cleanup is unconfirmed");
                    }
                } else {
                    self.watch_late_confirmed_route_cleanup(
                        pending.target.in_attachment(replacement.id().clone()),
                        replacement,
                    );
                }
            }
        }
        let attachment = match self.current_attachment() {
            Ok(attachment) => attachment,
            Err(_) => return,
        };
        attachment.begin_revoke();
        match attachment.drain_media(CANCEL_OBSERVATION_TIMEOUT).await {
            MediaAttachmentRevocation::Confirmed => {}
            MediaAttachmentRevocation::Unknown { reason } => {
                tracing::warn!(%reason, "media attachment remains Unknown during Bridge close");
                self.watch_late_confirmed_route_cleanup(
                    self.owner
                        .read()
                        .await
                        .clone()
                        .in_attachment(attachment.id().clone()),
                    attachment.clone(),
                );
                return;
            }
        }
        let owner = self.owner.read().await.clone();
        let attached_owner = owner.in_attachment(attachment.id().clone());
        if let Err(error) = self
            .finish_attachment_cleanup(&attached_owner, &attachment)
            .await
        {
            tracing::warn!(%error, "remote routes remain registered during Bridge close");
            return;
        }
        self.active_operations
            .forget_confirmed_attachment(attachment.id())
            .await;
        self.clear_closed_attachment(&attachment);
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
        let attachment = match self.current_attachment() {
            Ok(attachment) => attachment,
            Err(error) => return service_error(request_id, error),
        };
        let service = attachment.service();
        let capabilities = service.capabilities_for(&owner);
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
        let allow_cleanup_receipt = allows_cleanup_receipt(&operation, self.app_host.is_some());
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
        // `owner` is the immutable host snapshot captured by `handle`; the RPC
        // body cannot select this owner or an attachment. Attachment-scoped
        // services apply their host-issued attachment ID around this logical
        // owner, so Bridge keeps the original context for later cancellation.
        let context = MediaOperationContext::new(identity.clone(), owner)
            .with_initiator(MediaInitiator {
                request_id: Some(request_id.clone()),
                ..Default::default()
            })
            .with_max_payload_bytes(payload_limit);
        let context = timeout_budget_ms
            .map(|budget| context.clone().with_timeout_budget_ms(budget))
            .unwrap_or(context);
        let operation_incarnation = context.operation_incarnation();
        let operation_lease = match self
            .active_operations
            .register(
                context.clone(),
                service.clone(),
                Some(attachment),
                allow_cleanup_receipt,
            )
            .await
        {
            Ok(operation) => operation,
            Err(()) => {
                return service_error(
                    request_id,
                    MediaError::new(
                        MediaErrorKind::Busy,
                        "media operation identity is already active for this Bridge connection",
                    ),
                )
            }
        };
        let guard = ActiveOperationGuard::new(self.active_operations.clone(), operation_lease);
        let execute = service.execute(context, operation);
        let timeout = timeout_budget_ms
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_MEDIA_REQUEST_TIMEOUT);
        let closing = self
            .closing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let result = tokio::select! {
            biased;
            _ = owner_transition.cancelled() => Err(MediaError::new(MediaErrorKind::Cancelled, "media owner changed during the operation")),
            _ = closing.cancelled() => Err(MediaError::new(MediaErrorKind::Cancelled, "media host connection closed")),
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
                        incarnation: operation_incarnation.as_string(),
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
        let attachment = match self.current_attachment() {
            Ok(attachment) => attachment,
            Err(error) => return service_error(request_id, error),
        };
        let service = attachment.service();
        let capabilities = service.capabilities_for(&owner);
        let Some(generation) = self.next_generation() else {
            return service_error(
                request_id,
                MediaError::new(
                    MediaErrorKind::NativeFailure,
                    "media operation generation is exhausted",
                ),
            );
        };
        let context = MediaOperationContext::new(
            MediaOperationId::new(generation, capabilities.service_epoch),
            owner,
        )
        .with_initiator(MediaInitiator {
            request_id: Some(request_id.clone()),
            ..Default::default()
        })
        .with_max_payload_bytes(capabilities.max_payload_bytes);
        let operation = match self
            .active_operations
            .register(context.clone(), service.clone(), Some(attachment), false)
            .await
        {
            Ok(operation) => operation,
            Err(()) => {
                return service_error(
                    request_id,
                    MediaError::new(
                        MediaErrorKind::Busy,
                        "media operation identity is already active for this Bridge connection",
                    ),
                )
            }
        };
        let guard = ActiveOperationGuard::new(self.active_operations.clone(), operation);
        let closing = self
            .closing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let result = tokio::select! {
            biased;
            _ = owner_transition.cancelled() => Err(MediaError::new(MediaErrorKind::Cancelled, "media owner changed during the read")),
            _ = closing.cancelled() => Err(MediaError::new(MediaErrorKind::Cancelled, "media host connection closed")),
            result = tokio::time::timeout(DEFAULT_MEDIA_REQUEST_TIMEOUT, service.read_media(context, MediaRef(media_ref))) => match result {
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
        owner: MediaOwner,
        operation_id: dto::MediaOperationIdDto,
        incarnation: String,
    ) -> dto::MediaServiceResponseDto {
        let identity = operation_id_from_dto(operation_id);
        let incarnation = match MediaOperationIncarnation::from_string(&incarnation) {
            Some(incarnation) => incarnation,
            None => {
                return service_error(
                    request_id,
                    MediaError::new(
                        MediaErrorKind::InvalidRequest,
                        "media operation incarnation is invalid",
                    ),
                )
            }
        };
        // Resolve the exact host-scheduled owner and incarnation before
        // awaiting the service so neither field can be retargeted.
        let Some(flight) = self
            .active_operations
            .cancel_identity(&identity, &owner, &incarnation)
            .await
        else {
            return service_error(
                request_id,
                MediaError::new(
                    MediaErrorKind::NotFound,
                    "media operation owner or incarnation is not active for this Bridge connection",
                ),
            );
        };
        match flight.observe(CANCEL_OBSERVATION_TIMEOUT).await {
            CancellationObservation::Completed(Ok(())) => dto::MediaServiceResponseDto {
                request_id,
                result: Some(dto::MediaServiceResultDto::Cancelled),
                error: None,
            },
            CancellationObservation::Completed(Err(error)) => service_error(request_id, error),
            CancellationObservation::TimedOut => service_error(
                request_id,
                MediaError::new(
                    MediaErrorKind::Timeout,
                    "media cancellation is still running; issue a new request to join the existing cancellation",
                ),
            ),
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
        dto::MediaResourceRefDto::Operation {
            operation_id,
            incarnation,
        } => MediaResourceRef::Operation {
            identity: operation_id_from_dto(operation_id),
            incarnation: MediaOperationIncarnation::from_string(&incarnation).ok_or_else(|| {
                MediaError::new(
                    MediaErrorKind::InvalidRequest,
                    "media operation incarnation is invalid",
                )
            })?,
        },
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
        MediaResourceRef::Operation {
            identity,
            incarnation,
        } => dto::MediaResourceRefDto::Operation {
            operation_id: operation_id_to_dto(identity),
            incarnation: incarnation.as_string(),
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
    use remote_control_client::host_media::{
        RemoteMediaConnector, RemoteMediaHostConfigProvider, RemoteMediaHostControlChannel,
    };
    use std::sync::{Mutex as StdMutex, OnceLock};
    use tokio::sync::{watch, Notify};

    // These in-process test operations have no detached worker: once their
    // future returns or is dropped, its native work really has stopped.
    struct TestNativeOperation {
        witness: Option<Box<dyn media_api::MediaNativeWork>>,
        finished: CancellationToken,
    }

    impl Drop for TestNativeOperation {
        fn drop(&mut self) {
            if let Some(witness) = self.witness.take() {
                witness.confirm();
            }
            self.finished.cancel();
        }
    }

    async fn wait_for_test_operation_cleanup(
        attachment: &OnceLock<MediaAttachmentHandle>,
        context: &MediaOperationContext,
    ) -> Result<(), MediaError> {
        let attachment = attachment.get().ok_or_else(|| {
            MediaError::new(MediaErrorKind::Unavailable, "test attachment not installed")
        })?;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match attachment.operation_cleanup(context) {
                    media_runtime::MediaOperationCleanup::Confirmed => return Ok(()),
                    media_runtime::MediaOperationCleanup::Unknown => {
                        return Err(MediaError::new(
                            MediaErrorKind::Unavailable,
                            "test native operation cleanup is unknown",
                        ));
                    }
                    media_runtime::MediaOperationCleanup::Pending => tokio::task::yield_now().await,
                }
            }
        })
        .await
        .map_err(|_| MediaError::new(MediaErrorKind::Timeout, "test operation did not settle"))?
    }

    struct OwnerTrackingService {
        calls: StdMutex<Vec<(MediaOwner, MediaOperation)>>,
        old_owner: MediaOwner,
        old_owner_stopped: AtomicBool,
        stop_started: Notify,
        release_stop: CancellationToken,
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
                    let resources = if context.owner.logical_owner() == &self.old_owner
                        && !self.old_owner_stopped.load(Ordering::Acquire)
                    {
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
                MediaOperation::Stop { target } => {
                    self.stop_started.notify_one();
                    self.release_stop.cancelled().await;
                    self.old_owner_stopped.store(true, Ordering::Release);
                    Ok(MediaResult::Stopped { resource: target })
                }
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

        async fn cancel(&self, _context: MediaOperationContext) -> Result<(), MediaError> {
            Ok(())
        }
    }

    struct BlockingOwnerService {
        execute_started: Notify,
        execute_contexts: StdMutex<Vec<MediaOperationContext>>,
        cancel_contexts: StdMutex<Vec<MediaOperationContext>>,
        cancel_count: watch::Sender<usize>,
        cancel_release: CancellationToken,
        operation_cancelled: CancellationToken,
        operation_finished: CancellationToken,
        attachment: OnceLock<MediaAttachmentHandle>,
    }

    impl BlockingOwnerService {
        fn new() -> Self {
            let (cancel_count, _) = watch::channel(0);
            Self {
                execute_started: Notify::new(),
                execute_contexts: StdMutex::new(Vec::new()),
                cancel_contexts: StdMutex::new(Vec::new()),
                cancel_count,
                cancel_release: CancellationToken::new(),
                operation_cancelled: CancellationToken::new(),
                operation_finished: CancellationToken::new(),
                attachment: OnceLock::new(),
            }
        }

        async fn wait_for_cancels(&self, minimum: usize) {
            let mut count = self.cancel_count.subscribe();
            loop {
                if *count.borrow() >= minimum {
                    return;
                }
                count
                    .changed()
                    .await
                    .expect("the test service keeps its cancellation counter alive");
            }
        }
    }

    #[async_trait]
    impl MediaService for BlockingOwnerService {
        fn capabilities(&self) -> MediaCapabilities {
            MediaCapabilities {
                service_epoch: 7,
                support_revision: 1,
                supported_operations: vec![media_api::MediaOperationKind::Status],
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
            let blocked_request = context
                .initiator
                .as_ref()
                .and_then(|initiator| initiator.request_id.as_deref())
                == Some("blocked-old-owner");
            let work = if blocked_request {
                Some(TestNativeOperation {
                    witness: context.begin_native_work()?,
                    finished: self.operation_finished.clone(),
                })
            } else {
                None
            };
            self.execute_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(context);
            if blocked_request {
                let _work = work;
                self.execute_started.notify_one();
                self.operation_cancelled.cancelled().await;
                return Err(MediaError::new(
                    MediaErrorKind::Cancelled,
                    "the blocked test operation was cancelled",
                ));
            }
            match operation {
                MediaOperation::Status { .. } => Ok(MediaResult::Status {
                    resources: Vec::new(),
                }),
                _ => Err(MediaError::new(
                    MediaErrorKind::Unsupported,
                    "the blocking test service only supports status",
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

        async fn cancel(&self, context: MediaOperationContext) -> Result<(), MediaError> {
            let blocked_request = self
                .execute_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .find(|active| {
                    active.identity == context.identity
                        && active.owner == context.owner
                        && active.operation_incarnation() == context.operation_incarnation()
                        && active
                            .initiator
                            .as_ref()
                            .and_then(|value| value.request_id.as_deref())
                            == Some("blocked-old-owner")
                })
                .cloned();
            let cancel_count = {
                let mut contexts = self
                    .cancel_contexts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                contexts.push(context);
                contexts.len()
            };
            self.cancel_count.send_replace(cancel_count);
            self.cancel_release.cancelled().await;
            if let Some(blocked_request) = blocked_request {
                self.operation_cancelled.cancel();
                self.operation_finished.cancelled().await;
                wait_for_test_operation_cleanup(&self.attachment, &blocked_request).await?;
            }
            Ok(())
        }
    }

    struct FailingCancelService {
        cancel_attempts: std::sync::atomic::AtomicUsize,
        execute_attempts: std::sync::atomic::AtomicUsize,
    }

    struct LateConfirmDrainService {
        block_next_status: AtomicBool,
        status_started: Notify,
        release_status: CancellationToken,
    }

    struct InFlightStatusService {
        active: StdMutex<Option<MediaOperationContext>>,
        execute_started: Notify,
        cancellation: CancellationToken,
        operation_finished: CancellationToken,
        attachment: OnceLock<MediaAttachmentHandle>,
        cancel_contexts: StdMutex<Vec<MediaOperationContext>>,
    }

    impl InFlightStatusService {
        fn active_context(&self) -> Option<MediaOperationContext> {
            self.active
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
        }
    }

    #[async_trait]
    impl MediaService for InFlightStatusService {
        fn capabilities(&self) -> MediaCapabilities {
            MediaCapabilities {
                service_epoch: 7,
                support_revision: 1,
                supported_operations: vec![
                    media_api::MediaOperationKind::Status,
                    media_api::MediaOperationKind::Synthesize,
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
            match operation {
                MediaOperation::Synthesize { .. } => {
                    let _work = TestNativeOperation {
                        witness: context.begin_native_work()?,
                        finished: self.operation_finished.clone(),
                    };
                    *self
                        .active
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) = Some(context);
                    self.execute_started.notify_one();
                    self.cancellation.cancelled().await;
                    *self
                        .active
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) = None;
                    Err(MediaError::new(
                        MediaErrorKind::Cancelled,
                        "the in-flight test operation was cancelled",
                    ))
                }
                MediaOperation::Status { .. } => {
                    let resources = self
                        .active_context()
                        .map(|active| MediaResourceStatus {
                            resource: MediaResourceRef::Operation {
                                identity: active.identity.clone(),
                                incarnation: active.operation_incarnation(),
                            },
                            state: MediaResourceState::Active,
                            started_at_unix_ms: Some(1),
                            updated_at_unix_ms: Some(2),
                        })
                        .into_iter()
                        .collect();
                    Ok(MediaResult::Status { resources })
                }
                _ => Err(MediaError::new(
                    MediaErrorKind::Unsupported,
                    "the in-flight status test supports only status and synthesis",
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

        async fn cancel(&self, context: MediaOperationContext) -> Result<(), MediaError> {
            let active = self.active_context();
            self.cancel_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(context);
            self.cancellation.cancel();
            self.operation_finished.cancelled().await;
            if let Some(active) = active {
                wait_for_test_operation_cleanup(&self.attachment, &active).await?;
            }
            Ok(())
        }
    }

    struct TestRemoteMediaConfig;

    #[async_trait]
    impl RemoteMediaHostConfigProvider for TestRemoteMediaConfig {
        async fn resolve(
            &self,
            _context: &MediaOperationContext,
            _route_id: &str,
            _request: &MediaSessionRequest,
        ) -> Result<remote_control_client::media::RemoteMediaConfig, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "the route cleanup test does not negotiate sessions",
            ))
        }
    }

    struct TestRemoteMediaChannel;

    #[async_trait]
    impl RemoteMediaHostControlChannel for TestRemoteMediaChannel {
        async fn negotiate(
            &self,
            _request: client::protocol::media::MediaConnectionRequestDto,
        ) -> Result<client::protocol::media::MediaConnectionResponseDto, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "the route cleanup test does not negotiate sessions",
            ))
        }

        async fn control(
            &self,
            _request: client::protocol::media::MediaControlRequestDto,
        ) -> Result<client::protocol::media::MediaControlResponseDto, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "the route cleanup test does not control sessions",
            ))
        }
    }

    #[async_trait]
    impl MediaService for LateConfirmDrainService {
        fn capabilities(&self) -> MediaCapabilities {
            MediaCapabilities {
                service_epoch: 7,
                support_revision: 1,
                supported_operations: vec![media_api::MediaOperationKind::Status],
                supported_media_kinds: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }

        async fn execute(
            &self,
            _context: MediaOperationContext,
            operation: MediaOperation,
        ) -> Result<MediaResult, MediaError> {
            if matches!(&operation, MediaOperation::Status { .. })
                && self.block_next_status.swap(false, Ordering::AcqRel)
            {
                self.status_started.notify_one();
                self.release_status.cancelled().await;
            }
            match operation {
                MediaOperation::Status { .. } => Ok(MediaResult::Status {
                    resources: Vec::new(),
                }),
                _ => Err(MediaError::new(
                    MediaErrorKind::Unsupported,
                    "the late-confirmation test service only supports status",
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

        async fn cancel(&self, _context: MediaOperationContext) -> Result<(), MediaError> {
            Ok(())
        }
    }

    #[async_trait]
    impl MediaService for FailingCancelService {
        fn capabilities(&self) -> MediaCapabilities {
            MediaCapabilities {
                service_epoch: 7,
                support_revision: 1,
                supported_operations: vec![media_api::MediaOperationKind::Status],
                supported_media_kinds: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }

        async fn execute(
            &self,
            _context: MediaOperationContext,
            _operation: MediaOperation,
        ) -> Result<MediaResult, MediaError> {
            self.execute_attempts.fetch_add(1, Ordering::Relaxed);
            Err(MediaError::new(MediaErrorKind::Unsupported, "unused"))
        }

        async fn read_media(
            &self,
            _context: MediaOperationContext,
            _media: MediaRef,
        ) -> Result<MediaContent, MediaError> {
            Err(MediaError::new(MediaErrorKind::NotFound, "unused"))
        }

        async fn cancel(&self, _context: MediaOperationContext) -> Result<(), MediaError> {
            self.cancel_attempts.fetch_add(1, Ordering::Relaxed);
            Err(MediaError::new(
                MediaErrorKind::NativeFailure,
                "cancellation outcome is unknown",
            ))
        }
    }

    fn status_request(request_id: &str) -> dto::MediaServiceRequestDto {
        dto::MediaServiceRequestDto {
            request_id: request_id.into(),
            action: dto::MediaServiceActionDto::Execute {
                operation: dto::MediaOperationDto::Status { target: None },
                timeout_budget_ms: Some(5_000),
                max_payload_bytes: None,
            },
        }
    }

    fn wait_operation_request(
        request_id: &str,
        target: MediaOperationContext,
    ) -> dto::MediaServiceRequestDto {
        dto::MediaServiceRequestDto {
            request_id: request_id.into(),
            action: dto::MediaServiceActionDto::Execute {
                operation: dto::MediaOperationDto::Wait {
                    target: dto::MediaResourceRefDto::Operation {
                        operation_id: operation_id_to_dto(target.identity.clone()),
                        incarnation: target.operation_incarnation().as_string(),
                    },
                    after_sequence: Some(u64::MAX),
                    timeout_ms: 4_000,
                    limit: 1,
                },
                timeout_budget_ms: Some(5_000),
                max_payload_bytes: None,
            },
        }
    }

    async fn registered_request_context(
        rpc: &MediaServiceRpc,
        request_id: &str,
        expected_eligible: bool,
    ) -> MediaOperationContext {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let entries = rpc.active_operations.entries.lock().await;
                if let Some(entry) = entries.values().find(|entry| {
                    entry
                        .context
                        .initiator
                        .as_ref()
                        .and_then(|initiator| initiator.request_id.as_deref())
                        == Some(request_id)
                }) {
                    assert_eq!(entry.allow_cleanup_receipt, expected_eligible);
                    return entry.context.clone();
                }
                drop(entries);
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("request should reach the exact operation registry")
    }

    fn cancel_request(
        request_id: &str,
        operation_id: MediaOperationId,
        incarnation: String,
    ) -> dto::MediaServiceRequestDto {
        dto::MediaServiceRequestDto {
            request_id: request_id.into(),
            action: dto::MediaServiceActionDto::Cancel {
                operation_id: operation_id_to_dto(operation_id),
                incarnation,
            },
        }
    }

    fn session_owner(session_id: &str) -> MediaOwner {
        MediaOwner::Session {
            session_id: session_id.into(),
        }
    }

    #[test]
    fn operation_resource_roundtrips_the_host_issued_incarnation() {
        let resource = MediaResourceRef::Operation {
            identity: MediaOperationId::new(71, 7),
            incarnation: MediaOperationIncarnation::new(),
        };
        let expected_incarnation = match &resource {
            MediaResourceRef::Operation { incarnation, .. } => incarnation.as_string(),
            _ => unreachable!(),
        };
        let dto = resource_to_dto(resource.clone());
        assert!(matches!(
            &dto,
            dto::MediaResourceRefDto::Operation { incarnation, .. }
                if incarnation == &expected_incarnation
        ));
        assert_eq!(resource_from_dto(dto).unwrap(), resource);

        let invalid = dto::MediaResourceRefDto::Operation {
            operation_id: operation_id_to_dto(MediaOperationId::new(72, 7)),
            incarnation: "not-a-uuid".into(),
        };
        assert_eq!(
            resource_from_dto(invalid).unwrap_err().kind,
            MediaErrorKind::InvalidRequest
        );
    }

    #[tokio::test]
    async fn status_exposed_operation_handle_cancels_only_its_owner_and_incarnation() {
        let owner = session_owner("44444444-4444-4444-8444-444444444444");
        let service = Arc::new(InFlightStatusService {
            active: StdMutex::new(None),
            execute_started: Notify::new(),
            cancellation: CancellationToken::new(),
            operation_finished: CancellationToken::new(),
            attachment: OnceLock::new(),
            cancel_contexts: StdMutex::new(Vec::new()),
        });
        let rpc = MediaServiceRpc::new(service.clone(), owner.clone())
            .expect("test RPC should create an attachment");
        let attachment = rpc.current_attachment().expect("active attachment");
        service.attachment.set(attachment.clone()).unwrap();
        let other_attachment = rpc
            .attachment_host
            .attach(service.clone())
            .expect("a second active attachment can coexist");

        let execute_rpc = rpc.clone();
        let execute = tokio::spawn(async move {
            execute_rpc
                .handle(dto::MediaServiceRequestDto {
                    request_id: "in-flight-synthesis".into(),
                    action: dto::MediaServiceActionDto::Execute {
                        operation: dto::MediaOperationDto::Synthesize {
                            text: "wait for the external cancellation".into(),
                            language: None,
                            rate: None,
                            voice: None,
                        },
                        timeout_budget_ms: None,
                        max_payload_bytes: None,
                    },
                })
                .await
        });
        service.execute_started.notified().await;
        let active = service
            .active_context()
            .expect("the service should retain the host-minted active context");
        let attached_owner = owner.clone().in_attachment(attachment.id().clone());
        assert_eq!(active.owner, attached_owner);

        let status = rpc
            .handle(status_request("discover-operation-handle"))
            .await;
        let (status_incarnation, resources) = match status.result {
            Some(dto::MediaServiceResultDto::Operation {
                incarnation,
                result: dto::MediaResultDto::Status { resources },
                ..
            }) => (incarnation, resources),
            other => panic!("status request returned unexpected result: {other:?}"),
        };
        assert!(MediaOperationIncarnation::from_string(&status_incarnation).is_some());
        let (operation_id, incarnation) = resources
            .iter()
            .find_map(|resource| match &resource.resource {
                dto::MediaResourceRefDto::Operation {
                    operation_id,
                    incarnation,
                } => Some((operation_id.clone(), incarnation.clone())),
                _ => None,
            })
            .expect("Harness-style status should expose the active operation handle");
        assert_eq!(operation_id_from_dto(operation_id.clone()), active.identity);
        assert_eq!(incarnation, active.operation_incarnation().as_string());

        let wrong_owner = session_owner("55555555-5555-4555-8555-555555555555");
        let active_incarnation = active.operation_incarnation();
        let wrong_owner_cancel = rpc
            .cancel(
                "wrong-operation-owner".into(),
                wrong_owner,
                operation_id_to_dto(active.identity.clone()),
                active_incarnation.as_string(),
            )
            .await;
        assert_eq!(
            wrong_owner_cancel.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::NotFound)
        );
        // The same owner and operation handle in another attachment must not
        // be allowed to cancel the active operation on its original service.
        let mut other_context = active.clone();
        other_context.owner = owner.clone();
        other_attachment
            .service()
            .cancel(other_context)
            .await
            .expect("a cancellation outside the attachment is a no-op");
        assert!(service
            .cancel_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());

        let wrong_incarnation = MediaOperationIncarnation::new().as_string();
        let wrong_cancel = rpc
            .handle(cancel_request(
                "wrong-operation-incarnation",
                active.identity.clone(),
                wrong_incarnation,
            ))
            .await;
        assert_eq!(
            wrong_cancel.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::NotFound)
        );
        assert!(service
            .cancel_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());

        let cancelled = rpc
            .handle(cancel_request(
                "exact-operation-incarnation",
                active.identity.clone(),
                incarnation,
            ))
            .await;
        assert!(matches!(
            cancelled.result,
            Some(dto::MediaServiceResultDto::Cancelled)
        ));
        let cancel_contexts = service
            .cancel_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        assert_eq!(cancel_contexts.len(), 1);
        assert_eq!(cancel_contexts[0].owner, attached_owner);
        assert_eq!(cancel_contexts[0].identity, active.identity);
        assert!(cancel_contexts[0]
            .operation_incarnation()
            .same_instance(&active_incarnation));

        let execute_result = execute.await.expect("execute task should not panic");
        assert_eq!(
            execute_result.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Cancelled)
        );
    }

    #[tokio::test]
    async fn owner_transition_stops_old_resources_before_accepting_new_owner_requests() {
        let old_owner = session_owner("11111111-1111-4111-8111-111111111111");
        let new_owner = session_owner("22222222-2222-4222-8222-222222222222");
        let service = Arc::new(OwnerTrackingService {
            calls: StdMutex::new(Vec::new()),
            old_owner: old_owner.clone(),
            old_owner_stopped: AtomicBool::new(false),
            stop_started: Notify::new(),
            release_stop: CancellationToken::new(),
        });
        let rpc = MediaServiceRpc::new(service.clone(), old_owner.clone())
            .expect("test RPC should create an attachment");
        let old_attachment = rpc.current_attachment().expect("active attachment");
        let old_attached_owner = old_owner.clone().in_attachment(old_attachment.id().clone());
        let old_status = rpc.handle(status_request("old-owner-status")).await;
        assert!(matches!(
            old_status.result,
            Some(dto::MediaServiceResultDto::Operation {
                result: dto::MediaResultDto::Status { ref resources },
                ..
            }) if matches!(resources.as_slice(), [dto::MediaResourceStatusDto {
                resource: dto::MediaResourceRefDto::Session { session_id },
                state: dto::MediaResourceStateDto::Active,
                ..
            }] if session_id == "media_old")
        ));

        let transition_rpc = rpc.clone();
        let transition_owner = new_owner.clone();
        let transition =
            tokio::spawn(async move { transition_rpc.update_owner(transition_owner).await });
        tokio::time::timeout(Duration::from_secs(3), service.stop_started.notified())
            .await
            .expect("drain should try to stop the old attachment resource");
        assert_eq!(old_attachment.phase(), MediaAttachmentPhase::Revoking);
        assert!(!service.old_owner_stopped.load(Ordering::Acquire));
        assert_eq!(*rpc.owner.read().await, old_owner);
        assert_eq!(rpc.current_attachment().unwrap().id(), old_attachment.id());
        let pending = rpc.handle(status_request("blocked-during-stop")).await;
        assert_eq!(
            pending.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Unavailable)
        );
        service.release_stop.cancel();
        transition
            .await
            .expect("transition task should not panic")
            .expect("old attachment and resources should drain");
        assert_eq!(old_attachment.phase(), MediaAttachmentPhase::Revoked);
        let new_attachment = rpc.current_attachment().expect("replacement attachment");
        assert_ne!(old_attachment.id(), new_attachment.id());
        let new_attached_owner = new_owner.clone().in_attachment(new_attachment.id().clone());
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
        let stop_index = calls
            .iter()
            .position(|(owner, operation)| {
                owner == &old_attached_owner
                    && matches!(operation, MediaOperation::Stop {
                        target: MediaResourceRef::Session(MediaSessionRef(media_api::MediaSessionId(id)))
                    } if id == "media_old")
            })
            .expect("the exact old attachment resource must be stopped");
        let new_status_index = calls
            .iter()
            .position(|(owner, operation)| {
                owner == &new_attached_owner && matches!(operation, MediaOperation::Status { .. })
            })
            .expect("the new status must run in the replacement attachment");
        assert!(stop_index < new_status_index);
        assert!(service.old_owner_stopped.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn request_scheduled_before_owner_change_cannot_enter_the_new_attachment() {
        let old_owner = session_owner("11111111-1111-4111-8111-111111111111");
        let new_owner = session_owner("22222222-2222-4222-8222-222222222222");
        let service = Arc::new(OwnerTrackingService {
            calls: StdMutex::new(Vec::new()),
            old_owner: old_owner.clone(),
            old_owner_stopped: AtomicBool::new(false),
            stop_started: Notify::new(),
            release_stop: CancellationToken::new(),
        });
        let rpc = MediaServiceRpc::new(service.clone(), old_owner)
            .expect("test RPC should create an attachment");
        // The transport captured the scope, but the spawned task has not yet
        // been polled when a subsequent session command completes.
        let connection = rpc.connection_token();
        let old_epoch = rpc.request_attachment_epoch();
        assert!(old_epoch.is_some());
        rpc.update_owner(new_owner)
            .await
            .expect("an idle old attachment should drain");
        assert_ne!(old_epoch, rpc.request_attachment_epoch());
        let calls_before = service.calls.lock().unwrap().len();
        let stale = rpc
            .handle_for_connection(status_request("delayed-old-request"), connection, old_epoch)
            .await;
        assert_eq!(
            stale.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Unavailable)
        );
        assert_eq!(service.calls.lock().unwrap().len(), calls_before);
        let fresh = rpc.handle(status_request("new-request")).await;
        assert!(matches!(
            fresh.result,
            Some(dto::MediaServiceResultDto::Operation {
                result: dto::MediaResultDto::Status { ref resources },
                ..
            }) if resources.is_empty()
        ));
    }

    #[tokio::test]
    async fn rpc_cancel_and_owner_transition_keep_the_scheduled_context() {
        let old_owner = session_owner("11111111-1111-4111-8111-111111111111");
        let new_owner = session_owner("22222222-2222-4222-8222-222222222222");
        let service = Arc::new(BlockingOwnerService::new());
        let rpc = MediaServiceRpc::new(service.clone(), old_owner.clone())
            .expect("test RPC should create an attachment");
        service
            .attachment
            .set(rpc.current_attachment().expect("active attachment"))
            .unwrap();

        let operation_rpc = rpc.clone();
        let operation = tokio::spawn(async move {
            operation_rpc
                .handle(status_request("blocked-old-owner"))
                .await
        });
        service.execute_started.notified().await;
        let operation_context = service
            .execute_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .find(|context| {
                context
                    .initiator
                    .as_ref()
                    .and_then(|initiator| initiator.request_id.as_deref())
                    == Some("blocked-old-owner")
            })
            .cloned()
            .expect("the blocked operation was scheduled with its trusted context");

        let cancel_rpc = rpc.clone();
        let operation_identity = operation_context.identity.clone();
        let operation_incarnation = operation_context.operation_incarnation();
        let cancel = tokio::spawn(async move {
            cancel_rpc
                .handle(dto::MediaServiceRequestDto {
                    request_id: "cancel-old-operation".into(),
                    action: dto::MediaServiceActionDto::Cancel {
                        operation_id: operation_id_to_dto(operation_identity),
                        incarnation: operation_incarnation.as_string(),
                    },
                })
                .await
        });
        service.wait_for_cancels(1).await;
        assert!(
            !cancel.is_finished(),
            "the cancellation must wait for the original operation's cleanup receipt"
        );

        let transition_rpc = rpc.clone();
        let transition_owner = new_owner.clone();
        let transition = tokio::spawn(async move {
            transition_rpc
                .update_owner(transition_owner)
                .await
                .expect("old attachment should drain after cancel");
        });
        service.cancel_release.cancel();

        let cancel_response = cancel.await.expect("cancel handler task did not panic");
        assert!(matches!(
            cancel_response.result,
            Some(dto::MediaServiceResultDto::Cancelled)
        ));
        let operation_response = operation
            .await
            .expect("operation handler task did not panic");
        assert_eq!(
            operation_response.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Cancelled)
        );
        transition
            .await
            .expect("owner transition task did not panic");

        let cancelled = service
            .cancel_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        assert_eq!(
            cancelled.len(),
            1,
            "RPC cancel, owner transition, and the operation guard must share one cancel flight"
        );
        assert!(
            cancelled
                .iter()
                .all(|context| context == &operation_context),
            "RPC cancel, transition cancel, and late guard cancel must use the scheduled context"
        );
        assert!(cancelled
            .iter()
            .all(|context| context.owner.logical_owner() == &old_owner));

        let new_status = rpc.handle(status_request("new-owner-status")).await;
        assert!(matches!(
            new_status.result,
            Some(dto::MediaServiceResultDto::Operation {
                result: dto::MediaResultDto::Status { ref resources },
                ..
            }) if resources.is_empty()
        ));
        let contexts = service
            .execute_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(contexts
            .iter()
            .any(|context| context.owner.logical_owner() == &new_owner));
    }

    #[tokio::test]
    async fn unknown_attachment_drain_keeps_routes_and_fails_closed_for_all_rpc_actions() {
        let old_owner = session_owner("11111111-1111-4111-8111-111111111111");
        let new_owner = session_owner("22222222-2222-4222-8222-222222222222");
        let service = Arc::new(FailingCancelService {
            cancel_attempts: std::sync::atomic::AtomicUsize::new(0),
            execute_attempts: std::sync::atomic::AtomicUsize::new(0),
        });
        let rpc = MediaServiceRpc::new(service.clone(), old_owner.clone())
            .expect("test RPC should own one attachment");
        let attachment = rpc.current_attachment().expect("active attachment");
        let old_route_owner = old_owner.clone().in_attachment(attachment.id().clone());
        let route = "1e5d7a65-b37c-4e51-9ac8-33d2c2a98a61".to_owned();
        rpc.registered_routes
            .lock()
            .await
            .insert((old_route_owner.clone(), route.clone()), 1);

        // Record the logical owner in the scoped service so the drain performs
        // Status calls. Those calls fail, which must leave the attachment Unknown.
        let direct = attachment.service();
        let _ = direct
            .execute(
                MediaOperationContext::new(MediaOperationId::new(900, 7), old_owner.clone()),
                MediaOperation::Status { target: None },
            )
            .await;
        let before_fence = service.execute_attempts.load(Ordering::Relaxed);

        rpc.begin_owner_transition()
            .expect("the old attachment should synchronously close admission");
        assert_ne!(attachment.phase(), MediaAttachmentPhase::Active);
        let rejected = direct
            .execute(
                MediaOperationContext::new(MediaOperationId::new(901, 7), old_owner.clone()),
                MediaOperation::Status { target: None },
            )
            .await
            .expect_err("direct Desktop service calls must also hit the fence");
        assert_eq!(rejected.kind, MediaErrorKind::Cancelled);
        assert_eq!(
            service.execute_attempts.load(Ordering::Relaxed),
            before_fence,
            "a rejected post-fence call must not reach the shared service"
        );

        assert!(rpc.update_owner(new_owner.clone()).await.is_err());
        assert_eq!(attachment.phase(), MediaAttachmentPhase::Unknown);
        assert!(rpc.transition_pending.load(Ordering::Acquire));
        assert_eq!(*rpc.owner.read().await, old_owner);
        assert!(
            rpc.attachment_host.attachment(attachment.id()).is_some(),
            "Unknown ownership must stay retained by the process host"
        );
        assert!(rpc
            .registered_routes
            .lock()
            .await
            .contains_key(&(old_route_owner, route.clone())));
        assert_eq!(
            attachment.wait_for_drain_completion().await,
            MediaAttachmentPhase::Unknown,
            "a stable Unknown drain must not keep a completion watcher alive"
        );

        let response = rpc
            .handle(dto::MediaServiceRequestDto {
                request_id: "blocked-unregister".into(),
                action: dto::MediaServiceActionDto::UnregisterConnection {
                    connection_id: route,
                },
            })
            .await;
        let error = response
            .error
            .expect("transition-pending RPC must fail closed");
        assert_eq!(error.kind, dto::MediaErrorKindDto::Unavailable);
        assert!(error.message.contains("owner transition is pending"));

        let drain_calls = service.execute_attempts.load(Ordering::Relaxed);
        assert!(rpc.update_owner(new_owner).await.is_err());
        assert_eq!(
            service.execute_attempts.load(Ordering::Relaxed),
            drain_calls,
            "retry must observe sticky Unknown without replaying Status/Stop"
        );
    }

    async fn record_owner_on_attachment(attachment: &MediaAttachmentHandle, owner: &MediaOwner) {
        attachment
            .service()
            .execute(
                MediaOperationContext::new(MediaOperationId::new(700, 7), owner.clone()),
                MediaOperation::Status { target: None },
            )
            .await
            .expect("the test service should register the owner with the attachment");
    }

    #[tokio::test]
    async fn late_confirmed_owner_transition_removes_only_the_old_attachment_route() {
        let owner = session_owner("11111111-1111-4111-8111-111111111111");
        let service = Arc::new(LateConfirmDrainService {
            block_next_status: AtomicBool::new(false),
            status_started: Notify::new(),
            release_status: CancellationToken::new(),
        });
        let mut rpc = MediaServiceRpc::new(service.clone(), owner.clone())
            .expect("test RPC should create an attachment");
        let connector = RemoteMediaConnector::new(
            Arc::new(TestRemoteMediaConfig),
            Arc::new(audio_provider::media_store::BoundedMediaResourceStore::default()),
        );
        Arc::get_mut(&mut rpc)
            .expect("the test RPC has one owner before spawning work")
            .remote_connector = Some(connector.clone());
        let old_attachment = rpc.current_attachment().expect("active attachment");
        record_owner_on_attachment(&old_attachment, &owner).await;
        let old_route_owner = owner.clone().in_attachment(old_attachment.id().clone());
        let connection_id = "1e5d7a65-b37c-4e51-9ac8-33d2c2a98a61".to_owned();
        let old_route = (old_route_owner.clone(), connection_id.clone());
        connector
            .register_attachment_route(
                old_route_owner.clone(),
                connection_id.clone(),
                "old route".into(),
                vec![MediaSessionMode::Audio],
                vec![client::protocol::media::MediaSourceKindDto::Microphone],
                Arc::new(TestRemoteMediaChannel),
                old_attachment.service(),
            )
            .await
            .expect("the test connector should register the old route");
        rpc.registered_routes
            .lock()
            .await
            .insert(old_route.clone(), 1);

        // Independent attachments may coexist while both are active. Prepare
        // the same-session route before revocation closes new admission, then
        // verify a late receipt only removes the exact old attachment's route.
        let replacement_attachment = rpc
            .attachment_host
            .attach(service.clone())
            .expect("a separate active attachment can be prepared");
        let replacement_route = (
            owner
                .clone()
                .in_attachment(replacement_attachment.id().clone()),
            connection_id.clone(),
        );
        connector
            .register_attachment_route(
                replacement_route.0.clone(),
                connection_id,
                "replacement route".into(),
                vec![MediaSessionMode::Audio],
                vec![client::protocol::media::MediaSourceKindDto::Microphone],
                Arc::new(TestRemoteMediaChannel),
                replacement_attachment.service(),
            )
            .await
            .expect("the test connector should register the replacement route");
        rpc.registered_routes
            .lock()
            .await
            .insert(replacement_route.clone(), 2);

        service.block_next_status.store(true, Ordering::Release);
        rpc.begin_owner_transition()
            .expect("transition should close old attachment admission");
        let transition_rpc = rpc.clone();
        let transition_owner = owner.clone();
        let transition =
            tokio::spawn(async move { transition_rpc.update_owner(transition_owner).await });
        service.status_started.notified().await;
        assert!(
            tokio::time::timeout(Duration::from_secs(3), transition)
                .await
                .expect("the bounded observer should return")
                .expect("transition task should not panic")
                .is_err(),
            "the blocked drain should exceed the caller's one-second observation"
        );
        assert_eq!(old_attachment.phase(), MediaAttachmentPhase::Revoking);
        assert!(rpc.registered_routes.lock().await.contains_key(&old_route));
        assert_eq!(replacement_attachment.phase(), MediaAttachmentPhase::Active);
        assert_eq!(
            rpc.registered_routes.lock().await.get(&replacement_route),
            Some(&2)
        );
        let rejected_attachment = rpc
            .attachment_host
            .attach(service.clone())
            .expect_err("new attachments must be fenced while the old drain is unconfirmed");
        assert_eq!(rejected_attachment.kind, MediaErrorKind::Unavailable);
        let pending = rpc
            .handle(status_request("pending-transition-status"))
            .await;
        assert_eq!(
            pending.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Unavailable)
        );

        service.release_status.cancel();
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(3),
                old_attachment.wait_for_drain_completion()
            )
            .await
            .expect("Harness drain should finish within its independent budget"),
            MediaAttachmentPhase::Revoked
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if !rpc.registered_routes.lock().await.contains_key(&old_route) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("late confirmed receipt should trigger old-route cleanup");
        assert_eq!(
            rpc.registered_routes.lock().await.get(&replacement_route),
            Some(&2),
            "same-session route on another attachment must survive old cleanup"
        );
        assert!(
            connector.supported_modes(&old_route_owner).await.is_empty(),
            "the old remote route should be removed after the exact drain confirms"
        );
        assert_eq!(
            connector.supported_modes(&replacement_route.0).await,
            vec![MediaSessionMode::Audio],
            "the same-session route registered under the new attachment must survive"
        );
    }

    #[tokio::test]
    async fn late_confirmed_close_cleans_routes_and_releases_its_watcher() {
        let owner = session_owner("33333333-3333-4333-8333-333333333333");
        let service = Arc::new(LateConfirmDrainService {
            block_next_status: AtomicBool::new(false),
            status_started: Notify::new(),
            release_status: CancellationToken::new(),
        });
        let mut rpc = MediaServiceRpc::new(service.clone(), owner.clone())
            .expect("test RPC should create an attachment");
        let connector = RemoteMediaConnector::new(
            Arc::new(TestRemoteMediaConfig),
            Arc::new(audio_provider::media_store::BoundedMediaResourceStore::default()),
        );
        Arc::get_mut(&mut rpc)
            .expect("the test RPC has one owner before close is spawned")
            .remote_connector = Some(connector.clone());
        let attachment = rpc.current_attachment().expect("active attachment");
        record_owner_on_attachment(&attachment, &owner).await;
        let route_owner = owner.clone().in_attachment(attachment.id().clone());
        let connection_id = "5f1ce2ee-0d1c-4ae9-9f98-b8f8c03b3dc5".to_owned();
        let route = (route_owner.clone(), connection_id.clone());
        connector
            .register_attachment_route(
                route_owner.clone(),
                connection_id,
                "close route".into(),
                vec![MediaSessionMode::Audio],
                vec![client::protocol::media::MediaSourceKindDto::Microphone],
                Arc::new(TestRemoteMediaChannel),
                attachment.service(),
            )
            .await
            .expect("the test connector should register a route before close");
        rpc.registered_routes.lock().await.insert(route.clone(), 1);
        let weak_rpc = Arc::downgrade(&rpc);

        service.block_next_status.store(true, Ordering::Release);
        let close_rpc = rpc.clone();
        let close = tokio::spawn(async move { close_rpc.close().await });
        service.status_started.notified().await;
        tokio::time::timeout(Duration::from_secs(3), close)
            .await
            .expect("close should return after its bounded observer")
            .expect("close task should not panic");
        assert_eq!(attachment.phase(), MediaAttachmentPhase::Revoking);
        drop(rpc);
        assert!(
            weak_rpc.upgrade().is_some(),
            "the late-confirmed cleanup must retain the route owner while the drain runs"
        );

        service.release_status.cancel();
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(3),
                attachment.wait_for_drain_completion()
            )
            .await
            .expect("Harness drain should finish within its independent budget"),
            MediaAttachmentPhase::Revoked
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if connector.supported_modes(&route_owner).await.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("late-confirmed close should unregister the exact remote route");
        tokio::time::timeout(Duration::from_secs(3), async {
            while weak_rpc.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("close watcher should finish route cleanup and release its owner");
    }

    #[tokio::test]
    async fn confirmed_owner_revoke_retires_only_its_unconfirmed_cancel_fences() {
        let old_owner = session_owner("11111111-1111-4111-8111-111111111111");
        let new_owner = session_owner("22222222-2222-4222-8222-222222222222");
        let service = Arc::new(BlockingOwnerService::new());
        let rpc = MediaServiceRpc::new(service.clone(), old_owner)
            .expect("test RPC should own an attachment");
        service
            .attachment
            .set(rpc.current_attachment().expect("active attachment"))
            .unwrap();
        let operation_rpc = rpc.clone();
        let operation = tokio::spawn(async move {
            operation_rpc
                .handle(status_request("blocked-old-owner"))
                .await
        });
        service.execute_started.notified().await;
        let identity = service
            .execute_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .find(|context| {
                context
                    .initiator
                    .as_ref()
                    .and_then(|initiator| initiator.request_id.as_deref())
                    == Some("blocked-old-owner")
            })
            .expect("the operation should be registered before transition")
            .identity
            .clone();

        // If the attachment drain captured the operation before its RPC future
        // unwinds, its service cancel can still settle. The guard's later cancel
        // is rejected by the already-revoking wrapper and remains Unknown until
        // the attachment-level Confirmed receipt arrives.
        service.cancel_release.cancel();
        rpc.update_owner(new_owner.clone())
            .await
            .expect("the attachment drain confirms no active resources remain");
        let response = operation.await.expect("operation task should not panic");
        assert_eq!(
            response.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Cancelled)
        );
        assert!(rpc.active_operations.context(&identity).await.is_none());

        let new_status = rpc.handle(status_request("after-confirmed-revoke")).await;
        assert!(matches!(
            new_status.result,
            Some(dto::MediaServiceResultDto::Operation {
                result: dto::MediaResultDto::Status { .. },
                ..
            })
        ));
    }

    #[tokio::test]
    async fn stale_route_cleanup_cannot_remove_a_same_id_replacement_incarnation() {
        let owner = session_owner("11111111-1111-4111-8111-111111111111");
        let rpc = MediaServiceRpc::new(
            Arc::new(FailingCancelService {
                cancel_attempts: std::sync::atomic::AtomicUsize::new(0),
                execute_attempts: std::sync::atomic::AtomicUsize::new(0),
            }),
            owner.clone(),
        )
        .expect("test RPC should own an attachment");
        let route_id = "1e5d7a65-b37c-4e51-9ac8-33d2c2a98a61".to_owned();
        let route_key = (owner.clone(), route_id.clone());
        rpc.registered_routes
            .lock()
            .await
            .insert(route_key.clone(), 2);

        rpc.unregister_route_bounded(
            owner,
            route_id,
            tokio::time::Instant::now() + CANCEL_OBSERVATION_TIMEOUT,
            Some(1),
            None,
        )
        .await
        .expect("old incarnation cleanup is a no-op without a connector");
        assert_eq!(rpc.registered_routes.lock().await.get(&route_key), Some(&2));
    }

    #[tokio::test]
    async fn guard_cancel_observation_is_bounded_and_keeps_fence() {
        let old_owner = session_owner("11111111-1111-4111-8111-111111111111");
        let new_owner = session_owner("22222222-2222-4222-8222-222222222222");
        let identity = MediaOperationId::new(41, 7);
        let old_context = MediaOperationContext::new(identity.clone(), old_owner)
            .with_timeout_budget_ms(1_000)
            .with_max_payload_bytes(1024);
        let registry = Arc::new(ActiveOperationRegistry::default());
        let service = Arc::new(BlockingOwnerService::new());
        let old = registry
            .register(old_context.clone(), service.clone(), None, false)
            .await
            .expect("first registration should succeed");
        let guard = ActiveOperationGuard::new(registry.clone(), old.clone());
        tokio::time::timeout(
            CANCEL_OBSERVATION_TIMEOUT + Duration::from_millis(500),
            guard.finish(true),
        )
        .await
        .expect("guard cancellation observation must be bounded");
        service.wait_for_cancels(1).await;

        let current_flight = registry
            .entries
            .lock()
            .await
            .get(&identity)
            .and_then(|entry| entry.cancellation.clone())
            .expect("a timed-out cancel must remain registered");
        let retry_registry = registry.clone();
        let retry_identity = identity.clone();
        let retry_owner = old.context.owner.clone();
        let retry_incarnation = old.context.operation_incarnation();
        let rpc_retry = async move {
            retry_registry
                .cancel_identity(&retry_identity, &retry_owner, &retry_incarnation)
                .await
                .expect("terminal unconfirmed cancellation remains fenced")
        };
        let transition_registry = registry.clone();
        let owner_transition = async move {
            let operations = transition_registry.reserve_all_cancels().await;
            assert_eq!(operations.len(), 1);
            transition_registry
                .cancel_lease(&operations[0])
                .await
                .expect("owner transition must join the unconfirmed cancel")
        };
        let (rpc_retry, owner_transition) = tokio::join!(rpc_retry, owner_transition);
        assert!(Arc::ptr_eq(
            &current_flight.incarnation,
            &rpc_retry.incarnation
        ));
        assert!(Arc::ptr_eq(
            &current_flight.incarnation,
            &owner_transition.incarnation
        ));
        assert_eq!(*service.cancel_count.borrow(), 1);
        assert!(matches!(
            rpc_retry.observe(Duration::from_millis(20)).await,
            CancellationObservation::TimedOut
        ));
        assert_eq!(registry.context(&identity).await, Some(old_context));
        let replay = MediaOperationContext::new(identity.clone(), new_owner)
            .with_timeout_budget_ms(2_000)
            .with_max_payload_bytes(2048);
        assert!(registry
            .register(replay, service.clone(), None, false)
            .await
            .is_err());
        assert_eq!(*service.cancel_count.borrow(), 1);
    }

    #[tokio::test]
    async fn confirmed_delayed_cancel_releases_only_its_original_incarnation() {
        let owner = session_owner("11111111-1111-4111-8111-111111111111");
        let identity = MediaOperationId::new(52, 7);
        let old_context = MediaOperationContext::new(identity.clone(), owner.clone())
            .with_timeout_budget_ms(1_000)
            .with_max_payload_bytes(1024);
        let new_context = MediaOperationContext::new(identity.clone(), owner.clone())
            .with_timeout_budget_ms(2_000)
            .with_max_payload_bytes(2048);
        let service = Arc::new(BlockingOwnerService::new());
        let registry = Arc::new(ActiveOperationRegistry::default());
        let old = registry
            .register(old_context.clone(), service.clone(), None, false)
            .await
            .expect("the original operation should register");
        let old_incarnation = old.context.operation_incarnation();

        let first = registry
            .cancel_lease(&old)
            .await
            .expect("the first cancel should start a flight");
        service.wait_for_cancels(1).await;
        let retry = registry
            .cancel_identity(&identity, &owner, &old_incarnation)
            .await
            .expect("a retry joins the original cancellation flight");
        assert!(Arc::ptr_eq(&first.incarnation, &retry.incarnation));
        assert_eq!(*service.cancel_count.borrow(), 1);
        assert!(matches!(
            first.observe(Duration::from_millis(20)).await,
            CancellationObservation::TimedOut
        ));

        registry.finish(&old, false).await;
        assert!(registry
            .register(new_context.clone(), service.clone(), None, false)
            .await
            .is_err());
        service.cancel_release.cancel();
        first
            .wait()
            .await
            .expect("the delayed cancel was confirmed");
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if registry.context(&identity).await.is_none() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("confirmation should release the terminal old incarnation");

        let replacement = registry
            .register(new_context.clone(), service.clone(), None, false)
            .await
            .expect("identity reuse is safe only after old confirmation");
        assert!(registry
            .cancel_identity(&identity, &owner, &old_incarnation)
            .await
            .is_none());
        assert_eq!(*service.cancel_count.borrow(), 1);
        assert!(registry.finish(&old, true).await.is_none());
        assert_eq!(registry.context(&identity).await, Some(new_context));
        assert!(!Arc::ptr_eq(&old.incarnation, &replacement.incarnation));
    }

    #[test]
    fn cleanup_receipt_is_enabled_only_for_wait_on_a_local_operation() {
        let identity = MediaOperationId::new(899, 7);
        let target = MediaResourceRef::Operation {
            identity,
            incarnation: MediaOperationIncarnation::new(),
        };
        let local_wait = MediaOperation::Wait {
            target: target.clone(),
            after: None,
            timeout_ms: 100,
            limit: 1,
        };
        assert!(allows_cleanup_receipt(&local_wait, true));
        assert!(!allows_cleanup_receipt(&local_wait, false));
        assert!(!allows_cleanup_receipt(
            &MediaOperation::Observe {
                target,
                after: None,
                limit: 1,
            },
            true
        ));
        assert!(!allows_cleanup_receipt(
            &MediaOperation::Synthesize {
                text: "test".into(),
                language: None,
                rate: None,
                voice: None,
            },
            true
        ));
        assert!(!allows_cleanup_receipt(
            &MediaOperation::Wait {
                target: MediaResourceRef::Session(MediaSessionRef(
                    media_api::MediaSessionId::from("remote")
                )),
                after: None,
                timeout_ms: 100,
                limit: 1,
            },
            true
        ));
    }

    struct NoDeviceAudio {
        cancels: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl media_api::audio::AudioService for NoDeviceAudio {
        fn capabilities(&self) -> media_api::audio::AudioCapabilitySnapshot {
            media_api::audio::AudioCapabilitySnapshot {
                service_epoch: 7,
                support_revision: 1,
                supported_operations: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }

        async fn execute(
            &self,
            _: media_api::audio::AudioOperationContext,
            _: media_api::audio::AudioOperation,
        ) -> Result<media_api::audio::AudioOperationSuccess, media_api::audio::AudioError> {
            Err(media_api::audio::AudioError::new(
                media_api::audio::AudioErrorKind::Unsupported,
                "no native audio in this test",
            ))
        }

        async fn cancel(
            &self,
            _: media_api::audio::AudioOperationContext,
        ) -> Result<(), media_api::audio::AudioError> {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn real_app_host_wait_registration_enables_only_safe_receipt_and_closes() {
        // AppMediaHost has a process-global host registry shared with boot.
        let _serial = crate::driver::LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let connection = crate::server::BridgeConnection::new();
        let (callback, _responder) =
            crate::media_bridge::new_media_device_callback(connection.audio_sink());
        let audio = Arc::new(NoDeviceAudio {
            cancels: std::sync::atomic::AtomicUsize::new(0),
        });
        let host = audio_provider::media_host::AppMediaHost::new(
            Arc::new(audio_provider::media_store::BoundedMediaResourceStore::default()),
            audio.clone(),
            None,
            Arc::new(audio_provider::media_native::CallbackMediaDevicePorts::new(
                callback,
            )),
            Vec::new(),
            Arc::new(audio_provider::media_cloud::AudioProviderCloudRegistry::new("wav")),
            Vec::new(),
            media_runtime::AudioRoutingSettings::default(),
            7,
            1024,
        )
        .unwrap();
        let attachment = host.create_media_attachment().unwrap();
        host.set_current_media_attachment(Some(attachment.clone()))
            .unwrap();
        let owner = session_owner("11111111-1111-4111-8111-111111111111");
        let rpc = MediaServiceRpc::for_app_host(host.clone(), owner.clone()).unwrap();
        let prior = MediaOperationContext::new(MediaOperationId::new(899, 7), owner);
        attachment
            .service()
            .execute(
                prior.clone(),
                MediaOperation::Sources {
                    kind: None,
                    usage: None,
                },
            )
            .await
            .unwrap();
        let target_rpc = rpc.clone();
        let target_wait = tokio::spawn(async move {
            target_rpc
                .handle(wait_operation_request("target-wait", prior))
                .await
        });
        let target_context = registered_request_context(&rpc, "target-wait", true).await;
        let wait_rpc = rpc.clone();
        let target_for_wait = target_context.clone();
        let wait = tokio::spawn(async move {
            wait_rpc
                .handle(wait_operation_request("host-wait", target_for_wait))
                .await
        });
        let context = registered_request_context(&rpc, "host-wait", true).await;
        assert_eq!(
            attachment.operation_cleanup(&context),
            MediaOperationCleanup::Pending
        );

        let first = rpc
            .handle(cancel_request(
                "host-cancel",
                context.identity.clone(),
                context.operation_incarnation().as_string(),
            ))
            .await;
        assert_eq!(
            first.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Busy)
        );
        assert_eq!(
            wait.await.unwrap().error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Cancelled)
        );
        let replay = rpc
            .handle(cancel_request(
                "host-cancel",
                context.identity.clone(),
                context.operation_incarnation().as_string(),
            ))
            .await;
        assert_eq!(
            replay.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Busy)
        );
        let retry = rpc
            .handle(cancel_request(
                "host-cancel-retry",
                context.identity.clone(),
                context.operation_incarnation().as_string(),
            ))
            .await;
        assert!(matches!(
            retry.result,
            Some(dto::MediaServiceResultDto::Cancelled)
        ));
        assert!(
            !target_wait.is_finished(),
            "cancelling an observer must not stop its target operation"
        );
        let target_cancel = rpc
            .handle(cancel_request(
                "target-cancel",
                target_context.identity.clone(),
                target_context.operation_incarnation().as_string(),
            ))
            .await;
        assert_eq!(
            target_cancel.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Busy)
        );
        assert_eq!(
            target_wait.await.unwrap().error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Cancelled)
        );
        assert_eq!(audio.cancels.load(Ordering::SeqCst), 0);

        rpc.close().await;
        assert_eq!(
            attachment.wait_for_drain_completion().await,
            MediaAttachmentPhase::Revoked
        );
        assert!(host
            .media_attachment_host()
            .attachment(attachment.id())
            .is_none());
    }

    #[tokio::test]
    async fn real_unified_wait_busy_recovers_without_replaying_cancel() {
        use std::{future::Future, task::Poll};

        let owner = session_owner("11111111-1111-4111-8111-111111111111");
        let shared = media_runtime::MediaRuntimeBuilder::new(Arc::new(
            audio_provider::media_store::BoundedMediaResourceStore::default(),
        ))
        .service_epoch(7)
        .build();
        let rpc = MediaServiceRpc::new(shared, owner.clone()).unwrap();
        let attachment = rpc.current_attachment().unwrap();
        let service = attachment.service();
        let prior = MediaOperationContext::new(MediaOperationId::new(900, 7), owner.clone());
        service
            .execute(
                prior.clone(),
                MediaOperation::Sources {
                    kind: None,
                    usage: None,
                },
            )
            .await
            .unwrap();
        let context = MediaOperationContext::new(MediaOperationId::new(901, 7), owner.clone())
            .with_timeout_budget_ms(5_000);
        let operation = MediaOperation::Wait {
            target: MediaResourceRef::Operation {
                identity: prior.identity.clone(),
                incarnation: prior.operation_incarnation(),
            },
            after: Some(MediaEventCursor { sequence: u64::MAX }),
            timeout_ms: 4_000,
            limit: 1,
        };
        assert!(allows_cleanup_receipt(&operation, true));
        let lease = rpc
            .active_operations
            .register(
                context.clone(),
                service.clone(),
                Some(attachment.clone()),
                true,
            )
            .await
            .unwrap();
        let mut execute = Box::pin(service.execute(context.clone(), operation));
        std::future::poll_fn(|cx| match execute.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(result) => panic!("the wait must remain pending: {result:?}"),
        })
        .await;
        assert_eq!(
            attachment.operation_cleanup(&context),
            MediaOperationCleanup::Pending
        );

        let status = rpc.handle(status_request("real-unified-status")).await;
        assert!(matches!(status.result,
            Some(dto::MediaServiceResultDto::Operation {
                result: dto::MediaResultDto::Status { ref resources }, ..
            }) if resources.iter().any(|resource| matches!(&resource.resource,
                dto::MediaResourceRefDto::Operation { operation_id, incarnation }
                if operation_id_from_dto(operation_id.clone()) == context.identity
                    && incarnation == &context.operation_incarnation().as_string()))));

        let cancel = || {
            cancel_request(
                "real-unified-cancel",
                context.identity.clone(),
                context.operation_incarnation().as_string(),
            )
        };
        let first = rpc.handle(cancel()).await;
        assert_eq!(
            first.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Busy)
        );
        assert_eq!(execute.await.unwrap_err().kind, MediaErrorKind::Cancelled);
        assert_eq!(
            attachment.operation_cleanup(&context),
            MediaOperationCleanup::Confirmed
        );
        rpc.active_operations
            .finish(&lease, true)
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert!(rpc
            .active_operations
            .context(&context.identity)
            .await
            .is_none());

        // Correlation replay keeps the first Busy; a fresh request receives
        // the exact completion receipt, without another native cancellation.
        let replay = rpc.handle(cancel()).await;
        assert_eq!(
            replay.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Busy)
        );
        let fresh = rpc
            .handle(cancel_request(
                "real-unified-cancel-retry",
                context.identity.clone(),
                context.operation_incarnation().as_string(),
            ))
            .await;
        assert!(matches!(
            fresh.result,
            Some(dto::MediaServiceResultDto::Cancelled)
        ));
        assert!(rpc
            .active_operations
            .cancel_identity(
                &context.identity,
                &session_owner("other"),
                &context.operation_incarnation()
            )
            .await
            .is_none());
        let replacement = MediaOperationContext::new(context.identity.clone(), owner);
        assert!(rpc
            .active_operations
            .cancel_identity(
                &context.identity,
                &replacement.owner,
                &replacement.operation_incarnation()
            )
            .await
            .is_none());
    }

    struct BusyWitnessService {
        work: StdMutex<HashMap<MediaOperationId, Box<dyn media_api::MediaNativeWork>>>,
        cancels: std::sync::atomic::AtomicUsize,
        cancel_kind: MediaErrorKind,
        cancel_started: Notify,
        cancel_release: CancellationToken,
        execute_release: CancellationToken,
    }

    #[async_trait]
    impl MediaService for BusyWitnessService {
        fn capabilities(&self) -> MediaCapabilities {
            MediaCapabilities {
                service_epoch: 7,
                support_revision: 1,
                supported_operations: Vec::new(),
                supported_media_kinds: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }

        async fn execute(
            &self,
            context: MediaOperationContext,
            _: MediaOperation,
        ) -> Result<MediaResult, MediaError> {
            self.work.lock().unwrap().insert(
                context.identity.clone(),
                context.begin_native_work()?.unwrap(),
            );
            self.execute_release.cancelled().await;
            if let Some(work) = self.work.lock().unwrap().remove(&context.identity) {
                work.confirm();
            }
            Err(MediaError::new(
                MediaErrorKind::Cancelled,
                "test operation ended",
            ))
        }

        async fn read_media(
            &self,
            _: MediaOperationContext,
            _: MediaRef,
        ) -> Result<MediaContent, MediaError> {
            Err(MediaError::new(MediaErrorKind::Unsupported, "unused"))
        }

        async fn cancel(&self, _: MediaOperationContext) -> Result<(), MediaError> {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            self.cancel_started.notify_one();
            self.cancel_release.cancelled().await;
            Err(MediaError::new(
                self.cancel_kind,
                "native cancellation not confirmed",
            ))
        }
    }

    #[tokio::test]
    async fn arbitrary_service_wait_does_not_inherit_unified_cleanup_semantics() {
        let native = Arc::new(BusyWitnessService {
            work: StdMutex::new(HashMap::new()),
            cancels: std::sync::atomic::AtomicUsize::new(0),
            cancel_kind: MediaErrorKind::Busy,
            cancel_started: Notify::new(),
            cancel_release: CancellationToken::new(),
            execute_release: CancellationToken::new(),
        });
        native.cancel_release.cancel();
        let owner = session_owner("other-service");
        let rpc = MediaServiceRpc::new(native.clone(), owner.clone()).unwrap();
        let attachment = rpc.current_attachment().unwrap();
        let target = MediaOperationContext::new(MediaOperationId::new(898, 7), owner);
        let wait_rpc = rpc.clone();
        let wait = tokio::spawn(async move {
            wait_rpc
                .handle(wait_operation_request("other-wait", target))
                .await
        });
        let context = registered_request_context(&rpc, "other-wait", false).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while attachment.operation_cleanup(&context) != MediaOperationCleanup::Pending {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let first = rpc
            .handle(cancel_request(
                "other-cancel",
                context.identity.clone(),
                context.operation_incarnation().as_string(),
            ))
            .await;
        assert_eq!(
            first.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Busy)
        );
        native.execute_release.cancel();
        assert_eq!(
            wait.await.unwrap().error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Cancelled)
        );
        assert_eq!(
            attachment.operation_cleanup(&context),
            MediaOperationCleanup::Confirmed
        );
        let retry = rpc
            .handle(cancel_request(
                "other-cancel-retry",
                context.identity.clone(),
                context.operation_incarnation().as_string(),
            ))
            .await;
        assert_eq!(
            retry.error.map(|error| error.kind),
            Some(dto::MediaErrorKindDto::Busy)
        );
        assert!(rpc
            .active_operations
            .context(&context.identity)
            .await
            .is_some());
        assert!(rpc
            .active_operations
            .receipts
            .lock()
            .unwrap()
            .values
            .is_empty());
        assert_eq!(native.cancels.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn busy_receipt_requires_exact_native_witness_and_never_redispatches() {
        use std::{future::Future, task::Poll};

        // The witness fixture directly enables the internal observer to test
        // its lifecycle races; production enables it only for Wait(Operation).
        for (kind, confirm, allowed) in [
            (MediaErrorKind::Busy, true, true),
            (MediaErrorKind::Busy, false, true),
            (MediaErrorKind::NativeFailure, true, true),
            (MediaErrorKind::Busy, true, false),
        ] {
            let native = Arc::new(BusyWitnessService {
                work: StdMutex::new(HashMap::new()),
                cancels: std::sync::atomic::AtomicUsize::new(0),
                cancel_kind: kind,
                cancel_started: Notify::new(),
                cancel_release: CancellationToken::new(),
                execute_release: CancellationToken::new(),
            });
            native.cancel_release.cancel();
            let host = MediaAttachmentHost::new();
            let attachment = host.attach(native.clone()).unwrap();
            let service = attachment.service();
            let context = MediaOperationContext::new(
                MediaOperationId::new(1000, 7),
                session_owner("witness"),
            );
            let registry = Arc::new(ActiveOperationRegistry::default());
            let lease = registry
                .register(
                    context.clone(),
                    service.clone(),
                    Some(attachment.clone()),
                    allowed,
                )
                .await
                .unwrap();
            let mut execute = Box::pin(service.execute(
                context.clone(),
                MediaOperation::Sources {
                    kind: None,
                    usage: None,
                },
            ));
            std::future::poll_fn(|cx| match execute.as_mut().poll(cx) {
                Poll::Pending => Poll::Ready(()),
                Poll::Ready(result) => panic!("test operation unexpectedly completed: {result:?}"),
            })
            .await;
            let first = registry
                .cancel_identity(
                    &context.identity,
                    &context.owner,
                    &context.operation_incarnation(),
                )
                .await
                .unwrap();
            assert_eq!(first.wait().await.unwrap_err().kind, kind);
            drop(execute);
            assert_eq!(
                attachment.operation_cleanup(&context),
                MediaOperationCleanup::Pending
            );
            assert_eq!(
                registry
                    .finish(&lease, true)
                    .await
                    .unwrap()
                    .wait()
                    .await
                    .unwrap_err()
                    .kind,
                kind
            );
            assert_eq!(
                registry
                    .cancel_identity(
                        &context.identity,
                        &context.owner,
                        &context.operation_incarnation()
                    )
                    .await
                    .unwrap()
                    .wait()
                    .await
                    .unwrap_err()
                    .kind,
                kind
            );
            let work = native
                .work
                .lock()
                .unwrap()
                .remove(&context.identity)
                .unwrap();
            if confirm {
                work.confirm();
            } else {
                drop(work);
            }
            assert_eq!(
                attachment.operation_cleanup(&context),
                if confirm {
                    MediaOperationCleanup::Confirmed
                } else {
                    MediaOperationCleanup::Unknown
                }
            );
            if confirm && kind == MediaErrorKind::Busy && allowed {
                tokio::time::timeout(Duration::from_secs(2), async {
                    while registry.context(&context.identity).await.is_some() {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("a late exact receipt must retire the entry without a client retry");
            }
            let retry = registry
                .cancel_identity(
                    &context.identity,
                    &context.owner,
                    &context.operation_incarnation(),
                )
                .await
                .unwrap();
            if confirm && kind == MediaErrorKind::Busy && allowed {
                retry.wait().await.unwrap();
                assert!(registry.context(&context.identity).await.is_none());
            } else {
                assert_eq!(retry.wait().await.unwrap_err().kind, kind);
                assert!(registry.context(&context.identity).await.is_some());
                assert!(registry.receipts.lock().unwrap().values.is_empty());
            }
            assert_eq!(native.cancels.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn completed_busy_after_operation_finish_observes_exact_cleanup() {
        use std::{future::Future, task::Poll};

        let native = Arc::new(BusyWitnessService {
            work: StdMutex::new(HashMap::new()),
            cancels: std::sync::atomic::AtomicUsize::new(0),
            cancel_kind: MediaErrorKind::Busy,
            cancel_started: Notify::new(),
            cancel_release: CancellationToken::new(),
            execute_release: CancellationToken::new(),
        });
        let host = MediaAttachmentHost::new();
        let attachment = host.attach(native.clone()).unwrap();
        let service = attachment.service();
        let context =
            MediaOperationContext::new(MediaOperationId::new(1001, 7), session_owner("late"));
        let registry = Arc::new(ActiveOperationRegistry::default());
        let lease = registry
            .register(
                context.clone(),
                service.clone(),
                Some(attachment.clone()),
                true,
            )
            .await
            .unwrap();
        let mut execute = Box::pin(service.execute(
            context.clone(),
            MediaOperation::Sources {
                kind: None,
                usage: None,
            },
        ));
        std::future::poll_fn(|cx| match execute.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(result) => panic!("test operation unexpectedly completed: {result:?}"),
        })
        .await;
        let flight = registry
            .cancel_identity(
                &context.identity,
                &context.owner,
                &context.operation_incarnation(),
            )
            .await
            .unwrap();
        native.cancel_started.notified().await;
        drop(execute);
        native
            .work
            .lock()
            .unwrap()
            .remove(&context.identity)
            .unwrap()
            .confirm();
        assert_eq!(
            attachment.operation_cleanup(&context),
            MediaOperationCleanup::Confirmed
        );
        let finish = registry.finish(&lease, true).await.unwrap();
        assert!(Arc::ptr_eq(&flight.incarnation, &finish.incarnation));
        assert!(matches!(
            finish.observe(Duration::from_millis(10)).await,
            CancellationObservation::TimedOut
        ));
        native.cancel_release.cancel();
        assert_eq!(flight.wait().await.unwrap_err().kind, MediaErrorKind::Busy);
        tokio::time::timeout(Duration::from_secs(1), async {
            while registry.context(&context.identity).await.is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the late Busy must be reconciled with the exact cleanup");
        registry
            .cancel_identity(
                &context.identity,
                &context.owner,
                &context.operation_incarnation(),
            )
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(native.cancels.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn witness_confirmed_between_pending_read_and_observer_start_is_not_lost() {
        use std::{future::Future, task::Poll};

        let native = Arc::new(BusyWitnessService {
            work: StdMutex::new(HashMap::new()),
            cancels: std::sync::atomic::AtomicUsize::new(0),
            cancel_kind: MediaErrorKind::Busy,
            cancel_started: Notify::new(),
            cancel_release: CancellationToken::new(),
            execute_release: CancellationToken::new(),
        });
        native.cancel_release.cancel();
        let host = MediaAttachmentHost::new();
        let attachment = host.attach(native.clone()).unwrap();
        let service = attachment.service();
        let context =
            MediaOperationContext::new(MediaOperationId::new(1002, 7), session_owner("race"));
        let registry = Arc::new(ActiveOperationRegistry::default());
        let lease = registry
            .register(
                context.clone(),
                service.clone(),
                Some(attachment.clone()),
                true,
            )
            .await
            .unwrap();
        let mut execute = Box::pin(service.execute(
            context.clone(),
            MediaOperation::Sources {
                kind: None,
                usage: None,
            },
        ));
        std::future::poll_fn(|cx| match execute.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(result) => panic!("test operation unexpectedly completed: {result:?}"),
        })
        .await;
        let flight = registry
            .cancel_identity(
                &context.identity,
                &context.owner,
                &context.operation_incarnation(),
            )
            .await
            .unwrap();
        assert_eq!(flight.wait().await.unwrap_err().kind, MediaErrorKind::Busy);
        drop(execute);
        assert_eq!(
            attachment.operation_cleanup(&context),
            MediaOperationCleanup::Pending
        );
        let fired = Arc::new(AtomicBool::new(false));
        let hook_fired = fired.clone();
        let hook_native = native.clone();
        let hook_identity = context.identity.clone();
        registry
            .entries
            .lock()
            .await
            .get_mut(&context.identity)
            .unwrap()
            .cleanup_read_test_hook = Some(Arc::new(move |status| {
            if status == MediaOperationCleanup::Pending {
                if let Some(work) = hook_native.work.lock().unwrap().remove(&hook_identity) {
                    hook_fired.store(true, Ordering::Release);
                    work.confirm();
                }
            }
        }));
        let _ = registry.finish(&lease, true).await;
        assert!(
            fired.load(Ordering::Acquire),
            "the exact witness must resolve immediately after a Pending read"
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while registry.context(&context.identity).await.is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the observer must recheck without any subsequent request");
        registry
            .cancel_identity(
                &context.identity,
                &context.owner,
                &context.operation_incarnation(),
            )
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(native.cancels.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn late_513_receipts_retire_while_unknown_stays_fenced_and_cache_is_bounded() {
        use std::{future::Future, task::Poll};

        let native = Arc::new(BusyWitnessService {
            work: StdMutex::new(HashMap::new()),
            cancels: std::sync::atomic::AtomicUsize::new(0),
            cancel_kind: MediaErrorKind::Busy,
            cancel_started: Notify::new(),
            cancel_release: CancellationToken::new(),
            execute_release: CancellationToken::new(),
        });
        native.cancel_release.cancel();
        let host = MediaAttachmentHost::new();
        let attachment = host.attach(native.clone()).unwrap();
        let service = attachment.service();
        let registry = Arc::new(ActiveOperationRegistry::default());
        let owner = session_owner("many");
        let mut contexts = Vec::new();
        // One permanently Unknown witness coexists with 513 confirmations.
        for generation in 0..514 {
            let context =
                MediaOperationContext::new(MediaOperationId::new(generation, 7), owner.clone());
            let lease = registry
                .register(
                    context.clone(),
                    service.clone(),
                    Some(attachment.clone()),
                    true,
                )
                .await
                .unwrap();
            let mut execute = Box::pin(service.execute(
                context.clone(),
                MediaOperation::Sources {
                    kind: None,
                    usage: None,
                },
            ));
            std::future::poll_fn(|cx| match execute.as_mut().poll(cx) {
                Poll::Pending => Poll::Ready(()),
                Poll::Ready(result) => panic!("test operation unexpectedly completed: {result:?}"),
            })
            .await;
            let flight = registry
                .cancel_identity(
                    &context.identity,
                    &context.owner,
                    &context.operation_incarnation(),
                )
                .await
                .unwrap();
            assert_eq!(flight.wait().await.unwrap_err().kind, MediaErrorKind::Busy);
            drop(execute);
            assert_eq!(
                registry
                    .finish(&lease, true)
                    .await
                    .unwrap()
                    .wait()
                    .await
                    .unwrap_err()
                    .kind,
                MediaErrorKind::Busy
            );
            assert_eq!(
                attachment.operation_cleanup(&context),
                MediaOperationCleanup::Pending
            );
            contexts.push(context);
        }
        assert_eq!(registry.entries.lock().await.len(), 514);
        let mut works = native.work.lock().unwrap();
        drop(works.remove(&contexts[0].identity).unwrap());
        for context in &contexts[1..] {
            works.remove(&context.identity).unwrap().confirm();
        }
        drop(works);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if registry.entries.lock().await.len() == 1
                    && !registry.cleanup_observer_running.load(Ordering::Acquire)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("late exact confirmations must retire without a new request");
        assert_eq!(
            attachment.operation_cleanup(&contexts[0]),
            MediaOperationCleanup::Unknown
        );
        assert_eq!(registry.receipts.lock().unwrap().values.len(), 512);
        let mut available = 0;
        let mut expired = None;
        let mut retained = None;
        for context in &contexts[1..] {
            match registry
                .cancel_identity(
                    &context.identity,
                    &context.owner,
                    &context.operation_incarnation(),
                )
                .await
            {
                Some(flight) => {
                    flight.wait().await.unwrap();
                    available += 1;
                    retained = Some(context.clone());
                }
                None => expired = Some(context.clone()),
            }
        }
        assert_eq!(available, 512);
        let retained = retained.unwrap();
        let active_replacement =
            MediaOperationContext::new(retained.identity.clone(), owner.clone());
        let active_lease = registry
            .register(
                active_replacement.clone(),
                service.clone(),
                Some(attachment.clone()),
                true,
            )
            .await
            .unwrap();
        registry
            .cancel_identity(
                &retained.identity,
                &retained.owner,
                &retained.operation_incarnation(),
            )
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(
            registry.context(&active_replacement.identity).await,
            Some(active_replacement)
        );
        assert_eq!(native.cancels.load(Ordering::SeqCst), 514);
        assert!(registry.finish(&active_lease, false).await.is_none());
        let expired = expired.expect("one confirmed receipt must have been evicted");
        let replacement = MediaOperationContext::new(expired.identity.clone(), owner);
        let replacement_lease = registry
            .register(replacement.clone(), service.clone(), Some(attachment), true)
            .await
            .unwrap();
        assert!(registry
            .cancel_identity(
                &expired.identity,
                &expired.owner,
                &expired.operation_incarnation()
            )
            .await
            .is_none());
        assert_eq!(native.cancels.load(Ordering::SeqCst), 514);
        assert!(registry.finish(&replacement_lease, false).await.is_none());
        assert!(
            registry
                .cancel_identity(
                    &replacement.identity,
                    &replacement.owner,
                    &replacement.operation_incarnation()
                )
                .await
                .is_none(),
            "a natural completion must not create a cancel receipt"
        );
    }

    #[tokio::test]
    async fn settled_cancel_error_keeps_fence_without_redispatch() {
        let owner = session_owner("11111111-1111-4111-8111-111111111111");
        let identity = MediaOperationId::new(63, 7);
        let old_context = MediaOperationContext::new(identity.clone(), owner)
            .with_timeout_budget_ms(1_000)
            .with_max_payload_bytes(1024);
        let registry = Arc::new(ActiveOperationRegistry::default());
        let service = Arc::new(FailingCancelService {
            cancel_attempts: std::sync::atomic::AtomicUsize::new(0),
            execute_attempts: std::sync::atomic::AtomicUsize::new(0),
        });
        let old = registry
            .register(old_context.clone(), service.clone(), None, false)
            .await
            .expect("the original operation should register");
        let failed = registry
            .cancel_lease(&old)
            .await
            .expect("the initial cancel should start");
        assert!(matches!(
            failed.observe(CANCEL_OBSERVATION_TIMEOUT).await,
            CancellationObservation::Completed(Err(_))
        ));
        assert_eq!(service.cancel_attempts.load(Ordering::Relaxed), 1);
        assert_eq!(registry.context(&identity).await, Some(old_context.clone()));

        let old_incarnation = old.context.operation_incarnation();
        let retry = registry
            .cancel_identity(&identity, &old_context.owner, &old_incarnation)
            .await
            .expect("a settled error remains the same unknown flight");
        assert!(Arc::ptr_eq(&failed.incarnation, &retry.incarnation));
        assert!(retry.wait().await.is_err());
        assert_eq!(service.cancel_attempts.load(Ordering::Relaxed), 1);
        registry.finish(&old, false).await;
        assert!(registry.context(&identity).await.is_some());
        assert!(registry
            .register(old_context, service.clone(), None, false)
            .await
            .is_err());
    }
}
