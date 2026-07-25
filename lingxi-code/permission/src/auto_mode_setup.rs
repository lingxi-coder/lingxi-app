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
    /// The `--expect-sha256` argument is missing. Code `missing_hash_arg`.
    MissingHashArg,
    /// The `--expect-sha256` argument is not a 64-char hex sha256. Code
    /// `bad_hash_arg`.
    BadHashArg,
    /// The proposal file's bytes do not hash to the reviewed digest. Code
    /// `hash_mismatch`. (The result frame also carries the `expectedSha256`; the
    /// `scope_mismatch` code's reason is RUNTIME-interpolated with `--apply-target`
    /// and is produced by the command handler — see the blueprint.)
    HashMismatch,
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
            ApplyFileGate::MissingHashArg => "missing_hash_arg",
            ApplyFileGate::BadHashArg => "bad_hash_arg",
            ApplyFileGate::HashMismatch => "hash_mismatch",
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
            ApplyFileGate::MissingHashArg => "--expect-sha256 is required: pass the 64-character hex sha256 of the proposal file\u{2019}s exact bytes, before --apply-file. Every non-interactive apply is hash-bound.",
            ApplyFileGate::BadHashArg => "--expect-sha256 must be the 64-character hex sha256 digest of the proposal file\u{2019}s exact bytes.",
            ApplyFileGate::HashMismatch => "The proposal file\u{2019}s bytes do not match the reviewed digest \u{2014} the file changed after it was approved. Nothing was written; regenerate the proposal, re-review, and retry.",
        }
    }
}

/// Is `s` a valid `--expect-sha256` argument — exactly 64 lowercase-or-uppercase
/// hex characters (the sha256 of the proposal file's exact bytes)?
#[must_use]
pub fn is_valid_expect_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Verify a proposal file's hash against the `--expect-sha256` argument (2.1.218
/// `auto_mode_setup_write` hash gate). `actual_sha256` is the LOWERCASE hex
/// sha256 of the proposal file's exact bytes, computed by the command layer.
/// Returns `Some(gate)` on failure — `MissingHashArg` (no `--expect-sha256`),
/// `BadHashArg` (not 64-hex), or `HashMismatch` (`actual !== expected`) — or
/// `None` when the hash matches.
///
/// The comparison is EXACT (`i !== e.expectedSha256`): `actual_sha256` is
/// lowercase hex, so an uppercase-but-correct `--expect-sha256` is a
/// `HashMismatch` (byte-faithful to the oracle, which does not case-fold).
#[must_use]
pub fn verify_proposal_hash(actual_sha256: &str, expect: Option<&str>) -> Option<ApplyFileGate> {
    let expect = match expect {
        None => return Some(ApplyFileGate::MissingHashArg),
        Some(e) if e.is_empty() => return Some(ApplyFileGate::MissingHashArg),
        Some(e) => e,
    };
    if !is_valid_expect_sha256(expect) {
        return Some(ApplyFileGate::BadHashArg);
    }
    if actual_sha256 != expect {
        return Some(ApplyFileGate::HashMismatch);
    }
    None
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

// ── vNs: the save-payload validator (2.1.218) ────────────────────────────────

/// The `$defaults` sentinel (`pV`) that an `autoMode` rule array must include so
/// it EXTENDS the shipped built-in rules instead of REPLACING them.
pub const AUTO_MODE_DEFAULTS_SENTINEL: &str = "$defaults";

/// `oDo` — the maximum characters (JS UTF-16 length) of a single rule entry.
const MAX_ENTRY_LEN_UTF16: usize = 10_000;

/// The rule-array categories checked after `environment` (`man`).
const AUTO_MODE_RULE_KEYS: [&str; 3] = ["allow", "soft_deny", "hard_deny"];

/// `LQ_` — does `s` contain a control character (`<32` except tab, `127..=159`,
/// U+2028, U+2029)? Such entries are not single-line text.
fn has_control_char(s: &str) -> bool {
    s.chars().any(|c| {
        let r = c as u32;
        (r < 32 && r != 9) || (127..=159).contains(&r) || r == 8232 || r == 8233
    })
}

/// `NQ_` (approx) — does `s` contain an invisible or bidirectional character
/// (zero-width, bidi controls, BOM, soft hyphen)? The exact `MQ_` regex is not
/// extracted; this covers the standard invisible/bidi set. Over-inclusion only
/// tightens a config-write validation (never accepts a malformed entry).
fn has_invisible_or_bidi(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c as u32,
            0x00AD | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180E
            | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x206A..=0x206F
            | 0xFEFF | 0xFFF9..=0xFFFB)
    })
}

