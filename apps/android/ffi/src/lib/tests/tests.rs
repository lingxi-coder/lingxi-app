use std::sync::Arc;

use async_trait::async_trait;
use client::protocol::events::ClientEvent;
use client::protocol::permission::PermissionRequest as PermissionRequestDto;
use harness_runtime::mobile::{ClientEventListener, MobileConfig, PermissionRequestSink};
use platform_api::{
    CameraControl, Clock, FileSystem, HttpTransport, LocationProvider, Platform, ProcessRunner,
    Sandbox, SharingService, WorktreeManager,
};
use tokio::sync::Mutex;

struct FakeAndroidLocation {
    failure: Option<&'static str>,
}

#[async_trait]
impl super::AndroidLocation for FakeAndroidLocation {
    async fn current_location(&self) -> Result<super::LocationFixFfi, super::LocationFfiError> {
        match self.failure {
            Some("permission") => Err(super::LocationFfiError::PermissionDenied),
            Some("unavailable") => Err(super::LocationFfiError::Unavailable),
            Some("timeout") => Err(super::LocationFfiError::Timeout),
            Some(message) => Err(super::LocationFfiError::Other {
                message: message.to_string(),
            }),
            None => Ok(super::LocationFixFfi {
                latitude: 31.2304,
                longitude: 121.4737,
                accuracy_m: Some(20.5),
                timestamp_ms: 1_753_000_000_000,
            }),
        }
    }
}

#[tokio::test]
async fn android_location_bridge_maps_fix_and_stable_errors() {
    let success = super::AndroidLocationBridge {
        inner: Box::new(FakeAndroidLocation { failure: None }),
    }
    .current_location()
    .await
    .expect("location fix");
    assert_eq!(success.latitude, 31.2304);
    assert_eq!(success.longitude, 121.4737);
    assert_eq!(success.accuracy_m, Some(20.5));
    assert_eq!(success.timestamp_ms, 1_753_000_000_000);

    let permission = super::AndroidLocationBridge {
        inner: Box::new(FakeAndroidLocation {
            failure: Some("permission"),
        }),
    }
    .current_location()
    .await
    .expect_err("permission failure");
    assert!(matches!(
        permission,
        platform_api::LocationError::PermissionDenied
    ));

    let unavailable = super::AndroidLocationBridge {
        inner: Box::new(FakeAndroidLocation {
            failure: Some("unavailable"),
        }),
    }
    .current_location()
    .await
    .expect_err("unavailable failure");
    assert!(matches!(
        unavailable,
        platform_api::LocationError::Unavailable
    ));

    let timeout = super::AndroidLocationBridge {
        inner: Box::new(FakeAndroidLocation {
            failure: Some("timeout"),
        }),
    }
    .current_location()
    .await
    .expect_err("timeout failure");
    assert!(matches!(timeout, platform_api::LocationError::Timeout));

    let other = super::AndroidLocationBridge {
        inner: Box::new(FakeAndroidLocation {
            failure: Some("native failure"),
        }),
    }
    .current_location()
    .await
    .expect_err("other failure");
    assert!(matches!(
        other,
        platform_api::LocationError::Other(message) if message == "native failure"
    ));
}

#[test]
fn android_host_environment_maps_to_shared_stable_facts() {
    let environment: platform_api::mobile_runtime_environment::MobileHostEnvironment =
        super::AndroidHostEnvironmentFfi {
            host_os_version: Some("16 (API 36)".to_string()),
            device_class: super::AndroidDeviceClassFfi::Tablet,
            execution_target: super::AndroidExecutionTargetFfi::Emulator,
            launch_mode: super::AndroidLaunchModeFfi::ScheduledHeadless,
        }
        .into();

    assert_eq!(
        environment.host_os,
        platform_api::mobile_runtime_environment::MobileHostOs::Android
    );
    assert_eq!(environment.host_os_version.as_deref(), Some("16 (API 36)"));
    assert_eq!(
        environment.device_class,
        platform_api::mobile_runtime_environment::MobileDeviceClass::Tablet
    );
    assert_eq!(
        environment.execution_target,
        platform_api::mobile_runtime_environment::MobileExecutionTarget::Emulator
    );
    assert_eq!(
        environment.launch_mode,
        platform_api::mobile_runtime_environment::MobileLaunchMode::ScheduledHeadless
    );
}

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

