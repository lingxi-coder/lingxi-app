//! Claude.ai OAuth client and Anthropic auth-source resolver.
//!
//! See spec §30 (Anthropic Auth). This crate owns:
//! - PKCE (RFC 7636) verifier/challenge + CSRF state generation
//! - Loopback HTTP listener for the redirect URI
//! - Auth-source resolver covering the 9 documented sources (A6)
//! - Static config + a placeholder rate-limit tracker
//! - Reactive + proactive token refresh (M3-04)
//! - Scope upgrade flow (M3-04)
//!
//! M3-04 implements `lingxi_api_client::oauth_hook::OAuthRefreshHook` (frozen
//! in M3-03 §3); we extend, never modify, the api-client trait surface.

#![forbid(unsafe_code)]

// `callback` uses `tokio::net::TcpListener` which tokio gates out under
// `--cfg loom`. Loom doesn't model network primitives anyway — the loom test
// only exercises the single-flight invariant on synthetic atomics — so we
// exclude this module from loom builds. Normal builds are unaffected.
#[cfg(not(loom))]
pub mod callback;
pub mod client;
pub mod config;
pub mod limits;
pub mod pkce;
pub mod refresh;
pub mod resolver;
pub mod scope_upgrade;

#[cfg(not(loom))]
pub use callback::{await_callback, CallbackError, CallbackParams};
pub use client::{ClaudeAiOAuthClient, OAuthError};
pub use config::{ClaudeAiOAuthConfig, CLAUDE_CODE_OAUTH_SCOPES, REFRESH_GRANT_TYPE};
pub use limits::{ClaudeAiLimitsState, ClaudeAiLimitsTracker, SubscriptionType};
pub use pkce::{generate_pkce, generate_state_token};
pub use refresh::{AuthState, RefreshDriver};
pub use resolver::{resolve, AuthSource, ResolverContext};
pub use scope_upgrade::{
    parse_scope_upgrade, run_scope_upgrade, PkceRunResult, PkceRunner, ScopeUpgradeRequired,
};
