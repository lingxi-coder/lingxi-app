//! Per-profile availability: drives the `/model` picker's Connect badge.
//!
//! `available = keychain_has(id) || env_set(var) || anthropic key/oauth present`.
//! A sibling list keyed by `profile_name` (spec §8) so the frozen `ModelListing`
//! DTO stays untouched; the tui joins it by provider/profile name.

use std::sync::Arc;

use llm_client::ProviderId;

use crate::CredentialSource;

/// Availability of one provider profile for the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAvailability {
    /// Provider identity.
    pub provider_id: ProviderId,
    /// Human profile name (the engine map keys by this; spec §6.2).
    pub profile_name: String,
    /// Credential id this availability was computed for.
    pub credential_id: String,
    /// Whether a usable credential is present.
    pub available: bool,
}

/// Compute availability for every `CredentialSource`.
///
/// `anthropic_has_api_key` / `anthropic_has_oauth` reflect the engine's resolved
/// Anthropic auth state (the composite serves those without a keychain/env id).
/// `openai_chatgpt_available` mirrors that pattern for the ChatGPT credential —
/// the engine ORs PAT-env / external-tokens-env / OAuth-session and passes the
/// combined result here (OpenAI OAuth tokens are stored under `openai-oauth-*`
/// keychain accounts, not under a `get_provider_key("openai-chatgpt")` slot, so
/// the generic arm cannot detect any of the three sources).
pub async fn compute_availability(
    credentials: &Arc<secret::CredentialManager>,
    sources: &[CredentialSource],
    anthropic_has_api_key: bool,
    anthropic_has_oauth: bool,
    openai_chatgpt_available: bool,
) -> Vec<ProviderAvailability> {
    let mut out = Vec::with_capacity(sources.len());
    for source in sources {
        let available = match source.credential_id.as_str() {
            "anthropic-api-key" => anthropic_has_api_key,
            "anthropic-oauth" => anthropic_has_oauth,
            "openai-chatgpt" => openai_chatgpt_available,
            id => {
                let keychain_has = matches!(credentials.get_provider_key(id).await, Ok(Some(_)));
                let env_set = source
                    .env_var
                    .as_deref()
                    .is_some_and(|var| std::env::var(var).is_ok());
                keychain_has || env_set
            }
        };
        out.push(ProviderAvailability {
            provider_id: source.provider_id.clone(),
            profile_name: source.profile_name.clone(),
            credential_id: source.credential_id.clone(),
            available,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::ProviderId;
    use std::sync::Arc;

    #[derive(Default)]
    struct MemStorage {
        map: std::sync::Mutex<
            std::collections::HashMap<(String, String), protocol::SecureStorageData>,
        >,
    }
    #[async_trait::async_trait]
    impl traits::SecureStorage for MemStorage {
        async fn store(&self, service: &str, account: &str, data: protocol::SecureStorageData)
            -> Result<(), traits::SecureStorageError> {
            self.map.lock().unwrap().insert((service.into(), account.into()), data);
            Ok(())
        }
        async fn retrieve(&self, service: &str, account: &str)
            -> Result<Option<protocol::SecureStorageData>, traits::SecureStorageError> {
            Ok(self.map.lock().unwrap().get(&(service.into(), account.into())).cloned())
        }
        async fn delete(&self, service: &str, account: &str) -> Result<(), traits::SecureStorageError> {
            self.map.lock().unwrap().remove(&(service.into(), account.into()));
            Ok(())
        }
        async fn list(&self, service: &str) -> Result<Vec<String>, traits::SecureStorageError> {
            Ok(self.map.lock().unwrap().keys().filter(|(s, _)| s == service).map(|(_, a)| a.clone()).collect())
        }
        fn is_encrypted(&self) -> bool { false }
        fn backend(&self) -> traits::SecureStorageBackend { traits::SecureStorageBackend::PlainText }
    }
    struct FixedClock;
    impl traits::Clock for FixedClock {
        fn now(&self) -> std::time::SystemTime {
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000)
        }
    }
    struct NoHttp;
    #[async_trait::async_trait]
    impl traits::HttpTransport for NoHttp {
        async fn request(&self, _req: protocol::HttpRequest)
            -> Result<protocol::HttpResponse, traits::HttpError> {
            panic!("availability tests must not perform HTTP");
        }
        async fn stream_sse(&self, _req: protocol::HttpRequest)
            -> Result<traits::http::SseStream, traits::HttpError> {
            panic!("availability tests must not perform HTTP");
        }
    }
    fn manager() -> Arc<secret::CredentialManager> {
        Arc::new(secret::CredentialManager::new(
            Arc::new(MemStorage::default()),
            Arc::new(FixedClock),
            Arc::new(NoHttp),
        ))
    }
    fn src(name: &str, env: Option<&str>) -> crate::CredentialSource {
        crate::CredentialSource {
            provider_id: ProviderId::OpenAICompatible { name: name.to_string() },
            profile_name: name.to_string(),
            credential_id: name.to_string(),
            env_var: env.map(str::to_string),
            kind: crate::CredentialKind::ApiKey,
        }
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serialize env-var mutation across async tests
    async fn keychain_makes_available() {
        let _g = ENV_LOCK.lock().unwrap();
        let cm = manager();
        cm.set_provider_key("openrouter", "k").await.expect("store");
        let map = compute_availability(&cm, &[src("openrouter", Some("NOPE_VAR"))], false, false, false).await;
        let entry = map.iter().find(|a| a.profile_name == "openrouter").expect("entry");
        assert!(entry.available);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serialize env-var mutation across async tests
    async fn env_makes_available() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("DEEPSEEK_AVAIL_VAR", "k");
        let map = compute_availability(&manager(), &[src("deepseek", Some("DEEPSEEK_AVAIL_VAR"))], false, false, false).await;
        std::env::remove_var("DEEPSEEK_AVAIL_VAR");
        assert!(map.iter().find(|a| a.profile_name == "deepseek").unwrap().available);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serialize env-var mutation across async tests
    async fn neither_is_unavailable() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("GLM_AVAIL_VAR");
        let map = compute_availability(&manager(), &[src("glm", Some("GLM_AVAIL_VAR"))], false, false, false).await;
        assert!(!map.iter().find(|a| a.profile_name == "glm").unwrap().available);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serialize env-var mutation across async tests
    async fn anthropic_api_key_marks_anthropic_available() {
        let _g = ENV_LOCK.lock().unwrap();
        let anthropic = crate::CredentialSource {
            provider_id: ProviderId::AnthropicFirstParty,
            profile_name: "anthropic".to_string(),
            credential_id: "anthropic-api-key".to_string(),
            env_var: None,
            kind: crate::CredentialKind::ApiKey,
        };
        let map = compute_availability(&manager(), &[anthropic], true, false, false).await;
        assert!(map.iter().find(|a| a.profile_name == "anthropic").unwrap().available);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serialize env-var mutation across async tests
    async fn anthropic_oauth_marks_anthropic_available() {
        let _g = ENV_LOCK.lock().unwrap();
        let anthropic = crate::CredentialSource {
            provider_id: ProviderId::AnthropicFirstParty,
            profile_name: "anthropic".to_string(),
            credential_id: "anthropic-oauth".to_string(),
            env_var: None,
            kind: crate::CredentialKind::OAuth,
        };
        let map = compute_availability(&manager(), &[anthropic], false, true, false).await;
        assert!(map.iter().find(|a| a.profile_name == "anthropic").unwrap().available);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serialize env-var mutation across async tests
    async fn openai_chatgpt_available_when_oauth_session_present() {
        let _g = ENV_LOCK.lock().unwrap();
        let chatgpt = crate::CredentialSource {
            provider_id: ProviderId::OpenAICompatible { name: "openai-chatgpt".to_string() },
            profile_name: "openai-chatgpt".to_string(),
            credential_id: "openai-chatgpt".to_string(),
            env_var: None,
            kind: crate::CredentialKind::ApiKey,
        };
        // not available with the flag false …
        let map = compute_availability(&manager(), &[chatgpt.clone()], false, false, false).await;
        assert!(!map.iter().find(|a| a.profile_name == "openai-chatgpt").unwrap().available);
        // … available with the flag true (OAuth session present)
        let map = compute_availability(&manager(), &[chatgpt], false, false, true).await;
        assert!(map.iter().find(|a| a.profile_name == "openai-chatgpt").unwrap().available);
    }
}
