//! F1-01 — version constant tests.
//!
//! `CLIENT_PROTOCOL_VERSION` is the distinct, client-facing protocol version
//! (governing decision §0.10), separate from `bridge::BRIDGE_PROTOCOL_VERSION`.
//! It must be a valid 3-component semver string so the F2 handshake and the
//! F1-09 structural version-diff guard can reason about major bumps.

use client_protocol::version::CLIENT_PROTOCOL_VERSION;
use std::path::{Path, PathBuf};

/// `CLIENT_PROTOCOL_VERSION` parses as a 3-component `major.minor.patch` semver.
///
/// Self-contained parse (no `semver` crate dep, no `serde_json` — the contract
/// crate stays minimal per §0): split on `.` and assert exactly three numeric
/// components.
#[test]
fn version_is_semver() {
    let parts: Vec<&str> = CLIENT_PROTOCOL_VERSION.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "CLIENT_PROTOCOL_VERSION must be major.minor.patch, got {CLIENT_PROTOCOL_VERSION:?}"
    );
    for (label, part) in [
        ("major", parts[0]),
        ("minor", parts[1]),
        ("patch", parts[2]),
    ] {
        assert!(
            !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()),
            "{label} component must be numeric, got {part:?}"
        );
    }
}

/// Mobile session-mode propagation pins the contract at `11.0.0`.
/// The required Chat/Code mode now travels through session rows and lifecycle
/// events, and the mobile launch configs version-lock on the added mode field.
#[test]
fn version_is_eleven_zero_zero() {
    assert_eq!(CLIENT_PROTOCOL_VERSION, "11.0.0");
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("client-protocol lives under <repo>/lingxi-code")
        .to_path_buf()
}

#[test]
fn obsolete_runtime_profile_selection_event_path_is_absent() {
    let root = repository_root();
    let files = [
        "lingxi-code/client-protocol/src/local_apps.rs",
        "clients/ios/Sources/LocalApps/LocalAppsModels.swift",
        "clients/ios/Sources/LocalApps/LocalAppsProtocolAdapter.swift",
        "clients/ios/Sources/LocalApps/LocalAppsStore.swift",
        "clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppsContract.kt",
        "clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppsViewModel.kt",
    ];
    let forbidden = [
        "RuntimeProfileSelection",
        "runtimeProfileSelection",
        "app_runtime_profile_selection_requested",
    ];

    for rel in files {
        let path = root.join(rel);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        for token in forbidden {
            assert!(
                !source.contains(token),
                "obsolete runtime-profile selector token {token:?} remains in {rel}; client protocol v10 keeps selection Host-owned"
            );
        }
    }
}
