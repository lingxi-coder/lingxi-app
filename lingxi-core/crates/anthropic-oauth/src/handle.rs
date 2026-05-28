//! `AuthHandle` implementation backed by the `ClaudeAiOAuthClient`.
//!
//! M5-11 ships a thin wrapper that surfaces the trait so the `/login` and
//! `/logout` slash-command handlers can compile + run. The full PKCE
//! interactive flow + keychain-clear logic is staged for M5-12 (CLI
//! binary): M5-12 will replace this wrapper with a real impl that drives
//! the existing [`ClaudeAiOAuthClient::build_authorize_url`] + the
//! `callback::await_callback` loopback listener and persists tokens via
//! [`lingxi_secret::CredentialManager`].
//!
//! Until then, [`OAuthHandle::login`] returns
//! [`AuthError::ServerError`] with a clear "not wired in M5-11" payload —
//! the `/login` handler will render this as
//! `"Could not log in: server rejected: …"`. [`OAuthHandle::logout`] is
//! best-effort and idempotent; [`OAuthHandle::current_user`] returns
//! `None` because M5-11 does not yet read the keychain.

use crate::client::ClaudeAiOAuthClient;
use async_trait::async_trait;
use lingxi_traits::{AuthError, AuthHandle, LoginInfo};
use std::sync::Arc;

/// `AuthHandle` impl wrapping the `ClaudeAiOAuthClient`. Thin in M5-11;
/// M5-12 (CLI binary) replaces the bodies with a real PKCE drive + keychain
/// integration.
pub struct OAuthHandle {
    #[allow(dead_code)]
    client: Arc<ClaudeAiOAuthClient>,
}

impl OAuthHandle {
    /// Construct a new handle wrapping the OAuth client.
    #[must_use]
    pub fn new(client: Arc<ClaudeAiOAuthClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl AuthHandle for OAuthHandle {
    async fn login(&self) -> Result<LoginInfo, AuthError> {
        // M5-12 will run the full PKCE flow here:
        // 1. `self.client.build_authorize_url()` (existing).
        // 2. Open the browser at the returned URL.
        // 3. `await_callback(...)` on the loopback listener.
        // 4. Exchange the code for tokens.
        // 5. Decode the ID token → email + org_id.
        // 6. Persist via `lingxi_secret::CredentialManager`.
        // 7. Return LoginInfo.
        Err(AuthError::ServerError(
            "interactive login not yet wired in v0.6.0 (M5-11); CLI binary in M5-12 will plug this in"
                .to_string(),
        ))
    }

    async fn logout(&self) -> Result<(), AuthError> {
        // Best-effort: there is nothing for M5-11 to clear yet (keychain
        // wiring lives in M5-12). The operation is idempotent and always
        // succeeds at this layer.
        Ok(())
    }

    async fn current_user(&self) -> Option<LoginInfo> {
        // M5-12 will read `lingxi_secret::CredentialManager` and decode the
        // cached ID token. Until then, we have no source of truth — return
        // `None` so `/status` shows the "not logged in" branch.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The `OAuthHandle` itself is exercised by the M5-11 integration test
    // (`commands/tests/batch_2_e2e.rs`) via the trait — that test runs the
    // /login + /logout slash commands end-to-end against a `MockAuth`
    // implementation. M5-11's `OAuthHandle` impl on the real
    // `ClaudeAiOAuthClient` is exercised once M5-12 wires the CLI binary
    // (which constructs a real `ClaudeAiOAuthClient` + spins the PKCE
    // flow). The unit test here just asserts that the type is `Send +
    // Sync` and dyn-compatible.

    #[allow(dead_code)]
    fn _oauth_handle_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OAuthHandle>();
    }

    #[test]
    fn auth_handle_trait_object_compiles() {
        // Verifies that the trait can be erased into `dyn AuthHandle`.
        // The concrete value type checking happens at the CLI integration
        // layer in M5-12.
        fn _accept(_h: &Arc<dyn AuthHandle>) {}
    }
}
