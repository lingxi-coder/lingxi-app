# Worktree `--worktree`/`--tmux` Launch Feature — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Make the port act on the (currently parsed-but-inert) `--worktree [name]` and `--tmux` CLI flags: at engine boot, `--worktree` creates+enters a worktree (populating `WorktreeSession`); `--tmux` additionally creates a detached named tmux session for it (`tmux new-session -d -s <name> -c <path>`) and stores its name in `WorktreeSession.tmux_session_name`, activating `ExitWorktree`'s already-coded keep/remove tmux path.

**Oracle:** `/Users/luolingfeng/.local/share/claude/versions/2.1.206`. Tmux create = `Ur("tmux",["new-session","-d","-s",name,"-c",path])` returning `{created, error}` (@216347493); result carries `tmuxSessionName` (@216349702).

## Global Constraints
- Never touch: `llm-client/data/models-dev/openrouter.json`, `llm-client/src/catalog/presets.rs`, `tools/agent/src/agent.rs`, `tools/agent/src/agent_test.rs`. Never `cargo fmt`. Commit to `main`; trailer ends `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. **Disk tight**: `-p` builds only, no throwaway worktrees.
- **INERT INVARIANT:** with NO `--worktree` flag, boot is byte-identical to today. All new work is gated on `argv.worktree.is_some()`.
- Substrate already present: `WorktreeManager::create_worktree` (creates + returns handle), `ctx.session_cwd.swap`, `WorktreeSession` (with `tmux_session_name`, `entered_existing`), `ExitWorktree`'s `Some(tmux_session_name)`-gated keep/remove path. The swarm tmux code (`platforms/posix/src/swarm/tmux.rs`) is pane-oriented — this feature adds SESSION-oriented creation separately.

---

### Task 1: tmux new-session builder (isolated)
**Files:** Create `platforms/posix/src/worktree_tmux.rs` (or a suitable posix module); export it. Test: inline.
**Produces:** `pub fn build_worktree_tmux_argv(session_name: &str, worktree_path: &Path) -> Vec<String>` → `["new-session","-d","-s",name,"-c",path]`; and `pub async fn create_worktree_tmux_session(runner: &dyn ProcessRunner, session_name: &str, worktree_path: &Path) -> Result<(), String>` that runs `tmux <argv>` and maps a non-zero exit to `Err(stderr)` (mirrors 206 `{created:false,error}`).
- [ ] Failing tests: argv shape exact (`new-session -d -s <n> -c <p>`); a mocked runner returning nonzero → `Err`; zero → `Ok`.
- [ ] Implement using the existing posix process seam (match how swarm/tmux runs tmux). Commit `feat(posix): worktree tmux new-session builder`.

### Task 2: worktree tmux session-name derivation
**Files:** wherever the launch wiring will read it (Task 3/4). Test: inline.
**Produces:** `pub fn worktree_tmux_session_name(worktree_name_or_slug: &str) -> String` — extract 206's exact naming during this task (search the binary near `tmuxSessionName`/`new-session`; if unrecoverable, derive a stable slug-legal name from the worktree name and DOCUMENT the deviation — the session name is not a user-facing byte-locked string).
- [ ] Test the naming is stable + tmux-legal (no spaces/dots that break `-s`). Commit.

### Task 3: boot consumption of `--worktree [name]`
**Files:** `apps/engine-desktop/src/lib.rs` (`build_runtime`), gated on `argv.worktree.is_some()`. Test: a boot-path test asserting the worktree is created + session_cwd swapped + WorktreeSession populated (entered_existing=false) when the flag is set; and byte-identical (no worktree, None session) when unset.
- [ ] Locate where `build_runtime` finishes wiring `session_cwd`/`worktree_session`/the WorktreeManager. After that, if `argv.worktree` is `Some(name_or_empty)`: `create_worktree(name_or_random, None, &[])`, `session_cwd.swap(handle.path, [handle.path])`, populate `worktree_session` (original_cwd = pre-swap cwd, entered_existing=false, tmux_session_name=None). Reuse the EnterWorktree slug/random-name logic where possible (extract a shared helper if needed — do NOT duplicate).
- [ ] INERT test: no `--worktree` → boot unchanged, worktree_session None. Commit `feat(engine): --worktree boot creates+enters a worktree`.

### Task 4: boot consumption of `--tmux`
**Files:** `apps/engine-desktop/src/lib.rs`, gated on `argv.worktree.is_some() && argv.tmux.is_some()`. Test: boot-path test with a mocked/faked tmux runner asserting `worktree_session.tmux_session_name == Some(<derived>)`.
- [ ] After Task 3's worktree creation, if `--tmux`: derive the session name (Task 2), call `create_worktree_tmux_session` (Task 1), and on success set `worktree_session.tmux_session_name = Some(name)`. On tmux failure, log + continue WITHOUT a session name (do not fail boot). Require `--worktree` (206: `--tmux` requires `--worktree`) — if `--tmux` without `--worktree`, surface the existing arg-validation or a clear error.
- [ ] INERT test: no `--tmux` → tmux_session_name stays None. Commit `feat(engine): --tmux boot creates a worktree tmux session`.

### Task 5: activate + test ExitWorktree's tmux keep/remove path
**Files:** `tools/worktree/src/worktree.rs` (the `Some(tmux_session_name)`-gated branch — verify/complete it). Test: inline.
- [ ] Confirm ExitWorktree, with a populated `tmux_session_name`, does: on `remove` → kill the tmux session (`tmux kill-session -t <name>`); on `keep` → leave it running and surface its name for reattach (byte-check the keep-result's tmux-name line vs 206 `wCd`/output schema `tmuxSessionName`). Extract any exact 206 output-string during this task.
- [ ] Tests: populate a session with `tmux_session_name=Some("x")`; `remove` → the kill argv was issued (mocked runner); `keep` → session left + name surfaced. Commit `feat(worktree): ExitWorktree manages the worktree tmux session on exit`.

---
## Final whole-branch review
Broad pass: INERT invariant (no `--worktree` = byte-identical boot), the boot-flow wiring reads the SAME shared `session_cwd`/`worktree_session` cells, tmux failures don't break boot, and the tmux argv/naming match 206 where recoverable. Note the un-unit-testable reattach UX needs manual QA (documented).
