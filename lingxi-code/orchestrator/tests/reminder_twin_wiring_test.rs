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
/// Both drivers now route every REBUILD through `reattach_outgoing_context`,
/// which prepends the leading context and re-appends this step's reminders in
/// one place. That is strictly stronger than the previous shape (N open-coded
/// assemblies that each had to remember), so this check moved with it: instead
/// of counting duplicated `insert`/`extend` lines, it pins that each driver has
/// exactly ONE hand-rolled assembly and that every other assembly goes through
/// the shared helper.
///
/// The tripwire property is unchanged, and so is the rule for reading a
/// failure: when it fires, the question is "does the new assembly re-append?"
/// A new site that routes through `reattach_outgoing_context` is a legitimate
/// bump; a new site that hand-rolls its own snapshot is the bug this test
/// exists to catch, and bumping the number to silence it defeats the check.
#[test]
fn every_rebuilt_request_snapshot_re_appends_the_turn_reminders() {
    const EXTEND: &str = "extend(turn_reminders.iter().cloned())";

    // Exactly one hand-rolled assembly per driver: the main path. The receiver
    // disambiguates the two drivers, as in `call_sites` above.
    assert_eq!(
        STREAMING
            .matches("self.prepend_leading_context(&mut snapshot).await")
            .count(),
        1,
        "the STREAMING driver must hand-roll exactly one assembly (the main \
         path); any other must go through reattach_outgoing_context"
    );
    assert_eq!(
        BATCHED
            .matches("orch.prepend_leading_context(&mut history_snapshot).await")
            .count(),
        1,
        "the BATCHED driver must hand-roll exactly one assembly (the main path)"
    );

    // Every rebuild-from-raw-history path routes through the shared helper:
    // the 529 fallback, the PTL truncation retry and the non-streaming
    // fallback in streaming; the three rebuilds inside
    // `call_api_with_ptl_recovery` in batched.
    assert_eq!(
        STREAMING.matches("self.reattach_outgoing_context(").count(),
        3,
        "STREAMING rebuilds (529 fallback, PTL retry, non-streaming fallback) \
         must each reattach; a rebuild that does not silently drops reminders"
    );
    assert_eq!(
        BATCHED.matches("orch.reattach_outgoing_context(").count(),
        3,
        "BATCHED rebuilds inside call_api_with_ptl_recovery must each reattach"
    );

    // Routing through the helper only proves anything while the helper still
    // re-appends. Two sites in the streaming file: the main path, and the one
    // inside `reattach_outgoing_context` itself (which the batched driver also
    // calls — the helper lives on the orchestrator, in conversation.rs).
    assert_eq!(
        STREAMING.matches(EXTEND).count(),
        2,
        "conversation.rs must re-append on the main path AND inside \
         reattach_outgoing_context; losing the latter silently un-does every \
         rebuild path in BOTH drivers"
    );
    assert_eq!(
        BATCHED.matches(EXTEND).count(),
        1,
        "the batched main path re-appends directly; its rebuilds go through \
         reattach_outgoing_context"
    );
}
