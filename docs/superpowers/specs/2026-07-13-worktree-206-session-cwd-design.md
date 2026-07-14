# Worktree 206 Parity + Session-CWD Plumbing — Design Spec

**Goal:** Bring LingXi's `EnterWorktree`/`ExitWorktree` tools to byte-parity with
Claude Code 2.1.206, including the **session-cwd switch** so that after
`EnterWorktree` every subsequent filesystem tool (Read/Write/Edit/Glob/Grep/Bash)
operates inside the worktree, and `ExitWorktree` restores the original directory.

**Oracle:** `/Users/luolingfeng/.local/share/claude/versions/2.1.206`.

**Architecture:** Introduce a shared, switchable **`SessionCwd`** cell in
`BuiltinToolContext` (replacing the frozen `workspace`/`trusted_dirs`); all FS
tools read the current cwd through it; the worktree tools swap it. Inert when no
worktree is active (cell = boot cwd), so a session that never calls
`EnterWorktree` is behaviorally byte-identical to today.

**Tech stack:** Rust, `arc-swap` (lock-free reads), existing `WorktreeManager`
trait, existing `ToolCallResult.context_modifier` hook.

---

## Global Constraints

- **Never touch the 4 dirty files:** `llm-client/data/models-dev/openrouter.json`,
  `llm-client/src/catalog/presets.rs`, `tools/agent/src/agent.rs`,
  `tools/agent/src/agent_test.rs`.
- **Never `cargo fmt`.**
- **Commit to `main`** (no remote); commit trailer ends exactly
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **Inert-when-no-worktree:** every task must keep a no-`EnterWorktree` session
  byte-identical to today (the cell defaults to boot cwd; `swap` is only ever
  called by the worktree tools).
- **LingXi carve-outs stay:** `.lingxi/worktrees` path namespace, LingXi rebrand.

---

## Verified 2.1.206 Contract (the byte oracle)

### EnterWorktree
- **searchHint** (already matches port): `create an isolated git worktree and switch into it` (@198130343).
- **Create branch** (`e.path` absent): swap cwd into new worktree, then message:
  > `Created worktree at ${worktreePath}${branch}. The session is now working in the worktree. Use ExitWorktree to leave mid-session, or exit the session to be prompted.`
  where `${branch}` = ` on branch ${worktreeBranch}` when a branch exists, else `""`.
- **Enter existing** (`e.path` present): `${o}` becomes `Entered` (same template as create) OR the mid-session existing-entry variant:
  > `Entered worktree at ${worktreePath}${branch}. This agent's working directory and write access now point at the worktree; the previous directory was left untouched.`
- **Side effects (206 `call`):** `process.chdir(worktreePath)`, `IS(worktreePath)` (set cwd state), agent-metadata cwd update (`Q8i`/`CJt`), cache invalidation `gre(r), yet(), H1(), rT.cache.clear?.(), XO(), Pde(), hv()?.refreshGitBranch?.()`, returns `contextLayers:[{kind:"working_directory", directory: worktreePath}]`.
- **Guards:** `if (ky() && !e.path) throw Error("Already in a worktree session")`.
- **Telemetry:** `tengu_worktree_created` / `tengu_worktree_entered_existing`.

### ExitWorktree
- **searchHint** (already matches port): `exit a worktree session and return to the original directory`.
- **Prompt (`vCd`)** — byte-exact, extract verbatim during implementation from @222208720:
  > `Exit a worktree session created by EnterWorktree and return the session to the original working directory.` + `## Scope` (ONLY session-created worktrees; NOT manual/previous-session; no-op outside a session, "Filesystem state is unchanged.") + `## When to Use` (only when user asks; do NOT call proactively) + `## Parameters` (`action` required keep|remove; `discard_changes` optional default false) + `## Behavior` (restores CWD to pre-Enter; clears CWD caches sysprompt/memory/plans; tmux killed on remove / left running on keep with reattach name; "Once exited, EnterWorktree can be called again").
- **Schema:** `{ action: "keep"|"remove" (required), discard_changes?: boolean (default false) }`.
- **`discard_changes` guard:** with `action:"remove"`, if the worktree has uncommitted files OR commits not on the original branch, REFUSE unless `discard_changes:true`. Refusal (`errorCode:1`, @222213206):
  > `…operates on worktrees created by EnterWorktree in the current session — it will not touch worktrees created manually or in a previous session. No filesystem changes were made.`
  (extract the exact refusal head verbatim during implementation).
- **Result mapper (`wCd`):** line 1 = `Kept worktree` (action keep) or `Removed worktree` (action remove), followed by ` (branch ${worktreeBranch})` when a branch exists; line 2 = `Returned to ${originalCwd}`.
- **Change summary (`RCd`):** `git -C <path> status --porcelain` → count non-empty lines = `changedFiles`; `git -C <path> rev-list --count <base>..HEAD` = `commits`. Returns null when git unavailable.
- **Telemetry:** `tengu_worktree_exited` (keep) / `tengu_worktree_removed` (remove) — verify names during implementation.

---

## Port Substrate Reality (why the plumbing is needed)

