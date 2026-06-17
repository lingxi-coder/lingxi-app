# Text/programmatic `profile/model` references Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the non-picker model-setting paths (`/model <text>`, mobile/bridge `SetModel`, config `default_model`) accept a `profile/model` reference that routes deterministically, and make the ambiguous-id error suggest the qualified forms.

**Architecture:** A pure `traits::parse_model_ref(input, &[ModelListing]) -> (model, Option<profile>)` (first-`/` splits off a profile only when the prefix is a known profile and the remainder is a model under it; else the whole string is a bare id — handles openrouter `/`-in-ids). Each entry point lists models, parses, and calls the existing `switch_model(model, Option<profile>)`. Reuses the merged profile-qualified-routing seam (`SessionState.model_profile` → `resolve_in`).

**Tech Stack:** Rust workspace (`lingxi-code/`). `cargo test` with `CARGO_PROFILE_DEV_DEBUG=0` (disk near-full).

**Reference spec:** `docs/superpowers/specs/2026-06-17-text-profile-model-refs-design.md`.

**Working dir:** `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code`. Branch: `text-profile-model-refs`.

## Verified anchors
- `ModelListing` (`traits/src/orchestrator.rs:273`): `{ display_model, request_model, provider_id (= profile name), provider_label }`.
- `OrchestratorHandle::switch_model(&self, model: &str, profile: Option<&str>)` (`traits/src/orchestrator.rs:306`) — already takes the profile. `list_model_listings() -> Vec<ModelListing>` (`:374`).
- `ModelRegistry::resolve_in` ambiguous arm (`llm-client/src/registry.rs:~120-128`): `Err(LlmError::InvalidRequest { message: format!("model reference '{requested}' is ambiguous across profiles: {}", … join(", ")) })`. Existing assertions: `llm-client/tests/profile_qualified_resolution_test.rs:20` (`message.contains("ambiguous")`), `llm-client/tests/registry_test.rs:56` (`resolve_rejects_ambiguous_model_references`).
- `/model` handler (`commands/core/src/model.rs:53`): `self.handle.switch_model(trimmed, None)`.
- mobile `SetModel` (`apps/engine-mobile/src/host.rs:954` + a 2nd site ~`:1124`): `handle.switch_model(&model, None)`.
- bridge `SetModel` (`apps/bridge-server/src/router.rs:324`): `self.handle.switch_model(&model, None)`.
- config: `orch_cfg.model.clone_from(&cfg.default_model)` (`apps/engine-desktop/src/lib.rs:1694`); `ConversationOrchestrator::new(orch_cfg, …)` (`:2711`) → `SessionState::empty(SessionId::new(), config.model.clone())` (`orchestrator/src/conversation.rs:662`, sets `model_profile: None`). The handle exists at `apps/engine-desktop/src/lib.rs:2730` (`let handle: Arc<dyn OrchestratorHandle> = orch.clone();`). The `model_providers` map is built at `:1590`; `assembled.client_config.providers` is in scope.

---

## File Structure
- Modify: `traits/src/orchestrator.rs` (`parse_model_ref` + unit tests)
- Modify: `llm-client/src/registry.rs` (ambiguous-error suggestion) + `llm-client/tests/registry_test.rs` if it asserts the exact message
- Modify: `commands/core/src/model.rs` (`/model` switch branch + test)
- Modify: `apps/engine-mobile/src/host.rs`, `apps/bridge-server/src/router.rs` (`SetModel`)
- Modify: `apps/engine-desktop/src/lib.rs` (config `default_model` parse + seed)

---

## Task 1: `parse_model_ref` pure function

**Files:** Modify `traits/src/orchestrator.rs`.

- [ ] **Step 1: Write the failing tests.** Add a `#[cfg(test)] mod parse_model_ref_tests` near `ModelListing` (or extend an existing test mod):

