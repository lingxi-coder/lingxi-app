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
/// defers to an intake conversation. The library's create sheet resolved the
/// name and the surface up front and then created the app outright with
/// `CreateApp.surface`, so the app's very first conversation is already rooted
/// in the app's own workspace, and `AppEventDto::AppCreated` names the record
/// that landed.
///
/// Bumped to 8.0.0 for the conversational create flow. This one IS breaking
/// under the F1-09 guard — the diff removes entries, it does not only add them:
///
/// - REMOVED the create sheet's identity-proposal command and its answering
///   event (the host's pre-creation guess at a name and a surface). "+" now
///   creates an empty shell and the app's own conversation settles both, so
///   there is nothing left to guess at before anything exists. Named
///   descriptively rather than by symbol so the deletion sweep's zero-hit grep
///   for those identifiers stays honest.
/// - ADDED `CreateApp.mode` (`shell` / `scaffolded`) — required, so an existing
///   client's `CreateApp` no longer deserializes. That is deliberate: the mode
///   is the single decision point for whether a scaffold lands, and a default
///   would let a client that never heard of shells mint one by accident.
/// - ADDED `AppRecordDto.scaffolded` — required, no serde default, for the same
///   reason: a record whose shell-ness is guessed is worse than a record that
///   fails to parse.
/// - ADDED the `request_id` correlation key on `CreateApp`,
///   `AppEventDto::AppCreated` and `ClientEvent::AppOperationFailed`, so the
///   caller that started a creation can recognise its own outcome instead of
///   inferring it from whatever record appeared last.
///
/// `snapshots/blessed_major.txt` is re-blessed in lockstep with EVERY
/// `contract_index.json` bless, additive ones included — its job is to answer
/// "what major was checked in", and a sidecar left behind at the previous
/// major silently satisfies the next breaking change's `current > blessed`
/// check with no bump at all.
///
/// Bumped to 9.0.0 for runtime-profile persistence. This is breaking under the
/// F1-09 guard because `AppManifestDto` now carries two new optional records
/// (`runtime_profile`, `dependency_snapshot`) whose UniFFI layout changes the
/// native bindings. The wire JSON is additive, but the mobile bindings are
/// positional and must version-lock with the host.
///
/// Bumped to 10.0.0 for the Phase 9 Local App plugin cutover. This removes the
/// obsolete runtime-profile selection command/event/capability path: runtime
/// family confirmation now flows only through the Host-owned Local App create
/// confirmation sheet and plugin commands, so keeping the old client command
/// family would preserve a dead incompatible UniFFI surface.
pub const CLIENT_PROTOCOL_VERSION: &str = "10.0.0";
