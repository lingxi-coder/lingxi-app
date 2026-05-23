//! LSP (Language Server Protocol) registry, action enum, and Tool wiring.
//!
//! M1.18 ships the engine-side surface: per-server state machine
//! ([`LspConnectionState`]), routing registry ([`LspRegistry`]), action
//! payload ([`LspAction`]), and the [`LspTool`] that exposes LSP to the
//! model. The production `LspTransport` (stdio JSON-RPC) is wired up in
//! Plan 16 inside `platforms/posix-minimal`.
//!
//! See spec §25 (LSP).

#![forbid(unsafe_code)]

pub mod action;
pub mod client;
pub mod config;
pub mod connection;
pub mod diagnostic_registry;
pub mod passive_feedback;
pub mod registry;
pub mod tool;
pub mod transport;

pub use action::{LspAction, LspResponse};
pub use client::LspClient;
pub use connection::LspConnectionState;
pub use diagnostic_registry::{DiagnosticEntry, LspDiagnosticRegistry};
pub use passive_feedback::PassiveDiagnosticSubscriber;
pub use registry::LspRegistry;
pub use tool::LspTool;
