//! `/goal` — set a session-scoped goal Claude checks before stopping, show the
//! active goal, or clear it early.
//!
//! Ported from the claude-code v2.1.198 binary. `strings -a` on the shipped
//! Mach-O turned up TWO command objects sharing the name `"goal"`:
//!
//! ```text
//! {type:"local-jsx",name:"goal",description:"Set a goal Claude checks before
//!   stopping",argumentHint:"[<condition> | clear]",immediate:!0,load:...}
//! {type:"local",name:"goal",supportsNonInteractive:!0,thinClientDispatch:
//!   "post-text",description:"Set a goal — keep working until the condition is
//!   met",get isHidden(){return!hr()},isEnabled:()=>hr()||Qi(),load:...}
//! ```
//! exported as `default` (the interactive `local-jsx` TUI dialog) and the
//! named export `goalNonInteractive` (the `local`/`supportsNonInteractive`
//! variant). LingXi is a headless engine, so this handler ports the SECOND
//! object's body — the plain-text branch function located alongside it:
//!
//! ```text
//! ...`${WMl(o.lastReason)}`:"";return{type:"text",value:
//!   `Goal active: ${o.condition} (${s})${i}`}}
//! if(trr(n)){let o=IEt(t);return{type:"text",value:o===null?"No goal set":
//!   `Goal cleared: ${o}`}}
//! if(n.length>wEt)return bt("goal_set","too_long"),{type:"text",value:
//!   `Goal condition is limited to ${wEt} characters (got ${n.length})`};
//! let r=kEt(n,t);if(r!==null)return{type:"text",value:r};
//! return{type:"query",value:`Goal set: ${n}`,prompt:nrr(n)};
//! ```
//!
//! `kEt(n,t)` is the trust/hooks-restricted gate (returns a fixed message on
//! failure, `null` on success); `nrr(n)` builds the model-facing directive
//! injected once the goal is accepted. `trr(n)` is the case-insensitive
//! clear-token membership test.
//!
//! ## Headless mapping onto `CommandResult`
//!
//! The binary's non-interactive result is `{type, value, prompt?}`: `value` is
//! always a short line for the invoking surface, and a successful `set` ALSO
//! carries `prompt` — the text queried to the model as the next turn. LingXi's
//! [`CommandResult`] has no variant carrying both a display line and an
//! injected turn at once, so the success path follows the explicit spec
//! instruction and returns [`CommandResult::InjectMessage`] with the directive
//! text (below), matching the `/review` handler's `InjectMessage`-only
//! precedent (`commands/core/src/review.rs`) rather than [`CommandResult::Done`].
//! The `status`/`clear`/`too-long` branches have no `prompt`, so they map onto
//! `CommandResult::Done` (the `/effort`/`/model` precedent).
//!
//! ## Known gaps (documented, not implemented — no shared file may be touched
//! by this port; see each fn's doc comment)
//!
//! * **No app-state goal seam.** `traits::OrchestratorHandle` has no
//!   `get_active_goal` / `set_active_goal` / `clear_active_goal` method
//!   (verified: `traits/src/orchestrator.rs` defines no such members). This
//!   handler keeps its own `Mutex`-guarded [`ActiveGoal`] instead — sound for
//!   as long as the registry holds ONE shared `Arc<GoalHandler>` per session
//!   (the same lifetime every other stateful core handler assumes), but it
//!   means a goal set through one `GoalHandler` instance is invisible to any
//!   other instance. A real app-state seam should replace this field.
//! * **No hook add/remove seam.** `OrchestratorHandle::list_hooks` is
//!   read-only; there is no `add_session_hook` / `remove_session_hook`. This
//!   port does NOT register or remove an actual `Stop` hook — the directive
//!   text alone asks the model to keep working, but nothing will mechanically
//!   block a real `Stop` event yet. Wiring that in requires both the new
//!   handle methods AND the turn-loop change the spec calls out separately
//!   (tagging a goal-owned hook so a non-blocking `ok:true` Prompt-hook result
//!   auto-clears `activeGoal` and allows the stop) — out of scope for a single
//!   command-handler file.
//! * **No trust / hooks-restricted query.** [`workspace_trusted`] and
//!   [`hooks_restricted`] are the `kEt(n,t)` gate's two halves; neither has a
//!   handle seam, so both hardcode the always-succeeds answer (mirrors the
//!   `effort.rs` `dynamic_workflows_enabled` "seam missing → hard value"
//!   pattern). The fixed gate-failure strings are kept ready to fire once a
//!   real query lands.
//! * **`lastReasonSuffix` formatting is NOT byte-verified.** The task's
//!   locked output-string list gives every OTHER literal verbatim but leaves
//!   this one as a `${lastReasonSuffix}` placeholder; the one targeted
//!   `strings` probe that reached it was truncated mid-fragment
//!   (`` `${WMl(o.lastReason)}` ``) with no confirmed static prefix text. Since
//!   nothing in this handler currently populates `last_reason` anyway (that is
//!   the turn-loop wiring gap above), [`GoalHandler::status`]'s suffix format
//!   is a best-effort placeholder, clearly marked, not a confirmed port.

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use traits::OrchestratorHandle;

