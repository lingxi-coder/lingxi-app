# Platform Runtime (posix) Fix Report

**STATUS: DONE — 2 of 7 gaps fixed; 5 deferred/documented**

## Build + Test

- `cargo test -p platform-posix -p tool-shell`: **exit 0** (all 136 + 12 tests pass, 4 new tests added)
- `cargo build --workspace`: **exit 0** (warnings pre-existing, no new errors)

## Kill-Sequence Finding (Gap 5)

Binary search confirmed: `treeKill(pid, 'SIGKILL')` is called **directly** in `ShellCommand.ts:337-343` — no SIGTERM grace period. The audit's description was accurate. LingXi's `kill()` was calling `kill_tree_unix` (SIGTERM + 5 s grace + SIGKILL), which caused a 5-second hang per kill. Fixed to call `kill_tree_force` (immediate SIGKILL).

## Per-Gap Status

| # | Severity | Item | Status |
|---|---|---|---|
| 1 | P0 | Shell snapshot mechanism | DEFERRED — multi-file addition, out of scope |
| 2 | P1 | `CLAUDE_CODE_SHELL` override | **FIXED** — `resolve_shell_path()` now reads env var first |
| 3 | P1 | `CLAUDE_CODE_DONT_INHERIT_ENV` | DEFERRED — only relevant once Gap 1 (snapshot) is fixed |
| 4 | P1 | `CLAUDE_ENV_FILE` not sourced | DEFERRED — no clean seam without snapshot mechanism |
| 5 | P1 | Kill sequence SIGTERM+5s vs SIGKILL | **FIXED** — `runner.rs::kill()` now uses `kill_tree_force` |
| 6 | P2 | Session env vars not injected | DEFERRED — coupled to Gap 1 (snapshot + session env) |
| 7 | P2 | Task output path/ID convention | DEFERRED — LOW impact, format-only difference |

## Files Changed

- `lingxi-code/tools/shell/src/bash.rs` — `resolve_shell_path()` now checks `CLAUDE_CODE_SHELL` env var; 4 new tests added
- `lingxi-code/platforms/posix/src/process/runner.rs` — `kill()` changed from `kill_tree_unix` (async, SIGTERM+grace) to `kill_tree_force` (sync, immediate SIGKILL)
- `lingxi-code/platforms/posix/src/process/kill_tree.rs` — module doc updated to describe both variants

## Concerns

None. The `Box::leak` in `resolve_shell_path()` allocates once per unique env-var value in tests (rare in production), which is acceptable since the function is called at session init only. The `kill_tree_unix` function is retained for any future callers that want graceful drain; only `ProcessRunner::kill()` now uses the force variant.
