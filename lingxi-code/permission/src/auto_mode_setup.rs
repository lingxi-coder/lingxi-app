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
}
