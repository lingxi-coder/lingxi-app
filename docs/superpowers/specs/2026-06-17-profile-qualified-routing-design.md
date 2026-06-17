# Profile-qualified model routing (design)

**Date:** 2026-06-17
**Status:** approved for planning
**Origin:** follow-up to the OpenAI-auth verification (`docs/superpowers/specs/2026-06-16-p3-enterprise-auth-design.md` and the de-collision fix `625025f8`). That fix made the codex-exclusive ids resolve, but genuinely-shared ids (e.g. `gpt-5.2` on both `openai` and `github-copilot`) remain ambiguous under bare-id resolution. This spec makes any picker selection route deterministically by carrying the provider profile to resolution.

## Goal

Let a model request optionally carry the provider **profile** so an id offered by multiple providers routes to the intended one. Picker selections already know the profile (`ModelOutcome::Commit { provider_id, request_model }`) — today the profile is dropped before resolution, so a shared id errors "ambiguous across profiles". This threads the profile through to `resolve`.

**Decided behaviour (brainstorm):**
- An **unqualified** request whose bare id is ambiguous **keeps erroring** (explicit, safe). No first-wins, no settings default-profile knob.
- The **picker** (and recent-model restore) always qualify, so users don't hit the error in normal use.
- Representation is **structured** (`Option<profile>` alongside the model), NOT a `"profile/model"` string — because openrouter request_model ids legitimately contain `/` (e.g. `openai/gpt-4o`), which would make a string qualifier ambiguous, and because the profile must stay out of model-name code paths (caching/betas/telemetry) that key off the bare id.

## Current flow (verified)

- `tui/src/screens/model.rs`: picker yields `ModelOutcome::Commit { provider_id, request_model }` — profile known here.
- `tui/src/root.rs:481-488`: `Commit` records `(provider_id, request_model)` to `recent_models` (which stores both), but sets `pending_switch_model = Some(request_model)` — **drops `provider_id`**.
- `traits::OrchestratorHandle::switch_model(&str)` (`traits/src/orchestrator.rs:303`): takes only the bare model.
- `orchestrator/src/provider_adapter.rs:391-401`: builds `LlmRequest::new(model)` from the bare stored model.
- `llm-client` `DefaultLlmClient::prepare` → `ModelRegistry::resolve(&request.model)` (`registry.rs:83`): matches the bare id across ALL providers; `>1` match → `InvalidRequest "ambiguous across profiles: …"`.
- Already structured/qualified elsewhere (precedent, unaffected): fallback **chains** (`ChainEntry { provider_id, model }`), routing aliases + `assemble::locate` (`profile/model`), `cost_wiring` (`profile/model` parse). Model-name consumers that must stay bare-id: `prompt_caching_enabled(model)`, betas assembler, telemetry, cost.

## Approach A — structured optional profile (chosen)

Carry an `Option<profile>` from the picker to `resolve`, never collapsing it into the model string. Rejected: `"profile/model"` string convention (Approach B) — the openrouter `/`-in-id collision needs a brittle "is segment 0 a known profile?" rule and the qualified string leaks into every model-name consumer.

## Components

### 1. `llm-client` — the resolution seam
- `LlmRequest` (`protocol.rs`): add field `profile: Option<String>` (default `None`, `#[serde(default)]`). Add builder `#[must_use] pub fn with_profile(mut self, profile: impl Into<String>) -> Self`. `LlmRequest::new` unchanged (profile `None`).
- `ModelRegistry` (`registry.rs`): add
  ```
  pub fn resolve_in(&self, requested: &str, profile: Option<&str>) -> Result<ResolvedRoute, LlmError>
  ```
  - Implementation: build the candidate match set exactly as today, but when `profile = Some(p)` **filter the providers to `profile_name == p` first** (then match `requested` against that one provider's models' `display_model`/`request_model`/`aliases`). Apply the SAME match-count arms to the resulting set (profile-scoped when `Some`, all-providers when `None`): `[]` → `ModelUnavailable`; `[one]` → route; `multiple` → `InvalidRequest "ambiguous …"`. So:
    - `Some(p)`, model present → route; absent (or unknown profile `p`) → `ModelUnavailable`; the `multiple` arm is normally unreachable within one profile but is handled identically for safety (e.g. a user profile with duplicate model entries).
    - `None` → today's behavior verbatim (cross-provider match; `multiple` → ambiguous error).
  - Re-express `resolve(requested)` as `self.resolve_in(requested, None)` (back-compat; all existing callers unchanged).
- `DefaultLlmClient::prepare_at` (`client.rs`): replace `self.registry.resolve(&request.model)` with `self.registry.resolve_in(&request.model, request.profile.as_deref())`.

### 2. `traits` — the selection seam
- `OrchestratorHandle::switch_model(&self, model: &str)` → `switch_model(&self, model: &str, profile: Option<&str>)`. (Single new optional param; keeps the call shape simple. All impls + callers updated.)

