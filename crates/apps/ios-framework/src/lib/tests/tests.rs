/// The root the ios-ish runtime validates local-app mounts against must be
/// the SAME directory the engine writes local apps into.
///
/// It was not. The engine's data root is whatever Swift's
/// `appSandboxRoot()` returns — `<AppSupport>/LingxiCode`, which reaches
/// the engine as `lingxi_home`'s parent — while the runtime INFERRED its
/// own root from `managed_root` (`<AppSupport>/mobile-linux/ios-ish`) by
/// cutting everything from `Library/Application Support` onward. The two
/// answers differ by three components, so `validate_mount` computed an
/// expected build path that nothing ever writes to and EVERY local-app
/// build failed on device with "mount host_path must be …".
///
/// The inference cannot be repaired in place: `LingxiCode` is not
/// derivable from `mobile-linux/ios-ish`. It has to be told.
#[test]
fn the_runtime_sandbox_root_is_the_engine_data_root_not_the_container() {
    // The literal shapes both sides produce on device, from
    // `LXISHDefaultWorkspace.managedRootPath()` and
    // `ConversationSourceFactory.appSandboxRoot()`.
    let container = "/private/var/mobile/Containers/Data/Application/203ED8B0";
    let support = format!("{container}/Library/Application Support");
    let engine_data_root = format!("{support}/LingxiCode");

    let config = super::IosMobileLinuxConfigFfi {
        mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
        managed_root: format!("{support}/mobile-linux/ios-ish"),
        workspace_host_path: format!("{engine_data_root}/workspaces/default"),
        stable_workspace_id: "default".to_string(),
        abi: "arm64".to_string(),
        rootfs_version: "3.20".to_string(),
        archive_sha256: None,
        authorization_file: None,
        app_sandbox_root: engine_data_root.clone(),
    };

    let resolved = super::resolve_mobile_linux_app_sandbox_root(&config)
        .expect("the shipped device paths must resolve");
    assert_eq!(
        resolved,
        std::path::PathBuf::from(&engine_data_root),
        "the runtime must validate mounts against the engine's data root; \
         resolving to {container:?} is what broke every local-app build"
    );

    // The inference this replaced returned exactly `container`. Pinning the
    // rejection keeps a well-meaning "fall back when it's empty" from
    // quietly restoring a second authority on this directory.
    let empty = super::IosMobileLinuxConfigFfi {
        app_sandbox_root: String::new(),
        ..config
    };
    assert!(
        super::resolve_mobile_linux_app_sandbox_root(&empty).is_err(),
        "an absent root must fail loudly, never fall back to a guess"
    );
}

/// A task's non-terminal status changes must NEVER surface as stream
/// events: `event_to_ffi` used to map EVERY `TaskStatusChanged` to
/// `kind: Exit`, so the `Running` emitted by PTY task CREATION (task id =
/// session id) closed a freshly opened, healthy terminal with
/// "[process exited]" on its very first read — and every restart died at
/// its own creation event the same way.
#[test]
fn non_terminal_task_status_events_are_skipped_and_terminal_ones_map_to_exit() {
    let event = |status| mobile_linux_api::MobileLinuxEvent {
        sequence: 1,
        task_id: Some("session-1".to_string()),
        kind: mobile_linux_api::MobileLinuxEventKind::TaskStatusChanged {
            status,
            exit_code: None,
            detail: None,
        },
    };
    for status in [
        mobile_linux_api::MobileLinuxTaskStatus::Queued,
        mobile_linux_api::MobileLinuxTaskStatus::Running,
        mobile_linux_api::MobileLinuxTaskStatus::Backgrounded,
    ] {
        assert!(
            super::event_to_ffi(event(status)).is_none(),
            "{status:?} must not become a stream event"
        );
    }
    let ffi = super::event_to_ffi(event(mobile_linux_api::MobileLinuxTaskStatus::Completed))
        .expect("terminal status maps");
    assert!(matches!(
        ffi.kind,
        super::MobileLinuxStreamEventKindFfi::Exit
    ));
    assert_eq!(ffi.stream_id, "session-1");
    let timed_out = super::event_to_ffi(event(mobile_linux_api::MobileLinuxTaskStatus::TimedOut))
        .expect("terminal status maps");
    assert!(timed_out.timed_out);
}

use std::sync::Arc;

