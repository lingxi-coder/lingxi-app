//! Host-private, connection-scoped media callbacks and owner-authenticated
//! MediaService requests over the existing Bridge connection.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use audio_provider::media_native::{schedule_native_media_stop, NativeMediaDeviceCallback};
use client::protocol::{
    events::ClientEvent,
    media::{
        MediaConnectionRequestDto, MediaConnectionResponseDto, MediaControlRequestDto,
        MediaControlResponseDto, MediaDeviceRequestDto, MediaDeviceRequestEnvelopeDto,
        MediaDeviceResponseDto, MediaDeviceResultDto, MediaErrorDto, MediaErrorKindDto,
    },
};
use media_api::{MediaError, MediaErrorKind};
use remote_control_client::{
    host_media::{RemoteMediaHostConfigProvider, RemoteMediaHostControlChannel},
    media::{RemoteIceServer, RemoteMediaConfig},
};
use tokio::sync::{oneshot, Mutex, OwnedSemaphorePermit, Semaphore};

use crate::audio_bridge::AudioRequestSink;

const DEVICE_CALLBACK_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PENDING_MEDIA_CALLBACKS: usize = 256;
const DEFAULT_MEDIA_GATHER_TIMEOUT_MS: u64 = 3_000;
const DEFAULT_MEDIA_CONNECT_TIMEOUT_MS: u64 = 15_000;

/// Desktop's frozen Remote Media transport settings. It mirrors the current
/// Electron Remote Control config contract: process-level STUN URLs with the
/// product defaults, no TURN credentials unless a future app config provider
/// injects them explicitly.
pub struct DesktopRemoteMediaConfigProvider {
    configuration: RemoteMediaConfig,
}

impl DesktopRemoteMediaConfigProvider {
    /// Snapshot the existing Remote Control STUN environment setting once at
    /// app composition. No credentials or signaling values enter the generic
    /// media API or Bridge DTOs.
    #[must_use]
    pub fn from_process_environment() -> Self {
        let defaults = remote_control_client::version::DEFAULT_STUN_URLS
            .iter()
            .map(|url| (*url).to_owned())
            .collect::<Vec<_>>();
        let urls = std::env::var("LINGXI_REMOTE_STUN")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|url| is_remote_stun_url(url))
                    .take(8)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter(|urls| !urls.is_empty())
            .unwrap_or(defaults);
        let configuration = RemoteMediaConfig {
            ice_servers: urls
                .into_iter()
                .map(|url| RemoteIceServer {
                    urls: vec![url],
                    username: None,
                    credential: None,
                })
                .collect(),
            relay_only: false,
            gather_timeout_ms: DEFAULT_MEDIA_GATHER_TIMEOUT_MS,
            connect_timeout_ms: DEFAULT_MEDIA_CONNECT_TIMEOUT_MS,
            max_payload_bytes: remote_control_client::media::MEDIA_PAYLOAD_MAX_BYTES,
        };
        Self { configuration }
    }
}

fn is_remote_stun_url(value: &str) -> bool {
    let Some((scheme, authority)) = value.split_once(':') else {
        return false;
    };
    matches!(scheme, "stun" | "stuns")
        && (1..=250).contains(&authority.len())
        && authority
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:%[]-".contains(&byte))
}

#[async_trait::async_trait]
impl RemoteMediaHostConfigProvider for DesktopRemoteMediaConfigProvider {
    async fn resolve(
        &self,
        context: &media_api::MediaOperationContext,
        _route_id: &str,
        _request: &media_api::MediaSessionRequest,
    ) -> Result<RemoteMediaConfig, MediaError> {
        if !matches!(context.owner, media_api::MediaOwner::Session { .. }) {
            return Err(MediaError::new(
                MediaErrorKind::InvalidRequest,
                "Remote Control media requires an authenticated session owner",
            ));
        }
        Ok(self.configuration.clone())
    }
}

struct PendingRequest {
    response: Option<oneshot::Sender<DeviceResponse>>,
    is_open: bool,
    open_permit: Option<OwnedSemaphorePermit>,
    open_generation: Option<Arc<Semaphore>>,
    token: Arc<()>,
}

type Pending = Arc<StdMutex<HashMap<String, PendingRequest>>>;
type OpenSlots = Arc<StdMutex<Arc<Semaphore>>>;

// The response may be sent successfully just as its receiver is dropped. An
// owned delivery closes an unclaimed open in both that race and the ordinary
// late-response case.
struct DeviceResponse {
    response: Option<MediaDeviceResponseDto>,
    cleanup: Option<Arc<BridgeCleanupCallback>>,
    open_permit: Option<OwnedSemaphorePermit>,
}

