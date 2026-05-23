//! `anthropic-beta` header constants + per-provider × per-endpoint assembler.
//!
//! **Source of truth**: `claude-code/src/constants/betas.ts` at upstream
//! commit `6a25909` (2026-05-23). Every constant in this file is locked
//! byte-for-byte against that reference; if any literal drifts, the parity
//! fixture `betas.json` MUST be updated in the same commit (the v0.4.0
//! pre-commit hook will enforce; out of scope for this file).
//!
//! Declaration order matches the upstream TypeScript file. The assembler
//! emits constants in **declaration order** so the comma-joined string is
//! deterministic across runs and platforms.

#![forbid(unsafe_code)]

// ---- 16 beta constants (spec §7 lines 676-700) ------------------------------

/// Core claude-code feature gate.
pub const CLAUDE_CODE_BETA: &str = "claude-code-20250219";
/// Interleaved-thinking output (Anthropic-specific).
pub const INTERLEAVED_THINKING: &str = "interleaved-thinking-2025-05-14";
/// 1M-token context window.
pub const CONTEXT_1M: &str = "context-1m-2025-08-07";
/// Context-management endpoint (cache write-points, etc.).
pub const CONTEXT_MANAGEMENT: &str = "context-management-2025-06-27";
/// Structured-output JSON-schema mode.
pub const STRUCTURED_OUTPUTS: &str = "structured-outputs-2025-12-15";
/// First-party web-search tool gating.
pub const WEB_SEARCH: &str = "web-search-2025-03-05";
/// Advanced tool-use semantics (Anthropic / Foundry).
pub const ADVANCED_TOOL_USE_1P: &str = "advanced-tool-use-2025-11-20";
/// Tool-search tool (Vertex / Bedrock).
pub const TOOL_SEARCH_TOOL_3P: &str = "tool-search-tool-2025-10-19";
/// Effort hint header.
pub const EFFORT: &str = "effort-2025-11-24";
/// Per-task token budget enforcement.
pub const TASK_BUDGETS: &str = "task-budgets-2026-03-13";
/// Prompt-caching scope control.
pub const PROMPT_CACHING_SCOPE: &str = "prompt-caching-scope-2026-01-05";
/// Fast-mode (lower-latency, smaller-batch).
pub const FAST_MODE: &str = "fast-mode-2026-02-01";
/// Redact thinking-block output.
pub const REDACT_THINKING: &str = "redact-thinking-2026-02-12";
/// Token-efficient tool encoding.
pub const TOKEN_EFFICIENT_TOOLS: &str = "token-efficient-tools-2026-03-28";
/// Advisor tool integration.
pub const ADVISOR_TOOL: &str = "advisor-tool-2026-03-01";
/// OAuth bearer-token auth on the messages endpoint.
pub const OAUTH: &str = "oauth-2025-04-20";

// ---- Per-provider / per-endpoint policy whitelists --------------------------

/// Vertex `count_tokens` allows ONLY these three (per `claude-code/src/constants/betas.ts`).
pub const VERTEX_COUNT_TOKENS_ALLOWED: &[&str] =
    &[CLAUDE_CODE_BETA, INTERLEAVED_THINKING, CONTEXT_MANAGEMENT];

/// Bedrock requires these to ride in `extraBodyParams`, NOT the header.
pub const BEDROCK_EXTRA_PARAMS_HEADERS: &[&str] =
    &[INTERLEAVED_THINKING, CONTEXT_1M, TOOL_SEARCH_TOOL_3P];

// ---- Provider × Endpoint matrix --------------------------------------------

/// API provider routing target. Determines which subset of `anthropic-beta`
/// entries ship in the header (the remainder either don't apply or move to
/// `extraBodyParams` per `BEDROCK_EXTRA_PARAMS_HEADERS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Direct Anthropic API (api.anthropic.com).
    Anthropic,
    /// Google Cloud Vertex AI Anthropic models.
    Vertex,
    /// AWS Bedrock Anthropic models.
    Bedrock,
}

/// Endpoint kind. Different endpoints accept different beta subsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// Non-streaming `POST /v1/messages`.
    MessagesCreate,
    /// Streaming `POST /v1/messages` (`stream: true`).
    MessagesCreateStream,
    /// `POST /v1/messages/count_tokens`.
    CountTokens,
}

