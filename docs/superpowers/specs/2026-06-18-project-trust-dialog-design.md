# Project trust dialog + `hasTrustDialogAccepted` store (design)

**Date:** 2026-06-18
**Status:** approved for implementation
**Origin:** the deferred "next, separate cycle" from `2026-06-18-repl-permission-prompt-design.md`. The interactive permission prompt (y/N per mutating tool) is wired (main `428ddc7e`); this adds the **startup trust gate** that claude-code shows before any tool/hook/plugin runs.

## Goal

Port claude-code's project-trust model: on entering a directory that hasn't been trusted, show a one-time **trust dialog**; persist acceptance per-project; **decline = exit**. Replace LingXi's hardcoded `project_trust: ProjectTrustLevel::Trusted` (engine-desktop `lib.rs:2791`, engine-mobile `host.rs:583`) with a value gated by that store + dialog.

## claude-code reference (the behavior to match)

- `utils/config.ts`: per-project config carries `hasTrustDialogAccepted?: boolean` (default false), stored under `config.projects[projectKey]` in `~/.claude.json`.
- `checkHasTrustDialogAccepted(cwd)`: true if the cwd's project config has it, OR **any ancestor path** does (parent-walk — trusting a parent trusts children).
- `components/TrustDialog/TrustDialog.tsx`: if `checkHasTrustDialogAccepted()` → skip (proceed). Else show the dialog. **The exact copy is byte-locked to `TrustDialog.tsx` (NOT the placeholder strings below — those were design guesses; the real copy is** title `"Accessing workspace:"` + cwd + `"Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take a moment to review what's in this folder first."` + `"Claude Code'll be able to read, edit, and execute files here."` + a `Security guide` link to `https://code.claude.com/docs/en/security` + options `"Yes, I trust this folder"` / `"No, exit"`**).
  - **Accept** ("Yes, I trust this folder") → set `hasTrustDialogAccepted: true` on the cwd project config (EXCEPT cwd==$HOME → in-memory `setSessionTrustAccepted`, not persisted), proceed.
  - **Decline** ("No, exit") → `gracefulShutdownSync(1)` (exit code **1**); the confirm:no keybinding path → `gracefulShutdownSync(0)`.
- `screens/REPL.tsx` + `utils/plugins/performStartupChecks.tsx`: tools/hooks/plugins run ONLY after the dialog clears. Because decline exits, a *running* session is always trusted — so `project_trust` stays `Trusted` for any session that proceeds; the dialog is purely a startup gate + persistence.

## LingXi seams (what exists)

- Store: `migrations::global_config` — `global_config_path()` (`~/.claude.json`), `get_project_config(path, key)` / `save_project_config(...)` (untyped `JsonMap`), `project_path_for_config(dir)` (the canonical project key). So `hasTrustDialogAccepted` is a key in the project's JsonMap.
- Trust type: `sandbox::decision::ProjectTrustLevel { Trusted, Untrusted }`; consumed by `sandbox::decision` (`Trusted && classifier_safe → sandbox`), threaded via `tool-api::BuiltinToolContext.project_trust`. Hardcoded `Trusted` at the two composition roots. (NOTE: bash/powershell already override `ctx.project_trust = Untrusted` per-tool for sandbox decisions — a SEPARATE axis, unchanged here.)
- REPL: `apps/cli/src/repl.rs::run_repl` (has the shared-stdin `BufReader` + `should_prompt_interactively` from the permission-prompt work — reuse for the dialog).
- TUI: `tui/src/root.rs` startup; `tui/src/state.rs:1403` statusline already has a fail-closed `trusted` param "for when a trust store lands".

## Components

