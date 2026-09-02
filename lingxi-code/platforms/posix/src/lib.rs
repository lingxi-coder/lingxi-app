//! Production POSIX platform crate (Linux + macOS).
//!
//! Implements `lingxi-traits` interfaces using real OS APIs. Replaces the
//! M1 `posix-minimal` crate for desktop runnable scenarios.
//!
//! ## `unsafe_code`
//!
//! The workspace lint is `deny(unsafe_code)`. Exactly one module in this
//! crate — [`process::spawn_unsafe`] — opts into `#![allow(unsafe_code)]`
//! for a single call to `libc::setsid()` from `CommandExt::pre_exec`. That
//! call is required so the background-spawned child becomes a process-group
//! leader, which lets [`process::kill_tree::kill_tree_unix`] tree-kill its
//! descendants via `killpg(2)`. See plan
//! `docs/superpowers/plans/2026-05-23-m2-06-securestorage-sse-process.md`
//! Tasks 14-15.

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
pub(crate) mod watch_helper;
pub mod worktree;
pub mod worktree_tmux;
pub mod wsl_detect;

pub use bridge::PosixBridgeTransport;
pub use clock::PosixClock;
pub use fs::PosixFileSystem;
pub use http::PosixHttp;
pub use lsp::PosixLspTransport;
pub use mcp::PosixMcpTransport;
// Convenience re-exports at the crate root so callers can write
// `platform_posix::{connect_ws, spawn_stdio}` directly.
pub use mcp::{connect_ws, spawn_stdio, McpTransportError};
pub use process::PosixProcess;
pub use runtime::PosixRuntime;
pub use sandbox::PosixSandbox;
pub use secure_storage::{
    plaintext_secure_storage, secure_storage_for_platform, secure_storage_for_policy,
    LinuxSecretStorage, MacOsKeychainStorage, PlainTextSecureStorage,
};
pub use swarm::TmuxSwarmBackend;
pub use worktree::PosixWorktreeManager;
