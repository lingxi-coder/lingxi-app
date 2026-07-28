//! Queued-command registry + `command_lifecycle` frames — the substrate for
//! the stream-json interrupt receipt contract.
//!
//! ORACLE (2.1.220, verified live against the binary with a seeded queue —
//! three uuid-stamped `user` frames followed by `interrupt` control_requests):
//!
//! * `system/init` advertises `capabilities:
//!   ["interrupt_receipt_v1","interrupt_cancel_queued_v1","msg_lifecycle_v1"]`
//!   (binary: `gPp=[xsa,Jlb,Isa]`, spread into the init frame between
//!   `plugins` and `mcp_server_errors`).
//! * `interrupt_receipt_v1` — the interrupt control_response success payload
//!   carries `still_queued`: uuids of async user messages that SURVIVE the
//!   interrupt (queue-resident, not the in-flight turn's own uuid). Live:
//!   `{"still_queued":["…2","…3"]}` while `…1` was mid-turn.
//! * `interrupt_cancel_queued_v1` — `cancel_queued:true` on the request
//!   cancels every surviving uuid alongside the abort; the response is
//!   `{"still_queued":[],"cancelled":[…]}` (`still_queued` always empty), and
//!   each cancelled uuid gets a terminal `command_lifecycle` frame BEFORE the
//!   control_response. Repeat interrupts are idempotent (an already-cancelled
//!   uuid is neither still-queued nor re-cancelled).
//! * `msg_lifecycle_v1` — `command_lifecycle` data frames (binary `Kkm`):
//!   `{type:"command_lifecycle",command_uuid,state,uuid,session_id}` in that
//!   key order. Observed states for user messages: `queued` (accepted into the
//!   queue) → `started` (turn dispatch) → terminal `completed` / `cancelled`
//!   (interrupted turn or cancel_queued) / `discarded` (still queue-resident
//!   at stream teardown — binary `Hkm`).
//!
//! The port's queue is the bounded stdin-router → turn-loop mpsc channel,
//! which is opaque; this registry shadows it with the uuid list so the
//! interrupt handler can produce real `still_queued` / `cancelled` receipts
//! and the cancel mark survives until the turn loop dequeues the frame.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;

use serde_json::json;
use serde_json::Value;

use crate::stream_json::OutboundTx;
use crate::stream_json_input::emit_raw_frame_queued;

/// A uuid-stamped async user message was accepted into the input queue.
pub const LIFECYCLE_QUEUED: &str = "queued";
/// The turn loop dequeued the message and is dispatching its turn.
pub const LIFECYCLE_STARTED: &str = "started";
/// Terminal: the turn ran to completion.
pub const LIFECYCLE_COMPLETED: &str = "completed";
/// Terminal: cancelled — either the running turn was interrupted, or the
/// queue-resident uuid was swept by `interrupt` + `cancel_queued:true`.
pub const LIFECYCLE_CANCELLED: &str = "cancelled";
/// Terminal: still queue-resident when the input stream tore down (`Hkm`).
pub const LIFECYCLE_DISCARDED: &str = "discarded";

/// `mCo(reason)` (binary @233106078: `function mCo(e){return Wpt(e)||Bxs(e)}`)
/// — does this turn terminal reason retire its folded commands as `cancelled`?
///
/// * `Wpt` (@233105388) is the interrupt pair:
///   `e==="aborted_streaming"||e==="aborted_tools"`.
/// * `Bxs` (@233105456) is the hard-failure set; every other reason
///   (`stop_hook_prevented`, `hook_stopped`, `tool_deferred`, `max_turns`,
///   `background_requested`, `completed`) and every unknown value fall through
///   the `default:return!1` arm to `completed`.
///
/// The schema calls this out as deliberate: "cancelled-over-completed is
/// deliberate dup-over-loss for exactly-once resenders" (@246207831).
#[must_use]
pub fn terminal_reason_is_cancelled(reason: &str) -> bool {
    matches!(
        reason,
        // Wpt
        "aborted_streaming"
            | "aborted_tools"
            // Bxs
            | "blocking_limit"
            | "rapid_refill_breaker"
            | "prompt_too_long"
            | "image_error"
            | "model_error"
            | "api_error"
            | "malformed_tool_use_exhausted"
            | "budget_exhausted"
            | "structured_output_retry_exhausted"
            | "tool_deferred_unavailable"
            | "turn_setup_failed"
    )
}

