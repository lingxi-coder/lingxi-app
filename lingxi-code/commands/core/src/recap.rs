//! `/recap` — one-line session recap via an isolated, read-only side query.
//!
//! Probed from the real 2.1.198 binary (`strings -a … | grep -F
//! 'name:"recap"'`): a `type:"local"` command, no `argumentHint` (no
//! positional args — `ParsedSlashCommand::raw_args`/`positional_args` are
//! unused here, matching e.g. `/clear`/`/doctor`, unlike `/voice`'s
//! `argumentHint:"[hold|tap|off]"`). Description verbatim:
//! `"Generate a one-line session recap now"`.
//!
//! ## Spec (behavior, not yet fully wireable — see GAP below)
//!
//! On invocation: (1) check whether the session has had at least one
//! qualifying turn — an assistant message, or a user compact-summary message
//! (the synthetic post-`/compact` continuation turn) — and if not, return the
//! fixed [`NO_TURN_MESSAGE`] immediately (this half IS implemented, via
//! [`OrchestratorHandle::conversation_transcript`]). (2) Otherwise issue an
//! isolated, single-turn (`maxTurns=1`) forked/side LLM query using the
//! current conversation context, with all tool use denied and the fixed
//! prompt text documented on the `recap_prompt_text_is_byte_exact` test below.
//! The query must NOT write to the transcript and must NOT mutate
//! conversation history/cache (upstream `skipTranscript:true`,
//! `skipCacheWrite:true`) — `/recap` is read-only w.r.t. session state, unlike
//! `/compact`. (3) On success, the model's trimmed text-block output becomes
//! the display. (4) If the underlying query itself returned an API-error
//! assistant message, that error message's own text is the display (dynamic,
//! not a fixed string). (5) If the caller aborts mid-flight, the fixed
//! `"Recap cancelled."` text (see the `cancelled_message_is_byte_exact` test
//! below) is returned. (6) On any other internal failure, the fixed
//! [`GENERIC_FAILURE_MESSAGE`].
//!
//! ## GAP: no fork-query primitive on `OrchestratorHandle`
//!
//! None of the exact needed methods exist yet on `traits::OrchestratorHandle`
//! (checked `traits/src/orchestrator.rs`). The closest analog is
//! [`OrchestratorHandle::force_compact`] (used by `commands/core/src/compact.rs`),
//! which proves LingXi already has real, wired, LLM-driven single-purpose
//! query infra (`CompactionOrchestrator` / `ForkedAgentRunner`) reachable from
//! a command handler — but `force_compact` truncates/replaces history, which
//! `/recap` must NOT do. A new trait method is needed, e.g. `async fn
//! generate_recap(&self) -> Result<RecapOutcome, HandleError>` (a
//! `{Ok(String), NoTurn, Aborted, Failed}`-shaped enum), implemented by
//! forking a single tool-denied, transcript-skipping turn through the same
//! forked-agent-runner machinery `/compact` uses, returning only the
//! assistant's concatenated text blocks (trimmed). Adding it is out of scope
//! here (`traits/` is a shared file this task must not edit); step (1) — the
//! only half expressible with the CURRENT trait surface — is implemented for
//! real below, and steps (2)-(4) fall back to the honest
//! [`GENERIC_FAILURE_MESSAGE`] until `generate_recap` lands. No `AuthHandle`
//! is needed either way (recap piggybacks on whatever model/session is
//! already authenticated and configured).

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use protocol::ConversationMessage;
use std::sync::Arc;
use traits::OrchestratorHandle;

/// Fixed no-arg-invocation display when the session has had zero qualifying
/// turns yet. Byte-exact from the 2.1.198 binary.
const NO_TURN_MESSAGE: &str = "Nothing to recap yet — send a message first.";

/// Fixed display for any internal failure other than the no-turn gate
/// (fallback system-prompt rebuild throws, or the forked query itself
/// throws). Byte-exact from the 2.1.198 binary. This is also the ONLY
/// reachable outcome once the no-turn gate passes, until the
/// `generate_recap` primitive described in the module docs lands — an honest
/// gap marker rather than a fabricated success.
const GENERIC_FAILURE_MESSAGE: &str = "Couldn't generate a recap. Run with --debug for details.";