impl DeviceResponse {
    fn into_response(mut self) -> MediaDeviceResponseDto {
        self.cleanup = None;
        drop(self.open_permit.take());
        self.response.take().expect("device response is present")
    }
}

impl Drop for DeviceResponse {
    fn drop(&mut self) {
        match (
            self.cleanup.take(),
            self.response.take(),
            self.open_permit.take(),
        ) {
            (
                Some(callback),
                Some(MediaDeviceResponseDto {
                    result: MediaDeviceResultDto::Opened { lease },
                    ..
                }),
                Some(permit),
            ) => schedule_native_media_stop(callback, lease.lease_id, permit),
            (
                Some(_),
                Some(MediaDeviceResponseDto {
                    result: MediaDeviceResultDto::Opened { .. },
                    ..
                }),
                None,
            ) => tracing::error!("abandoned Bridge open lost its cleanup reservation"),
            (
                None,
                Some(MediaDeviceResponseDto {
                    result: MediaDeviceResultDto::Opened { .. },
                    ..
                }),
                Some(permit),
            ) => {
                permit.forget();
                tracing::error!("abandoned Bridge open lost its connection generation");
            }
            _ => {}
        }
    }
}

struct PendingDeviceGuard {
    pending: Pending,
    request_id: String,
    token: Arc<()>,
}

impl Drop for PendingDeviceGuard {
    fn drop(&mut self) {
        let mut pending = self.pending.lock().unwrap();
        if let Some(request) = pending
            .get_mut(&self.request_id)
            .filter(|request| Arc::ptr_eq(&request.token, &self.token))
        {
            if request.is_open {
                drop(request.response.take());
            } else {
                pending.remove(&self.request_id);
            }
        }
    }
}

/// Product callback that forwards native device lease operations to the
/// authenticated Electron Bridge client.
#[derive(Clone)]
pub struct BridgeMediaDeviceCallback {
    sink: Arc<dyn AudioRequestSink>,
    pending: Pending,
    open_slots: OpenSlots,
    send_gate: Arc<Mutex<()>>,
}

struct BridgeBoundCallback {
    callback: BridgeMediaDeviceCallback,
    generation: Arc<Semaphore>,
}

struct BridgeCleanupCallback {
    callback: BridgeMediaDeviceCallback,
    generation: Arc<Semaphore>,
}

/// Response side bound to the exact Bridge connection that sent requests.
#[derive(Clone)]
pub struct MediaDeviceResponder {
    pending: Pending,
    sink: Arc<dyn AudioRequestSink>,
    open_slots: OpenSlots,
    send_gate: Arc<Mutex<()>>,
}

struct PendingConnection {
    request: MediaConnectionRequestDto,
    response: oneshot::Sender<MediaConnectionResponseDto>,
}

type PendingConnections = Arc<Mutex<HashMap<String, PendingConnection>>>;

/// Connection-scoped channel used by configured media connectors to request a
/// peer offer answer from the authenticated Electron host adapter.
pub struct BridgeMediaConnectionChannel {
    sink: Arc<dyn AudioRequestSink>,
    pending: PendingConnections,
}

/// Correlator for incoming Electron host-control answers.
#[derive(Clone)]
pub struct MediaConnectionResponder {
    pending: PendingConnections,
}

struct PendingControl {
    request: MediaControlRequestDto,
    response: oneshot::Sender<MediaControlResponseDto>,
}

type PendingControls = Arc<Mutex<HashMap<String, PendingControl>>>;

struct PendingMapGuard<T: Send + 'static> {
    pending: Arc<Mutex<HashMap<String, T>>>,
    request_id: String,
}

impl<T: Send + 'static> PendingMapGuard<T> {
    fn new(pending: Arc<Mutex<HashMap<String, T>>>, request_id: String) -> Self {
        Self {
            pending,
            request_id,
        }
    }
}

impl<T: Send + 'static> Drop for PendingMapGuard<T> {
    fn drop(&mut self) {
        let pending = self.pending.clone();
        let request_id = self.request_id.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                pending.lock().await.remove(&request_id);
            });
        }
    }
}

/// Connection-scoped channel for flushing or interrupting one remote peer's
/// output while preserving its microphone and session lifetime.
pub struct BridgeMediaControlChannel {
    sink: Arc<dyn AudioRequestSink>,
    pending: PendingControls,
}

/// Per-route adapter over this Bridge connection's authenticated host-only
/// offer and playback-control channels. Every outgoing message is pinned to
/// the exact opaque route ID selected by registration.
pub struct BridgeMediaHostControlChannel {
    connection_id: String,
    connection: Arc<BridgeMediaConnectionChannel>,
    control: Arc<BridgeMediaControlChannel>,
}

