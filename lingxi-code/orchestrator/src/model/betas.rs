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

// ---- beta constants (spec §7 lines 676-700; declaration order = betas.ts) ----
//
// Three of these — SUMMARIZE_CONNECTOR_TEXT / AFK_MODE / CLI_INTERNAL — are
// feature()/USER_TYPE-gated in betas.ts and resolve to `''` in the external
// default build. Their literals are reserved here byte-faithfully and ARE now
// wired into `assemble_beta_header` in TS declaration order, but behind
// emit-gates that are ALWAYS FALSE in the default external build (two compile-
// time `cfg!(feature = …)` flags that are default-off, and one runtime
// `USER_TYPE`/`CLAUDE_CODE_ENTRYPOINT` env check). The default header output is
// therefore byte-identical to claude-code (never over-emitted); the structure
// merely mirrors the TS file so a future build that flips a feature/env emits
// the entry in the correct declaration-order slot.
//
// Note: the `connector_text` and `transcript_classifier` Cargo features ARE
// declared (default-off) in `orchestrator`'s `[features]` section (mirroring
// `api-client`, where the betas module originated). The `cfg!(feature = …)`
// gates below therefore resolve identically in both crates: ALWAYS FALSE in
// the default external build, matching the external default exactly — constants
// are inert, same as external builds.

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
/// Connector-text summarization.
///
/// **Feature-gated / inert in the external build.** In `betas.ts` this is
/// `feature('CONNECTOR_TEXT') ? 'summarize-connector-text-2026-03-13' : ''`,
/// so the literal is only emitted when the `CONNECTOR_TEXT` build feature is
/// on; the external default build resolves it to `''`. It is wired into
/// [`assemble_beta_header`] in declaration order behind the default-off
/// `connector_text` Cargo feature, so it is emitted only when that feature is
/// compiled in (the external default build resolves it to `''`, exactly as TS).
pub const SUMMARIZE_CONNECTOR_TEXT: &str = "summarize-connector-text-2026-03-13";
/// AFK ("away-from-keyboard") transcript-classifier mode.
///
/// **Feature-gated / inert in the external build.** In `betas.ts` this is
/// `feature('TRANSCRIPT_CLASSIFIER') ? 'afk-mode-2026-01-31' : ''`, so the
/// literal is only emitted when the `TRANSCRIPT_CLASSIFIER` build feature is
/// on; the external default build resolves it to `''`. It is wired into
/// [`assemble_beta_header`] in declaration order behind the default-off
/// `transcript_classifier` Cargo feature, so it is never emitted externally.
pub const AFK_MODE: &str = "afk-mode-2026-01-31";
/// CLI-internal (Anthropic-employee) gate.
///
/// **`USER_TYPE === 'ant'`-gated / inert in the external build.** In
/// `betas.ts` this is
/// `process.env.USER_TYPE === 'ant' ? 'cli-internal-2026-02-09' : ''`, and the
/// `utils/betas.ts` assembler only pushes it when `USER_TYPE === 'ant' &&
/// CLAUDE_CODE_ENTRYPOINT === 'cli'` **and** the model is not a Haiku model
/// (`!isHaiku`). It is wired into [`assemble_beta_header`] behind the runtime
/// `USER_TYPE`/`CLAUDE_CODE_ENTRYPOINT` env gate ([`cli_internal_emit_gate`]).
///
/// BOUNDED DIVERGENCE (documented, not a parity gap — same style as the
/// `opus.rs` Opus-id divergence note): the TS `!isHaiku` sub-condition CANNOT
/// be honored here because [`assemble_beta_header`] takes no model parameter
/// (it routes on `Provider` × `Endpoint` only). The gate therefore omits the
/// `!isHaiku` clause; in the default external build `USER_TYPE` is unset so the
/// entry is never emitted regardless, and a Haiku request from an `ant`+`cli`
/// environment would over-emit this single beta versus TS. Threading the model
/// down to this assembler is the follow-up that closes the gap.
pub const CLI_INTERNAL: &str = "cli-internal-2026-02-09";
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
        // ---- feature()/USER_TYPE-gated trio (betas.ts declaration order) ----
        // All three resolve to NOT-emitted in the default external build:
        // - SUMMARIZE_CONNECTOR_TEXT / AFK_MODE are behind default-off Cargo
        //   features (`connector_text` / `transcript_classifier`), mirroring
        //   TS `feature('CONNECTOR_TEXT')` / `feature('TRANSCRIPT_CLASSIFIER')`.
        // - CLI_INTERNAL is behind the runtime `USER_TYPE`/`CLAUDE_CODE_ENTRYPOINT`
        //   env gate (see `cli_internal_emit_gate`), mirroring TS
        //   `process.env.USER_TYPE === 'ant' && CLAUDE_CODE_ENTRYPOINT === 'cli'`.
        (
            SUMMARIZE_CONNECTOR_TEXT,
            cfg!(feature = "connector_text") && applies_messages_only(endpoint),
        ),
        (
            AFK_MODE,
            cfg!(feature = "transcript_classifier") && applies_messages_only(endpoint),
        ),
        (
            CLI_INTERNAL,
            cli_internal_emit_gate() && applies_messages_only(endpoint),
        ),
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

