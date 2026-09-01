# Profile-qualified model routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Thread an optional provider profile from the `/model` picker through `switch_model` → orchestrator → `LlmRequest` → `ModelRegistry::resolve_in` so a model id offered by multiple providers (e.g. `gpt-5.2` on `openai` + `github-copilot`) routes to the intended provider; unqualified ambiguous ids keep erroring.

**Architecture:** Structured `Option<profile>` (NOT a `profile/model` string — openrouter ids contain `/`). The profile rides beside the model and never enters model-name code paths (caching/betas/telemetry). New `resolve_in(requested, profile)` filters providers by profile then applies the existing match-count logic; `resolve` delegates with `None`. The profile is sourced from session state per-call (the turn loop passes it alongside the model).

**Tech Stack:** Rust workspace (`lingxi-code/`). `cargo test` with `CARGO_PROFILE_DEV_DEBUG=0` (disk near-full).

**Reference spec:** `docs/superpowers/specs/2026-06-17-profile-qualified-routing-design.md`.

**Working dir:** `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code`. Branch: `profile-qualified-routing`.

## Verified anchor points
- `LlmRequest` (`llm-client/src/protocol.rs:28`): `#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]`, first field `pub model: String`. `LlmRequest::new(model)` at :63.
- `ModelRegistry::resolve` (`llm-client/src/registry.rs:83`): matches `requested` vs each model's `display_model`/`request_model`/`aliases` across all providers; `[]`→`ModelUnavailable`, `[one]`→route, `multiple`→`InvalidRequest "ambiguous across profiles: …"`.
- `DefaultLlmClient::prepare_at` (`llm-client/src/client.rs`): calls `self.registry.resolve(&request.model)` (then `validate_capabilities`, `encode_request`, `authenticate_at`).
- `OrchestratorHandle::switch_model(&self, model: &str)` (`platform-api/src/orchestrator.rs:303`); test mock at `:1036`.
- `OrchestratorApiClient` request methods (impls in `orchestrator/src/provider_adapter.rs`): `messages_create` (:1322), `count_tokens` (:1340), `messages_create_with_opts` (:1356), `messages_create_with_fallback`; all take `model: &str`. Private `build_request(model, system, msgs, tools, stream, max_tokens)` at :389 builds the `LlmRequest`.
- `handle_impl::switch_model` (`orchestrator/src/handle_impl.rs:98`): `let mut s = self.session.lock().await; s.model = model.to_string();`
- `SessionState.model: String` (`orchestrator/src/config.rs:37`).
- Turn loop reads `s.model` at `turn_loop.rs:305` (`(s.history.clone(), s.model.clone())`) and calls the `messages_create*` methods at ~656/660/672/709/762 with that `model`.
- TUI: `tui/src/state.rs:776` `pending_switch_model: Option<String>` (init `:949`); `tui/src/root.rs:488` `Commit { provider_id, request_model } => st.pending_switch_model = Some(request_model)` (drops provider_id); `pump_switch_model` at `:1218` (calls `handle.switch_model(&model)` at :1230); recent-restore at `:1190`.

---

## Task 1: `LlmRequest.profile` + `with_profile`

**Files:** Modify `llm-client/src/protocol.rs`.

- [ ] **Step 1: Write the failing test.** Append to `protocol.rs`'s `#[cfg(test)] mod tests` (or add one):

```rust
    #[test]
    fn with_profile_sets_field_and_new_defaults_none() {
        assert_eq!(LlmRequest::new("m").profile, None);
        assert_eq!(LlmRequest::new("m").with_profile("openai").profile.as_deref(), Some("openai"));
    }
```

- [ ] **Step 2: Run, expect FAIL** (`profile`/`with_profile` undefined): `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client --lib with_profile_sets_field 2>&1 | tail -8`

- [ ] **Step 3: Add the field.** In the `LlmRequest` struct (after `pub model: String,`):

```rust
    /// Optional provider profile that disambiguates `model` when the same id is
    /// offered by multiple providers. `None` = resolve across all providers
    /// (ambiguous ids error).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
```

- [ ] **Step 4: Add the builder.** In `impl LlmRequest` (after `new`):

```rust
    /// Pin the provider profile used to resolve `model`.
    #[must_use]
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }
```

- [ ] **Step 5: Run, expect PASS.** `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client --lib with_profile_sets_field 2>&1 | tail -6`
- [ ] **Step 6: Commit:** `git add llm-client/src/protocol.rs && git commit -m "feat(llm-client): LlmRequest.profile + with_profile"`

---

