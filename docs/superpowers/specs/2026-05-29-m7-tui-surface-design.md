# M7 TUI Surface — Design

**Status:** Draft, awaiting user review (2026-05-29)
**Target release:** v0.8.0
**Predecessor:** v0.7.0 (M6 — TUI Foundation)
**Successor (planned):** v0.9.0 (M8 — TUI Advanced + deferred engine wiring) → v1.0.0
**Author:** luolingfeng + Claude Opus 4.8

---

## §0 Decisions Locked (from brainstorm)

| # | Question | Answer |
|---|---|---|
| Q1 | Engine-wiring scope for M7 | **A — TUI-only.** All 5 deferred v0.7.0 engine items (real LLM compaction summary, CostTracker→AnalyticsBus, MCP auto-connect, OAuth PKCE, per-model cost) defer to M8. M7 screens render against the engine as-is. |
| Q2 | PromptInput depth | **A — full fidelity.** Vim (normal/insert/visual + motions + operators + counts), command palette, `@` completion, image paste, multi-line, history search. Consistent with the 1:1 parity north star. |
| Q3 | Syntax-highlight + diff stack | **A — `syntect` + `similar`** (+ `pulldown-cmark` for markdown). Pure-Rust `fancy-regex` backend. **Parity caveat:** highlight parity means "equivalent look," NOT byte-identical ANSI — highlight.js token classification cannot be reproduced in Rust. Literal-lock applies to structure + theme colors, not per-token output. |
| Q4 | Decomposition shape | **Approach 1 — foundation-first, then breadth.** Rendering primitives (ANSI/markdown/syntect/diff/VirtualList) first, then renderers + input + screens, then release. Mirrors M6's successful rhythm. |
| Q5 | TUI library | **iocraft** (locked in M6; `=0.8.3`). Ratatui remains the documented fallback. |
| Q6 | Validation | **Hybrid** — insta snapshots (pure renderers), behavior tests (interactive flows), parity fixtures (cross-cutting invariants). |

---

## §1 Goal & Non-Goals

### Goal

**v0.8.0 completes the single-user TUI surface.** Where v0.7.0 gave a working 3-zone REPL with 4 message types and basic input, v0.8.0 makes `lingxi-cli` render the *full* claude-code single-user experience: every message type a solo user sees, full-page screens (Doctor, Resume, Settings, Memory), a power-user-grade input editor (vim + palette + completion), rich content rendering (markdown, syntax-highlighted code, structured diffs), and windowed scrollback that scales to long sessions.

Concrete deliverables — must all ship for v0.8.0:

1. **Rendering primitives**: full ANSI parser (256-color + truecolor + cursor moves), markdown rendering, `syntect` syntax-highlighted code blocks, `StructuredDiff` viewer (syntect + `similar`).
2. **~22 single-user message renderers** matching claude-code's Ink equivalents (thinking, redacted-thinking, compact-boundary, system-text, system-api-error, rate-limit, shutdown, advisor, hook-progress, bash-input, bash-output, command, local-command-output, memory-input, plan, prompt, resource-update, image, attachment, grouped-tool-use, collapsed-read-search, plan-approval).
3. **VirtualMessageList**: windowed scrollback replacing M6's capped-500 buffer; scales to 10k+ messages without rendering them all.
4. **Advanced PromptInput** (full fidelity): multi-line editing, vim mode (normal/insert/visual + motions + operators + counts), command palette (`/` autocomplete), `@` file-ref completion, image paste, history search, footer/suggestions/help-menu/mode-indicator.
5. **4 screen areas**: Doctor, Resume (iocraft — replaces M5-08 stdio picker), Settings (Config/Settings/Status/Usage), Memory editor + MessageSelector (search/transcript/export).
6. **Theme picker**: selectable color themes, mapped to syntect `.tmTheme` for code.
7. Test surface: snapshots for pure renderers, behavior tests for interactive flows, ≥2 new parity fixtures.
8. Workspace version 0.7.0 → 0.8.0; annotated tags `m7.N` per sub-plan + `v0.8.0`.
9. Cross-platform compile gate green (5 targets; same posture as v0.6.0/v0.7.0).

### Non-Goals (M7 explicitly does NOT do)

