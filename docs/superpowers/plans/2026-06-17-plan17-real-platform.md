# Plan 17 — wire the real `platform-posix` into desktop (implementation plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace every stubbed `platform-posix-minimal` primitive wired at the desktop composition root with its real `platform-posix` equivalent, so the desktop CLI makes real network requests, runs real processes under a real sandbox, connects MCP transports, uses real git worktrees, and persists secrets to the OS keychain.

**Architecture:** Single branch (`plan17-real-platform`), one focused commit per primitive (or cohesive pair), in increasing-risk order so each swap is independently buildable and testable. The engine only *wires* these primitives — their behavior is already covered by `platform-posix`'s own test suite — so verification is "builds clean + existing affected tests pass + the stub message is gone", not re-proving end-to-end behavior.

**Tech Stack:** Rust workspace at `/Users/luolingfeng/Projects/LingXi-Next/lingxi-code`. Crates: `platform-posix` (real, reqwest/tokio/notify/git-backed), `platform-common` (shared `ReqwestHttp`), `platform-posix-minimal` (stub contract — stays in workspace, only the *dependency* from `engine-desktop`/`cli` is dropped). Composition root: `apps/engine-desktop/src/lib.rs` `build()` (async). CLI direct sites: `apps/cli/src/bypass_env.rs`, `apps/cli/src/run.rs`.

**Design doc:** `docs/superpowers/specs/2026-06-17-plan17-real-platform-design.md`

---

## Conventions for every task

- **Branch:** Work on `plan17-real-platform`. Before editing, run `git rev-parse --abbrev-ref HEAD` and confirm it prints `plan17-real-platform`. If a harness placed you on another branch or in a detached/worktree HEAD, run `git checkout plan17-real-platform` first. If the branch's tip differs from what the previous task committed, `git log --oneline -3` and reconcile (fast-forward) before starting.
- **Build prefix:** Disk is near-full. Prefix EVERY cargo command with `CARGO_PROFILE_DEV_DEBUG=0` (and `CARGO_PROFILE_TEST_DEBUG=0` for `cargo test`).
- **Staging:** Stage ONLY the exact files named in each task with `git add <path> <path>`. NEVER `git add -A` or `git add .` — the working tree has many unrelated untracked files (`session/*`, `.codegraph/`, `codex/`, `docs/parity-*`, etc.) that must never be swept in.
- **Never touch** any `session/*` working-tree file.
- **Line numbers are approximate** — they drift as edits land. Always re-grep for the exact construction site before editing (each task gives the grep). Match on the call expression, not the line number.

---

## Task 0: Baseline — capture the stub message and confirm the start state

**Files:** none (read-only baseline)

- [ ] **Step 1: Confirm branch and the import block**

Run:
```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
git rev-parse --abbrev-ref HEAD
grep -n "platform_posix_minimal" apps/engine-desktop/src/lib.rs apps/cli/src/bypass_env.rs apps/cli/src/run.rs
```
Expected: branch is `plan17-real-platform`; the engine import line is
```rust
use platform_posix_minimal::{
    PlainTextSecureStorage, PosixClock, PosixFileSystem, PosixHttp, PosixMcp, PosixProcess,
    PosixRuntime, PosixSandbox, PosixWorktree,
};
```
(exact grouping may differ) plus the CLI sites in `bypass_env.rs` and `run.rs`.

- [ ] **Step 2: Record the baseline stub message string**

Run:
```bash
grep -rn "posix-minimal: HTTP stub" platforms/posix-minimal/src/
```
Expected: the stub HTTP transport returns an error containing `posix-minimal: HTTP stub`. This is the string that must DISAPPEAR after Task 1. Note it for the Task 1 stub-gone check.

