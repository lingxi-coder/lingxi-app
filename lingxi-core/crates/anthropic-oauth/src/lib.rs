//! Claude.ai OAuth client and Anthropic auth-source resolver.
//!
//! See spec §30 (Anthropic Auth). This crate owns:
//! - PKCE (RFC 7636) verifier/challenge + CSRF state generation
//! - Loopback HTTP listener for the redirect URI
//! - Auth-source resolver covering the 9 documented sources (A6)
//! - Static config + a placeholder rate-limit tracker
//!
//! The full OAuth dance (browser open + token exchange + refresh) is wired up
//! by the cli-demo in Plan 16; M1.19 ships the building blocks.

#![forbid(unsafe_code)]

pub mod callback;
pub mod client;
pub mod config;
pub mod limits;
pub mod pkce;
pub mod resolver;

pub use callback::{await_callback, CallbackError, CallbackParams};
pub use client::{ClaudeAiOAuthClient, OAuthError};
pub use config::ClaudeAiOAuthConfig;
pub use limits::{ClaudeAiLimitsState, ClaudeAiLimitsTracker, SubscriptionType};
pub use pkce::{generate_pkce, generate_state_token};
pub use resolver::{resolve, AuthSource, ResolverContext};
