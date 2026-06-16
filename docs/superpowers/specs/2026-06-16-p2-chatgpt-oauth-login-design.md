# P2 — ChatGPT account OAuth login (design)

**Date:** 2026-06-16
**Status:** approved for planning
**Parent:** `docs/superpowers/specs/2026-06-16-openai-auth-codex-parity-design.md` (§ "P2")
**Scope decision:** FULL codex login alignment — PKCE browser flow + device-code flow + RFC-8693 API-key minting + token refresh + `ChatGPT-Account-ID` header, routed to the Codex backend.

## Goal

Let a user `/connect` their ChatGPT (OpenAI) account via OAuth and use it through `llm-client`, byte-aligned with how `codex` does ChatGPT login. After login the user can select an `openai-chatgpt` model and the turn loop talks the Responses API to `https://chatgpt.com/backend-api/codex` with a `Bearer` access token (auto-refreshed) plus the `ChatGPT-Account-ID` header.

## Reference behaviour (codex — verified)

Source: `codex/codex-rs/login/`, `model-provider/`, `model-provider-info/`.

**OAuth constants:**
- Issuer: `https://auth.openai.com`
- Client ID: `app_EMoamEEZ73f0CkXaXp7hrann`
- Authorize: `{issuer}/oauth/authorize`
- Token / refresh: `{issuer}/oauth/token`
- Scopes: `openid profile email offline_access api.connectors.read api.connectors.invoke`
- Extra authorize params: `id_token_add_organizations=true`, `codex_cli_simplified_flow=true`, `code_challenge_method=S256`, `originator`, `state`
- Loopback redirect: `http://localhost:1455/auth/callback` (fallback port `1457`), path `/auth/callback`
- Device-code: usercode `{issuer}/api/accounts/deviceauth/usercode`, poll `{issuer}/api/accounts/deviceauth/token`, verification URL `{issuer}/codex/device`
- Backend (ChatGPT auth): `https://chatgpt.com/backend-api/codex` (Responses API)

**PKCE login** (`login/src/server.rs`): bind loopback → build authorize URL with PKCE challenge + state → open browser → capture `?code&state` at `/auth/callback` (validate state) → `exchange_code_for_tokens`.

**Token exchange** (`exchange_code_for_tokens`, `/oauth/token`, form-encoded):
request `{ grant_type=authorization_code, code, redirect_uri, client_id, code_verifier }` → response `{ id_token, access_token, refresh_token }`.

**API-key minting** (RFC-8693, `obtain_api_key`, `/oauth/token`, form-encoded):
request `{ grant_type=urn:ietf:params:oauth:grant-type:token-exchange, client_id, requested_token=openai-api-key, subject_token=<id_token>, subject_token_type=urn:ietf:params:oauth:token-type:id_token }` → response `{ access_token }` (the minted `OPENAI_API_KEY`).

**Token refresh** (`/oauth/token`, JSON):
request `{ client_id, grant_type=refresh_token, refresh_token }` → response `{ id_token?, access_token, refresh_token? }`.
Proactive: refresh if access token expires within 5 min OR `last_refresh` > 8 days old. Reactive: on 401.

**Device-code** (`login/src/device_code_auth.rs`): POST usercode `{ client_id }` → `{ device_auth_id, user_code, interval }`; print verification URL + code; poll token endpoint `{ device_auth_id, user_code }` (403/404 = keep polling, 15-min cap) → `{ authorization_code, code_challenge, code_verifier }`; then converge on `exchange_code_for_tokens`.

**account_id**: parsed from the `id_token` claim `chatgpt_account_id` (full claim `https://api.openai.com/auth.chatgpt_account_id`); fedramp from `chatgpt_account_is_fedramp`.

**Header injection** (`model-provider/src/bearer_auth_provider.rs`): `Authorization: Bearer <access_token>` + `ChatGPT-Account-ID: <account_id>` (+ `X-OpenAI-Fedramp: true` when fedramp).

