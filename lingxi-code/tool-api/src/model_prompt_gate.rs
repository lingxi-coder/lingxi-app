//! The `Dh(model)` "simple system prompt" gate, shared by every model-gated
//! tool prompt.
//!
//! claude-code builds several tool prompts as `fn(model){ if(Dh(model)) return
//! SHORT; return LONG }` — the current-generation default models
//! (`claude-opus-4-8` / `claude-fable-5` / `claude-mythos-5`) get the terse
//! SHORT variant, classic models (sonnet/haiku/`claude-3-*`/`opus-4-0..4-7`)
//! get the verbose LONG one. The file tools (Read/Write/Edit/Glob/Grep) and the
//! task tool (TodoWrite) all consult the SAME predicate, so it lives here as a
//! single source of truth rather than being re-derived per crate.
//!
//! Ports `Dh` (binary offset ~195159752), `UWu` (~195159370), and `dfe`.

/// Port of claude-code `dfe(e)` (binary): `/-eap($|\[)/i.test(e)`. Matches an
/// early-access-program model id where `-eap` is at the end of the string or
/// immediately followed by `[`.
fn is_early_access_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(idx) = lower[from..].find("-eap") {
        let abs = from + idx;
        let after = abs + "-eap".len();
        match lower.as_bytes().get(after) {
            None => return true,        // `-eap` at end ($)
            Some(&b'[') => return true, // `-eap[`
            _ => from = after,
        }
    }
    false
}

/// Port of claude-code `UWu(model)` (binary offset 195159370): the
/// "uses the standard/long system prompt" model-class predicate.
///
/// ```js
/// function UWu(e){
///   if(dfe(e)) return false;                       // `-eap` (early-access) → false
///   let t = Fo(e);                                 // normalize (strip region/profile)
///   if(t.includes("claude-3-") || t.includes("haiku") || t.includes("sonnet")
///      || t==="claude-opus-4-0" || t==="claude-opus-4-1" || t==="claude-opus-4-5"
///      || t==="claude-opus-4-6" || t==="claude-opus-4-7") return true;
///   if(t==="claude-opus-4-8" || t==="claude-fable-5" || t==="claude-mythos-5") return false;
///   return !pd();                                  // unknown → !first-party
/// }
/// ```
///
/// `Fo` (binary offset ~ `function Fo(`) resolves application-inference-profiles
/// and strips Bedrock region prefixes. LingXi has no `Fo` port, so we match the
/// substring/equality checks on the raw model id after a light lowercasing —
/// for every published Anthropic id this is identical to `Fo(e)` (the id is
/// already canonical; region/profile-wrapped ids still contain the same
/// `claude-3-`/`haiku`/`sonnet` substrings). The `dfe` early-access check
/// (`/-eap($|\[)/i`) is reproduced verbatim. The unknown-model fallthrough
/// `!pd()` (NOT first-party/anthropicAws/gateway) is approximated as `false`
/// here because the provider class is not threaded into the tool layer; for the
/// default first-party deployment `pd()` is `true` so `!pd()` is `false`,
/// matching this default. This only affects genuinely unknown model ids on a
/// non-first-party provider — documented residual.
fn uwu_standard_model(model: &str) -> bool {
    // `dfe(e)` = `/-eap($|\[)/i.test(e)` — early-access models are NOT standard.
    if is_early_access_model(model) {
        return false;
    }
    let t = model.to_ascii_lowercase();
    if t.contains("claude-3-")
        || t.contains("haiku")
        || t.contains("sonnet")
        || t == "claude-opus-4-0"
        || t == "claude-opus-4-1"
        || t == "claude-opus-4-5"
        || t == "claude-opus-4-6"
        || t == "claude-opus-4-7"
    {
        return true;
    }
    // Oracle `oug()` @228079752 asks the CAPABILITY REGISTRY first:
    //   `if (LN(t,"lean_prompt") || t === "claude-mythos-5") return false;`
    // This used to be a hardcoded name list (`claude-opus-4-8 | claude-fable-5
    // | claude-mythos-5`). That list omitted `claude-opus-5`, which reached the
    // right answer only by falling through to the default below — correct by
    // accident, and silently wrong for any model added later. `mythos-5` stays
    // a NAME check because the oracle keeps it one: it carries no capabilities
    // at all yet must still take this branch.
    if traits::model_capabilities::has_capability(
        &t,
        traits::model_capabilities::ModelCapability::LeanPrompt,
    ) || t == "claude-mythos-5"
    {
        return false;
    }
    // `return !pd()` — provider class unavailable in the tool layer; default
    // first-party deployment ⇒ `pd()` true ⇒ `!pd()` false.
    false
}

