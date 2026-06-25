// lingxi-code/crates/core/src/settings/env_parser.rs
//! Walk a process-env snapshot, picking up keys under the three prefixes
//! `LINGXI_*` (highest) → `CLAUDE_CODE_*` → `CLAUDE_*` (lowest).
//!
//! The three prefixes overlay each other: when the same logical field is
//! set under multiple prefixes, the highest-priority prefix wins. Within
//! each prefix the `SCREAMING_SNAKE_CASE` key is mapped to the camelCase
//! field name in [`crate::settings::schema::SettingsJson`].
//!
//! Multi-valued fields (arrays) use `:` as element separator, matching
//! claude-code's PATH-style convention (spec §7).

use crate::settings::schema::SettingsJson;
use crate::settings::SettingsError;
use std::collections::BTreeMap;

/// Ordered (highest-priority first) prefix list. Locked by spec §7.
pub const PREFIX_PRIORITY: &[&str] = &["LINGXI_", "CLAUDE_CODE_", "CLAUDE_"];

/// Mapping from `SCREAMING_SNAKE_CASE` suffix → camelCase field name.
///
/// Each entry is `(suffix, field, kind)` where `kind` drives value parsing.
const FIELD_MAP: &[(&str, &str, FieldKind)] = &[
    ("MODEL", "model", FieldKind::Scalar),
    ("TELEMETRY_ENABLED", "telemetryEnabled", FieldKind::Bool),
    (
        "TRUSTED_DIRECTORIES",
        "trustedDirectories",
        FieldKind::Array,
    ),
    (
        "ADDITIONAL_DIRECTORIES",
        "additionalDirectories",
        FieldKind::Array,
    ),
    ("ENABLED_TOOLS", "enabledTools", FieldKind::Array),
    (
        "ADDITIONAL_INCLUDES",
        "additionalIncludes",
        FieldKind::Array,
    ),
];

#[derive(Debug, Clone, Copy)]
enum FieldKind {
    Scalar,
    Bool,
    Array,
}

/// LingXi-only multi-agent env suffixes. Each composes into a key path inside
/// the `multiAgent` settings object (NOT a top-level typed field), so they are
/// handled by [`parse_multi_agent_env`] rather than [`FIELD_MAP`].
///
/// `(suffix, dotted-path-into-multiAgent, kind)`:
/// - `MULTI_AGENT`            → `mode` (off|auto|force; any other value invalid)
/// - `MULTI_AGENT_ENABLED`    → `enabled` (bool)
/// - `MULTI_AGENT_STRATEGY`   → `strategy` (string)
/// - `MULTI_AGENT_TIMEOUT_SECONDS` → `limits.timeoutSeconds` (u64)
/// - `MULTI_AGENT_REVIEW_ROUNDS`   → `reviewers.maxReviewRounds` (u64)
const MULTI_AGENT_ENV: &[(&str, &str, MultiAgentKind)] = &[
    ("MULTI_AGENT", "mode", MultiAgentKind::Mode),
    ("MULTI_AGENT_ENABLED", "enabled", MultiAgentKind::Bool),
    ("MULTI_AGENT_STRATEGY", "strategy", MultiAgentKind::Str),
    (
        "MULTI_AGENT_TIMEOUT_SECONDS",
        "limits.timeoutSeconds",
        MultiAgentKind::Uint,
    ),
    (
        "MULTI_AGENT_REVIEW_ROUNDS",
        "reviewers.maxReviewRounds",
        MultiAgentKind::Uint,
    ),
];

#[derive(Debug, Clone, Copy)]
enum MultiAgentKind {
    /// off|auto|force
    Mode,
    Bool,
    Str,
    Uint,
}

/// Resolve the highest-priority env var name for a given multi-agent suffix
/// (used to re-derive the canonical var name when reporting an invalid value).
fn canonical_multi_agent_var(suffix: &str) -> String {
    format!("{}{suffix}", PREFIX_PRIORITY[0])
}

