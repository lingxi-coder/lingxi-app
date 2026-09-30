#[cfg(feature = "uniffi")]
use harness_runtime::mobile::ClientEventListener;

/// A [`harness_runtime::mobile::PermissionRequestSink`] that drops outbound permission requests. Mobile
/// always binds the adapter permission gate; with no foreign permission UI yet,
/// a request that is never answered simply parks the turn (the conversation can
/// still cancel it). Lighting up a real permission dialog is additive: a future
/// constructor will accept a foreign `PermissionRequestSink` callback interface.
///
/// Constructed only on the `target_os = "ios"` path; `allow(dead_code)` on the
/// host bindgen build (where that path is `cfg`'d out).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) struct NoopPermissionSink;

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl harness_runtime::mobile::PermissionRequestSink for NoopPermissionSink {
    async fn emit_request(&self, _request: client::protocol::permission::PermissionRequest) {}
}

/// The Swift-implemented permission sink the iOS app registers when it builds the
/// engine. Defined in this crate (not re-used from `harness-runtime::mobile`) so its UniFFI
/// converter registers under `ios_framework`'s tag — see [`crate::build_ios_engine`].
/// The host presents a prompt for each request and resolves it by submitting
/// `ClientCommand::ApprovePermission` / `DenyPermission` back through the handle.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosPermissionSink: Send + Sync {
    /// Deliver one outbound [`client::protocol::permission::PermissionRequest`] to
    /// the Swift host. Implementations enqueue a prompt and return promptly —
    /// they must not block the engine turn loop; the user's answer comes back via
    /// `MobileEngineHandle::submit`.
    async fn on_request(&self, request: client::protocol::permission::PermissionRequest);
}

/// Adapts the crate-local [`IosPermissionSink`] callback interface to the shared
/// [`harness_runtime::mobile::PermissionRequestSink`] the engine's adapter gate emits onto. One forwarding
/// hop per request; no transformation. Mirrors [`IosListenerBridge`].
///
/// Constructed only on the `target_os = "ios"` path of [`crate::build_ios_engine`];
/// `allow(dead_code)` on the host bindgen build (where that path is `cfg`'d out).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) struct IosPermissionSinkBridge {
    pub(super) inner: Box<dyn IosPermissionSink>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl harness_runtime::mobile::PermissionRequestSink for IosPermissionSinkBridge {
    async fn emit_request(&self, request: client::protocol::permission::PermissionRequest) {
        self.inner.on_request(request).await;
    }
}

/// The Swift-implemented event listener the iOS app registers when it builds the
/// engine. Defined in this crate (not re-used from `client-adapter`) so its
/// UniFFI converter registers under `ios_framework`'s tag — see [`crate::build_ios_engine`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosEventListener: Send + Sync {
    /// Deliver one fully-lowered [`client::protocol::events::ClientEvent`] to the
    /// Swift host. Implementations enqueue onto the UI's event stream and return
    /// promptly — they must not block the engine turn loop.
    async fn on_event(&self, event: client::protocol::events::ClientEvent);

    /// Deliver one structured workflow/subagent progress update without
    /// reconstructing live state from transcript events.
    async fn on_workflow_progress(
        &self,
        origin_session_id: String,
        task_id: String,
        run_id: String,
        progress: client::protocol::listings::WorkflowProgressDto,
    );
}

/// Flat cancellation error for the iOS-local `AudioService` callback.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum IosAudioFfiError {
    /// The Swift service could not cancel the requested operation.
    #[error("audio cancellation failed: {message}")]
    NativeFailure { message: String },
}

/// Crate-local iOS callback interface for the single app-scoped audio service.
/// Kept in this packager because UniFFI 0.28 cannot reference an external
/// callback-interface definition from `harness-runtime::mobile` metadata.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosAudioService: Send + Sync {
    /// Current support/readiness snapshot; must not request permission.
    fn capabilities(&self) -> client::protocol::audio::AudioCapabilitySnapshotDto;
    /// Execute one audio operation and return its structured terminal result.
    async fn execute(
        &self,
        request: client::protocol::audio::AudioOperationRequestDto,
    ) -> client::protocol::audio::AudioOperationResultDto;
    /// Cancel only the matching pending operation identity.
    async fn cancel(
        &self,
        identity: client::protocol::audio::AudioOperationIdDto,
    ) -> Result<(), IosAudioFfiError>;
}

