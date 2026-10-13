//! Product effort choices. Provider defaults remain internal runtime state.
use std::sync::Arc;

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use lingxi_core::host::effort::{trim_js_whitespace, LEVELS};
use lingxi_core::host::{OrchestratorHandle, ReasoningSelection};

pub(crate) struct EffortHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl EffortHandler {
    pub(crate) fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl BuiltinCommandHandler for EffortHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        let value = trim_js_whitespace(&args.raw_args).to_lowercase();
        let snapshot = match self.handle.effort_command_snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return CommandResult::Done {
                    display: Some(format!("Failed to read effort state: {error}")),
                };
            }
        };
        let controls = self.handle.conversation_controls().await;
        let levels: Vec<&str> = LEVELS
            .into_iter()
            .filter(|level| {
                controls.as_ref().is_some_and(|controls| {
                    controls
                        .reasoning_spec
                        .available
                        .contains(&ReasoningSelection::Level {
                            id: (*level).into(),
                        })
                })
            })
            .collect();
        let ultra_available = self.handle.dynamic_workflows_enabled().await
            && snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.capabilities.xhigh);
        let choices = format!(
            "{}{}",
            levels.join("|"),
            if ultra_available {
                "|ultracode [on|off]"
            } else {
                ""
            }
        );
        let display = if matches!(value.as_str(), "" | "help" | "-h" | "--help") {
            if levels.is_empty() {
                "Effort levels are unavailable for the active model".into()
            } else {
                format!("Usage: /effort <{choices}>")
            }
        } else if matches!(value.as_str(), "current" | "status") {
            let environment = std::env::var(branding::EFFORT_LEVEL_ENV).ok();
            let current = match snapshot {
                Some(snapshot) => snapshot.displayed(environment.as_deref()),
                None => self
                    .handle
                    .current_effort()
                    .await
                    .unwrap_or_else(|| "Model default".into()),
            };
            format!("Current effort level: {current}")
        } else if levels.contains(&value.as_str())
            || (value == "med" && levels.contains(&"medium"))
            || (ultra_available
                && matches!(
                    value.as_str(),
                    "ultracode" | "ultracode on" | "ultracode off"
                ))
        {
            return command_api::builtins::effort::EffortHandler::new(self.handle.clone())
                .handle(args)
                .await;
        } else {
            format!(
                "Invalid argument: {}. Valid options are: {choices}",
                args.raw_args
            )
        };
        CommandResult::Done {
            display: Some(display),
        }
    }

    fn name(&self) -> &str {
        "effort"
    }
    fn description(&self) -> &str {
        "Set the model effort level"
    }
    fn argument_hint(&self) -> Option<&str> {
        Some("<level>")
    }
}
