# T2b — Connect UI Presentation + OAuth Wiring — Design Spec

- **Date:** 2026-06-29
- **Status:** Approved (design); ready for implementation planning
- **Track:** T2b — track 2b (final functional track) of the connect-UI overhaul
- **Owner area:** `tui` crate (`/connect` screens), `commands/core` (connect driver seam), `apps/engine-desktop` + `apps/cli` (driver wiring), `llm-client` OAuth handles (read-only — already built)

## Context

The connect-UI overhaul decomposed into T1 (theme — done), T2a (data foundation —
done), and T2b (this spec). T2a made `/connect` data-driven and made it *honest*
about login methods, surfacing OAuth providers with a "browser sign-in — coming
soon" `Unavailable` screen. T2b makes those screens **real**, adds a
**per-provider detail panel**, and **unifies** the connect screens into one flow.

The user approved the **full** scope (all three pieces) with OAuth for **both**
Anthropic Pro/Max and OpenAI ChatGPT.

### Key enabling discovery

Both OAuth flows are **already fully built** in `llm-client` — T2b is wiring, not
crypto:
- **Anthropic Claude.ai (Pro/Max):** `llm-client/src/oauth/anthropic/handle.rs::OAuthHandle`
  (`new(client)`, `.with_browser_opener(opener)`), backed by `CallbackListener`
  (binds `127.0.0.1:{ephemeral}`, awaits `GET /callback?code=...`), PKCE, token
  exchange, subscription. Already constructed in `engine-desktop` (~:2159) as
  `Arc<dyn AuthHandle>`.
- **OpenAI ChatGPT:** `llm-client/src/oauth/openai/handle.rs::OpenAiOAuthHandle`
  ("browser-PKCE and device-code login"), injectable browser opener, 60s deadline,
  `OpenAiLoginInfo`, `OpenAiLoginError {BrowserOpenFailed, deadline}`.
- **The TUI driver pattern already exists:** the GitHub Copilot device-flow is
  driven by `commands/core/src/connect.rs::CopilotConnectDriver`
  (`begin → poll_to_completion`), threaded `DesktopRuntime.connect_copilot →
  AppState.copilot_connect_driver`, fired by a `pending_copilot_login` ticker.
  T2b mirrors this exactly for OAuth.

## Goals

1. Selecting an OAuth provider in `/connect` performs a **real browser sign-in**
   (Anthropic Pro/Max + ChatGPT), replacing T2a's `Unavailable` "coming soon"
   screen. On success the credential is stored and the provider flips to `✓`.
2. A provider supporting **multiple** auth methods (Anthropic: OAuth *and* API
   key) offers a **method-choice** step; single-method providers go straight to
   their flow.
3. A **per-provider detail panel** shows real catalog data (models), connected
   state, and the available login method(s) for the highlighted provider.
4. The connect screens (`connect_picker` + `connect` + `github_deploy`) become
   **one unified flow** (list + detail/action in one view), not screen-hops.
5. No regression to T2a (data-driven rows, trustworthy `✓`), the `/model` picker,
   or T1 theming.

## Non-goals

- **Building new OAuth crypto/flows** — both handles exist; T2b wires them.
- **Folding `/model` into the connect screen.** Decision: `/model` stays its own
  picker (selecting a model ≠ connecting a provider — distinct concerns; YAGNI),
  but is made **visually consistent** via the shared `picker_popup` component.
- The Warp / alt-screen rendering rework (**T3**).
- Mid-session credential refresh / token-rotation UI (the handles refresh
  transparently; no new UI).

## Decisions made

- **Mirror `CopilotConnectDriver`.** Add an `OAuthConnectDriver` seam in
  `commands/core` wrapping the two built handles; thread it `DesktopRuntime →
  AppState`; fire via a `pending_oauth_login` ticker. Same begin→await→store
  shape, so the async/credential plumbing matches the proven Copilot path.
- **Loopback callback** (not paste-code) — that is what the built handles do
  (matches claude-code). Implies a local browser; see Known limitations.
