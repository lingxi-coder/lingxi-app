# LLM Providers v2 — P3: Reasoning Params — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Support reasoning models — request-side controls (OpenAI o-series `reasoning_effort` + `max_completion_tokens`/no-`temperature`; Gemini `thinkingConfig.thinkingBudget`) and decoding reasoning traces into the canonical `Thinking` block (OpenAI/DeepSeek `reasoning_content`; Gemini `thought` parts).

**Architecture:** A profile declares reasoning config (`reasoningEffort`/`thinkingBudget`), threaded to the codec at construction. The codec applies it in `encode`; the effective value is `request.field.or(codec default)`. Decoders gain reasoning→`Thinking` mapping (additive). No `UsageApi` change — reasoning tokens remain folded into `output_tokens` (documented).

**Tech Stack:** Rust 1.82.0, serde_json. Run cargo from `lingxi-code/`.

**Spec:** `2026-06-01-llm-providers-v2-design.md` §3.2. Branch `llm-providers-v2` (P2 done, tag `llm-v2-p2`).

**Deviation note (deliberate, bounded):** the spec mentioned extending `ReasoningSupport` and mapping reasoning tokens to `reasoning_output`. We do NOT extend `ReasoningSupport` (the profile config drives behavior; unused enum variants are noise) and do NOT add a `UsageApi` reasoning field (it's a parity-sensitive type; reasoning tokens already count in `output_tokens`). Both keep P3 bounded and parity-safe.

**Parity gate:** reasoning-free requests are byte-identical (no `reasoningEffort`/`thinkingBudget` configured → `max_tokens` + `temperature` exactly as today). Existing `test-harness` parity suite stays green. Do NOT modify `traits/`.

---

## File Structure

| File | Change |
|---|---|
| `providers/src/request.rs` | `ReasoningEffort` enum; `CanonicalRequest.{reasoning_effort, thinking_budget}` |
| `providers/src/profile.rs` | `ProviderProfile.{reasoning_effort, thinking_budget}` + parse `reasoningEffort`/`thinkingBudget` |
| `providers/src/openai/mod.rs` | `OpenAiCodec::new(base_url, reasoning_effort)`; pass effective effort to encode |
| `providers/src/openai/encode.rs` | `encode_chat_body(req, reasoning_effort)`: `max_completion_tokens`/no-temp/`reasoning_effort` |
| `providers/src/openai/decode.rs` | non-stream `reasoning_content` → prepend `Thinking` |
| `providers/src/openai/stream.rs` | `delta.reasoning_content` → `Thinking` block + `ThinkingDelta` |
| `providers/src/gemini/mod.rs` | `GeminiCodec::new(base_url, thinking_budget)`; pass effective budget to encode |
| `providers/src/gemini/encode.rs` | `encode_generate_body(req, thinking_budget)`: `thinkingConfig` |
| `providers/src/gemini/decode.rs` | part `"thought": true` → `Thinking` |
| `providers/src/gemini/stream.rs` | `"thought": true` text part → `Thinking` block + `ThinkingDelta` |
| `providers/src/registry.rs` | pass `profile.reasoning_effort` / `profile.thinking_budget` to codec ctors |

---

## Task A: Request-side reasoning (atomic — codec ctor signatures change)

This is one atomic unit: the `OpenAiCodec::new`/`GeminiCodec::new` signatures change, which ripples to `registry.rs` and the codecs' own callers/tests. Apply together; compile green at the end.

**Files:** `request.rs`, `profile.rs`, `openai/mod.rs`, `openai/encode.rs`, `gemini/mod.rs`, `gemini/encode.rs`, `registry.rs`.

- [ ] **Step 1 — `request.rs`: `ReasoningEffort` + fields.**

Add the enum (after the imports):
```rust
/// Reasoning effort hint for reasoning-capable models (OpenAI o-series).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort {
    /// Minimal reasoning.
    Low,
    /// Default reasoning.
    Medium,
    /// Maximal reasoning.
    High,
}

impl ReasoningEffort {
    /// The wire token (`"low"`/`"medium"`/`"high"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Parse a case-insensitive `"low"`/`"medium"`/`"high"`; `None` otherwise.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}
