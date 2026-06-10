# Plan 3 prerequisites — final-review findings from Plan 2 (2026-06-11)

Transcribed verbatim requirements for the Plan 3 document (engine seam swap +
api-client deletion + modelProviders settings). Source: Plan-2 final review.

## Decisions Plan 3 must arbitrate against claude-code TS first

1. Retry-budget meaning: next_step allows initial + 3 retries (4 executions,
   3 sleeps); api-client with_retry_ctl does 3 total executions (2 sleeps).
   Same constants, different loop semantics — pick the claude-code behavior
   and pin it with a test before wiring.
2. x-should-retry: api-client honors `x-should-retry: false` as terminal even
   for 5xx; LlmError carries no equivalent. Implement (llm-client error
   enrichment or header passthrough) or document accepted divergence.
3. OAuth beta on messages.create: api-client betas.rs asserts OAUTH never
   rides messages.create; llm-client authenticate() adds it for OAuthBearer
   (and apply_beta_header preserves it). Verify against claude-code ground
   truth and align one side.
4. Mid-stream Overloaded: in-band error events stay retryable post-first-event
   (only Transport upgrades to StreamInterrupted) — caller must reset
   accumulation on replay; decide parity semantics.

## Wiring obligations

5. Drive prepare() + transport.execute() + codec.decode_response() manually on
   Anthropic routes so response HEADERS survive for: RateLimitInfo::from_headers,
   parse_unified_reset / parse_anthropic_ratelimit_reset,
   formatted_reset_times_from_headers, rate_limit_error_message (and
   x-should-retry if ported). DefaultLlmClient::execute consumes headers
   internally.
6. Driver loop around next_step: sleep on RetryAfter; re-encode max_tokens on
   AdjustMaxTokens (thread thinking_budget from ReasoningConfig.budget_tokens);
   re-resolve model on Fallback. Port resolve_retry_control
   (FALLBACK_FOR_ALL_PRIMARY_MODELS / USER_TYPE / IS_SANDBOX envs +
   is_non_custom_opus + subscriber state) — not ported in Plan 2.
7. 429 subscriber gate (!is_subscriber || is_enterprise) around the driver,
   terminal-429 message swap, and the reset-ladder sleep
   (retry-after → unified-reset → requests-reset → 1s).
8. Wire the five emit_* fns at api-client's emission points; port
   error_kind()/status_of() label tables re-typed to LlmError, plus
   new_request_id; port byte-locked user_agent() onto all requests.
9. anthropic-oauth: replace the internal api_client::oauth_hook::OAuthRefreshHook
   usage in credential_provider.rs with an inherent method BEFORE deleting
   api-client; decide the reactive-401 refresh+retry-once parity path
   (Authentication is Terminal in next_step today).
10. Render the byte-locked "Repeated 529 Overloaded errors" copy at error
    rendering (DriveStep::Terminal carries no repeated bit; reconstruct from
    RetryState.consecutive_overloaded + last error == Overloaded).
11. Plan-1 prereq still open: streaming responses carry no status/headers
    through LlmTransportBridge (traits::HttpTransport::stream_sse surfaces no
    metadata) — streaming-route rate-limit tracking and streaming-429
    retry-after need a traits-level change first.
12. Deferred from Plan 2: PricingCatalog population from the cost crate;
    absorb/replace the providers-crate ModelRouter layer.

## Minor carry-overs

- betas.rs module note stale: connector_text/transcript_classifier ARE now in
  orchestrator [features] (default-off).
- Spec wording says approximation "chars/4"; code (honestly documented) counts
  bytes. Align wording in the Plan-3 spec touch.
- count_tokens decode uses a literal "2023-06-01" (decode-only; harmless).
