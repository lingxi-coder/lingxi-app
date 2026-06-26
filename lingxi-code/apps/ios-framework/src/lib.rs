//! `ios-framework` (M8-P12 → M10-F3) — the iOS `UniFFI` packager.
//!
//! This crate is the FFI boundary between the Rust engine and the iOS app. The
//! Swift layer implements the [`traits::CameraControl`] / [`traits::VoiceRecorder`]
//! / [`traits::SharingService`] callback interfaces (see the skeletons under
//! `swift/`), hands them across as a [`PlatformImpls`] record, and Rust uses
//! them to construct an `IosPlatform` and assemble the mobile engine — so Rust
//! drives the device's native capabilities by calling *back* into Swift. That
//! bidirectional flow is the whole point of the `UniFFI` seam.
//!
//! ## Shared session host (F3-04)
//!
//! The real session host — [`MobileEngineHandle`] (owns the handle-owned tokio
//! runtime + the wired `MobileRuntime` + the registered `ClientEventListener`)
//! and its [`MobileEngineError`] — lives in `engine-mobile` and is RE-EXPORTED
//! here, NOT re-derived. That single-source rule (plan F3-04) is what stops iOS
//! and Android from drifting: this crate only adds the iOS-specific
//! `Platform`-construction wrapper around the shared `build_mobile_engine`.
//!
//! ## Inbound command path (F3-05)
//!
//! The async FFI entry point — `MobileEngineHandle::submit(ClientCommand) ->
//! Result<(), ClientError>` (under `uniffi`: `#[uniffi::export(async_runtime =
//! "tokio")]`) — is defined ONCE on the shared host in `engine-mobile` and
//! reaches Swift through the re-exported [`MobileEngineHandle`]. There is no
//! iOS-specific submit body: `SendPrompt` spawns the streaming turn on the
//! handle-owned runtime and returns promptly (results stream via the listener);
//! `Cancel` fires the in-flight token; `ApprovePermission`/`DenyPermission`
//! resolve the parked permission gate.
//!
//! ## Async-over-FFI runtime registration (F3-07)
//!
//! The async exports (`submit`, the F3-07 inspection helpers) cross the FFI seam
//! as `UniFFI` rust-futures. The foreign async executor is NAMED explicitly by the
//! `#[uniffi::export(async_runtime = "tokio")]` attribute on the shared host's
//! `submit` impl (in `engine-mobile`), backed by the workspace `uniffi` dep's
//! `tokio` feature (pinned offline in F3-00). That scaffolding polls every async
//! export on the handle-owned `rt-multi-thread` runtime — the one
//! [`MobileEngineHandle`] owns per governing decision §0.5 (one connection ⇒ one
//! engine host owning one runtime) — so Swift's `async` calls never block the UI
//! thread and resolve on the engine's own runtime. The
//! `async_submit_resolves_on_handle_runtime` host test proves the registration by
//! asserting an async export resolves on exactly that runtime.
//!
//! ## `UniFFI` status
//! The `uniffi` feature (default-on) lights up the real `UniFFI` surface: the
//! re-exported [`MobileEngineHandle`] is a `#[derive(uniffi::Object)]`, the
//! listener a callback interface, the DTOs `UniFFI` types. `engine-mobile` carries
//! the `setup_scaffolding!()`; this crate re-exports it (and adds its own for
//! the iOS-local exports) so the symbols land in the final `staticlib`/cdylib.

#![forbid(unsafe_code)]

use std::sync::Arc;
use traits::{CameraControl, SharingService, VoiceRecorder};
// `Platform` is named only inside the `cfg(target_os = "ios")` constructor body;
// importing it unconditionally warns on the host build, so scope it to iOS.
#[cfg(all(feature = "uniffi", target_os = "ios"))]
use traits::Platform;

// F3-04: the shared session host + its error type are DEFINED ONCE in
// `engine-mobile` and re-exported here. Both FFI packager crates re-export the
// SAME types so iOS and Android cannot drift (plan F3-04). The listener +
// permission-request-sink the constructor takes are likewise re-exported from
// the shared host.
#[cfg(feature = "uniffi")]
pub use engine_mobile::{
    ClientEventListener, MobileConfig, MobileEngineError, MobileEngineHandle, PermissionRequestSink,
};

/// The foreign (Swift) capability objects + config the engine needs to build an
/// `IosPlatform`. `UniFFI` marshals each `Arc<dyn …>` as a callback-interface
/// reference; `app_sandbox_root` is the app container path.
pub struct PlatformImpls {
    /// Swift `CameraControl` impl.
    pub camera: Arc<dyn CameraControl>,
    /// Swift `VoiceRecorder` impl.
    pub voice: Arc<dyn VoiceRecorder>,
    /// Swift `SharingService` impl.
    pub share: Arc<dyn SharingService>,
    /// Swift Keychain-backed `SecureStorage` impl, if provided. When `None` the
    /// composition root falls back to the non-persisting development stub.
    pub secure_storage: Option<Arc<dyn traits::SecureStorage>>,
    /// The app's writable sandbox container root.
    pub app_sandbox_root: String,
}

