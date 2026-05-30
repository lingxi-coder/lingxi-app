//! OAuth 2.1 PKCE handshake state (skeleton).
//!
//! Plan 13 lands the full state machine — PKCE generation, callback
//! listener, token exchange, refresh — backed by `lingxi-secret`. This
//! module ships only the state enum so downstream crates can reference
//! the variants.

use std::time::SystemTime;

/// Stages of the OAuth handshake.
///
/// `Secret<T>` deliberately does not implement `Clone`, so neither does
/// `OAuthState`; the registry holds the value behind an `RwLock` and
/// moves between variants when a transition happens.
#[derive(Debug)]
pub enum OAuthState {
    /// Local PKCE pair generated, listener about to start.
    Initiated {
        /// Loopback port the callback listener will bind to.
        callback_port: u16,
        /// PKCE code verifier.
        code_verifier: String,
        /// CSRF state token.
        state_token: String,
    },
    /// Listening on the loopback callback URL for the auth code.
    AwaitingCallback {
        /// Loopback port the callback listener is bound to.
        callback_port: u16,
        /// PKCE code verifier carried forward to token exchange.
        code_verifier: String,
        /// CSRF state token that must match the redirect.
        state_token: String,
        /// Authorization URL shown to the user.
        auth_url: String,
    },
    /// Exchanging the received code for tokens.
    ExchangingCode {
        /// Authorization code returned by the redirect.
        code: String,
    },
    /// Tokens acquired and ready for use.
    Authenticated {
        /// Bearer access token.
        access_token: lingxi_protocol::Secret<String>,
        /// Optional long-lived refresh token.
        refresh_token: Option<lingxi_protocol::Secret<String>>,
        /// Expiry of the current access token.
        expires_at: SystemTime,
    },
    /// Refreshing the access token using the refresh token.
    Refreshing {
        /// Refresh token being exchanged.
        refresh_token: lingxi_protocol::Secret<String>,
    },
}
