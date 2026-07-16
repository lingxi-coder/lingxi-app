# Worktree `--tmux` — Manual QA Checklist

The `--worktree`/`--tmux` launch feature and `ExitWorktree`'s tmux handling are
fully implemented, byte-verified against the 2.1.206 binary, and unit-tested with
mocked process runners. What automated tests **cannot** exercise is the real
terminal round-trip: a live `tmux` binary creating a session, you detaching, and
reattaching. This is that ~15-minute human pass (follow-up residual #3).

## Prerequisites

- macOS or Linux (not Windows — bare `--tmux` hard-errors there by design).
- `tmux` installed and on `PATH` (`tmux -V` succeeds).
- A build of the CLI (`cargo build -p cli`, or your usual run wrapper).
- Run from **inside a git repository** (worktree creation requires one).

Throughout, `SESSION` is the derived tmux session name, printed by the launch and
of the form `<repo-basename>_worktree-<flatten(name)>` (e.g. repo `myrepo`,
`--worktree feat` → `myrepo_worktree-feat`).

---

## A. Native mode (bare `--tmux`) — happy path

1. Launch: `lingxi-cli --worktree qa-native --tmux` from a git repo.
2. ☐ The session boots **inside the new worktree** (`.lingxi/worktrees/qa-native`);
   the working directory line / prompt reflects the worktree path.
3. ☐ At launch, **stderr** prints:
   `Created tmux session: <repo>_worktree-qa-native` then
   `To attach: tmux attach -t <repo>_worktree-qa-native`.
4. ☐ In a **separate** terminal: `tmux ls` lists a session named
   `<repo>_worktree-qa-native`.
5. ☐ `tmux attach -t <repo>_worktree-qa-native` attaches to a shell whose cwd is
   the worktree. Detach again (`Ctrl-b d`).

## B. `keep` leaves the session running + surfaces the name

5. In the running session, exit the worktree with **keep** (invoke `ExitWorktree`
   with `action: "keep"`, however your flow triggers it).
6. ☐ The tool's result message ends with:
   `Tmux session <name> is still running; reattach with: tmux attach -t <name>`
   (a single space before `Tmux`, inline — not on its own line).
7. ☐ `tmux ls` (separate terminal) STILL lists the session.
8. ☐ `tmux attach -t <name>` reattaches successfully.
9. ☐ The session is back in the **original** launch directory (not the worktree).

## C. `remove` kills the session

10. Launch a fresh one: `lingxi-cli --worktree qa-remove --tmux`.
11. Confirm `tmux ls` shows `<repo>_worktree-qa-remove`.
12. Exit with **remove** (`ExitWorktree` `action: "remove"`, add
    `discard_changes: true` if it reports uncommitted changes).
13. ☐ `tmux ls` (separate terminal) NO LONGER lists the session (it was killed).
14. ☐ The result message does NOT contain a "still running / reattach" line.
15. ☐ The worktree directory is gone from `.lingxi/worktrees/`.

## D. Classic mode (`--tmux=classic`)

16. Launch: `lingxi-cli --worktree qa-classic --tmux=classic`.
17. ☐ Boots into the worktree with a tmux session created the same way
    (`tmux ls` shows `<repo>_worktree-qa-classic`); behavior matches native for
    session creation.

## E. Pre-flight validation (native only)

18. Temporarily make `tmux` unavailable (e.g. `PATH= lingxi-cli ...` or rename the
    binary), then run `lingxi-cli --worktree qa-x --tmux`.
19. ☐ Boot HARD-fails before creating a worktree, with a message containing
    `tmux is not installed.` followed by the platform install hint
    (macOS: `Install tmux with: brew install tmux`).
20. ☐ No worktree was created (`.lingxi/worktrees/` unchanged).
21. Now with tmux available, run `lingxi-cli --worktree=qa-y --tmux=classic` with
    tmux removed from PATH → ☐ classic mode does NOT hard-fail on the missing
    tmux (it skips the pre-flight); the worktree is created and the tmux-session
    creation just warns.
22. Run `lingxi-cli --tmux` **without** `--worktree` → ☐ hard error
    `--tmux requires --worktree`.

## F. Missing-original-cwd fallback (residual #2 — optional, destructive)

Only if you want to exercise the `xCd`/`kAs` fallback live:

23. Launch `lingxi-cli --worktree qa-fallback` (tmux optional) from a throwaway dir
    `/tmp/qa-origin`.
24. From another terminal, `rm -rf /tmp/qa-origin` (delete the original cwd).
25. Exit with **keep** → ☐ message: `The original directory /tmp/qa-origin no
    longer exists, so the session is now in <worktree path>.` (fell back to the
    worktree; NO "Consider restarting" advice).
26. Repeat, but exit with **remove** → ☐ message ends with `Consider restarting
    LingXi from an existing directory.` (fell back past the deleted worktree to
    `$HOME`/tmp).

---

## Cleanup

- `tmux kill-server` (or kill individual `<repo>_worktree-*` sessions) to clear
  any leftover sessions.
- `git worktree list` / `git worktree remove` for any kept worktrees.

## If something fails

Capture: the exact launch command, the printed session name, `tmux ls` output,
and the `ExitWorktree` result message. The implementation lives in
`apps/engine-desktop/src/lib.rs` (`apply_worktree_launch`),
`platforms/posix/src/worktree_tmux.rs` (argv builders), and
`tools/worktree/src/worktree.rs` (`ExitWorktreeTool`). See
`docs/worktree-206-tmux-followups-2026-07-14.md` for the shipped-commit map.
