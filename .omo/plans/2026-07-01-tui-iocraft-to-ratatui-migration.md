# TUI Migration: iocraft → Ratatui via `tui-core` / `tui` / `tui-rata` 3-crate split

- **Status:** Proposed (pre-implementation). Reviewed by Oracle (pressure-test) on 2026-07-01.
- **Date:** 2026-07-01
- **Scope:** `lingxi-code/tui` and its 3 dependents (`apps/cli`, `apps/engine-desktop`, `test-harness`).
- **Decision:** GO — migrate off iocraft onto Ratatui using a physically separate 3-crate split. Reject the runtime env-switch and workspace-feature variants.

---

## 1. Goal & Motivation

Migrate the terminal UI from the `iocraft` reactive framework to `ratatui`, to gain:

- Fixed bottom composer/footer, independent scrollback viewport, complex overlays, mouse scroll, resize reflow, streaming message + permission prompt control.
- Removal of iocraft ownership of the terminal lifecycle (raw mode / alt-screen / event pump).
- Removal of the dual-keymap + crossterm-version bridge (see §3).
- Collapse of the 5,952-line `root.rs` monolith into a conventional Ratatui draw/event loop.

**If the *only* goal were the crossterm version conflict**, a cheaper spike exists (converge workspace on crossterm 0.29, or patch/fork iocraft). That would NOT address the `root.rs` monolith, dual key handling, or render-loop coupling. This plan assumes the broader goal.

---

## 2. Verified Findings (grounded in code)

- Current UI crate: `lingxi-code/tui` (package `tui`), ~139 source files.
- iocraft coupling: **367 references across 65 files**; **64 files** have real code-level coupling (`use iocraft`, `element!`, `#[component]`, `iocraft::Color`).
- `root.rs` = **5,952 lines / 168 symbols** — owns the iocraft reconciler + main loop (`root.fullscreen()…await` / `render_loop()` at `session.rs:585-592`) + 3 async bridge pumps (`bridge_rx`, `multiagent_rx`, `permission_rx`) + bash pump + OSC-52 copy pump + streaming redraw ticks + scroll viewport.
- **Crossterm version conflict:** iocraft 0.8.3 bundles crossterm **0.29** (`tui/Cargo.toml:46`); workspace pins crossterm **0.28** (`Cargo.toml:222`). `root.rs` carries a dual keymap: `map_key` (0.28) + `map_iocraft_key` (0.29) and ~25 `iocraft_to_crossterm028_key()` bridge call sites.
- **Dependents of `tui`** (path deps, exactly 3): `apps/cli/Cargo.toml:56`, `apps/engine-desktop/Cargo.toml:134`, `test-harness/Cargo.toml:112`.
- **Keymap is already backend-neutral:** `events/keymap.rs:215-263` uses crossterm-0.28 `KeyEvent`; the iocraft shim is isolated in `root.rs:212-250`.
- **Native scrollback is OPT-IN, not default:** the `stdout.println(TerminalLine)` path is gated behind `LINGXI_TUI_NATIVE_SCROLLBACK` (`root.rs:4193-4203`). The default keeps messages inside `VirtualMessageList`. Ratatui native-scroll (`insert_before`) vs. in-frame `List`/`Paragraph.scroll()` is a **design decision to make**, not proven reuse.

---

## 3. Topology Decision

Adopt **three physically separate crates**:

```
tui-core   iocraft-FREE. Neutral render model + pure reducers + backend-neutral state.
tui        current iocraft UI, depends on tui-core. DELETED at cutover.
tui-rata   new ratatui UI, depends on tui-core. Zero iocraft, crossterm 0.28 only.
```

The `cli` binary depends on **one UI crate at a time** — never both — guaranteeing a single terminal owner and a single crossterm version in any binary.

### Rejected alternatives

- **Runtime env-switch (`LINGXI_TUI_BACKEND`)** — compiles both backends into one binary → links crossterm 0.28 **and** 0.29 simultaneously, two owners of raw-mode/alt-screen. Preserves the conflict instead of removing it.
- **Workspace Cargo features** — features unify across the dep graph; if one consumer enables iocraft and another ratatui, one binary still compiles both. Same failure as env-switch.