## Task 2: `ModelRegistry::resolve_in` (profile-scoped resolution)

**Files:** Modify `llm-client/src/registry.rs`; Create `llm-client/tests/profile_qualified_resolution_test.rs`.

- [ ] **Step 1: Write the failing regression test.** Create `llm-client/tests/profile_qualified_resolution_test.rs`:

```rust
//! Profile-qualified resolution: a shared id (gpt-5.2 on openai + github-copilot)
//! resolves by profile; unqualified stays ambiguous (decided behaviour).
use llm_client::{builtin_presets, ClientConfig, LlmError, ModelRegistry};

fn registry() -> ModelRegistry {
    ModelRegistry::from_config(ClientConfig { providers: builtin_presets().providers })
        .expect("registry")
}

#[test]
fn shared_id_resolves_by_profile() {
    let reg = registry();
    assert_eq!(reg.resolve_in("gpt-5.2", Some("openai")).expect("openai").profile_name, "openai");
    assert_eq!(
        reg.resolve_in("gpt-5.2", Some("github-copilot")).expect("copilot").profile_name,
        "github-copilot"
    );
}

#[test]
fn unqualified_shared_id_still_ambiguous() {
    match registry().resolve_in("gpt-5.2", None) {
        Err(LlmError::InvalidRequest { message }) => assert!(message.contains("ambiguous")),
        other => panic!("expected ambiguous error, got {other:?}"),
    }
}

#[test]
fn qualified_absent_model_is_unavailable() {
    // gpt-5.2 is not in the zai profile.
    assert!(matches!(registry().resolve_in("gpt-5.2", Some("zai")), Err(LlmError::ModelUnavailable)));
    // unknown profile.
    assert!(matches!(registry().resolve_in("gpt-5.2", Some("nope")), Err(LlmError::ModelUnavailable)));
}

#[test]
fn unique_unqualified_still_resolves() {
    assert_eq!(registry().resolve_in("gpt-4.1", None).expect("unique").profile_name, "openai");
    // resolve() delegates to resolve_in(.., None)
    assert_eq!(registry().resolve("gpt-4.1").expect("unique").profile_name, "openai");
}
```

- [ ] **Step 2: Run, expect FAIL** (`resolve_in` undefined): `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client --test profile_qualified_resolution_test 2>&1 | tail -10`

- [ ] **Step 3: Implement `resolve_in` + delegate `resolve`.** In `registry.rs`, replace the existing `pub fn resolve(&self, requested: &str) -> Result<ResolvedRoute, LlmError>` with:

```rust
    /// Resolve a model id, optionally scoped to one provider profile.
    /// `profile = Some(p)` matches only within profile `p` (absent model or
    /// unknown profile → `ModelUnavailable`); `None` matches across all
    /// providers (ambiguous → error). See `resolve` for the unscoped form.
    pub fn resolve_in(
        &self,
        requested: &str,
        profile: Option<&str>,
    ) -> Result<ResolvedRoute, LlmError> {
        let mut matches = Vec::new();
        for provider in &self.config.providers {
            if let Some(p) = profile {
                if provider.profile_name != p {
                    continue;
                }
            }
            for model in &provider.models {
                let is_match = model.display_model == requested
                    || model.request_model == requested
                    || model.aliases.iter().any(|alias| alias == requested);
                if is_match {
                    matches.push((provider, model));
                }
            }
        }

        match matches.as_slice() {
            [] => Err(LlmError::ModelUnavailable),
            [(provider, model)] => Ok(ResolvedRoute {
                provider_id: provider.provider_id.clone(),
                profile_name: provider.profile_name.clone(),
                request_model: model.request_model.clone(),
                display_model: model.display_model.clone(),
                pricing_model: PricingModelRef {
                    pricing_provider_id: provider.provider_id.clone(),
                    billing_model: model.billing_model.clone(),
                    request_model: model.request_model.clone(),
                    display_model: model.display_model.clone(),
                },
                capabilities: model.capabilities,
            }),
            multiple => Err(LlmError::InvalidRequest {
                message: format!(
                    "model reference '{requested}' is ambiguous across profiles: {}",
                    multiple
                        .iter()
                        .map(|(provider, _)| provider.profile_name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }),
        }
    }

    /// Resolve across all providers (unscoped). Ambiguous ids error.
    pub fn resolve(&self, requested: &str) -> Result<ResolvedRoute, LlmError> {
        self.resolve_in(requested, None)
    }
```

(This is the existing `resolve` body with one added `profile` filter at the top of the provider loop; `resolve` now delegates.)

