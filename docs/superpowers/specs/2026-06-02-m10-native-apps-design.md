# LingXi Code M10 — Native Apps (Electron · iOS · Android) Design

> **Status**: DRAFT v1
> **Date**: 2026-06-02
> **Target version**: v0.13.0 (foundation), v0.14.0+ (app tracks)
> **Predecessors**: M8 / v0.9.0 (Composable Engine & Mobile), M9 / v0.10.0 (Multi-Agent TUI)
> **Theme**: One client contract, two transports, three native renderers — full TUI parity.

---

## §1 Goal & non-goals

### Goal

Deliver **three native front-end applications** on top of the existing platform-agnostic
Rust engine, each reaching **full TUI parity**:

1. **Electron desktop** (`clients/electron/`) — TypeScript + React, talking to a local
   `bridge-server` over WebSocket/JSON-RPC (out-of-process engine).
2. **iOS** (`clients/ios/`) — SwiftUI, consuming the existing `.xcframework` via UniFFI
   (in-process engine on device).
3. **Android** (`clients/android/`) — Jetpack Compose, consuming the existing `.aar` via
   UniFFI (in-process engine on device).

The governing principle: the TUI already renders the engine's **entire** surface from a
turn-event / `Effect` stream. We expose that surface **once** as a versioned contract
(`client-protocol`) and have all three UIs render it. "Parity" becomes "render the
contract," not three independent re-derivations that drift.

### Non-goals (deferred)

- **Engine behavior changes** — this is a presentation + transport program. The
  orchestrator, tools, permission gate, sessions, etc. are byte-equivalent before and after.
- **New tools or capabilities** — no new tool crates; apps render the existing 40-tool surface.
- **Remote-drive of a desktop engine from mobile** — the bridge protocol's original
  "mobile drives desktop" use case stays out of scope. Mobile runs the engine in-process;
  Electron runs a *local* bridge-server child. Cross-device remote-drive is a later milestone.
- **App store / signing / distribution pipelines** — packaging, notarization, Play/App Store
  submission are deferred to a follow-up milestone. M10 targets local dev builds.
- **Push notifications, deep links, widgets, background execution** — platform-native
  affordances beyond the parity surface are out of scope.
- **Theme authoring UI** — the 6 built-in themes ship; a theme editor does not.

---

## §2 Decomposition

Full-parity × 3 platforms is a **program**, sequenced as **one shared foundation +
three app tracks built in lockstep**. This spec covers the program at the architecture
level and the **foundation** in implementation depth. Each app track gets its own
implementation plan derived from the §5 parity checklist.

| Track | Crates / dirs | Blocks |
|---|---|---|
| **F — Foundation** | `client-protocol/`, `client-adapter/`, `bridge-server` (complete), UniFFI surface in `ios-framework`/`android-aar`, `clients/shared/` (TS SDK) | everything |
| **A1 — Electron** | `clients/electron/` | F-1, F-2 |
| **A2 — iOS** | `clients/ios/` | F-1, F-3 |
| **A3 — Android** | `clients/android/` | F-1, F-3 |

---

## §3 Architecture

```
            ┌─────────────────────────────────────────────┐
            │            client-protocol  (NEW crate)       │  ← one versioned DTO contract
            │  Events: msg deltas, thinking, tool cards,    │    (everything a client can see)
            │  permission reqs, task/agent rows, MCP/hooks  │
            │  listings, cost, model list, slash catalog,   │
            │  memory, settings/auth                        │
            │  Commands: send, approve/deny, cancel, model, │    (everything a client can do)
            │  run-command, resume, …                       │
            └───────────────┬───────────────────────────────┘
                            │  client-adapter (NEW crate): engine Effect/event ⇄ DTO
            ┌───────────────┴───────────────┐
            ▼                               ▼
   ┌──────────────────┐            ┌──────────────────┐
   │ bridge-server    │            │ ios-framework /  │
   │ (complete it):   │            │ android-aar:     │
   │ WS + JSON-RPC,   │            │ UniFFI foreign   │
   │ handshake, auth, │            │ stream + methods │
   │ engine-desktop   │            │ engine in-process│
   └────────┬─────────┘            └────────┬─────────┘
            │ WebSocket                      │ in-process FFI
            ▼                                ▼
   ┌──────────────────┐   ┌──────────────────┐   ┌──────────────────┐
   │ clients/electron │   │ clients/ios      │   │ clients/android  │
   │ TS + React       │   │ SwiftUI          │   │ Jetpack Compose  │
   └──────────────────┘   └──────────────────┘   └──────────────────┘
```

