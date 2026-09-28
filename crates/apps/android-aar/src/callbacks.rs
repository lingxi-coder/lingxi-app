#[cfg(feature = "uniffi")]
use harness_runtime::mobile::ClientEventListener;

/// FFI error surface for the Android share callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`platform_api::ShareError`].
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
/// or dismissed the system share sheet. Mapped to [`platform_api::ShareResult`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone)]
pub enum ShareResultFfi {
    /// The user completed the share (chose a target app).
    Success,
    /// The user dismissed the share sheet without sharing.
    Cancelled,
}

/// Crate-local foreign callback interface for native sharing — the Kotlin app
/// implements it over the system `Intent.ACTION_SEND` share sheet. Bridged to
/// [`platform_api::SharingService`] by [`AndroidShareBridge`]. The payload crosses the
/// seam as three flat optionals (`text` / `url` / `image_bytes`).
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidShare: Send + Sync {
    /// Present the native share sheet for the given payload and report whether
    /// the user completed or cancelled it.
    async fn share(
        &self,
        text: Option<String>,
        url: Option<String>,
        image_bytes: Option<Vec<u8>>,
    ) -> Result<ShareResultFfi, ShareFfiError>;
}

/// Adapts the crate-local [`AndroidShare`] callback interface to the shared
/// [`platform_api::SharingService`] seam the engine consumes. Destructures
/// [`platform_api::SharePayload`] into the flat `text` / `url` / `image_bytes` args
/// and fans [`ShareResultFfi`] / [`ShareFfiError`] back out onto
/// [`platform_api::ShareResult`] / [`platform_api::ShareError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) struct AndroidShareBridge {
    pub(super) inner: Box<dyn AndroidShare>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl platform_api::SharingService for AndroidShareBridge {
    async fn share(
        &self,
        payload: platform_api::SharePayload,
    ) -> Result<platform_api::ShareResult, platform_api::ShareError> {
        let platform_api::SharePayload {
            text,
            url,
            image_bytes,
        } = payload;
        match self.inner.share(text, url, image_bytes).await {
            Ok(ShareResultFfi::Success) => Ok(platform_api::ShareResult::Success),
            Ok(ShareResultFfi::Cancelled) => Ok(platform_api::ShareResult::Cancelled),
            Err(ShareFfiError::Unsupported) => Err(platform_api::ShareError::Unsupported),
            Err(ShareFfiError::Other { message }) => Err(platform_api::ShareError::Other(message)),
        }
    }
}

/// FFI error surface for the Android one-shot location callback interface.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum LocationFfiError {
    /// The user denied Android's location runtime permission.
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

/// FFI carrier for one resolved Android location.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct LocationFixFfi {
    /// Latitude in decimal degrees (WGS-84).
    pub latitude: f64,
    /// Longitude in decimal degrees (WGS-84).
    pub longitude: f64,
    /// Horizontal accuracy in meters, when reported by Android.
    pub accuracy_m: Option<f64>,
    /// Fix time, epoch milliseconds.
    pub timestamp_ms: u64,
}

/// Crate-local foreign callback interface for one-shot Android location.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidLocation: Send + Sync {
    /// Resolve the device's current location once.
    async fn current_location(&self) -> Result<LocationFixFfi, LocationFfiError>;
}

/// Adapts the Kotlin callback onto the shared engine location seam.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) struct AndroidLocationBridge {
    pub(super) inner: Box<dyn AndroidLocation>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl platform_api::LocationProvider for AndroidLocationBridge {
    async fn current_location(
        &self,
    ) -> Result<platform_api::LocationFix, platform_api::LocationError> {
        match self.inner.current_location().await {
            Ok(fix) => Ok(platform_api::LocationFix {
                latitude: fix.latitude,
                longitude: fix.longitude,
                accuracy_m: fix.accuracy_m,
                timestamp_ms: fix.timestamp_ms,
            }),
            Err(LocationFfiError::PermissionDenied) => {
                Err(platform_api::LocationError::PermissionDenied)
            }
            Err(LocationFfiError::Unavailable) => Err(platform_api::LocationError::Unavailable),
            Err(LocationFfiError::Timeout) => Err(platform_api::LocationError::Timeout),
            Err(LocationFfiError::Other { message }) => {
                Err(platform_api::LocationError::Other(message))
            }
        }
    }
}

/// FFI error surface for the Android notification callback interface. A flat
/// enum so `UniFFI` can render it for an async `callback_interface` method; the
/// bridge fans it back out onto the richer [`platform_api::NotificationError`].
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

