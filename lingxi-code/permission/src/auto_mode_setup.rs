//! WIZARD-06 permission-layer foundation for the `/auto-mode-setup` wizard.
//!
//! This module is the byte-exact permission substrate the wizard's apply/write
//! path consumes: the `removeFromPermissionsAllow` proposal-array validator
//! (`tFt` cap + error strings) and the `--apply-file` read-gate result codes +
//! messages. The interactive recon / LLM-propose / TUI-review layers (WIZARD-06
//! S4-S6) are built in later waves and call into these.

use serde_json::Value;

/// `tFt` — the maximum number of entries a `removeFromPermissionsAllow` proposal
/// array may carry (2.1.218: `tFt=200`).
pub const MAX_REMOVE_FROM_PERMISSIONS_ALLOW: usize = 200;

/// Validate a proposal's `removeFromPermissionsAllow` value (the wizard's offer
/// to remove destructive ALLOW rules the user already had). Returns
/// `Some(error_message)` (byte-exact vs 2.1.218) on the first failure, or `None`
/// when the value is absent/`null` or a valid array of well-formed rule strings.
///
/// 1:1 with the oracle validator:
/// ```js
/// let r=e.removeFromPermissionsAllow;
/// if(r!==void 0){
///   if(!Array.isArray(r))return"removeFromPermissionsAllow must be an array of rule strings.";
///   if(r.length>tFt)return`removeFromPermissionsAllow has ${r.length} entries; the maximum is ${tFt}.`;
///   for(let[n,o]of r.entries())if(typeof o!=="string"||!smr(o))
///     return`removeFromPermissionsAllow[${n}] is not a rule string the removal offer could have produced.`
/// }
/// ```
///
/// `smr(o)` (well-formed-rule check) is approximated by [`is_removable_rule_string`]
/// — a non-empty `Tool` / `Tool(content)` shape; the offer only ever produces
/// such strings, so a stricter check would only over-reject a HAND-crafted
/// payload (never accept a malformed one into the removal set).
#[must_use]
pub fn validate_remove_from_permissions_allow(value: Option<&Value>) -> Option<String> {
    let value = match value {
        None | Some(Value::Null) => return None,
        Some(v) => v,
    };
    let Some(arr) = value.as_array() else {
        return Some("removeFromPermissionsAllow must be an array of rule strings.".to_string());
    };
    if arr.len() > MAX_REMOVE_FROM_PERMISSIONS_ALLOW {
        return Some(format!(
            "removeFromPermissionsAllow has {} entries; the maximum is {}.",
            arr.len(),
            MAX_REMOVE_FROM_PERMISSIONS_ALLOW
        ));
    }
    for (n, o) in arr.iter().enumerate() {
        let ok = o.as_str().is_some_and(is_removable_rule_string);
        if !ok {
            return Some(format!(
                "removeFromPermissionsAllow[{n}] is not a rule string the removal offer could have produced."
            ));
        }
    }
    None
}

/// `smr` approximation: a rule string the removal offer could have produced —
/// non-empty, of the shape `Tool` or `Tool(content)` (a leading identifier tool
/// name, optionally followed by a parenthesised content spec). See
/// [`validate_remove_from_permissions_allow`] for why an approximation is safe.
#[must_use]
pub fn is_removable_rule_string(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    // Split off an optional `(content)` suffix; the tool name is what precedes it.
    let tool = match s.split_once('(') {
        Some((tool, rest)) => {
            if !rest.ends_with(')') {
                return false;
            }
            tool
        }
        None => s,
    };
    let tool = tool.trim();
    !tool.is_empty()
        && tool
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '*'))
}

