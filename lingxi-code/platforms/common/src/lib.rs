//! Cross-platform MCP transport helpers shared by `platforms/posix` and
//! `platforms/windows`.
//!
//! Stdio spawn lives per-platform because process creation differs slightly
//! between POSIX and Windows, but the WebSocket / SSE / HTTP connectors and
//! small shared utilities (stderr ring buffer, `StdioConfig`) live here to
//! avoid duplication.

#![forbid(unsafe_code)]

pub mod http;
pub mod mcp_http;
pub mod mcp_sse;
pub mod mcp_stdio;
pub mod mcp_ws;

pub use http::ReqwestHttp;
pub use mcp_http::{connect_http, HttpConnectError};
pub use mcp_sse::{connect_sse, SseConnectError, IDE_AUTH_HEADER};
