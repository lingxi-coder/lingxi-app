# M6 TUI Literal Lock Catalog

> **Purpose**: Per spec §2.8, every user-visible string the TUI renders must
> match claude-code's source byte-for-byte unless there is an explicit
> documented reason to diverge. This file is the **canonical index** of every
> such string, indexed against its claude-code source and against the LingXi
> adoption site at `file:line` precision.
>
> **Scope**: M6-01..M6-09 inclusive (TUI foundation through release).
>
> **claude-code reference**: the `claude-code/` submodule is **not checked
> out** in this worktree. claude-code references therefore use the
> `2026-05-28-snapshot` reference established by the M6-05 / M6-07 parity
> fixtures (`tui_permission_dialogs.json` / `tui_listings.json`
> `_claude_code_version`), not live `.tsx` line numbers. LingXi sites carry
> real `file:line` against the v0.7.0 tree. On the next `claude-code/`
> submodule bump, re-extract the `.tsx` literals (see §8) and back-fill the
> live line numbers.
>
> **Audit cadence**: re-verify on every claude-code upgrade and on every M7+
> feature that touches the TUI surface.

## §1 StatusLine

LingXi site: `lingxi-code/crates/tui/src/components/status_line.rs`. claude-code
source: `src/components/StatusLine.tsx` (snapshot).

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 1 | field separator `" "` (single space) | `status_line.rs:66` (`format!`) | `"{model} {cwd} {cost} {ctx%} {mode}"` |
| 2 | cost prefix `$` (inside the cost string) | `status_line.rs:36` default `"$0.0000"` | M6-06 4-decimal parity |
| 3 | context-pct suffix `%` | `status_line.rs:67` (`"{:.0}%"`) | rounded to whole percent |
| 4 | mode label `default` | `status_line.rs:47` | `PermissionMode::Default` |
| 5 | mode label `plan` | `status_line.rs:48` | `PermissionMode::Plan` |
| 6 | mode label `acceptEdits` | `status_line.rs:49` | `PermissionMode::AcceptEdits` |
| 7 | mode label `bypassPermissions` | `status_line.rs:50` | `PermissionMode::BypassPermissions` |
| 8 | mode label `dontAsk` / `bubble` / `auto` | `status_line.rs:51-53` | remaining `PermissionMode` variants |

## §2 Spinner

LingXi site: `lingxi-code/crates/tui/src/components/spinner.rs`. claude-code
source: `src/components/Spinner.tsx:41` (snapshot).

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 1 | spinner frames `· ✢ ✳ ✶ ✻ ✽ ✽ ✻ ✶ ✳ ✢ ·` (12 frames) | `spinner.rs:17` (`SPINNER_FRAMES`) | claude-code `[...DEFAULT, ...reverse(DEFAULT)]` — NOT braille; locked byte-for-byte |
| 2 | verbs `Crunching` / `Thinking` / `Generating` | `spinner.rs:23` (`VERBS_M6`) | rotation cycle |
| 3 | spinner line format `"{frame} {verb}…"` (U+2026 ellipsis, not three dots) | `spinner.rs:56` (`format_spinner_line`) | single space between frame and verb |

## §3 PromptInput

LingXi site: `lingxi-code/crates/tui/src/components/prompt_input.rs`.
claude-code source: `src/components/PromptInput/PromptInput.tsx` (snapshot).

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 1 | prompt prefix `"> "` | `prompt_input.rs:109` (`format!("> {}", props.text)`) | rendered before the editable line |

## §4 Permission Dialogs

LingXi sites under `lingxi-code/crates/tui/src/components/permissions/`.
claude-code source: `src/components/permissions/PermissionRequest.tsx`
(snapshot). **Documented divergence**: LingXi uses numeric-key button labels
(`[1] Allow Once` / `[2] Allow Always` / `[N] Deny`) where claude-code's
React Select uses `Yes` / `Yes, and don't ask again…` / `No` — see §7 and
the `tui_permission_dialogs.json` `_comment`.

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 1 | tool-use header `"Claude needs your permission to use {tool}"` | `tool_use_confirm.rs:69` | `{}` = tool name |
| 2 | `"[1] Allow Once"` | `tool_use_confirm.rs:79`, `exit_plan_mode.rs:64` | numeric-key affordance |
| 3 | `"[2] Allow Always"` | `tool_use_confirm.rs:80`, `exit_plan_mode.rs:65` | |
| 4 | `"[N] Deny"` | `tool_use_confirm.rs:81`, `exit_plan_mode.rs:66` | |
| 5 | exit-plan header `"Claude Code needs your approval for the plan"` | `exit_plan_mode.rs:54` | |
| 6 | bypass title `"WARNING: Claude Code running in Bypass Permissions mode"` | `bypass_permissions.rs:75` | |
| 7 | bypass body1 (dangerous-commands warning) | `bypass_permissions.rs:76` | full sentence locked |
| 8 | bypass body2 (responsibility acceptance) | `bypass_permissions.rs:77` | full sentence locked |