**One protocol, two transports, three renderers.**

- **Electron** is out-of-process: it spawns a local `bridge-server` child and speaks
  WebSocket/JSON-RPC. The engine never links into Node.
- **iOS / Android** are in-process: UniFFI exposes `client-protocol` commands + an
  event-stream callback; the engine runs on the device.
- Both transports carry the **same** `client-protocol`. The adapter is the only place
  that knows engine internals.

### §3.1 Dependency rules (extends M8 `deny.toml`)

- `client-protocol` depends only on `protocol` (+ serde). No engine subsystem deps.
- `client-adapter` depends on `client-protocol` + the engine event/effect types. It is the
  single allowed engine→DTO bridge.
- `bridge-server` depends on `client-adapter`, `client-protocol`, `bridge`, `engine-desktop`.
- `ios-framework`/`android-aar` depend on `client-adapter`, `client-protocol`, `engine-mobile`.
- `clients/*` (non-Rust) depend on generated bindings / the TS SDK only — never on Rust source.

---

## §4 Foundation components (immediate build)

### §4.1 `client-protocol/` (new engine crate)

Versioned DTOs for the full client-visible surface. Derived as mechanically as possible
from existing engine event types (so parity is structural), but expressed as an explicit,
serde-stable contract independent of internal enums.

**Events (engine → client):**
- Session lifecycle: `SessionStarted`, `SessionEnded`, `SessionList`, `SessionResumed`.
- Message stream: assistant `TextDelta`, `ThinkingDelta`, `ToolUseStarted`/`ToolUseResult`
  (tool card payloads for all ~22 renderers), `MessageComplete`.
- Permission: `PermissionRequest` (tool, input, suggestions), `PermissionResolved`.
- Tasks/agents: `TaskRow`, `TaskOutputChunk`, `TaskStatusChanged`, `AgentList`,
  coordinator/team status, worker-permission events.
- Listings: `McpServers`, `Hooks`, `Agents`, `SlashCommandCatalog`, `MemoryEntries`.
- Meta: `CostUpdate`, `UsageUpdate`, `ModelList`, `ModelChanged`, `SettingsSnapshot`,
  `AuthState`, `Error`.

**Commands (client → engine):**
- `SendPrompt`, `Cancel`/`Abort`, `ApprovePermission`/`DenyPermission`,
  `SetModel`, `RunSlashCommand`, `ResumeSession`, `NewSession`, `ListSessions`,
  `SearchMessages`, `ExportSession`, `SetTheme`, `Login`/`Logout`, `RefreshListings`.

**Versioning:** a `CLIENT_PROTOCOL_VERSION` constant; serde round-trip + JSON-schema
snapshot tests freeze the wire format. Bumped on any breaking field change.

### §4.2 `client-adapter/` (new engine crate)

The single canonical mapping between engine turn-events / `Effect`s and `client-protocol`.
Reuses the TUI's existing render fixtures as parity test inputs (given the same engine
event a TUI renderer consumes, the adapter must emit the matching DTO).

### §4.3 `bridge-server` completion (Electron transport)

Grow the M8 hello-world into the real server:
- WebSocket listener (bind localhost, ephemeral port written to a lockfile for the app).
- `ClientHello`/`ServerHello` handshake (types exist in `bridge::wire`).
- Auth (token in lockfile, echoed by the client — reuse the IDE-bridge lockfile pattern).
- Frame routing: `BridgeRequest{method,params}` → `client-protocol` Command →
  `engine-desktop`; engine events → `client-adapter` → `client-protocol` Event →
  streamed `BridgeResponse`/event frames.

### §4.4 UniFFI client surface (mobile transport)

