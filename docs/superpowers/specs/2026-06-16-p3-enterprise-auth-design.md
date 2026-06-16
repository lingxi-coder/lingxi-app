# P3 — Enterprise auth modes: PAT + external tokens (design)

**Date:** 2026-06-16
**Status:** approved for planning
**Parent:** `docs/superpowers/specs/2026-06-16-openai-auth-codex-parity-design.md` (§ "P3")
**Scope decision:** PAT (Personal Access Token) + ChatgptAuthTokens (externally-supplied tokens). **AgentIdentity is explicitly OUT** — it is the only heavy mode (vendored crypto crate, per-request Ed25519 `AgentAssertion` signing, JWKS fetch + task registration) and is specific to OpenAI's agent-runtime platform, which lingxi (a general client) does not use.

## Goal

Add two enterprise auth modes for the OpenAI Codex backend, byte-aligned with codex, reusing the P2 ChatGPT plumbing:
- **PAT**: a long-lived `at-*` personal access token; on load, a `whoami` call resolves the account_id/fedramp.
- **External tokens**: an externally-supplied access token + account_id (e.g. minted by an enterprise auth server), no local refresh.

## Reference behaviour (codex — verified)

Source: `codex/codex-rs/login/src/auth/{personal_access_token.rs,manager.rs}`, `model-provider/src/bearer_auth_provider.rs`.

- **PAT** (`personal_access_token.rs`): token classified by `at-` prefix (`access_token.rs`). On load, `hydrate_personal_access_token` does `GET {authapi}/v1/user-auth-credential/whoami` with `Authorization: Bearer <pat>` → `{ email, chatgpt_user_id, chatgpt_account_id, chatgpt_plan_type, chatgpt_account_is_fedramp }`. authapi base default `https://auth.openai.com/api/accounts` (env `CODEX_AUTHAPI_BASE_URL`). Static — never refreshes (`manager.rs` refresh skips PAT). Request headers via `BearerAuthProvider`: `Authorization: Bearer <pat>` + `ChatGPT-Account-ID: <account_id>` + `X-OpenAI-Fedramp: true` (if fedramp). Backend = standard ChatGPT backend.
- **ChatgptAuthTokens** (`manager.rs` `login_with_chatgpt_auth_tokens`): caller supplies `access_token` (an OpenAI JWT) + `chatgpt_account_id` (+ optional `chatgpt_plan_type`). Stored ephemerally (in-memory, never to disk). No local refresh (external system owns lifecycle). Same header set as the PKCE Chatgpt mode. fedramp comes from the access_token's id_token claims.
- Both modes set the identical header triple as our P2 `ChatGptAuthenticator` already produces.

## Our P2 plumbing being reused

- `llm_client::Credential::ChatGptOAuth { access_token, account_id: Option<String>, fedramp: bool }`
- `llm_client::AuthStrategy::ChatGptOAuth` + `ChatGptAuthenticator` (Bearer + `ChatGPT-Account-ID` + `X-OpenAI-Fedramp`), dispatched in `client.rs::authenticate_at`.
- The `openai-chatgpt` provider preset (Codex backend `https://chatgpt.com/backend-api/codex`, Responses API).
- `provider-config::MultiCredentialProvider` routes `credential_id == "openai-chatgpt"` to whatever delegate the engine registered under that key.
- `openai-oauth::token_data::parse_id_token(jwt) -> IdTokenClaims{account_id, fedramp, email}`.

**Key simplification:** PAT and external-tokens both resolve to `Credential::ChatGptOAuth` and target the same `openai-chatgpt` profile. So P3 adds NO new `AuthStrategy`, `Credential` variant, or authenticator — only two new `CredentialProvider`s plus engine selection logic. They are *alternative* ways to populate the single `openai-chatgpt` delegate (mutually exclusive, like anthropic api-key vs oauth).

## Components (all in the `openai-oauth` crate)

### 1. `config.rs` addition
Add `authapi_base_url: String` to `OpenAiOAuthConfig` (default `https://auth.openai.com/api/accounts`), and a helper `whoami_url()` → `{authapi_base_url}/v1/user-auth-credential/whoami`.

### 2. `pat.rs` (new)
- `PatMetadata { account_id: Option<String>, fedramp: bool, email: Option<String>, plan: Option<String> }`.
- `async fn whoami(cfg: &OpenAiOAuthConfig, http: &Arc<dyn HttpTransport>, pat: &str) -> Result<PatMetadata, OAuthError>` — `GET cfg.whoami_url()` with `Authorization: Bearer <pat>`; parse the codex whoami response (`chatgpt_account_id` → account_id, `chatgpt_account_is_fedramp` → fedramp, `email`, `chatgpt_plan_type`). Non-200 → `OAuthError`.
- `struct PatCredentialProvider { pat: String, account_id: Option<String>, fedramp: bool }` impl `llm_client::CredentialProvider`: `load` returns `Credential::ChatGptOAuth { access_token: pat.clone(), account_id, fedramp }`. **Static** — no refresh, no per-request network. Constructed by the engine AFTER a one-time `whoami` (metadata cached). Token redacted in Debug.