use async_trait::async_trait;
use client_protocol::events::ClientEvent;
use client_protocol::permission::PermissionRequest as PermissionRequestDto;
use harness_runtime::mobile::{ClientEventListener, MobileConfig, PermissionRequestSink};
use platform_api::{
    CameraControl, Clock, FileSystem, HttpTransport, Platform, ProcessRunner, Sandbox,
    SharingService, WorktreeManager,
};
use tokio::sync::Mutex;

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
fn mobile_linux_legacy_status_reports_the_posix_stub_backend() {
    let status =
        super::ios_mobile_linux_status_from_config(Some(&super::IosMobileLinuxConfigFfi {
            mode: super::MobileLinuxRuntimeModeFfi::Legacy,
            managed_root: "/tmp/mobile-linux".to_string(),
            workspace_host_path: "/tmp/workspaces/default".to_string(),
            stable_workspace_id: "default".to_string(),
            abi: "arm64".to_string(),
            rootfs_version: "v1".to_string(),
            archive_sha256: None,
            authorization_file: None,
            app_sandbox_root: "/tmp".to_string(),
        }));

    assert!(matches!(
        status.mode,
        super::MobileLinuxRuntimeModeFfi::Legacy
    ));
    assert_eq!(status.backend, "ios-posix");
}

#[test]
fn mobile_linux_command_api_reports_unavailable_on_host_without_device_bridge() {
    let handle = super::create_ios_mobile_linux_runtime(super::IosMobileLinuxConfigFfi {
        mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
        managed_root: "/tmp/mobile-linux".to_string(),
        workspace_host_path: "/tmp/workspaces/default".to_string(),
        stable_workspace_id: "default".to_string(),
        abi: "arm64".to_string(),
        rootfs_version: "v1".to_string(),
        archive_sha256: None,
        authorization_file: None,
        app_sandbox_root: "/tmp".to_string(),
    })
    .expect("runtime handle");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let err = runtime
        .block_on(handle.run_command(super::MobileLinuxCommandRequestFfi {
            command: "/bin/sh".to_string(),
            args: vec!["-lc".to_string(), "echo hi".to_string()],
            cwd: None,
            env: std::collections::HashMap::new(),
            stdin: None,
            timeout_ms: Some(1000),
            network: super::MobileLinuxNetworkPolicyFfi::Allowed,
            mounts: vec![],
        }))
        .expect_err("command should fail on host without the device bridge");

    assert!(matches!(
        err,
        super::MobileLinuxOperationFfiError::Unavailable { .. }
    ));
}

#[test]
fn mobile_linux_handle_reuses_one_runtime_instance_for_multiple_calls() {
    let handle = super::create_ios_mobile_linux_runtime(super::IosMobileLinuxConfigFfi {
        mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
        managed_root: "/tmp/mobile-linux".to_string(),
        workspace_host_path: "/tmp/workspaces/default".to_string(),
        stable_workspace_id: "default".to_string(),
        abi: "arm64".to_string(),
        rootfs_version: "v1".to_string(),
        archive_sha256: None,
        authorization_file: None,
        app_sandbox_root: "/tmp".to_string(),
    })
    .expect("handle");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let first = rt.block_on(handle.status()).expect("first status");
    let second = rt.block_on(handle.status()).expect("second status");
    assert_eq!(first.last_error, second.last_error);
    assert_eq!(first.backend, second.backend);

    let err = rt
        .block_on(handle.open_pty(super::MobileLinuxPtyOpenRequestFfi {
            command: "/bin/sh".to_string(),
            args: vec![],
            cwd: Some("/workspace/default".to_string()),
            env: std::collections::HashMap::new(),
            cols: 80,
            rows: 24,
            mounts: vec![],
        }))
        .expect_err("pty must be unavailable on host/simulator");

    assert!(matches!(
        err,
        super::MobileLinuxOperationFfiError::Unavailable { .. }
    ));
}

#[test]
fn mobile_linux_workspace_id_rejects_guest_path_components() {
    let config = super::IosMobileLinuxConfigFfi {
        mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
        managed_root: "/tmp/lingxi-app/mobile-linux".to_string(),
        workspace_host_path: "/tmp/lingxi-app/workspaces/project".to_string(),
        stable_workspace_id: "..".to_string(),
        abi: "arm64".to_string(),
        rootfs_version: "v1".to_string(),
        archive_sha256: None,
        authorization_file: None,
        app_sandbox_root: "/tmp/lingxi-app".to_string(),
    };

    assert!(matches!(
        super::validate_mobile_linux_workspace_config("/tmp/lingxi-app", &config),
        Err(super::MobileLinuxOperationFfiError::InvalidRequest { .. })
    ));
}