Extend `ios-framework`/`android-aar`:
- A foreign-callable `submit(command: ClientCommand)` and a `ClientEventListener`
  callback interface (UniFFI) that streams `ClientEvent`s to Swift/Kotlin.
- The engine (`engine-mobile`) runs in-process; the adapter feeds the listener.

### §4.5 `clients/shared/` (TS SDK)

A typed WebSocket client wrapping `client-protocol` for Electron (connect to the
bridge-server lockfile, handshake, send Commands, subscribe to Events). Swift/Kotlin
consume generated UniFFI bindings directly; if useful, a thin shared view-model layer
per platform may wrap them, but no cross-platform JS is shared with mobile.

---

## §5 Parity surface (per-app checklist)

Each item is a **shared checklist applied to all three apps**, so they advance in
lockstep rather than one racing ahead. Mirrors the TUI inventory (M7–M9):

1. 3-zone layout: status line / scrollback / prompt input.
2. Streaming assistant text + thinking.
3. ~22 message renderers (tool cards: file ops, shell, search, web, diff, etc.).
4. 3 permission dialogs (allow / allow-always / deny, with suggestions).
5. Background-task rows + footer + task-output dialog (live tailing).
6. Coordinator / team status chrome + worker-permission chrome.
7. `/agents` discovery screen.
8. Full-page screens: Doctor, Resume, Settings, Memory.
9. Message search / jump / export selector.
10. Theme picker (6 themes) + model picker + cost/usage display.
11. Slash-command catalog + invocation + result rendering.
12. Prompt input affordances: multi-line, history, `@`-completion, image paste/attach.

## §6 Data flow

1. UI input → `client-protocol` **Command** → transport → `engine-{desktop,mobile}` →
   orchestrator turn loop.
2. Engine emits turn events / `Effect`s → `client-adapter` → **Events** → transport →
   UI renders **incrementally** (streaming deltas).
3. Permission: engine emits `PermissionRequest` → UI dialog → decision Command → gate
   resolves. Same request/stream pattern for task-output tailing, model switch, and
   slash-command results.

## §7 Testing

- `client-protocol`: serde round-trip + JSON-schema snapshot (locks the wire format).
- `client-adapter`: engine-event fixtures → expected DTOs (reuse TUI fixtures).
- `bridge-server`: extend existing `protocol_roundtrip_test`; add handshake/auth tests +
  an end-to-end "drive one turn over WS" integration test against a real `engine-desktop`.
- UniFFI: foreign-binding smoke tests (the foreign interface exposes commands + listener).
- Apps:
  - Electron → Playwright against a **fixture bridge** (deterministic protocol replay) +
    unit tests for renderers.
  - iOS → XCUITest + unit tests; fixture event stream where the engine is stubbed.
  - Android → Compose UI tests + unit tests; same fixture pattern.
- Pattern reused from M9: real data where the engine is live, deterministic protocol
  fixtures where it is stubbed.

## §8 Build order

- **M10-F1** — `client-protocol` + `client-adapter` (+ tests). No transport.
- **M10-F2** — `bridge-server` walking skeleton: connect, handshake, run one turn,
  stream assistant text to a CLI test client.
- **M10-F3** — UniFFI client surface: same skeleton proven from a Swift/Kotlin unit test.
- **M10-A*** — scaffold all three apps (`clients/electron`, `clients/ios`,
  `clients/android`) to an MVP chat walking skeleton, then iterate the §5 parity
  checklist surface-by-surface across all three together.

## §9 Repo layout

New top-level `clients/` directory beside `lingxi-code/`:

```
clients/
├── electron/      ← TS + React; spawns bridge-server, talks WebSocket
├── ios/           ← SwiftUI Xcode project; consumes .xcframework
├── android/       ← Jetpack Compose Gradle project; consumes .aar
└── shared/        ← TS client SDK wrapping client-protocol (Electron)
```

New Rust crates land inside the `lingxi-code/` workspace:
`lingxi-code/client-protocol/`, `lingxi-code/client-adapter/`.

## §10 Isolation

All M10 work is performed in an **isolated git worktree** so the multi-crate + multi-client
restructure never destabilizes `main`. The worktree is the workspace for the foundation
and all three app tracks until the program is ready to integrate.