/// Result of the `--apply-file` read gate (2.1.218 `auto_mode_setup_write`
/// codes). The gate refuses to read a proposal file unless it is an absolute
/// path under the system temp dir or the Claude config dir AND is not covered by
/// a `permissions.deny` READ rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyFileGate {
    /// The path is not an absolute path under the system-temp / config dir (or is
    /// rejected by the traversal guard). Code `bad_path`.
    BadPath,
    /// The path is covered by a `permissions.deny` read rule. Code `read_denied`.
    ReadDenied,
    /// The proposal file could not be read (missing / not a regular file / …).
    /// Code `read_failed`.
    ReadFailed,
}

impl ApplyFileGate {
    /// The byte-exact `code` string emitted with the `auto_mode_setup_write`
    /// telemetry + returned to the reviewing host.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            ApplyFileGate::BadPath => "bad_path",
            ApplyFileGate::ReadDenied => "read_denied",
            ApplyFileGate::ReadFailed => "read_failed",
        }
    }

    /// The byte-exact human-readable `reason` string (2.1.218).
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            ApplyFileGate::BadPath => "Pass an absolute path under the system temp directory or the Claude config directory \u{2014} --apply-file only reads proposal files the reviewing host wrote there.",
            ApplyFileGate::ReadDenied => "That path is covered by a permissions.deny read rule. Write the proposal somewhere the session can read.",
            ApplyFileGate::ReadFailed => "Couldn\u{2019}t read the proposal file. Check the path and that it is a regular file.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remove_validation_accepts_absent_and_valid() {
        assert_eq!(validate_remove_from_permissions_allow(None), None);
        assert_eq!(
            validate_remove_from_permissions_allow(Some(&Value::Null)),
            None
        );
        assert_eq!(
            validate_remove_from_permissions_allow(Some(&json!(["Bash(*)", "Bash(rm:*)", "Edit"]))),
            None
        );
    }

    #[test]
    fn remove_validation_rejects_non_array_and_bad_entries() {
        assert_eq!(
            validate_remove_from_permissions_allow(Some(&json!("Bash(*)"))),
            Some("removeFromPermissionsAllow must be an array of rule strings.".to_string())
        );
        assert_eq!(
            validate_remove_from_permissions_allow(Some(&json!(["Bash(*)", 7]))),
            Some(
                "removeFromPermissionsAllow[1] is not a rule string the removal offer could have produced."
                    .to_string()
            )
        );
        assert_eq!(
            validate_remove_from_permissions_allow(Some(&json!(["Bash(*)", ""]))),
            Some(
                "removeFromPermissionsAllow[1] is not a rule string the removal offer could have produced."
                    .to_string()
            )
        );
    }

    #[test]
    fn remove_validation_enforces_max_cap() {
        assert_eq!(MAX_REMOVE_FROM_PERMISSIONS_ALLOW, 200);
        let over: Vec<Value> = (0..201).map(|_| json!("Bash(*)")).collect();
        assert_eq!(
            validate_remove_from_permissions_allow(Some(&Value::Array(over))),
            Some("removeFromPermissionsAllow has 201 entries; the maximum is 200.".to_string())
        );
        // Exactly at the cap is allowed.
        let at: Vec<Value> = (0..200).map(|_| json!("Bash(*)")).collect();
        assert_eq!(
            validate_remove_from_permissions_allow(Some(&Value::Array(at))),
            None
        );
    }

    #[test]
    fn apply_file_gate_codes_and_reasons_are_byte_exact() {
        assert_eq!(ApplyFileGate::BadPath.code(), "bad_path");
        assert_eq!(ApplyFileGate::ReadDenied.code(), "read_denied");
        assert_eq!(ApplyFileGate::ReadFailed.code(), "read_failed");
        assert_eq!(
            ApplyFileGate::ReadDenied.reason(),
            "That path is covered by a permissions.deny read rule. Write the proposal somewhere the session can read."
        );
        assert!(ApplyFileGate::BadPath
            .reason()
            .starts_with("Pass an absolute path under the system temp directory"));
        assert!(ApplyFileGate::ReadFailed
            .reason()
            .starts_with("Couldn\u{2019}t read the proposal file."));
    }
}
