# M7 TUI Literal Lock Catalog

> **Purpose**: Per spec §2.8, every user-visible string the M7 TUI surface
> renders must match claude-code's source byte-for-byte unless there is an
> explicit documented reason to diverge. This file is the **canonical index** of
> the M7 renderer + screen strings, indexed against their claude-code source and
> the LingXi adoption site at `file:line` precision. Extends the M6 catalog
> (`m6-tui-literals.md`) for the M7-01..M7-16 surface.
>
> **Scope**: M7-01..M7-16 inclusive (rendering primitives → screens → release).
>
> **claude-code reference**: the `claude-code/` submodule is **not checked out**
> in this worktree. claude-code references use the `2026-05-28-snapshot`
> convention established by the M6 catalog + the M7 parity fixtures
> (`parity_tui_renderers_m7.json` / `parity_tui_screens.json`
> `_claude_code_version`), not live `.tsx` line numbers. LingXi sites carry real
> `file:line` against the v0.8.0 tree (paths relative to `lingxi-core/`). On the
> next `claude-code/` submodule bump, re-extract the `.tsx` literals (see §11)
> and back-fill the live line numbers.
>
> **Audit cadence**: re-verify on every claude-code upgrade and on every M8+
> feature that touches the TUI surface.
>
> **EXPLICIT EXCEPTION (spec §0 Q3)**: syntax-highlight **per-token colors** are
> NOT locked here. Parity for syntect output is *equivalent look*, not
> byte-identical ANSI. This catalog locks markdown element **structure**, the
> diff `+`/`-` markers + hunk-header **format**, and the renderer/screen
> **strings** — never per-token highlight color. See §3 / §4 / §10.

## §1 Message renderers — system/assistant (M7-04)

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 1.1 | `∴ ` (U+2234 + space) thinking marker | `crates/tui/src/components/messages/thinking.rs:18` | `THINKING_MARKER` |
| 1.2 | `(ctrl+o to expand)` | `crates/tui/src/components/messages/thinking.rs:21` | `EXPAND_HINT` (collapsed thinking) |
| 1.3 | `✻ Conversation compacted (ctrl+o for history)` (U+273B) | `crates/tui/src/components/messages/compact_boundary.rs:12` | `BOUNDARY_LINE` — replaces the M6-08 placeholder |
| 1.4 | `(ctrl+o to expand)` | `crates/tui/src/components/messages/system_api_error.rs:15` | `EXPAND_HINT` (truncated api-error) |
| 1.5 | `✔` (U+2714) advisor tick | `crates/tui/src/components/messages/advisor.rs:26` | `TICK` (figures.tick) |
| 1.6 | `Advisor has reviewed the conversation and will apply the feedback` | `crates/tui/src/components/messages/advisor.rs:28` | `REVIEWED_LINE` |
| 1.7 | `✓` / `✗` (U+2713 / U+2717) plan-approval check/cross | `crates/tui/src/components/messages/plan_approval.rs:19,21` | `CHECK` / `CROSS` |
| 1.8 | shutdown rejected tail | `crates/tui/src/components/messages/shutdown.rs:15` | `REJECTED_TAIL` |

## §2 Message renderers — user (M7-05)

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 2.1 | `! ` bash-input prefix | `crates/tui/src/components/messages/bash_input.rs:18` | `PREFIX` |
| 2.2 | `  ⎿  ` (2sp + U+23BF + 2sp) gutter | `crates/tui/src/components/messages/local_command_output.rs:21` | `GUTTER` (IndentedContent) |
| 2.3 | `(no content)` | `crates/tui/src/components/messages/local_command_output.rs:24` | `NO_CONTENT_MESSAGE` |
| 2.4 | `● ` (U+25CF + space) group marker | `crates/tui/src/components/messages/grouped_tool_use.rs:28` | `MARKER` |
| 2.5 | `  ⎿  ` collapsed-read-search gutter | `crates/tui/src/components/messages/collapsed_read_search.rs:22` | `GUTTER` |
| 2.6 | `Got it.` memory-saved | `crates/tui/src/components/messages/memory_input.rs:24` | `SAVING_MESSAGE` |
| 2.7 | `[Image #N]` / `[Image]` / `[Image #N] (WxH)` | `crates/tui/src/components/messages/image.rs:19,20` | `render_image_label` placeholder |

## §3 Markdown structure (M7-01)

Structure only (element markers + one `StyledLine` per visual line) — NOT
per-token color. Locked by `parity_tui_renderers_m7.rs::markdown_*`.

