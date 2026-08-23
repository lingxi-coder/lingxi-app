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
///
/// Bumped to 5.0.0 when the local-app designer/generation pipeline was
/// removed from the wire contract: the design/questionnaire/plan/generation
/// command, event, and DTO families are gone and `AppWorkflowStateDto`
/// collapsed to `draft` / `ready`. Removals are BREAKING structural changes
/// under the F1-09 guard, so this is a real major bump.
/// Bumped to 6.0.0 when `DeviceContextDto` shrank to the stable
/// `os`/`form_factor` target pair. The viewport, safe-area, color-scheme,
/// reduced-motion and input-mode fields were removed: they are live values
/// the generated page reads from `window.lingxi.v2.deviceContext`, and the
/// record is now written by the host from its own device facts rather than
/// declared by the agent. Removals are BREAKING under the F1-09 guard.
/// Bumped to 7.0.0 for the create-flow reshape: creating a local app no longer
/// defers to an intake conversation. The library's create sheet resolves the
/// name and the surface up front (`ProposeAppIdentity` / `AppIdentityProposed`)
/// and then creates the app outright with `CreateApp.surface`, so the app's very
/// first conversation is already rooted in the app's own workspace, and
/// `AppEventDto::AppCreated` names the record that landed.
///
/// ⚠️ The F1-09 index diff for this change is purely ADDITIVE — nothing was
/// removed, so the guard classifies it `Compatible` and did not itself require
/// a MAJOR. The bump is a DELIBERATE choice to make every client re-bless
/// against the reshaped create flow rather than silently speak half of it;
/// `snapshots/blessed_major.txt` is re-blessed to match. Do not cite a removal
/// here that did not happen — an earlier draft named a `StartAppCreation`
/// command that has never existed in this repository.
pub const CLIENT_PROTOCOL_VERSION: &str = "7.0.0";
