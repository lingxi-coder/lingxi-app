#[cfg(feature = "uniffi")]
use super::configuration::AndroidMobileLinuxConfigFfi;
#[cfg(feature = "uniffi")]
use super::linux_conversion::{
    capability_to_ffi, command_request_to_traits, command_result_to_ffi,
    configured_workspace_guest_path, event_to_ffi, mobile_linux_error_to_ffi, mount_spec_to_traits,
    process_handle_to_ffi, process_handle_to_traits, pty_handle_to_ffi, pty_handle_to_traits,
    pty_open_request_to_traits, status_to_ffi, task_snapshot_to_ffi,
};
#[cfg(feature = "uniffi")]
use super::linux_types::{
    AndroidMobileLinuxEventSink, MobileLinuxApiErrorFfi, MobileLinuxCapabilityFfi,
    MobileLinuxCommandRequestFfi, MobileLinuxCommandResultFfi, MobileLinuxEventFfi,
    MobileLinuxEventKindFfi, MobileLinuxMountSpecFfi, MobileLinuxProcessHandleFfi,
    MobileLinuxPtyOpenRequestFfi, MobileLinuxPtySessionHandleFfi, MobileLinuxPtySizeFfi,
    MobileLinuxRootfsStateFfi, MobileLinuxRuntimeModeFfi, MobileLinuxStatusFfi,
    MobileLinuxTaskSnapshotFfi, MobileLinuxTaskStateFfi,
};
#[cfg(feature = "uniffi")]
use mobile_linux_api::MAX_MOBILE_LINUX_EVENT_BATCH;
#[cfg(feature = "uniffi")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Persistent Android mobile-linux runtime handle. Holds one runtime instance so
/// PTY/task/event state survives across FFI calls.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Object))]
pub struct AndroidMobileLinuxRuntimeHandle {
    pub(super) runtime: Arc<dyn mobile_linux_api::MobileLinuxRuntime>,
}

#[cfg(feature = "uniffi")]
pub(super) static MOBILE_LINUX_FFI_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn build_android_mobile_linux_runtime_handle(
    config: AndroidMobileLinuxConfigFfi,
) -> Result<Arc<AndroidMobileLinuxRuntimeHandle>, MobileLinuxApiErrorFfi> {
    let runtime = require_mobile_linux_runtime(Some(&config))?;
    Ok(Arc::new(AndroidMobileLinuxRuntimeHandle { runtime }))
}

#[cfg(feature = "uniffi")]
pub(super) struct AndroidMobileLinuxEventSinkBridge {
    pub(super) task_id: String,
    pub(super) inner: Box<dyn AndroidMobileLinuxEventSink>,
}

