# llm-client Engine Adoption — Plan 2 of 3 (oauth bridge + orchestrator policy ports)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bridge anthropic-oauth into llm-client's credential seam and port api-client's claude-code-parity policy modules (retry driver, rate-limit, betas, fallback, overflow copy, telemetry emission, count_tokens facade) into orchestrator as standalone llm-client-typed modules — WITHOUT yet touching the live engine path (api-client stays alive until Plan 3).

**Architecture:** Spec `docs/superpowers/specs/2026-06-10-llm-client-engine-adoption-design.md` rev2.1, phases P2b+P3, with three exploration-driven amendments executed in Task 7: (a) the TUI never imports `api_client::rate_limit` (message text arrives as a pre-formatted String), so the rate-limit status type stays orchestrator-local — no protocol-crate move; (b) `llm_client::CredentialProvider::load` becomes async (BoxFuture, same hand-rolled pattern as `Transport`) so "token refresh inside load()" is implementable; (c) `LlmError` gains an `Overloaded` variant (529/overloaded_error currently collapses into `ProviderInternal`, losing the bit `MAX_529_RETRIES` and opus-fallback need), and `DefaultLlmClient` gains `prepare_count_tokens` (resolve+validate+encode+authenticate for the count_tokens endpoint — closes the tracked auth-seam prereq).

**Tech Stack:** Rust 2021, workspace at `lingxi-code/`, TDD, clippy clean per touched crate, conventional commits with trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

**Working directory:** run cargo from `lingxi-code/`; git from the worktree root.

**Porting convention used below:** "PORT `api-client/src/X.rs` → `orchestrator/src/model/X.rs`" means: copy the file verbatim INCLUDING its `#[cfg(test)]` tests and doc comments, then apply only the listed mechanical changes. The source file stays untouched (api-client is deleted in Plan 3). Where parity constants are byte-locked, the ported tests asserting them are the lock.

---

### Task 1: llm-client — async CredentialProvider + `LlmError::Overloaded` + `prepare_count_tokens`

**Files:**
- Modify: `llm-client/src/credentials.rs`, `llm-client/src/error.rs`, `llm-client/src/retry.rs`, `llm-client/src/providers/anthropic.rs`, `llm-client/src/providers/mod.rs`, `llm-client/src/client.rs`
- Test: `llm-client/tests/client_auth_test.rs`, `llm-client/tests/error_decode_test.rs`, `llm-client/tests/retry_test.rs`, `llm-client/tests/anthropic_codec_test.rs` (prepare_count_tokens test goes in `client_auth_test.rs`)

- [ ] **Step 1: Write the failing tests**

In `llm-client/tests/client_auth_test.rs`, the `RecordingStore` impl block must change shape (async trait) — replace its `impl CredentialProvider for RecordingStore` with:

```rust
    impl CredentialProvider for RecordingStore {
        fn load<'a>(
            &'a self,
            scope: &'a CredentialScope,
        ) -> llm_client::BoxFuture<'a, Result<Credential, LlmError>> {
            let id = scope.credential_id.as_deref().unwrap_or("missing").to_string();
            Box::pin(async move { Ok(Credential::BearerToken(format!("token-for-{id}"))) })
        }
    }
```

and append these tests:

```rust
#[tokio::test]
async fn prepare_count_tokens_is_authenticated_for_anthropic_routes() {
    std::env::set_var("LLM_CLIENT_AUTH_TEST_CT", "ct-key");
    let client = client_with(
        ProviderId::AnthropicFirstParty,
        ProtocolFamily::AnthropicMessages,
        "https://api.anthropic.com",
        AuthStrategy::ApiKey,
        CredentialConfig::Env { var: "LLM_CLIENT_AUTH_TEST_CT".to_string() },
    );

    let prepared = client
        .prepare_count_tokens(&LlmRequest::new("p-model").with_user_text("hi"))
        .await
        .expect("prepared");

    assert!(prepared.url.ends_with("/v1/messages/count_tokens"));
    assert_eq!(prepared.headers.get("x-api-key").map(String::as_str), Some("ct-key"));
    assert!(prepared.body_json.get("max_tokens").is_none());
}

#[tokio::test]
async fn prepare_count_tokens_rejects_non_anthropic_routes() {
    let client = client_with(
        ProviderId::OpenAI,
        ProtocolFamily::OpenAiChat,
        "https://api.openai.com/v1",
        AuthStrategy::None,
        CredentialConfig::None,
    );

    let err = client
        .prepare_count_tokens(&LlmRequest::new("p-model").with_user_text("hi"))
        .await
        .unwrap_err();

    assert!(matches!(err, LlmError::InvalidRequest { message } if message.contains("count_tokens")));
}
```