/// `wEt` — the max goal-condition length (v2.1.198).
const MAX_CONDITION_CHARS: usize = 4000;

/// `trr(n)` — case-insensitive clear-token membership. Order/spelling per the
/// task spec (not independently re-derived from the binary beyond the
/// located `trr(n)` call site).
const CLEAR_TOKENS: &[&str] = &["clear", "stop", "off", "reset", "none", "cancel"];

/// Literal, shared by both the "no active goal" status branch and the "clear
/// with nothing to clear" branch (binary: `o===null?"No goal set"`, and the
/// status branch's `else` arm reaching the same text).
const NO_GOAL_SET: &str = "No goal set";

/// `kEt`'s trust-gate failure message, verbatim from the locked output-string
/// list. [`workspace_trusted`] currently always returns `true`, so this is
/// unreachable until a real trust seam lands — kept ready for that seam.
const TRUST_GATE_MESSAGE: &str =
    "/goal is only available in trusted workspaces. Restart, accept the trust dialog, and try again.";

/// `kEt`'s hooks-restricted failure message, verbatim from the locked
/// output-string list. [`hooks_restricted`] currently always returns `false`,
/// so this is unreachable until a real restriction query lands.
const HOOKS_RESTRICTED_MESSAGE: &str = "/goal can't run while hooks are restricted (disableAllHooks or allowManagedHooksOnly is set in settings or by policy).";

/// `bt("goal_set","too_long")` — the binary's own telemetry call on the
/// too-long guard. Passed straight through `telemetry::emit_command_failed`
/// (a generic `tracing::error!(event=.., error=..)` transport) rather than
/// through the LingXi `tengu_command_<name>_*` batch-1/2 convention
/// (`telemetry::tengu::command`), since `goal` was never one of those batches
/// and this file may not add a new constant to that shared module.
const TELEMETRY_GOAL_SET_EVENT: &str = "goal_set";
const TELEMETRY_TOO_LONG_PROPERTY: &str = "too_long";

/// `tengu_stop_hook_removed` — the binary's clear-path telemetry event (per
/// the task spec), fired when an existing goal's `Stop` hook is torn down.
/// This port has no real hook to remove (see the module doc's "no hook
/// add/remove seam" gap); the event still fires so the telemetry surface is
/// faithful to the semantic action (a goal WAS cleared), carrying the cleared
/// condition as `details`.
const TELEMETRY_STOP_HOOK_REMOVED: &str = "tengu_stop_hook_removed";

/// `kEt(n,t)` half 1 — workspace-trust query. `OrchestratorHandle` has no
/// trust-query method (verified via `traits/src/orchestrator.rs`), so this
/// always resolves `true` (every workspace treated as trusted) until a real
/// seam is added — the same "seam missing → hard value" shape as
/// `effort.rs`'s `dynamic_workflows_enabled`.
fn workspace_trusted() -> bool {
    true
}

/// `kEt(n,t)` half 2 — hooks-restricted query (`disableAllHooks` /
/// `allowManagedHooksOnly`). Already ported in `hooks/src/executor.rs` +
/// `hooks/src/loader.rs`, but `commands/core` cannot see the `hooks` crate's
/// settings without a new `OrchestratorHandle` method. Hardcoded to `false`
/// (never restricted) until that seam lands.
fn hooks_restricted() -> bool {
    false
}

/// `Goal condition is limited to {n} characters (got {got})` — verbatim.
fn too_long_message(got: usize) -> String {
    format!("Goal condition is limited to {MAX_CONDITION_CHARS} characters (got {got})")
}

/// `nrr(n)` — the fixed model-facing directive injected once a goal is
/// accepted. Verbatim from the locked output-string list.
fn directive_for(condition: &str) -> String {
    format!(
        "A session-scoped Stop hook is now active with condition: \"{condition}\". Briefly \
acknowledge the goal, then immediately start (or continue) working toward it — treat the \
condition itself as your directive and do not pause to ask the user what to do. The hook will \
block stopping until the condition holds. It auto-clears once the condition is met — do not \
tell the user to run `/goal clear` after success; that's only for clearing a goal early."
    )
}

