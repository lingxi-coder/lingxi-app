#[cfg(feature = "uniffi")]
use super::configuration::{
    mobile_linux_authorization_verified, resolve_mobile_linux_app_sandbox_root,
    validate_mobile_linux_workspace_config, IosMobileLinuxConfigFfi,
};
#[cfg(feature = "uniffi")]
use super::linux_conversion::{
    capability_to_ffi, command_request_from_ffi, command_result_to_ffi, event_to_ffi,
    mobile_linux_error_to_ffi, mount_spec_from_ffi, pty_request_from_ffi, status_to_ffi,
    task_snapshot_to_ffi,
};
#[cfg(feature = "uniffi")]
use super::linux_types::MobileLinuxOperationFfiError;
#[cfg(feature = "uniffi")]
use super::linux_types::{
    IosMobileLinuxEventSink, MobileLinuxCapabilityFfi, MobileLinuxCommandRequestFfi,
    MobileLinuxCommandResultFfi, MobileLinuxMountSpecFfi, MobileLinuxPtyOpenRequestFfi,
    MobileLinuxPtySessionFfi, MobileLinuxRootfsStateFfi, MobileLinuxRuntimeModeFfi,
    MobileLinuxStatusFfi, MobileLinuxStreamEventFfi, MobileLinuxStreamEventKindFfi,
    MobileLinuxStreamSourceFfi, MobileLinuxTaskFfi, MobileLinuxTaskStateFfi,
};
#[cfg(feature = "uniffi")]
use super::mobile_linux_sdk;
#[cfg(feature = "uniffi")]
use mobile_linux_api::MAX_MOBILE_LINUX_EVENT_BATCH;
use std::sync::Arc;

/// Preserve the raw journal watermark when lifecycle events have no stream
/// representation. The payload-free reserved `runtime` event only advances a
/// reader's cursor; terminal consumers ignore that stream and render no text.
/// One bounded SDK page is read per call, with no public FFI DTO/ABI change.
#[cfg(feature = "uniffi")]
pub(super) async fn read_stream_event_page<F, Fut>(
    after_sequence: Option<u64>,
    limit: usize,
    read_page: F,
) -> Result<Vec<MobileLinuxStreamEventFfi>, mobile_linux_api::MobileLinuxError>
where
    F: FnOnce(Option<u64>, usize) -> Fut,
    Fut: std::future::Future<
        Output = Result<
            Vec<mobile_linux_api::MobileLinuxEvent>,
            mobile_linux_api::MobileLinuxError,
        >,
    >,
{
    let limit = limit.min(MAX_MOBILE_LINUX_EVENT_BATCH);
    if limit == 0 {
        return Ok(Vec::new());
    }
    let page = read_page(after_sequence, limit).await?;
    let Some(newest_sequence) = page.iter().map(|event| event.sequence).max() else {
        return Ok(Vec::new());
    };
    let mut events: Vec<_> = page.into_iter().filter_map(event_to_ffi).collect();
    if events.iter().all(|event| event.sequence < newest_sequence) {
        // At least one raw event was filtered, so this marker still fits the
        // requested page limit. It must never signal an exit or an error.
        events.push(MobileLinuxStreamEventFfi {
            sequence: newest_sequence,
            task_id: None,
            stream_id: "runtime".to_string(),
            source: MobileLinuxStreamSourceFfi::Run,
            kind: MobileLinuxStreamEventKindFfi::StdoutLine,
            text: None,
            data: None,
            exit_code: None,
            timed_out: false,
        });
    }
    Ok(events)
}

