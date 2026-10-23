//! What a viewer keeps between runs: its own identity, the computers it has
//! paired with, and which computers recently needed the tunnel.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::clock::Clock;
use crate::keys::{IdentityKey, Signer, KEY_BYTES};
use crate::version::DIRECT_FAILURE_MEMORY_MS;

/// A computer this device has paired with. `host_key` is what every later answer must be signed with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedHost {
    /// The host's id.
    pub host_id: String,
    /// `b64u` of the host's raw Ed25519 key, pinned when the user approved this device on the desktop.
    pub host_key: String,
    /// Display name.
    pub name: String,
    /// When pairing completed (ms since the epoch).
    pub paired_at: u64,
}

/// This device's long-term identity.
#[derive(Clone)]
pub struct DeviceIdentity {
    /// `id_from_key(public_key)`, the id the desktop knows this device by.
    pub id: String,
    /// Raw 32-byte Ed25519 public key.
    pub public_key: [u8; KEY_BYTES],
    /// Signs on behalf of the device; the private key may live in a platform keystore.
    pub signer: Arc<dyn Signer>,
}

impl std::fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl DeviceIdentity {
    /// An identity for any signer.
    pub fn from_signer(signer: Arc<dyn Signer>) -> Self {
        let public_key = signer.public_key();
        Self {
            id: crate::keys::id_from_key(&public_key),
            public_key,
            signer,
        }
    }
}

/// The key store failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("key store: {0}")]
pub struct KeysError(pub String);

/// Persistent device state, implemented by the app over its Keystore / Keychain / DataStore.
#[async_trait]
pub trait ViewerKeys: Send + Sync {
    /// The device identity, created on first use.
    async fn identity(&self) -> Result<DeviceIdentity, KeysError>;
    /// Throw the device key away; the next [`identity`](Self::identity) makes a new
    /// one. This is what "pair again" after a revocation does: the desktop remembers
    /// the revoked id and would refuse the old key forever.
    async fn reset_identity(&self) -> Result<(), KeysError>;
    /// The pinned key of a computer, if this device has paired with it.
    async fn pinned_host(&self, host_id: &str) -> Option<PinnedHost>;
    /// Remember a computer's key once the user has approved this device on it.
    async fn pin(&self, host: PinnedHost);
    /// Forget a computer. The desktop still lists this device until the user removes it there.
    async fn unpair(&self, host_id: &str);
}

/// A [`ViewerKeys`] that keeps everything in memory (tests; a fallback when storage fails).
#[derive(Default)]
pub struct MemoryViewerKeys {
    identity: Mutex<Option<Arc<IdentityKey>>>,
    hosts: Mutex<HashMap<String, PinnedHost>>,
}

impl MemoryViewerKeys {
    /// An empty store; the identity is generated on first use.
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// A store with this identity.
    pub fn with_identity(key: IdentityKey) -> Arc<Self> {
        let store = Self::default();
        *store.identity.lock().expect("identity") = Some(Arc::new(key));
        Arc::new(store)
    }

    /// Every pinned host.
    pub fn pinned_hosts(&self) -> Vec<PinnedHost> {
        self.hosts
            .lock()
            .expect("hosts")
            .values()
            .cloned()
            .collect()
    }
}

impl std::fmt::Debug for MemoryViewerKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryViewerKeys").finish_non_exhaustive()
    }
}

#[async_trait]
impl ViewerKeys for MemoryViewerKeys {
    async fn identity(&self) -> Result<DeviceIdentity, KeysError> {
        let key = {
            let mut slot = self.identity.lock().expect("identity");
            Arc::clone(slot.get_or_insert_with(|| Arc::new(IdentityKey::generate())))
        };
        Ok(DeviceIdentity::from_signer(key))
    }

    async fn reset_identity(&self) -> Result<(), KeysError> {
        *self.identity.lock().expect("identity") = None;
        Ok(())
    }

    async fn pinned_host(&self, host_id: &str) -> Option<PinnedHost> {
        self.hosts.lock().expect("hosts").get(host_id).cloned()
    }

    async fn pin(&self, host: PinnedHost) {
        self.hosts
            .lock()
            .expect("hosts")
            .insert(host.host_id.clone(), host);
    }

    async fn unpair(&self, host_id: &str) {
        self.hosts.lock().expect("hosts").remove(host_id);
    }
}

/// Which computers could not be reached directly lately. A host that needed the
/// RTDB tunnel is dialled through the tunnel for ten minutes without a failed
/// direct attempt first. Kept in memory.
pub struct TunnelMemory {
    clock: Arc<dyn Clock>,
    ttl_ms: u64,
    until: Mutex<HashMap<String, u64>>,
}

impl TunnelMemory {
    /// A memory with the protocol's ten-minute window.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self::with_ttl(clock, DIRECT_FAILURE_MEMORY_MS)
    }

    /// A memory with a custom window.
    pub fn with_ttl(clock: Arc<dyn Clock>, ttl_ms: u64) -> Self {
        Self {
            clock,
            ttl_ms,
            until: Mutex::new(HashMap::new()),
        }
    }

    /// Whether `host_id` should skip the direct attempt.
    pub fn prefers(&self, host_id: &str) -> bool {
        let mut until = self.until.lock().expect("tunnel memory");
        match until.get(host_id) {
            Some(deadline) if self.clock.now_ms() < *deadline => true,
            Some(_) => {
                until.remove(host_id);
                false
            }
            None => false,
        }
    }

    /// Dial `host_id` through the tunnel for the next ten minutes.
    pub fn remember(&self, host_id: &str) {
        self.until
            .lock()
            .expect("tunnel memory")
            .insert(host_id.to_owned(), self.clock.now_ms() + self.ttl_ms);
    }

    /// Forget a host.
    pub fn forget(&self, host_id: &str) {
        self.until.lock().expect("tunnel memory").remove(host_id);
    }
}
