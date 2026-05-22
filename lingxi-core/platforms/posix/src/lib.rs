//! Production POSIX platform crate (Linux + macOS).
//!
//! Implements `lingxi-traits` interfaces using real OS APIs. Replaces the
//! M1 `posix-minimal` crate for desktop runnable scenarios.

#![forbid(unsafe_code)]

pub mod clock;
pub mod fs;
pub mod http;
pub mod process;
pub mod runtime;
pub mod secure_storage;
pub mod worktree;

pub use clock::PosixClock;
pub use fs::PosixFileSystem;
pub use http::PosixHttp;
pub use process::PosixProcess;
pub use runtime::PosixRuntime;
pub use secure_storage::PlainTextSecureStorage;
pub use worktree::PosixWorktreeManager;
