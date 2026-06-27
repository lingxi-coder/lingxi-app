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
//! [`parse_event_type`] recognizes all 30 of claude-code's `HOOK_EVENTS`
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
//! ### `http` / `agent` field-level mapping
//!
//! The claude-code `http` schema (`schemas/hooks.ts:97-126`) carries `headers`
//! env-var interpolation gated by an `allowedEnvVars` allowlist; both are now
//! parsed onto [`HookExecutor::Http`] (`headers` + `allowed_env_vars`) and the
//! executor interpolates `$VAR`/`${VAR}` references in header VALUES gated on the
//! allowlist (the `cHm` port — names absent from the list resolve to `""`). The
//! `agent` schema (`schemas/hooks.ts:128-163`) carries NO `agent_type`
//! (claude-code's agent hook runs an inline verifier `query()`, not a named
//! subagent); the Rust [`HookExecutor::Agent`] variant requires a non-optional
//! `agent_type`, so it is filled with the crate's canonical default
//! `"general-purpose"` (matching the agent-hook fixtures in
//! `parity_hooks_runtime.rs` and the default in `agent_executor.rs`). The `model`
//! override both schemas allow is now carried: `prompt` onto
//! [`HookExecutor::Prompt::model`] and `agent` onto [`HookExecutor::Agent::model`]
//! (threaded to the spawner). The remaining `agent`-hook residual is the
//! inline-verifier-vs-named-subagent semantic (claude-code runs an in-process
//! `query()`; LingXi spawns a named subagent), which is an architectural
//! difference, not a dropped field.

use crate::definition::{HookCondition, HookDefinition, HookExecutor, HookSource};
use crate::events::HookEventType;
use protocol::HookId;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// Managed-policy gate over which settings-tier hooks are allowed to load.
///
/// Byte-faithful port of claude-code's hook-config resolver `vBr`
/// (`claude.exe` off 195521070):
///
/// ```js
/// function vBr(){
///   let e=Hn("policySettings");
///   if(e?.disableAllHooks===!0)return{};                       // (A) no hooks at all
///   if(e?.allowManagedHooksOnly===!0||Bl())return e?.hooks??{};// (B) managed-only (safe-mode too)
///   if(iS("hooks"))return e?.hooks??{};                        // (C) strictPluginOnly → managed-only
///   let t=ts();                                                // main effective settings
///   if(t.disableAllHooks===!0)return e?.hooks??{};             // (D) settings-tier disable → managed-only
///   return t.hooks??{};                                        // (E) all merged hooks
/// }
/// ```
///
/// where `Bl()` (off 192139854) = `CLAUDE_CODE_SAFE_MODE || --safe-mode`, and
/// `iS("hooks")` (off 195520574) = the `strictPluginOnlyCustomization` policy
/// flag (`true`, or an array containing `"hooks"`).
///
/// `vBr` returns the merged hook *config map*; this crate instead loads each
/// tier's hooks separately ([`parse_hooks_from_settings_json`] per [`HookSource`]), so
/// the equivalent decision is "is THIS source's tier allowed to load at all?".
/// [`Self::allows_source`] reproduces `vBr`'s branches as a per-source predicate:
/// when only managed hooks survive (branches B/C/D) every non-[`HookSource::Managed`]
/// tier is suppressed; when `disableAllHooks` is set in the policy tier
/// (branch A) even the managed tier is suppressed.
///
/// `policySettings.disableAllHooks` / `allowManagedHooksOnly` are read from the
/// managed/policy settings tier; `safe_mode` is the runtime `Bl()` signal;
/// `settings_disable_all_hooks` is the merged-settings (`ts()`) `disableAllHooks`
/// flag (branch D). Sourcing all three from resolved settings lives at the engine
/// composition root (cross-crate); this struct is the pure decision the loader
/// applies. The default-constructed gate (all `false`) allows every tier — the
/// faithful no-managed-policy path.
// Each bool is a distinct binary signal in `vBr` (disableAllHooks /
// allowManagedHooksOnly / Bl() safe-mode / iS("hooks") / ts().disableAllHooks);
// collapsing them into an enum would lose the 1:1 mapping to the source flags.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HookPolicyGate {
    /// `policySettings.disableAllHooks === true` — suppresses EVERY tier,
    /// including managed (branch A: `vBr` returns `{}`).
    pub policy_disable_all_hooks: bool,
    /// `policySettings.allowManagedHooksOnly === true` — only the managed tier
    /// loads (branch B).
    pub allow_managed_hooks_only: bool,
    /// `Bl()` — `CLAUDE_CODE_SAFE_MODE` env or `--safe-mode` flag. Folds into
    /// branch B (managed-only) exactly like `allowManagedHooksOnly`.
    pub safe_mode: bool,
    /// `strictPluginOnlyCustomization` (`iS("hooks")`, branch C) — also collapses
    /// to managed-only.
    pub strict_plugin_only: bool,
    /// `ts().disableAllHooks === true` (the merged main-settings flag, branch D)
    /// — collapses to managed-only (the managed tier is still honored).
    pub settings_disable_all_hooks: bool,
}

