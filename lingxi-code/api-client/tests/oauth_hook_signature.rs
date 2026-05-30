//! Frozen contract check: trait + types defined in M3-03 have the byte-for-byte
//! shape M3-04 depends on. If this test breaks, M3-04 will fail to link.

use api_client::oauth_hook::{
    register_oauth_hook, BearerToken, MiddlewareError, NoOpOAuthHook, OAuthHookError,
    OAuthRefreshHook, TokenHash,
};
use protocol::Secret;
use std::sync::Arc;

#[test]
fn trait_object_is_send_sync_static() {
    fn _accepts<T: Send + Sync + 'static>() {}
    _accepts::<Box<dyn OAuthRefreshHook>>();
}

#[test]
fn bearer_token_field_is_pub_secret_string() {
    // The spec requires `pub struct BearerToken(pub Secret<String>);` so M3-04
    // can construct one without an opaque accessor.
    let s = Secret::new("test".to_string());
    let bt = BearerToken(s);
    let _ = bt; // smoke
}

#[test]
fn token_hash_is_32_byte_array() {
    let th = TokenHash([0u8; 32]);
    let _ = th;
}

#[test]
fn no_op_oauth_hook_impls_trait() {
    fn _accepts<H: OAuthRefreshHook>(_: H) {}
    _accepts(NoOpOAuthHook);
}

#[tokio::test]
async fn register_oauth_hook_signature_is_locked() {
    let hook: Arc<dyn OAuthRefreshHook> = Arc::new(NoOpOAuthHook);
    // First registration succeeds; second returns HookAlreadyRegistered.
    let _: Result<(), MiddlewareError> = register_oauth_hook(hook.clone());
    let r = register_oauth_hook(hook);
    // Either both succeed (first call already ran in another test) or this
    // returns HookAlreadyRegistered — both are acceptable in a parallel test
    // runner. What we're locking is the signature, not the side effect.
    let _ = r;
}

#[test]
fn oauth_hook_error_has_refresh_failed_variant() {
    // Exact variant names matter — M3-04 matches on them.
    let _ = OAuthHookError::RefreshFailed("any".into());
    let _ = OAuthHookError::TokenStale;
    let _ = OAuthHookError::ProviderUnreachable("any".into());
}
