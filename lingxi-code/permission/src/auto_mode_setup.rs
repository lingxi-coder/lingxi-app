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
    /// The proposal file was truncated at the read cap. Code `too_large`.
    TooLarge,
    /// The `--expect-sha256` argument is not a 64-char hex sha256. Code
    /// `bad_hash_arg`. (The `missing_hash_arg`/`hash_mismatch`/`scope_mismatch`
    /// codes carry RUNTIME-interpolated reasons and are produced by the command
    /// handler — built in the command wave; see the blueprint.)
    BadHashArg,
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
            ApplyFileGate::TooLarge => "too_large",
            ApplyFileGate::BadHashArg => "bad_hash_arg",
        }
    }

    /// The byte-exact human-readable `reason` string (2.1.218). (Only the codes
    /// with a STATIC reason live here; the interpolated ones are formatted at the
    /// command handler.)
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            ApplyFileGate::BadPath => "Pass an absolute path under the system temp directory or the Claude config directory \u{2014} --apply-file only reads proposal files the reviewing host wrote there.",
            ApplyFileGate::ReadDenied => "That path is covered by a permissions.deny read rule. Write the proposal somewhere the session can read.",
            ApplyFileGate::ReadFailed => "Couldn\u{2019}t read the proposal file. Check the path and that it is a regular file.",
            ApplyFileGate::TooLarge => "The proposal file is over the 1 MB cap \u{2014} a real proposal is a few KB. Regenerate it with --propose.",
            ApplyFileGate::BadHashArg => "--expect-sha256 must be the 64-character hex sha256 digest of the proposal file\u{2019}s exact bytes.",
        }
    }
}

/// Is `s` a valid `--expect-sha256` argument — exactly 64 lowercase-or-uppercase
/// hex characters (the sha256 of the proposal file's exact bytes)?
#[must_use]
pub fn is_valid_expect_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

// ── S3 `--apply-file` path predicates (2.1.218) ──────────────────────────────

/// `Qc` — a UNC path (`//host` or `\\host`).
#[must_use]
fn is_unc_path(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 2 && matches!(b[0], b'/' | b'\\') && matches!(b[1], b'/' | b'\\')
}

/// `Kf` — a WSL UNC path, 1:1 with `/^[\\/]{2}wsl(\$|\.localhost)[\\/]/i`:
/// two separators, `wsl`, then `$` or `.localhost`, then one separator.
#[must_use]
fn is_wsl_path(p: &str) -> bool {
    let lower = p.to_ascii_lowercase();
    let b = lower.as_bytes();
    if b.len() < 2 || !matches!(b[0], b'/' | b'\\') || !matches!(b[1], b'/' | b'\\') {
        return false;
    }
    let rest = &lower[2..];
    for prefix in ["wsl$", "wsl.localhost"] {
        if let Some(after) = rest.strip_prefix(prefix) {
            if after.starts_with(['/', '\\']) {
                return true;
            }
        }
    }
    false
}

/// `Cft.resolve`-style LEXICAL normalization: collapse `.`/`..` components
/// WITHOUT touching the filesystem (does not follow symlinks — mirrors the
/// oracle `path.resolve`). A leading `..` above the root is dropped.
fn lexical_normalize(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                if !out.pop() {
                    // Above the root — keep dropping (matches path.resolve, which
                    // clamps at `/`).
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `Pge`/`vbi` — a path that normalizes (resolving `.`/`..`) to a `/net/<host>`
/// autofs network mount. Only absolute (`/`-rooted) POSIX paths qualify.
#[must_use]
fn is_net_automount_path(p: &str) -> bool {
    if !p.starts_with('/') {
        return false;
    }
    let mut t: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            t.pop();
            continue;
        }
        t.push(seg);
        if t.len() == 2 && t[0].eq_ignore_ascii_case("net") {
            return true;
        }
    }
    false
}

/// `r7e` — reject a path as network-borne (unsafe to read as a local proposal
/// file): `(is_unc && !is_wsl) || is_net_automount`.
#[must_use]
pub fn is_rejected_network_path(p: &str) -> bool {
    is_unc_path(p) && !is_wsl_path(p) || is_net_automount_path(p)
}

/// `DOd` — is `path` located under any of the provided containment `roots` (the
/// system temp dir and the Claude config dir, each supplied both resolved and
/// realpath-canonicalized by the caller)? 1:1 with the oracle relative-path test
/// (`rel != "" && !rel.startsWith("..") && !isAbsolute(rel)`).
#[must_use]
pub fn path_under_containment_root(path: &std::path::Path, roots: &[std::path::PathBuf]) -> bool {
    // Lexically resolve the path FIRST (oracle `Cft.resolve(e)`), so a mid-path
    // `..` escape (`/tmp/a/../../etc/x`) cannot lexically "match" a root.
    let resolved = lexical_normalize(path);
    for root in roots {
        if let Ok(rel) = resolved.strip_prefix(root) {
            // Non-empty rel (path strictly under root); strip_prefix on a
            // normalized path never yields a `..`-leading remainder, so a genuine
            // descendant is the only match — `rel == ""` (path == root) is
            // excluded (oracle `o !== ""`).
            if rel.components().next().is_some() {
                return true;
            }
        }
    }
    false
}