NOTE: the existing auth tests call `client.prepare(...)` — `prepare` becomes ASYNC in this task (it awaits credential loading). Convert every `#[test] fn` in `client_auth_test.rs` that calls `prepare` into `#[tokio::test] async fn` with `.await`. Same for `transport_execute_test.rs`/`transport_stream_test.rs` if they call `prepare` indirectly via `execute`/`execute_stream` (those are already async — only `client_route_test.rs` needs converting where it calls `prepare`).

In `llm-client/tests/error_decode_test.rs` append:

```rust
#[test]
fn overloaded_maps_to_dedicated_variant() {
    let codec = anthropic_codec();

    assert!(matches!(
        codec.decode_response(anthropic_error(529, "overloaded_error", "Overloaded")).unwrap_err(),
        LlmError::Overloaded
    ));
    assert!(matches!(
        anthropic_codec().decode_response(ProviderResponse::json(529, serde_json::Value::Null)).unwrap_err(),
        LlmError::Overloaded
    ));
    assert!(matches!(
        codec.decode_response(anthropic_error(500, "api_error", "boom")).unwrap_err(),
        LlmError::ProviderInternal
    ));
}
```

In `llm-client/tests/retry_test.rs` append:

```rust
#[test]
fn overloaded_errors_are_retryable() {
    assert_eq!(
        RetryPolicy.classify_error(&LlmError::Overloaded),
        RetryDecision::Retry { after: None }
    );
}
```

- [ ] **Step 2: Run to verify RED**

Run: `cargo test -p llm-client --no-fail-fast 2>&1 | grep -E "^error" | sort -u | head`
Expected: missing `Overloaded` variant, `prepare_count_tokens` not found, CredentialProvider impl signature mismatch.

- [ ] **Step 3: Implement**

`llm-client/src/error.rs` — add a variant after `ProviderInternal`:

```rust
    /// Provider reported overload (Anthropic 529 / overloaded_error).
    #[error("provider overloaded")]
    Overloaded,
```

`llm-client/src/retry.rs` — `classify_error`: move nothing else; add `LlmError::Overloaded` next to `ProviderInternal` in the retry arm:

```rust
            LlmError::Transport { .. } | LlmError::ProviderInternal | LlmError::Overloaded => {
                RetryDecision::Retry { after: None }
            }
```

`llm-client/src/providers/anthropic.rs::map_error` — change the fallthrough comment block: add before the `_ =>` arm:

```rust
        "overloaded_error" => LlmError::Overloaded,
```

`llm-client/src/providers/mod.rs::map_error_status` — add before the `_ =>` arm:

```rust
        529 => LlmError::Overloaded,
```

`llm-client/src/credentials.rs` — make the trait async via BoxFuture and update the two built-ins:

```rust
use crate::{BoxFuture, LlmError, ProviderId};
```

```rust
/// Loads credentials for a provider/profile scope.
///
/// `load` is async so implementations can refresh expiring material
/// (e.g. OAuth) inside the lookup.
pub trait CredentialProvider: fmt::Debug + Send + Sync {
    /// Load credential material for a scope.
    fn load<'a>(
        &'a self,
        scope: &'a CredentialScope,
    ) -> BoxFuture<'a, Result<Credential, LlmError>>;
}
```

`StaticCredentialProvider`:

```rust
impl CredentialProvider for StaticCredentialProvider {
    fn load<'a>(
        &'a self,
        _scope: &'a CredentialScope,
    ) -> BoxFuture<'a, Result<Credential, LlmError>> {
        let credential = self.credential.clone();
        Box::pin(async move { Ok(credential) })
    }
}
```

`EnvCredentialProvider`:

```rust
impl CredentialProvider for EnvCredentialProvider {
    fn load<'a>(
        &'a self,
        _scope: &'a CredentialScope,
    ) -> BoxFuture<'a, Result<Credential, LlmError>> {
        let result = std::env::var(&self.variable_name)
            .map(Credential::ApiKey)
            .map_err(|_| LlmError::Authentication);
        Box::pin(async move { result })
    }
}
```

`llm-client/src/client.rs` — `prepare`, `authenticate`, `load_secret` become async (`pub async fn prepare`, `async fn authenticate`, `async fn load_secret`) with `.await` on the two `load(...)` calls and on the internal calls (`self.authenticate(...).await`, `self.load_secret(...).await`); `execute`/`execute_stream` add `.await` to their `self.prepare(request)` calls. Then add the count_tokens preparation (after `execute_stream`):

```rust
    /// Resolve, validate, encode, and authenticate an Anthropic
    /// `count_tokens` call. Errors on non-Anthropic routes.
    pub async fn prepare_count_tokens(
        &self,
        request: &LlmRequest,
    ) -> Result<ProviderRequest, LlmError> {
        let resolved_route = self.registry.resolve(&request.model)?;
        validate_capabilities(request, resolved_route.capabilities)?;

        let entry = self
            .routes
            .get(&resolved_route.profile_name)
            .ok_or(LlmError::ModelUnavailable)?;
        if !matches!(entry.protocol, ProtocolFamily::AnthropicMessages) {
            return Err(LlmError::InvalidRequest {
                message: format!(
                    "count_tokens is only available on AnthropicMessages routes, not {:?}",
                    entry.protocol
                ),
            });
        }

        let codec = crate::AnthropicMessagesCodec::new(entry_base_url(entry), "2023-06-01");
        let mut routed_request = request.clone();
        routed_request.model.clone_from(&resolved_route.request_model);
        let provider_request = codec.encode_count_tokens_request(&routed_request)?;
        self.authenticate(entry, &resolved_route.profile_name, provider_request)
            .await
    }
```

Implementation note for the executor: `RouteEntry` does not store `base_url` today; the cleanest concrete approach (do this rather than the `entry_base_url` placeholder above): add `base_url: String` to `RouteEntry` (populated in `from_config` from `provider.base_url` BEFORE `provider.profile_name` is moved), and construct the codec from it as shown. Alternatively store a second prepared codec — do NOT downcast `Box<dyn WireCodec>`.

- [ ] **Step 4: Full suite + clippy GREEN**

Run: `cargo test -p llm-client -p platform-common 2>&1 | awk '/test result: ok/ {p+=$4} /FAILED/ {f+=1} END {print p" passed, "f+0" failed"}'` — all passed (count grows by 4).
Run: `cargo clippy -p llm-client -p platform-common --all-targets 2>&1 | grep -cE "^(warning|error)" || true` → 0.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/llm-client lingxi-code/platforms/common lingxi-code/Cargo.lock && git commit -m "feat(llm-client)!: async CredentialProvider, Overloaded error class, authenticated count_tokens prepare

load() returns BoxFuture so credential stores can refresh OAuth material
in place; prepare()/execute()/execute_stream() become async accordingly.
Anthropic 529/overloaded_error maps to a dedicated retryable
LlmError::Overloaded (the retry driver needs the distinction for
MAX_529_RETRIES and opus fallback). DefaultLlmClient::prepare_count_tokens
closes the unauthenticated count_tokens seam.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: anthropic-oauth implements `llm_client::CredentialProvider`

