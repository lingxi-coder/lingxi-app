//! iOS iSH-backed implementation of the shared mobile-linux runtime seam.
//!
//! The native integration is intentionally isolated to `native` so ABI drift is
//! contained to one module. Host and simulator builds compile a safe
//! unavailable stub rather than trying to link the device-only symbols.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::spawn_blocking;
use traits::{
    LinuxCommandRequest, LinuxCommandResult, LinuxProcessHandle, MobileLinuxCapability,
    MobileLinuxError, MobileLinuxEvent, MobileLinuxEventKind, MobileLinuxRuntime,
    MobileLinuxRuntimeMode, MobileLinuxTaskSnapshot, MobileLinuxTaskStatus, MountPurpose,
    MountSpec, PtyOpenRequest, PtySessionHandle, PtySize, RootfsState, RootfsStatus,
    SandboxBackend,
};

const MAX_EVENTS: usize = 4096;
const PTY_IDLE_POLL: Duration = Duration::from_millis(25);
const LINGXI_DOT_DIR: &str = ".lingxi";

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
        format!("/workspace/{}", self.stable_workspace_id)
    }

    fn default_workspace_mount(&self) -> MountSpec {
        MountSpec {
            host_path: self.workspace_host_path.clone(),
            guest_path: self.workspace_guest_path(),
            read_only: false,
            purpose: MountPurpose::Workspace,
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
        Self {
            state: Arc::new(RuntimeState {
                config,
                mounts: RwLock::new(vec![workspace_mount]),
                tasks: Mutex::new(HashMap::new()),
                events: Mutex::new(VecDeque::new()),
                next_id: AtomicU64::new(1),
                next_sequence: AtomicU64::new(1),
                booted: AtomicBool::new(false),
                pty: Mutex::new(None),
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
                "/root".to_string(),
                "/tmp".to_string(),
                "/var/tmp".to_string(),
                self.state.config.workspace_guest_path(),
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
    ) -> Result<LinuxCommandResult, MobileLinuxError> {
        let config_json = self.native_config_json()?;
        let payload = RunRequestPayload::from_request(request, mounts);
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

    async fn run_inner(
        &self,
        request: LinuxCommandRequest,
        lifecycle_status: MobileLinuxTaskStatus,
    ) -> Result<(LinuxCommandResult, String, Arc<TaskControl>), MobileLinuxError> {
        validate_request(&request)?;
        self.boot().await?;
        let mounts = self.merged_mounts(&request.mounts)?;
        self.apply_mounts(&mounts).await?;
        let (task_id, task) = self.create_task(
            if matches!(lifecycle_status, MobileLinuxTaskStatus::Backgrounded) {
                "bg"
            } else {
                "task"
            },
            display_command(&request.command, &request.args),
            lifecycle_status,
        );
        match self.run_native_request(&request, &mounts).await {
            Ok(result) => {
                self.emit_result_frames(&task_id, &result);
                Ok((result, task_id, task))
            }
            Err(error) => {
                self.emit_runtime_error(Some(task_id.clone()), error.to_string());
                self.finish_task(
                    &task_id,
                    &task,
                    MobileLinuxTaskStatus::Failed,
                    None,
                    Some(error.to_string()),
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
                        traits::mobile_linux::MAX_MOBILE_LINUX_EVENT_BATCH as u32,
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
                                runtime.emit_runtime_error(
                                    Some(session_id.clone()),
                                    format!(
                                        "ignoring PTY event for unexpected session {}",
                                        event.session_id
                                    ),
                                );
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
                                    runtime.finish_task(
                                        &session_id,
                                        &control.task,
                                        MobileLinuxTaskStatus::Completed,
                                        Some(0),
                                        event.detail.clone(),
                                    );
                                    runtime.emit_pty_closed_once(
                                        &session_id,
                                        &control,
                                        Some(0),
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
            streaming_output: false,
            background_processes: false,
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
            let _ = self.kill(&LinuxProcessHandle { id }).await;
        }
        self.state.booted.store(false, Ordering::Release);
        Ok(())
    }

    async fn run(
        &self,
        request: LinuxCommandRequest,
    ) -> Result<LinuxCommandResult, MobileLinuxError> {
        let (result, task_id, task) = self
            .run_inner(request, MobileLinuxTaskStatus::Running)
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

    async fn spawn_background(
        &self,
        _request: LinuxCommandRequest,
    ) -> Result<LinuxProcessHandle, MobileLinuxError> {
        Err(MobileLinuxError::Unsupported)
    }

    async fn kill(&self, _handle: &LinuxProcessHandle) -> Result<(), MobileLinuxError> {
        Err(MobileLinuxError::Unsupported)
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
        self.native_close_pty(&handle.id).await?;
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

    async fn configure_mounts(&self, mounts: Vec<MountSpec>) -> Result<(), MobileLinuxError> {
        let mut normalized = Vec::with_capacity(mounts.len().saturating_add(1));
        normalized.push(validate_mount(
            &self.state.config.default_workspace_mount(),
            &self.state.config,
        )?);
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
            .take(limit.min(traits::mobile_linux::MAX_MOBILE_LINUX_EVENT_BATCH))
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
    mounts: Vec<MountPayload>,
}

impl RunRequestPayload {
    fn from_request(request: &LinuxCommandRequest, mounts: &[MountSpec]) -> Self {
        Self {
            command: request.command.clone(),
            args: request.args.clone(),
            cwd: request.cwd.clone(),
            env: request.env.clone(),
            stdin: request.stdin.clone(),
            timeout_ms: request.timeout_ms,
            network: match request.network {
                traits::NetworkPolicy::Disabled => "disabled",
                traits::NetworkPolicy::LoopbackOnly => "loopback-only",
                traits::NetworkPolicy::Allowed => "allowed",
            },
            mounts: mounts.iter().map(MountPayload::from_mount).collect(),
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

#[derive(Debug, Clone, Deserialize)]
struct NativePtyEventPayload {
    sequence: u64,
    session_id: String,
    kind: String,
    data_base64: Option<String>,
    detail: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NativePollEnvelope {
    ok: bool,
    error: Option<NativeErrorPayload>,
    #[serde(default)]
    events: Vec<NativePtyEventPayload>,
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
    if matches!(
        request.network,
        traits::NetworkPolicy::Disabled | traits::NetworkPolicy::LoopbackOnly
    ) {
        return Err(MobileLinuxError::InvalidRequest(
            "ios-ish runtime requires NetworkPolicy::Allowed; Disabled and LoopbackOnly are not enforced by the native bridge"
                .to_string(),
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
            return Err(MobileLinuxError::InvalidRequest(
                "workspace mount host_path must match workspace_host_path".to_string(),
            ));
        }
    } else if mount.guest_path == config.workspace_guest_path() {
        return Err(MobileLinuxError::InvalidRequest(
            "only workspace mounts may target the managed workspace guest path".to_string(),
        ));
    }

    Ok(MountSpec {
        host_path,
        guest_path: mount.guest_path.clone(),
        read_only: mount.read_only,
        purpose: mount.purpose,
    })
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
    use traits::NetworkPolicy;

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
            mounts: Vec::new(),
        }
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
            r#"{"ok":true,"result":{"stdout":"Linux\n","stderr":"","exit_code":0,"timed_out":false,"cancelled":false}}"#,
        )
        .expect("parse native result");

        assert_eq!(result.stdout, "Linux\n");
        assert_eq!(result.exit_code, 0);
        assert!(!result.timed_out);
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
    fn validate_request_rejects_unenforced_network_policies() {
        let mut request = command_request();
        request.network = NetworkPolicy::Disabled;
        let error = validate_request(&request).expect_err("disabled network must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));

        request.network = NetworkPolicy::LoopbackOnly;
        let error = validate_request(&request).expect_err("loopback-only network must fail");
        assert!(matches!(error, MobileLinuxError::InvalidRequest(_)));
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
    fn capability_and_background_apis_report_no_background_support() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("app");
        let runtime = IosIshRuntime::new(test_config(&root));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        let capability = rt.block_on(runtime.probe_capability());
        assert!(!capability.background_processes);

        let error = rt
            .block_on(runtime.spawn_background(command_request()))
            .expect_err("background spawn must be unsupported");
        assert!(matches!(error, MobileLinuxError::Unsupported));

        let error = rt
            .block_on(runtime.kill(&LinuxProcessHandle {
                id: String::from("bg-1"),
            }))
            .expect_err("background kill must be unsupported");
        assert!(matches!(error, MobileLinuxError::Unsupported));
    }
}