#[cfg(feature = "uniffi")]
pub(super) struct IosAudioServiceBridge {
    pub(super) inner: Box<dyn IosAudioService>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl harness_runtime::mobile::NativeAudioService for IosAudioServiceBridge {
    fn capabilities(&self) -> client::protocol::audio::AudioCapabilitySnapshotDto {
        self.inner.capabilities()
    }

    async fn execute(
        &self,
        request: client::protocol::audio::AudioOperationRequestDto,
    ) -> client::protocol::audio::AudioOperationResultDto {
        self.inner.execute(request).await
    }

    async fn cancel(
        &self,
        identity: client::protocol::audio::AudioOperationIdDto,
    ) -> Result<(), harness_runtime::mobile::AudioFfiError> {
        self.inner
            .cancel(identity)
            .await
            .map_err(|error| match error {
                IosAudioFfiError::NativeFailure { message } => {
                    harness_runtime::mobile::AudioFfiError::NativeFailure { message }
                }
            })
    }
}

/// Adapts the crate-local [`IosEventListener`] callback interface to the shared
/// [`ClientEventListener`] the engine's adapter sink expects. One forwarding hop
/// per event; no transformation. (`UniFFI` lifts a `callback_interface` as a
/// `Box<dyn …>`, so the bridge owns the boxed foreign object directly.)
#[cfg(feature = "uniffi")]
pub(super) struct IosListenerBridge {
    pub(super) inner: Box<dyn IosEventListener>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl ClientEventListener for IosListenerBridge {
    async fn on_event(&self, event: client::protocol::events::ClientEvent) {
        self.inner.on_event(event).await;
    }

    async fn on_workflow_progress(
        &self,
        origin_session_id: String,
        task_id: String,
        run_id: String,
        progress: client::protocol::listings::WorkflowProgressDto,
    ) {
        self.inner
            .on_workflow_progress(origin_session_id, task_id, run_id, progress)
            .await;
    }
}

/// FFI error surface for the iOS share callback interface. A flat enum so `UniFFI`
/// can render it for an async `callback_interface` method; the bridge fans it
/// back out onto the richer [`lingxi_core::host::ShareError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum ShareFfiError {
    /// Sharing is unsupported on this device / for this payload.
    #[error("sharing unsupported")]
    Unsupported,
    /// Any other native failure.
    #[error("share error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// FFI carrier for the outcome of a native share — whether the user completed
/// or dismissed the system share sheet. Mapped to [`lingxi_core::host::ShareResult`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone)]
pub enum ShareResultFfi {
    /// The user completed the share (chose a target app).
    Success,
    /// The user dismissed the share sheet without sharing.
    Cancelled,
}

/// Crate-local foreign callback interface for native sharing — the Swift app
/// implements it over `UIActivityViewController`. Bridged to
/// [`lingxi_core::host::SharingService`] by [`IosShareBridge`]. The payload crosses the
/// seam as three flat optionals (`text` / `url` / `image_bytes`).
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosShare: Send + Sync {
    /// Present the native share sheet for the given payload and report whether
    /// the user completed or cancelled it.
    async fn share(
        &self,
        text: Option<String>,
        url: Option<String>,
        image_bytes: Option<Vec<u8>>,
    ) -> Result<ShareResultFfi, ShareFfiError>;
}

/// Adapts the crate-local [`IosShare`] callback interface to the shared
/// [`lingxi_core::host::SharingService`] seam the engine consumes. Destructures
/// [`lingxi_core::host::SharePayload`] into the flat `text` / `url` / `image_bytes` args
/// and fans [`ShareResultFfi`] / [`ShareFfiError`] back out onto
/// [`lingxi_core::host::ShareResult`] / [`lingxi_core::host::ShareError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) struct IosShareBridge {
    pub(super) inner: Box<dyn IosShare>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::SharingService for IosShareBridge {
    async fn share(
        &self,
        payload: lingxi_core::host::SharePayload,
    ) -> Result<lingxi_core::host::ShareResult, lingxi_core::host::ShareError> {
        let lingxi_core::host::SharePayload {
            text,
            url,
            image_bytes,
        } = payload;
        match self.inner.share(text, url, image_bytes).await {
            Ok(ShareResultFfi::Success) => Ok(lingxi_core::host::ShareResult::Success),
            Ok(ShareResultFfi::Cancelled) => Ok(lingxi_core::host::ShareResult::Cancelled),
            Err(ShareFfiError::Unsupported) => Err(lingxi_core::host::ShareError::Unsupported),
            Err(ShareFfiError::Other { message }) => {
                Err(lingxi_core::host::ShareError::Other(message))
            }
        }
    }
}

/// FFI error surface for the iOS location callback interface. Flat, like its
/// notification/camera siblings, so `UniFFI` can render it for an async
/// `callback_interface` method.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum LocationFfiError {
    /// The user denied location permission.
    #[error("location permission denied")]
    PermissionDenied,
    /// Location services are off, restricted, or absent.
    #[error("location unavailable")]
    Unavailable,
    /// No fix arrived before the native deadline.
    #[error("location timed out")]
    Timeout,
    /// Any other native failure.
    #[error("location error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// FFI carrier for one resolved location crossing the callback-interface
/// seam. Mapped to [`lingxi_core::host::LocationFix`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct LocationFixFfi {
    /// Latitude in decimal degrees (WGS-84).
    pub latitude: f64,
    /// Longitude in decimal degrees (WGS-84).
    pub longitude: f64,
    /// Horizontal accuracy in meters, when the platform reports one.
    pub accuracy_m: Option<f64>,
    /// Fix time, epoch milliseconds.
    pub timestamp_ms: u64,
}

/// Crate-local foreign callback interface for one-shot location — the Swift
/// app implements it over `CLLocationManager`. Bridged to
/// [`lingxi_core::host::LocationProvider`] by [`IosLocationBridge`].
///
/// One-shot only: continuous tracking would need a host-to-page push channel
/// that does not exist yet, and a background-location entitlement nobody has
/// asked for.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosLocation: Send + Sync {
    /// Resolve the device's current location once.
    async fn current_location(&self) -> Result<LocationFixFfi, LocationFfiError>;
}

