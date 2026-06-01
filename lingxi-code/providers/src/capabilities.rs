//! Per-provider capability descriptor used to gate features and degrade
//! gracefully ("omit, never invent").

/// How a provider exposes reasoning / thinking traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningSupport {
    /// No reasoning trace.
    None,
    /// Reasoning is decoded from responses into the canonical `Thinking`
    /// block, but never re-encoded into request history.
    DecodeOnly,
}

/// Where a provider expects the system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemStyle {
    /// A dedicated top-level field (Anthropic `system`, Gemini
    /// `systemInstruction`).
    TopLevel,
    /// A leading role message (`OpenAI` `system`/`developer`).
    RoleMessage,
}

/// What a provider can do. Codecs consult this to gate tools and features.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Capabilities {
    /// Native function-calling support.
    pub native_tools: bool,
    /// Streaming (SSE) support.
    pub streaming: bool,
    /// Image input support.
    pub vision: bool,
    /// Prompt-caching support.
    pub prompt_cache: bool,
    /// Reasoning-trace support.
    pub reasoning: ReasoningSupport,
    /// Whether multiple tool calls can be returned in one turn.
    pub parallel_tool_calls: bool,
    /// Maximum output tokens the provider accepts, if known.
    pub max_output_tokens: Option<u32>,
    /// Where the system prompt goes.
    pub system_style: SystemStyle,
}

impl Capabilities {
    /// Capabilities for the Anthropic provider (everything native).
    #[must_use]
    pub fn anthropic() -> Self {
        Self {
            native_tools: true,
            streaming: true,
            vision: true,
            prompt_cache: true,
            reasoning: ReasoningSupport::DecodeOnly,
            parallel_tool_calls: true,
            max_output_tokens: None,
            system_style: SystemStyle::TopLevel,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_caps_are_fully_native() {
        let c = Capabilities::anthropic();
        assert!(c.native_tools);
        assert!(c.streaming);
        assert_eq!(c.system_style, SystemStyle::TopLevel);
    }
}
