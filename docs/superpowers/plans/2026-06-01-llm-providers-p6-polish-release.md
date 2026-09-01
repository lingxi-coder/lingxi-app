# LLM Providers — P6 (Polish + Docs + Release) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the LLM Providers feature: enforce the capability guardrail (fail fast when a non-tool-capable model is selected with tools), surface multi-provider model syntax in `/model`, document provider configuration, and cut the v0.11.0 release.

**Architecture:** Small, additive polish + docs + a mechanical version bump. Capability enforcement lands in the orchestrator's `ProviderApiAdapter::stream` (the only path carrying tools). `/model` gets a lightweight provider-prefixed example listing. Docs: README section + a dedicated usage doc + CHANGELOG. Release: bump 68 crate manifests `0.10.0 → 0.11.0` and refresh the three version-string TUI snapshots (the M9-style cascade).

**Tech Stack:** Rust 1.82.0. Builds on P1–P5 (the full provider stack: routing + 3 codecs + cost).

**Spec:** `docs/superpowers/specs/2026-06-01-llm-providers-design.md` (§3.3/§4 capabilities, §6 `/model`, §12 P6).

**Conventions (read first):**
- Run all commands from `lingxi-code/`. Rust 1.82.0. Docs/CHANGELOG/README live at the **repo root** (`/Users/luolingfeng/Projects/LingXi-Next/`), NOT under `lingxi-code/` — `cd` to the repo root for `git add` of those.
- Workspace lints: `missing_docs`/`pedantic` under `-D warnings`. The provider/orchestrator-source gates: `cargo clippy -p providers --all-targets -- -D warnings` (clean) and `cargo build -p orchestrator --tests` (no new warnings).
- Do NOT modify `traits/` (frozen).

**Deferred (out of P6 scope — documented, not done):**
- Profile-accurate `/model` listing (would need the registry plumbed to the `OrchestratorHandle`; P6 ships a static syntax hint instead).
- Streaming-error taxonomy hook (P3 reviewer note), Azure URL-template variant, vision/reasoning, the pre-existing `tool-api` test-target debt.

---

## File Structure
- Modify: `lingxi-code/orchestrator/src/provider_adapter.rs` — capability fail-fast in `stream` + test.
- Modify: `lingxi-code/orchestrator/src/handle_impl.rs` — `list_available_models` provider examples.
- Modify: `README.md` (repo root) — LLM Providers section + Subsystem-status row + v0.11.0 references.
- Create: `docs/LLM_PROVIDERS.md` (repo root) — provider configuration usage doc.
- Modify: `CHANGELOG.md` (repo root) — `[0.11.0]` entry.
- Modify: 68 `Cargo.toml` files — version `0.10.0 → 0.11.0`.
- Modify: 3 TUI version snapshots (regenerated).

---

## Task 1: Capability fail-fast guardrail

**Files:**
- Modify: `lingxi-code/orchestrator/src/provider_adapter.rs`

