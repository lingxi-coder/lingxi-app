//! Managed model allowlist — `availableModels` / `enforceAvailableModels` /
//! `modelOverrides`.
//!
//! 1:1 port of claude-code 2.1.207's managed model-restriction mechanism. An
//! enterprise administrator publishes an `availableModels` allowlist (plus the
//! `enforceAvailableModels` gate and an optional `modelOverrides` map) in the
//! MANAGED (`policySettings`) settings tier; the CLI then constrains which
//! models a user may select and which model `Default` resolves to.
//!
//! ## Allowlist entry semantics (binary `sl()` matcher, describe text verbatim)
//!
//! > Allowlist of models that users can select. Accepts family aliases
//! > (`"opus"` allows any opus version), version prefixes (`"opus-4-5"` allows
//! > only that version), and full model IDs. If undefined, all models are
//! > available. If empty array, only the default model is available.
//!
//! - `None` (unset) ⇒ every model is allowed.
//! - `Some([])` (empty) ⇒ nothing is allowed here (only the default model, which
//!   the picker/resolver keep selectable out-of-band).
//! - A **family alias** entry (`opus`/`sonnet`/`haiku`/`fable`) matches any model
//!   whose id contains that family token at a word boundary
//!   (binary `HYm`→`shi`) — UNLESS the allowlist also carries a more specific
//!   version-prefix of that family (binary `z5l`), in which case the broad
//!   family entry is skipped so the admin's narrower intent wins.
//! - A **version-prefix** entry (`opus-4-5`, `claude-opus-4-5`) matches a model
//!   whose id starts with the entry at a `-`/end boundary (binary `DYm`→`V5l`),
//!   trying both the bare entry and a `claude-`-prefixed form.
//! - A **full model id** entry matches that id exactly.
//! - `modelOverrides` reverse-maps a provider-specific id (e.g. a Bedrock
//!   inference-profile ARN) back to its Anthropic id before matching (binary
//!   `K5l`), so an admin whose fleet runs Bedrock can allowlist the Anthropic id.
//!
//! ## Normalization (binary `Ps`)
//!
//! Every id/entry is `trim().to_lowercase()`-ed and has a trailing `[1m]`
//! long-context suffix stripped before comparison.
//!
//! ## `enforceAvailableModels` policy-provenance (binary `ROn`)
//!
//! The enforce gate only binds when a **policy-owned** allowlist exists — the
//! flag on its own (no `availableModels` in the SAME policy view) is inert. A
//! policy source that exists but fails to load "refuses cascade-trust mode"
//! (fail-closed): enforcement from lower tiers is suppressed until the policy
//! source is fixed. See [`resolve_enforcement`].
//!
//! ## Deferred (documented remainder — needs the alias/catalog resolver)
//!
//! The binary matcher has two further branches that resolve an ALIAS *input*
//! (or a versioned-alias allowlist entry) through the live model catalog
//! (`Fx`/`Zo`/`_kr` — tier- and env-dependent). Those hops, the non-env-free
//! entitlement guard (`_On`), and the Windows `hkcu` registry cascade-trust
//! shortcut are NOT ported here; this module implements the env-free-resolution
//! path (the one the default-model constraint uses:
//! `sl(r,{allowlist,overridesMap,envFreeAliasResolution:!0})`), which covers the
//! three documented entry kinds (family alias, version prefix, full id) plus
//! `modelOverrides`.

use std::collections::BTreeMap;

/// Family-alias tokens (binary `vxr`): a bare family name matches any version.
const FAMILY_ALIASES: &[&str] = &["sonnet", "opus", "haiku", "fable"];

/// All model-alias tokens (binary `wpe`), post-`Ps` (the `[1m]` variants
/// collapse onto their base). An allowlist entry that is a pure alias is handled
/// by the family/alias branches, not the version-prefix branch.
const ALL_ALIASES: &[&str] = &["sonnet", "opus", "haiku", "fable", "best", "opusplan"];

