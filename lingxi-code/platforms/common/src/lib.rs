//! Cross-platform MCP transport helpers shared by `platforms/posix` and
//! `platforms/windows`.
//!
//! Stdio spawn lives per-platform because process creation differs slightly
//! between POSIX and Windows, but the WebSocket / SSE / HTTP connectors and
//! small shared utilities (stderr ring buffer, `StdioConfig`) live here to
//! avoid duplication.
//!
//! Also hosts the shared built-in Anthropic LLM config so `engine-desktop` and
//! `engine-mobile` stay in sync without duplicating the model table.

#![forbid(unsafe_code)]

pub mod http;
pub mod llm_config;
pub mod llm_transport;
pub mod mcp_http;
pub mod mcp_sse;
pub mod mcp_stdio;
pub mod mcp_ws;

pub use http::ReqwestHttp;
pub use llm_config::{apply_settings_providers, builtin_anthropic_config};
pub use llm_transport::LlmTransportBridge;
pub use mcp_http::{connect_http, HttpConnectError};
pub use mcp_sse::{connect_sse, SseConnectError, IDE_AUTH_HEADER};