/// JS `String.length` (UTF-16 code units).
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `iDo(name, entries)` — validate one rule array; returns the byte-exact error
/// message on the first bad entry, or `None` when valid. `rte` (entry normalize)
/// is approximated by the identity (its `FQ_` strip only removes invisible
/// characters that `has_invisible_or_bidi` already rejects).
fn validate_save_array(name: &str, entries: &[&str]) -> Option<String> {
    if entries.len() > MAX_REMOVE_FROM_PERMISSIONS_ALLOW {
        return Some(format!(
            "{name} has {} entries; the maximum is {}.",
            entries.len(),
            MAX_REMOVE_FROM_PERMISSIONS_ALLOW
        ));
    }
    for entry in entries {
        if entry.trim().is_empty() {
            return Some(format!("{name} contains an empty entry."));
        }
        let len = utf16_len(entry);
        if len > MAX_ENTRY_LEN_UTF16 {
            return Some(format!(
                "{name} contains an entry of {len} characters; the maximum is {MAX_ENTRY_LEN_UTF16}."
            ));
        }
        if has_control_char(entry) {
            return Some(format!(
                "{name} contains an entry with a control character; entries must be single-line text."
            ));
        }
        if has_invisible_or_bidi(entry) {
            return Some(format!(
                "{name} contains an entry with an invisible or bidirectional character; entries must be plainly renderable text."
            ));
        }
    }
    None
}

/// Read a JSON string array as `Vec<&str>`, or `None` when the value is not an
/// array of strings.
fn string_array<'a>(v: Option<&'a Value>) -> Option<Vec<&'a str>> {
    v?.as_array()?.iter().map(Value::as_str).collect()
}

/// `vNs(payload)` — validate the `{autoMode, removeFromPermissionsAllow}` save
/// payload the wizard writes. Returns the byte-exact error message on the first
/// failure, or `None` when the payload is valid and non-empty.
///
/// `auto_mode` is the parsed `autoMode` block (`{environment, allow?, soft_deny?,
/// hard_deny?}`) or `None`; `remove` is the `removeFromPermissionsAllow` value.
/// The `RRt().safeParse` zod pass is subsumed by the structural checks here (the
/// port has no standalone autoMode-block schema); a non-array `environment`/rule
/// key is treated as absent, matching the "empty/omit" guidance.
#[must_use]
pub fn validate_auto_mode_save(auto_mode: Option<&Value>, remove: Option<&Value>) -> Option<String> {
    let remove_empty = match remove {
        None | Some(Value::Null) => true,
        Some(v) => v.as_array().is_some_and(|a| a.is_empty()),
    };
    if auto_mode.is_none() && remove_empty {
        return Some("Nothing to save.".to_string());
    }
    if let Some(am) = auto_mode {
        let environment = string_array(am.get("environment")).unwrap_or_default();
        if environment.is_empty() {
            return Some("autoMode.environment is empty \u{2014} nothing to save.".to_string());
        }
        if let Some(e) = validate_save_array("environment", &environment) {
            return Some(e);
        }
        if environment.contains(&AUTO_MODE_DEFAULTS_SENTINEL) {
            return Some(format!(
                "autoMode.environment must not contain \"{AUTO_MODE_DEFAULTS_SENTINEL}\" \u{2014} skipped slots get their shipped default text written verbatim instead."
            ));
        }
        for key in AUTO_MODE_RULE_KEYS {
            let Some(s) = am.get(key) else {
                continue; // `s === void 0` → skip
            };
            let arr = string_array(Some(s)).unwrap_or_default();
            if arr.is_empty() {
                return Some(format!(
                    "autoMode.{key} is empty \u{2014} omit the key when nothing was accepted for it."
                ));
            }
            if let Some(e) = validate_save_array(key, &arr) {
                return Some(e);
            }
            if !arr.contains(&AUTO_MODE_DEFAULTS_SENTINEL) {
                return Some(format!(
                    "autoMode.{key} is missing the literal entry \"{AUTO_MODE_DEFAULTS_SENTINEL}\" \u{2014} without it the array replaces the shipped rules instead of extending them."
                ));
            }
        }
    }
    validate_remove_from_permissions_allow(remove)
}

