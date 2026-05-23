//! `lingxi-jsonrpc` — protocol-agnostic JSON-RPC 2.0 framing and routing.
//!
//! Used by `lingxi-mcp`, `lingxi-bridge`, and `lingxi-lsp`. See plan
//! `docs/superpowers/plans/2026-05-23-m2-02a-jsonrpc.md`.

#![forbid(unsafe_code)]

pub mod codec;
pub mod inbound;
pub mod messages;
pub mod router;

pub use codec::{CodecError, LineCodec, LspCodec};
pub use inbound::{BoxedHandler, Dispatcher, InboundHandler};
pub use messages::{
    Id, Message, Notification, Request, Response, ResponseError, INTERNAL_ERROR, INVALID_PARAMS,
    INVALID_REQUEST, JSONRPC_VERSION, METHOD_NOT_FOUND, PARSE_ERROR,
};
pub use router::{OutboundMessage, Router, RouterError, DEFAULT_TIMEOUT};

/// Crate version constant — kept in sync with `Cargo.toml`.
pub const VERSION: &str = "0.1.0";