/// Policy-settings shape consumed by [`HookPolicyGate::from_policy_settings_json`].
#[derive(Debug, Default, Deserialize)]
struct PolicySettingsTop {
    #[serde(default, rename = "disableAllHooks")]
    disable_all_hooks: Option<bool>,
    #[serde(default, rename = "allowManagedHooksOnly")]
    allow_managed_hooks_only: Option<bool>,
}

impl HookPolicyGate {
    /// Build a gate from the raw policy/managed `settings.json` text plus the
    /// two runtime signals (`safe_mode` = `Bl()`, `settings_disable_all_hooks` =
    /// `ts().disableAllHooks`).
    ///
    /// Mirrors the binary's `e?.` optional-chaining fail-open: a malformed /
    /// missing policy JSON parses to all-`None` (no gating from the policy tier),
    /// exactly as `Hn("policySettings")` yielding `undefined` leaves
    /// `e?.disableAllHooks` and `e?.allowManagedHooksOnly` `undefined` (falsy).
    ///
    /// `strict_plugin_only` (the `iS("hooks")` branch) is sourced separately at
    /// the composition root (it reads a DIFFERENT policy field,
    /// `strictPluginOnlyCustomization`, which can be a bool or a string array);
    /// callers that resolve it should set it via the returned value's field.
    #[must_use]
    pub fn from_policy_settings_json(
        policy_json: Option<&str>,
        safe_mode: bool,
        settings_disable_all_hooks: bool,
    ) -> Self {
        let policy: PolicySettingsTop = policy_json
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        Self {
            // `=== !0` ⇒ strict `Some(true)`.
            policy_disable_all_hooks: policy.disable_all_hooks == Some(true),
            allow_managed_hooks_only: policy.allow_managed_hooks_only == Some(true),
            safe_mode,
            strict_plugin_only: false,
            settings_disable_all_hooks,
        }
    }

    /// `true` when only the managed/policy tier's hooks are allowed (branches
    /// B/C/D of `vBr`): `allowManagedHooksOnly`, safe-mode, strict-plugin-only,
    /// or the merged-settings `disableAllHooks`. (Independent of branch A, which
    /// suppresses even the managed tier — see [`Self::allows_source`].)
    #[must_use]
    pub fn managed_only(&self) -> bool {
        self.allow_managed_hooks_only
            || self.safe_mode
            || self.strict_plugin_only
            || self.settings_disable_all_hooks
    }

