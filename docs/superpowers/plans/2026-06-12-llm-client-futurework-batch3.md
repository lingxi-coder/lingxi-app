# llm-client Future-Work Batch 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the last three spec rev2.7 future-work items: an OpenAiResponses codec (the only protocol family without one), a Gemini File API upload flow (raw-byte channel + request builders + client driver), and TUI rate-limit rendering via an additive `OutputEvent` variant. The fourth candidate (streaming-path `LlmResponse.cost`) is **closed as a decision, not code** — see "Closed item" below.

**Architecture:** All codec work stays inside `llm-client` (no repo-internal deps). The raw-byte upload channel threads additively through `protocol::HttpRequest` → `traits::HttpTransport` impls → `LlmTransportBridge` → `ProviderRequest`. Rate-limit flows orchestrator (header parse, already live) → additive `traits::OutputEvent::RateLimit` → TUI bridge → ported claude-code message composer (copy strings already byte-locked in `tui/src/components/messages/rate_limit.rs`).

**Tech stack:** Rust workspace `lingxi-code/`. TDD, observed RED. clippy pedantic `-D warnings --all-targets --no-deps`.

**Standing constraints (NON-NEGOTIABLE):**
- `traits/` and `protocol/` are **frozen-additive**: `git diff main -- lingxi-code/traits lingxi-code/protocol` must show ONLY added lines — zero removed/modified lines.
- NEVER `git add -A` / `git add .` — stage named paths only.
- No secret material (API keys, tokens, header values) in error messages or logs.
- Commit trailer exactly: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- Sequential implementers only (one edit-agent at a time in this worktree).
- Fresh-worktree gotcha: mcp stdio tests need `cargo build -p mock_stdio_mcp` before the first `cargo test` run.

**Ground-truth reference sources (read these, do not guess wire shapes):**
- OpenAI Responses API (Rust reference, vendored Codex CLI):
  - `/Users/luolingfeng/Projects/LingXi-Next/codex/codex-rs/codex-api/src/common.rs` — `ResponsesApiRequest` (model, instructions, input, tools, tool_choice, parallel_tool_calls, reasoning{effort,summary}, store, stream, include, …), `ResponseCompletedUsage { input_tokens, input_tokens_details{cached_tokens}, output_tokens, output_tokens_details{reasoning_tokens}, total_tokens }`.
  - `/Users/luolingfeng/Projects/LingXi-Next/codex/codex-rs/codex-api/src/sse/responses.rs` — SSE event names + payload shapes (`response.created`, `response.output_item.added/done`, `response.output_text.delta`, `response.function_call_arguments.delta`, `response.reasoning_text.delta`, `response.reasoning_summary_text.delta`, `response.completed`, `response.failed`, `response.incomplete`).
  - `/Users/luolingfeng/Projects/LingXi-Next/codex/codex-rs/protocol/src/models.rs` — `ResponseItem` input/output item shapes (`message`, `function_call`, `function_call_output`, `reasoning`).
- claude-code TS rate-limit parity:
  - `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/services/claudeAiLimits.ts` — unified header names (`anthropic-ratelimit-unified-status`, `-reset`, `-fallback`, `-representative-claim`, `-overage-status`, `-overage-reset`, `-overage-disabled-reason`, per-claim `-${abbrev}-utilization` / `-${abbrev}-reset`) and the `ClaudeAILimits` shape.
  - `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/services/rateLimitMessages.ts` — `getRateLimitMessage` (WARNING_THRESHOLD = 0.7 at line 72, status gates, copy strings, reset-time formatting).
  - Upsell copy strings are ALREADY byte-locked in `tui/src/components/messages/rate_limit.rs` — reuse, do not re-type.
- Existing codec conventions: `llm-client/src/providers/openai.rs` (esp. `normalize_usage` subset rules: cached/reasoning tokens are SUBSETS — subtract to keep buckets independent), `azure_openai.rs` (thin-wrapper delegate pattern), test layout `llm-client/tests/openai_codec_test.rs`.

