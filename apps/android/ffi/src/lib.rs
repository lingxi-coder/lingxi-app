//! `android-aar` (M8-P12 → M10-F3) — the Android `UniFFI` packager.
//!
//! The FFI boundary between the Rust engine and the Android app. The Kotlin
//! layer implements the unified [`platform_api::AudioService`] plus the
//! [`platform_api::CameraControl`] / [`platform_api::SharingService`] callbacks
//! (skeletons under `kotlin/`), hands them across as a [`PlatformImpls`] record, and Rust uses them to build
//! an `AndroidPlatform` and assemble the mobile engine — Rust calls *back* into
//! Kotlin for native capabilities.
//!
//! ## Shared session host (F3-04)
//!
//! The real session host — [`MobileEngineHandle`] (owns the handle-owned tokio
//! runtime + the wired `MobileRuntime` + the registered `ClientEventListener`)
//! and its [`MobileEngineError`] — lives in `harness-runtime::mobile` and is RE-EXPORTED
//! here, NOT re-derived. That single-source rule (plan F3-04) is what stops iOS
//! and Android from drifting: this crate only adds the Android-specific
//! `Platform`-construction wrapper around the shared `build_mobile_engine`.
//!
//! ## Inbound command path (F3-05)
//!
//! The async FFI entry point — `MobileEngineHandle::submit(ClientCommand) ->
//! Result<(), ClientError>` (under `uniffi`: `#[uniffi::export(async_runtime =
//! "tokio")]`) — is defined ONCE on the shared host in `harness-runtime::mobile` and
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
//! `submit` impl (in `harness-runtime::mobile`), backed by the workspace `uniffi` dep's
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
//! listener a callback interface, the DTOs `UniFFI` types. `harness-runtime::mobile` carries
//! the `setup_scaffolding!()`; this crate re-exports it (and adds its own for
//! the Android-local exports) so the symbols land in the final library.

#![forbid(unsafe_code)]
// Documentation debt, not a decision that docs do not matter: this crate had
// 201 undocumented public item(s) when `missing_docs` was measured across the
// workspace (2026-09-16). The lint stays `warn` at the workspace level so a NEW
// crate still inherits the requirement; this allow is scoped here so the debt
// is visible per crate and can be repaid one crate at a time by deleting this
// line.
#![allow(missing_docs)]
// Dead code kept visible, not swept: this crate had 6 item(s) rustc could
// reach from nothing when the workspace was measured (2026-09-16). The lint
// stays `warn` at the workspace level so a NEW crate still inherits it; this
// allow is scoped here so the count is per crate and repayable by deleting this
// line. This is the category where "named, computed, never wired" hides — some
// of these read like features that were built and never connected. Each wants a
// decision (delete, or wire), not a blanket deletion.
// ⚠️ The count above is ONE macOS, lib-target measurement. It is not a list of
// deletable items — see docs/HANDOFF-dead-code-adjudication-2026-09-17.md,
// which records two near-misses where it said "dead" about live code.
#![allow(dead_code)]