/// Parse the LingXi-only `LINGXI_MULTI_AGENT*` (and lower-priority prefix)
/// env vars into a `multiAgent` JSON object. Returns `None` when no recognized
/// multi-agent var is set. Invalid values are pushed onto `invalid`.
///
/// Prefix overlay matches the rest of `parse_env`: highest-priority prefix
/// wins per logical field.
fn parse_multi_agent_env(
    env: &BTreeMap<String, String>,
    invalid: &mut Vec<(String, String)>,
) -> Option<serde_json::Value> {
    use serde_json::Value;

    // First writer (highest-priority prefix) wins per suffix.
    let mut chosen: BTreeMap<&'static str, (String, MultiAgentKind)> = BTreeMap::new();
    for prefix in PREFIX_PRIORITY {
        for (suffix, path, kind) in MULTI_AGENT_ENV {
            let key = format!("{prefix}{suffix}");
            if let Some(value) = env.get(&key) {
                chosen.entry(*path).or_insert_with(|| (value.clone(), *kind));
            }
        }
    }
    if chosen.is_empty() {
        return None;
    }

    let mut root = serde_json::Map::new();
    for (path, (raw, kind)) in &chosen {
        // Re-derive the suffix from the path for canonical invalid reporting.
        let suffix = MULTI_AGENT_ENV
            .iter()
            .find_map(|(s, p, _)| (p == path).then_some(*s))
            .unwrap_or("MULTI_AGENT");
        let parsed: Option<Value> = match kind {
            MultiAgentKind::Mode => match raw.as_str() {
                "off" | "auto" | "force" => Some(Value::String(raw.clone())),
                _ => None,
            },
            MultiAgentKind::Bool => match raw.as_str() {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => None,
            },
            MultiAgentKind::Str => Some(Value::String(raw.clone())),
            MultiAgentKind::Uint => raw.parse::<u64>().ok().map(|n| Value::from(n)),
        };
        match parsed {
            Some(v) => insert_dotted(&mut root, path, v),
            None => invalid.push((canonical_multi_agent_var(suffix), raw.clone())),
        }
    }

    if root.is_empty() {
        None
    } else {
        Some(Value::Object(root))
    }
}