When a model whose provider reports `native_tools = false` is asked to run a turn WITH tools, fail fast with a clear error rather than silently dropping the tools. (All three v1 providers report `native_tools = true`, so this never triggers in v1 — it's a guardrail for future non-tool models; tested with a stub.) The only path carrying tools is `StreamingApiClient::stream`.

- [ ] **Step 1: Add the guard to `stream`**

In `lingxi-code/orchestrator/src/provider_adapter.rs`, in the `StreamingApiClient::stream` impl, after `let resolved = self.router.resolve(model)?;` and BEFORE building the `CanonicalRequest`, insert:

```rust
        if !tools.is_empty() && !resolved.provider.capabilities().native_tools {
            return Err(ApiError::Http(platform_api::HttpError::InvalidRequest(format!(
                "model {model:?} ({:?}) does not support tool use; \
                 select a tool-capable model or run without tools",
                resolved.provider.id()
            ))));
        }
```

(If `traits` isn't already imported in this file, use the fully-qualified `platform_api::HttpError` as shown — no new `use` needed. `ApiError` is already in scope.)

- [ ] **Step 2: Add a test**

In `provider_adapter.rs`'s `#[cfg(test)] mod tests`, the existing `StubProvider` uses `Capabilities::anthropic()` (native_tools=true). Add a no-tools-capable variant + a test. Append to the test module:

```rust
    /// A provider that reports no native tool support.
    struct NoToolsProvider;

    #[async_trait]
    impl LlmProvider for NoToolsProvider {
        fn id(&self) -> cost::ProviderId {
            cost::ProviderId::Custom { name: "no-tools".to_string() }
        }
        fn capabilities(&self) -> &Capabilities {
            // A leaked const ref keeps the signature `-> &Capabilities` simple
            // for this test-only stub.
            use std::sync::OnceLock;
            static CAPS: OnceLock<Capabilities> = OnceLock::new();
            CAPS.get_or_init(|| Capabilities {
                native_tools: false,
                streaming: true,
                vision: false,
                prompt_cache: false,
                reasoning: providers::ReasoningSupport::None,
                parallel_tool_calls: false,
                max_output_tokens: None,
                system_style: providers::SystemStyle::RoleMessage,
            })
        }
        async fn complete(&self, _req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
            unreachable!("not used in this test")
        }
        async fn stream(
            &self,
            _req: CanonicalRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
            Ok(futures::stream::empty::<Result<StreamEvent, ApiError>>().boxed())
        }
    }

    struct FixedRouter(std::sync::Arc<dyn LlmProvider>);
    impl providers::ModelRouter for FixedRouter {
        fn resolve(&self, model: &str) -> Result<providers::Resolved, ApiError> {
            Ok(providers::Resolved { provider: self.0.clone(), model: model.to_string() })
        }
        fn available_profiles(&self) -> Vec<String> {
            vec!["fixed".to_string()]
        }
    }

    #[tokio::test]
    async fn stream_with_tools_on_non_tool_model_fails_fast() {
        let router = std::sync::Arc::new(FixedRouter(std::sync::Arc::new(NoToolsProvider)));
        let adapter = ProviderApiAdapter::new(router);
        let tools = vec![serde_json::json!({"name": "Read"})];
        let err = adapter
            .stream("custom/no-tool-model", None, Vec::new(), tools)
            .await
            .expect_err("must reject tools on a non-tool-capable model");
        assert!(matches!(err, ApiError::Http(platform_api::HttpError::InvalidRequest(_))));
    }

    #[tokio::test]
    async fn stream_without_tools_on_non_tool_model_is_allowed() {
        let router = std::sync::Arc::new(FixedRouter(std::sync::Arc::new(NoToolsProvider)));
        let adapter = ProviderApiAdapter::new(router);
        let _s = adapter
            .stream("custom/no-tool-model", None, Vec::new(), Vec::new())
            .await
            .expect("no tools → allowed");
    }
```

(Imports: the test module already has `use super::*;`, `use providers::Capabilities;`, `use futures::StreamExt;`. Add `use providers;` items as fully-qualified `providers::{ReasoningSupport, SystemStyle, ModelRouter, Resolved}` references inline as shown, or add them to the test module's `use` list if cleaner. `async_trait`, `BoxStream`, `CanonicalRequest`, `MessageResponse`, `StreamEvent`, `ApiError` are already in scope via `use super::*`.)

- [ ] **Step 3: Test + commit**

Run: `cargo test -p orchestrator provider_adapter` → all pass (the existing 2 bridge tests + the 2 new guardrail tests).
Run: `cargo build -p orchestrator --tests 2>&1 | grep -i "warning.*provider_adapter"` → empty.
```bash
git add orchestrator/src/provider_adapter.rs
git commit -m "feat(llm-p6): fail-fast when tools are sent to a non-tool-capable model"
```

---

## Task 2: `/model` provider-prefixed examples

**Files:**
- Modify: `lingxi-code/orchestrator/src/handle_impl.rs`

`list_available_models` is purely informational for the no-arg `/model` display. Add provider-prefixed examples so users discover the `provider/model` syntax. (Profile-accurate listing is deferred — see plan header.)

- [ ] **Step 1: Extend the list + update the doc comment**

In `lingxi-code/orchestrator/src/handle_impl.rs`, replace the `list_available_models` body (the doc comment + the `vec![...]`) with:

```rust
    /// Model names shown by the no-arg `/model` display. Purely informational —
    /// `switch_model` accepts any string. Includes the Anthropic defaults plus
    /// `provider/model` examples so the multi-provider syntax is discoverable;
    /// actual availability of a non-Anthropic provider depends on its API key /
    /// settings profile (see `docs/LLM_PROVIDERS.md`).
    async fn list_available_models(&self) -> Vec<String> {
        vec![
            "claude-opus-4-7".to_string(),
            "claude-sonnet-4-6".to_string(),
            "claude-haiku-4-5".to_string(),
            "openai/gpt-4o".to_string(),
            "openai/gpt-4o-mini".to_string(),
            "gemini/gemini-2.0-flash".to_string(),
        ]
    }
```

- [ ] **Step 2: Update the corresponding test (if any)**

Search for a test asserting the old list contents:
Run: `grep -rn "list_available_models" orchestrator/src test-harness/tests`
If a test asserts the exact old 3-element list, update it to assert the list now CONTAINS `"openai/gpt-4o"` and `"gemini/gemini-2.0-flash"` (and still the claude models). If no such assertion exists, no change.

- [ ] **Step 3: Test + commit**

Run: `cargo test -p orchestrator` → all pass.
```bash
git add orchestrator/src/handle_impl.rs
git commit -m "feat(llm-p6): /model surfaces provider/model syntax examples"
```

---

## Task 3: README + usage doc

**Files:**
- Modify: `README.md` (repo root)
- Create: `docs/LLM_PROVIDERS.md` (repo root)

- [ ] **Step 1: Create the usage doc**

Create `/Users/luolingfeng/Projects/LingXi-Next/docs/LLM_PROVIDERS.md`:

```markdown
# LLM Providers

LingXi defaults to Anthropic but can use any of several providers as the
main-loop model backend. Selection is by a `provider/model` string; a bare
string (or any `claude-*`) stays on Anthropic, so existing configs are
unchanged.

## Built-in providers

| Prefix | Provider | API key env | Notes |
|---|---|---|---|
| `anthropic/` (or bare / `claude-*`) | Anthropic | `ANTHROPIC_API_KEY` | Default; full feature parity |
| `openai/` | OpenAI | `OPENAI_API_KEY` | OpenAI Chat Completions |
| `gemini/` | Google Gemini | `GEMINI_API_KEY` | `generateContent` |

Examples:

```bash
# OpenAI
OPENAI_API_KEY=sk-... cargo run -p cli -- --model openai/gpt-4o
# Gemini
GEMINI_API_KEY=... cargo run -p cli -- --model gemini/gemini-2.0-flash
# Anthropic (default — unchanged)
ANTHROPIC_API_KEY=sk-ant-... cargo run -p cli -- --model claude-opus-4-7
```

## OpenAI-compatible endpoints (Groq, Together, Ollama, vLLM, OpenRouter, …)

Declare a named profile in `settings.json` under `providers`. Each profile has
a `type` (`anthropic` | `openai` | `gemini`), an optional `baseUrl`, and an
optional `apiKeyEnv` (the env var holding the key; `null` for no auth). Select
it as `profilename/model`.

```jsonc
{
  "model": "groq/llama-3.3-70b",
  "providers": {
    "groq":   { "type": "openai", "baseUrl": "https://api.groq.com/openai/v1", "apiKeyEnv": "GROQ_API_KEY" },
    "ollama": { "type": "openai", "baseUrl": "http://localhost:11434/v1", "apiKeyEnv": null }
  }
}
```

## Capabilities & limitations (v1)

- **Tool use** is translated natively for all three providers (the agentic loop
  works on each). A model whose provider lacks native function-calling is
  rejected when tools are present, rather than silently degraded.
- **Anthropic-only features** (prompt caching, extended-thinking blocks, server
  tools, citations) are omitted on other providers — never fabricated.
- **Not yet supported:** image/vision input, reasoning-model parameters
  (`max_completion_tokens`), Azure OpenAI's deployment URL template, and
  Vertex/Bedrock signed auth. Cost is attributed per provider; unpriced models
  record zero cost rather than an invented rate.
```

- [ ] **Step 2: Add a README section + Subsystem-status row + bump version references**

In `/Users/luolingfeng/Projects/LingXi-Next/README.md`:
1. Add a new intro paragraph near the top (after the existing v0.10.0 paragraph) describing v0.11.0:
```
v0.11.0 adds **multi-LLM-provider support**: an in-engine provider layer so
LingXi can use OpenAI / OpenAI-compatible (Groq, Together, Ollama, vLLM,
OpenRouter, …) and Google Gemini as the model backend, selected via a
`provider/model` string. Anthropic stays the default and claude-code parity is
unchanged — every provider normalizes to LingXi's canonical message format. See
`docs/LLM_PROVIDERS.md`.
```
2. Change the "Subsystem status (v0.10.0)" heading to "(v0.11.0)" and add a row:
```
| LLM Providers (OpenAI-compatible + Gemini codecs, `provider/model` routing, per-provider cost) | Complete | v0.11.0 |
```
3. Add `docs/LLM_PROVIDERS.md` to the Architecture/navigation list if one exists.

- [ ] **Step 3: Commit**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add README.md docs/LLM_PROVIDERS.md
git commit -m "docs(llm-p6): LLM Providers usage doc + README section"
```

---

## Task 4: CHANGELOG entry

**Files:**
- Modify: `CHANGELOG.md` (repo root)

- [ ] **Step 1: Add the `[0.11.0]` entry**

In `/Users/luolingfeng/Projects/LingXi-Next/CHANGELOG.md`, add a new entry ABOVE the existing `[0.10.0]` entry (match the file's existing heading style):

```markdown
## [0.11.0] — LLM Providers

Multi-LLM-provider support: an integrated, in-engine provider layer so LingXi
can use providers beyond Anthropic as the model backend.

### Added
- `providers` crate: an `LlmProvider` abstraction with a pure `WireCodec` /
  `SseDecoder` translation core and a `GenericClient` harness; every provider
  normalizes to the canonical Anthropic-shaped message types, so the
  orchestrator / TUI / session / cost layers are unchanged.
- **OpenAI / OpenAI-compatible codec** — Chat Completions encode/decode +
  streaming tool-call reassembly. Covers OpenAI, Azure (via base URL), Groq,
  Together, Ollama, vLLM, DeepSeek, OpenRouter, … through settings profiles.
- **Native Google Gemini codec** — `generateContent` + `streamGenerateContent`,
  with `functionCall`/`functionResponse` tool pairing.
- `provider/model` selection (`openai/gpt-4o`, `gemini/gemini-2.0-flash`); bare
  / `claude-*` strings stay on Anthropic (back-compat). Named provider profiles
  in `settings.json` (`providers` object). `/model` surfaces the syntax.
- Per-provider cost attribution (OpenAI + Gemini reference price tables;
  prefix-aware `ModelRef`).
- Capability guardrail: tools sent to a non-tool-capable model fail fast.

### Unchanged
- Anthropic is the default; the Anthropic request/response wire and cost events
  are byte-identical (verified by the existing parity suites). `traits/` and the
  `api-client` Anthropic path are untouched.
```

- [ ] **Step 2: Commit**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
git add CHANGELOG.md
git commit -m "docs(llm-p6): CHANGELOG 0.11.0 entry"
```

---

## Task 5: Version bump 0.10.0 → 0.11.0

**Files:**
- Modify: 68 `Cargo.toml` files
- Modify: 3 regenerated TUI version snapshots

- [ ] **Step 1: Bump all crate manifests**

From `lingxi-code/`, bump every crate version (only the exact `version = "0.10.0"` package line):

Run:
```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
grep -rl '^version = "0.10.0"' --include="Cargo.toml" . | while read -r f; do
  sed -i '' 's/^version = "0.10.0"/version = "0.11.0"/' "$f"
done
```
(On Linux, use `sed -i` without the `''`.) Then verify the count:
Run: `grep -rl '^version = "0.11.0"' --include="Cargo.toml" . | wc -l` → expect 68 (and `grep -rl '^version = "0.10.0"'` → 0).

- [ ] **Step 2: Update the Cargo.lock + build**

Run: `cargo build --workspace` → Finished (this rewrites `Cargo.lock` with the new versions).

- [ ] **Step 3: Regenerate the version-string snapshots**

Three TUI snapshots render `CARGO_PKG_VERSION` and now mismatch (`v0.10.0`→`v0.11.0`): `render_placeholder`, `render_settings_screen::settings_status_tab`, and `screens::settings::status::status_renders_real_snapshot_rows`. (The doctor screen uses a FIXED sample version — it does NOT change; leave it.)

Run: `INSTA_UPDATE=always cargo test -p tui 2>&1 | tail -3`
Then VERIFY the snapshot diff is ONLY version strings:
Run: `git -C /Users/luolingfeng/Projects/LingXi-Next diff -- 'lingxi-code/tui/**/*.snap'`
Expected: only `v0.10.0`→`v0.11.0` (e.g. `lingxi-tui v0.10.0`→`v0.11.0`, `lingxi-cli v0.10.0`→`v0.11.0`) changes. If ANY non-version line changed, STOP and report (do not commit a behavior-changing snapshot).

- [ ] **Step 4: Re-run TUI tests clean**

Run: `cargo test -p tui 2>&1 | grep -E "test result: FAILED|; [1-9][0-9]* failed" | head` → empty (0 failed).

- [ ] **Step 5: Commit**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
git add -A
git commit -m "release(llm-p6): bump crates 0.10.0 -> 0.11.0 + refresh version snapshots"
```
(Confirm `git status` shows only Cargo.toml/Cargo.lock + the 3 `.snap` files; if a stray `test_entry` is untracked, do NOT add it.)

---

## Task 6: Final gate + finalize

**Files:** none (verification only).

- [ ] **Step 1: fmt + provider/orchestrator lint**

Run: `cargo fmt -p providers -p orchestrator -p cost`; `git status --porcelain | grep -v test_entry` — commit any fmt changes (`style(llm-p6): cargo fmt`).
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.

- [ ] **Step 2: Full test sweep**

Run: `cargo test -p providers` → all pass.
Run: `cargo test -p orchestrator` → all pass (incl. the new guardrail + `/model` tests).
Run: `cargo test -p cost` → all pass.
Run: `cargo test -p tui` → all pass (version snapshots refreshed).

- [ ] **Step 3: Back-compat gate**

Run: `cargo test -p test-harness 2>&1 | grep -E "FAILED|error\[|test result:" | grep -v "0 failed" | head` → empty (all parity suites green).

- [ ] **Step 4: Workspace build + deps**

Run: `cargo build --workspace` → Finished.
Run: `bash scripts/check-deps.sh` → `check-deps: OK — 73 workspace crates, no §8.1 dependency violations`.

- [ ] **Step 5: Tag (local only, no push)**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
git tag -a llm-p6 -m "LLM Providers P6 (polish/release): capability guardrail, /model syntax hints, docs, CHANGELOG, v0.11.0 bump. Feature complete."
git tag -a v0.11.0 -m "v0.11.0 — Multi-LLM-Provider support (OpenAI-compatible + Gemini + provider/model routing + per-provider cost), Anthropic default unchanged."
git --no-pager tag -l "llm-p6" "v0.11.0"
```

---

## Self-Review (completed during planning)

**1. Spec coverage (§3.3/§4/§6/§12 P6):**
- Capability fail-fast enforcement → Task 1 (in the only tool-carrying path; tested with a non-tool stub since all v1 providers are tool-capable).
- `/model` provider-aware listing → Task 2 (lightweight syntax-hint version; profile-accurate listing deferred with rationale).
- Docs (README + usage doc) → Task 3; CHANGELOG → Task 4.
- Release bump → Task 5 (68 manifests + the 3 version snapshots, M9-style cascade handling).
- Back-compat throughout → Task 6 gate (test-harness parity green; Anthropic untouched).
- *Deferred (documented):* streaming-error taxonomy hook, Azure variant, vision/reasoning, profile-accurate `/model`, the pre-existing `tool-api` debt.

**2. Placeholder scan:** none — code steps are complete; doc/CHANGELOG/README content is provided in full; the version bump is an exact scripted command + a verify step that gates on "version strings only".

**3. Type consistency:** the guardrail uses `resolved.provider.capabilities().native_tools` + `resolved.provider.id()` (the `LlmProvider` trait surface from P1) and `ApiError::Http(HttpError::InvalidRequest(..))` (no new variant); the test `FixedRouter`/`NoToolsProvider` implement `providers::{ModelRouter, Resolved, LlmProvider, Capabilities, ReasoningSupport, SystemStyle}` — all existing public API. `list_available_models` keeps its `async fn -> Vec<String>` signature. The version bump touches only the `version = "0.10.0"` package line (not dependency version pins, which use `.workspace`/path).
