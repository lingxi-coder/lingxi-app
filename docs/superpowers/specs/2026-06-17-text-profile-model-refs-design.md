# Text/programmatic `profile/model` references (design)

**Date:** 2026-06-17
**Status:** approved for planning
**Origin:** follow-up to profile-qualified routing (`docs/superpowers/specs/2026-06-17-profile-qualified-routing-design.md`, merged `8cc3f40e`). That made the `/model` PICKER route shared ids by profile. The non-picker model-setting paths (`/model <text>`, mobile/bridge `SetModel`, config `default_model`) still pass `profile = None`, so a shared id (e.g. `gpt-5.2` on `openai` + `github-copilot`) errors `"ambiguous across profiles: …"` with no text way to disambiguate. This adds a `profile/model` text syntax to those paths.

## Goal

Let the text/programmatic model-setting entry points accept a `profile/model` reference that routes deterministically, reusing the structured `switch_model(model, Option<profile>)` seam. Unqualified ambiguous ids keep erroring — but the error now suggests the qualified forms.

## Decided (brainstorm)

- **Scope:** all four non-picker entry points — `/model <arg>`, `engine-mobile` `SetModel`, `bridge-server` `SetModel`, and config `default_model`.
- **Approach A (chosen):** a pure `parse_model_ref` function in `traits`; each entry point lists the models, parses, and calls `switch_model(model, profile)`. Rejected Approach B (a new `OrchestratorHandle::switch_model_ref` method) — it grows the already-large trait and `default_model` (no handle at init) still needs separate handling.
- **No** settings-based default-provider preference (rejected earlier).

## Disambiguation rule

`parse_model_ref(input, listings) -> (model: String, profile: Option<String>)`:
- No `/` in `input` → `(input, None)` (bare; resolved unscoped — unique resolves, ambiguous errors with a suggestion).
- Contains `/` → split at the FIRST `/` into `(prefix, rest)`:
  - If `prefix` is a known profile name (`== some listing.provider_id`) AND `rest` matches a `request_model` under that profile → `(rest, Some(prefix))` (qualified).
  - Else → `(input, None)` (treat the whole string as a bare id — covers openrouter ids that legitimately contain `/`, e.g. `openai/gpt-4o`).
- Edge: a 2-segment `openai/gpt-4o` resolves **toward qualified** (profile `openai`, model `gpt-4o`) when `openai` is a profile and has `gpt-4o`. To select openrouter's same-named bare id, the user writes the fully-qualified `openrouter/openai/gpt-4o` (first `/` splits off `openrouter`; `rest = openai/gpt-4o` matches openrouter's request_model). Deterministic and documented.
- Safe on empty string / bare `/` / unknown prefix → `(input, None)`.

## Components

### 1. `traits` — pure parser
`pub fn parse_model_ref(input: &str, listings: &[ModelListing]) -> (String, Option<String>)` (next to `ModelListing`). Reads only `provider_id` (= profile name) and `request_model` from `listings`; no side effects; the disambiguation rule above. Fully unit-tested.

### 2. `llm-client` — error suggests the qualified forms
`ModelRegistry::resolve_in` `multiple` (ambiguous) arm (`registry.rs`): change the message from `model reference 'X' is ambiguous across profiles: openai, github-copilot` to additionally suggest the qualified refs, e.g. `… ambiguous across profiles: openai, github-copilot — qualify it, e.g. openai/X or github-copilot/X`. One change; benefits every path (including programmatic, which surfaces the error string).

### 3. Entry points (each: list → parse → switch)
- `/model <arg>` (`commands/core/src/model.rs`, switch branch): `let listings = self.handle.list_model_listings().await; let (model, profile) = traits::parse_model_ref(trimmed, &listings); self.handle.switch_model(&model, profile.as_deref()).await`. Display string uses the resolved `model` (or echoes the user input — keep the existing "Switched to model: {…}" template; show the bare model).
- `engine-mobile` `SetModel` (`apps/engine-mobile/src/host.rs`, both sites): same list→parse→switch.
- `bridge-server` `SetModel` (`apps/bridge-server/src/router.rs`): same; the `ModelChanged` event carries the resolved bare `model`.
- config `default_model`: at session initialization (where the initial `SessionState.model` is set from `OrchestratorConfig.default_model`), run `parse_model_ref(default_model, &listings)` against the engine's assembled listings and set `SessionState.model` + `SessionState.model_profile`. (Locate the exact init site in the plan; the engine already builds the model listings/`model_providers` map during composition.)

### 4. Data flow
Text/config entry → `list_model_listings()` (profile-aware) → `parse_model_ref` → (qualified `Some` or bare `None`) → `switch_model(model, profile)` → `SessionState.model_profile` → turn loop → `resolve_in`. The picker path is unchanged (already structured).

## Error handling

- Qualified ref whose model isn't in the named profile → falls through to bare-id resolution (may resolve uniquely, or error-with-suggestion if ambiguous, or `ModelUnavailable`).
- Bare ambiguous → `resolve_in` error now includes the qualified suggestions.
- `parse_model_ref` never panics; degenerate inputs return `(input, None)`.

## Testing (TDD)

- `traits::parse_model_ref` pure unit tests: bare-unique → `(id, None)`; `openai/gpt-5.2` → `("gpt-5.2", Some("openai"))`; fully-qualified openrouter `openrouter/openai/gpt-4o` → `("openai/gpt-4o", Some("openrouter"))`; 2-segment `openai/gpt-4o` → qualified (when openai has gpt-4o); unknown prefix `foo/bar` → `("foo/bar", None)`; empty / `/` → `(input, None)`. Use a small hand-built `Vec<ModelListing>` fixture.
- `llm-client`: the ambiguous-error message contains the qualified suggestion forms (update the existing assertion in `profile_qualified_resolution_test.rs` / registry tests).
- `/model` handler: a mock handle whose `list_model_listings` returns openai+copilot for `gpt-5.2`; `/model openai/gpt-5.2` → `switch_model("gpt-5.2", Some("openai"))` called; `/model gpt-4.1` (unique) → `switch_model("gpt-4.1", None)`.
- mobile + bridge `SetModel`: same list→parse→switch assertion via their mocks.
- config: `default_model = "openai/gpt-5.2"` → initial `SessionState.model == "gpt-5.2"`, `model_profile == Some("openai")` (unit-test the init helper, or a focused engine-build test).

## Out of scope

- The picker path (done).
- Settings-based default-provider preference (rejected).
- The existing `profile/model` parsing for routing aliases/chains in `provider-config` (`assemble::locate`, `cost_wiring`) — unchanged; that's a separate config layer.

## File structure (new + modified)

- Modify: `lingxi-code/traits/src/orchestrator.rs` (`parse_model_ref` next to `ModelListing`)
- Modify: `lingxi-code/llm-client/src/registry.rs` (ambiguous-error suggestion) + the test asserting the message
- Modify: `lingxi-code/commands/core/src/model.rs` (`/model` switch branch)
- Modify: `lingxi-code/apps/engine-mobile/src/host.rs` (both `SetModel` sites), `lingxi-code/apps/bridge-server/src/router.rs` (`SetModel`)
- Modify: the session-init site that seeds `SessionState.model` from `default_model` (engine composition; pin the exact file/line in the plan)
- Create: `lingxi-code/traits/tests/parse_model_ref_test.rs` (or an inline `#[cfg(test)]` mod) for the pure-parser cases