- [ ] **Step 4: Run, expect PASS.** `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client --test profile_qualified_resolution_test 2>&1 | tail -8`
- [ ] **Step 5: Run the existing registry/client tests** (resolve delegation must not regress): `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client 2>&1 | grep -E "test result:|FAILED" | grep -v "0 passed; 0 failed" | tail`
- [ ] **Step 6: Commit:** `git add llm-client/src/registry.rs llm-client/tests/profile_qualified_resolution_test.rs && git commit -m "feat(llm-client): ModelRegistry::resolve_in (profile-scoped resolution)"`

---

## Task 3: `prepare` uses the request profile + E2E resolution test

**Files:** Modify `llm-client/src/client.rs`; Modify `llm-client/tests/profile_qualified_resolution_test.rs`.

- [ ] **Step 1: Write the failing E2E test.** Append to `profile_qualified_resolution_test.rs`:

```rust
// E2E: prepare() routes a shared id to the profile-selected provider's base_url.
#[derive(Debug)]
struct ApiKeyStub;
impl llm_client::CredentialProvider for ApiKeyStub {
    fn load<'a>(
        &'a self,
        _s: &'a llm_client::CredentialScope,
    ) -> llm_client::BoxFuture<'a, Result<llm_client::Credential, LlmError>> {
        Box::pin(async { Ok(llm_client::Credential::ApiKey("k".into())) })
    }
}

#[tokio::test]
async fn prepare_routes_shared_id_by_profile() {
    use llm_client::client::DefaultLlmClient;
    use llm_client::LlmRequest;
    use std::sync::Arc;
    let client = DefaultLlmClient::from_config(ClientConfig { providers: builtin_presets().providers })
        .expect("client")
        .with_credential_provider(Arc::new(ApiKeyStub));
    // gpt-5.2 via openai → api.openai.com
    let p = client
        .prepare(&LlmRequest::new("gpt-5.2").with_profile("openai"))
        .await
        .expect("openai prepare");
    assert!(p.provider_request.url.starts_with("https://api.openai.com/"), "got {}", p.provider_request.url);
    // gpt-5.2 via github-copilot → api.githubcopilot.com
    let p = client
        .prepare(&LlmRequest::new("gpt-5.2").with_profile("github-copilot"))
        .await
        .expect("copilot prepare");
    assert!(p.provider_request.url.starts_with("https://api.githubcopilot.com"), "got {}", p.provider_request.url);
}
```

(`Credential`, `CredentialProvider`, `CredentialScope`, `BoxFuture` are public — confirm the exact import paths against `client_auth_test.rs`. The copilot base_url is `https://api.githubcopilot.com`; verify in `presets.rs` and adjust the assertion to whatever the preset declares.)

- [ ] **Step 2: Run, expect FAIL** (prepare ignores profile → ambiguous error on gpt-5.2): `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client --test profile_qualified_resolution_test prepare_routes 2>&1 | tail -10`

- [ ] **Step 3: Use the profile in `prepare_at`.** In `client.rs`, find `let resolved_route = self.registry.resolve(&request.model)?;` (inside `prepare_at`) and change to:

```rust
        let resolved_route = self
            .registry
            .resolve_in(&request.model, request.profile.as_deref())?;
```

- [ ] **Step 4: Run, expect PASS.** `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client --test profile_qualified_resolution_test 2>&1 | tail -8`
- [ ] **Step 5: Commit:** `git add llm-client/src/client.rs llm-client/tests/profile_qualified_resolution_test.rs && git commit -m "feat(llm-client): prepare() resolves with LlmRequest.profile"`

---

## Task 4: `switch_model` carries the profile (traits + orchestrator state)

**Files:** Modify `platform-api/src/orchestrator.rs`, `orchestrator/src/config.rs`, `orchestrator/src/handle_impl.rs`, `orchestrator/src/test_support.rs`.

- [ ] **Step 1: Change the trait signature.** In `platform-api/src/orchestrator.rs:303`:

```rust
    async fn switch_model(&self, model: &str, profile: Option<&str>) -> Result<(), HandleError>;
```

And the test mock at `:1036`:

```rust
            async fn switch_model(&self, _: &str, _: Option<&str>) -> Result<(), HandleError> { Ok(()) }
```

- [ ] **Step 2: Add `SessionState.model_profile`.** In `orchestrator/src/config.rs` (right after `pub model: String,` at :37):

```rust
    /// Provider profile pinned alongside `model` (disambiguates shared ids).
    /// `None` = resolve unscoped. Set by `switch_model`.
    pub model_profile: Option<String>,
```

