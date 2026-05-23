//! Production POSIX platform crate (Linux + macOS).
//!
//! Implements `lingxi-traits` interfaces using real OS APIs. Replaces the
//! M1 `posix-minimal` crate for desktop runnable scenarios.

#![forbid(unsafe_code)]

pub mod bridge;
pub mod clock;
pub mod fs;
pub mod http;
pub mod lsp;
pub mod mcp;
pub mod process;
pub mod runtime;
pub mod sandbox;
pub mod secure_storage;
pub mod swarm;
pub mod worktree;

pub use bridge::PosixBridgeTransport;
pub use clock::PosixClock;
pub use fs::PosixFileSystem;
pub use http::PosixHttp;
pub use lsp::PosixLspTransport;
pub use mcp::PosixMcpTransport;
// Convenience re-exports at the crate root so callers can write
// `lingxi_platform_posix::{connect_ws, spawn_stdio}` directly.
pub use mcp::{connect_ws, spawn_stdio, McpTransportError};
pub use process::PosixProcess;
pub use runtime::PosixRuntime;
pub use sandbox::PosixSandbox;
pub use secure_storage::PlainTextSecureStorage;
pub use swarm::TmuxSwarmBackend;
pub use worktree::PosixWorktreeManager;