### 3. `external_tokens.rs` (new)
- `struct ExternalTokensCredentialProvider { access_token: String, account_id: Option<String>, fedramp: bool }` impl `CredentialProvider`: `load` returns `Credential::ChatGptOAuth{...}`. Static, no network. Debug redacts the token.
- A constructor `from_supplied(access_token, account_id: Option<String>)` that, when fedramp isn't otherwise known, parses it from `token_data::parse_id_token(&access_token)` (best-effort; defaults false).

### 4. `lib.rs`
Export `PatCredentialProvider`, `ExternalTokensCredentialProvider`, `whoami`, `PatMetadata`.

## Engine wiring (`apps/engine-desktop/src/lib.rs`)

In build(), replace the single OAuth-only `openai-chatgpt` delegate construction with a **precedence selector** (most-explicit first):

1. **PAT** — if env `OPENAI_PERSONAL_ACCESS_TOKEN` is set: `whoami` it once (warn + fall through on failure), build `PatCredentialProvider`.
2. **External tokens** — else if env `OPENAI_CHATGPT_ACCESS_TOKEN` AND `OPENAI_CHATGPT_ACCOUNT_ID` are set: build `ExternalTokensCredentialProvider`.
3. **OAuth login** — else if a stored OAuth session exists (`get_openai_oauth_tokens()` → `init_refresh_driver` → `OpenAiOAuthCredentialProvider`, the P2 path).
4. **none** — no `openai-chatgpt` delegate.

Insert the winner into `oauth_delegates` under `"openai-chatgpt"`. Factor the selection into a small async helper returning `Option<Arc<dyn CredentialProvider>>` + a bool `available` so it is unit-testable and keeps build() readable.

## Picker availability

Generalize the P2 `openai_chatgpt_has_oauth` bool passed to `compute_availability` into `openai_chatgpt_available` = (PAT env set) OR (external env pair set) OR (OAuth session present). Same `"openai-chatgpt"` arm in `availability.rs`; only the source of the bool changes (engine ORs the three). Rename the param to `openai_chatgpt_available` for clarity.

## Data flow

Boot: engine selector picks a source by precedence → registers a `CredentialProvider` under `"openai-chatgpt"`. Per request to an `openai-chatgpt` model: router → `MultiCredentialProvider` → the selected delegate → `Credential::ChatGptOAuth{access_token, account_id, fedramp}` → `authenticate_at` (AuthStrategy::ChatGptOAuth) → `ChatGptAuthenticator` sets the header triple → `OpenAiResponsesCodec` POSTs the Codex backend. Identical to P2 from `authenticate_at` onward; only the credential SOURCE differs.

## Error handling

- PAT `whoami` failure (network/non-200) → `tracing::warn!` and fall through to the next precedence source (don't abort boot).
- External-tokens: if only one of the two env vars is set → ignore (treat as not configured) + `warn!` once that the pair is incomplete.
- Missing account_id → `ChatGPT-Account-ID` header omitted (request still attempted), matching `ChatGptAuthenticator`.
- Tokens never logged; both providers' Debug redacts the secret.

## Testing (TDD, no live network)

- `pat.rs`: `whoami` parses the codex response shape (mock HTTP) → account_id/fedramp; non-200 → error. `PatCredentialProvider::load` returns `ChatGptOAuth` with the cached metadata; Debug redacts the PAT.
- `external_tokens.rs`: `load` returns `ChatGptOAuth` with supplied account_id; `from_supplied` parses fedramp from a JWT when present; Debug redacts.
- engine selector helper: table test over (PAT set / external set / oauth present / none) → asserts the expected source wins and `available` is correct. (If the selector touches the keychain/whoami, test the pure precedence logic on injected inputs.)
- availability: `openai-chatgpt` available when the new OR-flag is true (extend the P2 test).

## Out of scope

- **AgentIdentity** (deferred indefinitely; documented above).
- Interactive `/connect` flow for PAT (env-var config only for P3; a `/connect chatgpt-pat` could be added later).
- PAT keychain storage (env-var only this phase).

## File structure (new + modified)

- Create: `lingxi-code/openai-oauth/src/pat.rs`, `lingxi-code/openai-oauth/src/external_tokens.rs`
- Modify: `lingxi-code/openai-oauth/src/config.rs` (authapi_base_url + whoami_url), `lingxi-code/openai-oauth/src/lib.rs` (exports)
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs` (precedence selector + availability OR-flag)
- Modify: `lingxi-code/provider-config/src/availability.rs` (rename param to `openai_chatgpt_available`; update callers + tests)