```
Add two fields to `CanonicalRequest` (after `temperature`):
```rust
    /// Reasoning effort (OpenAI o-series). When set, the OpenAI codec emits
    /// `reasoning_effort`, uses `max_completion_tokens`, and omits `temperature`.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Thinking-token budget (Gemini 2.5). When set, the Gemini codec emits
    /// `generationConfig.thinkingConfig`.
    pub thinking_budget: Option<u32>,
```
In `CanonicalRequest::new`, initialize both to `None`. Re-export `ReasoningEffort` from `lib.rs` (add to the `pub use request::{...}` line). Update the existing `new_sets_defaults` test to assert both are `None`.

- [ ] **Step 2 — `profile.rs`: profile fields + parsing.**

Add to `ProviderProfile`:
```rust
    /// Default reasoning effort for this profile's models (OpenAI o-series).
    pub reasoning_effort: Option<crate::request::ReasoningEffort>,
    /// Default thinking-token budget for this profile's models (Gemini 2.5).
    pub thinking_budget: Option<u32>,
```
In `builtin_profiles`, set both `None` for all three built-ins. In `parse_profiles`, after `api_key_env`, parse:
```rust
        let reasoning_effort = obj
            .get("reasoningEffort")
            .and_then(serde_json::Value::as_str)
            .and_then(crate::request::ReasoningEffort::parse);
        let thinking_budget = obj
            .get("thinkingBudget")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok());
```
and include both in the constructed `ProviderProfile`. Add a test `parse_reasoning_fields` (a profile with `"reasoningEffort":"high"` → `Some(High)`; `"thinkingBudget":2048` → `Some(2048)`).

- [ ] **Step 3 — `openai`: ctor + encode.**

`openai/mod.rs`: add field `reasoning_effort: Option<crate::request::ReasoningEffort>` to `OpenAiCodec`; change `new` to `pub fn new(base_url: Option<String>, reasoning_effort: Option<crate::request::ReasoningEffort>) -> Self`. In `encode_request`, compute `let effort = req.reasoning_effort.or(self.reasoning_effort);` and call `encode::encode_chat_body(req, effort)`. Update `mod.rs`'s own tests to pass `None` for the new ctor arg.

`openai/encode.rs`: change `encode_chat_body` to `pub fn encode_chat_body(req: &CanonicalRequest, reasoning_effort: Option<crate::request::ReasoningEffort>) -> Value`. Replace the `max_tokens` + `temperature` insertion with:
```rust
    if let Some(effort) = reasoning_effort {
        // Reasoning models: max_completion_tokens (not max_tokens), no temperature.
        body.insert("max_completion_tokens".to_string(), json!(req.max_tokens));
        body.insert("reasoning_effort".to_string(), json!(effort.as_str()));
    } else {
        body.insert("max_tokens".to_string(), json!(req.max_tokens));
        if let Some(t) = req.temperature {
            body.insert("temperature".to_string(), json!(t));
        }
    }
```
(Keep `model`, `messages`, `tools`, `stream` exactly as before.) Update the existing `encode.rs` tests that call `encode_chat_body(&req)` → `encode_chat_body(&req, None)`. Add a test `reasoning_effort_uses_max_completion_tokens_and_no_temperature`:
```rust
    #[test]
    fn reasoning_effort_uses_max_completion_tokens_and_no_temperature() {
        let mut req = CanonicalRequest::new("o3-mini");
        req.temperature = Some(0.7);
        let body = encode_chat_body(&req, Some(crate::request::ReasoningEffort::High));
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["max_completion_tokens"], req.max_tokens);
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn no_reasoning_keeps_max_tokens_and_temperature() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.temperature = Some(0.5);
        let body = encode_chat_body(&req, None);
        assert_eq!(body["max_tokens"], req.max_tokens);
        assert_eq!(body["temperature"], 0.5);
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("max_completion_tokens").is_none());
    }