Update any `SessionState { … }` literal constructors in the crate to include `model_profile: None` (grep `SessionState {` and fix each; many use `..Default::default()` and need no change — verify).

- [ ] **Step 3: Set both in `switch_model`.** In `orchestrator/src/handle_impl.rs:98`:

```rust
    async fn switch_model(&self, model: &str, profile: Option<&str>) -> Result<(), HandleError> {
        let mut s = self.session.lock().await;
        s.model = model.to_string();
        s.model_profile = profile.map(str::to_string);
        Ok(())
    }
```

- [ ] **Step 4: Update the `test_support.rs` mock** (`orchestrator/src/test_support.rs:792`): add the `profile: Option<&str>` param (store it if the mock tracks model, else ignore with `_`). Match the real impl's behavior the mock emulates.

- [ ] **Step 5: Build the crates** (compile errors reveal remaining callers): `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p platform-api -p orchestrator 2>&1 | tail -15`. Fix any `switch_model(` call site the compiler flags (there should be none outside tui yet; tui is Task 6).
- [ ] **Step 6: Commit:** `git add platform-api/src/orchestrator.rs orchestrator/src/config.rs orchestrator/src/handle_impl.rs orchestrator/src/test_support.rs && git commit -m "feat(orchestrator): switch_model carries provider profile into SessionState"`

---

## Task 5: thread the profile through the turn loop → request

**Files:** Modify `platform-api/src/orchestrator.rs` (`OrchestratorApiClient` sigs), `orchestrator/src/provider_adapter.rs`, `orchestrator/src/turn_loop.rs`, `orchestrator/src/test_support.rs` + `test_support_stream.rs` (mocks).

- [ ] **Step 1: Add `profile: Option<&str>` to the `OrchestratorApiClient` request methods.** In the trait def (in `platform-api/src/orchestrator.rs` — grep `trait OrchestratorApiClient`), add `profile: Option<&str>` as the 2nd param (after `model: &str`) to: `messages_create`, `messages_create_with_opts`, `messages_create_with_fallback`, `count_tokens`. (Leave non-request methods alone.)

- [ ] **Step 2: Thread into `build_request`.** In `orchestrator/src/provider_adapter.rs`, change `build_request`'s signature to take `profile: Option<&str>` (after `model`) and set it on the request:

```rust
    fn build_request(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        stream: bool,
        max_tokens: Option<u32>,
    ) -> Result<LlmRequest, LlmError> {
        // … existing body … but change the `LlmRequest::new(model)` line to:
        let mut req = LlmRequest::new(model);
        if let Some(p) = profile {
            req = req.with_profile(p);
        }
        // … rest unchanged …
```

Update each `OrchestratorApiClient` impl method (`messages_create` :1322, `count_tokens` :1340, `messages_create_with_opts` :1356, `messages_create_with_fallback`) to accept `profile` and pass it into its `build_request(model, profile, …)` call. The retry/telemetry/`model`-label code in those methods keeps using bare `model`.

- [ ] **Step 3: Update the turn loop.** In `orchestrator/src/turn_loop.rs:305`, read the profile too:

```rust
        (s.history.clone(), s.model.clone(), s.model_profile.clone())
```

Thread the new `model_profile` (an `Option<String>`) to the call sites (~656/660/672/709/762) and pass `model_profile.as_deref()` as the `profile` arg to `messages_create` / `messages_create_with_opts` / `messages_create_with_fallback`. For the fallback model itself pass `None` (the fallback config string has no profile). Adjust the tuple-binding and any helper signatures the profile must traverse (the compiler will pinpoint them).

- [ ] **Step 4: Update the mock `OrchestratorApiClient` impls** in `orchestrator/src/test_support.rs` and `test_support_stream.rs` to the new signatures (`profile: Option<&str>` param; ignore with `_` unless the mock asserts on it).

- [ ] **Step 5: Build + test the orchestrator.** `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p orchestrator 2>&1 | tail -15` then `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p orchestrator 2>&1 | grep -E "test result:|FAILED|error\[" | grep -v "0 passed; 0 failed" | tail`. Fix any remaining callers the compiler flags.

- [ ] **Step 6: Add an orchestrator regression test.** Add a unit test (in `provider_adapter.rs` test mod, mirroring `build_request_sets_two_cache_breakpoints_by_default` at :1880) asserting `build_request("gpt-5.2", Some("github-copilot"), …)` yields a request with `req.profile.as_deref() == Some("github-copilot")` and `req.model == "gpt-5.2"`. Run it.
- [ ] **Step 7: Commit:** `git add orchestrator/ platform-api/src/orchestrator.rs && git commit -m "feat(orchestrator): thread model profile through the turn loop into LlmRequest"`