---

## 4. `tui-core` Boundary

### 4a. Move as-is — zero code-level iocraft (verified clean)

- `streaming.rs` (`apply_event`)
- `events/keymap.rs`, `events/mod.rs`, `events/orchestrator_bridge.rs` (`KeyAction`, `TurnEvent`, `BridgeOutputStream`)
- `render/{ansi,markdown,markdown_table,syntax,diff,model_name}.rs`
- `rate_limit_messages.rs`, `recent_models.rs`, `replay.rs`
- `startup_bypass.rs`, `startup_trust.rs`, `theme_detect.rs`, `theme_persist.rs`
- `telemetry.rs`, `error.rs`, `bash_runner.rs`, `commands/{color,copy}.rs`
- `multiagent/{adapter,apply,event,fixture,mod,poller,state}.rs` (only `style.rs` couples)
- `components/tasks/*`, `components/coordinator/*` (string/data renderers)

> **Extraction-order correction (2026-07-01, discovered during step 1):** "iocraft-free" is necessary but NOT sufficient for "movable now" — a module can only move once all its *intra-crate* deps are already in core (or upstream). Verified sub-classification of the list above:
> - **Wave 1 — true leaves, zero intra-`tui` deps — ✅ ALL DONE:** `multiagent/state.rs` ✅, `error.rs` ✅, `recent_models.rs` ✅ (`dirs`+`memory`+`serde_json`), `telemetry.rs` ✅ (`permission`+`telemetry`), `bash_runner.rs` ✅ (`async-trait`), `multiagent/{event,adapter,fixture,poller}` ✅ (`traits`+`tokio`). Verified: `cargo check -p tui-core -p tui` green, `cargo test -p tui-core` 15/15. `commands/*` reclassified to Wave 2 — `commands/copy.rs` uses `crate::state::RenderedMessage` (blocked on state).
> - **Wave 2 — blocked on `crate::theme` (need §4b first):** `theme_detect.rs`, `theme_persist.rs`.
> - **Wave 2 — blocked on `crate::render`/`components::messages` (need §4b + message move):** `rate_limit_messages.rs`, `render/{ansi,markdown,syntax,diff,markdown_table,model_name}.rs` (depend on `render::StyleColor`/`SpanStyle` in `render/mod.rs`).
> - **Wave 2 — blocked on `crate::terminal::RawGuard`:** `startup_bypass.rs`, `startup_trust.rs`.
> - **Wave 3 — NOT a leaf (was mis-listed):** `events/keymap.rs` — its `KeyAction` embeds `crate::screens::settings::SettingsTab` and its fns reference `crate::state::AppState` + the permission dialog states. Moves only after `state` + those move.
>
> **Extraction mechanic (proven):** move real content → `tui-core`; leave a one-line `pub use tui_core::<path>::*;` shim in the original `tui` file so every `crate::…` path and downstream re-export keeps resolving. Verified green end-to-end (`cargo check -p tui-core -p tui`, `cargo test -p tui-core` 8/8).

### 4b. Small surgery, then core — the color decouple ("A1")

The neutral color type already exists: `render/mod.rs:63 StyleColor` (`Default/Named/Rgb/Indexed`); `SpanStyle.fg/bg` are already `StyleColor` (`render/mod.rs:156,158`). Only the *conversion* leaks:

1. `render/mod.rs` — delete `StyleColor::to_iocraft()` (line 81) + `named_to_iocraft()` (line 94) + `use iocraft::Color` (line 21). Relocate conversion to a UI-side adapter. `StyleColor`/`SpanStyle` then move to core untouched.
2. `components/messages/mod.rs` — re-type `TerminalSpan.fg/bg`: `Option<iocraft::Color>` → `Option<StyleColor>` (lines 53-55); drop `terminal_color()`; `from_styled_span` stores `StyleColor`. Then the ~35 message renderers + `render_entry_to_terminal_lines` (line 514) move to core.
3. `theme.rs` — re-type `Theme` palette fields `iocraft::Color` → `StyleColor` (`theme.rs:11,237-268`). Consumers (`theme.error/dim/warning`) pass the color through — mechanical.
4. `multiagent/style.rs` — 1 `Color` ref, same treatment.

