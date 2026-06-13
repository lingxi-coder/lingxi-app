//! `android-aar` (M8-P12 → M10-F3) — the Android `UniFFI` packager.
//!
//! The FFI boundary between the Rust engine and the Android app. The Kotlin
//! layer implements the [`traits::CameraControl`] / [`traits::VoiceRecorder`] /
//! [`traits::SharingService`] callback interfaces (skeletons under `kotlin/`),
//! hands them across as a [`PlatformImpls`] record, and Rust uses them to build
//! an `AndroidPlatform` and assemble the mobile engine — Rust calls *back* into
//! Kotlin for native capabilities.
//!
//! ## Shared session host (F3-04)
//!
//! The real session host — [`MobileEngineHandle`] (owns the handle-owned tokio
//! runtime + the wired `MobileRuntime` + the registered `ClientEventListener`)
//! and its [`MobileEngineError`] — lives in `engine-mobile` and is RE-EXPORTED
//! here, NOT re-derived. That single-source rule (plan F3-04) is what stops iOS
//! and Android from drifting: this crate only adds the Android-specific
//! `Platform`-construction wrapper around the shared `build_mobile_engine`.
//!
//! ## Inbound command path (F3-05)
//!
//! The async FFI entry point — `MobileEngineHandle::submit(ClientCommand) ->
//! Result<(), ClientError>` (under `uniffi`: `#[uniffi::export(async_runtime =
//! "tokio")]`) — is defined ONCE on the shared host in `engine-mobile` and
//! reaches Kotlin through the re-exported [`MobileEngineHandle`]. There is no
//! Android-specific submit body: `SendPrompt` spawns the streaming turn on the
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
//! engine host owning one runtime) — so Kotlin's `suspend` calls never block the
//! main thread and resolve on the engine's own runtime. The
//! `async_submit_resolves_on_handle_runtime` host test proves the registration by
//! asserting an async export resolves on exactly that runtime.
//!
//! ## `UniFFI` status
//! The `uniffi` feature (default-on) lights up the real `UniFFI` surface: the
//! re-exported [`MobileEngineHandle`] is a `#[derive(uniffi::Object)]`, the
//! listener a callback interface, the DTOs `UniFFI` types. `engine-mobile` carries
//! the `setup_scaffolding!()`; this crate re-exports it (and adds its own for
//! the Android-local exports) so the symbols land in the final library.

#![forbid(unsafe_code)]

use std::sync::Arc;
use traits::{CameraControl, SharingService, VoiceRecorder};
// `Platform` is named only inside the `cfg(target_os = "android")` constructor
// body; importing it unconditionally warns on the host build, so scope it.
#[cfg(all(feature = "uniffi", target_os = "android"))]
use traits::Platform;

// F3-04: the shared session host + its error type are DEFINED ONCE in
// `engine-mobile` and re-exported here. Both FFI packager crates re-export the
// SAME types so iOS and Android cannot drift (plan F3-04).
#[cfg(feature = "uniffi")]
pub use engine_mobile::{
    ClientEventListener, MobileConfig, MobileEngineError, MobileEngineHandle, PermissionRequestSink,
};

/// The foreign (Kotlin) capability objects + config needed to build an
/// `AndroidPlatform`. `UniFFI` marshals each `Arc<dyn …>` as a callback-interface
/// reference; `app_files_root` is the app's private files-dir.
pub struct PlatformImpls {
    /// Kotlin `CameraControl` impl (`CameraX`).
    pub camera: Arc<dyn CameraControl>,
    /// Kotlin `VoiceRecorder` impl (`MediaRecorder`).
    pub voice: Arc<dyn VoiceRecorder>,
    /// Kotlin `SharingService` impl (`Intent.ACTION_SEND`).
    pub share: Arc<dyn SharingService>,
    /// The app's private files-dir root.
    pub app_files_root: String,
}

/// FFI carrier for the Android shell/sandbox configuration (spec r3 §Android
/// inputs). `None` anywhere upstream keeps shell support fully absent.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct AndroidShellConfigFfi {
    /// `ApplicationInfo.nativeLibraryDir`.
    pub native_library_dir: String,
    /// Directory the shell treats as `$HOME` / workspace.
    pub shell_workspace_root: String,
    /// App cache dir (`$TMPDIR`).
    pub app_cache_root: String,
    /// Application package name.
    pub package_name: String,
    /// `PackageInfo.longVersionCode`.
    pub package_version_code: i64,
    /// filesDir / cacheDir / codeCacheDir / noBackupFilesDir roots.
    pub app_writable_roots: Vec<String>,
    /// Master enable flag.
    pub enable_shell: bool,
    /// D11: host attests secrets are Keystore-backed.
    pub secrets_in_keystore: bool,
    /// D11: explicit user acceptance of data exposure.
    pub shell_data_exposure_accepted: bool,
}

/// FFI carrier for the Android `Git`-tool configuration (spec P4 §G5 gate +
/// §G3 auth). `None`/`null` anywhere upstream keeps Git support fully absent.
///
/// Mirrors [`AndroidShellConfigFfi`]: the Kotlin host supplies the enable flag,
/// the repository workspace root, the system CA-certificate directory, and the
/// in-memory HTTPS token. The token rides this FFI record only long enough to be
/// copied into the engine's `AndroidGitSecret` (held outside the broadly-cloned
/// public [`tool_api::AndroidGitToolCtx`]); it is never written to disk or a
/// child-process env (libgit2 is in-process).
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct AndroidGitConfigFfi {
    /// Master enable flag for the Git tool.
    pub enable_git: bool,
    /// App-private repository root (absolute path); all git ops are anchored here.
    pub workspace_root: String,
    /// System CA-certificate directory for TLS verification. Empty = use the
    /// libgit2/OpenSSL defaults.
    pub ca_cert_dir: String,
    /// In-memory HTTPS token (PAT) for network ops, or `None` for public remotes.
    pub https_token: Option<String>,
    /// Filesystem path to the SSH private key (spec §G7), or empty for
    /// HTTPS-only. Host-supplied; validated to stay inside `app_files_root`
    /// before reaching the engine secret seam (defense-in-depth — see
    /// [`build_android_engine`]'s git mapping).
    pub ssh_private_key_path: String,
    /// Path to the matching SSH public key, or empty (libssh2 derives it from
    /// the private key).
    pub ssh_public_key_path: String,
    /// Passphrase decrypting the SSH private key, or `None`. In-memory only.
    pub ssh_passphrase: Option<String>,
    /// Pinned SSH host-key fingerprints (lowercase-hex SHA-256). An empty list
    /// rejects every host key (fail-closed).
    pub ssh_known_hosts_sha256_hex: Vec<String>,
}

/// Top-level `UniFFI` constructor: build the mobile engine from the Kotlin-supplied
/// platform callbacks + event listener. (Under `uniffi`: `#[uniffi::export]`.)
///
/// This is a THIN wrapper: it constructs the Android-specific `Platform` from the
/// foreign callbacks and then delegates ALL runtime/adapter/listener wiring to
/// the shared [`engine_mobile::build_mobile_engine`] (F3-04) — so the heavy
/// lifting lives in exactly one place. The returned [`MobileEngineHandle`] owns
/// the tokio runtime + the wired orchestrator + the registered listener.
///
/// On non-Android hosts this returns [`MobileEngineError::PlatformUnavailable`] —
/// the `AndroidPlatform` is only linked under `cfg(target_os = "android")` — so
/// the crate still compiles and the SHARED host is exercised off-device through
/// the test shim (which calls `build_mobile_engine` with a portable fake
/// `Platform`).
#[cfg(feature = "uniffi")]
pub fn build_mobile_engine(
    impls: PlatformImpls,
    listener: Arc<dyn ClientEventListener>,
    permission_sink: Arc<dyn PermissionRequestSink>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    #[cfg(target_os = "android")]
    {
        use platform_android::{AndroidPlatform, AndroidPlatformInputs};
        let cfg = MobileConfig {
            cwd: std::path::PathBuf::from(&impls.app_files_root),
            claude_home: std::path::PathBuf::from(&impls.app_files_root).join(".claude"),
            ..MobileConfig::default()
        };
        let platform: Arc<dyn Platform> = Arc::new(AndroidPlatform::new(AndroidPlatformInputs {
            app_files_root: std::path::PathBuf::from(impls.app_files_root),
            camera: impls.camera,
            voice: impls.voice,
            share: impls.share,
            stt: None,
            tts: None,
            notifications: None,
            clipboard: None,
            shell: None,
        }));
        engine_mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (impls, listener, permission_sink);
        Err(MobileEngineError::PlatformUnavailable)
    }
}

