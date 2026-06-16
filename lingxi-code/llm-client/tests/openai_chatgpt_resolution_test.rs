//! Regression: the openai-chatgpt (Codex-backend) model ids must resolve
//! UNAMBIGUOUSLY even though the `openai` (api.openai.com) preset is also present.
//!
//! Originally the hand-authored openai-chatgpt slice reused ids
//! (gpt-5-codex / gpt-5.3-codex) that also lived in the vendored `openai` AND
//! `github-copilot` slices, so `ModelRegistry::resolve` returned "ambiguous
//! across profiles" and the ChatGPT-login backend was unreachable. Fix: the
//! codex-backend-EXCLUSIVE ids (gpt-5-codex, gpt-5.3-codex) live ONLY in
//! openai-chatgpt — dropped from both openai and github-copilot. This test pins
//! that, using the REAL builtin catalog (all presets coexisting, as the engine
//! builds it).
//!
//! NOTE: genuinely-shared standard ids (e.g. gpt-5.2 on both openai and
//! github-copilot) remain intentionally ambiguous under bare-id resolution —
//! that is a separate, pre-existing multi-provider concern (the picker
//! disambiguates by profile), not part of codex-backend reachability.

use llm_client::{builtin_presets, ClientConfig, ModelRegistry};

fn registry() -> ModelRegistry {
    let cat = builtin_presets();
    ModelRegistry::from_config(ClientConfig { providers: cat.providers })
        .expect("registry from builtin presets")
}

#[test]
fn codex_models_resolve_to_openai_chatgpt_unambiguously() {
    let reg = registry();
    for id in ["gpt-5.3-codex", "gpt-5-codex"] {
        let route = reg
            .resolve(id)
            .unwrap_or_else(|e| panic!("'{id}' must resolve, got: {e:?}"));
        assert_eq!(
            route.profile_name, "openai-chatgpt",
            "'{id}' must route to the Codex-backend (ChatGPT-login) profile"
        );
    }
}

/// Codex-exclusive ids must NOT appear in openai or github-copilot anymore, so
/// the only owner is openai-chatgpt (the assertion above). This guards the
/// de-collision at the data layer too — a future re-vendor that reintroduces a
/// codex id into another slice would resurface the ambiguity.
#[test]
fn codex_ids_owned_solely_by_openai_chatgpt() {
    let cat = builtin_presets();
    for id in ["gpt-5-codex", "gpt-5.3-codex"] {
        let owners: Vec<&str> = cat
            .providers
            .iter()
            .filter(|p| p.models.iter().any(|m| m.request_model == id))
            .map(|p| p.profile_name.as_str())
            .collect();
        assert_eq!(owners, ["openai-chatgpt"], "'{id}' must be owned solely by openai-chatgpt");
    }
}