/// Adapts the crate-local [`IosLocation`] callback interface to the shared
/// [`lingxi_core::host::LocationProvider`] seam the engine consumes.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) struct IosLocationBridge {
    pub(super) inner: Box<dyn IosLocation>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::LocationProvider for IosLocationBridge {
    async fn current_location(
        &self,
    ) -> Result<lingxi_core::host::LocationFix, lingxi_core::host::LocationError> {
        match self.inner.current_location().await {
            Ok(fix) => Ok(lingxi_core::host::LocationFix {
                latitude: fix.latitude,
                longitude: fix.longitude,
                accuracy_m: fix.accuracy_m,
                timestamp_ms: fix.timestamp_ms,
            }),
            Err(LocationFfiError::PermissionDenied) => {
                Err(lingxi_core::host::LocationError::PermissionDenied)
            }
            Err(LocationFfiError::Unavailable) => {
                Err(lingxi_core::host::LocationError::Unavailable)
            }
            Err(LocationFfiError::Timeout) => Err(lingxi_core::host::LocationError::Timeout),
            Err(LocationFfiError::Other { message }) => {
                Err(lingxi_core::host::LocationError::Other(message))
            }
        }
    }
}

/// FFI error surface for the iOS notification callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`lingxi_core::host::NotificationError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum NotificationFfiError {
    /// The user denied notification permission.
    #[error("notification permission denied")]
    PermissionDenied,
    /// Any other native failure.
    #[error("notification error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// Crate-local foreign callback interface for native notifications — the Swift