/// Normalize a model id or allowlist entry for comparison — binary `Ps` composed
/// with `trim().toLowerCase()`: lowercase, trim, then strip a trailing `[1m]`
/// long-context suffix (binary `Ps(e)=e.replace(/\[1m\]$/i,"")`).
#[must_use]
pub fn normalize(s: &str) -> String {
    let lowered = s.trim().to_lowercase();
    lowered.strip_suffix("[1m]").unwrap_or(&lowered).to_string()
}

/// Whether a normalized token is a bare family alias (binary `XB`→`vxr`).
#[must_use]
fn is_family_alias(s: &str) -> bool {
    FAMILY_ALIASES.contains(&s)
}

/// Whether a normalized token is any model alias (binary `Fx`→`wpe`, post-`Ps`).
#[must_use]
fn is_alias(s: &str) -> bool {
    ALL_ALIASES.contains(&s)
}

/// Binary `shi(e,t)`: does token `family` appear inside `model` at word
/// boundaries (each side is start/end or a non-`[a-z0-9]` char)? This is the
/// family-membership test — `shi("claude-opus-4-5","opus")` is true.
#[must_use]
fn family_segment_match(model: &str, family: &str) -> bool {
    if family.is_empty() {
        return false;
    }
    let bytes = model.as_bytes();
    let flen = family.len();
    let mut start = 0;
    while let Some(pos) = model[start..].find(family) {
        let at = start + pos;
        let left_ok = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        let end = at + flen;
        let right_ok = end == model.len() || !bytes[end].is_ascii_alphanumeric();
        if left_ok && right_ok {
            return true;
        }
        start = at + 1;
    }
    false
}

/// Binary `V5l(e,t)`: `e` starts with `t` at a `-`/end boundary.
#[must_use]
fn starts_with_segment(model: &str, prefix: &str) -> bool {
    if !model.starts_with(prefix) {
        return false;
    }
    model.len() == prefix.len() || model.as_bytes()[prefix.len()] == b'-'
}

/// Binary `DYm(e,t)` (env-free arm, `e` already a concrete id): match the
/// version-prefix entry against the bare form and a `claude-`-prefixed form.
#[must_use]
fn version_prefix_match(model: &str, entry: &str) -> bool {
    if starts_with_segment(model, entry) {
        return true;
    }
    if !entry.starts_with("claude-") && starts_with_segment(model, &format!("claude-{entry}")) {
        return true;
    }
    false
}

/// Binary `z5l(e,t)`: is `token` a version-prefix segment of some NON-family
/// entry in the normalized allowlist `o`? Guards the family-alias branch so a
/// narrower version entry (e.g. `opus-4-5`) suppresses the broad family entry
/// (`opus`).
#[must_use]
fn is_prefix_segment_of_any(token: &str, allowlist: &[String]) -> bool {
    for entry in allowlist {
        if is_family_alias(entry) {
            continue;
        }
        let mut start = 0;
        while let Some(pos) = entry[start..].find(token) {
            let at = start + pos;
            let end = at + token.len();
            if end == entry.len() || entry.as_bytes()[end] == b'-' {
                return true;
            }
            start = at + 1;
        }
    }
    false
}

/// Binary `K5l(e,t)`: reverse-map a provider-specific model id back to its
/// Anthropic id via `modelOverrides` (key = Anthropic id, value = provider id).
/// If `model` normalizes-equal to some override VALUE, return that entry's KEY;
/// otherwise return `model` unchanged.
#[must_use]
fn apply_overrides(model: &str, overrides: &BTreeMap<String, String>) -> String {
    let target = normalize(model);
    for (anthropic_id, provider_id) in overrides {
        if normalize(provider_id) == target {
            return anthropic_id.clone();
        }
    }
    model.to_string()
}

