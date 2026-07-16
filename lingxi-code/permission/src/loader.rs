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
    /// TOP-LEVEL managed-settings lockdown flag (a sibling of `permissions`,
    /// NOT inside it — claude-code schema: `allowManagedPermissionRulesOnly:
    /// E.boolean().optional()`). Only meaningful when set in the managed
    /// (policySettings) tier; consumed via
    /// [`allow_managed_permission_rules_only_from_settings_json`].
    #[serde(default, rename = "allowManagedPermissionRulesOnly")]
    allow_managed_permission_rules_only: Option<bool>,
    /// TOP-LEVEL `disableAutoMode` killswitch. claude-code's schema declares
    /// `disableAutoMode: E.enum(["disable"]).optional()` at BOTH the top level
    /// AND inside `permissions` (`Bpa()` checks both positions); this is the
    /// top-level sibling. Consumed via [`auto_mode_disabled_from_settings_json`].
    #[serde(default, rename = "disableAutoMode")]
    disable_auto_mode: Option<String>,
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
    /// `permissions.disableAutoMode` — the auto-mode killswitch at its
    /// permissions-block position (`Bpa()`'s `e.permissions?.disableAutoMode`).
    #[serde(default, rename = "disableAutoMode")]
    disable_auto_mode: Option<String>,
    /// Extra directories (beyond cwd) inside which `acceptEdits`/auto-allow
    /// treats edits as writable. 1:1 with claude-code `permissions.additionalDirectories`,
    /// which `TGd` folds into `ToolPermissionContext.additionalWorkingDirectories`
    /// and `b$` unions with cwd for the `kF` working-dir auto-allow check.
    #[serde(default, rename = "additionalDirectories")]
    additional_directories: Vec<String>,
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
///
/// `"manual"` is accepted as an alias for `"default"` (parity 2.1.207): the
/// binary's zod schema declares `defaultMode:
/// E.preprocess(ZS, E.enum([...]))` where `ZS(e)=e==="manual"?"default":e`
/// normalizes the value BEFORE the enum validation, and the field's `.describe`
/// text reads "'manual' is accepted as an alias for 'default'". Without this
/// arm a tier whose `defaultMode` is `"manual"` returns `None` and is skipped
/// by the multi-tier reader, letting a lower-priority tier win.
#[must_use]
pub fn default_mode_from_settings_json(raw: &str) -> Option<PermissionMode> {
    let top: SettingsTop = serde_json::from_str(raw).ok()?;
    match top.permissions?.default_mode?.as_str() {
        "default" | "manual" => Some(PermissionMode::Default),
        "plan" => Some(PermissionMode::Plan),
        "acceptEdits" => Some(PermissionMode::AcceptEdits),
        "bypassPermissions" => Some(PermissionMode::BypassPermissions),
        "dontAsk" => Some(PermissionMode::DontAsk),
        // #32: claude-code's settings `defaultMode` enum includes "auto"
        // (`E.enum(["default","acceptEdits","bypassPermissions","plan","dontAsk",
        // "auto"])`). Accepted at parse; the actual runtime ENTRY into auto-mode
        // is further gated (model gate + circuit-breaker + disableAutoMode) by
        // [`crate::auto_gate`] — the boot mode-load path applies
        // [`crate::auto_gate::apply_auto_mode_gate`], which downgrades `auto` to
        // `default` when the gate is closed (claude-code `xms`).
        "auto" => Some(PermissionMode::Auto),
        _ => None,
    }
}

