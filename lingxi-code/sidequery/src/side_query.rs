//! `SideQueryClient` trait and request/response DTOs.
//!
//! A side query is a stateless, one-shot LLM call: no tool loop, no state
//! machine slot, no shared cache prefix with the parent. The host owns
//! lifetimes and timeouts. Side queries power §6.3 memory selection,
//! §7.4 permission explanations, §6.5 session search, lightweight
//! classifiers, and prompt suggestions.
//!
//! For full-loop spawns that need to share the parent's prompt cache, use
//! [`crate::forked_agent::ForkedAgentRunner`] instead.

use crate::purposes::QuerySource;
use async_trait::async_trait;
use protocol::ConversationMessage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Parameters for one side-query LLM call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideQueryRequest {
    /// Model id (e.g. `claude-haiku-4-5`).
    pub model: String,
    /// Optional provider profile that owns `model`.
    ///
    /// Compaction inherits this from the live session so a model id shared by
    /// multiple providers is routed through the same provider as the parent
    /// turn instead of falling back to registry-first resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Optional system prompt; when `None` the provider default applies.
    pub system_prompt: Option<String>,
    /// Conversation messages to feed the model.
    pub messages: Vec<ConversationMessage>,
    /// Tool schemas exposed to the model (may be empty).
    pub tools: Vec<Value>,
    /// Tool-choice control (e.g. force a specific tool / json mode).
    pub tool_choice: Option<Value>,
    /// Structured-output schema (e.g. JSON Schema for `response_format`).
    pub output_format: Option<Value>,
    /// Output cap.
    pub max_tokens: u32,
    /// Caller-side retry budget for transient API errors.
    pub max_retries: u32,
    /// Sampling temperature; defaults to provider default when `None`.
    pub temperature: Option<f32>,
    /// Session thinking configuration to INHERIT on this call (cc 2.1.198
    /// "Subagents + compaction inherit extended thinking config"; binary: the
    /// compaction summarizer passes `thinkingConfig: mXt(r)` — the session
    /// `options.thinkingConfig` — @216945141). `None` = legacy: no `thinking`
    /// field on the wire (utility side queries — memory selection, WebFetch
    /// summarization — match the binary's explicit `{type:"disabled"}`
    /// callers). Replaces the never-forwarded `thinking_budget` knob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<llm_client::model::thinking::ThinkingConfig>,
    /// Stop sequences.
    pub stop_sequences: Vec<String>,
    /// COGS tag — see [`QuerySource`].
    pub query_source: QuerySource,
    /// Skip the engine-injected system-prompt prefix (e.g. LINGXI.md preamble).
    pub skip_system_prompt_prefix: bool,
}

/// Decoded outcome of one side-query call.
#[derive(Debug, Clone)]
pub struct SideQueryResponse {
    /// Best-effort flattened plain-text response when present.
    pub text: Option<String>,
    /// Structured-output payload when the model returned JSON.
    pub structured: Option<Value>,
    /// Tool calls emitted by the model (typically empty for side queries).
    pub tool_calls: Vec<Value>,
    /// Token / cost usage for COGS attribution.
    pub usage: cost::Usage,
    /// Stop-reason as reported by the provider (`end_turn`, `tool_use`, ...).
    pub stop_reason: Option<String>,
}

/// Side-query failure surface.
#[derive(Debug, Clone, Error)]
pub enum SideQueryError {
    /// Underlying API call failed (transport, 4xx/5xx, malformed stream).
    #[error(transparent)]
    Api(#[from] llm_client::LlmError),
    /// Provider returned a response we could not decode into a
    /// [`SideQueryResponse`].
    #[error("invalid response: {0}")]
    InvalidResponse(String),
}

/// One-shot LLM client. Implementations route to `AnthropicProvider` (M1)
/// and to other providers in Plan 04 follow-ups.
#[async_trait]
pub trait SideQueryClient: Send + Sync {
    /// Issue one side query and return the decoded response.
    async fn query(&self, request: SideQueryRequest) -> Result<SideQueryResponse, SideQueryError>;
}
