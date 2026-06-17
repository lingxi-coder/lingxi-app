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

use async_trait::async_trait;
use command_api::builtin_support::names::core_description;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use std::sync::Arc;
use telemetry::tengu::command as cmd_evt;
use traits::OrchestratorHandle;

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
        telemetry::emit_command_started(cmd_evt::MODEL_STARTED);
        let trimmed = args.raw_args.trim();
        if trimmed.is_empty() {
            // List mode.
            let available = self.handle.list_available_models().await;
            let snap = self.handle.get_status_snapshot().await;
            telemetry::emit_command_completed(cmd_evt::MODEL_COMPLETED, "list");
            return CommandResult::Done {
                display: Some(format!(
                    "Current model: {}\nAvailable: {}",
                    snap.model,
                    available.join(", ")
                )),
            };
        }
        // Switch mode. Resolve an optional `profile/model` qualifier so a shared
        // id (offered by multiple providers) routes deterministically.
        let listings = self.handle.list_model_listings().await;
        let (model, profile) = traits::parse_model_ref(trimmed, &listings);
        match self.handle.switch_model(&model, profile.as_deref()).await {
            Ok(()) => {
                telemetry::emit_command_completed(cmd_evt::MODEL_COMPLETED, "switch");
                CommandResult::Done {
                    display: Some(format!("Switched to model: {model}")),
                }
            }
            Err(e) => {
                let msg = e.to_string();
                telemetry::emit_command_failed(cmd_evt::MODEL_FAILED, &msg);
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
    use orchestrator::test_support::MockOrchestratorHandle;

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
        let snap = traits::StatusSnapshot {
            model: "claude-opus-4-7".into(),
            ..traits::StatusSnapshot::default()
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

    /// `/model openai/gpt-5.2` with a fixture that has both openai and
    /// github-copilot offering `gpt-5.2` must pass `("gpt-5.2", Some("openai"))`
    /// to `switch_model`. A model unique to one provider (`gpt-4.1`) must pass
    /// `("gpt-4.1", None)`.
    #[tokio::test]
    async fn model_switch_parses_profile_qualified_ref() {
        use traits::ModelListing;
        fn listing(provider_id: &str, request_model: &str) -> ModelListing {
            ModelListing {
                display_model: request_model.to_string(),
                request_model: request_model.to_string(),
                provider_id: provider_id.to_string(),
                provider_label: provider_id.to_string(),
            }
        }

        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_model_listings(vec![
            listing("openai", "gpt-5.2"),
            listing("github-copilot", "gpt-5.2"),
            listing("openai", "gpt-4.1"),
        ]);
        let h = ModelHandler::new(mock.clone());

        // Qualified ref resolves to (bare model, profile).
        let _ = h.handle(&args("openai/gpt-5.2")).await;
        assert_eq!(
            mock.last_switch(),
            Some(("gpt-5.2".to_string(), Some("openai".to_string())))
        );

        // Unique bare model → no profile.
        let _ = h.handle(&args("gpt-4.1")).await;
        assert_eq!(
            mock.last_switch(),
            Some(("gpt-4.1".to_string(), None))
        );
    }
}