/// Inject the assembled `anthropic-beta` header into a prepared request.
///
/// Merges the header value produced by [`assemble_beta_header`] with any
/// pre-existing `anthropic-beta` value already present in `request.headers`,
/// using comma-join with deduplication.
///
/// **Merge semantics:** any values already in the header are preserved first
/// (in their original order); assembled entries that are not already present
/// are appended in declaration order.  Duplicate entries are silently dropped.
///
/// **Why merge instead of replace:** the auth layer (`llm_client::authenticate`)
/// may inject `oauth-2025-04-20` into `anthropic-beta` BEFORE this policy
/// function is called.  A plain `BTreeMap::insert` would silently overwrite
/// that value, stripping the oauth beta and breaking messages.create calls
/// under OAuth sessions (Plan 3).  Merge ensures the auth-layer value survives
/// alongside the full assembled set.
pub fn apply_beta_header(
    request: &mut llm_client::ProviderRequest,
    provider: Provider,
    endpoint: Endpoint,
) {
    let assembled = assemble_beta_header(provider, endpoint);

    let merged = match request.headers.get("anthropic-beta") {
        None => assembled,
        Some(existing) => {
            // Collect pre-existing entries, preserving order.
            let mut parts: Vec<&str> = existing.split(',').map(str::trim).collect();
            // Append assembled entries that are not already present.
            for entry in assembled.split(',') {
                let entry = entry.trim();
                if !entry.is_empty() && !parts.contains(&entry) {
                    parts.push(entry);
                }
            }
            parts.join(",")
        }
    };

    request.headers.insert("anthropic-beta".to_string(), merged);
}

/// Variant of [`apply_beta_header`] that also appends `oauth-2025-04-20` when
/// `is_oauth_subscriber` is `true`.
///
/// **Parity:** mirrors `claude-code/src/utils/betas.ts:251-252`:
/// ```text
/// if (isClaudeAISubscriber()) { betaHeaders.push(OAUTH_BETA_HEADER) }
/// ```
/// where `OAUTH_BETA_HEADER = 'oauth-2025-04-20'` (`constants/oauth.ts:36`).
///
/// Under OAuth subscriber auth, `oauth-2025-04-20` is appended as one element of
/// the comma-joined `anthropic-beta` list — never as a standalone clobbering
/// insert.  When the header already contains `oauth-2025-04-20` (e.g., injected
/// earlier by `llm_client::authenticate`), the deduplication pass ensures it
/// appears exactly once.
///
/// For non-subscriber routes (`is_oauth_subscriber = false`) this function
/// behaves identically to [`apply_beta_header`].
pub fn apply_beta_header_with_auth(
    request: &mut llm_client::ProviderRequest,
    provider: Provider,
    endpoint: Endpoint,
    is_oauth_subscriber: bool,
) {
    apply_beta_header(request, provider, endpoint);

    if is_oauth_subscriber {
        let existing = request.headers.get("anthropic-beta").map(String::as_str);
        let merged = match existing {
            None => OAUTH.to_string(),
            Some(current) => {
                if current.split(',').any(|seg| seg == OAUTH) {
                    current.to_string()
                } else {
                    format!("{current},{OAUTH}")
                }
            }
        };
        request
            .headers
            .insert("anthropic-beta".to_string(), merged);
    }
}

