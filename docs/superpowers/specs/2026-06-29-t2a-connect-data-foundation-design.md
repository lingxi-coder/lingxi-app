# T2a — Connect UI Data Foundation — Design Spec

- **Date:** 2026-06-29
- **Status:** Approved scope (T2a); ready for implementation planning
- **Track:** T2a of the connect-UI overhaul (T2 is track 2 of 3 — see "Bigger picture")
- **Owner area:** `tui` crate (`/connect` screens) + `apps/engine-desktop` (data seam) + `llm-client` catalog (read-only source)

## Context

The `/connect` UI overhaul decomposed into three tracks (see the T1 spec,
`2026-06-28-theme-color-foundation-design.md`). T1 (theme/color foundation)
shipped and merged. This spec is **T2a — the data-correctness backbone** of T2,
chosen (over "all of T2 at once") to fix the real-data + trust problems first,
at the lowest risk, before any presentation overhaul.

### Current behaviour (the problems, confirmed in code)

1. **The provider list is hardcoded.** `connect_picker.rs::default_connect_rows()`
   is a static `vec![...]` of 9 providers. It is not derived from the real
   `llm-client` provider catalog, so the picker can drift from the providers the
   engine actually supports (new providers never appear; removed ones linger).
2. **`✓` connected state is a fragile join.** Each row's `connected` flag is
   computed by `any_connected(availability, &["best", "guess", "keys"])` — a
   per-row hand-authored list of candidate keys looked up in
   `AppState.provider_availability`. A key-naming mismatch silently yields no
   `✓`, even when the provider is configured.
3. **Login-method descriptions can lie.** `connect.rs` implements exactly two
   flows — a masked API-key field (`ConnectFlow::ApiKey`) and the Copilot OAuth
   device-flow (`ConnectFlow::Copilot`). But row descriptions promise
   "Pro/Max sign-in" (Anthropic OAuth) and "Sign in with ChatGPT" (ChatGPT
   OAuth) that have no flow behind them — selecting such a provider drops the
   user into a dead API-key field.

### The real data sources (already exist)

- **`llm-client/src/catalog/presets.rs::builtin_presets() -> BuiltinCatalog`**
  → `providers: Vec<ProviderProfile>`. Each `ProviderProfile`
  (`llm-client/src/config.rs:52`) carries `profile_name`, `auth: AuthStrategy`
  (`config.rs:137`: `ApiKey` / `Bearer` / `OAuthBearer` / `CopilotBearer` /
  `ChatGptOAuth` / `AwsSigV4` / `GcpToken` / `AzureToken` / `None`), `credential`,
  and `models`. **This is the authoritative provider set + the real per-provider
  login method.**
- **`DesktopRuntime.provider_availability`** (`apps/engine-desktop/src/lib.rs`
  ~:4714, built from real credential checks) → `BTreeMap<profile_name, bool>` =
  "has a usable credential". The `✓` source already exists and is already
  keyed by `profile_name`; it is only joined fragilely.

## Goals

1. The `/connect` provider list is **derived from the real catalog**
   (`builtin_presets()`), so it reflects exactly the providers the engine
   supports. New catalog providers appear automatically.
2. The `✓` is **trustworthy**: keyed directly by `profile_name` against
   `provider_availability` — no per-row candidate-key guessing.
3. The per-provider **login method is derived from the real `AuthStrategy`**, and
   the connect screen never promises a flow it cannot perform: methods without an
   implemented flow are surfaced honestly ("browser sign-in — coming soon"),
   never as a dead key field.
4. No regression to the existing pure-reducer + render-oracle architecture, the
   `/model` picker, or T1's theming.

## Non-goals (deferred to T2b / later)

- **Per-provider detail panel** (showing a provider's models / pricing / current
  auth state).
- **Unified visual flow** — merging `connect_picker` / `connect` / `model` /
  `github_deploy` into one screen. (T2a keeps the existing screen boundaries.)
- **Wiring NEW OAuth login flows** (Anthropic Pro/Max OAuth, OpenAI ChatGPT
  OAuth as live browser flows). T2a only *surfaces* them honestly; implementing
  them is separate feature work.
