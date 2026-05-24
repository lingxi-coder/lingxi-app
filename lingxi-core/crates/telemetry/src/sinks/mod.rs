//! Concrete `AnalyticsSink` implementations: `NoOp` (default), `InMemory`
//! (tests), and the Statsig trait extension point.
//!
//! Populated in Task 10 (`NoOp` + `InMemory`) and Task 11 (Statsig trait + mock).

/// Module marker — referenced by `tests/skeleton_test.rs` to confirm the
/// module path resolves before Task 10 lands the real sinks.
#[doc(hidden)]
#[must_use]
pub fn _module_marker() -> &'static str {
    "lingxi-telemetry-sinks"
}
