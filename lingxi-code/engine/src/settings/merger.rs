// lingxi-code/crates/core/src/settings/merger.rs
//! Per-field merge dispatcher.
//!
//! Signature is consume-and-return because every layer is short-lived during
//! [`crate::settings::Settings::load`]. Strategy choice routes through
//! [`crate::settings::schema::strategy_for`] — Task 5 wires up
//! [`MergeStrategy::ConcatDedup`]; Task 6 adds `DeepMerge` + `Override`.

use crate::settings::schema::SettingsJson;

/// Merge two settings layers — `next` overlays `prev` per field strategy.
///
/// Locked signature: every caller (loader, tests, parity driver) uses this
/// exact form. Do not change without updating every call site.
#[must_use]
pub fn merge(prev: SettingsJson, next: SettingsJson) -> SettingsJson {
    SettingsJson {
        dollar_schema: next.dollar_schema.or(prev.dollar_schema),
        trusted_directories: concat_dedup(prev.trusted_directories, next.trusted_directories),
        additional_directories: concat_dedup(
            prev.additional_directories,
            next.additional_directories,
        ),
        enabled_tools: concat_dedup(prev.enabled_tools, next.enabled_tools),
        additional_includes: concat_dedup(prev.additional_includes, next.additional_includes),
        claude_md_excludes: concat_dedup(prev.claude_md_excludes, next.claude_md_excludes),
        sandbox: deep_merge_object(prev.sandbox, next.sandbox),
        hooks: deep_merge_object(prev.hooks, next.hooks),
        // Deep-merge the opaque block. NOTE: claude-code concat-dedups the
        // allow/deny/ask arrays across tiers; deep_merge_object takes `next`
        // for a matching array key. This only affects a reader of the MERGED
        // field — the permission loader reads each settings FILE per-source
        // (`permission::permission_rules_from_settings_json`), so it is moot
        // for rule loading. (Refine to per-array concat if the merged field is
        // ever consumed directly.)
        permissions: deep_merge_object(prev.permissions, next.permissions),
        // Scalar fields — Override semantics: next wins when set, else prev.
        // OUTSTYLE.1: `outputStyle` is a string in claude-code and merges
        // scalar-override (settingsMergeCustomizer special-cases only arrays).
        output_style: next.output_style.or(prev.output_style),
        telemetry_enabled: next.telemetry_enabled.or(prev.telemetry_enabled),
        model: next.model.or(prev.model),
        providers: deep_merge_object(prev.providers, next.providers),
        routing: deep_merge_value_opt(prev.routing, next.routing),
        // LingXi-only multi-agent config. MUST deep-merge (same treatment as
        // `routing`) — a plain `next.or(prev)` overwrite would let a project
        // layer's `multiAgent` swallow the user layer's whole block.
        multi_agent: deep_merge_value_opt(prev.multi_agent, next.multi_agent),
    }
}

/// Concatenate `prev` then append from `next`, dropping duplicates while
/// preserving first-seen order. Matches spec §7 `ConcatDedup` semantics.
fn concat_dedup(prev: Option<Vec<String>>, next: Option<Vec<String>>) -> Option<Vec<String>> {
    match (prev, next) {
        (None, None) => None,
        (Some(v), None) | (None, Some(v)) => Some(v),
        (Some(p), Some(n)) => {
            let mut out: Vec<String> = Vec::with_capacity(p.len() + n.len());
            for s in p.into_iter().chain(n.into_iter()) {
                if !out.contains(&s) {
                    out.push(s);
                }
            }
            Some(out)
        }
    }
}

/// Deep-merge two object-shaped fields.
///
/// Both sides None → None. One side None → the other side. Both sides Some
/// → key-by-key merge: matching nested objects recurse via [`deep_merge_value`];
/// scalar / array / mismatched-shape keys take `next` (later source wins).
fn deep_merge_object(
    prev: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    next: Option<std::collections::BTreeMap<String, serde_json::Value>>,
) -> Option<std::collections::BTreeMap<String, serde_json::Value>> {
    match (prev, next) {
        (None, None) => None,
        (Some(v), None) | (None, Some(v)) => Some(v),
        (Some(mut p), Some(n)) => {
            for (k, v_next) in n {
                match p.remove(&k) {
                    Some(v_prev) => {
                        p.insert(k, deep_merge_value(v_prev, v_next));
                    }
                    None => {
                        p.insert(k, v_next);
                    }
                }
            }
            Some(p)
        }
    }
}