## §5 Message Renderers

LingXi sites under `lingxi-code/crates/tui/src/components/messages/`.

### §5.1 UserTextMessage

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 1 | prefix `"> "` | `user_text.rs:21` (`format!("> {}", body)`) | default fg |

### §5.2 AssistantTextMessage

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 1 | marker `"● "` (U+25CF + space) | `assistant_text.rs:21` (`format!("● {}", body)`) | cyan |

### §5.3 AssistantToolUseMessage

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 1 | marker `"●"` (U+25CF) | `assistant_tool_use.rs:14` (`MARKER`) | space added in header `format!` |
| 2 | focus prefix `"> "` | `assistant_tool_use.rs:16` (`FOCUS_PREFIX`) | only when focused |
| 3 | collapsed header `"{prefix}● {tool}({preview})"` | `assistant_tool_use.rs:46` | single-line JSON preview with one space after `:` / `,` |

### §5.4 UserToolResultMessage

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 1 | marker `"└ "` (U+2514 + space) | `user_tool_result.rs:22` (`MARKER`) | dim-gray |
| 2 | indent `"  "` (2 spaces) | `user_tool_result.rs:24` (`INDENT`) | continuation lines |
| 3 | collapsed multi-line suffix `" (+{N} lines)"` | `user_tool_result.rs:124` | N = total lines − 1 |
| 4 | truncation footer `"[output truncated, {N} more lines]"` | `user_tool_result.rs:147` | expanded form, N = dropped lines |

## §6 System Messages

LingXi site: `lingxi-code/crates/tui/src/app.rs`.

| # | Literal | LingXi site | Notes |
|---|---|---|---|
| 1 | `"^C interrupted by user"` | `app.rs:133` | Ctrl-C while a turn is in flight |
| 2 | `"^C (press Ctrl-C again or type /exit to quit)"` | `app.rs:148` | idle SIGINT re-arm window |

> Note: the `Compacted N → M messages` boundary marker is rendered by the
> `/compact` command path (M6-08) via the slash dispatcher, not by `app.rs`
> directly; see `lingxi-commands` for its exact literal.

## §7 Documented divergences

| # | Literal (LingXi) | Literal (claude-code) | Rationale |
|---|---|---|---|
| 1 | `[1] Allow Once` / `[2] Allow Always` / `[N] Deny` | `Yes` / `Yes, and don't ask again for X commands in Y` / `No` (React Select) | LingXi surfaces the numeric-key affordance directly in the label, since the TUI is keyboard-driven and there is no mouse-clickable Select widget. Locked in `tui_permission_dialogs.json`. |
| 2 | spinner frames `· ✢ ✳ ✶ ✻ ✽` (×2 mirrored) | same (claude-code `Spinner.tsx:41`) | No divergence — LingXi copies claude-code's frame set byte-for-byte. |

## §8 Audit checklist (re-run on each upgrade)

- [ ] Check out / bump the `claude-code/` submodule; back-fill live `.tsx`
      line numbers for §1–§6.
- [ ] Re-extract literals from `src/components/StatusLine.tsx`,
      `src/components/Spinner.tsx`, `src/components/PromptInput/`,
      `src/components/permissions/PermissionRequest.tsx`,
      `src/components/messages/*.tsx`, `src/screens/REPL.tsx`.
- [ ] Diff against this catalog; update LingXi sites for any new/changed literal.
- [ ] Bump `_claude_code_version` in `tui_renderers.json` + `tui_repl_loop.json`
      + `tui_permission_dialogs.json` + `tui_listings.json`.
- [ ] Re-run `cargo test -p lingxi-test-harness --test parity_tui_renderers`
      + `parity_tui_repl_loop` + `parity_tui_permission_dialogs`
      + `parity_tui_listings`.