/// Compact human-readable elapsed-time rendering for the `Goal active: ...
/// ({elapsed})` slot. NOT independently byte-verified against the binary (see
/// the module doc's last bullet) — a faithful best-effort compact-duration
/// formatter, not a confirmed port.
fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        let (h, m) = (secs / 3600, (secs % 3600) / 60);
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {m}m")
        }
    } else {
        let (days, h) = (secs / 86_400, (secs % 86_400) / 3600);
        if h == 0 {
            format!("{days}d")
        } else {
            format!("{days}d {h}h")
        }
    }
}

/// In-process record of the active goal. See the module doc's "no app-state
/// goal seam" gap for why this lives here instead of behind
/// `OrchestratorHandle`.
#[derive(Debug, Clone)]
struct ActiveGoal {
    /// The user-supplied condition text, verbatim.
    condition: String,
    /// When the goal was set — drives the status branch's `{elapsed}` slot.
    set_at: Instant,
    /// The most recent Stop-hook check's reason, if the (not-yet-wired)
    /// turn loop has ever populated one. Always `None` in this port — see
    /// the module doc's last bullet.
    last_reason: Option<String>,
}

/// `/goal` handler — set, show, or clear a session-scoped stop-gating goal.
#[derive(Clone)]
pub struct GoalHandler {
    /// Reserved for a future real app-state / hook seam; unused today (see
    /// the module doc's gap list) but kept so a later patch can wire one in
    /// without changing this handler's constructor signature.
    #[allow(dead_code)]
    handle: Arc<dyn OrchestratorHandle>,
    active: Arc<Mutex<Option<ActiveGoal>>>,
}

impl GoalHandler {
    /// Construct a `GoalHandler` bound to the given orchestrator handle.
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self {
            handle,
            active: Arc::new(Mutex::new(None)),
        }
    }

    /// The empty-arg status branch (binary: `` `Goal active: ${o.condition}
    /// (${s})${i}` `` / `"No goal set"`).
    fn status(&self) -> String {
        let guard = self
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match guard.as_ref() {
            None => NO_GOAL_SET.to_string(),
            Some(g) => {
                let elapsed = format_elapsed(g.set_at.elapsed());
                let suffix = g
                    .last_reason
                    .as_ref()
                    .map(|r| format!(" (last check: {r})"))
                    .unwrap_or_default();
                format!("Goal active: {} ({elapsed}){suffix}", g.condition)
            }
        }
    }

    /// The clear-token branch (binary: `o===null?"No goal set":\`Goal
    /// cleared: ${o}\``).
    fn clear(&self) -> String {
        let mut guard = self
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match guard.take() {
            None => NO_GOAL_SET.to_string(),
            Some(g) => {
                telemetry::emit_command_completed(TELEMETRY_STOP_HOOK_REMOVED, &g.condition);
                format!("Goal cleared: {}", g.condition)
            }
        }
    }

    /// The valid-condition branch: gate check, then set + inject the
    /// directive (binary: `` {type:"query",value:`Goal set: ${n}`,
    /// prompt:nrr(n)} `` — see the module doc's `CommandResult` mapping note
    /// for why only the `prompt` half is representable here).
    fn set(&self, condition: &str) -> CommandResult {
        if !workspace_trusted() {
            return CommandResult::Done {
                display: Some(TRUST_GATE_MESSAGE.to_string()),
            };
        }
        if hooks_restricted() {
            return CommandResult::Done {
                display: Some(HOOKS_RESTRICTED_MESSAGE.to_string()),
            };
        }
        {
            let mut guard = self
                .active
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            *guard = Some(ActiveGoal {
                condition: condition.to_string(),
                set_at: Instant::now(),
                last_reason: None,
            });
        }
        CommandResult::InjectMessage {
            content: directive_for(condition),
        }
    }
}