/// `MODE-SETTINGS-AUTO-TRUST-01`: may a settings tier of this `source` GRANT
/// `defaultMode: "auto"`?
///
/// claude-code 2.1.211's `initialPermissionModeFromCLI` only honors a settings
/// `defaultMode` of `"auto"` when it was declared by a TRUSTED tier —
/// `policySettings`, `userSettings`, or `flagSettings`
/// (`!["policySettings","userSettings","flagSettings"].some(t =>
/// getSettings(t)?.permissions?.defaultMode==="auto")` → ignore). A
/// `defaultMode: "auto"` coming from `projectSettings` or `localSettings` is
/// IGNORED (warn + `tengu_settings_auto_mode_untrusted_source_ignored`) because
/// those files are repo-controllable: a committed `.lingxi/settings.json` in an
/// untrusted repo must NOT be able to put the session into classifier-driven
/// auto-accept mode.
///
/// Returns `true` for the trusted tiers (User/Policy/Flag) and `false` for the
/// repo-controllable ones (Project/Local) and the runtime tiers
/// (CliArg/Command/Session), which never carry a settings `defaultMode`. This is
/// citation-neutral: the five external modes may still be set from ANY tier;
/// only `auto` is trust-gated.
#[must_use]
pub fn auto_mode_grantable_by_source(source: PermissionRuleSource) -> bool {
    matches!(
        source,
        PermissionRuleSource::UserSettings
            | PermissionRuleSource::PolicySettings
            | PermissionRuleSource::FlagSettings
    )
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

/// Does this settings file DISABLE auto mode? True iff `disableAutoMode ==
/// "disable"` at EITHER position — top-level `disableAutoMode` OR
/// `permissions.disableAutoMode` — 1:1 with claude-code's `Bpa()`
/// (`e.disableAutoMode==="disable"||e.permissions?.disableAutoMode==="disable"`).
///
/// The `disableAutoMode` schema is `E.enum(["disable"]).optional()` at both
/// positions, so `"disable"` is the only meaningful value. When any settings
/// tier disables it, [`crate::PermissionPolicy::auto_mode_disabled`] is set so
/// both the boot mode-load gate ([`crate::auto_gate::apply_auto_mode_gate`]) and
/// the live `set_permission_mode` gate refuse `auto`.
#[must_use]
pub fn auto_mode_disabled_from_settings_json(raw: &str) -> bool {
    let Ok(top) = serde_json::from_str::<SettingsTop>(raw) else {
        return false;
    };
    if top.disable_auto_mode.as_deref() == Some("disable") {
        return true;
    }
    top.permissions.and_then(|p| p.disable_auto_mode).as_deref() == Some("disable")
}

/// Parse `permissions.additionalDirectories` (a string array of extra working
/// directories) from one settings file's raw JSON. These are the dirs beyond
/// `cwd` inside which `acceptEdits` mode auto-allows safe file edits.
///
/// 1:1 with claude-code: `TGd(s, settings.permissions?.additionalDirectories, …)`
/// folds each entry into `ToolPermissionContext.additionalWorkingDirectories`,
/// which `b$(e)=new Set([cwd(),...e.additionalWorkingDirectories.keys()])` unions
/// with the cwd to form the `kF` working-dir auto-allow set.
///
/// Returns `Vec::new()` when the block, the field, or the JSON is absent/invalid
/// (best-effort projection, like the other loaders here). The returned paths are
/// the RAW settings strings as [`PathBuf`]s (relative / `~`-prefixed / absolute) —
/// they are resolved against the policy's filesystem roots at authorize time by
/// `expand_path`, so the caller need not pre-resolve them.
#[must_use]
pub fn additional_directories_from_settings_json(raw: &str) -> Vec<std::path::PathBuf> {
    serde_json::from_str::<SettingsTop>(raw)
        .ok()
        .and_then(|t| t.permissions)
        .map(|p| {
            p.additional_directories
                .into_iter()
                .map(std::path::PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Does this settings file set the managed-only permission-rule lockdown?
/// True iff the TOP-LEVEL `allowManagedPermissionRulesOnly` is exactly `true`.
///
/// 1:1 with claude-code `$wt()` (`wr("policySettings")?.
/// allowManagedPermissionRulesOnly===!0`): when ANY managed tier sets it true
/// (`u.some((g)=>g.allowManagedPermissionRulesOnly===!0)` in the managed-tier
/// fold), `RKt()` returns ONLY the policySettings rules — schema text: "When
/// true (and set in managed settings), only permission rules (allow/deny/ask)
/// from managed settings are respected. User, project, local, and CLI argument
/// permission rules are ignored." The caller applies the retain; this helper is
/// the pure per-file parse (best-effort like the other loaders here).
#[must_use]
pub fn allow_managed_permission_rules_only_from_settings_json(raw: &str) -> bool {
    serde_json::from_str::<SettingsTop>(raw)
        .ok()
        .and_then(|t| t.allow_managed_permission_rules_only)
        == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_mode_grantable_only_from_trusted_tiers() {
        // MODE-SETTINGS-AUTO-TRUST-01: policy/user/flag may grant auto.
        assert!(auto_mode_grantable_by_source(PermissionRuleSource::UserSettings));
        assert!(auto_mode_grantable_by_source(PermissionRuleSource::PolicySettings));
        assert!(auto_mode_grantable_by_source(PermissionRuleSource::FlagSettings));
        // Repo-controllable tiers may NOT.
        assert!(!auto_mode_grantable_by_source(
            PermissionRuleSource::ProjectSettings
        ));
        assert!(!auto_mode_grantable_by_source(
            PermissionRuleSource::LocalSettings
        ));
        // Runtime tiers never carry a settings defaultMode.
        assert!(!auto_mode_grantable_by_source(PermissionRuleSource::CliArg));
        assert!(!auto_mode_grantable_by_source(PermissionRuleSource::Command));
        assert!(!auto_mode_grantable_by_source(PermissionRuleSource::Session));
    }

    #[test]
    fn no_permissions_block_is_empty() {
        assert!(
            permission_rules_from_settings_json("{}", PermissionRuleSource::UserSettings)
                .unwrap()
                .is_empty()
        );
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
        assert!(permission_rules_from_settings_json(
            "{not json",
            PermissionRuleSource::UserSettings
        )
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
        let rules = permission_rules_from_settings_json(raw, PermissionRuleSource::ProjectSettings)
            .unwrap();
        assert_eq!(rules.len(), 4);
        // Every rule carries the caller's source.
        assert!(rules
            .iter()
            .all(|r| r.source == PermissionRuleSource::ProjectSettings));

        let find = |tool: &str, content: Option<&str>| {
            rules
                .iter()
                .find(|r| r.value.tool_name == tool && r.value.rule_content.as_deref() == content)
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
            find("Read", Some("./secrets/**"))
                .expect("deny Read")
                .behavior,
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
        assert!(f(
            r#"{ "permissions": { "disableBypassPermissionsMode": "disable" } }"#
        ));
        assert!(!f(
            r#"{ "permissions": { "disableBypassPermissionsMode": "enable" } }"#
        ));
        assert!(!f(r#"{ "permissions": {} }"#));
        assert!(!f("{}"));
        assert!(!f("not json"));
    }

    #[test]
    fn auto_mode_killswitch_parses_both_positions() {
        let f = auto_mode_disabled_from_settings_json;
        // TOP-LEVEL position.
        assert!(f(r#"{ "disableAutoMode": "disable" }"#));
        // permissions-block position.
        assert!(f(r#"{ "permissions": { "disableAutoMode": "disable" } }"#));
        // Both set → still true.
        assert!(f(
            r#"{ "disableAutoMode": "disable", "permissions": { "disableAutoMode": "disable" } }"#
        ));
        // Coexists with other keys.
        assert!(f(
            r#"{ "permissions": { "disableAutoMode": "disable", "allow": ["Read"] } }"#
        ));
        // Only "disable" counts (enum(["disable"])); anything else / absent → false.
        assert!(!f(r#"{ "disableAutoMode": "enable" }"#));
        assert!(!f(r#"{ "permissions": { "disableAutoMode": "" } }"#));
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
        // parity 2.1.207: "manual" is an alias for "default" (ZS preprocess).
        // Must map to Default (NOT None) so the tier is not skipped.
        assert!(matches!(
            m(r#"{ "permissions": { "defaultMode": "manual" } }"#),
            Some(PermissionMode::Default)
        ));
        // Absent / no block / unknown → None (caller defaults to Default).
        assert!(m(r#"{ "permissions": {} }"#).is_none());
        assert!(m("{}").is_none());
        assert!(m(r#"{ "permissions": { "defaultMode": "bogus" } }"#).is_none());
    }

    /// Regression for the parity-2.1.207 `manual`-alias tier bug: the CLI's
    /// multi-tier reader (`read_cli_mode_settings`) applies `if let Some(m) =
    /// default_mode_from_settings_json(..)` per tier, project last (highest
    /// priority). Before the fix a project `defaultMode:"manual"` returned
    /// `None`, so it was SKIPPED and a lower-priority user tier (`plan`) won.
    /// After the fix `manual` → `Some(Default)`, so the project tier wins.
    #[test]
    fn manual_project_tier_beats_lower_priority_user_tier() {
        let user = r#"{ "permissions": { "defaultMode": "plan" } }"#;
        let project = r#"{ "permissions": { "defaultMode": "manual" } }"#;
        // Mirror the reader loop: user first, then project (project last wins).
        let mut default_mode: Option<PermissionMode> = None;
        for raw in [user, project] {
            if let Some(m) = default_mode_from_settings_json(raw) {
                default_mode = Some(m);
            }
        }
        assert_eq!(
            default_mode,
            Some(PermissionMode::Default),
            "project defaultMode:manual must beat user defaultMode:plan"
        );
    }

    #[test]
    fn additional_directories_parse() {
        use std::path::PathBuf;
        let f = additional_directories_from_settings_json;
        // Present → returned as raw PathBufs (relative / ~ / absolute preserved).
        assert_eq!(
            f(
                r#"{ "permissions": { "additionalDirectories": ["../sibling", "~/work", "/abs/dir"] } }"#
            ),
            vec![
                PathBuf::from("../sibling"),
                PathBuf::from("~/work"),
                PathBuf::from("/abs/dir"),
            ]
        );
        // Absent field / no block / no JSON → empty (best-effort).
        assert!(f(r#"{ "permissions": { "allow": ["Bash"] } }"#).is_empty());
        assert!(f(r#"{ "permissions": {} }"#).is_empty());
        assert!(f("{}").is_empty());
        assert!(f("not json").is_empty());
        // Coexists with other permissions keys.
        assert_eq!(
            f(r#"{ "permissions": { "allow": ["Read"], "additionalDirectories": ["a"] } }"#),
            vec![PathBuf::from("a")]
        );
    }

    #[test]
    fn allow_managed_permission_rules_only_parses_top_level() {
        let f = allow_managed_permission_rules_only_from_settings_json;
        // TOP-LEVEL true → lockdown.
        assert!(f(r#"{ "allowManagedPermissionRulesOnly": true }"#));
        // Coexists with a permissions block.
        assert!(f(
            r#"{ "allowManagedPermissionRulesOnly": true, "permissions": { "deny": ["Bash(rm:*)"] } }"#
        ));
        // Exactly-true semantics (`===!0`): false / absent / wrong type → off.
        assert!(!f(r#"{ "allowManagedPermissionRulesOnly": false }"#));
        assert!(!f("{}"));
        assert!(!f("not json"));
        // INSIDE `permissions` is the WRONG place (schema keeps it top-level) —
        // must not trigger the lockdown.
        assert!(!f(
            r#"{ "permissions": { "allowManagedPermissionRulesOnly": true } }"#
        ));
    }

    /// The caller-side retain (mirroring claude-code `RKt()` under `$wt()`):
    /// when the lockdown is set, only `PolicySettings`-sourced rules survive.
    #[test]
    fn managed_only_lockdown_retain_drops_non_managed_rules() {
        let user = permission_rules_from_settings_json(
            r#"{ "permissions": { "allow": ["WebFetch"] } }"#,
            PermissionRuleSource::UserSettings,
        )
        .unwrap();
        let managed = permission_rules_from_settings_json(
            r#"{ "permissions": { "deny": ["Bash(rm:*)"] } }"#,
            PermissionRuleSource::PolicySettings,
        )
        .unwrap();
        let mut rules: Vec<PermissionRule> = user.into_iter().chain(managed).collect();
        assert_eq!(rules.len(), 2);
        rules.retain(|r| r.source == PermissionRuleSource::PolicySettings);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].value.tool_name, "Bash");
        assert!(matches!(rules[0].behavior, PermissionBehavior::Deny));
    }

    #[test]
    fn missing_arrays_default_empty() {
        // Only `allow` present; `deny`/`ask` default to [].
        let raw = r#"{ "permissions": { "allow": ["Bash"] } }"#;
        let rules = permission_rules_from_settings_json(raw, PermissionRuleSource::CliArg).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].value.tool_name, "Bash");
    }
}
