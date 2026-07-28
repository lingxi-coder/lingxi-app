//! `platform-android` (M8-P10) — the Android platform skeleton.
//!
//! [`AndroidPlatform`] implements the [`traits::Platform`] aggregate. The core
//! OS handles (filesystem/clock/process/sandbox/worktree) are currently reused
//! from `platform-posix-minimal` (portable Rust, valid on Android). The `http`
//! handle is the shared real client ([`http_client::ReqwestHttp`],
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

pub mod capabilities;
pub mod config;
pub mod policy;
pub mod process;
pub mod receipt;
pub mod sandbox;

pub use capabilities::{AndroidSandboxCapabilities, CapabilityCache};
pub use config::AndroidShellConfig;
pub use policy::{
    build_shell_env, plan_from_policy, AndroidSandboxPlan, ExecTarget, NetProfile, ProcessCleanup,
    Rlimit, RlimitResource, SeccompRef,
};
pub use process::AndroidMinijailProcessRunner;
pub use receipt::AndroidSandboxReceipt;
pub use sandbox::AndroidMinijailSandbox;

use platform_common::{MobileLinuxProcessRunner, MobileLinuxSandbox};
use std::path::PathBuf;
use std::sync::Arc;
use traits::{
    CameraControl, Clipboard, Clock, FileSystem, HttpTransport, MobileLinuxRuntime,
    MobileLinuxRuntimeMode, MountPurpose, MountSpec, NotificationService, Platform, ProcessRunner,
    Sandbox, SandboxBackend, SandboxError, SecureStorage, SharingService, SpeechToText,
    TextToSpeech, UnavailableMobileLinuxRuntime, VoiceRecorder, WorktreeManager,
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
    /// Native Android Keystore-backed secure store (Kotlin impl), when wired.
    /// `None` keeps the non-persisting development stub, which gates OAuth
    /// `/login` off (it cannot persist tokens). Inject a real store to enable
    /// subscription login.
    pub secure_storage: Option<Arc<dyn SecureStorage>>,
    /// Mobile Linux runtime bridge (Android PRoot path). `None` keeps the
    /// legacy shell runner in place.
    pub mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    /// Host workspace root exposed to the mobile-linux guest. Defaults to
    /// `<app_files_root>/workspaces/default` when unset.
    pub mobile_linux_workspace_root: Option<PathBuf>,
    /// Stable workspace identifier for the guest path `/workspace/<id>`.
    pub mobile_linux_workspace_id: Option<String>,
    /// Managed rootfs directory reserved for the runtime implementation.
    pub mobile_linux_managed_root: Option<PathBuf>,
    /// Android shell/sandbox configuration (spec r3). `None` keeps shell
    /// support fully absent (posix-minimal stubs stay wired).
    pub shell: Option<AndroidShellConfig>,
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
    secure_storage: Option<Arc<dyn SecureStorage>>,
    mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    mobile_linux_mode: MobileLinuxRuntimeMode,
    /// The shared capability cache when shell support is wired (`None` for the
    /// posix-minimal-stub configuration). Held so the eager probe (engine-mobile)
    /// and the runner can read/populate the SAME instance the sandbox reads.
    shell_caps: Option<std::sync::Arc<crate::capabilities::CapabilityCache>>,
}

impl AndroidPlatform {
    /// Assemble an [`AndroidPlatform`] from native inputs.
    #[must_use]
    pub fn new(inputs: AndroidPlatformInputs) -> Self {
        Self::new_with_mode(inputs, MobileLinuxRuntimeMode::Legacy)
    }

