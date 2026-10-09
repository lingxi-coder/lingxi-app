//! `ios-framework` (M8-P12 → M10-F3) — the iOS `UniFFI` packager.
//!
//! This crate is the FFI boundary between the Rust engine and the iOS app. The
//! Swift layer implements the unified [`lingxi_core::host::AudioService`] plus the
//! [`lingxi_core::host::CameraControl`] / [`lingxi_core::host::SharingService`] callbacks
//! (see the skeletons under `swift/`), hands them across as a [`PlatformImpls`] record, and Rust uses
//! them to construct an `IosPlatform` and assemble the mobile engine — so Rust
//! drives the device's native capabilities by calling *back* into Swift. That
//! bidirectional flow is the whole point of the `UniFFI` seam.
//!
//! ## Shared session host (F3-04)
//!
//! The real session host — [`MobileEngineHandle`] (owns the handle-owned tokio
//! runtime + the wired `MobileRuntime` + the registered `ClientEventListener`)
//! and its [`MobileEngineError`] — lives in `harness-runtime::mobile` and is RE-EXPORTED
//! here, NOT re-derived. That single-source rule (plan F3-04) is what stops iOS
//! and Android from drifting: this crate only adds the iOS-specific
//! `Platform`-construction wrapper around the shared `build_mobile_engine`.
//!
//! ## Inbound command path (F3-05)
//!
//! The async FFI entry point — `MobileEngineHandle::submit(ClientCommand) ->
//! Result<(), ClientError>` (under `uniffi`: `#[uniffi::export(async_runtime =
//! "tokio")]`) — is defined ONCE on the shared host in `harness-runtime::mobile` and
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
//! `submit` impl (in `harness-runtime::mobile`), backed by the workspace `uniffi` dep's
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
//! listener a callback interface, the DTOs `UniFFI` types. `harness-runtime::mobile` carries
//! the `setup_scaffolding!()`; this crate re-exports it (and adds its own for
//! the iOS-local exports) so the symbols land in the final `staticlib`/cdylib.

#![forbid(unsafe_code)]
// Documentation debt, not a decision that docs do not matter: this crate had
// 140 undocumented public item(s) when `missing_docs` was measured across the
// workspace (2026-09-16). The lint stays `warn` at the workspace level so a NEW
// crate still inherits the requirement; this allow is scoped here so the debt
// is visible per crate and can be repaid one crate at a time by deleting this
// line.
#![allow(missing_docs)]
// Dead code kept visible, not swept: this crate had 4 item(s) rustc could
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

#[cfg(feature = "uniffi")]
pub use harness_runtime::mobile::{
    max_audio_payload_bytes, ClientEventListener, CronDueOccurrenceDto, CronFireStatusDto,
    CronTaskDto, FiredCronJobDto, MobileConfig, MobileCronStoreHandle,
    MobileEngineError, MobileEngineHandle, MobileOAuthSessionDto, MobileOAuthStateDto,
    MobileSessionMode, ModelBillingModeDto, ModelCapabilitiesDto, ModelDetailsDto, ModelPricingDto,
    ModelPricingTierDto, PermissionRequestSink, ProviderCatalogEntryDto, ProviderConnectionTestDto,
    SessionModeDto,
};
mod callbacks;
mod configuration;
mod host;
mod linux_conversion;
mod linux_runtime;
mod linux_types;
#[cfg(feature = "uniffi")]
pub use callbacks::CameraFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::CapturedImageFfi;
#[cfg(feature = "uniffi")]
pub use callbacks::ClipboardFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::DeviceControlFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::IosAudioFfiError;
#[cfg(feature = "uniffi")]
pub use callbacks::IosAudioService;
#[cfg(feature = "uniffi")]
pub use callbacks::IosCamera;
#[cfg(feature = "uniffi")]
pub use callbacks::IosClipboard;
#[cfg(feature = "uniffi")]
pub use callbacks::IosDeviceControl;
#[cfg(feature = "uniffi")]
pub use callbacks::IosEventListener;
#[cfg(feature = "uniffi")]
pub use callbacks::IosLocation;
#[cfg(feature = "uniffi")]
pub use callbacks::IosNotification;
#[cfg(feature = "uniffi")]
pub use callbacks::IosPermissionSink;
#[cfg(feature = "uniffi")]
pub use callbacks::IosSecureStorage;
#[cfg(feature = "uniffi")]
pub use callbacks::IosShare;
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
use configuration::ios_mobile_config_from_launch_config;
#[cfg(test)]
use configuration::ios_project_cwd;
#[cfg(test)]
use configuration::resolve_mobile_linux_app_sandbox_root;
#[cfg(test)]
use configuration::validate_mobile_linux_workspace_config;
#[cfg(feature = "uniffi")]
pub use configuration::IosDeviceClassFfi;
#[cfg(feature = "uniffi")]
pub use configuration::IosEngineLaunchConfigFfi;
#[cfg(feature = "uniffi")]
pub use configuration::IosExecutionTargetFfi;
#[cfg(feature = "uniffi")]
pub use configuration::IosHostEnvironmentFfi;
#[cfg(feature = "uniffi")]
pub use configuration::IosLaunchModeFfi;
#[cfg(feature = "uniffi")]
pub use configuration::IosMobileLinuxConfigFfi;
#[cfg(feature = "uniffi")]
pub use configuration::IosProviderConfigFfi;
#[cfg(feature = "uniffi")]
pub use host::build_ios_cron_store;
#[cfg(feature = "uniffi")]
pub use host::build_ios_engine;
#[cfg(feature = "uniffi")]
pub use host::build_ios_engine_with_config;
#[cfg(feature = "uniffi")]
pub use host::build_mobile_engine;
pub use host::PlatformImpls;
#[cfg(test)]
use linux_conversion::event_to_ffi;
#[cfg(feature = "uniffi")]
pub use linux_runtime::create_ios_mobile_linux_runtime;
#[cfg(feature = "uniffi")]
pub use linux_runtime::ios_mobile_linux_status;
#[cfg(test)]
use linux_runtime::ios_mobile_linux_status_from_config;
#[cfg(feature = "uniffi")]
pub use linux_runtime::probe_ios_mobile_linux;
#[cfg(feature = "uniffi")]
pub use linux_runtime::repair_ios_mobile_linux;
#[cfg(feature = "uniffi")]
pub use linux_runtime::reset_ios_mobile_linux;
#[cfg(feature = "uniffi")]
pub use linux_runtime::verify_ios_mobile_linux;
pub use linux_runtime::IosMobileLinuxRuntimeHandle;
#[cfg(feature = "uniffi")]
pub use linux_types::IosMobileLinuxEventSink;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxCapabilityFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxCommandRequestFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxCommandResultFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxEventSinkFfiError;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxMountPurposeFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxMountSpecFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxNetworkPolicyFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxOperationFfiError;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxPtyOpenRequestFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxPtySessionFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxRootfsStateFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxRuntimeModeFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxStatusFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxStreamEventFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxStreamEventKindFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxStreamSourceFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxTaskFfi;
#[cfg(feature = "uniffi")]
pub use linux_types::MobileLinuxTaskStateFfi;

