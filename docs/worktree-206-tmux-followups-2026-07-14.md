# Worktree 206 + tmux-launch — Follow-up residuals

Status of the worktree/tmux parity work as of 2026-07-14. The two features
(worktree 206 core, `--worktree`/`--tmux` launch) and all requested residual
fixes are **DONE and on `main`** (see the commit table at the bottom). This doc
records only the **accepted residuals** — deliberate, documented gaps that were
scoped out, each with what building it would take. None block the shipped work.

---

## 1. `--tmux=<mode>` inner string not interpreted (presence-only gating)

**What ships:** `--tmux` creates a detached tmux session iff the flag is present.
The flag's *value* (`--tmux` native vs `--tmux=classic`, 206's mode string) is
**threaded through config but not acted on** — any present value gates creation
identically.

**Why deferred:** 206's classic-vs-native distinction changes terminal
*attach/inline* behavior, which is a live-terminal UX concern with no unit-test
surface. The session-creation half (the parity-critical part) is byte-faithful.

**To build:** interpret `tmux_launch: Option<String>`'s inner value in
`engine-desktop`'s `apply_worktree_launch` — extract 206's mode branch (search
the binary near the `--tmux` option handler + `createTmuxSessionForWorktree`
launch path @216368487/@216369152, which does `tmux new-session … -- <execPath>
<args>` for the inline/attach variant vs the plain detached `-d` session). Wire
the two argv shapes. **Needs live-tmux manual QA** — not unit-testable.
**Est: small-medium**, mostly QA.

## 2. `kAs` missing-original-cwd fallback (ExitWorktree message)

**What ships:** ExitWorktree's model message uses 206's `kAs` **normal branch**
byte-exact: `Session is now back in {original_cwd}.`

**Why deferred:** 206's `xCd` restore returns `{restoredCwd, originalCwdMissing,
fellBackToWorktree}` and `kAs` has a fallback string for when the original cwd
was deleted while inside the worktree (`The original directory {cwd} no longer
exists, so the session is now in {restoredCwd}.` + optional `Consider restarting
LingXi from an existing directory.`). The port's `session_cwd.swap` has **no
`restoredCwd`/`fellBackToWorktree` substrate** — it assumes the original cwd
still exists. Rather than invent a fake `restoredCwd`, only the reachable normal
branch was ported.

**To build:** port 206's `xCd` restore logic — after the swap-back, stat
`original_cwd`; if gone, fall back to home/tmp and set the missing-cwd flags,
then emit the fallback `kAs` string. Touches ExitWorktree's restore path +
`SessionCwd`. **Est: medium.** Low priority (rare edge — user deletes the repo
dir mid-worktree-session).

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
