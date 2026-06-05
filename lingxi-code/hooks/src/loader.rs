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
//!
//! ## Event-name coverage
//!
//! [`parse_event_type`] recognizes all 27 of claude-code's `HOOK_EVENTS`
//! (`entrypoints/sdk/coreTypes.ts:25-53`); each maps to a
//! [`HookEventType`] variant.
//!
//! ## Per-hook fields & deferrals
//!
//! Only the `"command"` hook type is parsed; the `prompt` / `http` / `agent`
//! types are deferred (they need executor variants wired through the loader).
//! The additive `once` (`schemas/hooks.ts:51-54`) and `statusMessage`
//! (`schemas/hooks.ts:47-50`) fields ARE parsed and carried onto
//! [`HookDefinition`], but their behaviors — `once` self-removal-after-success
//! and the `statusMessage` spinner display — are executor / TUI work and are
//! NOT wired here.

use crate::definition::{HookCondition, HookDefinition, HookExecutor, HookSource};
use crate::events::HookEventType;
use protocol::HookId;
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
    /// claude-code `once` (`schemas/hooks.ts:51-54`): run once then remove.
    /// Parsed and carried onto [`HookDefinition::once`]; the self-removal
    /// runtime behavior is executor work (deferred).
    #[serde(default)]
    once: Option<bool>,
    /// claude-code `statusMessage` (`schemas/hooks.ts:47-50`): custom spinner
    /// text. Parsed and carried onto [`HookDefinition::status_message`]; the
    /// TUI spinner wiring is presentation work (deferred).
    #[serde(default, rename = "statusMessage")]
    status_message: Option<String>,
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
                    once: entry.once.unwrap_or(false),
                    status_message: entry.status_message.clone(),
                });
            }
        }
    }
    Ok(out)
}