    /// Assemble an [`AndroidPlatform`] while explicitly selecting the shared
    /// mobile Linux runtime mode. The legacy constructor remains unchanged for
    /// current production callers.
    #[must_use]
    pub fn new_with_mode(
        inputs: AndroidPlatformInputs,
        mobile_linux_mode: MobileLinuxRuntimeMode,
    ) -> Self {
        use platform_posix_minimal::{
            PosixClock, PosixFileSystem, PosixProcess, PosixSandbox, PosixWorktree,
        };
        let default_workspace_root = inputs.app_files_root.join("workspaces").join("default");
        let workspace_root = inputs
            .mobile_linux_workspace_root
            .clone()
            .or_else(|| {
                inputs
                    .shell
                    .as_ref()
                    .map(|cfg| cfg.shell_workspace_root.clone())
            })
            .unwrap_or(default_workspace_root);
        let workspace_id = inputs
            .mobile_linux_workspace_id
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let workspace_valid =
            workspace_root_is_allowed(&workspace_root, inputs.mobile_linux_managed_root.as_deref());
        let mobile_linux_runtime = match (mobile_linux_mode, inputs.mobile_linux.clone()) {
            (MobileLinuxRuntimeMode::MobileLinux, Some(runtime)) => Some(runtime),
            (MobileLinuxRuntimeMode::MobileLinux, None) => {
                Some(Arc::new(UnavailableMobileLinuxRuntime::unavailable(
                    SandboxBackend::AndroidProot,
                    MobileLinuxRuntimeMode::MobileLinux,
                    "android",
                    "unknown",
                    "mobile-linux mode selected without a wired Android runtime",
                )) as Arc<dyn MobileLinuxRuntime>)
            }
            (MobileLinuxRuntimeMode::Legacy, runtime) => runtime,
        };
        let (process, sandbox, shell_caps, effective_mobile_linux_runtime) = match (
            mobile_linux_mode,
            mobile_linux_runtime.clone(),
            inputs.shell,
        ) {
            (MobileLinuxRuntimeMode::MobileLinux, Some(runtime), _) => {
                let sandbox_result = if workspace_valid {
                    MobileLinuxSandbox::new(
                        runtime.clone(),
                        default_mobile_linux_mounts(&workspace_root, &workspace_id),
                    )
                } else {
                    Err(SandboxError::Unavailable(
                        "invalid mobile-linux workspace mount configuration".to_string(),
                    ))
                };
                let (runtime, sandbox): (Arc<dyn MobileLinuxRuntime>, Arc<dyn Sandbox>) =
                    match sandbox_result {
                        Ok(sandbox) => (runtime, Arc::new(sandbox)),
                        Err(error) => {
                            let unavailable: Arc<dyn MobileLinuxRuntime> =
                                Arc::new(UnavailableMobileLinuxRuntime::unavailable(
                                    SandboxBackend::AndroidProot,
                                    MobileLinuxRuntimeMode::MobileLinux,
                                    "android",
                                    "unknown",
                                    format!("invalid mobile-linux workspace mount: {error}"),
                                ));
                            let sandbox = MobileLinuxSandbox::new(unavailable.clone(), Vec::new())
                                .expect("empty mobile-linux mount set must be valid");
                            (unavailable, Arc::new(sandbox))
                        }
                    };
                let process: Arc<dyn ProcessRunner> =
                    Arc::new(MobileLinuxProcessRunner::new(runtime.clone()));
                (process, sandbox, None, Some(runtime))
            }
            (_, _, Some(shell_cfg)) => {
                let caps = Arc::new(crate::capabilities::CapabilityCache::new());
                let process: Arc<dyn ProcessRunner> = Arc::new(
                    crate::process::AndroidMinijailProcessRunner::new(caps.clone()),
                );
                let sandbox: Arc<dyn Sandbox> = Arc::new(
                    crate::sandbox::AndroidMinijailSandbox::new(shell_cfg, caps.clone()),
                );
                (process, sandbox, Some(caps), mobile_linux_runtime)
            }
            (_, _, None) => (
                Arc::new(PosixProcess::new()) as Arc<dyn ProcessRunner>,
                Arc::new(PosixSandbox::new()) as Arc<dyn Sandbox>,
                None,
                mobile_linux_runtime,
            ),
        };
        Self {
            fs: Arc::new(PosixFileSystem::new(inputs.app_files_root)),
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
            mobile_linux: effective_mobile_linux_runtime,
            mobile_linux_mode,
            shell_caps,
        }
    }