/// Top-level `UniFFI` constructor: build the mobile engine from the Swift-supplied
/// platform callbacks + event listener. (Under `uniffi`: `#[uniffi::export]`.)
///
/// This is a THIN wrapper: it constructs the iOS-specific `Platform` from the
/// foreign callbacks and then delegates ALL runtime/adapter/listener wiring to
/// the shared [`engine_mobile::build_mobile_engine`] (F3-04) — so the heavy
/// lifting lives in exactly one place. The returned [`MobileEngineHandle`] owns
/// the tokio runtime + the wired orchestrator + the registered listener.
///
/// On non-iOS hosts this returns [`MobileEngineError::PlatformUnavailable`] —
/// the `IosPlatform` is only linked under `cfg(target_os = "ios")` — so the
/// crate still compiles and the SHARED host is exercised off-device through the
/// test shim (which calls `build_mobile_engine` with a portable fake `Platform`).
#[cfg(feature = "uniffi")]
pub fn build_mobile_engine(
    impls: PlatformImpls,
    listener: Arc<dyn ClientEventListener>,
    permission_sink: Arc<dyn PermissionRequestSink>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    #[cfg(target_os = "ios")]
    {
        use platform_ios::{IosPlatform, IosPlatformInputs};
        let cfg = MobileConfig {
            cwd: std::path::PathBuf::from(&impls.app_sandbox_root),
            claude_home: std::path::PathBuf::from(&impls.app_sandbox_root).join(branding::DOT_DIR),
            // P0.2: production injects the real LINGXI.md hierarchy provider so the
            // orchestrator loads `<cwd>/LINGXI.md` + `<claude_home>/LINGXI.md` into
            // its system prompt and `fire_instructions_loaded()` fires over them.
            memory_provider: Some(orchestrator::prompt::real_provider()),
            ..MobileConfig::default()
        };
        let platform: Arc<dyn Platform> = Arc::new(IosPlatform::new(IosPlatformInputs {
            app_sandbox_root: std::path::PathBuf::from(impls.app_sandbox_root),
            camera: impls.camera,
            voice: impls.voice,
            share: impls.share,
            stt: None,
            tts: None,
            notifications: None,
            clipboard: None,
            secure_storage: impls.secure_storage,
        }));
        engine_mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (impls, listener, permission_sink);
        Err(MobileEngineError::PlatformUnavailable)
    }
}

// ---------------------------------------------------------------------------
// M10-P3a: the foreign-callable engine constructor.
// ---------------------------------------------------------------------------
//
// `build_mobile_engine` (above) takes an `Arc<dyn Platform>` + the foreign
// callback objects (camera / voice / share) — none of which are UniFFI types —
// so it cannot itself cross the FFI boundary. The Swift app needs SOME exported
// constructor to obtain a `MobileEngineHandle`; the only piece it must supply
// for a text conversation is the `ClientEventListener` (already a UniFFI
// callback interface) — the engine's own `IosPlatform` supplies fs / http /
// clock from `platform-posix-minimal`, and a text turn never touches the
// camera / voice / share device capabilities.
//
// So this thin `#[uniffi::export]` wrapper takes ONLY UniFFI-marshalable inputs
// (the listener + plain config strings), constructs default device-capability
// stubs + a no-op permission sink on the Rust side, threads the runtime config
// (api base / key / model) into a `MobileConfig`, and delegates to the shared
// `build_mobile_engine`. This is ADDITIVE FFI packaging only — it changes no
// engine semantics and touches neither the `traits` crate nor Android.
//
// SECRETS: `api_key` arrives as a parameter the Swift side reads from the
// process environment (`ANTHROPIC_API_KEY`) / an app setting at runtime; it is
// NEVER hardcoded, logged, or persisted here. An empty key is valid — the
// orchestrator only fails at `run_turn` with a 401 (mirrors `MobileConfig`).

/// Device-capability stubs used when the foreign host does not (yet) wire the
/// camera / voice / share callbacks. A text conversation never invokes these;
/// each method returns the trait's "unavailable" error so an accidental call is
/// a clean error rather than a panic. M9 replaces these with the real
/// Swift-backed callback objects threaded through a richer constructor.
///
/// Constructed only on the device/simulator (`target_os = "ios"`) path of
/// [`build_ios_engine`]; on the host bindgen build that path is `cfg`'d out, so
/// the stubs are dead there — `allow(dead_code)` keeps the host build warning-clean.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod stub_capabilities {
    use async_trait::async_trait;
    use traits::{
        CameraControl, CameraError, CapturePhotoOpts, CapturedImage, ShareError, SharePayload,
        ShareResult, SharingService, VoiceError, VoiceRecorder, VoiceRecording, VoiceRecordingOpts,
    };

    /// No-op camera: capture / pick both report the hardware as unavailable.
    pub struct StubCamera;

    #[async_trait]
    impl CameraControl for StubCamera {
        async fn capture_photo(
            &self,
            _opts: CapturePhotoOpts,
        ) -> Result<CapturedImage, CameraError> {
            Err(CameraError::DeviceUnavailable)
        }
        async fn pick_from_library(&self) -> Result<CapturedImage, CameraError> {
            Err(CameraError::DeviceUnavailable)
        }
    }

    /// No-op voice recorder: never records.
    pub struct StubVoice;

    #[async_trait]
    impl VoiceRecorder for StubVoice {
        async fn start_recording(&self, _opts: VoiceRecordingOpts) -> Result<(), VoiceError> {
            Err(VoiceError::Other("voice capture not wired".to_string()))
        }
        async fn stop_recording(&self) -> Result<VoiceRecording, VoiceError> {
            Err(VoiceError::NotRecording)
        }
        async fn is_recording(&self) -> bool {
            false
        }
    }

    /// No-op share service: reports sharing unsupported.
    pub struct StubShare;

    #[async_trait]
    impl SharingService for StubShare {
        async fn share(&self, _payload: SharePayload) -> Result<ShareResult, ShareError> {
            Err(ShareError::Unsupported)
        }
    }
}