#[cfg(feature = "uniffi")]
pub(super) fn ios_mobile_linux_status_from_config(
    config: Option<&IosMobileLinuxConfigFfi>,
) -> MobileLinuxStatusFfi {
    match config {
        None => MobileLinuxStatusFfi {
            state: MobileLinuxRootfsStateFfi::Unsupported,
            backend: "ios-posix".to_string(),
            mode: MobileLinuxRuntimeModeFfi::Legacy,
            platform: "ios".to_string(),
            abi: "unknown".to_string(),
            version: None,
            managed_root: None,
            active_root: None,
            staged_root: None,
            archive_sha256: None,
            installed_size_bytes: None,
            writable_guest_paths: vec![],
            last_error: Some("legacy unavailable backend selected".to_string()),
        },
        Some(cfg) if matches!(cfg.mode, MobileLinuxRuntimeModeFfi::Legacy) => {
            MobileLinuxStatusFfi {
                state: MobileLinuxRootfsStateFfi::Unsupported,
                backend: "ios-posix".to_string(),
                mode: MobileLinuxRuntimeModeFfi::Legacy,
                platform: "ios".to_string(),
                abi: cfg.abi.clone(),
                version: Some(cfg.rootfs_version.clone()),
                managed_root: Some(cfg.managed_root.clone()),
                active_root: None,
                staged_root: None,
                archive_sha256: cfg.archive_sha256.clone(),
                installed_size_bytes: None,
                writable_guest_paths: vec![
                    "/root".to_string(),
                    "/tmp".to_string(),
                    "/var/tmp".to_string(),
                    "/workspace".to_string(),
                ],
                last_error: Some("legacy unavailable backend selected".to_string()),
            }
        }
        Some(cfg) => match ios_mobile_linux_runtime(Some(cfg)) {
            Some(runtime) => {
                let probe_rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("ios mobile-linux status runtime");
                match probe_rt.block_on(runtime.rootfs_status()) {
                    Ok(status) => status_to_ffi(status),
                    Err(error) => fallback_ios_mobile_linux_status(cfg, Some(error.to_string())),
                }
            }
            None => fallback_ios_mobile_linux_status(cfg, None),
        },
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn fallback_ios_mobile_linux_status(
    cfg: &IosMobileLinuxConfigFfi,
    override_error: Option<String>,
) -> MobileLinuxStatusFfi {
    let auth_present = mobile_linux_authorization_verified(cfg.authorization_file.as_ref());
    let (state, message) = if auth_present {
        (
            MobileLinuxRootfsStateFfi::Unsupported,
            "authorization present, but iSH runtime is not linked in this build",
        )
    } else {
        (
            MobileLinuxRootfsStateFfi::BlockedByLicense,
            "missing additional written authorization for PRoot/iSH redistribution",
        )
    };
    MobileLinuxStatusFfi {
        state,
        backend: "ios-ish".to_string(),
        mode: MobileLinuxRuntimeModeFfi::MobileLinux,
        platform: "ios".to_string(),
        abi: cfg.abi.clone(),
        version: Some(cfg.rootfs_version.clone()),
        managed_root: Some(cfg.managed_root.clone()),
        active_root: None,
        staged_root: None,
        archive_sha256: cfg.archive_sha256.clone(),
        installed_size_bytes: None,
        writable_guest_paths: vec![
            "/root".to_string(),
            "/tmp".to_string(),
            "/var/tmp".to_string(),
            "/workspace".to_string(),
        ],
        last_error: Some(override_error.unwrap_or_else(|| message.to_string())),
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn ios_mobile_linux_runtime(
    config: Option<&IosMobileLinuxConfigFfi>,
) -> Option<Arc<dyn mobile_linux_api::MobileLinuxRuntime>> {
    let cfg = config?;
    if matches!(cfg.mode, MobileLinuxRuntimeModeFfi::Legacy) {
        return None;
    }
    if let Some(runtime) = linked_ios_mobile_linux_runtime(cfg) {
        return Some(runtime);
    }
    let auth_present = mobile_linux_authorization_verified(cfg.authorization_file.as_ref());
    let runtime = if auth_present {
        mobile_linux_api::UnavailableMobileLinuxRuntime::unavailable(
            mobile_linux_api::SandboxBackend::IosIsh,
            mobile_linux_api::MobileLinuxRuntimeMode::MobileLinux,
            "ios",
            cfg.abi.clone(),
            "authorization present, but iSH runtime is not linked in this build",
        )
    } else {
        mobile_linux_api::UnavailableMobileLinuxRuntime::blocked(
            mobile_linux_api::SandboxBackend::IosIsh,
            mobile_linux_api::MobileLinuxRuntimeMode::MobileLinux,
            "ios",
            cfg.abi.clone(),
            "missing additional written authorization for PRoot/iSH redistribution",
        )
    };
    Some(Arc::new(runtime) as Arc<dyn mobile_linux_api::MobileLinuxRuntime>)
}

#[cfg(feature = "uniffi")]
pub(super) fn linked_ios_mobile_linux_runtime(
    cfg: &IosMobileLinuxConfigFfi,
) -> Option<Arc<dyn mobile_linux_api::MobileLinuxRuntime>> {
    let app_sandbox_root = resolve_mobile_linux_app_sandbox_root(cfg).ok()?;
    let (workspace_host_path, stable_workspace_id) =
        validate_mobile_linux_workspace_config(app_sandbox_root.to_string_lossy().as_ref(), cfg)
            .ok()?;
    let runtime = mobile_linux_sdk::linked_runtime(mobile_linux_sdk::IosIshRuntimeConfig {
        managed_root: std::path::PathBuf::from(&cfg.managed_root),
        app_sandbox_root,
        workspace_host_path,
        stable_workspace_id,
        abi: cfg.abi.clone(),
        rootfs_version: cfg.rootfs_version.clone(),
        archive_sha256: cfg.archive_sha256.clone(),
        authorization_file: cfg.authorization_file.clone(),
    });
    Some(runtime.unwrap_or_else(|error| {
        Arc::new(
            mobile_linux_api::UnavailableMobileLinuxRuntime::unavailable(
                mobile_linux_api::SandboxBackend::IosIsh,
                mobile_linux_api::MobileLinuxRuntimeMode::MobileLinux,
                "ios",
                cfg.abi.clone(),
                error.to_string(),
            ),
        )
    }))
}

#[cfg(feature = "uniffi")]
pub(super) async fn unavailable_runtime_error(
    runtime: Arc<dyn mobile_linux_api::MobileLinuxRuntime>,
) -> MobileLinuxOperationFfiError {
    match runtime.rootfs_status().await {
        Ok(status)
            if matches!(
                status.state,
                mobile_linux_api::RootfsState::BlockedByLicense
            ) =>
        {
            MobileLinuxOperationFfiError::LicenseBlocked {
                message: status
                    .last_error
                    .unwrap_or_else(|| "mobile-linux runtime blocked by license gate".to_string()),
            }
        }
        Ok(status) => MobileLinuxOperationFfiError::Unavailable {
            message: status
                .last_error
                .unwrap_or_else(|| "mobile-linux runtime unavailable".to_string()),
        },
        Err(error) => mobile_linux_error_to_ffi(error),
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn probe_runtime(
    config: Option<&IosMobileLinuxConfigFfi>,
) -> Option<Arc<dyn mobile_linux_api::MobileLinuxRuntime>> {
    ios_mobile_linux_runtime(config)
}

/// Probe the iOS mobile-linux bridge without constructing the full engine.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn probe_ios_mobile_linux(config: Option<IosMobileLinuxConfigFfi>) -> MobileLinuxCapabilityFfi {
    match ios_mobile_linux_runtime(config.as_ref()) {
        Some(runtime) => {
            let probe_rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("ios mobile-linux probe runtime");
            capability_to_ffi(probe_rt.block_on(runtime.probe_capability()))
        }
        None => capability_to_ffi(mobile_linux_api::MobileLinuxCapability {
            available: false,
            backend: mobile_linux_api::SandboxBackend::IosIsh,
            mode: mobile_linux_api::MobileLinuxRuntimeMode::Legacy,
            reason: Some("legacy unavailable backend selected".to_string()),
            streaming_output: false,
            background_processes: false,
            pty: false,
            bind_mounts: false,
            rootfs_integrity: false,
        }),
    }
}

/// Inspect the iOS mobile-linux rootfs state without constructing the full engine.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn ios_mobile_linux_status(config: Option<IosMobileLinuxConfigFfi>) -> MobileLinuxStatusFfi {
    ios_mobile_linux_status_from_config(config.as_ref())
}

/// Phase-1 verify action: returns the current computed iOS mobile-linux status.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn verify_ios_mobile_linux(config: Option<IosMobileLinuxConfigFfi>) -> MobileLinuxStatusFfi {
    ios_mobile_linux_status_from_config(config.as_ref())
}

/// Phase-1 repair action: returns the current computed iOS mobile-linux status.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn repair_ios_mobile_linux(config: Option<IosMobileLinuxConfigFfi>) -> MobileLinuxStatusFfi {
    ios_mobile_linux_status_from_config(config.as_ref())
}

/// Phase-1 reset action: returns the current computed iOS mobile-linux status.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn reset_ios_mobile_linux(config: Option<IosMobileLinuxConfigFfi>) -> MobileLinuxStatusFfi {
    ios_mobile_linux_status_from_config(config.as_ref())
}

#[cfg(feature = "uniffi")]
pub(super) struct MobileLinuxStreamSinkBridge {
    pub(super) stream_id: String,
    pub(super) source: MobileLinuxStreamSourceFfi,
    pub(super) inner: Box<dyn IosMobileLinuxEventSink>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl mobile_linux_api::ProcessStreamSink for MobileLinuxStreamSinkBridge {
    async fn stdout_line(&self, line: String) -> Result<(), mobile_linux_api::ProcessError> {
        self.inner
            .on_event(MobileLinuxStreamEventFfi {
                sequence: 0,
                task_id: None,
                stream_id: self.stream_id.clone(),
                source: self.source,
                kind: MobileLinuxStreamEventKindFfi::StdoutLine,
                text: Some(line),
                data: None,
                exit_code: None,
                timed_out: false,
            })
            .await
            .map_err(|err| mobile_linux_api::ProcessError::Io(err.to_string()))
    }

    async fn stderr_chunk(&self, chunk: Vec<u8>) -> Result<(), mobile_linux_api::ProcessError> {
        self.inner
            .on_event(MobileLinuxStreamEventFfi {
                sequence: 0,
                task_id: None,
                stream_id: self.stream_id.clone(),
                source: self.source,
                kind: MobileLinuxStreamEventKindFfi::StderrChunk,
                text: Some(String::from_utf8_lossy(&chunk).into_owned()),
                data: Some(chunk),
                exit_code: None,
                timed_out: false,
            })
            .await
            .map_err(|err| mobile_linux_api::ProcessError::Io(err.to_string()))
    }
}

#[cfg_attr(feature = "uniffi", derive(uniffi::Object))]
pub struct IosMobileLinuxRuntimeHandle {
    pub(super) runtime: Arc<dyn mobile_linux_api::MobileLinuxRuntime>,
}

impl IosMobileLinuxRuntimeHandle {
    pub(super) fn new(runtime: Arc<dyn mobile_linux_api::MobileLinuxRuntime>) -> Self {
        Self { runtime }
    }

    pub(super) async fn availability_error(&self) -> MobileLinuxOperationFfiError {
        unavailable_runtime_error(self.runtime.clone()).await
    }
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn create_ios_mobile_linux_runtime(
    config: IosMobileLinuxConfigFfi,
) -> Result<Arc<IosMobileLinuxRuntimeHandle>, MobileLinuxOperationFfiError> {
    if matches!(config.mode, MobileLinuxRuntimeModeFfi::Legacy) {
        return Err(MobileLinuxOperationFfiError::Unavailable {
            message: "legacy unavailable backend selected".to_string(),
        });
    }
    let app_sandbox_root = resolve_mobile_linux_app_sandbox_root(&config)?;
    let _ = validate_mobile_linux_workspace_config(
        app_sandbox_root.to_string_lossy().as_ref(),
        &config,
    )?;
    let runtime =
        probe_runtime(Some(&config)).ok_or_else(|| MobileLinuxOperationFfiError::Unavailable {
            message: "legacy unavailable backend selected".to_string(),
        })?;
    Ok(Arc::new(IosMobileLinuxRuntimeHandle::new(runtime)))
}

#[cfg_attr(feature = "uniffi", uniffi::export(async_runtime = "tokio"))]
impl IosMobileLinuxRuntimeHandle {
    pub async fn capability(&self) -> MobileLinuxCapabilityFfi {
        capability_to_ffi(self.runtime.probe_capability().await)
    }

    pub async fn status(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .rootfs_status()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn verify_rootfs(
        &self,
    ) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .verify_rootfs()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn repair_rootfs(
        &self,
    ) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .repair_rootfs()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn reset_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .reset_rootfs()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn boot(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .boot()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn shutdown(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .shutdown()
            .await
            .map_err(mobile_linux_error_to_ffi)?;
        self.runtime
            .rootfs_status()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn run_command(
        &self,
        request: MobileLinuxCommandRequestFfi,
    ) -> Result<MobileLinuxCommandResultFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .run(command_request_from_ffi(request))
            .await
            .map(command_result_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn run_command_streaming(
        &self,
        request: MobileLinuxCommandRequestFfi,
        sink: Box<dyn IosMobileLinuxEventSink>,
    ) -> Result<MobileLinuxCommandResultFfi, MobileLinuxOperationFfiError> {
        let stream_id = format!(
            "run-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_millis())
                .unwrap_or(0)
        );
        let bridge = Arc::new(MobileLinuxStreamSinkBridge {
            stream_id: stream_id.clone(),
            source: MobileLinuxStreamSourceFfi::Run,
            inner: sink,
        });
        match self
            .runtime
            .run_streaming(command_request_from_ffi(request), bridge.clone())
            .await
        {
            Ok(result) => Ok(command_result_to_ffi(result)),
            Err(error) => {
                let detail = error.to_string();
                let _ = bridge
                    .inner
                    .on_event(MobileLinuxStreamEventFfi {
                        sequence: 0,
                        task_id: None,
                        stream_id,
                        source: MobileLinuxStreamSourceFfi::Run,
                        kind: MobileLinuxStreamEventKindFfi::Error,
                        text: Some(detail.clone()),
                        data: None,
                        exit_code: None,
                        timed_out: matches!(error, mobile_linux_api::MobileLinuxError::Timeout),
                    })
                    .await;
                Err(mobile_linux_error_to_ffi(error))
            }
        }
    }

    pub async fn spawn_task(
        &self,
        request: MobileLinuxCommandRequestFfi,
    ) -> Result<MobileLinuxTaskFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .spawn_background(command_request_from_ffi(request))
            .await
            .map(|handle| MobileLinuxTaskFfi {
                id: handle.id,
                title: "guest-command".to_string(),
                state: MobileLinuxTaskStateFfi::Running,
                detail: None,
            })
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn list_tasks(
        &self,
    ) -> Result<Vec<MobileLinuxTaskFfi>, MobileLinuxOperationFfiError> {
        let capability = self.runtime.probe_capability().await;
        if !capability.available {
            return Err(self.availability_error().await);
        }
        self.runtime
            .list_tasks()
            .await
            .map(|tasks| tasks.into_iter().map(task_snapshot_to_ffi).collect())
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn task_status(
        &self,
        task_id: String,
    ) -> Result<Option<MobileLinuxTaskFfi>, MobileLinuxOperationFfiError> {
        let capability = self.runtime.probe_capability().await;
        if !capability.available {
            return Err(self.availability_error().await);
        }
        self.runtime
            .task_status(&task_id)
            .await
            .map(|task| task.map(task_snapshot_to_ffi))
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn read_events(
        &self,
        after_sequence: Option<u64>,
        limit: Option<u32>,
    ) -> Result<Vec<MobileLinuxStreamEventFfi>, MobileLinuxOperationFfiError> {
        let capability = self.runtime.probe_capability().await;
        if !capability.available {
            return Err(self.availability_error().await);
        }
        read_stream_event_page(
            after_sequence,
            limit.unwrap_or(MAX_MOBILE_LINUX_EVENT_BATCH as u32) as usize,
            |cursor, page_limit| self.runtime.read_events(cursor, page_limit),
        )
        .await
        .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn kill_task(
        &self,
        task_id: String,
    ) -> Result<MobileLinuxTaskFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .kill(&mobile_linux_api::LinuxProcessHandle {
                id: task_id.clone(),
                enforcement: mobile_linux_api::LinuxEnforcementReceipt::default(),
            })
            .await
            .map_err(mobile_linux_error_to_ffi)?;
        Ok(MobileLinuxTaskFfi {
            id: task_id,
            title: "guest-command".to_string(),
            state: MobileLinuxTaskStateFfi::Cancelled,
            detail: Some("cancelled".to_string()),
        })
    }

    pub async fn open_pty(
        &self,
        request: MobileLinuxPtyOpenRequestFfi,
    ) -> Result<MobileLinuxPtySessionFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .open_pty(pty_request_from_ffi(request))
            .await
            .map(|handle| MobileLinuxPtySessionFfi {
                id: handle.id,
                available: true,
                detail: None,
            })
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn write_pty(
        &self,
        session_id: String,
        data: Vec<u8>,
    ) -> Result<(), MobileLinuxOperationFfiError> {
        self.runtime
            .write_pty(&mobile_linux_api::PtySessionHandle { id: session_id }, data)
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn resize_pty(
        &self,
        session_id: String,
        cols: u16,
        rows: u16,
    ) -> Result<(), MobileLinuxOperationFfiError> {
        self.runtime
            .resize_pty(
                &mobile_linux_api::PtySessionHandle { id: session_id },
                mobile_linux_api::PtySize { cols, rows },
            )
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn close_pty(&self, session_id: String) -> Result<(), MobileLinuxOperationFfiError> {
        self.runtime
            .close_pty(&mobile_linux_api::PtySessionHandle { id: session_id })
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn configure_mounts(
        &self,
        mounts: Vec<MobileLinuxMountSpecFfi>,
    ) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .configure_mounts(mounts.into_iter().map(mount_spec_from_ffi).collect())
            .await
            .map_err(mobile_linux_error_to_ffi)?;
        self.runtime
            .rootfs_status()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }
}

#[cfg(all(test, feature = "uniffi"))]
mod event_paging_tests {
    use super::*;
    use mobile_linux_api::{MobileLinuxEvent, MobileLinuxEventKind, MobileLinuxTaskStatus};

    fn lifecycle(sequence: u64) -> MobileLinuxEvent {
        MobileLinuxEvent {
            sequence,
            task_id: Some("task-1".to_string()),
            kind: MobileLinuxEventKind::TaskStatusChanged {
                status: MobileLinuxTaskStatus::Running,
                exit_code: None,
                detail: None,
            },
        }
    }

    fn output(sequence: u64) -> MobileLinuxEvent {
        MobileLinuxEvent {
            sequence,
            task_id: Some("pty-1".to_string()),
            kind: MobileLinuxEventKind::PtyOutput {
                session_id: "pty-1".to_string(),
                data: b"ready\n".to_vec(),
            },
        }
    }

    fn journal_page(
        journal: &[MobileLinuxEvent],
        after: Option<u64>,
        limit: usize,
    ) -> Vec<MobileLinuxEvent> {
        journal
            .iter()
            .filter(|event| event.sequence > after.unwrap_or(0))
            .take(limit)
            .cloned()
            .collect()
    }

    fn assert_watermark(event: &MobileLinuxStreamEventFfi, sequence: u64) {
        assert_eq!(event.sequence, sequence);
        assert_eq!(event.stream_id, "runtime");
        assert!(matches!(
            event.kind,
            MobileLinuxStreamEventKindFfi::StdoutLine
        ));
        assert!(event.task_id.is_none());
        assert!(event.text.is_none());
        assert!(event.data.is_none());
        assert!(event.exit_code.is_none());
        assert!(!event.timed_out);
    }

    #[tokio::test]
    async fn mobile_linux_event_paging_limit_one_reaches_pty_output_on_next_read() {
        let journal = vec![lifecycle(1), output(2)];
        let mut requests = Vec::new();
        let first = read_stream_event_page(None, 1, |after, limit| {
            requests.push((after, limit));
            std::future::ready(Ok(journal_page(&journal, after, limit)))
        })
        .await
        .expect("first page");
        assert_eq!(first.len(), 1);
        assert_watermark(&first[0], 1);
        let second = read_stream_event_page(Some(first[0].sequence), 1, |after, limit| {
            requests.push((after, limit));
            std::future::ready(Ok(journal_page(&journal, after, limit)))
        })
        .await
        .expect("second page");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].sequence, 2);
        assert_eq!(second[0].stream_id, "pty-1");
        assert_eq!(second[0].data.as_deref(), Some(b"ready\n".as_slice()));
        assert_eq!(requests, vec![(None, 1), (Some(1), 1)]);
    }

    #[tokio::test]
    async fn mobile_linux_event_paging_wholly_filtered_page_is_bounded_and_advances() {
        let mut journal: Vec<_> = (1..=256).map(lifecycle).collect();
        journal.push(output(257));
        let mut requests = Vec::new();
        let first = read_stream_event_page(None, 256, |after, limit| {
            requests.push((after, limit));
            std::future::ready(Ok(journal_page(&journal, after, limit)))
        })
        .await
        .expect("filtered page");
        assert_eq!(first.len(), 1);
        assert_watermark(&first[0], 256);
        assert_eq!(
            requests.len(),
            1,
            "do not scan a concurrently growing journal"
        );
        let second = read_stream_event_page(Some(first[0].sequence), 256, |after, limit| {
            requests.push((after, limit));
            std::future::ready(Ok(journal_page(&journal, after, limit)))
        })
        .await
        .expect("visible page");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].sequence, 257);
    }

    #[tokio::test]
    async fn mobile_linux_event_paging_preserves_filtered_tail_watermark_without_extra_output() {
        let journal = vec![output(1), lifecycle(2), lifecycle(3)];
        let page = read_stream_event_page(None, 3, |after, limit| {
            std::future::ready(Ok(journal_page(&journal, after, limit)))
        })
        .await
        .expect("mixed page");
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].stream_id, "pty-1");
        assert_watermark(&page[1], 3);
        assert!(page.len() <= 3);

        let journal = vec![lifecycle(1), output(2)];
        let page = read_stream_event_page(None, 2, |after, limit| {
            std::future::ready(Ok(journal_page(&journal, after, limit)))
        })
        .await
        .expect("visible tail");
        assert_eq!(
            page.len(),
            1,
            "a visible tail already carries the raw watermark"
        );
        assert_eq!(page[0].sequence, 2);
    }

    #[tokio::test]
    async fn mobile_linux_event_paging_empty_zero_and_large_limits_preserve_bounds() {
        let empty = read_stream_event_page(None, 1, |_, _| std::future::ready(Ok(Vec::new())))
            .await
            .expect("empty page");
        assert!(empty.is_empty());
        let zero = read_stream_event_page(None, 0, |_, _| {
            panic!("zero-sized reads must not call the SDK");
            #[allow(unreachable_code)]
            std::future::ready(Ok(Vec::new()))
        })
        .await
        .expect("zero-sized page");
        assert!(zero.is_empty());
        let page = read_stream_event_page(None, usize::MAX, |_, limit| {
            assert_eq!(limit, MAX_MOBILE_LINUX_EVENT_BATCH);
            std::future::ready(Ok((1..=limit as u64).map(output).collect()))
        })
        .await
        .expect("clamped page");
        assert_eq!(page.len(), MAX_MOBILE_LINUX_EVENT_BATCH);
    }
}