// ---------------------------------------------------------------------------
// T2.1 — foreign (Kotlin) speech callback interfaces + their engine bridges.
// ---------------------------------------------------------------------------
//
// The Kotlin layer implements two crate-local async callback interfaces —
// `AndroidStt` (system `SpeechRecognizer`) and `AndroidTts` (system
// `TextToSpeech`) — and hands them across the FFI seam. The engine consumes the
// SHARED `traits::SpeechToText` / `traits::TextToSpeech` seams, so a thin bridge
// struct adapts each crate-local interface to its `traits` counterpart.
//
// These interfaces are DEFINED IN THIS CRATE (mirroring the `IosEventListener`
// pattern) so their UniFFI `FfiConverter`s register under `android_aar`'s tag —
// a prerequisite for naming them as parameter types in a `#[uniffi::export]`
// constructor here.
//
// RETURN SHAPE (UniFFI 0.28.3): async callback-interface methods return
// `Result<T, E>` where `E` is a `#[derive(uniffi::Error)]` enum — this is the
// supported async-callback fallible shape on 0.28. The bridge maps the FFI
// error variants onto the richer `SttError` / `TtsError` (mic-permission →
// `PermissionDenied`, etc.).

/// FFI error surface for the Android speech callback interfaces. A flat enum so
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

/// Crate-local foreign callback interface for native speech-to-text — the Kotlin
/// app implements it over the system `SpeechRecognizer` (opens the live mic,
/// listens for one utterance, returns the final transcript). Bridged to
/// [`traits::SpeechToText`] by [`AndroidSttBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidStt: Send + Sync {
    /// Open the mic, listen for a single utterance, and return the recognized
    /// text. `language` is a BCP-47 hint (`None` = device default).
    async fn transcribe(&self, language: Option<String>) -> Result<String, SpeechFfiError>;
}

/// Crate-local foreign callback interface for native text-to-speech — the Kotlin
/// app implements it over the system `TextToSpeech` engine, returning 16-bit
/// signed little-endian mono PCM. Bridged to [`traits::TextToSpeech`] by
/// [`AndroidTtsBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidTts: Send + Sync {
    /// Synthesize `text` to PCM16 audio at [`TtsAudioFfi::sample_rate_hz`].
    /// `voice` is a provider-specific id (`None` = system default voice).
    async fn synthesize(
        &self,
        text: String,
        voice: Option<String>,
    ) -> Result<TtsAudioFfi, SpeechFfiError>;
}

/// FFI carrier for synthesized audio crossing the callback-interface seam:
/// PCM16 frames + the sample rate the Kotlin engine produced them at.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct TtsAudioFfi {
    /// Raw PCM16 frames (16-bit signed little-endian, mono).
    pub pcm: Vec<u8>,
    /// Sample rate of `pcm` in Hz.
    pub sample_rate_hz: u32,
}

// ---------------------------------------------------------------------------
// Share — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidStt/AndroidTts/AndroidCamera pattern: the Kotlin layer
// implements a crate-local async `AndroidShare` callback interface (the system
// `Intent.ACTION_SEND` share sheet) and hands it across the FFI seam. The
// engine consumes the SHARED `traits::SharingService` seam, so
// `AndroidShareBridge` adapts the crate-local interface to its `traits`
// counterpart. The shared `traits::SharePayload` is destructured into the three
// flat `text` / `url` / `image_bytes` args to keep the FFI flat; the bridge
// maps the FFI result/error back onto `traits::ShareResult` / `traits::ShareError`.

/// FFI error surface for the Android share callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::ShareError`].
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