/// Runtime emit-gate for [`CLI_INTERNAL`]. Mirrors the `utils/betas.ts`
/// assembler condition `process.env.USER_TYPE === 'ant' &&
/// process.env.CLAUDE_CODE_ENTRYPOINT === 'cli'`. Returns `false` (the default
/// external build, where neither var is set) so the entry is never emitted.
///
/// See the [`CLI_INTERNAL`] doc for the BOUNDED divergence: the TS `!isHaiku`
/// sub-condition is not honored here because [`assemble_beta_header`] has no
/// model parameter.
fn cli_internal_emit_gate() -> bool {
    std::env::var("USER_TYPE").as_deref() == Ok("ant")
        && std::env::var("CLAUDE_CODE_ENTRYPOINT").as_deref() == Ok("cli")
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

    /// The three feature()/USER_TYPE-gated betas: literals are byte-exact
    /// against betas.ts (lines 23-30), regardless of the (inert) gating.
    #[test]
    fn feature_gated_beta_constants_match_betas_ts_byte_for_byte() {
        assert_eq!(SUMMARIZE_CONNECTOR_TEXT, "summarize-connector-text-2026-03-13");
        assert_eq!(AFK_MODE, "afk-mode-2026-01-31");
        assert_eq!(CLI_INTERNAL, "cli-internal-2026-02-09");
    }

    /// In the external default build these three resolve to `''` in betas.ts,
    /// so they must NEVER appear in any assembled header (any provider ×
    /// endpoint). They are deliberately not wired into `assemble_beta_header`.
    #[test]
    fn feature_gated_betas_never_emitted_in_external_build() {
        for provider in [Provider::Anthropic, Provider::Vertex, Provider::Bedrock] {
            for endpoint in [
                Endpoint::MessagesCreate,
                Endpoint::MessagesCreateStream,
                Endpoint::CountTokens,
            ] {
                let s = assemble_beta_header(provider, endpoint);
                for gated in [SUMMARIZE_CONNECTOR_TEXT, AFK_MODE, CLI_INTERNAL] {
                    assert!(
                        !s.split(',').any(|p| p == gated),
                        "feature-gated beta {gated} must NOT be emitted externally \
                         for {provider:?}/{endpoint:?}; got: {s}",
                    );
                }
            }
        }
    }

    /// The [`CLI_INTERNAL`] runtime emit-gate is `false` in the default env
    /// (neither `USER_TYPE` nor `CLAUDE_CODE_ENTRYPOINT` set), so it stays inert.
    /// (Asserted without mutating process env so the test is parallel-safe.)
    #[test]
    fn cli_internal_emit_gate_is_false_in_default_env() {
        // In CI / external builds USER_TYPE is unset → gate closed.
        if std::env::var("USER_TYPE").is_err() {
            assert!(!cli_internal_emit_gate());
        }
    }

    /// The two compile-time-gated entries ([`SUMMARIZE_CONNECTOR_TEXT`] /
    /// [`AFK_MODE`])
    /// are behind default-off Cargo features, so the default build never emits
    /// them. Assert the features are NOT compiled in for this (default) test run.
    #[test]
    fn connector_text_and_transcript_classifier_features_are_default_off() {
        assert!(!cfg!(feature = "connector_text"));
        assert!(!cfg!(feature = "transcript_classifier"));
    }

    /// Even though the trio is now wired into the `all` array (in declaration
    /// order, between [`TOKEN_EFFICIENT_TOOLS`] and [`ADVISOR_TOOL`]), the build
    /// must keep emitting them as NOT-present for every provider × endpoint.
    /// (Complements `feature_gated_betas_never_emitted_in_external_build`, which
    /// asserts the same observable contract; this one documents the wiring.)
    #[test]
    fn wired_feature_gated_trio_stays_inert_in_default_build() {
        // Only meaningful when USER_TYPE is unset (the external default).
        if std::env::var("USER_TYPE").is_ok() {
            return;
        }
        let s = assemble_beta_header(Provider::Anthropic, Endpoint::MessagesCreate);
        for gated in [SUMMARIZE_CONNECTOR_TEXT, AFK_MODE, CLI_INTERNAL] {
            assert!(!s.split(',').any(|p| p == gated), "{gated} leaked: {s}");
        }
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

    /// When a pre-existing `anthropic-beta` value is present (e.g., injected by
    /// `llm-client`'s `authenticate()` for OAuth sessions), `apply_beta_header`
    /// must PRESERVE it and comma-join the assembled betas after it rather than
    /// overwriting.  The oauth-2025-04-20 beta must survive so that
    /// messages.create calls under Plan 3 reach the Anthropic API with both the
    /// oauth gate and the full assembled set.
    #[test]
    fn apply_beta_header_preserves_existing_values() {
        let mut req = llm_client::ProviderRequest::post_json(
            "https://api.anthropic.com/v1/messages",
            serde_json::json!({"model": "claude-sonnet-4-6", "max_tokens": 1024}),
        );

        // Pre-insert the oauth beta that llm-client's authenticate() would set.
        req.headers
            .insert("anthropic-beta".to_string(), "oauth-2025-04-20".to_string());

        apply_beta_header(&mut req, Provider::Anthropic, Endpoint::MessagesCreate);

        let value = req
            .headers
            .get("anthropic-beta")
            .expect("anthropic-beta header must be present after apply_beta_header");

        // The pre-existing oauth value must be preserved (as the first segment).
        assert!(
            value.starts_with("oauth-2025-04-20,"),
            "header must start with the pre-existing oauth value; got: {value}",
        );

        // The full assembled set must also be present.
        let assembled = assemble_beta_header(Provider::Anthropic, Endpoint::MessagesCreate);
        for beta in assembled.split(',') {
            assert!(
                value.split(',').any(|p| p == beta),
                "assembled beta '{beta}' must appear in the merged header; got: {value}",
            );
        }
    }

    /// When the pre-existing `anthropic-beta` value already contains one of the
    /// entries that `assemble_beta_header` would add, the final merged header
    /// must NOT contain that entry more than once (no duplicates).
    #[test]
    fn apply_beta_header_does_not_duplicate_entries() {
        let mut req = llm_client::ProviderRequest::post_json(
            "https://api.anthropic.com/v1/messages",
            serde_json::json!({"model": "claude-sonnet-4-6", "max_tokens": 1024}),
        );

        // Pre-insert a value that is already part of the assembled set.
        req.headers
            .insert("anthropic-beta".to_string(), CLAUDE_CODE_BETA.to_string());

        apply_beta_header(&mut req, Provider::Anthropic, Endpoint::MessagesCreate);

        let value = req
            .headers
            .get("anthropic-beta")
            .expect("anthropic-beta header must be present after apply_beta_header");

        // CLAUDE_CODE_BETA must appear exactly once — no duplicate.
        assert_eq!(
            value.split(',').filter(|p| *p == CLAUDE_CODE_BETA).count(),
            1,
            "CLAUDE_CODE_BETA must appear exactly once in the merged header; got: {value}",
        );
    }

    // ---- Task 2: OAuth subscriber beta parity tests ----
    //
    // Reference: `claude-code/src/utils/betas.ts:251-252`:
    //   if (isClaudeAISubscriber()) { betaHeaders.push(OAUTH_BETA_HEADER) }
    // Reference: `claude-code/src/constants/oauth.ts:36`:
    //   export const OAUTH_BETA_HEADER = 'oauth-2025-04-20' as const

    /// When `is_oauth_subscriber` is true, `apply_beta_header_with_auth` must
    /// append `oauth-2025-04-20` alongside model betas (comma-joined, no clobber).
    /// Mirrors `betas.ts:251-252`: `if (isClaudeAISubscriber()) { betaHeaders.push(OAUTH_BETA_HEADER) }`.
    #[test]
    fn oauth_beta_appended_for_subscriber() {
        let mut req = llm_client::ProviderRequest::post_json(
            "https://api.anthropic.com/v1/messages",
            serde_json::json!({"model": "claude-sonnet-4-6", "max_tokens": 1024}),
        );
        apply_beta_header_with_auth(
            &mut req,
            Provider::Anthropic,
            Endpoint::MessagesCreate,
            true,
        );
        let value = req
            .headers
            .get("anthropic-beta")
            .expect("anthropic-beta header must be present");

        // Must contain the core model beta AND the oauth beta.
        assert!(
            value.split(',').any(|p| p == CLAUDE_CODE_BETA),
            "claude-code beta must be present; got: {value}",
        );
        assert!(
            value.split(',').any(|p| p == OAUTH),
            "oauth-2025-04-20 must be present for subscriber; got: {value}",
        );
        // No duplicate oauth entries.
        assert_eq!(
            value.split(',').filter(|p| *p == OAUTH).count(),
            1,
            "oauth-2025-04-20 must appear exactly once; got: {value}",
        );
    }

    /// When `is_oauth_subscriber` is false, `oauth-2025-04-20` must NOT be added.
    /// Mirrors `betas.ts:251`: the push is conditional on `isClaudeAISubscriber()`.
    #[test]
    fn oauth_beta_absent_for_non_subscriber() {
        let mut req = llm_client::ProviderRequest::post_json(
            "https://api.anthropic.com/v1/messages",
            serde_json::json!({"model": "claude-sonnet-4-6", "max_tokens": 1024}),
        );
        apply_beta_header_with_auth(
            &mut req,
            Provider::Anthropic,
            Endpoint::MessagesCreate,
            false,
        );
        let value = req
            .headers
            .get("anthropic-beta")
            .expect("anthropic-beta header must be present");
        assert!(
            !value.split(',').any(|p| p == OAUTH),
            "oauth-2025-04-20 must NOT be present for non-subscriber; got: {value}",
        );
    }

    /// When `is_oauth_subscriber` is true but the header already contains
    /// `oauth-2025-04-20`, the merged result must still contain it exactly once.
    #[test]
    fn oauth_beta_appended_for_subscriber_no_duplicate_if_already_present() {
        let mut req = llm_client::ProviderRequest::post_json(
            "https://api.anthropic.com/v1/messages",
            serde_json::json!({"model": "claude-sonnet-4-6", "max_tokens": 1024}),
        );
        // Pre-seed the oauth beta (as authenticate() in llm-client would do).
        req.headers
            .insert("anthropic-beta".to_string(), OAUTH.to_string());
        apply_beta_header_with_auth(
            &mut req,
            Provider::Anthropic,
            Endpoint::MessagesCreate,
            true,
        );
        let value = req
            .headers
            .get("anthropic-beta")
            .expect("anthropic-beta header must be present");
        assert_eq!(
            value.split(',').filter(|p| *p == OAUTH).count(),
            1,
            "oauth-2025-04-20 must appear exactly once even when pre-seeded; got: {value}",
        );
    }

    /// A `ProviderRequest::post_json(...)` gains an `anthropic-beta` header
    /// equal to `assemble_beta_header(Provider::Anthropic, Endpoint::MessagesCreate)`
    /// after `apply_beta_header` is called.
    #[test]
    fn apply_beta_header_inserts_assembled_header_into_request() {
        let mut req = llm_client::ProviderRequest::post_json(
            "https://api.anthropic.com/v1/messages",
            serde_json::json!({"model": "claude-sonnet-4-6", "max_tokens": 1024}),
        );

        // Before: no anthropic-beta header.
        assert!(!req.headers.contains_key("anthropic-beta"));

        apply_beta_header(&mut req, Provider::Anthropic, Endpoint::MessagesCreate);

        // After: header equals what assemble_beta_header would return.
        let expected = assemble_beta_header(Provider::Anthropic, Endpoint::MessagesCreate);
        assert_eq!(
            req.headers.get("anthropic-beta").map(String::as_str),
            Some(expected.as_str()),
            "anthropic-beta header must equal assemble_beta_header output"
        );
    }
}
