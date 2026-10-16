//! Product effort choices. Provider defaults remain internal runtime state.
use std::sync::Arc;

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use lingxi_core::host::effort::{javascript_whitespace, trim_js_whitespace, LEVELS};
use lingxi_core::host::{OrchestratorHandle, ReasoningSelection};
use lingxi_core::types::utf16_json::Utf16JsonProjection;

fn projection_from_utf16(code_units: Vec<u16>) -> Utf16JsonProjection {
    let display = String::from_utf16_lossy(&code_units);
    Utf16JsonProjection::root_string(display, code_units)
        .expect("display text is derived from the same exact UTF-16 units")
}

fn invalid_argument_display(raw_args: &[u16], choices: &str) -> Utf16JsonProjection {
    let mut code_units = "Invalid argument: ".encode_utf16().collect::<Vec<_>>();
    code_units.extend_from_slice(raw_args);
    code_units.extend(". Valid options are: ".encode_utf16());
    code_units.extend(choices.encode_utf16());
    projection_from_utf16(code_units)
}

fn is_ultracode_option(value: &str) -> bool {
    matches!(
        value
            .split(javascript_whitespace)
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>()
            .as_slice(),
        ["ultracode"] | ["ultracode", "on"] | ["ultracode", "off"]
    )
}

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
        let raw_args_units = args.raw_args.string_units("").unwrap_or_default();
        // `raw_args_utf8` refuses a source projection with an isolated surrogate;
        // that input can still be shown in an error projection below without
        // replacing its exact UTF-16 code unit.
        let value = args
            .raw_args_utf8()
            .map(|raw| trim_js_whitespace(raw).to_ascii_lowercase());
        let snapshot = match self.handle.effort_command_snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return CommandResult::Done {
                    display: Some(format!("Failed to read effort state: {error}").into()),
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
        if value.as_deref().is_some_and(|value| {
            matches!(value, "auto" | "unset")
                || levels.contains(&value)
                || (value == "med" && levels.contains(&"medium"))
                || (ultra_available && is_ultracode_option(value))
        }) {
            return command_api::builtins::effort::EffortHandler::new(self.handle.clone())
                .handle(args)
                .await;
        }

        let display = match value.as_deref() {
            Some(value) if matches!(value, "" | "help" | "-h" | "--help") => {
                format!("Usage: /effort <{choices}>").into()
            }
            Some(value) if matches!(value, "current" | "status") => {
                let environment = std::env::var(branding::EFFORT_LEVEL_ENV).ok();
                let current = match snapshot {
                    Some(snapshot) => snapshot.displayed(environment.as_deref()),
                    None => self
                        .handle
                        .current_effort()
                        .await
                        .unwrap_or_else(|| "Model default".into()),
                };
                format!("Current effort level: {current}").into()
            }
            Some(_) | None => invalid_argument_display(&raw_args_units, &choices),
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

    fn parsed_units(raw_args: &[u16]) -> ParsedSlashCommand {
        let mut input_units = "/effort ".encode_utf16().collect::<Vec<_>>();
        input_units.extend_from_slice(raw_args);
        let input = projection_from_utf16(input_units);
        command_api::parser::parse_slash_command(&input).expect("valid effort command")
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

    #[tokio::test]
    async fn ultracode_accepts_ascii_case_and_javascript_whitespace() {
        let handle = Arc::new(MockOrchestratorHandle::new());
        let model = "anthropic/claude-sonnet-4-5";
        handle.set_status_snapshot(StatusSnapshot {
            model: model.into(),
            ..StatusSnapshot::default()
        });
        handle.set_dynamic_workflows_gate(true, false);
        handle.set_effort_command_source(move |active_model, session| {
            Ok(Some(EffortCommandSnapshot {
                model: active_model.into(),
                settings_key: "claude-sonnet-4-5".into(),
                session,
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
            }))
        });

        let result = EffortHandler::new(handle.clone())
            .handle(&parsed("ULTRACODE\u{00a0}ON"))
            .await;
        assert!(handle.ultracode_enabled().await);
        let CommandResult::Done {
            display: Some(display),
        } = result
        else {
            panic!("ultracode option must complete normally");
        };
        assert!(display.string_at("").unwrap().starts_with("Ultracode on"));
    }

    #[tokio::test]
    async fn invalid_utf16_argument_is_preserved_in_error_display() {
        let handle = Arc::new(MockOrchestratorHandle::new());
        handle.set_effort_command_source(|_, _| Ok(None));
        let raw_args = [b'X' as u16, 0xd800, b'Y' as u16];

        let result = EffortHandler::new(handle)
            .handle(&parsed_units(&raw_args))
            .await;
        let CommandResult::Done {
            display: Some(display),
        } = result
        else {
            panic!("invalid option must return a display message");
        };
        let mut expected = "Invalid argument: ".encode_utf16().collect::<Vec<_>>();
        expected.extend_from_slice(&raw_args);
        expected.extend(". Valid options are: auto|unset".encode_utf16());
        assert_eq!(display.string_units(""), Some(expected));
    }
}