/// Crate-local foreign callback interface for native sharing — the Kotlin app
/// implements it over the system `Intent.ACTION_SEND` share sheet. Bridged to
/// [`traits::SharingService`] by [`AndroidShareBridge`]. The payload crosses the
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
/// [`traits::SharingService`] seam the engine consumes. Destructures
/// [`traits::SharePayload`] into the flat `text` / `url` / `image_bytes` args
/// and fans [`ShareResultFfi`] / [`ShareFfiError`] back out onto
/// [`traits::ShareResult`] / [`traits::ShareError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidShareBridge {
    inner: Box<dyn AndroidShare>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::SharingService for AndroidShareBridge {
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

// ---------------------------------------------------------------------------
// Notifications — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidShare pattern: the Kotlin layer implements a crate-local
// async `AndroidNotification` callback interface (the system
// `NotificationManager`) and hands it across the FFI seam. The engine consumes
// the SHARED `traits::NotificationService` seam, so `AndroidNotificationBridge`
// adapts the crate-local interface to its `traits` counterpart. The shared
// `traits::NotificationRequest` is destructured into the flat `title` / `body`
// / `tag` args to keep the FFI flat; the bridge maps the FFI error back onto
// `traits::NotificationError`. This is ENGINE-DRIVEN by `tool-notification`
// (the model posts a notification) — no user-facing UI affordance.

/// FFI error surface for the Android notification callback interface. A flat
/// enum so `UniFFI` can render it for an async `callback_interface` method; the
/// bridge fans it back out onto the richer [`traits::NotificationError`].
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
/// [`traits::NotificationService`] by [`AndroidNotificationBridge`]. The request
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
/// shared [`traits::NotificationService`] seam the engine consumes.
/// Destructures [`traits::NotificationRequest`] into the flat `title` / `body`
/// / `tag` args and fans [`NotificationFfiError`] back out onto
/// [`traits::NotificationError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidNotificationBridge {
    inner: Box<dyn AndroidNotification>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::NotificationService for AndroidNotificationBridge {
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

// ---------------------------------------------------------------------------
// Clipboard — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidNotification pattern: the Kotlin layer implements a
// crate-local async `AndroidClipboard` callback interface (the system
// `ClipboardManager`) and hands it across the FFI seam. The engine consumes
// the SHARED `traits::Clipboard` seam, so `AndroidClipboardBridge` adapts the
// crate-local interface to its `traits` counterpart; the bridge maps the FFI
// error back onto `traits::ClipboardError`. This is ENGINE-DRIVEN by
// `tool-clipboard` (the model reads/writes the pasteboard) — no user-facing UI
// affordance. NOTE Android 10+ restricts clipboard READS to the focused app /
// default IME — when a read is not permitted the Kotlin side returns `None`
// gracefully rather than crashing.

/// FFI error surface for the Android clipboard callback interface. A flat enum
/// so `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::ClipboardError`].
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
/// `primaryClip.getItemAt(0).coerceToText`). Bridged to [`traits::Clipboard`]
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
/// [`traits::Clipboard`] seam the engine consumes. One forwarding hop per call;
/// maps [`ClipboardFfiError`] back out onto [`traits::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidClipboardBridge {
    inner: Box<dyn AndroidClipboard>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::Clipboard for AndroidClipboardBridge {
    async fn set_text(&self, text: String) -> Result<(), traits::ClipboardError> {
        self.inner
            .set_text(text)
            .await
            .map_err(clipboard_error_from_ffi)
    }
    async fn get_text(&self) -> Result<Option<String>, traits::ClipboardError> {
        self.inner
            .get_text()
            .await
            .map_err(clipboard_error_from_ffi)
    }
}

/// Fan a flat [`ClipboardFfiError`] back out onto the richer
/// [`traits::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn clipboard_error_from_ffi(e: ClipboardFfiError) -> traits::ClipboardError {
    match e {
        ClipboardFfiError::Unsupported => traits::ClipboardError::Unsupported,
        ClipboardFfiError::Other { message } => traits::ClipboardError::Other(message),
    }
}

// ---------------------------------------------------------------------------
// Camera — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidStt/AndroidTts speech pattern: the Kotlin layer implements
// a crate-local async `AndroidCamera` callback interface (CameraX capture +
// system photo picker) and hands it across the FFI seam. The engine consumes
// the SHARED `traits::CameraControl` seam, so `AndroidCameraBridge` adapts the
// crate-local interface to its `traits` counterpart. Camera position crosses
// the seam as a plain `front: bool` (true = front/selfie, false = rear) to keep
// the FFI flat; the bridge maps it to `traits::CameraPosition`.

/// FFI error surface for the Android camera callback interface. A flat enum so
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

/// Crate-local foreign callback interface for native camera access — the Kotlin
/// app implements it over `CameraX` (capture) and the system photo picker
/// (library). Bridged to [`traits::CameraControl`] by [`AndroidCameraBridge`].
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
/// [`traits::CameraControl`] seam the engine consumes. Maps
/// [`traits::CameraPosition`] onto the flat `front` bool, threads
/// `allow_editing`, and fans [`CameraFfiError`] back out onto
/// [`traits::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidCameraBridge {
    inner: Box<dyn AndroidCamera>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::CameraControl for AndroidCameraBridge {
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
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn captured_image_from_ffi(img: CapturedImageFfi) -> traits::CapturedImage {
    traits::CapturedImage {
        jpeg_bytes: img.jpeg_bytes,
        width: img.width,
        height: img.height,
    }
}

/// Fan a flat [`CameraFfiError`] back out onto the richer [`traits::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn camera_error_from_ffi(e: CameraFfiError) -> traits::CameraError {
    match e {
        CameraFfiError::PermissionDenied => traits::CameraError::PermissionDenied,
        CameraFfiError::Cancelled => traits::CameraError::Cancelled,
        CameraFfiError::DeviceUnavailable => traits::CameraError::DeviceUnavailable,
        CameraFfiError::Other { message } => traits::CameraError::Other(message),
    }
}

// ---------------------------------------------------------------------------
// Voice — foreign (Kotlin) mic-recorder callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidShare/AndroidCamera pattern: the Kotlin layer implements a
// crate-local async `AndroidVoice` callback interface (the system
// `MediaRecorder` capturing the raw mic) and hands it across the FFI seam. The
// engine consumes the SHARED `traits::VoiceRecorder` seam, so
// `AndroidVoiceBridge` adapts the crate-local interface to its `traits`
// counterpart. This is the RAW mic recorder driven by `tool-voice`
// (start/stop/is_recording), distinct from the `AndroidStt` system recognizer.
// `traits::VoiceRecordingOpts` is destructured into the flat `sample_rate_hz` /
// `format` args to keep the FFI flat; the bridge maps the FFI result/error back
// onto `traits::VoiceRecording` / `traits::VoiceError`.

/// FFI error surface for the Android mic-recorder callback interface. A flat
/// enum so `UniFFI` can render it for an async `callback_interface` method; the
/// bridge fans it back out onto the richer [`traits::VoiceError`].
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

/// Crate-local foreign callback interface for native mic recording — the Kotlin
/// app implements it over the system `MediaRecorder`. Bridged to
/// [`traits::VoiceRecorder`] by [`AndroidVoiceBridge`]. Driven by the engine
/// through `tool-voice` (start/stop/is_recording); the recording opts cross the
/// seam as the flat `sample_rate_hz` / `format` args.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidVoice: Send + Sync {
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

/// Adapts the crate-local [`AndroidVoice`] callback interface to the shared
/// [`traits::VoiceRecorder`] seam the engine consumes. Destructures
/// [`traits::VoiceRecordingOpts`] into the flat `sample_rate_hz` / `format`
/// args, converts [`VoiceRecordingFfi`] back to [`traits::VoiceRecording`], and
/// fans [`VoiceFfiError`] back out onto [`traits::VoiceError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidVoiceBridge {
    inner: Box<dyn AndroidVoice>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::VoiceRecorder for AndroidVoiceBridge {
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
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn voice_error_from_ffi(e: VoiceFfiError) -> traits::VoiceError {
    match e {
        VoiceFfiError::PermissionDenied => traits::VoiceError::PermissionDenied,
        VoiceFfiError::NotRecording => traits::VoiceError::NotRecording,
        VoiceFfiError::Other { message } => traits::VoiceError::Other(message),
    }
}

/// Adapts the crate-local [`AndroidStt`] callback interface to the shared
/// [`traits::SpeechToText`] seam the engine consumes. One forwarding hop per
/// call; maps [`SpeechFfiError`] onto [`traits::SttError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidSttBridge {
    inner: Box<dyn AndroidStt>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::SpeechToText for AndroidSttBridge {
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

/// Adapts the crate-local [`AndroidTts`] callback interface to the shared
/// [`traits::TextToSpeech`] seam the engine consumes. Maps [`SpeechFfiError`]
/// onto [`traits::TtsError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidTtsBridge {
    inner: Box<dyn AndroidTts>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::TextToSpeech for AndroidTtsBridge {
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

// ---------------------------------------------------------------------------
// T2.2 — the foreign-callable Android engine constructor.
// ---------------------------------------------------------------------------
//
// Mirrors `ios-framework::build_ios_engine`: a thin `#[uniffi::export]` wrapper
// taking ONLY UniFFI-marshalable inputs (the crate-local event listener + the
// stt/tts callback objects + plain config strings), constructing stub camera /
// voice / share capabilities + a no-op permission sink, threading the runtime
// config into a `MobileConfig`, and delegating to the shared
// `engine_mobile::build_mobile_engine`. ADDITIVE — it does not touch the
// existing non-exported `build_mobile_engine` above, the `traits` crate, or iOS.

/// Device-capability stubs for the camera / voice / share callbacks the Android
/// constructor does not (yet) wire. A text/speech conversation never invokes
/// these; each returns the trait's "unavailable" error. Mirrors
/// `ios-framework`'s `stub_capabilities`.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
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

/// A [`PermissionRequestSink`] that drops outbound permission requests (mirrors
/// `ios-framework`'s `NoopPermissionSink`). Mobile always binds the adapter
/// permission gate; with no foreign permission UI yet, an unanswered request
/// simply parks the turn (still cancellable).
#[cfg(feature = "uniffi")]
// Reference implementation mirroring `ios-framework`'s `NoopPermissionSink`; the
// Android constructor binds `AndroidPermissionSinkBridge` instead, so this is
// unconstructed on every target — keep it as the documented no-op shape.
#[allow(dead_code)]
struct NoopPermissionSink;

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl engine_mobile::PermissionRequestSink for NoopPermissionSink {
    async fn emit_request(&self, _request: client_protocol::permission::PermissionRequest) {}
}

/// The Kotlin-implemented permission sink the Android app registers when it builds
/// the engine. Defined in THIS crate (not re-used from `engine-mobile`) so its
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
/// shared [`PermissionRequestSink`] the engine's adapter gate emits onto. One
/// forwarding hop per request; no transformation. Mirrors [`AndroidListenerBridge`].
///
/// Constructed only on the `target_os = "android"` path of
/// [`build_android_engine`]; `allow(dead_code)` on the host bindgen build (where
/// that path is `cfg`'d out).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidPermissionSinkBridge {
    inner: Box<dyn AndroidPermissionSink>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl engine_mobile::PermissionRequestSink for AndroidPermissionSinkBridge {
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
}

/// Adapts the crate-local [`AndroidEventListener`] callback interface to the
/// shared [`ClientEventListener`] the engine's adapter sink expects. One
/// forwarding hop per event; no transformation.
#[cfg(feature = "uniffi")]
struct AndroidListenerBridge {
    inner: Box<dyn AndroidEventListener>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl ClientEventListener for AndroidListenerBridge {
    async fn on_event(&self, event: client_protocol::events::ClientEvent) {
        self.inner.on_event(event).await;
    }
}

/// The mobile `Shell`-tool registration gate (spec r3 §Registration gates +
/// D11, P5b §B2): enabled iff config opts in AND the device probe proves the
/// enforcement we promise AND the bundled mksh+toybox bootstrap succeeded. Pure;
/// host-testable (NOT `cfg(target_os)`-gated, so the host tests reach it). The
/// six conjuncts are `enable_shell` + the D11 secrets gate (config opt-in),
/// capability-available + seccomp-filter + net-deny-verified (probe-proven
/// enforcement), and `bundled_shell_ready` (P5b: the bundled shell was
/// bootstrapped and exec-verified). The bundled conjunct makes the gate
/// fail-closed — if the bundled mksh+toybox cannot be staged/exec'd, the Shell
/// stays absent rather than falling back to the device's system sh.
///
/// Called from the `cfg(target_os = "android")` branch of
/// [`build_android_engine`]; on the host build the only caller is the unit test,
/// so `allow(dead_code)` there (mirrors the file's other host-unused items).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
// The six conjuncts ARE distinct boolean gate inputs (spec r3 §Registration
// gates + D11 + P5b §B2) named 1:1 at the single call site; a struct/enum would
// obscure the formula, not clarify it.
#[allow(clippy::fn_params_excessive_bools)]
#[must_use]
fn android_shell_gate(
    enable_shell: bool,
    secrets_gate_satisfied: bool,
    caps_available: bool,
    seccomp_filter: bool,
    net_deny_verified: bool,
    bundled_shell_ready: bool,
) -> bool {
    enable_shell
        && secrets_gate_satisfied
        && caps_available
        && seccomp_filter
        && net_deny_verified
        && bundled_shell_ready
}

/// The mobile `Git`-tool registration gate (spec P4 §G5): enabled iff config
/// opts in AND the workspace is a ready directory AND the CA store is reachable.
/// The token is deliberately NOT part of the gate (spec §G5) — a missing token
/// disables only the network ops (clone/fetch/pull), surfaced via `has_token`.
/// Pure; host-testable (NOT `cfg(target_os)`-gated, so the host tests reach it).
///
/// Called from the `cfg(target_os = "android")` branch of
/// [`build_android_engine`]; on the host build the only caller is the unit test,
/// so `allow(dead_code)` there (mirrors the file's other host-unused items).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
#[must_use]
fn android_git_gate(enable_git: bool, workspace_ready: bool, ca_store_reachable: bool) -> bool {
    enable_git && workspace_ready && ca_store_reachable
}

/// P5b bundled-shell bootstrap result — the typed values the android branch of
/// [`build_android_engine`] threads onto `AndroidShellConfig` (so `prepare`
/// targets the bundled mksh + leads PATH with the applet farm) and onto the
/// capability cache + Shell-tool ctx. `Some` ONLY when staging + exec
/// verification fully succeeded (spec P5b §B2 fail-closed).
#[cfg(target_os = "android")]
struct BundledShell {
    mksh_path: std::path::PathBuf,
    mksh_hash: String,
    applet_dir: std::path::PathBuf,
    mksh_version: Option<String>,
}

/// P5b bundled-shell bootstrap: stage the toybox applet symlink farm in an
/// app-private dir and prove the version-locked bundled mksh+toybox shipped as
/// `libmksh.so`/`libtoybox.so` under `native_library_dir` execve + dispatch
/// end-to-end, then hash `libmksh.so` for the runner's content-identity check
/// (spec P5b §T4b — the recorded hash MUST equal the real sha256 or the runner
/// refuses to exec).
///
/// FAIL-CLOSED (spec P5b §B2): ANY I/O / staging / exec-verification failure
/// returns `None`, which drops the bundled config fields and flips the gate's
/// `bundled_shell_ready` conjunct false — the Shell then stays absent rather
/// than falling back to the device's system sh.
///
/// The applet-resolution mechanism is the symlink farm
/// (`<applet_dir>/<applet>` → `<native_library_dir>/libtoybox.so`): toybox
/// multicall-dispatches on `argv[0]`, so a symlink farm is the ONLY mechanism
/// (command-rewrite does not work). This mirrors the device-proven
/// [`android_bundled_shell_probe`] logic but returns typed values.
#[cfg(target_os = "android")]
fn bootstrap_bundled_shell(native_library_dir: &str, app_files_root: &str) -> Option<BundledShell> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use std::process::Command;

    let nl = Path::new(native_library_dir);
    let mksh = nl.join("libmksh.so");
    let toybox = nl.join("libtoybox.so");
    if !mksh.exists() || !toybox.exists() {
        return None;
    }

    // 1) (re)build the applet symlink farm in an app-private dir. Wipe any stale
    //    farm first so a re-launch always reflects the current bundled toybox.
    let applet_dir = Path::new(app_files_root).join("applet-bin");
    let _ = std::fs::remove_dir_all(&applet_dir);
    std::fs::create_dir_all(&applet_dir).ok()?;
    for applet in platform_android::capabilities::BUNDLED_TOYBOX_APPLETS {
        let link = applet_dir.join(applet);
        // symlink <applet_dir>/<applet> -> <nl>/libtoybox.so; tolerate a
        // pre-existing link (e.g. a racing relaunch) but fail closed otherwise.
        if symlink(&toybox, &link).is_err() && !link.exists() {
            return None;
        }
    }

    // 2) verify bundled exec end-to-end: bare mksh execve, then a toybox applet
    //    resolved via the symlink farm on PATH (proves argv[0] dispatch).
    let mksh_ok = Command::new(&mksh)
        .args(["-c", "echo hi"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("hi"))
        .unwrap_or(false);
    if !mksh_ok {
        return None;
    }
    let applet_ok = Command::new(&mksh)
        .args(["-c", "echo hi | grep hi"])
        .env("PATH", &applet_dir)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("hi"))
        .unwrap_or(false);
    if !applet_ok {
        return None;
    }

    // 3) hash libmksh.so for the runner identity check (must match what
    //    `prepare` records as `BundledHelper.hash`).
    let bytes = std::fs::read(&mksh).ok()?;
    let mksh_hash = hex::encode(Sha256::digest(&bytes));

    // 4) bundled mksh version (best-effort; mksh prints `$KSH_VERSION`).
    let mksh_version = Command::new(&mksh)
        .args(["-c", "echo $KSH_VERSION"])
        .output()
        .ok()
        .and_then(|o| {
            let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        });

    Some(BundledShell {
        mksh_path: mksh,
        mksh_hash,
        applet_dir,
        mksh_version,
    })
}

/// Foreign-callable constructor for the Android app (plan T2.2).
///
/// Builds a fully-wired [`MobileEngineHandle`] from the Kotlin-supplied event
/// listener + speech callbacks + runtime config. The handle owns its tokio
/// runtime and streams every [`client_protocol::events::ClientEvent`] to
/// `listener.on_event(..)`; the app drives turns via
/// [`MobileEngineHandle::submit`]. The `stt` / `tts` callbacks are bridged into
/// the mobile `Platform` so `tool-speech` can route through the device's native
/// recognizer / synthesizer.
///
/// - `api_base`  — Anthropic-compatible base URL.
/// - `api_key`   — read by Kotlin from an app setting at runtime. Empty is valid
///   (turns 401 at `run_turn`); never hardcoded here.
/// - `model`     — default model id for new turns.
/// - `app_files_root` — the app's private files-dir the engine roots its
///   filesystem + `~/.claude`-equivalent under.
/// - `listener`  — the foreign [`AndroidEventListener`] (bridged to the shared
///   [`ClientEventListener`]).
/// - `stt` / `tts` — the foreign speech callbacks (bridged to
///   [`traits::SpeechToText`] / [`traits::TextToSpeech`]).
/// - `camera` — the foreign camera callback (bridged to
///   [`traits::CameraControl`]) so `tool-camera` routes through `CameraX` +
///   the system photo picker.
/// - `share` — the foreign share callback (bridged to
///   [`traits::SharingService`]) so `tool-share` routes through the system
///   `Intent.ACTION_SEND` share sheet.
/// - `shell` — optional Android sandbox/shell config (spec r3 §Android
///   inputs); `None`/`null` keeps shell support fully absent.
/// - `git` — optional Android Git-tool config (spec P4 §G5 gate + §G3 auth);
///   `None`/`null` keeps Git support fully absent.
///
/// On non-Android hosts this returns [`MobileEngineError::PlatformUnavailable`]
/// (the `AndroidPlatform` is only linked under `cfg(target_os = "android")`).
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::too_many_arguments)]
// FFI constructor: one flat arg per Kotlin callback.
// Single linear constructor body: probe → shell gate → git gate → delegate. The
// per-tool gate blocks (spec r3 §Registration gates + P4 §G5) read most clearly
// inline at the one call site, so the length is intrinsic, not decomposable.
#[allow(clippy::too_many_lines)]
pub fn build_android_engine(
    api_base: String,
    api_key: String,
    model: String,
    app_files_root: String,
    listener: Box<dyn AndroidEventListener>,
    stt: Box<dyn AndroidStt>,
    tts: Box<dyn AndroidTts>,
    camera: Box<dyn AndroidCamera>,
    share: Box<dyn AndroidShare>,
    voice: Box<dyn AndroidVoice>,
    notifications: Box<dyn AndroidNotification>,
    clipboard: Box<dyn AndroidClipboard>,
    permissions: Box<dyn AndroidPermissionSink>,
    shell: Option<AndroidShellConfigFfi>,
    git: Option<AndroidGitConfigFfi>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    let listener: Arc<dyn ClientEventListener> =
        Arc::new(AndroidListenerBridge { inner: listener });
    #[cfg(target_os = "android")]
    {
        use platform_android::{AndroidPlatform, AndroidPlatformInputs};
        // P5b: capture the app-private files root as an owned `String` up front —
        // `app_files_root` is consumed below into `AndroidPlatformInputs`, but the
        // bundled-shell bootstrap (which must run BEFORE `shell_cfg` is built +
        // moved) needs it to stage the applet symlink farm under it.
        let app_files_root_str = app_files_root.clone();
        let mut cfg = MobileConfig {
            cwd: std::path::PathBuf::from(&app_files_root),
            claude_home: std::path::PathBuf::from(&app_files_root).join(".claude"),
            ..MobileConfig::default()
        };
        if !api_base.is_empty() {
            cfg.api_base = api_base;
        }
        cfg.api_key = api_key;
        if !model.is_empty() {
            cfg.default_model = model;
        }
        // P5b-T7: bundled-shell bootstrap MUST run BEFORE `shell_cfg` is built —
        // `shell_cfg` is moved into `AndroidPlatformInputs.shell` (so `prepare`
        // can target the bundled mksh) below, before the capability probe + the
        // registration gate, so its bundled fields have to be set up front. The
        // arg-less `probe_android_capabilities()` cannot know the bundled paths,
        // so this is the only seam that owns them. Fail-closed (§B2): `None` ⇒ no
        // bundled fields ⇒ `bundled_ready=false` ⇒ gate false ⇒ Shell absent (no
        // system-sh fallback). `bundled` stays alive for the later cache + gate
        // reads — the `shell_cfg` builder only `as_ref().map(..)`-clones out of it.
        let bundled = shell
            .as_ref()
            .and_then(|s| bootstrap_bundled_shell(&s.native_library_dir, &app_files_root_str));
        let shell_cfg = shell.map(|s| platform_android::AndroidShellConfig {
            native_library_dir: std::path::PathBuf::from(s.native_library_dir),
            shell_workspace_root: std::path::PathBuf::from(s.shell_workspace_root),
            app_cache_root: std::path::PathBuf::from(s.app_cache_root),
            package_name: s.package_name,
            package_version_code: s.package_version_code,
            app_writable_roots: s.app_writable_roots.into_iter().map(Into::into).collect(),
            enable_shell: s.enable_shell,
            secrets_in_keystore: s.secrets_in_keystore,
            shell_data_exposure_accepted: s.shell_data_exposure_accepted,
            bundled_mksh_path: bundled.as_ref().map(|b| b.mksh_path.clone()),
            bundled_mksh_hash: bundled.as_ref().map(|b| b.mksh_hash.clone()),
            bundled_applet_dir: bundled.as_ref().map(|b| b.applet_dir.clone()),
        });
        // P3-T5: keep a clone of the shell config before it is moved into
        // `AndroidPlatformInputs.shell` — the registration gate (computed below,
        // after the probe) needs `enable_shell` + the D11 secrets gate from it.
        let shell_cfg_for_gate = shell_cfg.clone();
        let android_platform = AndroidPlatform::new(AndroidPlatformInputs {
            app_files_root: std::path::PathBuf::from(app_files_root),
            camera: Arc::new(AndroidCameraBridge { inner: camera }),
            voice: Arc::new(AndroidVoiceBridge { inner: voice }),
            share: Arc::new(AndroidShareBridge { inner: share }),
            stt: Some(Arc::new(AndroidSttBridge { inner: stt })),
            tts: Some(Arc::new(AndroidTtsBridge { inner: tts })),
            notifications: Some(Arc::new(AndroidNotificationBridge {
                inner: notifications,
            })),
            clipboard: Some(Arc::new(AndroidClipboardBridge { inner: clipboard })),
            shell: shell_cfg,
        });

        // D8: run the eager capability probe and populate the SHARED cache
        // BEFORE erasing to `Arc<dyn Platform>` and assembling the (synchronous)
        // tool registry inside `build_mobile_engine`. The probe result drives
        // both `Sandbox::prepare` and the per-plan registration gates, so it MUST
        // be cached before any of those read it.
        //
        // We hold the CONCRETE `AndroidPlatform` here (the shared
        // `build_mobile_engine` takes `Arc<dyn Platform>` and cannot downcast to
        // reach `shell_capability_cache()`), so this is the only seam that owns
        // both the cache and a pre-registration moment.
        //
        // Runtime for the `block_on`: the handle-owned engine runtime is built
        // INSIDE `build_mobile_engine`, so no `tokio::runtime::Handle` exists yet
        // at this point. The probe is independent of the engine runtime, so we
        // spin up a transient current-thread runtime just for this one call and
        // drop it immediately — clean and correct (option (b) per the plan).
        if let Some(cache) = android_platform.shell_capability_cache() {
            let probe_rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| {
                    MobileEngineError::Internal(format!(
                        "capability probe runtime build failed: {e}"
                    ))
                })?;
            let caps =
                probe_rt.block_on(platform_android::capabilities::probe_android_capabilities());
            cache.set(caps);

            // P5b-T7: fold the bundled-shell bootstrap result into the cached
            // capabilities so capability reporting is TRUTHFUL — the arg-less
            // `probe_android_capabilities()` cannot know the bundled paths, so
            // `bundled_shell_exec` is false until proven here. Only when the
            // bootstrap succeeded (`bundled.is_some()`).
            let bundled_ready = bundled.is_some();
            if bundled_ready {
                let mut c = cache.get();
                c.bundled_shell_exec = true;
                c.bundled_mksh_version = bundled.as_ref().and_then(|b| b.mksh_version.clone());
                c.bundled_applets = platform_android::capabilities::BUNDLED_TOYBOX_APPLETS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect();
                cache.set(c);
            }

            // P3-T5 + P5b-T7: compute the Shell-tool registration gate + prompt
            // info from the just-probed capabilities + the bundled bootstrap +
            // the `AndroidShellConfig`, and thread it onto `MobileConfig` for
            // `tool-shell-mobile::register_all` (the registration gate) + the tool
            // prompt. Absent (`None`) whenever no shell config was supplied —
            // shell support then stays fully absent. The bundled conjunct keeps
            // the gate fail-closed (§B2): no bundled bootstrap ⇒ Shell absent.
            if let Some(shell_cfg) = shell_cfg_for_gate {
                let caps = cache.get();
                cfg.android_shell = Some(tool_api::AndroidShellToolCtx {
                    enabled: android_shell_gate(
                        shell_cfg.enable_shell,
                        shell_cfg.secrets_gate_satisfied(),
                        caps.available(),
                        caps.seccomp_filter,
                        caps.net_deny_verified,
                        bundled_ready,
                    ),
                    applets: if bundled_ready {
                        caps.bundled_applets.clone()
                    } else {
                        caps.toybox_applets.clone()
                    },
                    sh_version: if bundled_ready {
                        caps.bundled_mksh_version.clone()
                    } else {
                        caps.system_sh_version.clone()
                    },
                    bundled: bundled_ready,
                });
            }
        }

        // P4-T10: compute the Git-tool registration gate + thread the in-process
        // HTTPS token onto `MobileConfig`. Independent of the capability probe
        // (Git is decoupled from minijail — spec §G5: no sandbox capability in
        // its gate, no exec). Absent (`None`) whenever no git config was supplied
        // — Git support then stays fully absent. The token rides the separate
        // `android_git_secret` field (NOT the broadly-cloned public
        // `AndroidGitToolCtx`, which only exposes `has_token`), so it never
        // enters the public tool carrier.
        if let Some(c) = git {
            let workspace_ready = std::path::Path::new(&c.workspace_root).is_dir();
            let ca_store_reachable =
                c.ca_cert_dir.is_empty() || std::path::Path::new(&c.ca_cert_dir).exists();
            cfg.android_git = Some(tool_api::AndroidGitToolCtx {
                enabled: android_git_gate(c.enable_git, workspace_ready, ca_store_reachable),
                has_token: c.https_token.is_some(),
                workspace_root: c.workspace_root.clone(),
            });
            // Defense-in-depth: the SSH private-key path is HOST-supplied (not
            // model-supplied), but still validate it stays inside the app
            // sandbox (`app_files_root`) before handing it to libgit2. An empty
            // path = no SSH. If validation fails (escapes the sandbox / missing),
            // drop ALL ssh fields so an SSH op reports "not configured" rather
            // than passing an out-of-sandbox key.
            let ssh_root = std::path::Path::new(&app_files_root_str);
            let ssh_key_ok = !c.ssh_private_key_path.is_empty()
                && tool_git_mobile::auth::validate_ssh_key_path(
                    &c.ssh_private_key_path,
                    ssh_root,
                )
                .is_ok();
            let (ssh_private_key_path, ssh_public_key_path, ssh_passphrase, ssh_known_hosts) =
                if ssh_key_ok {
                    (
                        Some(c.ssh_private_key_path),
                        if c.ssh_public_key_path.is_empty() {
                            None
                        } else {
                            Some(c.ssh_public_key_path)
                        },
                        c.ssh_passphrase,
                        c.ssh_known_hosts_sha256_hex,
                    )
                } else {
                    (None, None, None, Vec::new())
                };
            cfg.android_git_secret = Some(tool_api::AndroidGitSecret {
                token: c.https_token,
                ca_dir: if c.ca_cert_dir.is_empty() {
                    None
                } else {
                    Some(c.ca_cert_dir)
                },
                ssh_private_key_path,
                ssh_public_key_path,
                ssh_passphrase,
                ssh_known_hosts_sha256_hex: ssh_known_hosts,
            });
        }

        let platform: Arc<dyn Platform> = Arc::new(android_platform);
        let permission_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(AndroidPermissionSinkBridge { inner: permissions });
        engine_mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (
            api_base,
            api_key,
            model,
            app_files_root,
            listener,
            stt,
            tts,
            camera,
            share,
            voice,
            notifications,
            clipboard,
            permissions,
            shell,
            git,
        );
        Err(MobileEngineError::PlatformUnavailable)
    }
}

/// P0a gate probe: run the on-device minijail smoke and return it as JSON
/// (`{"ok":bool,"no_new_privs":bool,"child_exit_zero":bool,"reason":...}`).
/// Keys are serde's default `snake_case` (`SmokeResult` has no `rename_all`).
/// Host builds report the structural reason.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[must_use]
pub fn android_sandbox_smoke() -> String {
    #[cfg(target_os = "android")]
    {
        serde_json::to_string(&platform_android_minijail::minijail_smoke())
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"reason\":\"serialize: {e}\"}}"))
    }
    #[cfg(not(target_os = "android"))]
    {
        "{\"ok\":false,\"reason\":\"host build\"}".to_string()
    }
}