- [ ] **Step 3: Confirm a clean baseline build**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-desktop -p lingxi-cli`
Expected: builds clean (warnings OK). If it does not build, STOP and report — the baseline is broken before any Plan 17 change.

No commit (read-only).

---

## Task 1: HTTP + Clock + Runtime (the headline fix)

These three are import-only drop-ins: same type names, same `new()` constructors. `PosixClock`/`PosixRuntime` are already real even in the minimal crate; `PosixHttp` is the stub→real swap that makes the CLI able to make real API calls.

**Files:**
- Modify: `apps/engine-desktop/src/lib.rs` (import block, `:54-57` approx)

- [ ] **Step 1: Re-grep the import block**

Run: `grep -n "use platform_posix_minimal::{" apps/engine-desktop/src/lib.rs`
Note the exact line and the full brace list.

- [ ] **Step 2: Split the import — move HTTP/Clock/Runtime to `platform_posix`**

Edit the import block so `PosixHttp`, `PosixClock`, `PosixRuntime` come from `platform_posix` and the rest stay on `platform_posix_minimal` for now. Result:
```rust
use platform_posix::{PosixClock, PosixHttp, PosixRuntime};
use platform_posix_minimal::{
    PlainTextSecureStorage, PosixFileSystem, PosixMcp, PosixProcess, PosixSandbox, PosixWorktree,
};
```
(Keep whatever other imports already exist; only move these three names across.)

- [ ] **Step 3: Build**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-desktop`
Expected: clean. `PosixHttp::new()`, `PosixClock::new()`, `PosixRuntime::new()` all exist on the real crate with the same signatures, so no call-site edits are needed. If the compiler complains that `PosixHttp` is ambiguous or that a method moved, re-read the real `platform_posix::PosixHttp` signature and reconcile.

- [ ] **Step 4: Stub-gone check (the proof the HTTP swap took)**

Run:
```bash
CARGO_PROFILE_DEV_DEBUG=0 cargo build -p lingxi-cli
CLAUDE_CODE_MAX_RETRIES=0 ./target/debug/lingxi-cli -p hello 2>&1 | tee /tmp/plan17-http.txt; echo "exit=$?"
grep -c "posix-minimal: HTTP stub" /tmp/plan17-http.txt
```
Expected: the `grep -c` prints `0` — the stub message is GONE. The command instead fails (in this no-network sandbox) with a *real* transport/connection error (e.g. a reqwest DNS/connect error), which is the desired new behavior. The binary still exits promptly (retries capped at 0). Capture `/tmp/plan17-http.txt` content in your report.

- [ ] **Step 5: Run affected tests**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop`
Expected: pass (or the same pre-existing failures as baseline — none should be newly introduced by an import swap).

- [ ] **Step 6: Commit**

```bash
git add apps/engine-desktop/src/lib.rs
git commit -m "feat(engine-desktop): wire real PosixHttp/Clock/Runtime (Plan 17.1)

Swaps the stub platform-posix-minimal HTTP transport for the real
reqwest-backed platform_posix::PosixHttp at the composition root, so the
desktop CLI makes real network requests. Clock/Runtime move with it
(import-only, same new() ctors).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: FileSystem — consolidate to all-real `PosixFileSystem::new(cwd)`

The minimal `PosixFileSystem` has a stubbed file-watcher; the real one uses a `notify` watcher. The ctor is already `new(path)` in both crates, and the engine already constructs the *real* `platform_posix::PosixFileSystem` at two sites (`:2861,:2910`). This task moves the import to the real crate so ALL `PosixFileSystem::new(...)` sites resolve to the real type.

**Files:**
- Modify: `apps/engine-desktop/src/lib.rs` (import block + verify call sites)

- [ ] **Step 1: Inventory the FileSystem call sites**

Run: `grep -n "PosixFileSystem::new" apps/engine-desktop/src/lib.rs`
Expected: several sites. Some already qualify the real crate as `platform_posix::PosixFileSystem::new(...)` (the already-real `:2861,:2910`); others use the bare `PosixFileSystem::new(...)` that currently resolves to the minimal import.

- [ ] **Step 2: Move `PosixFileSystem` to the real import**

Edit the import block: remove `PosixFileSystem` from the `platform_posix_minimal::{…}` list and add it to the `platform_posix::{…}` list:
```rust
use platform_posix::{PosixClock, PosixFileSystem, PosixHttp, PosixRuntime};
use platform_posix_minimal::{PlainTextSecureStorage, PosixMcp, PosixProcess, PosixSandbox, PosixWorktree};
```

- [ ] **Step 3: De-duplicate the already-qualified sites (optional tidy)**