/// Crate-local foreign callback interface for native notifications — the Kotlin
/// app implements it over the system `NotificationManager`. Bridged to
/// [`platform_api::NotificationService`] by [`AndroidNotificationBridge`]. The request
/// crosses the seam as the flat `title` / `body` / `tag` args.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidNotification: Send + Sync {
    /// Post a single local notification. `tag` (when present) lets a later post
    /// replace an earlier one (the notification id / channel tag).
    async fn notify(
        &self,
        title: String,
        body: String,
        tag: Option<String>,
    ) -> Result<(), NotificationFfiError>;
}

/// Adapts the crate-local [`AndroidNotification`] callback interface to the
/// shared [`platform_api::NotificationService`] seam the engine consumes.
/// Destructures [`platform_api::NotificationRequest`] into the flat `title` / `body`
/// / `tag` args and fans [`NotificationFfiError`] back out onto
/// [`platform_api::NotificationError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) struct AndroidNotificationBridge {
    pub(super) inner: Box<dyn AndroidNotification>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl platform_api::NotificationService for AndroidNotificationBridge {
    async fn notify(
        &self,
        req: platform_api::NotificationRequest,
    ) -> Result<(), platform_api::NotificationError> {
        let platform_api::NotificationRequest { title, body, tag } = req;
        match self.inner.notify(title, body, tag).await {
            Ok(()) => Ok(()),
            Err(NotificationFfiError::PermissionDenied) => {
                Err(platform_api::NotificationError::PermissionDenied)
            }
            Err(NotificationFfiError::Other { message }) => {
                Err(platform_api::NotificationError::Other(message))
            }
        }
    }
}

/// FFI error surface for the Android clipboard callback interface. A flat enum
/// so `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`platform_api::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum ClipboardFfiError {
    /// The platform does not support this clipboard operation (e.g. Android
    /// 10+ restricts clipboard reads to the focused app / default IME).
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
/// Kotlin app implements it over the system `ClipboardManager` (set via
/// `ClipData.newPlainText` + `setPrimaryClip`; get via
/// `primaryClip.getItemAt(0).coerceToText`). Bridged to [`platform_api::Clipboard`]
/// by [`AndroidClipboardBridge`]. `get_text` returns `None` when the clipboard
/// is empty or a read is not permitted by the platform.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidClipboard: Send + Sync {
    /// Write plain `text` to the system clipboard.
    async fn set_text(&self, text: String) -> Result<(), ClipboardFfiError>;
    /// Read plain text from the system clipboard. Returns `None` when empty or
    /// when a background read is not permitted (Android 10+ restriction).
    async fn get_text(&self) -> Result<Option<String>, ClipboardFfiError>;
}

/// Adapts the crate-local [`AndroidClipboard`] callback interface to the shared
/// [`platform_api::Clipboard`] seam the engine consumes. One forwarding hop per call;
/// maps [`ClipboardFfiError`] back out onto [`platform_api::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) struct AndroidClipboardBridge {
    pub(super) inner: Box<dyn AndroidClipboard>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl platform_api::Clipboard for AndroidClipboardBridge {
    async fn set_text(&self, text: String) -> Result<(), platform_api::ClipboardError> {
        self.inner
            .set_text(text)
            .await
            .map_err(clipboard_error_from_ffi)
    }
    async fn get_text(&self) -> Result<Option<String>, platform_api::ClipboardError> {
        self.inner
            .get_text()
            .await
            .map_err(clipboard_error_from_ffi)
    }
}