/// The pre-read half of the `--apply-file` gate. Returns:
/// - `Some(BadPath)` when the path is not absolute, is network-borne (`r7e`), or
///   is not under a containment root (`!DOd`);
/// - `Some(ReadDenied)` when `is_read_denied(path)` (a `permissions.deny` READ
///   rule covers it, or a UNC network-dir block);
/// - `None` when the caller may proceed to read the file (a read failure then
///   maps to [`ApplyFileGate::ReadFailed`]).
///
/// `roots` are the resolved + canonicalized temp/config containment roots and
/// `is_read_denied` is supplied by the command layer from the live policy.
#[must_use]
pub fn apply_file_pre_read_gate(
    path: &std::path::Path,
    roots: &[std::path::PathBuf],
    is_read_denied: impl Fn(&std::path::Path) -> bool,
) -> Option<ApplyFileGate> {
    let path_str = path.to_string_lossy();
    if !path.is_absolute()
        || is_rejected_network_path(&path_str)
        || !path_under_containment_root(path, roots)
    {
        return Some(ApplyFileGate::BadPath);
    }
    if is_read_denied(path) {
        return Some(ApplyFileGate::ReadDenied);
    }
    None
}

// ── apply/write settings mutation (2.1.218) ──────────────────────────────────

/// Build the `autoMode` settings object an accepted proposal writes. 1:1 with
/// the oracle:
/// ```js
/// {
///   environment: proposal.environment,
///   ...(proposal.allow.length>0 && {allow: proposal.allow}),
///   ...(proposal.soft_deny.length>0 && {soft_deny: proposal.soft_deny}),
///   ...(proposal.hard_deny.length>0 && {hard_deny: proposal.hard_deny})
/// }
/// ```
/// `environment` is ALWAYS written; the three rule arrays are included only when
/// non-empty (an empty category is omitted, not written as `[]`). Key insertion
/// order matches the oracle (environment, allow, soft_deny, hard_deny) — the
/// crate's `serde_json` has `preserve_order`.
#[must_use]
pub fn build_auto_mode_settings(
    environment: &[Value],
    allow: &[Value],
    soft_deny: &[Value],
    hard_deny: &[Value],
) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("environment".into(), Value::Array(environment.to_vec()));
    if !allow.is_empty() {
        map.insert("allow".into(), Value::Array(allow.to_vec()));
    }
    if !soft_deny.is_empty() {
        map.insert("soft_deny".into(), Value::Array(soft_deny.to_vec()));
    }
    if !hard_deny.is_empty() {
        map.insert("hard_deny".into(), Value::Array(hard_deny.to_vec()));
    }
    Value::Object(map)
}