/// P2 acceptance probe: exercise the REAL prepare→runner→`run_jailed` path for
/// one `command` rooted at `workspace`, and return the outcome as JSON
/// (`{"stdout","stderr","exit_code","timed_out","enforcement_failed"}`).
///
/// This is NOT a bypass: it constructs a probed [`CapabilityCache`], an
/// [`AndroidMinijailSandbox`] + [`AndroidMinijailProcessRunner`] over that SAME
/// cache, `prepare()`s the command under a deny-net policy, and `run()`s it
/// through the same jailed fork/exec the engine uses. The wall-clock timeout is
/// hardcoded to **2 seconds** so a `sleep 10` probe reliably trips the watchdog
/// (`timed_out=true`) without making the instrumentation test slow.
///
/// `enforcement_failed` is `null` on success; on the host build (no Android
/// device) it is `"host build"` and the rest are empty/zero — so a JVM-host run
/// fails loudly rather than silently passing.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)] // FFI export: UniFFI marshals owned `String`.
#[must_use]
pub fn android_sandbox_run_probe(command: String, workspace: String) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (command, workspace);
        "{\"enforcement_failed\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use platform_android::{
            capabilities::{probe_android_capabilities, CapabilityCache},
            AndroidMinijailProcessRunner, AndroidMinijailSandbox, AndroidShellConfig,
        };
        use std::collections::HashMap;
        use traits::{
            NetworkPolicy, ProcessCommand, ProcessRunner, ResourceLimits, Sandbox, SandboxPolicy,
        };

        // The probe runtime: capability probe + the jailed run are independent
        // of the engine runtime, so spin up a transient current-thread runtime
        // and drop it (mirrors `build_android_engine`'s eager-probe seam).
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                return format!("{{\"enforcement_failed\":\"probe runtime build: {e}\"}}");
            }
        };

        // Probe REAL capabilities and share ONE cache across sandbox + runner —
        // exactly as `AndroidPlatform::new` does.
        let cache = Arc::new(CapabilityCache::new());
        cache.set(rt.block_on(probe_android_capabilities()));

        let ws = std::path::PathBuf::from(&workspace);
        let cfg = AndroidShellConfig {
            native_library_dir: ws.join("native-lib"),
            shell_workspace_root: ws.clone(),
            app_cache_root: ws.join("cache"),
            package_name: "com.lingxi.code".to_string(),
            package_version_code: 1,
            app_writable_roots: vec![ws.clone()],
            enable_shell: true,
            secrets_in_keystore: true,
            shell_data_exposure_accepted: true,
            bundled_mksh_path: None,
            bundled_mksh_hash: None,
            bundled_applet_dir: None,
        };
        let sandbox = AndroidMinijailSandbox::new(cfg, cache.clone());
        let runner = AndroidMinijailProcessRunner::new(cache);

        // Deny-net policy (the P2 acceptance default). 2s wall-clock timeout so
        // `sleep 10` trips the watchdog.
        // Request NO filesystem confinement: Android's fs boundary is the app
        // UID, not Landlock (shipping kernels disable it), so `plan_from_policy`
        // rejects any non-empty writable/denied path set. Net-deny is the only
        // active confinement here.
        let policy = SandboxPolicy {
            network: NetworkPolicy::Disabled,
            writable_paths: vec![],
            denied_paths: vec![],
            allow_subprocess: true,
            limits: ResourceLimits::default(),
        };
        let proc_cmd = ProcessCommand {
            command: "/system/bin/sh".to_string(),
            args: vec!["-c".to_string(), command],
            cwd: Some(ws),
            env: HashMap::new(),
            timeout: Some(std::time::Duration::from_secs(2)),
            stdin: None,
        };

        let prepared = match sandbox.prepare(proc_cmd, &policy) {
            Ok(p) => p,
            Err(e) => {
                return format!(
                    "{{\"enforcement_failed\":\"prepare: {}\"}}",
                    e.to_string().replace('"', "'")
                );
            }
        };
        match rt.block_on(runner.run(&prepared)) {
            Ok(out) => serde_json::json!({
                "stdout": out.stdout,
                "stderr": out.stderr,
                "exit_code": out.exit_code,
                "timed_out": out.timed_out,
                "enforcement_failed": serde_json::Value::Null,
            })
            .to_string(),
            Err(e) => format!(
                "{{\"enforcement_failed\":\"run: {}\"}}",
                e.to_string().replace('"', "'")
            ),
        }
    }
}