**Closed item (record, no code):** "streaming-path `LlmResponse.cost` estimate" is closed as WONTFIX-by-design: the streaming path yields only `LlmEvent`s and never assembles an `LlmResponse`; real billing for streamed turns is recorded via the per-field usage merge → `CostTracker::record_api_response_v2` (batch 1), and TUI cost display flows from `EndTurn`. T11 records this in the spec.

---

## Task 1: OpenAiResponses codec — request types + `encode_request`

**Files:**
- Create: `lingxi-code/llm-client/src/providers/openai_responses.rs`
- Modify: `lingxi-code/llm-client/src/providers/mod.rs` (export)
- Test: `lingxi-code/llm-client/tests/openai_responses_codec_test.rs`

Codec struct `OpenAiResponsesCodec { base_url: String }`, `new(base_url: &str)`. URL: `{base_url}/responses` (trim trailing `/` on base, same as `OpenAiChatCodec`).

Encoding rules (pin with tests, one behavior per test):
- [ ] `request.system` blocks joined with `\n\n` → top-level `instructions` string (omit when empty).
- [ ] Messages → `input` array. Text content: user role → `{type:"message", role:"user", content:[{type:"input_text", text}]}`; assistant role → `output_text` parts. `ContentBlock::ToolCall {id, name, input}` → top-level item `{type:"function_call", call_id, name, arguments: <JSON-string>}`. `ContentBlock::ToolResult {tool_call_id, content, is_error}` → `{type:"function_call_output", call_id, output: <string>}` (stringify non-text content the same way openai.rs does for tool results).
- [ ] Images: user-message `Image{media_type,bytes}` → `{type:"input_image", image_url: "data:<mt>;base64,<b64>"}`; `ImageUrl{url}` → `{type:"input_image", image_url: url}`. `Document` → `{type:"input_file", filename:"document", file_data:"data:<mt>;base64,<b64>"}` (mirror batch-2 Chat decision).
- [ ] Tools → flattened Responses shape: `{type:"function", name, description, parameters: input_schema, strict:false}` (NOT nested under `function` — that is the Chat shape; verify against codex `common.rs`).
- [ ] `tool_choice`: Auto→`"auto"`, None→`"none"`, Required→`"required"`, Tool{name}→`{type:"function", name}`.
- [ ] `max_tokens` → `max_output_tokens`; `temperature`/`top_p` pass through; `stop_sequences` → REJECT with `LlmError::InvalidRequest` (the Responses API has no stop param — verify in codex reference; if it does exist there, encode it and pin the test to that shape).
- [ ] `request.reasoning: Some(ReasoningConfig{budget_tokens})` → `reasoning: {effort}` with the documented mapping: `budget_tokens ≤ 1024` → `"low"`, `≤ 8192` → `"medium"`, else `"high"`. Doc-comment the lossy mapping. (This finally un-stages the spec §1 "staged until the Responses API codec exists" note.)
- [ ] `response_format`: JsonObject → `text: {format: {type:"json_object"}}`; JsonSchema → the Responses `text.format` json_schema shape per codex reference; if the reference lacks it, REJECT with InvalidRequest (kept-rejected pattern, doc the evidence).
- [ ] `stream: true` → body `stream: true` (framing stays `StreamFraming::Sse`). `store: false` always (stateless parity; doc-comment).
- [ ] Headers: `content-type: application/json` only (auth added by `DefaultLlmClient::authenticate`, Bearer for OpenAI family — verify in `client.rs` authenticate match).
- [ ] Reject `Reasoning`/`RedactedThinking`/`ServerToolUse`/`ConnectorText` history blocks the same way `reject_unsupported_content_blocks` does in openai.rs (reuse it if visible, else local twin).

Steps: write failing tests (RED observed) → implement → GREEN → clippy pedantic on llm-client → commit.

## Task 2: OpenAiResponses codec — `decode_response`

**Files:** same codec file + test file.

