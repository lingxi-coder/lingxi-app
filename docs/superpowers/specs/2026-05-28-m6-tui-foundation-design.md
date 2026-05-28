# M6 TUI Foundation — Design

**Status:** Draft, awaiting user review (2026-05-28)
**Target release:** v0.7.0
**Predecessor:** v0.6.0 (M5 — Conversational Agent Loop)
**Successor (planned):** v0.8.0 (M7 — TUI Surface) → v0.9.0 (M8 — TUI Advanced) → v1.0.0
**Author:** luolingfeng + Claude Opus 4.7

---

## §0 Decisions Locked (from brainstorm)

| # | Question | Answer |
|---|---|---|
| Q1 | M6 scope vs claude-code's UI surface | **All 144 components** across M6+M7+M8 (M6 covers the foundational ~30) |
| Q2 | Engine wiring strategy | **Tiered (D)** — wire daily-driver engine (cost, MCP/Hooks/Agents listings, force_compact); team/coordinator/voice/grove render correct empty-state placeholders (true 1:1 with how claude-code renders unconfigured features) |
| Q3 | Milestone shape | **Split M6/M7/M8** — M6 = Foundation, M7 = Surface, M8 = Advanced. Single M6 would have been 30-40 sub-plans. |
| Q4 | TUI library | **iocraft** (React/Ink-style retained-mode for Rust). Ratatui is the documented fallback if iocraft hits a blocker at the M6-01 gate. claw-code does NOT use a real TUI (styled stdio only), so it's not a precedent. Codex uses ratatui — kept as escape hatch. |
| Q5 | Integration with `lingxi-cli` | **A** — TUI replaces stdio REPL as default. `lingxi-cli` (no args, TTY) → TUI. `-p` unchanged. `--no-tui` falls back to v0.6.0 stdio REPL. Refactor: split `lingxi-cli` into `lingxi-cli` (binary, argv/dispatch) + `lingxi-tui` (library, UI). |
| Q6 | Validation approach | **C** — Hybrid: insta snapshots for pure renderers, behavior tests for interactive flows, parity fixtures for cross-cutting invariants. |
| Q7 | Decomposition shape | **Approach 3** — Sequenced for visible progress. Working TUI by M6-02, engine wiring concentrated in M6-06..M6-08 once the UI shape reveals data needs. Mirrors M5's rhythm. |

---

## §1 Goal & Non-Goals

### Goal

**v0.7.0 is the first version where `lingxi-cli` looks and feels like `claude`.** The line-based REPL from v0.6.0 becomes the `--no-tui` fallback; the default experience is a fullscreen iocraft TUI that visually and behaviorally matches claude-code's Ink REPL screen.

Concrete deliverables — must all ship for v0.7.0:

1. `lingxi-cli` (no args, TTY detected) → enters fullscreen TUI with StatusLine, message scrollback, PromptInput, and SpinnerWithVerb during streaming.
2. `lingxi-cli -p "fix bug X"` → unchanged from v0.6.0 (plain stdout, no TUI).
3. `lingxi-cli --no-tui` → preserved v0.6.0 stdio REPL (CI / accessibility / dumb-terminal fallback).
4. End-to-end flow: type a prompt → see assistant text stream in token-by-token → see tool use rendered as collapsible block → see permission dialog → approve/deny → see tool result → return to PromptInput. All without leaving the TUI.
5. ~30 components ship, behaviorally matching claude-code's Ink equivalents:
   - 4 message renderers (UserText, AssistantText, AssistantToolUse, UserToolResult)
   - PromptInput (line editing, history nav — no vim mode in M6)
   - StatusLine (model, cwd, cost, context%, permission mode)
   - SpinnerWithVerb + streaming integration
   - 3 permission dialogs (ToolUseConfirm, ExitPlanMode, BypassPermissionsMode)
   - REPL screen shell + layout primitives + scrollback navigation (j/k/g/G/PgUp/PgDn)
6. Tier-1 engine wiring lands:
   - Cost path real (StatusLine shows actual `$0.x`, not zeros)
   - MCP / Hooks / Agents registries plumbed into `OrchestratorHandle::list_*`
   - `force_compact` calls real `lingxi_compaction::Compactor::compact()`
7. Test surface: ~15 insta snapshots, ~30 behavior tests, ~5 PTY integration tests, 2 new parity fixtures (`parity_tui_renderers`, `parity_tui_repl_loop`).
8. Workspace version 0.6.0 → 0.7.0; annotated tags `m6.9` + `v0.7.0`.
9. Cross-platform compile check (macOS/Linux/Windows desktop; Android/iOS mobile compile-only as before).

### Non-Goals (M6 explicitly does NOT do)

- **Doctor / Resume / Settings / Memory editor full-page screens** → M7. M6 keeps Resume as the stdio picker from M5-08.
- **PromptInput advanced features**: vim mode, command palette (`/` autocomplete UI), `@` file ref completion, image paste, multi-line editor, `ContextSuggestions`, `QuickOpenDialog` → M7.
- **VirtualMessageList** (windowed scrollback for 10k+ messages) → M7. M6 uses a capped buffer (last 500 messages).
- **Advanced message renderers** (AdvisorMessage, AssistantThinkingMessage, HookProgressMessage, CompactBoundaryMessage, RateLimitMessage, TaskAssignmentMessage, ~24 more) → M7.
- **Search / transcript / export UI** → M7.
- **Theme picker, syntax-highlighted code blocks, structured diff viewer** → M7. M6 uses fixed colors.
- **Coordinator / Team / Swarm UI / Voice / `grove` / FPS metrics / Anthropic-internal banners / IDE bridge dialogs** → M8.
- **`MessageSelector` (transcript jump-back)** → M7.
- **Mouse support** → M7+. Keyboard only in M6.
- **Bracketed paste / multi-line paste handling beyond default** → M7.
- **OAuth real PKCE flow** → M7. `/login` stays stubbed.

### Success Criteria

v0.7.0 release equivalent to all of these passing:

1. `lingxi-cli` opens a fullscreen TUI on a 80×24+ TTY; quits cleanly on `Ctrl-C` then `Ctrl-D` (or `/exit`).
2. Streaming SSE displays token-by-token in TUI without flicker; spinner shows during in-flight turn.
3. Tool use renders as collapsible block; permission dialog accepts `1`/`2`/`N`/`Esc`/`Enter` matching claude-code.
4. StatusLine displays real cost (after M6-06), correct model name, cwd, context%, permission mode.
5. `/clear /exit /help /cost /model /mcp /hooks /agents /compact` slash commands round-trip through the TUI cleanly.
6. `--no-tui` falls back to v0.6.0 stdio REPL with no regression.
7. `cargo test --workspace` passes; known flakes (3) acceptable.
8. Cross-platform compile gate green for 5 targets.
9. `ALL_EVENT_NAMES.len() == 330` (exact count locked in M6-09).
10. Annotated tag `v0.7.0` exists, points to release commit.

---

## §2 Architecture

### 2.1 Crate layout (post-refactor)

```
lingxi-core/
├── crates/
│   ├── cli/                  ← already exists; M6-01 refactors it
│   │   ├── src/
│   │   │   ├── main.rs          (thin tokio::main entry; dispatches print/repl/tui modes)
│   │   │   ├── argv.rs          (clap; unchanged surface, adds --no-tui detection)
│   │   │   ├── init.rs          (build_runtime; extended to construct CostTracker + registries)
│   │   │   ├── run.rs           (mode dispatch: print vs no_tui_repl vs tui)
│   │   │   ├── sigint.rs        (unchanged)
│   │   │   ├── exit_codes.rs    (unchanged)
│   │   │   ├── repl.rs          ← KEEPS the v0.6.0 stdio REPL (now --no-tui only)
│   │   │   └── repl_loop.rs     ← KEEPS, same role
│   │   └── Cargo.toml           (adds dep on lingxi-tui)
│   │
│   └── tui/                  ← NEW crate (M6-01)
│       ├── src/
│       │   ├── lib.rs           (public API: TuiApp, run_tui_session)
│       │   ├── app.rs           (top-level <App> iocraft component; root state)
│       │   ├── theme.rs         (color palette; fixed in M6, picker in M7)
│       │   ├── layout.rs        (Layout primitives; height computation)
│       │   ├── events/
│       │   │   ├── mod.rs       (TuiEvent enum: Key, Resize, OrchestratorMessage, Tick)
│       │   │   ├── keymap.rs    (matches claude-code's bindings)
│       │   │   └── orchestrator_bridge.rs  (mpsc from ConversationOrchestrator)
│       │   ├── screens/
│       │   │   └── repl.rs      (REPL screen — the only screen in M6)
│       │   ├── components/
│       │   │   ├── status_line.rs
│       │   │   ├── prompt_input.rs       (basic line editing)
│       │   │   ├── spinner.rs            (SpinnerWithVerb + frame ticker)
│       │   │   ├── scrollback.rs         (simple capped message buffer)
│       │   │   ├── messages/
│       │   │   │   ├── mod.rs            (Message renderer dispatch)
│       │   │   │   ├── user_text.rs
│       │   │   │   ├── assistant_text.rs
│       │   │   │   ├── assistant_tool_use.rs
│       │   │   │   └── user_tool_result.rs
│       │   │   └── permissions/
│       │   │       ├── mod.rs
│       │   │       ├── tool_use_confirm.rs
│       │   │       ├── exit_plan_mode.rs
│       │   │       └── bypass_permissions.rs
│       │   ├── streaming.rs     (subscribes to orchestrator stream; pushes deltas into scrollback)
│       │   ├── ansi.rs          (minimal ANSI SGR parser for Bash tool output)
│       │   ├── telemetry.rs     (TUI lifecycle events)
│       │   └── parity/
│       │       └── fixtures/    (tui-specific parity fixtures)
│       ├── tests/
│       │   ├── snapshots/       (insta .snap files)
│       │   ├── render_*.rs      (snapshot tests for stable renderers)
│       │   └── behavior_*.rs    (behavior tests for interactive flows)
│       └── Cargo.toml
```

**Why a library crate** (not a binary):
- `lingxi-cli` stays the only binary in the workspace (clean DAG)
- TUI is unit-testable as a library (no `main.rs` test harness)
- `lingxi-cli` calls `lingxi_tui::run_tui_session(runtime, cancel_token).await?`
- M7/M8 don't add binaries either — they extend `lingxi-tui`

### 2.2 Mode dispatch in `lingxi-cli::main`

```rust
fn main() -> ExitCode {
    let argv = Argv::parse();
    let runtime = init::build_runtime(&argv)?;

    let mode = decide_mode(&argv);
    match mode {
        Mode::Print(prompt) => run::run_oneshot(runtime, prompt),
        Mode::Tui          => lingxi_tui::run_tui_session(runtime, cancel).block_on(),
        Mode::StdioRepl    => repl_loop::run(runtime, cancel).block_on(),  // v0.6.0 path
    }
}

fn decide_mode(a: &Argv) -> Mode {
    if let Some(p) = &a.prompt_or_print {
        return Mode::Print(p.clone());
    }
    if a.no_tui || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Mode::StdioRepl;
    }
    Mode::Tui
}
```

**TTY detection** uses `std::io::IsTerminal` (stable since 1.70). If stdin or stdout isn't a TTY (CI, pipes, heredocs), we fall back to StdioRepl automatically.

### 2.3 Data flow