- Tools read cwd from `BuiltinToolContext.workspace` / `trusted_dirs` (e.g.
  `grep.rs:573`), and `PosixFileSystem` is rooted at `cwd` — all **frozen at boot**
  (`engine-desktop/src/lib.rs:4886`) and **cloned by value into each tool**.
- The registry + `BuiltinToolContext` are built **once per session**; nothing
  switches cwd mid-session today.
- `ContextModifier` (`Box<dyn FnOnce(ToolUseContext)->ToolUseContext>`) mutates
  `ToolUseContext`, which tools do NOT read cwd from — so it alone can't switch cwd.
- Bash keeps its own persistent `STATE.cwd` (`bash.rs:902`).
- `tmux` code exists but is swarm-only (`platforms/posix/src/swarm/tmux.rs`), not
  worktree-attached.

⇒ A shared, switchable session-cwd is the only faithful path.

---

## Design: components (each = one SDD task)

### Task 1 — `SessionCwd` cell (`tool-api`)
New `SessionCwd { cwd: ArcSwap<PathBuf>, trusted_dirs: ArcSwap<Vec<PathBuf>> }`
with `current() -> Arc<PathBuf>`, `trusted() -> Arc<Vec<PathBuf>>`,
`swap(cwd, trusted_dirs)`, `new(boot_cwd)`. Lock-free reads. Unit-tested in
isolation. **Interface produced:** `Arc<SessionCwd>`.

### Task 2 — `BuiltinToolContext` migration (broad, highest-risk)
Replace `workspace: PathBuf` / `trusted_dirs: Vec<PathBuf>` with
`session_cwd: Arc<SessionCwd>`; add `cwd(&self) -> PathBuf` and
`trusted_dirs(&self) -> Vec<PathBuf>` accessors returning the current values.
Update every read site (`self.ctx.workspace`, `self.ctx.trusted_dirs`) across
Read/Write/Edit/MultiEdit/Glob/Grep/Bash/dir_validate + all construction sites
(engine-desktop, engine-mobile, test_support). **Inert:** accessors return the
boot cwd until a swap happens. Tests assert byte-identical behavior with no swap.

### Task 3 — FS re-rooting
`PosixFileSystem` consults `SessionCwd` for relative-path resolution (or the tools
always pass cwd()-resolved absolute paths). Verify Read/Write/Edit land in the
worktree after a swap.

### Task 4 — Bash `STATE.cwd` re-point
On swap, re-point the persistent shell cwd; on exit, restore. Test a Bash `pwd`
before/after a simulated swap.

### Task 5 — CWD-cache invalidation hook
A `SessionCwd::on_swap` callback (or an orchestrator hook) that clears the
CWD-dependent caches (system-prompt sections, memory files, plans dir), mirroring
206 `gre/yet/H1/rT.cache.clear/XO/Pde`. Wire the orchestrator to re-derive on swap.

### Task 6 — `WorktreeManager::enter_existing(path)`
New trait method for 206's `e.path` branch (enter an existing worktree, resolve
its branch). Default impl returns `Unsupported`. Mock + posix impls.

### Task 7 — `EnterWorktree` rewrite
Schema `{ name?, path? }`; on success call `SessionCwd.swap(...)` via the
`context_modifier` (or directly on the shared cell) + cache-invalidate; byte-exact
`Created/Entered worktree…` messages; `Already in a worktree session` guard;
telemetry. Keep slug validation for the create path.

### Task 8 — `ExitWorktree` rewrite + tmux
Schema `{ action: keep|remove, discard_changes? }`; byte-exact `vCd` prompt; the
`discard_changes` refusal guard (errorCode:1 message); capture change-summary
(`RCd`) before removal; restore cwd via the cell; `wCd` result
(`Kept worktree`/`Removed worktree` + ` (branch X)` + `Returned to {originalCwd}`);
tmux kill-on-remove / keep-on-keep + reattach name where the swarm substrate
allows (else documented residual with a `verify-before-change` note).

---

## Error handling
- No active worktree session → `ExitWorktree` is a no-op (reports session inactive,
  "Filesystem state is unchanged."), matching 206.
- `remove` with uncommitted work and `discard_changes:false` → refuse, errorCode:1,
  "No filesystem changes were made." — no cwd swap, no removal.
- Manager errors (git/io/unsupported) → surface faithfully; no cwd swap on failure.
- Swap failure (target unreadable) → restore prior cwd, surface error (mirror 206
  `xCd` realpath fallback).

## Testing
- Task 1: `SessionCwd` swap/current unit tests.
- Task 2: every migrated tool has a "no-swap = byte-identical" test.
- Tasks 3–4: post-swap Read/Bash land in the worktree; post-exit restore.
- Task 7–8: byte-exact message golden tests (Created/Entered/Kept/Removed/Returned),
  discard_changes guard (refuse vs force), change-summary, no-op-outside-session.
- Full inert check: a build with no `EnterWorktree` call passes the existing tool
  suites unchanged.

## Out of scope / documented residuals
- Agent-metadata cwd update (`CJt`) if no reachable substrate — document.
- iTerm-specific tmux niceties beyond kill/keep/reattach-name.