- [ ] Parse top-level `{id, model, status, output: [...], usage, incomplete_details}`.
- [ ] `output[]` mapping: `message` item → its `content[]` `output_text` parts become `ContentBlock::Text`; `function_call` → `ContentBlock::ToolCall {id: call_id, name, input: parse(arguments)}` (tolerant: unparseable arguments → `Value::String(raw)`, mirror openai.rs); `reasoning` item → `ContentBlock::Reasoning` from its summary text parts when present, else skip tolerantly. Unknown item types → skip (tolerant-decoder convention).
- [ ] `stop_reason`: status `"completed"` + any function_call in output → `"tool_use"`; `"completed"` → `"end_turn"`; `"incomplete"` + `incomplete_details.reason == "max_output_tokens"` → `"max_tokens"`; other incomplete reasons → pass reason through verbatim.
- [ ] Usage normalization (mirror openai.rs subset rules EXACTLY): `billable.input = input_tokens - cached_tokens`, `cache_read = cached_tokens`, `billable.output = output_tokens - reasoning_tokens`, `reasoning_output = reasoning_tokens`, `provider_reported_total_tokens = total_tokens`, saturating subtraction.
- [ ] Status ≥ 400 → delegate to the shared error decode path the same way openai.rs `decode_response` does (status check first).
- [ ] `cost: None`, `provider_metadata` redaction same as openai.rs.

TDD steps as Task 1. Commit.

## Task 3: OpenAiResponses codec — stream decoder

**Files:** same codec file + test file.

SSE frames are TYPED events (`event:`/JSON `type` field — check codex `sse/responses.rs` for which discriminator the wire uses; codex parses the JSON `type` field).

- [ ] `response.created` → `LlmEvent::MessageStart` with snapshot response (id/model from payload, empty content, default usage).
- [ ] `response.output_item.added` with `item.type == "function_call"` → `ContentBlockStart {index, content_block: ToolCall{id: call_id, name, input: {}}}`. Track index mapping `output_index` → our sequential block index.
- [ ] `response.output_text.delta` → open a Text block on first delta for that item (`ContentBlockStart` then `TextDelta`), subsequent → `ContentBlockDelta::TextDelta`.
- [ ] `response.function_call_arguments.delta` → `ContentBlockDelta::InputJsonDelta {partial_json: delta}`.
- [ ] `response.reasoning_text.delta` AND `response.reasoning_summary_text.delta` → Reasoning block (`ContentBlockStart(Reasoning)` once, then `ThinkingDelta`).
- [ ] `response.output_item.done` → `ContentBlockStop` for that item's index.
- [ ] `response.completed` → close any open blocks, then `MessageDelta { delta: {stop_reason}, usage: Some(normalized) }` (usage from `response.usage`, same normalization as Task 2; stop_reason derived as Task 2) then `MessageStop`.
- [ ] `response.failed` → decode into `LlmError` via the error taxonomy (payload `response.error` message/code); `response.incomplete` → treat as completed with the incomplete stop_reason mapping.
- [ ] Unknown event types → ignore tolerantly. `finish()` on EOF without `response.completed` → close open blocks + `MessageStop` (mirror openai.rs finish semantics).

TDD steps; include one full happy-path transcript test (created → text deltas → function_call added/args/done → completed) asserting the exact LlmEvent sequence. Commit.

## Task 4: registry + settings type `openai-responses`

**Files:**
- Modify: `lingxi-code/llm-client/src/client.rs` (build_codec arm at client.rs:497-503 — REPLACE the `Err(...)` arm with `OpenAiResponsesCodec::new(base_url)`; this file is not frozen)
- Modify: `lingxi-code/platforms/common/src/llm_config.rs` (`apply_settings_providers`: new provider type string `"openai-responses"` → `ProtocolFamily::OpenAiResponses`, baseUrl/apiKeyEnv required, same validation as `"openai"`)
- Modify: settings schema doc-comments where the other 7 type strings are listed (grep `"azure-openai"` in platforms/common + engine settings schema.rs to find every enumeration site)
- Tests: extend `platforms/common` settings tests (mirror the `"openai"` cases) + a `client.rs` route test proving an openai-responses profile prepares a `{base}/responses` POST.

