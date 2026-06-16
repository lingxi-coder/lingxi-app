# OpenAI Auth — Codex Parity (design)

**Date:** 2026-06-16
**Status:** P1 approved for implementation; P2/P3 scoped, not yet specced
**Author:** brainstormed with user (luolingfeng)

## Goal

Add first-class OpenAI support to `llm-client`, fully aligned with how the
`codex` project authenticates to OpenAI — including ChatGPT account
(subscription) login, not just API keys. Behaviour on the wire (endpoints,
headers, request bodies, token exchange) should match codex byte-for-byte where
it matters.

## Background — how codex does OpenAI auth

Source of truth: `codex/codex-rs/login/`, `codex/codex-rs/model-provider/`,
`codex/codex-rs/agent-identity/`, `codex/codex-rs/model-provider-info/`.

`CodexAuth` (`login/src/auth/manager.rs`) has **5 auth modes** (`ApiAuthMode`):

| Mode | Credential | Header(s) applied |
|---|---|---|
| `ApiKey` | `OPENAI_API_KEY` | `Authorization: Bearer <key>` |
| `Chatgpt` | ChatGPT account OAuth (PKCE), refreshable | `Authorization: Bearer <access_token>` + `ChatGPT-Account-ID: <account_id>` |
| `ChatgptAuthTokens` | externally-supplied tokens | same as `Chatgpt` |
| `AgentIdentity` | ed25519-signed JWT (enterprise/agent) | self-signed `Authorization` + `ChatGPT-Account-ID` (+ `X-OpenAI-Fedramp` when applicable) |
| `PersonalAccessToken` | static PAT | bearer |

Header injection is a single trait `AuthProvider::add_auth_headers(&mut HeaderMap)`
(`model-provider/src/auth.rs`, `bearer_auth_provider.rs`).

Key constants:
- OAuth issuer: `https://auth.openai.com`; token endpoint `{issuer}/oauth/token`
- Client ID: `app_EMoamEEZ73f0CkXaXp7hrann`
- PKCE authorization-code via localhost callback `http://localhost:<port>/auth/callback`; also a device-code flow (`login/src/device_code_auth.rs`)
- After login, codex does a token-exchange to also mint an API key, persisted to `auth.json` under `CODEX_HOME`
- ChatGPT-login backend: `CHATGPT_CODEX_BASE_URL = https://chatgpt.com/backend-api/codex`
- API-key backend: `https://api.openai.com/v1`
- **Wire API is Responses-only** — codex removed `wire_api = "chat"` (`CHAT_WIRE_API_REMOVED_ERROR`). All OpenAI traffic uses the Responses API (`/responses`).

**Correctness constraint:** a ChatGPT *subscription* login token only works against
the Codex backend (Responses API, `gpt-5-codex`-class models). It is **not**
general api.openai.com access.

## Our side — the target architecture

- `llm-client/src/config.rs` `AuthStrategy` enum already models "bearer + extra
  headers" (`CopilotBearer` = GitHub OAuth token as bearer + Copilot headers).
- `llm-client/src/client.rs:~500` (`authenticate_at`) is the seam mapping
  `AuthStrategy` → an `Authenticator` (`apply(request)`); the equivalent of
  codex's `AuthProvider`.
- `llm-client/src/providers/openai_responses.rs` (855 lines, ported from
  `codex sse/responses.rs`, tested) speaks the Responses API; dispatched in
  `client.rs:666` for `ProtocolFamily::OpenAiResponses`; builds `{base}/responses`.
- `anthropic-oauth/` (3663 lines: `pkce`, `callback`, `refresh`,
  `credential_provider`, `subscription`, `profile`, `handle`) is a near-identical
  structural template for an OAuth login crate.
- Storage: `secret::CredentialManager` + per-profile keychain + env fallback
  (NOT codex's `auth.json`).
- Built-in providers come from `llm-client::builtin_presets()` and flow
  automatically through `provider-config::assemble()` into `/connect`, the
  `/model` picker, availability badges, credentials, and routing.

## Chosen approach — C (hybrid)

A literal copy of codex's auth is not viable: `codex/login` is bound to
`AuthManager` + `auth.json` + `codex_api` + `codex_protocol`, a large transitive
graph that collides with our `CredentialManager`. So:

- **Port** the portable OAuth *flow* (PKCE login, localhost callback, device-code,
  refresh, token-exchange) onto our `anthropic-oauth` architecture, into a new
  `openai-oauth` crate — reusing codex's **exact** endpoints / client_id /
  request bodies so the wire bytes are identical.
- **Vendor** codex's `agent-identity` crate directly (clean, self-contained,
  crypto-heavy — `ed25519-dalek`/`jsonwebtoken`/`crypto_box`; the kind of code to
  copy, not rewrite).
