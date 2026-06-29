# T3 — Inline Render Mode: Findings & Why It's Not a Quick Fix

- **Date:** 2026-06-29
- **Status:** Investigation finding (no code change recommended yet — see Conclusion)
- **Context:** The concurrent session shipped an opt-in inline render mode (`LINGXI_TUI_INLINE`, commit `75f14e891`) to sidestep Warp's alt-screen ghosting. An adversarial verification (this finding) confirms it is **safe as an opt-in** but **does not yet achieve its goal**, and a real fix is architectural.

## What works (verified)

- **Truly opt-in / zero regression.** Default stays `.fullscreen()` (alt-screen). Both production mounts (`run_tui_session` session.rs:575, `run_resume_picker`) branch on `inline_render_mode()`. The default path is byte-unchanged.
- **Warp detection** (`8eda2929f`) is exact: `TERM_PROGRAM == "WarpTerminal"`, pure + unit-tested, no misfire. The "warn on Warp" notice only warns.
- Inline mode genuinely avoids alt-screen (`render_loop()` never emits `EnterAlternateScreen`), so it sidesteps the ghosting *cause*.

## Why it does NOT yet flow into scrollback (the real issue)

Two compounding problems:

1. **App-level viewport windowing is still active in inline mode.** `tui/src/root.rs:3810` computes `viewport = viewport_height(rows, prompt_rows)` (= `rows − (FIXED_CHROME_ROWS=5 + prompt_rows)`) and `:3814` calls `render_screen(&st, viewport, vp_width)` — which **windows the scrollback to ~one screen** — *before* the inline branch at `:3845` drops only the OUTER full-height pin. So the conversation is still clamped to one screen; it never grows past it to flow into native scrollback.

2. **iocraft 0.8.3 inline rendering can't do Ink-style history.** Even if (1) were removed, iocraft's `StdTerminal::clear_canvas` (`~/.cargo/.../iocraft-0.8.3/src/terminal.rs:198-223`) issues `Clear::All` + `Clear::Purge` (CSI 2J + **CSI 3J = purge native scrollback**) whenever a changed row is above the visible window once canvas height ≥ terminal rows (its issue-#118 workaround). iocraft re-renders the **whole canvas** each frame; it has no `Static`/insert-before mechanism to print history to scrollback once and re-render only the live region (the Ink / claude-code model). So once the conversation exceeds one screen, inline mode **purges scrollback and full-clears**, defeating the goal and causing flicker.

   (Correction to an earlier informal claim: the purge is NOT literally "every frame" — `write_canvas` does a relative-cursor row-diff and only falls back to the purge when a changed row is above `visible_start = prev_height − term_h`. Pure bottom-append streaming wouldn't purge. But because of (1) the canvas is pinned to ~one screen, so the purge condition stays armed.)

## Conclusion / recommendation

- Inline mode behaves as advertised **only for conversations shorter than one screen.** For real (multi-screen) conversations it cannot flow into scrollback with iocraft 0.8.3 as-is.
- A real fix requires **Ink-style Static / insert-before rendering** — printing finalized history lines to the terminal's scrollback once (never re-rendered) and keeping only the live region (current prompt + spinner) as the small dynamic inline canvas. iocraft 0.8.3 does not expose this; it would be an **iocraft-level change** (custom render path or an upstream contribution), not a small edit in this repo.
- **Recommended:** keep inline mode flagged EXPERIMENTAL (it is), document this limitation, and treat the Static-rendering work as a deliberate, separately-scoped effort with **live terminal verification** (Warp + iTerm2) — not a speculative blind edit (TUI rendering can't be verified without a real terminal; pyte/unit tests don't catch scrollback/ghosting behavior).
- Removing the viewport windowing in inline mode (problem 1 alone) is a small change that would help SHORT content but make LONG content worse (more purges), so it should NOT be done in isolation.