/// Exact prefix `compaction::prompt::get_compact_user_summary_message` stamps
/// on the synthetic post-`/compact` continuation user turn (`"This session is
/// being continued from a previous conversation that ran out of context."`,
/// followed by the formatted summary). Duplicated here as a literal rather
/// than an import: `command-core`'s regular (non-test) dependency graph does
/// not include the `compaction` crate (it is dev-only, per
/// `commands/core/Cargo.toml`), and `Cargo.toml` is a shared file this task
/// must not edit. Used only to recognize the "qualifying turn" case (1) —
/// this handler never constructs or mutates such a message.
const COMPACT_SUMMARY_PREFIX: &str =
    "This session is being continued from a previous conversation that ran out of context.";

/// `/recap` handler.
///
/// Wires the real no-turn gate against the live transcript; the forked
/// side-query itself is a documented GAP (see module docs) — no
/// `traits::OrchestratorHandle` method exists yet to run it.
#[derive(Clone)]
pub struct RecapHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl RecapHandler {
    /// Construct a `RecapHandler` bound to the given orchestrator handle.
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }

    /// Whether `history` contains at least one qualifying turn: an assistant
    /// message, or a user message that is the synthetic post-`/compact`
    /// summary continuation (recognized by [`COMPACT_SUMMARY_PREFIX`]).
    /// Plain user messages and bare system messages do not qualify — a recap
    /// needs either a model turn to summarize or a compaction summary
    /// standing in for the earlier portion of the conversation.
    fn has_qualifying_turn(history: &[ConversationMessage]) -> bool {
        history.iter().any(|m| match m {
            ConversationMessage::Assistant { .. } => true,
            ConversationMessage::User { .. } => {
                m.text_content().starts_with(COMPACT_SUMMARY_PREFIX)
            }
            ConversationMessage::System { .. } => false,
        })
    }
}

