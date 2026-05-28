//! Hook loader (M6-07) — projects parsed settings JSON into a
//! `Vec<HookDefinition>` ready to feed into [`crate::HookRegistry::register`].
//!
//! Settings shape (claude-code compatible — `settings.json`):
//! ```json
//! {
//!   "hooks": {
//!     "PreToolUse": [
//!       { "matcher": "Write|Edit", "hooks": [
//!         { "type": "command", "command": "./fmt.sh", "timeout": 60 }
//!       ]}
//!     ]
//!   }
//! }
//! ```
//! Missing or empty `hooks` block yields `vec![]`. Unknown event names and
//! entries lacking a `command` are silently skipped — the loader is a
//! best-effort projection, not a strict validator.

use crate::definition::{HookCondition, HookDefinition, HookExecutor, HookSource};
use crate::events::HookEventType;
use lingxi_protocol::HookId;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// Top-level settings shape consumed by [`parse_hooks_from_settings_json`].
#[derive(Debug, Deserialize)]
struct SettingsTop {
    #[serde(default)]
    hooks: HashMap<String, Vec<MatcherGroup>>,
}

#[derive(Debug, Deserialize)]
struct MatcherGroup {
    #[serde(default)]
    matcher: Option<String>,
    #[serde(default)]
    hooks: Vec<HookEntry>,
}

#[derive(Debug, Deserialize)]
struct HookEntry {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
}

/// Parse the raw JSON string of a settings file into hook definitions.
///
/// Unknown event names are silently skipped. Entries without a `command`
/// field, or whose `type` is not `"command"`, are skipped. Returns
/// `Ok(vec![])` when the input has no `hooks` block at all.
///
/// `source` is propagated onto every returned [`HookDefinition`] so the
/// registry can later display trust info per origin.
pub fn parse_hooks_from_settings_json(
    raw: &str,
    source: HookSource,
) -> Result<Vec<HookDefinition>, serde_json::Error> {
    let top: SettingsTop = serde_json::from_str(raw)?;
    let mut out = Vec::new();
    for (event_name, groups) in top.hooks {
        let Some(event_type) = parse_event_type(&event_name) else {
            continue;
        };
        for group in groups {
            for entry in group.hooks {
                let Some(command) = entry.command else {
                    continue;
                };
                if entry.kind.as_deref() != Some("command") {
                    continue;
                }
                let condition = group.matcher.as_ref().map(|m| HookCondition {
                    pattern: m.clone(),
                    match_tool_name: true,
                    match_input: false,
                });
                out.push(HookDefinition {
                    id: HookId::new(),
                    name: command.clone(),
                    events: vec![event_type.clone()],
                    if_condition: condition,
                    executor: HookExecutor::Command {
                        command,
                        args: vec![],
                        env: HashMap::new(),
                        cwd: None,
                    },
                    source,
                    blocking: true,
                    timeout: entry.timeout.map(Duration::from_secs),
                    priority: 0,
                });
            }
        }
    }
    Ok(out)
}

fn parse_event_type(name: &str) -> Option<HookEventType> {
    match name {
        "PreToolUse" => Some(HookEventType::PreToolUse),
        "PostToolUse" => Some(HookEventType::PostToolUse),
        "Stop" => Some(HookEventType::Stop),
        "Notification" => Some(HookEventType::Notification),
        "UserPromptSubmit" => Some(HookEventType::UserPromptSubmit),
        "SubagentStop" => Some(HookEventType::SubagentStop),
        "PreCompact" => Some(HookEventType::PreCompact),
        "SessionStart" => Some(HookEventType::SessionStart),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_settings_yields_no_hooks() {
        let hooks = parse_hooks_from_settings_json("{}", HookSource::User).unwrap();
        assert!(hooks.is_empty());
    }

    #[test]
    fn one_pretooluse_command_hook() {
        let raw = r#"{
          "hooks": {
            "PreToolUse": [
              { "matcher": "Write|Edit", "hooks": [
                { "type": "command", "command": "./fmt.sh", "timeout": 30 }
              ]}
            ]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::Project).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].name, "./fmt.sh");
        assert_eq!(hooks[0].events, vec![HookEventType::PreToolUse]);
        assert_eq!(hooks[0].timeout, Some(Duration::from_secs(30)));
        assert_eq!(hooks[0].source, HookSource::Project);
        let cond = hooks[0].if_condition.as_ref().expect("matcher present");
        assert_eq!(cond.pattern, "Write|Edit");
    }

    #[test]
    fn unknown_event_is_skipped() {
        let raw =
            r#"{ "hooks": { "Bogus": [{ "hooks": [{ "type": "command", "command": "x" }]}]}}"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::User).unwrap();
        assert!(hooks.is_empty());
    }

    #[test]
    fn missing_command_field_is_skipped() {
        let raw = r#"{ "hooks": { "Stop": [{ "hooks": [{ "type": "command" }]}]}}"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::User).unwrap();
        assert!(hooks.is_empty());
    }
}
