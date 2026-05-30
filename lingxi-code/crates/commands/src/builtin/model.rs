//! `/model` — list (no arg) or switch (one arg) the active model.
//!
//! Locked display templates (`LingXi` UX, M5-11 T0 step 2 L3/L4):
//!   - No-arg list: `"Current model: {name}\nAvailable: {csv}"`
//!   - Switch:      `"Switched to model: {name}"`
//!
//! Failure prefix: `"Could not switch model: "`.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-11-commands-batch-2.md`
//! Task 4.

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;

/// `/model` handler — list / switch active model.
#[derive(Clone)]
pub struct ModelHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl ModelHandler {
    /// Construct a `ModelHandler` bound to the given orchestrator handle.
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl BuiltinCommandHandler for ModelHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit_command_started(cmd_evt::MODEL_STARTED);
        let trimmed = args.raw_args.trim();
        if trimmed.is_empty() {
            // List mode.
            let available = self.handle.list_available_models().await;
            let snap = self.handle.get_status_snapshot().await;
            lingxi_telemetry::emit_command_completed(cmd_evt::MODEL_COMPLETED, "list");
            return CommandResult::Done {
                display: Some(format!(
                    "Current model: {}\nAvailable: {}",
                    snap.model,
                    available.join(", ")
                )),
            };
        }
        // Switch mode.
        match self.handle.switch_model(trimmed).await {
            Ok(()) => {
                lingxi_telemetry::emit_command_completed(cmd_evt::MODEL_COMPLETED, "switch");
                CommandResult::Done {
                    display: Some(format!("Switched to model: {trimmed}")),
                }
            }
            Err(e) => {
                let msg = e.to_string();
                lingxi_telemetry::emit_command_failed(cmd_evt::MODEL_FAILED, &msg);
                CommandResult::Done {
                    display: Some(format!("Could not switch model: {msg}")),
                }
            }
        }
    }
    fn name(&self) -> &str {
        "model"
    }
    fn description(&self) -> &str {
        core_description("model")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_orchestrator::test_support::MockOrchestratorHandle;

    fn args(raw: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "model".to_string(),
            raw_args: raw.to_string(),
            positional_args: raw.split_whitespace().map(str::to_string).collect(),
        }
    }

    #[tokio::test]
    async fn list_mode_when_no_args() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_available_models(vec!["claude-opus-4-7".into(), "claude-sonnet-4-6".into()]);
        let snap = lingxi_traits::StatusSnapshot {
            model: "claude-opus-4-7".into(),
            ..lingxi_traits::StatusSnapshot::default()
        };
        mock.set_status_snapshot(snap);
        let h = ModelHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args("")).await {
            assert_eq!(
                s,
                "Current model: claude-opus-4-7\nAvailable: claude-opus-4-7, claude-sonnet-4-6"
            );
        } else {
            panic!();
        }
    }

    #[tokio::test]
    async fn switch_mode_when_arg_provided() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = ModelHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args("claude-haiku-4-5")).await
        {
            assert_eq!(s, "Switched to model: claude-haiku-4-5");
        } else {
            panic!();
        }
    }

    #[tokio::test]
    async fn switch_mode_with_whitespace_trim() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = ModelHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } =
            h.handle(&args("  claude-opus-4-7  ")).await
        {
            assert_eq!(s, "Switched to model: claude-opus-4-7");
        } else {
            panic!();
        }
    }

    #[tokio::test]
    async fn switch_failure_prefixes_error() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_switch_model_error("invalid name".into());
        let h = ModelHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args("foo")).await {
            assert_eq!(
                s,
                "Could not switch model: handle action failed: invalid name"
            );
        } else {
            panic!();
        }
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = ModelHandler::new(mock);
        assert_eq!(h.name(), "model");
        assert_eq!(h.description(), "Set the model for Claude Code to use");
    }
}