/// `Njo(reason, aborted)` (binary @239414857:
/// `function Njo(e,t){return t||mCo(e)?"cancelled":"completed"}`) — the
/// terminal `command_lifecycle` state for every uuid folded into a finished
/// turn. `reason` is `undefined` when the turn produced no terminal reason.
#[must_use]
pub fn terminal_lifecycle_state(reason: Option<&str>, aborted: bool) -> &'static str {
    if aborted || reason.is_some_and(terminal_reason_is_cancelled) {
        LIFECYCLE_CANCELLED
    } else {
        LIFECYCLE_COMPLETED
    }
}

/// Shadow registry of uuid-stamped user messages between stdin arrival and
/// turn dispatch.
#[derive(Default)]
pub struct QueuedCommands {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Uuids resident in the input channel, FIFO (arrival order — the order
    /// `still_queued` / `cancelled` receipts list them in).
    queued: Vec<String>,
    /// Uuids cancelled while queue-resident (`cancel_queued:true`). The frame
    /// itself is still in the mpsc channel; the turn loop consults this set at
    /// dequeue and drops the frame without running it. The terminal
    /// `cancelled` lifecycle was already emitted at interrupt time.
    cancelled: HashSet<String>,
}

impl QueuedCommands {
    /// Fresh, empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a uuid entering the input queue (stdin router, pre-send).
    pub fn on_queued(&self, uuid: &str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.queued.push(uuid.to_string());
    }

    /// The turn loop dequeued this uuid. Returns `true` when the turn should
    /// run; `false` when the uuid was cancelled while queue-resident (the
    /// caller must skip the frame — its terminal lifecycle already went out).
    #[must_use]
    pub fn on_dequeued(&self, uuid: &str) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.queued.retain(|u| u != uuid);
        !inner.cancelled.remove(uuid)
    }

    /// Uuids that survive a plain `interrupt` (queue-resident, minus any
    /// already cancel-pending — the binary's `G.filter((jt)=>!HRu(jt))`).
    #[must_use]
    pub fn still_queued(&self) -> Vec<String> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .queued
            .iter()
            .filter(|u| !inner.cancelled.contains(*u))
            .cloned()
            .collect()
    }

    /// `cancel_queued:true`: mark every surviving uuid cancelled and return
    /// them (FIFO). Already-cancelled uuids are not re-listed, so a repeat
    /// interrupt yields an empty receipt (idempotent).
    #[must_use]
    pub fn cancel_all_queued(&self) -> Vec<String> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let swept: Vec<String> = inner
            .queued
            .iter()
            .filter(|u| !inner.cancelled.contains(*u))
            .cloned()
            .collect();
        for uuid in &swept {
            inner.cancelled.insert(uuid.clone());
        }
        swept
    }

    /// Stream teardown (`Hkm`): drain every surviving uuid for a terminal
    /// `discarded` lifecycle. Clears the registry.
    #[must_use]
    pub fn drain_for_discard(&self) -> Vec<String> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let survivors: Vec<String> = inner
            .queued
            .iter()
            .filter(|u| !inner.cancelled.contains(*u))
            .cloned()
            .collect();
        inner.queued.clear();
        inner.cancelled.clear();
        survivors
    }
}

/// Build the `command_lifecycle` data frame — byte-shape of the binary's
/// `Kkm` wrapper: `{type,command_uuid,state,uuid,session_id}` in that order
/// (`uuid` is a fresh frame id, `command_uuid` names the tracked message).
#[must_use]
pub fn build_command_lifecycle_frame(command_uuid: &str, state: &str, session_id: &str) -> Value {
    json!({
        "type": "command_lifecycle",
        "command_uuid": command_uuid,
        "state": state,
        "uuid": uuid::Uuid::new_v4().to_string(),
        "session_id": session_id,
    })
}

/// The registry plus the outbound frame queue + session id — everything a
/// site needs to both track a uuid and put its `command_lifecycle` frame on
/// the wire. Shared by the stdin router (`queued`), the turn loop
/// (`started` / terminals), and the control dispatcher (interrupt receipts).
pub struct QueueLifecycle {
    /// The shadow registry.
    pub queued: QueuedCommands,
    out_tx: Arc<OutboundTx>,
    session_id: String,
}

impl QueueLifecycle {
    /// Bind the registry to the outbound drain + this session's id.
    #[must_use]
    pub fn new(out_tx: Arc<OutboundTx>, session_id: String) -> Self {
        Self {
            queued: QueuedCommands::new(),
            out_tx,
            session_id,
        }
    }

    /// Emit one `command_lifecycle` frame through the single-writer drain.
    pub fn emit(&self, command_uuid: &str, state: &str) {
        let frame = build_command_lifecycle_frame(command_uuid, state, &self.session_id);
        emit_raw_frame_queued(&self.out_tx, &frame);
    }

