//! Smoke test: M3-06 module skeleton + `TelemetryError` + `OverflowPolicy` exist.
//! First failing test of M3-06.

#[test]
fn telemetry_error_has_spec_variants() {
    use lingxi_telemetry::TelemetryError;
    let e1 = TelemetryError::UnknownEvent {
        name: "tengu_unknown".to_string(),
    };
    assert_eq!(
        e1.to_string(),
        "unknown event name tengu_unknown (not in tengu_* schema)",
        "UnknownEvent Display string must match spec §5 byte-for-byte",
    );
    let e2 = TelemetryError::PayloadInvalid {
        event: "tengu_api_request_succeeded".to_string(),
        detail: "missing model field".to_string(),
    };
    assert_eq!(
        e2.to_string(),
        "event payload validation failed for tengu_api_request_succeeded: missing model field",
        "PayloadInvalid Display string must match spec §5 byte-for-byte",
    );
}

#[test]
fn overflow_policy_default_is_drop_oldest() {
    use lingxi_telemetry::{AnalyticsBus, OverflowPolicy};
    let bus = AnalyticsBus::new();
    assert!(matches!(bus.overflow_policy(), OverflowPolicy::DropOldest));
}

#[test]
fn tengu_module_compiles() {
    // Reach for the registry constant; subsequent tasks populate it.
    let all: &[&'static str] = lingxi_telemetry::tengu::ALL_EVENT_NAMES;
    // Empty for now (Task 1 ships the empty registry); Tasks 2-9 populate it.
    assert_eq!(all.len(), 0);
}

#[test]
fn sinks_module_compiles() {
    // Module exists but is empty; just confirm the path resolves.
    let _: fn() = || {
        let _ = lingxi_telemetry::sinks::_module_marker();
    };
}
