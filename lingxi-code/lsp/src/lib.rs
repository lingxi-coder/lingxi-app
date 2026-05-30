//! LSP (Language Server Protocol) registry and core primitives.
//!
//! M1.18 ships the engine-side surface: per-server state machine
//! ([`LspConnectionState`]), routing registry ([`LspRegistry`]), action
//! payload ([`LspAction`]).
//!
//! The `Tool`-implementing `LSPTool` lives in `lingxi-tools` (M4-07) to
//! break a `lingxi-lsp → lingxi-tools` cycle now that M4-07 needs
//! `lingxi-tools → lingxi-lsp` for `LspClient` + `tool_operations`.
//!
//! See spec §25 (LSP).

#![forbid(unsafe_code)]

pub mod action;
pub mod client;
pub mod config;
pub mod connection;
pub mod diagnostic_registry;
pub mod open_file_tracker;
pub mod passive_feedback;
pub mod registry;
pub mod tool_operations;
pub mod transport;

pub use action::{LspAction, LspResponse};
pub use client::LspClient;
pub use connection::LspConnectionState;
pub use diagnostic_registry::{DiagnosticEntry, LspDiagnosticRegistry};
pub use open_file_tracker::OpenFileTracker;
pub use passive_feedback::PassiveDiagnosticSubscriber;
pub use registry::LspRegistry;
pub use tool_operations::{
    LspOperation, LspOperationError, LspOperationResult, MAX_LSP_FILE_SIZE_BYTES,
};