/// Whether `model` is permitted by the allowlist (env-free-resolution arm of the
/// binary `sl()` matcher). `allowlist == None` ⇒ every model allowed; an empty
/// allowlist ⇒ nothing allowed here.
#[must_use]
pub fn is_model_allowed(
    model: &str,
    allowlist: Option<&[String]>,
    overrides: Option<&BTreeMap<String, String>>,
) -> bool {
    let entries = match allowlist {
        None => return true,                     // `if(!n)return!0`
        Some(a) if a.is_empty() => return false, // `if(n.length===0)return!1`
        Some(a) => a,
    };
    let o: Vec<String> = entries.iter().map(|l| normalize(l)).collect();
    let i = normalize(model);

    // (A) exact match, input not a bare family alias.
    if o.iter().any(|e| e == &i) && !is_family_alias(&i) {
        return true;
    }

    // Reverse-map through modelOverrides, then re-normalize.
    let mapped = match overrides {
        Some(m) if !m.is_empty() => apply_overrides(model, m),
        _ => model.to_string(),
    };
    let a = normalize(&mapped);

    // (B) exact match after override reverse-map.
    if o.iter().any(|e| e == &a) && (!is_family_alias(&a) || !is_prefix_segment_of_any(&a, &o)) {
        return true;
    }

    // (C) family-alias allowlist entries — skipped when a narrower version entry
    // for the same family is also present.
    for entry in &o {
        if is_family_alias(entry)
            && !is_prefix_segment_of_any(entry, &o)
            && family_segment_match(&a, entry)
        {
            return true;
        }
    }

    // (F) version-prefix / full-id allowlist entries (aliases handled above /
    // in the deferred catalog-resolution branches).
    for entry in &o {
        if !is_family_alias(entry) && !is_alias(entry) && version_prefix_match(&a, entry) {
            return true;
        }
    }

    false
}

/// Resolve the default model under an active allowlist to the "first allowed
/// `availableModels` entry" (binary `enforceAvailableModels` describe text: "if
/// the default model for the user tier is not in `availableModels`, Default
/// resolves to the first allowed `availableModels` entry instead").
///
/// Iterates the allowlist entries in order (admin priority) and returns the
/// first `candidate` that entry permits. `candidates` is the concrete model
/// catalog to resolve family/prefix entries against.
#[must_use]
pub fn first_allowed_model(
    allowlist: &[String],
    candidates: &[String],
    overrides: Option<&BTreeMap<String, String>>,
) -> Option<String> {
    for entry in allowlist {
        let single = [entry.clone()];
        for cand in candidates {
            if is_model_allowed(cand, Some(&single), overrides) {
                return Some(cand.clone());
            }
        }
    }
    None
}

// ── enforceAvailableModels policy-provenance (binary `ROn`) ────────────────

/// Byte-exact warning strings (binary), emitted deduplicated by the caller.
pub mod warnings {
    /// The enforce flag is set in the policy view but no `availableModels` is —
    /// enforcement stays disabled (the flag requires a policy-owned allowlist).
    pub const ENFORCE_WITHOUT_ALLOWLIST: &str = "enforceAvailableModels: the policy view sets the enforce flag but not availableModels; enforcement is disabled (the flag requires a policy-owned allowlist)";
    /// A policy source exists but failed to load — refuse cascade-trust mode.
    pub const POLICY_LOAD_FAILED_CASCADE: &str = "enforceAvailableModels: a policy source exists but failed to load; refusing cascade-trust mode (model enforcement from user/project settings is disabled until the policy source is fixed)";
    /// Prefix of the policy-tier read-failure message (binary appends the error).
    pub const POLICY_READ_FAILED_PREFIX: &str =
        "enforceAvailableModels: policy-tier settings read failed; refusing cascade-trust mode: ";
    /// A model was requested for a subagent but is not allowlisted.
    pub const NOT_IN_ALLOWLIST_SUBAGENT: &str =
        "\" is not in the availableModels allowlist; inheriting the parent model instead";
    /// A model is not allowlisted; keep the current session model.
    pub const NOT_IN_ALLOWLIST_SESSION: &str =
        "\" is not in the availableModels allowlist; keeping the session model";
    /// A teammate model is not allowlisted; use the default teammate model.
    pub const NOT_IN_ALLOWLIST_TEAMMATE: &str =
        "\" is not in the availableModels allowlist; using the default teammate model instead";
}