/// Correlator for inbound native playback barrier receipts.
#[derive(Clone)]
pub struct MediaControlResponder {
    pending: PendingControls,
}

/// Create a device callback and its response correlator for one connection.
#[must_use]
pub fn new_media_device_callback(
    sink: Arc<dyn AudioRequestSink>,
) -> (Arc<BridgeMediaDeviceCallback>, MediaDeviceResponder) {
    let pending = Arc::new(StdMutex::new(HashMap::new()));
    let open_slots = Arc::new(StdMutex::new(Arc::new(Semaphore::new(
        MAX_PENDING_MEDIA_CALLBACKS,
    ))));
    let send_gate = Arc::new(Mutex::new(()));
    (
        Arc::new(BridgeMediaDeviceCallback {
            sink: sink.clone(),
            pending: pending.clone(),
            open_slots: open_slots.clone(),
            send_gate: send_gate.clone(),
        }),
        MediaDeviceResponder {
            pending,
            sink,
            open_slots,
            send_gate,
        },
    )
}

/// Create one peer negotiation request channel for a Bridge connection.
#[must_use]
pub fn new_media_connection_channel(
    sink: Arc<dyn AudioRequestSink>,
) -> (Arc<BridgeMediaConnectionChannel>, MediaConnectionResponder) {
    let pending = Arc::new(Mutex::new(HashMap::new()));
    (
        Arc::new(BridgeMediaConnectionChannel {
            sink,
            pending: pending.clone(),
        }),
        MediaConnectionResponder { pending },
    )
}

/// Create one peer output-barrier channel for a Bridge connection.
#[must_use]
pub fn new_media_control_channel(
    sink: Arc<dyn AudioRequestSink>,
) -> (Arc<BridgeMediaControlChannel>, MediaControlResponder) {
    let pending = Arc::new(Mutex::new(HashMap::new()));
    (
        Arc::new(BridgeMediaControlChannel {
            sink,
            pending: pending.clone(),
        }),
        MediaControlResponder { pending },
    )
}

impl BridgeMediaHostControlChannel {
    /// Bind both host-only transports to one registered route.
    #[must_use]
    pub fn for_route(
        connection_id: impl Into<String>,
        connection: Arc<BridgeMediaConnectionChannel>,
        control: Arc<BridgeMediaControlChannel>,
    ) -> Arc<Self> {
        Arc::new(Self {
            connection_id: connection_id.into(),
            connection,
            control,
        })
    }

    fn wrong_route() -> MediaError {
        MediaError::new(
            MediaErrorKind::InvalidRequest,
            "media control request does not match its registered route",
        )
    }
}

#[async_trait::async_trait]
impl RemoteMediaHostControlChannel for BridgeMediaHostControlChannel {
    async fn negotiate(
        &self,
        request: MediaConnectionRequestDto,
    ) -> Result<MediaConnectionResponseDto, MediaError> {
        if request.connection_id != self.connection_id {
            return Err(Self::wrong_route());
        }
        self.connection.request(request).await.map_err(|_| {
            MediaError::new(
                MediaErrorKind::Unavailable,
                "media offer channel is unavailable",
            )
        })
    }

    async fn control(
        &self,
        request: MediaControlRequestDto,
    ) -> Result<MediaControlResponseDto, MediaError> {
        if request.connection_id != self.connection_id {
            return Err(Self::wrong_route());
        }
        self.control.request(request).await.map_err(|_| {
            MediaError::new(
                MediaErrorKind::Unavailable,
                "media control channel is unavailable",
            )
        })
    }
}

impl MediaDeviceResponder {
    /// Resolve only the pending request with the exact request ID.
    pub async fn resolve(&self, response: MediaDeviceResponseDto) -> bool {
        if response.validate().is_err() {
            return false;
        }
        let pending = {
            let mut pending = self.pending.lock().unwrap();
            pending.remove(&response.request_id)
        };
        let Some(pending) = pending else {
            return false;
        };
        let PendingRequest {
            response: sender,
            is_open,
            open_permit,
            open_generation,
            token: _,
        } = pending;
        let delivery = DeviceResponse {
            response: Some(response),
            cleanup: if is_open {
                open_generation.map(|generation| {
                    Arc::new(BridgeCleanupCallback {
                        callback: BridgeMediaDeviceCallback {
                            sink: self.sink.clone(),
                            pending: self.pending.clone(),
                            open_slots: self.open_slots.clone(),
                            send_gate: self.send_gate.clone(),
                        },
                        generation,
                    })
                })
            } else {
                None
            },
            open_permit,
        };
        if let Some(sender) = sender {
            // A failed send drops the guarded response. A successful send may
            // also be dropped by the receiver before it is polled.
            let _ = sender.send(delivery);
        }
        true
    }