/// Build the `anthropic-beta` header value for the given provider + endpoint.
///
/// Constants are emitted in declaration order (which mirrors
/// `claude-code/src/constants/betas.ts`) and comma-joined with no spaces.
///
/// **Vertex × `CountTokens`** is a special case: only the three entries in
/// `VERTEX_COUNT_TOKENS_ALLOWED` may appear; everything else is excluded.
///
/// **Bedrock** moves `BEDROCK_EXTRA_PARAMS_HEADERS` from header to body;
/// they are excluded from the header here.
#[must_use]
pub fn assemble_beta_header(provider: Provider, endpoint: Endpoint) -> String {
    // Declaration-order list of every constant (matches betas.ts top-to-bottom):
    let all = [
        (CLAUDE_CODE_BETA, applies_default(endpoint)),
        (INTERLEAVED_THINKING, applies_default(endpoint)),
        (CONTEXT_1M, applies_messages_only(endpoint)),
        (CONTEXT_MANAGEMENT, applies_default(endpoint)),
        (STRUCTURED_OUTPUTS, applies_messages_only(endpoint)),
        (WEB_SEARCH, applies_messages_only(endpoint)),
        (ADVANCED_TOOL_USE_1P, applies_messages_only(endpoint)),
        (TOOL_SEARCH_TOOL_3P, applies_messages_only(endpoint)),
        (EFFORT, applies_messages_only(endpoint)),
        (TASK_BUDGETS, applies_messages_only(endpoint)),
        (PROMPT_CACHING_SCOPE, applies_messages_only(endpoint)),
        (FAST_MODE, applies_messages_only(endpoint)),
        (REDACT_THINKING, applies_messages_only(endpoint)),
        (TOKEN_EFFICIENT_TOOLS, applies_messages_only(endpoint)),
        (ADVISOR_TOOL, applies_messages_only(endpoint)),
        // OAUTH rides only on the token-refresh POST in M3-04; never on
        // messages.create.
        (OAUTH, false),
    ];

    let mut parts: Vec<&str> = Vec::new();
    for (name, endpoint_ok) in all {
        if !endpoint_ok {
            continue;
        }

        // Per-provider exclusions:
        let allow = match provider {
            Provider::Anthropic => {
                // Anthropic 1P does NOT carry the 3P-only TOOL_SEARCH_TOOL_3P.
                name != TOOL_SEARCH_TOOL_3P
            }
            Provider::Vertex => {
                if matches!(endpoint, Endpoint::CountTokens) {
                    VERTEX_COUNT_TOKENS_ALLOWED.contains(&name)
                } else {
                    // Vertex does NOT carry the 1P-only ADVANCED_TOOL_USE_1P.
                    name != ADVANCED_TOOL_USE_1P
                }
            }
            Provider::Bedrock => {
                // Bedrock moves these to extraBodyParams; never to the header.
                !BEDROCK_EXTRA_PARAMS_HEADERS.contains(&name)
                    // Bedrock also does NOT carry 1P-only ADVANCED_TOOL_USE_1P.
                    && name != ADVANCED_TOOL_USE_1P
            }
        };
        if allow {
            parts.push(name);
        }
    }
    parts.join(",")
}

/// Entries that apply to every endpoint variant.
fn applies_default(_endpoint: Endpoint) -> bool {
    true
}

