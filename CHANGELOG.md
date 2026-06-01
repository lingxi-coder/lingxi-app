# Changelog

## [0.12.0] — LLM Providers v2

Extends the v0.11.0 provider layer with multimodal input, reasoning-model
controls, three managed-cloud providers (Azure / Vertex / Bedrock), and a core
router — all behind the same canonical, Anthropic-shaped seam, so the
orchestrator / TUI / session / cost layers remain unchanged.

### Added
- **Vision (image input):** a canonical `ContentBlock::Image { source }`
  (base64 | URL) translated per provider — Anthropic native `image` block,
  OpenAI `image_url` content parts, Gemini `inlineData`. A fail-fast guardrail
  rejects images sent to a non-vision model.
- **Reasoning models:** per-profile `reasoningEffort` (OpenAI o-series →
  `reasoning_effort` + `max_completion_tokens`, no `temperature`) and
  `thinkingBudget` (Gemini 2.5 `thinkingConfig`); reasoning traces
  (`reasoning_content`, Gemini `thought` parts) decode into the canonical
  `Thinking` block.
- **Azure OpenAI** (`type: azureOpenAi`): the OpenAI body over Azure's
  deployment URL + `api-key` header.
- **Vertex AI** (`type: vertex`): the Gemini body over the regional
  `aiplatform.googleapis.com` endpoint, authed with a GCP OAuth2 token
  (`gcp_auth`; service-account / ADC credentials).
- **Bedrock** (`type: bedrock`): Claude via `InvokeModel` with AWS SigV4
  signing (`aws-sigv4`; environment credentials).
- **Router** (`routing` settings): model **aliases**, **fallback** chains on
  transient errors, and **retry** with backoff — modeled as `LlmProvider`
  decorators over the registry.

### New seams
- An async `Authenticator` trait (replacing the synchronous auth attach) so
  AWS SigV4 + cloud token minting run on the built request just before
  transport; `StaticAuth` wraps the v0.11.0 API-key styles.

### Unchanged / bounded
- Anthropic stays the default; request/response/cost behavior is byte-identical
  for image-free, reasoning-free conversations (the parity suite is the gate).
  `traits/` is untouched.
- Bedrock is non-streaming (a synthetic single-shot stream); signed-auth uses
  environment credentials; `/model` shows examples (configured models are
  enumerable via `ModelRouter::available_models()`); TUI paste-to-image and
  Bedrock-specific pricing are tracked follow-ups.

## [0.11.0] — LLM Providers

Multi-LLM-provider support: an integrated, in-engine provider layer so LingXi
can use providers beyond Anthropic as the model backend.

### Added
- `providers` crate: an `LlmProvider` abstraction with a pure `WireCodec` /
  `SseDecoder` translation core and a `GenericClient` harness; every provider
  normalizes to the canonical Anthropic-shaped message types, so the
  orchestrator / TUI / session / cost layers are unchanged.
- **OpenAI / OpenAI-compatible codec** — Chat Completions encode/decode +
  streaming tool-call reassembly. Covers OpenAI, Azure (via base URL), Groq,
  Together, Ollama, vLLM, DeepSeek, OpenRouter, … through settings profiles.
- **Native Google Gemini codec** — `generateContent` + `streamGenerateContent`,
  with `functionCall`/`functionResponse` tool pairing.
- `provider/model` selection (`openai/gpt-4o`, `gemini/gemini-2.0-flash`); bare
  / `claude-*` strings stay on Anthropic (back-compat). Named provider profiles
  in `settings.json` (`providers` object). `/model` surfaces the syntax.
- Per-provider cost attribution (OpenAI + Gemini reference price tables;
  prefix-aware `ModelRef`).
- Capability guardrail: tools sent to a non-tool-capable model fail fast.

### Unchanged
- Anthropic is the default; the Anthropic request/response wire and cost events
  are byte-identical (verified by the existing parity suites). `traits/` and the
  `api-client` Anthropic path are untouched.

## [0.10.0] — M9 Multi-Agent TUI Surface

Completes the multi-agent TUI: the message types, status chrome, dialogs, and
read-only agent discovery a user sees when subagents, in-process teammates, and
background tasks are active — built UI-first against a presentation adapter,
rendering real data on the live `TaskRegistryHandle` path and deterministic
fixtures where the execution pool is still stubbed.

Highlights:

- **Team message renderers (M9-03):** `TaskAssignment`, `UserTeammate`
  (task-completed + note), `UserAgentNotification`, `UserChannel`, plus
  team-memory collapse/saved parts — literal-locked to claude-code.
- **Background tasks (M9-04/05):** per-`TaskState` row renderers (7) +
  `ShellProgress` + live output tailing (`TaskRegistryHandle::output`); the
  `BackgroundTaskStatus` footer + `BackgroundTasksDialog` (list↔detail) routed
  via `active_screen`; the real `tasks::TaskRegistry` wired into the desktop
  composition root + a `MultiAgentEvent` render-loop pump.
- **Coordinator chrome (M9-06):** `TeamStatus` footer, `TeammateViewHeader`,
  `AgentProgressLine` (tree-char progress), `CoordinatorAgentStatus` panel +
  teammate-view mode.
- **Worker permissions (M9-07):** `WorkerBadge` + `WorkerPendingPermission` in
  the M6 permission focus-trap, with a cross-state-seam test.
- **Agent discovery (M9-08):** read-only `AgentsList` + `AgentDetail` via a
  `/agents` screen (catalog from `OrchestratorHandle::list_agents`).
- **Validation (M9-09):** `parity_tui_multiagent` fixture; repaired the
  `test-harness` parity suite for the new `Screen`/`RenderedMessage` variants.

**Versioning note:** M8 documented `[0.9.0]` in this changelog but never bumped
the crate versions (they stayed `0.8.0`). v0.10.0 bumps all workspace crates
`0.8.0` → `0.10.0` in one step; there is no `v0.9.0` tag.

## [0.9.0] — M8 Composable Engine + Mobile

Restructured the workspace from a `crates/`-wrapped, `lingxi-`-prefixed,
tool-monolith layout into a **flat, composition-root architecture** where the
same core agent logic assembles into separate desktop and mobile builds by
choosing a different set of capability crates — not by `#[cfg(target_os)]`
scattered through the code.

Highlights:

- **Repo + naming:** `lingxi-core/` → `lingxi-code/`; dropped the `crates/`
  wrapper (all crates flat at root) and the `lingxi-` crate-name prefix
  (`protocol`, `traits`, …). `core` → `engine` (avoids the sysroot collision).
- **Plugin abstractions:** extracted `tool-api`, `skill-api`, `command-api`;
  split the tool monolith into 14 desktop `tool-*` crates, `skills/` into
  `skill-api` + `skill-builtin`, and `commands/` into `command-api` +
  `command-core/desktop/mobile`. The byte-locked `/help` golden + 99-name
  surface are preserved.