| # | Literal / structure | LingXi site | Notes |
|---|---|---|---|
| 3.1 | `│` (U+2502) blockquote bar | `crates/tui/src/render/markdown.rs:26` | `BLOCKQUOTE_BAR` (dim) |
| 3.2 | heading → text-only line (no `#`) | `crates/tui/src/render/markdown.rs:79` (`render`) | e.g. `# Title` → `Title` |
| 3.3 | list item → `- {item}` line | `crates/tui/src/render/markdown.rs:79` | one `StyledLine` per item |
| 3.4 | inline-code / bold / italic → flattened text run | `crates/tui/src/render/markdown.rs:79` | structure; color is the inline-code theme color |
| 3.5 | fenced block → ``` fences DROPPED, one `StyledLine` per code line | `crates/tui/src/render/markdown.rs` (code-fence branch) | syntect-colored body (color NOT locked, §10) |

## §4 StructuredDiff (M7-02)

| # | Literal / format | LingXi site | Notes |
|---|---|---|---|
| 4.1 | `+` add sigil | `crates/tui/src/render/diff.rs:117` | `sigil(LineKind::Add)` |
| 4.2 | `-` remove sigil | `crates/tui/src/render/diff.rs:118` | `sigil(LineKind::Remove)` |
| 4.3 | gutter `{line_no:>w} {sigil} ` | `crates/tui/src/render/diff.rs:126` | right-aligned line number + sigil + space |
| 4.4 | `@@ -{os},{ol} +{ns},{nl} @@` hunk header | `crates/tui/src/render/diff.rs:270` | `hunk_header` (claude-code colorDiff format) |

## §5 Doctor screen (M7-11)

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 5.1 | `Diagnostics` (bold section header) | `crates/tui/src/screens/doctor.rs:148` | first section header |
| 5.2 | `Terminal` (bold section header) | `crates/tui/src/screens/doctor.rs:156` | LingXi divergence from claude-code's `Updates` (see review note `doctor.rs:128`) — LingXi shows real terminal capabilities, not auto-update status |
| 5.3 | `none configured` (MCP) | `crates/tui/src/screens/doctor.rs:132` | MCP row when total == 0 |
| 5.4 | `configured, not connected` (MCP) | `crates/tui/src/screens/doctor.rs:134` | total > 0 & connected == 0 |
| 5.5 | `unknown` (auth state) | `crates/tui/src/screens/doctor.rs:69` | `auth_state` until OAuth lands (M8) |
| 5.6 | `lingxi-cli v{VERSION}` | `crates/tui/src/screens/doctor.rs:63` | `cli_version` |

## §6 Resume screen (M7-12)

| # | Literal / format | LingXi site | Notes |
|---|---|---|---|
| 6.1 | `(1 message)` / `({n} messages)` count label | `crates/tui/src/screens/resume.rs:55-58` | `ResumeRow::from_meta` count_label (singular/plural) |
| 6.2 | RFC3339-seconds timestamp body | `crates/tui/src/screens/resume.rs` (`from_meta`) | via shared `format_rfc3339_seconds` (byte-identical to the M5-08 stdio picker) |

## §7 Settings screens (M7-13)

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 7.1 | `Config` tab title | `crates/tui/src/screens/settings/mod.rs:82` | `SettingsTab::title` |
| 7.2 | `Settings` tab title | `crates/tui/src/screens/settings/mod.rs:83` | |
| 7.3 | `Status` tab title | `crates/tui/src/screens/settings/mod.rs:84` | |
| 7.4 | `Usage` tab title | `crates/tui/src/screens/settings/mod.rs:85` | flat cost (per-model breakdown is M8) |

## §8 Memory screen + MessageSelector (M7-14)

| # | Literal / format | LingXi site | Notes |
|---|---|---|---|
| 8.1 | `Saved {path}` memory save status | `crates/tui/src/root.rs:333` (Memory arm) | success status |
| 8.2 | `Could not save memory: {e}` | `crates/tui/src/root.rs:336` | save error |
| 8.3 | `Conversation exported to: {path}` | `crates/tui/src/components/message_selector.rs:338` (`report_export`) | export success (§4 R10) |
| 8.4 | `Failed to export conversation: {err}` | `crates/tui/src/components/message_selector.rs:349` (`report_export`) | export I/O error |
| 8.5 | default export filename + `.txt` clamp | `crates/tui/src/components/message_selector.rs` (`default_export_filename` / `resolve_export_filename`) | basename-clamped to the export dir (path-traversal safe) |

## §9 PromptInput footer / vim mode indicator (M7-06/08/09)

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 9.1 | `-- NORMAL --` | `crates/tui/src/components/prompt_input/vim.rs:1567` | `mode_indicator(Normal)` |
| 9.2 | `-- INSERT --` | `crates/tui/src/components/prompt_input/vim.rs:1568` | `mode_indicator(Insert)` |
| 9.3 | `-- VISUAL --` | `crates/tui/src/components/prompt_input/footer.rs:182` | `v` (charwise) |
| 9.4 | `-- VISUAL LINE --` | `crates/tui/src/components/prompt_input/footer.rs:190` | `V` (linewise) — distinguished by the M7-09 footer fn |
| 9.5 | `shift + ⏎ for newline` newline hint | `crates/tui/src/components/prompt_input/footer.rs:129` | dim hint row |
| 9.6 | `? for shortcuts` help hint | `crates/tui/src/components/prompt_input/footer.rs:128` | dim hint row |

## §10 Documented divergences

- **Syntax-highlight per-token color EXCEPTION (spec §0 Q3)**: this catalog
  deliberately does NOT lock per-token highlight output. syntect parity =
  equivalent look (the right tokens get the right *kind* of emphasis), NOT
  byte-identical ANSI. The structure (line counts, fence boundaries) IS locked
  (§3.5); the colors are covered by the lingxi-tui insta snapshots, which are
  theme-stable but not claimed byte-identical to claude-code's highlighter.
- **Doctor second section header** (§5.2): LingXi shows `Terminal` (real
  terminal capabilities: truecolor + size) where claude-code's Doctor shows
  `Updates` (auto-update status). LingXi has no auto-updater, so the row is
  repurposed to terminal diagnostics. Documented at `doctor.rs:128`.
- **Numeric-key permission divergence** (carried from the M6 catalog §6): the
  permission dialogs accept `1`/`2`/`n` numeric keys; unchanged in M7.

## §11 Audit checklist (re-run on claude-code upgrade)

1. Re-extract the `.tsx` literals for each renderer/screen above and back-fill
   the live claude-code line numbers (replacing the `2026-05-28-snapshot`
   convention).
2. Diff each LingXi `file:line` literal against its claude-code source; any
   mismatch is a parity regression unless documented in §10.
3. Re-run `parity_tui_renderers_m7` + `parity_tui_screens` + the M6 fixtures;
   all must stay green.
4. Confirm the syntax-highlight exception still holds (no per-token color lock
   crept in).
5. Update this catalog + bump the audit date.