/// P5c acceptance probe: exercise the REAL prepare→runner→`run_jailed` path for
/// one `command` running through the BUNDLED mksh+toybox (not the device's
/// system sh), and return the outcome as JSON
/// (`{"stdout","stderr","exit_code","timed_out","enforcement_failed"}`).
///
/// This is the on-device end-to-end proof of the P5 bundled-shell chain. It
/// mirrors [`android_sandbox_run_probe`] but first bootstraps the bundled shell
/// via [`bootstrap_bundled_shell`] (stage the toybox applet symlink farm, verify
/// bundled execve + dispatch, sha256 `libmksh.so`), then sets the THREE bundled
/// `AndroidShellConfig` fields from the result. Because those fields are set,
/// `prepare()` selects `ExecTarget::BundledHelper{mksh}` and leads PATH with the
/// applet farm, and the runner content-identity-checks the recorded sha256
/// against the real `libmksh.so` before execve (spec P5b §T4b). So a non-empty
/// `stdout` from a bundled command implicitly proves staging + the hash check +
/// the jailed bundled exec all passed end-to-end.
///
/// The wall-clock timeout is hardcoded to **2 seconds** so a `sleep 10` probe
/// reliably trips the watchdog (`timed_out=true`). The deny-net `SandboxPolicy`
/// is the same one P2 proved (`net_deny_verified`).
///
/// `enforcement_failed` is `null` on success; on the host build (no Android
/// device) it is `"host build"`, and `"bundled bootstrap failed"` if
/// `bootstrap_bundled_shell` returns `None` — so a JVM-host run or a broken
/// bundle fails loudly rather than silently passing.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)] // FFI export: UniFFI marshals owned `String`.
#[must_use]
pub fn android_bundled_shell_run_probe(
    native_lib_dir: String,
    app_files_root: String,
    command: String,
) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (native_lib_dir, app_files_root, command);
        "{\"enforcement_failed\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use platform_android::{
            capabilities::{probe_android_capabilities, CapabilityCache},
            AndroidMinijailProcessRunner, AndroidMinijailSandbox, AndroidShellConfig,
        };
        use std::collections::HashMap;
        use traits::{
            NetworkPolicy, ProcessCommand, ProcessRunner, ResourceLimits, Sandbox, SandboxPolicy,
        };

        // Bootstrap the bundled shell: stage the applet symlink farm, verify
        // bundled execve + dispatch, sha256 libmksh.so. `None` = fail-closed.
        let Some(bundled) = bootstrap_bundled_shell(&native_lib_dir, &app_files_root) else {
            return "{\"enforcement_failed\":\"bundled bootstrap failed\"}".to_string();
        };

        // The probe runtime: capability probe + the jailed run are independent
        // of the engine runtime, so spin up a transient current-thread runtime
        // and drop it (mirrors `android_sandbox_run_probe`).
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                return format!("{{\"enforcement_failed\":\"probe runtime build: {e}\"}}");
            }
        };

        // Probe REAL capabilities and share ONE cache across sandbox + runner.
        let cache = Arc::new(CapabilityCache::new());
        cache.set(rt.block_on(probe_android_capabilities()));

        let root = std::path::PathBuf::from(&app_files_root);
        let cfg = AndroidShellConfig {
            native_library_dir: std::path::PathBuf::from(&native_lib_dir),
            shell_workspace_root: root.clone(),
            app_cache_root: root.join("cache"),
            package_name: "com.lingxi.code".to_string(),
            package_version_code: 1,
            app_writable_roots: vec![root.clone()],
            enable_shell: true,
            secrets_in_keystore: true,
            shell_data_exposure_accepted: true,
            // The three bundled fields drive `prepare` onto BundledHelper{mksh}
            // + the applet-farm PATH, and the runner's content-identity check.
            bundled_mksh_path: Some(bundled.mksh_path),
            bundled_mksh_hash: Some(bundled.mksh_hash),
            bundled_applet_dir: Some(bundled.applet_dir),
        };
        let sandbox = AndroidMinijailSandbox::new(cfg, cache.clone());
        let runner = AndroidMinijailProcessRunner::new(cache);

        // Deny-net policy (the P2 acceptance default, same filter P2 proved via
        // `net_deny_verified`). 2s wall-clock timeout so `sleep 10` trips the
        // watchdog. No filesystem confinement (Android's fs boundary is the app
        // UID, not Landlock); net-deny is the only active confinement here.
        let policy = SandboxPolicy {
            network: NetworkPolicy::Disabled,
            writable_paths: vec![],
            denied_paths: vec![],
            allow_subprocess: true,
            limits: ResourceLimits::default(),
        };
        let proc_cmd = ProcessCommand {
            // `prepare` normalizes the system-sh sentinel onto BundledHelper{mksh}
            // because the bundled fields are set.
            command: "/system/bin/sh".to_string(),
            args: vec!["-c".to_string(), command],
            cwd: Some(root),
            env: HashMap::new(),
            timeout: Some(std::time::Duration::from_secs(2)),
            stdin: None,
        };

        let prepared = match sandbox.prepare(proc_cmd, &policy) {
            Ok(p) => p,
            Err(e) => {
                return format!(
                    "{{\"enforcement_failed\":\"prepare: {}\"}}",
                    e.to_string().replace('"', "'")
                );
            }
        };
        match rt.block_on(runner.run(&prepared)) {
            Ok(out) => serde_json::json!({
                "stdout": out.stdout,
                "stderr": out.stderr,
                "exit_code": out.exit_code,
                "timed_out": out.timed_out,
                "enforcement_failed": serde_json::Value::Null,
            })
            .to_string(),
            Err(e) => format!(
                "{{\"enforcement_failed\":\"run: {}\"}}",
                e.to_string().replace('"', "'")
            ),
        }
    }
}

