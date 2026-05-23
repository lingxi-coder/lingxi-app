//! Smoke test: the new M3-05 modules compile and export the expected symbols.
//! First failing test of M3-05.

#[test]
fn events_module_exports_emit_fn() {
    // Compile-time assertion: the function signature is reachable.
    let _: fn() = || {
        // Reference the symbol so the linker pulls it in.
        let _: &dyn std::any::Any = &lingxi_cost::emit_cost_recorded;
    };
}

#[test]
fn summary_module_exports_cost_summary() {
    fn _accepts(_: &lingxi_cost::CostSummary) {}
}

#[test]
fn cost_error_has_new_variants() {
    use lingxi_cost::CostError;
    // The two new variants from spec §5.
    let e1 = CostError::UnknownModel {
        model: "claude-foo".to_string(),
    };
    assert_eq!(
        e1.to_string(),
        "Cost tracking unavailable for claude-foo",
        "UnknownModel Display string must match spec byte-for-byte",
    );
    let e2 = CostError::BudgetExceeded {
        limit: 100.00,
        current: 150.00,
    };
    assert_eq!(
        e2.to_string(),
        "Budget exceeded ($150.00); stopped.",
        "BudgetExceeded Display string must match spec byte-for-byte",
    );
}

#[test]
fn budget_threshold_bps_constants_are_locked() {
    assert_eq!(lingxi_cost::BUDGET_WARNING_THRESHOLD_BPS, 8000_u32);
    assert_eq!(lingxi_cost::BUDGET_EXCEEDED_THRESHOLD_BPS, 10000_u32);
    assert_eq!(lingxi_cost::BATCH_DISCOUNT_BPS, 5000_u32);
}