- **The 5 deferred engine-wiring items** → M8: real LLM compaction summary (ForkedAgentRunner), CostTracker→AnalyticsBus, MCP auto-connect, OAuth real PKCE flow, per-model cost breakdown. M7 screens render against the engine as-is (Usage shows flat cost, Doctor shows "configured, not connected", `/login` stays stubbed).
- **Team / Coordinator / Swarm renderers** → M8: UserTeammateMessage, UserChannelMessage, TaskAssignmentMessage, UserAgentNotificationMessage, teamMemCollapsed/Saved.
- **Voice / `grove` / FPS metrics / IDE bridge dialogs / Anthropic-internal banners** → M8.
- **Mouse mode** → M8. Keyboard only remains (vim/palette are keyboard-driven).
- **Inline terminal image display** (kitty/iTerm2/sixel rendering) → M8 terminal-protocol cluster. M7 image paste does detection + ref insertion only.
- **Byte-identical syntax-highlight output** — explicitly out: parity means equivalent-looking highlighting, not reproducing highlight.js token classification.

### Success Criteria

v0.8.0 release equivalent to all of these passing:

1. A markdown assistant message with a fenced ```rust block renders with syntect highlighting; an Edit tool result renders as a colored StructuredDiff.
2. A 5,000-message session scrolls smoothly (VirtualMessageList renders only the viewport window).
3. Vim mode: `Esc` → normal, `dw`/`cc`/`x`/`p`/counts work; `/` opens command palette with live filtering; `@` completes file paths.
4. `/doctor` opens the Doctor screen; `--resume` (no id) opens the iocraft Resume picker; Settings + Memory screens open and navigate.
5. Theme picker switches the active theme; code blocks and StatusLine reflect it live.
6. All ~22 new renderers display correctly for their message types.
7. `cargo test --workspace` passes; known flakes acceptable.
8. Cross-platform compile gate green for 5 targets.
9. `ALL_EVENT_NAMES.len()` extends from 326 to a count locked in M7-16 (new TUI events for screens/vim/search; exact number audited there).
10. Annotated tag `v0.8.0` exists, points to release commit.

---

## §2 Architecture

### 2.1 Crate layout — all in `lingxi-tui` (extend, no new crates)

M7 is pure surface within the established crate. No new workspace members; `lingxi-tui` grows new modules.

```
lingxi-core/crates/tui/src/
├── render/                       ← NEW: shared rendering primitives (M7-01/02)
│   ├── mod.rs
│   ├── ansi.rs                   ← MOVED+EXPANDED from src/ansi.rs (256/truecolor/cursor)
│   ├── markdown.rs               ← NEW: markdown → styled lines (pulldown-cmark)
│   ├── syntax.rs                 ← NEW: syntect wrapper (lang detect + theme)
│   └── diff.rs                   ← NEW: StructuredDiff (similar + syntect)
├── components/
│   ├── messages/                 ← EXPANDED: +~22 renderers (M7-04/05)
│   │   ├── thinking.rs, redacted_thinking.rs, compact_boundary.rs,
│   │   │   system_text.rs, system_api_error.rs, rate_limit.rs, shutdown.rs,
│   │   │   advisor.rs, hook_progress.rs, plan_approval.rs,
│   │   │   grouped_tool_use.rs, collapsed_read_search.rs,
│   │   │   bash_input.rs, bash_output.rs, command.rs,
│   │   │   local_command_output.rs, memory_input.rs, plan.rs,
│   │   │   prompt.rs, resource_update.rs, image.rs, attachment.rs
│   │   └── mod.rs                ← dispatch table extended
│   ├── virtual_message_list.rs   ← NEW: windowed scrollback (M7-03)
│   ├── prompt_input/             ← REFACTOR: prompt_input.rs → submodule (M7-06..10)
│   │   ├── mod.rs                (editor core; multi-line)
│   │   ├── vim.rs                (mode state machine, motions, operators)
│   │   ├── palette.rs            (/ command autocomplete)
│   │   ├── completion.rs         (@ file-ref completion)
│   │   ├── history_search.rs     (Ctrl-R style)
│   │   ├── image_paste.rs        (bracketed paste + image protocol detect)
│   │   └── footer.rs             (suggestions / help-menu / mode-indicator)
│   └── message_selector.rs       ← NEW: search/transcript/export (M7-14)
├── screens/                      ← EXPANDED: +4 screen areas
│   ├── repl.rs                   (existing)
│   ├── doctor.rs                 ← NEW (M7-11)
│   ├── resume.rs                 ← NEW (M7-12, replaces M5-08 stdio picker view)
│   ├── settings/                 ← NEW (M7-13)
│   │   ├── mod.rs, config.rs, settings.rs, status.rs, usage.rs
│   └── memory.rs                 ← NEW (M7-14)
├── theme.rs                      ← EXPANDED: multi-theme + picker (M7-15)
└── telemetry.rs                  ← EXPANDED: screen/vim/search events
```

**Why no new crate:** M7 is the same crate's surface deepened. The engine boundary (`OrchestratorHandle`, `lingxi-orchestrator`) is untouched — M7 adds zero engine wiring (that's M8). Keeping it in `lingxi-tui` preserves the clean workspace DAG.

### 2.2 New dependencies (pinned)

| Crate | Purpose | Pin discipline |
|---|---|---|
| `syntect` | syntax highlighting (code blocks + diff coloring) | pin exact; verify MSRV 1.82; `fancy-regex` backend (pure-Rust, avoids `onig` C dep) |
| `similar` | diff computation (line + word level) for StructuredDiff | pin exact |
| `pulldown-cmark` | markdown parsing → events the renderer styles | pin exact; standard pure-Rust CommonMark parser |

All deps verified against MSRV 1.82 and the iocraft/crossterm stack before locking in M7-01/M7-02. Same exact-pin discipline as `iocraft = "=0.8.3"`. If `syntect` won't pin cleanly on 1.82, fall back to `two-face` or a minimal tokenizer (documented escape hatch — see §4 R2).

### 2.3 Data-flow changes

- **VirtualMessageList replaces the capped buffer.** Today `AppState.messages: Vec<RenderedMessage>` is capped at 500 and fully re-rendered. M7-03 introduces a windowed view: the full message log is retained, but only the rows intersecting the viewport (+ a small overscan) are rendered to iocraft elements each frame. Scroll offset drives the window. The renderer becomes `render_window(&[RenderedMessage], offset, viewport_height)`. A per-message rendered-height cache backs the offset math.
- **Markdown/syntax/diff are pure functions.** `render::markdown::render(text, theme) -> Vec<StyledLine>`, `render::syntax::highlight(code, lang, theme) -> Vec<StyledLine>`, `render::diff::render(old, new, theme) -> Vec<StyledLine>`. Renderers call these; no new async or state. Testable in isolation (snapshots).
- **Screens are modal overlays / route states.** A new `AppState.active_screen: Option<Screen>` enum (Doctor/Resume/Settings/Memory). When `Some`, the REPL screen yields to the active screen; the keymap routes to it (same focus-trap discipline as M6's permission dialogs). `Esc` / `q` returns to REPL.
- **Vim mode is PromptInput-local state.** `PromptInput` gains a `vim: VimState { mode: Normal|Insert|Visual, pending_operator, count, register }`. The keymap consults vim state before falling through to default editing. No AppState change beyond the input widget.

### 2.4 Theme system

`theme.rs` expands from M6's fixed palette to a `Theme` struct + a registry of named themes (dark/light + claude-code's set). The active theme is in `AppState.theme`. The theme picker screen (or `/theme` command) sets it. Code-block highlighting maps the active theme to a syntect `.tmTheme` (bundled). StatusLine, message colors, and diff colors all read from the active `Theme`.

### 2.5 Live-key routing priority (carries the M6 focus-trap lesson)

M6's final review caught a ship-blocker: a live-key path that bypassed the permission focus-trap. M7 adds more contenders for live keys (screens, vim, palette, completion, history-search). All route through the **single** `handle_live_key` dispatcher established in M6, which branches in strict priority order:

```
1. pending_permission.is_some()      → permission dialog (M6)
2. active_screen.is_some()           → active screen (M7)
3. palette / completion / history    → active input overlay (M7)
4. prompt_input.vim or default edit  → input widget (M7)
5. scrollback nav                    → VirtualMessageList (M7)
```

One dispatcher, one priority order, one place to test the seams. No parallel key path is introduced (the M6 bug was a parallel path).

### 2.6 What does NOT change

- `lingxi-orchestrator`, `OrchestratorHandle` trait + impl — **zero engine changes in M7**.
- All v0.7.0 parity fixtures + M6 component behavior continue passing.
- `-p` print mode and `--no-tui` stdio REPL — untouched. M5-08 stdio resume picker stays as the `--no-tui` fallback; M7-12 adds an iocraft *view* over the same loader.
- JSONL format, slash command surface (99), session/resume engine.
- Telemetry: additive only (new screen/vim/search names; no renames).

### 2.7 Telemetry

New events for the new interactive surface (exact set + count locked in M7-16):
- Screen lifecycle: `tengu_tui_screen_opened` / `tengu_tui_screen_closed` (screen-name attribute), or per-screen events if claude-code distinguishes.
- Vim: `tengu_tui_vim_mode_entered` (aggregated, not per-keystroke).
- Search/palette: `tengu_tui_search_opened`, `tengu_tui_command_palette_opened`.
- `tengu_tui_key_pressed` (the aggregated counter deferred from M6) — registered here **only if** the windowed aggregator is built; otherwise stays deferred and documented.
- `lingxi_core_v0_8_0_released` (once-guarded release marker, follows the v0.7.0 pattern).

Every registered name gets a real emit site (M6 discipline). Final count audited and locked in M7-16 — reported as the real number, not an estimate (the M6 "330→326" lesson).

### 2.8 Literal lock discipline (carried from M6 §2.8)

Every user-visible string the TUI renders matches claude-code's source byte-for-byte unless there's an explicit reason to diverge. The implementer for each renderer/screen reads the equivalent claude-code `.tsx` first and copies the exact literal. **Exception:** syntax-highlight per-token colors (parity = equivalent look; see §0 Q3). M7-16 extends the literal-lock catalog (`docs/superpowers/literals/`) with the new renderers + screens.

---

## §3 Per-Sub-Plan Deliverables

16 sub-plans, sequential, each ending with annotated tag `m7.N` and two-stage review before the next.

### M7-01 — Full ANSI parser + markdown foundation
**Goal:** Upgrade M6's 8/16-color ANSI parser to full fidelity; add markdown rendering scaffold.
**Lands:** `render/` module created; `ansi.rs` moved+expanded (256-color, truecolor `38;2;r;g;b`, cursor-move/erase sequences handled or safely skipped); `render/markdown.rs` parses CommonMark (pulldown-cmark) → `Vec<StyledLine>` (headings, bold/italic, lists, blockquote, inline code, links). Code fences emit a placeholder span (filled by M7-02). Dep verification: `pulldown-cmark` pinned + MSRV 1.82 confirmed.
**Tests:** ANSI snapshots (256 + truecolor + reset + malformed); markdown snapshots (each element + partial/streaming unclosed fence). **Tag:** `m7.1`

### M7-02 — syntect highlighting + StructuredDiff viewer
**Goal:** Wire `syntect` + `similar`; render highlighted code blocks and diffs.
**Lands:** `render/syntax.rs` (lang detection from fence info-string + path; theme→`.tmTheme` map; `fancy-regex` backend); `render/diff.rs` (`similar` line+word diff → green/red + syntax-colored hunks, claude-code StructuredDiff layout); markdown code fences now highlight; Edit/Write tool results render as StructuredDiff. **Gate:** syntect builds on MSRV 1.82 with full dep tree (else fall back per §4 R2).
**Tests:** syntax snapshots (rust/python/js/json/unknown-lang fallback); diff snapshots (add/remove/modify/word-level). **Tag:** `m7.2`

### M7-03 — VirtualMessageList
**Goal:** Windowed scrollback replacing the capped-500 buffer.
**Lands:** `components/virtual_message_list.rs`; retains full log, renders only viewport+overscan; per-message rendered-height cache backs offset math; `g`/`G`/PgUp/PgDn/`j`/`k` preserved; variable-height messages handled. AppState migrates from `Vec` cap to full retention. **Gate:** correct mixed-height rendering at 5k messages (else degrade per §4 R3).
**Tests:** behavior (5k messages → only window rendered; scroll math; variable-height offset correctness; exact first/last visible at given offset); perf smoke (render count bounded by viewport, not log size). **Tag:** `m7.3`

### M7-04 — Message renderers batch 1 (system/assistant — 10)
**Goal:** thinking, redacted-thinking, compact-boundary, system-text, system-api-error, rate-limit, shutdown, advisor, hook-progress, plan-approval.
**Lands:** 10 renderer files + dispatch entries; each reads claude-code TSX first (literal lock). CompactBoundaryMessage replaces M6-08's `[Compacted]` SystemText placeholder.
**Tests:** snapshot per renderer (collapsed/expanded where applicable). **Tag:** `m7.4`

### M7-05 — Message renderers batch 2 (user — 12)
**Goal:** bash-input, bash-output, command, local-command-output, memory-input, plan, prompt, resource-update, image, attachment, grouped-tool-use, collapsed-read-search.
**Lands:** ~12 renderer files + dispatch; image renderer shows placeholder/metadata (terminal image protocols are M8); grouped-tool-use + collapsed-read-search implement claude-code's folding.
**Tests:** snapshot per renderer. **Tag:** `m7.5`

### M7-06 — PromptInput multi-line + footer
**Goal:** Refactor `prompt_input.rs` → `prompt_input/` submodule; multi-line editing + footer surface.
**Lands:** multi-line buffer (newline insert, cursor up/down across lines, wrap); `footer.rs` (suggestions, help-menu, mode-indicator, placeholder). PromptInput height grows 1→N rows.
**Tests:** behavior (multi-line edits, cursor nav, height calc); footer snapshots. **Tag:** `m7.6`

### M7-07 — Command palette + `@` completion
**Goal:** `/` slash-command autocomplete dropdown + `@` file-ref completion.
**Lands:** `palette.rs` (live-filter the 99 commands, arrow-select, Tab/Enter complete); `completion.rs` (`@` triggers path completion against cwd, fuzzy filter). Both render as overlays above PromptInput with focus-trap (priority 3 in §2.5).
**Tests:** behavior (filter, select, complete, dismiss; focus-trap); snapshots. **Tag:** `m7.7`

### M7-08 — Vim mode 1 (normal/insert + motions)
**Goal:** Vim normal/insert modes + motions.
**Lands:** `vim.rs` `VimState` machine; `Esc`/`i`/`a`/`o`/`I`/`A`/`O` mode transitions; motions `h j k l w b e 0 $ ^ gg G f/t`; counts (`3w`). Mode-indicator in footer reflects state.
**Tests:** behavior (each motion, mode transitions, counts) against claude-code vim semantics. **Tag:** `m7.8`

### M7-09 — Vim mode 2 (visual + operators)
**Goal:** Visual mode + operators.
**Lands:** visual/visual-line modes; operators `d c y x p P` + operator-motion combos (`dw cc d$ yy`); register (yank/paste). Completes core vim fidelity. **Gate:** operator×motion matrix passes (obscure cases — `.` repeat, macros, ex-commands — may defer to M8 with a documented "vim parity subset" line per §4 R1).
**Tests:** behavior (operator×motion matrix, visual selection, yank/paste, counts). **Tag:** `m7.9`

### M7-10 — History search + image paste
**Goal:** Ctrl-R history search; bracketed-paste + image paste.
**Lands:** `history_search.rs` (reverse incremental search over prompt history); `image_paste.rs` (bracketed-paste handling; image protocol detect — kitty/iTerm2 — placeholder ref insertion `[Image #N]`; NOT inline display). Multi-line paste handled cleanly.
**Tests:** behavior (history search filter/cycle/accept; paste splitting; image ref insertion). **Tag:** `m7.10`

### M7-11 — Doctor screen
**Goal:** iocraft Doctor screen (claude-code Doctor.tsx parity).
**Lands:** `screens/doctor.rs`; diagnostics rows (versions, config paths, MCP configured/connected status, auth state, terminal capabilities); `active_screen` overlay + keymap routing (priority 2 in §2.5) + `Esc` return.
**Tests:** snapshot (fixed diagnostic state); behavior (open/close routing). **Tag:** `m7.11`

### M7-12 — Resume screen (iocraft)
**Goal:** Replace M5-08 stdio picker view with an iocraft Resume screen.
**Lands:** `screens/resume.rs`; lists recent sessions (reuses M5-08 loader — no engine change), preview, arrow-select, Enter resumes; `--resume` (no id) opens it. M5-08 stdio picker stays as `--no-tui` fallback.
**Tests:** behavior (list/select/resume; empty state); snapshot. **Tag:** `m7.12`

### M7-13 — Settings screens
**Goal:** Config / Settings / Status / Usage screens (claude-code Settings dir parity).
**Lands:** `screens/settings/` (4 sub-screens); reads real settings (M3 settings store); writes through existing M3 stores only (no new persistence logic — §4 R7); Status/Usage render against current engine (Usage = flat cost until M8 per-model; documented). Tab navigation between sub-screens.
**Tests:** snapshot per sub-screen; behavior (tab nav, settings read). **Tag:** `m7.13`

### M7-14 — Memory editor + MessageSelector
**Goal:** Memory file editor + search/transcript/export.
**Lands:** `screens/memory.rs` (MemoryFileSelector + edit, reuses M3 memory store); `components/message_selector.rs` (search messages, jump-back, export transcript to a sane default path). `/export`, search keybind wired.
**Tests:** behavior (memory select/edit; search filter/jump; export writes file); snapshots. **Tag:** `m7.14`

### M7-15 — Theme picker
**Goal:** Multi-theme system + picker.
**Lands:** `theme.rs` expanded to `Theme` registry (claude-code's named themes); picker screen/`/theme`; live re-render on switch; code-block syntect theme follows active theme.
**Tests:** snapshot (≥2 themes for StatusLine + a message + a code block); behavior (switch applies). **Tag:** `m7.15`

### M7-16 — Parity fixtures + release v0.8.0
**Goal:** Cross-cutting validation, version bump, release.
**Lands:** ≥2 new parity fixtures (`parity_tui_renderers_m7` for the new renderers/markdown/syntax/diff structure; `parity_tui_screens` for screen flows); literal-lock catalog extended; telemetry count audited + locked; all `Cargo.toml` 0.7.0→0.8.0; release doc + CHANGELOG + README; annotated `m7.16` + `v0.8.0`. **Final review** explicitly probes cross-state seams (§5.6): screen open while permission pending; palette in vim normal mode; paste while a screen is open.
**Tests:** full cumulative suite; v0.7.0 fixtures still pass; release marker emits once. **Tag:** `m7.16` + `v0.8.0`

### Summary

| Sub-plan | Lands | Tag |
|---|---|---|
| M7-01 | Full ANSI + markdown foundation | m7.1 |
| M7-02 | syntect + StructuredDiff | m7.2 |
| M7-03 | VirtualMessageList | m7.3 |
| M7-04 | Renderers batch 1 (system/assistant) | m7.4 |
| M7-05 | Renderers batch 2 (user) | m7.5 |
| M7-06 | PromptInput multi-line + footer | m7.6 |
| M7-07 | Command palette + `@` completion | m7.7 |
| M7-08 | Vim mode 1 (motions) | m7.8 |
| M7-09 | Vim mode 2 (operators) | m7.9 |
| M7-10 | History search + image paste | m7.10 |
| M7-11 | Doctor screen | m7.11 |
| M7-12 | Resume screen | m7.12 |
| M7-13 | Settings screens | m7.13 |
| M7-14 | Memory editor + MessageSelector | m7.14 |
| M7-15 | Theme picker | m7.15 |
| M7-16 | Parity + release v0.8.0 | m7.16 + v0.8.0 |

**Estimate:** 16 sub-plans, ~190 tasks, ~5.5 calendar weeks at sustained pace. Variance ±1.5 weeks (vim parity + VirtualMessageList are the drivers).

---

## §4 Risk Register

### High-impact risks

| # | Risk | Impact | Probability | Mitigation |
|---|---|---|---|---|
| R1 | **Vim byte-parity is fiddly** (operator-pending timeouts, count×motion, register semantics, visual boundaries) | High | High | Split across M7-08/09. Behavior-test the operator×motion matrix exhaustively. Accept a documented "vim subset parity" line for obscure cases (`.` repeat, macros, ex-commands) — defer those to M8 rather than block. |
| R2 | **syntect MSRV / dep conflict** on Rust 1.82 or with iocraft/crossterm tree | High — blocks M7-02+ | Medium | M7-01/02 verify full dep tree against 1.82 before renderer code. Use `fancy-regex` (pure-Rust) backend. Fallback: `two-face` or minimal tokenizer (documented escape hatch). |
| R3 | **VirtualMessageList variable-height correctness** — off-by-one row math with mixed-height messages | High — scrollback is core | Medium-high | Per-message height cache + offset tests against mixed-height fixtures. Fallback: raise M6 cap (500→large N) + render-all with perf ceiling (documented degrade). |
| R4 | **Screen-overlay routing regressions** — re-break the M6 permission focus-trap | High | Medium | Single `handle_live_key` dispatcher with strict priority order (§2.5). Behavior test the cross-state seam (screen open while permission pending). |

### Medium-impact risks

| # | Risk | Impact | Probability | Mitigation |
|---|---|---|---|---|
| R5 | Markdown/ANSI edge cases (nested lists, tables, partial streaming fence, CJK width) | Medium | Medium | M7-01 best-effort partial markdown (no panic on unclosed fence); `unicode-width`; snapshot tricky cases. |
| R6 | Renderer breadth → literal drift (22 renderers) | Medium | High | Implementer reads TSX first; literal-lock catalog extended M7-16; parity fixture locks high-value strings. |
| R7 | Settings/Memory write-back scope creep | Medium | Medium | M7 reads real data, writes through existing M3 stores only. Engine-change needs → defer that piece; keep M7 surface-only. |
| R8 | Image paste terminal-protocol rabbit hole | Medium | Medium | M7-10 scopes to detection + ref insertion only. Inline display → M8 terminal-protocol cluster. |

### Low-impact risks

| # | Risk | Impact | Probability | Mitigation |
|---|---|---|---|---|
| R9 | Theme/`.tmTheme` bundling size | Low | Low | Curated theme set; lazy-load. |
| R10 | MessageSelector/export file-write safety | Low | Low | Default export path (`~/.lingxi/exports/` or cwd); no arbitrary overwrite without confirm. |
| R11 | Telemetry count audit drift (M6 330-vs-326 lesson) | Low | Medium | M7-16 audits actual count, locks it; release doc reports real number. |

### Hard gates / fallback decisions

1. **End of M7-02:** syntect builds on MSRV 1.82 with full dep tree. If NOT → `two-face`/minimal tokenizer (R2), revise, continue.
2. **End of M7-03:** VirtualMessageList renders mixed-height content correctly at 5k. If NOT → raised-cap render-all with perf ceiling (R3), continue.
3. **End of M7-09:** vim operator×motion matrix passes. If obscure cases balloon → defer them to M8 with documented "vim parity subset" line (R1); core vim still ships.

None block the *milestone* — each degrades scope, not schedule.

---

## §5 Verification

### 5.1 Test categories (hybrid, unchanged from M6)

| Layer | Tool | Scope | Catches |
|---|---|---|---|
| Unit | `cargo test` | Pure fns (ANSI, markdown, syntax, diff, vim math, scroll-window, fuzzy filter) | Logic bugs |
| Behavior | `cargo test` + driver | Component event→state→render via the same fn the live mount calls | State machine, focus-trap, vim, scroll |
| Snapshot | `insta` | Stable rendered views | Label/color/layout regressions |
| Integration | `expectrl`/`rexpect` | Whole binary via PTY | End-to-end smoke, screen open/close, terminal restore |

### 5.2 Per-area test budget

| Area | Snapshots | Behavior |
|---|---|---|
| ANSI parser | 6+ | — |
| Markdown | 8+ | — |
| syntect + diff | 8+ | — |
| VirtualMessageList | 2 | 6+ (window math, mixed height, 5k perf) — **critical** |
| Renderers (M7-04/05) | ~30 | a few (folding) |
| PromptInput multi-line | 2 | 6+ |
| Palette + `@` | 2 | 6+ (incl. focus-trap) |
| Vim | — | 20+ (motion + operator×motion matrix, counts, visual, registers) — **critical** |
| History/paste | 1 | 6+ |
| Screens (×4 areas) | 1-2 each | open/close + cross-state seam |
| Theme picker | ≥2 themes × 3 surfaces | switch applies |

**Targets:** ~60 snapshots, ~70 behavior tests, ~6 PTY smokes by M7-16.

### 5.3 Parity fixtures (≥2 new in M7-16)

- **`parity_tui_renderers_m7.json` + driver** — new renderers' high-value strings/markers, markdown element structure, syntax-highlight *structure* (not per-token color), StructuredDiff layout (+/− markers, hunk headers).
- **`parity_tui_screens.json` + driver** — scripted screen flows: Doctor open→rows→close; Resume list→select; Settings tab nav; Memory open→edit→save; search→jump→export.
- v0.7.0's `parity_tui_renderers` + `parity_tui_repl_loop` continue passing.

### 5.4 Workspace verification gate (every sub-plan)

Run **from inside `lingxi-core/`** (toolchain pins rust 1.82.0; running from repo root uses host toolchain → spurious lint noise; this bit M6-08):

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace --target {x86_64-unknown-linux-gnu, x86_64-apple-darwin, x86_64-pc-windows-gnu, aarch64-linux-android, aarch64-apple-ios}
```

Known flakes (allowed rerun): `rapid_writes_collapse_to_single_event`, `writer_output_equals_single_turn_fixture`, `streaming_concurrent_tools_test`, `lingxi-platform-posix` fs_watch FSEvents timing tests.

### 5.5 Manual verification checklist

Headless agents can't drive a real terminal (v0.7.0 gap #5). Each sub-plan lists a human smoke item; the **final human smoke** before v0.8.0 covers: markdown+code renders; a real Edit shows a colored diff; 5k-message scroll smooth; vim `dw`/`/`palette/`@`complete work; Doctor/Resume/Settings/Memory open+close; theme switch applies. Run on tmux + a truecolor terminal (alacritty/iTerm2/kitty).

### 5.6 Final-review discipline (carried from M6)

M6's final holistic review caught a ship-blocker (unwired permission focus-trap) — a cross-state *seam* between sub-plans that per-sub-plan tests missed. M7 has more such seams (screens × permissions × vim × palette contend for live keys). **The M7-16 final review explicitly probes cross-state seams:** open a screen while a permission is pending; trigger the palette in vim normal mode; paste while a screen is open. Named step, not an afterthought.

### 5.7 Performance budget

| Metric | Budget | Where |
|---|---|---|
| Scroll render cost (5k messages) | bounded by viewport, not log size | M7-03 perf smoke |
| Frame render with highlighted code + diff | <16ms typical | M7-02 informal bench |
| Memory across 5k-message session | <100MB | M7-16 informal |

Soft budgets — violations documented and triaged M7 vs M8, not auto-blocking.

---

## §6 Schedule

### 6.1 Cadence

Single-Claude pace, sequential. Calibrated from M6 (9 sub-plans ~3.5wk) and M5 (14 ~3-4wk).

| Sub-plan | Tasks | Calendar | Cumulative |
|---|---|---|---|
| M7-01 | 12-14 | 2-3d | 3d |
| M7-02 | 12-14 | 2-3d | 6d |
| M7-03 | 12-14 | 2-3d | 9d |
| M7-04 | 12-14 | 2d | 11d |
| M7-05 | 12-14 | 2d | 13d |
| M7-06 | 10-12 | 2d | 15d |
| M7-07 | 12-14 | 2d | 17d |
| M7-08 | 12-14 | 2-3d | 20d |
| M7-09 | 12-14 | 2-3d | 23d |
| M7-10 | 10-12 | 2d | 25d |
| M7-11 | 10-12 | 1-2d | 27d |
| M7-12 | 10-12 | 1-2d | 28d |
| M7-13 | 14-16 | 2-3d | 31d |
| M7-14 | 12-14 | 2d | 33d |
| M7-15 | 8-10 | 1-2d | 35d |
| M7-16 | 14-16 | 2-3d | 38d |

**Total: ~190 tasks, ~5.5 calendar weeks. Variance ±1.5 weeks** (vim + VirtualMessageList drive it).

### 6.2 Dependencies

```
M7-01 (ANSI+markdown) ──┬─→ M7-02 (syntect+diff, needs markdown fences)
                        │     └─→ renderers (M7-04/05 use markdown+syntax+diff)
                        │     └─→ M7-13 Settings, M7-15 theme
M7-03 (VirtualMessageList) ─→ renderers plug into it
M7-06 (multi-line) ──→ M7-07 (palette/@), M7-08/09 (vim), M7-10 (history/paste)
Screens (M7-11..14) depend on M7-01/02; otherwise independent
M7-15 (theme) depends on M7-02 + M7-01
M7-16 depends on everything
```

Critical path: M7-01 → M7-02 → renderers/screens; and M7-06 → M7-07/08/09/10. Sequential by discipline.

### 6.3 Slip handling

>1.5× estimate → (1) scope creep: defer slice to M8, document; (2) real blocker: BLOCKED, escalate; (3) hidden dependency: precursor sub-plan M7-Na. Vim (M7-08/09) and VirtualMessageList (M7-03) carry pre-authorized degrade paths (§4) so they slip scope, not schedule.

### 6.4 Tag and release policy

Per-sub-plan annotated tags `m7.1`…`m7.16`, local only. Release tag `v0.8.0` in M7-16. **No remote push from Claude.** No force-push / skip-hooks / amends.

### 6.5 Worktree strategy

Dedicated worktree `m7-execution` on branch `m7-execution`, created at M7-01 via `superpowers:using-git-worktrees`. After M7-16 lands `v0.8.0`, fast-forward merge to `main` via `superpowers:finishing-a-development-branch` (Option 1), with the cross-state-seam review (§5.6) before merge.

### 6.6 After M7

1. **Pause for review** — no auto-progression to M8.
2. **M8 brainstorm** — TUI Advanced + 5 deferred engine items: Coordinator/Team/Swarm UI, Voice, grove, IDE bridge dialogs, mouse mode, terminal image protocols, full terminal-protocol cluster, *plus* real LLM compaction summary, AnalyticsBus, MCP auto-connect, OAuth PKCE, per-model cost. Heaviest milestone (UI + backend) — likely splits at its own brainstorm.
3. **Then v1.0.0** — polish + release.

---

## References

- M6 design (predecessor): `docs/superpowers/specs/2026-05-28-m6-tui-foundation-design.md`
- v0.7.0 release notes: `docs/superpowers/releases/2026-05-29-v0.7.0.md`
- M6 literal-lock catalog: `docs/superpowers/literals/m6-tui-literals.md`
- claude-code source: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/`
  - `screens/Doctor.tsx`, `screens/ResumeConversation.tsx` — screen references
  - `components/Settings/{Config,Settings,Status,Usage}.tsx` — settings screen references
  - `components/messages/*.tsx` — the 22 renderer references
  - `components/PromptInput/*` — advanced input references (vim, palette, completion, footer)
  - `components/StructuredDiff/colorDiff.ts`, `components/memory/MemoryFileSelector.tsx`
  - `utils/cliHighlight.ts`, `utils/markdown.ts` — highlight/markdown references
- `syntect`, `similar`, `pulldown-cmark` — crate docs (verify MSRV 1.82 at M7-01/02)

---

**End of design.**
