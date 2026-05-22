//! Per-turn and per-session usage records: token counts by class, server-side
//! tool counters, and an optional API speed selector.
//!
//! Values are pure counters — converting to money happens in
//! [`crate::calculator::CostCalculator`].

use crate::pricing::TokenClass;
use serde::{Deserialize, Serialize};

/// Token counters bucketed by [`TokenClass`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input (prompt) tokens.
    pub input: u64,
    /// Output (completion) tokens.
    pub output: u64,
    /// Tokens written into the prompt cache.
    pub cache_write: u64,
    /// Tokens read from the prompt cache.
    pub cache_read: u64,
    /// Reasoning / thinking output tokens.
    pub reasoning_output: u64,
}

/// Composite usage record: tokens plus optional non-token counters and
/// optional speed hint.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Per-class token counts.
    pub tokens: TokenUsage,
    /// Optional server-side tool counters (web search, etc.).
    pub server_tool_use: Option<ServerToolUsage>,
    /// Optional API speed selector.
    pub speed: Option<ApiSpeed>,
}

/// Counters for server-side tools that bill per-request rather than per-token.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ServerToolUsage {
    /// Number of server-side web search requests.
    pub web_search_requests: u32,
}

/// Hint about which API speed tier was used for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiSpeed {
    /// Standard-latency tier.
    Standard,
    /// Fast (priority / low-latency) tier.
    Fast,
}

impl Usage {
    /// Return the token count for the given [`TokenClass`].
    #[must_use]
    pub fn tokens_for(&self, class: TokenClass) -> u64 {
        match class {
            TokenClass::Input => self.tokens.input,
            TokenClass::Output => self.tokens.output,
            TokenClass::CacheWrite => self.tokens.cache_write,
            TokenClass::CacheRead => self.tokens.cache_read,
            TokenClass::ReasoningOutput => self.tokens.reasoning_output,
        }
    }

    /// Sum of all token classes.
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.tokens.input
            + self.tokens.output
            + self.tokens.cache_write
            + self.tokens.cache_read
            + self.tokens.reasoning_output
    }

    /// Accumulate `other` into `self`, merging token counts and server-tool
    /// counters component-wise.
    pub fn add(&mut self, other: &Usage) {
        self.tokens.input += other.tokens.input;
        self.tokens.output += other.tokens.output;
        self.tokens.cache_write += other.tokens.cache_write;
        self.tokens.cache_read += other.tokens.cache_read;
        self.tokens.reasoning_output += other.tokens.reasoning_output;
        if let Some(s) = other.server_tool_use {
            self.server_tool_use
                .get_or_insert(ServerToolUsage::default())
                .web_search_requests += s.web_search_requests;
        }
    }
}
