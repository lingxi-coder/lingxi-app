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
//! - [`router::CommandRouter`] — the engine-routing seam for the FULL command
//!   surface (model, listings, slash, tasks, session control) the connection
//!   delegates non-turn/non-permission commands to (F2-08).

#![forbid(unsafe_code)]

pub mod router;
pub mod server;