For any site already written as `platform_posix::PosixFileSystem::new(...)`, you MAY simplify it to the bare `PosixFileSystem::new(...)` now that the bare name resolves to the real crate — but only if it reads cleaner. Leaving the fully-qualified form is also correct. Do NOT change the constructor arguments.

- [ ] **Step 4: Build**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-desktop`
Expected: clean. The real `PosixFileSystem::new(cwd)` has the same `new(PathBuf)` signature; if any site passed a `&Path` vs `PathBuf` mismatch surfaces, adapt the argument (`.to_path_buf()` / `.clone()`) to match the real signature without changing intent.

- [ ] **Step 5: Run affected tests**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop`
Expected: pass / no new failures.

- [ ] **Step 6: Commit**

```bash
git add apps/engine-desktop/src/lib.rs
git commit -m "feat(engine-desktop): wire real PosixFileSystem with notify watcher (Plan 17.2)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Process + Sandbox — the bash/shell tool goes stub → real

These are paired: they feed the same process-runner builder chain (`with_process_runner(process, sandbox)`). Both are import-only (`new()` ctors). The real `PosixProcess` runs tokio child processes with timeout + env contract; the real `PosixSandbox` self-checks `sandbox-exec` (macOS) / `bwrap` (Linux) availability and degrades to a wrapped no-op when absent, so booting on a host without the binary still succeeds.

**Files:**
- Modify: `apps/engine-desktop/src/lib.rs` (import block + verify the builder-chain sites)

- [ ] **Step 1: Inventory the Process/Sandbox sites**

Run: `grep -n "PosixProcess::new\|PosixSandbox::new" apps/engine-desktop/src/lib.rs`
Expected: paired sites (Process `:2123,:2234,:2485`; Sandbox `:2124,:2235,:2486` approx).

- [ ] **Step 2: Move both to the real import**

Edit the import block: move `PosixProcess` and `PosixSandbox` from the minimal list to the real list:
```rust
use platform_posix::{PosixClock, PosixFileSystem, PosixHttp, PosixProcess, PosixRuntime, PosixSandbox};
use platform_posix_minimal::{PlainTextSecureStorage, PosixMcp, PosixWorktree};
```

- [ ] **Step 3: Build**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-desktop`
Expected: clean. Both real types expose `new()`. If the real `PosixProcess`/`PosixSandbox` `new()` takes an argument the stub did not (e.g. a config), read the real signature via `grep -n "pub fn new" platforms/posix/src/process.rs platforms/posix/src/sandbox.rs` and supply the argument the surrounding context already has (cwd / config). Report any signature divergence in your status.

- [ ] **Step 4: Run affected tests**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop`
Expected: pass / no new failures.

- [ ] **Step 5: Sandbox availability sanity (macOS host)**

This dev host is macOS (has `sandbox-exec`); Linux `bwrap` is NOT exercised here. Just confirm the build links and the engine boots — the real sandbox's availability self-check is covered by `platform-posix`'s own tests. No extra command beyond the build/test above.

- [ ] **Step 6: Commit**

```bash
git add apps/engine-desktop/src/lib.rs
git commit -m "feat(engine-desktop): wire real PosixProcess + PosixSandbox (Plan 17.3)

The bash/shell tool now executes real child processes under the real
sandbox (sandbox-exec on macOS / bwrap on Linux, self-checking
availability with a documented wrapped-no-op fallback).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: SecureStorage — swap to the `secure_storage_for_platform` async factory

The minimal `PlainTextSecureStorage` is a stub (`BackendUnavailable`). Replace its single construction site with the platform factory, which auto-selects macOS Keychain / Linux libsecret with a plaintext-file fallback and warns once on fallback. `build()` is already `async`, so the `.await` wires cleanly. The factory never fails `build()` for a missing backend (it falls back); it only errors if it cannot create the plaintext base dir — surface that as a `BuildError`.

**Files:**
- Modify: `apps/engine-desktop/src/lib.rs` (import block + the storage construction site `:1322` approx)

- [ ] **Step 1: Read the factory signature and the current site**