- [ ] RED: settings test for `"openai-responses"` profile parse + route test → fail on the old `Err` arm.
- [ ] Implement, GREEN, clippy on llm-client + platform-common, commit.

## Task 5: raw-byte body channel (protocol → transports → bridge → ProviderRequest)

**Files:**
- Modify (FROZEN-ADDITIVE — added lines only): `lingxi-code/protocol/src/transport.rs` — add to `HttpRequest`: `#[serde(default, skip_serializing_if = "Option::is_none")] pub body_bytes: Option<Vec<u8>>` with doc `/// Optional raw request body; takes precedence over body when set.`
- Modify: every `HttpRequest { ... }` struct literal in NON-frozen crates (compiler-driven `body_bytes: None` additions). For literals inside `traits/` or `protocol/` tests: ADD the field line only (pure insertion is additive — verify with `git diff` that no existing line changed).
- Modify: `platforms/*/src` ReqwestHttp `request()` (and any native mobile DynHttp forwarder): when `body_bytes` is Some, send those bytes as the body (and do NOT also send `body`).
- Modify: `lingxi-code/llm-client/src/protocol.rs` — `ProviderRequest`: add `#[serde(default, skip_serializing_if = "Option::is_none")] pub body_bytes: Option<Vec<u8>>` (existing `Default` derive keeps test literals compiling via `..Default::default()` where used; fix non-default literals compiler-driven).
- Modify: `platforms/common/src/llm_transport.rs` `to_http_request`: map `body_bytes` through; when Some, `body: None`.
- Tests: platform-common bridge test (bytes pass through verbatim, body_json ignored when bytes set); reqwest impl test if an existing local-server harness exists (else unit-test the request-building seam).

- [ ] RED → implement → GREEN → workspace `cargo build` (catches every literal) → frozen-diff check: `git diff main --stat -- lingxi-code/traits lingxi-code/protocol` then `git diff main -- lingxi-code/protocol | grep -c '^-[^-]'` MUST be 0 → clippy battery on touched crates → commit.

## Task 6: Gemini File API upload flow

**Files:**
- Create: `lingxi-code/llm-client/src/providers/gemini_files.rs`
- Modify: `lingxi-code/llm-client/src/client.rs` (driver method), `providers/mod.rs`
- Test: `lingxi-code/llm-client/tests/gemini_files_test.rs`

No vendored reference exists — implement against the documented Google resumable protocol and pin every byte with tests:
- [ ] `start_upload_request(base_url, num_bytes, mime_type, display_name) -> ProviderRequest`: POST `{upload_base}/upload/v1beta/files` where `upload_base` = base_url with a trailing `/v1beta` (or `/v1`) path segment stripped (doc the convention; gemini profile base_urls end in `/v1beta` — verify in `gemini.rs` URL building and llm_config defaults). Headers: `x-goog-upload-protocol: resumable`, `x-goog-upload-command: start`, `x-goog-upload-header-content-length: <num_bytes>`, `x-goog-upload-header-content-type: <mime>`, `content-type: application/json`. Body: `{"file": {"display_name": display_name}}`.
- [ ] `parse_start_response(headers) -> Result<String, LlmError>`: extract `x-goog-upload-url` (case-insensitive header lookup), error without echoing header values (no-secret rule).
- [ ] `upload_finalize_request(upload_url, bytes) -> ProviderRequest`: POST to upload_url, headers `x-goog-upload-command: upload, finalize`, `x-goog-upload-offset: 0`, `content-length` left to transport; `body_bytes: Some(bytes)`, `body_json: Value::Null`.
- [ ] `parse_upload_response(body) -> GeminiFile { name, uri, mime_type, state }` (serde struct, camelCase wire: `file.name`, `file.uri`, `file.mimeType`, `file.state` with states `PROCESSING`/`ACTIVE`/`FAILED`).
- [ ] `file_status_request(base_url, file_name) -> ProviderRequest`: GET `{upload_base}/v1beta/{file_name}` (file_name is `files/<id>`); `parse_file_status(body) -> GeminiFile`.
- [ ] Driver on `DefaultLlmClient`: `pub async fn upload_file(&self, model_or_alias: &str, bytes: Vec<u8>, mime_type: &str, display_name: &str) -> Result<GeminiFile, LlmError>` — resolve route (must be a Gemini-family profile, else InvalidRequest), build start request, `authenticate` it (x-goog-api-key seam — reuse the existing authenticate path), execute via transport, parse upload URL, build upload+finalize with `body_bytes`, authenticate + execute, parse `GeminiFile`. NO polling inside the driver (no timer dep in llm-client): doc that callers poll `file_status_request` until `ACTIVE` for video/PDF; images are ACTIVE immediately. Mock-transport tests for the full two-step flow + auth header presence + error paths (missing upload-url header, FAILED state passthrough).
- [ ] The resulting `uri` plugs into the EXISTING `ContentBlock::ImageUrl → file_data.file_uri` Gemini encoding (batch 2) — add one integration-shaped test proving a returned uri round-trips into `encode_request` file_data.

