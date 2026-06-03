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
        }));
        engine_mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (impls, listener, permission_sink);
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
