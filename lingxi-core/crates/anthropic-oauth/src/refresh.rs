//! OAuth token refresh: reactive (401-driven) + proactive (timer-driven).
//!
//! See spec §3 (cross-plan trait), §4 Flow A (lifecycle), §7 (wire identifiers),
//! §8 M3-04 phase list.
//!
//! M3-04 implements [`lingxi_api_client::oauth_hook::OAuthRefreshHook`] (frozen
//! in M3-03). The single-flight invariant is enforced via [`AuthState::refresh_lock`]
//! with double-check-after-acquire.

#![allow(dead_code)] // populated by Task 2 onward.

use std::sync::Arc;

/// Placeholder — populated by Task 2 with the real fields.
pub struct AuthState;

/// Placeholder — populated by Task 2 with the real impl.
pub struct RefreshDriver {
    pub(crate) state: Arc<AuthState>,
}