Each UI writes its own boundary adapter: `tui` keeps `StyleColor → iocraft::Color`; `tui-rata` adds `StyleColor → ratatui::style::Color`. One-time, ~4 files.

> **§4b progress (2026-07-01):**
> - **KEYSTONE DONE ✅** — `render/mod.rs` is now iocraft-free. `StyleColor::to_iocraft()` + `named_to_iocraft()` were relocated into a `tui`-side extension trait `StyleColorIocraftExt` in new file `tui/src/render_iocraft.rs`; `xterm256_to_rgb` made `pub`; the `.to_iocraft()` call syntax preserved at all 7 consumer files via `use crate::render_iocraft::StyleColorIocraftExt;`. Behavior-preserving. Verified: `cargo check -p tui` green, `cargo test -p tui --lib render_iocraft` passes.
> - **REMAINING (one atomic cascade — Theme+TerminalSpan are coupled):**
>   1. `theme.rs`: `ansi()→StyleColor::Named(matching variant)` (`_→Default`), `rgb()→StyleColor::Rgb`, `Theme` 13 fields + `TuiTheme` consts → `StyleColor`; update the ~4 parity tests' expected values (`Color::X`→equivalent `StyleColor`).
>   2. `messages/mod.rs`: `TerminalSpan.fg/bg: Option<StyleColor>`; `TerminalSpan::colored`/`colored_terminal_lines` take `StyleColor`; `from_styled_span` stores `span.style.fg` directly (drop `.to_iocraft()`); `terminal_color(StyleColor)→Option<StyleColor>` (Default→None).
>   3. **PARITY TRAP:** rewrite `fg_sgr`/`bg_sgr` to take `StyleColor` and reproduce the CURRENT bytes exactly — `Named(n)`→the SGR of `named_to_iocraft(n)` (fixed table); `Rgb`→`38;2;r;g;b`; **`Indexed(i)`→`xterm256_to_rgb(i)`→`38;2;r;g;b` (truecolor, NOT `38;5;i`)**; `Default`→`39`/`49`. Keep `encode_terminal_line_ansi` iocraft-free so it can move to core.
>   4. Fix every iocraft `element!{ color: theme.X }` / `.color(theme.X)` consumer to add `.to_iocraft()` (compiler-driven).
>   5. Then move `render/` (mod + submodules) + `theme.rs` into `tui-core` behind shims; add deps (`unicode-width`, `unicode-segmentation`, `syntect`, `pulldown-cmark`, `similar`, `serde`) mirroring `tui/Cargo.toml` pins.
>
> This cascade must be applied atomically (a partial edit leaves `tui` uncompilable) — do it as one focused pass with `cargo check` at the end, not piecemeal.

### 4c. `AppState` field triage (the real work — NOT a wholesale move)

`AppState` has **no iocraft-typed fields by name**, but ~13 fields reference structs defined in iocraft-importing modules (`crate::components::*` / `crate::screens::*`). Moving `AppState` whole would drag those modules into core. Triage:

**→ Move to `tui-core` (pure/business, neutral types):**

| Field | Type |
|---|---|
| `streaming`, `cancel_token`, `in_flight_turn` | `Option<StreamingState>`, cancel token, `Option<TurnInFlight>` |
| `prompt_text`, `prompt_cursor`, `history`, `history_cursor` | `String`, `usize`, `Vec<String>`, `Option<usize>` |
| `scroll_offset`, `viewport_width` | `usize` |
| `status` | `StatusSnapshot` |
| `theme`, `theme_setting`, `syntax_highlighting_disabled`, `reduced_motion` | `Theme` (after 4b), enums, bools |
| `current_todo` | `Option<CurrentTodo>` |
| `sigint_armed_at`, `sigint_armed_key`, `should_exit`, `resume_request` | timers/flags |
| `pending_config_edit`, `pending_open_settings`, `pending_turn`, `pending_slash`, `pending_bash` | request flags (`SettingsTab` is a plain enum — verify neutral) |
| `focused_tool_id`, `expanded`, `tool_call_inputs` | ids + `HashMap` |
| `last_rate_limit_text`, `has_shown_overage_notification` | text/bool |
| `pending_permission_resp_tx`, `pending_permission_started_at`, `permission_queue` | channel + timer + `VecDeque<PermissionExchange>` (see §4d) |
| `command_argument_names`, `vim_enabled` | `HashMap`, bool |

