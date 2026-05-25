//! Release-marker events. Emitted once at the first successful
//! `Engine::init()` after each tagged release. The constant lives here
//! (one event per release) so version-bump audits have a single grep target.

/// v0.5.0 release marker — emitted once on first init after the upgrade.
/// Wire string locked: appears byte-for-byte in test assertions + the
/// downstream Statsig analytics schema. Actual emit wires in M5 alongside
/// the `Engine::init()` boot path; M4-09 only registers the constant so
/// downstream sinks reserve the slot.
pub const LINGXI_CORE_V0_5_0_RELEASED: &str = "lingxi_core_v0_5_0_released";

/// Order-locked array of all release-marker names; consumed by
/// `tengu::ALL_EVENT_NAMES`. Append-only: never reorder or remove entries.
pub(crate) const NAMES: &[&str] = &[LINGXI_CORE_V0_5_0_RELEASED];