### 3. `orchestrator` — thread the profile per-call (NOT a stored field)
Refined during planning: the model is passed **per-call** from the turn loop (`turn_loop.rs:305` reads `s.model`) into the `OrchestratorApiClient` request methods, and fallback *switches* `session.model`. So the profile must ride alongside the per-call model, not live as a stale adapter field.
- `SessionState` (`config.rs:37`, `pub model: String`): add `pub model_profile: Option<String>`.
- `handle_impl::switch_model` (`handle_impl.rs:98`): set both `s.model` and `s.model_profile`.
- `OrchestratorApiClient` request methods gain a `profile: Option<&str>` param: `messages_create`, `messages_create_with_opts`, `messages_create_with_fallback`, `count_tokens` (the request-building ones). `build_request(model, profile, …)` does `let mut req = LlmRequest::new(model); if let Some(p) = profile { req = req.with_profile(p); }`.
- Turn loop (`turn_loop.rs:305` and the call sites ~656/660/672/709/762): read `s.model_profile` alongside `s.model` and pass it through. Source the profile from session state.
- **fallback_model** (`config.fallback_model: Option<String>`, a bare config string with no profile) is requested with `profile = None` — bare resolution (acceptable; fallback ids are well-known/anthropic and the unqualified-ambiguous error still applies if one ever collides).
- Model-name consumers (`prompt_caching_enabled`, betas, telemetry, cost, the stream adapter's `model` label field) keep receiving the bare `model` — unchanged.

### 4. `tui` — stop dropping the profile
- `root.rs:488`: `Commit { provider_id, request_model }` → call the switch path with BOTH (set a `pending_switch_model: Option<(String /*model*/, Option<String> /*profile*/)>` or a sibling `pending_switch_profile`), so `switch_model(model, Some(provider_id))` is invoked. `record_recent_model(&provider_id, &request_model)` already keeps both.
- Recent-model restore (`root.rs:1190-1193`) already carries `(provider_id, request_model)` → pass the provider_id as the profile when re-selecting.

## Data flow

Picker `Commit (profile, model)` → `switch_model(model, Some(profile))` → orchestrator stores `(model, model_profile)` → turn builds `LlmRequest::new(model).with_profile(profile)` → `prepare` → `resolve_in(model, Some(profile))` → unique route → unchanged auth/codec/transport path. Fallback chains already structured → unaffected.

## Error handling

- Qualified, model not in that profile (or unknown profile) → `LlmError::ModelUnavailable` with a message naming model + profile.
- Unqualified, ambiguous → existing `InvalidRequest "ambiguous across profiles: …"` (unchanged).
- Unqualified, unique → unchanged.

## Testing (TDD)

- `llm-client` `registry`/`resolve_in`: with the full builtin catalog — `resolve_in("gpt-5.2", Some("github-copilot"))` → copilot route; `Some("openai")` → openai route; `resolve_in("gpt-5.2", None)` → ambiguous error (unchanged); `resolve_in("gpt-5.2", Some("zai"))` (absent) → `ModelUnavailable`. `resolve("gpt-4.1")` (unique, None) still works.
- `llm-client` end-to-end `prepare`: full builtin catalog + `LlmRequest::new("gpt-5.2").with_profile("github-copilot")` → `provider_request.url` targets the github-copilot base_url (proves the profile picks the provider for a genuinely-shared id). Mirror for `Some("openai")` → api.openai.com.
- `LlmRequest::with_profile` sets the field; `new` leaves it `None`.
- `orchestrator`: `switch_model(m, Some(p))` stores both; the built `LlmRequest` carries the profile; model-name consumers still see the bare id.
- `tui`: `Commit` no longer drops `provider_id` — the switch path receives `(model, Some(provider_id))`; recent-restore carries the profile. (Unit-test the `pending_switch_*` wiring at whatever seam `root.rs` exposes.)
- Update existing `switch_model(&str)` callers/mocks across the workspace to the new signature (grep `switch_model`).

## File structure (new + modified)

- Modify: `lingxi-code/llm-client/src/protocol.rs` (`LlmRequest.profile` + `with_profile`), `llm-client/src/registry.rs` (`resolve_in`, `resolve` delegates), `llm-client/src/client.rs` (`prepare_at` uses `resolve_in`)
- Create: `lingxi-code/llm-client/tests/profile_qualified_resolution_test.rs`
- Modify: `lingxi-code/traits/src/orchestrator.rs` (`switch_model` signature + the test mock at :1036; `OrchestratorApiClient` request-method signatures gain `profile: Option<&str>`)
- Modify: `lingxi-code/orchestrator/src/config.rs` (`SessionState.model_profile`), `handle_impl.rs` (`switch_model` sets both), `provider_adapter.rs` (`build_request` + the `OrchestratorApiClient` impls take/use profile), `turn_loop.rs` (read `s.model_profile` at :305 and pass through the call sites), `test_support.rs` + `test_support_stream.rs` (mock impls)
- Modify: `lingxi-code/tui/src/state.rs` (`pending_switch_model` carries the profile — e.g. `Option<(String, Option<String>)>`), `lingxi-code/tui/src/root.rs` (commit at :488 + `pump_switch_model` at :1218 + recent-restore at :1190 thread the profile)
- Grep-and-update: every `switch_model(` and `OrchestratorApiClient` request-method caller/mock in the workspace for the new signatures.

## Out of scope

- Changing the unqualified-ambiguous policy (stays erroring).
- The de-collision data fix (already merged, `625025f8`).
- A settings-based default-profile / preference-order knob (rejected in brainstorm).
- Any `"profile/model"` string-qualifier syntax in user-facing config (structured only; routing aliases/chains keep their existing `profile/model` config form, which is parsed in `provider-config`, not in `resolve`).
