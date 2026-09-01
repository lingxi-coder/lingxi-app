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

/// The Phase 9 Local App plugin cutover pins the contract at `10.0.0`.
/// Removing the obsolete runtime-profile selection command/event/capability
/// path is a deliberate breaking change because the generated native bindings
/// are positional — see `client_protocol::version::CLIENT_PROTOCOL_VERSION`'s
/// doc comment.
#[test]
fn version_is_ten_zero_zero() {
    assert_eq!(CLIENT_PROTOCOL_VERSION, "10.0.0");
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("client-protocol lives under <repo>/lingxi-code")
        .to_path_buf()
}

#[test]
fn legacy_runtime_profile_selection_wire_variants_are_absent() {
    let root = repository_root();
    let files = [
        "lingxi-code/client-protocol/src/commands.rs",
        "lingxi-code/client-protocol/src/local_apps.rs",
        "clients/shared/src/protocol.ts",
        "clients/shared/src/validation.ts",
        "clients/ios/Sources/LocalApps/LocalAppsModels.swift",
        "clients/ios/Sources/LocalApps/LocalAppsProtocolAdapter.swift",
        "clients/ios/Sources/LocalApps/LocalAppsStore.swift",
        "clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppsContract.kt",
        "clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppsViewModel.kt",
    ];
    let forbidden = [
        "runtime_profile_selection",
        "RuntimeProfileSelection",
        "runtimeProfileSelection",
        "resolve_app_runtime_profile_selection",
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