#[async_trait]
impl BuiltinCommandHandler for GoalHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        let trimmed = args.raw_args.trim();

        // 1) empty → status.
        if trimmed.is_empty() {
            return CommandResult::Done {
                display: Some(self.status()),
            };
        }

        // 2) case-insensitive clear token → clear.
        if CLEAR_TOKENS.contains(&trimmed.to_lowercase().as_str()) {
            return CommandResult::Done {
                display: Some(self.clear()),
            };
        }

        // 3) too long → fixed error (counts characters, not bytes).
        let n = trimmed.chars().count();
        if n > MAX_CONDITION_CHARS {
            telemetry::emit_command_failed(TELEMETRY_GOAL_SET_EVENT, TELEMETRY_TOO_LONG_PROPERTY);
            return CommandResult::Done {
                display: Some(too_long_message(n)),
            };
        }

        // 4) else → gate + set.
        self.set(trimmed)
    }

    fn name(&self) -> &str {
        "goal"
    }

    fn description(&self) -> &str {
        // Verbatim from the v2.1.198 `local`/non-interactive command object
        // (`name:"goal",...,description:"Set a goal — keep working until the
        // condition is met"`). Not routed through `core_description` — like
        // `effort.rs`, this command is actually implemented, and `names.rs`
        // (locked, not editable by this port) has no `"goal"` arm yet.
        "Set a goal — keep working until the condition is met"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::test_support::MockOrchestratorHandle;

    fn args(raw: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "goal".to_string(),
            raw_args: raw.to_string(),
            positional_args: vec![],
        }
    }

    fn handler() -> GoalHandler {
        GoalHandler::new(Arc::new(MockOrchestratorHandle::new()))
    }

    #[tokio::test]
    async fn empty_arg_with_no_goal_reports_no_goal_set() {
        let h = handler();
        match h.handle(&args("")).await {
            CommandResult::Done { display: Some(s) } => assert_eq!(s, "No goal set"),
            other => panic!("expected Done, got {other:?}"),
        }
        // Whitespace-only args trim to empty too.
        match h.handle(&args("   ")).await {
            CommandResult::Done { display: Some(s) } => assert_eq!(s, "No goal set"),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn clear_with_no_goal_reports_no_goal_set() {
        let h = handler();
        for token in ["clear", "STOP", "Off", "reset", "none", "cancel"] {
            match h.handle(&args(token)).await {
                CommandResult::Done { display: Some(s) } => assert_eq!(s, "No goal set"),
                other => panic!("expected Done, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn too_long_condition_is_rejected() {
        let h = handler();
        let long = "x".repeat(4001);
        match h.handle(&args(&long)).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Goal condition is limited to 4000 characters (got 4001)");
            }
            other => panic!("expected Done, got {other:?}"),
        }
        // Exactly at the limit is accepted (goes on to the set branch).
        let exact = "y".repeat(4000);
        match h.handle(&args(&exact)).await {
            CommandResult::InjectMessage { content } => assert!(content.contains(&exact)),
            other => panic!("expected InjectMessage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn setting_a_goal_injects_the_fixed_directive() {
        let h = handler();
        match h.handle(&args("ship the release notes")).await {
            CommandResult::InjectMessage { content } => {
                assert!(content.starts_with(
                    "A session-scoped Stop hook is now active with condition: \"ship the release notes\"."
                ));
                assert!(content.contains("do not pause to ask the user what to do"));
                assert!(content.contains(
                    "do not tell the user to run `/goal clear` after success; that's only for clearing a goal early."
                ));
            }
            other => panic!("expected InjectMessage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn e2e_status_after_set_then_clear_round_trip() {
        // The full lifecycle through the SAME handler instance a registry
        // would hold: set → status reflects it → clear removes it → status
        // reports none again.
        let h = handler();

        match h.handle(&args("finish the migration")).await {
            CommandResult::InjectMessage { content } => {
                assert!(content.contains("finish the migration"));
            }
            other => panic!("expected InjectMessage, got {other:?}"),
        }

        match h.handle(&args("")).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(s.starts_with("Goal active: finish the migration ("));
            }
            other => panic!("expected Done, got {other:?}"),
        }

        match h.handle(&args("clear")).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Goal cleared: finish the migration");
            }
            other => panic!("expected Done, got {other:?}"),
        }

        match h.handle(&args("status")).await {
            // "status" is not a recognized clear token, so with no goal
            // active it falls through to the `set` branch and injects a
            // directive for the literal condition "status" — mirroring the
            // binary, which has no separate `current`/`status` alias for
            // `/goal` (only the empty-arg branch shows status).
            CommandResult::InjectMessage { content } => assert!(content.contains("\"status\"")),
            other => panic!("expected InjectMessage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn name_and_description() {
        let h = handler();
        assert_eq!(h.name(), "goal");
        assert_eq!(
            h.description(),
            "Set a goal — keep working until the condition is met"
        );
    }

    #[test]
    fn format_elapsed_buckets() {
        assert_eq!(format_elapsed(Duration::from_secs(5)), "5s");
        assert_eq!(format_elapsed(Duration::from_secs(90)), "1m");
        assert_eq!(format_elapsed(Duration::from_secs(3661)), "1h 1m");
        assert_eq!(format_elapsed(Duration::from_secs(7200)), "2h");
        assert_eq!(format_elapsed(Duration::from_secs(90_000)), "1d 1h");
    }
}