    /// Whether hooks from `source` are allowed to load under this gate.
    ///
    /// - Branch A (`policy_disable_all_hooks`): no tier loads — returns `false`
    ///   for every source, including [`HookSource::Managed`].
    /// - Branches B/C/D ([`Self::managed_only`]): only [`HookSource::Managed`]
    ///   loads.
    /// - Otherwise (branch E): every tier loads.
    #[must_use]
    pub fn allows_source(&self, source: HookSource) -> bool {
        if self.policy_disable_all_hooks {
            return false;
        }
        if self.managed_only() {
            return source == HookSource::Managed;
        }
        true
    }
}

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
    /// `http` hook request headers (`schemas/hooks.ts:106-111`). Values may
    /// reference env vars via `$VAR`/`${VAR}`, interpolated by the executor
    /// gated on [`Self::allowed_env_vars`] (claude-code `cHm`).
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    /// `http` hook `allowedEnvVars` (`schemas/hooks.ts`): env-var names header
    /// values may interpolate. Names absent here resolve to `""`. Required for
    /// any interpolation to occur.
    #[serde(default, rename = "allowedEnvVars")]
    allowed_env_vars: Option<Vec<String>>,
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
    /// claude-code `async` (`schemas/hooks.ts`: `async:boolean().optional()
    /// .describe("If true, hook runs in background without blocking")`). Maps to
    /// [`HookDefinition::blocking`] = `!async` — `blocking == false` routes the
    /// hook to the background async registry. The related `asyncRewake` /
    /// `asyncTimeout` (15000) / `rewakeMessage` refinements need new
    /// [`HookDefinition`] fields and are deferred (struct-field ripple).
    #[serde(default, rename = "async")]
    r#async: Option<bool>,
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
    parse_into(raw, source)
}

/// [`parse_hooks_from_settings_json`] gated by a [`HookPolicyGate`].
///
/// When the gate suppresses `source` ([`HookPolicyGate::allows_source`] is
/// `false` — e.g. a `disableAllHooks` / `allowManagedHooksOnly` managed policy,
/// or safe-mode), this returns `Ok(vec![])` WITHOUT parsing `raw`, mirroring the
/// binary's `vBr` returning `{}` / only the managed tier's hooks for the
/// corresponding tiers. When the gate allows the source it is byte-identical to
/// [`parse_hooks_from_settings_json`].
///
/// Composition roots that have resolved managed/policy settings + the safe-mode
/// signal should call this per source instead of the ungated variant so the
/// policy gate is honored at load time. The default-constructed gate
/// ([`HookPolicyGate::default`]) allows every source, so wiring it in is
/// behavior-neutral until a real policy is supplied.
pub fn parse_hooks_from_settings_json_gated(
    raw: &str,
    source: HookSource,
    gate: HookPolicyGate,
) -> Result<Vec<HookDefinition>, serde_json::Error> {
    if !gate.allows_source(source) {
        return Ok(Vec::new());
    }
    parse_into(raw, source)
}

