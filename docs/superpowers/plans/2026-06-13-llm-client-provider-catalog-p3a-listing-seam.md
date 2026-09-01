# llm-client provider catalog — Phase 3-A (catalog → engine model-listing seam) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Fresh implementer subagent per task + two-stage review (spec then quality). Run edit-agents SEQUENTIALLY in one checkout (concurrent-agent worktree hazard). Steps use checkbox (`- [ ]`).

**Goal:** Expose the llm-client provider catalog (openrouter/deepseek/glm-coding/github-copilot, with pretty display names + provider labels) through a new `OrchestratorHandle::list_model_listings()` accessor, so the TUI picker (Phase 3-B) can render a grouped, display-named model list. **Listing only — live request routing to the new providers stays deferred to Plan 3c.**

**Architecture:** `builtin_presets()` is static data, so the listings can be produced on demand inside `ProviderApiAdapter` — no engine-app wiring or `ClientConfig` merge is needed. A new `ModelListing` DTO is added **additively** to the frozen `traits/` crate (new struct + new defaulted trait method — zero removed/modified lines), plus a defaulted method on the orchestrator's `OrchestratorApiClient` seam. `ProviderApiAdapter` overrides it to map `llm_client::ModelRegistry::from_config(builtin_presets())`'s `available_models()` into the DTO, attaching a hand-authored provider label. `ConversationOrchestrator`'s `OrchestratorHandle` impl delegates to the api seam. Existing `available_models() -> Vec<String>` is untouched (back-compat).

**Tech Stack:** Rust, `async_trait` (traits crate), existing `llm_client::{builtin_presets, ModelRegistry, ClientConfig, ModelListing}`.

**Spec:** `docs/superpowers/specs/2026-06-13-llm-client-provider-catalog-design.md` (Phase 3). **Base:** `parity-llm-client-3a` (Phases 1+2 merged; current HEAD has the catalog + Copilot).

---

## Conventions (every task)

- Cargo root `lingxi-code/`; cargo from there, git from worktree repo root with explicit paths. NEVER `git add -A`.
- Lints: `cargo clippy -p <crate> --all-targets --no-deps -- -D warnings`; `-D missing-docs`. New `pub` items need `///` docs. If a doc comment trips `doc_markdown`, wrap symbols in backticks.
- TDD: failing test, OBSERVE RED, implement.
- Commit `git commit -F <tempfile>`; trailer EXACTLY (own line, blank line before):
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```
- **Frozen crates `traits/` + `protocol/`: ADDITIVE ONLY.** New structs/methods are allowed; NO existing line may be removed or modified. Verify with `git diff parity-llm-client-3a -- lingxi-code/traits | grep -c '^-[^-]'` → MUST print `0` (zero removed lines). `protocol/` must stay fully untouched.
- `engine-desktop`, `engine-mobile`, `tui`, `orchestrator` must keep building.

## File structure (Phase 3-A)

- Modify `lingxi-code/platform-api/src/orchestrator.rs` — add `ModelListing` DTO + defaulted `list_model_listings()` on `OrchestratorHandle` (additive).
- Modify `lingxi-code/orchestrator/src/conversation.rs` — defaulted `list_model_listings()` on `OrchestratorApiClient`.

**Type path (PINNED):** `platform-api/src/lib.rs` only does `pub mod orchestrator;` (no crate-root re-exports), so the DTO is referenced everywhere as **`platform_api::orchestrator::ModelListing`**, added to each file's existing `use platform_api::orchestrator::{...}` import group (e.g. `handle_impl.rs:32` already imports `OrchestratorHandle, StatusSnapshot, …` from there). No `platform-api/src/lib.rs` edit is needed.
- Modify `lingxi-code/orchestrator/src/provider_adapter.rs` — override it; map `builtin_presets()` → DTO + provider-label helper + tests.
- Modify `lingxi-code/orchestrator/src/handle_impl.rs` — override `OrchestratorHandle::list_model_listings()` delegating to `self.api`.
- Modify `lingxi-code/orchestrator/Cargo.toml` only if `llm-client` is not already a dep (it is — `model/count_tokens.rs` uses it; no change expected).

## Out of scope (3-A)

- The TUI picker UI (Phase 3-B).
- Live request routing to the new providers (Plan 3c) — `switch_model` already accepts any id; routing availability is unchanged.
- Merging `builtin_presets()` into the engine's live `ClientConfig` (not needed for listing).

---

### Task 0: worktree + baseline

- [ ] **Step 1:** Create an isolated worktree via `superpowers:using-git-worktrees`, off `parity-llm-client-3a` HEAD. Suggested `provider-catalog-p3a`. Do NOT work in the primary checkout.
- [ ] **Step 2:** `cd lingxi-code && cargo build -p orchestrator -p platform-api` → clean.
- [ ] **Step 3:** `cd lingxi-code && cargo test -p orchestrator 2>&1 | grep -E 'test result:' | awk '{s+=$4} END{print "baseline orchestrator passed:", s}'` — record. No commit.

---

### Task 1: additive `ModelListing` DTO + `OrchestratorHandle::list_model_listings` (frozen traits/)

**Files:** Modify `lingxi-code/platform-api/src/orchestrator.rs` only. In every orchestrator file that uses the DTO, add `ModelListing` to the existing `use platform_api::orchestrator::{...}` import group and reference it bare as `ModelListing`.

- [ ] **Step 1: Add the DTO.** In `lingxi-code/platform-api/src/orchestrator.rs`, immediately BEFORE the `#[async_trait] pub trait OrchestratorHandle` line (line ~253), add this new struct (purely additive — inserts new lines, removes none):