/// A [`PermissionRequestSink`] that drops outbound permission requests. Mobile
/// always binds the adapter permission gate; with no foreign permission UI yet,
/// a request that is never answered simply parks the turn (the conversation can
/// still cancel it). Lighting up a real permission dialog is additive: a future
/// constructor will accept a foreign `PermissionRequestSink` callback interface.
///
/// Constructed only on the `target_os = "ios"` path; `allow(dead_code)` on the
/// host bindgen build (where that path is `cfg`'d out).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct NoopPermissionSink;

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl engine_mobile::PermissionRequestSink for NoopPermissionSink {
    async fn emit_request(&self, _request: client_protocol::permission::PermissionRequest) {}
}

// The foreign permission sink (`IosPermissionSink`) is a callback interface
// DEFINED IN THIS CRATE — mirroring `IosEventListener` — so its UniFFI
// `FfiConverter` lands under `ios_framework`'s tag, a prerequisite for naming it
// as a parameter type in `build_ios_engine`. Where the listener carries OUTBOUND
// events, this carries the engine's OUTBOUND permission requests to the Swift
// host's prompt UI; the inbound resolution flows back through
// `MobileEngineHandle::submit(ClientCommand::Approve/DenyPermission)`.
// `IosPermissionSinkBridge` adapts this crate-local interface to the shared
// `engine_mobile::PermissionRequestSink` the engine's adapter gate emits onto.
/// The Swift-implemented permission sink the iOS app registers when it builds the
/// engine. Defined in this crate (not re-used from `engine-mobile`) so its UniFFI
/// converter registers under `ios_framework`'s tag — see [`build_ios_engine`].
/// The host presents a prompt for each request and resolves it by submitting
/// `ClientCommand::ApprovePermission` / `DenyPermission` back through the handle.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosPermissionSink: Send + Sync {
    /// Deliver one outbound [`client_protocol::permission::PermissionRequest`] to
    /// the Swift host. Implementations enqueue a prompt and return promptly —
    /// they must not block the engine turn loop; the user's answer comes back via
    /// `MobileEngineHandle::submit`.
    async fn on_request(&self, request: client_protocol::permission::PermissionRequest);
}

/// Adapts the crate-local [`IosPermissionSink`] callback interface to the shared
/// [`PermissionRequestSink`] the engine's adapter gate emits onto. One forwarding
/// hop per request; no transformation. Mirrors [`IosListenerBridge`].
///
/// Constructed only on the `target_os = "ios"` path of [`build_ios_engine`];
/// `allow(dead_code)` on the host bindgen build (where that path is `cfg`'d out).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosPermissionSinkBridge {
    inner: Box<dyn IosPermissionSink>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl engine_mobile::PermissionRequestSink for IosPermissionSinkBridge {
    async fn emit_request(&self, request: client_protocol::permission::PermissionRequest) {
        self.inner.on_request(request).await;
    }
}

// The foreign event listener (`IosEventListener`) is a callback interface
// DEFINED IN THIS CRATE so its UniFFI `FfiConverterArc` lands under
// `ios_framework`'s tag — a prerequisite for naming it in a `#[uniffi::export]`
// function here. (The shared `client_adapter::ClientEventListener` registers its
// converter under `client_adapter`'s tag, so it cannot be a parameter type in
// an export from a different crate.) `IosListenerBridge` adapts this crate-local
// interface to the shared `ClientEventListener` the engine actually feeds.
/// The Swift-implemented event listener the iOS app registers when it builds the
/// engine. Defined in this crate (not re-used from `client-adapter`) so its
/// UniFFI converter registers under `ios_framework`'s tag — see [`build_ios_engine`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosEventListener: Send + Sync {
    /// Deliver one fully-lowered [`client_protocol::events::ClientEvent`] to the
    /// Swift host. Implementations enqueue onto the UI's event stream and return
    /// promptly — they must not block the engine turn loop.
    async fn on_event(&self, event: client_protocol::events::ClientEvent);
}

