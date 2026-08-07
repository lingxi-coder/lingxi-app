//! `platform-ios` (M8-P10) — the iOS platform skeleton.
//!
//! [`IosPlatform`] implements the [`traits::Platform`] aggregate. The core OS
//! handles (filesystem/clock/process/sandbox/worktree) are currently reused
//! from `platform-posix-minimal` — those impls are portable Rust (`std::fs`
//! over the App-Sandbox root, `std::time`, and `Unsupported` stubs) and valid
//! on iOS. The `http` handle is the shared real client
//! ([`http_client::ReqwestHttp`], `reqwest` + `rustls-tls`), so a
//! keyed conversation streams against the real provider rather than the
//! posix-minimal stub. The iOS-specific device capabilities (camera, voice,
//! share) are injected as `Arc<dyn …>` trait objects implemented natively in
//! Swift via `UniFFI` (P12).
//!
//! M9 replaces the reused posix handles with App-Sandbox-aware iOS impls. The
//! crate is intentionally **not** `#[cfg(target_os = "ios")]`-gated: the
//! skeleton is portable, so it compiles and is verified on the host build and
//! cross-compiles to `aarch64-apple-ios` unchanged.

#![forbid(unsafe_code)]

use platform_common::{MobileLinuxProcessRunner, MobileLinuxSandbox};
use std::path::PathBuf;
use std::sync::Arc;
use traits::{
    CameraControl, Clipboard, Clock, FileSystem, HttpTransport, MobileLinuxRuntime,
    MobileLinuxRuntimeMode, MountPurpose, MountSpec, NotificationService, Platform, ProcessRunner,
    Sandbox, SandboxBackend, SecureStorage, SharingService, SpeechToText, TextToSpeech,
    UnavailableMobileLinuxRuntime, VoiceRecorder, WorktreeManager,
};

/// Construction inputs for [`IosPlatform`].
///
/// The native capabilities are supplied by the Swift layer (via `UniFFI` in P12);
/// `app_sandbox_root` is the container directory the filesystem is confined to.
pub struct IosPlatformInputs {
    /// The app's writable sandbox container root.
    pub app_sandbox_root: PathBuf,
    /// Native camera (Swift impl).
    pub camera: Arc<dyn CameraControl>,
    /// Native microphone recorder (Swift impl).
    pub voice: Arc<dyn VoiceRecorder>,
    /// Native share sheet (Swift impl).
    pub share: Arc<dyn SharingService>,
    /// Native speech-to-text (Swift impl), when wired. `None` keeps the
    /// pre-speech behavior (the `speech` tool reports "unavailable").
    pub stt: Option<Arc<dyn SpeechToText>>,
    /// Native text-to-speech (Swift impl), when wired.
    pub tts: Option<Arc<dyn TextToSpeech>>,
    /// Native system notifications (Swift impl), when wired. `None` keeps the
    /// `notification` tool reporting "unavailable".
    pub notifications: Option<Arc<dyn NotificationService>>,
    /// Native system clipboard (Swift impl), when wired. `None` keeps the
    /// `clipboard` tool reporting "unavailable".
    pub clipboard: Option<Arc<dyn Clipboard>>,
    /// Native iOS Keychain-backed secure store (Swift impl), when wired. `None`
    /// keeps the non-persisting development stub, which gates OAuth `/login` off
    /// (it cannot persist tokens). Inject a real store to enable subscription login.
    pub secure_storage: Option<Arc<dyn SecureStorage>>,
    /// Mobile Linux runtime bridge (iSH path). `None` keeps the legacy
    /// unavailable shell behavior in place.
    pub mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    /// Explicit host workspace root exposed to the guest when mobile-linux mode
    /// is selected. Legacy mode ignores it.
    pub workspace_host_path: Option<PathBuf>,
    /// Stable guest workspace id used to produce `/workspace/<id>`.
    pub stable_workspace_id: Option<String>,
}

/// The iOS [`Platform`].
pub struct IosPlatform {
    fs: Arc<dyn FileSystem>,
    http: Arc<dyn HttpTransport>,
    clock: Arc<dyn Clock>,
    process: Arc<dyn ProcessRunner>,
    sandbox: Arc<dyn Sandbox>,
    worktree: Arc<dyn WorktreeManager>,
    camera: Arc<dyn CameraControl>,
    voice: Arc<dyn VoiceRecorder>,
    share: Arc<dyn SharingService>,
    stt: Option<Arc<dyn SpeechToText>>,
    tts: Option<Arc<dyn TextToSpeech>>,
    notifications: Option<Arc<dyn NotificationService>>,
    clipboard: Option<Arc<dyn Clipboard>>,
    secure_storage: Option<Arc<dyn SecureStorage>>,
    mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
}

