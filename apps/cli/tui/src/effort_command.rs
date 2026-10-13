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
        // Native keeps both reset forms available even when this model has no named levels.
        let choices = format!(
            "{}{}auto|unset{}",
            levels.join("|"),
            if levels.is_empty() { "" } else { "|" },
            if ultra_available {
                "|ultracode [on|off]"
            } else {
                ""
            }
        );
        let display = if matches!(value.as_str(), "" | "help" | "-h" | "--help") {
            format!("Usage: /effort <{choices}>")
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
        } else if matches!(value.as_str(), "auto" | "unset")
            || levels.contains(&value.as_str())
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
        Some("<level|auto|unset>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_core::host::effort::{EffortCapabilities, EffortCommandSnapshot, EffortState};
    use lingxi_core::host::effort_table::SessionEffort;
    use lingxi_core::host::{
        ConversationControls, PermissionControlState, ReasoningControlSpec, StatusSnapshot,
    };
    use lingxi_core::types::utf16_json::Utf16JsonProjection;
    use orchestrator::test_support::MockOrchestratorHandle;

    fn parsed(args: &str) -> ParsedSlashCommand {
        command_api::parser::parse_slash_command(&Utf16JsonProjection::from(format!(
            "/effort {args}"
        )))
        .expect("valid effort command")
    }

    #[tokio::test]
    async fn auto_and_unset_clear_a_previously_selected_effort_level() {
        let handle = Arc::new(MockOrchestratorHandle::new());
        let model = "anthropic/claude-sonnet-4-5";
        handle.set_status_snapshot(StatusSnapshot {
            model: model.into(),
            ..StatusSnapshot::default()
        });
        handle.set_conversation_controls(ConversationControls {
            model_reference: model.into(),
            permission: PermissionControlState {
                requested: "default".into(),
                effective: "default".into(),
                modes: Vec::new(),
            },
            requested_reasoning_selection: ReasoningSelection::Automatic,
            effective_reasoning_selection: ReasoningSelection::Automatic,
            reasoning_spec: ReasoningControlSpec {
                available: LEVELS
                    .into_iter()
                    .map(|id| ReasoningSelection::Level { id: id.into() })
                    .collect(),
                selections_persistable: true,
                budget_range: None,
                provider_default: ReasoningSelection::Automatic,
                forced: false,
                modifiable: true,
                disabled_reason: None,
            },
        });

        let base = EffortCommandSnapshot {
            model: model.into(),
            settings_key: "claude-sonnet-4-5".into(),
            session: SessionEffort::Default,
            primary: None,
            capabilities: EffortCapabilities {
                supported: true,
                max: true,
                xhigh: true,
                thinking_disabled_cap: false,
            },
            state: EffortState::default(),
            user_settings_path: None,
            save_default: false,
            organization_start_effort: None,
        };
        handle.set_effort_command_source(move |active_model, session| {
            let mut snapshot = base.clone();
            snapshot.model = active_model.into();
            snapshot.primary = match &session {
                SessionEffort::Level(value) => Some(value.clone()),
                SessionEffort::Inherit | SessionEffort::Default => None,
            };
            snapshot.session = session;
            Ok(Some(snapshot))
        });

        let handler = EffortHandler::new(handle.clone());
        for reset in ["auto", "unset"] {
            handler.handle(&parsed("high")).await;
            assert_eq!(
                handle
                    .effort_command_snapshot()
                    .await
                    .unwrap()
                    .unwrap()
                    .session,
                SessionEffort::Level(serde_json::Value::String("high".into()))
            );

            handler.handle(&parsed(reset)).await;
            assert_eq!(
                handle
                    .effort_command_snapshot()
                    .await
                    .unwrap()
                    .unwrap()
                    .session,
                SessionEffort::Default,
                "{reset} clears the current session override"
            );
            assert_eq!(handle.current_effort().await, None);
        }
    }
}
