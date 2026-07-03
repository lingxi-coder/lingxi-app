//! `/fork` — spawn a background agent that inherits the full conversation.
//!
//! 1:1 port of the claude-code `type: 'local-jsx'` command `fork`, verified
//! against the shipped `claude.exe` v2.1.198 (compiled names `xrc`/`Ptf`):
//!
//! ```text
//! xrc={type:"local-jsx",name:"fork",description:"Spawn a background agent
//! that inherits the full conversation",argumentHint:"<directive>",
//! isEnabled:()=>!tv(),load:...}
//!
//! Ptf=async(e,t,n)=>{
//!   let r=n.trim();
//!   if(!r)return e("Usage: /fork \<directive\>",{display:"system"}),null;
//!   let o=await t$o(r,t,t.canUseTool??ND);
//!   if(!o)return e(tv()?"Forking is not available in coordinator sessions.
//!     Use /branch instead.":"Cannot fork before the first conversation turn",
//!     {display:"system"}),null;
//!   return e(`${Hnt} forked ${o.name} (${o.agentId.slice(-4)})`,
//!     {display:"system"}),null;
//! };
//! ```
//!
//! `Hnt` = `"⑂"` (U+2442, the OCR-FORK control-picture glyph) — byte-exact
//! in [`FORK_ICON`].
//!
//! ## Control-flow restructuring (headless, behavior-preserving)
//!
//! claude's `Ptf` *attempts* the fork first (`t$o(...)`) and only picks
//! between the two failure messages afterward, branching on `tv()`
//! (coordinator-mode) at that point. This headless port has no local-jsx
//! dialog surface to drive that two-step shape, so it restructures the same
//! three outcomes into explicit early-exit gates: empty directive → usage;
//! coordinator session → the `/branch` pointer; no prior turn → the
//! "cannot fork yet" notice; otherwise attempt the spawn. This changes no
//! observable behavior: `isEnabled:()=>!tv()` already keeps `/fork` out of
//! the palette in a coordinator session (so `t$o` is never reached with
//! `tv()` true via normal dispatch), and gating up-front just avoids a
//! doomed spawn attempt when the command is invoked directly.
//!
//! ## Trait-surface gap (flagged for the integration pass)
//!
//! Two [`traits::OrchestratorHandle`] methods this handler calls do not
//! exist on the trait yet:
//!
//!   * `async fn is_coordinator_session(&self) -> bool` — new additive
//!     default-`false` method, same shape as the existing
//!     `emit_coordinator_status` default.
//!   * `async fn fork_conversation(&self, directive: &str) ->
//!     Result<ForkOutcome, HandleError>` — new method. A benign additive
//!     default should return `Err(HandleError::Unimplemented("fork_conversation".into()))`,
//!     matching the documented purpose of the existing `Unimplemented`
//!     variant ("used by ... default impls ... so existing handle
//!     implementations don't need to override"). The concrete override
//!     belongs in the composition roots (`apps/engine-desktop`,
//!     `apps/engine-mobile`), delegating to the already-ported
//!     `traits::fork_subagent::build_forked_messages` /
//!     `build_child_message` plus the existing `BackgroundAgentSpawner` /
//!     `SubagentSpawner::spawn_async` lifecycle — this handler intentionally
//!     does not touch either (composition-root concern, not `commands/core`).
//!   * `traits::ForkOutcome { name: String, agent_id: String }` — new plain
//!     struct alongside `HandleError` in `traits::orchestrator`, re-exported
//!     from the crate root next to it.
//!
//! Until those three land this module will not compile on its own — by
//! design, per the multi-agent wiring split (this file is only the handler;
//! a separate integration pass adds the trait surface + registration).
//!
//! The `/branch` pointer in the coordinator message is otherwise dead in
//! LingXi today (`/branch` is currently registered as an interactive-only
//! command, not a real headless one), but the string is still correct
//! guidance for the (identical) interactive surface, so it is ported
//! byte-exact rather than reworded.

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use protocol::ConversationMessage;
use std::sync::Arc;
use traits::{ForkOutcome, OrchestratorHandle};

/// `${Hnt}` — the fork-success icon, U+2442 (OCR-FORK control-picture
/// glyph). Byte-exact with the claude-code v2.1.198 binary.
const FORK_ICON: char = '\u{2442}';

