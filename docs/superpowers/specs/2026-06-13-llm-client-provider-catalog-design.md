# llm-client provider catalog + grouped model picker — design

> **Status:** approved design (2026-06-13). Three phases, each becomes its own
> implementation plan executed/reviewed independently.
> **Branch base:** `parity-llm-client-3a` (the in-flight llm-client adoption).
> **Reference repos** (read-only, at repo root, pulled to latest this session):
> `opencode/` (provider/auth patterns, models.dev consumption, Copilot device
> flow) and `liter-llm/`.

## Goal

Add the remaining first-class providers — **OpenRouter, DeepSeek, GLM (Zhipu
coding plan), and GitHub Copilot** — to the new `llm-client` crate, sourced from
a **vendored models.dev snapshot** for model metadata plus a hand-authored
routing table for wire/auth concerns. Then surface the catalog in the TUI
`/model` picker with a **Recent** section, **provider grouping**, and **search**
(mirroring opencode's model picker).

This pulls part of Plan 3c (multi-provider) forward, but targets `llm-client` —
the crate that survives Plan 3b's deletion of the legacy `providers` crate — so
the work is not thrown away.

### Non-goals (deferred)

- Runtime `/models` discovery / live catalog refresh (snapshot is static; a
  refresh **script** regenerates it out-of-band). Dynamic fetch → later / 3c.
- The full `modelProviders` **settings** schema + arbitrary user-defined
  providers (Plan 3c).
- Picker affordances **Favorite** (`ctrl+f`), **Connect provider** (`ctrl+a`),
  and **Free** badges — optional, deferred unless trivially free. Recent +
  grouping + search are the must-haves.
- The legacy `providers` crate is **not** touched (it is bypassed by 3a, deleted
  by 3b).

## Core architectural insight: catalog metadata vs. routing

The vendored models.dev snapshot owns **catalog metadata** (model ids, display
names, cost, context/output limits, capability flags). A hand-authored Rust
**routing table** owns **wire + auth concerns** (base URL, `ProtocolFamily`,
`AuthStrategy`, provider-specific headers).

This split is *forced* by GLM: models.dev's `zhipuai-coding-plan` entry points
`api` at `https://open.bigmodel.cn/api/coding/paas/v4` (an OpenAI-style paas
path), but we want the **Anthropic-compatible** coding endpoint
(`https://open.bigmodel.cn/api/anthropic`) so we can reuse the existing
`AnthropicMessagesCodec`. The snapshot's `api`/`env` fields are therefore
**advisory** — the routing table overrides them. Benefit: the snapshot can be
refreshed mechanically and will never clobber a routing decision.

## Provider facts (pinned from `models.dev/api.json`, 2026-06-13)

| Provider | models.dev id | base URL (routing) | protocol | auth | env / cred |
|---|---|---|---|---|---|
| OpenRouter | `openrouter` | `https://openrouter.ai/api/v1` | `OpenAiChat` | ApiKey (Bearer) | `OPENROUTER_API_KEY` |
| DeepSeek | `deepseek` | `https://api.deepseek.com` | `OpenAiChat` | ApiKey (Bearer) | `DEEPSEEK_API_KEY` |
| GLM (coding) | `zhipuai-coding-plan` | `https://open.bigmodel.cn/api/anthropic` **(override)** | `AnthropicMessages` | ApiKey (`x-api-key`) | `ZHIPU_API_KEY` |
| GitHub Copilot | `github-copilot` | `https://api.githubcopilot.com` | `OpenAiChat` | `CopilotBearer` (direct GitHub token — see Phase 2 correction) | `GITHUB_TOKEN` |

- OpenRouter ships 337 models (~176 KB slice); DeepSeek 4; GLM-coding 6;
  Copilot 23 (~13 KB). Total vendored ≈ 200 KB (openrouter dominates). The full
  `api.json` is 2.3 MB — **only the four slices are vendored**, never the whole
  file.
- OpenRouter wants two optional ranking headers: `HTTP-Referer` and `X-Title`.
- Copilot's models include `claude-opus-4.7`, `claude-sonnet-4.5`, `gpt-5.4-nano`,
  `gemini-2.5-pro`, etc. — hence the screenshot's "Claude Opus 4.8 · GitHub
  Copilot" rows (model × provider pairs).

## Field mapping: models.dev `Model` → llm-client

models.dev `Model` schema (see `opencode/packages/core/src/models-dev.ts`):
`{ id, name, family?, release_date, attachment, reasoning, temperature,
tool_call, interleaved?, cost?, limit{context,input?,output}, modalities?,
structured_output?, status?, … }`.

| models.dev field | llm-client target | notes |
|---|---|---|
| `id` | `ModelProfile.request_model` + `billing_model` | wire id + pricing key |
| `name` | `ModelProfile.display_model` | picker display label |
| `tool_call` | `Capabilities.tools` | |
| `reasoning` | `Capabilities.reasoning` | |
| `structured_output` | `Capabilities.structured_output` | absent → false |
| `modalities.input ⊇ image` | `Capabilities.vision` | |
| `modalities.input ⊇ pdf` | `Capabilities.documents` | |
| (always) | `Capabilities.streaming = true` | all four stream |
| `cost.{input,output,cache_read,cache_write}` | pricing catalog `TokenPricing` | USD per 1M tokens; absent / `0` → free (still priced, cost 0) |
| `limit.context` | context-window for preflight/overflow | |
| `status` (`alpha`/`beta`/`deprecated`) | carried as metadata; deprecated may be filtered | |

---

## Phase 1 — Provider catalog (pure data, offline-testable)

**Deliverable:** `llm_client::builtin_presets()` returns a catalog of
`ProviderProfile` + `ModelProfile` + pricing for OpenRouter, DeepSeek, and
GLM-coding, mergeable into `ClientConfig`. No new wire codecs.

### Components

1. **Vendored snapshot** — `llm-client/data/models-dev/{openrouter,deepseek,
   zhipuai-coding-plan}.json` (Copilot's slice lands in Phase 2). Each is the
   per-provider object lifted verbatim from `api.json`. A committed
   `llm-client/scripts/refresh-models-dev.sh` re-fetches `api.json` and rewrites
   the slices (idempotent; documents provenance + date). Files are real
   committed JSON, embedded via `include_str!`.

2. **models.dev serde schema** (`src/catalog/models_dev.rs`) — `#[derive(Deserialize)]`
   structs for `Provider` + `Model` + `Cost` + `Limit` + `Modalities`,
   tolerant of unknown fields (`#[serde(default)]`, no `deny_unknown_fields`) so
   future api.json additions don't break parsing.

3. **Routing-override table** (`src/catalog/presets.rs`) — a hand-authored
   `const`/function table keyed by provider giving: `base_url`,
   `ProtocolFamily`, `AuthStrategy`, the credential env var, and any
   provider-specific headers (OpenRouter `HTTP-Referer` / `X-Title`). This is
   the source of truth for wire/auth; it **overrides** the snapshot's
   `api`/`env`.

4. **Catalog builder** (`builtin_presets()`) — lazily parses each embedded slice,
   maps `Model` → `ModelProfile` + pricing per the table above, attaches the
   routing override, and returns the catalog. Codec selection by
   `ProtocolFamily`: `OpenAiChat` → existing `OpenAiChatCodec` (OpenRouter,
   DeepSeek); `AnthropicMessages` → existing `AnthropicMessagesCodec` (GLM).

### Provider-specific header injection (deferred — not in Phase 1)

OpenRouter's `HTTP-Referer` / `X-Title` are *optional* ranking headers (they only
affect leaderboard attribution; routes work without them). Injecting them needs a
per-route static-header seam that `ProviderProfile` does not have today, so it is
**deferred** out of Phase 1 — see the implementation plan's "Out of scope"
section. No secret material would ever go in these headers.

### Tests (Phase 1)

- Snapshot parses; each provider yields the expected model count
  (openrouter 337, deepseek 4, glm-coding 6) — guards against a truncated
  re-vendor.
- Field mapping: a representative model (e.g. `deepseek-chat`, `glm-4.7`) maps to
  the right `display_model`/`request_model`, capabilities, and pricing.
- Routing override beats snapshot `api`: GLM resolves to `…/api/anthropic` +
  `AnthropicMessages`, **not** the coding/paas path.
- `builtin_presets()` merges into `ClientConfig` and `ModelRegistry::resolve`
  finds a model from each provider.
- Free model (`cost.input == 0`) is priced at cost 0, not "unpriced".

---

## Phase 2 — Copilot auth subsystem

**Deliverable:** GitHub Copilot as a working provider: the full interactive
device-flow login (mechanism, behind a mockable HTTP seam) + a synchronous
request authenticator that injects the Copilot header set.

> **Correction vs. the original draft (verified against opencode's actual
> `copilot.ts`):** opencode does **NOT** exchange the GitHub token for a
> short-lived Copilot token at `copilot_internal/v2/token`. Its OAuth-App token
> (`CLIENT_ID = "Ov23li8tweQw6odWQebz"`) is used **directly** as
> `Authorization: Bearer <github-token>` against `api.githubcopilot.com`
> (`expires: 0`). So there is **no token exchange and no caching** — request-time
> auth is pure synchronous header injection (fits the existing `Authenticator`
> trait). The only network is the one-time device-flow login.

### Auth flow (ported from `opencode/packages/opencode/src/plugin/github-copilot/copilot.ts`)

1. **Device-flow login (interactive, one-time).** llm-client supplies the
   *mechanism*; the host drives the loop (sleep + UI).
   - `CopilotLogin::begin()` → `POST https://github.com/login/device/code`
     (JSON `{client_id, scope: "read:user"}`, `Accept: application/json`) →
     returns `{ user_code, verification_uri, device_code, interval }`. The host
     shows `user_code` + `verification_uri`.
   - `CopilotLogin::poll_once()` → `POST https://github.com/login/oauth/access_token`
     (JSON `{client_id, device_code, grant_type: "urn:ietf:params:oauth:grant-type:device_code"}`)
     → returns a classified `PollOutcome`: `Success(github_token)` /
     `Pending { interval }` / `SlowDown { interval }` (RFC 8628 §3.5:
     `(interval+5)`, or server-provided `interval`) / `Failed`. The **pure
     classification + backoff math** lives here; the host owns the sleep+retry
     loop so the state machine is unit-testable without timers.
   - The GitHub OAuth token is persisted by the host via the existing
     `CredentialConfig` (e.g. `HostManaged` / `Env`) — never written to disk by
     llm-client.
   - Enterprise (`copilot-api.<domain>`) is **deferred**; v1 targets
     `github.com` / `api.githubcopilot.com`.

2. **Request authentication (synchronous, no exchange).** A
   `CopilotAuthenticator` (implements the existing `Authenticator` trait) holds
   the GitHub OAuth token (loaded via `CredentialConfig` like every other
   provider) and injects, per request:
   - `Authorization: Bearer <github-token>`
   - `User-Agent: <COPILOT_USER_AGENT>`
   - `Openai-Intent: conversation-edits`
   - `X-GitHub-Api-Version: 2026-06-01`
   - `x-initiator: agent` (LingXi is an agentic client; a static default —
     refining per-request user/agent is deferred)
   - strips any inbound `x-api-key` (defensive parity with opencode).
   - `Copilot-Vision-Request: true` is **deferred** (only needed for image
     requests; inspecting encoded provider JSON for images is out of v1 scope).

### Wiring

- New `AuthStrategy::CopilotBearer` variant (additive to llm-client's own
  `config.rs`). In `client.rs::authenticate`, this variant loads the secret
  (the GitHub token) via the existing `load_secret` path and constructs
  `CopilotAuthenticator::new(secret)`.
- No change to `build_codec`: Copilot uses the existing `OpenAiChatCodec`
  (`ProtocolFamily::OpenAiChat`).

### Mockable HTTP seam

The two device-flow network calls (device-code, access-token poll) go through a
small injected async `CopilotHttp` trait (the crate has **no** HTTP-client dep —
transport is always injected, same as `Transport`). Unit tests drive
`begin` + `poll_once` (pending → slow_down → success → failed, and the backoff
math) with a mock implementation; the real impl is host-provided. No live GitHub
calls in CI.

### Secret hygiene

The GitHub OAuth token is held in a redacting-`Debug` wrapper, never logged,
never placed in error messages. (Same discipline as the Android `AndroidGitSecret`
precedent.)

### Phase 2 vendoring

Add `llm-client/data/models-dev/github-copilot.json` (extend
`refresh-models-dev.sh`'s provider list) + a `github-copilot` entry in the
routing table: `OpenAiChat`, base `https://api.githubcopilot.com`, auth =
`CopilotBearer`, credential `Env { var: "GITHUB_TOKEN" }`.

### Tests (Phase 2)

- Device-flow (against the mock seam): `begin` parses the device-code response;
  `poll_once` classifies `Success`/`Pending`/`SlowDown`/`Failed`; `slow_down`
  backoff = `(interval+5)` or server-provided override.
- Header injection: the exact Copilot header set is present with the right
  values; inbound `x-api-key` is stripped; the token never appears in
  `Debug`/error output.
- `CopilotAuthenticator` is constructed for `AuthStrategy::CopilotBearer` in the
  client auth path.
- Copilot models parse and resolve via `ModelRegistry`; the github-copilot preset
  routes to `https://api.githubcopilot.com` + `OpenAiChat`.

---

## Phase 3 — Grouped model picker (TUI)

**Deliverable:** the `/model` picker (`tui/src/screens/model.rs`) shows a
**Recent** group, **provider-grouped** sections, and a **search** filter — the
layout in the reference screenshot — replacing today's flat `Vec<String>` list.

### Data: richer catalog accessor

Today `OrchestratorHandle::list_available_models() -> Vec<String>` returns flat
wire ids. Phase 3 adds a richer accessor returning grouped entries:
`{ model_id, display_name, provider_id, provider_label, tier?/free? }`, sourced
from the Phase 1/2 catalog. The flat accessor stays for back-compat; the picker
uses the rich one.

### Recent persistence

A new persisted store of the last-N selected **`(provider_id, model_id)` pairs**
(the screenshot shows the same model under different providers, so recents key on
the pair, not the bare id). Most-recent-first, capped (e.g. N=5). Persisted
across sessions (settings/state file alongside existing TUI state). Updated on
every committed model switch.

### Picker reducer + renderer (`screens/model.rs`)

`ModelScreenState` grows from `Vec<String>` to a structured model:
- ordered **groups**: `Recent` first (from the store, deduped against the live
  catalog), then one group per provider (catalog order).
- a **search** query string; typing filters rows by display name / provider /
  id; group headers with no surviving rows are hidden.
- navigation (`↑/↓/j/k`) moves over **selectable rows only** (skips group
  headers); `Enter` commits the highlighted row's `(provider, model)`; `Esc/q`
  cancels. Commit still raises the async `pending_switch_model` path unchanged,
  and additionally records the recent pair.
- renderer reproduces the screenshot: bold "Select model", search line, purple
  group headers, gray provider suffix per row, highlighted row, optional `Free`
  right-aligned badge.

### Optional (deferred unless trivial)

Favorite (`ctrl+f`), Connect-provider (`ctrl+a`) footer actions, and `Free`
badges. Recent + grouping + search are required; these are not.

### Tests (Phase 3)

- Recent store: round-trips, caps at N, most-recent-first, dedups on re-select,
  keys on `(provider, model)`.
- Reducer: search filters rows + hides empty headers; nav skips headers; Enter
  commits the right pair and records the recent; Esc cancels with no change.
- Renderer (string-snapshot, matching the existing `render_model_to_string`
  test style): Recent group + ≥2 provider groups + a highlighted row render in
  the expected layout; empty-catalog → "No models available."

---

## Cross-cutting constraints

- **Frozen crates:** `traits/` + `protocol/` are additive-only — verify
  `git diff main -- lingxi-code/traits lingxi-code/protocol` shows zero removed/
  modified lines at the end of each phase.
- **No `git add -A`** — untracked `codex/`, `liter-llm/`, `opencode/`,
  `.codegraph/` live at repo root; stage only named `lingxi-code/...` +
  `docs/...` + `llm-client/data/...` paths.
- **Standing work untouched:** the user's `parity-llm-client-3a` checkout has
  uncommitted work — do every phase in an isolated worktree; never disturb the
  primary tree. Run edit-agents **sequentially** (concurrent-agent worktree
  hazard).
- **Lints:** `-D missing-docs` + clippy pedantic `-D warnings` per touched crate.
- **engine-mobile must keep building.**
- **TDD:** observed RED before implementing each unit.
- **Commit trailer** exactly as the branch convention requires.
- **No secret material** in logs/errors (Copilot tokens especially).

## Open questions / decisions locked

- **Locked:** home = `llm-client`; all four providers incl. Copilot; Copilot =
  full device-flow + exchange + headers; GLM = Anthropic-compat only; model
  lists = vendored models.dev snapshot, static.
- **Deferred:** dynamic model discovery; Favorite/Connect/Free picker
  affordances; `modelProviders` settings (3c).