```rust
/// One model entry for the grouped `/model` picker. Sourced from the llm-client
/// provider catalog: `display_model` is the human label, `request_model` is the
/// wire id passed to `switch_model`, `provider_id` is the stable grouping key,
/// and `provider_label` is the human provider header (e.g. "GitHub Copilot").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelListing {
    /// Human-facing model label (e.g. "DeepSeek Chat").
    pub display_model: String,
    /// Provider-local wire model id (what `switch_model` accepts).
    pub request_model: String,
    /// Stable provider key for grouping + recents (the catalog profile name).
    pub provider_id: String,
    /// Human provider header (e.g. "DeepSeek", "GitHub Copilot").
    pub provider_label: String,
}
```

- [ ] **Step 2: Add the defaulted trait method.** Inside `pub trait OrchestratorHandle`, immediately AFTER the existing `async fn list_available_models(&self) -> Vec<String>;` line (~330), add:

```rust
    /// Richer model listing for the grouped `/model` picker (display name +
    /// provider label + wire id), sourced from the llm-client catalog. The
    /// DEFAULT returns empty so existing impls/mocks compile unchanged; only the
    /// live `ConversationOrchestrator` overrides it.
    async fn list_model_listings(&self) -> Vec<ModelListing> {
        Vec::new()
    }
```

- [ ] **Step 3: Re-export check.** None needed — `platform-api/src/lib.rs` only does `pub mod orchestrator;`, and downstream already names these types via `platform_api::orchestrator::{...}`. Do NOT edit `lib.rs`.

- [ ] **Step 4: Frozen guard.** From the worktree repo root:

Run: `git diff parity-llm-client-3a -- lingxi-code/traits | grep -c '^-[^-]'`
Expected: `0` (no removed lines — purely additive).
Run: `git diff parity-llm-client-3a -- lingxi-code/protocol | grep -c '^[-+]'`
Expected: `0` (protocol untouched).

- [ ] **Step 5: Build traits + a defaulted-impl smoke test.** Add a unit test at the bottom of `orchestrator.rs` (inside an existing `#[cfg(test)] mod tests` if present, else add one) proving the default is empty:

```rust
#[cfg(test)]
mod model_listing_default_tests {
    use super::*;

    struct Dummy;
    #[async_trait]
    impl OrchestratorHandle for Dummy {
        // Only implement what the compiler requires; this test exists to prove
        // `list_model_listings` has a working default. If OrchestratorHandle has
        // many required methods, prefer to delete this test and rely on Task 2/3
        // coverage instead (note that in the commit message).
        async fn list_available_models(&self) -> Vec<String> { Vec::new() }
    }
}
```

NOTE to implementer: `OrchestratorHandle` likely has MANY required methods, making a `Dummy` impl impractical. If so, DELETE this smoke test and instead just confirm the crate compiles; the real default-vs-override coverage lands in Tasks 2–3. Use your judgment; do not write a 40-method stub.

- [ ] **Step 6:** `cd lingxi-code && cargo build -p platform-api` → clean. `cargo clippy -p platform-api --all-targets --no-deps -- -D warnings` → clean.

- [ ] **Step 7: Commit.**

```bash
git add lingxi-code/platform-api/src/orchestrator.rs
git commit -F - <<'EOF'
feat(traits): additive ModelListing DTO + OrchestratorHandle::list_model_listings

New struct + defaulted async method (empty) for the grouped /model picker.
Purely additive to the frozen traits crate — no existing line removed/modified;
existing impls compile unchanged.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 2: OrchestratorApiClient default + ProviderApiAdapter override

**Files:** Modify `lingxi-code/orchestrator/src/conversation.rs`, `lingxi-code/orchestrator/src/provider_adapter.rs`.

- [ ] **Step 1: Add the defaulted seam method.** In `conversation.rs`, inside `pub trait OrchestratorApiClient`, right after the existing defaulted `fn available_models(&self) -> Vec<String> { Vec::new() }` (~line 115), add:

```rust
    /// Richer catalog listing for the grouped `/model` picker. Default returns
    /// empty (mocks / non-routing impls); `ProviderApiAdapter` overrides it.
    fn list_model_listings(&self) -> Vec<ModelListing> {
        Vec::new()
    }
```

NOTE: use the path by which `platform_api::ModelListing` is reachable in the orchestrator crate. If orchestrator re-exports it (e.g. `pub use platform_api::...`), prefer that; otherwise use the fully-qualified `platform_api::orchestrator::ModelListing` (or `platform_api::ModelListing` if re-exported in Task 1). Pick ONE path and use it consistently in Tasks 2–3. Confirm by grepping how the orchestrator already names `OrchestratorHandle`'s types.

- [ ] **Step 2: Write the failing adapter test.** In `provider_adapter.rs`'s `#[cfg(test)] mod tests`, add (use the same `ModelListing` path as Step 1):

```rust
    #[test]
    fn list_model_listings_exposes_catalog_with_pretty_labels() {
        let provider = Arc::new(StubProvider::new());
        let router = Arc::new(StubRouter {
            provider,
            seen_resolve: Mutex::new(None),
        });
        let adapter = ProviderApiAdapter::new(router);
        let listings = OrchestratorApiClient::list_model_listings(&adapter);

        // DeepSeek Chat is present with the pretty provider label.
        let ds = listings
            .iter()
            .find(|m| m.request_model == "deepseek-chat")
            .expect("deepseek-chat listed");
        assert_eq!(ds.display_model, "DeepSeek Chat");
        assert_eq!(ds.provider_label, "DeepSeek");
        assert_eq!(ds.provider_id, "deepseek");

        // GitHub Copilot models carry the "GitHub Copilot" label.
        assert!(listings
            .iter()
            .any(|m| m.provider_label == "GitHub Copilot"));
        // The catalog is large (4 providers, 350+ models).
        assert!(listings.len() >= 347);
    }
```

