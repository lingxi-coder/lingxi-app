//! Plugin blocklist — static + remote.
//!
//! The static blocklist is shipped with the engine binary and never
//! changes at runtime. The remote blocklist is fetched from `fetch_url`
//! by Plan 16's blocklist refresher and replaced atomically via
//! [`PluginBlocklist::set_remote_blocklist`].
//!
//! See spec §15.5.

use protocol::PluginId;
use std::collections::HashSet;
use tokio::sync::RwLock;

/// Combined static + remote plugin blocklist.
///
/// Lookups are async because the remote half is guarded by an `RwLock`
/// so it can be swapped without re-acquiring the static half.
pub struct PluginBlocklist {
    static_block: HashSet<PluginId>,
    remote_block: RwLock<HashSet<PluginId>>,
    /// URL the remote blocklist refresher pulls from.
    pub fetch_url: String,
}

impl PluginBlocklist {
    /// Build a blocklist whose remote portion is fetched from `fetch_url`.
    /// The static portion starts empty and is populated by the host at
    /// construction time (typically read from a managed-settings file).
    #[must_use]
    pub fn new(fetch_url: String) -> Self {
        Self {
            static_block: HashSet::new(),
            remote_block: RwLock::new(HashSet::new()),
            fetch_url,
        }
    }

    /// Return `Some(reason)` if `id` appears in either blocklist; `None`
    /// otherwise. Reason text is shaped for log and error messages.
    pub async fn is_blocked(&self, id: &PluginId) -> Option<String> {
        if self.static_block.contains(id) {
            return Some("static blocklist".into());
        }
        if self.remote_block.read().await.contains(id) {
            return Some("remote blocklist".into());
        }
        None
    }

    /// Atomically replace the remote blocklist with `ids`.
    pub async fn set_remote_blocklist(&self, ids: HashSet<PluginId>) {
        *self.remote_block.write().await = ids;
    }
}