```rust
#[cfg(test)]
mod parse_model_ref_tests {
    use super::{parse_model_ref, ModelListing};

    fn listing(provider_id: &str, request_model: &str) -> ModelListing {
        ModelListing {
            display_model: request_model.to_string(),
            request_model: request_model.to_string(),
            provider_id: provider_id.to_string(),
            provider_label: provider_id.to_string(),
        }
    }
    fn fixture() -> Vec<ModelListing> {
        vec![
            listing("openai", "gpt-5.2"),
            listing("openai", "gpt-4o"),
            listing("github-copilot", "gpt-5.2"),
            listing("openrouter", "openai/gpt-4o"),
        ]
    }

    #[test]
    fn bare_id_no_slash() {
        assert_eq!(parse_model_ref("gpt-5.2", &fixture()), ("gpt-5.2".into(), None));
    }
    #[test]
    fn qualified_two_segments() {
        assert_eq!(parse_model_ref("openai/gpt-5.2", &fixture()), ("gpt-5.2".into(), Some("openai".into())));
        assert_eq!(parse_model_ref("github-copilot/gpt-5.2", &fixture()), ("gpt-5.2".into(), Some("github-copilot".into())));
    }
    #[test]
    fn two_segment_prefers_qualified_when_model_in_profile() {
        // openai HAS gpt-4o → qualified, NOT openrouter's bare "openai/gpt-4o"
        assert_eq!(parse_model_ref("openai/gpt-4o", &fixture()), ("gpt-4o".into(), Some("openai".into())));
    }
    #[test]
    fn fully_qualified_openrouter_slash_id() {
        // first '/' splits off "openrouter"; rest "openai/gpt-4o" IS an openrouter model
        assert_eq!(
            parse_model_ref("openrouter/openai/gpt-4o", &fixture()),
            ("openai/gpt-4o".into(), Some("openrouter".into()))
        );
    }
    #[test]
    fn unknown_prefix_is_bare() {
        assert_eq!(parse_model_ref("foo/bar", &fixture()), ("foo/bar".into(), None));
    }
    #[test]
    fn degenerate_inputs_safe() {
        assert_eq!(parse_model_ref("", &fixture()), ("".into(), None));
        assert_eq!(parse_model_ref("/", &fixture()), ("/".into(), None));
    }
}
```

- [ ] **Step 2: Run, expect FAIL** (`parse_model_ref` undefined): `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p traits parse_model_ref 2>&1 | tail -10`

- [ ] **Step 3: Implement** (place near `ModelListing`, as a free `pub fn`):

```rust
/// Parse a (possibly `profile/model`) text reference against the live model
/// listings into `(request_model, profile)`.
///
/// Qualified only when the first `/`-segment is a known profile (`provider_id`)
/// AND the remainder is a `request_model` under that profile; otherwise the
/// whole string is returned as a bare id (so openrouter ids that contain `/`,
/// e.g. `openai/gpt-4o`, route as bare ids). `profile = None` ⇒ unscoped.
#[must_use]
pub fn parse_model_ref(input: &str, listings: &[ModelListing]) -> (String, Option<String>) {
    if let Some((prefix, rest)) = input.split_once('/') {
        if listings
            .iter()
            .any(|l| l.provider_id == prefix && l.request_model == rest)
        {
            return (rest.to_string(), Some(prefix.to_string()));
        }
    }
    (input.to_string(), None)
}
```

- [ ] **Step 4: Run, expect PASS.** `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p traits parse_model_ref 2>&1 | tail -8`
- [ ] **Step 5: Commit:** `git add traits/src/orchestrator.rs && git commit -m "feat(traits): parse_model_ref for profile/model text references"`

---

## Task 2: ambiguous-id error suggests qualified forms

**Files:** Modify `llm-client/src/registry.rs`, and `llm-client/tests/registry_test.rs` if it pins the message.

- [ ] **Step 1: Check the existing assertion.** Read `llm-client/tests/registry_test.rs:56` `resolve_rejects_ambiguous_model_references` — note whether it asserts the exact message or just that it errors. Read `llm-client/tests/profile_qualified_resolution_test.rs:20` (asserts `contains("ambiguous")`).

- [ ] **Step 2: Write/adjust the failing assertion.** In `profile_qualified_resolution_test.rs` `unqualified_shared_id_still_ambiguous`, strengthen the assertion to require the suggestion:

```rust
        Err(LlmError::InvalidRequest { message }) => {
            assert!(message.contains("ambiguous"), "got {message}");
            assert!(message.contains("openai/gpt-5.2"), "error should suggest the qualified form, got {message}");
        }
```

- [ ] **Step 3: Run, expect FAIL** (message has no suggestion yet): `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client --test profile_qualified_resolution_test unqualified 2>&1 | tail -10`

- [ ] **Step 4: Implement.** In `registry.rs` `resolve_in`, change the `multiple` arm to append suggestions:

```rust
            multiple => {
                let profiles: Vec<&str> =
                    multiple.iter().map(|(p, _)| p.profile_name.as_str()).collect();
                let suggestions = profiles
                    .iter()
                    .map(|p| format!("{p}/{requested}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(LlmError::InvalidRequest {
                    message: format!(
                        "model reference '{requested}' is ambiguous across profiles: {} \
                         — qualify it, e.g. {}",
                        profiles.join(", "),
                        suggestions
                    ),
                })
            }
```

- [ ] **Step 5: Run, expect PASS** + fix `registry_test.rs` if it asserted the old exact string (it should still pass if it only checks `is_err()` / `contains`). `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client 2>&1 | grep -E "test result:|FAILED" | grep -v "0 passed; 0 failed" | tail`
- [ ] **Step 6: Commit:** `git add llm-client/src/registry.rs llm-client/tests/profile_qualified_resolution_test.rs <registry_test.rs if touched> && git commit -m "feat(llm-client): ambiguous-model error suggests profile/model qualified forms"`

---

## Task 3: `/model` accepts `profile/model`

**Files:** Modify `commands/core/src/model.rs`.

- [ ] **Step 1: Write the failing test.** In `model.rs`'s `#[cfg(test)] mod tests` (or add one), use a mock `OrchestratorHandle` that records `switch_model`'s `(model, profile)` and returns openai+copilot listings for `gpt-5.2`. Assert `/model openai/gpt-5.2` calls `switch_model("gpt-5.2", Some("openai"))` and `/model gpt-4.1` (unique) calls `switch_model("gpt-4.1", None)`.

```rust
// sketch — adapt to the crate's existing mock-handle pattern (grep the test
// module for how OrchestratorHandle is mocked elsewhere in command-core).
#[tokio::test]
async fn model_switch_parses_profile_qualified_ref() {
    let handle = Arc::new(RecordingHandle::with_listings(vec![
        listing("openai", "gpt-5.2"), listing("github-copilot", "gpt-5.2"), listing("openai", "gpt-4.1"),
    ]));
    let h = ModelHandler::new(handle.clone());
    h.handle(&args("openai/gpt-5.2")).await;
    assert_eq!(handle.last_switch(), Some(("gpt-5.2".into(), Some("openai".into()))));
    h.handle(&args("gpt-4.1")).await;
    assert_eq!(handle.last_switch(), Some(("gpt-4.1".into(), None)));
}
```
(If `command-core` has no reusable recording `OrchestratorHandle` mock, write a minimal one in the test module implementing the trait, with `list_model_listings` returning the fixture and `switch_model` recording into a `Mutex<Option<(String, Option<String>)>>`. Most other methods can be defaulted/`unimplemented!()` if unused — but `list_available_models`/`get_status_snapshot` are called in the list branch; the switch branch only needs `list_model_listings` + `switch_model`.)

- [ ] **Step 2: Run, expect FAIL** (handler still passes `None`).

- [ ] **Step 3: Implement.** In the switch branch of `ModelHandler::handle` (`model.rs:~52`), replace `match self.handle.switch_model(trimmed, None).await {` with a list→parse→switch:

```rust
        // Switch mode. Resolve an optional `profile/model` qualifier so a shared
        // id (offered by multiple providers) routes deterministically.
        let listings = self.handle.list_model_listings().await;
        let (model, profile) = traits::parse_model_ref(trimmed, &listings);
        match self.handle.switch_model(&model, profile.as_deref()).await {
            Ok(()) => {
                telemetry::emit_command_completed(cmd_evt::MODEL_COMPLETED, "switch");
                CommandResult::Done { display: Some(format!("Switched to model: {model}")) }
            }
            Err(e) => { /* unchanged error arm, but use {model} in any echo if present */ }
        }
```
(Keep the existing error arm; the display now shows the resolved bare `model`. Confirm `traits` is a dependency of `command-core` — it is, via `traits::OrchestratorHandle` already imported.)

- [ ] **Step 4: Run, expect PASS.** `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p command-core model 2>&1 | grep -E "test result:|FAILED" | tail`
- [ ] **Step 5: Commit:** `git add commands/core/src/model.rs && git commit -m "feat(/model): accept profile/model qualified references"`

---

## Task 4: mobile + bridge `SetModel` accept `profile/model`

**Files:** Modify `apps/engine-mobile/src/host.rs`, `apps/bridge-server/src/router.rs`.

- [ ] **Step 1: Update mobile (both sites).** In `apps/engine-mobile/src/host.rs` at the `SetModel` handler (`:954`) and the 2nd `switch_model(&model, None)` site (~`:1124`), replace each with:

```rust
                    let listings = handle.list_model_listings().await;
                    let (model_id, profile) = traits::parse_model_ref(&model, &listings);
                    handle
                        .switch_model(&model_id, profile.as_deref())
                        .await
```
(Match the surrounding `.map_err(...)` / binding. `handle` is the `Arc<dyn OrchestratorHandle>` already in scope; `model` is the incoming `String`. Confirm `traits` is a dep of `engine-mobile`.)

- [ ] **Step 2: Update bridge.** In `apps/bridge-server/src/router.rs:324` `ClientCommand::SetModel { model }`:

```rust
            ClientCommand::SetModel { model } => {
                let listings = self.handle.list_model_listings().await;
                let (model_id, profile) = traits::parse_model_ref(&model, &listings);
                match self.handle.switch_model(&model_id, profile.as_deref()).await {
                    Ok(()) => sink.emit(ClientEvent::ModelChanged { model: model_id }).await,
                    Err(e) => { /* unchanged error arm */ }
                }
            }
```
(The `ModelChanged` event now carries the resolved bare `model_id`. Confirm `traits` is a dep of `bridge-server`; if not, add it to that crate's `Cargo.toml`.)

- [ ] **Step 3: Build + test both apps.** `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-mobile -p bridge-server 2>&1 | tail -8`; run any existing SetModel tests: `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p engine-mobile -p bridge-server 2>&1 | grep -E "test result:|FAILED" | grep -v "0 passed; 0 failed" | tail`. If a SetModel test exists, extend it to assert a qualified ref parses; if none exists, the parser is already unit-tested (Task 1) and the wiring is compile-checked — note that.
- [ ] **Step 4: Commit:** `git add apps/engine-mobile/src/host.rs apps/bridge-server/src/router.rs <Cargo.toml if touched> && git commit -m "feat(mobile,bridge): SetModel accepts profile/model qualified references"`

---

## Task 5: config `default_model` accepts `profile/model`

**Files:** Modify `apps/engine-desktop/src/lib.rs`.

Context: `orch_cfg.model.clone_from(&cfg.default_model)` (`:1694`) seeds the initial model; `SessionState::empty` sets `model_profile: None`. The handle is built at `:2730`. The assembled providers (`assembled.client_config.providers`) and `model_providers` map (`:1590`) are in scope at `:1694`.

- [ ] **Step 1: Parse `default_model` into (model, profile) at `:1694`.** Build a `Vec<ModelListing>` from the assembled providers and parse. Replace `orch_cfg.model.clone_from(&cfg.default_model);` with:

```rust
    // Resolve an optional `profile/model` qualifier in the configured default
    // model so a shared id routes deterministically on the first turn.
    let default_listings: Vec<traits::ModelListing> = assembled
        .client_config
        .providers
        .iter()
        .flat_map(|p| {
            p.models.iter().map(move |m| traits::ModelListing {
                display_model: m.display_model.clone(),
                request_model: m.request_model.clone(),
                provider_id: p.profile_name.clone(),
                provider_label: provider_profile_label(&p.profile_name),
            })
        })
        .collect();
    let (default_model_id, default_model_profile) =
        traits::parse_model_ref(&cfg.default_model, &default_listings);
    orch_cfg.model = default_model_id.clone();
```
(Confirm `assembled` and `provider_profile_label` are in scope at this point — `assembled` is built around `:1574`; if it's defined AFTER `:1694`, move this block to just after `assembled` is available but before `ConversationOrchestrator::new` at `:2711`. Verify ordering and place accordingly. `ModelProfile`'s field names `display_model`/`request_model` match `ModelListing` — confirm against `llm-client` `ModelProfile`.)

- [ ] **Step 2: Seed the profile after the handle exists.** Immediately after `let handle: Arc<dyn OrchestratorHandle> = orch.clone();` (`:2730`), seed the profile when present:

```rust
    // Seed the initial model_profile from a profile-qualified default_model
    // (SessionState::empty starts it at None).
    if let Some(profile) = default_model_profile.as_deref() {
        if let Err(e) = handle.switch_model(&default_model_id, Some(profile)).await {
            tracing::warn!(error = %e, "failed to seed default model profile");
        }
    }
```

- [ ] **Step 3: Build the engine.** `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-desktop 2>&1 | tail -10`. Fix scope/ordering issues (move the Step-1 block to where `assembled` is live if needed).

- [ ] **Step 4: Add a focused test.** If `engine-desktop` has a build/runtime test seam that exposes the initial session model/profile (grep the test module for `default_model` / `model_providers` tests around `:3437`), add one asserting that a `DesktopConfig` with `default_model = "openai/gpt-5.2"` (and openai+copilot both serving gpt-5.2) yields initial `model == "gpt-5.2"` and `model_profile == Some("openai")`. If no such seam exists, rely on the compile + the parser's Task-1 unit tests + a manual note; do NOT fabricate a test harness.
- [ ] **Step 5: Commit:** `git add apps/engine-desktop/src/lib.rs && git commit -m "feat(engine): config default_model accepts profile/model qualified reference"`

---

## Task 6: full verification

- [ ] **Step 1: Workspace build.** `CARGO_PROFILE_DEV_DEBUG=0 cargo build --workspace 2>&1 | tail -3`. Expected `Finished`.
- [ ] **Step 2: Affected-crate tests.** `CARGO_PROFILE_DEV_DEBUG=0 cargo test -p traits -p llm-client -p command-core -p engine-mobile -p bridge-server -p engine-desktop 2>&1 | grep -E "test result:|FAILED|error\[|error:" | grep -v "0 passed; 0 failed"`. Expected: only `test result: ok` (the known-pre-existing `pty_smoke::print_mode_unaffected_by_tui_routing` in `tui` is NOT in this set; if it appears, it's unrelated/environmental).
- [ ] **Step 3: Clippy touched crates.** `CARGO_PROFILE_DEV_DEBUG=0 cargo clippy -p traits -p llm-client -p command-core 2>&1 | grep -E "^warning:|^error:" | grep -v "unused manifest key"`. Fix any NEW lints in the files you changed (ignore pre-existing lints in untouched code).

No further commit. Feature complete.

---

## Notes for the implementer
- The parser is the only non-trivial logic and is fully unit-tested in Task 1 — every entry point just does `list_model_listings() → parse_model_ref → switch_model(model, profile.as_deref())`.
- Display strings / events now carry the RESOLVED bare `model` (not the qualified input) — intentional.
- `traits` must be a dependency of every entry-point crate (`command-core`, `engine-mobile`, `bridge-server`, `engine-desktop`) — it already is for the handle; add to `Cargo.toml` only if a build error says otherwise.
- The config path (Task 5) is order-sensitive: the listings block must run where `assembled` is live and before `ConversationOrchestrator::new`. Verify by compiling.
- Every cargo command keeps `CARGO_PROFILE_DEV_DEBUG=0`.