- The `/model` picker curation (curated list / only-connected / Recent group) —
  already implemented earlier in `model.rs`; untouched here.
- Changing the `llm-client` catalog data model or any byte-locked wire surface.

## Decisions made

- **Provider SET + auth method are data-driven; display labels stay curated.**
  The catalog (`ProviderProfile`) carries `profile_name` + `auth` but no
  human label / blurb / "popular" grouping. Rather than widen the shared
  `llm-client` config struct (bigger, shared surface), T2a keeps a small
  **curated display-metadata map** in the `tui` crate keyed by `profile_name`
  (label, one-line blurb, popular flag), with a **title-cased fallback** for any
  `profile_name` not in the map. This makes the provider *existence* fully
  data-driven (a new catalog provider still renders, just with a derived label)
  while keeping nice labels for known providers. The auth-method phrasing in the
  description is **derived from `AuthStrategy`**, not the curated blurb — so it
  cannot lie.
- **One new thread-through seam, mirroring `provider_availability`.**
  engine-desktop assembles the connectable-provider list once at build and
  threads it into `AppState` exactly like `provider_availability`.

## Design

### Data flow

```
apps/engine-desktop build()  (where provider_availability is already built)
  └─ assemble connectable_providers: Vec<ConnectableProvider>
       from builtin_presets().providers  (profile_name, auth: AuthStrategy)
       joined with provider_availability  (connected: bool, keyed by profile_name)
  └─ DesktopRuntime.connectable_providers  (new field, beside provider_availability)

session.rs (mount)
  └─ AppState::set_connectable_providers(std::mem::take(&mut runtime.connectable_providers))

tui/src/screens/connect_picker.rs
  └─ connect_rows_from(&AppState.connectable_providers)        [replaces default_connect_rows]
       for each ConnectableProvider:
         display = connect_display_meta(&profile_name)         [curated map + title-case fallback]
         method  = ConnectMethod::from(auth)                   [ApiKey | CopilotDevice | OAuthSoon]
         ConnectRow {
           provider_id: profile_name,
           label:       display.label,
           description: display.blurb + method.suffix(),        [e.g. " — API key" / " — device sign-in" / " — browser sign-in (coming soon)"]
           popular:     display.popular,
           connected:   provider.connected,                     [direct, keyed by profile_name]
           method,                                              [carried so the connect screen picks the right flow]
         }

tui/src/screens/connect.rs
  └─ open flow from ConnectRow.method (NOT from a hardcoded description):
       ConnectMethod::ApiKey        → ConnectFlow::ApiKey { provider_id, label }
       ConnectMethod::CopilotDevice → ConnectFlow::Copilot
       ConnectMethod::OAuthSoon     → ConnectFlow::Unavailable { label, reason }   [new: honest "coming soon" screen]
```

(The `AuthKind → ConnectMethod` mapping happens ONCE in `connect_picker`
(`ConnectMethod::from`): `ApiKey`/`Bearer` → `ApiKey`; `CopilotBearer` →
`CopilotDevice`; `ChatGptOAuth`/`OAuthBearer` (and the unmapped
`AwsSigV4`/`GcpToken`/`AzureToken`) → `OAuthSoon`; `None` → provider filtered out
of the connectable set. `connect.rs` then only ever sees `ConnectMethod`.)

### Components

- **`apps/engine-desktop/src/lib.rs` (modify).** Beside the existing
  `provider_availability` build, assemble `connectable_providers:
  Vec<ConnectableProvider>` from `builtin_presets().providers` joined with the
  availability map. Add it to `DesktopRuntime` (new field). The
  `ConnectableProvider` type (`{ profile_name: String, auth: AuthKind, connected:
  bool }`) is a small TUI-facing DTO — define it where `AppState` can consume it
  (a shared low type, or re-exported), carrying a TUI-local `AuthKind` mirror of
  the relevant `AuthStrategy` variants so the `tui` crate need not depend on
  `llm-client` internals.
- **`tui/src/state.rs` + `tui/src/session.rs` (modify).** Add
  `AppState.connectable_providers: Vec<ConnectableProvider>` + a
  `set_connectable_providers` setter, threaded at mount exactly like
  `set_provider_availability`. Default empty.
