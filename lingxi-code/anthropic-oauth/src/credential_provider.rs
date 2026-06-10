//! `llm_client::CredentialProvider` over the OAuth refresh machinery.
//!
//! [`OAuthCredentialProvider`] serves the current OAuth access token,
//! refreshing in place (single-flight via the underlying `refresh_lock`)
//! when the token is expired per the state's clock.

use std::fmt;
use std::sync::Arc;

use api_client::oauth_hook::OAuthRefreshHook;
use llm_client::{BoxFuture, Credential, CredentialProvider, CredentialScope, LlmError};

use crate::refresh::RefreshDriver;

/// Serves the current OAuth access token, refreshing in place when expired
/// (single-flight via the underlying refresh lock).
///
/// Wraps an `Arc<RefreshDriver>` and bridges into `llm_client`'s credential
/// seam without requiring the `api-client` hook indirection at the call site.
pub struct OAuthCredentialProvider {
    driver: Arc<RefreshDriver>,
}

impl OAuthCredentialProvider {
    /// Wrap a refresh driver.
    #[must_use]
    pub fn new(driver: Arc<RefreshDriver>) -> Self {
        Self { driver }
    }
}

impl fmt::Debug for OAuthCredentialProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthCredentialProvider").finish_non_exhaustive()
    }
}

impl CredentialProvider for OAuthCredentialProvider {
    fn load<'a>(
        &'a self,
        _scope: &'a CredentialScope,
    ) -> BoxFuture<'a, Result<Credential, LlmError>> {
        Box::pin(async move {
            let state = &self.driver.state;

            // Check expiry under a read lock.
            let (is_expired, token_hash, access_token_str) = {
                let token = state.token.read().await;
                let now = state.clock.now();
                let expired = token.expires_at <= now;
                let hash = token.token_hash();
                // Clone the string out from under the lock (Secret does not
                // implement Clone; we expose only at this final conversion point).
                let s = token.access_token.expose_secret().clone();
                (expired, hash, s)
            };

            if !is_expired {
                return Ok(Credential::BearerToken(access_token_str));
            }

            // Expired → single-flight refresh (double-check-after-acquire lives
            // inside `RefreshDriver::refresh`).  Map any failure to
            // LlmError::Authentication with NO secret material in the message.
            let bearer = <RefreshDriver as OAuthRefreshHook>::refresh(
                &*self.driver,
                token_hash,
            )
            .await
            .map_err(|_| LlmError::Authentication)?;

            Ok(Credential::BearerToken(
                bearer.0.expose_secret().clone(),
            ))
        })
    }
}
