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
//! The `"command"`, `"http"`, `"agent"`, and `"prompt"` hook types are parsed
//! (mapping onto [`HookExecutor::Command`] / [`HookExecutor::Http`] /
//! [`HookExecutor::Agent`] / [`HookExecutor::Prompt`] respectively; the executor
//! already routes each variant to its runner in `executor.rs`). The `"prompt"`
//! type (`schemas/hooks.ts:67-95`) is the inline single-turn LLM evaluator
//! (`execPromptHook.ts`): its `prompt` (required) and optional `model` are
//! carried onto [`HookExecutor::Prompt`], and `prompt_executor.rs` runs it
//! against the injected `HookPromptRunner` (a structured no-op when no runner is
//! wired). A `prompt` entry missing its `prompt` field is skipped.
//!
//! The additive `once` (`schemas/hooks.ts:51-54`) and `statusMessage`
//! (`schemas/hooks.ts:47-50`) fields ARE parsed and carried onto
//! [`HookDefinition`], but their behaviors — `once` self-removal-after-success
//! and the `statusMessage` spinner display — are executor / TUI work and are
//! NOT wired here.
//!
//! ### `http` / `agent` field-level deferrals
//!
//! The claude-code `http` schema (`schemas/hooks.ts:97-126`) carries `headers`
//! env-var interpolation gated by an `allowedEnvVars` allowlist; the Rust
//! [`HookExecutor::Http`] variant has no `allowed_env_vars` field, so headers
//! are carried verbatim and `allowedEnvVars` is dropped (interpolation deferred
//! — would need a frozen-side field). The `agent` schema
//! (`schemas/hooks.ts:128-163`) carries NO `agent_type` (claude-code's agent
//! hook runs an inline verifier `query()`, not a named subagent); the Rust
//! [`HookExecutor::Agent`] variant requires a non-optional `agent_type`, so it
//! is filled with the crate's canonical default `"general-purpose"` (matching
//! the agent-hook fixtures in `parity_hooks_runtime.rs` and the default in
//! `agent_executor.rs`). The `model` field both schemas allow is dropped: no
//! Rust variant carries it.

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
    /// `command` hook executable (`schemas/hooks.ts:33-34`).
    #[serde(default)]
    command: Option<String>,
    /// `http` hook endpoint URL (`schemas/hooks.ts:99`). Always sent via POST.
    #[serde(default)]
    url: Option<String>,
    /// `http` hook request headers (`schemas/hooks.ts:106-111`). Carried
    /// verbatim; `allowedEnvVars` env-var interpolation is deferred (no Rust
    /// field for the allowlist).
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    /// `agent` / `prompt` hook prompt text (`schemas/hooks.ts:138-142` /
    /// `67-73`). For an `agent` hook it is the verifier prompt; for a `prompt`
    /// hook it is the inline-LLM evaluation prompt (with `$ARGUMENTS`).
    #[serde(default)]
    prompt: Option<String>,
    /// `prompt` / `agent` hook model override (`schemas/hooks.ts:81-86`).
    /// Consumed only by the `prompt` arm; the `agent` arm has no model field on
    /// its [`HookExecutor::Agent`] variant, so it is dropped there.
    #[serde(default)]
    model: Option<String>,
    /// `timeout` in seconds, shared by all hook types
    /// (`schemas/hooks.ts:42-46` / `75-79` / `101-105` / `144-148`).
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
    /// claude-code per-hook `if` (`schemas/hooks.ts:35` `IfConditionSchema`):
    /// a permission-rule-syntax pattern (e.g. `"Bash(git push:*)"`) that gates
    /// the hook on the tool name + tool input matching. Parsed and carried onto
    /// [`HookCondition::if_pattern`], evaluated by
    /// [`crate::matcher::matches_if_condition`] in `match_event`.
    #[serde(default, rename = "if")]
    if_pattern: Option<String>,
}

/// Default agent type for an `agent` hook.
///
/// claude-code's `agent` schema (`schemas/hooks.ts:128-163`) carries NO agent
/// type — its agent hook runs an inline verifier `query()` rather than a named
/// subagent. The Rust [`HookExecutor::Agent`] variant, by contrast, spawns a
/// subagent and so requires a non-optional `agent_type`. This crate's canonical
/// default (the `agent_executor.rs` spawn target, the `parity_hooks_runtime.rs`
/// fixture, and the `agent_type` doc example in `definition.rs`) is
/// `"general-purpose"`, so that is what an `agent` settings entry resolves to.
const DEFAULT_AGENT_TYPE: &str = "general-purpose";

