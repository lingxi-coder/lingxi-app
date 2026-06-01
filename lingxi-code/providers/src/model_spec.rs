//! Parse a model string into a `(profile, model)` pair.
//!
//! `"openai/gpt-4o"` → profile `openai`, model `gpt-4o`. A string with no `/`
//! (or any `claude-*` string, even if it contains a `/`) resolves to the
//! built-in `anthropic` profile, so every model string used before P2
//! routes unchanged.

/// The default profile name used for bare / `claude-*` model strings.
pub const DEFAULT_PROFILE: &str = "anthropic";

/// A parsed model selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    /// Provider profile name to resolve (e.g. `anthropic`, `openai`, `groq`).
    pub profile: String,
    /// Provider-local model id, with any `profile/` prefix stripped.
    pub model: String,
}

impl ModelSpec {
    /// Parse a model string. Back-compat: no `/`, or a `claude-*` model,
    /// maps to the [`DEFAULT_PROFILE`] (`anthropic`) with the full string as
    /// the model id.
    #[must_use]
    pub fn parse(input: &str) -> Self {
        if input.starts_with("claude-") {
            return Self {
                profile: DEFAULT_PROFILE.to_string(),
                model: input.to_string(),
            };
        }
        match input.split_once('/') {
            Some((profile, model)) if !profile.is_empty() && !model.is_empty() => Self {
                profile: profile.to_string(),
                model: model.to_string(),
            },
            _ => Self {
                profile: DEFAULT_PROFILE.to_string(),
                model: input.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixed_splits_profile_and_model() {
        let s = ModelSpec::parse("openai/gpt-4o");
        assert_eq!(s.profile, "openai");
        assert_eq!(s.model, "gpt-4o");
    }

    #[test]
    fn bare_string_is_anthropic_backcompat() {
        let s = ModelSpec::parse("claude-opus-4-7");
        assert_eq!(s.profile, "anthropic");
        assert_eq!(s.model, "claude-opus-4-7");
    }

    #[test]
    fn claude_with_slash_stays_anthropic() {
        // A claude model id is never reinterpreted as profile/model.
        let s = ModelSpec::parse("claude-3-5/sonnet");
        assert_eq!(s.profile, "anthropic");
        assert_eq!(s.model, "claude-3-5/sonnet");
    }

    #[test]
    fn non_claude_no_slash_is_anthropic_profile() {
        let s = ModelSpec::parse("some-model");
        assert_eq!(s.profile, "anthropic");
        assert_eq!(s.model, "some-model");
    }

    #[test]
    fn custom_profile_name() {
        let s = ModelSpec::parse("groq/llama-3.3-70b");
        assert_eq!(s.profile, "groq");
        assert_eq!(s.model, "llama-3.3-70b");
    }
}
