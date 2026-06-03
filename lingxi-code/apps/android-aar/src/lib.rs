//! `android-aar` (M8-P12 → M10-F3) — the Android UniFFI packager.
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
//! ## UniFFI status
//! The `uniffi` feature (default-on) lights up the real UniFFI surface: the
//! re-exported [`MobileEngineHandle`] is a `#[derive(uniffi::Object)]`, the
//! listener a callback interface, the DTOs UniFFI types. `engine-mobile` carries
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
/// `AndroidPlatform`. UniFFI marshals each `Arc<dyn …>` as a callback-interface
/// reference; `app_files_root` is the app's private files-dir.
pub struct PlatformImpls {
    /// Kotlin `CameraControl` impl (CameraX).
    pub camera: Arc<dyn CameraControl>,
    /// Kotlin `VoiceRecorder` impl (MediaRecorder).
    pub voice: Arc<dyn VoiceRecorder>,
    /// Kotlin `SharingService` impl (Intent.ACTION_SEND).
    pub share: Arc<dyn SharingService>,
    /// The app's private files-dir root.
    pub app_files_root: String,
}

/// Top-level UniFFI constructor: build the mobile engine from the Kotlin-supplied
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
/// UniFFI can render it for an async `callback_interface` method; the bridge
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
/// UniFFI can render it for an async `callback_interface` method; the bridge
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
/// UniFFI can render it for an async `callback_interface` method; the bridge
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
/// app implements it over CameraX (capture) and the system photo picker
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
/// enum so UniFFI can render it for an async `callback_interface` method; the
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
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct NoopPermissionSink;

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl engine_mobile::PermissionRequestSink for NoopPermissionSink {
    async fn emit_request(&self, _request: client_protocol::permission::PermissionRequest) {}
}

/// The Kotlin-implemented event listener the Android app registers when it builds
/// the engine. Defined in THIS crate (not re-used from `client-adapter`) so its
/// UniFFI converter registers under `android_aar`'s tag — a prerequisite for
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
///   [`traits::CameraControl`]) so `tool-camera` routes through CameraX +
///   the system photo picker.
/// - `share` — the foreign share callback (bridged to
///   [`traits::SharingService`]) so `tool-share` routes through the system
///   `Intent.ACTION_SEND` share sheet.
///
/// On non-Android hosts this returns [`MobileEngineError::PlatformUnavailable`]
/// (the `AndroidPlatform` is only linked under `cfg(target_os = "android")`).
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
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
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    let listener: Arc<dyn ClientEventListener> =
        Arc::new(AndroidListenerBridge { inner: listener });
    #[cfg(target_os = "android")]
    {
        use platform_android::{AndroidPlatform, AndroidPlatformInputs};
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
        let platform: Arc<dyn Platform> = Arc::new(AndroidPlatform::new(AndroidPlatformInputs {
            app_files_root: std::path::PathBuf::from(app_files_root),
            camera: Arc::new(AndroidCameraBridge { inner: camera }),
            voice: Arc::new(AndroidVoiceBridge { inner: voice }),
            share: Arc::new(AndroidShareBridge { inner: share }),
            stt: Some(Arc::new(AndroidSttBridge { inner: stt })),
            tts: Some(Arc::new(AndroidTtsBridge { inner: tts })),
        }));
        let permission_sink: Arc<dyn PermissionRequestSink> = Arc::new(NoopPermissionSink);
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
            self.count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
}