/// Deep-merge two `Option<serde_json::Value>` fields.
///
/// Both sides `None` → `None`. One side `None` → the other side. Both sides
/// `Some` → recurse via [`deep_merge_value`] (object keys merged; scalar/array
/// mismatch takes `next`).
fn deep_merge_value_opt(
    prev: Option<serde_json::Value>,
    next: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    match (prev, next) {
        (None, None) => None,
        (Some(v), None) | (None, Some(v)) => Some(v),
        (Some(p), Some(n)) => Some(deep_merge_value(p, n)),
    }
}

/// Recurse into a single JSON node. Matches deep-merge semantics for two-level
/// nesting (e.g. `hooks.PreToolUse.{Bash,Read}` from the test).
fn deep_merge_value(prev: serde_json::Value, next: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match (prev, next) {
        (Value::Object(mut p), Value::Object(n)) => {
            for (k, v_next) in n {
                match p.remove(&k) {
                    Some(v_prev) => {
                        p.insert(k, deep_merge_value(v_prev, v_next));
                    }
                    None => {
                        p.insert(k, v_next);
                    }
                }
            }
            Value::Object(p)
        }
        // Mismatched shape or non-object — next wins (Override semantics).
        (_, next) => next,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::schema::SettingsJson;

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn concat_dedup_preserves_first_seen_order() {
        let prev = SettingsJson {
            trusted_directories: Some(vec![s("/a"), s("/b"), s("/c")]),
            ..Default::default()
        };
        let next = SettingsJson {
            trusted_directories: Some(vec![s("/b"), s("/d"), s("/a")]),
            ..Default::default()
        };
        let merged = merge(prev, next);
        assert_eq!(
            merged.trusted_directories.as_deref(),
            Some(&[s("/a"), s("/b"), s("/c"), s("/d")][..]),
            "expected prev order kept and only new entries appended"
        );
    }

    #[test]
    fn concat_dedup_handles_one_side_none() {
        let prev = SettingsJson {
            trusted_directories: Some(vec![s("/a")]),
            ..Default::default()
        };
        let next = SettingsJson::default();
        let merged = merge(prev, next);
        assert_eq!(merged.trusted_directories.as_deref(), Some(&[s("/a")][..]));
    }

    #[test]
    fn concat_dedup_covers_all_four_array_fields() {
        let prev = SettingsJson {
            additional_directories: Some(vec![s("/x")]),
            enabled_tools: Some(vec![s("Bash")]),
            additional_includes: Some(vec![s("CLAUDE.md")]),
            ..Default::default()
        };
        let next = SettingsJson {
            additional_directories: Some(vec![s("/y")]),
            enabled_tools: Some(vec![s("Read")]),
            additional_includes: Some(vec![s("AGENTS.md")]),
            ..Default::default()
        };
        let merged = merge(prev, next);
        assert_eq!(
            merged.additional_directories.as_deref(),
            Some(&[s("/x"), s("/y")][..])
        );
        assert_eq!(
            merged.enabled_tools.as_deref(),
            Some(&[s("Bash"), s("Read")][..])
        );
        assert_eq!(
            merged.additional_includes.as_deref(),
            Some(&[s("CLAUDE.md"), s("AGENTS.md")][..])
        );
    }

    #[test]
    fn object_deep_merge_recurses_one_level() {
        use serde_json::json;
        use std::collections::BTreeMap;

        let mut prev_sandbox = BTreeMap::new();
        prev_sandbox.insert("enabled".to_string(), json!(true));
        prev_sandbox.insert("failIfUnavailable".to_string(), json!(false));

        let mut next_sandbox = BTreeMap::new();
        next_sandbox.insert("failIfUnavailable".to_string(), json!(true));
        next_sandbox.insert(
            "network".to_string(),
            json!({"allowedDomains": ["github.com"]}),
        );

        let prev = SettingsJson {
            sandbox: Some(prev_sandbox),
            ..Default::default()
        };
        let next = SettingsJson {
            sandbox: Some(next_sandbox),
            ..Default::default()
        };

        let merged = merge(prev, next);
        let s = merged.sandbox.unwrap();
        assert_eq!(
            s.get("enabled"),
            Some(&json!(true)),
            "prev-only key survives"
        );
        assert_eq!(
            s.get("failIfUnavailable"),
            Some(&json!(true)),
            "next overrides scalar"
        );
        assert_eq!(
            s.get("network"),
            Some(&json!({"allowedDomains": ["github.com"]})),
            "next-only key is added"
        );
    }

    #[test]
    fn object_deep_merge_recurses_two_levels() {
        use serde_json::json;
        use std::collections::BTreeMap;

        let mut prev_hooks = BTreeMap::new();
        prev_hooks.insert("PreToolUse".to_string(), json!({"Bash": ["echo prev"]}));

        let mut next_hooks = BTreeMap::new();
        next_hooks.insert("PreToolUse".to_string(), json!({"Read": ["echo next"]}));

        let prev = SettingsJson {
            hooks: Some(prev_hooks),
            ..Default::default()
        };
        let next = SettingsJson {
            hooks: Some(next_hooks),
            ..Default::default()
        };

        let merged = merge(prev, next);
        let h = merged.hooks.unwrap();
        // Inner objects are deep-merged: both PreToolUse subkeys land.
        assert_eq!(
            h.get("PreToolUse"),
            Some(&json!({"Bash": ["echo prev"], "Read": ["echo next"]}))
        );
    }

    #[test]
    fn scalar_override_next_wins_when_set() {
        let prev = SettingsJson {
            model: Some("sonnet".into()),
            telemetry_enabled: Some(false),
            ..Default::default()
        };
        let next = SettingsJson {
            model: Some("opus".into()),
            telemetry_enabled: None,
            ..Default::default()
        };
        let merged = merge(prev, next);
        assert_eq!(merged.model.as_deref(), Some("opus"));
        assert_eq!(
            merged.telemetry_enabled,
            Some(false),
            "next is None, so prev survives"
        );
    }

    #[test]
    fn providers_deep_merge_combines_profiles() {
        use serde_json::json;
        use std::collections::BTreeMap;
        let mut p = BTreeMap::new();
        p.insert("groq".to_string(), json!({"type": "openai"}));
        let mut n = BTreeMap::new();
        n.insert("ollama".to_string(), json!({"type": "openai"}));
        let prev = SettingsJson {
            providers: Some(p),
            ..Default::default()
        };
        let next = SettingsJson {
            providers: Some(n),
            ..Default::default()
        };
        let merged = merge(prev, next).providers.unwrap();
        assert!(merged.contains_key("groq") && merged.contains_key("ollama"));
    }

    #[test]
    fn multi_agent_deep_merges_disjoint_keys_across_layers() {
        use serde_json::json;
        // user layer declares candidates+arbiter; project layer declares only
        // triggers. Deep-merge must KEEP both — a plain override would drop the
        // user layer's candidates entirely.
        let user = SettingsJson {
            multi_agent: Some(json!({
                "enabled": true,
                "candidates": [{"id": "fast", "model": "p/m"}],
                "arbiter": {"model": "p/arb"}
            })),
            ..Default::default()
        };
        let project = SettingsJson {
            multi_agent: Some(json!({
                "mode": "force",
                "triggers": {"security": true}
            })),
            ..Default::default()
        };
        // load order: user (prev/lower) then project (next/higher).
        let merged = merge(user, project).multi_agent.unwrap();
        // user-only keys survive
        assert_eq!(merged.get("enabled"), Some(&json!(true)));
        assert_eq!(
            merged.get("candidates"),
            Some(&json!([{"id": "fast", "model": "p/m"}])),
            "user candidates must NOT be clobbered by project layer"
        );
        assert_eq!(merged.get("arbiter"), Some(&json!({"model": "p/arb"})));
        // project keys land too
        assert_eq!(merged.get("mode"), Some(&json!("force")));
        assert_eq!(merged.get("triggers"), Some(&json!({"security": true})));
    }

    #[test]
    fn multi_agent_nested_object_recurses_not_overwrites() {
        use serde_json::json;
        let user = SettingsJson {
            multi_agent: Some(json!({"reviewers": {"crossReview": true, "maxReviewRounds": 1}})),
            ..Default::default()
        };
        let project = SettingsJson {
            multi_agent: Some(json!({"reviewers": {"maxReviewRounds": 2}})),
            ..Default::default()
        };
        let merged = merge(user, project).multi_agent.unwrap();
        // crossReview from user survives; maxReviewRounds overridden by project.
        assert_eq!(
            merged.get("reviewers"),
            Some(&json!({"crossReview": true, "maxReviewRounds": 2}))
        );
    }
}