TDD; clippy; commit.

## Task 7: orchestrator rate-limit header parse extension

**Files:**
- Modify: `lingxi-code/orchestrator/src/model/rate_limit.rs` (`RateLimitInfo` + `from_headers`)
- Tests: in-file unit tests (existing convention)

Port `parseClaudeAiLimits` from `claude-code/src/services/claudeAiLimits.ts` (READ IT FIRST — header names at lines ~160-200 and ~380-445):
- [ ] Add fields to `RateLimitInfo` (orchestrator-local, NOT frozen): `status: Option<String>` (unified-status), `resets_at: Option<u64>` (unified-reset, unix seconds), `utilization: Option<f64>` (per-claim `anthropic-ratelimit-unified-{abbrev}-utilization` where abbrev derives from representative-claim exactly as the TS maps it — read the TS abbrev function and pin it), `claim_resets_at: Option<u64>` (per-claim `-reset`), `overage_resets_at: Option<u64>`, `fallback_available: Option<bool>` (`unified-fallback == "available"`).
- [ ] `from_headers` parses all of the above tolerantly (absent → None, malformed numbers → None). Existing three fields and their parsing MUST be untouched (additive within the struct).
- [ ] Snapshot conversion to `traits::RateLimitSnapshot` unchanged (3 fields — do NOT touch traits here).
- [ ] Tests: full header set, partial, malformed utilization, abbrev mapping per claim type (five_hour / seven_day / seven_day_opus / seven_day_sonnet).

TDD; commit.

## Task 8: additive `OutputEvent::RateLimit` + orchestrator emit-on-change

**Files:**
- Modify (FROZEN-ADDITIVE): `lingxi-code/traits/src/orchestrator.rs` — append new variant to `OutputEvent` (it is `#[non_exhaustive]`): `RateLimit { status: Option<String>, rate_limit_type: Option<String>, utilization: Option<f64>, resets_at: Option<u64>, overage_status: Option<String>, overage_resets_at: Option<u64>, overage_disabled_reason: Option<String>, fallback_available: Option<bool>, is_using_overage: bool }` + default trait method `emit_rate_limit(...)` on `OutputStream` (default: no-op) following the exact pattern of `emit_usage`. ONLY appended lines.
- Modify: `lingxi-code/orchestrator/src/` turn loop (find where `emit_usage`/`emit_end_turn` are called — conversation/streaming loop): after each completed API call, read `self.api.last_rate_limit_info()`; if `Some` and DIFFERENT from the last-emitted snapshot (store `last_emitted: Mutex<Option<RateLimitInfo>>` or in loop state), call `emit_rate_limit`. `is_using_overage` = `overage_status.is_some()` && status indicates overage in TS terms — read rateLimitMessages.ts `isUsingOverage` derivation and mirror it.
- Tests: orchestrator test with a MockOutputStream capturing emit_rate_limit (extend existing mock — it gets the default no-op for free; add a recording override), proving (a) emit on first snapshot, (b) NO duplicate emit on identical snapshot, (c) re-emit on change.

