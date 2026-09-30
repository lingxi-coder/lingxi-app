/// Mobile Linux runtime mode exposed to the Android host.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxRuntimeModeFfi {
    MobileLinux,
}

impl From<MobileLinuxRuntimeModeFfi> for mobile_linux_api::MobileLinuxRuntimeMode {
    fn from(value: MobileLinuxRuntimeModeFfi) -> Self {
        match value {
            MobileLinuxRuntimeModeFfi::MobileLinux => Self::MobileLinux,
        }
    }
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

/// FFI capability snapshot for the Android mobile-linux runtime.
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

/// FFI rootfs status snapshot for the Android mobile-linux runtime.
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

/// FFI mount purpose for the mobile-linux runtime.
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

/// FFI mount descriptor for guest-visible paths.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxMountSpecFfi {
    pub host_path: String,
    pub guest_path: String,
    pub read_only: bool,
    pub purpose: MobileLinuxMountPurposeFfi,
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxEnvEntryFfi {
    pub key: String,
    pub value: String,
}

/// FFI request for a command executed inside the mobile-linux runtime.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxCommandRequestFfi {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<MobileLinuxEnvEntryFfi>,
    pub stdin: Option<String>,
    pub timeout_ms: Option<u64>,
    pub allow_network: bool,
    pub mounts: Vec<MobileLinuxMountSpecFfi>,
}

/// FFI command result returned by the mobile-linux runtime.
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

/// FFI background-process handle.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxProcessHandleFfi {
    pub id: String,
}

/// FFI PTY size.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone, Copy)]
pub struct MobileLinuxPtySizeFfi {
    pub cols: u16,
    pub rows: u16,
}

/// FFI PTY open request.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxPtyOpenRequestFfi {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<MobileLinuxEnvEntryFfi>,
    pub size: MobileLinuxPtySizeFfi,
    pub mounts: Vec<MobileLinuxMountSpecFfi>,
}

/// FFI PTY-session handle.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxPtySessionHandleFfi {
    pub id: String,
}

/// FFI stream sink for live command output.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidMobileLinuxStreamSink: Send + Sync {
    async fn stdout_line(&self, line: String) -> Result<(), MobileLinuxApiErrorFfi>;
    async fn stderr_chunk(&self, chunk: Vec<u8>) -> Result<(), MobileLinuxApiErrorFfi>;
}

/// FFI error surface for mobile-linux operations.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum MobileLinuxApiErrorFfi {
    #[error("mobile-linux runtime unavailable: {message}")]
    Unavailable { message: String },
    #[error("mobile-linux runtime license blocked: {message}")]
    LicenseBlocked { message: String },
    #[error("invalid mobile-linux request: {message}")]
    InvalidRequest { message: String },
    #[error("mobile-linux operation failed: {message}")]
    OperationFailed { message: String },
}

/// Event kind emitted by run/PTY streaming APIs.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxEventKindFfi {
    TaskStatusChanged,
    StdoutLine,
    StderrChunk,
    PtyOutput,
    PtyClosed,
    RuntimeError,
}

/// Streaming/runtime event delivered to the Android host.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxEventFfi {
    pub sequence: u64,
    pub task_id: Option<String>,
    pub kind: MobileLinuxEventKindFfi,
    pub data: Option<Vec<u8>>,
    pub text: Option<String>,
    pub session_id: Option<String>,
    pub status: Option<MobileLinuxTaskStateFfi>,
    pub exit_code: Option<i32>,
    pub timed_out: Option<bool>,
    pub cancelled: Option<bool>,
    pub detail: Option<String>,
}

/// Crate-local callback interface for mobile-linux events.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidMobileLinuxEventSink: Send + Sync {
    async fn on_event(&self, event: MobileLinuxEventFfi) -> Result<(), MobileLinuxApiErrorFfi>;
}

/// FFI task kind for host-visible runtime work.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxTaskKindFfi {
    Command,
    PtySession,
}

/// FFI task status for host-visible runtime work.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxTaskStateFfi {
    Queued,
    Running,
    Backgrounded,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

/// FFI task snapshot. Phase-1 keeps this fail-closed unless a real runtime is linked.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxTaskSnapshotFfi {
    pub task_id: String,
    pub status: MobileLinuxTaskStateFfi,
    pub command: String,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub detail: Option<String>,
}