// `Platform` is named only inside the `cfg(target_os = "android")` constructor
// body; importing it unconditionally warns on the host build, so scope it.
#[cfg(feature = "uniffi")]
pub use harness_runtime::mobile::{
    max_audio_payload_bytes, ClientEventListener, CronDueOccurrenceDto, CronFireStatusDto,
    CronTaskDto, FiredCronJobDto, LocalAppBackgroundRunDto, MobileConfig, MobileCronStoreHandle,
    MobileEngineError, MobileEngineHandle, MobileSessionMode, ModelBillingModeDto,
    ModelCapabilitiesDto, ModelDetailsDto, ModelPricingDto, ModelPricingTierDto,
    PermissionRequestSink, ProviderConnectionTestDto, SessionModeDto,
};
mod callbacks;
mod configuration;
mod host;
mod linux_conversion;
mod linux_runtime;
mod linux_types;
mod probes;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidAudioFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidAudioService;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidCamera;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidClipboard;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidComputerUseFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidComputerUseHost;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidDeviceControl;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidEventListener;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidGitCredentialProvider;
#[cfg(test)]
use callbacks::AndroidGitCredentialProviderBridge;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidLocation;
#[cfg(test)]
use callbacks::AndroidLocationBridge;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidNotification;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidPermissionSink;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidScreenshotFfi;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidSecureStorage;
#[cfg(feature = "uniffi")]
pub use callbacks::AndroidShare;
#[cfg(feature = "uniffi")]
pub use callbacks::CameraFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::CapturedImageFfi;
#[cfg(feature = "uniffi")]
pub use callbacks::ClipboardFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::DeviceControlFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::LocationFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::LocationFixFfi;
#[cfg(feature = "uniffi")]
pub use callbacks::NotificationFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::SecureStorageFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::ShareFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::ShareResultFfi;
#[cfg(test)]
use configuration::android_project_cwd;
pub use configuration::AndroidDeviceClassFfi;
pub use configuration::AndroidEngineLaunchConfigFfi;
pub use configuration::AndroidExecutionTargetFfi;
#[cfg(feature = "uniffi")]
pub use configuration::AndroidGitConfigFfi;
pub use configuration::AndroidHostEnvironmentFfi;
pub use configuration::AndroidLaunchModeFfi;
#[cfg(feature = "uniffi")]
pub use configuration::AndroidMobileLinuxConfigFfi;
pub use configuration::AndroidProviderConfigFfi;
#[cfg(feature = "uniffi")]
pub use configuration::AndroidShellConfigFfi;
#[cfg(test)]
use host::android_git_gate;
#[cfg(test)]
use host::android_guest_shell_enabled;
#[cfg(feature = "uniffi")]
pub use host::build_android_cron_store;
#[cfg(feature = "uniffi")]
pub use host::build_android_engine;
#[cfg(feature = "uniffi")]
#[cfg(feature = "uniffi")]
pub use host::build_mobile_engine;
pub use host::PlatformImpls;
#[cfg(feature = "uniffi")]
pub use linux_runtime::build_android_mobile_linux_runtime_handle;
#[cfg(feature = "uniffi")]
pub use linux_runtime::AndroidMobileLinuxRuntimeHandle;
#[cfg(feature = "uniffi")]
pub use linux_types::AndroidMobileLinuxEventSink;
#[cfg(feature = "uniffi")]
pub use linux_types::AndroidMobileLinuxStreamSink;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxApiErrorFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxCapabilityFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxCommandRequestFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxCommandResultFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxEnvEntryFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxEventFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxEventKindFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxMountPurposeFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxMountSpecFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxProcessHandleFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxPtyOpenRequestFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxPtySessionHandleFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxPtySizeFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxRootfsStateFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxRuntimeModeFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxStatusFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxTaskKindFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxTaskSnapshotFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxTaskStateFfi;
#[cfg(feature = "uniffi")]
pub use probes::android_git_probe;
#[cfg(feature = "uniffi")]
pub use probes::android_git_probe_authed;
// F3-04: the shared session host + its error type are DEFINED ONCE in
// `harness-runtime::mobile` and re-exported here. Both FFI packager crates re-export the
// SAME types so iOS and Android cannot drift (plan F3-04).

// ---------------------------------------------------------------------------
// Share — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidStt/AndroidTts/AndroidCamera pattern: the Kotlin layer
// implements a crate-local async `AndroidShare` callback interface (the system
// `Intent.ACTION_SEND` share sheet) and hands it across the FFI seam. The
// engine consumes the SHARED `platform_api::SharingService` seam, so
// `AndroidShareBridge` adapts the crate-local interface to its `traits`
// counterpart. The shared `platform_api::SharePayload` is destructured into the three
// flat `text` / `url` / `image_bytes` args to keep the FFI flat; the bridge
// maps the FFI result/error back onto `platform_api::ShareResult` / `platform_api::ShareError`.

// ---------------------------------------------------------------------------
// Location — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Notifications — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidShare pattern: the Kotlin layer implements a crate-local
// async `AndroidNotification` callback interface (the system
// `NotificationManager`) and hands it across the FFI seam. The engine consumes
// the SHARED `platform_api::NotificationService` seam, so `AndroidNotificationBridge`
// adapts the crate-local interface to its `traits` counterpart. The shared
// `platform_api::NotificationRequest` is destructured into the flat `title` / `body`
// / `tag` args to keep the FFI flat; the bridge maps the FFI error back onto
// `platform_api::NotificationError`. This is ENGINE-DRIVEN by `tool-notification`
// (the model posts a notification) — no user-facing UI affordance.