/// P2 acceptance probe: run the REAL capability probe and return the matrix as
/// JSON (`{"net_deny_verified":bool,"seccomp_filter":bool,...}`). The strong
/// proof of net-deny enforcement is `net_deny_verified` — the probe forked a
/// child under the net-deny seccomp filter and observed `socket()` ⇒ `EPERM`.
/// Host builds report `{"probed":false,"net_deny_verified":false}`.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[must_use]
pub fn android_sandbox_capabilities() -> String {
    #[cfg(not(target_os = "android"))]
    {
        "{\"probed\":false,\"net_deny_verified\":false,\"reason\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => return format!("{{\"probed\":false,\"reason\":\"probe runtime: {e}\"}}"),
        };
        let caps = rt.block_on(platform_android::capabilities::probe_android_capabilities());
        serde_json::json!({
            "probed": caps.probed,
            "minijail_smoke": caps.minijail_smoke,
            "no_new_privs": caps.no_new_privs,
            "seccomp_filter": caps.seccomp_filter,
            "seccomp_tsync": caps.seccomp_tsync,
            "net_deny_verified": caps.net_deny_verified,
            "pgid_kill": caps.pgid_kill,
            "landlock_abi": caps.landlock_abi,
            "system_sh_version": caps.system_sh_version,
            "toybox_applets": caps.toybox_applets,
            "reason": caps.reason,
        })
        .to_string()
    }
}