**→ Coupled by composition — must neutralize before core (each state struct lives in an iocraft module; verify it is pure data, then move the struct to core, leaving only its `#[component]`/`element!` renderer in the UI crate):**

| Field | Type (iocraft module) |
|---|---|
| `height_cache` | `components::virtual_message_list::HeightCache` |
| `tool_use_dialog_state` | `components::permissions::tool_use_confirm::ToolUseConfirmState` |
| `exit_plan_dialog_state` | `components::permissions::exit_plan_mode::ExitPlanModeState` |
| `bypass_dialog_state` | `components::permissions::bypass_permissions::BypassPermissionsState` |
| `palette` | `PaletteState` (prompt_input) |
| `completion` | `CompletionState` (prompt_input) |
| `vim` | `components::prompt_input::VimState` |
| `history_search` | `Option<components::prompt_input::HistorySearchState>` |
| `paste` | `components::prompt_input::PasteState` |
| `active_screen` | `Option<screens::Screen>` (each variant carries per-screen state inline, e.g. `Screen::Doctor(DoctorDiagnostics)`) |
| `message_selector` | `components::message_selector::MessageSelectorState` |
| `multiagent` | `multiagent::MultiAgentState` (data struct — likely clean) |

**Audit result (2026-07-01 — RESOLVED):** the state structs are **pure data**; the sync reducers mutate them and the `#[component]` fns only read them. Per-struct findings:

| Struct | Fields | Key handler | Verdict |
|---|---|---|---|
| `PaletteState` | `open/filter/selected` | (none on struct) | ✅ pure — move to core |
| `CompletionState` | `open/filter/selected/candidates` | `handle_key(KeyCode)` uses **`iocraft::KeyCode`** (only `use iocraft::prelude::*`, no crossterm import) | ⚠️ fields pure; **re-type the handler to crossterm-0.28 `KeyCode`** on move |
| `PasteState` | ids + `Vec<Attachment>` + `Vec<(u32,String)>` | pure `process_paste` | ✅ pure — move to core |
| `VimState` | mode/command/register/visual enums | `vim.rs` imports **only** `crossterm::event` (no iocraft) | ✅ fully clean — move to core |
| `HistorySearchState` | `query/match_index/saved_prompt/saved_cursor` | `crossterm::event` | ✅ pure — move to core |
| `MessageSelectorState` | `open/mode/query/filtered/selected/export` | `handle_message_selector_key(KeyEvent)` uses `crossterm::event` | ✅ pure — move to core |
| `ToolUseConfirmState` | `focus: DialogFocus` | `handle_key(KeyEvent)` uses `crossterm::event` | ✅ pure — move to core |
| `ExitPlanModeState` | `focus: DialogFocus` | `crossterm::event` (verified: `handle_key` at `exit_plan_mode.rs:33`) | ✅ pure — move to core |
| `BypassPermissionsState` | `focus`-style | `crossterm::event` | ✅ pure — move to core |
| `MultiAgentState` | `Vec<TaskRow>`, `Vec<WorkerRow>` (all `String`) | (data only; header comment: "Pure data held on AppState") | ✅ fully clean — move to core |
| `HeightCache` | height/width numbers (`virtual_message_list.rs:446`, iocraft-prelude file) | (data only) | ⚠️ struct pure, but **likely rebuilt** in `tui-rata` (Ratatui owns layout) — do NOT assume it moves |
| `Screen` | 24 variants, each carrying a per-screen state struct | per-variant pure handlers (`handle_resume_key`, `apply_settings_key`, …) | ⚠️ see below |

