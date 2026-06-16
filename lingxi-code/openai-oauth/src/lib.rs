//! OpenAI / ChatGPT-account OAuth login for llm-client.
//!
//! Mirrors the `anthropic-oauth` crate: PKCE + device-code login, token
//! refresh, and an `llm_client::CredentialProvider` that serves a
//! `Credential::ChatGptOAuth` (bearer + ChatGPT-Account-ID). Byte-aligned with
//! codex's ChatGPT auth (see docs/superpowers/specs/2026-06-16-p2-chatgpt-oauth-login-design.md).

#![forbid(unsafe_code)]

pub mod callback;
pub mod client;
pub mod config;
pub mod credential_provider;
pub mod device_code;
pub mod pkce;
pub mod refresh;
pub mod token_data;

#[cfg(any(test, feature = "testsupport"))]
pub mod testsupport;

pub use client::{init_refresh_driver, OAuthError, OpenAiOAuthClient};
pub use config::OpenAiOAuthConfig;
pub use credential_provider::OpenAiOAuthCredentialProvider;
pub use device_code::{request_device_code, run_device_code_login, DeviceUserCode, PollOutcome};
pub use refresh::{AuthState, RefreshDriver};
pub use token_data::{parse_id_token, IdTokenClaims};