/// P4d acceptance probe: drive the REAL [`tool_git_mobile::GitTool`] path for one
/// structured git operation, and return the `ToolCallResult` data (or the error)
/// as JSON. This is the on-device proof that libgit2's OpenSSL TLS + the Android
/// system cacerts (`/system/etc/security/cacerts`) verify a real HTTPS clone —
/// the one thing the host `file://` tests (P4b/c) cannot exercise.
///
/// It is NOT a bypass: it builds a [`tool_api::BuiltinToolContext`] with
/// `android_git = Some(AndroidGitToolCtx { enabled: true, has_token: false,
/// workspace_root })` + `android_git_secret = Some(AndroidGitSecret { token:
/// None, ca_dir: Some(ca_cert_dir) })`, constructs the `GitTool`, parses
/// `operation_json` into the tool input `Value`, and runs `GitTool::call(..)` on
/// a transient current-thread runtime — exactly the engine's path. No token is
/// needed: the acceptance clone targets a PUBLIC repo.
///
/// - `operation_json` — the structured tool input (e.g.
///   `{"operation":"clone","repo_url":"https://github.com/.../x.git","repo":"cloned"}`).
/// - `workspace` — the app-private workspace root all ops are anchored under.
/// - `ca_cert_dir` — the system CA-certificate directory for TLS verification
///   (the device passes `/system/etc/security/cacerts`).
///
/// Returns the `ToolCallResult.data` JSON on success, or `{"error": "..."}` on a
/// parse / tool error. On the host build (no Android device) it returns
/// `{"error":"host build"}` so a JVM-host run fails loudly rather than silently
/// passing.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)] // FFI export: UniFFI marshals owned `String`.
#[must_use]
pub fn android_git_probe(operation_json: String, workspace: String, ca_cert_dir: String) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (operation_json, workspace, ca_cert_dir);
        "{\"error\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
        use tool_api::tool_trait::Tool;
        use tool_api::{AndroidGitSecret, AndroidGitToolCtx};
        use traits::process::ProcessOutput;

        // Parse the structured operation input. A malformed payload is a probe
        // error, not a tool failure.
        let input: serde_json::Value = match serde_json::from_str(&operation_json) {
            Ok(v) => v,
            Err(e) => {
                return format!(
                    "{{\"error\":\"parse operation_json: {}\"}}",
                    e.to_string().replace('"', "'")
                );
            }
        };

        // Build the REAL BuiltinToolContext. The git ops drive libgit2 + the
        // filesystem directly, so the test-support stub fs/process/sandbox
        // handles are inert here; only `android_git` + `android_git_secret`
        // matter. No token (public repo); the CA dir points at the system store.
        let mut ctx = shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        ctx.android_git = Some(AndroidGitToolCtx {
            enabled: true,
            has_token: false,
            workspace_root: workspace,
        });
        ctx.android_git_secret = Some(AndroidGitSecret {
            token: None,
            ca_dir: if ca_cert_dir.is_empty() {
                None
            } else {
                Some(ca_cert_dir)
            },
            ssh_private_key_path: None,
            ssh_public_key_path: None,
            ssh_passphrase: None,
            ssh_known_hosts_sha256_hex: Vec::new(),
        });

        let tool = tool_git_mobile::GitTool::new(ctx);

        // The clone/local ops are sync inside an async `call`; run on a transient
        // current-thread runtime and drop it (mirrors `android_sandbox_run_probe`).
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => return format!("{{\"error\":\"probe runtime build: {e}\"}}"),
        };
        match rt.block_on(tool.call(input, fresh_ctx(), fresh_tx())) {
            Ok(result) => serde_json::to_string(&result.data)
                .unwrap_or_else(|e| format!("{{\"error\":\"serialize: {e}\"}}")),
            Err(e) => format!(
                "{{\"error\":\"{}\"}}",
                e.to_string().replace('"', "'").replace('\n', " ")
            ),
        }
    }
}

