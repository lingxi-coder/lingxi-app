//! `sandbox-runtime` is a complete behavioral 1:1 Rust rewrite of the
//! `@anthropic-ai/sandbox-runtime@0.0.54` npm package (vendored reference at
//! `docs/superpowers/references/sandbox-runtime-0.0.54/`). It provides OS-level
//! filesystem and network sandboxing for arbitrary processes via `bubblewrap`
//! (Linux), `sandbox-exec` (macOS), and `AppContainer` (Windows), with
//! proxy-based network domain filtering: a host HTTP/`SOCKS` forward proxy plus
//! an optional TLS-MITM layer, bridged into the bwrap `--unshare-net` network
//! namespace via `socat`.
//!
//! Built in phases (see the umbrella design spec under
//! `docs/superpowers/specs/`). Landed so far: the pure core (domain pattern
//! grammar plus host matcher in [`config`], [`host`], [`matcher`]), parent-proxy
//! plus `NO_PROXY` resolution in [`parent_proxy`], and the base `CONNECT`
//! allowlist proxy in [`dial`] and [`connect_proxy`]. Remaining: proxy
//! completion (plain-HTTP forwarding, parent-proxy chaining, the request-filter
//! body hook), the `socat`/bwrap bridge, `SOCKS5`, TLS-MITM, seccomp, the
//! `sandbox-manager` orchestration, the macOS/Windows backends, and the CLI.

#![forbid(unsafe_code)]

pub mod config;
pub mod connect_proxy;
pub mod dial;
pub mod host;
pub mod matcher;
pub mod parent_proxy;