/// Adapts the crate-local [`IosEventListener`] callback interface to the shared
/// [`ClientEventListener`] the engine's adapter sink expects. One forwarding hop
/// per event; no transformation. (`UniFFI` lifts a `callback_interface` as a
/// `Box<dyn …>`, so the bridge owns the boxed foreign object directly.)
#[cfg(feature = "uniffi")]
struct IosListenerBridge {
    inner: Box<dyn IosEventListener>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl ClientEventListener for IosListenerBridge {
    async fn on_event(&self, event: client_protocol::events::ClientEvent) {
        self.inner.on_event(event).await;
    }
}

// ---------------------------------------------------------------------------
// Device-capability FFI block (iOS parity with android-aar).
// ---------------------------------------------------------------------------
//
// Mirrors `android-aar`'s AndroidStt/AndroidTts/AndroidCamera/AndroidShare/
// AndroidVoice/AndroidNotification/AndroidClipboard callback interfaces + their
// FFI types + engine bridges, s/Android/Ios/ for the interface/bridge names.
// These interfaces are DEFINED IN THIS CRATE (mirroring `IosEventListener`) so
// their UniFFI `FfiConverter`s register under `ios_framework`'s tag — a
// prerequisite for naming them as parameter types in `build_ios_engine`. The
// engine consumes the SHARED `traits::*` seams, so each crate-local interface is
// adapted by a thin bridge struct to its `traits` counterpart.
//
// RETURN SHAPE (UniFFI 0.28.3): async callback-interface methods return
// `Result<T, E>` where `E` is a `#[derive(uniffi::Error)]` enum.

/// FFI error surface for the iOS speech callback interfaces. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer `traits::SttError` / `traits::TtsError`.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum SpeechFfiError {
    /// The user denied microphone permission (STT only).
    #[error("microphone permission denied")]
    PermissionDenied,
    /// No speech detected before the listen timeout (STT only).
    #[error("no speech detected")]
    NoSpeech,
    /// No usable recognizer / synthesizer on the device.
    #[error("speech service unavailable")]
    Unavailable,
    /// A transient failure — safe to retry.
    #[error("transient speech error: {message}")]
    Retriable {
        /// Human-readable detail from the native side.
        message: String,
    },
    /// Any other native failure.
    #[error("speech error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// Crate-local foreign callback interface for native speech-to-text — the Swift
/// app implements it over `SFSpeechRecognizer` (opens the live mic, listens for
/// one utterance, returns the final transcript). Bridged to
/// [`traits::SpeechToText`] by [`IosSttBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosStt: Send + Sync {
    /// Open the mic, listen for a single utterance, and return the recognized
    /// text. `language` is a BCP-47 hint (`None` = device default).
    async fn transcribe(&self, language: Option<String>) -> Result<String, SpeechFfiError>;
}

/// Crate-local foreign callback interface for native text-to-speech — the Swift
/// app implements it over `AVSpeechSynthesizer`, returning 16-bit signed
/// little-endian mono PCM. Bridged to [`traits::TextToSpeech`] by [`IosTtsBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosTts: Send + Sync {
    /// Synthesize `text` to PCM16 audio at [`TtsAudioFfi::sample_rate_hz`].
    /// `voice` is a provider-specific id (`None` = system default voice).
    async fn synthesize(
        &self,
        text: String,
        voice: Option<String>,
    ) -> Result<TtsAudioFfi, SpeechFfiError>;
}

/// FFI carrier for synthesized audio crossing the callback-interface seam:
/// PCM16 frames + the sample rate the Swift engine produced them at.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct TtsAudioFfi {
    /// Raw PCM16 frames (16-bit signed little-endian, mono).
    pub pcm: Vec<u8>,
    /// Sample rate of `pcm` in Hz.
    pub sample_rate_hz: u32,
}

/// FFI error surface for the iOS share callback interface. A flat enum so `UniFFI`
/// can render it for an async `callback_interface` method; the bridge fans it
/// back out onto the richer [`traits::ShareError`].
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
/// or dismissed the system share sheet. Mapped to [`traits::ShareResult`].
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
/// [`traits::SharingService`] by [`IosShareBridge`]. The payload crosses the
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
/// [`traits::SharingService`] seam the engine consumes. Destructures
/// [`traits::SharePayload`] into the flat `text` / `url` / `image_bytes` args
/// and fans [`ShareResultFfi`] / [`ShareFfiError`] back out onto
/// [`traits::ShareResult`] / [`traits::ShareError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosShareBridge {
    inner: Box<dyn IosShare>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::SharingService for IosShareBridge {
    async fn share(
        &self,
        payload: traits::SharePayload,
    ) -> Result<traits::ShareResult, traits::ShareError> {
        let traits::SharePayload {
            text,
            url,
            image_bytes,
        } = payload;
        match self.inner.share(text, url, image_bytes).await {
            Ok(ShareResultFfi::Success) => Ok(traits::ShareResult::Success),
            Ok(ShareResultFfi::Cancelled) => Ok(traits::ShareResult::Cancelled),
            Err(ShareFfiError::Unsupported) => Err(traits::ShareError::Unsupported),
            Err(ShareFfiError::Other { message }) => Err(traits::ShareError::Other(message)),
        }
    }
}

/// FFI error surface for the iOS notification callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::NotificationError`].
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
/// [`traits::NotificationService`] by [`IosNotificationBridge`]. The request
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
/// [`traits::NotificationService`] seam the engine consumes. Destructures
/// [`traits::NotificationRequest`] into the flat `title` / `body` / `tag` args
/// and fans [`NotificationFfiError`] back out onto [`traits::NotificationError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosNotificationBridge {
    inner: Box<dyn IosNotification>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::NotificationService for IosNotificationBridge {
    async fn notify(
        &self,
        req: traits::NotificationRequest,
    ) -> Result<(), traits::NotificationError> {
        let traits::NotificationRequest { title, body, tag } = req;
        match self.inner.notify(title, body, tag).await {
            Ok(()) => Ok(()),
            Err(NotificationFfiError::PermissionDenied) => {
                Err(traits::NotificationError::PermissionDenied)
            }
            Err(NotificationFfiError::Other { message }) => {
                Err(traits::NotificationError::Other(message))
            }
        }
    }
}

