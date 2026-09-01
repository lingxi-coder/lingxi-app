# Worktree 206 Parity + Session-CWD Plumbing — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Byte-parity `EnterWorktree`/`ExitWorktree` vs CC 2.1.206 including the session-cwd switch (subsequent FS tools operate in the worktree; ExitWorktree restores).

**Architecture:** A shared switchable `SessionCwd` cell (arc-swap) in `BuiltinToolContext` replacing frozen `workspace`/`trusted_dirs`; all FS tools read cwd through it; worktree tools swap it. Inert when no worktree active.

**Spec:** `docs/superpowers/specs/2026-07-13-worktree-206-session-cwd-design.md`. **Oracle:** `/Users/luolingfeng/.local/share/claude/versions/2.1.206`.

## Global Constraints

- Never touch: `llm-client/data/models-dev/openrouter.json`, `llm-client/src/catalog/presets.rs`, `tools/agent/src/agent.rs`, `tools/agent/src/agent_test.rs`.
- Never `cargo fmt`. Commit to `main`. Trailer ends `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **INERT INVARIANT:** with no `EnterWorktree` call, every FS tool behaves byte-identically to today. `SessionCwd` initializes to boot cwd; `swap` is only ever called by the worktree tools. Every task proves this with a no-swap test.
- Crate names use `tool-web`-style single-`tool` prefixes (`cargo build -p tool-api`, `-p tool-file`, `-p tool-shell`, `-p tool-worktree`).
- LingXi carve-outs stay (`.lingxi/worktrees`, rebrand). Telemetry names: `tengu_worktree_created`, `tengu_worktree_entered_existing`, `tengu_worktree_kept`, `tengu_worktree_removed`.

---

### Task 1: `SessionCwd` cell

**Files:**
- Create: `tool-api/src/session_cwd.rs`
- Modify: `tool-api/src/lib.rs` (add `mod session_cwd; pub use`)
- Test: inline `#[cfg(test)]` in `session_cwd.rs`

**Interfaces — Produces:**
```rust
pub struct SessionCwd { /* private: ArcSwap<PathBuf>, ArcSwap<Vec<PathBuf>> */ }
impl SessionCwd {
    pub fn new(boot_cwd: PathBuf, trusted: Vec<PathBuf>) -> Arc<Self>;
    pub fn cwd(&self) -> PathBuf;                 // current cwd (clone)
    pub fn trusted_dirs(&self) -> Vec<PathBuf>;   // current trusted (clone)
    pub fn swap(&self, cwd: PathBuf, trusted: Vec<PathBuf>);   // atomic swap
    pub fn set_on_swap(&self, cb: Box<dyn Fn(&Path) + Send + Sync>); // Task 5 hook (no-op until set)
}
```

- [ ] **Step 1:** Add `arc-swap` to `tool-api/Cargo.toml` (check workspace for an existing pin first; reuse if present).
- [ ] **Step 2:** Write failing tests: `new().cwd()==boot`, `swap` then `cwd()==new`, `trusted_dirs()` tracks swap, concurrent reads see last swap. Run — FAIL.
- [ ] **Step 3:** Implement with `arc_swap::ArcSwap<PathBuf>` + `ArcSwap<Vec<PathBuf>>`; `on_swap` stored in an `ArcSwapOption`/`Mutex<Option<..>>`, invoked inside `swap`.
- [ ] **Step 4:** `cargo test -p tool-api session_cwd` — PASS.
- [ ] **Step 5:** Commit `feat(tool-api): SessionCwd switchable cwd cell`.

---

### Task 2: `BuiltinToolContext` migration (broad, highest-risk)

**Files:**
- Modify: `tool-api/src/builtin_context.rs` (fields + accessors)
- Modify: all read sites — `tools/file/src/{read,write,edit,multi_edit,glob,grep,dir_validate}.rs`, `tools/shell/src/bash.rs` (cwd derivation), plus any other `self.ctx.workspace`/`self.ctx.trusted_dirs` reader (discover via grep).
- Modify: construction sites — `apps/engine-desktop/src/lib.rs`, `apps/engine-mobile/src/host.rs`, `tool-api/src/test_support.rs` (and any `BuiltinToolContext {` literal).
- Test: a `no_swap_is_identical` test per touched tool crate.

**Interfaces — Consumes:** `Arc<SessionCwd>` (Task 1). **Produces:** `ctx.cwd() -> PathBuf`, `ctx.trusted_dirs() -> Vec<PathBuf>`.