fn build_handle(root: &std::path::Path) -> Arc<harness_runtime::mobile::MobileEngineHandle> {
    let platform: Arc<dyn Platform> = Arc::new(HostFakePlatform::new(root.to_path_buf()));
    let listener: Arc<dyn ClientEventListener> = Arc::new(FakeListener::default());
    let perm_sink: Arc<dyn PermissionRequestSink> = Arc::new(RecordingPermissionSink::default());
    let cfg = MobileConfig {
        build_info: harness_runtime::mobile::BuildInfo::new(
            env!("CARGO_PKG_VERSION"),
            option_env!("LINGXI_GIT_SHA_SHORT").unwrap_or("unknown"),
        ),
        cwd: root.to_path_buf(),
        lingxi_home: root.join(".lingxi"),
        ..MobileConfig::default()
    };
    harness_runtime::mobile::build_mobile_engine(cfg, platform, listener, perm_sink)
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
    let _orch: Arc<orchestrator::ConversationOrchestrator> = handle.inner().orchestrator.clone();
    let _gate = handle.permission_gate();
    let _listener: Arc<dyn ClientEventListener> = handle.listener();

    // The M8 smoke signal reflects the verified file-backed mobile Plugin
    // catalog. Anchored to the live roster, never to a literal: this assert sat at a
    // stale `1` while `BUILTIN_MOBILE` grew to five, and because the module
    // is `#[cfg(feature = "uniffi")]` a plain `cargo test --workspace`
    // compiled none of it, so the rot only surfaced under `--all-features`.
    let plugin_skills = harness_runtime::mobile::mobile_plugin_skill_names();
    assert_eq!(
        plugin_skills.len(),
        27,
        "mobile ships exactly 27 Plugin skills"
    );
    assert_eq!(
        handle.skill_count() as usize,
        plugin_skills.len(),
        "the handle must expose the live verified mobile Plugin skill catalog"
    );
    // `create-local-app` is always present so the agent can enter the
    // template-guided, approval-gated local-app workflow offline.
    assert!(
        plugin_skills.iter().any(|name| name == "create-local-app"),
        "mobile Plugin skills must include create-local-app; got {:?}",
        plugin_skills
    );
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

#[test]
fn android_guest_shell_requires_explicit_policy() {
    use super::{android_guest_shell_enabled, AndroidShellConfigFfi};
    assert!(!android_guest_shell_enabled(None));
    let mut shell = AndroidShellConfigFfi {
        enable_shell: true,
        secrets_in_keystore: true,
        shell_data_exposure_accepted: true,
    };
    assert!(android_guest_shell_enabled(Some(&shell)));
    shell.secrets_in_keystore = false;
    assert!(!android_guest_shell_enabled(Some(&shell)));
    shell.secrets_in_keystore = true;
    shell.shell_data_exposure_accepted = false;
    assert!(!android_guest_shell_enabled(Some(&shell)));
    shell.shell_data_exposure_accepted = true;
    shell.enable_shell = false;
    assert!(!android_guest_shell_enabled(Some(&shell)));
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

#[test]
fn android_project_cwd_accepts_managed_project_and_local_app_workspaces() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lingxi-android-project-{nonce}"));
    let project_id = "12345678-1234-4abc-8def-1234567890ab";
    let workspace = root.join("projects").join(project_id).join("workspace");
    std::fs::create_dir_all(&workspace).expect("create project fixture");

    let legacy = super::android_project_cwd(root.to_str().expect("utf8"), None)
        .expect("missing project cwd preserves the legacy app root");
    assert_eq!(legacy, root.canonicalize().expect("canonical root"));

    let resolved = super::android_project_cwd(
        root.to_str().expect("utf8"),
        Some(workspace.to_str().expect("utf8")),
    )
    .expect("managed project workspace is accepted");
    assert_eq!(
        resolved,
        workspace.canonicalize().expect("canonical workspace")
    );

    // v3 local apps: `apps/<engine-minted id>/workspace` is a first-class
    // conversation scope on Android too — the engine mints and lists app
    // sessions platform-neutrally, so a gate that rejects them here would
    // show rows that can never be opened.
    let app_workspace = root.join("apps").join("9b48dfb5").join("workspace");
    std::fs::create_dir_all(&app_workspace).expect("create app fixture");
    let resolved_app = super::android_project_cwd(
        root.to_str().expect("utf8"),
        Some(app_workspace.to_str().expect("utf8")),
    )
    .expect("local app workspace is accepted");
    assert_eq!(
        resolved_app,
        app_workspace
            .canonicalize()
            .expect("canonical app workspace")
    );

    let illegal_app = root.join("apps").join("Bad_ID").join("workspace");
    std::fs::create_dir_all(&illegal_app).expect("create illegal-app fixture");
    assert!(
        super::android_project_cwd(
            root.to_str().expect("utf8"),
            Some(illegal_app.to_str().expect("utf8")),
        )
        .is_err(),
        "ids the engine could never mint must not become conversation workspaces"
    );

    let malformed = root.join("projects").join("user-name").join("workspace");
    std::fs::create_dir_all(&malformed).expect("create malformed fixture");
    assert!(
        super::android_project_cwd(
            root.to_str().expect("utf8"),
            Some(malformed.to_str().expect("utf8")),
        )
        .is_err(),
        "user-controlled names must never become project directories"
    );

    let outside = std::env::temp_dir().join(format!("lingxi-outside-project-{nonce}"));
    std::fs::create_dir_all(&outside).expect("create outside fixture");
    assert!(
        super::android_project_cwd(
            root.to_str().expect("utf8"),
            Some(outside.to_str().expect("utf8")),
        )
        .is_err(),
        "workspaces outside filesDir must fail closed"
    );

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(outside);
}

/// P4-T10: the FFI → `AndroidGitToolCtx` mapping yields `enabled = false`
/// when `enable_git` is false even if the other gate inputs (workspace +
/// CA store) are satisfied, and regardless of a present credential provider.
/// Pure-fn level (mirrors the body of [`build_android_engine`]'s git mapping).
#[test]
fn ffi_mapping_disabled_when_enable_git_false() {
    use super::android_git_gate;

    let cfg = super::AndroidGitConfigFfi {
        enable_git: false,
        // Use the workspace's own dir so the readiness check would pass.
        workspace_root: env!("CARGO_MANIFEST_DIR").to_string(),
        // Empty CA dir is treated as reachable (libgit2/OpenSSL defaults).
        ca_cert_dir: String::new(),
        ssh_private_key_path: String::new(),
        ssh_public_key_path: String::new(),
        ssh_known_hosts_sha256_hex: Vec::new(),
    };

    let workspace_ready = std::path::Path::new(&cfg.workspace_root).is_dir();
    let ca_store_reachable =
        cfg.ca_cert_dir.is_empty() || std::path::Path::new(&cfg.ca_cert_dir).exists();
    assert!(workspace_ready, "fixture workspace_root must be a real dir");
    assert!(ca_store_reachable, "empty CA dir must count as reachable");

    // Secrets now ride the per-op `AndroidGitCredentialProvider` callback
    // rather than the FFI config; `has_token` reflects whether that provider
    // is present (mirrors `build_android_engine`'s `credential_provider.is_some()`).
    let credential_provider: Option<std::sync::Arc<dyn tool_api::GitCredentialProvider>> = Some(
        std::sync::Arc::new(super::AndroidGitCredentialProviderBridge {
            inner: Box::new(TestCredProvider),
        }),
    );

    let ctx = tool_api::AndroidGitToolCtx {
        enabled: android_git_gate(cfg.enable_git, workspace_ready, ca_store_reachable),
        has_token: credential_provider.is_some(),
        workspace_root: cfg.workspace_root.clone(),
    };

    assert!(
        !ctx.enabled,
        "Git must be disabled when enable_git is false, even with workspace + CA + provider set"
    );
    assert!(
        ctx.has_token,
        "has_token must still reflect a supplied credential provider"
    );
}

#[test]
fn mobile_linux_run_command_rejects_empty_command() {
    let err =
        super::linux_conversion::command_request_to_traits(super::MobileLinuxCommandRequestFfi {
            command: "   ".to_string(),
            args: vec![],
            cwd: None,
            env: vec![],
            stdin: None,
            timeout_ms: None,
            allow_network: false,
            mounts: vec![],
        })
        .expect_err("empty command must be rejected");
    assert!(matches!(
        err,
        super::MobileLinuxApiErrorFfi::InvalidRequest { .. }
    ));
}

/// Minimal host-side [`super::AndroidGitCredentialProvider`] impl for tests:
/// exercises the bridge onto `tool_api::GitCredentialProvider`.
struct TestCredProvider;
impl super::AndroidGitCredentialProvider for TestCredProvider {
    fn https_token(&self) -> Option<String> {
        Some("pat-token".to_string())
    }
    fn ssh_passphrase(&self) -> Option<String> {
        None
    }
}