- **Composition roots:** `apps/engine-desktop` (40 tools) and
  `apps/engine-mobile` (cross-platform subset + camera/voice/share) own all
  registry assembly; the `cli` binary delegates to `engine-desktop` and now
  ships real tools (previously an empty registry).
- **Mobile:** `traits::Platform` aggregate + device-capability callback traits
  (`CameraControl`/`VoiceRecorder`/`SharingService`/`ComputerControl`);
  `platform-ios`/`platform-android` skeletons; `tool-camera/voice/share` +
  `tool-computer-use/android-use/ios-use`; `apps/ios-framework`/`android-aar`
  UniFFI packager crates with Swift/Kotlin callback-interface skeletons.
- **Bridge:** `bridge::wire` remote-drive protocol types + `apps/bridge-server`
  skeleton.
- **Guard rails:** `scripts/check-deps.sh` enforces the §8.1 dependency graph
  (tool independence, platform isolation, apps-are-leaves, API-crate purity);
  `deny.toml` for supply-chain hygiene.

**BREAKING:** every crate was renamed/moved. Library consumers must update
imports (`lingxi_core` → `engine`, `lingxi_protocol` → `protocol`, `tools::*`
runtime types → `tool_api::*`, `commands::*` → `command_api`/`command_core`,
`skills::*` → `skill_api`). The `lingxi-cli` *binary* name is unchanged.

UniFFI binding generation (the `uniffi` crate dep + `#[uniffi::export]` +
`uniffi-bindgen`) and real Swift/Kotlin/desktop-automation capability impls are
deferred to M9; M8 ships the structure, contracts, and host-verified skeletons.

## [0.8.0] — M7 TUI Surface

The full single-user TUI surface. Builds on the v0.7.0 iocraft foundation with
rich rendering primitives (full ANSI 16/256/truecolor, markdown via
`pulldown-cmark`, syntect syntax highlighting, `similar`-backed StructuredDiff),
~22 message renderers, a windowed `VirtualMessageList` scrollback (replacing the
capped buffer), an advanced multi-line `PromptInput` (vim Normal/Insert/Visual +
motions/operators/counts, command palette, `@`-completion, history search, image
paste), four full-page screens (Doctor / Resume / Settings / Memory), a message
search/jump/export selector, and a 6-theme picker with live preview. Non-TTY and
`--no-tui` continue to fall back byte-for-byte to the stdio REPL. 16 sub-plans
M7-01..M7-16 delivered the surface.

### Sub-plans delivered (16 total)

| Plan | Component |
|---|---|
| **M7-01** | ANSI 16/256/truecolor parser + markdown rendering (`render::ansi` / `render::markdown`, `StyledLine`/`StyledSpan`). |
| **M7-02** | syntect syntax highlighting + `similar`-backed StructuredDiff (`render::syntax` / `render::diff`). |
| **M7-03** | `VirtualMessageList` windowed scrollback + line-height cache + scroll math. |
| **M7-04** | System/assistant message renderers (thinking, compact-boundary, system-text/api-error, rate-limit, shutdown, advisor, hook-progress, plan-approval). |
| **M7-05** | User message renderers (bash-input/output, command, local-command-output, memory-input, plan, prompt, resource-update, image, attachment, grouped-tool-use, collapsed-read-search). |
| **M7-06** | Multi-line `PromptInput` (content-driven height, vertical cursor, footer). |
| **M7-07** | Command palette (`/`) + `@`-path completion overlays. |
| **M7-08** | Vim Normal/Insert + motions (w/b/e, f/t, counts) + mode indicator. |
| **M7-09** | Vim operators (d/c/y × motions), Visual/Visual-line, registers/paste. |
| **M7-10** | History search (Ctrl-R) + image-paste coalescer. |
| **M7-11** | Doctor screen + `active_screen` route-state foundation. |
| **M7-12** | Resume picker screen (interactive list → select). |
| **M7-13** | Settings screens (Config / Settings / Status / Usage tabs). |
| **M7-14** | Memory editor screen + MessageSelector (search / jump / export). |
| **M7-15** | 6-theme picker with live preview + persistence; syntect theme follows. |
| **M7-16** | Parity fixtures + telemetry lock + v0.8.0 release (this plan). |

### Cross-cutting locks (M7-16)

- **TUI parity fixtures (2 new)**: `parity_tui_renderers_m7.json` +
  `parity_tui_renderers_m7.rs` (8 tests: 4 M7-04/05 renderers + 6 markdown
  element structures + 3 syntax cases + 1 multi-hunk diff) and
  `parity_tui_screens.json` + `parity_tui_screens.rs` (5 scripted screen
  flows through the live dispatcher). Structure-asserting (not per-token color).
- **Cross-state-seam review**: `cross_state_seam_test.rs` (7 tests) — the named
  M6-lesson safety net probing the single `handle_live_key` priority chain
  (permission > screen > overlay > vim > scroll).
- **Literal-lock catalog**: `docs/superpowers/literals/m7-tui-literals.md` —
  every M7 renderer + screen string indexed to its claude-code source.
- **v0.7.0 parity fixtures**: all prior fixtures continue passing unchanged.
- **Release marker**: `lingxi_core_v0_8_0_released` emitted once on first
  `ConversationOrchestrator::new` after upgrade via `std::sync::Once`.

### Telemetry events added (M7-01..M7-16)

| Plan | Count | Events |
|---|---|---|
| M7-01..M7-15 | 0 | (every TUI event candidate deferred to the M7-16 audit) |
| M7-16 | 3 | `tengu_tui_screen_opened/screen_closed` (emit sites in `AppState::open_*`/`close_screen`), `tengu_tui_search_opened` (`MessageSelectorState::open`/`open_export`) |
| M7-16 | 1 | `lingxi_core_v0_8_0_released` |
| **Total added** | **4** | (326 baseline + 4 = **330**) |

