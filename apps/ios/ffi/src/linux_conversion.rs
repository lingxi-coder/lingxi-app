#[cfg(feature = "uniffi")]
use super::linux_types::MobileLinuxOperationFfiError;
#[cfg(feature = "uniffi")]
use super::linux_types::{
    MobileLinuxCapabilityFfi, MobileLinuxCommandRequestFfi, MobileLinuxCommandResultFfi,
    MobileLinuxMountPurposeFfi, MobileLinuxMountSpecFfi, MobileLinuxNetworkPolicyFfi,
    MobileLinuxPtyOpenRequestFfi, MobileLinuxRootfsStateFfi, MobileLinuxRuntimeModeFfi,
    MobileLinuxStatusFfi, MobileLinuxStreamEventFfi, MobileLinuxStreamEventKindFfi,
    MobileLinuxStreamSourceFfi, MobileLinuxTaskFfi, MobileLinuxTaskStateFfi,
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
pub(super) fn capability_to_ffi(
    capability: mobile_linux_api::MobileLinuxCapability,
) -> MobileLinuxCapabilityFfi {
    MobileLinuxCapabilityFfi {
        available: capability.available,
        backend: mobile_linux_backend_name(capability.backend),
        mode: match capability.mode {
            mobile_linux_api::MobileLinuxRuntimeMode::Legacy => MobileLinuxRuntimeModeFfi::Legacy,
            mobile_linux_api::MobileLinuxRuntimeMode::MobileLinux => {
                MobileLinuxRuntimeModeFfi::MobileLinux
            }
        },
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
        mode: match status.mode {
            mobile_linux_api::MobileLinuxRuntimeMode::Legacy => MobileLinuxRuntimeModeFfi::Legacy,
            mobile_linux_api::MobileLinuxRuntimeMode::MobileLinux => {
                MobileLinuxRuntimeModeFfi::MobileLinux
            }
        },
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
pub(super) fn mount_purpose_from_ffi(
    value: MobileLinuxMountPurposeFfi,
) -> mobile_linux_api::MountPurpose {
    match value {
        MobileLinuxMountPurposeFfi::Workspace => mobile_linux_api::MountPurpose::Workspace,
        MobileLinuxMountPurposeFfi::Memory => mobile_linux_api::MountPurpose::Memory,
        MobileLinuxMountPurposeFfi::Skills => mobile_linux_api::MountPurpose::Skills,
        MobileLinuxMountPurposeFfi::Shared => mobile_linux_api::MountPurpose::Shared,
        MobileLinuxMountPurposeFfi::External => mobile_linux_api::MountPurpose::External,
        MobileLinuxMountPurposeFfi::Temp => mobile_linux_api::MountPurpose::Temp,
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn network_policy_from_ffi(
    value: MobileLinuxNetworkPolicyFfi,
) -> mobile_linux_api::NetworkPolicy {
    match value {
        MobileLinuxNetworkPolicyFfi::Disabled => mobile_linux_api::NetworkPolicy::Disabled,
        MobileLinuxNetworkPolicyFfi::LoopbackOnly => mobile_linux_api::NetworkPolicy::LoopbackOnly,
        MobileLinuxNetworkPolicyFfi::Allowed => mobile_linux_api::NetworkPolicy::Allowed,
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn mount_spec_from_ffi(value: MobileLinuxMountSpecFfi) -> mobile_linux_api::MountSpec {
    mobile_linux_api::MountSpec {
        host_path: std::path::PathBuf::from(value.host_path),
        guest_path: value.guest_path,
        read_only: value.read_only,
        purpose: mount_purpose_from_ffi(value.purpose),
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn command_request_from_ffi(
    value: MobileLinuxCommandRequestFfi,
) -> mobile_linux_api::LinuxCommandRequest {
    mobile_linux_api::LinuxCommandRequest {
        command: value.command,
        args: value.args,
        cwd: value.cwd,
        env: value.env.into_iter().collect(),
        stdin: value.stdin,
        timeout_ms: value.timeout_ms,
        network: network_policy_from_ffi(value.network),
        resource_limits: mobile_linux_api::ResourceLimits::default(),
        mounts: value.mounts.into_iter().map(mount_spec_from_ffi).collect(),
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn pty_request_from_ffi(
    value: MobileLinuxPtyOpenRequestFfi,
) -> mobile_linux_api::PtyOpenRequest {
    mobile_linux_api::PtyOpenRequest {
        command: value.command,
        args: value.args,
        cwd: value.cwd,
        env: value.env.into_iter().collect(),
        size: mobile_linux_api::PtySize {
            cols: value.cols,
            rows: value.rows,
        },
        mounts: value.mounts.into_iter().map(mount_spec_from_ffi).collect(),
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn command_result_to_ffi(
    value: mobile_linux_api::LinuxCommandResult,
) -> MobileLinuxCommandResultFfi {
    MobileLinuxCommandResultFfi {
        stdout: value.stdout,
        stderr: value.stderr,
        exit_code: value.exit_code,
        timed_out: value.timed_out,
        cancelled: value.cancelled,
    }
}

#[cfg(feature = "uniffi")]
pub(super) fn task_snapshot_to_ffi(
    value: mobile_linux_api::MobileLinuxTaskSnapshot,
) -> MobileLinuxTaskFfi {
    MobileLinuxTaskFfi {
        id: value.task_id,
        title: value.command,
        state: match value.status {
            mobile_linux_api::MobileLinuxTaskStatus::Queued
            | mobile_linux_api::MobileLinuxTaskStatus::Running
            | mobile_linux_api::MobileLinuxTaskStatus::Backgrounded => {
                MobileLinuxTaskStateFfi::Running
            }
            mobile_linux_api::MobileLinuxTaskStatus::Completed => {
                MobileLinuxTaskStateFfi::Completed
            }
            mobile_linux_api::MobileLinuxTaskStatus::Failed
            | mobile_linux_api::MobileLinuxTaskStatus::TimedOut => MobileLinuxTaskStateFfi::Failed,
            mobile_linux_api::MobileLinuxTaskStatus::Cancelled => {
                MobileLinuxTaskStateFfi::Cancelled
            }
        },
        detail: value.detail,
    }
}

#[cfg(feature = "uniffi")]
/// `None` for events that have no stream representation and must be SKIPPED
/// (not surfaced as a bogus stream event).
pub(super) fn event_to_ffi(
    value: mobile_linux_api::MobileLinuxEvent,
) -> Option<MobileLinuxStreamEventFfi> {
    Some(match value.kind {
        mobile_linux_api::MobileLinuxEventKind::TaskStatusChanged {
            status,
            exit_code,
            detail,
        } => {
            // ONLY terminal statuses read as an exit. This arm used to map
            // EVERY status change to `kind: Exit` — including the `Running`
            // emitted by task CREATION. The PTY task's id IS the session id,
            // so merely opening a terminal planted `{kind: exit, stream:
            // <session>, code: nil}` at the head of the event log; the
            // terminal's first read closed a perfectly healthy shell with
            // "[process exited]" and stopped polling, and every restart died
            // the same way at its own creation event.
            if matches!(
                status,
                mobile_linux_api::MobileLinuxTaskStatus::Queued
                    | mobile_linux_api::MobileLinuxTaskStatus::Running
                    | mobile_linux_api::MobileLinuxTaskStatus::Backgrounded
            ) {
                return None;
            }
            MobileLinuxStreamEventFfi {
                sequence: value.sequence,
                task_id: value.task_id.clone(),
                stream_id: value
                    .task_id
                    .clone()
                    .unwrap_or_else(|| "runtime".to_string()),
                source: MobileLinuxStreamSourceFfi::Run,
                kind: MobileLinuxStreamEventKindFfi::Exit,
                text: detail,
                data: None,
                exit_code,
                timed_out: matches!(status, mobile_linux_api::MobileLinuxTaskStatus::TimedOut),
            }
        }
        mobile_linux_api::MobileLinuxEventKind::StdoutLine { line } => MobileLinuxStreamEventFfi {
            sequence: value.sequence,
            task_id: value.task_id.clone(),
            stream_id: value
                .task_id
                .clone()
                .unwrap_or_else(|| "runtime".to_string()),
            source: MobileLinuxStreamSourceFfi::Run,
            kind: MobileLinuxStreamEventKindFfi::StdoutLine,
            text: Some(line),
            data: None,
            exit_code: None,
            timed_out: false,
        },
        mobile_linux_api::MobileLinuxEventKind::StderrChunk { chunk } => {
            MobileLinuxStreamEventFfi {
                sequence: value.sequence,
                task_id: value.task_id.clone(),
                stream_id: value
                    .task_id
                    .clone()
                    .unwrap_or_else(|| "runtime".to_string()),
                source: MobileLinuxStreamSourceFfi::Run,
                kind: MobileLinuxStreamEventKindFfi::StderrChunk,
                text: Some(String::from_utf8_lossy(&chunk).into_owned()),
                data: Some(chunk),
                exit_code: None,
                timed_out: false,
            }
        }
        mobile_linux_api::MobileLinuxEventKind::PtyOutput { session_id, data } => {
            MobileLinuxStreamEventFfi {
                sequence: value.sequence,
                task_id: value.task_id,
                stream_id: session_id,
                source: MobileLinuxStreamSourceFfi::Pty,
                kind: MobileLinuxStreamEventKindFfi::StdoutLine,
                text: Some(String::from_utf8_lossy(&data).into_owned()),
                data: Some(data),
                exit_code: None,
                timed_out: false,
            }
        }
        mobile_linux_api::MobileLinuxEventKind::PtyClosed {
            session_id,
            exit_code,
            detail,
        } => MobileLinuxStreamEventFfi {
            sequence: value.sequence,
            task_id: value.task_id,
            stream_id: session_id,
            source: MobileLinuxStreamSourceFfi::Pty,
            kind: MobileLinuxStreamEventKindFfi::Exit,
            text: detail,
            data: None,
            exit_code,
            timed_out: false,
        },
        mobile_linux_api::MobileLinuxEventKind::RuntimeError { detail } => {
            MobileLinuxStreamEventFfi {
                sequence: value.sequence,
                task_id: value.task_id,
                stream_id: "runtime".to_string(),
                source: MobileLinuxStreamSourceFfi::Run,
                kind: MobileLinuxStreamEventKindFfi::Error,
                text: Some(detail),
                data: None,
                exit_code: None,
                timed_out: false,
            }
        }
    })
}

#[cfg(feature = "uniffi")]
pub(super) fn mobile_linux_error_to_ffi(
    error: mobile_linux_api::MobileLinuxError,
) -> MobileLinuxOperationFfiError {
    match error {
        mobile_linux_api::MobileLinuxError::Unsupported => {
            MobileLinuxOperationFfiError::Unsupported
        }
        mobile_linux_api::MobileLinuxError::RestartRequired(message) => {
            MobileLinuxOperationFfiError::Unavailable {
                message: format!("restart_required: {message}"),
            }
        }
        mobile_linux_api::MobileLinuxError::Unavailable(message) => {
            MobileLinuxOperationFfiError::Unavailable { message }
        }
        mobile_linux_api::MobileLinuxError::LicenseBlocked(message) => {
            MobileLinuxOperationFfiError::LicenseBlocked { message }
        }
        mobile_linux_api::MobileLinuxError::Integrity(message)
        | mobile_linux_api::MobileLinuxError::InvalidRequest(message) => {
            MobileLinuxOperationFfiError::InvalidRequest { message }
        }
        mobile_linux_api::MobileLinuxError::Io(message) => {
            MobileLinuxOperationFfiError::Io { message }
        }
        mobile_linux_api::MobileLinuxError::NetworkPolicyUnavailable(message) => {
            MobileLinuxOperationFfiError::Io {
                message: format!("network_policy_unavailable: {message}"),
            }
        }
        mobile_linux_api::MobileLinuxError::ResourceLimitExceeded(message) => {
            MobileLinuxOperationFfiError::Io {
                message: format!("resource_limit_exceeded: {message}"),
            }
        }
        mobile_linux_api::MobileLinuxError::Timeout => MobileLinuxOperationFfiError::Timeout,
    }
}
