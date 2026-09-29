#[cfg(feature = "uniffi")]
use super::configuration::AndroidMobileLinuxConfigFfi;
#[cfg(feature = "uniffi")]
use super::linux_types::{
    MobileLinuxApiErrorFfi, MobileLinuxCapabilityFfi, MobileLinuxCommandRequestFfi,
    MobileLinuxCommandResultFfi, MobileLinuxEventFfi, MobileLinuxEventKindFfi,
    MobileLinuxMountPurposeFfi, MobileLinuxMountSpecFfi, MobileLinuxProcessHandleFfi,
    MobileLinuxPtyOpenRequestFfi, MobileLinuxPtySessionHandleFfi, MobileLinuxRootfsStateFfi,
    MobileLinuxRuntimeModeFfi, MobileLinuxStatusFfi, MobileLinuxTaskSnapshotFfi,
    MobileLinuxTaskStateFfi,
};

#[cfg(feature = "uniffi")]
pub(super) fn mobile_linux_backend_name(backend: mobile_linux_api::SandboxBackend) -> String {
    match backend {
        mobile_linux_api::SandboxBackend::LinuxNamespaces => "linux-namespaces",
        mobile_linux_api::SandboxBackend::LinuxFirejail => "linux-firejail",
        mobile_linux_api::SandboxBackend::MacOsSandboxExec => "macos-sandbox-exec",
        mobile_linux_api::SandboxBackend::WindowsJobObject => "windows-job-object",
        mobile_linux_api::SandboxBackend::AndroidMinijail => "android-minijail",
        mobile_linux_api::SandboxBackend::AndroidProot => "android-proot",
        mobile_linux_api::SandboxBackend::IosIsh => "ios-ish",
        mobile_linux_api::SandboxBackend::None => "none",
    }
    .to_string()
}

