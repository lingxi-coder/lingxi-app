# OpenAI API-key Provider (P1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an out-of-the-box OpenAI API-key provider preset to `llm-client` so users can `/connect openai` and pick OpenAI models, talking the Responses API to `api.openai.com`.

**Architecture:** A single built-in preset added to `llm-client::builtin_presets()`. Everything downstream (`/connect`, `/model` picker, availability, credentials, routing) flows automatically through `provider-config::assemble()` — identical mechanism to the `zai` preset added previously. The preset uses `ProtocolFamily::OpenAiResponses` (codex is Responses-only) + `AuthStrategy::Bearer` + `ProviderId::OpenAI`.

**Tech Stack:** Rust workspace (`lingxi-code/`). Vendored models.dev JSON slices. `cargo test` (run with `CARGO_PROFILE_DEV_DEBUG=0` — this volume runs near-full on disk).

**Reference spec:** `docs/superpowers/specs/2026-06-16-openai-auth-codex-parity-design.md` (§ "P1").

**Working directory for all commands:** `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code`

---

## File Structure

- Create: `lingxi-code/llm-client/data/models-dev/openai.json` — vendored models.dev `openai` slice (50 models).
- Modify: `lingxi-code/llm-client/src/catalog/presets.rs` — add the `OPENAI` const, the `Preset` entry, and update the count-guard test.
- Modify: `lingxi-code/provider-config/src/assemble.rs` — update the provider-count guard + name-presence assertion in `merges_anthropic_and_presets`.
- Modify: `lingxi-code/provider-config/src/lib.rs` — update the `links_llm_client` smoke count.
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs` — add `"openai" => "OpenAI"` to `provider_profile_label()`.
- Modify: `lingxi-code/orchestrator/src/provider_adapter.rs` — add `"openai" => "OpenAI"` to `provider_label()`.

---

## Task 1: Vendor the models.dev `openai` slice

**Files:**
- Create: `lingxi-code/llm-client/data/models-dev/openai.json`

- [ ] **Step 1: Fetch models.dev and write the openai slice**

Run (from `lingxi-code`):

```bash
curl -s "https://models.dev/api.json" -o /tmp/modelsdev.json && \
python3 -c "
import json
d=json.load(open('/tmp/modelsdev.json'))
p=d['openai']
open('llm-client/data/models-dev/openai.json','w').write(json.dumps(p, indent=2, sort_keys=True)+'\n')
print('models:', len(p['models']))
"
```

Expected output: `models: 50`

(If the count is NOT 50, stop — models.dev drifted; update the count in Task 2 Step 1 and Step 4 to match the printed number, and note the drift in the commit message.)

- [ ] **Step 2: Sanity-check the slice parses as a `ProviderSlice`**

The shape is identical to the other vendored slices (top-level `id`, `name`, `models`); `ProviderSlice` ignores unknown fields and treats `api` as optional. Verify the file is valid JSON and has the expected top-level keys:

Run:

```bash
python3 -c "import json; d=json.load(open('llm-client/data/models-dev/openai.json')); print(sorted(d.keys())); print('id=',d['id'],'name=',d['name'])"
```

Expected: keys include `['doc', 'env', 'id', 'models', 'name', 'npm']`, `id= openai name= OpenAI`

- [ ] **Step 3: Commit**

```bash
git add llm-client/data/models-dev/openai.json
git commit -m "feat(llm-client): vendor models.dev openai slice (50 models)"
```

---

## Task 2: Add the `openai` preset (TDD via the count guard)

**Files:**
- Modify: `lingxi-code/llm-client/src/catalog/presets.rs`

- [ ] **Step 1: Update the count-guard test to expect the new preset (the failing test)**

In `llm-client/src/catalog/presets.rs`, in `mod tests`, change the provider count from 5 to 6 and add the `openai` model-count assertion.

Change:

```rust
        let catalog = builtin_presets();
        assert_eq!(catalog.providers.len(), 5);