    /// Router hook: register + `queued` lifecycle for a uuid-stamped user
    /// frame about to enter the input channel.
    pub fn command_queued(&self, uuid: &str) {
        self.queued.on_queued(uuid);
        self.emit(uuid, LIFECYCLE_QUEUED);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_frame_matches_kkm_shape_and_order() {
        let frame = build_command_lifecycle_frame("cmd-1", LIFECYCLE_QUEUED, "sess-9");
        let keys: Vec<&str> = frame
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec!["type", "command_uuid", "state", "uuid", "session_id"],
            "Kkm key order: type,command_uuid,state,uuid,session_id"
        );
        assert_eq!(frame["type"], "command_lifecycle");
        assert_eq!(frame["command_uuid"], "cmd-1");
        assert_eq!(frame["state"], "queued");
        assert_eq!(frame["session_id"], "sess-9");
        assert!(frame["uuid"].as_str().is_some_and(|u| !u.is_empty()));
    }

    #[test]
    fn plain_interrupt_lists_survivors_in_fifo_order() {
        let q = QueuedCommands::new();
        q.on_queued("u1");
        q.on_queued("u2");
        q.on_queued("u3");
        // u1 dequeued for the running turn — not a survivor.
        assert!(q.on_dequeued("u1"));
        assert_eq!(q.still_queued(), vec!["u2", "u3"]);
        // Receipt is a read, not a sweep: survivors remain queued.
        assert_eq!(q.still_queued(), vec!["u2", "u3"]);
    }

    #[test]
    fn cancel_queued_sweeps_once_and_is_idempotent() {
        let q = QueuedCommands::new();
        q.on_queued("u1");
        q.on_queued("u2");
        assert_eq!(q.cancel_all_queued(), vec!["u1", "u2"]);
        // Idempotent: the second interrupt finds nothing to cancel or list.
        assert_eq!(q.cancel_all_queued(), Vec::<String>::new());
        assert_eq!(q.still_queued(), Vec::<String>::new());
        // The turn loop must skip the cancelled frames when they surface.
        assert!(!q.on_dequeued("u1"));
        assert!(!q.on_dequeued("u2"));
        // …and the cancel mark is consumed (a re-used uuid runs normally).
        q.on_queued("u1");
        assert!(q.on_dequeued("u1"));
    }

    /// `mCo = Wpt || Bxs` over the binary's FULL switch (@233105388 /
    /// @233105456): every `return!0` arm is `cancelled`, every `return!1` arm
    /// and the `default` fall-through stay `completed`.
    #[test]
    fn terminal_reason_split_matches_wpt_and_bxs() {
        for reason in [
            // Wpt
            "aborted_streaming",
            "aborted_tools",
            // Bxs `return!0`
            "blocking_limit",
            "rapid_refill_breaker",
            "prompt_too_long",
            "image_error",
            "model_error",
            "api_error",
            "malformed_tool_use_exhausted",
            "budget_exhausted",
            "structured_output_retry_exhausted",
            "tool_deferred_unavailable",
            "turn_setup_failed",
        ] {
            assert!(
                terminal_reason_is_cancelled(reason),
                "mCo({reason}) must be true"
            );
            assert_eq!(terminal_lifecycle_state(Some(reason), false), "cancelled");
        }
        for reason in [
            // Bxs `return!1`
            "stop_hook_prevented",
            "hook_stopped",
            "tool_deferred",
            "max_turns",
            "background_requested",
            "completed",
            // `default:return!1`
            "something_new",
        ] {
            assert!(
                !terminal_reason_is_cancelled(reason),
                "mCo({reason}) must be false"
            );
            assert_eq!(terminal_lifecycle_state(Some(reason), false), "completed");
        }
    }

    /// `Njo(e,t)`: the abort flag alone forces `cancelled`, and an absent
    /// reason (`e===void 0` ⇒ `Bxs` returns `!1`) is `completed`.
    #[test]
    fn terminal_state_honours_abort_flag_and_absent_reason() {
        assert_eq!(terminal_lifecycle_state(None, false), "completed");
        assert_eq!(terminal_lifecycle_state(None, true), "cancelled");
        assert_eq!(
            terminal_lifecycle_state(Some("max_turns"), true),
            "cancelled"
        );
        assert_eq!(
            terminal_lifecycle_state(Some("completed"), false),
            "completed"
        );
    }

    #[test]
    fn teardown_drain_returns_survivors_and_clears() {
        let q = QueuedCommands::new();
        q.on_queued("u1");
        q.on_queued("u2");
        let _ = q.cancel_all_queued();
        q.on_queued("u3");
        // Only the un-cancelled survivor is discarded; cancelled uuids already
        // received their terminal lifecycle.
        assert_eq!(q.drain_for_discard(), vec!["u3"]);
        assert_eq!(q.still_queued(), Vec::<String>::new());
    }
}