- [ ] **Step 1:** Replace `pub workspace: PathBuf` + `pub trusted_dirs: Vec<PathBuf>` with `pub session_cwd: Arc<SessionCwd>`. Add `pub fn cwd(&self) -> PathBuf { self.session_cwd.cwd() }` and `pub fn trusted_dirs(&self) -> Vec<PathBuf> { self.session_cwd.trusted_dirs() }`.
- [ ] **Step 2:** Grep every `self.ctx.workspace` / `self.ctx.trusted_dirs` / `ctx.workspace` read; replace with `self.ctx.cwd()` / `self.ctx.trusted_dirs()`. (Command: `grep -rn "\.workspace\b\|\.trusted_dirs\b" tools/ tool-api/`.)
- [ ] **Step 3:** Update every `BuiltinToolContext { ... }` literal + `test_support` helpers to build `session_cwd: SessionCwd::new(cwd, vec![cwd])`. Keep `PosixFileSystem::new(cwd)` for now (Task 3 re-roots).
- [ ] **Step 4:** Add `no_swap_is_identical` test in each touched crate: build ctx, assert `ctx.cwd()==boot_cwd`, run a representative tool (e.g. Glob), assert unchanged result.
- [ ] **Step 5:** `cargo build --workspace` + `cargo test -p tool-api -p tool-file -p tool-shell` — PASS.
- [ ] **Step 6:** Commit `refactor(tool-api): thread SessionCwd through BuiltinToolContext read sites`.

---

### Task 3: FS re-rooting

**Files:** Modify `platforms/posix/src/**` `PosixFileSystem` (relative-path resolution), or confirm tools already pass `ctx.cwd()`-resolved absolute paths. Test: `tools/file`.

- [ ] **Step 1:** Determine how `PosixFileSystem` resolves relative paths (constructor `cwd` vs per-call). If it holds a frozen `cwd`, give it an `Arc<SessionCwd>` and resolve relative paths through `session_cwd.cwd()`.
- [ ] **Step 2:** Failing test: after `session_cwd.swap(worktree)`, a `Read`/`Write` of a relative path resolves under the worktree. Run — FAIL.
- [ ] **Step 3:** Implement. Keep absolute-path behavior unchanged.
- [ ] **Step 4:** `cargo test -p tool-file -p platform-posix` — PASS.
- [ ] **Step 5:** Commit `feat(posix-fs): resolve relative paths through SessionCwd`.

---

### Task 4: Bash `STATE.cwd` re-point

**Files:** Modify `tools/shell/src/bash.rs` (persistent `STATE.cwd`). Test: `tools/shell`.

- [ ] **Step 1:** Locate the persistent shell cwd (`STATE.cwd`, bash.rs:902) and its initialization from workspace.
- [ ] **Step 2:** Failing test: `pwd` via Bash after a simulated `session_cwd.swap(worktree)` returns the worktree; after restore, the origin. Run — FAIL.
- [ ] **Step 3:** Init `STATE.cwd` from `ctx.cwd()`; on each Bash call, if `STATE.cwd` still equals the pre-swap origin AND a swap happened, re-point to `ctx.cwd()` (mirror 206: chdir moves the shell). Simplest faithful rule: derive the shell's base cwd from `ctx.cwd()` when the shell has no explicit user `cd` yet.
- [ ] **Step 4:** `cargo test -p tool-shell` — PASS.
- [ ] **Step 5:** Commit `feat(bash): re-point persistent shell cwd on session-cwd swap`.

---

### Task 5: CWD-cache invalidation hook

**Files:** Modify `orchestrator/src/**` (register a `SessionCwd::set_on_swap` callback that clears CWD-dependent caches: system-prompt sections, memory files, plans dir). Discover the actual caches first. Test: `orchestrator`.

- [ ] **Step 1:** Identify CWD-dependent caches in the port (system-prompt env/memory sections, plans dir). Grep `orchestrator/src/prompt/` + memory + plans for cached-by-cwd state. If the port re-derives per-turn (no cache), this task is a documented no-op with a test asserting re-derivation already reflects the swapped cwd.
- [ ] **Step 2:** Failing test: after a swap, the next system-prompt render reflects the new cwd (env `Primary working directory:` line). Run — FAIL (or confirm already-passing → no-op).
- [ ] **Step 3:** Register `session_cwd.set_on_swap(|new_cwd| { /* invalidate caches */ })` at composition root.
- [ ] **Step 4:** `cargo test -p orchestrator` — PASS.
- [ ] **Step 5:** Commit `feat(orchestrator): invalidate CWD-dependent caches on session-cwd swap`.

---

### Task 6: `WorktreeManager::enter_existing(path)`

**Files:** Modify `platform-api/src/worktree.rs` (+ trait), `platforms/posix/src/**` posix impl, `tool-api/src/test_support.rs` `MockWorktreeManager`. Test: mock + posix.

**Interfaces — Produces:**
```rust
async fn enter_existing(&self, path: &Path) -> Result<WorktreeHandle, WorktreeError>;
```
Default impl: `Err(WorktreeError::Unsupported)`.