**Key-type coupling — the whole surface (VERIFIED):** the reducers were built **crossterm-0.28-native**. Every overlay/dialog/vim/selector/screen handler already imports `crossterm::event`. **The complete iocraft-`KeyCode` leak is exactly TWO structs — `CompletionState` and `PaletteState`** (both `@`/`/` prompt-input overlays; each has `handle_key(code: KeyCode)` resolving `KeyCode` through `use iocraft::prelude::*` with no crossterm import — `completion.rs:140`, `palette.rs:219/261`). Re-type those handlers to crossterm-0.28 `KeyCode` (or the neutral `KeyAction`) on move. Everything else — vim, history_search, message_selector, all three permission dialogs, and the interactive screens — is already crossterm-0.28.

**`Screen` (the one real decision, not a blocker):** 24 variants (`Doctor(DoctorDiagnostics)`, `Resume(ResumeState)`, `Settings(SettingsState)`, `Stats(StatsState)`, `Model(ModelScreenState)`, … `Transcript(TranscriptScreenState)` — full list in `screens/mod.rs:48-217`). Each payload follows the SAME proven pattern (pure state struct + pure crossterm-0.28 key handler + iocraft render component), so they are *individually* movable. BUT since every screen **renderer** is rewritten in `tui-rata` (§4e), do NOT drag the 24-variant enum + payloads through the neutral boundary prematurely. **Recommendation:** move each screen's *data snapshot* (e.g. `StatsData`, `DoctorDiagnostics`, `SettingsData`, MCP/hooks/agents lists) to core where shared; rebuild the `Screen` enum + `active_screen` routing + per-screen UI state in `tui-rata`. Treat `active_screen` as a **UI-crate field**, not a core field.

**Net:** 10/12 overlay structs move to core unchanged; `CompletionState` needs a 2-method key-type re-type; `HeightCache` is likely replaced by Ratatui layout; `Screen` is rebuilt in `tui-rata` (its data snapshots move to core). No struct holds an iocraft handle — the coupling is entirely (a) `iocraft::Color` in the render/theme model (§4b) and (b) `CompletionState`'s two `iocraft::KeyCode` handlers.

### 4d. `PermissionExchange`

Almost neutral, but names `WorkerPermissionInfo`, which lives in `components/permissions/worker.rs` (imports `iocraft::prelude::*`) (`permission_bridge.rs:36-45`, `worker.rs:7-29`). **Order:** hoist `WorkerPermissionInfo` (pure data) into core first, then move `PermissionExchange`.

### 4e. Discard — rebuilt natively in `tui-rata`

- `root.rs`, `app.rs`, `session.rs` (reconciler + main loop + dispatch + the iocraft key shim)
- `terminal.rs` (iocraft-era guard — ratatui provides its own)
- all `screens/*` (iocraft component trees)
- `components/{scrollback, virtual_message_list, message_selector, spinner, status_line, picker_popup}`, all `prompt_input/*` (vim editor), all `permissions/*` renderers

---

## 5. Commit Sequence — everything reversible until cutover

The **first irreversible commit is deleting/renaming old `tui` at cutover** (step 7). Every prior step is additive/reversible.

1. **Create `tui-core`**; move the §4a clean modules. Old `tui` re-exports from core → compiles, zero behavior change.
2. **Color decouple** (§4b). Old `tui` adds `StyleColor → iocraft::Color` adapter *in `tui`* (not core). Move `render`/`messages`/`theme` to core. Covered by existing `render` tests.
3. **AppState field-triage** (§4c): move business fields to core; neutralize + move the composition-coupled state structs (verify pure-data first); hoist `WorkerPermissionInfo` then `PermissionExchange` (§4d).
4. **Scaffold `tui-rata`** on `tui-core` + crossterm 0.28. Verify with `cargo tree -p tui-rata -i crossterm` that exactly one crossterm version links. Decide native-scroll vs. in-frame model here.
5. **Rebuild in `tui-rata`**: runtime shell → message/scrollback → composer/footer → overlays/pickers → permission screens → task/team/multiagent views.
6. **Repoint dependents** (`apps/cli`, `apps/engine-desktop`, `test-harness`) from `tui` → `tui-rata` — 3 one-line edits — ONLY after `tui-rata` runs the main session + resume picker.
7. **Cutover (first irreversible step):** delete old `tui`; optionally rename `tui-rata` → `tui`.