/// app implements it over `UNUserNotificationCenter`. Bridged to
/// [`lingxi_core::host::NotificationService`] by [`IosNotificationBridge`]. The request
/// crosses the seam as the flat `title` / `body` / `tag` args.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosNotification: Send + Sync {
    /// Post a single local notification. `tag` (when present) lets a later post
    /// replace an earlier one (the notification request identifier).
    async fn notify(
        &self,
        title: String,
        body: String,
        tag: Option<String>,
    ) -> Result<(), NotificationFfiError>;
}

/// Adapts the crate-local [`IosNotification`] callback interface to the shared
/// [`lingxi_core::host::NotificationService`] seam the engine consumes. Destructures
/// [`lingxi_core::host::NotificationRequest`] into the flat `title` / `body` / `tag` args
/// and fans [`NotificationFfiError`] back out onto [`lingxi_core::host::NotificationError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) struct IosNotificationBridge {
    pub(super) inner: Box<dyn IosNotification>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::NotificationService for IosNotificationBridge {
    async fn notify(
        &self,
        req: lingxi_core::host::NotificationRequest,
    ) -> Result<(), lingxi_core::host::NotificationError> {
        let lingxi_core::host::NotificationRequest { title, body, tag } = req;
        match self.inner.notify(title, body, tag).await {
            Ok(()) => Ok(()),
            Err(NotificationFfiError::PermissionDenied) => {
                Err(lingxi_core::host::NotificationError::PermissionDenied)
            }
            Err(NotificationFfiError::Other { message }) => {
                Err(lingxi_core::host::NotificationError::Other(message))
            }
        }
    }
}

/// FFI error surface for the iOS clipboard callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`lingxi_core::host::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum ClipboardFfiError {
    /// The platform does not support this clipboard operation.
    #[error("clipboard operation unsupported")]
    Unsupported,
    /// Any other native failure.
    #[error("clipboard error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// Crate-local foreign callback interface for native clipboard access — the
/// Swift app implements it over `UIPasteboard` (set via `string =`; get via
/// `string`). Bridged to [`lingxi_core::host::Clipboard`] by [`IosClipboardBridge`].
/// `get_text` returns `None` when the clipboard is empty or holds no text.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosClipboard: Send + Sync {
    /// Write plain `text` to the system clipboard.
    async fn set_text(&self, text: String) -> Result<(), ClipboardFfiError>;
    /// Read plain text from the system clipboard. Returns `None` when empty.
    async fn get_text(&self) -> Result<Option<String>, ClipboardFfiError>;
}

/// Adapts the crate-local [`IosClipboard`] callback interface to the shared
/// [`lingxi_core::host::Clipboard`] seam the engine consumes. One forwarding hop per call;
/// maps [`ClipboardFfiError`] back out onto [`lingxi_core::host::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) struct IosClipboardBridge {
    pub(super) inner: Box<dyn IosClipboard>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::Clipboard for IosClipboardBridge {
    async fn set_text(&self, text: String) -> Result<(), lingxi_core::host::ClipboardError> {
        self.inner
            .set_text(text)
            .await
            .map_err(clipboard_error_from_ffi)
    }
    async fn get_text(&self) -> Result<Option<String>, lingxi_core::host::ClipboardError> {
        self.inner
            .get_text()
            .await
            .map_err(clipboard_error_from_ffi)
    }
}

/// Fan a flat [`ClipboardFfiError`] back out onto the richer
/// [`lingxi_core::host::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) fn clipboard_error_from_ffi(e: ClipboardFfiError) -> lingxi_core::host::ClipboardError {
    match e {
        ClipboardFfiError::Unsupported => lingxi_core::host::ClipboardError::Unsupported,
        ClipboardFfiError::Other { message } => lingxi_core::host::ClipboardError::Other(message),
    }
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum DeviceControlFfiError {
    /// The platform cannot provide the requested operation.
    #[error("device control unavailable")]
    Unavailable,
    /// The platform rejected the requested deep link or haptic style.
    #[error("device control rejected: {message}")]
    Rejected { message: String },
    /// Any other native failure.
    #[error("device control error: {message}")]
    Other { message: String },
}