- **Method-choice only when needed.** The connect screen derives a provider's
  method *set* from the catalog: Anthropic = {OAuth, ApiKey} → choice; ChatGPT =
  {OAuth} → straight to OAuth; api-key providers = {ApiKey} → straight to field;
  Copilot = {Device} → existing deployment→device path.
- **Unify by composition, not rewrite.** Keep the existing pure reducers
  (`connect_picker`, `connect`, `github_deploy`) as the state/logic; the
  unification is a new *render+navigation* layer that shows the picker and the
  selected provider's action pane together. The byte-locked reducers are reused,
  not rewritten.

## Design

### Piece 1 — OAuth wiring

**Driver seam (`commands/core/src/connect.rs`, beside `CopilotConnectDriver`):**

```
pub trait OAuthConnectDriver: Send + Sync {
    /// Kick off the browser-PKCE / loopback flow for `provider` (e.g.
    /// "anthropic", "openai-chatgpt"): build the authorize URL, open the
    /// browser, run the loopback callback, exchange + store the credential.
    /// Returns when the flow is terminal (success = credential stored).
    async fn login(&self, provider: &str) -> Result<(), ConnectError>;
}
```

`login()` is a single await that opens the browser, runs the loopback callback,
and exchanges + stores the credential internally. The TUI does NOT need mid-flight
progress callbacks: the ticker sets `AwaitingSignIn` immediately *before* calling
`login()`, then sets `Done`/`Failed` from its result. (This is simpler than the
Copilot device-code path, which must surface a code mid-flow.)

The engine impl (`engine-desktop`) wraps `OAuthHandle` (anthropic) /
`OpenAiOAuthHandle` (openai), selecting by `provider`. Injectable browser opener
(real = shell `open`; tests = no-op). Threaded `DesktopRuntime.connect_oauth →
cli::init::Runtime → mode.rs → session::Runtime → AppState.oauth_connect_driver`
(the full chain — the T2a lesson: do NOT stop at session.rs).

**Connect-screen flow (`tui/src/screens/connect.rs`):** replace
`ConnectFlow::Unavailable` (T2a) for oauth-tagged providers with:

```
ConnectFlow::OAuth { provider: String, label: String, phase: OAuthPhase }
enum OAuthPhase { Starting, AwaitingSignIn, Done, Failed { reason: String } }
```

Host setter (mirroring the Copilot `set_device_code`/`set_done`/`set_failed`):
`set_oauth_phase(..)`. Render: `AwaitingSignIn` → "Opening your browser —
complete the sign-in there…"; `Done` → "Signed in ✓"; `Failed { reason }` →
"Sign-in failed: {reason}". Any key on a terminal phase (Done/Failed) returns to
the REPL (mirrors the Copilot terminal-phase fix). Esc cancels mid-flight.

**Trigger:** `root::pump_open_connect` (T2a routes by method) gains an OAuth route
→ open the OAuth connect screen + raise `pending_oauth_login` (with the provider).
A ticker (mirror the copilot-login ticker) drains it, calls
`oauth_connect_driver.login(provider)`, and pushes phase updates to the screen;
on success refreshes `provider_availability` (so `✓` appears).

**Method-choice step (`connect.rs`):** for a provider whose catalog method set has
>1 option (Anthropic), a new `ConnectFlow::MethodChoice { provider, label, options:
Vec<ConnectMethod> }` — a tiny highlight reducer (↑/↓, Enter picks, Esc cancels) →
on pick, transitions to the chosen flow (OAuth or ApiKey). Single-method providers
skip it.

### Piece 2 — Per-provider detail panel

A pure render function `render_provider_detail(provider: &ConnectableProvider,
catalog_models: &[String]) -> String` showing: label; connected state (`✓
connected via <kind>` / `not connected`); model list (from the catalog
`ProviderProfile.models`, capped/elided); and the available login method(s). The
catalog model list must reach the TUI — extend the T2a `ConnectableProvider` DTO
(or a sibling map) with each provider's model names (engine-side, from
`builtin_presets()`), threaded the same way as `provider_auth_methods`.

### Piece 3 — Unified visual flow