    /// Drop pending calls when the authenticated Bridge client disconnects.
    pub async fn drain(&self) -> usize {
        let _send_guard = self.send_gate.lock().await;
        let mut pending = self.pending.lock().unwrap();
        let count = pending.len();
        pending.clear();
        *self.open_slots.lock().unwrap() = Arc::new(Semaphore::new(MAX_PENDING_MEDIA_CALLBACKS));
        count
    }

    /// Number of callbacks waiting on a native response.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}

impl MediaConnectionResponder {
    /// Resolve only the request with the exact offer and session correlation.
    pub async fn resolve(&self, response: MediaConnectionResponseDto) -> bool {
        if response.validate().is_err() {
            return false;
        }
        let pending = {
            let mut pending = self.pending.lock().await;
            let Some(entry) = pending.get(&response.request_id) else {
                return false;
            };
            if !response.matches_request(&entry.request) {
                return false;
            }
            pending.remove(&response.request_id)
        };
        let Some(pending) = pending else {
            return false;
        };
        pending.response.send(response).is_ok()
    }

    /// Drop pending peer negotiations when the Bridge connection closes.
    pub async fn drain(&self) -> usize {
        let mut pending = self.pending.lock().await;
        let count = pending.len();
        pending.clear();
        count
    }
}

impl MediaControlResponder {
    /// Resolve only the exact session connection and generation.
    pub async fn resolve(&self, response: MediaControlResponseDto) -> bool {
        if response.validate().is_err() {
            return false;
        }
        let pending = {
            let mut pending = self.pending.lock().await;
            let Some(entry) = pending.get(&response.request_id) else {
                return false;
            };
            if !response.matches_request(&entry.request) {
                return false;
            }
            pending.remove(&response.request_id)
        };
        let Some(pending) = pending else {
            return false;
        };
        pending.response.send(response).is_ok()
    }

    /// Drop pending output barriers when the Bridge connection closes.
    pub async fn drain(&self) -> usize {
        let mut pending = self.pending.lock().await;
        let count = pending.len();
        pending.clear();
        count
    }
}

impl BridgeMediaConnectionChannel {
    /// Send a bounded offer to the exact authenticated host route and await its
    /// correlated answer. This lane is never an Agent/model message.
    pub async fn request(
        &self,
        request: MediaConnectionRequestDto,
    ) -> Result<MediaConnectionResponseDto, String> {
        request.validate()?;
        let request_id = request.request_id.clone();
        let expected = request.clone();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.contains_key(&request_id) {
                return Err("media connection request identity is already active".into());
            }
            if pending.len() >= MAX_PENDING_MEDIA_CALLBACKS {
                return Err("media connection callback capacity is full".into());
            }
            pending.insert(
                request_id.clone(),
                PendingConnection {
                    request: request.clone(),
                    response: tx,
                },
            );
        }
        let _pending_guard = PendingMapGuard::new(self.pending.clone(), request_id.clone());
        if !self
            .sink
            .emit_request(ClientEvent::MediaConnectionRequest { request })
            .await
        {
            self.pending.lock().await.remove(&request_id);
            return Err("authenticated media host adapter is not attached".into());
        }
        match tokio::time::timeout(DEVICE_CALLBACK_TIMEOUT, rx).await {
            Ok(Ok(response)) if response.matches_request(&expected) => Ok(response),
            Ok(Ok(_)) => {
                Err("media connection host response did not match the pending offer".into())
            }
            Ok(Err(_)) => Err("Bridge connection ended before the media answer arrived".into()),
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                Err("media connection offer exceeded its response deadline".into())
            }
        }
    }
}

impl BridgeMediaControlChannel {
    /// Request a precise flush or interruption and return the actual native
    /// render receipt from that owner-bound connection.
    pub async fn request(
        &self,
        request: MediaControlRequestDto,
    ) -> Result<MediaControlResponseDto, String> {
        request.validate()?;
        let request_id = request.request_id.clone();
        let expected = request.clone();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.contains_key(&request_id) {
                return Err("media control request identity is already active".into());
            }
            if pending.len() >= MAX_PENDING_MEDIA_CALLBACKS {
                return Err("media control callback capacity is full".into());
            }
            pending.insert(
                request_id.clone(),
                PendingControl {
                    request: request.clone(),
                    response: tx,
                },
            );
        }
        let _pending_guard = PendingMapGuard::new(self.pending.clone(), request_id.clone());
        if !self
            .sink
            .emit_request(ClientEvent::MediaControlRequest { request })
            .await
        {
            self.pending.lock().await.remove(&request_id);
            return Err("authenticated media host adapter is not attached".into());
        }
        match tokio::time::timeout(DEVICE_CALLBACK_TIMEOUT, rx).await {
            Ok(Ok(response))
                if response.validate().is_ok() && response.matches_request(&expected) =>
            {
                Ok(response)
            }
            Ok(Ok(_)) => Err("media control response is invalid".into()),
            Ok(Err(_)) => Err("Bridge connection ended before media playback completed".into()),
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                Err("media output barrier exceeded its response deadline".into())
            }
        }
    }
}

