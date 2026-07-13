//! Per-teammate mailbox → runner PUMP.
//!
//! THE GAP this closes: a coordinator `SendMessage` to a teammate is delivered
//! into the teammate's [`TeammateMailbox`] (via the [`crate::MailboxRouter`]),
//! but nothing in production drains that mailbox into the teammate's actual
//! turn loop. The teammate's real input is the task handler's `send_message`
//! (→ `pool.send_event(Event::UserMessage)`), reachable through the
//! [`TeamSpawnSeam::send_message`] override on the production `TaskRegistry`.
//! The two are otherwise disconnected, so a routed message would sit unread.
//!
//! The pump is the bridge: one task per teammate that parks on the teammate's
//! mailbox and, for each message, calls `spawn_seam.send_message(task_id, text)`
//! — the Rust analogue of claude-code's `injectUserMessageToTeammate`. The
//! mailbox's bounded `VecDeque` IS the pending-message queue (claude-code's
//! `queuePendingMessage`); the pump drains it in FIFO order.
//!
//! ## Lifecycle / auto-resume divergence
//!
//! Rust teammates are PERSISTENT: the runner parks on its persist-mode `recv()`
//! forever and only leaves on kill. There is no "stopped-but-resumable" state,
//! so claude-code's `resumeAgentBackground` (which re-spawns a stopped agent on
//! the next `SendMessage`) has NO analogue here. The pump exits when the
//! teammate is gone — `send_message` → [`TeamSpawnError::Terminated`] — and the
//! teammate stays dead; the pending-queue (the mailbox) is the faithful
//! behavior. We deliberately do NOT re-spawn.

use std::sync::Arc;
use std::time::Duration;

use traits::team_spawn::{TeamSpawnError, TeamSpawnSeam};

use crate::mailbox::{TeammateMailbox, TeammateMessage};

/// How long the pump parks on the mailbox before re-checking. On timeout it
/// simply re-parks (no message lost — `deliver` notifies the waker), so this is
/// only a liveness floor, not a polling interval that risks dropping messages.
const PUMP_PARK: Duration = Duration::from_secs(30);

/// Extract the user-visible text a teammate should receive as a `UserMessage`
/// from a [`TeammateMessage`]. The mailbox carries the already-rendered content
/// (plain text for ordinary messages; a JSON blob for the structured
/// shutdown / plan-approval handshake), so the payload is simply its `content`
/// — mirroring what `injectUserMessageToTeammate` feeds the turn loop.
#[must_use]
fn message_text(msg: &TeammateMessage) -> String {
    msg.content.clone()
}

/// Run the mailbox → runner pump loop for ONE teammate until the teammate is
/// gone.
///
/// `mailbox` is the teammate's inbox (obtained from
/// `MailboxRouter::get(agent_id)`); `task_id` is the handler-generated id the
/// `TeamSpawnSeam` keys on; `spawn_seam` is the production registry seam whose
/// `send_message` lands the text on the running agent.
///
/// Order of operations (drain-then-wait, so no message is lost between the
/// mailbox being registered at spawn and the pump starting):
/// 1. Drain any backlog already queued before the pump started, sending each in
///    FIFO order. If a backlog send hits [`TeamSpawnError::Terminated`], the
///    teammate is already gone → return immediately.
/// 2. Park on `wait_for_message`; on a delivered message, send it. On a park
///    TIMEOUT, poll [`TeamSpawnSeam::is_alive`]: if the teammate has reached a
///    terminal state (or been evicted) — even though no message ever arrived to
///    surface `Terminated` — RETURN so the caller can unregister its mailbox.
///    Otherwise re-park (the loop continues).
/// 3. Stop on the first `Terminated` from `send_message` (the teammate's runner
///    dropped its receiver). Any OTHER error is logged and the loop continues —
///    a transient internal error must not silently strand the teammate.
pub async fn run_teammate_pump(
    mailbox: Arc<TeammateMailbox>,
    task_id: String,
    spawn_seam: Arc<dyn TeamSpawnSeam>,
) {
    run_teammate_pump_inner(mailbox, task_id, spawn_seam, PUMP_PARK).await;
}