/// FFI error surface for the iOS clipboard callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::ClipboardError`].
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
/// `string`). Bridged to [`traits::Clipboard`] by [`IosClipboardBridge`].
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
/// [`traits::Clipboard`] seam the engine consumes. One forwarding hop per call;
/// maps [`ClipboardFfiError`] back out onto [`traits::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosClipboardBridge {
    inner: Box<dyn IosClipboard>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::Clipboard for IosClipboardBridge {
    async fn set_text(&self, text: String) -> Result<(), traits::ClipboardError> {
        self.inner
            .set_text(text)
            .await
            .map_err(clipboard_error_from_ffi)
    }
    async fn get_text(&self) -> Result<Option<String>, traits::ClipboardError> {
        self.inner.get_text().await.map_err(clipboard_error_from_ffi)
    }
}

/// Fan a flat [`ClipboardFfiError`] back out onto the richer
/// [`traits::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn clipboard_error_from_ffi(e: ClipboardFfiError) -> traits::ClipboardError {
    match e {
        ClipboardFfiError::Unsupported => traits::ClipboardError::Unsupported,
        ClipboardFfiError::Other { message } => traits::ClipboardError::Other(message),
    }
}

/// FFI error surface for the iOS camera callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::CameraError`].
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
/// Bridged to [`traits::CameraControl`] by [`IosCameraBridge`].
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
}

/// Adapts the crate-local [`IosCamera`] callback interface to the shared
/// [`traits::CameraControl`] seam the engine consumes. Maps
/// [`traits::CameraPosition`] onto the flat `front` bool, threads
/// `allow_editing`, and fans [`CameraFfiError`] back out onto
/// [`traits::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosCameraBridge {
    inner: Box<dyn IosCamera>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::CameraControl for IosCameraBridge {
    async fn capture_photo(
        &self,
        opts: traits::CapturePhotoOpts,
    ) -> Result<traits::CapturedImage, traits::CameraError> {
        let front = matches!(opts.position, traits::CameraPosition::Front);
        match self.inner.capture_photo(front, opts.allow_editing).await {
            Ok(img) => Ok(captured_image_from_ffi(img)),
            Err(e) => Err(camera_error_from_ffi(e)),
        }
    }
    async fn pick_from_library(&self) -> Result<traits::CapturedImage, traits::CameraError> {
        match self.inner.pick_from_library().await {
            Ok(img) => Ok(captured_image_from_ffi(img)),
            Err(e) => Err(camera_error_from_ffi(e)),
        }
    }
}

/// Convert an FFI [`CapturedImageFfi`] into the shared [`traits::CapturedImage`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn captured_image_from_ffi(img: CapturedImageFfi) -> traits::CapturedImage {
    traits::CapturedImage {
        jpeg_bytes: img.jpeg_bytes,
        width: img.width,
        height: img.height,
    }
}

/// Fan a flat [`CameraFfiError`] back out onto the richer [`traits::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn camera_error_from_ffi(e: CameraFfiError) -> traits::CameraError {
    match e {
        CameraFfiError::PermissionDenied => traits::CameraError::PermissionDenied,
        CameraFfiError::Cancelled => traits::CameraError::Cancelled,
        CameraFfiError::DeviceUnavailable => traits::CameraError::DeviceUnavailable,
        CameraFfiError::Other { message } => traits::CameraError::Other(message),
    }
}

/// FFI error surface for the iOS secure-storage callback interface. A flat enum
/// so `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::SecureStorageError`].
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
/// `(service, account)`. Bridged to [`traits::SecureStorage`] by
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
    async fn delete(&self, service: String, account: String)
        -> Result<(), SecureStorageFfiError>;
    /// List every `account` stored under `service`.
    async fn list(&self, service: String) -> Result<Vec<String>, SecureStorageFfiError>;
}

/// Adapts the crate-local [`IosSecureStorage`] (opaque-blob FFI) to the shared
/// [`traits::SecureStorage`] seam: serde-encodes `SecureStorageData` to a blob on
/// store, decodes on retrieve, and reports the Keychain as an encrypted backend.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosSecureStorageBridge {
    inner: Box<dyn IosSecureStorage>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::SecureStorage for IosSecureStorageBridge {
    async fn store(
        &self,
        service: &str,
        account: &str,
        data: protocol::SecureStorageData,
    ) -> Result<(), traits::SecureStorageError> {
        let blob = serde_json::to_vec(&data)
            .map_err(|e| traits::SecureStorageError::Io(format!("serialize: {e}")))?;
        self.inner
            .store(service.to_string(), account.to_string(), blob)
            .await
            .map_err(securestorage_error_from_ffi)
    }
    async fn retrieve(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<protocol::SecureStorageData>, traits::SecureStorageError> {
        match self
            .inner
            .retrieve(service.to_string(), account.to_string())
            .await
            .map_err(securestorage_error_from_ffi)?
        {
            Some(blob) => {
                let data = serde_json::from_slice(&blob)
                    .map_err(|e| traits::SecureStorageError::Io(format!("deserialize: {e}")))?;
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }
    async fn delete(&self, service: &str, account: &str) -> Result<(), traits::SecureStorageError> {
        self.inner
            .delete(service.to_string(), account.to_string())
            .await
            .map_err(securestorage_error_from_ffi)
    }
    async fn list(&self, service: &str) -> Result<Vec<String>, traits::SecureStorageError> {
        self.inner
            .list(service.to_string())
            .await
            .map_err(securestorage_error_from_ffi)
    }
    fn is_encrypted(&self) -> bool {
        true
    }
    fn backend(&self) -> traits::SecureStorageBackend {
        traits::SecureStorageBackend::IosKeychain
    }
}

/// Fan a flat [`SecureStorageFfiError`] back out onto [`traits::SecureStorageError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn securestorage_error_from_ffi(e: SecureStorageFfiError) -> traits::SecureStorageError {
    match e {
        SecureStorageFfiError::PermissionDenied { message } => {
            traits::SecureStorageError::PermissionDenied(message)
        }
        SecureStorageFfiError::BackendUnavailable { message } => {
            traits::SecureStorageError::BackendUnavailable(message)
        }
        SecureStorageFfiError::Io { message } => traits::SecureStorageError::Io(message),
    }
}

/// FFI error surface for the iOS mic-recorder callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::VoiceError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum VoiceFfiError {
    /// The user denied microphone permission.
    #[error("microphone permission denied")]
    PermissionDenied,
    /// `stop_recording` was called with no active session.
    #[error("not currently recording")]
    NotRecording,
    /// Any other native failure.
    #[error("voice error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// FFI carrier for a finished recording crossing the callback-interface seam:
/// the encoded audio bytes + their MIME type. Mapped to [`traits::VoiceRecording`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct VoiceRecordingFfi {
    /// Encoded audio bytes.
    pub audio_bytes: Vec<u8>,
    /// MIME type of `audio_bytes` (e.g. `"audio/m4a"`).
    pub mime_type: String,
}

/// Crate-local foreign callback interface for native mic recording — the Swift
/// app implements it over `AVAudioRecorder`. Bridged to [`traits::VoiceRecorder`]
/// by [`IosVoiceBridge`]. Driven by the engine through `tool-voice`
/// (start/stop/is_recording); the recording opts cross the seam as the flat
/// `sample_rate_hz` / `format` args.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosVoice: Send + Sync {
    /// Begin a mic recording session at the given sample rate / container format.
    async fn start_recording(
        &self,
        sample_rate_hz: u32,
        format: String,
    ) -> Result<(), VoiceFfiError>;
    /// Stop the active session and return the captured audio.
    async fn stop_recording(&self) -> Result<VoiceRecordingFfi, VoiceFfiError>;
    /// Whether a recording session is currently active.
    async fn is_recording(&self) -> bool;
}