Run:
```bash
grep -n "pub async fn secure_storage_for_platform" platforms/posix/src/secure_storage/factory.rs
grep -n "PlainTextSecureStorage::new\|let storage =" apps/engine-desktop/src/lib.rs
grep -n "claude_home\|config_dir\|cfg\." apps/engine-desktop/src/lib.rs | head -40
```
Expected signature:
```rust
pub async fn secure_storage_for_platform(
    user: String,
    config_dir: PathBuf,
    plaintext_path: PathBuf,
) -> Result<Arc<dyn SecureStorage>, SecureStorageError>
```
Current site: `let storage = Arc::new(PlainTextSecureStorage::new());`. Confirm `cfg.claude_home` (or the equivalent config dir field on `DesktopConfig`) is in scope at the construction site. If the field is named differently, use the actual field that holds the user's `~/.claude`-equivalent config directory.

- [ ] **Step 2: Add the factory + error imports**

Ensure the import section brings in the factory and its error. Add (next to the other `platform_posix` imports):
```rust
use platform_posix::secure_storage::{secure_storage_for_platform, SecureStorageError};
```
Verify the real path with `grep -rn "pub use\|pub mod secure_storage\|pub fn secure_storage_for_platform" platforms/posix/src/lib.rs platforms/posix/src/secure_storage/mod.rs` and use whatever the crate actually re-exports (it may already re-export `secure_storage_for_platform` at the crate root — prefer the shortest valid path).

- [ ] **Step 3: Replace the construction site**

Replace:
```rust
let storage = Arc::new(PlainTextSecureStorage::new());
```
with:
```rust
let storage = secure_storage_for_platform(
    whoami::username(),
    cfg.claude_home.clone(),
    cfg.claude_home.join(".credentials.json"),
)
.await
.map_err(|e| BuildError::SecureStorage(e.to_string()))?;
```
Notes:
- `secure_storage_for_platform` already returns `Arc<dyn SecureStorage>` — do NOT wrap it in another `Arc::new(...)`.
- For the `user` argument: if the `whoami` crate is not already a dependency, use whatever the codebase already uses to get the current username (grep `whoami\|USER\|username` across `apps/engine-desktop` and `platforms/posix`). If nothing exists, read it from the `USER` env with a fallback: `std::env::var("USER").unwrap_or_else(|_| "default".to_string())`. Do NOT add a new dependency just for this — reuse what's present.
- For `plaintext_path`: a `.credentials.json` (or the existing plaintext-credentials filename the codebase already uses — grep `credentials.json` to match it) under the config dir.

- [ ] **Step 4: Add the `BuildError::SecureStorage` variant if absent**