#[async_trait]
impl BuiltinCommandHandler for RecapHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        telemetry::emit_command_started("tengu_command_recap_started");

        let history = self.handle.conversation_transcript().await;
        if !Self::has_qualifying_turn(&history) {
            telemetry::emit_command_completed("tengu_command_recap_completed", "no-turn");
            return CommandResult::Done {
                display: Some(NO_TURN_MESSAGE.to_string()),
            };
        }

        // GAP (see module docs): the isolated, tool-denied, transcript-skipping
        // fork query has no `OrchestratorHandle` primitive to call yet. Fail
        // honestly rather than fabricate a recap or silently run a mutating
        // path (`force_compact` would truncate history, which `/recap` must
        // never do).
        telemetry::emit_command_failed(
            "tengu_command_recap_failed",
            "no fork-query primitive on OrchestratorHandle yet (see recap.rs module docs)",
        );
        CommandResult::Done {
            display: Some(GENERIC_FAILURE_MESSAGE.to_string()),
        }
    }

    fn name(&self) -> &str {
        "recap"
    }

    fn description(&self) -> &str {
        "Generate a one-line session recap now"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::test_support::MockOrchestratorHandle;
    use protocol::{ContentBlock, MessageId};

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "recap".to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        }
    }

    fn user_text(text: &str) -> ConversationMessage {
        ConversationMessage::user(MessageId::new(), text.to_string())
    }

    fn assistant_text(text: &str) -> ConversationMessage {
        ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
        }
    }

    /// Minimal local stub overriding just `conversation_transcript` — the
    /// shared `MockOrchestratorHandle` (in `orchestrator::test_support`, a
    /// shared file out of scope for this task) has no transcript setter, so a
    /// tiny in-file stub exercises the qualifying-turn gate end-to-end without
    /// touching it. Every other method is a benign stub; only
    /// `conversation_transcript` is exercised by `RecapHandler::handle`.
    struct TranscriptStub(Vec<ConversationMessage>);

    #[async_trait]
    impl OrchestratorHandle for TranscriptStub {
        async fn current_session_id(&self) -> protocol::SessionId {
            protocol::SessionId::new()
        }
        async fn clear_session(&self) -> Result<(), traits::HandleError> {
            Ok(())
        }
        async fn force_compact(&self) -> Result<traits::CompactionSummary, traits::HandleError> {
            Ok(traits::CompactionSummary::default())
        }
        async fn snapshot_cost(&self) -> traits::CostSnapshot {
            traits::CostSnapshot::default()
        }
        async fn switch_model(
            &self,
            _model: &str,
            _profile: Option<&str>,
        ) -> Result<(), traits::HandleError> {
            Ok(())
        }
        async fn request_exit(&self) {}
        async fn current_should_exit(&self) -> bool {
            false
        }
        async fn open_memory_editor(&self) -> Result<traits::MemoryEditorOutcome, traits::HandleError> {
            Err(traits::HandleError::Unimplemented("stub".into()))
        }
        async fn list_mcp_servers(&self) -> Vec<traits::McpServerInfo> {
            Vec::new()
        }
        async fn list_hooks(&self) -> Vec<traits::HookInfo> {
            Vec::new()
        }
        async fn list_agents(&self) -> Vec<traits::AgentInfo> {
            Vec::new()
        }
        async fn run_doctor_checks(&self) -> traits::DoctorReport {
            traits::DoctorReport::default()
        }
        async fn get_status_snapshot(&self) -> traits::StatusSnapshot {
            traits::StatusSnapshot::default()
        }
        async fn edit_config_file(&self) -> Result<traits::MemoryEditorOutcome, traits::HandleError> {
            Err(traits::HandleError::Unimplemented("stub".into()))
        }
        async fn edit_permissions_file(
            &self,
        ) -> Result<traits::MemoryEditorOutcome, traits::HandleError> {
            Err(traits::HandleError::Unimplemented("stub".into()))
        }
        async fn list_available_models(&self) -> Vec<String> {
            Vec::new()
        }
        async fn conversation_transcript(&self) -> Vec<ConversationMessage> {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn no_turn_when_history_empty() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = RecapHandler::new(mock);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Nothing to recap yet — send a message first.");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_turn_when_only_plain_user_messages() {
        let handle = Arc::new(TranscriptStub(vec![user_text("hello"), user_text("world")]));
        let h = RecapHandler::new(handle);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Nothing to recap yet — send a message first.");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn qualifying_turn_with_assistant_message_falls_back_to_generic_failure() {
        // The no-turn gate passes (there IS a qualifying turn), but the
        // fork-query primitive doesn't exist yet (see module GAP docs) — the
        // honest fallback is the fixed generic-failure text, never a
        // fabricated recap.
        let handle = Arc::new(TranscriptStub(vec![
            user_text("please fix the bug"),
            assistant_text("Fixed it."),
        ]));
        let h = RecapHandler::new(handle);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Couldn't generate a recap. Run with --debug for details.");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn qualifying_turn_with_compact_summary_user_message() {
        // Post-/compact continuation turn (synthetic user message, no
        // assistant turn yet) also qualifies — recap should attempt to run
        // (and, pending the GAP, fall back to the generic failure) rather
        // than claim "nothing to recap".
        let summary_text = format!(
            "{COMPACT_SUMMARY_PREFIX}\n\nSummary:\nWe were mid-refactor of the parser."
        );
        let handle = Arc::new(TranscriptStub(vec![user_text(&summary_text)]));
        let h = RecapHandler::new(handle);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Couldn't generate a recap. Run with --debug for details.");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = RecapHandler::new(mock);
        assert_eq!(h.name(), "recap");
        assert_eq!(h.description(), "Generate a one-line session recap now");
    }

    /// Byte-exact prompt text the forked side-query MUST send once
    /// `generate_recap` lands (see module GAP docs) — probed from the
    /// 2.1.198 binary. Kept as a test-only constant (rather than a dead
    /// production `const`) so it doesn't trip `-D warnings` before it has a
    /// call site, per the same convention as `release_notes.rs`'s deferred
    /// `format_release_notes`.
    const RECAP_PROMPT: &str = "The user stepped away and is coming back. Recap in under 40 words, 1-2 plain sentences, no markdown. Lead with the overall goal and current task, then the one next action. Skip root-cause narrative, fix internals, secondary to-dos, and em-dash tangents.";

    #[test]
    fn recap_prompt_text_is_byte_exact() {
        assert!(!RECAP_PROMPT.is_empty());
        assert!(RECAP_PROMPT.starts_with("The user stepped away and is coming back."));
        assert!(RECAP_PROMPT.ends_with("em-dash tangents."));
    }

    /// Byte-exact cancellation text (spec item 5) — also test-only pending
    /// `generate_recap` (no cancellation signal reaches `BuiltinCommandHandler::handle`
    /// today; see module GAP docs).
    const CANCELLED_MESSAGE: &str = "Recap cancelled.";

    #[test]
    fn cancelled_message_is_byte_exact() {
        assert_eq!(CANCELLED_MESSAGE, "Recap cancelled.");
    }
}