/// Port of claude-code `Dh(model)` (binary offset 195159752), the
/// "simple system prompt" gate:
///
/// ```js
/// Dh = wn((e)=>{
///   if(!e) return false;                                            // no model → false
///   if(st(process.env.LINGXI_SIMPLE_SYSTEM_PROMPT)) return true;   // env-truthy → simple
///   if(_l(process.env.LINGXI_SIMPLE_SYSTEM_PROMPT)) return false;  // env-defined-falsy → standard
///   return !UWu(e) || FWu(e);
/// });
/// ```
///
/// `FWu(model)` (binary offset 195159014) consults `clientDataCache
/// .simple_system_prompt` and the `tengu_velvet_cascade` config flag — neither
/// is plumbed into the tool layer. On a fresh/default config both are absent so
/// `FWu` returns `false`, reducing the model branch to `!UWu(model)`. This is
/// the dominant, parity-faithful behavior; a config that force-enables the
/// simple prompt for a specific model via those keys is a documented residual.
///
/// Returns `true` when the SHORT prompt variant should be served, `false` for
/// the LONG one. `None` (no model known) mirrors the JS `Dh(undefined)` path and
/// returns `false`.
#[must_use]
pub fn dh_simple_system_prompt(model: Option<&str>) -> bool {
    // `if(!e) return false` — the gate short-circuits to the long prompt when
    // no model is known (the JS `Dh(undefined)` path).
    let Some(model) = model.filter(|m| !m.is_empty()) else {
        return false;
    };
    // LingXi's multi-provider harness is intentionally more explicit than the
    // model-specific Claude prompt. A non-Claude model must never inherit the
    // short prompt from Claude's unknown-model fallthrough.
    if traits::model_capabilities::prompt_profile_for(model)
        == traits::model_capabilities::PromptProfile::FullHarness
    {
        return false;
    }
    let env = std::env::var("LINGXI_SIMPLE_SYSTEM_PROMPT").ok();
    if traits::env::is_env_truthy(env.as_deref()) {
        return true;
    }
    if traits::env::is_env_defined_falsy(env.as_deref()) {
        return false;
    }
    // `return !UWu(e) || FWu(e)` with `FWu(e) == false` (config keys absent).
    !uwu_standard_model(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_long() {
        assert!(!dh_simple_system_prompt(None));
        assert!(!dh_simple_system_prompt(Some("")));
    }

    /// `claude-opus-5` is SHORT because it carries `lean_prompt` in the
    /// capability registry — not because it fell off the end of a name list.
    /// Before the registry it was absent from both branches and reached the
    /// right answer by accident.
    #[test]
    fn opus_5_is_short_via_the_capability_registry() {
        assert!(dh_simple_system_prompt(Some("claude-opus-5")));
        assert!(traits::model_capabilities::has_capability(
            "claude-opus-5",
            traits::model_capabilities::ModelCapability::LeanPrompt
        ));
    }

    /// Every model the oracle marks `lean_prompt` takes the SHORT prompt, and
    /// the pre-lean Opus models still take the LONG one.
    #[test]
    fn lean_prompt_models_are_short_and_older_opus_is_long() {
        for m in ["claude-opus-4-8", "claude-opus-5", "claude-fable-5"] {
            assert!(dh_simple_system_prompt(Some(m)), "{m} must be SHORT");
        }
        for m in ["claude-opus-4-5", "claude-opus-4-6", "claude-opus-4-7"] {
            assert!(!dh_simple_system_prompt(Some(m)), "{m} must be LONG");
        }
    }

    #[test]
    fn new_models_are_short() {
        assert!(dh_simple_system_prompt(Some("claude-opus-4-8")));
        assert!(dh_simple_system_prompt(Some("claude-fable-5")));
        assert!(dh_simple_system_prompt(Some("claude-mythos-5")));
    }

    #[test]
    fn non_claude_models_keep_long_tool_prompts() {
        for model in [
            "gpt-5.5",
            "deepseek-v4-flash",
            "gemini-3.5-flash",
            "glm-5.1",
        ] {
            assert!(
                !dh_simple_system_prompt(Some(model)),
                "{model} must keep the full harness/tool prompt"
            );
        }
    }

    #[test]
    fn classic_models_are_long() {
        for m in [
            "claude-3-5-sonnet-20241022",
            "claude-3-haiku-20240307",
            "claude-sonnet-4-5",
            // Sonnet 5 (2.1.198 `LBd`): no "lean_prompt" capability in the
            // registry, so it falls into the `includes("sonnet")` standard arm
            // → LONG prompt (unlike opus-4-8 / fable-5 / mythos-5).
            "claude-sonnet-5",
            "claude-opus-4-0",
            "claude-opus-4-1",
            "claude-opus-4-5",
            "claude-opus-4-6",
            "claude-opus-4-7",
        ] {
            assert!(!dh_simple_system_prompt(Some(m)), "{m} should be LONG");
        }
    }

    #[test]
    fn early_access_is_long() {
        // `-eap` ⇒ UWu=false ⇒ Dh would be true, BUT only because dfe forces
        // UWu false; opus-4-8-eap is genuinely early-access so still SHORT.
        // The dfe carve-out matters for ids that would otherwise be standard.
        assert!(is_early_access_model("claude-sonnet-4-5-eap"));
        assert!(is_early_access_model("claude-opus-4-7-eap[foo]"));
        assert!(!is_early_access_model("claude-opus-4-8"));
    }

    #[test]
    fn anthropic_aws_alias_targets_are_long() {
        // M2 Part B pin: the 2.1.198 alias table's anthropic_aws per_provider
        // targets — sonnet → "claude-sonnet-4-6", opus → "claude-opus-4-7"
        // (verified against the real binary registry) — are both UWu-standard
        // models, so an anthropicAws-routed session keeps the LONG prompt.
        assert!(!dh_simple_system_prompt(Some("claude-sonnet-4-6")));
        assert!(!dh_simple_system_prompt(Some("claude-opus-4-7")));
    }
}