#[test]
fn mobile_linux_workspace_rejects_parent_traversal_into_protected_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app_root = temp.path().join("app");
    let managed_root = app_root.join("mobile-linux");
    std::fs::create_dir_all(app_root.join(".lingxi")).expect("create protected root");
    std::fs::create_dir_all(&managed_root).expect("create managed root");

    for workspace_host_path in [
        app_root.join("workspaces/default/../.."),
        app_root.join("workspaces/default/../../.lingxi/state"),
        app_root.join("workspaces/default/../../mobile-linux/rootfs"),
        app_root.join("workspaces/default/../../providers/credentials"),
    ] {
        let config = super::IosMobileLinuxConfigFfi {
            mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
            managed_root: managed_root.to_string_lossy().into_owned(),
            workspace_host_path: workspace_host_path.to_string_lossy().into_owned(),
            stable_workspace_id: "default".to_string(),
            abi: "arm64".to_string(),
            rootfs_version: "v1".to_string(),
            archive_sha256: None,
            authorization_file: None,
            app_sandbox_root: app_root.to_string_lossy().into_owned(),
        };

        assert!(matches!(
            super::validate_mobile_linux_workspace_config(
                app_root.to_str().expect("utf8 app root"),
                &config
            ),
            Err(super::MobileLinuxOperationFfiError::InvalidRequest { .. })
        ));
    }
}

#[cfg(unix)]
#[test]
fn mobile_linux_workspace_rejects_symlink_aliases_to_protected_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app_root = temp.path().join("app");
    let managed_root = app_root.join("mobile-linux");
    let providers_root = app_root.join("providers");
    let aliases_root = app_root.join("workspaces");
    std::fs::create_dir_all(app_root.join(".lingxi")).expect("create lingxi root");
    std::fs::create_dir_all(&managed_root).expect("create managed root");
    std::fs::create_dir_all(&providers_root).expect("create providers root");
    std::fs::create_dir_all(&aliases_root).expect("create aliases root");

    for (name, destination) in [
        ("lingxi-link", app_root.join(".lingxi")),
        ("managed-link", managed_root.clone()),
        ("provider-link", providers_root),
    ] {
        let alias = aliases_root.join(name);
        std::os::unix::fs::symlink(destination, &alias).expect("create protected alias");
        let config = super::IosMobileLinuxConfigFfi {
            mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
            managed_root: managed_root.to_string_lossy().into_owned(),
            workspace_host_path: alias.join("workspace").to_string_lossy().into_owned(),
            stable_workspace_id: "default".to_string(),
            abi: "arm64".to_string(),
            rootfs_version: "v1".to_string(),
            archive_sha256: None,
            authorization_file: None,
            app_sandbox_root: app_root.to_string_lossy().into_owned(),
        };

        assert!(matches!(
            super::validate_mobile_linux_workspace_config(
                app_root.to_str().expect("utf8 app root"),
                &config
            ),
            Err(super::MobileLinuxOperationFfiError::InvalidRequest { .. })
        ));
    }
}

#[test]
fn standalone_mobile_linux_runtime_applies_workspace_boundary_validation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app_root = temp.path().join("app");
    let managed_root = app_root.join("mobile-linux");
    std::fs::create_dir_all(app_root.join(".lingxi")).expect("create protected root");
    std::fs::create_dir_all(&managed_root).expect("create managed root");
    let config = super::IosMobileLinuxConfigFfi {
        mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
        managed_root: managed_root.to_string_lossy().into_owned(),
        workspace_host_path: app_root
            .join("workspaces/default/../../.lingxi/state")
            .to_string_lossy()
            .into_owned(),
        stable_workspace_id: "default".to_string(),
        abi: "arm64".to_string(),
        rootfs_version: "v1".to_string(),
        archive_sha256: None,
        authorization_file: None,
        app_sandbox_root: app_root.to_string_lossy().into_owned(),
    };

    assert!(matches!(
        super::create_ios_mobile_linux_runtime(config),
        Err(super::MobileLinuxOperationFfiError::InvalidRequest { .. })
    ));
}