- [ ] **Step 1:** Add the trait method with a default `Unsupported`. Implement in the posix manager (resolve branch via `git -C <path> rev-parse --abbrev-ref HEAD`, verify it's a git worktree).
- [ ] **Step 2:** Mock: record `entered_existing` calls; scriptable error.
- [ ] **Step 3:** Tests: posix enters an existing worktree; mock records; default returns Unsupported.
- [ ] **Step 4:** `cargo test -p tool-api -p platform-posix` — PASS.
- [ ] **Step 5:** Commit `feat(worktree): WorktreeManager::enter_existing`.

---

### Task 7: `EnterWorktree` rewrite

**Files:** Modify `tools/worktree/src/worktree.rs` (`EnterWorktreeTool`). Test: same file.

**Schema:** `{ "name": {type:string} (optional), "path": {type:string} (optional) }` — no required. (Keep slug validation for the `name`/create path; extract exact 206 param descriptions during this task via `grep -abo` on the binary.)

**Byte-exact messages (206):**
- Create: `Created worktree at {path}{branch}. The session is now working in the worktree. Use ExitWorktree to leave mid-session, or exit the session to be prompted.` where `{branch}` = ` on branch {worktreeBranch}` or ``.
- Enter existing (mid-session): `Entered worktree at {path}{branch}. This agent's working directory and write access now point at the worktree; the previous directory was left untouched.`
- Guard: reject `Already in a worktree session` when already in a worktree and no `path`.

- [ ] **Step 1:** Failing golden tests for both messages (create + enter-existing) + the guard + that `context_modifier`/shared cell swap sets `ctx.cwd()` to the worktree.
- [ ] **Step 2:** Rewrite `call`: parse `{name?, path?}`; `path` present → `manager.enter_existing(path)`; else `manager.create_worktree(name.unwrap_or_else(gen_slug), ...)`; on success `ctx.session_cwd.swap(handle.path, [handle.path])`; emit `tengu_worktree_created`/`tengu_worktree_entered_existing`; return the byte-exact message as `model_content`.
- [ ] **Step 3:** Update `description()`/`prompt()` to the 206 text (extract verbatim during this task).
- [ ] **Step 4:** `cargo test -p tool-worktree` — PASS.
- [ ] **Step 5:** Commit `feat(worktree): EnterWorktree 206 parity + session-cwd swap`.

---

### Task 8: `ExitWorktree` rewrite + tmux

**Files:** Modify `tools/worktree/src/worktree.rs` (`ExitWorktreeTool`). Test: same file.

**Schema:** `{ "action": {enum:["keep","remove"]} (required), "discard_changes": {type:boolean, default false} }`.

**Prompt (`vCd`) — byte-exact** (full text captured in the spec; re-extract from the binary `function vCd(){return` during this task to guarantee bytes).

**Refusal (remove + dirty + !discard_changes):** errorCode:1, ends `No filesystem changes were made.` (extract the exact full refusal head verbatim during this task).

**Result (`wCd`):** line 1 `Kept worktree`/`Removed worktree` + ` (branch {worktreeBranch})` when branch present; line 2 `Returned to {originalCwd}`.

**Change summary (`RCd`):** `git -C <path> status --porcelain` non-empty line count = changedFiles; `git -C <path> rev-list --count <base>..HEAD` = commits.

- [ ] **Step 1:** Failing golden tests: keep result, remove result, `Returned to {cwd}` line, discard_changes refusal (dirty+false → refuse, no swap, no removal), discard_changes:true forces removal, no-op-outside-session, cwd restored to origin after exit.
- [ ] **Step 2:** Rewrite `call`: no active worktree → no-op message (session inactive, filesystem unchanged); else compute change-summary; if `remove` && dirty && !discard_changes → refuse (errorCode:1); else restore cwd via `ctx.session_cwd.swap(original)`, remove (if `remove`) or keep, emit `tengu_worktree_kept`/`tengu_worktree_removed`, return `wCd` message.
- [ ] **Step 3:** tmux: on `remove` kill the attached tmux session; on `keep` leave it + return its name. Use the swarm tmux substrate (`platforms/posix/src/swarm/tmux.rs`) where reachable; if a worktree-attached tmux session isn't tracked, document the residual and omit the reattach-name (verify-before-change — do NOT invent a session name).
- [ ] **Step 4:** Update `description()`/`prompt()` to `vCd`.
- [ ] **Step 5:** `cargo test -p tool-worktree` — PASS.
- [ ] **Step 6:** Commit `feat(worktree): ExitWorktree 206 parity (action/discard_changes/tmux) + cwd restore`.

---

## Final whole-branch review
After Task 8: dispatch a broad code-reviewer over the full range (`git merge-base` → HEAD) focused on the INERT INVARIANT (no-swap byte-identical), the highest-risk Task 2 migration, and byte-fidelity of the 206 strings.