/// The policy (managed / `policySettings`) view of the model-restriction keys.
#[derive(Debug, Clone, Default)]
pub struct PolicyModelView {
    /// Policy-tier `availableModels` (`None` ⇒ key absent in the policy view).
    pub available_models: Option<Vec<String>>,
    /// Policy-tier `enforceAvailableModels`.
    pub enforce: Option<bool>,
    /// Policy-tier `modelOverrides`.
    pub model_overrides: Option<BTreeMap<String, String>>,
}

/// Whether the policy source loaded, and if so its model view.
#[derive(Debug, Clone)]
pub enum PolicySource {
    /// The policy source loaded (or is simply absent) — carries its model view.
    Loaded(PolicyModelView),
    /// A policy source exists on disk but failed to parse (fail-closed).
    Failed,
}

/// Resolved enforcement state (binary `ROn` return, minus the Windows `hkcu`
/// cascade-trusted shortcut which has no LingXi analog).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelEnforcement {
    /// A policy source failed to load — every model is refused (fail-closed).
    Refused,
    /// No enforcement in effect — the allowlist imposes no restriction.
    Inactive,
    /// Enforcement is active with a policy-owned allowlist.
    Active {
        /// The policy-owned allowlist.
        allowlist: Vec<String>,
        /// The policy-owned `modelOverrides` (empty when unset).
        overrides: BTreeMap<String, String>,
    },
}

/// Resolve `enforceAvailableModels` from the policy source (binary `ROn`).
///
/// `warn` receives each byte-exact warning string once (the caller is
/// responsible for de-duplication, mirroring the binary's module-level `SN`
/// set). Provenance rules:
/// - policy load failed ⇒ [`ModelEnforcement::Refused`] (fail-closed).
/// - enforce flag set but no policy `availableModels` ⇒ warn + inactive.
/// - enforce not `true`, or no/empty policy allowlist ⇒ inactive.
/// - otherwise ⇒ active with the policy-owned allowlist + overrides.
#[must_use]
pub fn resolve_enforcement(source: &PolicySource, warn: &mut dyn FnMut(&str)) -> ModelEnforcement {
    let view = match source {
        PolicySource::Failed => {
            warn(warnings::POLICY_READ_FAILED_PREFIX);
            return ModelEnforcement::Refused;
        }
        PolicySource::Loaded(v) => v,
    };

    // Enforce flag present-and-true but no policy-owned allowlist ⇒ inert.
    if view.enforce == Some(true) && view.available_models.is_none() {
        warn(warnings::ENFORCE_WITHOUT_ALLOWLIST);
        return ModelEnforcement::Inactive;
    }

    match &view.available_models {
        Some(list) if view.enforce == Some(true) && !list.is_empty() => ModelEnforcement::Active {
            allowlist: list.clone(),
            overrides: view.model_overrides.clone().unwrap_or_default(),
        },
        // enforce not true, or allowlist absent/empty ⇒ no restriction.
        _ => ModelEnforcement::Inactive,
    }
}

