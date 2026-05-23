//! Frozen cross-plan trait surface for OAuth token refresh.
//!
//! **This file is the M3-03 contract surface; M3-04 implements without
//! modifying.** Spec §3 (lines 240-273) freezes the trait body. Adding
//! new methods or changing existing signatures after this file lands is
//! forbidden by the "extends not modifies" cross-plan rule.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use lingxi_protocol::Secret;
use std::sync::{Arc, OnceLock};
use thiserror::Error;

/// Trait that an OAuth provider implementation (M3-04) registers with the
/// api-client middleware. The middleware calls `refresh` reactively on HTTP
/// 401 and calls `proactive_refresh` on a timer if a long-lived registration
/// is configured.
///
/// **Single-flight contract (v3 §16.3)**: concurrent reactive + proactive
/// invocations during the same expiry window MUST collapse to one HTTP
/// refresh. The implementation owns the lock; this trait does not enforce
/// it (the impl-side `refresh_lock: Arc<Mutex<()>>` is M3-04's responsibility).
#[async_trait]
pub trait OAuthRefreshHook: Send + Sync + 'static {
    /// Invoked by api-client's middleware on HTTP 401. Returns either a
    /// fresh bearer token to retry with, or an error to surface to the
    /// caller. Must be single-flight: concurrent invocations during the
    /// same expiry window collapse to one refresh.
    async fn refresh(&self, prev_token_hash: TokenHash) -> Result<BearerToken, OAuthHookError>;

    /// Optional hook for proactive refresh; api-client calls this on a
    /// timer once `register_proactive` is consumed. Default: no-op so
    /// implementations that only handle reactive 401 don't need to opt in.
    async fn proactive_refresh(&self) -> Result<(), OAuthHookError> {
        Ok(())
    }
}

/// Bearer token wrapper. Field is `pub` so M3-04 can construct one without
/// an opaque accessor — this is part of the frozen cross-plan contract.
///
/// Note: does not implement `Clone` because `Secret<T>` intentionally
/// does not (see `lingxi-protocol/src/secret.rs` line 76); callers must
/// wrap in `Arc` if they need shared ownership.
#[derive(Debug)]
pub struct BearerToken(pub Secret<String>);

/// SHA-256 of the in-use token. Computation lives in M3-04; api-client only
/// passes the value through to `refresh()` so the impl can detect whether
/// another caller already rotated the token under the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenHash(pub [u8; 32]);

/// Errors returned by an `OAuthRefreshHook` implementation. Variants are
/// frozen — M3-04 matches on these exact names.
#[derive(Debug, Clone, Error)]
pub enum OAuthHookError {
    /// Refresh attempted but the `IdP` rejected the `refresh_token`.
    #[error("refresh failed: {0}")]
    RefreshFailed(String),

    /// The token hash passed to `refresh` is older than what the hook
    /// has stored; another caller already rotated. Caller should retry
    /// with the freshly-stored token instead of triggering another refresh.
    #[error("token stale; reload from store")]
    TokenStale,

    /// Network or transport failure reaching the `IdP`. Distinguishes from
    /// `RefreshFailed` (which is an `IdP`-side rejection).
    #[error("provider unreachable: {0}")]
    ProviderUnreachable(String),
}

/// Process-global middleware registration errors.
#[derive(Debug, Clone, Error)]
pub enum MiddlewareError {
    /// A hook was already registered for this process. M3-04 wires once at
    /// `Engine::init()`; multiple `register_oauth_hook` calls indicate a bug
    /// (e.g. two engines in the same process).
    #[error("OAuth hook already registered")]
    HookAlreadyRegistered,
}

/// Default no-op hook so M3-03 can land before M3-04 ships its concrete
/// impl. Returns `OAuthHookError::TokenStale` so a caller exercising the
/// hook path in tests fails fast rather than spinning.
#[derive(Debug, Default)]
pub struct NoOpOAuthHook;

#[async_trait]
impl OAuthRefreshHook for NoOpOAuthHook {
    async fn refresh(&self, _prev: TokenHash) -> Result<BearerToken, OAuthHookError> {
        Err(OAuthHookError::TokenStale)
    }
}

static REGISTERED_HOOK: OnceLock<Arc<dyn OAuthRefreshHook>> = OnceLock::new();

/// Register the process-global OAuth hook. M3-04 calls this once at
/// `Engine::init()`. Second + calls return `MiddlewareError::HookAlreadyRegistered`.
///
/// # Errors
/// Returns [`MiddlewareError::HookAlreadyRegistered`] if a hook is already in place.
pub fn register_oauth_hook(hook: Arc<dyn OAuthRefreshHook>) -> Result<(), MiddlewareError> {
    REGISTERED_HOOK
        .set(hook)
        .map_err(|_| MiddlewareError::HookAlreadyRegistered)
}

/// Internal accessor used by `anthropic.rs::do_request_with_middleware`.
/// Returns `NoOpOAuthHook` when no hook has been registered — keeps tests
/// that don't care about auth from having to plumb a real impl.
#[must_use]
pub fn current_hook() -> Arc<dyn OAuthRefreshHook> {
    REGISTERED_HOOK
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(NoOpOAuthHook))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_op_returns_token_stale() {
        let h = NoOpOAuthHook;
        let fut = h.refresh(TokenHash([0u8; 32]));
        let r = futures::executor::block_on(fut);
        assert!(matches!(r, Err(OAuthHookError::TokenStale)));
    }

    #[tokio::test]
    async fn no_op_proactive_is_ok() {
        assert!(NoOpOAuthHook.proactive_refresh().await.is_ok());
    }
}