/// Entries that apply only to messages.create (non-stream + stream).
fn applies_messages_only(endpoint: Endpoint) -> bool {
    matches!(
        endpoint,
        Endpoint::MessagesCreate | Endpoint::MessagesCreateStream
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Lock the 16 constants byte-for-byte against spec §7 lines 676-700.
    #[test]
    fn beta_constants_match_spec_byte_for_byte() {
        assert_eq!(CLAUDE_CODE_BETA, "claude-code-20250219");
        assert_eq!(INTERLEAVED_THINKING, "interleaved-thinking-2025-05-14");
        assert_eq!(CONTEXT_1M, "context-1m-2025-08-07");
        assert_eq!(CONTEXT_MANAGEMENT, "context-management-2025-06-27");
        assert_eq!(STRUCTURED_OUTPUTS, "structured-outputs-2025-12-15");
        assert_eq!(WEB_SEARCH, "web-search-2025-03-05");
        assert_eq!(ADVANCED_TOOL_USE_1P, "advanced-tool-use-2025-11-20");
        assert_eq!(TOOL_SEARCH_TOOL_3P, "tool-search-tool-2025-10-19");
        assert_eq!(EFFORT, "effort-2025-11-24");
        assert_eq!(TASK_BUDGETS, "task-budgets-2026-03-13");
        assert_eq!(PROMPT_CACHING_SCOPE, "prompt-caching-scope-2026-01-05");
        assert_eq!(FAST_MODE, "fast-mode-2026-02-01");
        assert_eq!(REDACT_THINKING, "redact-thinking-2026-02-12");
        assert_eq!(TOKEN_EFFICIENT_TOOLS, "token-efficient-tools-2026-03-28");
        assert_eq!(ADVISOR_TOOL, "advisor-tool-2026-03-01");
        assert_eq!(OAUTH, "oauth-2025-04-20");
    }

    #[test]
    fn vertex_count_tokens_whitelist_is_exactly_three() {
        assert_eq!(VERTEX_COUNT_TOKENS_ALLOWED.len(), 3);
        assert!(VERTEX_COUNT_TOKENS_ALLOWED.contains(&CLAUDE_CODE_BETA));
        assert!(VERTEX_COUNT_TOKENS_ALLOWED.contains(&INTERLEAVED_THINKING));
        assert!(VERTEX_COUNT_TOKENS_ALLOWED.contains(&CONTEXT_MANAGEMENT));
    }

    #[test]
    fn bedrock_extra_params_headers_is_exactly_three() {
        assert_eq!(BEDROCK_EXTRA_PARAMS_HEADERS.len(), 3);
        assert!(BEDROCK_EXTRA_PARAMS_HEADERS.contains(&INTERLEAVED_THINKING));
        assert!(BEDROCK_EXTRA_PARAMS_HEADERS.contains(&CONTEXT_1M));
        assert!(BEDROCK_EXTRA_PARAMS_HEADERS.contains(&TOOL_SEARCH_TOOL_3P));
    }

    #[test]
    fn anthropic_messages_create_emits_full_set_minus_oauth_and_bedrock_exclusives() {
        // Anthropic (1P) provider emits everything applicable to messages.create.
        // OAuth header is not emitted on messages.create — it rides on token-refresh
        // POST only (M3-04). Bedrock-exclusive entries don't apply.
        let s = assemble_beta_header(Provider::Anthropic, Endpoint::MessagesCreate);
        // Declaration order — matches claude-code/src/constants/betas.ts:
        let expected = [
            CLAUDE_CODE_BETA,
            INTERLEAVED_THINKING,
            CONTEXT_1M,
            CONTEXT_MANAGEMENT,
            STRUCTURED_OUTPUTS,
            WEB_SEARCH,
            ADVANCED_TOOL_USE_1P,
            EFFORT,
            TASK_BUDGETS,
            PROMPT_CACHING_SCOPE,
            FAST_MODE,
            REDACT_THINKING,
            TOKEN_EFFICIENT_TOOLS,
            ADVISOR_TOOL,
        ]
        .join(",");
        assert_eq!(s, expected);
    }

    #[test]
    fn vertex_count_tokens_emits_only_whitelisted_three() {
        let s = assemble_beta_header(Provider::Vertex, Endpoint::CountTokens);
        let expected = [CLAUDE_CODE_BETA, INTERLEAVED_THINKING, CONTEXT_MANAGEMENT].join(",");
        assert_eq!(s, expected);
    }

    #[test]
    fn bedrock_messages_create_excludes_header_only_entries() {
        // Bedrock moves INTERLEAVED_THINKING / CONTEXT_1M / TOOL_SEARCH_TOOL_3P
        // to extraBodyParams, not the header.
        let s = assemble_beta_header(Provider::Bedrock, Endpoint::MessagesCreate);
        for excluded in BEDROCK_EXTRA_PARAMS_HEADERS {
            assert!(
                !s.split(',').any(|p| p == *excluded),
                "Bedrock header must NOT contain {excluded}; got: {s}",
            );
        }
        // TOOL_SEARCH_TOOL_3P is in BEDROCK_EXTRA_PARAMS_HEADERS, so it must
        // NOT appear in the Bedrock header (loop above already asserts this,
        // but make it explicit here for the 3P entry specifically).
        assert!(!s.split(',').any(|p| p == TOOL_SEARCH_TOOL_3P));
        // Sanity: at least one entry must be there.
        assert!(
            !s.is_empty(),
            "Bedrock messages.create must emit at least one beta"
        );
    }

    #[test]
    fn anthropic_count_tokens_header_is_short() {
        // count_tokens is a much narrower endpoint; Anthropic provider only
        // emits the entries declared `applies_to_count_tokens()`.
        let s = assemble_beta_header(Provider::Anthropic, Endpoint::CountTokens);
        // Whatever the exact subset, it must NOT carry OAuth or task-budgets etc.
        assert!(!s.split(',').any(|p| p == OAUTH));
        assert!(!s.split(',').any(|p| p == TASK_BUDGETS));
    }
}