> **Telemetry count reconciliation** (the M6 "330-vs-326, report the real
> number" discipline): M7-01..M7-15 registered **0** new events — every
> palette/screen/vim/search candidate was deferred to this audit. M7-16 adds
> only the candidates with a REAL emit site: `screen_opened`, `screen_closed`,
> `search_opened`, plus the `lingxi_core_v0_8_0_released` marker → **330**.
> `tengu_tui_command_palette_opened` (per-keystroke open/close churn — no clean
> once-per-open transition), `tengu_tui_vim_mode_entered` (Esc-from-Insert
> churn; the spec wants an aggregated entry), and `tengu_tui_key_pressed` (still
> no windowed aggregator) stay DEFERRED to M8 — registering them would mint dead
> names. `ALL_EVENT_NAMES.len() == 330` is locked in
> `registry_is_exactly_330_entries`.

### Known deferred gaps (carry to M8)

| Gap | Status | Planned fix |
|---|---|---|
| Live assistant text → markdown/syntect (#211) | `AssistantTextMessage` renders the body as plain `● {body}`; markdown/syntect wired only into secondary renderers (advisor/local-output/plan) | M8 — streaming-aware measurement makes it riskier than a closeout commit |
| Height-cache expanded-aware measurement (#207) | `render_text_for_measure` proxy can drift on expanded UserToolResult | M8 |
| Resume picker corrupt-`.jsonl` (#208) | one corrupt session file aborts the whole listing | M8 |
| `JsonlWriter::append` ordering (#209) | golden-test flake root cause | M8 |
| Config tab `$EDITOR` edit handoff (#210) | `pending_config_edit` raised; terminal suspend/resume not wired | M8 |
| `force_compact` real LLM summary | Stub (`ForkedAgentRunner` not wired) | M8 |
| `CostTracker` → AnalyticsBus / per-model cost | Flat totals only | M8 |
| MCP auto-connect from `.mcp.json` | Loaded `Disconnected` | M8 |
| `OAuthHandle::login` real PKCE flow | Stub (Doctor `auth_state` = `"unknown"`) | M8 |
| Team / Coordinator / Swarm renderers | Off (UserTeammate/Channel/TaskAssignment/AgentNotification, teamMem collapsed/saved) | M8 |
| Voice / `grove` / FPS metrics / IDE-bridge dialogs / Anthropic-internal banners | Off | M8 |
| Mouse mode | Off (keyboard-only) | M8 |
| Inline terminal image display (kitty/iTerm2/sixel) | Detect + `[Image #N]` ref-insert only | M8 |
| Vim obscure cases (`.` repeat, macros, ex-commands, `/`-search) | Deferred per the M7-09 "vim parity subset" gate; `/` is consumed as a no-op in Normal | M8 |
| `tengu_tui_command_palette_opened` / `vim_mode_entered` / `key_pressed` | Deferred (no clean/aggregated emit site) | M8 |
| Manual real-terminal TUI smoke | Deferred to human (headless agent env) — same as v0.7.0 gap #5 | human verification |

### Version bump

All 42 `Cargo.toml` files: `0.7.0 → 0.8.0`. No new workspace crates in M7 — all
TUI modules live inside the existing `lingxi-tui` crate.

## [0.7.0] — M6 TUI Foundation

The first iocraft-based terminal UI for LingXi. Replaces the v0.6.0 stdio
REPL with a three-zone layout (StatusLine / Scrollback / PromptInput) that
streams tokens, surfaces tool use + tool results inline, shows interactive
permission dialogs (`[1] Allow Once` / `[2] Allow Always` / `[N] Deny`), and
wires real cost + MCP/Hooks/Agents listings + `/compact` into the TUI
surface. Non-TTY and `--no-tui` fall back byte-for-byte to v0.6.0 stdio REPL
behavior. 9 sub-plans M6-01..M6-09 delivered these components.

### Sub-plans delivered (9 total)

- **M6-01** — iocraft foundation: new `lingxi-tui` crate, event loop,
  panic-safe terminal restore, placeholder render.
- **M6-02** — Minimal working REPL: StatusLine + Scrollback + PromptInput +
  2 message renderers (UserText / AssistantText).
- **M6-03** — Streaming + `SpinnerWithVerb` (12-frame braille, batched).
- **M6-04** — Tool use rendering: `AssistantToolUseMessage` +
  `UserToolResultMessage` + minimal ANSI parser for Bash output.
- **M6-05** — 3 permission dialogs (tool_use / exit_plan_mode /
  bypass_permissions) + focus-trap.
- **M6-06** — Engine wiring: real cost in StatusLine.
- **M6-07** — Engine wiring: real MCP / Hooks / Agents listings (empty-state
  surfaces parity match).
- **M6-08** — Engine wiring: real `force_compact` through the compactor.
- **M6-09** — TUI parity fixtures + literal-lock catalog + v0.7.0 release
  tag (this plan).

### Cross-cutting locks (M6-09)

- **TUI parity fixtures (2 new)**: `tui_renderers.json` +
  `parity_tui_renderers.rs` (6 tests: StatusLine + 3 spinner frames + 4
  message renderers) and `tui_repl_loop.json` + `parity_tui_repl_loop.rs`
  (3 scripted interactive scenarios).
- **Literal lock catalog**: every user-visible TUI string indexed against
  its claude-code TSX source at `docs/superpowers/literals/m6-tui-literals.md`.
- **v0.6.0 parity fixtures**: all prior parity fixtures continue passing
  unchanged.
- **Release marker**: `lingxi_core_v0_7_0_released` emitted once on first
  `ConversationOrchestrator::new` after upgrade via `std::sync::Once`.

### Telemetry events added (M6-01..M6-09)

| Plan | Count | Events |
|---|---|---|
| M6-01 | 4 | `tengu_tui_session_started/ended`, `tengu_tui_first_render`, `tengu_tui_resize` |
| M6-03 | 2 | `tengu_tui_streaming_render_started/ended` |
| M6-05 | 2 | `tengu_tui_permission_dialog_shown/resolved` |
| M6-06/07/08 | 0 | (engine wiring reused existing M3/M4/M5 event names) |
| M6-09 | 2 | `tengu_tui_scroll_started/ended` (emit sites in `app::scroll_with_viewport`) |
| M6-09 | 1 | `lingxi_core_v0_7_0_released` |
| **Total added** | **11** | (315 baseline + 11 = **326**) |

> **Telemetry count reconciliation**: the M6 design spec §2.6 estimated
> ~15 new events → 330. The actual M6 total is **326**. The 4-event gap is
> `tengu_tui_key_pressed` (spec'd as an aggregated once-per-second counter —
> deferred to M7 because the windowed aggregator infrastructure does not yet
> exist) plus the spec's over-estimate of engine-wiring events (M6-02/04/06/07/08
> reused existing M3/M4/M5 event names rather than registering new ones).
> `ALL_EVENT_NAMES.len() == 326` is locked in `registry_is_exactly_326_entries`.

### Known deferred gaps (carry to M7)

| Gap | Status | Planned fix |
|---|---|---|
| `force_compact` summary text | Stub (`[forked-agent-stub]`); collapse + counts real | M7 real LLM summarization |
| `CostTracker` → AnalyticsBus | Not wired (`tengu_cost_recorded` doesn't fire); `total_api_duration_ms` reads 0; single-provider (`Anthropic` hardcoded) | M7 |
| MCP servers from `.mcp.json` | Loaded as `Disconnected` (no auto-connect) | M7 |
| `OAuthHandle::login` real flow | Stub | M7 OAuth UI |
| Manual real-terminal TUI smoke | Deferred to human (headless agent env) | human verification |
| iocraft 0.8.3 crossterm 0.29 vs workspace 0.28 | Parallel `map_iocraft_key` mapper in `root.rs` | tracked |
| Per-model cost breakdown | Flat totals only in `CostSnapshot` | M7 |
| `tengu_tui_key_pressed` | Deferred (no windowed aggregator) | M7 |
| Vim mode / command palette autocomplete / theme picker / syntax highlight / structured diff / full ANSI parser / mouse mode | Off | M7/M8 |

### Version bump

All 42 `Cargo.toml` files: `0.6.0 → 0.7.0`. New entry:
`lingxi-code/crates/tui/Cargo.toml` (introduced by M6-01).

## [0.6.0] — M5 Execution Engine 全集

Lands the **complete execution engine**: a `ConversationOrchestrator`
batched turn loop, streaming SSE, interactive permission gate, Hooks
4-arm runtime (Builtin/Http/Command/Agent), byte-equivalent session JSONL,
`--resume` session loading, 18 implemented slash commands (out of 99
registered), `lingxi-cli` binary, stdio REPL mode, and 4 cross-cutting
parity fixtures locking the v0.6.0 surface. 14 sub-plans M5-01..M5-14
delivered these components.

### Major components (M5 milestone)

- **M5-01** — Engine wiring close-out: runner pump + TaskOutput spool +
  RegistryToolInvoker connected end-to-end.
- **M5-02** — `ConversationOrchestrator` core: batched turn loop
  (`run_turn` / `run_turn_with_cancel`), `TurnOutcome` enum, new
  `lingxi-orchestrator` crate.
- **M5-03** — System-prompt dynamic assembly: env block + memory block +
  tools block + `SystemPromptAssembler`.
- **M5-04** — Streaming SSE: mid-stream text deltas + tool-dispatch
  + `tengu_orchestrator_turn_streaming_{started,completed}` events.
- **M5-05** — Permission gate UX: y/N stderr prompts + `NoOpPermissionGate`
  + `tengu_orchestrator_permission_{prompted,answered}` events.
- **M5-06** — Hooks 4-arm runtime: Builtin (in-process), Http (SSRF-guarded
  POST), Command (deferred stub), Agent (subagent spawner). 8 hook events.
- **M5-07** — Session JSONL byte-equivalent: djb2 hash + UUID v4 lowercase +
  `JsonlWriter`/`JsonlReader` + 3 golden session fixtures.
- **M5-08** — `--resume` session loading: chain validate + interactive
  picker + `tengu_session_resume_{started,completed}` events.
- **M5-09** — 99 slash commands surface: `BUILTIN_COMMAND_NAMES` const +
  `register_all_builtin_commands` + `RegistrySlashDispatcher` + stub
  literal `"{name}: not implemented in v0.6.0 (M5)"`.
- **M5-10** — Commands batch 1: `/clear /compact /help /exit /memory /init`
  (6 commands, 18 telemetry events).
- **M5-11** — Commands batch 2: `/cost /config /model /permissions /mcp
  /hooks /agents /login /logout /version /status /doctor`
  (12 commands, 36 telemetry events).
- **M5-12** — `lingxi-cli` binary: clap argv + one-shot dispatch + `--resume`
  + `--json` flags.
- **M5-13** — Stdio REPL mode: read-line loop + SIGINT + EOF + 2 REPL
  lifecycle events.
- **M5-14** — Cross-cutting parity + release: 4 parity drivers
  (`parity_orchestrator.rs`, `parity_slash_commands.rs` extension,
  `parity_session_jsonl.rs`, `parity_hooks_runtime.rs`), version bump,
  CHANGELOG/README/release docs, annotated tags.

### Cross-cutting locks (M5-14)

- **Turn loop**: `run_turn("prompt") -> ConversationOutcome::EndTurn` for
  single-turn; `run_turn(...)` with `max_turns=0` produces
  `OrchestratorError::MaxTurnsReached`; `run_turn_with_cancel` with
  pre-cancelled token returns `Ok(TurnOutcome::Cancelled)`.
- **Slash commands**: 99 commands registered via
  `register_all_builtin_commands`; 18 implemented (batch 1 + batch 2);
  81 stubs return `"{name}: not implemented in v0.6.0 (M5)"`.
- **Session JSONL**: `JsonlWriter` produces LF-only, compact-JSON lines;
  user line has `userType: "external"`, `isSidechain: false`;
  `parentUuid` chain is user→assistant.
- **Hooks matrix**: Builtin arm fires handlers; Http arm applies SSRF guard
  before dispatch; Command arm is a documented stub; Agent arm returns error
  without spawner.
- **Telemetry**: `ALL_EVENT_NAMES.len() == 315` including 1 new release
  marker `lingxi_core_v0_6_0_released` (emitted on first `Engine::init()`
  after upgrade).

## [0.5.0] — M4 Tools 全集

Lands the **40-tool** parity surface: every concrete `Tool` implementation
from claude-code's `src/tools/` directory is now a Rust `impl Tool` with
byte-aligned input/output schemas, byte-aligned error strings, hermetic
test coverage via M1 trait injection, and a `tengu_tool_*` event triple
(`started`/`completed`/`failed`). 8 sub-plans M4-01..M4-08 delivered the
tool bodies; M4-09 ships the cross-cutting parity fixtures, version bump,
docs, and release tag.

### Tools delivered (40 total, 9 categories)

- **File ops (5, M4-01)** — `Read`, `Write`, `Edit`, `NotebookEdit`,
  `Glob`. `MAX_FILE_READ_SIZE = 262_144` bytes, binary detection via
  first-8KB NUL-byte scan, 1-based line indexing.
- **Search (1, M4-01)** — `Grep` over ripgrep with 100-match-per-file cap.
- **Shell (4, M4-02)** — `Bash`, `PowerShell`, `REPL`, `Sleep`. All gated
  through `lingxi_sandbox::Sandbox` per M2-04 lock.
- **Web (2, M4-03)** — `WebFetch` (5 MB cap, follows up to 3 redirects),
  `WebSearch` (via `lingxi-api-client`'s search endpoint).
- **Workflow (5, M4-04)** — `TodoWrite`, `EnterPlanMode`, `ExitPlanMode`,
  `EnterWorktree`, `ExitWorktree`.
- **Agent + Task (8, M4-05)** — `Agent` (recursive subagent dispatch),
  `TaskCreate`, `TaskGet`, `TaskList`, `TaskUpdate`, `TaskStop`,
  `TaskOutput`, `SendMessage`. Task IDs use the 9-char
  `[bartwmd][0-9a-z]{8}` format.
- **Team (2, M4-06)** — `TeamCreate`, `TeamDelete`.
- **MCP + LSP (5, M4-07)** — `MCP`, `McpAuth`, `ListMcpResources`,
  `ReadMcpResource`, `LSP`. Tool full-name prefix `mcp__<server>__<tool>`
  preserved for federated MCP servers.
- **System (8, M4-08)** — `AskUserQuestion` (4-option cap, 60-char
  labels, 200-char question), `Brief`, `Config` (4-field allowlist:
  `model`/`outputStyle`/`theme`/`verbose`), `Skill`, `ScheduleCron`
  (5-field cron), `ToolSearch` (top-20 token-overlap), `RemoteTrigger`
  (local stub), `SyntheticOutput`.

### Cross-cutting locks (M4-09)

- **Registry cardinality**: `register_all_builtin_tools(reg, ctx)`
  produces exactly 40 tools. Asserted by
  `crates/test-harness/src/parity/fixtures/registry_40_tools.json` +
  `parity_registry.rs`.
- **Telemetry coverage**: every of the 40 tools has its 3 lifecycle
  events (`tengu_tool_<snake>_{started,completed,failed}`) registered.
  134 `tengu_tool_*` events total in `ALL_EVENT_NAMES` (M3-06 baseline +
  M4-02..08 deltas). Asserted by `parity_telemetry_coverage.rs`. Event
  suffix is `_completed` for tool events (locked at M3-06; NOT
  `_succeeded`).
- **Output truncation**: every tool with variable-length output routes
  through `lingxi_tools::shared::truncate` with
  `MAX_TOOL_OUTPUT_LENGTH = 30_000` chars and the truncation suffix
  `"\n\n[Output truncated due to length]"`. Asserted by
  `parity_output_truncation.rs`; 4 tools opted out via doc-comment
  markers (`mcp`, `lsp`, `ask_user_question`, `config` — bounded output
  by construction).
- **Permission decision surface**: every tool file declares a
  `PermissionResult` variant (`Allow` / `Deny` / `Ask`) in its
  `permission_required` body. Default gate `AllowAllGate` (M4-01); real
  `DenyAllGate`-style runtime denial is owned by `lingxi_permission`'s
  own `policy::tests` suite. Asserted by `parity_permission_denial.rs`.
- **Release marker**: new top-level telemetry event
  `lingxi_core_v0_5_0_released` registered in
  `lingxi_telemetry::tengu::release::NAMES` (1 entry). Wires up in M5
  alongside `Engine::init()`. `ALL_EVENT_NAMES` count: 237 → 238.

### Crates expanded

- `lingxi-tools` — 29 builtin tool source files under `src/builtin/`
  hosting all 40 `impl Tool` blocks (some files host multiple tools,
  e.g. `task.rs` hosts the 6 task tools, `plan_mode.rs` hosts Enter+Exit,
  `mcp.rs` hosts MCP+McpAuth+ListMcpResources+ReadMcpResource). The
  `BuiltinToolContext` struct grew across M4-01..M4-08 to expose every
  trait-injected dependency the tool bodies need.
- `lingxi-telemetry` — new `tengu/release.rs` sub-module holding the
  release-marker constant. `ALL_EVENT_NAMES` grew by 1.
- `lingxi-test-harness` — 4 new cross-cutting parity drivers in
  `tests/`: `parity_registry.rs`, `parity_telemetry_coverage.rs`,
  `parity_output_truncation.rs`, `parity_permission_denial.rs`. New
  fixture `parity/fixtures/registry_40_tools.json`.

### Workspace

- Version bump 0.1.0 → 0.5.0 across all 39 `Cargo.toml` files
  (38 packages + the `mock_stdio_mcp` fixture sub-crate).
- New gate `tools/scripts/check_version.sh` asserts every Cargo.toml is
  at `0.5.0`.

### Tag

- `m4.9` (M4-09 completion).
- `v0.5.0` (release).

Both annotated, both lowercase, both on the verification-pass commit.

---

## [0.4.0] — M3 Engine Completion

Locks in claude-code's engine surface — Settings, Memory, real API client,
OAuth refresh, cost events, telemetry schema — at 1:1 byte-aligned parity
with claude-code upstream commit `6a25909` (2026-05-23). 8-10-week single-
developer sustained-Rust delivery per spec §9.

### Crates added

- `lingxi-telemetry-macros` — new sibling proc-macro crate. Ships the
  `tengu_event_audit!()` macro which walks `lingxi-telemetry::tengu/*.rs`
  at compile time and emits `compile_error!()` if any payload struct uses
  bare `String` (must be `Verified` or `PiiTagged`), omits
  `#[serde(deny_unknown_fields)]`, or any payload enum omits
  `#[non_exhaustive]`. Per spec §7 Event evolution policy (lines 781-789).

### Crates expanded

- `lingxi-core` — new `settings/` module tree: `schema.rs` (full
  `SettingsJson` with `deny_unknown_fields`), `env_parser.rs` (3-prefix
  priority `LINGXI_*` > `CLAUDE_CODE_*` > `CLAUDE_*`), `loader.rs`
  (4-layer: env > user > project > defaults), `merger.rs` (per-field
  array/object merge dispatcher), `tracer.rs` (provenance per field),
  and three `tengu_settings_*` events.
- `lingxi-memory` — new `claude_md/` and `memdir/` sub-modules. Walks the
  CLAUDE.md / CLAUDE.local.md hierarchy bottom-up with a 10 MB cap per
  file. Memdir scan applies a 365-day hard-drop threshold then ranks by
  the `score_bps: u64` product (jaccard × age weight × tier weight × team
  boost) — fixed-point `u64` basis points throughout, no `f64` in the
  scoring path. `secret_scan.rs` adapts the existing v3 §16.5
  `lingxi_secret::SecretScanner` (gitleaks rule reuse — no duplicate rule
  set). `tengu_agent_memory_loaded` + `tengu_memory_secret_redacted` emit
  through M3-06's schema.
- `lingxi-api-client` — non-streaming `messages.create` + `count_tokens`
  endpoints, retry middleware (3 attempts at 500ms / 1s / 2s ± 20% jitter
  for thundering-herd mitigation), `Retry-After` + `anthropic-ratelimit-
  requests-reset` aware rate-limit handling, frozen `OAuthRefreshHook`
  trait surface (M3-04 implements; this crate never re-modifies the
  trait), and `BetaHeaderRegistry` emitting only the headers relevant to
  the current request kind (16 locked `anthropic-beta` constants from
  claude-code @ 6a25909, per-provider × per-endpoint applicability).
  Bedrock extra-params route + Vertex `count_tokens` 3-constant allowlist
  captured verbatim.
- `lingxi-anthropic-oauth` — concrete `RefreshDriver` implementing
  M3-03's `OAuthRefreshHook`. Reactive 401 refresh and proactive task
  share a single `refresh_lock: Arc<tokio::sync::Mutex<()>>` with
  double-check-after-acquire; loom test in `refresh_single_flight_test.rs`
  (v3 §32.7 hotspot) verifies concurrent paths collapse to one HTTP
  refresh. Proactive wake interval `min(remaining/2, 5 min)` handles
  short-lived (< 5 min TTL) tokens. 403-with-`required_scopes` re-runs
  PKCE preserving the existing `refresh_token`. Endpoints HTTPS-pinned:
  authorize `https://claude.ai/oauth/authorize`, token
  `https://console.anthropic.com/v1/oauth/token`. Five tengu_oauth_*
  events including `_proactive_canceled` on `Engine::shutdown`.
- `lingxi-cost` — `events.rs` emits `tengu_cost_recorded` /
  `tengu_cost_budget_warning` / `tengu_cost_budget_exceeded` and the
  four `tengu_api_*` events (started / succeeded / failed / rate_limited).
  `is_batch_request: bool` reserved in the `tengu_cost_recorded` payload
  for forward compatibility with M4's Batch endpoint; always `false` in
  v0.4.0. No 50% batch discount in M3 (arrives in M4 alongside the
  endpoint).
- `lingxi-telemetry` — new `tengu/` module tree with 8 sub-modules
  (`api`, `agent`, `session`, `tool`, `cost`, `oauth`, `memory`,
  `settings`) declaring 143 events as the single authoritative source.
  Every payload struct `#[serde(deny_unknown_fields)]`, every payload
  enum `#[non_exhaustive]`, every user-derived string field
  `Verified` / `PiiTagged` (NOT bare `String`). Three new sinks:
  `NoOpSink` (default, no network), `InMemorySink` (test capture),
  `StatsigSink` trait + `MockStatsigSink` skeleton with statsig wire
  shape `{event_name, value, metadata}`.

### 1:1 parity guarantees locked (v0.4.0 additions on top of v0.3.0)

- **Settings**: file paths `~/.claude/settings.json` + `<repo>/.claude/
  settings.json`. 4-layer priority `env > user > project > defaults`. Env
  prefix priority `LINGXI_*` > `CLAUDE_CODE_*` > `CLAUDE_*`. Array-merge
  fields `trustedDirectories`, `additionalDirectories`, `enabledTools`,
  `additionalIncludes`. Object-merge fields `sandbox`, `hooks`,
  `outputStyle`. `"$schema"` not emitted (claude-code does not emit it
  either; reader tolerates for forward compat).
- **Memory**: `CLAUDE.md` (case-sensitive), `CLAUDE.local.md`,
  `~/.claude/memdir/`, `~/.claude/team-mem/`. `MAX_MEMORY_FILE_SIZE = 10 *
  1024 * 1024`. `MEMORY_AGE_PENALTY_DAYS = 30` (relevance penalty unit,
  NOT a drop threshold). `MEMORY_AGE_HARD_DROP_DAYS = 365` (scan-time
  hygiene drop). `MEMORY_MIN_AGE_WEIGHT_BPS = 1_000` (even very old
  entries stay reachable at 10% weight). `DEFAULT_RELEVANT_MEMORIES = 5`.
  Scoring is fixed-point u64 (basis points) — NOT `f64`, cross-platform
  deterministic per §4 Flow C.
- **API client**: base URL `https://api.anthropic.com`, version header
  `anthropic-version: 2023-06-01`, User-Agent
  `claude-cli/<CARGO_PKG_VERSION> (external, cli)`, retry budget 3 with
  exponential backoff 500ms / 1s / 2s ± 20% jitter, streaming timeout 600s,
  `messages.create` timeout 120s, `count_tokens` timeout 30s. Rate-limit
  error string `"Rate limited; retrying in {N}s"`. 16 `anthropic-beta`
  constants locked verbatim from claude-code @ 6a25909.
- **OAuth**: authorize endpoint `https://claude.ai/oauth/authorize`,
  token endpoint `https://console.anthropic.com/v1/oauth/token`, OAuth
  beta header value `oauth-2025-04-20`, refresh grant_type
  `refresh_token`, PKCE method `S256`, 256-bit CSPRNG state token,
  loopback redirect template `http://127.0.0.1:{port}/callback`, 5-minute
  login flow deadline, three scopes `read:user` / `write:messages` /
  `read:projects`. Proactive refresh lead `min(remaining/2, 5 * 60)`
  seconds. Single-flight via `refresh_lock: Arc<tokio::sync::Mutex<()>>`
  per v3 §16.3. 401 retry policy: retry ONCE after refresh.
- **Cost events**: `tengu_cost_recorded` payload fields
  `model: Verified`, `input_tokens: u64`, `output_tokens: u64`,
  `cache_read_input_tokens: u64`, `cache_creation_input_tokens: u64`,
  `cost_usd: u64` (nano-USD per v3 §17), `session_id: Verified`,
  `is_batch_request: bool` (reserved for M4; always false in M3).
  `tengu_cost_budget_warning` uses `percent_bps: u64` (basis points,
  fixed-point per §4 Flow C; M3-06's BudgetWarningPayload locks the type).
- **Telemetry**: ~200 event names organized into 8 modules with locked
  per-category counts: api=25, agent=30, session=15, tool=40, cost=10,
  oauth=8, memory=12, settings=3 (= 143 explicit; ~55 incremental from
  M2-touched subsystems). Statsig wire shape `{event_name, value,
  metadata}` per `claude-code/src/services/statsig.ts`. All payload
  strings `Verified` / `PiiTagged`; `strip_proto_fields` runs at
  every general-access sink. Event-name list is append-only; field
  additions to existing events use sibling-v2 names (`tengu_<name>_v2`)
  over a 2-minor-release deprecation cycle.

### Tests + verification

- Workspace test count: ~700 functional tests + ~24 non-functional gates
  (loom / fuzz / criterion / chaos) per spec §6. Up from 488 at v0.3.0
  (per master spec §6 line 590; M2 final test count); M3 adds ~200-300.
  Net add: ~150 unit + 12 contract drivers + ~25 integration + 6 parity
  + 8 loom + 4 fuzz + 6 criterion + 6 chaos.
- 16 parity drivers gate on every PR: 7 inherited from M2 (M2-07) plus 9
  new from M3 (`parity_settings_merge`, `parity_memory_loading`,
  `parity_memory_relevance`, `parity_messages_create`, `parity_betas`,
  `parity_oauth_pkce_refresh`, `parity_cost_events`, `parity_tengu_events`,
  `parity_full_v0_4_0_smoke`).
- New CI workflows: `ci-loom.yml` (weekly Monday 06:00 UTC),
  `ci-fuzz.yml` (daily 07:00 UTC, continue-on-error: true for v0.4.0),
  `ci-bench.yml` (weekly Monday 08:00 UTC, regression check
  continue-on-error initially), `ci-chaos.yml` (weekly Monday 09:00 UTC,
  hard gate).
- `ci.yml` gains `cross-compile-musl` (v3 §32.4 Layer 5 — `cargo check`
  against `x86_64-unknown-linux-musl`), `supply-chain` (`cargo deny` +
  `cargo audit` + `cargo vet` per v3 §32.4 Layers 1-3), and
  `parity-fixtures` (all 16 `parity_*` drivers).
- `cargo test --workspace` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all --check` clean.
- Existing `cross-compile-desktop` (x86_64-unknown-linux-gnu, aarch64-
  apple-darwin, x86_64-pc-windows-msvc) and `cross-compile-mobile`
  (aarch64-linux-android, aarch64-apple-ios — informational only) jobs
  preserved unchanged from M2-07.

### Known deferrals carried forward to M4+

- **`/v1/messages/batches` endpoint + 50% batch discount** — M4
  alongside the Batch API. The `is_batch_request: bool` field in
  `tengu_cost_recorded` is reserved for that work.
- **Cross-device token sync via claude.ai** — Out of M3. Would land in
  M6 if needed.
- **Statsig HTTP endpoint wiring** — `StatsigSink` trait + `MockStatsigSink`
  skeleton ship in M3-06; real HTTP client + retry remain consumer
  responsibility (M6 task if a real Statsig SDK key becomes available).
- **Embedding-based memory relevance** — M3-02 ships the keyword + age +
  tier heuristic. claude-code may use embeddings; if so, M3.5 or M4 can
  swap to embeddings without breaking the public `MemoryProvider` trait.
- **Anthropic SDK `files` / `models` / `organizations` endpoints** — Not
  in claude-code's usage; not in M3. Land in a separate plan if needed.
- **cargo-fuzz hard-gate** — Currently `continue-on-error: true` in
  `ci-fuzz.yml`. Flip to hard-gate at v0.5.0+.
- **cargo-bench regression baseline** — Currently informational. Baseline
  tooling + `benches/baselines/v0_4_0.json` audit data ship in v0.5.0.
- **cargo-vet supply-chain audit data** — `supply-chain/` audit directory
  is an M4 deliverable; v0.4.0 ships the workflow scaffold only.

### Migration from v0.3.0

The following surfaces changed in source-incompatible ways. Downstream
users of `lingxi-core` as a library MUST update accordingly:

- **`lingxi-telemetry::tengu`** is a new top-level module tree. Code that
  emits events through `AnalyticsBus::log_event` should now reference the
  typed event names from `lingxi_telemetry::tengu::<category>` instead of
  hand-rolled `&'static str`s. Existing string-based call sites still
  compile, but the audit proc-macro will flag any new payload that
  bypasses the typed schema.
- **`lingxi-api-client::OAuthRefreshHook`** is a new trait. Downstream
  consumers that want to participate in 401-driven refresh must implement
  this trait and register via `register_oauth_hook(...)`. The trait is
  frozen — M3-04's `RefreshDriver` is the canonical impl; future
  consumers should compose, not modify.
- **`lingxi-memory` API shape**: the public `MemoryProvider` trait gains
  `find_relevant_memories(query, k) -> Vec<MemoryEntry>` and
  `load_claude_md_hierarchy(repo_root) -> Vec<MemoryEntry>`. Existing
  callers of the M2 shape see `non_exhaustive` warnings.
- **`lingxi-core::settings`** is a new module. The 4-layer loader
  (`Settings::load(LoadInputs)`) replaces any ad-hoc settings reading.
  Downstream code that read settings via direct `serde_json::from_str`
  on `.claude/settings.json` should switch to the loader so it picks up
  the env-var and project-layer merges automatically.

## [0.3.0] — M2 claude-code Behavioral Parity

### Crates added
- `lingxi-jsonrpc` — JSON-RPC 2.0 framing shared by MCP and LSP. Supports
  Content-Length-prefixed (LSP / modern MCP) and line-delimited (older MCP)
  framing with auto-detect, outbound request router with timeout + drop-cancel,
  inbound request router (for `roots/list`, `elicitation/create`), and a
  notification broker.

### Crates expanded
- `lingxi-sandbox` — full `SandboxRuntimeConfig` schema (matches claude-code
  `entrypoints/sandboxTypes.ts` field-for-field), `convert_settings_to_runtime_config`,
  `dependency_check`, `violation_store`, and `wrap_with_sandbox` dispatcher
  (macOS `sandbox-exec` SBPL profile, Linux `bwrap+socat`, Windows/WSL1 Unsupported).
- `lingxi-mcp` — real client over `lingxi-jsonrpc` covering `initialize`, `list_tools`,
  `call_tool` (with timeout error string `"MCP server \"...\" tool \"...\" timed out
  after Ns"`), `list_resources`, `list_prompts`, `read_resource`, `ping`, plus
  inbound `roots/list` and `elicitation/create` handlers. Identity locked:
  `name="claude-code"`, `title="Claude Code"`, capabilities `{"roots":{}, "elicitation":{}}`.
- `lingxi-lsp` — real client over `lingxi-jsonrpc` with 9 tool operations
  (`goToDefinition`, `findReferences`, `hover`, `documentSymbol`, `workspaceSymbol`,
  `goToImplementation`, `prepareCallHierarchy`, `incomingCalls`, `outgoingCalls`),
  1-based ↔ 0-based line/character translation, `textDocument/didOpen` registry,
  `textDocument/publishDiagnostics` accumulation, 10 MB `MAX_LSP_FILE_SIZE_BYTES`
  cap, and plugin-only `register_config` (visibility narrowed to `pub(crate)`).
- `lingxi-bridge` — lockfile-based local IDE bridge. Reads `~/.claude/ide/<port>.lock`,
  builds an MCP-over-WebSocket transport spec with header
  `X-Claude-Code-Ide-Authorization`. The 8-char pairing protocol and
  project-scoped JWT machinery from v0.2.0 were removed (they had no claude-code
  counterpart). Cloud Remote Control bridge remains out of scope.
- `lingxi-platform-posix` — real impls land for `Sandbox` (macOS + Linux + WSL2),
  `McpTransport` (stdio + sse + http + ws), `LspTransport`, `SwarmBackend`
  (tmux + iTerm + InProcess fallback), `FileSystem::watch` (notify + debounce),
  `HttpTransport::stream_sse`, `ProcessRunner::spawn_background` + `kill_tree`
  + `pwd -P` cwd tracking, `SecureStorage` (macOS Keychain via `security` CLI
  + plaintext fallback).
- `lingxi-platform-windows` — `Sandbox` and `SwarmBackend` explicitly return
  `Unsupported` (claude-code does not support sandbox or tmux on Windows).
  `FileSystem::watch` switches to `notify`'s `ReadDirectoryChangesW` path.
  `SecureStorage` remains plaintext (Windows Credential Vault deferred).

### 1:1 parity guarantees locked
- Worktree branch prefix: `worktree-` (was `lingxi/` in v0.2.0). Slug flattening
  `/` → `+`. Path: `<repo_root>/.claude/worktrees/<flattened-slug>`.
- MCP client identity: `name="claude-code"`, `title="Claude Code"`,
  `websiteUrl="https://claude.com/claude-code"`, capabilities
  `{"roots":{}, "elicitation":{}}` (empty objects, not null).
- IDE WebSocket auth header: `X-Claude-Code-Ide-Authorization` (literal).
- macOS Keychain service name format: `Claude Code{oauth_suffix}-credentials{dir_hash}`.
- LSP file size cap: `MAX_LSP_FILE_SIZE_BYTES = 10_000_000` (10 MB).
- Sandbox WSL1 refusal: `"sandbox.enabled is set but WSL1 is not supported (requires WSL2)"`.
- Sandbox unsupported-platform: `"sandbox.enabled is set but ${platform} is not supported (requires macOS, Linux, or WSL2)"`.
- Windows tmux refusal: `"--tmux is not supported on Windows"`.
- LSP `register_config` is `pub(crate)` — only plugins can register LSP servers.
- MCP tool name format: `mcp__<server>__<tool>`.

### Known deferrals carried forward to M3+
- Cloud Remote Control bridge (claude.ai worker integration, ~14k TS lines).
- In-process MCP transports (computer-use, Chrome) — depend on a separate
  computer-use server crate.
- Linux SecureStorage native backend (libsecret) — plaintext fallback only,
  matching claude-code's TODO.
- Android / iOS platform crates — M3 milestone.
- Plugin marketplace UI and `.mcpb` bundle installer — M4 UI Layer.
- Web pty-server — M4 UI Layer.
- macOS SBPL profile fidelity beyond the M2 template — separate research task.

### Migration from v0.2.0

The following surfaces changed in source-incompatible ways. Downstream users
of `lingxi-core` as a library MUST update accordingly:

- **Worktree branch prefix:** existing v0.2.0 worktrees with `lingxi/<slug>`
  branches are not recognized by v0.3.0 cleanup. Run `git worktree remove`
  manually for any orphan v0.2.0 worktree before upgrading.
- **Worktree path:** `WorktreeManager::create_worktree` parameter renamed from
  `worktree_base` to `repo_root`. The path is now hardcoded to
  `<repo_root>/.claude/worktrees/<flattened-slug>`.
- **`lingxi-bridge` API:** the 9-variant `BridgeMessage` enum, `BridgeCode`,
  `JwtVerifier`, `RateLimiter`, and `PairingManager` types were removed.
  `IdeBridge` now exposes only `connect()` + `disconnect()`; transport details
  live in `lingxi-mcp`.
- **`crates/bridge` dependencies:** `rand`, `sha2`, `hmac`, `base64` removed
  from `Cargo.toml`. Add `lingxi-mcp` dependency.
- **`lingxi-lsp::LspRegistry::register_config`** is now `pub(crate)`. External
  callers must register LSP servers through `crates/plugin`'s
  `register_plugin_servers` path.
- **`ProcessRunner::spawn_background`** previously returned `Unsupported` on all
  platforms; now returns a real `ProcessHandle` on posix/windows.
- **`api-client::types::StreamEvent`** gained new variants (`Thinking`,
  `SignatureDelta`, `CitationsDelta`, `ConnectorTextDelta`, etc.). Existing
  match arms over `StreamEvent` will hit `non_exhaustive` warnings — add a
  catch-all or update arms.

### Tests + verification
- Workspace test count: ~145 (v0.2.0 baseline 104 + 12 contract suites + 7
  parity fixtures + per-plan tests from M2-01..M2-06).
- `cargo test --workspace` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all --check` clean.
- `cargo check -p lingxi-platform-posix --no-default-features` clean.
- `cargo check -p lingxi-platform-windows --no-default-features` clean.
- Desktop cross-compile matrix (`x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`,
  `x86_64-pc-windows-msvc`) green. Android/iOS targets are informational only
  for v0.3.0 (M3 scope).

## [0.2.0] — M2 Production Desktop Platforms

### Crates shipped
- `lingxi-platform-posix` (Linux + macOS) — real impls for Clock, FileSystem (with inotify watch on Linux), HTTP (reqwest), Process (tokio::process), Runtime (tokio::spawn), SecureStorage (plain-text file fallback), Worktree (git CLI). Sandbox, MCP stdio, LSP, Swarm, Bridge ship as type-correct stubs.
- `lingxi-platform-windows` — mirrors posix with Windows-friendly fallbacks. Same stubbed surfaces; uses fs2 cross-platform locking instead of LockFileEx directly; symlink uses `tokio::fs::symlink_file` under cfg(windows).

### Platform support
- Linux/macOS/Windows: production-ready for most engine workloads; demo cli-demo still wires posix-minimal.
- Android/iOS: unchanged from M1 — cross-compile only, M3 work.

### Known deferred (M2-followup TODOs in code)
- Real OS sandbox isolation (Linux user namespaces, macOS sandbox-exec, Windows Job Objects).
- Full JSON-RPC framing for MCP stdio and LSP (request id tracking, Content-Length headers, notification streaming).
- FSEvents (macOS) and ReadDirectoryChangesW (Windows) for filesystem watch.
- tmux/Windows Terminal CLI driver for SwarmBackend.
- WebSocket BridgeTransport.
- Native SecureStorage backends (libsecret, macOS Keychain, Windows Credential Vault) — current PlainTextFile fallback is functional but not encrypted.
- `http.stream_sse` — non-trivial but needed for live API streaming; currently returns InvalidRequest.

## [0.1.0] — M1 Foundation Release

### Crates shipped
- protocol, core, traits, api-client (Plan 01)
- permission, secret, cost (Plan 02)
- tools, hooks (Plan 03)
- memory, mcp (Plan 04)
- compaction (Plan 05)
- agent (Plan 06)
- tasks, coordinator (Plan 07)
- sidequery (Plan 08)
- skills, commands, outputstyles (Plan 09)
- session, filestate, msgqueue (Plan 10)
- cron (Plan 11)
- sandbox, lsp (Plan 12)
- telemetry, anthropic-oauth (Plan 13)
- bridge (Plan 14)
- plugin (Plan 15)
- uniffi-bridge, platforms/posix-minimal, examples/cli-demo (Plan 16)
- test-harness (Plan 17)

### Platform support
- Linux/macOS/Windows: runnable via cli-demo + posix-minimal
- Android/iOS: cross-compile gate only; production platform crates land in M3

### Tests + verification (Plan 17)
- 104 tests pass under `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo fmt --all --check` clean
- Filesystem trait contract suite + posix-minimal driver
- `10k-iterations` Cargo feature flag plumbed on test-harness (CI wiring in M2)

### Deferred to M2
- Contract suites for the remaining 12 traits (process, http, mcp, worktree,
  swarm, secure_storage, sandbox, lsp, bridge, runtime, clock,
  hook_broadcaster) — pattern seeded in Plan 17, replication across traits
  is mechanical.
- Property tests at 10K iterations across all 11 property domains — feature
  flag is in place, individual suites read it during the M2 expansion.
- Parity fixture recordings (12 scenarios from claude-code reference) and
  per-scenario driver tests — namespace scaffold lands here.
- Contract coverage CLI + CI gate (`ratio ≤ 0.05`) — written into the
  M2 plan; trait method registry is small enough to maintain by hand
  until then.

### Tag
- `v0.1.0` — M1 v0.1.0, desktop-runnable, mobile cross-compile only.