impl BridgeMediaDeviceCallback {
    async fn request_with_timeout(
        &self,
        request: MediaDeviceRequestEnvelopeDto,
        timeout: Duration,
    ) -> MediaDeviceResponseDto {
        self.request_with_capacity(request, timeout, None, false)
            .await
    }

    async fn request_with_capacity(
        &self,
        request: MediaDeviceRequestEnvelopeDto,
        timeout: Duration,
        expected_generation: Option<&Arc<Semaphore>>,
        cleanup: bool,
    ) -> MediaDeviceResponseDto {
        if let Err(message) = request.validate() {
            return device_error(
                request.request_id,
                MediaErrorKindDto::InvalidRequest,
                message,
            );
        }
        let is_open = matches!(&request.request, MediaDeviceRequestDto::Open { .. });
        let is_stop = matches!(&request.request, MediaDeviceRequestDto::Stop { .. });
        let request_id = request.request_id.clone();
        let (tx, rx) = oneshot::channel();
        // Closing a socket takes this gate before advancing its generation, so
        // an old lease cleanup cannot be sent to a newly attached client.
        let send_guard = self.send_gate.lock().await;
        let generation = self.open_slots.lock().unwrap().clone();
        if expected_generation.is_some_and(|expected| !Arc::ptr_eq(expected, &generation)) {
            return device_error(
                request_id,
                MediaErrorKindDto::Unavailable,
                "native media connection was replaced before the lease operation".into(),
            );
        }
        let token = Arc::new(());
        {
            let mut pending = self.pending.lock().unwrap();
            if pending.contains_key(&request_id) {
                return device_error(
                    request_id,
                    MediaErrorKindDto::InvalidRequest,
                    "media device request identity is already active".into(),
                );
            }
            // Reserve capacity for stops when abandoned opens occupy the
            // normal callback capacity.
            let limit = if cleanup && is_stop {
                MAX_PENDING_MEDIA_CALLBACKS * 3
            } else if is_stop {
                MAX_PENDING_MEDIA_CALLBACKS * 2
            } else {
                MAX_PENDING_MEDIA_CALLBACKS
            };
            if pending.len() >= limit {
                return device_error(
                    request_id,
                    MediaErrorKindDto::Busy,
                    "native media callback capacity is full".into(),
                );
            }
            let open_permit = if is_open {
                match generation.clone().try_acquire_owned() {
                    Ok(permit) => Some(permit),
                    Err(_) => {
                        return device_error(
                            request_id,
                            MediaErrorKindDto::Busy,
                            "native media open cleanup capacity is full".into(),
                        );
                    }
                }
            } else {
                None
            };
            pending.insert(
                request_id.clone(),
                PendingRequest {
                    response: Some(tx),
                    is_open,
                    open_permit,
                    open_generation: is_open.then_some(generation),
                    token: token.clone(),
                },
            );
        }
        let _pending_guard = PendingDeviceGuard {
            pending: self.pending.clone(),
            request_id: request_id.clone(),
            token,
        };
        let emitted = self
            .sink
            .emit_request(ClientEvent::MediaDeviceRequest { request })
            .await;
        if !emitted {
            self.pending.lock().unwrap().remove(&request_id);
            return device_error(
                request_id,
                MediaErrorKindDto::Unavailable,
                "the native device host is not attached".into(),
            );
        }
        drop(send_guard);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => response.into_response(),
            Ok(Err(_)) => device_error(
                request_id,
                MediaErrorKindDto::Unavailable,
                "the Bridge connection ended before the native media response arrived".into(),
            ),
            Err(_) => device_error(
                request_id,
                MediaErrorKindDto::Timeout,
                "the native media callback exceeded its response deadline".into(),
            ),
        }
    }
}

#[async_trait::async_trait]
impl NativeMediaDeviceCallback for BridgeMediaDeviceCallback {
    fn bind_for_open(&self) -> Option<Arc<dyn NativeMediaDeviceCallback>> {
        let generation = self.open_slots.lock().unwrap().clone();
        Some(Arc::new(BridgeBoundCallback {
            callback: self.clone(),
            generation,
        }))
    }

    async fn request(&self, request: MediaDeviceRequestEnvelopeDto) -> MediaDeviceResponseDto {
        self.request_with_timeout(request, DEVICE_CALLBACK_TIMEOUT)
            .await
    }
}