/// Gate a single model through resolved enforcement (binary `P4`): `Some(false)`
/// = refused/blocked, `None` = no enforcement, `Some(true)` = allowed.
#[must_use]
pub fn model_allowed_under(enforcement: &ModelEnforcement, model: &str) -> Option<bool> {
    match enforcement {
        ModelEnforcement::Refused => Some(false),
        ModelEnforcement::Inactive => None,
        ModelEnforcement::Active {
            allowlist,
            overrides,
        } => Some(is_model_allowed(model, Some(allowlist), Some(overrides))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sl(model: &str, allow: &[&str]) -> bool {
        let a: Vec<String> = allow.iter().map(|s| (*s).to_string()).collect();
        is_model_allowed(model, Some(&a), None)
    }

    #[test]
    fn undefined_allowlist_permits_everything() {
        assert!(is_model_allowed("claude-opus-4-5-20990101", None, None));
        assert!(is_model_allowed("literally-anything", None, None));
    }

    #[test]
    fn empty_allowlist_permits_nothing() {
        assert!(!sl("claude-opus-4-5", &[]));
        assert!(!sl("opus", &[]));
    }

    #[test]
    fn full_model_id_exact_match() {
        assert!(sl(
            "claude-opus-4-5-20990101",
            &["claude-opus-4-5-20990101"]
        ));
        assert!(!sl(
            "claude-sonnet-4-5-20990101",
            &["claude-opus-4-5-20990101"]
        ));
    }

    #[test]
    fn family_alias_matches_any_version() {
        // "opus" allows any opus version (describe text).
        assert!(sl("claude-opus-4-5-20990101", &["opus"]));
        assert!(sl("claude-opus-4-1-20250805", &["opus"]));
        // but not a different family.
        assert!(!sl("claude-sonnet-4-5-20990101", &["opus"]));
        assert!(!sl("claude-haiku-4-5", &["opus"]));
    }

    #[test]
    fn family_alias_word_boundary_only() {
        // "opus" must match at a token boundary — no substring false-positives.
        assert!(!sl("claude-magnumopusish-1", &["opus"]));
        assert!(sl("claude-opus-4-5", &["opus"]));
    }

    #[test]
    fn version_prefix_matches_that_version_only() {
        // "opus-4-5" allows only that version (describe text). Bare + claude- form.
        assert!(sl("claude-opus-4-5-20990101", &["opus-4-5"]));
        assert!(sl("opus-4-5-20990101", &["opus-4-5"]));
        assert!(sl("claude-opus-4-5-20990101", &["claude-opus-4-5"]));
        // a different version is NOT matched by the prefix.
        assert!(!sl("claude-opus-4-1-20250805", &["opus-4-5"]));
        // and the prefix is boundary-anchored: opus-4-50 must not match opus-4-5.
        assert!(!sl("claude-opus-4-50-x", &["opus-4-5"]));
    }

    #[test]
    fn narrower_version_entry_suppresses_broad_family_entry() {
        // Admin lists BOTH "opus" and "opus-4-5": the family entry is suppressed
        // (binary z5l guard) so a different opus version is NOT allowed via the
        // broad family entry.
        assert!(sl("claude-opus-4-5-20990101", &["opus", "opus-4-5"]));
        assert!(!sl("claude-opus-4-1-20250805", &["opus", "opus-4-5"]));
    }

    #[test]
    fn case_and_whitespace_normalized() {
        assert!(sl("  Claude-OPUS-4-5-20990101  ", &["  OPUS  "]));
        assert!(sl("CLAUDE-OPUS-4-5", &["Claude-Opus-4-5"]));
    }

    #[test]
    fn one_m_suffix_stripped() {
        // The `[1m]` long-context suffix is stripped before comparison.
        assert!(sl("claude-opus-4-5-20990101[1m]", &["opus"]));
        assert!(sl("claude-opus-4-5[1M]", &["claude-opus-4-5"]));
        assert!(sl("opus[1m]", &["opus"]));
    }

    #[test]
    fn model_overrides_reverse_maps_provider_id() {
        // Admin allowlists the Anthropic id; the fleet runs a Bedrock ARN. The
        // ARN reverse-maps to the Anthropic id before matching.
        let mut ov = BTreeMap::new();
        ov.insert(
            "claude-opus-4-5".to_string(),
            "arn:aws:bedrock:us-east-1::inference-profile/opus".to_string(),
        );
        let allow = vec!["claude-opus-4-5".to_string()];
        assert!(is_model_allowed(
            "arn:aws:bedrock:us-east-1::inference-profile/opus",
            Some(&allow),
            Some(&ov)
        ));
        // Without the override map, the raw ARN does not match.
        assert!(!is_model_allowed(
            "arn:aws:bedrock:us-east-1::inference-profile/opus",
            Some(&allow),
            None
        ));
    }

    #[test]
    fn first_allowed_model_picks_first_entry_in_order() {
        let allow = vec!["haiku".to_string(), "opus".to_string()];
        let catalog = vec![
            "claude-opus-4-5-20990101".to_string(),
            "claude-haiku-4-5-20990101".to_string(),
            "claude-sonnet-4-5-20990101".to_string(),
        ];
        // "haiku" is the first allowlist entry ⇒ resolves to the haiku catalog id.
        assert_eq!(
            first_allowed_model(&allow, &catalog, None).as_deref(),
            Some("claude-haiku-4-5-20990101")
        );
    }

    #[test]
    fn first_allowed_model_none_when_nothing_matches() {
        let allow = vec!["opus".to_string()];
        let catalog = vec!["gpt-5.5".to_string(), "gemini-2.0".to_string()];
        assert_eq!(first_allowed_model(&allow, &catalog, None), None);
    }

    // ── enforceAvailableModels policy-provenance ──────────────────────────

    fn resolve(view: PolicyModelView) -> (ModelEnforcement, Vec<String>) {
        let mut msgs = Vec::new();
        let out = resolve_enforcement(&PolicySource::Loaded(view), &mut |m| {
            msgs.push(m.to_string())
        });
        (out, msgs)
    }

    #[test]
    fn enforce_requires_policy_owned_allowlist() {
        // Flag true but no policy availableModels ⇒ inactive + byte-exact warn.
        let (state, msgs) = resolve(PolicyModelView {
            available_models: None,
            enforce: Some(true),
            model_overrides: None,
        });
        assert_eq!(state, ModelEnforcement::Inactive);
        assert_eq!(msgs, vec![warnings::ENFORCE_WITHOUT_ALLOWLIST.to_string()]);
    }

    #[test]
    fn enforce_active_with_policy_allowlist() {
        let (state, msgs) = resolve(PolicyModelView {
            available_models: Some(vec!["opus".to_string()]),
            enforce: Some(true),
            model_overrides: None,
        });
        assert_eq!(
            state,
            ModelEnforcement::Active {
                allowlist: vec!["opus".to_string()],
                overrides: BTreeMap::new(),
            }
        );
        assert!(msgs.is_empty());
    }

    #[test]
    fn enforce_inactive_when_flag_absent() {
        // Allowlist present but enforce not true ⇒ inactive (no restriction).
        let (state, _) = resolve(PolicyModelView {
            available_models: Some(vec!["opus".to_string()]),
            enforce: None,
            model_overrides: None,
        });
        assert_eq!(state, ModelEnforcement::Inactive);
    }

    #[test]
    fn enforce_inactive_when_allowlist_empty() {
        // "Has no effect when availableModels is unset or an empty array."
        let (state, _) = resolve(PolicyModelView {
            available_models: Some(vec![]),
            enforce: Some(true),
            model_overrides: None,
        });
        assert_eq!(state, ModelEnforcement::Inactive);
    }

    #[test]
    fn policy_load_failure_refuses_cascade_trust() {
        let mut msgs = Vec::new();
        let out = resolve_enforcement(&PolicySource::Failed, &mut |m| msgs.push(m.to_string()));
        assert_eq!(out, ModelEnforcement::Refused);
        assert_eq!(msgs, vec![warnings::POLICY_READ_FAILED_PREFIX.to_string()]);
    }

    #[test]
    fn model_allowed_under_gate() {
        let active = ModelEnforcement::Active {
            allowlist: vec!["opus".to_string()],
            overrides: BTreeMap::new(),
        };
        assert_eq!(
            model_allowed_under(&active, "claude-opus-4-5-20990101"),
            Some(true)
        );
        assert_eq!(
            model_allowed_under(&active, "claude-sonnet-4-5"),
            Some(false)
        );
        assert_eq!(model_allowed_under(&ModelEnforcement::Inactive, "x"), None);
        assert_eq!(
            model_allowed_under(&ModelEnforcement::Refused, "claude-opus-4-5"),
            Some(false)
        );
    }
}
