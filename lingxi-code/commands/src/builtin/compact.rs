//! `/compact` — runs a forced compaction pass and reports the summary.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-10-commands-batch-1.md`
//! Task 4.

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use std::sync::Arc;
use telemetry::tengu::command as cmd_evt;
use traits::OrchestratorHandle;

/// `/compact` handler — calls
/// [`OrchestratorHandle::force_compact`](traits::OrchestratorHandle::force_compact)
/// and renders the summary template.
#[derive(Clone)]
pub struct CompactHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl CompactHandler {
    /// Construct a `CompactHandler` bound to the given orchestrator handle.
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl BuiltinCommandHandler for CompactHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        telemetry::emit_command_started(cmd_evt::COMPACT_STARTED);
        match self.handle.force_compact().await {
            Ok(summary) => {
                let details = format!(
                    "{{\"messages_before\":{},\"messages_after\":{},\"bytes_saved\":{}}}",
                    summary.messages_before, summary.messages_after, summary.bytes_saved
                );
                telemetry::emit_command_completed(cmd_evt::COMPACT_COMPLETED, &details);
                CommandResult::Done {
                    display: Some(format!(
                        "Compacted: {} → {} messages ({} bytes saved).",
                        summary.messages_before, summary.messages_after, summary.bytes_saved
                    )),
                }
            }
            Err(e) => {
                let msg = e.to_string();
                telemetry::emit_command_failed(cmd_evt::COMPACT_FAILED, &msg);
                CommandResult::Done {
                    display: Some(format!("Could not compact: {msg}")),
                }
            }
        }
    }

    fn name(&self) -> &str {
        "compact"
    }

    fn description(&self) -> &str {
        core_description("compact")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::test_support::MockOrchestratorHandle;
    use traits::CompactionSummary;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "compact".to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        }
    }

    #[tokio::test]
    async fn success_renders_summary_template() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_compact_summary(CompactionSummary {
            messages_before: 42,
            messages_after: 7,
            bytes_saved: 18_345,
        });
        let h = CompactHandler::new(mock);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Compacted: 42 → 7 messages (18345 bytes saved).");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn failure_prefixes_handle_error() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_compact_error("model 429".to_string());
        let h = CompactHandler::new(mock);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Could not compact: handle action failed: model 429");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn zero_messages_renders_correctly() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_compact_summary(CompactionSummary {
            messages_before: 0,
            messages_after: 0,
            bytes_saved: 0,
        });
        let h = CompactHandler::new(mock);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Compacted: 0 → 0 messages (0 bytes saved).");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = CompactHandler::new(mock);
        assert_eq!(h.name(), "compact");
        assert_eq!(h.description(), "Compact the conversation to a summary");
    }

    /// M6-08 Task 12: end-to-end smoke through the real
    /// `ConversationOrchestrator` (not the mock). Confirms `/compact`
    /// renders a *real* non-zero delta — `40 → N` with `N < 40` — proving
    /// the trait, handler, and template are wired to the live compactor.
    ///
    /// Asserts on the COUNTS (real history collapse), NOT on the summary
    /// body (which is the M3 `[stub-summary …]` placeholder until M7
    /// wires a real `ForkedAgentRunner`).
    #[tokio::test]
    async fn real_orchestrator_renders_non_zero_delta() {
        use compaction::CompactionOrchestrator;
        use orchestrator::test_support::{
            noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
            StaticMemoryProvider,
        };
        use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
        use protocol::{ConversationMessage, MessageId};

        let api = Arc::new(MockApiClient::new(vec![]));
        let tools = Arc::new(tool_api::registry::ToolRegistry::new());
        let hooks = noop_hook_executor();
        let perms = Arc::new(NoOpPermissionGate);
        let output = Arc::new(MockOutputStream::new());
        let memory = Arc::new(StaticMemoryProvider::empty());
        let orch = Arc::new(
            ConversationOrchestrator::new(
                OrchestratorConfig::default(),
                api,
                tools,
                hooks,
                perms,
                output,
                memory,
                std::env::temp_dir(),
            )
            .with_compaction(Arc::new(CompactionOrchestrator::new(1_000))),
        );
        {
            let session = orch.session();
            let mut s = session.lock().await;
            for i in 0..40 {
                s.history.push(ConversationMessage::user(
                    MessageId::new(),
                    format!(
                        "msg-{i} body padding to push token count past the autocompact threshold"
                    ),
                ));
            }
        }

        let handle: Arc<dyn OrchestratorHandle> = orch.clone();
        let h = CompactHandler::new(handle);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(s.starts_with("Compacted: 40 → "), "got: {s}");
                assert!(!s.contains("Compacted: 40 → 40 "), "no-op detected: {s}");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }
}
