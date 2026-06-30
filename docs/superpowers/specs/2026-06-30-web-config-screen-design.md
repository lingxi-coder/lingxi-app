# `/web` Provider Configuration Screen — Design Spec

- **Date:** 2026-06-30
- **Status:** Approved (design); awaiting user review before implementation planning
- **Owner area:** `tui` crate (`/web` screens), `tools/web` client-side search config, `apps/engine-desktop` runtime wiring, secure credential persistence

## Context

LingXi now has provider-agnostic client-side `WebSearch` for non-Anthropic models
such as GitHub Copilot. The runtime can search through a fallback chain
(`TAVILY_API_KEY` → `BRAVE_API_KEY` → `LINGXI_SEARXNG_URL` → DuckDuckGo), but the
only configuration surface is environment variables. That makes discovery poor:
users cannot see which web backend is active, whether credentials exist, or test a
provider before relying on it during chat.

The user asked for a `/web` configuration interface “like `/connect`”. Existing
project patterns:

- `/connect` uses a searchable picker plus provider-specific flow screens,
  status/detail lines, pending actions, and secure credential storage.
- `/settings` is read-only for most config; writes go through a `$EDITOR` handoff,
  so turning it into an interactive web-secret editor would fight its contract.
- `tools/web/src/web_search_client.rs` already defines the providers this screen
  configures: Auto, DuckDuckGo, Tavily, Brave, SearXNG.

## Goals

1. Add a dedicated `/web` TUI flow, visually and behaviorally close to `/connect`.
2. Let users inspect and choose the active web-search provider: Auto,
   DuckDuckGo, Tavily, Brave, or SearXNG.
3. Persist secrets securely: Tavily and Brave API keys go to the existing secure
   credential store, never plaintext settings.
4. Persist non-secrets in user settings: active provider and SearXNG URL.
5. Support a provider test action that runs the same client-side search path used
   by chat-time `WebSearch`.
6. Keep the runtime and UI in sync: what `/web` reports is what `WebSearch` uses.

## Non-goals

- Add Exa, SerpAPI, Perplexity, or other search providers in v1.
- Store secrets in project-local files or plaintext JSON.
- Rewrite the existing Anthropic hosted WebSearch path.
- Change `/settings` from read-only/tabbed status into an interactive editor.
- Add browser mockups or a graphical UI; this is a terminal/TUI surface.

## Approved UX

### Entry

Typing `/web` opens a dedicated Web provider picker.

Rows:

1. **Auto** — fallback chain.
2. **DuckDuckGo** — keyless fallback.
3. **Tavily** — API-key search.
4. **Brave** — API-key search.
5. **SearXNG** — URL-based search instance.

### Picker detail panel

The highlighted row shows detail lines like `/connect`:

- configured / missing key / missing URL / keyless
- backend type and intended use
- active provider status
- fallback behavior
- shortcut hints: `Enter` configure/select, `t` test search, `Esc` close

### Provider config screens

Selecting a provider opens a provider-specific config screen:

- **Auto:** set active provider to Auto; no secret entry.
- **DuckDuckGo:** set active provider to DuckDuckGo; no secret entry.
- **Tavily:** paste API key; save to secure store; can test.
- **Brave:** paste API key; save to secure store; can test.
- **SearXNG:** enter instance URL; save to settings; can test.

The first implementation should keep the flow small and pure: a picker screen and
a config/detail screen are enough. No multi-pane rewrite is required.

### Test search

Config screens support `t` to run a test search. The default query is a stable,
real query such as `current weather Beijing`. The screen displays either:

- success: provider label + result count + top result title/URL, or
- failure: provider label + HTTP/transport/config error.

## Architecture

### New TUI modules

Add a small `/web` screen family:

- `tui/src/screens/web_picker.rs`
  - pure rows, status/detail builders, highlight/search reducer
  - no I/O
- `tui/src/screens/web_config.rs`
  - pure reducer for provider config and test status display
  - text buffer for Tavily/Brave API key or SearXNG URL
  - no I/O