- **`tui/src/screens/connect_picker.rs` (modify).** Replace
  `default_connect_rows(availability)` with `connect_rows_from(&[ConnectableProvider])`.
  Add the curated `connect_display_meta(profile_name) -> DisplayMeta` (seed from
  the current 9 labels/blurbs/popular flags; title-case fallback) and the
  `ConnectMethod` enum + `from(AuthKind)` + `suffix()`. `ConnectRow` gains a
  `method: ConnectMethod` field. The grouping (Popular / Providers) and the pure
  reducer are unchanged.
- **`tui/src/screens/connect.rs` (modify).** Select `ConnectFlow` from the row's
  `ConnectMethod` (threaded via `pending_connect`). Add a new
  `ConnectFlow::Unavailable { label, reason }` terminal state that renders an
  honest "browser sign-in isn't available in this build yet — connect with an API
  key instead" message (Esc closes). No dead key field.
- **Untouched:** the `/model` picker (`model.rs`), `github_deploy.rs`, the
  `llm-client` catalog data model + wire surfaces, T1 theming, `picker_popup.rs`
  rendering.

### Error handling / fallbacks

| Situation | Behaviour |
|---|---|
| `connectable_providers` empty (engine not built / pre-mount) | Picker shows no rows (or a single "no providers" line) — never panics. Mirrors the empty-`provider_availability` no-`✓` behaviour today. |
| `profile_name` not in the curated display map | Title-cased `profile_name` as the label + a generic blurb; row still renders (data-driven existence preserved). |
| `AuthStrategy` variant with no TUI mapping (`AwsSigV4`/`GcpToken`/`AzureToken`) | Mapped to `ConnectMethod::OAuthSoon`-style "not available in this build" (honest); not offered as a working flow. (These aren't in the current connectable set; covered defensively.) |
| Provider connected but selected again | Existing connect-screen behaviour (re-enter credential) — unchanged. |

### Testing

**Pure unit tests (deterministic, no I/O):**
- `connect_rows_from`: a fixed `Vec<ConnectableProvider>` → expected rows (label
  from curated map; title-case fallback for an unknown `profile_name`; `connected`
  copied straight through; `method` derived from `AuthKind`; description suffix
  matches the method).
- `ConnectMethod::from(AuthKind)` table: `ApiKey`/`Bearer` → `ApiKey`;
  `CopilotBearer` → `CopilotDevice`; `ChatGptOAuth`/`OAuthBearer` → `OAuthSoon`.
- `connect.rs` flow selection: a row with each `ConnectMethod` opens the right
  `ConnectFlow` (incl. the new `Unavailable`).
- ✓ correctness: a provider with `connected: true` renders the leading `✓`;
  `false` renders none — keyed purely by `profile_name`, no candidate-key list.

**Integration (engine seam):**
- An engine-desktop test asserting `build()` populates `connectable_providers`
  from the real `builtin_presets()` set and that each entry's `connected` matches
  the `provider_availability` value for the same `profile_name` (mirrors the
  existing `build_surfaces_provider_availability_and_adapter` test).

**Regression:** the `/model` picker tests, `github_deploy` tests, and the
connect-picker reducer tests stay green; the catalog wire surfaces are untouched.

## Known limitations

- Display **labels/blurbs** for known providers remain curated (not from the
  catalog) — a deliberate trade to avoid widening the shared config struct. A
  brand-new provider renders with a title-cased label until it's added to the map.
- OAuth methods are **surfaced but not performed** in T2a (honest "coming soon").
  Wiring them is T2b / separate feature work.
- The four connect-related screens remain separate (unified visual = T2b).

## Bigger picture (tracks, for reference)

1. **T1** — theme/color foundation. ✅ done + merged (`6f1c22b27`).
2. **T2a (this spec)** — connect-UI data foundation. ← implement next.
3. **T2b** — connect-UI presentation: per-provider detail panel, unified visual
   flow, and wiring the missing OAuth login flows. Builds on T2a.
4. **T3** — Warp / alt-screen rendering model. Independent, hardest, last.
