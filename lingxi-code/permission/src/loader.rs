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

/// The `permissions` block. `allow`/`deny`/`ask` are arrays of rule strings.
#[derive(Debug, Default, Deserialize)]
struct PermissionsBlock {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
    #[serde(default)]
    ask: Vec<String>,
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
    fn missing_arrays_default_empty() {
        // Only `allow` present; `deny`/`ask` default to [].
        let raw = r#"{ "permissions": { "allow": ["Bash"] } }"#;
        let rules =
            permission_rules_from_settings_json(raw, PermissionRuleSource::CliArg).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].value.tool_name, "Bash");
    }
}