```
                ┌──────────────────────────────────────────┐
                │ ConversationOrchestrator (lingxi-core)   │
                │ — unchanged from v0.6.0                  │
                └──────────────────────────────────────────┘
                          │                          ▲
                          │ emits stream:            │ feeds user_input:
                          │  TurnStarted             │  Prompt(text)
                          │  TextDelta(s)            │  PermissionResponse(decision)
                          │  ToolUseStart(...)       │  Cancel
                          │  ToolUseResult(...)      │
                          │  PermissionRequest(...)  │
                          │  TurnEnded(outcome)      │
                          ▼                          │
                ┌──────────────────────────────────────────┐
                │ orchestrator_bridge::Channel (mpsc)      │
                └──────────────────────────────────────────┘
                          │                          ▲
                          ▼                          │
                ┌──────────────────────────────────────────┐
                │ TuiApp.state (root iocraft component)    │
                │ — messages: Vec<RenderedMessage>          │
                │ — pending_permission: Option<...>         │
                │ — streaming: Option<{turn_id, partial}>   │
                │ — prompt_text: String                     │
                │ — status: StatusSnapshot                  │
                │ — scroll_offset: usize                    │
                └──────────────────────────────────────────┘
                          │                          ▲
                          ▼                          │
                ┌──────────────────────────────────────────┐
                │ iocraft reconciler → crossterm           │
                │ (renders <App> declaratively each tick)  │
                └──────────────────────────────────────────┘
                          ▲
                          │
                ┌──────────────────────────────────────────┐
                │ Keyboard events (crossterm event loop)   │
                │ → routed by keymap                       │
                │ → dispatched into AppState               │
                └──────────────────────────────────────────┘
```

**Three async sources, one render loop**:
1. **Orchestrator events** — `mpsc::Receiver<TurnEvent>` from ConversationOrchestrator
2. **Keyboard events** — `crossterm::event::EventStream` (async)
3. **Animation ticker** — `tokio::time::interval(100ms)` for spinner frames

Merged via `tokio::select!` inside `lingxi_tui::run_tui_session`. iocraft handles the actual re-render whenever the root component's state changes via `use_state` setters.

### 2.4 iocraft component contract

Every component in `lingxi-tui::components` follows this shape:

```rust
use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct StatusLineProps {
    pub model: String,
    pub cwd: PathBuf,
    pub cost: Money,
    pub context_pct: f32,
    pub permission_mode: PermissionMode,
}

#[component]
pub fn StatusLine(props: &StatusLineProps) -> impl Into<AnyElement<'static>> {
    element! {
        Box(flex_direction: FlexDirection::Row, padding: 1) {
            Text(content: format!("{} ", props.model))
            Text(content: format!("{} ", props.cwd.display()))
            Text(content: format!("${} ", props.cost.format()))
            Text(content: format!("{:.0}%", props.context_pct * 100.0))
        }
    }
}
```

Props mirror the equivalent Ink component's prop type 1:1; the component name matches the Ink filename (`StatusLine.tsx` → `status_line.rs::StatusLine`). This is the translation discipline.

### 2.5 Engine wiring (tier 1) — what changes outside `lingxi-tui`

| Surface | Today (v0.6.0) | After M6 |
|---|---|---|
| `OrchestratorHandle::snapshot_cost()` | returns zeros | reads from `lingxi_cost::CostTracker` (M3 already built) |
| `OrchestratorHandle::list_mcp_servers()` | returns `vec![]` | reads from `lingxi_mcp::ClientRegistry` |
| `OrchestratorHandle::list_hooks()` | returns `vec![]` | reads from `lingxi_hooks::HookRegistry` |
| `OrchestratorHandle::list_agents()` | returns `vec![]` | reads from `lingxi_agent::AgentCatalog` |
| `OrchestratorHandle::force_compact()` | no-op stub | calls `lingxi_compaction::Compactor::compact()` |

**Approach:** extend `MockOrchestratorHandle` → `OrchestratorHandleImpl` (the real one). Each `list_*` and `snapshot_cost` becomes a thin read against the actual registry. No new registries needed — they all exist from M3/M4.

`OAuthHandle::login` stays stubbed (M7 territory). `team`/`coordinator`/`voice`/`grove` registries do not exist in lingxi-core yet — their list endpoints return `vec![]` and the UI renders empty state. This is **a true 1:1 match with claude-code**, which shows the same empty state when the user has none configured.

### 2.6 Telemetry

New events added in M6 (exact list locked in M6-09):

- `tengu_tui_session_started` / `tengu_tui_session_ended`
- `tengu_tui_first_render`
- `tengu_tui_key_pressed` (aggregated; fires once per second with a counter)
- `tengu_tui_permission_dialog_shown` / `tengu_tui_permission_dialog_resolved`
- `tengu_tui_scroll_started` / `tengu_tui_scroll_ended`
- `tengu_tui_streaming_render_started` / `tengu_tui_streaming_render_ended`
- `tengu_tui_resize` (terminal resize)
- `lingxi_core_v0_7_0_released` (once-guarded, follows v0.6.0 pattern)

Estimate: ~15 new events → `ALL_EVENT_NAMES.len() == 330` (locked in M6-09).

### 2.7 What does NOT change

- `lingxi-orchestrator` core — turn loop, streaming, permission gate, session driver all unchanged (only `handle_impl.rs` gets wiring updates in M6-06/07/08)
- All v0.6.0 parity fixtures continue passing
- JSONL format, slash command surface (99), event schema (no breaking changes; only additions)
- `OrchestratorHandle` trait surface — only its impl changes; trait shape stays
- `OAuthHandle::login` — still stubbed (deferred to M7)
- `-p` print mode — byte-for-byte identical output to v0.6.0

### 2.8 Literal Lock Discipline

Every user-visible string the TUI renders must match claude-code's source byte-for-byte unless we have an explicit reason to diverge. Examples in this spec ("Allow Once", "Allow Always", "Deny", "Crunching", `●`, `└ `, "[output truncated, N more lines]", "Compacted N → M messages") are **representative**, not authoritative. The implementer for each component reads the equivalent claude-code `.tsx` first and copies the exact literal. M6-09 produces a single canonical **literal lock list** consolidating every string, indexed against its claude-code source file + line.

---

## §3 Per-Sub-Plan Deliverables

Each sub-plan ends with an annotated tag `m6.N` and is reviewed before the next begins. Sub-plans are sequential; no parallelism within M6.

### M6-01 — iocraft Foundation

**Goal:** Refactor `lingxi-cli` to split out `lingxi-tui` library crate; set up iocraft + crossterm event loop; render a placeholder "Hello, TUI" frame to prove the pipeline works.

