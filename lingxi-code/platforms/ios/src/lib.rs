//! `platform-ios` (M8-P10) — the iOS platform skeleton.
//!
//! [`IosPlatform`] implements the [`traits::Platform`] aggregate. The core OS
//! handles (filesystem/clock/process/sandbox/worktree) are currently reused
//! from `platform-posix-minimal` — those impls are portable Rust (`std::fs`
//! over the App-Sandbox root, `std::time`, and `Unsupported` stubs) and valid
//! on iOS. The `http` handle is the shared real client
//! ([`platform_common::http::ReqwestHttp`], `reqwest` + `rustls-tls`), so a
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

use std::path::PathBuf;
use std::sync::Arc;
use traits::{
    CameraControl, Clipboard, Clock, FileSystem, HttpTransport, NotificationService, Platform,
    ProcessRunner, Sandbox, SharingService, SpeechToText, TextToSpeech, VoiceRecorder,
    WorktreeManager,
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
}

impl IosPlatform {
    /// Assemble an [`IosPlatform`] from native inputs.
    #[must_use]
    pub fn new(inputs: IosPlatformInputs) -> Self {
        use platform_posix_minimal::{
            PosixClock, PosixFileSystem, PosixProcess, PosixSandbox, PosixWorktree,
        };
        Self {
            fs: Arc::new(PosixFileSystem::new(inputs.app_sandbox_root)),
            http: Arc::new(platform_common::http::ReqwestHttp::new()),
            clock: Arc::new(PosixClock::new()),
            process: Arc::new(PosixProcess::new()),
            sandbox: Arc::new(PosixSandbox::new()),
            worktree: Arc::new(PosixWorktree::new()),
            camera: inputs.camera,
            voice: inputs.voice,
            share: inputs.share,
            stt: inputs.stt,
            tts: inputs.tts,
            notifications: inputs.notifications,
            clipboard: inputs.clipboard,
        }
    }
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
    // computer_control() defaults to None — screen automation is not an iOS
    // capability in M8.
}
