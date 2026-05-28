//! Bridge `tokio::signal::ctrl_c` → REPL state machine.
//!
//! The REPL needs to react to Ctrl+C in two modes:
//!   - Turn in flight: cancel the turn via `CancellationToken`
//!   - Idle prompt: arm a flag, exit on second Ctrl+C within 2 seconds
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` Task 3.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// SIGINT source that bridges `tokio::signal::ctrl_c` to the REPL state
/// machine.
pub struct SigintSource {
    /// Fires once per Ctrl+C received.
    notify: Arc<Notify>,
    /// Set to `true` while the REPL is at the idle prompt and the first
    /// Ctrl+C has fired.  Cleared by the 2-second timeout task or by exit.
    pub idle_armed: Arc<AtomicBool>,
}

impl SigintSource {
    /// Spawn the background SIGINT listener task. Returns a handle that REPL
    /// components can subscribe to.
    #[must_use]
    pub fn spawn() -> Self {
        let notify = Arc::new(Notify::new());
        let notify_c = notify.clone();
        tokio::spawn(async move {
            loop {
                if tokio::signal::ctrl_c().await.is_ok() {
                    notify_c.notify_waiters();
                }
            }
        });
        Self {
            notify,
            idle_armed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Await the next Ctrl+C signal.
    pub async fn wait(&self) {
        self.notify.notified().await;
    }

    /// Spawn a background task that calls `token.cancel()` on the next
    /// Ctrl+C, then exits.  The returned `JoinHandle` should be held until
    /// the turn ends naturally; dropping it aborts the watcher so the next
    /// iteration gets a fresh one.
    #[must_use]
    pub fn arm_for_turn(&self, token: CancellationToken) -> tokio::task::JoinHandle<()> {
        let notify = self.notify.clone();
        tokio::spawn(async move {
            notify.notified().await;
            token.cancel();
        })
    }

    /// Returns `true` iff the idle-armed flag was set, and atomically clears
    /// it.
    #[must_use]
    pub fn take_idle_armed(&self) -> bool {
        self.idle_armed.swap(false, Ordering::SeqCst)
    }

    /// Set the idle-armed flag.
    pub fn arm_idle(&self) {
        self.idle_armed.store(true, Ordering::SeqCst);
    }

    /// Clear the idle-armed flag.
    pub fn disarm_idle(&self) {
        self.idle_armed.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn idle_armed_round_trip() {
        let s = SigintSource {
            notify: Arc::new(Notify::new()),
            idle_armed: Arc::new(AtomicBool::new(false)),
        };
        assert!(!s.take_idle_armed());
        s.arm_idle();
        assert!(s.take_idle_armed());
        // take should have cleared it:
        assert!(!s.take_idle_armed());
    }

    #[tokio::test]
    async fn arm_for_turn_cancels_token_on_notify() {
        let s = SigintSource {
            notify: Arc::new(Notify::new()),
            idle_armed: Arc::new(AtomicBool::new(false)),
        };
        let token = CancellationToken::new();
        let _h = s.arm_for_turn(token.clone());
        // Yield to the executor so the spawned task can register with the
        // Notify before we fire it; otherwise notify_waiters fires with no
        // waiters and the token never gets cancelled.
        tokio::task::yield_now().await;
        // Manually fire the notification (simulating SIGINT).
        s.notify.notify_waiters();
        // The spawned task should see it and cancel the token.
        for _ in 0..100 {
            if token.is_cancelled() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("token did not get cancelled within 100 yields");
    }
}
