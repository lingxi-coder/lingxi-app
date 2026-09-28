//! Idle-prompt `Notification` hook wiring for the REPL.
//!
//! Parity: claude-code fires
//! `sendNotification({ message: "Claude is waiting for your input",
//! notificationType: "idle_prompt" })` once the REPL has been IDLE for
//! `messageIdleNotifThresholdMs` after the last response — a `setTimeout` that
//! fires only when `!isLoading && !toolJSX && no focused dialog &&
//! idleTimeSinceResponse >= threshold` (`screens/REPL.tsx:3930-3940`).
//!
//! The Rust port has no per-user global-config seam, so the threshold is a
//! const default that is byte-faithful to claude-code's
//! `DEFAULT_GLOBAL_CONFIG.messageIdleNotifThresholdMs = 60000`
//! (`utils/config.ts:612`).
//!
//! This module defines the small [`IdleNotifier`] seam the repl input loop
//! races its (cancel-unsafe) `read_line` against, plus the production
//! [`OrchestratorIdleNotifier`] that arms a real `tokio::time::sleep` timer and
//! fires [`ConversationOrchestrator::fire_notification`]. The seam is a trait so
//! unit tests inject a deterministic timer (immediate / never) and a recording
//! `fire`, with no real wall-clock sleeps.

use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::Duration;

/// Byte-faithful wire message fired when the REPL goes idle awaiting input.
/// claude-code `sendNotification({ message: ... })` (`screens/REPL.tsx:3935`).
pub const IDLE_PROMPT_MESSAGE: &str = "Claude is waiting for your input";

/// Byte-faithful `notification_type` discriminator for the idle-prompt
/// notification. claude-code `notificationType: 'idle_prompt'`
/// (`screens/REPL.tsx:3936`). Feeds `HookEvent::Notification { kind }` →
/// `NotificationPayload.notification_type`.
pub const IDLE_PROMPT_NOTIFICATION_TYPE: &str = "idle_prompt";

/// Default idle threshold before the idle-prompt `Notification` fires.
///
/// Byte-faithful to claude-code's `DEFAULT_GLOBAL_CONFIG`:
/// `messageIdleNotifThresholdMs: 60000` (`utils/config.ts:612`), read at
/// fire-decision time via `getGlobalConfig().messageIdleNotifThresholdMs`
/// (`screens/REPL.tsx:3933`). The port has no per-user override seam, so this
/// const default stands in faithfully.
pub const MESSAGE_IDLE_NOTIF_THRESHOLD_MS: u64 = 60_000;

/// (2.1.269) Opt-out for the background-task check below, port-renamed from
/// `CLAUDE_CODE_BG_TASKS_REPORT_RUNNING`. Set it to `0` to restore the old
/// behaviour of announcing idleness regardless of what is still running.
pub const BG_TASKS_REPORT_RUNNING_ENV: &str = "LINGXI_BG_TASKS_REPORT_RUNNING";

/// Should a still-running background task suppress the idle notification?
///
/// 2.1.269 fixed headless sessions "reporting 'waiting for your input' while
/// background agents were still running" — the session is not waiting on the
/// user at all, it is waiting on its own work, and a notification saying
/// otherwise sends someone to a terminal that needs nothing from them.
///
/// Default ON; `…=0` restores the old behaviour.
#[must_use]
pub fn bg_tasks_suppress_idle(raw: Option<&str>) -> bool {
    !matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("0") | Some("false") | Some("no") | Some("off")
    )
}

/// Injectable seam the repl input loop races its `read_line` against.
///
/// Decoupled from the concrete orchestrator so the loop is testable with a
/// deterministic timer (no real wall-clock) and a recording fire.
pub trait IdleNotifier: Send + Sync {
    /// Arm a fresh idle timer for the current idle period, or `None` when the
    /// notifier is *gated off* (no `Notification` hook registered) — the loop
    /// then never selects on an idle branch, exactly like the unarmed
    /// `ConfigChange` watcher.
    ///
    /// Returns a `'static` future that resolves once the idle threshold has
    /// elapsed. The loop re-arms (calls this again) at the top of every `step`,
    /// so each post-turn idle period gets its own fresh timer — the parity
    /// `clearTimeout` + re-`setTimeout` on each re-render.
    fn arm_timer(&self) -> Option<BoxFuture<'static, ()>>;

    /// Fire the idle-prompt `Notification` once. Best-effort: must never affect
    /// the input loop, mirroring `fire_session_end`.
    fn fire(&self) -> BoxFuture<'_, ()>;
}