    /// The shared shell capability cache, when shell support is wired.
    ///
    /// Returns `Some(Arc<CapabilityCache>)` when the platform was constructed
    /// with an [`AndroidShellConfig`], `None` for the posix-minimal-stub
    /// configuration. The eager probe (engine-mobile) uses this to populate the
    /// cache before tool registration; the runner uses it to gate per-plan
    /// admission (e.g., `DenyNet` requires `seccomp_filter + net_deny_verified`).
    #[must_use]
    pub fn shell_capability_cache(
        &self,
    ) -> Option<std::sync::Arc<crate::capabilities::CapabilityCache>> {
        self.shell_caps.clone()
    }

    /// Selected mobile Linux runtime mode.
    #[must_use]
    pub fn mobile_linux_mode(&self) -> MobileLinuxRuntimeMode {
        self.mobile_linux_mode
    }
}

fn default_mobile_linux_mounts(
    workspace_root: &std::path::Path,
    workspace_id: &str,
) -> Vec<MountSpec> {
    let workspace_id = sanitize_workspace_id(workspace_id);
    vec![MountSpec {
        host_path: workspace_root.to_path_buf(),
        guest_path: format!("/workspace/{workspace_id}"),
        read_only: false,
        purpose: MountPurpose::Workspace,
    }]
}

fn sanitize_workspace_id(input: &str) -> String {
    let filtered: String = input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if filtered.is_empty() {
        "default".to_string()
    } else {
        filtered
    }
}

