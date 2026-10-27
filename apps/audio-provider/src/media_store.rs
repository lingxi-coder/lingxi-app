//! Bounded, process-local storage for owner-scoped media payloads.
//!
//! Media bytes stay behind opaque references. A reference can only be read or
//! deleted with the same trusted [`MediaOwner`] that created it.

use media_api::{
    MediaAsset, MediaError, MediaErrorKind, MediaMetadata, MediaOperationContext, MediaOwner,
    MediaPayload, MediaRef, MediaResourceStore,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

const DEFAULT_MAX_ENTRIES: usize = 256;
const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

struct Entry {
    owner: MediaOwner,
    metadata: MediaMetadata,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct State {
    entries: HashMap<MediaRef, Entry>,
    insertion_order: VecDeque<MediaRef>,
    total_bytes: usize,
}

/// Process-local bounded store used by Desktop, Android, and iOS app hosts.
///
/// The store is app-scoped and survives individual Agent or engine turns,
/// while each reference remains limited to the stable owner recorded on put.
/// Entries are evicted oldest-first when the process-wide cap is reached.
#[derive(Clone)]
pub struct BoundedMediaResourceStore {
    state: Arc<Mutex<State>>,
    max_entries: usize,
    max_bytes: usize,
}

impl Default for BoundedMediaResourceStore {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_BYTES)
    }
}

impl BoundedMediaResourceStore {
    /// Create a process-local store with explicit aggregate limits.
    #[must_use]
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        assert!(max_entries > 0, "media store must allow at least one entry");
        assert!(max_bytes > 0, "media store must allow at least one byte");
        Self {
            state: Arc::new(Mutex::new(State::default())),
            max_entries,
            max_bytes,
        }
    }

    /// Current number of stored resources, for diagnostics and tests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .len()
    }

    /// Whether the store currently contains no media resources.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn evict_until_fits(&self, state: &mut State, byte_len: usize) {
        while state.entries.len() >= self.max_entries
            || state.total_bytes.saturating_add(byte_len) > self.max_bytes
        {
            let Some(oldest) = state.insertion_order.pop_front() else {
                break;
            };
            if let Some(entry) = state.entries.remove(&oldest) {
                state.total_bytes = state.total_bytes.saturating_sub(entry.bytes.len());
            }
        }
    }
}

#[async_trait::async_trait]
impl MediaResourceStore for BoundedMediaResourceStore {
    async fn put(
        &self,
        context: &MediaOperationContext,
        payload: MediaPayload,
    ) -> Result<MediaAsset, MediaError> {
        payload.validate(context.max_payload_bytes)?;
        if payload.bytes.is_empty() {
            return Err(MediaError::new(
                MediaErrorKind::NoData,
                "empty media payloads cannot be stored",
            ));
        }
        let byte_len = payload.bytes.len();
        if byte_len > self.max_bytes {
            return Err(MediaError::new(
                MediaErrorKind::MediaTooLarge,
                "media payload exceeds the app store limit",
            ));
        }
        let reference = MediaRef(uuid::Uuid::new_v4().to_string());
        let asset = MediaAsset {
            reference: reference.clone(),
            metadata: payload.metadata.clone(),
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.evict_until_fits(&mut state, byte_len);
        state.total_bytes = state.total_bytes.saturating_add(byte_len);
        state.insertion_order.push_back(reference.clone());
        state.entries.insert(
            reference,
            Entry {
                owner: context.owner.clone(),
                metadata: payload.metadata,
                bytes: payload.bytes,
            },
        );
        Ok(asset)
    }

    async fn get(
        &self,
        context: &MediaOperationContext,
        media: &MediaRef,
        max_bytes: u64,
    ) -> Result<MediaPayload, MediaError> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = state.entries.get(media).ok_or_else(|| {
            MediaError::new(MediaErrorKind::NotFound, "media reference is unavailable")
        })?;
        if entry.owner != context.owner {
            return Err(MediaError::new(
                MediaErrorKind::NotFound,
                "media reference is not visible to this owner",
            ));
        }
        let limit = max_bytes.min(context.max_payload_bytes);
        if u64::try_from(entry.bytes.len()).unwrap_or(u64::MAX) > limit {
            return Err(MediaError::new(
                MediaErrorKind::MediaTooLarge,
                "stored media exceeds the requested payload limit",
            ));
        }
        Ok(MediaPayload {
            metadata: entry.metadata.clone(),
            bytes: entry.bytes.clone(),
        })
    }

    async fn delete(
        &self,
        context: &MediaOperationContext,
        media: &MediaRef,
    ) -> Result<(), MediaError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = state.entries.get(media).ok_or_else(|| {
            MediaError::new(MediaErrorKind::NotFound, "media reference is unavailable")
        })?;
        if entry.owner != context.owner {
            return Err(MediaError::new(
                MediaErrorKind::NotFound,
                "media reference is not visible to this owner",
            ));
        }
        if let Some(entry) = state.entries.remove(media) {
            state.total_bytes = state.total_bytes.saturating_sub(entry.bytes.len());
        }
        state.insertion_order.retain(|reference| reference != media);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_api::{MediaKind, MediaOperationId};

    fn context(owner: MediaOwner) -> MediaOperationContext {
        MediaOperationContext::new(MediaOperationId::new(1, 1), owner).with_max_payload_bytes(32)
    }

    fn payload(bytes: &[u8]) -> MediaPayload {
        MediaPayload {
            metadata: MediaMetadata {
                kind: MediaKind::Image,
                mime_type: "image/jpeg".into(),
                byte_len: bytes.len() as u64,
                duration_ms: None,
                width_px: Some(1),
                height_px: Some(1),
                sample_rate_hz: None,
                channels: None,
                captured_at_unix_ms: Some(1234),
            },
            bytes: bytes.to_vec(),
        }
    }

    #[tokio::test]
    async fn stored_payloads_are_owner_scoped_and_keep_capture_time() {
        let store = BoundedMediaResourceStore::new(4, 64);
        let alice = context(MediaOwner::Session {
            session_id: "alice".into(),
        });
        let bob = context(MediaOwner::Session {
            session_id: "bob".into(),
        });
        let asset = store.put(&alice, payload(&[1, 2, 3])).await.unwrap();
        let data = store.get(&alice, &asset.reference, 32).await.unwrap();
        assert_eq!(data.bytes, [1, 2, 3]);
        assert_eq!(data.metadata.captured_at_unix_ms, Some(1234));
        assert_eq!(
            store
                .get(&bob, &asset.reference, 32)
                .await
                .unwrap_err()
                .kind,
            MediaErrorKind::NotFound
        );
    }

    #[tokio::test]
    async fn store_evicts_oldest_payload_within_aggregate_caps() {
        let store = BoundedMediaResourceStore::new(1, 16);
        let owner = context(MediaOwner::Ui {
            instance_id: "preview".into(),
        });
        let first = store.put(&owner, payload(&[1, 2])).await.unwrap();
        let second = store.put(&owner, payload(&[3, 4])).await.unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(
            store
                .get(&owner, &first.reference, 32)
                .await
                .unwrap_err()
                .kind,
            MediaErrorKind::NotFound
        );
        assert_eq!(
            store
                .get(&owner, &second.reference, 32)
                .await
                .unwrap()
                .bytes,
            [3, 4]
        );
    }
}
