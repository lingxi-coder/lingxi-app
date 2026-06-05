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
        }
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
            self.orch
                .fire_notification(IDLE_PROMPT_MESSAGE, IDLE_PROMPT_NOTIFICATION_TYPE)
                .await;
        })
    }
}
