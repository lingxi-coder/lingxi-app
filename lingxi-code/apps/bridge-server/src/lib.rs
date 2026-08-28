//! `bridge-server` library surface — the connection-scoped WebSocket server loop.
//!
//! The binary (`src/main.rs`) is the host entrypoint; this library holds the
//! reusable, testable transport routing so the F2 e2e suites can drive it
//! against a deterministic engine (mock streaming client + real tool) without a
//! process boundary.
//!
//! - [`server::BridgeConnection`] — the [`bridge::FramePump`] for one client
//!   connection: routes inbound [`client_protocol::commands::ClientCommand`]s to
//!   the engine and pushes outbound [`client_protocol::events::ClientEvent`]s /
//!   permission requests back as [`bridge::Frame`]s.
//! - [`server::TurnDriver`] — the engine entry the connection calls for a turn.
//! - [`driver::OrchestratorTurnDriver`] — the PRODUCTION [`server::TurnDriver`]
//!   over a real [`orchestrator::ConversationOrchestrator`]: one `run_turn`
//!   streams one turn whose engine events flow out through the connection's
//!   event sink (S1).
//! - [`router::CommandRouter`] — the engine-routing seam for the FULL command
//!   surface (model, listings, slash, tasks, session control) the connection
//!   delegates non-turn/non-permission commands to (F2-08).
//! - [`boot`] — S2: env/argv → [`engine_desktop::DesktopConfig`] resolution and
//!   the assembly of a fully-bound [`server::BridgeConnection`] from a real
//!   [`engine_desktop::DesktopRuntime`], shared by the binary and its tests.

#![forbid(unsafe_code)]

pub mod boot;
pub mod driver;
pub mod mcp_bridge;
pub mod router;
pub mod server;
pub mod settings_bridge;