**Files:**
- Create: `lingxi-core/crates/tui/` (new crate; src/lib.rs, app.rs, theme.rs, layout.rs, events/mod.rs, events/keymap.rs)
- Create: `lingxi-core/crates/tui/Cargo.toml` (`iocraft = "=0.6"` pinned exact)
- Modify: `lingxi-core/Cargo.toml` (add `crates/tui` to workspace members + default-members)
- Modify: `lingxi-core/crates/cli/src/main.rs` (add Mode dispatch)
- Modify: `lingxi-core/crates/cli/src/argv.rs` (add `--no-tui` flag)
- Modify: `lingxi-core/crates/cli/src/run.rs` (route to `lingxi_tui::run_tui_session` or existing `repl_loop`)
- Modify: `lingxi-core/crates/cli/Cargo.toml` (add `lingxi-tui` dep)

**Key deliverables:**
- `lingxi_tui::run_tui_session(runtime, cancel) -> Result<()>` public API
- `TuiEvent` enum (`Key`, `Resize`, `OrchestratorMessage`, `Tick`)
- crossterm raw mode + alt screen enter/exit (with `std::panic::set_hook` panic guard)
- `tokio::select!` merges keyboard + orchestrator + 100ms ticker
- iocraft `<App>` root component rendering a single `<Box>` with "lingxi-tui v0.7.0"
- `Ctrl-C` then `Ctrl-D` cleanly exits and restores terminal
- TTY detection in `decide_mode` routes non-TTY to StdioRepl
- **iocraft Gate (last task)**: prototype validates streaming render + focus-trap dialog + async key events. If gate fails → switch to ratatui, revise M6-01, retry. Documented in M6-01 verification gate.

**Tests:**
- Behavior: `is_terminal` mocked → asserts StdioRepl path chosen for non-TTY
- Behavior: feed a `Key(Esc)` event → assert app event loop receives it
- PTY smoke: `lingxi-cli` launches, renders one frame, exits on Ctrl-D

**Telemetry:** `tengu_tui_session_started`, `tengu_tui_session_ended`, `tengu_tui_first_render`, `tengu_tui_resize` — 4 events registered.

**Verification gate:** `cargo check --workspace` green; `cargo test --workspace` green; `cargo run -p lingxi-cli` opens TUI and quits cleanly; `lingxi-cli -p "hi" </dev/null` still works (no regression).

**Tag:** `m6.1`

---

### M6-02 — Minimal Working REPL

**Goal:** First end-to-end runnable TUI. Type a prompt, get assistant text back (no streaming, no tools yet — batched response only). Status line on top, scrollback in middle, prompt input on bottom.

**Files:**
- Create: `crates/tui/src/screens/repl.rs`
- Create: `crates/tui/src/components/status_line.rs`
- Create: `crates/tui/src/components/prompt_input.rs` (line editing: chars, backspace, arrow keys, Enter to submit, home/end)
- Create: `crates/tui/src/components/scrollback.rs` (Vec<RenderedMessage>; capped at 500)
- Create: `crates/tui/src/components/messages/mod.rs`, `user_text.rs`, `assistant_text.rs`
- Modify: `crates/tui/src/app.rs` (wire AppState; subscribe to orchestrator non-streaming `run_turn`)

**Key deliverables:**
- Three-zone layout: StatusLine (1 row, top) / Scrollback (flex grow, middle) / PromptInput (1-3 rows, bottom)
- User can type, press Enter, see their text appear in scrollback as UserTextMessage
- Orchestrator's `run_turn` response renders as AssistantTextMessage
- Up/Down arrow navigates prompt history
- `j` / `k` in scrollback scrolls when prompt is empty
- `Ctrl-C` cancels current turn (if running) or clears prompt; second `Ctrl-C` confirms exit
- `/clear`, `/exit`, `/help` slash commands work in TUI
- Color theme: hardcoded (assistant=cyan, user=default, error=red, dim=gray)

**Tests:**
- Snapshot: StatusLine at fixed state
- Snapshot: AssistantTextMessage with 3-line text body
- Snapshot: UserTextMessage with single-line input
- Behavior: feed keys `h`, `i`, `Enter` → AppState.prompt_text="" and scrollback has UserTextMessage("hi")
- Behavior: PgUp twice scrolls scrollback offset by 2 viewport heights

**Verification gate:** `cargo run -p lingxi-cli` shows working REPL; can do a single non-streaming turn end-to-end.

**Tag:** `m6.2`

---

### M6-03 — Streaming + SpinnerWithVerb

**Goal:** Assistant text streams in token-by-token. Spinner overlays the prompt input while a turn is in-flight.

**Files:**
- Create: `crates/tui/src/components/spinner.rs` (frame ticker, label, color cycling)
- Create: `crates/tui/src/streaming.rs` (subscribes to streaming SSE channel; appends deltas to last AssistantTextMessage)
- Modify: `crates/tui/src/screens/repl.rs` (mount Spinner above PromptInput while `app.streaming.is_some()`)
- Modify: `crates/tui/src/app.rs` (call `orchestrator.run_turn_streaming(...)` instead of `run_turn`)