**Files:**
- Modify: `anthropic-oauth/Cargo.toml` (add `llm-client = { path = "../llm-client" }`)
- Create: `anthropic-oauth/src/credential_provider.rs`
- Modify: `anthropic-oauth/src/lib.rs` (`pub mod credential_provider;` + re-export `OAuthCredentialProvider`)
- Test: `anthropic-oauth/tests/credential_provider_test.rs`

- [ ] **Step 1: Read the local API first**

Read `anthropic-oauth/src/refresh.rs` and `anthropic-oauth/src/testsupport.rs` to confirm: `AuthState` field access (`token: RwLock<TokenInfo>`), `TokenInfo { access_token: Secret<String>, expires_at: SystemTime, .. }`, how `RefreshDriver` exposes refresh (per M3-04 it implements `api_client::oauth_hook::OAuthRefreshHook::refresh`), which test-support constructors exist for building an `AuthState`/`RefreshDriver` with a fake transport+clock, and how secrecy exposes the token string (`ExposeSecret::expose_secret`). Adjust the literal code below to the actual constructor names — the BEHAVIORAL contract in the tests is fixed.

- [ ] **Step 2: Write the failing test**

`anthropic-oauth/tests/credential_provider_test.rs` — behavioral contract (adapt constructors to testsupport):

```rust
use anthropic_oauth::OAuthCredentialProvider;
use llm_client::{Credential, CredentialProvider, CredentialScope, ProviderId};

#[tokio::test]
async fn load_returns_current_token_when_fresh() {
    // build AuthState via testsupport with a non-expired token "tok-fresh"
    let provider = OAuthCredentialProvider::new(/* auth state / driver from testsupport */);

    let credential = provider
        .load(&CredentialScope::new(ProviderId::AnthropicFirstParty, "anthropic"))
        .await
        .expect("credential");

    assert_eq!(credential, Credential::BearerToken("tok-fresh".to_string()));
}

#[tokio::test]
async fn load_refreshes_expired_token_single_flight() {
    // build AuthState with an EXPIRED token and a scripted refresh returning "tok-refreshed"
    let provider = OAuthCredentialProvider::new(/* ... */);

    let credential = provider
        .load(&CredentialScope::new(ProviderId::AnthropicFirstParty, "anthropic"))
        .await
        .expect("credential");

    assert_eq!(credential, Credential::BearerToken("tok-refreshed".to_string()));
}

#[tokio::test]
async fn refresh_failure_maps_to_authentication_error() {
    // expired token + scripted refresh FAILURE
    let provider = OAuthCredentialProvider::new(/* ... */);

    let err = provider
        .load(&CredentialScope::new(ProviderId::AnthropicFirstParty, "anthropic"))
        .await
        .expect_err("must fail");

    assert!(matches!(err, llm_client::LlmError::Authentication));
}
```

- [ ] **Step 3: Verify RED, then implement**

`anthropic-oauth/src/credential_provider.rs` shape (adapt to actual AuthState/RefreshDriver API):

```rust
//! `llm_client::CredentialProvider` over the OAuth refresh machinery.

use std::sync::Arc;

use llm_client::{BoxFuture, Credential, CredentialProvider, CredentialScope, LlmError};
use secrecy::ExposeSecret;

/// Serves the current OAuth access token, refreshing in place when expired
/// (single-flight via the underlying refresh lock).
#[derive(Debug)]
pub struct OAuthCredentialProvider {
    driver: Arc<crate::RefreshDriver>,
}

impl OAuthCredentialProvider {
    /// Wrap a refresh driver / auth state handle.
    pub fn new(driver: Arc<crate::RefreshDriver>) -> Self {
        Self { driver }
    }
}

impl CredentialProvider for OAuthCredentialProvider {
    fn load<'a>(
        &'a self,
        _scope: &'a CredentialScope,
    ) -> BoxFuture<'a, Result<Credential, LlmError>> {
        Box::pin(async move {
            // expired (per the state's clock)? → refresh (single-flight,
            // double-checked inside the driver) then read again.
            // Map any refresh failure to LlmError::Authentication with no
            // secret material in the message.
            let token = self
                .driver
                .fresh_access_token() // adapt: whatever returns a guaranteed-fresh token
                .await
                .map_err(|_| LlmError::Authentication)?;
            Ok(Credential::BearerToken(token.expose_secret().to_string()))
        })
    }
}
```

