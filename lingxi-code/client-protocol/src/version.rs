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
///
/// Bumped to 2.0.0 (local-apps#questionnaire, Task 2 fix-forward): removing
/// `AppRecordDto.template` / `AppManifestDto.template` (the core
/// `local_apps::AppRecord`/`AppManifest` no longer carry a template — apps
/// are now designed from a free-text `brief`) is a BREAKING structural
/// change per the F1-09 guard.
///
/// Bumped to 3.0.0 when the static catalog and its command/event fields were
/// removed in favor of the dynamic brief/questionnaire/plan contract. Those
/// removals are BREAKING structural changes under the F1-09 guard, so this is
/// a real major bump rather than an additive protocol revision.
///
/// Bumped to 4.0.0 when app records began carrying the persisted
/// `git_enabled` creation choice.
pub const CLIENT_PROTOCOL_VERSION: &str = "4.0.0";