/// Adapts the crate-local [`IosVoice`] callback interface to the shared
/// [`traits::VoiceRecorder`] seam the engine consumes. Destructures
/// [`traits::VoiceRecordingOpts`] into the flat `sample_rate_hz` / `format`
/// args, converts [`VoiceRecordingFfi`] back to [`traits::VoiceRecording`], and
/// fans [`VoiceFfiError`] back out onto [`traits::VoiceError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosVoiceBridge {
    inner: Box<dyn IosVoice>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::VoiceRecorder for IosVoiceBridge {
    async fn start_recording(
        &self,
        opts: traits::VoiceRecordingOpts,
    ) -> Result<(), traits::VoiceError> {
        let traits::VoiceRecordingOpts {
            sample_rate_hz,
            format,
        } = opts;
        self.inner
            .start_recording(sample_rate_hz, format)
            .await
            .map_err(voice_error_from_ffi)
    }
    async fn stop_recording(&self) -> Result<traits::VoiceRecording, traits::VoiceError> {
        match self.inner.stop_recording().await {
            Ok(rec) => Ok(traits::VoiceRecording {
                audio_bytes: rec.audio_bytes,
                mime_type: rec.mime_type,
            }),
            Err(e) => Err(voice_error_from_ffi(e)),
        }
    }
    async fn is_recording(&self) -> bool {
        self.inner.is_recording().await
    }
}

/// Fan a flat [`VoiceFfiError`] back out onto the richer [`traits::VoiceError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn voice_error_from_ffi(e: VoiceFfiError) -> traits::VoiceError {
    match e {
        VoiceFfiError::PermissionDenied => traits::VoiceError::PermissionDenied,
        VoiceFfiError::NotRecording => traits::VoiceError::NotRecording,
        VoiceFfiError::Other { message } => traits::VoiceError::Other(message),
    }
}

/// Adapts the crate-local [`IosStt`] callback interface to the shared
/// [`traits::SpeechToText`] seam the engine consumes. One forwarding hop per
/// call; maps [`SpeechFfiError`] onto [`traits::SttError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosSttBridge {
    inner: Box<dyn IosStt>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::SpeechToText for IosSttBridge {
    async fn transcribe(
        &self,
        opts: traits::SttOpts,
    ) -> Result<traits::SttTranscript, traits::SttError> {
        match self.inner.transcribe(opts.language.clone()).await {
            Ok(text) => Ok(traits::SttTranscript {
                text,
                language: opts.language,
                confidence: None,
            }),
            Err(e) => Err(match e {
                SpeechFfiError::PermissionDenied => traits::SttError::PermissionDenied,
                SpeechFfiError::NoSpeech => traits::SttError::NoSpeech,
                SpeechFfiError::Unavailable => traits::SttError::Unavailable,
                SpeechFfiError::Retriable { message } => traits::SttError::Retriable(message),
                SpeechFfiError::Other { message } => traits::SttError::Other(message),
            }),
        }
    }
}