- [ ] RED → implement → GREEN → frozen-diff check on traits (added lines only) → clippy → commit.

> **T8 as-built note:** the shipped `OutputEvent::RateLimit` variant carries 9 fields — it ADDS `claim_resets_at` and DROPS `is_using_overage` vs the sketch above. `is_using_overage` must be derived sink-side (TUI) from `overage_status`/TS semantics in Task 9.

## Task 9: TUI rate-limit wiring + message composer port

**Files:**
- Create: `lingxi-code/tui/src/rate_limit_messages.rs` (or under an existing module dir — follow tui layout)
- Modify: `lingxi-code/tui/src/events/orchestrator_bridge.rs` (override `emit_rate_limit` → `TurnEvent::RateLimit{...}` carrying the same fields), `tui/src/streaming.rs` (`apply_event` arm), state/render plumbing, message-list render site (find where `RenderedMessage` variants render; add a RateLimit rendered message using the EXISTING `RateLimitMessage` component + its byte-locked upsell strings).
- Tests: unit tests for the composer (copy parity), apply_event test (event → rendered message appears once; identical consecutive events do not duplicate).

Port `getRateLimitMessage` from `claude-code/src/services/rateLimitMessages.ts` (READ IT FULLY first):
- [ ] Threshold gate: only produce a warning when `utilization >= 0.7` (WARNING_THRESHOLD) for `allowed_warning`, always for `rejected`; mirror the TS branch order EXACTLY (status rejected → error message; allowed_warning → warning; overage states per TS).
- [ ] Copy strings byte-identical to the TS (curly apostrophes U+2019, ellipsis U+2026 conventions already used in `rate_limit.rs`). Reset-time formatting: reuse `orchestrator::model::rate_limit::format_reset_time` if exported, else port the TS `formatResetTime` locally (pin with tests).
- [ ] Upsell selection: mirror `RateLimitMessage.tsx getUpsellMessage` for the context the TUI HAS (subscription granularity like Max-20x is not plumbed → scope-guard: use the generic arms (`UPGRADE`, `UPGRADE_OR_EXTRA`) and document the gap in a code comment + spec). Do NOT invent new copy.
- [ ] `apply_event`: `TurnEvent::RateLimit` → compose message; if composer returns Some and text differs from the last rendered rate-limit text, push the rendered message; never duplicate consecutive identical messages.

TDD; clippy on tui; commit.

## Task 10: docs + spec revision

**Files:**
- Modify: `lingxi-code/docs/superpowers/specs/2026-06-10-llm-client-engine-adoption-design.md` — add rev 2.8: batch 3 complete (OpenAiResponses codec — every protocol family now has a codec; Gemini File API upload flow + raw-byte transport channel; TUI rate-limit rendering via `OutputEvent::RateLimit`; streaming-cost item CLOSED by design with the rationale above). Update the rev-history list and any "remaining" lists.
- [ ] Commit (docs only).

## Task 11: final verification (controller-run, not a subagent)

- [ ] `cargo build -p mock_stdio_mcp` then full `cargo test --workspace` — expect green modulo known flakes (tool-shell cwd_persistence under parallel load — verify green isolated if it fires; tui pty_smoke needs the lingxi-cli binary; platform-posix fs_watch FSEvents churn).
- [ ] Clippy battery (pedantic `-D warnings --all-targets --no-deps`) on: llm-client, platform-common, orchestrator, traits, protocol, tui, engine-desktop, engine-mobile, client-adapter, cost.
- [ ] Frozen-crate check: `git diff main -- lingxi-code/traits lingxi-code/protocol` → added lines ONLY (grep `'^-[^-]'` count must be 0 for both).
- [ ] Final whole-plan review subagent (two-stage: spec compliance vs this plan, then code quality), fix loop until READY TO MERGE.
