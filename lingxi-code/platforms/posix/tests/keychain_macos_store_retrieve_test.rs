//! `MacOsKeychainStorage` store/retrieve round-trip via the real `security` CLI.
//!
//! Gated on macOS only. Uses a temp-suffixed service name and unique account
//! so concurrent CI runs don't collide. Skipped automatically when the
//! `security` CLI is not present (matches our backend's
//! `BackendUnavailable` semantics).

#![cfg(target_os = "macos")]

use lingxi_platform_posix::secure_storage::MacOsKeychainStorage;
use lingxi_protocol::{SecretKindDto, SecureStorageData, SecureStorageMetadata};
use lingxi_traits::SecureStorage;
use std::path::PathBuf;
use std::time::SystemTime;

fn unique_account() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let pid = std::process::id();
    format!("lingxi-test-{pid}-{nanos}")
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

fn mk_storage(tag: &str) -> Option<MacOsKeychainStorage> {
    let user = std::env::var("USER").unwrap_or_else(|_| "test".into());
    // Per-test tag suffix so the dir_hash differs across tests, avoiding
    // any keychain races between tests running in parallel within the same
    // binary.
    let config_dir = PathBuf::from(format!(
        "/tmp/lingxi-keychain-test-{}-{}",
        std::process::id(),
        tag
    ));
    let default_dir = PathBuf::from("/Users/_lingxi_test_default/.claude");
    MacOsKeychainStorage::new(user, config_dir, default_dir, String::new()).ok()
}

#[tokio::test]
async fn store_then_retrieve_round_trip() {
    let Some(storage) = mk_storage("roundtrip") else {
        eprintln!("security CLI unavailable; skipping");
        return;
    };

    let account = unique_account();
    let payload = mk_payload(b"sk-ant-test-1234567890");

    storage
        .store("-credentials", &account, payload.clone())
        .await
        .expect("store");

    let read = storage
        .retrieve("-credentials", &account)
        .await
        .expect("retrieve");
    let read = read.expect("entry must exist");
    assert_eq!(
        read.expose_secret_bytes(),
        payload.expose_secret_bytes(),
        "round-trip mismatch"
    );

    // Cleanup so we don't litter the user's keychain.
    storage
        .delete("-credentials", &account)
        .await
        .expect("delete");
}

#[tokio::test]
async fn delete_then_retrieve_returns_none() {
    let Some(storage) = mk_storage("delete") else {
        eprintln!("security CLI unavailable; skipping");
        return;
    };
    let account = unique_account();
    let payload = mk_payload(b"to-delete");
    storage
        .store("-credentials", &account, payload)
        .await
        .expect("store");
    storage
        .delete("-credentials", &account)
        .await
        .expect("delete");
    let after = storage
        .retrieve("-credentials", &account)
        .await
        .expect("retrieve");
    assert!(after.is_none(), "deleted entry must read None");
}
