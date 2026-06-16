//! Profile-qualified resolution: a shared id (gpt-5.2 on openai + github-copilot)
//! resolves by profile; unqualified stays ambiguous (decided behaviour).
use llm_client::{builtin_presets, ClientConfig, LlmError, ModelRegistry};

fn registry() -> ModelRegistry {
    ModelRegistry::from_config(ClientConfig { providers: builtin_presets().providers })
        .expect("registry")
}

#[test]
fn shared_id_resolves_by_profile() {
    let reg = registry();
    assert_eq!(reg.resolve_in("gpt-5.2", Some("openai")).expect("openai").profile_name, "openai");
    assert_eq!(reg.resolve_in("gpt-5.2", Some("github-copilot")).expect("copilot").profile_name, "github-copilot");
}

#[test]
fn unqualified_shared_id_still_ambiguous() {
    match registry().resolve_in("gpt-5.2", None) {
        Err(LlmError::InvalidRequest { message }) => assert!(message.contains("ambiguous")),
        other => panic!("expected ambiguous error, got {other:?}"),
    }
}

#[test]
fn qualified_absent_model_is_unavailable() {
    assert!(matches!(registry().resolve_in("gpt-5.2", Some("zai")), Err(LlmError::ModelUnavailable)));
    assert!(matches!(registry().resolve_in("gpt-5.2", Some("nope")), Err(LlmError::ModelUnavailable)));
}

#[test]
fn unique_unqualified_still_resolves() {
    assert_eq!(registry().resolve_in("gpt-4.1-mini", None).expect("unique").profile_name, "openai");
    assert_eq!(registry().resolve("gpt-4.1-mini").expect("unique").profile_name, "openai");
}
