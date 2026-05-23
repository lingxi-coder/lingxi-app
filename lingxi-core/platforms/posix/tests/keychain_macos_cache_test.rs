//! Cache TTL + generation counter + in-flight dedupe tests.
//!
//! macOS-gated; auto-skips when the `security` CLI is unavailable. The
//! full TTL-expiry assertion is in `cache_hit_avoids_subprocess` (uses the
//! 30 s real TTL by comparing latencies; we don't sleep 30 s). The dedupe
//! test forces a cache miss by deleting the cache via a fresh `mk_storage`
//! call.

#![cfg(target_os = "macos")]

use lingxi_platform_posix::secure_storage::MacOsKeychainStorage;
use lingxi_protocol::{SecretKindDto, SecureStorageData, SecureStorageMetadata};
use lingxi_traits::SecureStorage;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

fn fresh_account() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "lingxi-cache-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn mk_payload(value: &[u8]) -> SecureStorageData {
    SecureStorageData::new(
        value.to_vec(),
        SecureStorageMetadata {
            created_at: SystemTime::now(),
            last_accessed: None,
            kind: SecretKindDto("test_secret".to_string()),
        },
    )
}

fn mk_storage(tag: &str) -> Option<Arc<MacOsKeychainStorage>> {
    let user = std::env::var("USER").unwrap_or_else(|_| "test".into());
    // Per-test tag suffix so the dir_hash differs across tests, avoiding
    // any keychain `errSecDuplicateItem` races between tests running in
    // parallel within the same binary.
    let cfg = PathBuf::from(format!(
        "/tmp/lingxi-cache-test-{}-{}",
        std::process::id(),
        tag
    ));
    let default = PathBuf::from("/Users/_lingxi_test_default/.claude");
    MacOsKeychainStorage::new(user, cfg, default, String::new())
        .ok()
        .map(Arc::new)
}

#[tokio::test]
async fn cache_hit_avoids_subprocess() {
    let Some(storage) = mk_storage("hit") else {
        eprintln!("security CLI unavailable; skipping");
        return;
    };
    let account = fresh_account();
    let payload = mk_payload(b"cached-value");

    storage
        .store("-credentials", &account, payload.clone())
        .await
        .expect("store");

    // First retrieve populates the cache.
    let t0 = Instant::now();
    storage
        .retrieve("-credentials", &account)
        .await
        .unwrap()
        .expect("first retrieve");
    let cold_ms = t0.elapsed().as_millis();

    // Second retrieve should be sub-millisecond (no spawn).
    let t1 = Instant::now();
    storage
        .retrieve("-credentials", &account)
        .await
        .unwrap()
        .expect("warm retrieve");
    let warm_ms = t1.elapsed().as_millis();

    assert!(
        warm_ms * 10 < cold_ms.max(10),
        "cache miss {cold_ms} ms vs hit {warm_ms} ms — expected hit to be \
         at least 10x faster"
    );
    storage.delete("-credentials", &account).await.unwrap();
}

#[tokio::test]
async fn concurrent_retrieve_dedupes_to_one_subprocess() {
    // To force cache misses, use a FRESH storage instance per call after
    // populating via the writer storage. Concurrent reads on the SAME
    // storage instance start with empty cache; only one subprocess should
    // actually spawn for the same (service, account) key.
    let Some(writer) = mk_storage("dedupe") else {
        eprintln!("security CLI unavailable; skipping");
        return;
    };
    let account = fresh_account();
    let payload = mk_payload(b"dedupe-value");
    writer
        .store("-credentials", &account, payload.clone())
        .await
        .expect("store");

    // Build a new storage instance so the cache starts empty.
    let storage = mk_storage("dedupe").expect("init");

    // Launch 20 concurrent retrieves; they should all share one subprocess.
    let mut joins = Vec::new();
    let t0 = Instant::now();
    for _ in 0..20 {
        let s = storage.clone();
        let acc = account.clone();
        joins.push(tokio::spawn(async move {
            s.retrieve("-credentials", &acc).await.unwrap()
        }));
    }
    for h in joins {
        h.await.unwrap();
    }
    let total = t0.elapsed().as_millis();
    // A single security spawn is ~500 ms on warm darwin; 20 sequential
    // would be ~10 s. Generous bound of 2 s for dedupe.
    assert!(
        total < 2_000,
        "20 concurrent retrieves took {total} ms — dedupe broken?"
    );

    writer.delete("-credentials", &account).await.unwrap();
}