#[cfg(feature = "uniffi")]
pub(super) fn rootfs_state_to_ffi(
    state: mobile_linux_api::RootfsState,
) -> MobileLinuxRootfsStateFfi {
    match state {
        mobile_linux_api::RootfsState::Missing => MobileLinuxRootfsStateFfi::Missing,
        mobile_linux_api::RootfsState::Installing => MobileLinuxRootfsStateFfi::Installing,
        mobile_linux_api::RootfsState::Ready => MobileLinuxRootfsStateFfi::Ready,
        mobile_linux_api::RootfsState::Corrupt => MobileLinuxRootfsStateFfi::Corrupt,
        mobile_linux_api::RootfsState::Repairing => MobileLinuxRootfsStateFfi::Repairing,
        mobile_linux_api::RootfsState::Resetting => MobileLinuxRootfsStateFfi::Resetting,
        mobile_linux_api::RootfsState::Unsupported => MobileLinuxRootfsStateFfi::Unsupported,
        mobile_linux_api::RootfsState::BlockedByLicense => {
            MobileLinuxRootfsStateFfi::BlockedByLicense
        }
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn configured_workspace_guest_path(cfg: &AndroidMobileLinuxConfigFfi) -> String {
    let workspace_id = cfg
        .stable_workspace_id
        .as_deref()
        .unwrap_or("default")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>();
    let workspace_id = if workspace_id.is_empty() {
        "default".to_string()
    } else {
        workspace_id
    };
    format!("/workspace/{workspace_id}")
}

#[cfg(feature = "uniffi")]
pub(super) fn capability_to_ffi(
    capability: mobile_linux_api::MobileLinuxCapability,
) -> MobileLinuxCapabilityFfi {
    MobileLinuxCapabilityFfi {
        available: capability.available,
        backend: mobile_linux_backend_name(capability.backend),
        mode: MobileLinuxRuntimeModeFfi::MobileLinux,
        reason: capability.reason,
        streaming_output: capability.streaming_output,
        background_processes: capability.background_processes,
        pty: capability.pty,
        bind_mounts: capability.bind_mounts,
        rootfs_integrity: capability.rootfs_integrity,
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn status_to_ffi(status: mobile_linux_api::RootfsStatus) -> MobileLinuxStatusFfi {
    MobileLinuxStatusFfi {
        state: rootfs_state_to_ffi(status.state),
        backend: mobile_linux_backend_name(status.backend),
        mode: MobileLinuxRuntimeModeFfi::MobileLinux,
        platform: status.platform,
        abi: status.abi,
        version: status.version,
        managed_root: status
            .managed_root
            .map(|p| p.to_string_lossy().into_owned()),
        active_root: status.active_root.map(|p| p.to_string_lossy().into_owned()),
        staged_root: status.staged_root.map(|p| p.to_string_lossy().into_owned()),
        archive_sha256: status.archive_sha256,
        installed_size_bytes: status.installed_size_bytes,
        writable_guest_paths: status.writable_guest_paths,
        last_error: status.last_error,
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn mount_purpose_to_traits(
    value: MobileLinuxMountPurposeFfi,
) -> mobile_linux_api::MountPurpose {
    match value {
        MobileLinuxMountPurposeFfi::Workspace => mobile_linux_api::MountPurpose::Workspace,
        MobileLinuxMountPurposeFfi::LocalAppBuild => mobile_linux_api::MountPurpose::LocalAppBuild,
        MobileLinuxMountPurposeFfi::Memory => mobile_linux_api::MountPurpose::Memory,
        MobileLinuxMountPurposeFfi::Skills => mobile_linux_api::MountPurpose::Skills,
        MobileLinuxMountPurposeFfi::Shared => mobile_linux_api::MountPurpose::Shared,
        MobileLinuxMountPurposeFfi::External => mobile_linux_api::MountPurpose::External,
        MobileLinuxMountPurposeFfi::Temp => mobile_linux_api::MountPurpose::Temp,
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn command_request_to_traits(
    request: MobileLinuxCommandRequestFfi,
) -> Result<mobile_linux_api::LinuxCommandRequest, MobileLinuxApiErrorFfi> {
    let command = request.command.trim();
    if command.is_empty() {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: "command must not be empty".to_string(),
        });
    }
    let mounts = request
        .mounts
        .into_iter()
        .map(mount_spec_to_traits)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(mobile_linux_api::LinuxCommandRequest {
        command: command.to_string(),
        args: request.args,
        cwd: request.cwd,
        env: request
            .env
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect(),
        stdin: request.stdin,
        timeout_ms: request.timeout_ms,
        network: if request.allow_network {
            mobile_linux_api::NetworkPolicy::Allowed
        } else {
            mobile_linux_api::NetworkPolicy::Disabled
        },
        resource_limits: mobile_linux_api::ResourceLimits::default(),
        mounts,
    })
}

#[cfg(feature = "uniffi")]
pub(super) fn command_result_to_ffi(
    result: mobile_linux_api::LinuxCommandResult,
) -> MobileLinuxCommandResultFfi {
    MobileLinuxCommandResultFfi {
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
        timed_out: result.timed_out,
        cancelled: result.cancelled,
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn task_status_to_ffi(
    status: mobile_linux_api::MobileLinuxTaskStatus,
) -> MobileLinuxTaskStateFfi {
    match status {
        mobile_linux_api::MobileLinuxTaskStatus::Queued => MobileLinuxTaskStateFfi::Queued,
        mobile_linux_api::MobileLinuxTaskStatus::Running => MobileLinuxTaskStateFfi::Running,
        mobile_linux_api::MobileLinuxTaskStatus::Backgrounded => {
            MobileLinuxTaskStateFfi::Backgrounded
        }
        mobile_linux_api::MobileLinuxTaskStatus::Completed => MobileLinuxTaskStateFfi::Completed,
        mobile_linux_api::MobileLinuxTaskStatus::Failed => MobileLinuxTaskStateFfi::Failed,
        mobile_linux_api::MobileLinuxTaskStatus::Cancelled => MobileLinuxTaskStateFfi::Cancelled,
        mobile_linux_api::MobileLinuxTaskStatus::TimedOut => MobileLinuxTaskStateFfi::TimedOut,
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn task_snapshot_to_ffi(
    task: mobile_linux_api::MobileLinuxTaskSnapshot,
) -> MobileLinuxTaskSnapshotFfi {
    MobileLinuxTaskSnapshotFfi {
        task_id: task.task_id,
        status: task_status_to_ffi(task.status),
        command: task.command,
        started_at_ms: task.started_at_ms,
        finished_at_ms: task.finished_at_ms,
        exit_code: task.exit_code,
        detail: task.detail,
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn event_to_ffi(event: mobile_linux_api::MobileLinuxEvent) -> MobileLinuxEventFfi {
    match event.kind {
        mobile_linux_api::MobileLinuxEventKind::TaskStatusChanged {
            status,
            exit_code,
            detail,
        } => MobileLinuxEventFfi {
            sequence: event.sequence,
            task_id: event.task_id,
            kind: MobileLinuxEventKindFfi::TaskStatusChanged,
            data: None,
            text: None,
            session_id: None,
            status: Some(task_status_to_ffi(status)),
            exit_code,
            timed_out: Some(matches!(
                status,
                mobile_linux_api::MobileLinuxTaskStatus::TimedOut
            )),
            cancelled: Some(matches!(
                status,
                mobile_linux_api::MobileLinuxTaskStatus::Cancelled
            )),
            detail,
        },
        mobile_linux_api::MobileLinuxEventKind::StdoutLine { line } => MobileLinuxEventFfi {
            sequence: event.sequence,
            task_id: event.task_id,
            kind: MobileLinuxEventKindFfi::StdoutLine,
            data: None,
            text: Some(line),
            session_id: None,
            status: None,
            exit_code: None,
            timed_out: None,
            cancelled: None,
            detail: None,
        },
        mobile_linux_api::MobileLinuxEventKind::StderrChunk { chunk } => MobileLinuxEventFfi {
            sequence: event.sequence,
            task_id: event.task_id,
            kind: MobileLinuxEventKindFfi::StderrChunk,
            data: Some(chunk),
            text: None,
            session_id: None,
            status: None,
            exit_code: None,
            timed_out: None,
            cancelled: None,
            detail: None,
        },
        mobile_linux_api::MobileLinuxEventKind::PtyOutput { session_id, data } => {
            MobileLinuxEventFfi {
                sequence: event.sequence,
                task_id: event.task_id,
                kind: MobileLinuxEventKindFfi::PtyOutput,
                data: Some(data),
                text: None,
                session_id: Some(session_id),
                status: None,
                exit_code: None,
                timed_out: None,
                cancelled: None,
                detail: None,
            }
        }
        mobile_linux_api::MobileLinuxEventKind::PtyClosed {
            session_id,
            exit_code,
            detail,
        } => MobileLinuxEventFfi {
            sequence: event.sequence,
            task_id: event.task_id,
            kind: MobileLinuxEventKindFfi::PtyClosed,
            data: None,
            text: None,
            session_id: Some(session_id),
            status: None,
            exit_code,
            timed_out: None,
            cancelled: None,
            detail,
        },
        mobile_linux_api::MobileLinuxEventKind::RuntimeError { detail } => MobileLinuxEventFfi {
            sequence: event.sequence,
            task_id: event.task_id,
            kind: MobileLinuxEventKindFfi::RuntimeError,
            data: None,
            text: None,
            session_id: None,
            status: None,
            exit_code: None,
            timed_out: None,
            cancelled: None,
            detail: Some(detail),
        },
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn mount_spec_to_traits(
    mount: MobileLinuxMountSpecFfi,
) -> Result<mobile_linux_api::MountSpec, MobileLinuxApiErrorFfi> {
    if mount.guest_path.trim().is_empty() || !mount.guest_path.starts_with('/') {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: format!("invalid guest mount path: {}", mount.guest_path),
        });
    }
    if mount.host_path.trim().is_empty() {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: "host mount path must not be empty".to_string(),
        });
    }
    Ok(mobile_linux_api::MountSpec {
        host_path: std::path::PathBuf::from(mount.host_path),
        guest_path: mount.guest_path,
        read_only: mount.read_only,
        purpose: mount_purpose_to_traits(mount.purpose),
    })
}

#[cfg(feature = "uniffi")]
pub(super) fn pty_open_request_to_traits(
    request: MobileLinuxPtyOpenRequestFfi,
) -> Result<mobile_linux_api::PtyOpenRequest, MobileLinuxApiErrorFfi> {
    let command = request.command.trim();
    if command.is_empty() {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: "pty command must not be empty".to_string(),
        });
    }
    let mounts = request
        .mounts
        .into_iter()
        .map(mount_spec_to_traits)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(mobile_linux_api::PtyOpenRequest {
        command: command.to_string(),
        args: request.args,
        cwd: request.cwd,
        env: request
            .env
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect(),
        size: mobile_linux_api::PtySize {
            cols: request.size.cols,
            rows: request.size.rows,
        },
        mounts,
    })
}

#[cfg(feature = "uniffi")]
pub(super) fn process_handle_to_ffi(
    handle: mobile_linux_api::LinuxProcessHandle,
) -> MobileLinuxProcessHandleFfi {
    MobileLinuxProcessHandleFfi { id: handle.id }
}

#[cfg(feature = "uniffi")]
pub(super) fn process_handle_to_traits(
    handle: MobileLinuxProcessHandleFfi,
) -> Result<mobile_linux_api::LinuxProcessHandle, MobileLinuxApiErrorFfi> {
    if handle.id.trim().is_empty() {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: "process handle id must not be empty".to_string(),
        });
    }
    Ok(mobile_linux_api::LinuxProcessHandle {
        id: handle.id,
        enforcement: mobile_linux_api::LinuxEnforcementReceipt::default(),
    })
}

#[cfg(feature = "uniffi")]
pub(super) fn pty_handle_to_ffi(
    handle: mobile_linux_api::PtySessionHandle,
) -> MobileLinuxPtySessionHandleFfi {
    MobileLinuxPtySessionHandleFfi { id: handle.id }
}

#[cfg(feature = "uniffi")]
pub(super) fn pty_handle_to_traits(
    handle: MobileLinuxPtySessionHandleFfi,
) -> Result<mobile_linux_api::PtySessionHandle, MobileLinuxApiErrorFfi> {
    if handle.id.trim().is_empty() {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: "pty handle id must not be empty".to_string(),
        });
    }
    Ok(mobile_linux_api::PtySessionHandle { id: handle.id })
}

#[cfg(feature = "uniffi")]
pub(super) fn mobile_linux_error_to_ffi(
    error: mobile_linux_api::MobileLinuxError,
) -> MobileLinuxApiErrorFfi {
    match error {
        mobile_linux_api::MobileLinuxError::Unsupported => MobileLinuxApiErrorFfi::Unavailable {
            message: "runtime unsupported on this build".to_string(),
        },
        mobile_linux_api::MobileLinuxError::RestartRequired(message) => {
            MobileLinuxApiErrorFfi::Unavailable {
                message: format!("restart_required: {message}"),
            }
        }
        mobile_linux_api::MobileLinuxError::Unavailable(message) => {
            MobileLinuxApiErrorFfi::Unavailable { message }
        }
        mobile_linux_api::MobileLinuxError::LicenseBlocked(message) => {
            MobileLinuxApiErrorFfi::LicenseBlocked { message }
        }
        mobile_linux_api::MobileLinuxError::InvalidRequest(message) => {
            MobileLinuxApiErrorFfi::InvalidRequest { message }
        }
        mobile_linux_api::MobileLinuxError::Integrity(message)
        | mobile_linux_api::MobileLinuxError::Io(message) => {
            MobileLinuxApiErrorFfi::OperationFailed { message }
        }
        mobile_linux_api::MobileLinuxError::NetworkPolicyUnavailable(message) => {
            MobileLinuxApiErrorFfi::OperationFailed {
                message: format!("network_policy_unavailable: {message}"),
            }
        }
        mobile_linux_api::MobileLinuxError::ResourceLimitExceeded(message) => {
            MobileLinuxApiErrorFfi::OperationFailed {
                message: format!("resource_limit_exceeded: {message}"),
            }
        }
        mobile_linux_api::MobileLinuxError::Timeout => MobileLinuxApiErrorFfi::OperationFailed {
            message: "timeout".to_string(),
        },
    }
}