/// Insert `value` at a (max two-level) dotted `path` inside `root`, creating
/// intermediate objects as needed. e.g. `"limits.timeoutSeconds"`.
fn insert_dotted(root: &mut serde_json::Map<String, serde_json::Value>, path: &str, value: serde_json::Value) {
    use serde_json::Value;
    match path.split_once('.') {
        None => {
            root.insert(path.to_string(), value);
        }
        Some((head, tail)) => {
            let entry = root
                .entry(head.to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(inner) = entry {
                inner.insert(tail.to_string(), value);
            }
        }
    }
}

/// Parse a process-env snapshot into a partial [`SettingsJson`] + a list of
/// invalid `(var, value)` pairs for telemetry.
///
/// # Errors
///
/// This function never returns `Err` directly — all parse failures are
/// reported via the invalid-list return so the loader can keep going.
/// The `Result` shape is preserved for API symmetry with the other modules.
pub fn parse_env(
    env: &BTreeMap<String, String>,
) -> Result<(SettingsJson, Vec<(String, String)>), SettingsError> {
    let mut chosen: BTreeMap<&'static str, String> = BTreeMap::new();
    let mut invalid: Vec<(String, String)> = Vec::new();

    // Walk prefixes highest-priority first; first writer wins per field.
    for prefix in PREFIX_PRIORITY {
        for (suffix, field, _kind) in FIELD_MAP {
            let key = format!("{prefix}{suffix}");
            if let Some(value) = env.get(&key) {
                chosen.entry(*field).or_insert_with(|| value.clone());
            }
        }
    }

    // Also surface any env var with a recognized prefix that we didn't
    // know how to map — these are likely typos worth telemetering. Keys
    // handled by the multi-agent pass below are NOT unknown, so exclude them.
    for (k, v) in env {
        let is_known_field = FIELD_MAP
            .iter()
            .any(|(suffix, _, _)| PREFIX_PRIORITY.iter().any(|p| format!("{p}{suffix}") == *k));
        let is_multi_agent = MULTI_AGENT_ENV
            .iter()
            .any(|(suffix, _, _)| PREFIX_PRIORITY.iter().any(|p| format!("{p}{suffix}") == *k));
        if PREFIX_PRIORITY.iter().any(|p| k.starts_with(*p)) && !is_known_field && !is_multi_agent {
            invalid.push((k.clone(), v.clone()));
        }
    }

    // Build the SettingsJson from chosen values.
    let mut out = SettingsJson::default();
    for (field, raw) in &chosen {
        let kind = FIELD_MAP
            .iter()
            .find_map(|(_, f, k)| (f == field).then_some(*k))
            .expect("chosen field must be in FIELD_MAP");
        match (kind, *field) {
            (FieldKind::Scalar, "model") => out.model = Some(raw.clone()),
            (FieldKind::Bool, "telemetryEnabled") => match raw.as_str() {
                "true" => out.telemetry_enabled = Some(true),
                "false" => out.telemetry_enabled = Some(false),
                _ => {
                    // Re-derive the canonical env-var name from the highest-priority prefix that holds this value.
                    let var = PREFIX_PRIORITY
                        .iter()
                        .map(|p| format!("{p}TELEMETRY_ENABLED"))
                        .find(|k| env.get(k).map(String::as_str) == Some(raw.as_str()))
                        .unwrap_or_else(|| "LINGXI_TELEMETRY_ENABLED".to_string());
                    invalid.push((var, raw.clone()));
                }
            },
            (FieldKind::Array, "trustedDirectories") => {
                out.trusted_directories = Some(raw.split(':').map(str::to_string).collect());
            }
            (FieldKind::Array, "additionalDirectories") => {
                out.additional_directories = Some(raw.split(':').map(str::to_string).collect());
            }
            (FieldKind::Array, "enabledTools") => {
                out.enabled_tools = Some(raw.split(':').map(str::to_string).collect());
            }
            (FieldKind::Array, "additionalIncludes") => {
                out.additional_includes = Some(raw.split(':').map(str::to_string).collect());
            }
            _ => unreachable!("FIELD_MAP / match arm mismatch — every entry must be handled"),
        }
    }

    // LingXi-only multi-agent env vars compose into the opaque `multiAgent`
    // object rather than a typed top-level field.
    out.multi_agent = parse_multi_agent_env(env, &mut invalid);

    Ok((out, invalid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn lingxi_prefix_wins_over_claude_code() {
        let env = env(&[
            ("LINGXI_MODEL", "claude-opus-4-7"),
            ("CLAUDE_CODE_MODEL", "claude-sonnet-4-5"),
            ("CLAUDE_MODEL", "claude-sonnet-4"),
        ]);
        let (parsed, _) = parse_env(&env).unwrap();
        assert_eq!(parsed.model.as_deref(), Some("claude-opus-4-7"));
    }

    #[test]
    fn claude_code_prefix_wins_over_claude() {
        let env = env(&[
            ("CLAUDE_CODE_MODEL", "claude-sonnet-4-5"),
            ("CLAUDE_MODEL", "claude-sonnet-4"),
        ]);
        let (parsed, _) = parse_env(&env).unwrap();
        assert_eq!(parsed.model.as_deref(), Some("claude-sonnet-4-5"));
    }

    #[test]
    fn array_value_splits_on_colon() {
        let env = env(&[("LINGXI_TRUSTED_DIRECTORIES", "/a:/b:/c")]);
        let (parsed, _) = parse_env(&env).unwrap();
        assert_eq!(
            parsed.trusted_directories.as_deref(),
            Some(&["/a".to_string(), "/b".to_string(), "/c".to_string()][..])
        );
    }

    #[test]
    fn bool_value_parses_true_false_only() {
        let env_ok = env(&[("LINGXI_TELEMETRY_ENABLED", "true")]);
        let (parsed, invalid) = parse_env(&env_ok).unwrap();
        assert_eq!(parsed.telemetry_enabled, Some(true));
        assert!(invalid.is_empty());

        let env_bad = env(&[("LINGXI_TELEMETRY_ENABLED", "yes")]);
        let (_, invalid) = parse_env(&env_bad).unwrap();
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0].0, "LINGXI_TELEMETRY_ENABLED");
        assert_eq!(invalid[0].1, "yes");
    }

    #[test]
    fn unknown_env_var_with_recognized_prefix_is_returned_in_invalid_list() {
        let env = env(&[("LINGXI_UNKNOWN_FIELD", "x")]);
        let (_, invalid) = parse_env(&env).unwrap();
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0].0, "LINGXI_UNKNOWN_FIELD");
    }

    #[test]
    fn unrelated_env_vars_are_ignored() {
        let env = env(&[("PATH", "/usr/bin"), ("HOME", "/Users/x")]);
        let (parsed, invalid) = parse_env(&env).unwrap();
        assert!(parsed.model.is_none());
        assert!(invalid.is_empty());
    }

    #[test]
    fn multi_agent_env_composes_into_object() {
        use serde_json::json;
        let env = env(&[
            ("LINGXI_MULTI_AGENT", "force"),
            ("LINGXI_MULTI_AGENT_ENABLED", "true"),
            ("LINGXI_MULTI_AGENT_STRATEGY", "dualLlmCompetitive"),
            ("LINGXI_MULTI_AGENT_TIMEOUT_SECONDS", "1800"),
            ("LINGXI_MULTI_AGENT_REVIEW_ROUNDS", "2"),
        ]);
        let (parsed, invalid) = parse_env(&env).unwrap();
        assert!(invalid.is_empty(), "multi-agent vars must not be flagged invalid: {invalid:?}");
        assert_eq!(
            parsed.multi_agent,
            Some(json!({
                "mode": "force",
                "enabled": true,
                "strategy": "dualLlmCompetitive",
                "limits": {"timeoutSeconds": 1800},
                "reviewers": {"maxReviewRounds": 2}
            }))
        );
    }

    #[test]
    fn multi_agent_env_absent_yields_none() {
        let env = env(&[("LINGXI_MODEL", "x")]);
        let (parsed, _) = parse_env(&env).unwrap();
        assert!(parsed.multi_agent.is_none());
    }

    #[test]
    fn multi_agent_mode_rejects_invalid_value() {
        let env = env(&[("LINGXI_MULTI_AGENT", "sometimes")]);
        let (parsed, invalid) = parse_env(&env).unwrap();
        // invalid mode is dropped (no `mode` key) and reported.
        assert!(parsed.multi_agent.is_none());
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0].0, "LINGXI_MULTI_AGENT");
        assert_eq!(invalid[0].1, "sometimes");
    }

    #[test]
    fn multi_agent_review_rounds_rejects_non_integer() {
        let env = env(&[("LINGXI_MULTI_AGENT_REVIEW_ROUNDS", "lots")]);
        let (parsed, invalid) = parse_env(&env).unwrap();
        assert!(parsed.multi_agent.is_none());
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0].0, "LINGXI_MULTI_AGENT_REVIEW_ROUNDS");
    }

    #[test]
    fn multi_agent_lower_priority_prefix_used_when_lingxi_absent() {
        use serde_json::json;
        let env = env(&[("CLAUDE_CODE_MULTI_AGENT", "auto")]);
        let (parsed, invalid) = parse_env(&env).unwrap();
        assert!(invalid.is_empty());
        assert_eq!(parsed.multi_agent, Some(json!({"mode": "auto"})));
    }

    #[test]
    fn multi_agent_lingxi_prefix_wins_over_claude_code() {
        use serde_json::json;
        let env = env(&[
            ("LINGXI_MULTI_AGENT", "force"),
            ("CLAUDE_CODE_MULTI_AGENT", "auto"),
        ]);
        let (parsed, _) = parse_env(&env).unwrap();
        assert_eq!(parsed.multi_agent, Some(json!({"mode": "force"})));
    }
}