#[cfg(feature = "uniffi")]
impl AndroidMobileLinuxEventSinkBridge {
    pub(super) async fn emit_event(
        &self,
        event: MobileLinuxEventFfi,
    ) -> Result<(), MobileLinuxApiErrorFfi> {
        self.inner.on_event(event).await
    }
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl mobile_linux_api::ProcessStreamSink for AndroidMobileLinuxEventSinkBridge {
    async fn stdout_line(&self, line: String) -> Result<(), mobile_linux_api::ProcessError> {
        self.inner
            .on_event(MobileLinuxEventFfi {
                sequence: 0,
                task_id: Some(self.task_id.clone()),
                kind: MobileLinuxEventKindFfi::StdoutLine,
                data: None,
                text: Some(line),
                session_id: None,
                status: None,
                exit_code: None,
                timed_out: None,
                cancelled: None,
                detail: None,
            })
            .await
            .map_err(|err| mobile_linux_api::ProcessError::Io(err.to_string()))
    }

    async fn stderr_chunk(&self, chunk: Vec<u8>) -> Result<(), mobile_linux_api::ProcessError> {
        self.inner
            .on_event(MobileLinuxEventFfi {
                sequence: 0,
                task_id: Some(self.task_id.clone()),
                kind: MobileLinuxEventKindFfi::StderrChunk,
                data: Some(chunk),
                text: None,
                session_id: None,
                status: None,
                exit_code: None,
                timed_out: None,
                cancelled: None,
                detail: None,
            })
            .await
            .map_err(|err| mobile_linux_api::ProcessError::Io(err.to_string()))
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn require_mobile_linux_runtime(
    config: Option<&AndroidMobileLinuxConfigFfi>,
) -> Result<Arc<dyn mobile_linux_api::MobileLinuxRuntime>, MobileLinuxApiErrorFfi> {
    match config {
        None => Err(MobileLinuxApiErrorFfi::LegacySelected),
        Some(cfg) if matches!(cfg.mode, MobileLinuxRuntimeModeFfi::Legacy) => {
            Err(MobileLinuxApiErrorFfi::LegacySelected)
        }
        Some(_) => {
            android_mobile_linux_runtime(config).ok_or(MobileLinuxApiErrorFfi::Unavailable {
                message: "mobile-linux runtime is not wired into this build".to_string(),
            })
        }
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn block_on_mobile_linux_call<T>(
    config: Option<&AndroidMobileLinuxConfigFfi>,
    op: impl FnOnce(
        Arc<dyn mobile_linux_api::MobileLinuxRuntime>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T, mobile_linux_api::MobileLinuxError>> + Send>,
    >,
) -> Result<T, MobileLinuxApiErrorFfi> {
    let runtime = require_mobile_linux_runtime(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxApiErrorFfi::OperationFailed {
            message: err.to_string(),
        })?;
    rt.block_on(op(runtime)).map_err(mobile_linux_error_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(async_runtime = "tokio"))]
impl AndroidMobileLinuxRuntimeHandle {
    pub async fn capability(&self) -> MobileLinuxCapabilityFfi {
        capability_to_ffi(self.runtime.probe_capability().await)
    }

    pub async fn status(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .rootfs_status()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn boot(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .boot()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn shutdown(&self) -> Result<(), MobileLinuxApiErrorFfi> {
        self.runtime
            .shutdown()
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn verify_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .verify_rootfs()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn repair_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .repair_rootfs()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn reset_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .reset_rootfs()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn run_command(
        &self,
        request: MobileLinuxCommandRequestFfi,
    ) -> Result<MobileLinuxCommandResultFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .run(command_request_to_traits(request)?)
            .await
            .map(command_result_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn run_command_streaming(
        &self,
        request: MobileLinuxCommandRequestFfi,
        sink: Box<dyn AndroidMobileLinuxEventSink>,
    ) -> Result<MobileLinuxCommandResultFfi, MobileLinuxApiErrorFfi> {
        let task_id = format!(
            "run-{}",
            MOBILE_LINUX_FFI_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let sink = Arc::new(AndroidMobileLinuxEventSinkBridge {
            task_id,
            inner: sink,
        });
        let result = self
            .runtime
            .run_streaming(command_request_to_traits(request)?, sink.clone())
            .await
            .map_err(mobile_linux_error_to_ffi)?;
        sink.emit_event(MobileLinuxEventFfi {
            sequence: 0,
            task_id: Some(sink.task_id.clone()),
            kind: MobileLinuxEventKindFfi::TaskStatusChanged,
            data: None,
            text: None,
            session_id: None,
            status: Some(if result.cancelled {
                MobileLinuxTaskStateFfi::Cancelled
            } else if result.timed_out {
                MobileLinuxTaskStateFfi::TimedOut
            } else if result.exit_code == 0 {
                MobileLinuxTaskStateFfi::Completed
            } else {
                MobileLinuxTaskStateFfi::Failed
            }),
            exit_code: Some(result.exit_code),
            timed_out: Some(result.timed_out),
            cancelled: Some(result.cancelled),
            detail: None,
        })
        .await?;
        Ok(command_result_to_ffi(result))
    }

    pub async fn spawn_background(
        &self,
        request: MobileLinuxCommandRequestFfi,
    ) -> Result<MobileLinuxProcessHandleFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .spawn_background(command_request_to_traits(request)?)
            .await
            .map(process_handle_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn kill_process(
        &self,
        handle: MobileLinuxProcessHandleFfi,
    ) -> Result<(), MobileLinuxApiErrorFfi> {
        self.runtime
            .kill(&process_handle_to_traits(handle)?)
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn open_pty(
        &self,
        request: MobileLinuxPtyOpenRequestFfi,
    ) -> Result<MobileLinuxPtySessionHandleFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .open_pty(pty_open_request_to_traits(request)?)
            .await
            .map(pty_handle_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn write_pty(
        &self,
        handle: MobileLinuxPtySessionHandleFfi,
        input: Vec<u8>,
    ) -> Result<(), MobileLinuxApiErrorFfi> {
        self.runtime
            .write_pty(&pty_handle_to_traits(handle)?, input)
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn resize_pty(
        &self,
        handle: MobileLinuxPtySessionHandleFfi,
        size: MobileLinuxPtySizeFfi,
    ) -> Result<(), MobileLinuxApiErrorFfi> {
        self.runtime
            .resize_pty(
                &pty_handle_to_traits(handle)?,
                mobile_linux_api::PtySize {
                    cols: size.cols,
                    rows: size.rows,
                },
            )
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn close_pty(
        &self,
        handle: MobileLinuxPtySessionHandleFfi,
    ) -> Result<(), MobileLinuxApiErrorFfi> {
        self.runtime
            .close_pty(&pty_handle_to_traits(handle)?)
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn configure_mounts(
        &self,
        mounts: Vec<MobileLinuxMountSpecFfi>,
    ) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
        let mounts = mounts
            .into_iter()
            .map(mount_spec_to_traits)
            .collect::<Result<Vec<_>, _>>()?;
        self.runtime
            .configure_mounts(mounts)
            .await
            .map_err(mobile_linux_error_to_ffi)?;
        self.status().await
    }

    pub async fn read_events(
        &self,
        after_sequence: Option<u64>,
        limit: Option<u32>,
    ) -> Result<Vec<MobileLinuxEventFfi>, MobileLinuxApiErrorFfi> {
        let limit = limit
            .map(|value| value as usize)
            .unwrap_or(MAX_MOBILE_LINUX_EVENT_BATCH)
            .min(MAX_MOBILE_LINUX_EVENT_BATCH);
        self.runtime
            .read_events(after_sequence, limit)
            .await
            .map(|events| events.into_iter().map(event_to_ffi).collect())
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn list_tasks(
        &self,
    ) -> Result<Vec<MobileLinuxTaskSnapshotFfi>, MobileLinuxApiErrorFfi> {
        self.runtime
            .list_tasks()
            .await
            .map(|tasks| tasks.into_iter().map(task_snapshot_to_ffi).collect())
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn task_status(
        &self,
        task_id: String,
    ) -> Result<Option<MobileLinuxTaskSnapshotFfi>, MobileLinuxApiErrorFfi> {
        if task_id.trim().is_empty() {
            return Err(MobileLinuxApiErrorFfi::InvalidRequest {
                message: "task_id must not be empty".to_string(),
            });
        }
        self.runtime
            .task_status(&task_id)
            .await
            .map(|task| task.map(task_snapshot_to_ffi))
            .map_err(mobile_linux_error_to_ffi)
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn android_mobile_linux_status_from_config(
    config: Option<&AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxStatusFfi {
    match config {
        None => MobileLinuxStatusFfi {
            state: MobileLinuxRootfsStateFfi::Unsupported,
            backend: "android-minijail".to_string(),
            mode: MobileLinuxRuntimeModeFfi::Legacy,
            platform: "android".to_string(),
            abi: "unknown".to_string(),
            version: None,
            managed_root: None,
            active_root: None,
            staged_root: None,
            archive_sha256: None,
            installed_size_bytes: None,
            writable_guest_paths: vec![],
            last_error: Some("legacy backend selected".to_string()),
        },
        Some(cfg) if matches!(cfg.mode, MobileLinuxRuntimeModeFfi::Legacy) => {
            MobileLinuxStatusFfi {
                state: MobileLinuxRootfsStateFfi::Unsupported,
                backend: "android-minijail".to_string(),
                mode: MobileLinuxRuntimeModeFfi::Legacy,
                platform: "android".to_string(),
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
                    configured_workspace_guest_path(cfg),
                ],
                last_error: Some("legacy backend selected".to_string()),
            }
        }
        Some(cfg) => MobileLinuxStatusFfi {
            state: MobileLinuxRootfsStateFfi::Unsupported,
            backend: "android-proot".to_string(),
            mode: MobileLinuxRuntimeModeFfi::MobileLinux,
            platform: "android".to_string(),
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
                configured_workspace_guest_path(cfg),
            ],
            last_error: Some("Android PRoot runtime is not linked in this build".to_string()),
        },
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn android_mobile_linux_runtime(
    config: Option<&AndroidMobileLinuxConfigFfi>,
) -> Option<Arc<dyn mobile_linux_api::MobileLinuxRuntime>> {
    let cfg = config?;
    if matches!(cfg.mode, MobileLinuxRuntimeModeFfi::Legacy) {
        return None;
    }
    #[cfg(target_os = "android")]
    {
        use std::collections::HashMap;
        use std::sync::{LazyLock, Mutex};

        static RUNTIMES: LazyLock<
            Mutex<HashMap<String, Arc<dyn mobile_linux_api::MobileLinuxRuntime>>>,
        > = LazyLock::new(|| Mutex::new(HashMap::new()));
        let key = format!(
            "{}|{}|{}|{}|{}",
            cfg.managed_root,
            cfg.app_sandbox_root,
            cfg.abi,
            cfg.rootfs_version,
            cfg.archive_sha256.as_deref().unwrap_or_default(),
        );
        if let Some(runtime) = RUNTIMES
            .lock()
            .expect("Android MobileLinux runtime registry")
            .get(&key)
            .cloned()
        {
            return Some(runtime);
        }
        let runtime = platform_android::AndroidProotRuntime::new(
            platform_android::AndroidProotRuntimeConfig {
                managed_root: std::path::PathBuf::from(&cfg.managed_root),
                app_sandbox_root: std::path::PathBuf::from(&cfg.app_sandbox_root),
                abi: cfg.abi.clone(),
                rootfs_version: cfg.rootfs_version.clone(),
                archive_sha256: cfg.archive_sha256.clone(),
            },
        );
        let runtime = Arc::new(runtime) as Arc<dyn mobile_linux_api::MobileLinuxRuntime>;
        RUNTIMES
            .lock()
            .expect("Android MobileLinux runtime registry")
            .insert(key, runtime.clone());
        return Some(runtime);
    }
    #[cfg(not(target_os = "android"))]
    {
        let runtime = mobile_linux_api::UnavailableMobileLinuxRuntime::unavailable(
            mobile_linux_api::SandboxBackend::AndroidProot,
            mobile_linux_api::MobileLinuxRuntimeMode::MobileLinux,
            "android",
            cfg.abi.clone(),
            "Android PRoot runtime requires an Android target",
        );
        Some(Arc::new(runtime) as Arc<dyn mobile_linux_api::MobileLinuxRuntime>)
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn android_mobile_linux_capability_from_config(
    config: Option<&AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxCapabilityFfi {
    match config {
        None => MobileLinuxCapabilityFfi {
            available: false,
            backend: "android-minijail".to_string(),
            mode: MobileLinuxRuntimeModeFfi::Legacy,
            reason: Some("legacy backend selected".to_string()),
            streaming_output: false,
            background_processes: false,
            pty: false,
            bind_mounts: false,
            rootfs_integrity: false,
        },
        Some(cfg) if matches!(cfg.mode, MobileLinuxRuntimeModeFfi::Legacy) => {
            MobileLinuxCapabilityFfi {
                available: false,
                backend: "android-minijail".to_string(),
                mode: MobileLinuxRuntimeModeFfi::Legacy,
                reason: Some("legacy backend selected".to_string()),
                streaming_output: false,
                background_processes: false,
                pty: false,
                bind_mounts: false,
                rootfs_integrity: false,
            }
        }
        Some(_cfg) => MobileLinuxCapabilityFfi {
            available: false,
            backend: "android-proot".to_string(),
            mode: MobileLinuxRuntimeModeFfi::MobileLinux,
            reason: Some("Android PRoot runtime is not linked in this build".to_string()),
            streaming_output: false,
            background_processes: false,
            pty: false,
            bind_mounts: false,
            rootfs_integrity: false,
        },
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn block_on_mobile_linux_status(
    config: Option<&AndroidMobileLinuxConfigFfi>,
    op: fn(
        Arc<dyn mobile_linux_api::MobileLinuxRuntime>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        mobile_linux_api::RootfsStatus,
                        mobile_linux_api::MobileLinuxError,
                    >,
                > + Send,
        >,
    >,
) -> MobileLinuxStatusFfi {
    let Some(runtime) = android_mobile_linux_runtime(config) else {
        return android_mobile_linux_status_from_config(config);
    };
    let fallback = android_mobile_linux_status_from_config(config);
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return fallback;
    };
    match rt.block_on(op(runtime)) {
        Ok(status) => status_to_ffi(status),
        Err(error) => {
            let mut status = fallback;
            status.last_error = Some(error.to_string());
            status
        }
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn verify_rootfs_op(
    runtime: Arc<dyn mobile_linux_api::MobileLinuxRuntime>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<mobile_linux_api::RootfsStatus, mobile_linux_api::MobileLinuxError>,
            > + Send,
    >,
> {
    Box::pin(async move { runtime.verify_rootfs().await })
}

#[cfg(feature = "uniffi")]
pub(super) fn repair_rootfs_op(
    runtime: Arc<dyn mobile_linux_api::MobileLinuxRuntime>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<mobile_linux_api::RootfsStatus, mobile_linux_api::MobileLinuxError>,
            > + Send,
    >,
> {
    Box::pin(async move { runtime.repair_rootfs().await })
}

#[cfg(feature = "uniffi")]
pub(super) fn reset_rootfs_op(
    runtime: Arc<dyn mobile_linux_api::MobileLinuxRuntime>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<mobile_linux_api::RootfsStatus, mobile_linux_api::MobileLinuxError>,
            > + Send,
    >,
> {
    Box::pin(async move { runtime.reset_rootfs().await })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_capability(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxCapabilityFfi {
    android_mobile_linux_capability_from_config(config.as_ref())
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_status(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxStatusFfi {
    android_mobile_linux_status_from_config(config.as_ref())
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_verify_rootfs(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxStatusFfi {
    block_on_mobile_linux_status(config.as_ref(), verify_rootfs_op)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_repair_rootfs(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxStatusFfi {
    block_on_mobile_linux_status(config.as_ref(), repair_rootfs_op)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_reset_rootfs(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxStatusFfi {
    block_on_mobile_linux_status(config.as_ref(), reset_rootfs_op)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_boot(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.boot().await })
    })
    .map(status_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_shutdown(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> Result<(), MobileLinuxApiErrorFfi> {
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move {
            runtime.shutdown().await?;
            Ok(())
        })
    })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_run_command(
    config: Option<AndroidMobileLinuxConfigFfi>,
    request: MobileLinuxCommandRequestFfi,
) -> Result<MobileLinuxCommandResultFfi, MobileLinuxApiErrorFfi> {
    let request = command_request_to_traits(request)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.run(request).await })
    })
    .map(command_result_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_run_command_streaming(
    config: Option<AndroidMobileLinuxConfigFfi>,
    request: MobileLinuxCommandRequestFfi,
    sink: Box<dyn AndroidMobileLinuxEventSink>,
) -> Result<MobileLinuxCommandResultFfi, MobileLinuxApiErrorFfi> {
    let request = command_request_to_traits(request)?;
    let task_id = format!(
        "run-{}",
        MOBILE_LINUX_FFI_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        let sink = std::sync::Arc::new(AndroidMobileLinuxEventSinkBridge {
            task_id: task_id.clone(),
            inner: sink,
        });
        Box::pin(async move {
            let result = runtime.run_streaming(request, sink.clone()).await?;
            sink.emit_event(MobileLinuxEventFfi {
                sequence: 0,
                task_id: Some(task_id),
                kind: MobileLinuxEventKindFfi::TaskStatusChanged,
                data: None,
                text: None,
                session_id: None,
                status: Some(if result.cancelled {
                    MobileLinuxTaskStateFfi::Cancelled
                } else if result.timed_out {
                    MobileLinuxTaskStateFfi::TimedOut
                } else if result.exit_code == 0 {
                    MobileLinuxTaskStateFfi::Completed
                } else {
                    MobileLinuxTaskStateFfi::Failed
                }),
                exit_code: Some(result.exit_code),
                timed_out: Some(result.timed_out),
                cancelled: Some(result.cancelled),
                detail: None,
            })
            .await
            .map_err(|err| mobile_linux_api::MobileLinuxError::Io(err.to_string()))?;
            Ok(result)
        })
    })
    .map(command_result_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_spawn_background(
    config: Option<AndroidMobileLinuxConfigFfi>,
    request: MobileLinuxCommandRequestFfi,
) -> Result<MobileLinuxProcessHandleFfi, MobileLinuxApiErrorFfi> {
    let request = command_request_to_traits(request)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.spawn_background(request).await })
    })
    .map(process_handle_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_kill_process(
    config: Option<AndroidMobileLinuxConfigFfi>,
    handle: MobileLinuxProcessHandleFfi,
) -> Result<(), MobileLinuxApiErrorFfi> {
    let handle = process_handle_to_traits(handle)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move {
            runtime.kill(&handle).await?;
            Ok(())
        })
    })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_open_pty(
    config: Option<AndroidMobileLinuxConfigFfi>,
    request: MobileLinuxPtyOpenRequestFfi,
) -> Result<MobileLinuxPtySessionHandleFfi, MobileLinuxApiErrorFfi> {
    let request = pty_open_request_to_traits(request)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.open_pty(request).await })
    })
    .map(pty_handle_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_write_pty(
    config: Option<AndroidMobileLinuxConfigFfi>,
    handle: MobileLinuxPtySessionHandleFfi,
    input: Vec<u8>,
) -> Result<(), MobileLinuxApiErrorFfi> {
    let handle = pty_handle_to_traits(handle)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move {
            runtime.write_pty(&handle, input).await?;
            Ok(())
        })
    })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_resize_pty(
    config: Option<AndroidMobileLinuxConfigFfi>,
    handle: MobileLinuxPtySessionHandleFfi,
    size: MobileLinuxPtySizeFfi,
) -> Result<(), MobileLinuxApiErrorFfi> {
    let handle = pty_handle_to_traits(handle)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move {
            runtime
                .resize_pty(
                    &handle,
                    mobile_linux_api::PtySize {
                        cols: size.cols,
                        rows: size.rows,
                    },
                )
                .await?;
            Ok(())
        })
    })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_close_pty(
    config: Option<AndroidMobileLinuxConfigFfi>,
    handle: MobileLinuxPtySessionHandleFfi,
) -> Result<(), MobileLinuxApiErrorFfi> {
    let handle = pty_handle_to_traits(handle)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move {
            runtime.close_pty(&handle).await?;
            Ok(())
        })
    })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_configure_mounts(
    config: Option<AndroidMobileLinuxConfigFfi>,
    mounts: Vec<MobileLinuxMountSpecFfi>,
) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
    let mounts = mounts
        .into_iter()
        .map(mount_spec_to_traits)
        .collect::<Result<Vec<_>, _>>()?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move {
            runtime.configure_mounts(mounts).await?;
            runtime.rootfs_status().await
        })
    })
    .map(status_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_list_tasks(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> Result<Vec<MobileLinuxTaskSnapshotFfi>, MobileLinuxApiErrorFfi> {
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.list_tasks().await })
    })
    .map(|tasks| tasks.into_iter().map(task_snapshot_to_ffi).collect())
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_read_events(
    config: Option<AndroidMobileLinuxConfigFfi>,
    after_sequence: Option<u64>,
    limit: Option<u32>,
) -> Result<Vec<MobileLinuxEventFfi>, MobileLinuxApiErrorFfi> {
    let limit = limit
        .map(|value| value as usize)
        .unwrap_or(MAX_MOBILE_LINUX_EVENT_BATCH)
        .min(MAX_MOBILE_LINUX_EVENT_BATCH);
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.read_events(after_sequence, limit).await })
    })
    .map(|events| events.into_iter().map(event_to_ffi).collect())
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_task_status(
    config: Option<AndroidMobileLinuxConfigFfi>,
    task_id: String,
) -> Result<Option<MobileLinuxTaskSnapshotFfi>, MobileLinuxApiErrorFfi> {
    if task_id.trim().is_empty() {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: "task_id must not be empty".to_string(),
        });
    }
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.task_status(&task_id).await })
    })
    .map(|snapshot| snapshot.map(task_snapshot_to_ffi))
}
