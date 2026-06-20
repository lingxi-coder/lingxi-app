//! Permission loader — projects a settings file's `permissions` block into a
//! `Vec<PermissionRule>` tagged with its source.
//!
//! Mirrors the hooks loader (`lingxi-hooks::parse_hooks_from_settings_json`): a
//! best-effort projection from raw settings JSON, NOT a strict validator. 1:1
//! with claude-code `settingsJsonToRules` (`utils/permissions/permissionsLoader.ts`):
//! the `permissions.{allow,deny,ask}` string arrays each become rules with the
//! corresponding [`PermissionBehavior`], all tagged with the caller-supplied
//! [`PermissionRuleSource`]. Each spec string is parsed via
//! [`PermissionRuleValue::from_rule_string`].
//!
//! ## Scope (parity phase 1 — foundation, no enforcement)
//! This is the pure load step. It does NOT wire a gate or read files at boot —
//! the enforcing gate ([`crate::PermissionPolicy`] + a `PermissionGate` impl)
//! and the boot-time multi-tier disk read are later phases. `defaultMode` and
//! `additionalDirectories` from the block are not consumed here (mode resolution
//! is a gate/boot concern).
//!
//! Settings shape (claude-code compatible — `settings.json`):
//! ```json
//! { "permissions": { "allow": ["Bash(npm run *)"], "deny": ["Read(./secrets/**)"], "ask": [] } }
//! ```

use crate::mode::PermissionMode;
use crate::rule::{PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionRuleValue};
use serde::Deserialize;

/// Top-level projection consumed by [`permission_rules_from_settings_json`].
/// A separate private struct (like the hooks loader) so this loader stays
/// decoupled from the engine's typed `SettingsJson`.
#[derive(Debug, Deserialize)]
struct SettingsTop {
    #[serde(default)]
    permissions: Option<PermissionsBlock>,
}

/// The `permissions` block. `allow`/`deny`/`ask` are arrays of rule strings;
/// `defaultMode` selects the mode-fallback for unmatched calls.
#[derive(Debug, Default, Deserialize)]
struct PermissionsBlock {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
    #[serde(default)]
    ask: Vec<String>,
    #[serde(default, rename = "defaultMode")]
    default_mode: Option<String>,
    #[serde(default, rename = "disableBypassPermissionsMode")]
    disable_bypass_permissions_mode: Option<String>,
}

/// Parse one settings file's raw JSON into permission rules tagged with
/// `source`. Returns `Ok(vec![])` when there is no `permissions` block.
///
/// # Errors
/// Returns the `serde_json::Error` if `raw` is not valid JSON. (A file that is
/// valid JSON but has no `permissions` block is not an error — it yields an
/// empty vec, matching the best-effort hooks-loader contract.)
pub fn permission_rules_from_settings_json(
    raw: &str,
    source: PermissionRuleSource,
) -> Result<Vec<PermissionRule>, serde_json::Error> {
    let top: SettingsTop = serde_json::from_str(raw)?;
    let Some(block) = top.permissions else {
        return Ok(Vec::new());
    };
    // Deny/allow/ask order is cosmetic here (the buckets are evaluated by
    // `PermissionPolicy::authorize`, deny-first); we just project every spec.
    let mut out = Vec::with_capacity(block.allow.len() + block.deny.len() + block.ask.len());
    for (behavior, specs) in [
        (PermissionBehavior::Deny, &block.deny),
        (PermissionBehavior::Allow, &block.allow),
        (PermissionBehavior::Ask, &block.ask),
    ] {
        for spec in specs {
            out.push(PermissionRule {
                value: PermissionRuleValue::from_rule_string(spec),
                behavior,
                source,
            });
        }
    }
    Ok(out)
}

/// Parse `permissions.defaultMode` into a [`PermissionMode`] (claude-code wire
/// names: `default` / `plan` / `acceptEdits` / `bypassPermissions` / `dontAsk`).
/// Returns `None` when the block, the field, or the value is absent/unrecognized
/// (the caller falls back to [`PermissionMode::Default`]).
#[must_use]
pub fn default_mode_from_settings_json(raw: &str) -> Option<PermissionMode> {
    let top: SettingsTop = serde_json::from_str(raw).ok()?;
    match top.permissions?.default_mode?.as_str() {
        "default" => Some(PermissionMode::Default),
        "plan" => Some(PermissionMode::Plan),
        "acceptEdits" => Some(PermissionMode::AcceptEdits),
        "bypassPermissions" => Some(PermissionMode::BypassPermissions),
        "dontAsk" => Some(PermissionMode::DontAsk),
        // #32: claude-code's settings `defaultMode` enum includes "auto"
        // (`E.enum(["default","acceptEdits","bypassPermissions","plan","dontAsk",
        // "auto"])`). Accepted at parse; the actual runtime ENTRY into auto-mode
        // is further gated (model gate + circuit-breaker + disableAutoMode) — a
        // separate concern, out of scope here.
        "auto" => Some(PermissionMode::Auto),
        _ => None,
    }
}