A new top-level connect screen state (`tui/src/screens/connect_flow.rs`, new) that
**composes** the existing pure pieces: it holds the `ConnectPickerState` (list,
T2a) + the highlighted provider's detail + the active action sub-state
(`ConnectScreenState` once the user drills in). Rendering is a two-pane layout
(list left, detail/action right — see the approved mockup). Navigation: ↑/↓ move
the list (live-updating the detail pane); `→`/Enter drills into the highlighted
provider's connect action (method-choice / api-key field / OAuth / copilot
deploy); Esc backs out a level then closes. `github_deploy` becomes a sub-state of
the copilot action rather than a separate top-level `Screen`.

The existing `Screen::ConnectPicker` / `Screen::Connect` / `Screen::GithubDeployment`
variants collapse into a single `Screen::ConnectFlow(ConnectFlowState)`; the old
reducers are reused as inner state machines (not deleted — their tests stay green).
`/model` keeps `Screen::Model` but its renderer adopts the same `picker_popup`
chrome for visual consistency.

### Error handling / fallbacks

| Situation | Behaviour |
|---|---|
| Browser can't open (bare SSH, no DISPLAY) | `OAuthPhase::Failed { reason: "couldn't open a browser…" }`; the screen suggests using an API key (for Anthropic) or copying the URL. Never hangs. |
| OAuth deadline / user abandons | 60s deadline (OpenAI) / loopback timeout → `Failed`; Esc cancels mid-flight. |
| OAuth provider has no driver wired (driver `None`) | Falls back to T2a behaviour for that provider (`Unavailable` "coming soon") — inert, no panic. |
| Empty catalog / no models for a provider | Detail panel shows "no models listed"; never panics. |
| Method-choice for a single-method provider | Skipped — straight to the one flow. |

### Testing

**Pure unit (deterministic, no I/O):**
- `OAuthPhase` render: each phase → expected text; terminal-phase any-key →
  Cancel; Esc → Cancel.
- `MethodChoice` reducer: ↑/↓ highlight, Enter → transitions to the chosen flow,
  Esc → Cancel; built only when method set >1.
- `render_provider_detail`: connected/not, model list elision, method labels.
- `ConnectFlowState` navigation: list ↑/↓ updates detail; `→` drills in; Esc
  backs out then closes — the composed state machine.
- Method-set derivation from catalog: Anthropic → {OAuth, ApiKey}; ChatGPT →
  {OAuth}; openai → {ApiKey}; copilot → {Device}.

**Driver (mock):** an `OAuthConnectDriver` test double (no browser, no network)
driving the phase sequence Starting→…→Done and →Failed; assert the screen renders
each and that Done triggers a `provider_availability` refresh.

**Integration (PTY, harness plays terminal):** drive the unified flow — highlight
a provider (detail updates), drill in, pick a method, and (with the mock driver)
reach Done — asserting the rendered panes. OAuth's real browser/loopback is NOT
exercised in PTY (mock the driver); a separate llm-client-level test already
covers the handles.

**Regression:** T2a connect-picker tests, the `/model` picker tests, and the
reused `connect`/`github_deploy` reducer tests stay green.

## Known limitations

- **OAuth needs a local browser + loopback port** (same as claude-code). Over bare
  SSH without forwarding, the browser-open fails → honest `Failed` + API-key
  fallback. (Device-code is a possible future fallback; out of scope here.)
- The unified two-pane layout is the largest/riskiest piece (TUI layout; the Warp
  alt-screen rendering issue from T1 lurks — but that is **T3**, not T2b). Planned
  **last** so it can be trimmed if it balloons.
- ChatGPT's flow is built but less battle-tested than Anthropic's; planning will
  verify `OpenAiOAuthHandle`'s exact trigger surface before wiring.

## Bigger picture (tracks)

1. **T1** — theme/color foundation. ✅ merged (`6f1c22b27`).
2. **T2a** — connect data foundation. ✅ merged (`3cf8e313d`).
3. **T2b (this spec)** — OAuth wiring + detail panel + unified visual. ← implement next.
4. **T3** — Warp / alt-screen rendering model. Independent, hardest, last.