If no `fresh_access_token`-like method exists, implement the expiry check + `refresh()` + re-read sequence inside `load` using `AuthState.token` RwLock and the driver's refresh entrypoint. Do NOT add new public API to `RefreshDriver` unless nothing suitable exists; if you must, keep it minimal (`pub async fn fresh_access_token(&self) -> Result<Secret<String>, OAuthHookError>`).

- [ ] **Step 4: GREEN + clippy + commit**

Run: `cargo test -p anthropic-oauth 2>&1 | grep -c FAILED` → 0; clippy clean.

```bash
git add lingxi-code/anthropic-oauth lingxi-code/Cargo.lock && git commit -m "feat(anthropic-oauth): implement llm_client::CredentialProvider

OAuthCredentialProvider serves the current access token and refreshes
in place (single-flight) when expired; failures map to
LlmError::Authentication. Bridges OAuth into DefaultLlmClient::
with_credential_provider without the api-client hook indirection.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: orchestrator `model::retry` — generic retry driver

**Files:**
- Modify: `orchestrator/Cargo.toml` (add `llm-client = { path = "../llm-client" }`, `rand = "0.8"`)
- Create: `orchestrator/src/model/mod.rs`, `orchestrator/src/model/retry.rs`, `orchestrator/src/model/overflow.rs`
- Modify: `orchestrator/src/lib.rs` (`pub mod model;`)

- [ ] **Step 1: PORT `api-client/src/overflow.rs` → `orchestrator/src/model/overflow.rs`**

Verbatim copy (constants `FLOOR_OUTPUT_TOKENS = 3000`, `SAFETY_BUFFER = 1000`, `Overflow` struct, `parse_max_tokens_overflow`, `adjusted_max_tokens`, regex byte-identical, ALL tests) with one mechanical change: `parse_max_tokens_overflow(status: u16, body: &str)` gains a sibling used by the new driver:

```rust
/// Parse the overflow shape out of an llm-client InvalidRequest message.
pub fn parse_overflow_message(message: &str) -> Option<Overflow> {
    parse_max_tokens_overflow(400, message)
}
```

(plus a test: the canonical message string parses; an unrelated message returns None).

- [ ] **Step 2: Write `orchestrator/src/model/retry.rs` — failing tests first**

Cadence constants are PORTED byte-exact from `api-client/src/retry.rs` WITH their lock tests (`DEFAULT_BASE_DELAYS_MS = [500, 1_000, 2_000]`, `DEFAULT_RETRY_BUDGET = 3`, `MAX_529_RETRIES = 3`, `JITTER_LOW = 0.8`, `JITTER_HIGH = 1.2`, `jittered_delay`, tests `jitter_bounds_are_locked_against_spec` / `stays_within_plus_minus_20_percent` / `default_base_delays_match_spec`). The new driver API and its tests:

```rust
/// What the driver decided after one failed attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum DriveStep {
    /// Sleep `delay` then retry the same request.
    RetryAfter(std::time::Duration),
    /// Re-encode with this max_tokens then retry (529-independent).
    AdjustMaxTokens(u32),
    /// Switch to the fallback model then retry.
    Fallback { fallback_model: String },
    /// Surface the error to the caller.
    Terminal,
}

/// Per-call mutable retry state (attempts, consecutive overloads).
#[derive(Debug, Default)]
pub struct RetryState {
    pub attempt: u8,
    pub consecutive_overloaded: u8,
}