#[test]
fn ios_project_cwd_accepts_managed_project_and_local_app_workspaces() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lingxi-ios-project-{nonce}"));
    let project_id = "12345678-1234-4abc-8def-1234567890ab";
    let workspace = root.join("Projects").join(project_id).join("workspace");
    std::fs::create_dir_all(&workspace).expect("create project fixture");

    let legacy = super::ios_project_cwd(root.to_str().expect("utf8"), None)
        .expect("missing project cwd preserves the legacy sandbox root");
    assert_eq!(legacy, root.canonicalize().expect("canonical root"));

    let resolved = super::ios_project_cwd(
        root.to_str().expect("utf8"),
        Some(workspace.to_str().expect("utf8")),
    )
    .expect("managed project workspace is accepted");
    assert_eq!(
        resolved,
        workspace.canonicalize().expect("canonical workspace")
    );

    // v3 local apps: `apps/<engine-minted id>/workspace` is a first-class
    // conversation scope — the exact cwd the client hands `prepare()` when
    // it jumps into a freshly created app's init session.
    let app_workspace = root.join("apps").join("9b48dfb5").join("workspace");
    std::fs::create_dir_all(&app_workspace).expect("create app fixture");
    let resolved_app = super::ios_project_cwd(
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
        super::ios_project_cwd(
            root.to_str().expect("utf8"),
            Some(illegal_app.to_str().expect("utf8")),
        )
        .is_err(),
        "ids the engine could never mint must not become conversation workspaces"
    );

    let malformed = root.join("Projects").join("user-name").join("workspace");
    std::fs::create_dir_all(&malformed).expect("create malformed fixture");
    assert!(
        super::ios_project_cwd(
            root.to_str().expect("utf8"),
            Some(malformed.to_str().expect("utf8")),
        )
        .is_err(),
        "user-controlled names must never become project directories"
    );

    let outside = std::env::temp_dir().join(format!("lingxi-ios-outside-project-{nonce}"));
    std::fs::create_dir_all(&outside).expect("create outside fixture");
    assert!(
        super::ios_project_cwd(
            root.to_str().expect("utf8"),
            Some(outside.to_str().expect("utf8")),
        )
        .is_err(),
        "workspace must stay under the app sandbox"
    );
}

#[test]
fn ios_launch_config_parses_provider_json_and_project_scope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_id = "12345678-1234-4abc-8def-1234567890ab";
    let workspace = temp
        .path()
        .join("Projects")
        .join(project_id)
        .join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");

    let cfg = super::ios_mobile_config_from_launch_config(&super::IosEngineLaunchConfigFfi {
        api_base: "https://example.invalid".to_string(),
        api_key: "sk-test".to_string(),
        model: "claude-test".to_string(),
        session_mode: harness_runtime::mobile::SessionModeDto::Code,
        vision_delegation_enabled: false,
        app_sandbox_root: temp.path().to_string_lossy().into_owned(),
        project_cwd: Some(workspace.to_string_lossy().into_owned()),
        provider_config: Some(super::IosProviderConfigFfi {
            provider_profiles_json:
                r#"{"openai":{"baseUrl":"https://api.openai.com/v1","wireApi":"responses"}}"#
                    .to_string(),
            routing_json: Some(r#"{"default":"openai"}"#.to_string()),
        }),
        mobile_linux: None,
        local_apps_full_runtime: false,
        local_apps_runtime_root: None,
        physical_memory_bytes: 7 * 1024_u64.pow(3),
        host_environment: None,
    })
    .expect("launch config");

    assert_eq!(
        cfg.cwd,
        workspace.canonicalize().expect("canonical workspace")
    );
    assert_eq!(cfg.api_base, "https://example.invalid");
    assert_eq!(cfg.api_key, "sk-test");
    assert_eq!(cfg.default_model, "claude-test");
    assert!(!cfg.vision_delegation_enabled);
    assert_eq!(cfg.physical_memory_bytes, 7 * 1024_u64.pow(3));
    let host = cfg.host_environment.as_ref().expect("mobile host fallback");
    assert_eq!(host.host_os, platform_api::MobileHostOs::Ios);
    assert_eq!(host.device_class, platform_api::MobileDeviceClass::Unknown);
    assert_eq!(
        host.execution_target,
        platform_api::MobileExecutionTarget::Unknown
    );
    assert_eq!(host.launch_mode, platform_api::MobileLaunchMode::Unknown);
    assert_eq!(
        cfg.lingxi_home,
        temp.path().join(branding::DOT_DIR),
        "global state remains rooted at the sandbox"
    );
    assert!(
        cfg.mobile_shell().is_none(),
        "iOS must not advertise Shell without a configured mobile-linux runtime"
    );
    let providers = cfg.provider_profiles.expect("provider profiles");
    assert!(providers.contains_key("openai"));
    assert_eq!(
        cfg.routing.expect("routing"),
        serde_json::json!({ "default": "openai" })
    );
}

