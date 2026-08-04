//! Protocol version constant + semver helpers.
//!
//! `CLIENT_PROTOCOL_VERSION` is the distinct, client-facing protocol version
//! (governing decision §0.10), separate from `bridge::BRIDGE_PROTOCOL_VERSION`.
//! Both are exchanged independently in the handshake. The version-guard
//! (F1-09) treats this as a STRUCTURAL diff: a removed/renamed/retyped variant
//! or field requires a MAJOR bump; a new variant or new optional field is
//! additive and requires no bump.

/// The client-protocol contract version, pinned for the M10 foundation
/// (decision §0.10). A change here is a deliberate, reviewed bump.
pub const CLIENT_PROTOCOL_VERSION: &str = "1.2.0";