#[async_trait::async_trait]
impl NativeMediaDeviceCallback for BridgeBoundCallback {
    async fn request(&self, request: MediaDeviceRequestEnvelopeDto) -> MediaDeviceResponseDto {
        self.callback
            .request_with_capacity(
                request,
                DEVICE_CALLBACK_TIMEOUT,
                Some(&self.generation),
                false,
            )
            .await
    }
}

#[async_trait::async_trait]
impl NativeMediaDeviceCallback for BridgeCleanupCallback {
    async fn request(&self, request: MediaDeviceRequestEnvelopeDto) -> MediaDeviceResponseDto {
        self.callback
            .request_with_capacity(
                request,
                DEVICE_CALLBACK_TIMEOUT,
                Some(&self.generation),
                true,
            )
            .await
    }
}

fn device_error(
    request_id: String,
    kind: MediaErrorKindDto,
    message: String,
) -> MediaDeviceResponseDto {
    MediaDeviceResponseDto {
        request_id,
        result: MediaDeviceResultDto::Error {
            error: MediaErrorDto { kind, message },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::protocol::media::{
        MediaDeviceLeaseDto, MediaDeviceOpenRequestDto, MediaOperationIdDto, MediaOwnerDto,
        MediaSessionIntentDto, MediaSessionModeDto, MediaSessionRequestDto, MediaSourceRefDto,
    };
    use media_api::{
        MediaDevicePorts, MediaOperationContext, MediaOperationId, MediaOwner, MediaSessionIntent,
        MediaSessionMode, MediaSessionRequest, MediaSourceId, MediaSourceRef,
    };
    use tokio::sync::mpsc;

    struct RecordingSink(mpsc::UnboundedSender<MediaDeviceRequestEnvelopeDto>);

    #[async_trait::async_trait]
    impl AudioRequestSink for RecordingSink {
        async fn emit_request(&self, event: ClientEvent) -> bool {
            match event {
                ClientEvent::MediaDeviceRequest { request } => self.0.send(request).is_ok(),
                _ => false,
            }
        }
    }

    fn fixture() -> (
        Arc<BridgeMediaDeviceCallback>,
        MediaDeviceResponder,
        mpsc::UnboundedReceiver<MediaDeviceRequestEnvelopeDto>,
    ) {
        let (events, receiver) = mpsc::unbounded_channel();
        let (callback, responder) = new_media_device_callback(Arc::new(RecordingSink(events)));
        (callback, responder, receiver)
    }

    fn open_slots(callback: &BridgeMediaDeviceCallback) -> Arc<Semaphore> {
        callback.open_slots.lock().unwrap().clone()
    }

    fn pending_open(
        generation: &Arc<Semaphore>,
        response: Option<oneshot::Sender<DeviceResponse>>,
    ) -> PendingRequest {
        PendingRequest {
            response,
            is_open: true,
            open_permit: Some(generation.clone().try_acquire_owned().unwrap()),
            open_generation: Some(generation.clone()),
            token: Arc::new(()),
        }
    }

    fn open_request(request_id: &str) -> MediaDeviceRequestEnvelopeDto {
        MediaDeviceRequestEnvelopeDto {
            request_id: request_id.into(),
            request: MediaDeviceRequestDto::Open {
                request: MediaDeviceOpenRequestDto {
                    identity: MediaOperationIdDto {
                        id: "operation".into(),
                        generation: 1,
                        service_epoch: 1,
                    },
                    owner: MediaOwnerDto::Session {
                        session_id: "session".into(),
                    },
                    initiator: None,
                    timeout_budget_ms: Some(1_000),
                    max_payload_bytes: 1_024,
                    request: MediaSessionRequestDto {
                        mode: MediaSessionModeDto::Audio,
                        intent: MediaSessionIntentDto::Observation,
                        audio_source: Some(MediaSourceRefDto {
                            id: "media-runtime:microphone".into(),
                        }),
                        video_source: None,
                    },
                },
            },
        }
    }

    fn opened(request_id: &str) -> MediaDeviceResponseDto {
        MediaDeviceResponseDto {
            request_id: request_id.into(),
            result: MediaDeviceResultDto::Opened {
                lease: MediaDeviceLeaseDto {
                    lease_id: "late-native-lease".into(),
                },
            },
        }
    }

    async fn acknowledge_cleanup(
        responder: &MediaDeviceResponder,
        events: &mut mpsc::UnboundedReceiver<MediaDeviceRequestEnvelopeDto>,
    ) {
        let request = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            request.request,
            MediaDeviceRequestDto::Stop { ref lease_id } if lease_id == "late-native-lease"
        ));
        assert!(
            responder
                .resolve(MediaDeviceResponseDto {
                    request_id: request.request_id,
                    result: MediaDeviceResultDto::Stopped,
                })
                .await
        );
    }

    #[tokio::test]
    async fn bridge_stops_opened_response_after_its_callback_timeout() {
        let (callback, responder, mut events) = fixture();
        let request = open_request("timed-out-open");
        let open = tokio::spawn(async move {
            callback
                .request_with_timeout(request, Duration::from_millis(25))
                .await
        });
        assert!(matches!(
            events.recv().await.unwrap().request,
            MediaDeviceRequestDto::Open { .. }
        ));
        assert!(matches!(
            open.await.unwrap().result,
            MediaDeviceResultDto::Error {
                error: MediaErrorDto {
                    kind: MediaErrorKindDto::Timeout,
                    ..
                }
            }
        ));
        assert_eq!(responder.pending_count().await, 1);
        assert!(responder.resolve(opened("timed-out-open")).await);
        acknowledge_cleanup(&responder, &mut events).await;
        assert_eq!(responder.pending_count().await, 0);
    }

    #[tokio::test]
    async fn bridge_stops_a_delivered_open_response_dropped_before_polling() {
        let (callback, responder, mut events) = fixture();
        let (sender, receiver) = oneshot::channel();
        let slots = open_slots(&callback);
        callback
            .pending
            .lock()
            .unwrap()
            .insert("unpolled-open".into(), pending_open(&slots, Some(sender)));
        assert!(responder.resolve(opened("unpolled-open")).await);
        drop(receiver);
        acknowledge_cleanup(&responder, &mut events).await;
        assert_eq!(responder.pending_count().await, 0);
    }

    #[tokio::test]
    async fn disconnect_resets_quarantined_open_capacity() {
        let (callback, responder, _) = fixture();
        let old_slots = open_slots(&callback);
        let weak = Arc::downgrade(&old_slots);
        old_slots.clone().try_acquire_owned().unwrap().forget();
        drop(old_slots);
        assert_eq!(responder.drain().await, 0);
        assert!(weak.upgrade().is_none());
        assert_eq!(
            open_slots(&callback).available_permits(),
            MAX_PENDING_MEDIA_CALLBACKS
        );
    }

    #[tokio::test]
    async fn late_delivery_after_disconnect_cannot_stop_a_new_connections_lease() {
        let (callback, responder, mut events) = fixture();
        let (sender, receiver) = oneshot::channel();
        let old_slots = open_slots(&callback);
        let weak = Arc::downgrade(&old_slots);
        callback
            .pending
            .lock()
            .unwrap()
            .insert("old-open".into(), pending_open(&old_slots, Some(sender)));
        drop(old_slots);
        assert!(responder.resolve(opened("old-open")).await);
        assert_eq!(responder.drain().await, 0);
        drop(receiver);
        tokio::time::timeout(Duration::from_secs(2), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn opened_session_operations_are_fenced_from_a_replacement_connection() {
        let (callback, responder, mut events) = fixture();
        let old_slots = open_slots(&callback);
        let weak = Arc::downgrade(&old_slots);
        drop(old_slots);
        let ports = Arc::new(audio_provider::media_native::CallbackMediaDevicePorts::new(
            callback,
        ));
        let open = tokio::spawn(async move {
            let context = MediaOperationContext::new(
                MediaOperationId::new(1, 1),
                MediaOwner::Session {
                    session_id: "session".into(),
                },
            );
            let request = MediaSessionRequest {
                mode: MediaSessionMode::Audio,
                intent: MediaSessionIntent::Observation,
                audio_source: Some(MediaSourceRef(MediaSourceId::from(
                    "media-runtime:microphone",
                ))),
                video_source: None,
            };
            ports.open(context, request).await
        });
        let request = events.recv().await.unwrap();
        assert!(responder.resolve(opened(&request.request_id)).await);
        let session = open.await.unwrap().unwrap();
        assert_eq!(responder.drain().await, 0);
        assert_eq!(
            session.next_input().await.unwrap_err().kind,
            MediaErrorKind::Unavailable
        );
        assert_eq!(
            session.stop().await.unwrap_err().kind,
            MediaErrorKind::Unavailable
        );
        drop(session);
        tokio::time::timeout(Duration::from_secs(2), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn old_request_guard_cannot_remove_a_new_generation_request() {
        let (callback, responder, _) = fixture();
        let old_token = Arc::new(());
        let old_guard = PendingDeviceGuard {
            pending: callback.pending.clone(),
            request_id: "reused-request".into(),
            token: old_token,
        };
        assert_eq!(responder.drain().await, 0);
        let slots = open_slots(&callback);
        let (sender, _receiver) = oneshot::channel();
        callback
            .pending
            .lock()
            .unwrap()
            .insert("reused-request".into(), pending_open(&slots, Some(sender)));
        drop(old_guard);
        assert!(callback
            .pending
            .lock()
            .unwrap()
            .get("reused-request")
            .unwrap()
            .response
            .is_some());
        assert_eq!(responder.drain().await, 1);
    }

    #[tokio::test]
    async fn abandoned_open_correlations_are_bounded_but_stops_keep_capacity() {
        let (callback, responder, mut events) = fixture();
        let slots = open_slots(&callback);
        {
            let mut pending = callback.pending.lock().unwrap();
            for index in 0..MAX_PENDING_MEDIA_CALLBACKS {
                pending.insert(format!("abandoned-{index}"), pending_open(&slots, None));
            }
        }
        let response = callback
            .request_with_timeout(open_request("new-open"), Duration::from_secs(1))
            .await;
        assert!(matches!(
            response.result,
            MediaDeviceResultDto::Error {
                error: MediaErrorDto {
                    kind: MediaErrorKindDto::Busy,
                    ..
                }
            }
        ));
        let stop = tokio::spawn(async move {
            callback
                .request_with_timeout(
                    MediaDeviceRequestEnvelopeDto {
                        request_id: "stop".into(),
                        request: MediaDeviceRequestDto::Stop {
                            lease_id: "lease".into(),
                        },
                    },
                    Duration::from_secs(1),
                )
                .await
        });
        let request = events.recv().await.unwrap();
        assert!(matches!(
            request.request,
            MediaDeviceRequestDto::Stop { .. }
        ));
        assert!(
            responder
                .resolve(MediaDeviceResponseDto {
                    request_id: request.request_id,
                    result: MediaDeviceResultDto::Stopped,
                })
                .await
        );
        assert!(matches!(
            stop.await.unwrap().result,
            MediaDeviceResultDto::Stopped
        ));
        assert_eq!(responder.drain().await, MAX_PENDING_MEDIA_CALLBACKS);
    }

    #[tokio::test]
    async fn bridge_open_slot_remains_reserved_during_late_cleanup() {
        let (callback, responder, mut events) = fixture();
        let slots = open_slots(&callback);
        let mut held = Vec::new();
        for _ in 1..MAX_PENDING_MEDIA_CALLBACKS {
            held.push(slots.clone().try_acquire_owned().unwrap());
        }
        callback
            .pending
            .lock()
            .unwrap()
            .insert("late-open".into(), pending_open(&slots, None));
        assert!(responder.resolve(opened("late-open")).await);
        let cleanup = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            cleanup.request,
            MediaDeviceRequestDto::Stop { .. }
        ));
        let response = callback
            .request_with_timeout(open_request("capacity-test"), Duration::from_secs(1))
            .await;
        assert!(matches!(
            response.result,
            MediaDeviceResultDto::Error {
                error: MediaErrorDto {
                    kind: MediaErrorKindDto::Busy,
                    ..
                }
            }
        ));
        assert!(
            responder
                .resolve(MediaDeviceResponseDto {
                    request_id: cleanup.request_id,
                    result: MediaDeviceResultDto::Stopped,
                })
                .await
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while slots.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(held);
        assert_eq!(slots.available_permits(), MAX_PENDING_MEDIA_CALLBACKS);
    }

    #[tokio::test]
    async fn late_cleanup_has_capacity_even_when_regular_stops_are_saturated() {
        let (callback, responder, mut events) = fixture();
        let slots = open_slots(&callback);
        let mut receivers = Vec::new();
        {
            let mut pending = callback.pending.lock().unwrap();
            for index in 0..MAX_PENDING_MEDIA_CALLBACKS - 1 {
                pending.insert(format!("open-{index}"), pending_open(&slots, None));
            }
            // Normal stops can occupy the second capacity band. A previous
            // cleanup can already occupy the independently reserved band.
            for index in 0..MAX_PENDING_MEDIA_CALLBACKS + 2 {
                let (sender, receiver) = oneshot::channel();
                receivers.push(receiver);
                pending.insert(
                    format!("stop-{index}"),
                    PendingRequest {
                        response: Some(sender),
                        is_open: false,
                        open_permit: None,
                        open_generation: None,
                        token: Arc::new(()),
                    },
                );
            }
        }
        let prior_cleanup_permit = slots.clone().try_acquire_owned().unwrap();
        assert!(responder.resolve(opened("open-0")).await);
        acknowledge_cleanup(&responder, &mut events).await;
        assert_eq!(responder.drain().await, MAX_PENDING_MEDIA_CALLBACKS * 2);
        drop(prior_cleanup_permit);
        drop(receivers);
    }
}