/// Shared projection used by both the ungated and gated entry points.
fn parse_into(
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
                    // claude routes an `async: true` settings hook to the
                    // background (non-blocking) path; `blocking == false` is that
                    // path here. Defaults to blocking when the field is absent.
                    blocking: !entry.r#async.unwrap_or(false),
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
                allowed_env_vars: entry.allowed_env_vars.clone().unwrap_or_default(),
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
                // claude-code's agent hook carries an optional `model` override
                // for the inline verifier; thread it to the spawner.
                model: entry.model.clone(),
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
/// (`entrypoints/sdk/coreTypes.ts:25-53`). Every one of those 30 event names
/// has a corresponding `HookEvent`/`HookEventType` variant in
/// [`crate::events`], so all 30 are recognized here. Unrecognized names
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
        // #39: three more user-configurable events (claude-code registry `nym`,
        // BIN off 205713189). Without these arms a settings.json hook keyed on
        // them is silently dropped at load.
        "PostToolBatch" => Some(HookEventType::PostToolBatch),
        "UserPromptExpansion" => Some(HookEventType::UserPromptExpansion),
        "MessageDisplay" => Some(HookEventType::MessageDisplay),
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
    fn all_30_event_names_parse_to_their_variant() {
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
            ("PostToolBatch", HookEventType::PostToolBatch),
            ("UserPromptExpansion", HookEventType::UserPromptExpansion),
            ("MessageDisplay", HookEventType::MessageDisplay),
        ];
        assert_eq!(cases.len(), 30, "claude-code HOOK_EVENTS has 30 names");
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
            allowed_env_vars: _,
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
    fn http_hook_parses_allowed_env_vars() {
        let raw = r#"{
          "hooks": {
            "PreToolUse": [{ "hooks": [
              { "type": "http",
                "url": "https://h.test/post",
                "headers": { "Authorization": "Bearer $MY_TOKEN" },
                "allowedEnvVars": ["MY_TOKEN", "OTHER"] }
            ]}]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::Project).unwrap();
        let HookExecutor::Http {
            allowed_env_vars, ..
        } = &hooks[0].executor
        else {
            panic!("expected Http executor");
        };
        assert_eq!(allowed_env_vars, &vec!["MY_TOKEN".to_string(), "OTHER".to_string()]);
    }

    #[test]
    fn agent_hook_parses_model_override() {
        let raw = r#"{
          "hooks": {
            "PreToolUse": [{ "hooks": [
              { "type": "agent", "prompt": "vet", "model": "claude-haiku-4-5" }
            ]}]
          }
        }"#;
        let hooks = parse_hooks_from_settings_json(raw, HookSource::Project).unwrap();
        let HookExecutor::Agent { model, .. } = &hooks[0].executor else {
            panic!("expected Agent executor");
        };
        assert_eq!(model.as_deref(), Some("claude-haiku-4-5"));
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
        let HookExecutor::Agent {
            agent_type,
            prompt,
            model: _,
        } = &hooks[0].executor
        else {
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

    // ---- #41: managed-policy hook gate (vBr / h$) --------------------------

    /// Minimal `{ "hooks": { "Stop": [{ "hooks": [command] }] } }` so a gated
    /// load that allows the source produces exactly one hook.
    fn one_stop_command() -> String {
        one_command("Stop")
    }

    #[test]
    fn policy_gate_default_allows_every_source() {
        // The default-constructed gate (no managed policy, not safe-mode) is the
        // faithful no-policy path: every tier loads.
        let gate = HookPolicyGate::default();
        for src in [
            HookSource::User,
            HookSource::Project,
            HookSource::Local,
            HookSource::Managed,
            HookSource::Plugin,
            HookSource::FrontMatter,
        ] {
            assert!(gate.allows_source(src), "default gate must allow {src:?}");
        }
        assert!(!gate.managed_only());
    }

    #[test]
    fn policy_disable_all_hooks_blocks_every_tier_including_managed() {
        // vBr branch A: `policySettings.disableAllHooks===true` → `{}` (no hooks
        // at all). Even the managed tier is suppressed.
        let policy = r#"{ "disableAllHooks": true }"#;
        let gate = HookPolicyGate::from_policy_settings_json(Some(policy), false, false);
        assert!(gate.policy_disable_all_hooks);
        for src in [HookSource::User, HookSource::Project, HookSource::Managed] {
            assert!(!gate.allows_source(src), "disableAllHooks must block {src:?}");
        }
    }

    #[test]
    fn allow_managed_hooks_only_keeps_only_managed_tier() {
        // vBr branch B: `policySettings.allowManagedHooksOnly===true` → only the
        // managed tier's hooks survive.
        let policy = r#"{ "allowManagedHooksOnly": true }"#;
        let gate = HookPolicyGate::from_policy_settings_json(Some(policy), false, false);
        assert!(gate.managed_only());
        assert!(gate.allows_source(HookSource::Managed));
        for src in [HookSource::User, HookSource::Project, HookSource::Plugin] {
            assert!(!gate.allows_source(src), "managed-only must block {src:?}");
        }
    }

    #[test]
    fn safe_mode_collapses_to_managed_only() {
        // vBr branch B fold: `Bl()` (CLAUDE_CODE_SAFE_MODE / --safe-mode) is OR'd
        // with allowManagedHooksOnly → only the managed tier loads.
        let gate = HookPolicyGate::from_policy_settings_json(None, true, false);
        assert!(gate.safe_mode);
        assert!(gate.managed_only());
        assert!(gate.allows_source(HookSource::Managed));
        assert!(!gate.allows_source(HookSource::User));
    }

    #[test]
    fn settings_tier_disable_all_hooks_collapses_to_managed_only() {
        // vBr branch D: `ts().disableAllHooks===true` keeps the managed tier
        // (returns `e?.hooks`), unlike branch A (policy disableAllHooks) which
        // drops it. So the managed tier still loads.
        let gate = HookPolicyGate::from_policy_settings_json(None, false, true);
        assert!(gate.settings_disable_all_hooks);
        assert!(gate.managed_only());
        assert!(gate.allows_source(HookSource::Managed));
        assert!(!gate.allows_source(HookSource::Project));
    }

    #[test]
    fn strict_plugin_only_collapses_to_managed_only() {
        // vBr branch C: `iS("hooks")` (strictPluginOnlyCustomization) → managed
        // tier only. Sourced at the composition root and set on the gate field.
        let gate = HookPolicyGate {
            strict_plugin_only: true,
            ..HookPolicyGate::default()
        };
        assert!(gate.managed_only());
        assert!(gate.allows_source(HookSource::Managed));
        assert!(!gate.allows_source(HookSource::User));
    }

    #[test]
    fn policy_disable_all_hooks_takes_precedence_over_managed_only() {
        // Branch A must win over B: when BOTH disableAllHooks and
        // allowManagedHooksOnly are set, even the managed tier is dropped.
        let policy = r#"{ "disableAllHooks": true, "allowManagedHooksOnly": true }"#;
        let gate = HookPolicyGate::from_policy_settings_json(Some(policy), false, false);
        assert!(!gate.allows_source(HookSource::Managed));
        assert!(!gate.allows_source(HookSource::User));
    }

    #[test]
    fn malformed_policy_json_fails_open() {
        // The binary's `e?.` optional chaining treats missing/undefined policy
        // as no gating. A malformed JSON parses to the all-`None` default ⇒ no
        // gating beyond the runtime signals (here both false ⇒ everything loads).
        let gate = HookPolicyGate::from_policy_settings_json(Some("not json {"), false, false);
        assert!(!gate.policy_disable_all_hooks);
        assert!(!gate.allow_managed_hooks_only);
        assert!(gate.allows_source(HookSource::User));
    }

    #[test]
    fn falsy_policy_flag_does_not_gate() {
        // `=== !0` is strict `true`: `disableAllHooks: false` must NOT gate.
        let policy = r#"{ "disableAllHooks": false, "allowManagedHooksOnly": false }"#;
        let gate = HookPolicyGate::from_policy_settings_json(Some(policy), false, false);
        assert!(!gate.policy_disable_all_hooks);
        assert!(!gate.managed_only());
        assert!(gate.allows_source(HookSource::User));
    }

    #[test]
    fn gated_load_suppresses_without_parsing_when_blocked() {
        // A blocked source returns an empty vec WITHOUT parsing — a managed
        // disableAllHooks suppresses the user tier.
        let gate = HookPolicyGate::from_policy_settings_json(
            Some(r#"{ "disableAllHooks": true }"#),
            false,
            false,
        );
        let raw = one_stop_command();
        let hooks =
            parse_hooks_from_settings_json_gated(&raw, HookSource::User, gate).unwrap();
        assert!(hooks.is_empty(), "disableAllHooks must suppress the user tier");
    }

    #[test]
    fn gated_load_managed_only_keeps_managed_drops_user() {
        let gate = HookPolicyGate::from_policy_settings_json(
            Some(r#"{ "allowManagedHooksOnly": true }"#),
            false,
            false,
        );
        let raw = one_stop_command();
        // Managed tier loads.
        let managed =
            parse_hooks_from_settings_json_gated(&raw, HookSource::Managed, gate).unwrap();
        assert_eq!(managed.len(), 1);
        assert_eq!(managed[0].source, HookSource::Managed);
        // User tier is suppressed.
        let user =
            parse_hooks_from_settings_json_gated(&raw, HookSource::User, gate).unwrap();
        assert!(user.is_empty());
    }

    #[test]
    fn gated_load_under_default_gate_equals_ungated() {
        // With the default (no-policy) gate, the gated entry point is
        // byte-identical to the ungated one.
        let raw = one_stop_command();
        let gated = parse_hooks_from_settings_json_gated(
            &raw,
            HookSource::User,
            HookPolicyGate::default(),
        )
        .unwrap();
        let ungated = parse_hooks_from_settings_json(&raw, HookSource::User).unwrap();
        assert_eq!(gated.len(), ungated.len());
        assert_eq!(gated.len(), 1);
        assert_eq!(gated[0].name, ungated[0].name);
        assert_eq!(gated[0].source, ungated[0].source);
    }
}