/// Adapts the crate-local [`IosTts`] callback interface to the shared
/// [`traits::TextToSpeech`] seam the engine consumes. Maps [`SpeechFfiError`]
/// onto [`traits::TtsError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosTtsBridge {
    inner: Box<dyn IosTts>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::TextToSpeech for IosTtsBridge {
    async fn synthesize(
        &self,
        opts: traits::TtsOpts,
    ) -> Result<traits::TtsAudio, traits::TtsError> {
        match self.inner.synthesize(opts.text, opts.voice).await {
            Ok(audio) => Ok(traits::TtsAudio {
                pcm: audio.pcm,
                sample_rate_hz: audio.sample_rate_hz,
            }),
            Err(e) => Err(match e {
                SpeechFfiError::Unavailable => traits::TtsError::Unavailable,
                SpeechFfiError::Retriable { message } | SpeechFfiError::Other { message } => {
                    traits::TtsError::SynthesisFailed(message)
                }
                // STT-only variants are not produced by a TTS impl; fold them
                // into a generic TTS error rather than panic.
                SpeechFfiError::PermissionDenied => {
                    traits::TtsError::Other("permission denied".to_string())
                }
                SpeechFfiError::NoSpeech => traits::TtsError::Other("no speech".to_string()),
            }),
        }
    }
}

/// Foreign-callable constructor for the iOS app (plan M10-P3a).
///
/// Builds a fully-wired [`MobileEngineHandle`] from the Swift-supplied event
/// listener + runtime config. The handle owns its tokio runtime and streams
/// every [`client_protocol::events::ClientEvent`] to `listener.on_event(..)`;
/// the app drives turns via [`MobileEngineHandle::submit`].
///
/// - `api_base`  — Anthropic-compatible base URL (e.g. `https://api.anthropic.com`).
/// - `api_key`   — read by Swift from `ANTHROPIC_API_KEY` / an app setting at
///   runtime. Empty is valid (turns 401 at `run_turn`); never hardcoded here.
/// - `model`     — default model id for new turns.
/// - `app_sandbox_root` — the app container path the engine roots its filesystem
///   + `~/.claude`-equivalent under.
/// - `listener`  — the foreign [`IosEventListener`] the adapter feeds (bridged to
///   the shared [`ClientEventListener`]).
///
/// On non-iOS hosts (and the iOS *simulator* IS `target_os = "ios"`, so it takes
/// the real path) this delegates to [`build_mobile_engine`]; off-device it
/// returns [`MobileEngineError::PlatformUnavailable`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::too_many_arguments)] // FFI constructor: one flat arg per Swift callback.
pub fn build_ios_engine(
    api_base: String,
    api_key: String,
    model: String,
    app_sandbox_root: String,
    listener: Box<dyn IosEventListener>,
    stt: Box<dyn IosStt>,
    tts: Box<dyn IosTts>,
    camera: Box<dyn IosCamera>,
    share: Box<dyn IosShare>,
    voice: Box<dyn IosVoice>,
    notifications: Box<dyn IosNotification>,
    clipboard: Box<dyn IosClipboard>,
    permissions: Box<dyn IosPermissionSink>,
    secure_storage: Option<Box<dyn IosSecureStorage>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    let listener: Arc<dyn ClientEventListener> = Arc::new(IosListenerBridge { inner: listener });
    #[cfg(target_os = "ios")]
    {
        use platform_ios::{IosPlatform, IosPlatformInputs};
        let mut cfg = MobileConfig {
            cwd: std::path::PathBuf::from(&app_sandbox_root),
            claude_home: std::path::PathBuf::from(&app_sandbox_root).join(branding::DOT_DIR),
            // P0.2: production injects the real LINGXI.md hierarchy provider so the
            // orchestrator loads `<cwd>/LINGXI.md` + `<claude_home>/LINGXI.md` into
            // its system prompt and `fire_instructions_loaded()` fires over them.
            memory_provider: Some(orchestrator::prompt::real_provider()),
            ..MobileConfig::default()
        };
        if !api_base.is_empty() {
            cfg.api_base = api_base;
        }
        cfg.api_key = api_key;
        if !model.is_empty() {
            cfg.default_model = model;
        }
        let platform: Arc<dyn Platform> = Arc::new(IosPlatform::new(IosPlatformInputs {
            app_sandbox_root: std::path::PathBuf::from(app_sandbox_root),
            camera: Arc::new(IosCameraBridge { inner: camera }),
            voice: Arc::new(IosVoiceBridge { inner: voice }),
            share: Arc::new(IosShareBridge { inner: share }),
            stt: Some(Arc::new(IosSttBridge { inner: stt })),
            tts: Some(Arc::new(IosTtsBridge { inner: tts })),
            notifications: Some(Arc::new(IosNotificationBridge {
                inner: notifications,
            })),
            clipboard: Some(Arc::new(IosClipboardBridge { inner: clipboard })),
            secure_storage: secure_storage
                .map(|s| Arc::new(IosSecureStorageBridge { inner: s }) as Arc<dyn traits::SecureStorage>),
        }));
        let permission_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(IosPermissionSinkBridge { inner: permissions });
        engine_mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (
            api_base,
            api_key,
            model,
            app_sandbox_root,
            listener,
            stt,
            tts,
            camera,
            share,
            voice,
            notifications,
            clipboard,
            permissions,
            secure_storage,
        );
        Err(MobileEngineError::PlatformUnavailable)
    }
}

// F3-04: re-export `engine-mobile`'s UniFFI scaffolding so the shared host's FFI
// symbols (the re-exported `MobileEngineHandle` / `MobileEngineError`) land in
// this crate's final library. Under the `uniffi` feature only.
#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

