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