/// The pump body, parameterised on the park duration so tests can drive the
/// liveness re-check without waiting the full [`PUMP_PARK`] floor. Production
/// callers go through [`run_teammate_pump`], which pins `PUMP_PARK`.
async fn run_teammate_pump_inner(
    mailbox: Arc<TeammateMailbox>,
    task_id: String,
    spawn_seam: Arc<dyn TeamSpawnSeam>,
    park: std::time::Duration,
) {
    // 1. Drain-then-send the startup backlog in FIFO order.
    for msg in mailbox.drain() {
        if matches!(
            deliver_one(&spawn_seam, &task_id, &msg).await,
            DeliverOutcome::Stop
        ) {
            return;
        }
    }

    // 2. Park on the mailbox and pump each delivered message.
    loop {
        let Some(msg) = mailbox.wait_for_message(park).await else {
            // Timeout. A teammate that died WITHOUT a pending message never
            // triggers `send_message`'s `Terminated`, so poll liveness here: if
            // it is gone, exit the pump (the caller unregisters its mailbox so a
            // later `SendMessage` resolves to "not found" instead of silently
            // queueing into an undrained inbox). Otherwise re-park — no message
            // was dropped (the waker fires on every `deliver`).
            if !spawn_seam.is_alive(&task_id).await {
                tracing::debug!(task_id, "teammate pump: task no longer alive, stopping pump");
                return;
            }
            continue;
        };
        if matches!(
            deliver_one(&spawn_seam, &task_id, &msg).await,
            DeliverOutcome::Stop
        ) {
            return;
        }
    }
}

/// The outcome of trying to inject one message into the runner.
enum DeliverOutcome {
    /// Delivered (or a transient error was logged) — keep pumping.
    Continue,
    /// The teammate is gone — the pump must stop.
    Stop,
}