/// Claude-code-parity retry decisions over llm-client's error taxonomy.
/// `thinking_budget` feeds the overflow adjustment; `ctl` mirrors the old
/// RetryControl (fallback gating).
pub fn next_step(
    state: &mut RetryState,
    ctl: &RetryControl,
    error: &llm_client::LlmError,
    thinking_budget: u32,
) -> DriveStep
```

Decision table the tests pin (write these as `#[test]`s FIRST, watch them fail, then implement):
1. `LlmError::Overloaded`, `allow_fallback=true`, fallback configured, after `MAX_529_RETRIES` consecutive overloads → `Fallback`; below the threshold → `RetryAfter(jittered base delay for the attempt)`; counter resets on any non-overloaded error.
2. `LlmError::RateLimited { retry_after: Some(d), .. }` → `RetryAfter(d)` (server wins, no jitter); `retry_after: None` → `RetryAfter(jittered)`.
3. `LlmError::ProviderInternal` / `Transport` → `RetryAfter(jittered)` while `state.attempt < DEFAULT_RETRY_BUDGET`, else `Terminal`.
4. `LlmError::InvalidRequest { message }` where `overflow::parse_overflow_message(message)` parses and `adjusted_max_tokens(...)` yields Some(n) → `AdjustMaxTokens(n)` (does not consume a budget attempt — mirror api-client `AdjustAndRetry`); unparseable InvalidRequest → `Terminal`.
5. `Authentication`/`PermissionDenied`/`ContextOverflow`/`QuotaExceeded`/`ModelUnavailable`/`StreamInterrupted`/`CostUnavailable`/`UnsupportedCapability` → `Terminal`.
6. Budget exhaustion: once `state.attempt >= DEFAULT_RETRY_BUDGET`, every otherwise-retryable class → `Terminal`.
7. `RetryControl` is PORTED as-is (`max_529_retries`, `fallback_model`, `primary_model`, `allow_fallback`, `is_external`, `is_sandbox`) with `is_non_custom_opus` gating ported in Task 5 wired by the caller (the driver only honors `allow_fallback && fallback_model.is_some()`).
8. `RetryPolicy` (llm-client) is consulted for the retry/no-retry split so the two layers cannot drift: assert in a test that every `DriveStep::Terminal`-class error is `RetryDecision::DoNotRetry` and vice versa (excluding the AdjustMaxTokens special case).

`next_step` is synchronous and pure given (state, ctl, error) — the CALLER sleeps (`tokio::time::sleep`) and re-executes; that keeps it unit-testable without a runtime and leaves transport/UI emission with the caller for Plan 3 wiring.

- [ ] **Step 3: GREEN + clippy + commit**

Run: `cargo test -p orchestrator model:: 2>&1 | grep "test result"` (plus full `cargo test -p orchestrator 2>&1 | grep -c FAILED` → 0) and clippy clean.