**Key deliverables:**
- TextDelta events from orchestrator appended to scrollback in real time, no flicker
- SpinnerWithVerb visible during streaming; hidden when turn ends
- Verb cycles: "Crunching", "Thinking", "Generating" (matches claude-code's verb pool)
- Spinner frame rate: 10 fps (100ms tick)
- Streaming render rate-limited: max 30 fps even if deltas arrive faster
- Ctrl-C during streaming → cancels via `CancellationToken`; orchestrator emits TurnEnded(Cancelled); UI returns to idle
- **Streaming Gate (last task)**: confirm 30fps sustained, no flicker on real Anthropic SSE. If gate fails → introduce batching, or fall back per R3 mitigation.

**Tests:**
- Behavior: simulate 5 TextDelta events with 50ms gap → scrollback shows concatenated text
- Behavior: assert Spinner mounted while `streaming.is_some()` and unmounted after TurnEnded
- Snapshot: spinner frame at frame_index=0, 5, 9
- Behavior: Ctrl-C during streaming → cancel_token.is_cancelled() == true within 100ms

**Telemetry:** `tengu_tui_streaming_render_started`, `tengu_tui_streaming_render_ended` — 2 events.

**Verification gate:** Manually verify streaming TUI session with a real API call; tokens stream visibly.

**Tag:** `m6.3`

---

### M6-04 — Tool Use Rendering

**Goal:** Assistant tool calls render as collapsible blocks; tool results render with truncation; full multi-tool turns visible end-to-end.

**Files:**
- Create: `crates/tui/src/components/messages/assistant_tool_use.rs` (header line `● ToolName(input_preview)`)
- Create: `crates/tui/src/components/messages/user_tool_result.rs` (collapsed: 1-line summary; expanded: 100 lines or 4000 chars max)
- Create: `crates/tui/src/ansi.rs` (minimal SGR parser for Bash output)
- Modify: `crates/tui/src/components/messages/mod.rs` (dispatcher)
- Modify: `crates/tui/src/app.rs` (handles ToolUseStart, ToolUseResult orchestrator events)

**Key deliverables:**
- ToolUse renders with leading `●` (claude-code's marker)
- Input is JSON-pretty-printed, truncated to one line by default
- Tool result renders with leading `└ ` and is dim-colored
- Result truncation: 100 lines max; "[output truncated, N more lines]" footer
- Bash tool stdout passed through `ansi.rs` (minimal SGR colors + reset only; full parser in M7)
- `e` keypress while a tool block is focused expands/collapses

**Tests:**
- Snapshot: AssistantToolUseMessage for `Read(file_path: "/tmp/x.rs")` collapsed
- Snapshot: UserToolResultMessage with 5-line result
- Snapshot: UserToolResultMessage with 200-line result (truncation shown)
- Behavior: press `e` on focused tool → state.expanded[id]=true
- Behavior: stream Bash output containing `\x1b[31mERR\x1b[0m` → red "ERR" rendered

**Verification gate:** Real `lingxi-cli` session with a tool call (Read or Bash); visible in TUI; expand/collapse works.

**Tag:** `m6.4`

---

### M6-05 — Permission Dialogs (3 critical)

**Goal:** Three modal dialogs work in TUI: ToolUseConfirm (per-call approval), ExitPlanMode (plan-to-execute), BypassPermissionsMode (dangerous mode toggle). Keyboard input matches claude-code.

**Files:**
- Create: `crates/tui/src/components/permissions/mod.rs`
- Create: `crates/tui/src/components/permissions/tool_use_confirm.rs`
- Create: `crates/tui/src/components/permissions/exit_plan_mode.rs`
- Create: `crates/tui/src/components/permissions/bypass_permissions.rs`
- Modify: `crates/tui/src/screens/repl.rs` (overlay dialog when `pending_permission.is_some()`)
- Modify: `crates/tui/src/app.rs` (handles PermissionRequest event; sends PermissionResponse back)

**Key deliverables:**
- Dialog shows: tool name, full input, three buttons: `[1] Allow Once`, `[2] Allow Always`, `[N] Deny`
- Number keys 1/2 / `N` / Enter resolve the dialog
- `Esc` denies (equivalent to N)
- Dialog displays above prompt input, single-line border, dim backdrop
- Dialog is focus owner — keys don't reach the prompt input while it's open
- ExitPlanMode dialog displays the proposed plan text before transition; same 3 options
- BypassPermissionsMode dialog has a warning banner + re-confirmation step (must type "yes")
- Telemetry: dialog shown + resolved events

**Tests:**
- Snapshot: ToolUseConfirm dialog at default state (Allow Once highlighted)
- Snapshot: ExitPlanMode dialog with 5-line plan body
- Snapshot: BypassPermissionsMode dialog with warning banner
- Behavior: feed `1` → response = AllowOnce sent to orchestrator
- Behavior: feed `Esc` → response = Deny
- Behavior: dialog open + feed text key → AppState.prompt_text unchanged (focus-trap works)
- Parity fixture: `parity_tui_permission_dialogs.json` locks key bindings + label literals

**Telemetry:** `tengu_tui_permission_dialog_shown`, `tengu_tui_permission_dialog_resolved` — 2 events.

**Verification gate:** End-to-end: prompt → tool call → permission dialog → approve → tool runs → result shown. All in TUI.

**Tag:** `m6.5`

---

### M6-06 — Engine Wiring 1: Cost Real

**Goal:** StatusLine displays real cost from the M3 cost tracker. `/cost` command shows real breakdown.

**Files:**
- Modify: `crates/orchestrator/src/handle_impl.rs::snapshot_cost` (read from CostTracker)
- Modify: `crates/orchestrator/src/conversation.rs` (hold `Arc<CostTracker>`; pass to handle)
- Modify: `crates/cli/src/init.rs` (construct CostTracker once, share with handle)
- Modify: `crates/commands/src/builtin/cost.rs` (verify it uses snapshot_cost correctly)
- Modify: `crates/tui/src/components/status_line.rs` (re-renders when cost changes via props)

**Key deliverables:**
- `CostSnapshot { total_usd: f64, by_model: HashMap<ModelId, ModelCost>, total_input_tokens, total_output_tokens, total_duration }` populated from CostTracker
- StatusLine shows `$0.0234` (4 decimals to match claude-code)
- `/cost` slash command output includes per-model breakdown, total tokens, total duration
- After each turn ends, CostTracker is updated by orchestrator (verify M3 cost events trigger)

**Tests:**
- Behavior: run a turn → CostTracker.snapshot().total_usd > 0
- Behavior: snapshot_cost returns the same value as the slash command reads
- Snapshot: StatusLine with cost=$0.12 (not the placeholder $0.000)
- Parity fixture extend: `parity_orchestrator_turn_loop.json` adds cost assertion post-turn

**Verification gate:** TUI session: do 1 turn → StatusLine cost > 0; `/cost` matches.

**Tag:** `m6.6`

---

### M6-07 — Engine Wiring 2: MCP / Hooks / Agents Listings

**Goal:** `/mcp`, `/hooks`, `/agents` slash commands show real configured items.

**Files:**
- Modify: `crates/orchestrator/src/handle_impl.rs` (`list_mcp_servers`, `list_hooks`, `list_agents` read real registries)
- Modify: `crates/cli/src/init.rs` (construct MCP `ClientRegistry`, Hooks `HookRegistry`, Agents `AgentCatalog`; share with handle)
- Modify: `crates/commands/src/builtin/mcp.rs`, `hooks.rs`, `agents.rs` (verify list rendering)
- Modify: `crates/tui/src/components/status_line.rs` (optional: add MCP count indicator)

**Key deliverables:**
- MCP registry loaded from `.mcp.json` and `~/.config/lingxi/mcp.json` at startup
- Hook registry loaded from settings + plugin sources at startup
- Agent catalog loaded from `~/.claude/agents/` + project `.claude/agents/` at startup
- `/mcp` lists each server with status; empty state renders "No MCP servers configured"
- `/hooks` lists per-event handlers; empty state renders "No hooks configured"
- `/agents` lists subagents with description; empty state renders "No subagents configured"

**Tests:**
- Behavior: with `.mcp.json` containing 2 stdio servers → `list_mcp_servers` returns 2 entries
- Behavior: with no `.mcp.json` → returns `vec![]` and `/mcp` shows empty-state literal
- Same shape for hooks and agents
- Parity fixture: `parity_tui_listings.json` locks empty-state literals

**Verification gate:** Sample project with `.mcp.json` + an agent → all 3 commands show data in TUI.

**Tag:** `m6.7`

---

### M6-08 — Engine Wiring 3: force_compact Real

**Goal:** `/compact` actually compacts. `force_compact` calls into the M3 compaction engine.

**Files:**
- Modify: `crates/orchestrator/src/handle_impl.rs::force_compact` (call `lingxi_compaction::Compactor::compact(history)`)
- Modify: `crates/orchestrator/src/conversation.rs` (construct Compactor with the orchestrator's ApiClient; share)
- Modify: `crates/commands/src/builtin/compact.rs` (verify it reports actual messages_before/after)

**Key deliverables:**
- `force_compact()` returns `CompactionOutcome { messages_before, messages_after, summary_id }`
- Compactor uses 5-layer hierarchical compaction from M3
- `/compact` displays "Compacted N → M messages" with real numbers
- Compaction boundary marked in scrollback (SystemTextMessage "[Compacted]" in M6; CompactBoundaryMessage in M7)
- **Compaction Gate (last task)**: real compaction does not corrupt history. If gate fails → revert M6-08, document gap in v0.7.0 release notes.

**Tests:**
- Behavior: history with 50 messages → force_compact → returns `messages_after < 50`
- Behavior: post-compact, next turn carries summary context
- Parity fixture: extend `parity_orchestrator_turn_loop.json` with compact scenario

**Risk note:** M3's compaction engine has caches and side effects. Generous testing budgeted.

**Verification gate:** TUI session with long history → `/compact` works visibly.

**Tag:** `m6.8`

---

### M6-09 — Parity Fixtures + Release v0.7.0

**Goal:** Cross-cutting validation, version bump, release artifacts. Closes M6.

**Files:**
- Create: `crates/test-harness/tests/parity_tui_renderers.rs`
- Create: `crates/test-harness/tests/parity_tui_repl_loop.rs`
- Create: `crates/test-harness/src/parity/fixtures/tui_renderers.json`
- Create: `crates/test-harness/src/parity/fixtures/tui_repl_loop.json`
- Modify: `crates/telemetry/src/tengu/release.rs` (add `lingxi_core_v0_7_0_released` constant)
- Modify: `crates/telemetry/src/tengu/mod.rs` (TOTAL: 315 → 330)
- Modify: ALL 42 `Cargo.toml` files: `0.6.0 → 0.7.0`
- Create: `docs/superpowers/releases/2026-XX-XX-v0.7.0.md`
- Modify: `CHANGELOG.md` (add `## [0.7.0]` section)
- Modify: `README.md` (milestone update)

**Key deliverables:**
- `ALL_EVENT_NAMES.len() == 330` (exact count locked here)
- `tengu_tui_*` events emitted via `tracing::info!` matching existing pattern
- Workspace verification gate green
- Cross-platform compile check (5 targets)
- Annotated tag `m6.9` (milestone) + `v0.7.0` (release)
- Release doc dates the ship, lists what landed, lists known deferred gaps for M7

**Tests:**
- All M6 cumulative tests pass
- 4 v0.6.0 parity fixtures still pass (no regressions)
- 2 new TUI parity fixtures pass
- `lingxi_core_v0_7_0_released` emits once on first `Engine::init()` after upgrade

**Verification gate:** clean repo state + all gates green + tags created (locally only).

**Tag:** `m6.9` + `v0.7.0`

---

### Summary table

| Sub-plan | What lands | Tag |
|---|---|---|
| M6-01 | iocraft + crate split + event loop foundation | `m6.1` |
| M6-02 | Minimal working REPL (StatusLine + PromptInput + 2 message types) | `m6.2` |
| M6-03 | Streaming + SpinnerWithVerb | `m6.3` |
| M6-04 | Tool use + tool result rendering | `m6.4` |
| M6-05 | 3 permission dialogs + focus-trap | `m6.5` |
| M6-06 | Engine wiring: real cost | `m6.6` |
| M6-07 | Engine wiring: MCP/Hooks/Agents listings | `m6.7` |
| M6-08 | Engine wiring: real force_compact | `m6.8` |
| M6-09 | Parity fixtures + v0.7.0 release | `m6.9` + `v0.7.0` |

**Estimate:** 9 sub-plans, ~110 tasks total, ~3.5 calendar weeks at sustained M5 pace.

---

## §4 Risk Register

### High-impact risks

| # | Risk | Impact | Probability | Mitigation |
|---|---|---|---|---|
| R1 | **iocraft hits a blocker** (missing widget, async bug, panic on resize) | High — could derail M6 mid-execution | Medium | **M6-01 prototype gate**: before locking iocraft, validate streaming + focus-trap + async events. If fails → switch to ratatui at M6-01, before component code exists. |
| R2 | **Terminal restoration after panic** (raw mode + alt screen still active → broken shell) | High | Medium | Set `std::panic::set_hook` in `run_tui_session`; restore terminal before propagating. Same pattern as gitui/atuin. Test by deliberately panicking in a render path. |
| R3 | **Streaming render perf** (TextDelta faster than reconcile → flicker, stutter) | Medium | Medium-high | M6-03 rate-limits to 30fps via `tokio::sync::Notify`. Batch accumulator if needed. Final fallback: ratatui's immediate-mode model. |
| R4 | **`force_compact` wiring surfaces M3 bugs** (compaction engine cold path) | High | Medium | M6-08 isolated. TDD: round-trip behavior test on 50-message history. Fix M3 bugs in-line if surfaced (precedent: M5-06 fixed lingxi-hooks bugs in-line). |
| R5 | **Translation drift across 30 components** | Medium | High | (a) parity fixtures for key bindings, labels, error literals; (b) implementer always reads equivalent `.tsx` first; (c) literal lock list in M6-09. |

### Medium-impact risks

| # | Risk | Impact | Probability | Mitigation |
|---|---|---|---|---|
| R6 | Windows ConPTY compatibility | Medium | Low-medium | Add Windows CI to M6-01; ship as "Linux/macOS supported, Windows experimental" if needed. |
| R7 | Terminal multiplexer issues (tmux/screen) | Medium | Medium | No mouse mode in M6; rely on crossterm's tmux-aware resize; manual test under tmux in M6-09. |
| R8 | TTY detection false negatives | Low-medium | Low | `--tui` opt-in flag (mirrors `--no-tui`). Document in `/help`. |
| R9 | Engine wiring breaks existing parity fixtures | Medium | Medium | Each wiring sub-plan runs all fixtures before tagging. |
| R10 | ANSI parsing for Bash output | Low-medium | Medium | M6-04 minimal parser (SGR + reset only). Full parser in M7. |

### Low-impact risks

| # | Risk | Impact | Probability | Mitigation |
|---|---|---|---|---|
| R11 | Unicode width edge cases (CJK + emoji) | Low | Low-medium | Use `unicode-width`; iocraft likely already does. Test with emoji + CJK sample. |
| R12 | Insta snapshot brittleness | Low | High during dev | `[filters]` to redact `\x1b[...m` sequences for layout snapshots. |
| R13 | iocraft version-bump breaks API | Low | Low | Pin exact version `=0.6`; update only between sub-plans. |

### Hard gates / fallback decisions

Three explicit decision points in the plan:

1. **End of M6-01:** iocraft prototype passes. If NOT → switch to ratatui, revise M6-01 plan.
2. **End of M6-03:** Streaming at 30fps smooth. If NOT → batching or ratatui fallback.
3. **End of M6-08:** Real compaction safe. If NOT → defer wiring to M7, revert M6-08, document gap.

---

## §5 Verification

### 5.1 Test categories

| Layer | Tool | Scope | What it catches | Cost |
|---|---|---|---|---|
| Unit (Rust) | `cargo test` | One function or struct method | Logic bugs in pure code | <1s each |
| Behavior (TUI) | `cargo test` + iocraft harness | Component event→state→render, no real terminal | State machine bugs, focus-trap, scroll math | <100ms each |
| Snapshot (TUI) | `insta` + buffer dump | Rendered ANSI bytes for stable views | Label typos, wrong color, broken layout | Cheap, produces .snap diffs |
| Integration (PTY) | `expectrl` or `rexpect` | Whole `lingxi-cli` binary through PTY | End-to-end smoke | ~5s each, ~10 total |

### 5.2 Per-component test budget

| Component class | Snapshots | Behavior | Notes |
|---|---|---|---|
| StatusLine | 2-3 | 0 — pure renderer | |
| 4 message renderers | 2 each | 1-2 each | 8 snap + 8 behavior |
| PromptInput | 1 | 6+ | |
| SpinnerWithVerb | 3 (frames 0/5/9) | 2 | |
| 3 permission dialogs | 1 each | 4+ each | focus-trap, text-key-blocked |
| REPL screen | 1 | 5+ | layout zones, scrollback, overlays |
| Scrollback | 1 | 5+ | push, cap, j/k, PgUp/PgDn, g/G |

**Targets:** ~15 snapshots, ~30 behavior tests, ~5 PTY integration tests by M6-09.

### 5.3 Parity fixtures

**`parity_tui_renderers.json`** — stable visual surfaces

```json
{
  "_claude_code_version": "X.Y.Z",
  "status_line": {
    "fixed_state": { "model": "...", "cwd": "...", "cost_usd": 0.123, "ctx_pct": 0.42, "mode": "normal" },
    "ansi_bytes": "<base64 rendered ANSI line>"
  },
  "spinner_frames": [ { "frame_index": 0, "verb": "Crunching", "char": "⠋" } ],
  "message_renderers": {
    "user_text": { "input": "...", "rendered_lines": [ "..." ] },
    "assistant_text": { "...": "..." },
    "assistant_tool_use_collapsed": { "...": "..." },
    "user_tool_result_collapsed": { "...": "..." }
  }
}
```

**`parity_tui_repl_loop.json`** — interactive flows

```json
{
  "scenarios": [
    {
      "name": "single_turn_no_tools",
      "keys": ["h", "i", "Enter"],
      "expected_messages_after": ["UserText('hi')", "AssistantText(...)"],
      "expected_orchestrator_calls": ["run_turn_streaming('hi')"]
    },
    {
      "name": "turn_with_tool_use_and_permission",
      "keys": ["...", "Enter", "1"],
      "expected_outcome": "ToolApprovedAndExecuted"
    },
    {
      "name": "cancel_during_streaming",
      "keys": ["...", "Enter", "C-c"],
      "expected_outcome": "TurnEnded(Cancelled)"
    }
  ]
}
```

Drivers in `lingxi-test-harness/tests/`, part of `cargo test --workspace`.

### 5.4 Workspace verification gate (every sub-plan)

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace --target x86_64-unknown-linux-gnu
cargo check --workspace --target x86_64-apple-darwin
cargo check --workspace --target x86_64-pc-windows-gnu
cargo check --workspace --target aarch64-linux-android
cargo check --workspace --target aarch64-apple-ios
```

Same gate as v0.6.0 — just adds `lingxi-tui` to its scope.

Known flakes (allowed to rerun):
- `rapid_writes_collapse_to_single_event` (v0.5.0)
- `writer_output_equals_single_turn_fixture` (v0.5.0)
- `streaming_concurrent_tools_test` (v0.6.0)

### 5.5 Manual verification checklist (per sub-plan)

- **M6-01:** launch, see "lingxi-tui v0.7.0" rendered, Ctrl-D exits, terminal restored.
- **M6-02:** type "hi", press Enter, see assistant response render.
- **M6-03:** observe streaming — tokens appear progressively, spinner overlays prompt.
- **M6-04:** run a turn that uses Read or Bash; tool call + result both visible.
- **M6-05:** real tool use → permission dialog → press 1 → tool runs.
- **M6-06:** after 1 turn, StatusLine cost > $0.
- **M6-07:** with sample `.mcp.json` containing 1 server, `/mcp` lists it.
- **M6-08:** force long history (paste 50 turns), `/compact` reports reduction.
- **M6-09:** all of the above still work; run on tmux + alacritty + Terminal.app.

### 5.6 Performance budget

| Metric | Budget | How measured |
|---|---|---|
| First-frame latency | <200ms | Manual stopwatch M6-01; PTY timestamp M6-09 |
| Streaming render rate | ≥30fps sustained | Manual observation M6-03 + frame counter |
| Memory growth across 100-turn session | <50MB | `valgrind --tool=massif`; M6-09 |
| Idle CPU (TUI open, no input) | <0.5% on 1 core | `top`; M6-09 |

Soft budgets — if violated, document and decide M6 vs M7.

### 5.7 What does NOT need new verification

- v0.6.0 parity fixtures continue passing unchanged
- M3/M4 cost/memory/MCP/hooks/agent tests continue passing
- Cross-compile matrix — same 5 targets

---

## §6 Schedule

### 6.1 Cadence

Single-Claude pace, sequential sub-plans. Calibrated from M5: 14 sub-plans in ~3-4 weeks → similar density.

| Sub-plan | Tasks (est.) | Calendar (est.) | Cumulative |
|---|---|---|---|
| M6-01 | 12-14 | 2-3 days | 3 days |
| M6-02 | 14-16 | 3-4 days | 7 days |
| M6-03 | 10-12 | 2 days | 9 days |
| M6-04 | 12-14 | 2-3 days | 12 days |
| M6-05 | 12-14 | 2-3 days | 15 days |
| M6-06 | 8-10 | 1-2 days | 17 days |
| M6-07 | 10-12 | 2 days | 19 days |
| M6-08 | 12-14 | 2-3 days | 22 days |
| M6-09 | 14-16 | 2-3 days | 25 days |

**Total: ~110 tasks, ~3.5 calendar weeks. Likely variance: ±1 week.** Main variance driver: iocraft gate at M6-01.

### 6.2 Dependencies

```
M6-01 (foundation)
  ├─→ M6-02 (depends on event loop + app shell)
  │     ├─→ M6-03 (depends on REPL + scrollback)
  │     │     └─→ M6-04 (depends on streaming)
  │     │           └─→ M6-05 (depends on tool use surface)
  │     │
  │     ├─→ M6-06 (depends on StatusLine for cost render)
  │     ├─→ M6-07 (depends on slash command surface — already wired)
  │     └─→ M6-08 (depends on /compact path — already wired)
  │
  └─→ M6-09 (depends on everything above)
```

Sequential by discipline (same as M5), even though M6-06/07/08 could in principle run in parallel.

### 6.3 Slip handling

If any sub-plan grows >1.5× estimated tasks or days:

1. **Scope creep?** — defer bloated part to M7. Document in release doc.
2. **Real blocker?** — file `BLOCKED` agent status, escalate to user.
3. **Hidden dependency?** — write small precursor sub-plan, treat as M6-Na.

Same protocol as M5.

### 6.4 Tag and release policy

- **Per-sub-plan annotated tags**: `m6.1` … `m6.9`. Created locally after gate passes.
- **Release tag**: `v0.7.0`, annotated, created in M6-09.
- **No remote push from Claude.** Same as v0.5.0 / v0.6.0.
- **No force-push, no skip-hooks, no amends.**

### 6.5 Worktree strategy

Dedicated worktree `m6-execution` on branch `m6-execution`, created at start of M6-01 via `superpowers:using-git-worktrees`. After M6-09 lands the release tag, fast-forward-merged to `main` via `superpowers:finishing-a-development-branch` Option 1.

### 6.6 What happens after M6

When `v0.7.0` is tagged and merged:

1. **Pause for review window.** No auto-progression to M7.
2. **M7 brainstorm**: TUI Surface — Doctor, Resume iocraft screen, Settings UI, Memory editor, full message renderer set, search/transcript/export, vim mode, command palette autocomplete, theme picker, syntax-highlighted code blocks, structured diff. Estimated 12-15 sub-plans.
3. **M8 brainstorm**: TUI Advanced — Coordinator/Team/Swarm UI + engine, Voice, grove, IDE bridge dialogs, rate-limit banners, Anthropic-internal. Estimated 10-12 sub-plans.
4. **v1.0.0**: polish + release.

Roadmap visible: ~10-14 calendar weeks from M6 start to v1.0.0. Not a commitment — just the shape.

---

## References

- M5 design (predecessor): `docs/superpowers/specs/2026-05-25-m5-conversational-agent-loop-design.md`
- v0.6.0 release notes: `docs/superpowers/releases/2026-05-28-v0.6.0.md`
- M4 design (tools complete): `docs/superpowers/specs/2026-05-24-m4-tools-implementation-design.md`
- M3 design (engine complete): `docs/superpowers/specs/2026-05-23-m3-engine-completion-design.md`
- claude-code source: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/`
  - `screens/REPL.tsx` — main REPL screen reference (5006 lines)
  - `components/StatusLine.tsx` — status line reference
  - `components/Spinner.tsx` — spinner reference
  - `components/PromptInput/` — input widget reference
  - `components/permissions/PermissionRequest.tsx` — permission dialog reference
  - `components/messages/UserTextMessage.tsx`, `AssistantTextMessage.tsx`, `AssistantToolUseMessage.tsx`, `UserToolResultMessage.tsx` — message renderer references
- codex Rust ratatui reference (fallback library): GitHub `openai/codex` — patterns for streaming, modal dialogs, focus management
- iocraft documentation: https://docs.rs/iocraft

---

**End of design.**