```

- [ ] **Step 4 — `gemini`: ctor + encode.**

`gemini/mod.rs`: add field `thinking_budget: Option<u32>` to `GeminiCodec`; change `new` to `pub fn new(base_url: Option<String>, thinking_budget: Option<u32>) -> Self`. In `encode_request`, compute `let budget = req.thinking_budget.or(self.thinking_budget);` and call `encode::encode_generate_body(req, budget)`. Update `mod.rs` tests to pass `None`.

`gemini/encode.rs`: change `encode_generate_body` to `pub fn encode_generate_body(req: &CanonicalRequest, thinking_budget: Option<u32>) -> Value`. In the `gen_config` block, after the temperature insert, add:
```rust
    if let Some(budget) = thinking_budget {
        gen_config.insert(
            "thinkingConfig".to_string(),
            json!({"thinkingBudget": budget, "includeThoughts": true}),
        );
    }
```
Update existing `encode.rs` tests calling `encode_generate_body(&req)` → `encode_generate_body(&req, None)`. Add a test `thinking_budget_emits_thinking_config`:
```rust
    #[test]
    fn thinking_budget_emits_thinking_config() {
        let req = CanonicalRequest::new("gemini-2.5-pro");
        let body = encode_generate_body(&req, Some(2048));
        assert_eq!(body["generationConfig"]["thinkingConfig"]["thinkingBudget"], 2048);
        assert_eq!(body["generationConfig"]["thinkingConfig"]["includeThoughts"], true);
    }
```

- [ ] **Step 5 — `registry.rs`: pass reasoning config to ctors.**

In the OpenAi branch: `OpenAiCodec::new(profile.base_url.clone(), profile.reasoning_effort)`. In the Gemini branch: `GeminiCodec::new(profile.base_url.clone(), profile.thinking_budget)`. (Also fix the registry tests' `ProviderProfile { ... }` literals to include the two new fields = `None`, and any other `ProviderProfile {…}` construction in tests across the crate.)

- [ ] **Step 6 — gates + commit.**
```bash
cargo test -p providers
cargo clippy -p providers --all-targets -- -D warnings
git add lingxi-code/providers/src/request.rs lingxi-code/providers/src/profile.rs lingxi-code/providers/src/openai/ lingxi-code/providers/src/gemini/ lingxi-code/providers/src/registry.rs lingxi-code/providers/src/lib.rs
git commit -m "feat(llm-v2 P3): request-side reasoning (OpenAI reasoning_effort/max_completion_tokens, Gemini thinkingConfig)"
```
Expected: all providers tests pass (existing + new); reasoning-free encode unchanged.

---

## Task B: Reasoning-trace decode (both codecs)

Additive to the decoders — maps provider reasoning into the canonical `Thinking` block. No signature changes.

**Files:** `openai/decode.rs`, `openai/stream.rs`, `gemini/decode.rs`, `gemini/stream.rs`.

- [ ] **Step 1 — OpenAI non-stream decode.**

In `decode_chat_response`, BEFORE pushing the text block, check for reasoning and prepend it:
```rust
    if let Some(reasoning) = message.get("reasoning_content").and_then(Value::as_str) {
        if !reasoning.is_empty() {
            content.push(ContentBlockApi::Thinking {
                thinking: reasoning.to_string(),
                signature: None,
            });
        }
    }
```
(Place this right after `let mut content = Vec::new();` so the `Thinking` block precedes text/tool — reasoning comes first.) Add a test: a response whose `message.reasoning_content` = "let me think" yields `content[0]` = `Thinking{thinking:"let me think"}` and the text after it.

- [ ] **Step 2 — OpenAI stream decode.**

Add to `OpenAiSseDecoder`: `reasoning_open: bool`, `reasoning_index: u32` (init false/0 in `new`). Add a `handle_reasoning(&mut self, text, out)` mirroring `handle_text` but opening a `ContentBlockApi::Thinking { thinking: String::new(), signature: None }` and emitting `ContentDelta::ThinkingDelta { thinking: text }`. In `push`, inside the `delta` block, BEFORE the text handling:
```rust
            if let Some(r) = delta.get("reasoning_content").and_then(Value::as_str) {
                self.handle_reasoning(r, &mut out);
            }