```

to:

```rust
        let catalog = builtin_presets();
        assert_eq!(catalog.providers.len(), 6);
```

And change:

```rust
        assert_eq!(count("zai"), 13);
        assert_eq!(count("github-copilot"), 23);
```

to:

```rust
        assert_eq!(count("zai"), 13);
        assert_eq!(count("openai"), 50);
        assert_eq!(count("github-copilot"), 23);
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client --lib catalog::presets::tests::every_preset_yields_expected_model_counts 2>&1 | tail -15
```

Expected: FAIL — `assertion left == right failed, left: 5, right: 6`.

- [ ] **Step 3: Add the `OPENAI` slice const**

In `presets.rs`, after the `ZAI` const line:

```rust
const ZAI: &str = include_str!("../../data/models-dev/zai.json");
```

add:

```rust
const OPENAI: &str = include_str!("../../data/models-dev/openai.json");
```

- [ ] **Step 4: Add the `Preset` entry**

In `presets.rs`, inside the `vec![ ... ]` in `fn presets()`, insert this entry immediately before the `// GitHub Copilot:` comment + its `Preset { ... }`:

```rust
        // OpenAI first-party: Responses API (codex removed the chat wire, so all
        // OpenAI traffic is Responses-only). API-key auth as a Bearer token.
        // ChatGPT account/OAuth login is a separate phase (see the design doc).
        Preset {
            profile_name: "openai",
            base_url: "https://api.openai.com/v1",
            protocol: ProtocolFamily::OpenAiResponses,
            auth: AuthStrategy::Bearer,
            provider_id: ProviderId::OpenAI,
            credential_env: "OPENAI_API_KEY",
            slice_json: OPENAI,
        },
```

- [ ] **Step 5: Add a decision-locking assertion (protocol + provider_id)**

This guards design decisions 1 (Responses-not-Chat) and 2 (`ProviderId::OpenAI`). In `mod tests`, at the end of `every_preset_yields_expected_model_counts` (after the `github-copilot` assertion, before the closing `}`), add:

```rust
        let openai = catalog
            .providers
            .iter()
            .find(|p| p.profile_name == "openai")
            .expect("openai preset present");
        assert_eq!(openai.protocol, ProtocolFamily::OpenAiResponses);
        assert_eq!(openai.provider_id, ProviderId::OpenAI);
        assert_eq!(openai.base_url, "https://api.openai.com/v1");
```

- [ ] **Step 6: Run the presets tests to verify they pass**

Run:

```bash
CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client --lib catalog::presets 2>&1 | tail -15
```

Expected: PASS (`test result: ok`).

- [ ] **Step 7: Commit**

```bash
git add llm-client/src/catalog/presets.rs
git commit -m "feat(llm-client): add OpenAI first-party preset (Responses + Bearer)"
```

---

## Task 3: Update provider-config count guards

**Files:**
- Modify: `lingxi-code/provider-config/src/assemble.rs`
- Modify: `lingxi-code/provider-config/src/lib.rs`

- [ ] **Step 1: Update the assemble provider-count + name assertion**

In `provider-config/src/assemble.rs`, in `fn merges_anthropic_and_presets`, change:

```rust
        let out = assemble(anthropic_only_inputs());
        assert_eq!(out.client_config.providers.len(), 6);
```

to:

```rust
        let out = assemble(anthropic_only_inputs());
        assert_eq!(out.client_config.providers.len(), 7);
```

And change:

```rust
        assert!(names.contains(&"zai"));
        assert!(names.contains(&"github-copilot"));
```

to:

```rust
        assert!(names.contains(&"zai"));
        assert!(names.contains(&"openai"));
        assert!(names.contains(&"github-copilot"));
```

- [ ] **Step 2: Update the lib smoke count**

In `provider-config/src/lib.rs`, in `mod smoke_tests::links_llm_client`, change:

```rust
        let cat = llm_client::builtin_presets();
        assert_eq!(cat.providers.len(), 5);
```

to:

```rust
        let cat = llm_client::builtin_presets();
        assert_eq!(cat.providers.len(), 6);
```

- [ ] **Step 3: Run provider-config tests to verify they pass**

Run:

```bash
CARGO_PROFILE_DEV_DEBUG=0 cargo test -p provider-config 2>&1 | grep -E "test result:|FAILED" | head
```

Expected: all `test result: ok`, no `FAILED`.

- [ ] **Step 4: Commit**

```bash
git add provider-config/src/assemble.rs provider-config/src/lib.rs
git commit -m "test(provider-config): update preset count guards for openai"
```

---

## Task 4: Add the picker labels

**Files:**
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs`
- Modify: `lingxi-code/orchestrator/src/provider_adapter.rs`

- [ ] **Step 1: Add the desktop label**

In `apps/engine-desktop/src/lib.rs`, in `fn provider_profile_label`, change:

```rust
        "zai" => "Z.AI".to_string(),
        "github-copilot" => "GitHub Copilot".to_string(),
```

to:

```rust
        "zai" => "Z.AI".to_string(),
        "openai" => "OpenAI".to_string(),
        "github-copilot" => "GitHub Copilot".to_string(),
```

- [ ] **Step 2: Add the orchestrator label**

In `orchestrator/src/provider_adapter.rs`, in `fn provider_label`, change:

```rust
        "zai" => "Z.AI",
        "github-copilot" => "GitHub Copilot",
        other => other,
```

to:

```rust
        "zai" => "Z.AI",
        "openai" => "OpenAI",
        "github-copilot" => "GitHub Copilot",
        other => other,
```

- [ ] **Step 3: Build + test the two crates**

Run:

```bash
CARGO_PROFILE_DEV_DEBUG=0 cargo test -p orchestrator -p engine-desktop 2>&1 | grep -E "test result:|FAILED|error\[|error:" | tail -10
```

Expected: `test result: ok` lines, no `FAILED`/`error`.

- [ ] **Step 4: Commit**

```bash
git add apps/engine-desktop/src/lib.rs orchestrator/src/provider_adapter.rs
git commit -m "feat(picker): label the openai provider as \"OpenAI\""
```

---

## Task 5: Full affected-crate verification

**Files:** none (verification only)

- [ ] **Step 1: Run all affected crates together**

Run:

```bash
CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client -p provider-config -p command-core -p orchestrator -p engine-desktop 2>&1 | grep -E "test result:|FAILED|error\[|error:" | grep -v "0 passed; 0 failed" | sort | uniq -c | tail -30
```

Expected: only `test result: ok` lines; zero `FAILED`, zero `error`.

- [ ] **Step 2: Confirm the preset is end-to-end visible (manual grep check)**

Run:

```bash
CARGO_PROFILE_DEV_DEBUG=0 cargo test -p llm-client --lib catalog::presets::tests::every_preset_yields_expected_model_counts -- --nocapture 2>&1 | tail -5
```

Expected: PASS — confirms `builtin_presets()` yields 6 providers including `openai` with the Responses/`ProviderId::OpenAI`/`api.openai.com/v1` shape.

No commit (verification only). P1 complete.

---

## Notes for the implementer

- **Do NOT** modify `provider-config/src/parse_providers.rs` — the user-defined `type: "openai"` path (Chat Completions) is intentionally left as-is; the built-in preset (Responses) is a separate surface. This divergence is by design (see spec § P1 decision 1).
- **Do NOT** add OAuth/ChatGPT-login, device-code, refresh, or the `ChatGPT-Account-ID` header — those are P2/P3.
- If `cargo` reports the disk is full, the cause is debug symbols; the `CARGO_PROFILE_DEV_DEBUG=0` prefix on every command prevents this — do not drop it.
- `command-core`'s crate name is `command-core` (singular), not `commands-core`.