/// Production [`IdleNotifier`]: arms a real `tokio::time::sleep` of
/// [`MESSAGE_IDLE_NOTIF_THRESHOLD_MS`] and fires
/// [`orchestrator::ConversationOrchestrator::fire_notification`] with the
/// byte-faithful idle-prompt message + `notification_type`.
///
/// `armed` reflects the cheap registration gate
/// (`ConversationOrchestrator::has_notification_hook`) resolved ONCE at repl
/// startup: when no `Notification` hook is registered, [`Self::arm_timer`]
/// returns `None` so no timer is ever armed and no spurious work occurs.
pub struct OrchestratorIdleNotifier {
    orch: Arc<orchestrator::ConversationOrchestrator>,
    threshold: Duration,
    armed: bool,
    /// Task registry consulted at FIRE time (AG-3). `None` keeps the old
    /// unconditional behaviour, for hosts that have no background tasks.
    tasks: Option<Arc<tasks::registry::TaskRegistry>>,
    /// Whether a running background task suppresses the notification.
    suppress_when_busy: bool,
}

impl OrchestratorIdleNotifier {
    /// Build a notifier over the concrete orchestrator. `armed` is the result
    /// of the startup `has_notification_hook` gate; pass `false` to disable the
    /// timer entirely (no subscriber → no work).
    #[must_use]
    pub fn new(orch: Arc<orchestrator::ConversationOrchestrator>, armed: bool) -> Self {
        Self {
            orch,
            threshold: Duration::from_millis(MESSAGE_IDLE_NOTIF_THRESHOLD_MS),
            armed,
            tasks: None,
            suppress_when_busy: bg_tasks_suppress_idle(
                std::env::var(BG_TASKS_REPORT_RUNNING_ENV).ok().as_deref(),
            ),
        }
    }

    /// Give the notifier the session's task registry so it can tell "waiting on
    /// the user" from "waiting on my own background work" (AG-3, 2.1.269).
    #[must_use]
    pub fn with_task_registry(mut self, tasks: Arc<tasks::registry::TaskRegistry>) -> Self {
        self.tasks = Some(tasks);
        self
    }

    /// Is a background task still running?
    ///
    /// Checked at FIRE time, not arm time: a task started during the idle window
    /// must still suppress the notification.
    async fn background_work_in_flight(&self) -> bool {
        let Some(tasks) = self.tasks.as_ref() else {
            return false;
        };
        tasks
            .list()
            .await
            .iter()
            .any(|task| matches!(task.base().status, tasks::state::TaskStatus::Running))
    }
}

impl IdleNotifier for OrchestratorIdleNotifier {
    fn arm_timer(&self) -> Option<BoxFuture<'static, ()>> {
        if !self.armed {
            return None;
        }
        let threshold = self.threshold;
        Some(Box::pin(async move {
            tokio::time::sleep(threshold).await;
        }))
    }

    fn fire(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            // AG-3 (2.1.269): a session with background agents still running is
            // not waiting for the user, so saying so sends someone to a terminal
            // that needs nothing from them.
            if self.suppress_when_busy && self.background_work_in_flight().await {
                return;
            }
            self.orch
                .fire_notification(IDLE_PROMPT_MESSAGE, IDLE_PROMPT_NOTIFICATION_TYPE)
                .await;
        })
    }
}

#[cfg(test)]
mod bg_tasks_gate_tests {
    use super::*;

    /// AG-3 (2.1.269) — default ON: a running background task suppresses the
    /// idle notification.
    #[test]
    fn the_check_is_on_by_default() {
        assert!(bg_tasks_suppress_idle(None));
        assert!(bg_tasks_suppress_idle(Some("")));
        assert!(bg_tasks_suppress_idle(Some("1")));
        assert!(bg_tasks_suppress_idle(Some("true")));
    }

    /// `CLAUDE_CODE_BG_TASKS_REPORT_RUNNING=0` restores the old behaviour, which
    /// is the escape hatch the CHANGELOG names.
    #[test]
    fn zero_restores_the_old_behaviour() {
        for raw in ["0", "false", "NO", " off "] {
            assert!(
                !bg_tasks_suppress_idle(Some(raw)),
                "{raw:?} must restore the old behaviour"
            );
        }
    }

    /// The check is useless unless the REPL actually hands over the registry.
    /// A `TaskRegistry` needs a runtime + filesystem + output manager, so it is
    /// not unit-constructible here; pin the composition instead, against the
    /// REPL's own source. Needles are assembled at runtime so they cannot match
    /// themselves inside `include_str!`.
    #[test]
    fn the_repl_hands_the_notifier_its_task_registry() {
        const SRC: &str = include_str!("repl.rs");
        let wiring = ".with_task_registr".to_string() + "y(runtime.task_registry";
        assert!(
            SRC.contains(&wiring),
            "the REPL must give the idle notifier the task registry, or the \
             background-task check can never see anything"
        );
    }
}