(If `StubRouter`/`StubProvider` construction differs in this file, mirror the EXISTING `available_models_delegates_to_router` test's setup verbatim — copy its router/provider construction.)

- [ ] **Step 3:** Run; confirm FAIL (method resolves to the default empty → `expect` panics / len 0):

Run: `cd lingxi-code && cargo test -p orchestrator list_model_listings_exposes_catalog`
Expected: FAIL. Quote it.

- [ ] **Step 4: Implement the override.** In `provider_adapter.rs`, inside `impl OrchestratorApiClient for ProviderApiAdapter` (next to the existing `available_models`), add:

```rust
    fn list_model_listings(&self) -> Vec<ModelListing> {
        catalog_model_listings()
    }
```

and add this free function + helper in the same module (NOT inside the impl), near the top or bottom of the non-test code:

```rust
/// Build the grouped-picker listing from the static llm-client catalog.
/// Listing only — routing to these providers is gated by credentials (Plan 3c).
fn catalog_model_listings() -> Vec<ModelListing> {
    let catalog = llm_client::builtin_presets();
    let registry = match llm_client::ModelRegistry::from_config(llm_client::ClientConfig {
        providers: catalog.providers,
    }) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    registry
        .available_models()
        .into_iter()
        .map(|m| ModelListing {
            display_model: m.display_model,
            request_model: m.request_model,
            provider_label: provider_label(&m.profile_name).to_string(),
            provider_id: m.profile_name,
        })
        .collect()
}

/// Human provider header for a catalog profile name.
fn provider_label(profile_name: &str) -> &str {
    match profile_name {
        "openrouter" => "OpenRouter",
        "deepseek" => "DeepSeek",
        "glm-coding" => "GLM (coding)",
        "github-copilot" => "GitHub Copilot",
        other => other,
    }
}
```

NOTE: confirm the import path for `builtin_presets`/`ModelRegistry`/`ClientConfig` — they are `llm_client::{builtin_presets, ModelRegistry, ClientConfig}`. `llm_client::ModelListing` (the catalog one) has fields `display_model`, `request_model`, `profile_name`, `provider_id`, etc. (see `llm-client/src/registry.rs`). Use `m.profile_name` for the grouping key/label; that is the catalog `profile_name` ("deepseek", "github-copilot", ...).

- [ ] **Step 5:** Run the test → PASS:

Run: `cd lingxi-code && cargo test -p orchestrator list_model_listings_exposes_catalog`
Expected: PASS.

- [ ] **Step 6:** Clippy: `cd lingxi-code && cargo clippy -p orchestrator --all-targets --no-deps -- -D warnings` → clean.

- [ ] **Step 7: Commit.**

```bash
git add lingxi-code/orchestrator/src/conversation.rs lingxi-code/orchestrator/src/provider_adapter.rs
git commit -F - <<'EOF'
feat(orchestrator): ProviderApiAdapter exposes catalog model listings

OrchestratorApiClient::list_model_listings (default empty) overridden by the
adapter to map llm_client::builtin_presets() into platform_api::ModelListing with
pretty provider labels. Listing only; routing deferred to 3c.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 3: ConversationOrchestrator handle override

**Files:** Modify `lingxi-code/orchestrator/src/handle_impl.rs`.

- [ ] **Step 1: Write the failing test.** Add an integration-style test that the live handle surfaces the catalog. The simplest seam: a test that builds (or reuses an existing test helper for) `ConversationOrchestrator` with a `ProviderApiAdapter`, casts to `&dyn OrchestratorHandle`, and asserts `list_model_listings()` is non-empty and contains a deepseek entry. INSPECT this file's existing tests (e.g. how `list_available_models` is tested, if at all) and MIRROR the established construction. If there is no existing handle-construction test pattern and building a full `ConversationOrchestrator` is heavy, instead add the test in the module that already constructs one, or rely on a thin assertion:

```rust
    // (placement + construction MUST mirror an existing handle test in this file)
    #[tokio::test]
    async fn handle_list_model_listings_surfaces_catalog() {
        let handle = /* build ConversationOrchestrator exactly as an existing test does */;
        let listings = OrchestratorHandle::list_model_listings(&handle).await;
        assert!(listings.iter().any(|m| m.request_model == "deepseek-chat"));
    }
```

If `handle_impl.rs` has NO existing orchestrator-construction test to mirror (construction is too heavy), DELETE this test and instead prove the delegation by a doc note + rely on the override being a one-line delegation reviewed in Task 4. State which path you took in the commit message. Do NOT fabricate a brittle constructor.

- [ ] **Step 2:** Run; confirm FAIL (default empty → no deepseek) IF you added the test.

Run: `cd lingxi-code && cargo test -p orchestrator handle_list_model_listings`
Expected: FAIL (or N/A if test omitted per above).

- [ ] **Step 3: Implement the override.** In `handle_impl.rs`, inside `impl OrchestratorHandle for ConversationOrchestrator`, right after `async fn list_available_models(&self) -> Vec<String> { ... }`, add:

```rust
    async fn list_model_listings(&self) -> Vec<ModelListing> {
        self.api.list_model_listings()
    }
```

(Confirm `self.api` is the field used by `list_available_models`; mirror exactly. Use the same `ModelListing` path as Tasks 1–2.)

- [ ] **Step 4:** Run the test → PASS (or skip if omitted):

Run: `cd lingxi-code && cargo test -p orchestrator handle_list_model_listings`
Expected: PASS.

- [ ] **Step 5:** Clippy: `cd lingxi-code && cargo clippy -p orchestrator --all-targets --no-deps -- -D warnings` → clean.

- [ ] **Step 6: Commit.**

```bash
git add lingxi-code/orchestrator/src/handle_impl.rs
git commit -F - <<'EOF'
feat(orchestrator): ConversationOrchestrator.list_model_listings delegates to api

OrchestratorHandle override surfaces the catalog listings end-to-end.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 4: verification + frozen + consumers

- [ ] **Step 1:** Full orchestrator + traits test:

Run: `cd lingxi-code && cargo test -p orchestrator -p platform-api 2>&1 | grep -E 'test result:' | awk '{s+=$4; f+=$6} END{print "passed:", s, "failed:", f}'`
Expected: `failed: 0`.

- [ ] **Step 2:** Consumers build (the new defaulted handle method must not break any impl; engine + tui must compile):

Run: `cd lingxi-code && cargo build -p orchestrator -p engine-desktop -p engine-mobile -p tui`
Expected: clean.

- [ ] **Step 3:** Frozen guard:

Run: `git diff parity-llm-client-3a -- lingxi-code/traits | grep -c '^-[^-]'` → `0` (additive only).
Run: `git diff parity-llm-client-3a -- lingxi-code/protocol | grep -c '^[-+]'` → `0` (untouched).

- [ ] **Step 4:** No stray untracked:

Run: `git status --short | grep -E '^\?\?' || echo "(clean)"`

- [ ] **Step 5:** No commit. 3-A complete — ready for final review; the TUI picker is Phase 3-B.

---

## Self-review (plan author)

**Spec coverage (3-A):** richer catalog accessor with display names + provider labels reachable by the TUI → Tasks 1–3. Listing-only (routing deferred) → adapter sources static `builtin_presets()`, no `ClientConfig`/routing change. ✓

**Frozen-crate discipline:** Task 1 adds ONLY a new struct + a new defaulted method to `traits/` (no removed/modified lines); Task 4 verifies removed-lines == 0 and `protocol/` fully untouched. ✓

**Placeholder scan:** all code shown. Two tests (Task 1 smoke, Task 3 handle test) are explicitly conditioned on existing construction patterns the implementer must mirror — with a clear "delete it rather than fabricate a brittle stub" instruction; the load-bearing coverage is the Task 2 adapter test (concrete, against real `builtin_presets()` data: deepseek-chat → "DeepSeek Chat"/"DeepSeek", a "GitHub Copilot" label, ≥347 models). ✓

**Type consistency:** `ModelListing { display_model, request_model, provider_id, provider_label }` used identically in traits decl, `OrchestratorApiClient` default, adapter override, handle override. The catalog mapping reads `llm_client::ModelListing.{display_model, request_model, profile_name}` (per registry.rs). The `ModelListing` path (`platform_api::ModelListing` vs `platform_api::orchestrator::ModelListing` vs an orchestrator re-export) is pinned by the implementer in Task 1/2 and used consistently. ✓
