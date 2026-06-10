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
//! plus `NO_PROXY` resolution and the CONNECT tunnel dialers in [`parent_proxy`],
//! CONNECT-target parsing plus the bounded direct dial in [`dial`], the
//! per-request filter body hook in [`request_filter`], and the unified hyper
//! forward proxy (plain-HTTP forwarding + `CONNECT` tunnelling + parent-proxy
//! chaining) in [`http_proxy`]. Remaining: the `socat`/bwrap bridge, `SOCKS5`,
//! TLS-MITM (P6), seccomp, the `sandbox-manager` orchestration, the
//! macOS/Windows backends, and the CLI.

#![forbid(unsafe_code)]

pub mod config;
pub mod dial;
pub mod env;
pub mod fs_args;
pub mod host;
pub mod http_proxy;
pub mod linux;
pub mod macos;
pub mod manager;
pub mod matcher;
pub mod mitm_ca;
pub mod mitm_leaf;
pub mod parent_proxy;
pub mod path_utils;
pub mod request_filter;
pub mod socks_proxy;
pub mod tls_terminate;
pub mod violation_store;
pub mod windows;