/// Inject one message's text into the teammate's runner via the seam, mapping
/// the seam's error space onto a pump decision.
async fn deliver_one(
    spawn_seam: &Arc<dyn TeamSpawnSeam>,
    task_id: &str,
    msg: &TeammateMessage,
) -> DeliverOutcome {
    let text = message_text(msg);
    match spawn_seam.send_message(task_id, text).await {
        Ok(()) => DeliverOutcome::Continue,
        // The teammate's runner is gone — stop pumping (no resume; Rust
        // teammates are persistent and only leave the loop on kill).
        Err(TeamSpawnError::Terminated) => {
            tracing::debug!(task_id, "teammate pump: runner terminated, stopping pump");
            DeliverOutcome::Stop
        }
        // A transient/internal failure: log and keep pumping rather than
        // silently strand the teammate. `Unsupported` should never happen in
        // production (the registry override supports the teammate handler), but
        // if it does it is non-fatal to the loop.
        Err(other) => {
            tracing::warn!(task_id, error = %other, "teammate pump: send_message failed, continuing");
            DeliverOutcome::Continue
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::mailbox::MessageSender;
    use async_trait::async_trait;
    use protocol::AgentId;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::time::SystemTime;

    /// A recording [`TeamSpawnSeam`] whose `send_message` appends each received
    /// text (in call order), and can be configured to return `Terminated` after
    /// a given number of successful deliveries (to drive the stop path).
    struct RecordingSeam {
        received: StdMutex<Vec<String>>,
        calls: AtomicUsize,
        /// After this many successful sends, the next `send_message` returns
        /// `Terminated`. `usize::MAX` ⇒ never terminate.
        terminate_after: usize,
        /// What [`TeamSpawnSeam::is_alive`] reports (drives the timeout-exit
        /// path). Defaults to `true`; a test can flip it to `false` to model a
        /// teammate that died without a pending message.
        alive: AtomicBool,
        /// Fired once when the pump stops (so a test can await it).
        stopped: Arc<tokio::sync::Notify>,
    }

    impl RecordingSeam {
        fn new(terminate_after: usize) -> Arc<Self> {
            Arc::new(Self {
                received: StdMutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                terminate_after,
                alive: AtomicBool::new(true),
                stopped: Arc::new(tokio::sync::Notify::new()),
            })
        }
        fn received(&self) -> Vec<String> {
            self.received.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl TeamSpawnSeam for RecordingSeam {
        async fn spawn_teammate(
            &self,
            _agent_id: AgentId,
            _name: String,
            _team_name: String,
            _description: String,
        ) -> Result<String, TeamSpawnError> {
            Ok(String::new())
        }
        async fn kill(&self, _task_id: &str) -> Result<(), TeamSpawnError> {
            Ok(())
        }
        async fn send_message(
            &self,
            _task_id: &str,
            message: String,
        ) -> Result<(), TeamSpawnError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n >= self.terminate_after {
                self.stopped.notify_one();
                return Err(TeamSpawnError::Terminated);
            }
            self.received.lock().unwrap().push(message);
            Ok(())
        }
        async fn is_alive(&self, _task_id: &str) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
    }

    fn msg(content: &str) -> TeammateMessage {
        TeammateMessage {
            from: MessageSender::Coordinator,
            content: content.to_string(),
            message_id: format!("m-{content}"),
            timestamp: SystemTime::now(),
            request_id: None,
        }
    }

    /// Park-then-deliver: N messages delivered AFTER the pump starts arrive at
    /// the seam's `send_message` in FIFO order.
    #[tokio::test]
    async fn pump_delivers_messages_in_order() {
        let mailbox = Arc::new(TeammateMailbox::new(AgentId::new()));
        let seam = RecordingSeam::new(usize::MAX);
        let seam_dyn: Arc<dyn TeamSpawnSeam> = seam.clone();
        let mb = mailbox.clone();

        let pump = tokio::spawn(async move {
            run_teammate_pump(mb, "task-1".to_string(), seam_dyn).await;
        });

        // Deliver three messages with the pump already parked.
        for c in ["one", "two", "three"] {
            mailbox.deliver(msg(c)).unwrap();
        }

        // Wait until all three are observed (bounded retry to avoid a flake).
        for _ in 0..200 {
            if seam.received().len() == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            seam.received(),
            vec!["one".to_string(), "two".to_string(), "three".to_string()],
            "messages delivered to the runner in FIFO order"
        );

        pump.abort();
    }

    /// A backlog queued BEFORE the pump starts is drained (drain-then-wait), so
    /// no message is lost between mailbox registration and pump startup.
    #[tokio::test]
    async fn pump_drains_startup_backlog() {
        let mailbox = Arc::new(TeammateMailbox::new(AgentId::new()));
        // Pre-load a backlog before the pump is ever spawned.
        for c in ["a", "b"] {
            mailbox.deliver(msg(c)).unwrap();
        }

        let seam = RecordingSeam::new(usize::MAX);
        let seam_dyn: Arc<dyn TeamSpawnSeam> = seam.clone();
        let mb = mailbox.clone();
        let pump = tokio::spawn(async move {
            run_teammate_pump(mb, "task-1".to_string(), seam_dyn).await;
        });

        // Then add one more after startup.
        mailbox.deliver(msg("c")).unwrap();

        for _ in 0..200 {
            if seam.received().len() == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            seam.received(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            "backlog drained first (FIFO), then the post-startup message"
        );

        pump.abort();
    }

    /// The pump STOPS when `send_message` returns `Terminated`.
    #[tokio::test]
    async fn pump_stops_on_terminated() {
        let mailbox = Arc::new(TeammateMailbox::new(AgentId::new()));
        // First send succeeds; the SECOND returns Terminated.
        let seam = RecordingSeam::new(1);
        let seam_dyn: Arc<dyn TeamSpawnSeam> = seam.clone();
        let stopped = seam.stopped.clone();
        let mb = mailbox.clone();

        let pump = tokio::spawn(async move {
            run_teammate_pump(mb, "task-1".to_string(), seam_dyn).await;
        });

        mailbox.deliver(msg("first")).unwrap();
        mailbox.deliver(msg("second-triggers-terminate")).unwrap();

        // The pump's task future completes (returns) shortly after the
        // terminating send. Await both the stop notification AND the join.
        tokio::time::timeout(Duration::from_secs(5), stopped.notified())
            .await
            .expect("the terminating send must fire");
        tokio::time::timeout(Duration::from_secs(5), pump)
            .await
            .expect("the pump task must return (stop) after Terminated")
            .expect("pump task did not panic");

        // Exactly the first message reached the runner; the second triggered
        // the terminal stop and was not recorded.
        assert_eq!(seam.received(), vec!["first".to_string()]);
    }

    /// A backlog whose FIRST send terminates stops the pump immediately
    /// (drain-path stop).
    #[tokio::test]
    async fn pump_stops_on_terminated_in_backlog() {
        let mailbox = Arc::new(TeammateMailbox::new(AgentId::new()));
        mailbox.deliver(msg("doomed")).unwrap();
        mailbox.deliver(msg("never-seen")).unwrap();

        // terminate_after = 0 ⇒ the very first send returns Terminated.
        let seam = RecordingSeam::new(0);
        let seam_dyn: Arc<dyn TeamSpawnSeam> = seam.clone();
        let mb = mailbox.clone();

        tokio::time::timeout(
            Duration::from_secs(5),
            run_teammate_pump(mb, "task-1".to_string(), seam_dyn),
        )
        .await
        .expect("pump returns immediately when the first backlog send terminates");

        assert!(
            seam.received().is_empty(),
            "no message recorded — the first send terminated"
        );
    }

    /// The pump EXITS on a park timeout once the seam reports the task is no
    /// longer alive — EVEN THOUGH no message ever arrives to surface
    /// `Terminated`. This closes the "dead teammate leaks a task re-parking on
    /// the timeout forever" leak (its mailbox is then unregistered by the
    /// caller).
    #[tokio::test]
    async fn pump_stops_when_task_not_alive_on_timeout() {
        let mailbox = Arc::new(TeammateMailbox::new(AgentId::new()));
        // Never terminates via `send_message` (no message is ever delivered),
        // but reports NOT-alive so the timeout branch must exit the pump.
        let seam = RecordingSeam::new(usize::MAX);
        seam.alive.store(false, Ordering::SeqCst);
        let seam_dyn: Arc<dyn TeamSpawnSeam> = seam.clone();
        let mb = mailbox.clone();

        // A tiny park makes the timeout branch fire immediately (production pins
        // the 30s `PUMP_PARK`; the liveness re-check is identical).
        tokio::time::timeout(
            Duration::from_secs(5),
            run_teammate_pump_inner(
                mb,
                "task-1".to_string(),
                seam_dyn,
                Duration::from_millis(10),
            ),
        )
        .await
        .expect("pump must exit once is_alive reports the task terminal");

        assert!(
            seam.received().is_empty(),
            "no message was ever delivered — the pump exited purely on the liveness check"
        );
    }

    /// While the task is still alive, a park timeout re-parks (the pump keeps
    /// running) and a later message is still delivered.
    #[tokio::test]
    async fn pump_reparks_while_alive_then_delivers() {
        let mailbox = Arc::new(TeammateMailbox::new(AgentId::new()));
        let seam = RecordingSeam::new(usize::MAX); // alive stays true
        let seam_dyn: Arc<dyn TeamSpawnSeam> = seam.clone();
        let mb = mailbox.clone();

        let pump = tokio::spawn(async move {
            run_teammate_pump_inner(
                mb,
                "task-1".to_string(),
                seam_dyn,
                Duration::from_millis(5),
            )
            .await;
        });

        // Let several park timeouts elapse (each re-parks because is_alive=true).
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(!pump.is_finished(), "pump keeps running while the task is alive");

        // A message delivered after the re-parks still lands.
        mailbox.deliver(msg("late")).unwrap();
        for _ in 0..200 {
            if seam.received() == vec!["late".to_string()] {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(seam.received(), vec!["late".to_string()]);

        pump.abort();
    }
}