/// Native callback implemented by Swift for status, haptics, and deep links.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosDeviceControl: Send + Sync {
    /// Return a JSON-encoded bounded [`lingxi_core::host::DeviceStatus`] record.
    async fn status_json(&self) -> Result<String, DeviceControlFfiError>;
    /// Trigger one host-approved style.
    async fn trigger_haptic(&self, style: String) -> Result<(), DeviceControlFfiError>;
    /// Open one already-validated external URL.
    async fn open_deep_link(&self, url: String) -> Result<(), DeviceControlFfiError>;
    /// Return JSON-encoded bounded calendar events for one query.
    async fn calendar_json(&self, request_json: String) -> Result<String, DeviceControlFfiError>;
    /// Return JSON-encoded bounded contacts for one search.
    async fn contacts_json(&self, request_json: String) -> Result<String, DeviceControlFfiError>;
}

#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) struct IosDeviceControlBridge {
    pub(super) inner: Box<dyn IosDeviceControl>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::DeviceStatusProvider for IosDeviceControlBridge {
    async fn status(
        &self,
    ) -> Result<lingxi_core::host::DeviceStatus, lingxi_core::host::DeviceStatusError> {
        let body = self
            .inner
            .status_json()
            .await
            .map_err(ios_device_control_error)?;
        serde_json::from_str(&body).map_err(|error| {
            lingxi_core::host::DeviceStatusError::Other(format!(
                "invalid native device status: {error}"
            ))
        })
    }
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::HapticService for IosDeviceControlBridge {
    async fn trigger(
        &self,
        style: lingxi_core::host::HapticStyle,
    ) -> Result<(), lingxi_core::host::HapticError> {
        self.inner
            .trigger_haptic(ios_haptic_style_to_wire(style).to_string())
            .await
            .map_err(|error| match error {
                DeviceControlFfiError::Unavailable => lingxi_core::host::HapticError::Unavailable,
                DeviceControlFfiError::Rejected { message }
                | DeviceControlFfiError::Other { message } => {
                    lingxi_core::host::HapticError::Other(message)
                }
            })
    }
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::DeepLinkOpener for IosDeviceControlBridge {
    async fn open(&self, url: String) -> Result<(), lingxi_core::host::DeepLinkError> {
        self.inner
            .open_deep_link(url)
            .await
            .map_err(ios_deep_link_error)
    }
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::CalendarProvider for IosDeviceControlBridge {
    async fn list_events(
        &self,
        query: lingxi_core::host::CalendarQuery,
    ) -> Result<Vec<lingxi_core::host::CalendarEvent>, lingxi_core::host::CalendarError> {
        let request = serde_json::to_string(&query)
            .map_err(|error| lingxi_core::host::CalendarError::Other(error.to_string()))?;
        let body = self
            .inner
            .calendar_json(request)
            .await
            .map_err(|error| match error {
                DeviceControlFfiError::Unavailable => lingxi_core::host::CalendarError::Unavailable,
                DeviceControlFfiError::Rejected { .. } => {
                    lingxi_core::host::CalendarError::PermissionDenied
                }
                DeviceControlFfiError::Other { message } => {
                    lingxi_core::host::CalendarError::Other(message)
                }
            })?;
        serde_json::from_str(&body).map_err(|error| {
            lingxi_core::host::CalendarError::Other(format!(
                "invalid native calendar response: {error}"
            ))
        })
    }
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::ContactsProvider for IosDeviceControlBridge {
    async fn search(
        &self,
        query: lingxi_core::host::ContactsQuery,
    ) -> Result<Vec<lingxi_core::host::Contact>, lingxi_core::host::ContactsError> {
        let request = serde_json::to_string(&query)
            .map_err(|error| lingxi_core::host::ContactsError::Other(error.to_string()))?;
        let body = self
            .inner
            .contacts_json(request)
            .await
            .map_err(|error| match error {
                DeviceControlFfiError::Unavailable => lingxi_core::host::ContactsError::Unavailable,
                DeviceControlFfiError::Rejected { .. } => {
                    lingxi_core::host::ContactsError::PermissionDenied
                }
                DeviceControlFfiError::Other { message } => {
                    lingxi_core::host::ContactsError::Other(message)
                }
            })?;
        serde_json::from_str(&body).map_err(|error| {
            lingxi_core::host::ContactsError::Other(format!(
                "invalid native contacts response: {error}"
            ))
        })
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn ios_haptic_style_to_wire(style: lingxi_core::host::HapticStyle) -> &'static str {
    match style {
        lingxi_core::host::HapticStyle::Light => "light",
        lingxi_core::host::HapticStyle::Medium => "medium",
        lingxi_core::host::HapticStyle::Heavy => "heavy",
        lingxi_core::host::HapticStyle::Success => "success",
        lingxi_core::host::HapticStyle::Warning => "warning",
        lingxi_core::host::HapticStyle::Error => "error",
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn ios_device_control_error(
    error: DeviceControlFfiError,
) -> lingxi_core::host::DeviceStatusError {
    match error {
        DeviceControlFfiError::Unavailable => lingxi_core::host::DeviceStatusError::Unavailable,
        DeviceControlFfiError::Rejected { message } | DeviceControlFfiError::Other { message } => {
            lingxi_core::host::DeviceStatusError::Other(message)
        }
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn ios_deep_link_error(
    error: DeviceControlFfiError,
) -> lingxi_core::host::DeepLinkError {
    match error {
        DeviceControlFfiError::Unavailable => lingxi_core::host::DeepLinkError::Unavailable,
        DeviceControlFfiError::Rejected { message } => {
            lingxi_core::host::DeepLinkError::Rejected(message)
        }
        DeviceControlFfiError::Other { message } => {
            lingxi_core::host::DeepLinkError::Other(message)
        }
    }
}

/// FFI error surface for the iOS camera callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`lingxi_core::host::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum CameraFfiError {
    /// The user denied camera / photo-library permission.
    #[error("camera permission denied")]
    PermissionDenied,
    /// The user cancelled the capture / picker.
    #[error("camera capture cancelled")]
    Cancelled,
    /// No camera hardware is available.
    #[error("camera device unavailable")]
    DeviceUnavailable,
    /// Any other native failure.
    #[error("camera error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// FFI carrier for a captured (or picked) image crossing the callback-interface
/// seam: JPEG-encoded bytes + the decoded pixel dimensions.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct CapturedImageFfi {
    /// JPEG-encoded image bytes.
    pub jpeg_bytes: Vec<u8>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

/// Crate-local foreign callback interface for native camera access — the Swift
/// app implements it over `UIImagePickerController` / `PHPickerViewController`.
/// Bridged to [`lingxi_core::host::CameraControl`] by [`IosCameraBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosCamera: Send + Sync {
    /// Capture a photo with the native camera UI. `front` selects the
    /// front/selfie camera when true (rear when false); `allow_editing`
    /// presents the native edit/crop UI after capture.
    async fn capture_photo(
        &self,
        front: bool,
        allow_editing: bool,
    ) -> Result<CapturedImageFfi, CameraFfiError>;
    /// Pick an existing image from the system photo library.
    async fn pick_from_library(&self) -> Result<CapturedImageFfi, CameraFfiError>;

    /// Capture, then downscale to at most `max_dimension` px on the longer
    /// side and re-encode at `jpeg_quality` (0.0..=1.0).
    ///
    /// The scaling happens natively because Rust ships no image codec here
    /// (the mobile build vendors its dependencies offline), and a
    /// full-resolution 12 MP JPEG is 3-6 MB — far past what a local app's
    /// bridge response, or a provider's vision endpoint, will take.
    async fn capture_photo_sized(
        &self,
        front: bool,
        allow_editing: bool,
        max_dimension: u32,
        jpeg_quality: f32,
    ) -> Result<CapturedImageFfi, CameraFfiError>;

    /// Library pick with the same native downscale contract as
    /// [`IosCamera::capture_photo_sized`].
    async fn pick_from_library_sized(
        &self,
        max_dimension: u32,
        jpeg_quality: f32,
    ) -> Result<CapturedImageFfi, CameraFfiError>;
}

/// Adapts the crate-local [`IosCamera`] callback interface to the shared
/// [`lingxi_core::host::CameraControl`] seam the engine consumes. Maps
/// [`lingxi_core::host::CameraPosition`] onto the flat `front` bool, threads
/// `allow_editing`, and fans [`CameraFfiError`] back out onto
/// [`lingxi_core::host::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) struct IosCameraBridge {
    pub(super) inner: Box<dyn IosCamera>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::CameraControl for IosCameraBridge {
    async fn capture_photo(
        &self,
        opts: lingxi_core::host::CapturePhotoOpts,
    ) -> Result<lingxi_core::host::CapturedImage, lingxi_core::host::CameraError> {
        let front = matches!(opts.position, lingxi_core::host::CameraPosition::Front);
        match self.inner.capture_photo(front, opts.allow_editing).await {
            Ok(img) => Ok(captured_image_from_ffi(img)),
            Err(e) => Err(camera_error_from_ffi(e)),
        }
    }
    async fn pick_from_library(
        &self,
    ) -> Result<lingxi_core::host::CapturedImage, lingxi_core::host::CameraError> {
        match self.inner.pick_from_library().await {
            Ok(img) => Ok(captured_image_from_ffi(img)),
            Err(e) => Err(camera_error_from_ffi(e)),
        }
    }
    // Overrides the trait's delegating defaults: on iOS the native side CAN
    // scale, and a local app's bridge budget depends on it doing so.
    async fn capture_photo_sized(
        &self,
        opts: lingxi_core::host::CapturePhotoOpts,
        max_dimension: u32,
        jpeg_quality: f32,
    ) -> Result<lingxi_core::host::CapturedImage, lingxi_core::host::CameraError> {
        let front = matches!(opts.position, lingxi_core::host::CameraPosition::Front);
        match self
            .inner
            .capture_photo_sized(front, opts.allow_editing, max_dimension, jpeg_quality)
            .await
        {
            Ok(img) => Ok(captured_image_from_ffi(img)),
            Err(e) => Err(camera_error_from_ffi(e)),
        }
    }
    async fn pick_from_library_sized(
        &self,
        max_dimension: u32,
        jpeg_quality: f32,
    ) -> Result<lingxi_core::host::CapturedImage, lingxi_core::host::CameraError> {
        match self
            .inner
            .pick_from_library_sized(max_dimension, jpeg_quality)
            .await
        {
            Ok(img) => Ok(captured_image_from_ffi(img)),
            Err(e) => Err(camera_error_from_ffi(e)),
        }
    }
}

/// Convert an FFI [`CapturedImageFfi`] into the shared [`lingxi_core::host::CapturedImage`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) fn captured_image_from_ffi(img: CapturedImageFfi) -> lingxi_core::host::CapturedImage {
    lingxi_core::host::CapturedImage {
        jpeg_bytes: img.jpeg_bytes,
        width: img.width,
        height: img.height,
    }
}

/// Fan a flat [`CameraFfiError`] back out onto the richer [`lingxi_core::host::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) fn camera_error_from_ffi(e: CameraFfiError) -> lingxi_core::host::CameraError {
    match e {
        CameraFfiError::PermissionDenied => lingxi_core::host::CameraError::PermissionDenied,
        CameraFfiError::Cancelled => lingxi_core::host::CameraError::Cancelled,
        CameraFfiError::DeviceUnavailable => lingxi_core::host::CameraError::DeviceUnavailable,
        CameraFfiError::Other { message } => lingxi_core::host::CameraError::Other(message),
    }
}

/// FFI error surface for the iOS secure-storage callback interface. A flat enum
/// so `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`lingxi_core::host::SecureStorageError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum SecureStorageFfiError {
    /// The OS denied access (e.g. Keychain item requires user auth / device unlock).
    #[error("secure storage permission denied: {message}")]
    PermissionDenied {
        /// Human-readable detail from the native side.
        message: String,
    },
    /// The Keychain is currently unusable.
    #[error("secure storage backend unavailable: {message}")]
    BackendUnavailable {
        /// Human-readable detail from the native side.
        message: String,
    },
    /// Any other native failure (non-zero OSStatus, etc.).
    #[error("secure storage io error: {message}")]
    Io {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// Crate-local foreign callback interface for the native iOS Keychain-backed
/// secure store — the Swift app implements it over `SecItemAdd`/`SecItemCopyMatching`
/// (kSecClass GenericPassword, kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
/// so items are excluded from iCloud/iTunes backups). The engine's serialized
/// `SecureStorageData` crosses the seam as an opaque `blob` keyed by
/// `(service, account)`. Bridged to [`lingxi_core::host::SecureStorage`] by
/// [`IosSecureStorageBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosSecureStorage: Send + Sync {
    /// Persist `blob` under `(service, account)`, overwriting any existing entry.
    async fn store(
        &self,
        service: String,
        account: String,
        blob: Vec<u8>,
    ) -> Result<(), SecureStorageFfiError>;
    /// Return the blob for `(service, account)`, or `None` if absent.
    async fn retrieve(
        &self,
        service: String,
        account: String,
    ) -> Result<Option<Vec<u8>>, SecureStorageFfiError>;
    /// Remove `(service, account)` (removing a non-existent entry is not an error).
    async fn delete(&self, service: String, account: String) -> Result<(), SecureStorageFfiError>;
    /// List every `account` stored under `service`.
    async fn list(&self, service: String) -> Result<Vec<String>, SecureStorageFfiError>;
}

/// Adapts the crate-local [`IosSecureStorage`] (opaque-blob FFI) to the shared
/// [`lingxi_core::host::SecureStorage`] seam: serde-encodes `SecureStorageData` to a blob on
/// store, decodes on retrieve, and reports the Keychain as an encrypted backend.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) struct IosSecureStorageBridge {
    pub(super) inner: Box<dyn IosSecureStorage>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::SecureStorage for IosSecureStorageBridge {
    async fn store(
        &self,
        service: &str,
        account: &str,
        data: lingxi_core::types::SecureStorageData,
    ) -> Result<(), lingxi_core::host::SecureStorageError> {
        let blob = serde_json::to_vec(&data)
            .map_err(|e| lingxi_core::host::SecureStorageError::Io(format!("serialize: {e}")))?;
        self.inner
            .store(service.to_string(), account.to_string(), blob)
            .await
            .map_err(securestorage_error_from_ffi)
    }
    async fn retrieve(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<lingxi_core::types::SecureStorageData>, lingxi_core::host::SecureStorageError>
    {
        match self
            .inner
            .retrieve(service.to_string(), account.to_string())
            .await
            .map_err(securestorage_error_from_ffi)?
        {
            Some(blob) => {
                let data = serde_json::from_slice(&blob).map_err(|e| {
                    lingxi_core::host::SecureStorageError::Io(format!("deserialize: {e}"))
                })?;
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }
    async fn delete(
        &self,
        service: &str,
        account: &str,
    ) -> Result<(), lingxi_core::host::SecureStorageError> {
        self.inner
            .delete(service.to_string(), account.to_string())
            .await
            .map_err(securestorage_error_from_ffi)
    }
    async fn list(
        &self,
        service: &str,
    ) -> Result<Vec<String>, lingxi_core::host::SecureStorageError> {
        self.inner
            .list(service.to_string())
            .await
            .map_err(securestorage_error_from_ffi)
    }
    fn is_encrypted(&self) -> bool {
        true
    }
    fn backend(&self) -> lingxi_core::host::SecureStorageBackend {
        lingxi_core::host::SecureStorageBackend::IosKeychain
    }
}

/// Fan a flat [`SecureStorageFfiError`] back out onto [`lingxi_core::host::SecureStorageError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(super) fn securestorage_error_from_ffi(
    e: SecureStorageFfiError,
) -> lingxi_core::host::SecureStorageError {
    match e {
        SecureStorageFfiError::PermissionDenied { message } => {
            lingxi_core::host::SecureStorageError::PermissionDenied(message)
        }
        SecureStorageFfiError::BackendUnavailable { message } => {
            lingxi_core::host::SecureStorageError::BackendUnavailable(message)
        }
        SecureStorageFfiError::Io { message } => lingxi_core::host::SecureStorageError::Io(message),
    }
}
