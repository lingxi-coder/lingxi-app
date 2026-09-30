//! `bridge-server` library surface — the connection-scoped WebSocket server loop.
//!
//! The binary (`src/main.rs`) is the host entrypoint; this library holds the
//! reusable, testable transport routing so the F2 e2e suites can drive it
//! against a deterministic engine (mock streaming client + real tool) without a
//! process boundary.
//!
//! - [`server::BridgeConnection`] — the [`bridge::FramePump`] for one client
//!   connection: routes inbound [`client::protocol::commands::ClientCommand`]s to
//!   the engine and pushes outbound [`client::protocol::events::ClientEvent`]s /
//!   permission requests back as [`bridge::Frame`]s.
//! - [`server::TurnDriver`] — the engine entry the connection calls for a turn.
//! - [`driver::OrchestratorTurnDriver`] — the PRODUCTION [`server::TurnDriver`]
//!   over a real [`orchestrator::ConversationOrchestrator`]: one `run_turn`
//!   streams one turn whose engine events flow out through the connection's
//!   event sink (S1).
//! - [`router::CommandRouter`] — the engine-routing seam for the FULL command
//!   surface (model, listings, slash, tasks, session control) the connection
//!   delegates non-turn/non-permission commands to (F2-08).
//! - [`audio_bridge::AudioBridge`] — the desktop `lingxi_core::host::AudioService`
//!   proxy: each device operation becomes one identity-scoped
//!   [`client::protocol::events::ClientEvent::AudioRequest`] awaiting the client's
//!   `AudioResponse` (the microphone/speaker live in Electron, not in the engine).
//! - [`boot`] — S2: env/argv → [`harness_runtime::desktop::DesktopConfig`] resolution and
//!   the assembly of a fully-bound [`server::BridgeConnection`] from a real
//!   [`harness_runtime::desktop::DesktopRuntime`], shared by the binary and its tests.

#![forbid(unsafe_code)]
// Documentation debt, not a decision that docs do not matter: this crate had
// 3 undocumented public item(s) when `missing_docs` was measured across the
// workspace (2026-09-16). The lint stays `warn` at the workspace level so a NEW
// crate still inherits the requirement; this allow is scoped here so the debt
// is visible per crate and can be repaid one crate at a time by deleting this
// line.
#![allow(missing_docs)]
// Dead code kept visible, not swept: this crate had 1 item(s) rustc could
// reach from nothing when the workspace was measured (2026-09-16). The lint
// stays `warn` at the workspace level so a NEW crate still inherits it; this
// allow is scoped here so the count is per crate and repayable by deleting this
// line. This is the category where "named, computed, never wired" hides — some
// of these read like features that were built and never connected. Each wants a
// decision (delete, or wire), not a blanket deletion.
// ⚠️ The count above is ONE macOS, lib-target measurement. It is not a list of
// deletable items — see docs/HANDOFF-dead-code-adjudication-2026-09-17.md,
// which records two near-misses where it said "dead" about live code.
#![allow(dead_code)]

#[cfg(unix)]
pub(crate) use platform_posix::PosixFileSystem as HostFileSystem;
#[cfg(windows)]
pub(crate) use platform_windows::WindowsFileSystem as HostFileSystem;

pub mod audio_bridge;
pub mod boot;
pub use configuration_admin::config_admin;
mod cron_host;
pub mod desktop_terminal;
pub mod driver;
pub use configuration_admin::hook_admin;
pub use configuration_admin::mcp_admin;
pub use configuration_admin::mcp_bridge;
pub use configuration_admin::plugin_admin;
mod provider_connection;
pub mod router;
pub mod server;
pub use configuration_admin::settings_bridge;
pub use configuration_admin::skills_admin;