/// Parse the raw JSON string of a settings file into hook definitions.
///
/// Unknown event names are silently skipped. An entry whose `type` is
/// `"command"`/`"http"`/`"agent"`/`"prompt"` but is missing the field that type
/// requires (`command` / `url` / `prompt` / `prompt`) is skipped, as is any
/// entry whose `type` is unrecognized or absent. Returns `Ok(vec![])` when the
/// input has no `hooks` block at all.
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
                let Some((name, executor)) = build_executor(&entry) else {
                    continue;
                };
                // The tool-name `matcher` is per-GROUP; the `if`-condition is
                // per-HOOK — combine them into one condition built per entry so a
                // hook can carry both (claude-code applies both filters).
                let condition =
                    build_condition(group.matcher.as_deref(), entry.if_pattern.as_deref());
                out.push(HookDefinition {
                    id: HookId::new(),
                    name,
                    events: vec![event_type.clone()],
                    if_condition: condition,
                    executor,
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

/// Combine a group's tool-name `matcher` and a hook's `if`-condition into a
/// single [`HookCondition`], or `None` when neither is present.
///
/// `matcher` (per-group) populates the B3 tool-name fields; `if_pattern`
/// (per-hook) populates the `if`-condition field. A hook may carry both — both
/// gate firing in `match_event`, mirroring claude-code's two sequential filters.
fn build_condition(matcher: Option<&str>, if_pattern: Option<&str>) -> Option<HookCondition> {
    if matcher.is_none() && if_pattern.is_none() {
        return None;
    }
    Some(HookCondition {
        pattern: matcher.unwrap_or_default().to_string(),
        match_tool_name: matcher.is_some(),
        match_input: if_pattern.is_some(),
        if_pattern: if_pattern.map(ToString::to_string),
    })
}

/// Project a single settings [`HookEntry`] onto its `(name, HookExecutor)`.
///
/// Returns `None` (so the caller skips the entry) when the `type` is missing,
/// unrecognized, or is a known type missing its required field. The per-type
/// timeout is NOT consumed here — it is carried
/// onto [`HookDefinition::timeout`] by the caller for every type uniformly, so
/// the executor's per-hook-timeout logic applies identically across arms.
fn build_executor(entry: &HookEntry) -> Option<(String, HookExecutor)> {
    match entry.kind.as_deref() {
        Some("command") => {
            let command = entry.command.clone()?;
            let executor = HookExecutor::Command {
                command: command.clone(),
                args: vec![],
                env: HashMap::new(),
                cwd: None,
            };
            Some((command, executor))
        }
        Some("http") => {
            let url = entry.url.clone()?;
            let executor = HookExecutor::Http {
                url: url.clone(),
                // claude-code always POSTs the hook input JSON
                // (`utils/hooks/execHttpHook.ts`; `schemas/hooks.ts:99`).
                method: "POST".to_string(),
                headers: entry.headers.clone().unwrap_or_default(),
                // A `timeout: 0` (or omitted) defers to the executor's HTTP
                // default; the parsed seconds value is the per-hook override.
                timeout: entry
                    .timeout
                    .map_or(Duration::ZERO, Duration::from_secs),
            };
            Some((url, executor))
        }
        Some("agent") => {
            let prompt = entry.prompt.clone()?;
            let executor = HookExecutor::Agent {
                agent_type: DEFAULT_AGENT_TYPE.to_string(),
                prompt,
            };
            // The hook name mirrors the command-hook convention of naming the
            // hook after its primary user-supplied field — here the agent type.
            Some((DEFAULT_AGENT_TYPE.to_string(), executor))
        }
        Some("prompt") => {
            // `prompt` hook (`schemas/hooks.ts:67-95`): the inline single-turn
            // LLM evaluator (`execPromptHook.ts`). Routes to
            // [`HookExecutor::Prompt`], executed by `prompt_executor.rs` against
            // the injected `HookPromptRunner`.
            let prompt = entry.prompt.clone()?;
            let executor = HookExecutor::Prompt {
                prompt,
                model: entry.model.clone(),
            };
            // The hook name mirrors the command/http/agent convention; the
            // prompt's primary user-supplied field is the prompt itself, so
            // name the hook `"prompt"` (the type) to stay short and stable.
            Some(("prompt".to_string(), executor))
        }
        // Any other unknown type and a missing `type` are skipped.
        _ => None,
    }
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
        // No `if` → if_pattern stays None, match_input false.
        assert_eq!(hooks[0].if_pattern(), None);
        assert!(!cond.match_input);
    }

    #[test]
    fn per_hook_if_condition_is_parsed_and_combines_with_matcher() {
        let raw = r#"{
          "hooks": {
            "PreToolUse": [
              { "matcher": "Bash", "hooks": [
                { "type": "command", "command": "./guard.sh", "if": "Bash(git push:*)" }
              ]}
            ]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::Project).unwrap();
        assert_eq!(hooks.len(), 1);
        // Tool-name matcher AND the `if`-condition are both carried on one condition.
        assert_eq!(hooks[0].matcher(), Some("Bash"));
        assert_eq!(hooks[0].if_pattern(), Some("Bash(git push:*)"));
        let cond = hooks[0].if_condition.as_ref().expect("condition present");
        assert!(cond.match_tool_name);
        assert!(cond.match_input);
    }

    #[test]
    fn if_condition_without_matcher_builds_condition() {
        let raw = r#"{
          "hooks": {
            "PreToolUse": [
              { "hooks": [
                { "type": "command", "command": "./guard.sh", "if": "Bash(rm:*)" }
              ]}
            ]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::User).unwrap();
        assert_eq!(hooks.len(), 1);
        // No group matcher → no tool-name matcher, but the `if` is carried.
        assert_eq!(hooks[0].matcher(), None);
        assert_eq!(hooks[0].if_pattern(), Some("Bash(rm:*)"));
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

    // ---- http hook parsing (schemas/hooks.ts:97-126) -----------------------

    #[test]
    fn http_hook_parses_to_http_executor_with_url_headers_timeout() {
        let raw = r#"{
          "hooks": {
            "PreToolUse": [{ "matcher": "Write|Edit", "hooks": [
              { "type": "http",
                "url": "https://hooks.example.com/pre",
                "headers": { "Authorization": "Bearer t", "X-Env": "prod" },
                "timeout": 12 }
            ]}]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::Project).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].events, vec![HookEventType::PreToolUse]);
        assert_eq!(hooks[0].source, HookSource::Project);
        // The per-hook timeout is carried onto the definition (seconds).
        assert_eq!(hooks[0].timeout, Some(Duration::from_secs(12)));
        let HookExecutor::Http {
            url,
            method,
            headers,
            timeout,
        } = &hooks[0].executor
        else {
            panic!("expected Http executor, got {:?}", hooks[0].executor);
        };
        assert_eq!(url, "https://hooks.example.com/pre");
        // claude-code always POSTs the hook input JSON.
        assert_eq!(method, "POST");
        assert_eq!(headers.get("Authorization").map(String::as_str), Some("Bearer t"));
        assert_eq!(headers.get("X-Env").map(String::as_str), Some("prod"));
        // The per-hook timeout is mirrored onto the executor's `timeout`.
        assert_eq!(*timeout, Duration::from_secs(12));
        // The hook name reflects the URL (primary user-supplied field).
        assert_eq!(hooks[0].name, "https://hooks.example.com/pre");
        // No matcher-independent matcher pattern is lost.
        assert_eq!(
            hooks[0].if_condition.as_ref().map(|c| c.pattern.as_str()),
            Some("Write|Edit"),
        );
    }

    #[test]
    fn http_hook_without_headers_defaults_to_empty_and_zero_timeout() {
        // No `headers` and no `timeout`: headers default empty, executor timeout
        // is ZERO (the executor then falls back to its HTTP default), and the
        // definition-level timeout is None.
        let raw = r#"{
          "hooks": {
            "PostToolUse": [{ "hooks": [
              { "type": "http", "url": "https://h.test/post" }
            ]}]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::User).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].timeout, None);
        let HookExecutor::Http {
            url,
            headers,
            timeout,
            ..
        } = &hooks[0].executor
        else {
            panic!("expected Http executor");
        };
        assert_eq!(url, "https://h.test/post");
        assert!(headers.is_empty());
        assert_eq!(*timeout, Duration::ZERO);
    }

    #[test]
    fn http_hook_missing_url_is_skipped() {
        let raw = r#"{ "hooks": { "PreToolUse": [{ "hooks": [
            { "type": "http", "timeout": 5 }
        ]}]}}"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::User).unwrap();
        assert!(hooks.is_empty(), "an http entry without a url is skipped");
    }

    // ---- agent hook parsing (schemas/hooks.ts:128-163) ---------------------

    #[test]
    fn agent_hook_parses_to_agent_executor_with_prompt_and_default_type() {
        let raw = r#"{
          "hooks": {
            "Stop": [{ "hooks": [
              { "type": "agent",
                "prompt": "Verify that unit tests ran and passed.",
                "timeout": 90 }
            ]}]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::Local).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].events, vec![HookEventType::Stop]);
        assert_eq!(hooks[0].source, HookSource::Local);
        assert_eq!(hooks[0].timeout, Some(Duration::from_secs(90)));
        let HookExecutor::Agent { agent_type, prompt } = &hooks[0].executor else {
            panic!("expected Agent executor, got {:?}", hooks[0].executor);
        };
        assert_eq!(prompt, "Verify that unit tests ran and passed.");
        // The schema carries no agent type; the crate default is used.
        assert_eq!(agent_type, "general-purpose");
    }

    #[test]
    fn agent_hook_missing_prompt_is_skipped() {
        let raw = r#"{ "hooks": { "Stop": [{ "hooks": [
            { "type": "agent", "timeout": 30 }
        ]}]}}"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::User).unwrap();
        assert!(hooks.is_empty(), "an agent entry without a prompt is skipped");
    }

    // ---- prompt hook parsing (schemas/hooks.ts:67-95) + mixed batches ------

    #[test]
    fn prompt_hook_parses_to_prompt_executor_with_prompt_and_model() {
        // `prompt` is the inline single-turn LLM evaluator (execPromptHook.ts).
        let raw = r#"{
          "hooks": {
            "PreToolUse": [{ "matcher": "Bash", "hooks": [
              { "type": "prompt",
                "prompt": "Is $ARGUMENTS a safe command?",
                "model": "claude-sonnet-4-6",
                "timeout": 15 }
            ]}]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::Project).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].events, vec![HookEventType::PreToolUse]);
        assert_eq!(hooks[0].source, HookSource::Project);
        assert_eq!(hooks[0].timeout, Some(Duration::from_secs(15)));
        let HookExecutor::Prompt { prompt, model } = &hooks[0].executor else {
            panic!("expected Prompt executor, got {:?}", hooks[0].executor);
        };
        assert_eq!(prompt, "Is $ARGUMENTS a safe command?");
        assert_eq!(model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(hooks[0].name, "prompt");
        assert_eq!(
            hooks[0].if_condition.as_ref().map(|c| c.pattern.as_str()),
            Some("Bash"),
        );
    }

    #[test]
    fn prompt_hook_without_model_defaults_to_none() {
        let raw = r#"{ "hooks": { "PostToolUse": [{ "hooks": [
            { "type": "prompt", "prompt": "evaluate this" }
        ]}]}}"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::User).unwrap();
        assert_eq!(hooks.len(), 1);
        let HookExecutor::Prompt { prompt, model } = &hooks[0].executor else {
            panic!("expected Prompt executor");
        };
        assert_eq!(prompt, "evaluate this");
        assert_eq!(*model, None);
    }

    #[test]
    fn prompt_hook_missing_prompt_is_skipped() {
        let raw = r#"{ "hooks": { "PreToolUse": [{ "hooks": [
            { "type": "prompt", "model": "claude-sonnet-4-6" }
        ]}]}}"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::User).unwrap();
        assert!(hooks.is_empty(), "a prompt entry without a prompt is skipped");
    }

    #[test]
    fn mixed_command_http_agent_prompt_in_one_group_all_parse() {
        // One matcher group carrying a command, an http, an agent, and a prompt
        // hook. All four now parse to their executors.
        let raw = r#"{
          "hooks": {
            "PreToolUse": [{ "matcher": "Bash", "hooks": [
              { "type": "command", "command": "./guard.sh" },
              { "type": "http", "url": "https://h.test/hook" },
              { "type": "agent", "prompt": "vet it" },
              { "type": "prompt", "prompt": "is $ARGUMENTS safe?" }
            ]}]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::Project).unwrap();
        assert_eq!(hooks.len(), 4, "command + http + agent + prompt all parse");
        // Every parsed hook keeps the group's matcher.
        for h in &hooks {
            assert_eq!(
                h.if_condition.as_ref().map(|c| c.pattern.as_str()),
                Some("Bash"),
            );
        }
        assert!(matches!(hooks[0].executor, HookExecutor::Command { .. }));
        assert!(matches!(hooks[1].executor, HookExecutor::Http { .. }));
        assert!(matches!(hooks[2].executor, HookExecutor::Agent { .. }));
        assert!(matches!(hooks[3].executor, HookExecutor::Prompt { .. }));
    }
}
