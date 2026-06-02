# LLM Providers v2 — Finish Deferred Features & Tech Debt — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the six bounded-deferred items from LLM Providers v2 (P8) plus pre-existing tech debt: Bedrock true streaming, AWS/Azure/GCP credential discovery, Bedrock/cloud pricing rows, `/model` real listing, TUI paste→image, and the `tool-api`/`tool-*` debt.

**Architecture:** Land on one branch (`llm-v2-finish-deferrals`), staged low-risk→high-risk, gates after each stage. Two stages touch the **frozen `traits/` crate** (authorized): both additions are **additive trait methods with default impls**, so existing impls are untouched and the parity suite stays green. AWS dep strategy was decided by a feasibility spike (below).

**Tech Stack:** Rust 1.82.0 (pinned, edition 2021). `aws-sigv4`/`aws-credential-types` (present). NEW: `aws-smithy-eventstream = "=0.60.13"` with `aws-smithy-types` pinned to `=1.3.6` (streaming). NO `aws-config` (see spike).

---

## Spike outcome (already run — informs Stages 4 & 5)

- ✅ `aws-smithy-eventstream 0.60.13` **builds on Rust 1.82** when `aws-smithy-types` is pinned to `=1.3.6` (it's a pure frame codec; no crypto pulled). → **Stage 5 uses it.**
- ❌ `aws-config` (any 1.8.x) **cannot build on 1.82**: requires `aws-smithy-types ^1.4.8` (rustc 1.91) *and* pulls `const-oid 0.10.2` (unstable `edition2024` Cargo feature). Pins do not converge. → **Stage 4 hand-rolls a lightweight AWS credential chain instead of `aws-config`.**
- Pinning `aws-smithy-types =1.3.6` satisfies the existing `aws-sigv4`/`aws-credential-types` (`^1.x`) AND `aws-smithy-eventstream 0.60.13` (`^1.3.4`); confirmed `cargo build -p providers` is green on 1.82 with it.

**Already verified done (was on the user's list, but is stale):** the `traits/` `doc_markdown` clippy lint was fixed in commit `aa1f5bb` ("backtick UniFFI/16_000"); `cargo clippy -p traits --all-targets -- -D warnings` is clean. **No task for it.** The `--no-deps` clippy workaround is no longer needed.

---

## File-structure map

| Stage | Crate(s) / files touched | Frozen `traits/`? |
|---|---|---|
| 1 Tech debt | `tool-api/Cargo.toml`; `tools/{cron,ui,skill,plan,meta,web,mcp,worktree,agent,lsp,team}/src/lib.rs` | no |
| 2 Pricing | `cost/src/pricing.rs`; `orchestrator/src/cost_wiring.rs`; `providers/src/bedrock.rs` | no |
| 3 `/model` | `providers/src/profile.rs`, `providers/src/registry.rs`, `providers/src/settings.rs`; `orchestrator/src/conversation.rs`, `orchestrator/src/provider_adapter.rs`, `orchestrator/src/handle_impl.rs` | no |
| 4 Cred discovery | `providers/src/authenticator.rs`, `providers/src/profile.rs`, `providers/src/registry.rs`, `providers/Cargo.toml` | no |
| 5 Bedrock streaming | `traits/src/http.rs` (**+ method, default impl**); `platforms/{posix,windows,posix-minimal}/src/http.rs`; `providers/src/anthropic_wire.rs` (new), `providers/src/bedrock.rs`, `providers/src/testutil.rs`, `providers/Cargo.toml` | **YES (additive)** |
| 6 Paste→image | `traits/src/orchestrator.rs` (**+ method, default impl**); `protocol/src/messages.rs`; `orchestrator/src/conversation.rs`, `orchestrator/src/handle_impl.rs`, `orchestrator/src/test_support.rs`; `tui/src/app.rs` | **YES (additive)** |
| 7 Docs+gates | `CHANGELOG.md`, `README.md`, `docs/LLM_PROVIDERS.md` | no |

Run all `cargo` from `lingxi-code/`. Per-stage gate: `cargo build -p <crate>` + `cargo test -p <crate>` + `cargo clippy -p <crate> --no-deps --all-targets -- -D warnings`. Stages that change the dep graph also run `bash scripts/check-deps.sh`.

---

## Stage 1 — Tech debt (trivial, no risk)

### Task 1.1: `tool-api` test build — add `futures` to dev-dependencies

`tool-api/src/test_support.rs:40` uses `futures::Stream` but `futures` is only an *optional* dep (behind the `test-support` feature), so `cargo test -p tool-api` fails to compile.

**Files:** Modify `lingxi-code/tool-api/Cargo.toml`.

- [ ] **Step 1 — confirm the failure.** Run `cargo test -p tool-api --lib 2>&1 | head`. Expected: `error[E0433]: failed to resolve: use of undeclared crate or module 'futures'` at `test_support.rs:40`.
- [ ] **Step 2 — add the dev-dep.** In `tool-api/Cargo.toml`, under `[dev-dependencies]`, add `futures = { workspace = true }` (the crate already lists `futures = { version = "0.3", optional = true }` under `[dependencies]`; use the workspace version for consistency — verify `futures` is in the workspace `[workspace.dependencies]`; if not, use `futures = "0.3"`).
- [ ] **Step 3 — verify.** `cargo test -p tool-api` → compiles and passes.
- [ ] **Step 4 — commit.**
```bash
git add lingxi-code/tool-api/Cargo.toml lingxi-code/Cargo.lock
git commit -m "fix(tool-api): add futures to dev-dependencies so own tests compile

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task 1.2: `tool-*` missing-docs — document the 11 `register_all` fns

`missing_docs = "warn"` (workspace `Cargo.toml`) flags 11 undocumented `pub fn register_all(...)`.

**Files:** Modify `src/lib.rs` in each of: `tools/cron`, `tools/ui`, `tools/skill`, `tools/plan`, `tools/meta`, `tools/web`, `tools/mcp`, `tools/worktree`, `tools/agent`, `tools/lsp`, `tools/team`.

- [ ] **Step 1 — find the exact warnings.** Run `cargo build -p cron-tools -p ui-tools 2>&1 | rg "missing documentation"` (adjust package names — discover them via `rg -l 'pub fn register_all' lingxi-code/tools/*/src/lib.rs`). Confirm each is a one-line `pub fn register_all(...)`.
- [ ] **Step 2 — add a doc line above each `register_all`.** Use a doc comment that matches each crate's category, e.g.:
```rust
/// Register this crate's built-in tools into `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
```
Keep the exact existing signature; only add the `///` line. Do this for all 11 files.
- [ ] **Step 3 — verify no missing-docs warnings remain.** `cargo build --workspace 2>&1 | rg "missing documentation"` → no output.
- [ ] **Step 4 — commit.**
```bash
git add lingxi-code/tools/*/src/lib.rs
git commit -m "docs(tools): document register_all() in 11 tool category crates (missing_docs)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**Stage 1 gate:** `cargo build --workspace` Finished; `cargo test -p tool-api` passes; `cargo build --workspace 2>&1 | rg "missing documentation"` empty.

---

## Stage 2 — Bedrock & cloud pricing (low)

Today a `bedrock/anthropic.claude-…` turn resolves to `ProviderId::OpenAICompatible{name:"bedrock"}` → **unpriced (0 cost)**. `vertex/…` and `azure/…` are likewise mis-mapped. Spec §5.

### Task 2.1: add `ProviderId::AmazonBedrock` + Bedrock price table

**Files:** Modify `cost/src/pricing.rs`.

- [ ] **Step 1 — add the enum variant.** In `cost/src/pricing.rs` `pub enum ProviderId` (line ~31), add after `GoogleGemini`:
```rust
    /// Amazon Bedrock (Claude-on-Bedrock) — distinct list prices from the
    /// first-party Anthropic API; model ids are Bedrock-namespaced.
    AmazonBedrock,
```
- [ ] **Step 2 — write a failing test** in the `tests` mod of `pricing.rs`:
```rust
#[test]
fn bedrock_claude_sonnet_is_priced() {
    let cat = PricingCatalog::builtin_reference();
    let mr = ModelRef { provider: ProviderId::AmazonBedrock,
        model: "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string() };
    let pricing = cat.resolve(&mr); // mirror the existing resolve test's call shape
    assert!(matches!(pricing, PricingResolution::ExactModel { .. }));
}
```
(Read the existing `resolve`/lookup test in this file first and match its exact API — `PricingCatalog` exposes the lookup the tracker uses; reuse that call, not an invented one.)
- [ ] **Step 3 — seed Bedrock rows** in `builtin_reference()` (after the Gemini block, before `c`). Use `insert_priced(ProviderId::AmazonBedrock, model, input, output, cache_write, cache_read)` (milli-USD per Mtok == nano-USD per token, per module docs). AWS Bedrock list prices for Claude (us-east-1, per-Mtok USD → milli-USD): Sonnet 3.5 v2 `$3/$15`; Haiku 3.5 `$0.80/$4`; Opus 3 `$15/$75`; Sonnet 3.7 `$3/$15`. Bedrock does not bill cache the same way — set cache_write/cache_read to the same ratios used for Anthropic rows (3_750/300 for $3 tier, etc.) for now:
```rust
        // Amazon Bedrock (Claude) — AWS published list prices (us-east-1).
        c.insert_priced(ProviderId::AmazonBedrock, "anthropic.claude-3-5-sonnet-20241022-v2:0", 3_000, 15_000, 3_750, 300);
        c.insert_priced(ProviderId::AmazonBedrock, "anthropic.claude-3-7-sonnet-20250219-v1:0", 3_000, 15_000, 3_750, 300);
        c.insert_priced(ProviderId::AmazonBedrock, "anthropic.claude-3-5-haiku-20241022-v1:0", 800, 4_000, 1_000, 80);
        c.insert_priced(ProviderId::AmazonBedrock, "anthropic.claude-3-opus-20240229-v1:0", 15_000, 75_000, 18_750, 1_500);
```
(Confirm the exact `insert_priced` arity/signature in this file — it was used for OpenAI/Gemini rows. If a provider-default is desired so unknown Bedrock ids don't read as 0, also add a `provider_defaults` entry mirroring the Sonnet tier — match how OpenAI/Gemini set or omit defaults.)
- [ ] **Step 4 — run test → PASS.** `cargo test -p cost bedrock_claude_sonnet_is_priced`.
- [ ] **Step 5 — commit.** `git add lingxi-code/cost/src/pricing.rs && git commit -m "feat(cost): ProviderId::AmazonBedrock + Bedrock-Claude reference price rows"` (+ co-author trailer).

### Task 2.2: wire `cost_wiring` + `BedrockProvider::id()` to the new variant

**Files:** Modify `orchestrator/src/cost_wiring.rs`, `providers/src/bedrock.rs`.

- [ ] **Step 1 — failing test** in `cost_wiring.rs` `tests`:
```rust
#[test]
fn bedrock_profile_maps_to_amazon_bedrock() {
    assert_eq!(provider_from_model("bedrock/anthropic.claude-3-5-sonnet-20241022-v2:0"),
        ProviderId::AmazonBedrock);
    // vertex reuses Gemini prices; azure reuses OpenAI prices.
    assert_eq!(provider_from_model("vertex/gemini-2.0-flash"), ProviderId::GoogleGemini);
    assert_eq!(provider_from_model("azure/gpt-4o"), ProviderId::OpenAI);
}
```
- [ ] **Step 2 — extend `provider_id_for_profile`** (cost_wiring.rs:39) — add arms before the `other =>`:
```rust
        "bedrock" => ProviderId::AmazonBedrock,
        "vertex" => ProviderId::GoogleGemini,
        "azure" => ProviderId::OpenAI,
```
(Update the function's doc comment to note Vertex/Azure reuse first-party price tables; Bedrock has its own.)
- [ ] **Step 3 — `BedrockProvider::id()`** (bedrock.rs:93): change `ProviderId::Anthropic` → `ProviderId::AmazonBedrock`. Update the import/use if needed (`cost::ProviderId`).
- [ ] **Step 4 — run tests → PASS.** `cargo test -p orchestrator cost_wiring && cargo test -p providers -p cost`.
- [ ] **Step 5 — commit.** `git add lingxi-code/orchestrator/src/cost_wiring.rs lingxi-code/providers/src/bedrock.rs && git commit -m "feat(cost): map bedrock/vertex/azure profiles to correct ProviderId for pricing"` (+ trailer).

**Stage 2 gate:** `cargo test -p cost -p orchestrator -p providers` pass; `cargo clippy -p cost -p orchestrator -p providers --no-deps --all-targets -- -D warnings` clean.

---

## Stage 3 — `/model` real listing (low-med)

`handle_impl::list_available_models` (handle_impl.rs:192) returns a **hardcoded** list. Wire it to the real router via the `OrchestratorApiClient` seam (NOT frozen — defined in `orchestrator/src/conversation.rs:26`). `ProviderApiAdapter` (provider_adapter.rs) holds `router: Arc<dyn ModelRouter>`, and `ModelRouter::available_models()` (registry.rs:268) already exists but returns only profile+alias names — enrich it to emit `provider/model` ids + `@alias` (spec §3.6), which needs a `models` field on `ProviderProfile`.

### Task 3.1: add `models: Vec<String>` to `ProviderProfile` + settings parse

**Files:** Modify `providers/src/profile.rs`, `providers/src/settings.rs` (the serde-facing settings struct that maps to `ProviderProfile` — confirm its path with `rg -l 'azure_deployment|azureDeployment' providers/src`).

- [ ] **Step 1 — add the field** to `pub struct ProviderProfile` (profile.rs:46), after `region`:
```rust
    /// Provider-local model ids this profile declares (for `/model` listing).
    /// Empty by default; populated from the `models` settings array.
    pub models: Vec<String>,
```
- [ ] **Step 2 — fix all struct literals.** `builtin_profiles` (profile.rs:77+) and every `ProviderProfile { … }` literal across the crate/tests must add `models: Vec::new()` (or `vec![]`). Find them: `rg -n 'ProviderProfile \{' providers/src`. Built-ins get `models: vec![]` unless a sensible default exists.
- [ ] **Step 3 — settings deserialization.** In the settings struct that deserializes the `providers` config block, add an optional `models: Option<Vec<String>>` (camelCase `models`, `#[serde(default)]`) and map it into `ProviderProfile.models` (`.unwrap_or_default()`). Add a parse test: `{"kind":"openAi","models":["llama-3.3-70b"]}` → `profile.models == ["llama-3.3-70b"]`.
- [ ] **Step 4 — verify.** `cargo test -p providers profile && cargo build -p providers`.
- [ ] **Step 5 — commit.** `git add lingxi-code/providers/src && git commit -m "feat(providers): ProviderProfile.models field + settings parse (for /model listing)"` (+ trailer).

### Task 3.2: enrich `ModelRouter::available_models()` to spec §3.6 shape

**Files:** Modify `providers/src/registry.rs`.

- [ ] **Step 1 — failing test** in registry.rs `tests` (reuse the existing `registry(...)` helper):
```rust
#[test]
fn available_models_lists_provider_model_ids_and_aliases() {
    let mut extra = BTreeMap::new();
    extra.insert("groq".to_string(), ProviderProfile {
        kind: ProviderKind::OpenAi, base_url: Some("https://x".into()), api_key_env: None,
        reasoning_effort: None, thinking_budget: None, azure_deployment: None,
        azure_api_version: None, project: None, region: None,
        models: vec!["llama-3.3-70b".into()],
    });
    let r = ProviderRegistry::new(/* profiles incl extra */, env, transport,
        RoutingConfig { aliases: [("fast".to_string(), "groq/llama-3.3-70b".to_string())].into(), ..Default::default() });
    let models = r.available_models();
    assert!(models.contains(&"groq/llama-3.3-70b".to_string()));
    assert!(models.iter().any(|m| m == "@fast"));
}
```
(Match the real `ProviderProfile`/`RoutingConfig` field names and the `registry(...)` test helper that already exists at registry.rs:286.)
- [ ] **Step 2 — rewrite `available_models`** (registry.rs:268):
```rust
    fn available_models(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (name, profile) in &self.profiles {
            if profile.models.is_empty() {
                out.push(name.clone()); // bare profile name when no models declared
            } else {
                for m in &profile.models {
                    out.push(format!("{name}/{m}"));
                }
            }
        }
        out.extend(self.routing.aliases.keys().map(|a| format!("@{a}")));
        out.sort();
        out.dedup();
        out
    }
```
- [ ] **Step 3 — run test → PASS.** `cargo test -p providers available_models`.
- [ ] **Step 4 — commit.** `git add lingxi-code/providers/src/registry.rs && git commit -m "feat(providers): available_models() emits provider/model ids + @aliases (spec §3.6)"` (+ trailer).

### Task 3.3: surface `available_models()` through the seam to `/model`

**Files:** Modify `orchestrator/src/conversation.rs` (trait), `orchestrator/src/provider_adapter.rs` (impl), `orchestrator/src/handle_impl.rs` (consumer), `orchestrator/src/test_support.rs` (MockApiClient inherits default — no change unless needed).

- [ ] **Step 1 — extend the trait** `OrchestratorApiClient` (conversation.rs:26) with an **additive default method**:
```rust
    /// Enumerate available `provider/model` ids + `@aliases` for `/model`'s
    /// list mode. Default returns empty so non-routing impls (mocks/stubs)
    /// need no override.
    fn available_models(&self) -> Vec<String> { Vec::new() }
```
- [ ] **Step 2 — override in `ProviderApiAdapter`** (provider_adapter.rs, inside `impl OrchestratorApiClient for ProviderApiAdapter`):
```rust
    fn available_models(&self) -> Vec<String> { self.router.available_models() }
```
- [ ] **Step 3 — wire `handle_impl::list_available_models`** (handle_impl.rs:192): call the seam, fall back to the existing hardcoded list only when empty (preserves library/test behavior):
```rust
    async fn list_available_models(&self) -> Vec<String> {
        let models = self.api.available_models();
        if !models.is_empty() {
            return models;
        }
        vec![ /* keep the existing hardcoded fallback list verbatim */ ]
    }
```
(Confirm `handle_impl` can reach `self.api` — the `OrchestratorHandleImpl` wraps `ConversationOrchestrator`, which holds `api: Arc<dyn OrchestratorApiClient>`. Use the existing accessor pattern the other handle methods use to reach orchestrator internals.)
- [ ] **Step 4 — test (integration):** add/extend a test where a `ProviderApiAdapter` over a registry with a declared model surfaces it through a real `OrchestratorHandleImpl::list_available_models()`. If wiring a full adapter in a unit test is heavy, assert at the adapter level: `ProviderApiAdapter::new(router).available_models()` returns the registry's list. Keep the existing `commands/core` `/model` list-mode test green.
- [ ] **Step 5 — verify + commit.** `cargo test -p orchestrator -p commands-core` (adjust pkg name). `git add lingxi-code/orchestrator/src && git commit -m "feat(orchestrator): /model lists real configured models via OrchestratorApiClient seam"` (+ trailer).

**Stage 3 gate:** `cargo test -p providers -p orchestrator` + `commands` tests pass; clippy clean on touched crates.

---

## Stage 4 — Signed-auth credential discovery (med; lightweight, no `aws-config`)

Spec §2.1/§4. Today: SigV4 reads only 3 env vars; GCP works but is untested; Azure AD doesn't exist. Per the spike, **do NOT add `aws-config`** — hand-roll a layered AWS chain and add `AzureAdAuthenticator`.

### Task 4.1: AWS credential provider chain (env → shared file → process → IMDSv2)

**Files:** Modify `providers/src/authenticator.rs`; new helper module `providers/src/aws_creds.rs`; `providers/src/registry.rs` (pass transport to `SigV4Authenticator`).

- [ ] **Step 1 — `aws_creds.rs`: a pure-ish resolver.** Define:
```rust
/// Resolved static AWS credentials.
pub struct AwsCreds { pub access_key: String, pub secret_key: String, pub session_token: Option<String> }
```
and an async `resolve(region, transport: &Arc<dyn HttpTransport>) -> Result<AwsCreds, ApiError>` that tries, in order, returning the first hit:
  1. **env**: `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/`AWS_SESSION_TOKEN` (current behavior).
  2. **shared file**: parse `${AWS_SHARED_CREDENTIALS_FILE:-~/.aws/credentials}` for the `[${AWS_PROFILE:-default}]` section's `aws_access_key_id`/`aws_secret_access_key`/`aws_session_token` (tiny INI scan — no new dep; also honor `aws_session_token`).
  3. **process**: if `~/.aws/config` profile has `credential_process`, run it, parse the JSON (`AccessKeyId`/`SecretAccessKey`/`SessionToken`).
  4. **IMDSv2**: PUT `http://169.254.169.254/latest/api/token` (`X-aws-ec2-metadata-token-ttl-seconds: 21600`) over `transport.request`, then GET `…/latest/meta-data/iam/security-credentials/` + the role, parse JSON creds. Short timeouts; treat any failure as "not on EC2" and continue.
  - On total miss: `Err(ApiError::Unauthorized("no AWS credentials found (env/profile/process/IMDS); set AWS_ACCESS_KEY_ID or run `aws sso login`)"))`.
  - Make the INI/JSON parsers pure functions so they're unit-testable without env/network.
- [ ] **Step 2 — failing unit tests** for the pure parsers: an INI fixture string → `AwsCreds`; a `credential_process` JSON fixture → `AwsCreds`; an IMDS creds JSON fixture → `AwsCreds`.
- [ ] **Step 3 — implement** the parsers + chain. Add `SigV4Authenticator::with_transport(region, Arc<dyn HttpTransport>)`; keep `new(region)` (env-only, used where no transport). `authorize()` calls `aws_creds::resolve(...)` then the unchanged `sign_in_place(...)`.
- [ ] **Step 4 — registry** (registry.rs Bedrock branch): build `SigV4Authenticator::with_transport(region, self.transport.clone())`.
- [ ] **Step 5 — keep the deterministic SigV4 signing test green** (authenticator.rs:248) — `sign_in_place` is unchanged. Run `cargo test -p providers`.
- [ ] **Step 6 — commit.** `git commit -m "feat(providers): layered AWS credential discovery (env/profile/process/IMDSv2) for SigV4"` (+ trailer).

### Task 4.2: `AzureAdAuthenticator` (client-credentials token) + config + registry branch

**Files:** Modify `providers/src/authenticator.rs`, `providers/src/profile.rs`, `providers/src/registry.rs` (+ settings parse from Task 3.1's settings file).

- [ ] **Step 1 — profile fields** (profile.rs `ProviderProfile`): add `azure_ad_tenant: Option<String>`, `azure_ad_client_id_env: Option<String>`, `azure_ad_client_secret_env: Option<String>` (+ update all struct literals from Task 3.1, + settings camelCase `azureAdTenant`/`azureAdClientIdEnv`/`azureAdClientSecretEnv`).
- [ ] **Step 2 — `AzureAdAuthenticator`** in authenticator.rs: holds `tenant`, `client_id`, `client_secret`, `scope` (default `https://cognitiveservices.azure.com/.default`), and `transport: Arc<dyn HttpTransport>`. `authorize()` POSTs `https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token` with `grant_type=client_credentials&client_id=…&client_secret=…&scope=…` (form-encoded), parses `access_token`, pushes `Authorization: Bearer …`. Cache the token with its `expires_in` in a `tokio::sync::Mutex<Option<(String, Instant)>>` (mirror `GcpTokenAuthenticator`'s lazy/cached shape). Map failures to `ApiError::Unauthorized`.
- [ ] **Step 3 — registry `AzureOpenAi` branch** (registry.rs ~191): if `api_key_env` resolves to a key → `StaticAuth(Auth::Header{name:"api-key",…})` (current); else if AD fields present → `AzureAdAuthenticator::new(...)`; else `StaticAuth(Auth::None)` (current fallback).
- [ ] **Step 4 — tests:** form-body builder + token-response parse as pure functions (fixtures); a registry test that an Azure profile with AD env vars set selects the AD authenticator (assert on a trait-observable property or via a small seam — do NOT hit the network). Construction test like `gcp_authenticator_constructs`.
- [ ] **Step 5 — re-export** `AzureAdAuthenticator` in `providers/src/lib.rs`.
- [ ] **Step 6 — commit.** `git commit -m "feat(providers): AzureAdAuthenticator (client-credentials) + AD profile config + registry wiring"` (+ trailer).

### Task 4.3: GCP discovery — error polish + documented test

**Files:** Modify `providers/src/authenticator.rs`.

- [ ] **Step 1 — improve the error strings** in `GcpTokenAuthenticator::authorize` to name the fix: e.g. `"GCP auth: no credentials found — set GOOGLE_APPLICATION_CREDENTIALS to a service-account key file, or run `gcloud auth application-default login`. ({e})"`.
- [ ] **Step 2 — add a doc-comment** on `GcpTokenAuthenticator` listing the discovery order (`GOOGLE_APPLICATION_CREDENTIALS` SA-key → ADC → GCE metadata) per spec §9 R5.
- [ ] **Step 3 — commit.** `git commit -m "docs(providers): document GCP credential discovery order + clearer auth error"` (+ trailer).

**Stage 4 gate:** `cargo test -p providers` pass; `cargo clippy -p providers --no-deps --all-targets -- -D warnings` clean; `bash scripts/check-deps.sh` OK (no new crates added in this stage — confirm).

---

## Stage 5 — Bedrock TRUE streaming (high; additive frozen-`traits` method)

Spec §3.5. Add a raw-byte stream method (default impl keeps all existing transports/mocks working), use `aws-smithy-eventstream` to decode binary frames, extract the inner Anthropic event JSON, map to canonical `StreamEvent` via a shared `anthropic_wire` mapper.

### Task 5.1: `HttpTransport::stream_raw_bytes` (frozen `traits/`, additive + default)

**Files:** Modify `traits/src/http.rs`.

- [ ] **Step 1 — add the type + method.** In `traits/src/http.rs`:
```rust
/// A pinned, boxed stream of raw response-body byte chunks. Returned by
/// [`HttpTransport::stream_raw_bytes`] for binary protocols (e.g. the AWS
/// event-stream); the caller frames/interprets the bytes.
pub type RawByteStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, HttpError>> + Send>>;
```
Add to the trait (after `stream_sse`), **with a default impl** so existing impls compile unchanged:
```rust
    /// Open a raw byte stream for a binary response protocol. Default impl
    /// buffers the full body via [`Self::request`] and yields it as one chunk
    /// — correct for decoders that frame a complete buffer (and for test
    /// transports). Production transports override for true incremental bytes.
    async fn stream_raw_bytes(&self, req: HttpRequest) -> Result<RawByteStream, HttpError> {
        let resp = self.request(req).await?;
        if resp.status >= 400 {
            return Err(HttpError::Status { status: resp.status, body: resp.body });
        }
        let bytes = resp.body.into_bytes();
        Ok(Box::pin(futures_util::stream::once(async move { Ok(bytes) })))
    }
```
- [ ] **Step 2 — dep for the default.** `traits` currently uses `futures_core`. Add `futures-util` (workspace) to `traits/Cargo.toml` IF not present, OR avoid it by hand-rolling a once-stream with `futures_core` + a tiny `poll`-based wrapper. Prefer reusing the workspace `futures-util` if `traits` may depend on it; otherwise keep the default impl `futures_core`-only. Decide by checking `traits/Cargo.toml` and `scripts/check-deps.sh` graph rules (traits is a leaf — adding `futures-util` may violate the dep-gate; if so, hand-roll).
- [ ] **Step 3 — keep the trait's existing unit test green;** add a test that a trivial mock returns one chunk from the default. `cargo test -p traits`.
- [ ] **Step 4 — gate `traits` clippy.** `cargo clippy -p traits --no-deps --all-targets -- -D warnings` clean (this is the crate that previously had the doc lint — keep it pristine).
- [ ] **Step 5 — commit.** `git add lingxi-code/traits && git commit -m "feat(traits): additive HttpTransport::stream_raw_bytes (default-impl, parity-safe) for binary streams"` (+ trailer).

### Task 5.2: production transports — true incremental `stream_raw_bytes`

**Files:** Modify `platforms/posix/src/http.rs`, `platforms/windows/src/http.rs`, `platforms/posix-minimal/src/http.rs`.

- [ ] **Step 1 — posix override** (mirror `stream_sse`'s request build, then map `resp.bytes_stream()`):
```rust
    async fn stream_raw_bytes(&self, req: HttpRequest) -> Result<traits::http::RawByteStream, HttpError> {
        // ...build `rb` exactly as stream_sse does (method/headers/body/timeout)...
        let resp = rb.send().await.map_err(|e| HttpError::Connection(e.to_string()))?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(HttpError::Status { status, body });
        }
        let s = resp.bytes_stream()
            .map(|r| r.map(|b| b.to_vec()).map_err(|e| HttpError::Connection(e.to_string())));
        Ok(Box::pin(s))
    }
```
(Refactor the duplicated request-builder into a private helper if it reduces copy-paste, following the file's existing style.)
- [ ] **Step 2 — windows + posix-minimal**: same shape (posix-minimal may not support streaming — it can keep the trait default; only override if it has a `bytes_stream` equivalent). Document any that intentionally keep the default.
- [ ] **Step 3 — verify.** `cargo build -p posix-platform` (adjust names) + existing platform tests.
- [ ] **Step 4 — commit.** `git commit -m "feat(platforms): true incremental stream_raw_bytes on reqwest transports"` (+ trailer).

### Task 5.3: `anthropic_wire` mapper + `aws-smithy-eventstream` Bedrock streaming

**Files:** new `providers/src/anthropic_wire.rs`; modify `providers/src/bedrock.rs`, `providers/src/lib.rs`, `providers/Cargo.toml`, `providers/src/testutil.rs`.

- [ ] **Step 1 — deps (the spike pins).** From `lingxi-code/`:
```bash
cargo add aws-smithy-eventstream@=0.60.13 -p providers
cargo update aws-smithy-types --precise 1.3.6
cargo build -p providers   # must be Finished on Rust 1.82
```
If the build pulls `aws-smithy-types` back to a 1.91 version, re-pin `=1.3.6` and add a comment in `Cargo.toml` documenting the pin + reason (MSRV 1.82). Then `bash scripts/check-deps.sh` and add the `aws-smithy-eventstream` license/graph allowance if flagged (follow how `aws-sigv4` was allowed in P6).
- [ ] **Step 2 — `anthropic_wire.rs`: pure mapper.** Extract a pure function that maps one Anthropic stream-event JSON `serde_json::Value` (the inner payload of a Bedrock chunk) into `Option<api_client::types::StreamEvent>`. Reuse `api_client`'s existing Anthropic SSE decode if it exposes a value→event function; if it only parses `data:` lines, build the SSE line (`format!("event: {t}\ndata: {json}\n\n")`) and call `api_client::sse::parse_sse_chunks`. Add unit tests mapping each event type (`message_start`/`content_block_delta` text + input_json/`message_delta`/`message_stop`) to the canonical `StreamEvent`, asserting parity with what `synthesize_stream` emits for the same content.
- [ ] **Step 3 — Bedrock streaming.** In `bedrock.rs` add `fn invoke_with_response_stream_url(&self, model)` → `…/model/{model}/invoke-with-response-stream`. Rewrite `stream()`:
  - build body (same `build_body`), `authorize`, then `transport.stream_raw_bytes(http)`.
  - feed the byte stream into an `aws_smithy_eventstream::frame::MessageFrameDecoder` (adapt exact API to 0.60.13): accumulate bytes, decode complete frames; for each frame, the payload is Bedrock's `{"bytes": "<base64 anthropic-event-json>"}` (or the raw JSON depending on the frame's `:event-type` — handle `chunk` events; surface `:exception-type` frames as `ApiError`). Base64-decode the inner bytes, `serde_json::from_slice` → Value, run `anthropic_wire::map_event` → emit `StreamEvent`.
  - Return a `BoxStream<'static, Result<StreamEvent, ApiError>>` built via `futures::stream::unfold` over the byte stream + a frame buffer (mirror the `sse_event_stream` unfold pattern in `platforms/posix/src/http.rs`).
  - Keep `complete()` and `synthesize_stream` (still used by `complete`-only callers / as the non-streaming `/invoke` path; do NOT delete).
- [ ] **Step 4 — testutil mock.** Add a `MockTransport::responding_raw_frames(frames: Vec<Vec<u8>>)` (or override `stream_raw_bytes`) in `providers/src/testutil.rs` so a Bedrock streaming test can feed fixed event-stream frames without a network.
- [ ] **Step 5 — Bedrock streaming test** (bedrock.rs `tests`): encode 2–3 Anthropic events as event-stream frames (use `aws-smithy-eventstream`'s encoder in the test, or hand-build the frame bytes), feed via the mock, assert the decoded `StreamEvent` sequence starts with `MessageStart` and ends with `MessageStop` and contains the expected `TextDelta`.
- [ ] **Step 6 — re-export** `anthropic_wire` (if public surface is needed) and verify `cargo test -p providers`.
- [ ] **Step 7 — commit.** `git add lingxi-code/providers lingxi-code/Cargo.lock && git commit -m "feat(llm-v2): real Bedrock event-stream streaming (aws-smithy-eventstream + anthropic_wire)"` (+ trailer).

**Stage 5 gate:** `cargo build --workspace` Finished on 1.82; `cargo test -p traits -p providers -p orchestrator -p test-harness` pass (parity green); `cargo clippy -p providers -p traits --no-deps --all-targets -- -D warnings` clean; `bash scripts/check-deps.sh` OK.

---

## Stage 6 — TUI paste→image (high; additive frozen-`traits` method)

Spec §3.1 / R3. TUI records pasted-image attachments (`state.paste.attachments`, paths) but never threads them to the orchestrator. Add an images-aware handle method (default delegates → existing impls untouched); the orchestrator reads each file, base64-encodes, builds `ContentBlock::Image`, and appends to the outgoing user message.

### Task 6.1: image-loading helper + outgoing-message construction

**Files:** Modify `protocol/src/messages.rs` (helper), `orchestrator/src/conversation.rs`.

- [ ] **Step 1 — `protocol` helper.** Add a function to build an `ImageSource::Base64` from raw bytes + a media-type, and a `ConversationMessage::user_with_images(text: String, images: Vec<ImageSource>)` that produces `content = [Text, Image, Image, …]` (Text first when non-empty). Unit test: round-trips through serde unchanged; image-free call equals `ConversationMessage::user(text)`.
- [ ] **Step 2 — orchestrator file→block loader.** In `conversation.rs`, add `fn load_image_block(path: &Path) -> Result<protocol::ContentBlock, OrchestratorError>`: read bytes, detect media-type from extension (`png`/`jpeg`/`gif`/`webp`; default `application/octet-stream` → error if unknown), base64-encode (use the workspace base64 dep — confirm it's available; orchestrator likely already has one transitively), return `ContentBlock::Image{ source: Base64{…} }`. Map I/O errors to a clear `OrchestratorError`.
- [ ] **Step 3 — inherent streaming variant.** Add `ConversationOrchestrator::run_turn_streaming_with_cancel_images(&self, prompt, image_paths: &[PathBuf], cancel)` that builds the user message via `user_with_images` (loading each path; on a load error, surface it as a turn error) and otherwise reuses the existing streaming turn body. Refactor the existing `run_turn_streaming_with_cancel` to delegate to it with an empty image list (DRY) — confirm the user-message construction site (conversation.rs:723) is the only divergence.
- [ ] **Step 4 — tests:** image-free path is byte-identical (existing streaming tests stay green); a unit test that `load_image_block` on a tiny PNG fixture yields a `Base64` block with `media_type == "image/png"`.
- [ ] **Step 5 — commit.** `git commit -m "feat(orchestrator): build ContentBlock::Image from pasted image paths on the outgoing user message"` (+ trailer).

### Task 6.2: images-aware handle method (frozen `traits/`, additive + default)

**Files:** Modify `traits/src/orchestrator.rs`, `orchestrator/src/handle_impl.rs`, `orchestrator/src/test_support.rs`.

- [ ] **Step 1 — trait method with default** (traits/src/orchestrator.rs, after `run_turn_streaming_with_cancel`):
```rust
    /// Streaming turn carrying pasted image file paths (TUI paste→image).
    /// Default delegates to [`Self::run_turn_streaming_with_cancel`] ignoring
    /// images, so non-TUI handle impls need no override.
    async fn run_turn_streaming_with_images(
        &self,
        prompt: &str,
        image_paths: &[std::path::PathBuf],
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<TurnOutcome, HandleError> {
        let _ = image_paths;
        self.run_turn_streaming_with_cancel(prompt, cancel).await
    }
```
- [ ] **Step 2 — `OrchestratorHandleImpl` override** (handle_impl.rs): map to `ConversationOrchestrator::run_turn_streaming_with_cancel_images`, converting outcomes like the existing override does.
- [ ] **Step 3 — keep `traits` clippy/tests green;** object-safety test (`_g<T: OrchestratorHandle>`) still compiles. `cargo test -p traits`.
- [ ] **Step 4 — commit.** `git commit -m "feat(traits): additive OrchestratorHandle::run_turn_streaming_with_images (default-impl)"` (+ trailer).

### Task 6.3: TUI threads attachments → handle, clears after submit

**Files:** Modify `tui/src/app.rs` (and `spawn_streaming_turn`).

- [ ] **Step 1 — collect paths.** At submit, read `state.paste.attachments` (the `Attachment{source: path}` list), collect `Vec<PathBuf>` in insertion order.
- [ ] **Step 2 — call the images method.** Change `spawn_streaming_turn` to accept the image paths and call `handle.run_turn_streaming_with_images(&prompt, &image_paths, cancel)` (instead of `…_with_cancel`); when empty it behaves identically (default delegates).
- [ ] **Step 3 — clear attachments** after a turn is spawned so they aren't re-sent next turn (reset `state.paste` attachments + `next_image_id`).
- [ ] **Step 4 — test:** extend `tui/tests/image_paste_test.rs` (or app-level test) to assert the collected paths reach the (mock) handle's images method. If the TUI test harness can't observe the handle call, assert the state-clearing + that `spawn_streaming_turn` is invoked with the paths.
- [ ] **Step 5 — session JSONL round-trip test** (orchestrator): a user message with an `Image` block writes + reads back byte-identically (additive serde variant). Place where the existing `message_to_jsonl` tests live.
- [ ] **Step 6 — commit.** `git commit -m "feat(tui): route pasted images into ContentBlock::Image on the outgoing turn"` (+ trailer).

**Stage 6 gate:** `cargo test -p traits -p protocol -p orchestrator -p tui` pass; clippy clean on touched crates; full parity suite (`test-harness`) green.

---

## Stage 7 — Docs + final holistic gate

**Files:** Modify `CHANGELOG.md`, `README.md`, `docs/LLM_PROVIDERS.md`.

- [ ] **Step 1 — CHANGELOG.** Add a `## [0.12.1]` (or `[Unreleased]`, per the repo's convention — check the top of `CHANGELOG.md`) entry documenting: real Bedrock streaming; AWS credential discovery (env/profile/process/IMDSv2) + the `aws-smithy-types =1.3.6` MSRV pin + the "no `aws-config`/SSO-full-OIDC" note (read cached SSO creds only); `AzureAdAuthenticator`; Bedrock/Vertex/Azure pricing fix; `/model` real listing + `models` config field; TUI paste→image. Update/replace the v0.12.0 "Unchanged / bounded" bullet that said these were deferred.
- [ ] **Step 2 — `docs/LLM_PROVIDERS.md`.** Document: `models` profile field; AWS credential discovery order + SSO caveat (`aws sso login`); Azure AD config keys (`azureAdTenant`/`azureAdClientIdEnv`/`azureAdClientSecretEnv`); that Bedrock now streams; the MSRV pin rationale.
- [ ] **Step 3 — `README.md`.** Bump the LLM Providers subsystem row note (streaming + cloud auth now complete).
- [ ] **Step 4 — full workspace gate.** From `lingxi-code/`:
```bash
cargo build --workspace
cargo test -p providers -p orchestrator -p engine -p cost -p traits -p protocol -p tui -p test-harness
cargo clippy --workspace --all-targets -- -D warnings   # NOTE: full workspace, NOT --no-deps — confirm green now that traits is clean
bash scripts/check-deps.sh
```
- [ ] **Step 5 — commit.** `git commit -m "docs(llm-v2): document finished deferrals (streaming, cred discovery, pricing, /model, paste-image)"` (+ trailer).

---

## Self-Review

**Spec coverage:** Bedrock streaming §3.5 (Stage 5) ✓; cred discovery §2.1/§4 (Stage 4) ✓ via lightweight chain (spike-justified deviation from `aws-config`); pricing §5 (Stage 2) ✓; `/model` §3.6 (Stage 3) ✓ incl. the missing `models` field; paste→image §3.1/R3 (Stage 6) ✓; tech debt (Stage 1) ✓; docs (Stage 7) ✓.

**Frozen-`traits` touches (authorized):** only two, both additive **default-impl** methods (`HttpTransport::stream_raw_bytes`, `OrchestratorHandle::run_turn_streaming_with_images`) — existing impls/mocks compile unchanged; parity suite is the gate. The doc_markdown item is already fixed (no task).

**Placeholder scan:** version-specific bits (`aws-smithy-eventstream 0.60.13` frame API, Bedrock chunk shape, exact settings struct path, base64 crate name) carry explicit "confirm/adapt" instructions with the fixture-test + build gate as the backstop — matching this repo's P6 plan convention. No bare TODOs.

**Type consistency:** `RawByteStream = Vec<u8>` chunks (not `bytes::Bytes`, to keep `traits` leaf-clean); `ProviderId::AmazonBedrock` used in pricing, cost_wiring, and `BedrockProvider::id()`; `ProviderProfile.models: Vec<String>` referenced in profile/registry/settings; `available_models()` consistent across `ModelRouter` (registry), `OrchestratorApiClient` (seam), and `handle_impl`.

**Risk notes:** Stage 5 is heaviest (frozen trait + new dep + binary framing) — its gate runs the full parity suite. Stage 4 SSO is intentionally shallow (cached creds + guidance error); full SSO OIDC is out of scope and documented. The `aws-smithy-types =1.3.6` pin is load-bearing for the 1.82 build and is documented in `Cargo.toml`.