---

## Task 6: TUI — stop dropping the profile

**Files:** Modify `tui/src/state.rs`, `tui/src/root.rs`.

- [ ] **Step 1: Widen `pending_switch_model`.** In `tui/src/state.rs:776`, change the field to carry the profile:

```rust
    pub pending_switch_model: Option<(String, Option<String>)>,
```

(init at `:949` stays `None`.) Update its doc comment to note the tuple is `(request_model, provider_id-as-profile)`.

- [ ] **Step 2: Carry the profile on Commit.** In `tui/src/root.rs:481-488`, the `ModelOutcome::Commit { provider_id, request_model }` arm: keep `record_recent_model(&provider_id, &request_model)`, and set:

```rust
                    st.pending_switch_model = Some((request_model, Some(provider_id)));
```

- [ ] **Step 3: Pass the profile in the pump.** In `pump_switch_model` (`tui/src/root.rs:1218`), the take at `:1224` now yields `(model, profile)`; update the `handle.switch_model(&model)` call at `:1230` to `handle.switch_model(&model, profile.as_deref()).await`. Adjust the binding (`let (model, profile) = …`).

- [ ] **Step 4: Recent-model restore carries the profile.** Wherever a recent selection is re-applied via `switch_model` (recent-restore path near `:1190`, and any other `switch_model` call in `root.rs`), pass the stored `provider_id` as the profile. (The recent list is `(provider_id, request_model)` — pass `Some(&provider_id)`.)

- [ ] **Step 5: Build + test the tui.** `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p tui 2>&1 | tail -15` then `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p tui 2>&1 | grep -E "test result:|FAILED|error\[" | grep -v "0 passed; 0 failed" | tail`. Fix any flagged `switch_model`/`pending_switch_model` site.
- [ ] **Step 6: Commit:** `git add tui/src/state.rs tui/src/root.rs && git commit -m "feat(tui): /model picker threads the provider profile into switch_model"`

---

## Task 7: workspace-wide caller sweep + full verification

**Files:** any remaining callers (verification-driven).

- [ ] **Step 1: Grep for stragglers.** `grep -rn "switch_model(" --include="*.rs" . | grep -v "/target/" | grep -v "profile"` — every hit must now pass a profile arg. Same for `OrchestratorApiClient` request-method callers across `apps/`, `coordinator/`, `agent/` (grep `messages_create(` / `messages_create_with_opts(` / `messages_create_with_fallback(` / `\.count_tokens(`). Update any to the new signatures (pass `None` where no profile context exists).
- [ ] **Step 2: Workspace build.** `CARGO_PROFILE_DEV_DEBUG=0 cargo build --workspace 2>&1 | tail -3`. Expected: `Finished`, zero errors.
- [ ] **Step 3: Affected-crate tests.** `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client -p platform-api -p orchestrator -p tui 2>&1 | grep -E "test result:|FAILED|error\[|error:" | grep -v "0 passed; 0 failed"`. Expected: only `test result: ok`.
- [ ] **Step 4: Clippy the touched crates.** `CARGO_PROFILE_DEV_DEBUG=0 cargo clippy -p llm-client -p orchestrator -p tui 2>&1 | grep -E "^warning:|^error:" | grep -v "unused manifest key"`. Fix new lints (field docs / etc.).
- [ ] **Step 5: Commit** any sweep fixes: `git add -A && git commit -m "chore: update switch_model / OrchestratorApiClient callers for profile param"` (stage only the touched source files — NOT untracked junk; list them explicitly if `-A` would sweep unrelated files).

No further commit. Feature complete.

---

## Notes for the implementer
- **Structured, not stringly:** never build a `"profile/model"` string — the profile is a separate `Option`. Model-name code (`prompt_caching_enabled`, betas, telemetry, cost, the stream adapter's `model` label) keeps the bare `model`.
- **Compiler-driven:** signature changes (Tasks 4–5) will flag every caller; the grep in Task 7 is the backstop. Pass `None` where there's no profile context.
- **fallback_model** is requested with `profile = None` (bare) — intentional (the config string carries no profile).
- Verify the copilot base_url literal in Task 3's assertion against `presets.rs` before relying on it.
- Every cargo command keeps `CARGO_PROFILE_DEV_DEBUG=0`. Don't `git add -A` blindly — the working tree has unrelated untracked files.