fn workspace_root_is_allowed(
    workspace_root: &std::path::Path,
    managed_root: Option<&std::path::Path>,
) -> bool {
    let text = workspace_root.to_string_lossy().to_ascii_lowercase();
    if text.contains("/.lingxi")
        || text.contains("keystore")
        || text.contains("credential")
        || text.contains("secret")
        || text.contains("token")
    {
        return false;
    }
    if let Some(managed_root) = managed_root {
        if workspace_root.starts_with(managed_root) || managed_root.starts_with(workspace_root) {
            return false;
        }
    }
    true
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
    fn secure_storage(&self) -> Option<Arc<dyn SecureStorage>> {
        self.secure_storage.clone()
    }
    fn mobile_linux(&self) -> Option<Arc<dyn MobileLinuxRuntime>> {
        self.mobile_linux.clone()
    }
    // computer_control() defaults to None.
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use traits::{
        CameraControl, CameraError, CapturePhotoOpts, CapturedImage, Platform, SandboxBackend,
        ShareError, SharePayload, ShareResult, SharingService, UnavailableMobileLinuxRuntime,
        VoiceError, VoiceRecorder, VoiceRecording, VoiceRecordingOpts,
    };

    struct NoCam;
    #[async_trait]
    impl CameraControl for NoCam {
        async fn capture_photo(&self, _: CapturePhotoOpts) -> Result<CapturedImage, CameraError> {
            Err(CameraError::DeviceUnavailable)
        }
        async fn pick_from_library(&self) -> Result<CapturedImage, CameraError> {
            Err(CameraError::DeviceUnavailable)
        }
    }
    struct NoVoice;
    #[async_trait]
    impl VoiceRecorder for NoVoice {
        async fn start_recording(&self, _: VoiceRecordingOpts) -> Result<(), VoiceError> {
            Err(VoiceError::NotRecording)
        }
        async fn stop_recording(&self) -> Result<VoiceRecording, VoiceError> {
            Err(VoiceError::NotRecording)
        }
        async fn is_recording(&self) -> bool {
            false
        }
    }
    struct NoShare;
    #[async_trait]
    impl SharingService for NoShare {
        async fn share(&self, _: SharePayload) -> Result<ShareResult, ShareError> {
            Err(ShareError::Unsupported)
        }
    }

    fn inputs(shell: Option<AndroidShellConfig>) -> AndroidPlatformInputs {
        AndroidPlatformInputs {
            app_files_root: std::env::temp_dir(),
            camera: std::sync::Arc::new(NoCam),
            voice: std::sync::Arc::new(NoVoice),
            share: std::sync::Arc::new(NoShare),
            stt: None,
            tts: None,
            notifications: None,
            clipboard: None,
            secure_storage: None,
            mobile_linux: None,
            mobile_linux_workspace_root: None,
            mobile_linux_workspace_id: None,
            mobile_linux_managed_root: None,
            shell,
        }
    }

    fn shell_cfg() -> AndroidShellConfig {
        AndroidShellConfig {
            native_library_dir: std::env::temp_dir(),
            shell_workspace_root: std::env::temp_dir(),
            app_cache_root: std::env::temp_dir(),
            package_name: "com.example".into(),
            package_version_code: 1,
            app_writable_roots: vec![],
            enable_shell: true,
            secrets_in_keystore: true,
            shell_data_exposure_accepted: false,
            bundled_mksh_path: None,
            bundled_mksh_hash: None,
            bundled_applet_dir: None,
        }
    }

    #[test]
    fn shell_config_wires_android_sandbox_and_runner() {
        let p = AndroidPlatform::new(inputs(Some(shell_cfg())));
        assert_eq!(p.sandbox().backend(), SandboxBackend::AndroidMinijail);
        assert!(
            !p.process().is_available(),
            "fresh (unprobed) cache reads conservative-unavailable until the eager probe"
        );
        assert!(
            !p.sandbox().is_available(),
            "fresh cache reads conservative-unavailable until the eager probe (P2)"
        );
    }

    #[test]
    fn no_shell_config_keeps_posix_minimal_stubs() {
        let p = AndroidPlatform::new(inputs(None));
        assert_eq!(p.sandbox().backend(), SandboxBackend::None);
    }

    #[test]
    fn shell_platform_exposes_shared_capability_cache() {
        let p = AndroidPlatform::new(inputs(Some(shell_cfg())));
        let cache = p
            .shell_capability_cache()
            .expect("shell platform exposes its cache");
        // Same instance the sandbox reads: setting it flips is_available().
        cache.set(crate::capabilities::AndroidSandboxCapabilities {
            probed: true,
            minijail_smoke: true,
            no_new_privs: true,
            ..crate::capabilities::AndroidSandboxCapabilities::default()
        });
        assert!(p.sandbox().is_available(), "sandbox reads the shared cache");
    }

    #[test]
    fn no_shell_platform_has_no_cache() {
        let p = AndroidPlatform::new(inputs(None));
        assert!(p.shell_capability_cache().is_none());
    }

    #[test]
    fn explicit_mobile_linux_mode_and_runtime_are_retained() {
        let runtime = Arc::new(UnavailableMobileLinuxRuntime::blocked(
            SandboxBackend::AndroidProot,
            MobileLinuxRuntimeMode::MobileLinux,
            "android",
            "arm64-v8a",
            "license blocked",
        ));
        let mut android_inputs = inputs(Some(shell_cfg()));
        android_inputs.mobile_linux = Some(runtime.clone());

        let platform =
            AndroidPlatform::new_with_mode(android_inputs, MobileLinuxRuntimeMode::MobileLinux);

        assert_eq!(
            platform.mobile_linux_mode(),
            MobileLinuxRuntimeMode::MobileLinux
        );
        let rt = platform.mobile_linux().expect("runtime should be injected");
        assert_eq!(rt.backend(), SandboxBackend::AndroidProot);
        assert_eq!(rt.mode(), MobileLinuxRuntimeMode::MobileLinux);
    }
}
