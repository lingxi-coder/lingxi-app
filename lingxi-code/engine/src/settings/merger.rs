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
        lingxi_md_excludes: concat_dedup(prev.lingxi_md_excludes, next.lingxi_md_excludes),
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
        // Scalar Override (later source wins), same as `telemetryEnabled`.
        ax_screen_reader: next.ax_screen_reader.or(prev.ax_screen_reader),
        // Scalar Override (later source wins) — `alwaysThinkingEnabled`.
        always_thinking_enabled: next
            .always_thinking_enabled
            .or(prev.always_thinking_enabled),
        // Scalar Override (later source wins) — `skipWebFetchPreflight` (P2-14).
        skip_web_fetch_preflight: next
            .skip_web_fetch_preflight
            .or(prev.skip_web_fetch_preflight),
        // Scalar Override — `askUserQuestionTimeout` (enum 60s|5m|10m|never).
        ask_user_question_timeout: next
            .ask_user_question_timeout
            .or(prev.ask_user_question_timeout),
        model: next.model.or(prev.model),
        // Managed model-restriction keys (H-BIN-08). `availableModels` (array)
        // and `enforceAvailableModels` (scalar) are scalar-override — CC's
        // `settingsMergeCustomizer` returns the source array for non-concat
        // arrays. `modelOverrides` (record) deep-merges per key (next wins).
        available_models: next.available_models.or(prev.available_models),
        enforce_available_models: next
            .enforce_available_models
            .or(prev.enforce_available_models),
        model_overrides: merge_string_map(prev.model_overrides, next.model_overrides),
        // 2.1.198 AWS/GCP auth-refresh script keys — plain strings, scalar
        // Override (later source wins), same as `model`/`outputStyle`.
        aws_auth_refresh: next.aws_auth_refresh.or(prev.aws_auth_refresh),
        aws_credential_export: next.aws_credential_export.or(prev.aws_credential_export),
        gcp_auth_refresh: next.gcp_auth_refresh.or(prev.gcp_auth_refresh),
        // HTTP-hook security allowlists (H-BIN-12) — both array-merge
        // (concat-dedup): CC `settingsMergeCustomizer` (`ipe`) concat-dedups
        // every array except `fallbackModel`, and both describe strings say
        // "Arrays merge across settings sources (same semantics as
        // allowedMcpServers)."
        allowed_http_hook_urls: concat_dedup(
            prev.allowed_http_hook_urls,
            next.allowed_http_hook_urls,
        ),
        http_hook_allowed_env_vars: concat_dedup(
            prev.http_hook_allowed_env_vars,
            next.http_hook_allowed_env_vars,
        ),
        // Enterprise login/version managed-policy keys (H-BIN-09) — all
        // scalar-override (later source wins); none is a concat/deep-merge
        // field (CC `settingsMergeCustomizer` special-cases only specific
        // arrays/objects, and none of these is one).
        force_login_method: next.force_login_method.or(prev.force_login_method),
        force_login_gateway_url: next.force_login_gateway_url.or(prev.force_login_gateway_url),
        force_login_org_uuid: next.force_login_org_uuid.or(prev.force_login_org_uuid),
        parent_settings_behavior: next
            .parent_settings_behavior
            .or(prev.parent_settings_behavior),
        minimum_version: next.minimum_version.or(prev.minimum_version),
        required_minimum_version: next
            .required_minimum_version
            .or(prev.required_minimum_version),
        required_maximum_version: next
            .required_maximum_version
            .or(prev.required_maximum_version),
        force_remote_settings_refresh: next
            .force_remote_settings_refresh
            .or(prev.force_remote_settings_refresh),
        // `companyAnnouncements` — array-merge (concat-dedup), matching CC's
        // `settingsMergeCustomizer` array customizer.
        company_announcements: concat_dedup(
            prev.company_announcements,
            next.company_announcements,
        ),
        // `plansDirectory` / `apiKeyHelper` — scalar Override (later source wins),
        // same as `model`/`outputStyle`.
        plans_directory: next.plans_directory.or(prev.plans_directory),
        api_key_helper: next.api_key_helper.or(prev.api_key_helper),
        providers: deep_merge_object(prev.providers, next.providers),
        routing: deep_merge_value_opt(prev.routing, next.routing),
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