// ── apply-file pre-write pipeline (2.1.218) ──────────────────────────────────

/// The outcome of the secure proposal-file read the COMMAND layer performs
/// (open `O_NOFOLLOW` → regular file → `nlink == 1` → read up to the 1 MB cap),
/// plus the sha256 the command layer computes over the exact bytes.
#[derive(Debug, Clone)]
pub enum ProposalRead {
    /// The file could not be securely read (missing / symlink / not a regular
    /// file / `nlink != 1` / io error) → `read_failed`.
    Failed,
    /// The file exceeded the read cap and was truncated → `too_large`.
    TooLarge,
    /// The exact bytes + their lowercase-hex sha256.
    Read { bytes: Vec<u8>, sha256_hex: String },
}

/// The `--apply-file` invocation arguments (the parsed CLI flags).
pub struct ApplyFileArgs<'a> {
    /// The `--apply-file` path.
    pub path: &'a std::path::Path,
    /// The resolved temp/config containment roots (see [`path_under_containment_root`]).
    pub roots: &'a [std::path::PathBuf],
    /// The `--expect-sha256` argument, if any.
    pub expect_sha256: Option<&'a str>,
    /// The `--apply-target` argument, if any (present ⇒ the scope check runs).
    pub apply_target: Option<&'a str>,
    /// The scope `--apply-target` expects (`YQ_[target]`), supplied by the caller.
    pub expected_scope: Option<&'a str>,
}

/// Result of the apply-file PRE-WRITE pipeline (everything up to the point the
/// proposal is ready to validate + write). On success the caller runs
/// [`validate_auto_mode_save`] then persists via the command layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyFilePipeline {
    /// A gate rejected the request. `code` is the byte-exact
    /// `auto_mode_setup_write` code; `reason` the byte-exact message.
    Rejected { code: String, reason: String },
    /// All pre-write gates passed; `proposal` is the parsed proposal object.
    Proceed { proposal: Value },
}

/// The byte-exact `scope_mismatch` reason (interpolated with `--apply-target`).
#[must_use]
pub fn scope_mismatch_reason(proposal_scope: Option<&str>, target: &str, expected: &str) -> String {
    match proposal_scope {
        None => format!(
            "This proposal was generated before save scope was recorded, so --apply-target {target} can\u{2019}t confirm it matches. Regenerate the proposal with --propose, answering scope={expected}."
        ),
        Some(s) => format!(
            "This proposal was generated for a different save scope ({s}) than --apply-target {target} expects ({expected}). Regenerate the proposal with --propose, answering scope={expected}."
        ),
    }
}