#[test]
fn ios_local_app_runtime_exposes_mobile_linux_shell_carrier() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime_root = temp.path().join("local-app-runtime");
    std::fs::create_dir_all(&runtime_root).expect("runtime root");

    let cfg = super::ios_mobile_config_from_launch_config(&super::IosEngineLaunchConfigFfi {
        api_base: String::new(),
        api_key: String::new(),
        model: String::new(),
        session_mode: harness_runtime::mobile::SessionModeDto::Code,
        vision_delegation_enabled: true,
        app_sandbox_root: temp.path().to_string_lossy().into_owned(),
        project_cwd: None,
        provider_config: None,
        mobile_linux: Some(super::IosMobileLinuxConfigFfi {
            mode: super::MobileLinuxRuntimeModeFfi::Legacy,
            managed_root: temp
                .path()
                .join("mobile-linux")
                .to_string_lossy()
                .into_owned(),
            workspace_host_path: temp.path().join("workspace").to_string_lossy().into_owned(),
            stable_workspace_id: "local-app-test".to_string(),
            abi: "arm64".to_string(),
            rootfs_version: "test".to_string(),
            archive_sha256: None,
            authorization_file: None,
            app_sandbox_root: temp.path().to_string_lossy().into_owned(),
        }),
        local_apps_full_runtime: false,
        local_apps_runtime_root: Some(runtime_root.to_string_lossy().into_owned()),
        physical_memory_bytes: 0,
        host_environment: None,
    })
    .expect("launch config");

    let shell = cfg
        .mobile_shell()
        .expect("local-app runtime should expose the mobile-linux shell carrier");
    assert!(shell.enabled);
    assert_eq!(shell.shell_path, "/bin/sh");
    assert!(shell.force_platform_sandbox);
}

#[test]
fn ios_host_environment_maps_stable_native_facts_without_model_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = super::ios_mobile_config_from_launch_config(&super::IosEngineLaunchConfigFfi {
        api_base: String::new(),
        api_key: String::new(),
        model: String::new(),
        session_mode: harness_runtime::mobile::SessionModeDto::Code,
        vision_delegation_enabled: true,
        app_sandbox_root: temp.path().to_string_lossy().into_owned(),
        project_cwd: None,
        provider_config: None,
        mobile_linux: None,
        local_apps_full_runtime: false,
        local_apps_runtime_root: None,
        physical_memory_bytes: 0,
        host_environment: Some(super::IosHostEnvironmentFfi {
            os_version: "19.0".to_string(),
            device_class: super::IosDeviceClassFfi::Tablet,
            execution_target: super::IosExecutionTargetFfi::Simulator,
            launch_mode: super::IosLaunchModeFfi::ScheduledHeadless,
        }),
    })
    .expect("launch config");

    let environment = cfg.host_environment.expect("host environment");
    assert_eq!(environment.host_os, platform_api::MobileHostOs::Ios);
    assert_eq!(environment.host_os_version.as_deref(), Some("19.0"));
    assert_eq!(
        environment.device_class,
        platform_api::MobileDeviceClass::Tablet
    );
    assert_eq!(
        environment.execution_target,
        platform_api::MobileExecutionTarget::Simulator
    );
    assert_eq!(
        environment.launch_mode,
        platform_api::MobileLaunchMode::ScheduledHeadless
    );
}

#[tokio::test]
async fn build_ios_cron_store_round_trips_without_engine() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_id = "12345678-1234-4abc-8def-1234567890ab";
    let workspace = temp
        .path()
        .join("Projects")
        .join(project_id)
        .join("workspace");
    std::fs::create_dir_all(temp.path().join(branding::DOT_DIR)).expect("state dir");
    std::fs::create_dir_all(&workspace).expect("workspace");

    let store = super::build_ios_cron_store(
        temp.path().to_string_lossy().into_owned(),
        Some(workspace.to_string_lossy().into_owned()),
    )
    .expect("build cron store");

    let created = store
        .create("* * * * *".to_string(), "hello".to_string(), false)
        .await
        .expect("one-shot creation");
    let updated = store
        .update(
            created.id.clone(),
            "*/15 * * * *".to_string(),
            "updated".to_string(),
            true,
        )
        .await
        .expect("recurring update");
    let due = store
        .due_occurrences(updated.next_fire_ms.expect("next fire").saturating_add(1))
        .await;
    assert_eq!(vec![created.id.clone()], vec![due[0].task_id.clone()]);
    assert_eq!(
        Some(updated.next_fire_ms.expect("next fire")),
        store.next_fire_time().await
    );
    assert!(store.delete(created.id).await);
    assert!(store.list().await.is_empty());
}