/// Does this settings file DISABLE `bypassPermissions` mode? True iff
/// `permissions.disableBypassPermissionsMode == "disable"` (claude-code's
/// bypass-permissions killswitch). When any tier disables it, the constructed
/// [`crate::PermissionPolicy`]'s `bypass_killswitch_active` is set so
/// `authorize` falls back to `Ask` even in `BypassPermissions` mode.
#[must_use]
pub fn bypass_permissions_disabled_from_settings_json(raw: &str) -> bool {
    serde_json::from_str::<SettingsTop>(raw)
        .ok()
        .and_then(|t| t.permissions)
        .and_then(|p| p.disable_bypass_permissions_mode)
        .as_deref()
        == Some("disable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_permissions_block_is_empty() {
        assert!(permission_rules_from_settings_json("{}", PermissionRuleSource::UserSettings)
            .unwrap()
            .is_empty());
        // Other settings keys present, but no permissions → still empty.
        let raw = r#"{ "model": "claude-opus-4-7" }"#;
        assert!(
            permission_rules_from_settings_json(raw, PermissionRuleSource::ProjectSettings)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn invalid_json_is_err() {
        assert!(permission_rules_from_settings_json("{not json", PermissionRuleSource::UserSettings)
            .is_err());
    }

    #[test]
    fn projects_allow_deny_ask_with_source_and_behavior() {
        let raw = r#"{
            "permissions": {
                "allow": ["Bash(npm run *)", "Read"],
                "deny": ["Read(./secrets/**)"],
                "ask": ["WebFetch"]
            }
        }"#;
        let rules =
            permission_rules_from_settings_json(raw, PermissionRuleSource::ProjectSettings).unwrap();
        assert_eq!(rules.len(), 4);
        // Every rule carries the caller's source.
        assert!(rules
            .iter()
            .all(|r| r.source == PermissionRuleSource::ProjectSettings));

        let find = |tool: &str, content: Option<&str>| {
            rules.iter().find(|r| {
                r.value.tool_name == tool && r.value.rule_content.as_deref() == content
            })
        };
        // allow → parsed rule_content.
        let bash = find("Bash", Some("npm run *")).expect("Bash(npm run *)");
        assert!(matches!(bash.behavior, PermissionBehavior::Allow));
        // bare allow.
        assert!(matches!(
            find("Read", None).expect("Read").behavior,
            PermissionBehavior::Allow
        ));
        // deny.
        assert!(matches!(
            find("Read", Some("./secrets/**")).expect("deny Read").behavior,
            PermissionBehavior::Deny
        ));
        // ask.
        assert!(matches!(
            find("WebFetch", None).expect("ask WebFetch").behavior,
            PermissionBehavior::Ask
        ));
    }

    #[test]
    fn empty_arrays_yield_no_rules() {
        let raw = r#"{ "permissions": { "allow": [], "deny": [], "ask": [] } }"#;
        assert!(
            permission_rules_from_settings_json(raw, PermissionRuleSource::UserSettings)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn bypass_killswitch_parses() {
        let f = bypass_permissions_disabled_from_settings_json;
        assert!(f(r#"{ "permissions": { "disableBypassPermissionsMode": "disable" } }"#));
        assert!(!f(r#"{ "permissions": { "disableBypassPermissionsMode": "enable" } }"#));
        assert!(!f(r#"{ "permissions": {} }"#));
        assert!(!f("{}"));
        assert!(!f("not json"));
    }

    #[test]
    fn default_mode_parses_wire_names() {
        let m = |raw: &str| default_mode_from_settings_json(raw);
        assert!(matches!(
            m(r#"{ "permissions": { "defaultMode": "dontAsk" } }"#),
            Some(PermissionMode::DontAsk)
        ));
        assert!(matches!(
            m(r#"{ "permissions": { "defaultMode": "acceptEdits" } }"#),
            Some(PermissionMode::AcceptEdits)
        ));
        assert!(matches!(
            m(r#"{ "permissions": { "defaultMode": "bypassPermissions" } }"#),
            Some(PermissionMode::BypassPermissions)
        ));
        // #32: "auto" is an accepted settings defaultMode value (was dropped→None).
        assert!(matches!(
            m(r#"{ "permissions": { "defaultMode": "auto" } }"#),
            Some(PermissionMode::Auto)
        ));
        // Absent / no block / unknown → None (caller defaults to Default).
        assert!(m(r#"{ "permissions": {} }"#).is_none());
        assert!(m("{}").is_none());
        assert!(m(r#"{ "permissions": { "defaultMode": "bogus" } }"#).is_none());
    }

    #[test]
    fn missing_arrays_default_empty() {
        // Only `allow` present; `deny`/`ask` default to [].
        let raw = r#"{ "permissions": { "allow": ["Bash"] } }"#;
        let rules =
            permission_rules_from_settings_json(raw, PermissionRuleSource::CliArg).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].value.tool_name, "Bash");
    }
}
