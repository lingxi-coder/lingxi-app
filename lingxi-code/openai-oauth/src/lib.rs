//! OpenAI / ChatGPT-account OAuth login for llm-client.
//!
//! Mirrors the `anthropic-oauth` crate: PKCE + device-code login, token
//! refresh, and an `llm_client::CredentialProvider` that serves a
//! `Credential::ChatGptOAuth` (bearer + ChatGPT-Account-ID). Byte-aligned with
//! codex's ChatGPT auth (see docs/superpowers/specs/2026-06-16-p2-chatgpt-oauth-login-design.md).

pub mod callback;
pub mod config;
pub mod pkce;
