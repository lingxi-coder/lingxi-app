# Worktree 206 + tmux-launch — Follow-up residuals

Status of the worktree/tmux parity work as of 2026-07-14. The two features
(worktree 206 core, `--worktree`/`--tmux` launch) and all requested residual
fixes are **DONE and on `main`** (see the commit table at the bottom). This doc
records only the **accepted residuals** — deliberate, documented gaps that were
scoped out, each with what building it would take. None block the shipped work.

---

## 1. `--tmux=<mode>` inner string — ✅ RESOLVED (`a69b9fb44`, 2026-07-16)

**Correction to the original scope below:** binary analysis found 206's "iTerm2
native panes" (the thing the `--tmux` help promises) is **vaporware** — `Dor()`
(`isWorktreeModeEnabled`) is a stubbed `return !0`, and the pane/split-window
code is all inside a dead `if(!1)` demo block. **Both** `--tmux` and
`--tmux=classic` do the identical detached `a4i` create we already shipped.

The *real* native-vs-classic difference (binary @230041975,
`re = Dor() && a.tmux===!0`) is a **pre-flight validation asymmetry**, now
implemented:
- **Bare `--tmux`** (native, `tmux_launch == Some("")`) hard-checks — before
  creating the worktree — not-Windows (`--tmux is not supported on Windows`) and
  tmux-installed via `tmux -V` (`i4i()`), erroring with the platform install
  hint (`s4i()`: brew / apt|dnf / WSL|Cygwin / generic).
- **`--tmux=classic`** skips the native pre-flight; a missing tmux degrades to
  the existing non-fatal create-session warning.

New `platform_posix::worktree_tmux::{tmux_is_installed, tmux_install_hint}`
(byte-faithful to `i4i`/`s4i`), `BuildError::{TmuxNotSupportedOnWindows,
TmuxNotInstalled}`. Review APPROVE (opus, byte-exact). Windows-reject branch
(`cfg!(windows)`) untestable on a non-Windows host.

<details><summary>Original (mis-scoped) note — kept for the record</summary>

Originally documented as "interpret the mode string → two argv shapes / attach
behavior, needs live-tmux QA." That was wrong: there are no two argv shapes (the
inline/attach `if(!1)` code is dead), and the mode difference is validation, not
session-creation. Resolved above.
</details>

## 2. `kAs` missing-original-cwd fallback — ✅ RESOLVED (`fcf56f46e`, 2026-07-16)

Ported 206's `xCd` restore (@222210592) + `kAs` message (@222211173). When
ExitWorktree restores the session cwd and `original_cwd` no longer exists on
disk, it falls back through `[worktree_path, $HOME, temp_dir]` (206's
`[n=worktreePath, homedir(), sG()]`), tracking `fell_back_to_worktree`. The
restore was moved to AFTER the worktree removal (206 order `het()`→`xCd`), so a
successful `remove` skips the deleted worktree (→ `$HOME`) while `keep` falls
back to the still-present worktree.

Message (`kAs`), byte-exact: missing + fell back to worktree → `The original
directory {cwd} no longer exists, so the session is now in {restored}.`; missing
+ fell back to `$HOME`/tmp → same + ` Consider restarting LingXi from an existing
directory.` Directory existence is probed via an injected `fn(&Path)->bool`
(`new()` = real `std::fs`; tests inject stubs). Review APPROVE (opus, byte-exact,
no bugs). Documented low-risk deviations: non-ENOENT stat errors fold into the
fallback (206 rethrows); temp dir is `std::env::temp_dir()` vs `CLAUDE_CODE_TMPDIR`.

## 3. Real tmux reattach UX — manual QA only

**What ships:** all tmux argv (`new-session -d -s <name> -c <path>`,
`kill-session -t <name>`), the session name
(`<basename(repo)>_worktree-<flatten(name)>` = 206 `bWn(repo, Xvt(name))`), and
the keep-side reattach line are **byte-verified against the 2.1.206 binary**, and
the wiring (boot populate → ExitWorktree kill/keep) is unit-tested with mocked
runners.

**Why a residual:** the *actual* terminal behavior — a real `tmux` binary
creating a session, the user detaching, `ExitWorktree` keep leaving it alive,
`tmux attach -t <name>` reattaching — cannot be exercised without a live tmux +
terminal. Coverage is argv/wiring tests, as flagged when the feature was
green-lit.

**To do:** a human runs `--worktree foo --tmux`, confirms the session exists
(`tmux ls`), detaches, exits with `keep`, and reattaches; then repeats with
`remove` and confirms the session is gone. **Est: ~15 min manual**, no code.

---

## Shipped (all on `main`, byte-verified vs 2.1.206)

| Item | Commit(s) |
|---|---|
| Worktree 206 core (EnterWorktree/ExitWorktree + session-cwd plumbing, 8-task SDD) | `a862844bd`→`e35752d81` |
| tmux-launch (`--worktree`/`--tmux` boot → create/tmux → exit kill/keep, 5-task SDD) | `508e55fdf`→`e8fd298d3` |
| Telemetry payload `{mid_session}` / `{mid_session,cwd_override}` | `d3513a688` |
| Subagent-cwd-override guards (both tools) | `d3513a688` |
| errorCode:4 "not the owner" remove guard | `4fe3f0ac5` |
| `ky()` refinement | `2bff1578d` |
| WebFetch UTF-16 fast-path | `7a8fce999` |
| **ExitWorktree model message = 206 `data.message`** (fixed mis-ported TUI-render surface) | `ae1c7ebde` |
| **Residual #1: `--tmux` native-mode pre-flight** (not-Windows + tmux-installed for bare `--tmux`; classic skips) | `a69b9fb44` |
| **Residual #2: ExitWorktree missing-original-cwd fallback** (206 `xCd`/`kAs`; fallback `[worktree, $HOME, tmp]`) | `fcf56f46e` |