#[cfg(feature = "uniffi")]
mod mobile_linux_sdk;

// `Platform` is named only inside the `cfg(target_os = "ios")` constructor body;
// importing it unconditionally warns on the host build, so scope it to iOS.

// F3-04: the shared session host + its error type are DEFINED ONCE in
// `harness-runtime::mobile` and re-exported here. Both FFI packager crates re-export the
// SAME types so iOS and Android cannot drift (plan F3-04). The listener +
// permission-request-sink the constructor takes are likewise re-exported from
// the shared host.

// ---------------------------------------------------------------------------
// M10-P3a: the foreign-callable engine constructor.
// ---------------------------------------------------------------------------
//
// `build_mobile_engine` (above) takes an `Arc<dyn Platform>` plus listener and
// permission traits. This thin exported constructor accepts the Swift-local
// callback interfaces (including one unified audio service), adapts them to the
// internal shared host traits, builds `IosPlatform`, and delegates to the same
// mobile engine. Filesystem / HTTP / clock still come from
// `platform-posix-minimal`; audio support is projected from the callback's
// initial snapshot and updated by its service lifecycle.
//
// SECRETS: `api_key` arrives as a parameter the Swift side reads from the
// process environment (`ANTHROPIC_API_KEY`) / an app setting at runtime; it is
// NEVER hardcoded, logged, or persisted here. An empty key is valid — the
// orchestrator only fails at `run_turn` with a 401 (mirrors `MobileConfig`).

/// Device-capability stubs used when the foreign host does not (yet) wire the
/// camera / share callbacks. A text conversation never invokes these;
/// each method returns the trait's "unavailable" error so an accidental call is
/// a clean error rather than a panic. M9 replaces these with the real
/// Swift-backed callback objects threaded through a richer constructor.
///
/// Constructed only on the device/simulator (`target_os = "ios"`) path of
/// [`crate::build_ios_engine`]; on the host bindgen build that path is `cfg`'d out, so
/// the stubs are dead there — `allow(dead_code)` keeps the host build warning-clean.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod stub_capabilities {
    use async_trait::async_trait;
    use lingxi_core::host::{
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

// The foreign permission sink (`IosPermissionSink`) is a callback interface
// DEFINED IN THIS CRATE — mirroring `IosEventListener` — so its UniFFI
// `FfiConverter` lands under `ios_framework`'s tag, a prerequisite for naming it
// as a parameter type in `build_ios_engine`. Where the listener carries OUTBOUND
// events, this carries the engine's OUTBOUND permission requests to the Swift
// host's prompt UI; the inbound resolution flows back through
// `MobileEngineHandle::submit(ClientCommand::Approve/DenyPermission)`.
// `IosPermissionSinkBridge` adapts this crate-local interface to the shared
// `harness_runtime::mobile::PermissionRequestSink` the engine's adapter gate emits onto.

// The foreign event listener (`IosEventListener`) is a callback interface
// DEFINED IN THIS CRATE so its UniFFI `FfiConverterArc` lands under
// `ios_framework`'s tag — a prerequisite for naming it in a `#[uniffi::export]`
// function here. (The shared `client::adapter::ClientEventListener` registers its
// converter under `client_adapter`'s tag, so it cannot be a parameter type in
// an export from a different crate.) `IosListenerBridge` adapts this crate-local
// interface to the shared `ClientEventListener` the engine actually feeds.

// ---------------------------------------------------------------------------
// Other device-capability FFI callbacks (iOS parity with android-aar).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Device status / haptics / deep links — one native callback object fans out
// to the three shared Platform seams.

// F3-04: re-export `harness-runtime::mobile`'s UniFFI scaffolding so the shared host's FFI
// symbols (the re-exported `MobileEngineHandle` / `MobileEngineError`) land in
// this crate's final library. Under the `uniffi` feature only.
#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

#[cfg(all(test, feature = "uniffi"))]
#[path = "lib/tests/tests.rs"]
mod tests;