/// Run the `--apply-file` PRE-WRITE pipeline (2.1.218): path gate → secure read →
/// hash verify → parse → scope check. Pure/testable: the caller performs the
/// actual fs read + sha256 and passes [`ProposalRead`]; `is_read_denied` comes
/// from the live policy's `Read`-`deny` rules.
#[must_use]
pub fn evaluate_apply_file(
    args: &ApplyFileArgs,
    is_read_denied: impl Fn(&std::path::Path) -> bool,
    read: ProposalRead,
) -> ApplyFilePipeline {
    let reject = |gate: ApplyFileGate| ApplyFilePipeline::Rejected {
        code: gate.code().to_string(),
        reason: gate.reason().to_string(),
    };
    // 1. path gate (bad_path / read_denied).
    if let Some(gate) = apply_file_pre_read_gate(args.path, args.roots, &is_read_denied) {
        return reject(gate);
    }
    // 2. secure read (read_failed / too_large).
    let (bytes, sha256) = match read {
        ProposalRead::Failed => return reject(ApplyFileGate::ReadFailed),
        ProposalRead::TooLarge => return reject(ApplyFileGate::TooLarge),
        ProposalRead::Read { bytes, sha256_hex } => (bytes, sha256_hex),
    };
    // 3. hash verify (missing_hash_arg / bad_hash_arg / hash_mismatch).
    if let Some(gate) = verify_proposal_hash(&sha256, args.expect_sha256) {
        return reject(gate);
    }
    // 4. parse (parse_failed when the bytes are not a readable proposal object).
    let proposal: Value = match serde_json::from_slice(&bytes) {
        Ok(v @ Value::Object(_)) => v,
        _ => {
            return ApplyFilePipeline::Rejected {
                code: "parse_failed".to_string(),
                reason: "That file doesn\u{2019}t contain a proposal this command can read. Regenerate it with --propose and pass that output.".to_string(),
            }
        }
    };
    // 5. scope check (only when --apply-target is provided).
    if let Some(target) = args.apply_target {
        let expected = args.expected_scope.unwrap_or("");
        let proposal_scope = proposal.get("scope").and_then(Value::as_str);
        if proposal_scope != args.expected_scope {
            return ApplyFilePipeline::Rejected {
                code: "scope_mismatch".to_string(),
                reason: scope_mismatch_reason(proposal_scope, target, expected),
            };
        }
    }
    ApplyFilePipeline::Proceed { proposal }
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
    fn hash_gate_codes_reasons_and_verification() {
        assert_eq!(ApplyFileGate::MissingHashArg.code(), "missing_hash_arg");
        assert_eq!(
            ApplyFileGate::MissingHashArg.reason(),
            "--expect-sha256 is required: pass the 64-character hex sha256 of the proposal file\u{2019}s exact bytes, before --apply-file. Every non-interactive apply is hash-bound."
        );
        assert_eq!(ApplyFileGate::HashMismatch.code(), "hash_mismatch");
        assert!(ApplyFileGate::HashMismatch
            .reason()
            .starts_with("The proposal file\u{2019}s bytes do not match the reviewed digest"));

        let digest = "a".repeat(64);
        // Missing / empty → MissingHashArg.
        assert_eq!(
            verify_proposal_hash(&digest, None),
            Some(ApplyFileGate::MissingHashArg)
        );
        assert_eq!(
            verify_proposal_hash(&digest, Some("")),
            Some(ApplyFileGate::MissingHashArg)
        );
        // Not 64-hex → BadHashArg.
        assert_eq!(
            verify_proposal_hash(&digest, Some("zzzz")),
            Some(ApplyFileGate::BadHashArg)
        );
        // Correct → None.
        assert_eq!(verify_proposal_hash(&digest, Some(&digest)), None);
        // Mismatch → HashMismatch.
        assert_eq!(
            verify_proposal_hash(&digest, Some(&"b".repeat(64))),
            Some(ApplyFileGate::HashMismatch)
        );
        // Uppercase-but-equal is a MISMATCH (byte-faithful: lowercase actual vs exact compare).
        assert_eq!(
            verify_proposal_hash(&digest, Some(&"A".repeat(64))),
            Some(ApplyFileGate::HashMismatch)
        );
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
    fn apply_file_pipeline_all_branches() {
        fn run(
            path: &str,
            expect: Option<&str>,
            target: Option<&str>,
            scope: Option<&str>,
            denied: bool,
            read: ProposalRead,
        ) -> ApplyFilePipeline {
            let roots = [PathBuf::from("/tmp/rev")];
            let args = ApplyFileArgs {
                path: Path::new(path),
                roots: &roots,
                expect_sha256: expect,
                apply_target: target,
                expected_scope: scope,
            };
            evaluate_apply_file(&args, |_| denied, read)
        }
        let digest = "a".repeat(64);
        let ok = |v: serde_json::Value| ProposalRead::Read {
            bytes: serde_json::to_vec(&v).unwrap(),
            sha256_hex: digest.clone(),
        };
        let g = "/tmp/rev/p.json";

        // bad_path (out of containment).
        assert!(matches!(
            run("/etc/passwd", Some(&digest), None, None, false, ok(json!({}))),
            ApplyFilePipeline::Rejected { code, .. } if code == "bad_path"
        ));
        // read_denied.
        assert!(matches!(
            run(g, Some(&digest), None, None, true, ok(json!({}))),
            ApplyFilePipeline::Rejected { code, .. } if code == "read_denied"
        ));
        // read_failed / too_large.
        assert!(matches!(
            run(g, Some(&digest), None, None, false, ProposalRead::Failed),
            ApplyFilePipeline::Rejected { code, .. } if code == "read_failed"
        ));
        assert!(matches!(
            run(g, Some(&digest), None, None, false, ProposalRead::TooLarge),
            ApplyFilePipeline::Rejected { code, .. } if code == "too_large"
        ));
        // hash: missing / mismatch.
        assert!(matches!(
            run(g, None, None, None, false, ok(json!({}))),
            ApplyFilePipeline::Rejected { code, .. } if code == "missing_hash_arg"
        ));
        let other = "b".repeat(64);
        assert!(matches!(
            run(g, Some(&other), None, None, false, ok(json!({}))),
            ApplyFilePipeline::Rejected { code, .. } if code == "hash_mismatch"
        ));
        // parse_failed (non-object bytes).
        assert_eq!(
            run(g, Some(&digest), None, None, false, ProposalRead::Read { bytes: b"not json".to_vec(), sha256_hex: digest.clone() }),
            ApplyFilePipeline::Rejected {
                code: "parse_failed".to_string(),
                reason: "That file doesn\u{2019}t contain a proposal this command can read. Regenerate it with --propose and pass that output.".to_string()
            }
        );
        // scope_mismatch — undefined scope.
        assert_eq!(
            run(g, Some(&digest), Some("user"), Some("user-scope"), false, ok(json!({"autoMode": {}}))),
            ApplyFilePipeline::Rejected {
                code: "scope_mismatch".to_string(),
                reason: "This proposal was generated before save scope was recorded, so --apply-target user can\u{2019}t confirm it matches. Regenerate the proposal with --propose, answering scope=user-scope.".to_string()
            }
        );
        // scope_mismatch — different scope.
        assert!(matches!(
            run(g, Some(&digest), Some("user"), Some("user-scope"), false, ok(json!({"scope": "project-scope"}))),
            ApplyFilePipeline::Rejected { code, reason } if code == "scope_mismatch" && reason.contains("different save scope (project-scope)")
        ));
        // Proceed — scope matches, and (separately) no target.
        assert!(matches!(
            run(g, Some(&digest), Some("user"), Some("user-scope"), false, ok(json!({"scope": "user-scope", "autoMode": {"environment": ["x"]}}))),
            ApplyFilePipeline::Proceed { .. }
        ));
        assert!(matches!(
            run(g, Some(&digest), None, None, false, ok(json!({"autoMode": {}}))),
            ApplyFilePipeline::Proceed { .. }
        ));
    }

    #[test]
    fn vns_save_validation_messages() {
        // Nothing to save.
        assert_eq!(
            validate_auto_mode_save(None, None),
            Some("Nothing to save.".to_string())
        );
        // environment empty.
        assert_eq!(
            validate_auto_mode_save(Some(&json!({"environment": []})), None),
            Some("autoMode.environment is empty \u{2014} nothing to save.".to_string())
        );
        // environment must not contain $defaults.
        assert_eq!(
            validate_auto_mode_save(Some(&json!({"environment": ["ok", "$defaults"]})), None),
            Some("autoMode.environment must not contain \"$defaults\" \u{2014} skipped slots get their shipped default text written verbatim instead.".to_string())
        );
        // a rule category present but empty.
        assert_eq!(
            validate_auto_mode_save(
                Some(&json!({"environment": ["ctx"], "allow": []})),
                None
            ),
            Some("autoMode.allow is empty \u{2014} omit the key when nothing was accepted for it.".to_string())
        );
        // a rule category missing the $defaults sentinel.
        assert_eq!(
            validate_auto_mode_save(
                Some(&json!({"environment": ["ctx"], "allow": ["read files"]})),
                None
            ),
            Some("autoMode.allow is missing the literal entry \"$defaults\" \u{2014} without it the array replaces the shipped rules instead of extending them.".to_string())
        );
        // iDo: empty entry / control char.
        assert_eq!(
            validate_auto_mode_save(Some(&json!({"environment": ["  "]})), None),
            Some("environment contains an empty entry.".to_string())
        );
        assert_eq!(
            validate_auto_mode_save(Some(&json!({"environment": ["a\u{0007}b"]})), None),
            Some("environment contains an entry with a control character; entries must be single-line text.".to_string())
        );
        assert_eq!(
            validate_auto_mode_save(Some(&json!({"environment": ["a\u{200B}b"]})), None),
            Some("environment contains an entry with an invisible or bidirectional character; entries must be plainly renderable text.".to_string())
        );
        // A fully valid payload → None.
        assert_eq!(
            validate_auto_mode_save(
                Some(&json!({"environment": ["uses git"], "allow": ["read files", "$defaults"]})),
                Some(&json!(["Bash(*)"]))
            ),
            None
        );
        // No autoMode but a non-empty removal is valid (not "Nothing to save").
        assert_eq!(
            validate_auto_mode_save(None, Some(&json!(["Bash(*)"]))),
            None
        );
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
