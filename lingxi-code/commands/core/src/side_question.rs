//! `/btw` — ask a quick side question answered by an isolated, read-only,
//! tool-denied, single-turn side query.
//!
//! claude-code (`commands/btw/btw.tsx` + `utils/sideQuestion.ts`) wraps the
//! user's question in a fixed `<system-reminder>` (you are a separate
//! lightweight agent, the main agent is NOT interrupted, NO tools, one-off
//! response, answer from context only) and issues a single-turn forked/side
//! query sharing the parent prompt cache, then shows the markdown answer in a
//! throwaway dialog — the answer NEVER enters the LLM conversation history.
//!
//! This handler is the LingXi analog: it delegates to the
//! [`OrchestratorHandle::answer_side_question`] seam — the SAME history-inert
//! single-turn `ForkedAgentRunner` `/recap` uses (tool-denied + single-turn by
//! construction), differing only in the prompt (the wrapped question). The
//! answer is rendered as a transcript system line by the caller
//! (`CommandResult::Done { display }`), never appended to `session.history`, so
//! the load-bearing parity property is preserved.
//!
//! Delivered through the SAME `run_core_command` bridge `/recap` uses (a
//! throwaway current-thread `block_on` on the render-loop's blocking thread),
//! so the multi-second isolated query behaves exactly like `/recap`'s.

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use platform_api::{OrchestratorHandle, RecapOutcome};
use std::sync::Arc;

/// Shown when `/btw` is invoked with no question. (The TUI dispatcher also
/// guards the empty case before reaching the handler, so this is a defensive
/// fallback.)
const USAGE_MESSAGE: &str = "Usage: /btw <your question>";

/// Shown when the side query returned successfully but produced no text.
const NO_RESPONSE_MESSAGE: &str = "No response received";

/// Shown when the side query is aborted mid-flight.
const CANCELLED_MESSAGE: &str = "Side question cancelled.";

/// `/btw` handler.
///
/// Bound to an orchestrator handle; runs the real isolated side query via the
/// [`OrchestratorHandle::answer_side_question`] seam and renders the trimmed
/// answer as read-only system output.
#[derive(Clone)]
pub struct SideQuestionHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl SideQuestionHandler {
    /// Construct a `SideQuestionHandler` bound to the given orchestrator handle.
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl BuiltinCommandHandler for SideQuestionHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        let question = args.raw_args.trim();
        if question.is_empty() {
            return CommandResult::Done {
                display: Some(USAGE_MESSAGE.to_string()),
            };
        }
        telemetry::emit_command_started("tengu_command_btw_started");

        // Run the isolated, tool-denied side query via the SAME single-turn
        // forked-agent runner `/recap` uses (history-inert by construction — the
        // answer never enters `session.history`). Map its outcome:
        //   Text (non-empty) → the model's trimmed answer
        //   Text (empty)     → the fixed "No response received" line
        //   Cancelled        → the fixed "Side question cancelled." line
        //   Err              → an honest failure line
        match self.handle.answer_side_question(question).await {
            Ok(RecapOutcome::Text(text)) if !text.trim().is_empty() => {
                telemetry::emit_command_completed("tengu_command_btw_completed", "ok");
                CommandResult::Done {
                    display: Some(text),
                }
            }
            Ok(RecapOutcome::Text(_)) => {
                telemetry::emit_command_completed("tengu_command_btw_completed", "empty");
                CommandResult::Done {
                    display: Some(NO_RESPONSE_MESSAGE.to_string()),
                }
            }
            Ok(RecapOutcome::Cancelled) => {
                telemetry::emit_command_completed("tengu_command_btw_completed", "cancelled");
                CommandResult::Done {
                    display: Some(CANCELLED_MESSAGE.to_string()),
                }
            }
            Err(e) => {
                telemetry::emit_command_failed("tengu_command_btw_failed", &e.to_string());
                CommandResult::Done {
                    display: Some(format!("Couldn't answer side question: {e}")),
                }
            }
        }
    }

    fn name(&self) -> &str {
        "btw"
    }

    fn description(&self) -> &str {
        "Ask a quick side question without interrupting the main conversation"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::test_support::MockOrchestratorHandle;

    fn args(raw: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "btw".to_string(),
            raw_args: raw.to_string(),
            positional_args: raw.split_whitespace().map(str::to_string).collect(),
        }
    }

    #[tokio::test]
    async fn bare_invocation_renders_usage() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = SideQuestionHandler::new(mock);
        match h.handle(&args("   ")).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Usage: /btw <your question>")
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unwired_handle_renders_honest_failure() {
        // The mock leaves `answer_side_question` at its trait default
        // (`Unimplemented` → Err), so the handler renders the honest failure
        // line rather than a fabricated answer.
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = SideQuestionHandler::new(mock);
        match h.handle(&args("why is the sky blue?")).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(s.starts_with("Couldn't answer side question:"), "got: {s}");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = SideQuestionHandler::new(mock);
        assert_eq!(h.name(), "btw");
        assert_eq!(
            h.description(),
            "Ask a quick side question without interrupting the main conversation"
        );
    }
}
