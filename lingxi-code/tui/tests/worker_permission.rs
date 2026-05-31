//! M9-07 — worker-permission chrome snapshots + cross-state-seam test.

use tui::components::permissions::worker::{render_worker_badge, render_worker_pending_to_string};

#[test]
fn worker_badge_snapshot() {
    insta::assert_snapshot!("worker_badge", render_worker_badge("alice"));
}

#[test]
fn worker_pending_snapshot() {
    insta::assert_snapshot!(
        "worker_pending_full",
        render_worker_pending_to_string("Bash", "run mkdir /tmp/x", Some("alice"), Some("my-team"))
    );
}