/// Map a settings hook event-name string to its [`HookEventType`] variant.
///
/// The recognized names are claude-code's `HOOK_EVENTS`
/// (`entrypoints/sdk/coreTypes.ts:25-53`). Every one of those 27 event names
/// has a corresponding `HookEvent`/`HookEventType` variant in
/// [`crate::events`], so all 27 are recognized here. Unrecognized names
/// (including event names with no variant yet, and arbitrary typos) return
/// `None` and are silently skipped by the caller.
fn parse_event_type(name: &str) -> Option<HookEventType> {
    match name {
        "PreToolUse" => Some(HookEventType::PreToolUse),
        "PostToolUse" => Some(HookEventType::PostToolUse),
        "PostToolUseFailure" => Some(HookEventType::PostToolUseFailure),
        "Notification" => Some(HookEventType::Notification),
        "UserPromptSubmit" => Some(HookEventType::UserPromptSubmit),
        "SessionStart" => Some(HookEventType::SessionStart),
        "SessionEnd" => Some(HookEventType::SessionEnd),
        "Stop" => Some(HookEventType::Stop),
        "StopFailure" => Some(HookEventType::StopFailure),
        "SubagentStart" => Some(HookEventType::SubagentStart),
        "SubagentStop" => Some(HookEventType::SubagentStop),
        "PreCompact" => Some(HookEventType::PreCompact),
        "PostCompact" => Some(HookEventType::PostCompact),
        "PermissionRequest" => Some(HookEventType::PermissionRequest),
        "PermissionDenied" => Some(HookEventType::PermissionDenied),
        "Setup" => Some(HookEventType::Setup),
        "TeammateIdle" => Some(HookEventType::TeammateIdle),
        "TaskCreated" => Some(HookEventType::TaskCreated),
        "TaskCompleted" => Some(HookEventType::TaskCompleted),
        "Elicitation" => Some(HookEventType::Elicitation),
        "ElicitationResult" => Some(HookEventType::ElicitationResult),
        "ConfigChange" => Some(HookEventType::ConfigChange),
        "WorktreeCreate" => Some(HookEventType::WorktreeCreate),
        "WorktreeRemove" => Some(HookEventType::WorktreeRemove),
        "InstructionsLoaded" => Some(HookEventType::InstructionsLoaded),
        "CwdChanged" => Some(HookEventType::CwdChanged),
        "FileChanged" => Some(HookEventType::FileChanged),
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

    /// A `command` entry under `event_name` (no matcher) so the parsed
    /// definition reflects exactly one event.
    fn one_command(event_name: &str) -> String {
        format!(
            r#"{{ "hooks": {{ "{event_name}": [{{ "hooks": [
                {{ "type": "command", "command": "x" }}
            ]}}]}}}}"#
        )
    }

    #[test]
    fn all_27_event_names_parse_to_their_variant() {
        // Mirrors claude-code HOOK_EVENTS (entrypoints/sdk/coreTypes.ts:25-53).
        // Each (settings name -> expected HookEventType) must round-trip.
        let cases: &[(&str, HookEventType)] = &[
            ("PreToolUse", HookEventType::PreToolUse),
            ("PostToolUse", HookEventType::PostToolUse),
            ("PostToolUseFailure", HookEventType::PostToolUseFailure),
            ("Notification", HookEventType::Notification),
            ("UserPromptSubmit", HookEventType::UserPromptSubmit),
            ("SessionStart", HookEventType::SessionStart),
            ("SessionEnd", HookEventType::SessionEnd),
            ("Stop", HookEventType::Stop),
            ("StopFailure", HookEventType::StopFailure),
            ("SubagentStart", HookEventType::SubagentStart),
            ("SubagentStop", HookEventType::SubagentStop),
            ("PreCompact", HookEventType::PreCompact),
            ("PostCompact", HookEventType::PostCompact),
            ("PermissionRequest", HookEventType::PermissionRequest),
            ("PermissionDenied", HookEventType::PermissionDenied),
            ("Setup", HookEventType::Setup),
            ("TeammateIdle", HookEventType::TeammateIdle),
            ("TaskCreated", HookEventType::TaskCreated),
            ("TaskCompleted", HookEventType::TaskCompleted),
            ("Elicitation", HookEventType::Elicitation),
            ("ElicitationResult", HookEventType::ElicitationResult),
            ("ConfigChange", HookEventType::ConfigChange),
            ("WorktreeCreate", HookEventType::WorktreeCreate),
            ("WorktreeRemove", HookEventType::WorktreeRemove),
            ("InstructionsLoaded", HookEventType::InstructionsLoaded),
            ("CwdChanged", HookEventType::CwdChanged),
            ("FileChanged", HookEventType::FileChanged),
        ];
        assert_eq!(cases.len(), 27, "claude-code HOOK_EVENTS has 27 names");
        for (name, expected) in cases {
            let raw = one_command(name);
            let hooks = parse_hooks_from_settings_json(&raw, HookSource::User).unwrap();
            assert_eq!(hooks.len(), 1, "event {name} should produce one hook");
            assert_eq!(
                hooks[0].events,
                vec![expected.clone()],
                "event name {name} must map to {expected:?}",
            );
        }
    }

    #[test]
    fn newly_recognized_event_parses_with_no_matcher() {
        // FileChanged was NOT recognized before this change; assert it now is
        // and carries no if-condition when no matcher is supplied.
        let raw = one_command("FileChanged");
        let hooks = parse_hooks_from_settings_json(&raw, HookSource::Project).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].events, vec![HookEventType::FileChanged]);
        assert!(hooks[0].if_condition.is_none());
    }

    #[test]
    fn once_true_parses_onto_definition() {
        let raw = r#"{
          "hooks": {
            "Stop": [{ "hooks": [
              { "type": "command", "command": "cleanup.sh", "once": true }
            ]}]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::User).unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(hooks[0].once, "once:true must be carried onto the definition");
        assert_eq!(hooks[0].status_message, None);
    }

    #[test]
    fn once_defaults_to_false_when_absent() {
        let raw = one_command("Stop");
        let hooks = parse_hooks_from_settings_json(&raw, HookSource::User).unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(!hooks[0].once, "absent once must default to false");
    }

    #[test]
    fn status_message_parses_onto_definition() {
        let raw = r#"{
          "hooks": {
            "PostToolUse": [{ "hooks": [
              { "type": "command", "command": "fmt.sh", "statusMessage": "Formatting…" }
            ]}]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::User).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].status_message.as_deref(), Some("Formatting…"));
        assert!(!hooks[0].once, "absent once must default to false");
    }

    #[test]
    fn once_and_status_message_parse_together() {
        let raw = r#"{
          "hooks": {
            "PreToolUse": [{ "matcher": "Write|Edit", "hooks": [
              { "type": "command", "command": "guard.sh",
                "once": true, "statusMessage": "Guarding" }
            ]}]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::Project).unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(hooks[0].once);
        assert_eq!(hooks[0].status_message.as_deref(), Some("Guarding"));
        assert_eq!(
            hooks[0].if_condition.as_ref().map(|c| c.pattern.as_str()),
            Some("Write|Edit"),
        );
    }
}