- `Screen::WebPicker` and `Screen::WebConfig` variants in `tui/src/screens/mod.rs`

Root/app glue mirrors `/connect`:

- `/web` slash-command intercept opens `WebPicker` synchronously from current
  snapshot data.
- Picker selection opens `WebConfig`.
- Save/test actions raise pending state for async root pumps to execute.
- Render path stays synchronous and pure.

### Persistence

Secrets:

- Tavily key → secure credential id such as `web:tavily`.
- Brave key → secure credential id such as `web:brave`.

Non-secrets:

- active provider → `~/.lingxi/settings.json` (suggested key: `webSearch.provider`).
- SearXNG URL → `~/.lingxi/settings.json` (suggested key: `webSearch.searxngUrl`).

No project-local config in v1. Settings are user-global, matching `/connect`'s
credential behavior.

### Runtime WebSearch integration

Replace the current env-only resolver with a config-aware resolver:

1. If active provider is specific:
   - use that provider if configured;
   - return an actionable error if required key/URL is missing;
   - do not silently pick a different provider unless a later design explicitly
     adds “fallback on selected-provider failure”.
2. If active provider is `auto`:
   - Tavily if secure key exists;
   - Brave if secure key exists;
   - SearXNG if URL exists;
   - DuckDuckGo fallback.

The resolver should still honor existing env vars as compatibility fallbacks:

- `TAVILY_API_KEY`
- `BRAVE_API_KEY`
- `LINGXI_SEARXNG_URL`

Precedence inside a provider:

1. secure store / settings value from `/web`
2. environment variable
3. missing/unconfigured error (or DuckDuckGo fallback in Auto)

## Data flow

1. User types `/web`.
2. TUI opens `WebPickerState` with an in-memory snapshot:
   - active provider
   - key existence for Tavily/Brave
   - SearXNG URL
   - last test result, if kept in memory
3. User selects provider.
4. `WebConfigState` renders editable provider config.
5. Save action raises pending root action:
   - save secret to credential store, or
   - write non-secret settings JSON.
6. Root pump completes save, refreshes snapshot, and redraws screen.
7. Test action calls the same client-side search resolver that chat uses.
8. Chat-time `WebSearch` uses the same resolver, so UI and runtime agree.

## Error handling

- Missing Tavily/Brave key:
  - UI: `Missing API key`.
  - Runtime: `Tavily is selected but no API key is configured. Run /web to configure it.`
- Missing SearXNG URL:
  - UI validates URL parse before save.
  - Runtime errors with an actionable message.
- Test search HTTP failure:
  - show provider + HTTP status or transport message inline.
- DuckDuckGo no results:
  - show `DuckDuckGo returned no results` if all fallbacks fail.
- Secure store unavailable:
  - show error; never fall back to plaintext secret storage.

## Testing plan

1. Pure reducer tests:
   - picker highlight/select/filter;
   - config text editing;
   - save/test action outcomes.
2. Persistence tests:
   - settings read/write for active provider and SearXNG URL;
   - credential-store key existence checks.
3. Resolver tests:
   - active provider wins;
   - Auto fallback order;
   - missing selected provider errors;
   - env fallback compatibility.
4. Render tests:
   - detail lines show configured/missing/test status.
5. Optional PTY test:
   - open `/web`, highlight DuckDuckGo, verify status/detail text.

## Open decisions for implementation planning

- Exact settings JSON shape should be chosen during implementation planning after
  checking the current settings schema constraints.
- Credential id strings should be centralized next to the resolver so UI and
  runtime cannot drift.
- Whether selected-provider failure should optionally fall back is out of v1;
  v1 errors loudly to make configuration problems visible.

## Acceptance criteria

- `/web` is discoverable and opens a provider picker.
- Users can configure Tavily/Brave keys and SearXNG URL from TUI.
- Users can select Auto or a specific provider.
- Test search runs from the UI and reports success/failure.
- Copilot and other non-Anthropic models use configured client-side WebSearch.
- Existing Anthropic hosted WebSearch remains available for Anthropic first-party
  sessions.