```
In `close_open_blocks`, close the reasoning block first if `reasoning_open` (push `ContentBlockStop{ index: reasoning_index }`, set false). Add a streaming test: frames with `delta.reasoning_content` "th"/"ink" then `delta.content` "answer" → a `Thinking` block (ThinkingDelta "th","ink") opened before the text block, both closed, terminal `MessageStop`.

- [ ] **Step 3 — Gemini non-stream decode.**

In `decode_generate_response`'s parts loop, handle a thought part BEFORE the text branch:
```rust
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    if !t.is_empty() {
                        content.push(ContentBlockApi::Thinking {
                            thinking: t.to_string(),
                            signature: None,
                        });
                    }
                }
            } else if let Some(text) = part.get("text").and_then(Value::as_str) {
                ...existing Text push...
            } else if let Some(fc) = part.get("functionCall") {
                ...existing...
            }
```
(Restructure the `if/else if` so the thought check precedes the text check.) Add a test: a part `{"text":"reasoning","thought":true}` → `Thinking`; a plain `{"text":"answer"}` → `Text`.

- [ ] **Step 4 — Gemini stream decode.**

Add `reasoning_open: bool` + `reasoning_index: u32` to `GeminiSseDecoder` (init in `new`). Add `handle_reasoning(text, out)` mirroring `handle_text` but for `Thinking`/`ThinkingDelta`. In `push`'s parts loop, handle thought parts first:
```rust
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    if let Some(t) = part.get("text").and_then(Value::as_str) {
                        self.handle_reasoning(t, &mut out);
                    }
                } else if let Some(text) = part.get("text").and_then(Value::as_str) {
                    self.handle_text(text, &mut out);
                } else if let Some(fc) = part.get("functionCall") {
                    self.handle_function_call(fc, &mut out);
                }
```
In `finish`, close the reasoning block first if `reasoning_open` (before the text-block close). Add a streaming test: a thought text part then a normal text part → a `Thinking` block (ThinkingDelta) before the text block, terminal sequence once.

- [ ] **Step 5 — gates + commit.**
```bash
cargo test -p providers openai gemini
cargo clippy -p providers --all-targets -- -D warnings
git add lingxi-code/providers/src/openai/decode.rs lingxi-code/providers/src/openai/stream.rs lingxi-code/providers/src/gemini/decode.rs lingxi-code/providers/src/gemini/stream.rs
git commit -m "feat(llm-v2 P3): decode reasoning traces into canonical Thinking (OpenAI reasoning_content, Gemini thought parts)"
```

---

## Task C: Phase gates + tag

- [ ] **Step 1:** `cargo test -p providers -p orchestrator -p test-harness` — all pass (parity green).
- [ ] **Step 2:** `cargo build --workspace` — Finished.
- [ ] **Step 3:** `cargo clippy -p providers --all-targets -- -D warnings` — clean.
- [ ] **Step 4:** `bash scripts/check-deps.sh` — OK 73 crates (no new deps).
- [ ] **Step 5:** `git tag -a llm-v2-p3 -m "LLM Providers v2 P3: reasoning params"`.

---

## Self-Review

**Spec coverage (§3.2):** `reasoning_effort`/`thinking_budget` request fields (A1) ✓; o-series `max_completion_tokens`/no-temp/`reasoning_effort` (A3) ✓; Gemini `thinkingConfig.thinkingBudget`+`includeThoughts` (A4) ✓; profile reasoning style (A2) + registry wiring (A5) ✓; decode OpenAI `reasoning_content` (B1/B2) ✓; decode Gemini `thought` (B3/B4) ✓; reasoning into canonical `Thinking` ✓. Deliberate bounded deferrals (no `ReasoningSupport` extension, no `UsageApi` reasoning-token field) documented above.

**Placeholder scan:** all code steps show full code or precise edits with exact JSON keys + test assertions. The decoder steps reference mirroring the existing `handle_text` pattern (which the implementer has in-file) with the exact new fields/branches specified.

**Type consistency:** `ReasoningEffort` (request.rs) used in profile.rs, both codecs' `new`, `encode_chat_body`'s param. `encode_chat_body(req, Option<ReasoningEffort>)` and `encode_generate_body(req, Option<u32>)` signatures consistent across mod.rs callers + tests. `reasoning_open`/`reasoning_index` parallel the existing `text_open`/`text_index` in both decoders.