impl IosPlatform {
    /// Assemble an [`IosPlatform`] from native inputs.
    #[must_use]
    pub fn new(inputs: IosPlatformInputs) -> Self {
        use platform_posix_minimal::{
            PosixClock, PosixFileSystem, PosixProcess, PosixSandbox, PosixWorktree,
        };
        let mobile_linux_selected = inputs
            .mobile_linux
            .as_ref()
            .is_some_and(|runtime| matches!(runtime.mode(), MobileLinuxRuntimeMode::MobileLinux));
        let workspace_mounts = build_mobile_linux_mounts(
            inputs.workspace_host_path.clone(),
            inputs.stable_workspace_id.clone(),
        );
        let runtime = inputs.mobile_linux.clone();
        let (process, sandbox, effective_runtime): (
            Arc<dyn ProcessRunner>,
            Arc<dyn Sandbox>,
            Option<Arc<dyn MobileLinuxRuntime>>,
        ) = if mobile_linux_selected {
            let runtime = runtime.expect("mobile-linux runtime must exist when selected");
            let (runtime, sandbox): (Arc<dyn MobileLinuxRuntime>, Arc<dyn Sandbox>) =
                match MobileLinuxSandbox::new(runtime.clone(), workspace_mounts) {
                    Ok(sandbox) => (runtime, Arc::new(sandbox)),
                    Err(error) => {
                        let unavailable: Arc<dyn MobileLinuxRuntime> =
                            Arc::new(UnavailableMobileLinuxRuntime::unavailable(
                                SandboxBackend::IosIsh,
                                MobileLinuxRuntimeMode::MobileLinux,
                                "ios",
                                "arm64",
                                format!("invalid mobile-linux workspace mount: {error}"),
                            ));
                        let sandbox = MobileLinuxSandbox::new(unavailable.clone(), Vec::new())
                            .expect("empty mobile-linux mount set must be valid");
                        (unavailable, Arc::new(sandbox))
                    }
                };
            let process =
                Arc::new(MobileLinuxProcessRunner::new(runtime.clone())) as Arc<dyn ProcessRunner>;
            (process, sandbox, Some(runtime))
        } else {
            (
                Arc::new(PosixProcess::new()) as Arc<dyn ProcessRunner>,
                Arc::new(PosixSandbox::new()) as Arc<dyn Sandbox>,
                runtime,
            )
        };
        Self {
            fs: Arc::new(PosixFileSystem::new(inputs.app_sandbox_root)),
            http: Arc::new(http_client::ReqwestHttp::new()),
            clock: Arc::new(PosixClock::new()),
            process,
            sandbox,
            worktree: Arc::new(PosixWorktree::new()),
            camera: inputs.camera,
            voice: inputs.voice,
            share: inputs.share,
            stt: inputs.stt,
            tts: inputs.tts,
            notifications: inputs.notifications,
            clipboard: inputs.clipboard,
            secure_storage: inputs.secure_storage,
            mobile_linux: effective_runtime,
        }
    }
}

fn build_mobile_linux_mounts(
    workspace_host_path: Option<PathBuf>,
    stable_workspace_id: Option<String>,
) -> Vec<MountSpec> {
    let guest_workspace_id = stable_workspace_id.unwrap_or_else(|| "default".to_string());
    workspace_host_path
        .map(|host_path| {
            vec![MountSpec {
                host_path,
                guest_path: traits::mobile_linux::guest_paths::workspace(&guest_workspace_id),
                read_only: false,
                purpose: MountPurpose::Workspace,
            }]
        })
        .unwrap_or_default()
}

impl Platform for IosPlatform {
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
        Some(self.camera.clone())
    }
    fn voice(&self) -> Option<Arc<dyn VoiceRecorder>> {
        Some(self.voice.clone())
    }
    fn share(&self) -> Option<Arc<dyn SharingService>> {
        Some(self.share.clone())
    }
    fn stt(&self) -> Option<Arc<dyn SpeechToText>> {
        self.stt.clone()
    }
    fn tts(&self) -> Option<Arc<dyn TextToSpeech>> {
        self.tts.clone()
    }
    fn notifications(&self) -> Option<Arc<dyn NotificationService>> {
        self.notifications.clone()
    }
    fn clipboard(&self) -> Option<Arc<dyn Clipboard>> {
        self.clipboard.clone()
    }
    fn secure_storage(&self) -> Option<Arc<dyn SecureStorage>> {
        self.secure_storage.clone()
    }
    fn mobile_linux(&self) -> Option<Arc<dyn MobileLinuxRuntime>> {
        self.mobile_linux.clone()
    }
    // computer_control() defaults to None — screen automation is not an iOS
    // capability in M8.
}
