//! Sandbox network-filtering subsystem (Linux bwrap `--unshare-net` + host
//! forward-proxy domain allowlisting). Faithful port of
//! `@anthropic-ai/sandbox-runtime@0.0.54` (vendored at
//! `docs/superpowers/references/sandbox-runtime-0.0.54/`).
//!
//! P1 (this): the pure core — domain pattern grammar, the host matcher, the
//! host validators/canonicalizers, and the network config. No proxy/async/bwrap.

#![forbid(unsafe_code)]

pub mod config;
pub mod connect_proxy;
pub mod dial;
pub mod host;
pub mod matcher;
pub mod parent_proxy;