/// Deep-merge two flat `String→String` maps (`modelOverrides`): union of keys,
/// `next` wins on a collision. A one-level record has no nested structure, so
/// this is CC's lodash object-merge for `modelOverrides`.
fn merge_string_map(
    prev: Option<std::collections::BTreeMap<String, String>>,
    next: Option<std::collections::BTreeMap<String, String>>,
) -> Option<std::collections::BTreeMap<String, String>> {
    match (prev, next) {
        (None, None) => None,
        (Some(v), None) | (None, Some(v)) => Some(v),
        (Some(mut p), Some(n)) => {
            for (k, v) in n {
                p.insert(k, v);
            }
            Some(p)
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
            additional_includes: Some(vec![s("LINGXI.md")]),
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
            Some(&[s("LINGXI.md"), s("AGENTS.md")][..])
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
    fn enterprise_login_version_keys_scalar_override() {
        // H-BIN-09: all enterprise login/version keys are scalar-override —
        // the higher-priority (`next`) layer wins when it sets the key, else the
        // lower layer survives. Models the 4-layer stack folding a base managed
        // tier under a higher-priority drop-in.
        use serde_json::json;
        let prev = SettingsJson {
            force_login_method: Some("claudeai".into()),
            required_minimum_version: Some("2.0.0".into()),
            force_login_org_uuid: Some(json!("org-base")),
            parent_settings_behavior: Some("first-wins".into()),
            force_remote_settings_refresh: Some(false),
            ..Default::default()
        };
        let next = SettingsJson {
            // next overrides method + min + org pin; leaves the rest unset.
            force_login_method: Some("gateway".into()),
            required_minimum_version: Some("2.1.207".into()),
            force_login_org_uuid: Some(json!(["org-a", "org-b"])),
            ..Default::default()
        };
        let merged = merge(prev, next);
        assert_eq!(merged.force_login_method.as_deref(), Some("gateway"));
        assert_eq!(merged.required_minimum_version.as_deref(), Some("2.1.207"));
        assert_eq!(
            merged.force_login_org_uuid,
            Some(json!(["org-a", "org-b"])),
            "org pin is scalar-override (source array wins, not concat)"
        );
        // Keys next left unset survive from prev.
        assert_eq!(merged.parent_settings_behavior.as_deref(), Some("first-wins"));
        assert_eq!(merged.force_remote_settings_refresh, Some(false));
    }

    #[test]
    fn http_hook_security_keys_concat_dedup_across_tiers() {
        // H-BIN-12: both HTTP-hook allowlists concat-dedup across settings
        // sources (CC `settingsMergeCustomizer` concat-dedups every array except
        // `fallbackModel`). A pattern/env-var declared in the lower tier survives
        // and the higher tier's entries append (deduped).
        let prev = SettingsJson {
            allowed_http_hook_urls: Some(vec![s("https://a.example.com/*"), s("https://shared/*")]),
            http_hook_allowed_env_vars: Some(vec![s("TOKEN_A"), s("SHARED")]),
            ..Default::default()
        };
        let next = SettingsJson {
            allowed_http_hook_urls: Some(vec![s("https://shared/*"), s("https://b.example.com/*")]),
            http_hook_allowed_env_vars: Some(vec![s("SHARED"), s("TOKEN_B")]),
            ..Default::default()
        };
        let merged = merge(prev, next);
        assert_eq!(
            merged.allowed_http_hook_urls.as_deref(),
            Some(
                &[
                    s("https://a.example.com/*"),
                    s("https://shared/*"),
                    s("https://b.example.com/*")
                ][..]
            )
        );
        assert_eq!(
            merged.http_hook_allowed_env_vars.as_deref(),
            Some(&[s("TOKEN_A"), s("SHARED"), s("TOKEN_B")][..])
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
}
