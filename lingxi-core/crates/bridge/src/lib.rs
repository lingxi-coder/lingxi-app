//! `lingxi-bridge` — IDE bridge over MCP-WebSocket.
//!
//! After M2-02d this crate is a thin lockfile-discovery + transport-spec
//! builder. The cloud Remote Control bridge (claude.ai workers) is deferred
//! to a separate milestone per spec §5. The local IDE bridge:
//!
//! 1. [`lockfile::IdeLockfile`] writes `~/.claude/ide/<port>.lock` with the
//!    auth token an IDE plugin must echo back in the
//!    `X-Claude-Code-Ide-Authorization` header.
//! 2. [`LockfileGuard`] removes that file on shutdown AND on panic.
//! 3. [`state::BridgeState`] is the observable connection snapshot held by
//!    the engine for UI / telemetry.
//! 4. [`IdeBridge`] glues the lockfile + transport stack into the engine's
//!    MCP registry (full wiring lands in later M2-02d tasks).

#![forbid(unsafe_code)]

pub mod lockfile;
pub mod state;
pub mod transport;

pub use lockfile::{IdeLockfile, LockfileBody, LockfileGuard, IDE_NAME, TRANSPORT};
pub use state::BridgeState;
pub use transport::IdeBridge;
