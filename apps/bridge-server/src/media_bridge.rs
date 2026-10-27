//! Host-private, connection-scoped media callbacks and owner-authenticated
//! MediaService requests over the existing Bridge connection.

use std::{collections::HashMap, sync::Arc, time::Duration};

use audio_provider::media_native::NativeMediaDeviceCallback;
use client::protocol::{
    events::ClientEvent,
    media::{
        MediaConnectionRequestDto, MediaConnectionResponseDto, MediaControlRequestDto,
        MediaControlResponseDto, MediaDeviceRequestEnvelopeDto, MediaDeviceResponseDto,
        MediaDeviceResultDto, MediaErrorDto, MediaErrorKindDto,
    },
};
use media_api::{MediaError, MediaErrorKind};
use remote_control_client::{
    host_media::{RemoteMediaHostConfigProvider, RemoteMediaHostControlChannel},
    media::{RemoteIceServer, RemoteMediaConfig},
};
use tokio::sync::{oneshot, Mutex};

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
    response: oneshot::Sender<MediaDeviceResponseDto>,
}

type Pending = Arc<Mutex<HashMap<String, PendingRequest>>>;

/// Product callback that forwards native device lease operations to the
/// authenticated Electron Bridge client.
pub struct BridgeMediaDeviceCallback {
    sink: Arc<dyn AudioRequestSink>,
    pending: Pending,
}

/// Response side bound to the exact Bridge connection that sent requests.
#[derive(Clone)]
pub struct MediaDeviceResponder {
    pending: Pending,
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
    let pending = Arc::new(Mutex::new(HashMap::new()));
    (
        Arc::new(BridgeMediaDeviceCallback {
            sink,
            pending: pending.clone(),
        }),
        MediaDeviceResponder { pending },
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
        let Some(pending) = self.pending.lock().await.remove(&response.request_id) else {
            return false;
        };
        pending.response.send(response).is_ok()
    }

    /// Drop pending calls when the authenticated Bridge client disconnects.
    pub async fn drain(&self) -> usize {
        let mut pending = self.pending.lock().await;
        let count = pending.len();
        pending.clear();
        count
    }

    /// Number of callbacks waiting on a native response.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
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

#[async_trait::async_trait]
impl NativeMediaDeviceCallback for BridgeMediaDeviceCallback {
    async fn request(&self, request: MediaDeviceRequestEnvelopeDto) -> MediaDeviceResponseDto {
        if let Err(message) = request.validate() {
            return device_error(
                request.request_id,
                MediaErrorKindDto::InvalidRequest,
                message,
            );
        }
        let request_id = request.request_id.clone();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.contains_key(&request_id) {
                return device_error(
                    request_id,
                    MediaErrorKindDto::InvalidRequest,
                    "media device request identity is already active".into(),
                );
            }
            if pending.len() >= MAX_PENDING_MEDIA_CALLBACKS {
                return device_error(
                    request_id,
                    MediaErrorKindDto::Busy,
                    "native media callback capacity is full".into(),
                );
            }
            pending.insert(request_id.clone(), PendingRequest { response: tx });
        }
        let _pending_guard = PendingMapGuard::new(self.pending.clone(), request_id.clone());
        if !self
            .sink
            .emit_request(ClientEvent::MediaDeviceRequest { request })
            .await
        {
            self.pending.lock().await.remove(&request_id);
            return device_error(
                request_id,
                MediaErrorKindDto::Unavailable,
                "the native device host is not attached".into(),
            );
        }
        match tokio::time::timeout(DEVICE_CALLBACK_TIMEOUT, rx).await {
            Ok(Ok(response)) if response.request_id == request_id => response,
            Ok(Ok(_)) => {
                self.pending.lock().await.remove(&request_id);
                device_error(
                    request_id,
                    MediaErrorKindDto::NativeFailure,
                    "the native device host returned a mismatched request identity".into(),
                )
            }
            Ok(Err(_)) => device_error(
                request_id,
                MediaErrorKindDto::Unavailable,
                "the Bridge connection ended before the native media response arrived".into(),
            ),
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                device_error(
                    request_id,
                    MediaErrorKindDto::Timeout,
                    "the native media callback exceeded its response deadline".into(),
                )
            }
        }
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