1. **`migrations/src/global_config.rs`** — the store API:
   - `check_has_trust_dialog_accepted(config_path, cwd) -> bool`: for `cwd` and each ancestor (`cwd`, `cwd.parent()`, … to root), read `get_project_config` and return true if any has `"hasTrustDialogAccepted": true`. Mirrors `checkHasTrustDialogAccepted`'s parent-walk.
   - `mark_trust_dialog_accepted(config_path, cwd) -> Result<()>`: `save_project_config` for `cwd`'s key with `hasTrustDialogAccepted = true` (preserving other keys).
   - Unit tests: cwd-accepted → true; ancestor-accepted → true; none → false; mark then check → true; mark preserves siblings.

2. **REPL startup gate** — `apps/cli/src/repl.rs::run_repl`:
   - After resolving cwd, before building the runtime/loop: if `should_prompt_interactively(is_tty, print)` AND `!check_has_trust_dialog_accepted(cfg_path, cwd)`, show the dialog over the shared stdin reader + stderr:
     - Prompt: the byte-exact `TrustDialog.tsx` copy (reference section above — "Accessing workspace:" … "Security guide: {url}" … "Yes, I trust this folder" / "No, exit") + a `[y/N]` reader. Read y/N (reuse the gate's y/yes/n/no/Empty parsing; Empty/n/EOF → decline).
     - Accept → `mark_trust_dialog_accepted(cfg_path, cwd)`, proceed.
     - Decline → exit code **1** (`gracefulShutdownSync(1)`) WITHOUT building the runtime.
   - Non-TTY / already-accepted → proceed with no prompt (today's behavior; `project_trust` Trusted).
   - The dialog reads the SAME shared `BufReader` BEFORE the loop starts, so no stdin contention (sequential).

3. **TUI startup gate** — `tui/src/root.rs` (or the startup screen):
   - Before the main loop, if interactive AND not accepted, render a trust modal/screen with the same copy + y/n (or a 2-option select). Accept → mark + continue; Decline → exit the TUI cleanly.
   - If the TUI's screen infra makes a blocking modal large, a minimal faithful version: a startup `TrustDialog` screen that owns the first frame and resolves to Continue/Exit. Reuse the existing screen/key-dispatch pattern.

4. **Composition wiring** — keep `project_trust: ProjectTrustLevel::Trusted` (a running session is trusted), but it is now REACHED only after the gate clears. No change to the value; the gate is the addition. (If a future "trusted=false but proceed" mode is wanted, that's out of scope — claude-code exits on decline.)

## Decisions (explicit)

- **Decline = exit** (faithful to claude-code "No, exit"). No "proceed untrusted" mode.
- **Non-interactive (piped / `-p` print / non-TTY) = proceed, no prompt** — matches today's hardcoded Trusted and claude-code (the dialog is interactive-only; headless can't prompt).
- **Parent-walk**: trusting a dir trusts its subdirs (claude-code semantics).
- **Persistence**: `hasTrustDialogAccepted` in the per-project `~/.claude.json` config (the existing store), byte-shape-compatible with claude-code's key.
- **Scope**: this is the trust DIALOG + store only. The per-tool `Untrusted` sandbox overrides (bash/powershell) and `--dangerously-skip-permissions` are separate and unchanged.

## Out of scope

- Migrating the statusline/hooks "fail-closed for-when-a-trust-store-lands" params to consult the store (they're moot once decline=exit means running⇒trusted; a refinement, not required).
- A "re-trust on directory change mid-session" flow.
- Enterprise/managed forced-trust policy tiers.

## Testing (TDD)

- Store: the unit tests in §1.
- REPL: `check_has_trust_dialog_accepted` false + scripted `"y\n"` on the shared reader → marks accepted + proceeds; `"n\n"`/EOF → returns the decline exit path WITHOUT building the runtime; already-accepted → no prompt (no byte consumed); non-TTY → no prompt. (Reuse the shared-`BufReader` test harness from the permission-prompt work.)
- TUI: a startup-gate test that an un-trusted dir shows the dialog screen and accept→continue / decline→exit; trusted dir → straight to the main screen.
- Regression: trusted/non-interactive paths build exactly as today.
