//! Platform abstraction traits.
//!
//! Engine crates depend on these traits; platform crates (`platforms/posix`,
//! `platforms/windows`, etc.) implement them. The engine never imports a
//! concrete runtime or OS API directly.
//!
//! See spec §4 (Trait System) and D17 (Runtime boundary).

#![forbid(unsafe_code)]

pub mod bridge;
pub mod clock;
pub mod effect_handler;
pub mod filesystem;
pub mod http;
pub mod lsp;
pub mod mcp;
pub mod process;
pub mod runtime;
pub mod sandbox;
pub mod secure_storage;
pub mod swarm;
pub mod worktree;

pub use bridge::{BridgeConfig, BridgeConnection, BridgeError, BridgeTransport};
pub use clock::Clock;
pub use effect_handler::EffectHandler;
pub use filesystem::{FileContent, FileEvent, FileEventKind, FileSystem, FlockGuard, FsError};
pub use http::{HttpError, HttpTransport};
pub use lsp::{LspError, LspRawConnection, LspServerCapabilities, LspServerConfig, LspTransport};
pub use mcp::*;
pub use process::{ProcessError, ProcessHandle, ProcessOutput, ProcessRunner};
pub use runtime::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};
pub use sandbox::{
    NetworkPolicy, ProcessCommand, ResourceLimits, Sandbox, SandboxBackend, SandboxCapability,
    SandboxError, SandboxFeatures, SandboxPolicy, SandboxedCommand, SandboxedTag,
};
pub use secure_storage::{SecureStorage, SecureStorageBackend, SecureStorageError};
pub use swarm::{PaneId, PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};
#[allow(unused_imports)]
pub use worktree::*;
pub use worktree::{WorktreeError, WorktreeHandle, WorktreeInfo, WorktreeManager};
