//! POSIX swarm backends: tmux (real), `iTerm` (`AppleScript`), `InProcess` (no-pane).
//!
//! Module layout mirrors claude-code `src/utils/swarm/backends/`:
//! - `detection`  — env + `which` probes
//! - `tmux`       — real `tmux` shell-out backend
//! - `iterm`      — AppleScript-via-`osascript` backend (Task 13)
//! - `inprocess`  — no-pane fallback that's always available (Task 14)
//! - `registry`   — auto-detect + construct the appropriate `Box<dyn SwarmBackend>` (Task 15)
//!
//! See spec §6.5 and the M2-05 plan for parity details.

pub mod detection;
pub mod inprocess;
pub mod iterm;
pub mod registry;
pub mod tmux;

pub use inprocess::InProcessSwarmBackend;
pub use iterm::ITermSwarmBackend;
pub use registry::SwarmRegistry;
pub use tmux::TmuxBackend;

/// Back-compat alias for the legacy v0.2.0 name. Existing callers
/// (`platform_posix::TmuxSwarmBackend`) continue to resolve to the
/// new real `TmuxBackend`.
pub type TmuxSwarmBackend = TmuxBackend;