Run: `grep -n "enum BuildError\|SecureStorage" apps/engine-desktop/src/lib.rs`
If `BuildError` has no `SecureStorage` variant, add one:
```rust
    #[error("secure storage init failed: {0}")]
    SecureStorage(String),
```
(Match the existing `thiserror`/manual `Display` style of the surrounding `BuildError` variants — if it's not `thiserror`, add the arm to the manual `Display`/`Error` impls instead.) If a suitable existing variant already conveys init failure, reuse it rather than adding a new one.

- [ ] **Step 5: Remove `PlainTextSecureStorage` from the minimal import**

Edit the import block to drop `PlainTextSecureStorage` from the `platform_posix_minimal::{…}` list (only `PosixMcp`, `PosixWorktree` should remain there after this task).

- [ ] **Step 6: Build**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-desktop`
Expected: clean. If `SecureStorage` (the trait) isn't in scope for the `Arc<dyn SecureStorage>` type, it's already imported wherever `storage` is consumed — the factory returns the boxed trait object, so you typically don't need to name the trait. Resolve any unused-import warning by removing a now-dead `SecureStorage` trait import only if the compiler flags it.

- [ ] **Step 7: Run affected tests**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop`
Expected: pass / no new failures. On this macOS host the factory selects the Keychain backend (or falls back to plaintext with a single warn) — either is acceptable; `build()` must succeed.

- [ ] **Step 8: Commit**

```bash
git add apps/engine-desktop/src/lib.rs
git commit -m "feat(engine-desktop): wire secure_storage_for_platform factory (Plan 17.4)

Replaces the stub PlainTextSecureStorage with the platform factory:
macOS Keychain / Linux libsecret with a warn-once plaintext fallback.
build() is already async so the factory awaits cleanly; only a
plaintext-base-dir failure surfaces as BuildError.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: MCP — rename `PosixMcp` → `PosixMcpTransport`

The real MCP transport type is named `PosixMcpTransport` (`platforms/posix/src/mcp.rs:145`), with `new()`. Rename the import and the single construction site (`:1881` approx).

**Files:**
- Modify: `apps/engine-desktop/src/lib.rs`

- [ ] **Step 1: Find the construction site**

Run: `grep -n "PosixMcp" apps/engine-desktop/src/lib.rs`
Expected: an import reference + a construction (`PosixMcp::new(...)`).

- [ ] **Step 2: Move + rename the import**

Remove `PosixMcp` from the `platform_posix_minimal::{…}` list; add `PosixMcpTransport` to the `platform_posix::{…}` list.

- [ ] **Step 3: Rename the construction site**

Change `PosixMcp::new(...)` → `PosixMcpTransport::new(...)`. Keep the exact arguments (the real `new()` is argument-compatible — confirm with `grep -n "pub fn new" platforms/posix/src/mcp.rs`; it's `new() -> Self`).

- [ ] **Step 4: Build**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-desktop`
Expected: clean.

- [ ] **Step 5: Run affected tests**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop`
Expected: pass / no new failures.

- [ ] **Step 6: Commit**

```bash
git add apps/engine-desktop/src/lib.rs
git commit -m "feat(engine-desktop): wire real PosixMcpTransport (Plan 17.5)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Worktree — rename + `PosixWorktreeManager::new(repo_root)`

The real type is `PosixWorktreeManager` with `new(repo_root: PathBuf)` (`platforms/posix/src/worktree.rs:121,133`). The stub `PosixWorktree::new()` took no repo root. Rename the import and the single site (`:2519` approx), supplying the repo root (the engine's `cwd`).

**Files:**
- Modify: `apps/engine-desktop/src/lib.rs`

- [ ] **Step 1: Find the construction site + confirm `cwd` in scope**

Run:
```bash
grep -n "PosixWorktree" apps/engine-desktop/src/lib.rs
grep -n "let cwd\|cfg.cwd" apps/engine-desktop/src/lib.rs | head
```
Expected: a construction `PosixWorktree::new(...)`; `cwd` (= `cfg.cwd.clone()`) is bound near the top of `build()` and in scope.

- [ ] **Step 2: Move + rename the import**

Remove `PosixWorktree` from the `platform_posix_minimal::{…}` list (which should now be EMPTY — delete the whole `use platform_posix_minimal::{…};` line in this task); add `PosixWorktreeManager` to the `platform_posix::{…}` list.

- [ ] **Step 3: Rename + add the repo-root argument**

Change `PosixWorktree::new()` → `PosixWorktreeManager::new(cwd.clone())`. Use the repo root the worktree manager should operate against — that's the engine `cwd`. If the surrounding code has a more specific repo-root binding (e.g. a discovered git toplevel), prefer that; otherwise `cwd.clone()` is correct.

- [ ] **Step 4: Build**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-desktop`
Expected: clean. There should now be NO remaining `platform_posix_minimal` reference in `apps/engine-desktop/src/lib.rs` production code.

- [ ] **Step 5: Confirm the engine src is minimal-free**

Run: `grep -rn "platform_posix_minimal" apps/engine-desktop/src/`
Expected: zero hits in non-test code. If a `#[cfg(test)]` site remains, note it for Task 8 (the sweep).

- [ ] **Step 6: Run affected tests**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop`
Expected: pass / no new failures.

- [ ] **Step 7: Commit**

```bash
git add apps/engine-desktop/src/lib.rs
git commit -m "feat(engine-desktop): wire real PosixWorktreeManager(repo_root) (Plan 17.6)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: CLI direct sites — `bypass_env.rs` HTTP + `run.rs` FileSystem

`apps/cli` has two direct stub uses: `bypass_env.rs:33` constructs the stub HTTP for the `has_internet` check (so it's always false), and `run.rs:342,:371` constructs the stub FileSystem for session-row loading. Swap both to `platform_posix`, and add `platform-posix` to `apps/cli/Cargo.toml`.

**Files:**
- Modify: `apps/cli/Cargo.toml` (add `platform-posix`)
- Modify: `apps/cli/src/bypass_env.rs`
- Modify: `apps/cli/src/run.rs`

- [ ] **Step 1: Add the `platform-posix` dependency**

Run: `grep -n "platform-posix" apps/cli/Cargo.toml`
If `platform-posix` is not listed, add it next to the existing `platform-posix-minimal` line, mirroring its `path`/`workspace` form. Read how `engine-desktop` declares it for the exact form:
```bash
grep -n "platform-posix" apps/engine-desktop/Cargo.toml
```
Add the matching line to `apps/cli/Cargo.toml` (e.g. `platform-posix = { path = "../../platforms/posix" }` or the workspace-dep form the repo uses).

- [ ] **Step 2: Swap `bypass_env.rs` HTTP**

Run: `grep -n "platform_posix_minimal" apps/cli/src/bypass_env.rs`
Change `platform_posix_minimal::http::PosixHttp::new()` → `platform_posix::PosixHttp::new()` (or the matching real module path — confirm with `grep -n "pub use.*PosixHttp\|pub struct PosixHttp" platforms/posix/src/lib.rs platforms/posix/src/http.rs`; the crate re-exports `platform_posix::PosixHttp`).

- [ ] **Step 3: Swap `run.rs` FileSystem (both sites)**

Run: `grep -n "platform_posix_minimal" apps/cli/src/run.rs`
Change each `platform_posix_minimal::PosixFileSystem::new(...)` → `platform_posix::PosixFileSystem::new(...)`, keeping the exact arguments.

- [ ] **Step 4: Build**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p lingxi-cli`
Expected: clean. `has_internet` now performs a real connectivity check instead of always-false.

- [ ] **Step 5: Confirm CLI src is minimal-free**

Run: `grep -rn "platform_posix_minimal" apps/cli/src/`
Expected: zero hits in non-test code (note any `#[cfg(test)]` remainder for Task 8).

- [ ] **Step 6: Run affected tests**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p lingxi-cli`
Expected: pass / no new failures. The `pty_smoke` `print_mode_unaffected_by_tui_routing` test (already fixed with `CLAUDE_CODE_MAX_RETRIES=0`) still passes — it now exits on a *real* connection error rather than the stub error, but the assertion only checks termination.

- [ ] **Step 7: Commit**

```bash
git add apps/cli/Cargo.toml apps/cli/src/bypass_env.rs apps/cli/src/run.rs
git commit -m "feat(cli): wire real platform_posix HTTP + FileSystem (Plan 17.7)

has_internet now does a real connectivity probe; session-row loading
uses the real filesystem. Adds the platform-posix dependency.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Dependency sweep — migrate remaining refs, drop `platform-posix-minimal`

Migrate any leftover `platform_posix_minimal` references (including `#[cfg(test)]` sites surfaced in Tasks 6/7), then remove the now-unreferenced `platform-posix-minimal` dependency from `engine-desktop` and `cli`. The `platform-posix-minimal` crate itself STAYS in the workspace.

**Files:**
- Modify: any remaining `platform_posix_minimal` test sites in `apps/engine-desktop/`, `apps/cli/`
- Modify: `apps/engine-desktop/Cargo.toml`, `apps/cli/Cargo.toml` (drop the dep)

- [ ] **Step 1: Find ALL remaining references**

Run: `grep -rn "platform_posix_minimal\|platform-posix-minimal" apps/engine-desktop/ apps/cli/`
Expected: only `Cargo.toml` dependency lines should remain (plus any `#[cfg(test)]` uses not yet migrated).

- [ ] **Step 2: Migrate remaining test-site references**

For each remaining `platform_posix_minimal::X` in a `#[cfg(test)]` block, swap it to the real `platform_posix::X` equivalent (applying the same rename rules: `PosixMcp`→`PosixMcpTransport`, `PosixWorktree`→`PosixWorktreeManager::new(repo_root)`, `PlainTextSecureStorage`→the factory or, in a test, whatever real construction the test needs). If a test specifically exercises the *minimal* contract (i.e. it WANTS the stub), leave it and keep the dep — but the design expects none such here; report it if found.

- [ ] **Step 3: Drop the dependency from both Cargo.toml files**

Run: `grep -n "platform-posix-minimal" apps/engine-desktop/Cargo.toml apps/cli/Cargo.toml`
Remove the `platform-posix-minimal` dependency line from BOTH `[dependencies]` (and any `[dev-dependencies]` if present and unreferenced).

- [ ] **Step 4: Build both crates**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-desktop -p lingxi-cli`
Expected: clean — proving the dep was truly unreferenced. If the compiler now reports an unresolved `platform_posix_minimal`, a reference was missed; go back to Step 1.

- [ ] **Step 5: Full affected-test run**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop -p lingxi-cli`
Expected: pass / no new failures.

- [ ] **Step 6: Commit**

```bash
git add apps/engine-desktop/Cargo.toml apps/cli/Cargo.toml
# plus any test files you migrated in Step 2:
# git add apps/engine-desktop/<test file> apps/cli/<test file>
git commit -m "chore(engine-desktop,cli): drop unused platform-posix-minimal dep (Plan 17.8)

All desktop/CLI sites now use the real platform-posix. The
platform-posix-minimal crate stays in the workspace as the minimal
contract for other consumers; only the dependency edges are removed.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Final verification — workspace build + platform-posix suite + stub-gone

**Files:** none (verification only)

- [ ] **Step 1: Workspace build**

Run: `CARGO_PROFILE_DEV_DEBUG=0 cargo build --workspace`
Expected: clean.

- [ ] **Step 2: platform-posix's own test suite (the real-behavior coverage)**

Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p platform-posix`
Expected: pass. This is the authoritative coverage for real Process/Sandbox/HTTP/Worktree/SecureStorage behavior — the engine only wires them.

- [ ] **Step 3: Final stub-gone confirmation**

Run:
```bash
CLAUDE_CODE_MAX_RETRIES=0 ./target/debug/lingxi-cli -p hello 2>&1 | tee /tmp/plan17-final.txt; echo "exit=$?"
grep -c "posix-minimal" /tmp/plan17-final.txt
```
Expected: `0` hits of any `posix-minimal` stub message. The CLI attempts a real connection (failing only because this sandbox has no network — a *real* transport error, not the stub).

- [ ] **Step 4: Report the verification evidence**

Summarize: workspace build status, `platform-posix` test count, the `/tmp/plan17-final.txt` content proving the stub is gone, and the **environment caveat** — this sandbox has no network and may lack `bwrap`, so a real API turn and the Linux sandbox path are NOT exercised here; their correctness rests on `platform-posix`'s tests (Step 2). macOS `sandbox-exec` is present.

No commit (verification only).

---

## Self-review (run by the plan author before handing off)

**Spec coverage** — every design row mapped:
- PosixHttp/Clock/Runtime (import-only) → Task 1 ✅
- PosixFileSystem (import-only, real watcher) → Task 2 ✅
- PosixProcess + PosixSandbox (import-only, paired) → Task 3 ✅
- PlainTextSecureStorage → `secure_storage_for_platform` factory (async) → Task 4 ✅
- PosixMcp → PosixMcpTransport (rename) → Task 5 ✅
- PosixWorktree → PosixWorktreeManager::new(repo_root) (rename + arg) → Task 6 ✅
- CLI sites (bypass_env HTTP, run.rs FS) + cli Cargo dep → Task 7 ✅
- Dependency sweep + drop minimal dep → Task 8 ✅
- Verification (workspace build, platform-posix suite, stub-gone, env caveat) → Tasks 0/1/9 ✅

**Type consistency** — names used are the real exported ones: `platform_posix::{PosixHttp, PosixClock, PosixRuntime, PosixFileSystem, PosixProcess, PosixSandbox, PosixMcpTransport, PosixWorktreeManager}` and `secure_storage_for_platform(user, config_dir, plaintext_path) -> Result<Arc<dyn SecureStorage>, SecureStorageError>`. The `BuildError::SecureStorage(String)` variant is introduced in Task 4 and used only there.

**Placeholder scan** — no TBD/TODO; every code-changing step shows the code; ambiguous signatures (Process/Sandbox `new`, secure-storage re-export path, username source, plaintext filename) carry an explicit "re-grep and match the real signature" instruction rather than a guess, because the exact form must be confirmed against the live crate at edit time.