#[cfg(all(test, feature = "uniffi"))]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use client_protocol::events::ClientEvent;
    use client_protocol::permission::PermissionRequest as PermissionRequestDto;
    use engine_mobile::{ClientEventListener, MobileConfig, PermissionRequestSink};
    use tokio::sync::Mutex;
    use traits::{
        CameraControl, Clock, FileSystem, HttpTransport, Platform, ProcessRunner, Sandbox,
        SharingService, VoiceRecorder, WorktreeManager,
    };

    /// Off-device fake [`Platform`] shim (portable `platform-posix-minimal`
    /// handles over a temp root). Lets the SHARED `build_mobile_engine` build a
    /// real handle on CI without an iOS device — exactly the spec §8 "prove from
    /// a Swift unit test", run on the host.
    struct HostFakePlatform {
        fs: Arc<dyn FileSystem>,
        http: Arc<dyn HttpTransport>,
        clock: Arc<dyn Clock>,
        process: Arc<dyn ProcessRunner>,
        sandbox: Arc<dyn Sandbox>,
        worktree: Arc<dyn WorktreeManager>,
    }

    impl HostFakePlatform {
        fn new(root: std::path::PathBuf) -> Self {
            use platform_posix_minimal::{
                PosixClock, PosixFileSystem, PosixHttp, PosixProcess, PosixSandbox, PosixWorktree,
            };
            Self {
                fs: Arc::new(PosixFileSystem::new(root)),
                http: Arc::new(PosixHttp::new()),
                clock: Arc::new(PosixClock::new()),
                process: Arc::new(PosixProcess::new()),
                sandbox: Arc::new(PosixSandbox::new()),
                worktree: Arc::new(PosixWorktree::new()),
            }
        }
    }

    impl Platform for HostFakePlatform {
        fn filesystem(&self) -> Arc<dyn FileSystem> {
            self.fs.clone()
        }
        fn http(&self) -> Arc<dyn HttpTransport> {
            self.http.clone()
        }
        fn clock(&self) -> Arc<dyn Clock> {
            self.clock.clone()
        }
        fn process(&self) -> Arc<dyn ProcessRunner> {
            self.process.clone()
        }
        fn sandbox(&self) -> Arc<dyn Sandbox> {
            self.sandbox.clone()
        }
        fn worktree(&self) -> Arc<dyn WorktreeManager> {
            self.worktree.clone()
        }
        fn camera(&self) -> Option<Arc<dyn CameraControl>> {
            None
        }
        fn voice(&self) -> Option<Arc<dyn VoiceRecorder>> {
            None
        }
        fn share(&self) -> Option<Arc<dyn SharingService>> {
            None
        }
    }

    /// A host-fake [`ClientEventListener`] that records every delivered event.
    #[derive(Default)]
    struct FakeListener {
        received: Mutex<Vec<ClientEvent>>,
    }

    #[async_trait]
    impl ClientEventListener for FakeListener {
        async fn on_event(&self, event: ClientEvent) {
            self.received.lock().await.push(event);
        }
    }

    /// A [`PermissionRequestSink`] that records the gate's outbound requests.
    #[derive(Default)]
    struct RecordingPermissionSink {
        count: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl PermissionRequestSink for RecordingPermissionSink {
        async fn emit_request(&self, _request: PermissionRequestDto) {
            self.count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn build_handle(
        root: &std::path::Path,
    ) -> Arc<engine_mobile::MobileEngineHandle> {
        let platform: Arc<dyn Platform> = Arc::new(HostFakePlatform::new(root.to_path_buf()));
        let listener: Arc<dyn ClientEventListener> = Arc::new(FakeListener::default());
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let cfg = MobileConfig {
            cwd: root.to_path_buf(),
            claude_home: root.join(".lingxi"),
            ..MobileConfig::default()
        };
        engine_mobile::build_mobile_engine(cfg, platform, listener, perm_sink)
            .expect("shared build_mobile_engine failed")
    }

    /// F3-04: the re-exported [`MobileEngineHandle`] holds the handle-owned tokio
    /// runtime AND the registered listener AND the connection-scoped permission
    /// gate — the grown-up form of the M8 stub (which held only a `Platform` +
    /// `skill_count`).
    #[test]
    fn handle_holds_runtime_and_listener() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let handle = build_handle(tmp.path());

        // Owns a live tokio runtime — drive a trivial future on it to prove it.
        let two = handle.runtime().block_on(async { 1 + 1 });
        assert_eq!(two, 2);

        // Holds the wired runtime: the orchestrator + the connection-scoped
        // adapter permission gate + the registered listener are all reachable.
        let _orch: Arc<orchestrator::ConversationOrchestrator> =
            handle.inner().orchestrator.clone();
        let _gate = handle.permission_gate();
        let _listener: Arc<dyn ClientEventListener> = handle.listener();

        // The M8 smoke signal still works: `skill_count` reflects the assembled
        // mobile builtin skill set (currently empty — mobile builtin skills are
        // markdown loaded from disk, not Rust-bundled — so it is 0, matching
        // `skill_api::builtin::BUILTIN_MOBILE`). The signal is that the call resolves
        // against the real wired handle, not that the count is non-zero.
        assert_eq!(handle.skill_count(), 0);
    }

    /// F3-04: `create_session` is no longer the M8 stub (which returned an
    /// `Internal` error). With a real wired runtime it returns the connection's
    /// live session ref.
    #[test]
    fn create_session_no_longer_stubbed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let handle = build_handle(tmp.path());

        let session = handle
            .create_session("claude-sonnet-4-20250514".to_string())
            .expect("create_session must no longer be stubbed");
        assert_eq!(session, 1);
    }
}
