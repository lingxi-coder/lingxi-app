//! iOS iSH-backed implementation of the shared mobile-linux runtime seam.
//!
//! The native integration is intentionally isolated to `native` so ABI drift is
//! contained to one module. Host and simulator builds compile a safe
//! unavailable stub rather than trying to link the device-only symbols.

use async_trait::async_trait;
use platform_api::mobile_linux::LinuxEnforcementReceipt;
use platform_api::{
    LinuxCommandRequest, LinuxCommandResult, LinuxProcessHandle, MobileLinuxCapability,
    MobileLinuxError, MobileLinuxEvent, MobileLinuxEventKind, MobileLinuxRuntime,
    MobileLinuxRuntimeMode, MobileLinuxTaskSnapshot, MobileLinuxTaskStatus, MountPurpose,
    MountSpec, ProcessStreamSink, PtyOpenRequest, PtySessionHandle, PtySize, RawStdioOpenRequest,
    RawStdioReadResult, RawStdioSessionHandle, RootfsState, RootfsStatus, SandboxBackend,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::spawn_blocking;

const MAX_EVENTS: usize = 4096;
const MAX_STREAM_CAPTURE_BYTES: usize = 256 * 1024;
const PTY_IDLE_POLL: Duration = Duration::from_millis(25);
const BACKGROUND_IDLE_POLL: Duration = Duration::from_millis(25);
const BACKGROUND_REAP_BUDGET: Duration = Duration::from_secs(3);
const LINGXI_DOT_DIR: &str = ".lingxi";
// Canonical value lives in the guest-path atlas; the local name is kept so
// the three mount-contract call sites read unchanged.
const LOCAL_APP_BUILD_GUEST_ROOT: &str =
    platform_api::mobile_linux::guest_paths::LOCAL_APP_BUILD_ROOT;

#[derive(Clone, Copy)]
enum ForegroundMountMode {
    Merged,
    RequestOnly,
}

/// Immutable iSH runtime identity and path configuration supplied by the iOS
/// framework bridge.
#[derive(Debug, Clone)]
pub struct IosIshRuntimeConfig {
    /// App-private directory that holds active/staged iSH rootfs state.
    pub managed_root: PathBuf,
    /// Canonical app sandbox root used to protect `.lingxi` and config trees.
    pub app_sandbox_root: PathBuf,
    /// Host workspace path exposed as the default `/workspace/<id>` mount.
    pub workspace_host_path: PathBuf,
    /// Stable guest workspace identifier appended under `/workspace`.
    pub stable_workspace_id: String,
    /// Guest ABI label surfaced in status responses.
    pub abi: String,
    /// Expected rootfs version label.
    pub rootfs_version: String,
    /// Optional archive digest surfaced in status responses.
    pub archive_sha256: Option<String>,
    /// Optional authorization file forwarded to the native bridge.
    pub authorization_file: Option<String>,
}

impl IosIshRuntimeConfig {
    fn active_root(&self) -> PathBuf {
        self.managed_root.join("alpine-rootfs")
    }

    fn lingxi_root(&self) -> PathBuf {
        self.app_sandbox_root.join(LINGXI_DOT_DIR)
    }

    fn workspace_guest_path(&self) -> String {
        platform_api::mobile_linux::guest_paths::workspace(&self.stable_workspace_id)
    }

    fn persistent_home_host_path(&self) -> PathBuf {
        self.managed_root.join("persistent/root")
    }

    fn default_workspace_mount(&self) -> MountSpec {
        MountSpec {
            host_path: self.workspace_host_path.clone(),
            guest_path: self.workspace_guest_path(),
            read_only: false,
            purpose: MountPurpose::Workspace,
        }
    }

    fn persistent_home_mount(&self) -> MountSpec {
        MountSpec {
            host_path: self.persistent_home_host_path(),
            guest_path: platform_api::mobile_linux::guest_paths::HOME.to_string(),
            read_only: false,
            purpose: MountPurpose::Shared,
        }
    }

    fn native_payload(&self) -> NativeConfigPayload {
        NativeConfigPayload {
            managed_root: self.managed_root.display().to_string(),
            workspace_host_path: self.workspace_host_path.display().to_string(),
            stable_workspace_id: self.stable_workspace_id.clone(),
            abi: self.abi.clone(),
            rootfs_version: self.rootfs_version.clone(),
            archive_sha256: self.archive_sha256.clone(),
            authorization_file: self.authorization_file.clone(),
        }
    }
}

#[derive(Debug)]
struct TaskControl {
    snapshot: Mutex<MobileLinuxTaskSnapshot>,
    native_handle: Mutex<Option<String>>,
    cancel_requested: AtomicBool,
    terminal_emitted: AtomicBool,
}

impl TaskControl {
    fn new(task_id: String, command: String, status: MobileLinuxTaskStatus) -> Self {
        Self {
            snapshot: Mutex::new(MobileLinuxTaskSnapshot {
                task_id,
                status,
                command,
                started_at_ms: Some(now_ms()),
                finished_at_ms: None,
                exit_code: None,
                detail: None,
            }),
            native_handle: Mutex::new(None),
            cancel_requested: AtomicBool::new(false),
            terminal_emitted: AtomicBool::new(false),
        }
    }
}

#[derive(Debug)]
struct PtyControl {
    task: Arc<TaskControl>,
    open: AtomicBool,
    close_emitted: AtomicBool,
}

#[derive(Debug)]
struct RuntimeState {
    config: IosIshRuntimeConfig,
    mounts: RwLock<Vec<MountSpec>>,
    tasks: Mutex<HashMap<String, Arc<TaskControl>>>,
    events: Mutex<VecDeque<MobileLinuxEvent>>,
    next_id: AtomicU64,
    next_sequence: AtomicU64,
    booted: AtomicBool,
    pty: Mutex<Option<(String, Arc<PtyControl>)>>,
    raw_stdio: Mutex<HashMap<String, RawStdioSessionHandle>>,
    native_lock: Arc<Mutex<()>>,
}

/// Concrete mobile-linux runtime backed by the native iSH bridge on device and
/// by a safe unavailable stub elsewhere.
#[derive(Clone, Debug)]
pub struct IosIshRuntime {
    state: Arc<RuntimeState>,
}

impl IosIshRuntime {
    /// Construct a new runtime without mutating filesystem or native state.
    #[must_use]
    pub fn new(config: IosIshRuntimeConfig) -> Self {
        let workspace_mount = config.default_workspace_mount();
        let persistent_home_mount = config.persistent_home_mount();
        Self {
            state: Arc::new(RuntimeState {
                config,
                mounts: RwLock::new(vec![workspace_mount, persistent_home_mount]),
                tasks: Mutex::new(HashMap::new()),
                events: Mutex::new(VecDeque::new()),
                next_id: AtomicU64::new(1),
                next_sequence: AtomicU64::new(1),
                booted: AtomicBool::new(false),
                pty: Mutex::new(None),
                raw_stdio: Mutex::new(HashMap::new()),
                native_lock: Arc::new(Mutex::new(())),
            }),
        }
    }

    fn next_id(&self, prefix: &str) -> String {
        format!(
            "{prefix}-{}",
            self.state.next_id.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn emit(&self, task_id: Option<String>, kind: MobileLinuxEventKind) {
        let mut events = self.state.events.lock().expect("ios-ish events mutex");
        events.push_back(MobileLinuxEvent {
            sequence: self.state.next_sequence.fetch_add(1, Ordering::Relaxed),
            task_id,
            kind,
        });
        while events.len() > MAX_EVENTS {
            events.pop_front();
        }
    }

    fn emit_runtime_error(&self, task_id: Option<String>, detail: impl Into<String>) {
        self.emit(
            task_id,
            MobileLinuxEventKind::RuntimeError {
                detail: detail.into(),
            },
        );
    }

    fn create_task(
        &self,
        prefix: &str,
        command: String,
        status: MobileLinuxTaskStatus,
    ) -> (String, Arc<TaskControl>) {
        let id = self.next_id(prefix);
        let task = Arc::new(TaskControl::new(id.clone(), command, status));
        self.state
            .tasks
            .lock()
            .expect("ios-ish tasks mutex")
            .insert(id.clone(), task.clone());
        self.emit(
            Some(id.clone()),
            MobileLinuxEventKind::TaskStatusChanged {
                status,
                exit_code: None,
                detail: None,
            },
        );
        (id, task)
    }

    fn create_task_with_id(
        &self,
        id: String,
        command: String,
        status: MobileLinuxTaskStatus,
    ) -> Arc<TaskControl> {
        let task = Arc::new(TaskControl::new(id.clone(), command, status));
        self.state
            .tasks
            .lock()
            .expect("ios-ish tasks mutex")
            .insert(id.clone(), task.clone());
        self.emit(
            Some(id),
            MobileLinuxEventKind::TaskStatusChanged {
                status,
                exit_code: None,
                detail: None,
            },
        );
        task
    }

    fn finish_task(
        &self,
        id: &str,
        task: &TaskControl,
        status: MobileLinuxTaskStatus,
        exit_code: Option<i32>,
        detail: Option<String>,
    ) {
        if task.terminal_emitted.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut snapshot = task.snapshot.lock().expect("ios-ish task snapshot mutex");
        snapshot.status = status;
        snapshot.finished_at_ms = Some(now_ms());
        snapshot.exit_code = exit_code;
        snapshot.detail.clone_from(&detail);
        drop(snapshot);
        self.emit(
            Some(id.to_string()),
            MobileLinuxEventKind::TaskStatusChanged {
                status,
                exit_code,
                detail,
            },
        );
    }

    fn native_unavailable_reason(&self) -> Option<String> {
        if native::is_available() {
            return None;
        }
        let native_lock = self.state.native_lock.lock().expect("ios-ish native lock");
        let response = native::availability_json();
        drop(native_lock);
        match response {
            Ok(json) => parse_availability_reason(&json)
                .or_else(|| Some("native iSH runtime reports unavailable".to_string())),
            Err(error) => Some(error),
        }
    }

    fn ensure_native_available(&self) -> Result<(), MobileLinuxError> {
        match self.native_unavailable_reason() {
            Some(reason) => Err(MobileLinuxError::Unavailable(reason)),
            None => Ok(()),
        }
    }

    fn rootfs_snapshot_with_error(
        &self,
        override_state: Option<RootfsState>,
        last_error: Option<String>,
    ) -> RootfsStatus {
        let active_root = self.state.config.active_root();
        let native_unavailable = self.native_unavailable_reason();
        let state = if let Some(reason) = native_unavailable.clone() {
            if override_state.is_some() {
                override_state.unwrap_or(RootfsState::Unsupported)
            } else {
                let _ = reason;
                RootfsState::Unsupported
            }
        } else if let Some(state) = override_state {
            state
        } else {
            filesystem_rootfs_state(&active_root, &self.state.config.managed_root)
        };
        RootfsStatus {
            state,
            backend: SandboxBackend::IosIsh,
            mode: MobileLinuxRuntimeMode::MobileLinux,
            platform: "ios".to_string(),
            abi: self.state.config.abi.clone(),
            version: Some(self.state.config.rootfs_version.clone()),
            managed_root: Some(self.state.config.managed_root.clone()),
            active_root: path_present(&active_root).then_some(active_root.clone()),
            staged_root: None,
            archive_sha256: self.state.config.archive_sha256.clone(),
            installed_size_bytes: directory_size(&active_root).ok(),
            writable_guest_paths: vec![
                platform_api::mobile_linux::guest_paths::HOME.to_string(),
                platform_api::mobile_linux::guest_paths::SCRATCH[0].to_string(),
                platform_api::mobile_linux::guest_paths::SCRATCH[1].to_string(),
                self.state.config.workspace_guest_path(),
                LOCAL_APP_BUILD_GUEST_ROOT.to_string(),
            ],
            last_error: last_error.or(native_unavailable),
        }
    }

    fn rootfs_snapshot(&self) -> RootfsStatus {
        self.rootfs_snapshot_with_error(None, None)
    }

    fn native_config_json(&self) -> Result<String, MobileLinuxError> {
        serde_json::to_string(&self.state.config.native_payload())
            .map_err(|error| MobileLinuxError::Io(format!("serialize native config: {error}")))
    }

    async fn install_rootfs(&self, reset: bool) -> Result<(), MobileLinuxError> {
        self.ensure_native_available()?;
        fs::create_dir_all(&self.state.config.managed_root).map_err(|error| {
            MobileLinuxError::Io(format!(
                "create managed root {}: {error}",
                self.state.config.managed_root.display()
            ))
        })?;
        let config_json = self.native_config_json()?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            if reset {
                native::reset_rootfs_json(&config_json)
            } else {
                native::install_rootfs_json(&config_json)
            }
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join install_rootfs: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        let _ = parse_native_ok(&response)?;
        Ok(())
    }

    async fn native_boot(&self) -> Result<(), MobileLinuxError> {
        self.ensure_native_available()?;
        let config_json = self.native_config_json()?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::boot_json(&config_json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join boot: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        let _ = parse_native_ok(&response)?;
        Ok(())
    }

    async fn apply_mounts(&self, mounts: &[MountSpec]) -> Result<(), MobileLinuxError> {
        self.ensure_native_available()?;
        let config_json = self.native_config_json()?;
        let payload = MountConfigPayload::from_mounts(mounts);
        let json = serde_json::to_string(&payload)
            .map_err(|error| MobileLinuxError::Io(format!("serialize mounts json: {error}")))?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::configure_mounts_json(&config_json, &json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join configure_mounts_json: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        let _ = parse_native_ok(&response)?;
        Ok(())
    }

    async fn run_native_request(
        &self,
        request: &LinuxCommandRequest,
        mounts: &[MountSpec],
        include_default_mounts: bool,
    ) -> Result<LinuxCommandResult, MobileLinuxError> {
        let config_json = self.native_config_json()?;
        let payload = RunRequestPayload::from_request(request, mounts, include_default_mounts);
        let json = serde_json::to_string(&payload)
            .map_err(|error| MobileLinuxError::Io(format!("serialize run request: {error}")))?;
        let native_lock = self.state.native_lock.clone();
        let response_json = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::run_sync_json(&config_json, &json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join run_json: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        parse_run_response(&response_json)
    }

    async fn native_spawn_background(
        &self,
        request: &LinuxCommandRequest,
        mounts: &[MountSpec],
    ) -> Result<NativeProcessStart, MobileLinuxError> {
        let config_json = self.native_config_json()?;
        let payload = RunRequestPayload::from_request(request, mounts, true);
        let request_json = serde_json::to_string(&payload).map_err(|error| {
            MobileLinuxError::Io(format!("serialize background request: {error}"))
        })?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::spawn_background_json(&config_json, &request_json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join spawn_background: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        parse_process_id_response(&response)
    }

    async fn native_kill_background(&self, process_id: &str) -> Result<bool, MobileLinuxError> {
        let config_json = self.native_config_json()?;
        let request_json = serde_json::to_string(&BackgroundProcessPayload {
            process_id: process_id.to_string(),
        })
        .map_err(|error| MobileLinuxError::Io(format!("serialize background kill: {error}")))?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::kill_background_json(&config_json, &request_json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join kill_background: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        parse_background_kill_response(&response)
    }

    async fn native_poll_background(
        &self,
        process_id: &str,
        after_sequence: Option<u64>,
        limit: u32,
    ) -> Result<Vec<NativeBackgroundEventPayload>, MobileLinuxError> {
        let config_json = self.native_config_json()?;
        let request_json = serde_json::to_string(&BackgroundPollPayload {
            process_id: process_id.to_string(),
            after_sequence,
            limit: Some(limit),
        })
        .map_err(|error| MobileLinuxError::Io(format!("serialize background poll: {error}")))?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::poll_background_json(&config_json, &request_json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join poll_background: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        parse_background_events(&response)
    }

    fn spawn_background_reader(
        &self,
        task_id: String,
        native_process_id: String,
        task: Arc<TaskControl>,
    ) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut last_sequence = None;
            loop {
                match runtime
                    .native_poll_background(
                        &native_process_id,
                        last_sequence,
                        platform_api::mobile_linux::MAX_MOBILE_LINUX_EVENT_BATCH as u32,
                    )
                    .await
                {
                    Ok(events) if events.is_empty() => {
                        if task.terminal_emitted.load(Ordering::Acquire) {
                            break;
                        }
                        tokio::time::sleep(BACKGROUND_IDLE_POLL).await;
                    }
                    Ok(events) => {
                        let mut terminal = false;
                        for event in events {
                            last_sequence = Some(
                                last_sequence
                                    .map_or(event.sequence, |current| current.max(event.sequence)),
                            );
                            match event.kind.as_str() {
                                "stdout_line" => runtime.emit(
                                    Some(task_id.clone()),
                                    MobileLinuxEventKind::StdoutLine {
                                        line: event.line.unwrap_or_default(),
                                    },
                                ),
                                "stderr_chunk" => match event.data_base64.as_deref() {
                                    Some(encoded) => match decode_base64(encoded) {
                                        Ok(chunk) => runtime.emit(
                                            Some(task_id.clone()),
                                            MobileLinuxEventKind::StderrChunk { chunk },
                                        ),
                                        Err(error) => runtime.emit_runtime_error(
                                            Some(task_id.clone()),
                                            error.to_string(),
                                        ),
                                    },
                                    None => runtime.emit_runtime_error(
                                        Some(task_id.clone()),
                                        "native stderr event omitted data_base64",
                                    ),
                                },
                                "process_exited" => {
                                    let (status, exit_code, detail) = background_terminal_state(
                                        task.cancel_requested.load(Ordering::Acquire),
                                        &event,
                                    );
                                    runtime.finish_task(&task_id, &task, status, exit_code, detail);
                                    terminal = true;
                                }
                                other => runtime.emit_runtime_error(
                                    Some(task_id.clone()),
                                    format!("unknown native background event kind: {other}"),
                                ),
                            }
                        }
                        if terminal {
                            break;
                        }
                    }
                    Err(error) => {
                        runtime.emit_runtime_error(Some(task_id.clone()), error.to_string());
                        runtime.finish_task(
                            &task_id,
                            &task,
                            MobileLinuxTaskStatus::Failed,
                            None,
                            Some(error.to_string()),
                        );
                        break;
                    }
                }
            }
        });
    }

    /// Probe whether an app-owned loopback server is accepting TCP connections.
    ///
    /// This is deliberately exposed on the concrete iSH runtime rather than the
    /// shared trait because only the iOS native bridge needs host-side probing.
    pub async fn probe_loopback_port(
        &self,
        port: u16,
        timeout: Duration,
    ) -> Result<bool, MobileLinuxError> {
        self.ensure_native_available()?;
        if port == 0 {
            return Err(MobileLinuxError::InvalidRequest(
                "loopback port must be greater than zero".to_string(),
            ));
        }
        let config_json = self.native_config_json()?;
        let request_json = serde_json::to_string(&LoopbackProbePayload {
            port,
            timeout_ms: timeout.as_millis().clamp(1, u128::from(u32::MAX)) as u32,
        })
        .map_err(|error| MobileLinuxError::Io(format!("serialize loopback probe: {error}")))?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::probe_loopback_json(&config_json, &request_json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join loopback probe: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        parse_loopback_probe(&response)
    }

    async fn run_inner(
        &self,
        request: LinuxCommandRequest,
        lifecycle_status: MobileLinuxTaskStatus,
        mount_mode: ForegroundMountMode,
    ) -> Result<(LinuxCommandResult, String, Arc<TaskControl>), MobileLinuxError> {
        validate_request(&request)?;
        self.boot().await?;
        let mounts = match mount_mode {
            ForegroundMountMode::Merged => self.merged_mounts(&request.mounts)?,
            ForegroundMountMode::RequestOnly => self.isolated_mounts(&request.mounts)?,
        };
        if matches!(mount_mode, ForegroundMountMode::Merged) {
            self.apply_mounts(&mounts).await?;
        }
        let (task_id, task) = self.create_task(
            if matches!(lifecycle_status, MobileLinuxTaskStatus::Backgrounded) {
                "bg"
            } else {
                "task"
            },
            display_command(&request.command, &request.args),
            lifecycle_status,
        );
        let native_result = self
            .run_native_request(
                &request,
                &mounts,
                matches!(mount_mode, ForegroundMountMode::Merged),
            )
            .await;
        match native_result {
            Ok(result) => {
                self.emit_result_frames(&task_id, &result);
                Ok((result, task_id, task))
            }
            Err(error) => {
                let detail = error.to_string();
                self.emit_runtime_error(Some(task_id.clone()), detail.clone());
                self.finish_task(
                    &task_id,
                    &task,
                    MobileLinuxTaskStatus::Failed,
                    None,
                    Some(detail),
                );
                Err(error)
            }
        }
    }

    fn emit_result_frames(&self, task_id: &str, result: &LinuxCommandResult) {
        for line in result.stdout.lines() {
            self.emit(
                Some(task_id.to_string()),
                MobileLinuxEventKind::StdoutLine {
                    line: line.to_string(),
                },
            );
        }
        if !result.stderr.is_empty() {
            self.emit(
                Some(task_id.to_string()),
                MobileLinuxEventKind::StderrChunk {
                    chunk: result.stderr.as_bytes().to_vec(),
                },
            );
        }
    }

    fn merged_mounts(
        &self,
        request_mounts: &[MountSpec],
    ) -> Result<Vec<MountSpec>, MobileLinuxError> {
        let mut mounts = self
            .state
            .mounts
            .read()
            .expect("ios-ish mounts rwlock")
            .clone();
        for mount in request_mounts {
            let normalized = validate_mount(mount, &self.state.config)?;
            if let Some(existing) = mounts
                .iter_mut()
                .find(|existing| existing.guest_path == normalized.guest_path)
            {
                *existing = normalized;
            } else {
                mounts.push(normalized);
            }
        }
        Ok(mounts)
    }

    fn isolated_mounts(
        &self,
        request_mounts: &[MountSpec],
    ) -> Result<Vec<MountSpec>, MobileLinuxError> {
        let build_count = request_mounts
            .iter()
            .filter(|mount| matches!(mount.purpose, MountPurpose::LocalAppBuild))
            .count();
        let store_count = request_mounts
            .iter()
            .filter(|mount| {
                matches!(mount.purpose, MountPurpose::Shared)
                    && mount.guest_path
                        == platform_api::mobile_linux::guest_paths::LOCAL_APP_DEPENDENCY_STORE
            })
            .count();
        if build_count != 1 || store_count > 1 || request_mounts.len() != 1 + store_count {
            return Err(MobileLinuxError::InvalidRequest(
                "isolated local-app execution requires exactly one LocalAppBuild mount and at most one validated dependency store mount".to_string(),
            ));
        }
        let mut mounts: Vec<MountSpec> = Vec::with_capacity(request_mounts.len());
        for mount in request_mounts {
            let normalized = validate_mount(mount, &self.state.config)?;
            mounts.retain(|existing| existing.guest_path != normalized.guest_path);
            mounts.push(normalized);
        }
        Ok(mounts)
    }

    async fn native_open_pty(
        &self,
        request: &PtyOpenRequest,
        mounts: &[MountSpec],
    ) -> Result<String, MobileLinuxError> {
        let config_json = self.native_config_json()?;
        let payload = PtyOpenPayload::from_request(request, mounts);
        let json = serde_json::to_string(&payload)
            .map_err(|error| MobileLinuxError::Io(format!("serialize PTY request: {error}")))?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::pty_open_json(&config_json, &json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join open_pty_json: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        parse_session_id_response(&response)
    }

    async fn native_open_raw_stdio(
        &self,
        request: &RawStdioOpenRequest,
        mounts: &[MountSpec],
    ) -> Result<(String, LinuxEnforcementReceipt), MobileLinuxError> {
        let config_json = self.native_config_json()?;
        let request_json = serde_json::to_string(&RawStdioOpenPayload::from_request(
            request, mounts,
        ))
        .map_err(|error| MobileLinuxError::Io(format!("serialize raw stdio open: {error}")))?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::raw_stdio_open_json(&config_json, &request_json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join raw stdio open: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        parse_raw_stdio_open_response(&response)
    }

    async fn native_raw_stdio_call(
        &self,
        operation: RawStdioNativeOperation,
        payload: RawStdioRequestPayload,
    ) -> Result<String, MobileLinuxError> {
        let config_json = self.native_config_json()?;
        let request_json = serde_json::to_string(&payload).map_err(|error| {
            MobileLinuxError::Io(format!("serialize raw stdio request: {error}"))
        })?;
        let native_lock = self.state.native_lock.clone();
        spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            match operation {
                RawStdioNativeOperation::Write => {
                    native::raw_stdio_write_json(&config_json, &request_json)
                }
                RawStdioNativeOperation::Read => {
                    native::raw_stdio_read_json(&config_json, &request_json)
                }
                RawStdioNativeOperation::Close => {
                    native::raw_stdio_close_json(&config_json, &request_json)
                }
                RawStdioNativeOperation::Dispose => {
                    native::raw_stdio_dispose_json(&config_json, &request_json)
                }
            }
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join raw stdio request: {error}")))?
        .map_err(MobileLinuxError::Io)
    }

    async fn native_write_pty(
        &self,
        session_id: &str,
        data: Vec<u8>,
    ) -> Result<(), MobileLinuxError> {
        let config_json = self.native_config_json()?;
        let request_json = serde_json::to_string(&PtyWritePayload {
            session_id: session_id.to_string(),
            data_base64: encode_base64(&data),
        })
        .map_err(|error| MobileLinuxError::Io(format!("serialize PTY write request: {error}")))?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::pty_write_json(&config_json, &request_json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join write_pty: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        let _ = parse_native_ok(&response)?;
        Ok(())
    }

    async fn native_resize_pty(
        &self,
        session_id: &str,
        size: PtySize,
    ) -> Result<(), MobileLinuxError> {
        let config_json = self.native_config_json()?;
        let request_json = serde_json::to_string(&PtyResizePayload {
            session_id: session_id.to_string(),
            cols: size.cols,
            rows: size.rows,
        })
        .map_err(|error| MobileLinuxError::Io(format!("serialize PTY resize request: {error}")))?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::pty_resize_json(&config_json, &request_json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join resize_pty: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        let _ = parse_native_ok(&response)?;
        Ok(())
    }

    async fn native_close_pty(&self, session_id: &str) -> Result<(), MobileLinuxError> {
        let config_json = self.native_config_json()?;
        let request_json = serde_json::to_string(&PtyClosePayload {
            session_id: session_id.to_string(),
        })
        .map_err(|error| MobileLinuxError::Io(format!("serialize PTY close request: {error}")))?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::pty_close_json(&config_json, &request_json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join close_pty: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        let _ = parse_native_ok(&response)?;
        Ok(())
    }

    async fn native_poll_pty(
        &self,
        after_sequence: Option<u64>,
        limit: u32,
    ) -> Result<Vec<NativePtyEventPayload>, MobileLinuxError> {
        let config_json = self.native_config_json()?;
        let request_json = serde_json::to_string(&PtyPollPayload {
            after_sequence,
            limit: Some(limit),
        })
        .map_err(|error| MobileLinuxError::Io(format!("serialize PTY poll request: {error}")))?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::poll_output_json(&config_json, &request_json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join poll_output_json: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        parse_poll_events(&response)
    }

    fn emit_pty_closed_once(
        &self,
        session_id: &str,
        control: &PtyControl,
        exit_code: Option<i32>,
        detail: Option<String>,
    ) {
        if control.close_emitted.swap(true, Ordering::AcqRel) {
            return;
        }
        self.emit(
            Some(session_id.to_string()),
            MobileLinuxEventKind::PtyClosed {
                session_id: session_id.to_string(),
                exit_code,
                detail,
            },
        );
    }

    fn current_pty(&self, session_id: &str) -> Result<Arc<PtyControl>, MobileLinuxError> {
        let guard = self.state.pty.lock().expect("ios-ish pty mutex");
        let (id, control) = guard.as_ref().ok_or_else(|| {
            MobileLinuxError::InvalidRequest("no PTY session is open".to_string())
        })?;
        if id != session_id {
            return Err(MobileLinuxError::InvalidRequest(
                "unknown PTY session handle".to_string(),
            ));
        }
        Ok(control.clone())
    }

    fn clear_current_pty(&self, session_id: &str) {
        let mut guard = self.state.pty.lock().expect("ios-ish pty mutex");
        if guard.as_ref().is_some_and(|(id, _)| id == session_id) {
            *guard = None;
        }
    }

    fn spawn_pty_reader(&self, session_id: String, control: Arc<PtyControl>) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut after_sequence = None;
            loop {
                if !control.open.load(Ordering::Acquire) {
                    break;
                }
                match runtime
                    .native_poll_pty(
                        after_sequence,
                        platform_api::mobile_linux::MAX_MOBILE_LINUX_EVENT_BATCH as u32,
                    )
                    .await
                {
                    Ok(events) if events.is_empty() => {
                        tokio::time::sleep(PTY_IDLE_POLL).await;
                    }
                    Ok(events) => {
                        for event in events {
                            after_sequence = Some(event.sequence);
                            if event.session_id != session_id {
                                // Skip silently. A straggler event from a
                                // previous session (the journal outlives a
                                // shell; reopen is a supported flow now) is
                                // not a fault of THIS session — but this used
                                // to emit a RuntimeError tagged with the NEW
                                // session id, which the terminal maps
                                // straight to a failed state: one stale event
                                // killed a healthy restarted shell.
                                continue;
                            }
                            match event.kind.as_str() {
                                "pty_output" => {
                                    let data = event
                                        .data_base64
                                        .as_deref()
                                        .map(decode_base64)
                                        .transpose()
                                        .unwrap_or_default()
                                        .unwrap_or_default();
                                    runtime.emit(
                                        Some(session_id.clone()),
                                        MobileLinuxEventKind::PtyOutput {
                                            session_id: session_id.clone(),
                                            data,
                                        },
                                    );
                                }
                                "pty_closed" => {
                                    control.open.store(false, Ordering::Release);
                                    // A shell SELF-exit (user typed `exit`, or the
                                    // guest process died) rides in with the real
                                    // wait-status-decoded code from the bridge's
                                    // ISHProcessExited observer; a bridge-initiated
                                    // close carries none and stays code 0.
                                    let exit_code = Some(event.exit_code.unwrap_or(0));
                                    runtime.finish_task(
                                        &session_id,
                                        &control.task,
                                        MobileLinuxTaskStatus::Completed,
                                        exit_code,
                                        event.detail.clone(),
                                    );
                                    runtime.emit_pty_closed_once(
                                        &session_id,
                                        &control,
                                        exit_code,
                                        event.detail,
                                    );
                                    runtime.clear_current_pty(&session_id);
                                    return;
                                }
                                other => runtime.emit_runtime_error(
                                    Some(session_id.clone()),
                                    format!("unknown PTY event kind: {other}"),
                                ),
                            }
                        }
                    }
                    Err(error) => {
                        control.open.store(false, Ordering::Release);
                        runtime.emit_runtime_error(Some(session_id.clone()), error.to_string());
                        runtime.finish_task(
                            &session_id,
                            &control.task,
                            MobileLinuxTaskStatus::Failed,
                            None,
                            Some(error.to_string()),
                        );
                        // Release the NATIVE side before announcing the death.
                        // The Swift bridge frees its one-interactive-PTY slot
                        // only through a close call; without this, the exit
                        // event below reaches the terminal, the user taps
                        // 重新启动 shell, and every reopen is refused with
                        // "only one interactive PTY session is supported per
                        // managed root" until the app is relaunched. Closing
                        // before the emit also keeps a prompt restart from
                        // racing this task for the slot. Best-effort: if the
                        // pipeline is so broken that close itself fails, the
                        // Swift bridge now releases the slot regardless.
                        let _ = runtime.native_close_pty(&session_id).await;
                        runtime.emit_pty_closed_once(
                            &session_id,
                            &control,
                            None,
                            Some(error.to_string()),
                        );
                        runtime.clear_current_pty(&session_id);
                        break;
                    }
                }
            }
        });
    }
}

#[async_trait]
impl MobileLinuxRuntime for IosIshRuntime {
    fn backend(&self) -> SandboxBackend {
        SandboxBackend::IosIsh
    }

    fn mode(&self) -> MobileLinuxRuntimeMode {
        MobileLinuxRuntimeMode::MobileLinux
    }

    async fn probe_capability(&self) -> MobileLinuxCapability {
        let reason = self.native_unavailable_reason();
        MobileLinuxCapability {
            available: reason.is_none(),
            backend: SandboxBackend::IosIsh,
            mode: MobileLinuxRuntimeMode::MobileLinux,
            reason,
            streaming_output: true,
            background_processes: true,
            pty: true,
            bind_mounts: true,
            rootfs_integrity: true,
        }
    }

    async fn boot(&self) -> Result<RootfsStatus, MobileLinuxError> {
        self.ensure_native_available()?;
        fs::create_dir_all(&self.state.config.managed_root).map_err(|error| {
            MobileLinuxError::Io(format!(
                "create managed root {}: {error}",
                self.state.config.managed_root.display()
            ))
        })?;
        let active_root = self.state.config.active_root();
        let current_state = filesystem_rootfs_state(&active_root, &self.state.config.managed_root);
        if matches!(current_state, RootfsState::Missing) {
            self.install_rootfs(false).await?;
        } else if matches!(current_state, RootfsState::Corrupt) {
            return Err(MobileLinuxError::Integrity(
                "active iSH rootfs is corrupt; run repair_rootfs".to_string(),
            ));
        }
        self.native_boot().await?;
        self.state.booted.store(true, Ordering::Release);
        Ok(self.rootfs_snapshot())
    }

    async fn shutdown(&self) -> Result<(), MobileLinuxError> {
        let current_pty = self.state.pty.lock().expect("ios-ish pty mutex").clone();
        if let Some((session_id, _)) = current_pty {
            let _ = self.close_pty(&PtySessionHandle { id: session_id }).await;
        }
        let raw_sessions: Vec<_> = self
            .state
            .raw_stdio
            .lock()
            .expect("ios-ish raw stdio mutex")
            .values()
            .cloned()
            .collect();
        for session in raw_sessions {
            let _ = self.close_raw_stdio(&session).await;
        }
        let task_ids: Vec<_> = self
            .state
            .tasks
            .lock()
            .expect("ios-ish tasks mutex")
            .iter()
            .filter(|(_, task)| !task.terminal_emitted.load(Ordering::Acquire))
            .map(|(id, _)| id.clone())
            .collect();
        for id in task_ids {
            let _ = self
                .kill(&LinuxProcessHandle {
                    id,
                    enforcement: LinuxEnforcementReceipt::default(),
                })
                .await;
        }
        self.state.booted.store(false, Ordering::Release);
        Ok(())
    }

    async fn run(
        &self,
        request: LinuxCommandRequest,
    ) -> Result<LinuxCommandResult, MobileLinuxError> {
        let (result, task_id, task) = self
            .run_inner(
                request,
                MobileLinuxTaskStatus::Running,
                ForegroundMountMode::Merged,
            )
            .await?;
        let cancelled = task.cancel_requested.load(Ordering::Acquire);
        let status = if cancelled {
            MobileLinuxTaskStatus::Cancelled
        } else if result.timed_out {
            MobileLinuxTaskStatus::TimedOut
        } else if result.cancelled {
            MobileLinuxTaskStatus::Cancelled
        } else if result.exit_code == 0 {
            MobileLinuxTaskStatus::Completed
        } else {
            MobileLinuxTaskStatus::Failed
        };
        self.finish_task(
            &task_id,
            &task,
            status,
            (!result.timed_out && !cancelled).then_some(result.exit_code),
            if cancelled || result.cancelled {
                Some("command cancelled".to_string())
            } else {
                result.timed_out.then(|| "command timed out".to_string())
            },
        );
        Ok(result)
    }

    async fn run_isolated(
        &self,
        request: LinuxCommandRequest,
    ) -> Result<LinuxCommandResult, MobileLinuxError> {
        let (result, task_id, task) = self
            .run_inner(
                request,
                MobileLinuxTaskStatus::Running,
                ForegroundMountMode::RequestOnly,
            )
            .await?;
        let cancelled = task.cancel_requested.load(Ordering::Acquire);
        let status = if cancelled {
            MobileLinuxTaskStatus::Cancelled
        } else if result.timed_out {
            MobileLinuxTaskStatus::TimedOut
        } else if result.cancelled {
            MobileLinuxTaskStatus::Cancelled
        } else if result.exit_code == 0 {
            MobileLinuxTaskStatus::Completed
        } else {
            MobileLinuxTaskStatus::Failed
        };
        self.finish_task(
            &task_id,
            &task,
            status,
            (!result.timed_out && !cancelled).then_some(result.exit_code),
            if cancelled || result.cancelled {
                Some("command cancelled".to_string())
            } else {
                result.timed_out.then(|| "command timed out".to_string())
            },
        );
        Ok(result)
    }

    async fn run_streaming(
        &self,
        request: LinuxCommandRequest,
        sink: Arc<dyn ProcessStreamSink>,
    ) -> Result<LinuxCommandResult, MobileLinuxError> {
        let mut cursor = self
            .state
            .next_sequence
            .load(Ordering::Acquire)
            .saturating_sub(1);
        let handle = self.spawn_background(request).await?;
        let mut stdout = String::new();
        let mut stderr = Vec::new();

        loop {
            for event in self
                .read_events(
                    Some(cursor),
                    platform_api::mobile_linux::MAX_MOBILE_LINUX_EVENT_BATCH,
                )
                .await?
            {
                cursor = cursor.max(event.sequence);
                if event.task_id.as_deref() != Some(handle.id.as_str()) {
                    continue;
                }
                let sink_result = match event.kind {
                    MobileLinuxEventKind::StdoutLine { line } => {
                        append_capped_stdout(&mut stdout, &line);
                        sink.stdout_line(line).await
                    }
                    MobileLinuxEventKind::StderrChunk { chunk } => {
                        append_capped_bytes(&mut stderr, &chunk);
                        sink.stderr_chunk(chunk).await
                    }
                    _ => Ok(()),
                };
                if let Err(error) = sink_result {
                    let _ = self.kill(&handle).await;
                    return Err(MobileLinuxError::from(error));
                }
            }

            let snapshot = self.task_status(&handle.id).await?.ok_or_else(|| {
                MobileLinuxError::Io("streaming task disappeared from the runtime".to_string())
            })?;
            if matches!(
                snapshot.status,
                MobileLinuxTaskStatus::Completed
                    | MobileLinuxTaskStatus::Failed
                    | MobileLinuxTaskStatus::Cancelled
                    | MobileLinuxTaskStatus::TimedOut
            ) {
                return Ok(LinuxCommandResult {
                    stdout,
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    exit_code: snapshot.exit_code.unwrap_or(-1),
                    timed_out: matches!(snapshot.status, MobileLinuxTaskStatus::TimedOut),
                    cancelled: matches!(snapshot.status, MobileLinuxTaskStatus::Cancelled),
                    enforcement: handle.enforcement,
                });
            }
            tokio::time::sleep(BACKGROUND_IDLE_POLL).await;
        }
    }

    async fn spawn_background(
        &self,
        request: LinuxCommandRequest,
    ) -> Result<LinuxProcessHandle, MobileLinuxError> {
        validate_request(&request)?;
        self.boot().await?;
        let mounts = self.merged_mounts(&request.mounts)?;
        self.apply_mounts(&mounts).await?;
        let (task_id, task) = self.create_task(
            "bg",
            display_command(&request.command, &request.args),
            MobileLinuxTaskStatus::Backgrounded,
        );
        let native_start = match self.native_spawn_background(&request, &mounts).await {
            Ok(start) if !start.process_id.trim().is_empty() => start,
            Ok(_) => {
                let error = MobileLinuxError::Io(
                    "native background spawn returned an empty process id".to_string(),
                );
                self.finish_task(
                    &task_id,
                    &task,
                    MobileLinuxTaskStatus::Failed,
                    None,
                    Some(error.to_string()),
                );
                return Err(error);
            }
            Err(error) => {
                self.finish_task(
                    &task_id,
                    &task,
                    MobileLinuxTaskStatus::Failed,
                    None,
                    Some(error.to_string()),
                );
                return Err(error);
            }
        };
        *task
            .native_handle
            .lock()
            .expect("ios-ish native handle mutex") = Some(native_start.process_id.clone());
        self.spawn_background_reader(task_id.clone(), native_start.process_id, task);
        Ok(LinuxProcessHandle {
            id: task_id,
            enforcement: native_start.enforcement,
        })
    }

    async fn kill(&self, handle: &LinuxProcessHandle) -> Result<(), MobileLinuxError> {
        let task = self
            .state
            .tasks
            .lock()
            .expect("ios-ish tasks mutex")
            .get(&handle.id)
            .cloned()
            .ok_or_else(|| MobileLinuxError::InvalidRequest("unknown task handle".to_string()))?;
        if task.terminal_emitted.load(Ordering::Acquire) {
            return Ok(());
        }
        let native_process_id = task
            .native_handle
            .lock()
            .expect("ios-ish native handle mutex")
            .clone()
            .ok_or_else(|| {
                MobileLinuxError::InvalidRequest(
                    "task is not a killable background process".to_string(),
                )
            })?;
        if self.native_kill_background(&native_process_id).await? {
            task.cancel_requested.store(true, Ordering::Release);
        }
        let deadline = tokio::time::Instant::now() + BACKGROUND_REAP_BUDGET;
        while !task.terminal_emitted.load(Ordering::Acquire) {
            if tokio::time::Instant::now() >= deadline {
                return Err(MobileLinuxError::Io(format!(
                    "background task {} did not reap within 3 seconds",
                    handle.id
                )));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }

    async fn open_pty(
        &self,
        request: PtyOpenRequest,
    ) -> Result<PtySessionHandle, MobileLinuxError> {
        validate_pty_request(&request)?;
        self.boot().await?;
        let mounts = self.merged_mounts(&request.mounts)?;
        self.apply_mounts(&mounts).await?;
        {
            let guard = self.state.pty.lock().expect("ios-ish pty mutex");
            if guard.is_some() {
                return Err(MobileLinuxError::InvalidRequest(
                    "only one PTY session is supported by the iSH bridge".to_string(),
                ));
            }
        }
        let session_id = match self.native_open_pty(&request, &mounts).await {
            Ok(session_id) => session_id,
            Err(error) => {
                self.emit_runtime_error(None, error.to_string());
                return Err(error);
            }
        };
        let task = self.create_task_with_id(
            session_id.clone(),
            display_command(&request.command, &request.args),
            MobileLinuxTaskStatus::Running,
        );
        if session_id.trim().is_empty() {
            self.finish_task(
                &session_id,
                &task,
                MobileLinuxTaskStatus::Failed,
                None,
                Some("native PTY open returned an empty session id".to_string()),
            );
            return Err(MobileLinuxError::Io(
                "native PTY open returned an empty session id".to_string(),
            ));
        }
        let control = Arc::new(PtyControl {
            task,
            open: AtomicBool::new(true),
            close_emitted: AtomicBool::new(false),
        });
        *self.state.pty.lock().expect("ios-ish pty mutex") =
            Some((session_id.clone(), control.clone()));
        self.spawn_pty_reader(session_id.clone(), control);
        Ok(PtySessionHandle { id: session_id })
    }

    async fn write_pty(
        &self,
        handle: &PtySessionHandle,
        input: Vec<u8>,
    ) -> Result<(), MobileLinuxError> {
        let control = self.current_pty(&handle.id)?;
        if !control.open.load(Ordering::Acquire) {
            return Err(MobileLinuxError::InvalidRequest(
                "PTY session is already closed".to_string(),
            ));
        }
        self.native_write_pty(&handle.id, input).await
    }

    async fn resize_pty(
        &self,
        handle: &PtySessionHandle,
        size: PtySize,
    ) -> Result<(), MobileLinuxError> {
        let control = self.current_pty(&handle.id)?;
        if !control.open.load(Ordering::Acquire) {
            return Err(MobileLinuxError::InvalidRequest(
                "PTY session is already closed".to_string(),
            ));
        }
        self.native_resize_pty(&handle.id, size).await
    }

    async fn close_pty(&self, handle: &PtySessionHandle) -> Result<(), MobileLinuxError> {
        let control = self.current_pty(&handle.id)?;
        control.open.store(false, Ordering::Release);
        // Do not `?` here. Only one PTY may be open per runtime, so returning
        // early on a failed native close left `state.pty` occupied for the life
        // of the handle and every later `open_pty` was refused with "only one
        // PTY session is supported by the iSH bridge". The slot is our own
        // bookkeeping: release it either way and report the failure afterwards.
        let native_result = self.native_close_pty(&handle.id).await;
        self.finish_task(
            &handle.id,
            &control.task,
            MobileLinuxTaskStatus::Completed,
            Some(0),
            Some("PTY closed".to_string()),
        );
        self.emit_pty_closed_once(
            &handle.id,
            &control,
            Some(0),
            Some("PTY closed".to_string()),
        );
        self.clear_current_pty(&handle.id);
        native_result
    }

    async fn open_raw_stdio(
        &self,
        request: RawStdioOpenRequest,
    ) -> Result<RawStdioSessionHandle, MobileLinuxError> {
        let validation_request = LinuxCommandRequest {
            command: request.command.clone(),
            args: request.args.clone(),
            cwd: request.cwd.clone(),
            env: request.env.clone(),
            stdin: None,
            timeout_ms: None,
            network: request.network,
            resource_limits: request.resource_limits,
            mounts: request.mounts.clone(),
        };
        validate_request(&validation_request)?;
        self.boot().await?;
        let mounts = self.merged_mounts(&request.mounts)?;
        let (id, enforcement) = self.native_open_raw_stdio(&request, &mounts).await?;
        enforcement.ensure_for(request.network, request.resource_limits)?;
        let handle = RawStdioSessionHandle { id, enforcement };
        self.state
            .raw_stdio
            .lock()
            .expect("ios-ish raw stdio mutex")
            .insert(handle.id.clone(), handle.clone());
        Ok(handle)
    }

    async fn write_raw_stdio(
        &self,
        handle: &RawStdioSessionHandle,
        input: Vec<u8>,
    ) -> Result<(), MobileLinuxError> {
        if !self
            .state
            .raw_stdio
            .lock()
            .expect("ios-ish raw stdio mutex")
            .contains_key(&handle.id)
        {
            return Err(MobileLinuxError::InvalidRequest(
                "unknown raw stdio session".to_string(),
            ));
        }
        let response = self
            .native_raw_stdio_call(
                RawStdioNativeOperation::Write,
                RawStdioRequestPayload {
                    session_id: handle.id.clone(),
                    data_base64: Some(encode_base64(&input)),
                    max_bytes: None,
                },
            )
            .await?;
        parse_native_ok(&response)
    }

    async fn read_raw_stdio(
        &self,
        handle: &RawStdioSessionHandle,
        max_bytes: usize,
    ) -> Result<RawStdioReadResult, MobileLinuxError> {
        if !self
            .state
            .raw_stdio
            .lock()
            .expect("ios-ish raw stdio mutex")
            .contains_key(&handle.id)
        {
            return Err(MobileLinuxError::InvalidRequest(
                "unknown raw stdio session".to_string(),
            ));
        }
        let response = self
            .native_raw_stdio_call(
                RawStdioNativeOperation::Read,
                RawStdioRequestPayload {
                    session_id: handle.id.clone(),
                    data_base64: None,
                    max_bytes: Some(max_bytes.clamp(1, 1_048_576) as u32),
                },
            )
            .await?;
        parse_raw_stdio_read_response(&response)
    }

    async fn close_raw_stdio(
        &self,
        handle: &RawStdioSessionHandle,
    ) -> Result<(), MobileLinuxError> {
        if !self
            .state
            .raw_stdio
            .lock()
            .expect("ios-ish raw stdio mutex")
            .contains_key(&handle.id)
        {
            return Ok(());
        }
        let request = RawStdioRequestPayload {
            session_id: handle.id.clone(),
            data_base64: None,
            max_bytes: None,
        };
        let close_response = self
            .native_raw_stdio_call(RawStdioNativeOperation::Close, request)
            .await?;
        parse_native_ok(&close_response)?;

        let deadline = tokio::time::Instant::now() + BACKGROUND_REAP_BUDGET;
        loop {
            let response = self
                .native_raw_stdio_call(
                    RawStdioNativeOperation::Read,
                    RawStdioRequestPayload {
                        session_id: handle.id.clone(),
                        data_base64: None,
                        max_bytes: Some(65_536),
                    },
                )
                .await?;
            if parse_raw_stdio_read_response(&response)?.closed {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(MobileLinuxError::Io(format!(
                    "raw stdio session {} did not reap within 3 seconds",
                    handle.id
                )));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        let dispose_response = self
            .native_raw_stdio_call(
                RawStdioNativeOperation::Dispose,
                RawStdioRequestPayload {
                    session_id: handle.id.clone(),
                    data_base64: None,
                    max_bytes: None,
                },
            )
            .await?;
        parse_native_ok(&dispose_response)?;
        self.state
            .raw_stdio
            .lock()
            .expect("ios-ish raw stdio mutex")
            .remove(&handle.id);
        Ok(())
    }

    async fn rootfs_status(&self) -> Result<RootfsStatus, MobileLinuxError> {
        Ok(self.rootfs_snapshot())
    }

    async fn verify_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
        let status = self.rootfs_snapshot();
        if matches!(status.state, RootfsState::Ready) {
            Ok(status)
        } else {
            Ok(self.rootfs_snapshot_with_error(
                Some(match status.state {
                    RootfsState::Missing => RootfsState::Missing,
                    RootfsState::Unsupported => RootfsState::Unsupported,
                    _ => RootfsState::Corrupt,
                }),
                status.last_error,
            ))
        }
    }

    async fn repair_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
        self.shutdown().await?;
        self.ensure_native_available()?;
        let config_json = self.native_config_json()?;
        let native_lock = self.state.native_lock.clone();
        let response = spawn_blocking(move || {
            let _guard = native_lock.lock().expect("ios-ish native lock");
            native::repair_rootfs_json(&config_json)
        })
        .await
        .map_err(|error| MobileLinuxError::Io(format!("join repair_rootfs: {error}")))?
        .map_err(MobileLinuxError::Io)?;
        let _ = parse_native_ok(&response)?;
        Ok(self.rootfs_snapshot())
    }

    async fn reset_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
        self.shutdown().await?;
        self.install_rootfs(true).await?;
        Ok(self.rootfs_snapshot())
    }

    fn current_mounts(&self) -> Vec<MountSpec> {
        self.state
            .mounts
            .read()
            .expect("ios-ish mounts rwlock")
            .clone()
    }

    async fn configure_mounts(&self, mounts: Vec<MountSpec>) -> Result<(), MobileLinuxError> {
        let mut normalized = Vec::with_capacity(mounts.len().saturating_add(2));
        normalized.push(validate_mount(
            &self.state.config.default_workspace_mount(),
            &self.state.config,
        )?);
        normalized.push(self.state.config.persistent_home_mount());
        for mount in mounts {
            let mount = validate_mount(&mount, &self.state.config)?;
            if let Some(existing) = normalized
                .iter_mut()
                .find(|existing| existing.guest_path == mount.guest_path)
            {
                *existing = mount;
            } else {
                normalized.push(mount);
            }
        }
        *self.state.mounts.write().expect("ios-ish mounts rwlock") = normalized.clone();
        self.apply_mounts(&normalized).await
    }

    async fn read_events(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<MobileLinuxEvent>, MobileLinuxError> {
        let after = after_sequence.unwrap_or(0);
        Ok(self
            .state
            .events
            .lock()
            .expect("ios-ish events mutex")
            .iter()
            .filter(|event| event.sequence > after)
            .take(limit.min(platform_api::mobile_linux::MAX_MOBILE_LINUX_EVENT_BATCH))
            .cloned()
            .collect())
    }

    async fn list_tasks(&self) -> Result<Vec<MobileLinuxTaskSnapshot>, MobileLinuxError> {
        let mut tasks: Vec<_> = self
            .state
            .tasks
            .lock()
            .expect("ios-ish tasks mutex")
            .values()
            .map(|task| {
                task.snapshot
                    .lock()
                    .expect("ios-ish task snapshot mutex")
                    .clone()
            })
            .collect();
        tasks.sort_by(|left, right| {
            left.started_at_ms
                .cmp(&right.started_at_ms)
                .then_with(|| left.task_id.cmp(&right.task_id))
        });
        Ok(tasks)
    }

    async fn task_status(
        &self,
        task_id: &str,
    ) -> Result<Option<MobileLinuxTaskSnapshot>, MobileLinuxError> {
        Ok(self
            .state
            .tasks
            .lock()
            .expect("ios-ish tasks mutex")
            .get(task_id)
            .map(|task| {
                task.snapshot
                    .lock()
                    .expect("ios-ish task snapshot mutex")
                    .clone()
            }))
    }
}

fn append_capped_stdout(output: &mut String, line: &str) {
    let remaining = MAX_STREAM_CAPTURE_BYTES.saturating_sub(output.len());
    if remaining == 0 {
        return;
    }
    let mut take = line.len().min(remaining);
    while !line.is_char_boundary(take) {
        take = take.saturating_sub(1);
    }
    output.push_str(&line[..take]);
    if output.len() < MAX_STREAM_CAPTURE_BYTES {
        output.push('\n');
    }
}

fn append_capped_bytes(output: &mut Vec<u8>, chunk: &[u8]) {
    let remaining = MAX_STREAM_CAPTURE_BYTES.saturating_sub(output.len());
    output.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
}

/// Build the shared trait object wired to this crate's iSH runtime backend.
#[must_use]
pub fn linked_runtime(config: IosIshRuntimeConfig) -> Arc<dyn MobileLinuxRuntime> {
    Arc::new(IosIshRuntime::new(config)) as Arc<dyn MobileLinuxRuntime>
}

#[derive(Debug, Serialize)]
struct NativeConfigPayload {
    managed_root: String,
    workspace_host_path: String,
    stable_workspace_id: String,
    abi: String,
    rootfs_version: String,
    archive_sha256: Option<String>,
    authorization_file: Option<String>,
}

#[derive(Debug, Serialize)]
struct MountConfigPayload {
    mounts: Vec<MountPayload>,
}

impl MountConfigPayload {
    fn from_mounts(mounts: &[MountSpec]) -> Self {
        Self {
            mounts: mounts.iter().map(MountPayload::from_mount).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct RunRequestPayload {
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
    env: BTreeMap<String, String>,
    stdin: Option<String>,
    timeout_ms: Option<u64>,
    network: &'static str,
    resource_limits: platform_api::ResourceLimits,
    mounts: Vec<MountPayload>,
    include_default_mounts: bool,
}

#[derive(Debug, Serialize)]
struct RawStdioOpenPayload {
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
    env: BTreeMap<String, String>,
    network: &'static str,
    resource_limits: platform_api::ResourceLimits,
    mounts: Vec<MountPayload>,
}

impl RawStdioOpenPayload {
    fn from_request(request: &RawStdioOpenRequest, mounts: &[MountSpec]) -> Self {
        Self {
            command: request.command.clone(),
            args: request.args.clone(),
            cwd: request.cwd.clone(),
            env: request.env.clone(),
            network: match request.network {
                platform_api::NetworkPolicy::Disabled => "disabled",
                platform_api::NetworkPolicy::LoopbackOnly => "loopback-only",
                platform_api::NetworkPolicy::Allowed => "allowed",
            },
            resource_limits: request.resource_limits,
            mounts: mounts.iter().map(MountPayload::from_mount).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct RawStdioRequestPayload {
    session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_bytes: Option<u32>,
}

#[derive(Clone, Copy)]
enum RawStdioNativeOperation {
    Write,
    Read,
    Close,
    Dispose,
}

impl RunRequestPayload {
    fn from_request(
        request: &LinuxCommandRequest,
        mounts: &[MountSpec],
        include_default_mounts: bool,
    ) -> Self {
        Self {
            command: request.command.clone(),
            args: request.args.clone(),
            cwd: request.cwd.clone(),
            env: request.env.clone(),
            stdin: request.stdin.clone(),
            timeout_ms: request.timeout_ms,
            network: match request.network {
                platform_api::NetworkPolicy::Disabled => "disabled",
                platform_api::NetworkPolicy::LoopbackOnly => "loopback-only",
                platform_api::NetworkPolicy::Allowed => "allowed",
            },
            resource_limits: request.resource_limits,
            mounts: mounts.iter().map(MountPayload::from_mount).collect(),
            include_default_mounts,
        }
    }
}

#[derive(Debug, Serialize)]
struct PtyOpenPayload {
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
    env: BTreeMap<String, String>,
    cols: u16,
    rows: u16,
    mounts: Vec<MountPayload>,
}

impl PtyOpenPayload {
    fn from_request(request: &PtyOpenRequest, mounts: &[MountSpec]) -> Self {
        Self {
            command: request.command.clone(),
            args: request.args.clone(),
            cwd: request.cwd.clone(),
            env: request.env.clone(),
            cols: request.size.cols,
            rows: request.size.rows,
            mounts: mounts.iter().map(MountPayload::from_mount).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct PtyWritePayload {
    session_id: String,
    data_base64: String,
}

#[derive(Debug, Serialize)]
struct PtyResizePayload {
    session_id: String,
    cols: u16,
    rows: u16,
}

#[derive(Debug, Serialize)]
struct PtyClosePayload {
    session_id: String,
}

#[derive(Debug, Serialize)]
struct PtyPollPayload {
    after_sequence: Option<u64>,
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
struct BackgroundProcessPayload {
    process_id: String,
}

#[derive(Debug, Serialize)]
struct BackgroundPollPayload {
    process_id: String,
    after_sequence: Option<u64>,
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
struct LoopbackProbePayload {
    port: u16,
    timeout_ms: u32,
}

#[derive(Debug, Serialize)]
struct MountPayload {
    host_path: String,
    guest_path: String,
    read_only: bool,
    purpose: &'static str,
}

impl MountPayload {
    fn from_mount(mount: &MountSpec) -> Self {
        Self {
            host_path: mount.host_path.display().to_string(),
            guest_path: mount.guest_path.clone(),
            read_only: mount.read_only,
            purpose: match mount.purpose {
                MountPurpose::Workspace => "workspace",
                MountPurpose::LocalAppBuild => "local_app_build",
                MountPurpose::Memory => "memory",
                MountPurpose::Skills => "skills",
                MountPurpose::Shared => "shared",
                MountPurpose::External => "external",
                MountPurpose::Temp => "temp",
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct RunResponsePayload {
    stdout: String,
    stderr: String,
    exit_code: i32,
    #[serde(default)]
    timed_out: bool,
    #[serde(default)]
    cancelled: bool,
    #[serde(default)]
    network_policy_enforced: bool,
    #[serde(default)]
    memory_limit_enforced: bool,
}

#[derive(Debug, Deserialize)]
struct NativeErrorPayload {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct NativeRunEnvelope {
    ok: bool,
    error: Option<NativeErrorPayload>,
    result: Option<RunResponsePayload>,
}

#[derive(Debug, Deserialize)]
struct NativeAvailabilityEnvelope {
    ok: bool,
    error: Option<NativeErrorPayload>,
    available: Option<bool>,
    kernel_reason: Option<String>,
    shell_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NativeOkEnvelope {
    ok: bool,
    error: Option<NativeErrorPayload>,
}

#[derive(Debug, Deserialize)]
struct NativeSessionEnvelope {
    ok: bool,
    error: Option<NativeErrorPayload>,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NativeRawStdioOpenEnvelope {
    ok: bool,
    error: Option<NativeErrorPayload>,
    session_id: Option<String>,
    #[serde(default)]
    network_policy_enforced: bool,
    #[serde(default)]
    memory_limit_enforced: bool,
}

#[derive(Debug, Deserialize)]
struct NativeRawStdioReadEnvelope {
    ok: bool,
    error: Option<NativeErrorPayload>,
    stdout_base64: Option<String>,
    stderr_base64: Option<String>,
    #[serde(default)]
    closed: bool,
    exit_code: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct NativeProcessEnvelope {
    ok: bool,
    error: Option<NativeErrorPayload>,
    process_id: Option<String>,
    #[serde(default)]
    network_policy_enforced: bool,
    #[serde(default)]
    memory_limit_enforced: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct NativeProcessStart {
    process_id: String,
    enforcement: LinuxEnforcementReceipt,
}

#[derive(Debug, Deserialize)]
struct NativeBackgroundKillEnvelope {
    ok: bool,
    error: Option<NativeErrorPayload>,
    termination_requested: Option<bool>,
    already_stopped: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct NativePtyEventPayload {
    sequence: u64,
    session_id: String,
    kind: String,
    data_base64: Option<String>,
    detail: Option<String>,
    /// Real guest exit code on a shell self-exit (`pty_closed` emitted by the
    /// bridge's ISHProcessExited observer). Absent for bridge-initiated
    /// closes, whose contract stays "closed cleanly" (code 0).
    #[serde(default)]
    exit_code: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct NativePollEnvelope {
    ok: bool,
    error: Option<NativeErrorPayload>,
    #[serde(default)]
    events: Vec<NativePtyEventPayload>,
}

#[derive(Debug, Clone, Deserialize)]
struct NativeBackgroundEventPayload {
    sequence: u64,
    kind: String,
    line: Option<String>,
    data_base64: Option<String>,
    exit_code: Option<i32>,
    cancelled: Option<bool>,
    detail: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NativeBackgroundPollEnvelope {
    ok: bool,
    error: Option<NativeErrorPayload>,
    #[serde(default)]
    events: Vec<NativeBackgroundEventPayload>,
}

#[derive(Debug, Deserialize)]
struct NativeLoopbackProbeEnvelope {
    ok: bool,
    error: Option<NativeErrorPayload>,
    reachable: Option<bool>,
}

fn background_terminal_state(
    cancel_requested: bool,
    event: &NativeBackgroundEventPayload,
) -> (MobileLinuxTaskStatus, Option<i32>, Option<String>) {
    let cancelled = cancel_requested || event.cancelled.unwrap_or(false);
    let status = if cancelled {
        MobileLinuxTaskStatus::Cancelled
    } else if event.exit_code == Some(0) {
        MobileLinuxTaskStatus::Completed
    } else {
        MobileLinuxTaskStatus::Failed
    };
    let detail = event
        .detail
        .clone()
        .or_else(|| cancelled.then(|| "task cancelled".to_string()));
    (status, event.exit_code, detail)
}

fn parse_run_response(json: &str) -> Result<LinuxCommandResult, MobileLinuxError> {
    let envelope = serde_json::from_str::<NativeRunEnvelope>(json)
        .map_err(|error| MobileLinuxError::Io(format!("parse run_json response: {error}")))?;
    if !envelope.ok {
        return Err(native_error_to_mobile(envelope.error.unwrap_or(
            NativeErrorPayload {
                code: "io".to_string(),
                message: "native run_json response indicated failure".to_string(),
            },
        )));
    }
    let payload = envelope.result.ok_or_else(|| {
        MobileLinuxError::Io("native run_json response omitted result".to_string())
    })?;
    Ok(LinuxCommandResult {
        stdout: payload.stdout,
        stderr: payload.stderr,
        exit_code: payload.exit_code,
        timed_out: payload.timed_out,
        cancelled: payload.cancelled,
        enforcement: LinuxEnforcementReceipt {
            network_policy_enforced: payload.network_policy_enforced,
            memory_limit_enforced: payload.memory_limit_enforced,
        },
    })
}

fn parse_native_ok(json: &str) -> Result<(), MobileLinuxError> {
    let envelope = serde_json::from_str::<NativeOkEnvelope>(json)
        .map_err(|error| MobileLinuxError::Io(format!("parse native response: {error}")))?;
    if envelope.ok {
        Ok(())
    } else {
        Err(native_error_to_mobile(envelope.error.unwrap_or(
            NativeErrorPayload {
                code: "io".to_string(),
                message: "native response indicated failure".to_string(),
            },
        )))
    }
}

fn parse_session_id_response(json: &str) -> Result<String, MobileLinuxError> {
    let envelope = serde_json::from_str::<NativeSessionEnvelope>(json)
        .map_err(|error| MobileLinuxError::Io(format!("parse PTY response: {error}")))?;
    if !envelope.ok {
        return Err(native_error_to_mobile(envelope.error.unwrap_or(
            NativeErrorPayload {
                code: "io".to_string(),
                message: "native PTY response indicated failure".to_string(),
            },
        )));
    }
    envelope
        .session_id
        .ok_or_else(|| MobileLinuxError::Io("native PTY response omitted session_id".to_string()))
}

fn parse_raw_stdio_open_response(
    json: &str,
) -> Result<(String, LinuxEnforcementReceipt), MobileLinuxError> {
    let envelope = serde_json::from_str::<NativeRawStdioOpenEnvelope>(json)
        .map_err(|error| MobileLinuxError::Io(format!("parse raw stdio open: {error}")))?;
    if !envelope.ok {
        return Err(native_error_to_mobile(envelope.error.unwrap_or(
            NativeErrorPayload {
                code: "io".to_string(),
                message: "native raw stdio open indicated failure".to_string(),
            },
        )));
    }
    let session_id = envelope.session_id.ok_or_else(|| {
        MobileLinuxError::Io("native raw stdio open omitted session_id".to_string())
    })?;
    Ok((
        session_id,
        LinuxEnforcementReceipt {
            network_policy_enforced: envelope.network_policy_enforced,
            memory_limit_enforced: envelope.memory_limit_enforced,
        },
    ))
}

fn parse_raw_stdio_read_response(json: &str) -> Result<RawStdioReadResult, MobileLinuxError> {
    let envelope = serde_json::from_str::<NativeRawStdioReadEnvelope>(json)
        .map_err(|error| MobileLinuxError::Io(format!("parse raw stdio read: {error}")))?;
    if !envelope.ok {
        return Err(native_error_to_mobile(envelope.error.unwrap_or(
            NativeErrorPayload {
                code: "io".to_string(),
                message: "native raw stdio read indicated failure".to_string(),
            },
        )));
    }
    Ok(RawStdioReadResult {
        stdout: envelope
            .stdout_base64
            .as_deref()
            .map(decode_base64)
            .transpose()?
            .unwrap_or_default(),
        stderr: envelope
            .stderr_base64
            .as_deref()
            .map(decode_base64)
            .transpose()?
            .unwrap_or_default(),
        closed: envelope.closed,
        exit_code: envelope.exit_code,
    })
}

fn parse_process_id_response(json: &str) -> Result<NativeProcessStart, MobileLinuxError> {
    let envelope = serde_json::from_str::<NativeProcessEnvelope>(json)
        .map_err(|error| MobileLinuxError::Io(format!("parse background response: {error}")))?;
    if !envelope.ok {
        return Err(native_error_to_mobile(envelope.error.unwrap_or(
            NativeErrorPayload {
                code: "io".to_string(),
                message: "native background response indicated failure".to_string(),
            },
        )));
    }
    let process_id = envelope.process_id.ok_or_else(|| {
        MobileLinuxError::Io("native background response omitted process_id".to_string())
    })?;
    Ok(NativeProcessStart {
        process_id,
        enforcement: LinuxEnforcementReceipt {
            network_policy_enforced: envelope.network_policy_enforced,
            memory_limit_enforced: envelope.memory_limit_enforced,
        },
    })
}

fn parse_background_kill_response(json: &str) -> Result<bool, MobileLinuxError> {
    let envelope = serde_json::from_str::<NativeBackgroundKillEnvelope>(json).map_err(|error| {
        MobileLinuxError::Io(format!("parse background kill response: {error}"))
    })?;
    if !envelope.ok {
        return Err(native_error_to_mobile(envelope.error.unwrap_or(
            NativeErrorPayload {
                code: "io".to_string(),
                message: "native background kill response indicated failure".to_string(),
            },
        )));
    }
    match (envelope.termination_requested, envelope.already_stopped) {
        (Some(requested), _) => Ok(requested),
        (None, Some(true)) => Ok(false),
        _ => Err(MobileLinuxError::Io(
            "native background kill response omitted termination state".to_string(),
        )),
    }
}

fn parse_poll_events(json: &str) -> Result<Vec<NativePtyEventPayload>, MobileLinuxError> {
    let envelope = serde_json::from_str::<NativePollEnvelope>(json)
        .map_err(|error| MobileLinuxError::Io(format!("parse PTY poll response: {error}")))?;
    if envelope.ok {
        Ok(envelope.events)
    } else {
        Err(native_error_to_mobile(envelope.error.unwrap_or(
            NativeErrorPayload {
                code: "io".to_string(),
                message: "native PTY poll indicated failure".to_string(),
            },
        )))
    }
}

fn parse_background_events(
    json: &str,
) -> Result<Vec<NativeBackgroundEventPayload>, MobileLinuxError> {
    let envelope = serde_json::from_str::<NativeBackgroundPollEnvelope>(json).map_err(|error| {
        MobileLinuxError::Io(format!("parse background poll response: {error}"))
    })?;
    if envelope.ok {
        Ok(envelope.events)
    } else {
        Err(native_error_to_mobile(envelope.error.unwrap_or(
            NativeErrorPayload {
                code: "io".to_string(),
                message: "native background poll indicated failure".to_string(),
            },
        )))
    }
}

fn parse_loopback_probe(json: &str) -> Result<bool, MobileLinuxError> {
    let envelope = serde_json::from_str::<NativeLoopbackProbeEnvelope>(json)
        .map_err(|error| MobileLinuxError::Io(format!("parse loopback probe response: {error}")))?;
    if !envelope.ok {
        return Err(native_error_to_mobile(envelope.error.unwrap_or(
            NativeErrorPayload {
                code: "io".to_string(),
                message: "native loopback probe indicated failure".to_string(),
            },
        )));
    }
    envelope
        .reachable
        .ok_or_else(|| MobileLinuxError::Io("native loopback probe omitted reachable".to_string()))
}

fn parse_availability_reason(json: &str) -> Option<String> {
    let envelope = serde_json::from_str::<NativeAvailabilityEnvelope>(json).ok()?;
    if envelope.ok && envelope.available.unwrap_or(false) {
        return None;
    }
    if let Some(error) = envelope.error {
        return Some(error.message);
    }
    let mut reasons = Vec::new();
    if let Some(reason) = envelope.kernel_reason.filter(|reason| !reason.is_empty()) {
        reasons.push(format!("kernel: {reason}"));
    }
    if let Some(reason) = envelope.shell_reason.filter(|reason| !reason.is_empty()) {
        reasons.push(format!("shell: {reason}"));
    }
    if reasons.is_empty() {
        Some("native iSH runtime reports unavailable".to_string())
    } else {
        Some(reasons.join("; "))
    }
}

fn native_error_to_mobile(error: NativeErrorPayload) -> MobileLinuxError {
    match error.code.as_str() {
        "invalid_request" => MobileLinuxError::InvalidRequest(error.message),
        "unavailable" => MobileLinuxError::Unavailable(error.message),
        "network_policy_unavailable" => MobileLinuxError::NetworkPolicyUnavailable(error.message),
        "resource_limit_exceeded" => MobileLinuxError::ResourceLimitExceeded(error.message),
        "io" => MobileLinuxError::Io(error.message),
        _ => MobileLinuxError::Io(error.message),
    }
}

fn validate_request(request: &LinuxCommandRequest) -> Result<(), MobileLinuxError> {
    if request.command.trim().is_empty() {
        return Err(MobileLinuxError::InvalidRequest(
            "command must not be empty".to_string(),
        ));
    }
    validate_no_nul(&request.command, "command")?;
    validate_args(&request.args)?;
    if let Some(cwd) = &request.cwd {
        validate_no_nul(cwd, "cwd")?;
        validate_guest_path(cwd, "cwd", true)?;
    }
    validate_env(&request.env)?;
    if let Some(stdin) = &request.stdin {
        validate_no_nul(stdin, "stdin")?;
    }
    let limits = request.resource_limits;
    if limits.max_cpu_seconds.is_some()
        || limits.max_processes.is_some()
        || limits.max_open_files.is_some()
    {
        return Err(MobileLinuxError::ResourceLimitExceeded(
            "ios-ish runtime supports only the per-execution memory limit".to_string(),
        ));
    }
    if matches!(limits.max_memory_mb, Some(0)) {
        return Err(MobileLinuxError::InvalidRequest(
            "max_memory_mb must be greater than zero".to_string(),
        ));
    }
    if matches!(request.timeout_ms, Some(0)) {
        return Err(MobileLinuxError::InvalidRequest(
            "timeout_ms must be greater than zero when provided".to_string(),
        ));
    }
    Ok(())
}

fn validate_pty_request(request: &PtyOpenRequest) -> Result<(), MobileLinuxError> {
    if request.command.trim().is_empty() {
        return Err(MobileLinuxError::InvalidRequest(
            "command must not be empty".to_string(),
        ));
    }
    validate_no_nul(&request.command, "command")?;
    validate_args(&request.args)?;
    if request.size.cols == 0 || request.size.rows == 0 {
        return Err(MobileLinuxError::InvalidRequest(
            "PTY size must be non-zero".to_string(),
        ));
    }
    if let Some(cwd) = &request.cwd {
        validate_no_nul(cwd, "cwd")?;
        validate_guest_path(cwd, "cwd", true)?;
    }
    validate_env(&request.env)?;
    Ok(())
}

fn validate_args(args: &[String]) -> Result<(), MobileLinuxError> {
    for arg in args {
        validate_no_nul(arg, "args")?;
    }
    Ok(())
}

fn validate_env(env: &BTreeMap<String, String>) -> Result<(), MobileLinuxError> {
    for (key, value) in env {
        if key.is_empty() {
            return Err(MobileLinuxError::InvalidRequest(
                "environment keys must not be empty".to_string(),
            ));
        }
        if key.contains('=') {
            return Err(MobileLinuxError::InvalidRequest(
                "environment keys must not contain '='".to_string(),
            ));
        }
        validate_no_nul(key, "environment key")?;
        validate_no_nul(value, "environment value")?;
    }
    Ok(())
}

fn validate_no_nul(value: &str, field: &str) -> Result<(), MobileLinuxError> {
    if value.contains('\0') {
        return Err(MobileLinuxError::InvalidRequest(format!(
            "{field} must not contain NUL bytes"
        )));
    }
    Ok(())
}

fn validate_mount(
    mount: &MountSpec,
    config: &IosIshRuntimeConfig,
) -> Result<MountSpec, MobileLinuxError> {
    let host_path = normalize_host_path(&mount.host_path, "host_path")?;
    validate_guest_path(&mount.guest_path, "guest_path", false)?;
    let managed_root = normalize_host_path(&config.managed_root, "managed_root")?;
    let app_root = normalize_host_path(&config.app_sandbox_root, "app_sandbox_root")?;
    let lingxi_root = normalize_host_path(&config.lingxi_root(), ".lingxi root")?;
    let workspace_root = normalize_host_path(&config.workspace_host_path, "workspace_host_path")?;

    if guest_path_has_prefix(
        &mount.guest_path,
        platform_api::mobile_linux::guest_paths::HOME,
    ) {
        return Err(MobileLinuxError::InvalidRequest(
            "request mounts may not replace the runtime-managed persistent /root".to_string(),
        ));
    }

    if host_path == app_root
        || host_path == managed_root
        || host_path == lingxi_root
        || host_path.starts_with(&managed_root)
        || managed_root.starts_with(&host_path)
        || host_path.starts_with(&lingxi_root)
        || lingxi_root.starts_with(&host_path)
    {
        return Err(MobileLinuxError::InvalidRequest(
            "mount host_path may not target app root, managed_root, or .lingxi".to_string(),
        ));
    }

    if path_contains_protected_subtree(&host_path) {
        return Err(MobileLinuxError::InvalidRequest(
            "mount host_path may not target provider/config subtrees".to_string(),
        ));
    }

    if matches!(mount.purpose, MountPurpose::Workspace) {
        if mount.guest_path != config.workspace_guest_path() {
            return Err(MobileLinuxError::InvalidRequest(format!(
                "workspace mount guest_path must be {}",
                config.workspace_guest_path()
            )));
        }
        if host_path != workspace_root {
            return Err(MobileLinuxError::InvalidRequest(format!(
                "workspace mount host_path must match workspace_host_path {} (got {})",
                workspace_root.display(),
                host_path.display()
            )));
        }
    } else if matches!(mount.purpose, MountPurpose::LocalAppBuild) {
        let (app_id, channel) = parse_local_app_build_guest_path(&mount.guest_path)?;
        let expected_host_path = normalize_host_path(
            &config
                .app_sandbox_root
                .join("apps")
                .join(app_id)
                .join("build")
                .join(channel),
            "local-app build host_path",
        )?;
        let workspace_host_path = normalize_host_path(
            &config
                .app_sandbox_root
                .join("apps")
                .join(app_id)
                .join("workspace"),
            "local-app workspace host_path",
        )?;
        if host_path != workspace_host_path
            && !local_app_build_host_path_matches(&host_path, &expected_host_path, channel)
        {
            // Both paths, always. A guard that prints only what it WANTED
            // leaves the reader to guess what it got — and the two differ
            // here by a prefix (`/var` vs `/private/var`), a stale container
            // UUID, or a channel mismatch, which are three different bugs
            // that read identically without the actual value.
            return Err(MobileLinuxError::InvalidRequest(format!(
                "local-app build mount host_path must be {} or workspace {} or its matching .{channel}.staging-<numeric nonce> sibling (got {})",
                expected_host_path.display(), workspace_host_path.display(),
                host_path.display()
            )));
        }
    } else if matches!(mount.purpose, MountPurpose::Shared)
        && mount.guest_path == platform_api::mobile_linux::guest_paths::LOCAL_APP_DEPENDENCY_STORE
    {
        let expected_root = normalize_host_path(
            &config.app_sandbox_root.join("dependency-cache"),
            "dependency store host root",
        )?;
        if !host_path.starts_with(&expected_root) {
            return Err(MobileLinuxError::InvalidRequest(
                "dependency store mount must remain inside the app sandbox dependency-cache"
                    .to_string(),
            ));
        }
    } else if mount.guest_path == config.workspace_guest_path() {
        return Err(MobileLinuxError::InvalidRequest(
            "only workspace mounts may target the managed workspace guest path".to_string(),
        ));
    } else if guest_path_has_prefix(&mount.guest_path, LOCAL_APP_BUILD_GUEST_ROOT) {
        return Err(MobileLinuxError::InvalidRequest(
            "local-app build guest path accepts only one root mount; nested mounts are forbidden"
                .to_string(),
        ));
    }

    Ok(MountSpec {
        host_path,
        guest_path: mount.guest_path.clone(),
        read_only: mount.read_only,
        purpose: mount.purpose,
    })
}

fn local_app_build_host_path_matches(
    host_path: &Path,
    expected_host_path: &Path,
    channel: &str,
) -> bool {
    if host_path == expected_host_path {
        return true;
    }
    if host_path.parent() != expected_host_path.parent() {
        return false;
    }

    let staging_prefix = format!(".{channel}.staging-");
    let Some(nonce) = host_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(staging_prefix.as_str()))
    else {
        return false;
    };
    !nonce.is_empty() && nonce.bytes().all(|byte| byte.is_ascii_digit())
}

fn parse_local_app_build_guest_path(path: &str) -> Result<(&str, &str), MobileLinuxError> {
    let relative = path
        .strip_prefix(LOCAL_APP_BUILD_GUEST_ROOT)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .ok_or_else(|| {
            MobileLinuxError::InvalidRequest(format!(
                "local-app build guest_path must be {LOCAL_APP_BUILD_GUEST_ROOT}/<app-id>/<channel>/project"
            ))
        })?;
    let mut segments = relative.split('/');
    let app_id = segments.next().unwrap_or_default();
    let channel = segments.next().unwrap_or_default();
    let project = segments.next().unwrap_or_default();
    if segments.next().is_some()
        || !is_valid_local_app_id(app_id)
        || !matches!(channel, "store" | "full")
        || project != platform_api::mobile_linux::guest_paths::LOCAL_APP_BUILD_PROJECT_DIR
    {
        return Err(MobileLinuxError::InvalidRequest(format!(
            "local-app build guest_path must be {LOCAL_APP_BUILD_GUEST_ROOT}/<app-id>/<store|full>/project"
        )));
    }
    Ok((app_id, channel))
}

fn is_valid_local_app_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn guest_path_has_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn validate_guest_path(value: &str, field: &str, allow_root: bool) -> Result<(), MobileLinuxError> {
    if !value.starts_with('/') {
        return Err(MobileLinuxError::InvalidRequest(format!(
            "{field} must be an absolute guest path"
        )));
    }
    if !allow_root && value == "/" {
        return Err(MobileLinuxError::InvalidRequest(format!(
            "{field} may not be the guest root"
        )));
    }
    for component in Path::new(value).components() {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            return Err(MobileLinuxError::InvalidRequest(format!(
                "{field} may not contain path traversal"
            )));
        }
    }
    Ok(())
}

fn normalize_host_path(path: &Path, field: &str) -> Result<PathBuf, MobileLinuxError> {
    if !path.is_absolute() {
        return Err(MobileLinuxError::InvalidRequest(format!(
            "{field} must be absolute"
        )));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(MobileLinuxError::InvalidRequest(format!(
                    "{field} may not contain parent traversal"
                )))
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }

    let mut existing = normalized.as_path();
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(existing) {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name() else {
                    return Err(MobileLinuxError::InvalidRequest(format!(
                        "{field} has no resolvable ancestor"
                    )));
                };
                missing.push(name.to_os_string());
                let Some(parent) = existing.parent() else {
                    return Err(MobileLinuxError::InvalidRequest(format!(
                        "{field} has no resolvable ancestor"
                    )));
                };
                existing = parent;
            }
            Err(error) => {
                return Err(MobileLinuxError::InvalidRequest(format!(
                    "{field} cannot be resolved safely: {error}"
                )))
            }
        }
    }
}

fn path_contains_protected_subtree(path: &Path) -> bool {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    components
        .iter()
        .any(|component| matches!(component.as_str(), ".lingxi" | "provider" | "providers"))
        || components
            .windows(2)
            .any(|pair| pair[0] == "library" && pair[1] == "preferences")
}

fn filesystem_rootfs_state(active_root: &Path, managed_root: &Path) -> RootfsState {
    let active_metadata = fs::symlink_metadata(active_root).ok();
    if let Some(metadata) = active_metadata {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return RootfsState::Corrupt;
        }
        let data_root = active_root.join("data");
        let meta_db = active_root.join("meta.db");
        let arch = active_root.join(".arch");
        let arch_ok = fs::read_to_string(&arch)
            .map(|value| value.trim() == "aarch64")
            .unwrap_or(false);
        return if data_root.is_dir() && meta_db.is_file() && arch_ok {
            RootfsState::Ready
        } else {
            RootfsState::Corrupt
        };
    }
    if let Ok(metadata) = fs::symlink_metadata(managed_root) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return RootfsState::Corrupt;
        }
    }
    RootfsState::Missing
}

fn path_present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn directory_size(path: &Path) -> std::io::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_size(&entry.path())?);
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}

fn display_command(command: &str, args: &[String]) -> String {
    if args.is_empty() {
        command.to_string()
    } else {
        format!("{command} {}", args.join(" "))
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        encoded.push(TABLE[(b0 >> 2) as usize] as char);
        encoded.push(TABLE[(((b0 & 0b11) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(TABLE[(((b1 & 0b1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

fn decode_base64(value: &str) -> Result<Vec<u8>, MobileLinuxError> {
    fn sextet(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let bytes = value.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(MobileLinuxError::Io(
            "native PTY payload is not valid base64".to_string(),
        ));
    }
    let mut decoded = Vec::with_capacity((bytes.len() / 4) * 3);
    for chunk in bytes.chunks(4) {
        let s0 = sextet(chunk[0]).ok_or_else(|| {
            MobileLinuxError::Io("native PTY payload is not valid base64".to_string())
        })?;
        let s1 = sextet(chunk[1]).ok_or_else(|| {
            MobileLinuxError::Io("native PTY payload is not valid base64".to_string())
        })?;
        let s2 = if chunk[2] == b'=' {
            None
        } else {
            Some(sextet(chunk[2]).ok_or_else(|| {
                MobileLinuxError::Io("native PTY payload is not valid base64".to_string())
            })?)
        };
        let s3 = if chunk[3] == b'=' {
            None
        } else {
            Some(sextet(chunk[3]).ok_or_else(|| {
                MobileLinuxError::Io("native PTY payload is not valid base64".to_string())
            })?)
        };
        decoded.push((s0 << 2) | (s1 >> 4));
        if let Some(s2) = s2 {
            decoded.push(((s1 & 0b1111) << 4) | (s2 >> 2));
            if let Some(s3) = s3 {
                decoded.push(((s2 & 0b11) << 6) | s3);
            }
        }
    }
    Ok(decoded)
}

mod native {
    #[cfg(all(target_os = "ios", not(target_abi = "sim")))]
    // The device bridge is the sole unsafe boundary in this crate: it validates
    // Rust strings before crossing the C ABI and immediately copies/frees every
    // owned string returned by the native iSH shim.
    #[allow(unsafe_code)]
    mod device {
        use std::ffi::{CStr, CString};
        use std::os::raw::c_char;

        unsafe extern "C" {
            fn lingxi_ish_is_available() -> bool;
            fn lingxi_ish_availability_json() -> *mut c_char;
            fn lingxi_ish_install_rootfs_json(config_json: *const c_char) -> *mut c_char;
            fn lingxi_ish_repair_rootfs_json(config_json: *const c_char) -> *mut c_char;
            fn lingxi_ish_reset_rootfs_json(config_json: *const c_char) -> *mut c_char;
            fn lingxi_ish_boot_json(config_json: *const c_char) -> *mut c_char;
            fn lingxi_ish_configure_mounts_json(
                config_json: *const c_char,
                mounts_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_run_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_background_spawn_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_background_kill_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_background_poll_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_raw_stdio_open_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_raw_stdio_write_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_raw_stdio_read_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_raw_stdio_close_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_raw_stdio_dispose_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_probe_loopback_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_pty_open_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_pty_write_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_pty_resize_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_pty_close_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_pty_poll_json(
                config_json: *const c_char,
                request_json: *const c_char,
            ) -> *mut c_char;
            fn lingxi_ish_free_string(value: *mut c_char);
        }

        pub fn is_available() -> bool {
            // SAFETY: pure availability probe with no arguments or aliasing.
            unsafe { lingxi_ish_is_available() }
        }

        pub fn availability_json() -> Result<String, String> {
            // SAFETY: no arguments or aliasing; ownership is transferred via the returned string pointer.
            unsafe { take_owned_string(lingxi_ish_availability_json()) }
        }

        pub fn install_rootfs_json(config_json: &str) -> Result<String, String> {
            call_unary(config_json, lingxi_ish_install_rootfs_json)
        }

        pub fn repair_rootfs_json(config_json: &str) -> Result<String, String> {
            call_unary(config_json, lingxi_ish_repair_rootfs_json)
        }

        pub fn reset_rootfs_json(config_json: &str) -> Result<String, String> {
            call_unary(config_json, lingxi_ish_reset_rootfs_json)
        }

        pub fn boot_json(config_json: &str) -> Result<String, String> {
            call_unary(config_json, lingxi_ish_boot_json)
        }

        pub fn configure_mounts_json(
            config_json: &str,
            mounts_json: &str,
        ) -> Result<String, String> {
            call_binary(config_json, mounts_json, lingxi_ish_configure_mounts_json)
        }

        pub fn run_sync_json(config_json: &str, request_json: &str) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_run_json)
        }

        pub fn spawn_background_json(
            config_json: &str,
            request_json: &str,
        ) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_background_spawn_json)
        }

        pub fn kill_background_json(
            config_json: &str,
            request_json: &str,
        ) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_background_kill_json)
        }

        pub fn poll_background_json(
            config_json: &str,
            request_json: &str,
        ) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_background_poll_json)
        }

        pub fn raw_stdio_open_json(
            config_json: &str,
            request_json: &str,
        ) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_raw_stdio_open_json)
        }

        pub fn raw_stdio_write_json(
            config_json: &str,
            request_json: &str,
        ) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_raw_stdio_write_json)
        }

        pub fn raw_stdio_read_json(
            config_json: &str,
            request_json: &str,
        ) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_raw_stdio_read_json)
        }

        pub fn raw_stdio_close_json(
            config_json: &str,
            request_json: &str,
        ) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_raw_stdio_close_json)
        }

        pub fn raw_stdio_dispose_json(
            config_json: &str,
            request_json: &str,
        ) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_raw_stdio_dispose_json)
        }

        pub fn probe_loopback_json(
            config_json: &str,
            request_json: &str,
        ) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_probe_loopback_json)
        }

        pub fn pty_open_json(config_json: &str, request_json: &str) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_pty_open_json)
        }

        pub fn pty_write_json(config_json: &str, request_json: &str) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_pty_write_json)
        }

        pub fn pty_resize_json(config_json: &str, request_json: &str) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_pty_resize_json)
        }

        pub fn pty_close_json(config_json: &str, request_json: &str) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_pty_close_json)
        }

        pub fn poll_output_json(config_json: &str, request_json: &str) -> Result<String, String> {
            call_binary(config_json, request_json, lingxi_ish_pty_poll_json)
        }

        fn call_unary(
            value: &str,
            function: unsafe extern "C" fn(*const c_char) -> *mut c_char,
        ) -> Result<String, String> {
            let value = CString::new(value).map_err(|_| "payload contains NUL".to_string())?;
            // SAFETY: the CString is NUL-terminated and lives for the duration of the call.
            unsafe { take_owned_string(function(value.as_ptr())) }
        }

        fn call_binary(
            left: &str,
            right: &str,
            function: unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_char,
        ) -> Result<String, String> {
            let left = CString::new(left).map_err(|_| "payload contains NUL".to_string())?;
            let right = CString::new(right).map_err(|_| "payload contains NUL".to_string())?;
            // SAFETY: both CStrings are NUL-terminated and live for the duration of the call.
            unsafe { take_owned_string(function(left.as_ptr(), right.as_ptr())) }
        }

        unsafe fn take_owned_string(value: *mut c_char) -> Result<String, String> {
            if value.is_null() {
                return Err("native bridge returned a null string".to_string());
            }
            let owned = CStr::from_ptr(value).to_string_lossy().into_owned();
            lingxi_ish_free_string(value);
            Ok(owned)
        }
    }

    #[cfg(not(all(target_os = "ios", not(target_abi = "sim"))))]
    mod device {
        fn unavailable() -> String {
            if cfg!(target_os = "ios") {
                "native iSH runtime is unavailable on the iOS simulator".to_string()
            } else {
                "native iSH runtime is only available on physical iOS devices".to_string()
            }
        }

        pub fn is_available() -> bool {
            false
        }

        pub fn availability_json() -> Result<String, String> {
            Err(unavailable())
        }

        pub fn raw_stdio_open_json(
            _config_json: &str,
            _request_json: &str,
        ) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn raw_stdio_write_json(
            _config_json: &str,
            _request_json: &str,
        ) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn raw_stdio_read_json(
            _config_json: &str,
            _request_json: &str,
        ) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn raw_stdio_close_json(
            _config_json: &str,
            _request_json: &str,
        ) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn raw_stdio_dispose_json(
            _config_json: &str,
            _request_json: &str,
        ) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn install_rootfs_json(_config_json: &str) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn repair_rootfs_json(_config_json: &str) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn reset_rootfs_json(_config_json: &str) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn boot_json(_config_json: &str) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn configure_mounts_json(
            _config_json: &str,
            _mounts_json: &str,
        ) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn run_sync_json(_config_json: &str, _request_json: &str) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn spawn_background_json(
            _config_json: &str,
            _request_json: &str,
        ) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn kill_background_json(
            _config_json: &str,
            _request_json: &str,
        ) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn poll_background_json(
            _config_json: &str,
            _request_json: &str,
        ) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn probe_loopback_json(
            _config_json: &str,
            _request_json: &str,
        ) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn pty_open_json(_config_json: &str, _request_json: &str) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn pty_write_json(_config_json: &str, _request_json: &str) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn pty_resize_json(_config_json: &str, _request_json: &str) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn pty_close_json(_config_json: &str, _request_json: &str) -> Result<String, String> {
            Err(unavailable())
        }

        pub fn poll_output_json(_config_json: &str, _request_json: &str) -> Result<String, String> {
            Err(unavailable())
        }
    }

    pub use device::*;
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::NetworkPolicy;

    fn test_config(root: &Path) -> IosIshRuntimeConfig {
        IosIshRuntimeConfig {
            managed_root: root.join("mobile-linux"),
            app_sandbox_root: root.to_path_buf(),
            workspace_host_path: root.join("workspaces/default"),
            stable_workspace_id: "default".to_string(),
            abi: "arm64".to_string(),
            rootfs_version: "v1".to_string(),
            archive_sha256: None,
            authorization_file: None,
        }
    }

    fn command_request() -> LinuxCommandRequest {
        LinuxCommandRequest {
            command: "/bin/sh".to_string(),
            args: vec!["-lc".to_string(), "echo hi".to_string()],
            cwd: Some("/workspace/default".to_string()),
            env: BTreeMap::from([(String::from("TERM"), String::from("xterm-256color"))]),
            stdin: None,
            timeout_ms: Some(1000),
            network: NetworkPolicy::Allowed,
            resource_limits: Default::default(),
            mounts: Vec::new(),
        }
    }

    #[test]
    fn streaming_capture_is_bounded_without_truncating_utf8() {
        let mut stdout = String::new();
        append_capped_stdout(&mut stdout, &"🙂".repeat(MAX_STREAM_CAPTURE_BYTES));
        assert!(stdout.len() <= MAX_STREAM_CAPTURE_BYTES);
        assert!(std::str::from_utf8(stdout.as_bytes()).is_ok());

        let mut stderr = Vec::new();
        append_capped_bytes(&mut stderr, &vec![b'x'; MAX_STREAM_CAPTURE_BYTES * 2]);
        assert_eq!(stderr.len(), MAX_STREAM_CAPTURE_BYTES);
    }

    #[test]
    fn run_request_payload_marks_isolated_runs_as_request_only() {
        let request = command_request();
        let payload = RunRequestPayload::from_request(&request, &[], false);
        let json = serde_json::to_value(&payload).expect("serialize payload");

        assert_eq!(
            json.get("include_default_mounts")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn workspace_mount_accepts_the_managed_workspace_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");

        let mount = validate_mount(&config.default_workspace_mount(), &config).expect("mount");
        assert_eq!(
            mount.host_path,
            normalize_host_path(&config.workspace_host_path, "workspace_host_path")
                .expect("normalized workspace path")
        );
        assert_eq!(mount.guest_path, "/workspace/default");
    }

    #[test]
    fn runtime_keeps_root_on_a_protected_persistent_host_mount() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let runtime = IosIshRuntime::new(config.clone());
        let mounts = runtime.state.mounts.read().expect("mounts");
        let home = mounts
            .iter()
            .find(|mount| mount.guest_path == "/root")
            .expect("persistent home mount");
        assert_eq!(home.host_path, config.managed_root.join("persistent/root"));
        assert!(!home.read_only);
    }

    /// `current_mounts` is the table `GuestPathFileSystem` translates against;
    /// it must expose the same live state the runtime actually mounts —
    /// including the persistent `/root` bind the runtime adds on its own.
    #[test]
    fn current_mounts_exposes_the_live_workspace_and_home_table() {
        use platform_api::MobileLinuxRuntime as _;
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let runtime = IosIshRuntime::new(config.clone());
        let mounts = runtime.current_mounts();
        assert_eq!(mounts.len(), 2);
        assert_eq!(
            mounts[0].guest_path,
            platform_api::mobile_linux::guest_paths::workspace(&config.stable_workspace_id)
        );
        assert_eq!(
            mounts[1].guest_path,
            platform_api::mobile_linux::guest_paths::HOME
        );
        assert_eq!(
            mounts[1].host_path,
            config.managed_root.join("persistent/root")
        );
    }

    #[test]
    fn merged_mounts_keep_default_workspace_and_home_binds() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let runtime = IosIshRuntime::new(config.clone());
        let host_path = root
            .join("apps")
            .join("abcd1234")
            .join("build")
            .join("store");
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");
        fs::create_dir_all(&host_path).expect("create build root");

        let mounts = runtime
            .merged_mounts(&[MountSpec {
                host_path,
                guest_path: "/var/lingxi/local-app-build/abcd1234/store/project".to_string(),
                read_only: false,
                purpose: MountPurpose::LocalAppBuild,
            }])
            .expect("merged mounts");
        assert_eq!(mounts.len(), 3);
        assert!(mounts
            .iter()
            .any(|mount| mount.guest_path == "/workspace/default"));
        assert!(mounts.iter().any(|mount| mount.guest_path == "/root"));
        assert!(mounts.iter().any(|mount| {
            mount.guest_path == "/var/lingxi/local-app-build/abcd1234/store/project"
                && matches!(mount.purpose, MountPurpose::LocalAppBuild)
        }));
    }

    #[test]
    fn isolated_mounts_exclude_default_workspace_and_home_binds() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let runtime = IosIshRuntime::new(config.clone());
        let host_path = root
            .join("apps")
            .join("abcd1234")
            .join("build")
            .join("store");
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");
        fs::create_dir_all(&host_path).expect("create build root");

        let mounts = runtime
            .isolated_mounts(&[MountSpec {
                host_path,
                guest_path: "/var/lingxi/local-app-build/abcd1234/store/project".to_string(),
                read_only: false,
                purpose: MountPurpose::LocalAppBuild,
            }])
            .expect("isolated mounts");
        assert_eq!(mounts.len(), 1);
        assert_eq!(
            mounts[0].guest_path,
            "/var/lingxi/local-app-build/abcd1234/store/project"
        );
        assert!(matches!(mounts[0].purpose, MountPurpose::LocalAppBuild));
    }

    #[test]
    fn isolated_mounts_reject_empty_or_non_build_requests() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let runtime = IosIshRuntime::new(config.clone());
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");

        assert!(matches!(
            runtime.isolated_mounts(&[]),
            Err(MobileLinuxError::InvalidRequest(_))
        ));
        assert!(matches!(
            runtime.isolated_mounts(&[MountSpec {
                host_path: config.workspace_host_path.clone(),
                guest_path: config.workspace_guest_path(),
                read_only: false,
                purpose: MountPurpose::Workspace,
            }]),
            Err(MobileLinuxError::InvalidRequest(_))
        ));
    }

    #[test]
    fn workspace_mount_rejects_guest_path_escape() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");

        let error = validate_mount(
            &MountSpec {
                host_path: config.workspace_host_path.clone(),
                guest_path: "/workspace/../etc".to_string(),
                read_only: false,
                purpose: MountPurpose::Workspace,
            },
            &config,
        )
        .expect_err("mount should fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }

    #[test]
    fn request_mount_cannot_replace_persistent_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let host = root.join("external");
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");
        fs::create_dir_all(&host).expect("create external");

        let error = validate_mount(
            &MountSpec {
                host_path: host,
                guest_path: "/root".to_string(),
                read_only: false,
                purpose: MountPurpose::External,
            },
            &config,
        )
        .expect_err("persistent root override must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }

    #[test]
    fn validate_request_accepts_guest_group_memory_limit() {
        let mut request = command_request();
        request.resource_limits.max_memory_mb = Some(800);
        validate_request(&request).expect("memory watchdog is supported");

        request.resource_limits.max_memory_mb = Some(0);
        assert!(matches!(
            validate_request(&request),
            Err(MobileLinuxError::InvalidRequest(_))
        ));
    }

    #[test]
    fn local_app_build_mount_accepts_matching_build_channel_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let host_path = root
            .join("apps")
            .join("abcd1234")
            .join("build")
            .join("store");
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(host_path.clone()).expect("create build root");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");

        let mount = validate_mount(
            &MountSpec {
                host_path,
                guest_path: "/var/lingxi/local-app-build/abcd1234/store/project".to_string(),
                read_only: false,
                purpose: MountPurpose::LocalAppBuild,
            },
            &config,
        )
        .expect("local app build mount");

        assert_eq!(
            mount.guest_path,
            "/var/lingxi/local-app-build/abcd1234/store/project"
        );
    }

    #[test]
    fn local_app_build_mount_accepts_matching_staging_channel_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let host_path = root
            .join("apps")
            .join("abcd1234")
            .join("build")
            .join(".store.staging-123456789");
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(host_path.clone()).expect("create staging root");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");

        let mount = validate_mount(
            &MountSpec {
                host_path: host_path.clone(),
                guest_path: "/var/lingxi/local-app-build/abcd1234/store/project".to_string(),
                read_only: false,
                purpose: MountPurpose::LocalAppBuild,
            },
            &config,
        )
        .expect("local app staging build mount");

        assert_eq!(
            mount.host_path,
            normalize_host_path(&host_path, "staging host path").expect("normalize staging path")
        );
        assert_eq!(
            mount.guest_path,
            "/var/lingxi/local-app-build/abcd1234/store/project"
        );
    }

    #[test]
    fn local_app_build_mount_rejects_nested_dependency_mounts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let host_path = root
            .join("apps")
            .join("abcd1234")
            .join("workspace")
            .join("node_modules");
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(host_path.clone()).expect("create app dependencies");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");

        let error = validate_mount(
            &MountSpec {
                host_path,
                guest_path: "/var/lingxi/local-app-build/abcd1234/store/project/node_modules"
                    .to_string(),
                read_only: true,
                purpose: MountPurpose::External,
            },
            &config,
        )
        .expect_err("nested app build mounts must be rejected");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }

    #[test]
    fn local_app_build_mount_rejects_wrong_guest_path_or_root_escape() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let valid_host = root
            .join("apps")
            .join("abcd1234")
            .join("build")
            .join("store");
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(valid_host.clone()).expect("create build root");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");

        let wrong_guest = validate_mount(
            &MountSpec {
                host_path: valid_host.clone(),
                guest_path: "/workspace/default".to_string(),
                read_only: false,
                purpose: MountPurpose::LocalAppBuild,
            },
            &config,
        )
        .expect_err("wrong guest path must fail");
        assert!(matches!(wrong_guest, MobileLinuxError::InvalidRequest(_)));

        let outside_host = validate_mount(
            &MountSpec {
                host_path: root.join("apps").join("other").join("build").join("store"),
                guest_path: "/var/lingxi/local-app-build/abcd1234/store/project".to_string(),
                read_only: false,
                purpose: MountPurpose::LocalAppBuild,
            },
            &config,
        )
        .expect_err("outside build root must fail");
        assert!(matches!(outside_host, MobileLinuxError::InvalidRequest(_)));

        let invalid_channel = validate_mount(
            &MountSpec {
                host_path: valid_host,
                guest_path: "/var/lingxi/local-app-build/abcd1234/debug/project".to_string(),
                read_only: false,
                purpose: MountPurpose::LocalAppBuild,
            },
            &config,
        )
        .expect_err("unknown build channel must fail");
        assert!(matches!(
            invalid_channel,
            MobileLinuxError::InvalidRequest(_)
        ));
    }

    #[test]
    fn local_app_build_mount_rejects_invalid_staging_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let build_root = root.join("apps").join("abcd1234").join("build");
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(&build_root).expect("create build root");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");

        for invalid_name in [
            ".store.staging-",
            ".store.staging-not-a-number",
            ".store.staging-123-extra",
            ".full.staging-123",
            ".store.previous-123",
            "store.staging-123",
        ] {
            let invalid_host = build_root.join(invalid_name);
            fs::create_dir_all(&invalid_host).expect("create invalid staging root");
            let error = validate_mount(
                &MountSpec {
                    host_path: invalid_host,
                    guest_path: "/var/lingxi/local-app-build/abcd1234/store/project".to_string(),
                    read_only: false,
                    purpose: MountPurpose::LocalAppBuild,
                },
                &config,
            )
            .expect_err("invalid staging path must fail");
            assert!(
                matches!(error, MobileLinuxError::InvalidRequest(_)),
                "unexpected result for {invalid_name}"
            );
        }

        let wrong_app_staging = root
            .join("apps")
            .join("other")
            .join("build")
            .join(".store.staging-123");
        fs::create_dir_all(&wrong_app_staging).expect("create wrong-app staging root");
        let error = validate_mount(
            &MountSpec {
                host_path: wrong_app_staging,
                guest_path: "/var/lingxi/local-app-build/abcd1234/store/project".to_string(),
                read_only: false,
                purpose: MountPurpose::LocalAppBuild,
            },
            &config,
        )
        .expect_err("wrong-app staging path must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }

    #[cfg(unix)]
    #[test]
    fn local_app_build_mount_rejects_staging_symlink_escape() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let build_root = root.join("apps").join("abcd1234").join("build");
        let outside = temp.path().join("outside");
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(&build_root).expect("create build root");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");
        fs::create_dir_all(&outside).expect("create outside root");
        let staging_link = build_root.join(".store.staging-123");
        std::os::unix::fs::symlink(&outside, &staging_link).expect("create staging symlink");

        let error = validate_mount(
            &MountSpec {
                host_path: staging_link,
                guest_path: "/var/lingxi/local-app-build/abcd1234/store/project".to_string(),
                read_only: false,
                purpose: MountPurpose::LocalAppBuild,
            },
            &config,
        )
        .expect_err("staging symlink escape must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }

    #[cfg(unix)]
    #[test]
    fn mount_rejects_symlink_alias_into_protected_roots() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        let alias_root = root.join("aliases");
        fs::create_dir_all(config.workspace_host_path.clone()).expect("create workspace");
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::create_dir_all(config.lingxi_root()).expect("create .lingxi");
        fs::create_dir_all(&alias_root).expect("create aliases");

        let alias = alias_root.join("managed-link");
        std::os::unix::fs::symlink(config.managed_root.clone(), &alias).expect("create symlink");

        let error = validate_mount(
            &MountSpec {
                host_path: alias.join("rootfs"),
                guest_path: "/tmp/managed".to_string(),
                read_only: true,
                purpose: MountPurpose::Temp,
            },
            &config,
        )
        .expect_err("mount should fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }

    #[test]
    fn filesystem_rootfs_state_marks_invalid_active_root_as_corrupt() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let config = test_config(&root);
        fs::create_dir_all(config.managed_root.clone()).expect("create managed root");
        fs::write(config.active_root(), b"not-a-directory").expect("write active root file");

        assert_eq!(
            filesystem_rootfs_state(&config.active_root(), &config.managed_root),
            RootfsState::Corrupt
        );
    }

    #[test]
    fn native_run_envelope_maps_to_command_result() {
        let result = parse_run_response(
            r#"{"ok":true,"result":{"stdout":"Linux\n","stderr":"","exit_code":0,"timed_out":false,"cancelled":false,"network_policy_enforced":true,"memory_limit_enforced":true}}"#,
        )
        .expect("parse native result");

        assert_eq!(result.stdout, "Linux\n");
        assert_eq!(result.exit_code, 0);
        assert!(!result.timed_out);
        assert_eq!(
            result.enforcement,
            LinuxEnforcementReceipt {
                network_policy_enforced: true,
                memory_limit_enforced: true,
            }
        );
    }

    #[test]
    fn pty_closed_event_carries_and_defaults_the_guest_exit_code() {
        // Shell SELF-exit (bridge's ISHProcessExited observer): the real
        // wait-status-decoded code rides in `exit_code`.
        let events = parse_poll_events(
            r#"{"ok":true,"events":[{"sequence":7,"session_id":"s1","kind":"pty_closed","data_base64":null,"detail":null,"exit_code":3}]}"#,
        )
        .expect("parse pty_closed with exit_code");
        assert_eq!(events[0].kind, "pty_closed");
        assert_eq!(events[0].exit_code, Some(3));

        // Bridge-initiated close omits the field entirely — must default to
        // None (the reader then reports the historical code 0), not fail.
        let events = parse_poll_events(
            r#"{"ok":true,"events":[{"sequence":8,"session_id":"s1","kind":"pty_closed","data_base64":null,"detail":null}]}"#,
        )
        .expect("parse pty_closed without exit_code");
        assert_eq!(events[0].exit_code, None);
    }

    #[test]
    fn native_error_envelope_is_not_treated_as_success() {
        let error = parse_native_ok(
            r#"{"ok":false,"error":{"code":"unavailable","message":"device only"}}"#,
        )
        .expect_err("native error must propagate");

        assert!(matches!(error, MobileLinuxError::Unavailable(_)));
    }

    #[test]
    fn validate_request_accepts_enforceable_network_policies() {
        let mut request = command_request();
        request.network = NetworkPolicy::Disabled;
        validate_request(&request).expect("Disabled is supported");

        request.network = NetworkPolicy::LoopbackOnly;
        validate_request(&request).expect("LoopbackOnly is supported");
    }

    #[test]
    fn validate_request_rejects_zero_timeout_and_invalid_env_keys() {
        let mut request = command_request();
        request.timeout_ms = Some(0);
        let error = validate_request(&request).expect_err("zero timeout must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));

        request = command_request();
        request.env.insert(String::new(), String::from("value"));
        let error = validate_request(&request).expect_err("empty env key must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));

        request = command_request();
        request
            .env
            .insert(String::from("BAD=KEY"), String::from("value"));
        let error = validate_request(&request).expect_err("env key with '=' must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }

    #[test]
    fn validate_request_rejects_nul_bytes() {
        let mut request = command_request();
        request.command = "/bin/\0sh".to_string();
        let error = validate_request(&request).expect_err("NUL command must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));

        request = command_request();
        request.args.push("bad\0arg".to_string());
        let error = validate_request(&request).expect_err("NUL arg must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));

        request = command_request();
        request
            .env
            .insert(String::from("TERM"), String::from("x\0term"));
        let error = validate_request(&request).expect_err("NUL env value must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }

    #[test]
    fn validate_pty_request_rejects_invalid_env_and_nul() {
        let mut request = PtyOpenRequest {
            command: "/bin/sh".to_string(),
            args: vec!["-i".to_string()],
            cwd: Some("/workspace/default".to_string()),
            env: BTreeMap::from([(String::from("TERM"), String::from("xterm"))]),
            size: PtySize { cols: 80, rows: 24 },
            mounts: Vec::new(),
        };
        request.env.insert(String::new(), String::from("value"));
        let error = validate_pty_request(&request).expect_err("empty env key must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));

        request = PtyOpenRequest {
            command: "/bin/\0sh".to_string(),
            args: vec!["-i".to_string()],
            cwd: Some("/workspace/default".to_string()),
            env: BTreeMap::new(),
            size: PtySize { cols: 80, rows: 24 },
            mounts: Vec::new(),
        };
        let error = validate_pty_request(&request).expect_err("NUL command must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }

    #[test]
    fn capability_reports_background_contract_even_when_native_bridge_is_unavailable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let runtime = IosIshRuntime::new(test_config(&root));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        let capability = rt.block_on(runtime.probe_capability());
        assert!(capability.background_processes);
        assert!(capability.streaming_output);
        assert!(!capability.available);

        let error = rt
            .block_on(runtime.spawn_background(command_request()))
            .expect_err("background spawn requires the device bridge");
        assert!(matches!(error, MobileLinuxError::Unavailable(_)));

        let error = rt
            .block_on(runtime.kill(&LinuxProcessHandle {
                id: String::from("bg-1"),
                enforcement: LinuxEnforcementReceipt::default(),
            }))
            .expect_err("unknown background handle must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
    }

    #[test]
    fn native_background_envelopes_preserve_stream_and_terminal_state() {
        let process = parse_process_id_response(
            r#"{"ok":true,"process_id":"process-42","guest_pid":42,"network_policy_enforced":true,"memory_limit_enforced":true}"#,
        )
        .expect("parse process id");
        assert_eq!(process.process_id, "process-42");
        assert_eq!(
            process.enforcement,
            LinuxEnforcementReceipt {
                network_policy_enforced: true,
                memory_limit_enforced: true,
            }
        );
        assert!(parse_background_kill_response(
            r#"{"ok":true,"process_id":"process-42","termination_requested":true}"#
        )
        .expect("parse requested termination"));
        assert!(!parse_background_kill_response(
            r#"{"ok":true,"process_id":"process-42","already_stopped":true}"#
        )
        .expect("parse idempotent termination"));

        let events = parse_background_events(
            r#"{"ok":true,"events":[{"sequence":7,"kind":"stdout_line","line":"ready","data_base64":null,"exit_code":null,"cancelled":null,"detail":null},{"sequence":8,"kind":"stderr_chunk","line":null,"data_base64":"d2Fybg==","exit_code":null,"cancelled":null,"detail":null},{"sequence":9,"kind":"process_exited","line":null,"data_base64":null,"exit_code":143,"cancelled":true,"detail":"terminated"}]}"#,
        )
        .expect("parse background events");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].line.as_deref(), Some("ready"));
        assert_eq!(
            decode_base64(events[1].data_base64.as_deref().unwrap()).unwrap(),
            b"warn"
        );
        assert_eq!(events[2].exit_code, Some(143));
        assert_eq!(events[2].cancelled, Some(true));

        let (status, exit_code, detail) = background_terminal_state(false, &events[2]);
        assert_eq!(status, MobileLinuxTaskStatus::Cancelled);
        assert_eq!(exit_code, Some(143));
        assert_eq!(detail.as_deref(), Some("terminated"));

        let completed = NativeBackgroundEventPayload {
            sequence: 10,
            kind: "process_exited".to_string(),
            line: None,
            data_base64: None,
            exit_code: Some(0),
            cancelled: Some(false),
            detail: None,
        };
        assert_eq!(
            background_terminal_state(false, &completed).0,
            MobileLinuxTaskStatus::Completed
        );
        assert_eq!(
            background_terminal_state(true, &completed).0,
            MobileLinuxTaskStatus::Cancelled
        );
    }

    #[test]
    fn native_raw_stdio_envelopes_preserve_bytes_and_enforcement() {
        let (session, enforcement) = parse_raw_stdio_open_response(
            r#"{"ok":true,"session_id":"raw-1","network_policy_enforced":true,"memory_limit_enforced":true}"#,
        )
        .expect("parse raw stdio open");
        assert_eq!(session, "raw-1");
        assert_eq!(
            enforcement,
            LinuxEnforcementReceipt {
                network_policy_enforced: true,
                memory_limit_enforced: true,
            }
        );

        let chunk = parse_raw_stdio_read_response(
            r#"{"ok":true,"stdout_base64":"Q29udGVudC1MZW5ndGg6IDINCg0Ke30=","stderr_base64":"d2Fybg==","closed":true,"exit_code":0,"error":null}"#,
        )
        .expect("parse raw stdio read");
        assert_eq!(chunk.stdout, b"Content-Length: 2\r\n\r\n{}");
        assert_eq!(chunk.stderr, b"warn");
        assert!(chunk.closed);
        assert_eq!(chunk.exit_code, Some(0));
    }

    #[test]
    fn simulator_raw_stdio_fails_closed_without_using_pty() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let runtime = IosIshRuntime::new(test_config(&root));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let error = rt
            .block_on(runtime.open_raw_stdio(RawStdioOpenRequest {
                command: "/opt/lingxi/toolchains/typescript/7.0.2/tsc".to_string(),
                args: vec!["--lsp".to_string(), "--stdio".to_string()],
                cwd: None,
                env: BTreeMap::new(),
                network: NetworkPolicy::Disabled,
                resource_limits: platform_api::ResourceLimits {
                    max_memory_mb: Some(384),
                    ..platform_api::ResourceLimits::default()
                },
                mounts: Vec::new(),
            }))
            .expect_err("non-device builds must reject raw stdio");
        assert!(matches!(error, MobileLinuxError::Unavailable(_)));
        assert!(runtime.state.pty.lock().expect("pty mutex").is_none());
    }

    #[test]
    fn loopback_probe_requires_reachable_field() {
        assert!(parse_loopback_probe(r#"{"ok":true,"reachable":true}"#).unwrap());
        let error = parse_loopback_probe(r#"{"ok":true}"#).expect_err("reachable is required");
        assert!(matches!(error, MobileLinuxError::Io(_)));
    }
}