## Template (our anthropic-oauth — verified)

`anthropic-oauth/` modules: `lib`, `handle` (login driver + `traits::AuthHandle`), `client` (authorize/exchange + `init_refresh_driver`), `config` (endpoints/client_id/scopes), `pkce`, `callback` (loopback listener), `refresh` (`AuthState` single-flight + proactive task, `RefreshDriver`), `credential_provider` (`impl llm_client::CredentialProvider`), `profile`, `resolver`, `subscription`, `limits`, `scope_upgrade`, `testsupport`.

- Login persists 3 keychain entries via `secret::CredentialManager::store_oauth_tokens(access, refresh, expires_at, scopes, email, org_id)`; reads via `get_oauth_tokens()`.
- Credential seam: `OAuthCredentialProvider` (`impl llm_client::CredentialProvider::load`) checks expiry under RwLock, single-flight refresh, returns `Credential::BearerToken`.
- Engine wiring: `apps/engine-desktop/src/lib.rs` build() phase 3 constructs the OAuth client/handle, detects stored tokens, `init_refresh_driver`, builds `OAuthCredentialProvider`, wires it into `MultiCredentialProvider` (single `oauth_delegate` today), registers `/connect`.
- Cargo deps: `protocol, traits, secret, telemetry, llm-client` + `serde, serde_json, thiserror, async-trait, url, base64, rand, sha2, urlencoding, tokio, tracing`.

## Approach C (hybrid) applied to P2

New crate `openai-oauth` mirroring `anthropic-oauth`'s module boundaries and patterns. Reuse codex's exact endpoints/client_id/scopes/request bodies so the wire is byte-identical. Reuse our `secret::CredentialManager` for storage (keychain entries `openai-oauth-*`), NOT codex's `auth.json`. The only genuinely-new piece versus the anthropic template is the `ChatGPT-Account-ID` header, handled by a new `Credential`/`AuthStrategy`/authenticator triad.

## Components

### 1. Crate `openai-oauth`

| Module | Responsibility | Key public items |
|---|---|---|
| `config.rs` | OpenAI OAuth constants + backend URL | `OpenAiOAuthConfig` (issuer, client_id, authorize/token endpoints, device-code endpoints, scopes, `CHATGPT_CODEX_BASE_URL`); fixed loopback ports 1455/1457 |
| `pkce.rs` | RFC-7636 verifier/S256 challenge + state | `generate_pkce()`, `generate_state_token()` |
| `callback.rs` | Loopback listener, **fixed port 1455 (fallback 1457)**, path `/auth/callback` | `CallbackListener`, `CallbackParams`, `CallbackError` |
| `token_data.rs` | Parse `id_token` JWT claims | `IdTokenClaims { account_id, fedramp, plan, email }`, `parse_id_token(jwt) -> IdTokenClaims` |
| `client.rs` | HTTP: authorize URL, code exchange, API-key mint, refresh-driver init | `OpenAiOAuthClient`, `build_authorize_url`, `exchange_code_for_tokens`, `obtain_api_key`, `init_refresh_driver` |
| `device_code.rs` | Device-code flow | `request_device_code()`, `poll_for_token()`, `run_device_code_login()` |
| `refresh.rs` | Token refresh (proactive 5min/8day + reactive), single-flight | `AuthState`, `RefreshDriver`, `spawn_proactive()` |
| `credential_provider.rs` | `impl llm_client::CredentialProvider` | `OpenAiOAuthCredentialProvider::new(Arc<RefreshDriver>)` → returns `Credential::ChatGptOAuth` |
| `handle.rs` | Login orchestration (PKCE browser + device-code), persistence | `OpenAiOAuthHandle` (`login`, `login_device_code`, `logout`, `current_user`) |
| `lib.rs` | Public surface | re-exports above |
| `testsupport.rs` | Mock HTTP, virtual clock, in-mem credential store | mirror anthropic `testsupport` |

