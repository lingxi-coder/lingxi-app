//! Anti-drift: every per-turn reminder must be injected by BOTH turn drivers.
//!
//! claude-code has ONE main loop. LingXi has two — the batched `turn_loop.rs`
//! and the streaming driver inside `conversation.rs` — and a reminder wired
//! into only one of them is invisible in exactly one mode. That failure is
//! silent: the feature works when you test it, and does nothing in production
//! if production runs the other driver.
//!
//! This is a SOURCE-level check, and it is honest about being weak: it proves
//! each driver mentions each reminder, not that the message is pushed in the
//! right position or at all. There is no driver-level integration harness for
//! this family of reminders; until there is, this catches the one mistake that
//! has actually been made.

const BATCHED: &str = include_str!("../src/turn_loop.rs");
const STREAMING: &str = include_str!("../src/conversation.rs");

/// Reminders that must appear in both drivers. Add a row when you add a
/// reminder — the point is that forgetting the second driver fails here.
const TWIN_REMINDERS: &[&str] = &[
    "conditional_rules_reminder_message",
    "nested_memory_reminder_message",
    "new_diagnostics_reminder_message",
    "skill_listing_reminder_message",
    "agent_listing_reminder_message",
];

/// Count DRIVER call sites only. The receiver disambiguates: the streaming
/// driver is a method on the orchestrator (`self.`), the batched driver takes
/// it as a parameter (`orch.`), and the in-file unit tests use a local binding
/// also named `orch` — which is why the streaming count keys on `self.`.
fn call_sites(src: &str, receiver: &str, name: &str) -> usize {
    src.matches(&format!("{receiver}.{name}().await")).count()
}

#[test]
fn every_per_turn_reminder_is_injected_by_both_drivers() {
    for name in TWIN_REMINDERS {
        assert_eq!(
            call_sites(BATCHED, "orch", name),
            1,
            "{name} must be invoked exactly once by the BATCHED driver (turn_loop.rs)"
        );
        assert_eq!(
            call_sites(STREAMING, "self", name),
            1,
            "{name} must be invoked exactly once by the STREAMING driver \
             (conversation.rs). A reminder wired into only the batched driver \
             is a streaming-only regression that no unit test will catch."
        );
    }
}

/// Every request assembly must carry THIS step's per-turn reminders.
///
/// Computing a reminder advances session state — sent-sets, delta trackers,
/// consume-once drains — so a retry can never recompute one: it comes back
/// `None` and the reminder is lost for the rest of the session. Only
/// `deferred_tools` and `date_change` used to be reused this way; every other
/// reminder was silently dropped whenever a request was rebuilt from raw
/// `session.history` (context-overflow recovery, the 529 fallback, the
/// non-streaming fallback, the PTL truncation retry, the post-compact retry).
///
/// Structural, like the check above, and for the same reason: there is no
/// harness that can drive a real recovery and inspect the retried request. It
/// pins the invariant "wherever a snapshot is rebuilt, the reminders go back
/// on", which is exactly what was violated.
#[test]
fn every_rebuilt_request_snapshot_re_appends_the_turn_reminders() {
    const EXTEND: &str = "extend(turn_reminders.iter().cloned())";

    // In the streaming driver each assembly of an outgoing request prepends the
    // additional-context message, so that count IS the number of assemblies.
    let assemblies = STREAMING.matches("insert(0, ctx_msg)").count();
    assert!(assemblies >= 4, "expected the main path plus its recoveries");
    assert_eq!(
        STREAMING.matches(EXTEND).count(),
        assemblies,
        "every request assembly in the STREAMING driver must re-append this \
         step's reminders; one that does not silently drops them"
    );

    // The batched driver assembles once itself and three times more inside
    // `call_api_with_ptl_recovery` — the truncation retry, the post-compact
    // retry, and the context-hint reject retry — each rebuilding from raw
    // `session.history`.
    //
    // This count is a tripwire, not a fact about the code: when it fires, the
    // question is "did the new rebuild site re-append?" A new site that DOES is
    // a legitimate bump (the context-hint retry was); a new site that does NOT
    // is the bug this test exists to catch, and bumping the number to silence
    // it defeats the whole check.
    assert_eq!(
        BATCHED.matches(EXTEND).count(),
        4,
        "batched driver: one main assembly plus the three rebuilds inside \
         call_api_with_ptl_recovery"
    );
}
