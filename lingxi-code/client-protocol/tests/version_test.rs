//! F1-01 — version constant tests.
//!
//! `CLIENT_PROTOCOL_VERSION` is the distinct, client-facing protocol version
//! (governing decision §0.10), separate from `bridge::BRIDGE_PROTOCOL_VERSION`.
//! It must be a valid 3-component semver string so the F2 handshake and the
//! F1-09 structural version-diff guard can reason about major bumps.

use client_protocol::version::CLIENT_PROTOCOL_VERSION;

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

/// Shrinking `DeviceContextDto` to the stable `os`/`form_factor` target pair
/// — the host writes it from its own device facts, and the removed
/// viewport/safe-area/color-scheme/reduced-motion/input-mode fields are live
/// values the page reads at runtime — pins the contract at `6.0.0` (a further
/// MAJOR bump past the designer-removal `5.0.0` — see
/// `client_protocol::version::CLIENT_PROTOCOL_VERSION`'s doc comment). A
/// change here is a deliberate, reviewed bump — this test makes an accidental
/// edit visible.
#[test]
fn version_is_six_zero_zero() {
    assert_eq!(CLIENT_PROTOCOL_VERSION, "6.0.0");
}