/// P5a make-or-break gate probe: prove that bundled executables packaged as
/// `lib*.so` under `native_lib_dir` can `execve` under Android 10+ W^X, and
/// decide which toybox applet-resolution mechanism works on-device.
///
/// This is a RAW exec probe — NOT jailed. It only proves the W^X/packaging
/// story (the minijail/deny-net path is unchanged from P2/P3 and proven
/// elsewhere). It returns JSON:
///
/// ```json
/// {"mksh_exec_ok":bool,"applet_symlink_ok":bool,"applet_rewrite_ok":bool,"reason":"..."}
/// ```
///
/// - `mksh_exec_ok`: `<native_lib_dir>/libmksh.so -c 'echo hi'` runs and stdout
///   contains `hi` — proves W^X execve of a bundled executable from
///   nativeLibraryDir works at all.
/// - `applet_symlink_ok`: a symlink `<applet_dir>/grep` → `libtoybox.so`,
///   exec'd as `<applet_dir>/grep foo` over stdin `foo\nbar`, outputs `foo` —
///   proves execve-through-a-symlink-into-nativeLibraryDir + toybox `argv[0]`
///   multicall dispatch under W^X (the PREFERRED applet mechanism for P5b).
/// - `applet_rewrite_ok`: `<native_lib_dir>/libtoybox.so grep foo` over the same
///   stdin outputs `foo` — the command-rewrite FALLBACK mechanism.
///
/// Host builds return `{"error":"host build"}` so a JVM-host run fails loudly
/// rather than silently passing.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)] // FFI export: UniFFI marshals owned `String`.
// Single linear probe body (raw mksh exec + symlink-farm + command-rewrite
// applet resolution); the length is intrinsic to the three-mechanism probe, not
// decomposable. Pre-existing P5a debt surfaced by the android-target clippy gate.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn android_bundled_shell_probe(native_lib_dir: String, applet_dir: String) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (native_lib_dir, applet_dir);
        "{\"error\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use std::io::Write;
        use std::os::unix::fs::symlink;
        use std::path::Path;
        use std::process::{Command, Stdio};

        let nl = Path::new(&native_lib_dir);
        let mksh = nl.join("libmksh.so");
        let toybox = nl.join("libtoybox.so");
        let mut reason = String::new();

        // (a) RAW mksh execve proof — the make-or-break W^X gate.
        let mksh_exec_ok = match Command::new(&mksh).args(["-c", "echo hi"]).output() {
            Ok(out) => {
                let so = String::from_utf8_lossy(&out.stdout);
                let ok = so.contains("hi");
                if !ok {
                    reason.push_str(&format!(
                        "mksh: status={:?} stdout={:?} stderr={:?}; ",
                        out.status.code(),
                        so,
                        String::from_utf8_lossy(&out.stderr)
                    ));
                }
                ok
            }
            Err(e) => {
                reason.push_str(&format!("mksh spawn: {e}; "));
                false
            }
        };

        // Helper: run a command with stdin "foo\nbar" and assert stdout == "foo".
        let run_grep = |mut cmd: Command, label: &str, reason: &mut String| -> bool {
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    reason.push_str(&format!("{label} spawn: {e}; "));
                    return false;
                }
            };
            if let Some(mut sin) = child.stdin.take() {
                let _ = sin.write_all(b"foo\nbar\n");
            }
            match child.wait_with_output() {
                Ok(out) => {
                    let so = String::from_utf8_lossy(&out.stdout);
                    let ok = so.lines().any(|l| l.trim() == "foo");
                    if !ok {
                        reason.push_str(&format!(
                            "{label}: status={:?} stdout={:?} stderr={:?}; ",
                            out.status.code(),
                            so,
                            String::from_utf8_lossy(&out.stderr)
                        ));
                    }
                    ok
                }
                Err(e) => {
                    reason.push_str(&format!("{label} wait: {e}; "));
                    false
                }
            }
        };

        // (b) Symlink-farm applet resolution (PREFERRED).
        let applet_symlink_ok = {
            let dir = Path::new(&applet_dir);
            let link = dir.join("grep");
            let setup_ok = std::fs::create_dir_all(dir)
                .map_err(|e| reason.push_str(&format!("applet_dir mkdir: {e}; ")))
                .is_ok();
            // Refresh the symlink (ignore a pre-existing one from a warm run).
            let _ = std::fs::remove_file(&link);
            if setup_ok {
                match symlink(&toybox, &link) {
                    Ok(()) => {
                        let mut c = Command::new(&link);
                        c.arg("foo");
                        run_grep(c, "applet_symlink", &mut reason)
                    }
                    Err(e) => {
                        reason.push_str(&format!("symlink: {e}; "));
                        false
                    }
                }
            } else {
                false
            }
        };

        // (c) Command-rewrite applet resolution (FALLBACK).
        let applet_rewrite_ok = {
            let mut c = Command::new(&toybox);
            c.args(["grep", "foo"]);
            run_grep(c, "applet_rewrite", &mut reason)
        };

        serde_json::json!({
            "mksh_exec_ok": mksh_exec_ok,
            "applet_symlink_ok": applet_symlink_ok,
            "applet_rewrite_ok": applet_rewrite_ok,
            "reason": if reason.is_empty() { "ok".to_string() } else { reason },
        })
        .to_string()
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
    /// real handle on CI without an Android device — exactly the spec §8 "prove
    /// from a Kotlin unit test", run on the host.
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
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn build_handle(root: &std::path::Path) -> Arc<engine_mobile::MobileEngineHandle> {
        let platform: Arc<dyn Platform> = Arc::new(HostFakePlatform::new(root.to_path_buf()));
        let listener: Arc<dyn ClientEventListener> = Arc::new(FakeListener::default());
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let cfg = MobileConfig {
            cwd: root.to_path_buf(),
            claude_home: root.join(".claude"),
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
        // `skill_builtin::BUILTIN_MOBILE`). The signal is that the call resolves
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

    /// P3-T5 + P5-T7: the Shell-tool registration gate is the conjunction of all
    /// six inputs — it is `true` ONLY when every input is `true`, and `false` if
    /// any single input is `false`. The sixth conjunct (`bundled_shell_ready`,
    /// P5-T7) makes the gate fail-closed when the bundled mksh+toybox bootstrap
    /// did not succeed: no system-sh fallback. Host-testable without a device.
    #[test]
    fn android_shell_gate_is_all_six_conjuncts() {
        use super::android_shell_gate;

        // All six true → enabled.
        assert!(
            android_shell_gate(true, true, true, true, true, true),
            "gate must be enabled when all six conjuncts hold"
        );

        // Each single-false case → disabled. (input index, label) drives the row.
        let cases = [
            (0, "enable_shell"),
            (1, "secrets_gate_satisfied"),
            (2, "caps_available"),
            (3, "seccomp_filter"),
            (4, "net_deny_verified"),
            (5, "bundled_shell_ready"),
        ];
        for (false_idx, label) in cases {
            let mut args = [true; 6];
            args[false_idx] = false;
            assert!(
                !android_shell_gate(args[0], args[1], args[2], args[3], args[4], args[5]),
                "gate must be disabled when {label} is false"
            );
        }
    }

    /// P4-T10: the Git-tool registration gate is the conjunction of all three
    /// inputs — `true` ONLY when every input is `true`, `false` if any single
    /// input is `false`. The token is NOT a gate input (spec §G5). Host-testable.
    #[test]
    fn android_git_gate_is_all_three_conjuncts() {
        use super::android_git_gate;

        // All three true → enabled.
        assert!(
            android_git_gate(true, true, true),
            "gate must be enabled when all three conjuncts hold"
        );

        // Each single-false case → disabled.
        let cases = [
            (0, "enable_git"),
            (1, "workspace_ready"),
            (2, "ca_store_reachable"),
        ];
        for (false_idx, label) in cases {
            let mut args = [true; 3];
            args[false_idx] = false;
            assert!(
                !android_git_gate(args[0], args[1], args[2]),
                "gate must be disabled when {label} is false"
            );
        }
    }

    /// P4-T10: the FFI → `AndroidGitToolCtx` mapping yields `enabled = false`
    /// when `enable_git` is false even if the other gate inputs (workspace +
    /// CA store) are satisfied, and regardless of a present token. Pure-fn level
    /// (mirrors the body of [`build_android_engine`]'s git mapping).
    #[test]
    fn ffi_mapping_disabled_when_enable_git_false() {
        use super::android_git_gate;

        let cfg = super::AndroidGitConfigFfi {
            enable_git: false,
            // Use the workspace's own dir so the readiness check would pass.
            workspace_root: env!("CARGO_MANIFEST_DIR").to_string(),
            // Empty CA dir is treated as reachable (libgit2/OpenSSL defaults).
            ca_cert_dir: String::new(),
            https_token: Some("pat-token".to_string()),
            ssh_private_key_path: String::new(),
            ssh_public_key_path: String::new(),
            ssh_passphrase: None,
            ssh_known_hosts_sha256_hex: Vec::new(),
        };

        let workspace_ready = std::path::Path::new(&cfg.workspace_root).is_dir();
        let ca_store_reachable =
            cfg.ca_cert_dir.is_empty() || std::path::Path::new(&cfg.ca_cert_dir).exists();
        assert!(workspace_ready, "fixture workspace_root must be a real dir");
        assert!(ca_store_reachable, "empty CA dir must count as reachable");

        let ctx = tool_api::AndroidGitToolCtx {
            enabled: android_git_gate(cfg.enable_git, workspace_ready, ca_store_reachable),
            has_token: cfg.https_token.is_some(),
            workspace_root: cfg.workspace_root.clone(),
        };

        assert!(
            !ctx.enabled,
            "Git must be disabled when enable_git is false, even with workspace + CA + token set"
        );
        assert!(
            ctx.has_token,
            "has_token must still reflect a supplied token"
        );
    }
}