Cargo deps mirror `anthropic-oauth/Cargo.toml`.

### 2. New credential seam in `llm-client`

The single new wire detail (the `ChatGPT-Account-ID` header):

- `types.rs` / `credentials.rs`: add `Credential::ChatGptOAuth { access_token: String, account_id: Option<String>, fedramp: bool }`.
- `config.rs`: add `AuthStrategy::ChatGptOAuth`.
- `auth.rs`: add `ChatGptAuthenticator { token, account_id, fedramp }` implementing `Authenticator::apply` — inserts `Authorization: Bearer <token>`, and when `account_id` is set `ChatGPT-Account-ID: <id>`, and when `fedramp` `X-OpenAI-Fedramp: true`. Mirror `CopilotAuthenticator`.
- `client.rs` `authenticate_at` (~line 500): for `AuthStrategy::ChatGptOAuth`, load the credential, match `Credential::ChatGptOAuth { .. }`, build a `ChatGptAuthenticator`, apply.

The account_id is re-derived from each refreshed `id_token` inside `refresh.rs`, so it tracks the live token.

### 3. Provider preset `openai-chatgpt`

In `llm-client/src/catalog/presets.rs`:
- `profile_name: "openai-chatgpt"`
- `base_url: "https://chatgpt.com/backend-api/codex"`
- `protocol: ProtocolFamily::OpenAiResponses`
- `auth: AuthStrategy::ChatGptOAuth`
- `provider_id: ProviderId::OpenAICompatible { name: "openai-chatgpt" }`
- `credential_env`: **none** — `openai-chatgpt` is OAuth, not env-keyed. This requires a small change to the preset machinery: make `Preset.credential_env` an `Option<&'static str>` (the 6 existing presets become `Some("…")`); when `None`, `builtin_presets()` sets `CredentialConfig::Static { id: profile_name }` (not `Env`), and `assemble()` records the credential source with `env_var: None`. The live token then resolves through the `OpenAiOAuthCredentialProvider` registered in `MultiCredentialProvider` under credential_id `openai-chatgpt` — exactly mirroring how the `anthropic` profile's OAuth resolves through its delegate rather than an env var (the OAuth delegate intercepts by credential_id before any static/env fallback).
- `slice_json`: a **hand-authored** `llm-client/data/models-dev/openai-chatgpt.json` (the lone non-models.dev slice). Models: `gpt-5.3-codex`, `gpt-5-codex`, `gpt-5.2`. Minimal models.dev-shaped objects (`id`, `name`, `tool_call`, `reasoning`, `modalities`, optional `limit`); a top-of-file comment explains why this slice is hand-authored (codex's Codex-backend model list is dynamic/`filter_by_auth`, not in models.dev).

Picker labels: `"openai-chatgpt" => "OpenAI (ChatGPT login)"` in both label maps.

### 4. Engine wiring & `/connect`

- `apps/engine-desktop/src/lib.rs` build(): construct `OpenAiOAuthClient`/`OpenAiOAuthHandle`; detect stored OpenAI tokens (`CredentialManager` keychain `openai-oauth-*`); `init_refresh_driver`; build `OpenAiOAuthCredentialProvider`.
- **`MultiCredentialProvider` refactor:** today it holds a single `oauth_delegate`. Generalize to a map/list of OAuth delegates keyed by `credential_id`, so `anthropic` (existing) and `openai-chatgpt` (new) both resolve. `load(scope)` dispatches to the delegate whose credential_id matches the scope's profile/credential_id; falls through to the existing static/env behaviour otherwise.
- `/connect`: add a `chatgpt` (alias `openai-chatgpt`) branch in `command-core` `ConnectHandler`, paralleling the existing `github-copilot` branch, driven by a new engine-supplied seam (e.g. `ChatGptConnectDriver`) wrapping `OpenAiOAuthHandle::login` (browser PKCE) with a device-code fallback when no browser is available. Update the `/connect` usage hint to list `chatgpt`.

## Data flow

`/connect chatgpt` → `OpenAiOAuthHandle::login` binds loopback 1455 → opens browser to authorize URL → captures code → `exchange_code_for_tokens` → parse `id_token` → `obtain_api_key` (mint, persisted alongside) → `store_oauth_tokens` (keychain `openai-oauth-*`, with account_id in meta). Boot: detect tokens → `init_refresh_driver` (proactive task). Per request to `openai-chatgpt`: router resolves the profile → `OpenAiOAuthCredentialProvider::load` returns `Credential::ChatGptOAuth{token,account_id,fedramp}` (refreshing if near expiry) → `ChatGptAuthenticator` sets `Authorization: Bearer` + `ChatGPT-Account-ID` → `OpenAiResponsesCodec` POSTs `https://chatgpt.com/backend-api/codex/responses`.

## Error handling

Mirror anthropic-oauth: login deadline (60s) → timeout error; callback state mismatch → rejected; code-exchange / refresh / device-poll failures → typed errors with secrets redacted from logs/URLs. Device-code: 403/404 keep polling to the 15-min cap, then timeout. Refresh single-flight prevents stampedes; reactive refresh on a 401 from the backend.

## Testing (TDD, no live network)

Per-module unit tests using the `testsupport` pattern (mock HTTP, virtual clock, in-mem credential store):
- `pkce`: challenge is S256(verifier); state is random/url-safe.
- `client`: authorize URL contains exact params/scopes/redirect; `exchange_code_for_tokens` posts the exact form body and parses the 3 tokens; `obtain_api_key` posts the RFC-8693 body and returns the minted key; refresh posts `grant_type=refresh_token`.
- `token_data`: claim extraction (account_id, fedramp) from a fixture JWT.
- `device_code`: usercode parse, poll loop on 403→success.
- `refresh`: proactive triggers on 5-min/8-day thresholds; single-flight under concurrency; account_id updates from a refreshed id_token.
- `credential_provider`: returns `ChatGptOAuth` with token+account_id; refreshes on expiry.
- `llm-client`: `ChatGptAuthenticator` sets exactly `Authorization` + `ChatGPT-Account-ID` (+ fedramp); `authenticate_at` dispatches `AuthStrategy::ChatGptOAuth`; the `openai-chatgpt` preset shape (base_url, protocol, auth) — count-guard + decision-locking asserts.
- `provider-config`: `openai-chatgpt` appears in the assembled config; count-guard bumps.
- `engine`: `MultiCredentialProvider` routes the right OAuth delegate per credential_id (anthropic vs openai-chatgpt) — table test.

## Out of scope (P3)

PersonalAccessToken mode, ChatgptAuthTokens (external-supplied tokens), AgentIdentity (vendored codex `agent-identity` crate, JWT signing). The `X-OpenAI-Fedramp` header is wired here (trivial) but full FedRAMP routing is P3.

## File structure (new + modified)

- Create: `lingxi-code/openai-oauth/` (Cargo.toml + the modules above)
- Create: `lingxi-code/llm-client/data/models-dev/openai-chatgpt.json` (hand-authored, 3 models)
- Modify: `lingxi-code/llm-client/src/{types,credentials,config,auth,client}.rs` (the credential seam)
- Modify: `lingxi-code/llm-client/src/catalog/presets.rs` (the preset + guards)
- Modify: `lingxi-code/provider-config/src/{assemble,lib}.rs` (count guards)
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs` (wiring + MultiCredentialProvider refactor) and `apps/engine-desktop/src/connect.rs` (the ChatGptConnectDriver seam)
- Modify: `lingxi-code/commands/core/src/connect.rs` (the `chatgpt` branch + usage hint)
- Modify: `lingxi-code/orchestrator/src/provider_adapter.rs` (picker label)
- Modify: `lingxi-code/Cargo.toml` workspace members (add `openai-oauth`)