/// `/fork` handler — spawns a background agent that inherits the full
/// conversation (claude-code `commands/fork`, local-jsx `Ptf`/`xrc`).
#[derive(Clone)]
pub struct ForkHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl ForkHandler {
    /// Construct a `ForkHandler` bound to the given orchestrator handle.
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl BuiltinCommandHandler for ForkHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        // `let r = n.trim(); if (!r) return e("Usage: /fork <directive>", ...)`.
        let directive = args.raw_args.trim();
        if directive.is_empty() {
            return CommandResult::Done {
                display: Some("Usage: /fork <directive>".to_string()),
            };
        }

        // `isEnabled:()=>!tv()` hides /fork from the palette entirely in a
        // coordinator session; ported here as an explicit early gate (see
        // module docs) rather than a post-hoc branch on a failed spawn.
        if self.handle.is_coordinator_session().await {
            return CommandResult::Done {
                display: Some(
                    "Forking is not available in coordinator sessions. Use /branch instead."
                        .to_string(),
                ),
            };
        }

        // "Has had a first turn": the live transcript must end with an
        // assistant message — that message is what the fork prefix is built
        // from. No new trait surface needed; `conversation_transcript` is an
        // existing default method (empty `Vec` when no session is tracked).
        let transcript = self.handle.conversation_transcript().await;
        let has_first_turn =
            matches!(transcript.last(), Some(ConversationMessage::Assistant { .. }));
        if !has_first_turn {
            return CommandResult::Done {
                display: Some("Cannot fork before the first conversation turn".to_string()),
            };
        }

        match self.handle.fork_conversation(directive).await {
            Ok(ForkOutcome { name, agent_id }) => {
                // `o.agentId.slice(-4)` — last 4 chars (or fewer, for a
                // shorter id). Agent ids are ASCII, so a byte slice is safe.
                let tail = &agent_id[agent_id.len().saturating_sub(4)..];
                CommandResult::Done {
                    display: Some(format!("{FORK_ICON} forked {name} ({tail})")),
                }
            }
            Err(e) => {
                // No upstream error string is shown for this branch in the
                // binary excerpt; mirror the established
                // `Could not <verb>: {msg}` prefix pattern (see
                // `model.rs`'s `Could not switch model: {msg}`) instead of
                // inventing a byte-exact claim this port can't verify.
                let msg = e.to_string();
                CommandResult::Done {
                    display: Some(format!("Could not fork conversation: {msg}")),
                }
            }
        }
    }

    fn name(&self) -> &str {
        "fork"
    }

    fn description(&self) -> &str {
        // Verbatim claude-code v2.1.198 command-object description.
        "Spawn a background agent that inherits the full conversation"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::test_support::MockOrchestratorHandle;

    fn args(raw: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "fork".to_string(),
            raw_args: raw.to_string(),
            positional_args: raw.split_whitespace().map(str::to_string).collect(),
        }
    }

    fn handler() -> ForkHandler {
        ForkHandler::new(Arc::new(MockOrchestratorHandle::new()))
    }

    #[tokio::test]
    async fn empty_args_render_usage() {
        for raw in ["", "   "] {
            match handler().handle(&args(raw)).await {
                CommandResult::Done { display: Some(s) } => {
                    assert_eq!(s, "Usage: /fork <directive>");
                }
                other => panic!("expected Done, got {other:?}"),
            }
        }
    }

    /// Default mock: `is_coordinator_session` and `conversation_transcript`
    /// both fall back to their `OrchestratorHandle` defaults (`false` /
    /// empty), so a non-empty directive lands on the "no first turn yet"
    /// branch without needing any new mock setter.
    #[tokio::test]
    async fn no_first_turn_yet_blocks_fork() {
        match handler().handle(&args("Investigate the bug")).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Cannot fork before the first conversation turn");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn whitespace_only_directive_is_treated_as_empty() {
        match handler().handle(&args("   \t  ")).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Usage: /fork <directive>");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn name_and_description() {
        let h = handler();
        assert_eq!(h.name(), "fork");
        assert_eq!(
            h.description(),
            "Spawn a background agent that inherits the full conversation"
        );
    }
}