// ---------------------------------------------------------------------------
// Clipboard — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidNotification pattern: the Kotlin layer implements a
// crate-local async `AndroidClipboard` callback interface (the system
// `ClipboardManager`) and hands it across the FFI seam. The engine consumes
// the SHARED `platform_api::Clipboard` seam, so `AndroidClipboardBridge` adapts the
// crate-local interface to its `traits` counterpart; the bridge maps the FFI
// error back onto `platform_api::ClipboardError`. This is ENGINE-DRIVEN by
// `tool-clipboard` (the model reads/writes the pasteboard) — no user-facing UI
// affordance. NOTE Android 10+ restricts clipboard READS to the focused app /
// default IME — when a read is not permitted the Kotlin side returns `None`
// gracefully rather than crashing.

// ---------------------------------------------------------------------------
// Device status / haptics / deep links — one native callback object fans out
// to the three shared Platform seams used by Local Apps.

// ---------------------------------------------------------------------------
// Secure storage — foreign (Kotlin) Keystore callback interface + engine bridge.
// ---------------------------------------------------------------------------
//
// The Kotlin app implements a crate-local async `AndroidSecureStorage` callback
// interface backed by the Android Keystore / EncryptedSharedPreferences. The
// engine's `protocol::SecureStorageData` is serde-encoded by the bridge into an
// OPAQUE `blob: Vec<u8>` keyed by (service, account); the native side stores /
// returns the blob verbatim (encrypted at rest by the Keystore). The bridge
// adapts it to the shared `platform_api::SecureStorage` seam and reports
// is_encrypted()=true / backend=AndroidKeystore so OAuth /login can persist.
//
// The bridge struct/impl + its error-fan-out are gated to `target_os =
// "android"` (not just `uniffi`) because they serde-encode through `serde_json`,
// which is a direct dependency only under the android target table — mirroring
// every other `serde_json::` use in this file. The callback interface + its FFI
// error enum stay plain `uniffi` so their converters register on the host
// bindgen build and `AndroidSecureStorage` can be named in `build_android_engine`.

// ---------------------------------------------------------------------------
// Camera — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// The Kotlin layer implements
// a crate-local async `AndroidCamera` callback interface (CameraX capture +
// system photo picker) and hands it across the FFI seam. The engine consumes
// the SHARED `platform_api::CameraControl` seam, so `AndroidCameraBridge` adapts the
// crate-local interface to its `traits` counterpart. Camera position crosses
// the seam as a plain `front: bool` (true = front/selfie, false = rear) to keep
// the FFI flat; the bridge maps it to `platform_api::CameraPosition`.

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// T2.2 — the foreign-callable Android engine constructor.
// ---------------------------------------------------------------------------
//
// Mirrors `ios-framework::build_ios_engine`: a thin `#[uniffi::export]` wrapper
// taking the event listener, one app-scoped audio callback, other device callbacks,
// and plain config strings, threading the runtime
// config into a `MobileConfig`, and delegating to the shared
// `harness_runtime::mobile::build_mobile_engine`. ADDITIVE — it does not touch the
// existing non-exported `build_mobile_engine` above, the `platform-api` crate, or iOS.

/// Device-capability stubs for the camera / share callbacks the Android
/// constructor does not (yet) wire. A text/speech conversation never invokes
/// these; each returns the trait's "unavailable" error. Mirrors
/// `ios-framework`'s `stub_capabilities`.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod stub_capabilities {
    use async_trait::async_trait;
    use platform_api::{
        CameraControl, CameraError, CapturePhotoOpts, CapturedImage, ShareError, SharePayload,
        ShareResult, SharingService,
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

    /// No-op share service: reports sharing unsupported.
    pub struct StubShare;

    #[async_trait]
    impl SharingService for StubShare {
        async fn share(&self, _payload: SharePayload) -> Result<ShareResult, ShareError> {
            Err(ShareError::Unsupported)
        }
    }
}

// ---------------------------------------------------------------------------
// Direct Android Computer Use callback + traits bridge.
// ---------------------------------------------------------------------------

// F3-04: re-export `harness-runtime::mobile`'s UniFFI scaffolding so the shared host's FFI
// symbols (the re-exported `MobileEngineHandle` / `MobileEngineError`) land in
// this crate's final library. Under the `uniffi` feature only.
#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

#[cfg(all(test, feature = "uniffi"))]
#[path = "lib/tests/tests.rs"]
mod tests;