/// Apply a `removeFromPermissionsAllow` set to a `permissions.allow` array:
/// return the array with every verbatim `to_remove` rule string filtered out,
/// plus the count removed. String comparison is EXACT (the offer only ever
/// carries strings copied verbatim from the existing allow list).
#[must_use]
pub fn remove_rules_from_permissions_allow(
    allow: &[String],
    to_remove: &[String],
) -> (Vec<String>, usize) {
    let removed_set: std::collections::HashSet<&str> =
        to_remove.iter().map(String::as_str).collect();
    let kept: Vec<String> = allow
        .iter()
        .filter(|rule| !removed_set.contains(rule.as_str()))
        .cloned()
        .collect();
    let removed = allow.len() - kept.len();
    (kept, removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::{Path, PathBuf};

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
        assert_eq!(ApplyFileGate::TooLarge.code(), "too_large");
        assert_eq!(
            ApplyFileGate::TooLarge.reason(),
            "The proposal file is over the 1 MB cap \u{2014} a real proposal is a few KB. Regenerate it with --propose."
        );
        assert_eq!(ApplyFileGate::BadHashArg.code(), "bad_hash_arg");
        assert_eq!(
            ApplyFileGate::BadHashArg.reason(),
            "--expect-sha256 must be the 64-character hex sha256 digest of the proposal file\u{2019}s exact bytes."
        );
    }

    #[test]
    fn expect_sha256_validation() {
        assert!(is_valid_expect_sha256(&"a".repeat(64)));
        assert!(is_valid_expect_sha256(
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        ));
        assert!(!is_valid_expect_sha256(&"a".repeat(63))); // too short
        assert!(!is_valid_expect_sha256(&"a".repeat(65))); // too long
        assert!(!is_valid_expect_sha256(&"g".repeat(64))); // non-hex
        assert!(!is_valid_expect_sha256(""));
    }

    #[test]
    fn rejected_network_paths() {
        // UNC (non-WSL) rejected.
        assert!(is_rejected_network_path("//host/share/x"));
        assert!(is_rejected_network_path("\\\\host\\share\\x"));
        // WSL UNC is the exception — NOT rejected.
        assert!(!is_rejected_network_path("//wsl$/Ubuntu/home/u/x"));
        assert!(!is_rejected_network_path("\\\\wsl.localhost\\Ubuntu\\x"));
        // /net/ autofs mount rejected (incl. via `..` normalization).
        assert!(is_rejected_network_path("/net/host/x"));
        assert!(is_rejected_network_path("/a/../net/host/x"));
        // Plain local absolute path — not network-borne.
        assert!(!is_rejected_network_path("/tmp/proposal.json"));
        assert!(!is_rejected_network_path("/net")); // needs a second segment
    }

    #[test]
    fn containment_root_and_dotdot_escape() {
        let roots = vec![PathBuf::from("/tmp/rev"), PathBuf::from("/home/u/.config")];
        assert!(path_under_containment_root(
            Path::new("/tmp/rev/proposal.json"),
            &roots
        ));
        assert!(path_under_containment_root(
            Path::new("/home/u/.config/sub/p.json"),
            &roots
        ));
        // path == root is NOT "under".
        assert!(!path_under_containment_root(Path::new("/tmp/rev"), &roots));
        // Not under any root.
        assert!(!path_under_containment_root(Path::new("/etc/passwd"), &roots));
        // SECURITY: a mid-path `..` escape must NOT lexically match a root.
        assert!(!path_under_containment_root(
            Path::new("/tmp/rev/a/../../etc/passwd"),
            &roots
        ));
        // A `..` that stays inside the root IS under it.
        assert!(path_under_containment_root(
            Path::new("/tmp/rev/a/../p.json"),
            &roots
        ));
    }

    #[test]
    fn pre_read_gate_decisions() {
        let roots = vec![PathBuf::from("/tmp/rev")];
        // Not absolute → BadPath.
        assert_eq!(
            apply_file_pre_read_gate(Path::new("rel/p.json"), &roots, |_| false),
            Some(ApplyFileGate::BadPath)
        );
        // Out of containment → BadPath.
        assert_eq!(
            apply_file_pre_read_gate(Path::new("/etc/passwd"), &roots, |_| false),
            Some(ApplyFileGate::BadPath)
        );
        // Network-borne → BadPath.
        assert_eq!(
            apply_file_pre_read_gate(Path::new("/net/h/p.json"), &roots, |_| false),
            Some(ApplyFileGate::BadPath)
        );
        // In containment but covered by a Read deny rule → ReadDenied.
        assert_eq!(
            apply_file_pre_read_gate(Path::new("/tmp/rev/p.json"), &roots, |_| true),
            Some(ApplyFileGate::ReadDenied)
        );
        // In containment, not denied → proceed (None; caller reads → ReadFailed on error).
        assert_eq!(
            apply_file_pre_read_gate(Path::new("/tmp/rev/p.json"), &roots, |_| false),
            None
        );
    }

    #[test]
    fn build_auto_mode_settings_omits_empty_categories_in_order() {
        // environment always; allow/soft_deny/hard_deny only when non-empty.
        let s = build_auto_mode_settings(
            &[json!("uses git")],
            &[json!("read files")],
            &[],
            &[json!("rm -rf /")],
        );
        let keys: Vec<&str> = s.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["environment", "allow", "hard_deny"]); // no soft_deny
        assert_eq!(s["environment"], json!(["uses git"]));
        assert_eq!(s["allow"], json!(["read files"]));
        assert_eq!(s["hard_deny"], json!(["rm -rf /"]));
        assert!(s.get("soft_deny").is_none());
        // Empty everything → just an (empty) environment array.
        let e = build_auto_mode_settings(&[], &[], &[], &[]);
        assert_eq!(e, json!({ "environment": [] }));
    }

    #[test]
    fn remove_rules_filters_verbatim_and_counts() {
        let allow = vec![
            "Bash(*)".to_string(),
            "Bash(rm:*)".to_string(),
            "Edit".to_string(),
            "Read(./x)".to_string(),
        ];
        let (kept, removed) =
            remove_rules_from_permissions_allow(&allow, &["Bash(*)".into(), "Read(./x)".into()]);
        assert_eq!(kept, vec!["Bash(rm:*)".to_string(), "Edit".to_string()]);
        assert_eq!(removed, 2);
        // A removal entry not present in allow is a no-op (exact match only).
        let (kept, removed) =
            remove_rules_from_permissions_allow(&allow, &["Bash(ls:*)".into()]);
        assert_eq!(kept.len(), 4);
        assert_eq!(removed, 0);
    }
}
