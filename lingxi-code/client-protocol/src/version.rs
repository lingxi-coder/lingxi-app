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
/// Bumped to 3.0.0 (local-apps#questionnaire, Task 5, coordinator ruling):
/// total removal of the static template catalog — `AppTemplateKindDto`,
/// `AppTemplateDto`, `ClientCommand::ListAppTemplates`,
/// `AppEventDto::AppTemplatesChanged`, and `ClientCommand::CreateApp.template`
/// are all deleted. Each is independently a BREAKING structural change per
/// the F1-09 guard (a removed variant / removed field), so this is a real
/// major bump, not folded into 2.0.0's.
pub const CLIENT_PROTOCOL_VERSION: &str = "3.0.0";