---

## 6. Test Plan

### Baseline (lock before migration)
- REPL screen snapshot; footer bottom-pinned; content scrolls independently.
- `/web` config typing `tvly-…` does not drop the first char.
- WebSearch/WebFetch tool message rendering.
- Permission prompt shortcuts.
- Resume session does not duplicate history.

### `tui-core` unit tests (backend-agnostic)
- Reducers, `apply_event` streaming, `render_entry_to_terminal_lines` golden `TerminalLine` output (shared by both UIs).
- `StyleColor` conversions round-trip.

### `tui-rata` tests (Ratatui `TestBackend`)
- 80×24 / 120×40 / 40×12 layout snapshots.
- Scrollback separated from footer/composer; long stdout does not break footer.
- Wide-char (CJK) cursor + wrap.
- Active-model footer; overlay centering/bottom sheets; permission focus + confirm/cancel.
- Resize reflow + stick-bottom.

> Note: iocraft string/`insta` snapshots and Ratatui `TestBackend` buffer grids are **different formats** — no shared golden across backends at the widget level. Cross-backend equivalence is asserted only at the `tui-core` `TerminalLine` layer.

### Commands
```bash
cargo test -p tui-core
cargo test -p tui-rata
cargo tree -p tui-rata -i crossterm   # MUST show exactly one version
cargo build -p cli --bin lingxi-cli
cargo fmt --check
cargo clippy -p tui-rata --all-targets
```

### Manual QA
Chat streaming; long-output scroll; Bash tool; WebSearch/WebFetch; permission prompt; slash command; model picker; resume; Ctrl-C/Esc/terminal restore; iTerm2 mouse wheel + paste.

---

## 7. Risks & Open Questions

1. **AppState composition coupling (§4c) — AUDITED + VERIFIED, resolved.** All 12 overlay/dialog state structs are pure data; no struct holds an iocraft handle. Residual work: (a) the two `iocraft::KeyCode` handlers — `CompletionState` + `PaletteState` — re-type to crossterm-0.28; (b) `HeightCache` likely replaced by Ratatui layout; (c) `Screen` (24 variants) rebuilt in `tui-rata` with its data snapshots moved to core, `active_screen` treated as a UI-crate field. Spot-check of screen payloads (`stats`/`help`/`transcript`/`skills`/`scroll`/`resume`/`memory`/`theme`/`connect*`) confirmed pure data + crossterm-0.28 handlers; only `settings/*` needs a rebuild-time KeyCode-type check (non-blocking — screens are rewritten anyway).
2. **Native scrollback is a design decision, not free reuse (§2)** — choose `insert_before` vs. in-frame scroll deliberately in step 4.
3. **Prompt input rewrite** — the vim editor (`vim.rs` 99 sym + `vim_test` 134 + completion + palette + history_search + image_paste) is a full modal editor rewrite in `tui-rata`, not a move.
4. **`root.rs` 3-pump event loop** — re-hosting `bridge_rx`/`multiagent_rx`/`permission_rx` + streaming ticks + cancellation on a hand-rolled `tokio::select!` loop is the core runtime risk; give it its own phase + tests (step 5, runtime shell).
5. **Known regressions to guard** — `/web` first-char drop (input/paste focus race), resume duplicate history (scrollback emit order), long-stdout footer break: no cross-backend automated net; lock via `tui-core` goldens + manual QA.
6. **crossterm 0.28 alignment** — confirm the chosen Ratatui version's crossterm major matches the workspace 0.28 pin (step 4 `cargo tree` gate).

---

## 8. Effort

- Safe first extraction (steps 1–3): **Medium**.
- Full `tui-rata` rebuild (steps 4–7): **Large** (runtime + ~25 screens + vim editor + permission dialogs).

Scope of the rewrite is unchanged by the crate split — the split is a safer *container*, not less work.
