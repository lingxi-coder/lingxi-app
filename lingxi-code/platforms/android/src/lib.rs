//! `platform-android` (M8-P10) — the Android platform skeleton.
//!
//! [`AndroidPlatform`] implements the [`traits::Platform`] aggregate. The core
//! OS handles (filesystem/clock/process/sandbox/worktree) are currently reused
//! from `platform-posix-minimal` (portable Rust, valid on Android). The `http`
//! handle is the shared real client ([`platform_common::http::ReqwestHttp`],
//! `reqwest` + `rustls-tls`), so a keyed conversation streams against the real
//! provider rather than the posix-minimal stub. The Android-specific device
//! capabilities (camera, voice, share) are injected as `Arc<dyn …>` trait
//! objects implemented natively in Kotlin via `UniFFI` (P12).
//!
//! M9 replaces the reused posix handles with scoped-storage-aware Android
//! impls. The crate is intentionally **not** `#[cfg(target_os = "android")]`-
//! gated: the skeleton is portable, so it compiles + is verified on the host
//! build and cross-compiles to `aarch64-linux-android` unchanged.

#![forbid(unsafe_code)]

pub mod policy;

pub use policy::{
    build_shell_env, plan_from_policy, AndroidSandboxPlan, ExecTarget, NetProfile, ProcessCleanup,
    Rlimit, RlimitResource, SeccompRef,
};

use std::path::PathBuf;
use std::sync::Arc;
use traits::{
    CameraControl, Clipboard, Clock, FileSystem, HttpTransport, NotificationService, Platform,
    ProcessRunner, Sandbox, SharingService, SpeechToText, TextToSpeech, VoiceRecorder,
    WorktreeManager,
};

/// Construction inputs for [`AndroidPlatform`].
///
/// The native capabilities are supplied by the Kotlin layer (via `UniFFI` in
/// P12); `app_files_root` is the app-private files directory the filesystem is
/// confined to.
pub struct AndroidPlatformInputs {
    /// The app's writable private files-dir root.
    pub app_files_root: PathBuf,
    /// Native camera (Kotlin impl).
    pub camera: Arc<dyn CameraControl>,
    /// Native microphone recorder (Kotlin impl).
    pub voice: Arc<dyn VoiceRecorder>,
    /// Native share sheet (Kotlin impl).
    pub share: Arc<dyn SharingService>,
    /// Native speech-to-text (Kotlin impl), when wired. `None` keeps the
    /// pre-speech behavior (the `speech` tool reports "unavailable").
    pub stt: Option<Arc<dyn SpeechToText>>,
    /// Native text-to-speech (Kotlin impl), when wired.
    pub tts: Option<Arc<dyn TextToSpeech>>,
    /// Native system notifications (Kotlin impl), when wired. `None` keeps the
    /// `notification` tool reporting "unavailable".
    pub notifications: Option<Arc<dyn NotificationService>>,
    /// Native system clipboard (Kotlin impl), when wired. `None` keeps the
    /// `clipboard` tool reporting "unavailable".
    pub clipboard: Option<Arc<dyn Clipboard>>,
}

/// The Android [`Platform`].
pub struct AndroidPlatform {
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

impl AndroidPlatform {
    /// Assemble an [`AndroidPlatform`] from native inputs.
    #[must_use]
    pub fn new(inputs: AndroidPlatformInputs) -> Self {
        use platform_posix_minimal::{
            PosixClock, PosixFileSystem, PosixProcess, PosixSandbox, PosixWorktree,
        };
        Self {
            fs: Arc::new(PosixFileSystem::new(inputs.app_files_root)),
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

impl Platform for AndroidPlatform {
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
    // computer_control() defaults to None.
}