- **Reuse our** `CredentialManager`/`credential_provider` for storage, keeping
  on-wire token shapes identical to codex.

Rejected: (A) re-implement everything (AgentIdentity JWT/crypto too risky to
hand-port); (B) vendor codex's `login`/`model-provider` wholesale (drags in an
incompatible storage/config stack, yields two parallel credential systems).

## Decomposition

Three independent spec → plan → implementation cycles:

| Phase | Scope | Size |
|---|---|---|
| **P1** | OpenAI **API-key** provider preset (this doc) | small |
| **P2** | ChatGPT **account OAuth login**: `openai-oauth` crate (PKCE + device-code + refresh + token-exchange), `CHATGPT_CODEX_BASE_URL`, `AuthStrategy::ChatGptOAuth` + `ChatGPT-Account-ID`, `/connect openai` login UX | large |
| **P3** | Enterprise modes: PersonalAccessToken, ChatgptAuthTokens (external), AgentIdentity (vendored crate), FedRAMP header | medium |

P2 and P3 get their own design docs before implementation.

---

## P1 — OpenAI API-key provider (detailed spec)

### Summary

Add a single built-in preset so OpenAI is usable out-of-the-box with an API key.
Everything else (`/connect`, picker, availability, credentials, routing) flows
automatically through `builtin_presets() → assemble()`, exactly like the `zai`
preset added previously.

### Components

**1. Vendored slice** — `llm-client/data/models-dev/openai.json`
Extracted verbatim from the models.dev `openai` provider object (50 models),
formatted 2-space-indent / sorted-keys to match the other slices.

**2. Preset entry** — `llm-client/src/catalog/presets.rs`

```
profile_name:   "openai"
base_url:       "https://api.openai.com/v1"
protocol:       ProtocolFamily::OpenAiResponses
auth:           AuthStrategy::Bearer
provider_id:    ProviderId::OpenAI
credential_env: "OPENAI_API_KEY"
slice_json:     OPENAI
```

**3. Picker labels** — add `"openai" => "OpenAI"` to:
- `apps/engine-desktop/src/lib.rs` `provider_profile_label()`
- `orchestrator/src/provider_adapter.rs` `provider_label()`

**4. Count-guard updates**
- `presets.rs`: `providers.len()` 5 → 6; add `assert_eq!(count("openai"), 50)`
- `provider-config/src/assemble.rs`: providers len 6 → 7; add
  `assert!(names.contains(&"openai"))`
- `provider-config/src/lib.rs` smoke: `builtin_presets().providers.len()` 5 → 6

### Data flow

`/model` selects an OpenAI model → router resolves the `openai` profile →
`OpenAiResponsesCodec(base_url)` builds `POST https://api.openai.com/v1/responses`
→ `authenticate_at` maps `AuthStrategy::Bearer` → `BearerAuthenticator`, adding
`Authorization: Bearer sk-…` → request sent. `/connect openai` collects the key
into the per-profile keychain (env fallback `OPENAI_API_KEY`).

### Design decisions

1. **Responses API, not Chat Completions** — byte-aligns with codex (which removed
   the chat wire). This intentionally diverges from the existing user-provider
   `type: "openai"` path, which maps to `OpenAiChat`. Both surfaces coexist; the
   first-party built-in preset uses the modern Responses API.
2. **`ProviderId::OpenAI`** (first-party variant), not
   `OpenAICompatible { name: "openai" }`. Semantically correct, matches codex's
   provider id, and is what pricing keys off.
3. **Vendor all 50 models as-is** (incl. embeddings + legacy gpt-3.5/gpt-4) for
   fidelity with the slice and consistency with how openrouter/deepseek were
   vendored. May filter to chat-capable models later if the picker is cluttered.

### Testing (TDD)

- Red the count guards first, then add slice + preset to green them.
- `presets.rs`: `count("openai") == 50`, `providers.len() == 6`.
- `provider-config`: assemble includes an `openai` profile with
  `CredentialConfig::Static { id: "openai" }` and an `OPENAI_API_KEY` env-fallback
  credential source.
- Codec dispatch: assert the `openai` profile resolves to the Responses codec and
  targets `…/responses` (mirror an existing client/route test).

### Out of scope for P1

OAuth/ChatGPT login, device-code, token refresh, account-id header, enterprise
modes, the Codex backend base URL. All deferred to P2/P3.
