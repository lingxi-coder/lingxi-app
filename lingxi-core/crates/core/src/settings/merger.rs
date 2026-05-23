// lingxi-core/crates/core/src/settings/merger.rs
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
        // Filled in by Task 6.
        sandbox: next.sandbox.or(prev.sandbox),
        hooks: next.hooks.or(prev.hooks),
        output_style: next.output_style.or(prev.output_style),
        // Filled in by Task 6.
        telemetry_enabled: next.telemetry_enabled.or(prev.telemetry_enabled),
        model: next.model.or(prev.model),
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
}