```bash
git add lingxi-code/orchestrator lingxi-code/Cargo.lock && git commit -m "feat(orchestrator): claude-code-parity retry driver over llm-client errors

Ports the locked cadence (500ms/1s/2s +/-20%, budget 3, MAX_529_RETRIES)
and max_tokens-overflow adjustment from api-client; decisions are pure
(DriveStep) over LlmError with the dedicated Overloaded class driving
consecutive-529 fallback. Cross-checked against RetryPolicy so the
classification layers cannot drift.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: orchestrator `model::rate_limit` — wholesale port

**Files:**
- Create: `orchestrator/src/model/rate_limit.rs` (PORT of `api-client/src/rate_limit.rs`)
- Modify: `orchestrator/src/model/mod.rs`

- [ ] **Step 1: PORT the module verbatim** (all pub items: `RATE_LIMIT_ERR_FMT`, `format_rate_limited_msg`, `parse_retry_after`, `parse_anthropic_ratelimit_reset`, `parse_unified_reset`, `PERSISTENT_RESET_CAP_MS`, `format_reset_time`, ALL tests including the byte-locked `format_reset_time_tests`). Mechanical changes only: `use crate::...` paths → local; if it referenced `ApiError`, re-point to `llm_client::LlmError` equivalents or drop the function from the port ONLY if nothing else in this plan calls it (record any drop in the commit message). orchestrator already deps chrono with `clock`; add `iana-time-zone = "0.1"` to orchestrator Cargo.toml (it is already in the workspace lock via api-client).

- [ ] **Step 2: Add one new adapter fn + test** feeding the tracker from llm-client types:

```rust
/// Seconds to wait, from an llm-client rate-limit error (server value wins).
pub fn retry_secs_from_error(error: &llm_client::LlmError) -> Option<u64> {
    match error {
        llm_client::LlmError::RateLimited { retry_after: Some(d), .. } => Some(d.as_secs().max(1)),
        llm_client::LlmError::RateLimited { retry_after: None, .. } => None,
        _ => None,
    }
}
```

Test: `RateLimited{retry_after: Some(7s)}` → `Some(7)`; `Some(0ms)` → `Some(1)`; `None` → `None`; non-rate-limit error → `None`; and `format_rate_limited_msg(7)` equals the byte-locked template output.

- [ ] **Step 3: GREEN + clippy + commit** (`feat(orchestrator): port rate-limit parsing/formatting onto llm-client types`, same trailer).

---

### Task 5: orchestrator `model::{betas, fallback, prompt_too_long}` — wholesale ports

**Files:**
- Create: `orchestrator/src/model/betas.rs` (PORT of `api-client/src/betas.rs`, verbatim incl. the 16 lock tests; no type changes needed — it is string/enum only)
- Create: `orchestrator/src/model/fallback.rs` (PORT of `api-client/src/opus.rs`: `NON_CUSTOM_OPUS_MODELS`, `is_non_custom_opus`, tests)
- Create: `orchestrator/src/model/prompt_too_long.rs` (PORT of `api-client/src/prompt_too_long.rs`: keep `PROMPT_TOO_LONG_ERROR_MESSAGE`, `parse_prompt_too_long_token_counts`, `is_prompt_too_long_body`, `prompt_too_long_token_gap` + their tests; the `classify_prompt_too_long`/`reclassify_prompt_too_long` ApiError-typed fns are NOT ported — llm-client's ContextOverflow classification replaces them; note the drop in the commit message)
- Modify: `orchestrator/src/model/mod.rs`
- Add one integration point + test in `betas.rs`:

```rust
/// Inject the assembled anthropic-beta header into a prepared request.
pub fn apply_beta_header(
    request: &mut llm_client::ProviderRequest,
    provider: Provider,
    endpoint: Endpoint,
) {
    request.headers.insert(
        "anthropic-beta".to_string(),
        assemble_beta_header(provider, endpoint),
    );
}
```

Test: a `ProviderRequest::post_json(...)` gains an `anthropic-beta` header equal to `assemble_beta_header(Provider::Anthropic, Endpoint::MessagesCreate)`.

- [ ] **GREEN + clippy + commit** (`feat(orchestrator): port betas/opus-fallback/prompt-too-long parity modules`, trailer; record the two dropped ApiError-typed fns).

---

### Task 6: orchestrator `model::{telemetry, count_tokens}`

**Files:**
- Create: `orchestrator/src/model/telemetry.rs` — PORT the five `emit_*` fns from `api-client/src/anthropic.rs`'s telemetry module verbatim (same event names `tengu_api_request_started/succeeded/failed/rate_limited`, `tengu_max_tokens_context_overflow_adjustment`; same key names and `LogEventMetadata` value kinds; same `&Option<Arc<AnalyticsBus>>` parameter shape) + their tests if any exist in api-client (check; if none, add one test per fn asserting the event name + keys via whatever capture helper `telemetry` exposes — check `telemetry` crate testsupport; if no capture exists, assert-compile-only is NOT acceptable: use the bus's documented test hook or skip with a `// parity: emission shapes locked by api-client originals until Plan 3 wiring tests` comment and record in report).
- Create: `orchestrator/src/model/count_tokens.rs`:

```rust
//! count_tokens facade: real endpoint on Anthropic routes, documented
//! character-based approximation elsewhere.

use llm_client::{client::DefaultLlmClient, AnthropicMessagesCodec, LlmError, LlmRequest, Transport};

/// Approximation divisor for non-Anthropic routes (chars/4 ≈ tokens).
pub const APPROX_CHARS_PER_TOKEN: u64 = 4;

/// Count input tokens for `request`'s resolved route.
pub async fn count_tokens(
    client: &DefaultLlmClient,
    transport: &dyn Transport,
    request: &LlmRequest,
) -> Result<u64, LlmError> {
    match client.prepare_count_tokens(request).await {
        Ok(provider_request) => {
            let response = transport.execute(&provider_request).await?;
            // decode via a throwaway codec: decode is stateless and
            // base_url-independent.
            AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01")
                .decode_count_tokens_response(&response)
        }
        Err(LlmError::InvalidRequest { message }) if message.contains("count_tokens") => {
            Ok(approximate_tokens(request))
        }
        Err(other) => Err(other),
    }
}

/// Character-count approximation used on non-Anthropic routes.
pub fn approximate_tokens(request: &LlmRequest) -> u64 {
    let mut chars = 0u64;
    for block in &request.system {
        chars += block.text.len() as u64;
    }
    for message in &request.messages {
        for block in &message.content {
            if let llm_client::ContentBlock::Text { text, .. } = block {
                chars += text.len() as u64;
            }
        }
    }
    (chars / APPROX_CHARS_PER_TOKEN).max(1)
}
```

Tests (TDD): approximation math (empty → 1; known char counts divide by 4); anthropic route happy path via a fake `Transport` returning `{"input_tokens": 2095}` → 2095; non-anthropic route falls back to approximation (build a 2-profile client); error statuses propagate (401 envelope → `Authentication`).

- [ ] **GREEN + clippy + commit** (`feat(orchestrator): telemetry emission + count_tokens facade on llm-client`, trailer).

---

### Task 7: spec rev2.2 + workspace verification

**Files:**
- Modify: `docs/superpowers/specs/2026-06-10-llm-client-engine-adoption-design.md`

- [ ] **Step 1: Amend the spec** (worktree root): in §3 `rate_limit.rs` bullet, replace the sentence about the status type moving to the shared protocol crate with: `The TUI consumes a pre-formatted message string from the orchestrator (verified: no api_client::rate_limit imports in tui), so all rate-limit types stay orchestrator-local.` In §2 anthropic-oauth bullet, append: `CredentialProvider::load is async (BoxFuture) for exactly this reason.` In §1 add a bullet: `LlmError::Overloaded distinguishes Anthropic 529/overloaded_error (retryable; drives MAX_529_RETRIES and opus fallback); DefaultLlmClient::prepare_count_tokens provides the authenticated count_tokens path.` In §3 count_tokens bullet append: `(facade in orchestrator::model::count_tokens; approximation = chars/4, min 1)`.

- [ ] **Step 2: Verification**

Run and record: `cargo test -p llm-client -p platform-common -p anthropic-oauth -p orchestrator 2>&1 | awk '/test result: ok/ {p+=$4; s+=1} /FAILED/ {f+=1} END {print p" passed / "s" suites / "f+0" failed"}'`; `cargo check --workspace 2>&1 | tail -2`; clippy over the four touched crates → 0.

- [ ] **Step 3: Commit** (`docs(spec): adoption rev2.2 - orchestrator-local rate-limit, async CredentialProvider, Overloaded class, count_tokens facade`, trailer).

---

## After this plan

Plan 3 (P4+P5): PricingCatalog population from the cost-crate price table (deferred from spec §3 — it lands with CostTracker wiring), agent seam re-type (`SubagentApiClient` → LlmResponse/LlmEvent, convert/accumulator/runner), orchestrator `provider_adapter` rebuilt on `DefaultLlmClient` + `model::*` (note: it currently routes through the `providers` crate's ModelRouter — Plan 3 must absorb/replace that layer too), apps hosts construction, `modelProviders` settings, api-client + oauth_hook deletion, TUI untouched except transitive. Write it against the then-current tree.
