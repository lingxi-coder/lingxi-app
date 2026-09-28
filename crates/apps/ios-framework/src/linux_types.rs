/// Mobile Linux runtime mode exposed to the iOS host.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MobileLinuxRuntimeModeFfi {
    Legacy,
    MobileLinux,
}

/// FFI rootfs lifecycle state.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxRootfsStateFfi {
    Missing,
    Installing,
    Ready,
    Corrupt,
    Repairing,
    Resetting,
    Unsupported,
    BlockedByLicense,
}

/// FFI capability snapshot for the iOS mobile-linux runtime.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxCapabilityFfi {
    pub available: bool,
    pub backend: String,
    pub mode: MobileLinuxRuntimeModeFfi,
    pub reason: Option<String>,
    pub streaming_output: bool,
    pub background_processes: bool,
    pub pty: bool,
    pub bind_mounts: bool,
    pub rootfs_integrity: bool,
}

/// FFI rootfs status snapshot for the iOS mobile-linux runtime.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxStatusFfi {
    pub state: MobileLinuxRootfsStateFfi,
    pub backend: String,
    pub mode: MobileLinuxRuntimeModeFfi,
    pub platform: String,
    pub abi: String,
    pub version: Option<String>,
    pub managed_root: Option<String>,
    pub active_root: Option<String>,
    pub staged_root: Option<String>,
    pub archive_sha256: Option<String>,
    pub installed_size_bytes: Option<u64>,
    pub writable_guest_paths: Vec<String>,
    pub last_error: Option<String>,
}

/// Network policy for a guest command.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxNetworkPolicyFfi {
    Disabled,
    LoopbackOnly,
    Allowed,
}

/// Mount purpose surfaced to the host UI.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxMountPurposeFfi {
    Workspace,
    LocalAppBuild,
    Memory,
    Skills,
    Shared,
    External,
    Temp,
}

/// Host→guest bind mount descriptor.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxMountSpecFfi {
    pub host_path: String,
    pub guest_path: String,
    pub read_only: bool,
    pub purpose: MobileLinuxMountPurposeFfi,
}

/// One-shot guest command request.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxCommandRequestFfi {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: std::collections::HashMap<String, String>,
    pub stdin: Option<String>,
    pub timeout_ms: Option<u64>,
    pub network: MobileLinuxNetworkPolicyFfi,
    pub mounts: Vec<MobileLinuxMountSpecFfi>,
}

/// Completed command result.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxCommandResultFfi {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub cancelled: bool,
}

/// PTY open request.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxPtyOpenRequestFfi {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: std::collections::HashMap<String, String>,
    pub cols: u16,
    pub rows: u16,
    pub mounts: Vec<MobileLinuxMountSpecFfi>,
}

/// PTY session snapshot returned to the host.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxPtySessionFfi {
    pub id: String,
    pub available: bool,
    pub detail: Option<String>,
}

/// Background task lifecycle state.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxTaskStateFfi {
    Running,
    Completed,
    Failed,
    Cancelled,
    Unavailable,
}

/// Background task summary for the host UI.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxTaskFfi {
    pub id: String,
    pub title: String,
    pub state: MobileLinuxTaskStateFfi,
    pub detail: Option<String>,
}

/// Streaming event kind surfaced to Swift.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxStreamEventKindFfi {
    StdoutLine,
    StderrChunk,
    Exit,
    Error,
}

/// Streaming source surfaced to Swift.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxStreamSourceFfi {
    Run,
    Pty,
}

/// Streaming event emitted while a command / PTY is active.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxStreamEventFfi {
    pub sequence: u64,
    pub task_id: Option<String>,
    pub stream_id: String,
    pub source: MobileLinuxStreamSourceFfi,
    pub kind: MobileLinuxStreamEventKindFfi,
    pub text: Option<String>,
    pub data: Option<Vec<u8>>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Streaming sink for command / PTY output.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosMobileLinuxEventSink: Send + Sync {
    async fn on_event(
        &self,
        event: MobileLinuxStreamEventFfi,
    ) -> Result<(), MobileLinuxEventSinkFfiError>;
}

/// Host-visible mobile-linux operation failure.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum MobileLinuxOperationFfiError {
    #[error("unsupported on this platform")]
    Unsupported,
    #[error("runtime unavailable: {message}")]
    Unavailable { message: String },
    #[error("license blocked: {message}")]
    LicenseBlocked { message: String },
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    #[error("io error: {message}")]
    Io { message: String },
    #[error("timeout")]
    Timeout,
}

/// Error returned by the host event sink.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum MobileLinuxEventSinkFfiError {
    #[error("event sink rejected update: {message}")]
    Rejected { message: String },
}
