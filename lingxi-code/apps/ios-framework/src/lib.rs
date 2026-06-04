//! `ios-framework` (M8-P12 → M10-F3) — the iOS UniFFI packager.
//!
//! This crate is the FFI boundary between the Rust engine and the iOS app. The
//! Swift layer implements the [`traits::CameraControl`] / [`traits::VoiceRecorder`]
//! / [`traits::SharingService`] callback interfaces (see the skeletons under
//! `swift/`), hands them across as a [`PlatformImpls`] record, and Rust uses
//! them to construct an `IosPlatform` and assemble the mobile engine — so Rust
//! drives the device's native capabilities by calling *back* into Swift. That
//! bidirectional flow is the whole point of the UniFFI seam.
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
//! ## UniFFI status
//! The `uniffi` feature (default-on) lights up the real UniFFI surface: the
//! re-exported [`MobileEngineHandle`] is a `#[derive(uniffi::Object)]`, the
//! listener a callback interface, the DTOs UniFFI types. `engine-mobile` carries
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
/// `IosPlatform`. UniFFI marshals each `Arc<dyn …>` as a callback-interface
/// reference; `app_sandbox_root` is the app container path.
pub struct PlatformImpls {
    /// Swift `CameraControl` impl.
    pub camera: Arc<dyn CameraControl>,
    /// Swift `VoiceRecorder` impl.
    pub voice: Arc<dyn VoiceRecorder>,
    /// Swift `SharingService` impl.
    pub share: Arc<dyn SharingService>,
    /// The app's writable sandbox container root.
    pub app_sandbox_root: String,
}

/// Top-level UniFFI constructor: build the mobile engine from the Swift-supplied
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
            claude_home: std::path::PathBuf::from(&impls.app_sandbox_root).join(".claude"),
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
/// per event; no transformation. (UniFFI lifts a `callback_interface` as a
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
pub fn build_ios_engine(
    api_base: String,
    api_key: String,
    model: String,
    app_sandbox_root: String,
    listener: Box<dyn IosEventListener>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    let listener: Arc<dyn ClientEventListener> = Arc::new(IosListenerBridge { inner: listener });
    #[cfg(target_os = "ios")]
    {
        use platform_ios::{IosPlatform, IosPlatformInputs};
        let mut cfg = MobileConfig {
            cwd: std::path::PathBuf::from(&app_sandbox_root),
            claude_home: std::path::PathBuf::from(&app_sandbox_root).join(".claude"),
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
            camera: Arc::new(stub_capabilities::StubCamera),
            voice: Arc::new(stub_capabilities::StubVoice),
            share: Arc::new(stub_capabilities::StubShare),
            stt: None,
            tts: None,
            notifications: None,
            clipboard: None,
        }));
        let permission_sink: Arc<dyn PermissionRequestSink> = Arc::new(NoopPermissionSink);
        engine_mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (api_base, api_key, model, app_sandbox_root, listener);
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
