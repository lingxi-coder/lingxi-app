//! The provider-neutral request the bridge builds and hands to a provider.

use protocol::ConversationMessage;

/// Default `max_tokens` ceiling. Matches `api_client::AnthropicProvider`'s
/// hardcoded value so the Anthropic path is unchanged.
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

/// A provider-neutral completion request. Codecs translate this into each
/// provider's native wire shape; the Anthropic path consumes only `model`,
/// `system`, `messages`, and `tools`.
#[derive(Debug, Clone)]
pub struct CanonicalRequest {
    /// Provider-local model id (any `provider/` prefix already stripped — P2).
    pub model: String,
    /// Optional system prompt.
    pub system: Option<String>,
    /// Conversation history, oldest first.
    pub messages: Vec<ConversationMessage>,
    /// Canonical (Anthropic-shaped) tool schema declarations.
    pub tools: Vec<serde_json::Value>,
    /// Maximum output tokens.
    pub max_tokens: u32,
    /// Optional sampling temperature.
    pub temperature: Option<f32>,
    /// Whether this is a streaming request.
    pub stream: bool,
}

impl CanonicalRequest {
    /// Construct a request for `model` with empty history and defaults.
    #[must_use]
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system: None,
            messages: Vec::new(),
            tools: Vec::new(),
            max_tokens: DEFAULT_MAX_TOKENS,
            temperature: None,
            stream: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_defaults() {
        let r = CanonicalRequest::new("gpt-4o");
        assert_eq!(r.model, "gpt-4o");
        assert_eq!(r.max_tokens, DEFAULT_MAX_TOKENS);
        assert!(r.system.is_none());
        assert!(r.messages.is_empty());
        assert!(!r.stream);
    }
}