/// Fan a flat [`ClipboardFfiError`] back out onto the richer
/// [`platform_api::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn clipboard_error_from_ffi(e: ClipboardFfiError) -> platform_api::ClipboardError {
    match e {
        ClipboardFfiError::Unsupported => platform_api::ClipboardError::Unsupported,
        ClipboardFfiError::Other { message } => platform_api::ClipboardError::Other(message),
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

/// Native callback implemented by Kotlin for status, haptics, and deep links.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidDeviceControl: Send + Sync {
    /// Return a JSON-encoded bounded [`platform_api::DeviceStatus`] record.
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
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) struct AndroidDeviceControlBridge {
    pub(super) inner: Box<dyn AndroidDeviceControl>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl platform_api::DeviceStatusProvider for AndroidDeviceControlBridge {
    async fn status(&self) -> Result<platform_api::DeviceStatus, platform_api::DeviceStatusError> {
        let body = self
            .inner
            .status_json()
            .await
            .map_err(device_control_error)?;
        serde_json::from_str(&body).map_err(|error| {
            platform_api::DeviceStatusError::Other(format!("invalid native device status: {error}"))
        })
    }
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl platform_api::HapticService for AndroidDeviceControlBridge {
    async fn trigger(
        &self,
        style: platform_api::HapticStyle,
    ) -> Result<(), platform_api::HapticError> {
        self.inner
            .trigger_haptic(haptic_style_to_wire(style).to_string())
            .await
            .map_err(|error| match error {
                DeviceControlFfiError::Unavailable => platform_api::HapticError::Unavailable,
                DeviceControlFfiError::Rejected { message }
                | DeviceControlFfiError::Other { message } => {
                    platform_api::HapticError::Other(message)
                }
            })
    }
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl platform_api::DeepLinkOpener for AndroidDeviceControlBridge {
    async fn open(&self, url: String) -> Result<(), platform_api::DeepLinkError> {
        self.inner
            .open_deep_link(url)
            .await
            .map_err(device_control_error_for_deep_link)
    }
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl platform_api::CalendarProvider for AndroidDeviceControlBridge {
    async fn list_events(
        &self,
        query: platform_api::CalendarQuery,
    ) -> Result<Vec<platform_api::CalendarEvent>, platform_api::CalendarError> {
        let request = serde_json::to_string(&query)
            .map_err(|error| platform_api::CalendarError::Other(error.to_string()))?;
        let body = self
            .inner
            .calendar_json(request)
            .await
            .map_err(|error| match error {
                DeviceControlFfiError::Unavailable => platform_api::CalendarError::Unavailable,
                DeviceControlFfiError::Rejected { .. } => {
                    platform_api::CalendarError::PermissionDenied
                }
                DeviceControlFfiError::Other { message } => {
                    platform_api::CalendarError::Other(message)
                }
            })?;
        serde_json::from_str(&body).map_err(|error| {
            platform_api::CalendarError::Other(format!("invalid native calendar response: {error}"))
        })
    }
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl platform_api::ContactsProvider for AndroidDeviceControlBridge {
    async fn search(
        &self,
        query: platform_api::ContactsQuery,
    ) -> Result<Vec<platform_api::Contact>, platform_api::ContactsError> {
        let request = serde_json::to_string(&query)
            .map_err(|error| platform_api::ContactsError::Other(error.to_string()))?;
        let body = self
            .inner
            .contacts_json(request)
            .await
            .map_err(|error| match error {
                DeviceControlFfiError::Unavailable => platform_api::ContactsError::Unavailable,
                DeviceControlFfiError::Rejected { .. } => {
                    platform_api::ContactsError::PermissionDenied
                }
                DeviceControlFfiError::Other { message } => {
                    platform_api::ContactsError::Other(message)
                }
            })?;
        serde_json::from_str(&body).map_err(|error| {
            platform_api::ContactsError::Other(format!("invalid native contacts response: {error}"))
        })
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn haptic_style_to_wire(style: platform_api::HapticStyle) -> &'static str {
    match style {
        platform_api::HapticStyle::Light => "light",
        platform_api::HapticStyle::Medium => "medium",
        platform_api::HapticStyle::Heavy => "heavy",
        platform_api::HapticStyle::Success => "success",
        platform_api::HapticStyle::Warning => "warning",
        platform_api::HapticStyle::Error => "error",
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn device_control_error(
    error: DeviceControlFfiError,
) -> platform_api::DeviceStatusError {
    match error {
        DeviceControlFfiError::Unavailable => platform_api::DeviceStatusError::Unavailable,
        DeviceControlFfiError::Rejected { message } | DeviceControlFfiError::Other { message } => {
            platform_api::DeviceStatusError::Other(message)
        }
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn device_control_error_for_deep_link(
    error: DeviceControlFfiError,
) -> platform_api::DeepLinkError {
    match error {
        DeviceControlFfiError::Unavailable => platform_api::DeepLinkError::Unavailable,
        DeviceControlFfiError::Rejected { message } => {
            platform_api::DeepLinkError::Rejected(message)
        }
        DeviceControlFfiError::Other { message } => platform_api::DeepLinkError::Other(message),
    }
}

/// FFI error surface for the Android secure-storage callback interface. A flat
/// enum so `UniFFI` can render it for an async `callback_interface` method; the
/// bridge fans it back out onto the richer [`platform_api::SecureStorageError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum SecureStorageFfiError {
    /// The OS denied access (e.g. Keystore unlock / user-auth required).
    #[error("secure storage permission denied: {message}")]
    PermissionDenied {
        /// Human-readable detail from the native side.
        message: String,
    },
    /// The Keystore/store is currently unusable.
    #[error("secure storage backend unavailable: {message}")]
    BackendUnavailable {
        /// Human-readable detail from the native side.
        message: String,
    },
    /// Any other native failure.
    #[error("secure storage io error: {message}")]
    Io {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// Crate-local foreign callback interface for the native Android Keystore-backed
/// secure store. The engine's serialized `SecureStorageData` crosses the seam as
/// an opaque `blob` keyed by `(service, account)`; the Kotlin side persists it in
/// the Keystore / EncryptedSharedPreferences. Bridged to [`platform_api::SecureStorage`]
/// by [`AndroidSecureStorageBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidSecureStorage: Send + Sync {
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

/// Adapts the crate-local [`AndroidSecureStorage`] (opaque-blob FFI) to the
/// shared [`platform_api::SecureStorage`] seam: serde-encodes `SecureStorageData` to a
/// blob on store, decodes on retrieve, and reports the Keystore as an encrypted
/// backend so the engine persists secrets there.
#[cfg(all(feature = "uniffi", target_os = "android"))]
pub(super) struct AndroidSecureStorageBridge {
    pub(super) inner: Box<dyn AndroidSecureStorage>,
}

#[cfg(all(feature = "uniffi", target_os = "android"))]
#[async_trait::async_trait]
impl platform_api::SecureStorage for AndroidSecureStorageBridge {
    async fn store(
        &self,
        service: &str,
        account: &str,
        data: protocol::SecureStorageData,
    ) -> Result<(), platform_api::SecureStorageError> {
        let blob = serde_json::to_vec(&data)
            .map_err(|e| platform_api::SecureStorageError::Io(format!("serialize: {e}")))?;
        self.inner
            .store(service.to_string(), account.to_string(), blob)
            .await
            .map_err(securestorage_error_from_ffi)
    }
    async fn retrieve(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<protocol::SecureStorageData>, platform_api::SecureStorageError> {
        match self
            .inner
            .retrieve(service.to_string(), account.to_string())
            .await
            .map_err(securestorage_error_from_ffi)?
        {
            Some(blob) => {
                let data = serde_json::from_slice(&blob).map_err(|e| {
                    platform_api::SecureStorageError::Io(format!("deserialize: {e}"))
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
    ) -> Result<(), platform_api::SecureStorageError> {
        self.inner
            .delete(service.to_string(), account.to_string())
            .await
            .map_err(securestorage_error_from_ffi)
    }
    async fn list(&self, service: &str) -> Result<Vec<String>, platform_api::SecureStorageError> {
        self.inner
            .list(service.to_string())
            .await
            .map_err(securestorage_error_from_ffi)
    }
    fn is_encrypted(&self) -> bool {
        true
    }
    fn backend(&self) -> platform_api::SecureStorageBackend {
        platform_api::SecureStorageBackend::AndroidKeystore
    }
}

/// Fan a flat [`SecureStorageFfiError`] back out onto [`platform_api::SecureStorageError`].
#[cfg(all(feature = "uniffi", target_os = "android"))]
pub(super) fn securestorage_error_from_ffi(
    e: SecureStorageFfiError,
) -> platform_api::SecureStorageError {
    match e {
        SecureStorageFfiError::PermissionDenied { message } => {
            platform_api::SecureStorageError::PermissionDenied(message)
        }
        SecureStorageFfiError::BackendUnavailable { message } => {
            platform_api::SecureStorageError::BackendUnavailable(message)
        }
        SecureStorageFfiError::Io { message } => platform_api::SecureStorageError::Io(message),
    }
}

/// FFI error surface for the Android camera callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`platform_api::CameraError`].
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

/// Crate-local foreign callback interface for native camera access — the Kotlin
/// app implements it over `CameraX` (capture) and the system photo picker
/// (library). Bridged to [`platform_api::CameraControl`] by [`AndroidCameraBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidCamera: Send + Sync {
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
}

/// Adapts the crate-local [`AndroidCamera`] callback interface to the shared
/// [`platform_api::CameraControl`] seam the engine consumes. Maps
/// [`platform_api::CameraPosition`] onto the flat `front` bool, threads
/// `allow_editing`, and fans [`CameraFfiError`] back out onto
/// [`platform_api::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) struct AndroidCameraBridge {
    pub(super) inner: Box<dyn AndroidCamera>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl platform_api::CameraControl for AndroidCameraBridge {
    async fn capture_photo(
        &self,
        opts: platform_api::CapturePhotoOpts,
    ) -> Result<platform_api::CapturedImage, platform_api::CameraError> {
        let front = matches!(opts.position, platform_api::CameraPosition::Front);
        match self.inner.capture_photo(front, opts.allow_editing).await {
            Ok(img) => Ok(captured_image_from_ffi(img)),
            Err(e) => Err(camera_error_from_ffi(e)),
        }
    }
    async fn pick_from_library(
        &self,
    ) -> Result<platform_api::CapturedImage, platform_api::CameraError> {
        match self.inner.pick_from_library().await {
            Ok(img) => Ok(captured_image_from_ffi(img)),
            Err(e) => Err(camera_error_from_ffi(e)),
        }
    }
}

/// Convert an FFI [`CapturedImageFfi`] into the shared [`platform_api::CapturedImage`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn captured_image_from_ffi(img: CapturedImageFfi) -> platform_api::CapturedImage {
    platform_api::CapturedImage {
        jpeg_bytes: img.jpeg_bytes,
        width: img.width,
        height: img.height,
    }
}

/// Fan a flat [`CameraFfiError`] back out onto the richer [`platform_api::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn camera_error_from_ffi(e: CameraFfiError) -> platform_api::CameraError {
    match e {
        CameraFfiError::PermissionDenied => platform_api::CameraError::PermissionDenied,
        CameraFfiError::Cancelled => platform_api::CameraError::Cancelled,
        CameraFfiError::DeviceUnavailable => platform_api::CameraError::DeviceUnavailable,
        CameraFfiError::Other { message } => platform_api::CameraError::Other(message),
    }
}

/// Host-implemented per-op Git credential provider (spec: per-op credential FFI).
/// Called synchronously inside libgit2's credentials callback, once per network
/// op — the host fetches the secret (e.g. from the Android Keystore) on demand so
/// no plaintext secret is held resident in the engine between ops.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
pub trait AndroidGitCredentialProvider: Send + Sync {
    /// HTTPS token (PAT), or `None` for anonymous/public remotes.
    fn https_token(&self) -> Option<String>;
    /// SSH private-key passphrase, or `None` if the key is unencrypted.
    fn ssh_passphrase(&self) -> Option<String>;
}

/// Adapts the crate-local [`AndroidGitCredentialProvider`] callback interface to
/// the shared [`tool_api::GitCredentialProvider`] seam the engine consumes. One
/// forwarding hop per call; both methods are synchronous (libgit2's credentials
/// callback is sync), so no async runtime is involved.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) struct AndroidGitCredentialProviderBridge {
    pub(super) inner: Box<dyn AndroidGitCredentialProvider>,
}

#[cfg(feature = "uniffi")]
impl tool_api::GitCredentialProvider for AndroidGitCredentialProviderBridge {
    fn https_token(&self) -> Option<String> {
        self.inner.https_token()
    }
    fn ssh_passphrase(&self) -> Option<String> {
        self.inner.ssh_passphrase()
    }
}

/// A [`harness_runtime::mobile::PermissionRequestSink`] that drops outbound permission requests (mirrors
/// `ios-framework`'s `NoopPermissionSink`). Mobile always binds the adapter
/// permission gate; with no foreign permission UI yet, an unanswered request
/// simply parks the turn (still cancellable).
#[cfg(feature = "uniffi")]
// Reference implementation mirroring `ios-framework`'s `NoopPermissionSink`; the
// Android constructor binds `AndroidPermissionSinkBridge` instead, so this is
// unconstructed on every target — keep it as the documented no-op shape.
#[allow(dead_code)]
pub(super) struct NoopPermissionSink;

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl harness_runtime::mobile::PermissionRequestSink for NoopPermissionSink {
    async fn emit_request(&self, _request: client_protocol::permission::PermissionRequest) {}
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum AndroidComputerUseFfiError {
    #[error("accessibility service disabled")]
    ServiceDisabled,
    #[error("Computer Use session inactive")]
    SessionInactive,
    #[error("Computer Use permission denied: {message}")]
    PermissionDenied { message: String },
    #[error("target package not allowed: {message}")]
    TargetNotAllowed { message: String },
    #[error("Computer Use tier insufficient: {message}")]
    TierInsufficient { message: String },
    #[error("protected Android surface: {message}")]
    ProtectedSurface { message: String },
    #[error("stale Android node: {message}")]
    StaleNode { message: String },
    #[error("Android Computer Use timeout: {message}")]
    Timeout { message: String },
    #[error("unsupported Android Computer Use operation: {message}")]
    Unsupported { message: String },
    #[error("Android Computer Use error: {message}")]
    Other { message: String },
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct AndroidScreenshotFfi {
    pub width: u32,
    pub height: u32,
    pub png_bytes: Vec<u8>,
}

/// Kotlin-owned Direct-build Computer Use host. JSON is used for the
/// Android-specific tree/action vocabulary so the UniFFI surface stays stable
/// while the strongly typed Rust trait remains the tool contract.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidComputerUseHost: Send + Sync {
    async fn status_json(&self) -> Result<String, AndroidComputerUseFfiError>;
    async fn request_access_json(
        &self,
        request_json: String,
    ) -> Result<String, AndroidComputerUseFfiError>;
    async fn list_granted_apps_json(&self) -> Result<String, AndroidComputerUseFfiError>;
    async fn screenshot(&self) -> Result<AndroidScreenshotFfi, AndroidComputerUseFfiError>;
    async fn ui_tree_json(&self) -> Result<String, AndroidComputerUseFfiError>;
    async fn find_nodes_json(
        &self,
        query_json: String,
    ) -> Result<String, AndroidComputerUseFfiError>;
    async fn inspect_node_json(
        &self,
        node_id: String,
    ) -> Result<String, AndroidComputerUseFfiError>;
    async fn perform_json(&self, action_json: String)
        -> Result<String, AndroidComputerUseFfiError>;
    async fn wait_for_json(
        &self,
        condition_json: String,
        timeout_ms: u64,
    ) -> Result<String, AndroidComputerUseFfiError>;
    async fn listen_json(&self, request_json: String)
        -> Result<String, AndroidComputerUseFfiError>;
    async fn speak_json(&self, request_json: String) -> Result<String, AndroidComputerUseFfiError>;
    async fn stop_audio(&self) -> Result<(), AndroidComputerUseFfiError>;
    async fn stop(&self) -> Result<(), AndroidComputerUseFfiError>;
}

#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) struct AndroidComputerUseBridge {
    pub(super) inner: Box<dyn AndroidComputerUseHost>,
}

#[cfg(feature = "uniffi")]
pub(super) fn computer_use_error_from_ffi(
    error: AndroidComputerUseFfiError,
) -> platform_api::AndroidAutomationError {
    use platform_api::AndroidAutomationError as Target;
    match error {
        AndroidComputerUseFfiError::ServiceDisabled => Target::ServiceDisabled,
        AndroidComputerUseFfiError::SessionInactive => Target::SessionInactive,
        AndroidComputerUseFfiError::PermissionDenied { message } => {
            Target::PermissionDenied(message)
        }
        AndroidComputerUseFfiError::TargetNotAllowed { message } => {
            Target::TargetNotAllowed(message)
        }
        AndroidComputerUseFfiError::TierInsufficient { message } => {
            Target::TierInsufficient(message)
        }
        AndroidComputerUseFfiError::ProtectedSurface { message } => {
            Target::ProtectedSurface(message)
        }
        AndroidComputerUseFfiError::StaleNode { message } => Target::StaleNode(message),
        AndroidComputerUseFfiError::Timeout { message } => Target::Timeout(message),
        AndroidComputerUseFfiError::Unsupported { message } => Target::Unsupported(message),
        AndroidComputerUseFfiError::Other { message } => Target::Other(message),
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn decode_computer_use_json<T: serde::de::DeserializeOwned>(
    value: String,
) -> Result<T, platform_api::AndroidAutomationError> {
    serde_json::from_str(&value).map_err(|error| {
        platform_api::AndroidAutomationError::Other(format!("invalid host JSON: {error}"))
    })
}

#[cfg(feature = "uniffi")]
pub(super) fn encode_computer_use_json<T: serde::Serialize>(
    value: &T,
) -> Result<String, platform_api::AndroidAutomationError> {
    serde_json::to_string(value).map_err(|error| {
        platform_api::AndroidAutomationError::Other(format!("cannot encode host JSON: {error}"))
    })
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl platform_api::AndroidUiAutomation for AndroidComputerUseBridge {
    async fn status(
        &self,
    ) -> Result<platform_api::AndroidAutomationStatus, platform_api::AndroidAutomationError> {
        let value = self
            .inner
            .status_json()
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn request_access(
        &self,
        request: platform_api::AndroidAccessRequest,
    ) -> Result<Vec<platform_api::AndroidAppInfo>, platform_api::AndroidAutomationError> {
        let request = encode_computer_use_json(&request)?;
        let value = self
            .inner
            .request_access_json(request)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn list_granted_apps(
        &self,
    ) -> Result<Vec<platform_api::AndroidAppInfo>, platform_api::AndroidAutomationError> {
        let value = self
            .inner
            .list_granted_apps_json()
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn screenshot(
        &self,
    ) -> Result<platform_api::AndroidScreenshot, platform_api::AndroidAutomationError> {
        let value = self
            .inner
            .screenshot()
            .await
            .map_err(computer_use_error_from_ffi)?;
        Ok(platform_api::AndroidScreenshot {
            width: value.width,
            height: value.height,
            png_bytes: value.png_bytes,
        })
    }

    async fn ui_tree(
        &self,
    ) -> Result<platform_api::AndroidUiSnapshot, platform_api::AndroidAutomationError> {
        let value = self
            .inner
            .ui_tree_json()
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn find_nodes(
        &self,
        query: platform_api::AndroidNodeQuery,
    ) -> Result<Vec<platform_api::AndroidUiNode>, platform_api::AndroidAutomationError> {
        let query = encode_computer_use_json(&query)?;
        let value = self
            .inner
            .find_nodes_json(query)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn inspect_node(
        &self,
        node_id: String,
    ) -> Result<platform_api::AndroidUiNode, platform_api::AndroidAutomationError> {
        let value = self
            .inner
            .inspect_node_json(node_id)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn perform(
        &self,
        action: platform_api::AndroidAction,
    ) -> Result<platform_api::AndroidActionResult, platform_api::AndroidAutomationError> {
        let action = encode_computer_use_json(&action)?;
        let value = self
            .inner
            .perform_json(action)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn wait_for(
        &self,
        condition: platform_api::AndroidWaitCondition,
        timeout_ms: u64,
    ) -> Result<platform_api::AndroidActionResult, platform_api::AndroidAutomationError> {
        let condition = encode_computer_use_json(&condition)?;
        let value = self
            .inner
            .wait_for_json(condition, timeout_ms)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn listen(
        &self,
        request: platform_api::AndroidAudioListenRequest,
    ) -> Result<platform_api::AndroidAudioTranscript, platform_api::AndroidAutomationError> {
        let request = encode_computer_use_json(&request)?;
        let value = self
            .inner
            .listen_json(request)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn speak(
        &self,
        request: platform_api::AndroidAudioSpeakRequest,
    ) -> Result<platform_api::AndroidAudioSpeakResult, platform_api::AndroidAutomationError> {
        let request = encode_computer_use_json(&request)?;
        let value = self
            .inner
            .speak_json(request)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn stop_audio(&self) -> Result<(), platform_api::AndroidAutomationError> {
        self.inner
            .stop_audio()
            .await
            .map_err(computer_use_error_from_ffi)
    }

    async fn stop(&self) -> Result<(), platform_api::AndroidAutomationError> {
        self.inner.stop().await.map_err(computer_use_error_from_ffi)
    }
}

/// The Kotlin-implemented permission sink the Android app registers when it builds
/// the engine. Defined in THIS crate (not re-used from `harness-runtime::mobile`) so its
/// `UniFFI` converter registers under `android_aar`'s tag — a prerequisite for
/// naming it as a parameter type in [`build_android_engine`]. Mirrors
/// `AndroidEventListener`: where the listener carries OUTBOUND events, this carries
/// the engine's OUTBOUND permission requests to the Kotlin host's prompt UI; the
/// inbound resolution flows back through
/// `MobileEngineHandle::submit(ClientCommand::Approve/DenyPermission)`.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidPermissionSink: Send + Sync {
    /// Deliver one outbound [`client_protocol::permission::PermissionRequest`] to
    /// the Kotlin host. Implementations enqueue a prompt and return promptly —
    /// they must not block the engine turn loop; the user's answer comes back via
    /// `MobileEngineHandle::submit`.
    async fn on_request(&self, request: client_protocol::permission::PermissionRequest);
}

/// Adapts the crate-local [`AndroidPermissionSink`] callback interface to the
/// shared [`harness_runtime::mobile::PermissionRequestSink`] the engine's adapter gate emits onto. One
/// forwarding hop per request; no transformation. Mirrors [`AndroidListenerBridge`].
///
/// Constructed only on the `target_os = "android"` path of
/// [`build_android_engine`]; `allow(dead_code)` on the host bindgen build (where
/// that path is `cfg`'d out).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) struct AndroidPermissionSinkBridge {
    pub(super) inner: Box<dyn AndroidPermissionSink>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl harness_runtime::mobile::PermissionRequestSink for AndroidPermissionSinkBridge {
    async fn emit_request(&self, request: client_protocol::permission::PermissionRequest) {
        self.inner.on_request(request).await;
    }
}

/// The Kotlin-implemented event listener the Android app registers when it builds
/// the engine. Defined in THIS crate (not re-used from `client-adapter`) so its
/// `UniFFI` converter registers under `android_aar`'s tag — a prerequisite for
/// naming it as a parameter type in [`build_android_engine`]. Mirrors
/// `ios-framework::IosEventListener`.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidEventListener: Send + Sync {
    /// Deliver one fully-lowered [`client_protocol::events::ClientEvent`] to the
    /// Kotlin host. Implementations enqueue onto the UI's event stream and return
    /// promptly — they must not block the engine turn loop.
    async fn on_event(&self, event: client_protocol::events::ClientEvent);

    /// Deliver one session-owned workflow progress update.
    async fn on_workflow_progress(
        &self,
        _origin_session_id: String,
        _task_id: String,
        _run_id: String,
        _progress: client_protocol::listings::WorkflowProgressDto,
    ) {
    }
}

/// Flat cancellation error for the Android-local `AudioService` callback.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum AndroidAudioFfiError {
    /// The Kotlin service could not cancel the requested operation.
    #[error("audio cancellation failed: {message}")]
    NativeFailure { message: String },
}

/// Crate-local Android callback interface for the single app-scoped audio
/// service. UniFFI 0.28 requires callback interfaces to be declared in the
/// export crate rather than referenced from `harness-runtime::mobile` metadata.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidAudioService: Send + Sync {
    /// Current support/readiness snapshot; must not request permission.
    fn capabilities(&self) -> client_protocol::audio::AudioCapabilitySnapshotDto;
    /// Execute one audio operation and return its structured terminal result.
    async fn execute(
        &self,
        request: client_protocol::audio::AudioOperationRequestDto,
    ) -> client_protocol::audio::AudioOperationResultDto;
    /// Cancel only the matching pending operation identity.
    async fn cancel(
        &self,
        identity: client_protocol::audio::AudioOperationIdDto,
    ) -> Result<(), AndroidAudioFfiError>;
}

#[cfg(feature = "uniffi")]
pub(super) struct AndroidAudioServiceBridge {
    pub(super) inner: Box<dyn AndroidAudioService>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl harness_runtime::mobile::NativeAudioService for AndroidAudioServiceBridge {
    fn capabilities(&self) -> client_protocol::audio::AudioCapabilitySnapshotDto {
        self.inner.capabilities()
    }

    async fn execute(
        &self,
        request: client_protocol::audio::AudioOperationRequestDto,
    ) -> client_protocol::audio::AudioOperationResultDto {
        self.inner.execute(request).await
    }

    async fn cancel(
        &self,
        identity: client_protocol::audio::AudioOperationIdDto,
    ) -> Result<(), harness_runtime::mobile::AudioFfiError> {
        self.inner
            .cancel(identity)
            .await
            .map_err(|error| match error {
                AndroidAudioFfiError::NativeFailure { message } => {
                    harness_runtime::mobile::AudioFfiError::NativeFailure { message }
                }
            })
    }
}

/// Adapts the crate-local [`AndroidEventListener`] callback interface to the
/// shared [`ClientEventListener`] the engine's adapter sink expects. One
/// forwarding hop per event; no transformation.
#[cfg(feature = "uniffi")]
pub(super) struct AndroidListenerBridge {
    pub(super) inner: Box<dyn AndroidEventListener>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl ClientEventListener for AndroidListenerBridge {
    async fn on_event(&self, event: client_protocol::events::ClientEvent) {
        self.inner.on_event(event).await;
    }

    async fn on_workflow_progress(
        &self,
        origin_session_id: String,
        task_id: String,
        run_id: String,
        progress: client_protocol::listings::WorkflowProgressDto,
    ) {
        self.inner
            .on_workflow_progress(origin_session_id, task_id, run_id, progress)
            .await;
    }
}
