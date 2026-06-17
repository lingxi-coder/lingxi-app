# Plan 17 — wire the real `platform-posix` into desktop (design)

**Date:** 2026-06-17
**Status:** approved for planning
**Origin:** investigating a hung `pty_smoke` test surfaced that `engine-desktop::build()` wires `platform-posix-minimal` — a stub platform. Its HTTP/Process/Sandbox/MCP/Worktree/SecureStorage primitives are stubs that fail every operation, so the desktop CLI cannot make a real API call, run a real process, sandbox, use MCP, or persist secrets. The real implementations exist in `platform-posix` (and shared `platform-common`); production was never switched over. The stub modules carry explicit `Plan 17 …` deferral markers. This completes that migration.

## Goal

Make the desktop CLI fully functional by replacing every stubbed `platform-posix-minimal` primitive with its real `platform-posix` equivalent at the composition root, so a real turn actually performs a network request, the bash/shell tool actually executes (under a real sandbox), MCP transports connect, git worktrees work, and secrets persist to the OS keychain.

## Verified migration surface

All 9 primitives `engine-desktop` imports from `platform_posix_minimal` (`apps/engine-desktop/src/lib.rs:54-57`) have real equivalents in `platform-posix`/`platform-common`. `engine-desktop` already depends on `platform-posix`; `apps/cli` depends only on `platform-posix-minimal` and must add `platform-posix`.

| Primitive | minimal status | real equivalent (`platform_posix::`) | swap kind |
|---|---|---|---|
| `PosixHttp` | STUB (returns `HttpError::Connection`) | `PosixHttp` (= `platform_common::http::ReqwestHttp`), `new()` | import-only |
| `PosixClock` | real | `PosixClock`, `new()` | import-only |
| `PosixRuntime` | real | `PosixRuntime`, `new()` | import-only |
| `PosixFileSystem` | partial (watch is a stub; file ops real) | `PosixFileSystem::new(cwd)` (real `notify` watcher) | import-only (ctor already `new(path)`) |
| `PosixProcess` | STUB (`Unsupported`) | `PosixProcess`, `new()` (tokio process + timeout + env contract) | import-only |
| `PosixSandbox` | STUB (no-op tag) | `PosixSandbox`, `new()` (macOS `sandbox-exec` / Linux `bwrap`, self-checks availability) | import-only |
| `PosixMcp` | STUB | **`PosixMcpTransport`**, `new()` | rename |
| `PosixWorktree` | STUB | **`PosixWorktreeManager::new(repo_root)`** | rename + arg |
| `PlainTextSecureStorage` | STUB (`BackendUnavailable`) | factory `secure_storage_for_platform(user, config_dir, plaintext_path).await` → `Arc<dyn SecureStorage>` | async factory |

`build()` is already `async` (`lib.rs:1312`), so the async secure-storage factory wires cleanly.

**Engine call sites** (`apps/engine-desktop/src/lib.rs`): HTTP `:1320,:1329`; Clock `:1321`; SecureStorage `:1322`; Runtime `:1404,:1531,:1764,:2092,:2205,:2324,:2387,:2584`; FileSystem `:2206,:2209,:2385,:2482` (stub) + `:2861,:2910` (already real); Process `:2123,:2234,:2485`; Sandbox `:2124,:2235,:2486`; MCP `:1881`; Worktree `:2519`. (Lines approximate; the plan re-greps.)

**CLI direct sites** (`apps/cli/src/`): `bypass_env.rs:33` (`PosixHttp` for the `has_internet` check — currently always false because stubbed) and `run.rs:342,:371` (`PosixFileSystem` for session-row loading). Plus `apps/cli/Cargo.toml` (add `platform-posix`).

## Decided (brainstorm)

- **Secure storage = the platform factory** (`secure_storage_for_platform`): auto-selects macOS Keychain / Linux libsecret with plaintext-file fallback — the Plan 17 "OS keychain" end-state. Paths from `cfg.claude_home` + a plaintext fallback file under it.
- **Drop the `platform-posix-minimal` dependency** from `engine-desktop` and `cli` once the swap is complete AND no references remain (production OR test). There are ~9 `platform_posix_minimal` references across `engine-desktop`/`cli` src today (some in tests) — the plan migrates production sites first, then sweeps the remaining (incl. test) references; the dep is removed only after the sweep makes the crate unreferenced. The `platform-posix-minimal` crate itself stays in the workspace (other potential consumers / the minimal-platform contract).

## Approach: full swap, per-primitive ordered commits

Single branch; one focused commit per primitive (or per cohesive pair), in increasing-risk order so each is independently verifiable:

1. **HTTP + Clock + Runtime** — import-only drop-ins, no hidden state. *This alone makes the CLI able to make real API calls (the headline fix).*
2. **FileSystem** — consolidate the already-dual-sourced stub/real to all-real (`PosixFileSystem::new(cwd)`).
3. **Process + Sandbox** — paired in the same `with_process_runner(process, sandbox)` builder chain; the bash/shell tool goes stub→real.
4. **SecureStorage** — swap `:1322` to `secure_storage_for_platform(user, cfg.claude_home, plaintext_path).await?` with error handling (warn + plaintext fallback is inside the factory).
5. **MCP** rename `PosixMcp`→`PosixMcpTransport`.
6. **Worktree** rename + `PosixWorktreeManager::new(cwd)`.
7. **CLI sites** — `bypass_env.rs` HTTP (makes `has_internet` real), `run.rs` FS; add `platform-posix` to `apps/cli/Cargo.toml`.
8. **Dependency sweep + drop** — migrate any remaining `platform_posix_minimal` refs (incl. tests) to `platform_posix`; remove the `platform-posix-minimal` dep from `engine-desktop` + `cli` once unreferenced.

## Components / file structure

- Modify: `apps/engine-desktop/src/lib.rs` — the import block (`:54-57`) + the per-primitive call sites above (3 of them adapted: MCP rename, Worktree rename+arg, SecureStorage async factory).
- Modify: `apps/engine-desktop/Cargo.toml` (drop `platform-posix-minimal` at the end), `apps/cli/Cargo.toml` (add `platform-posix`; drop `platform-posix-minimal` at the end).
- Modify: `apps/cli/src/bypass_env.rs`, `apps/cli/src/run.rs` (swap to real `platform_posix`).
- Possibly modify: a few `#[cfg(test)]` sites in those crates that reference `platform_posix_minimal` (sweep to real).

## Error handling

- Real Process/Sandbox now actually execute — intended. Sandbox self-checks `sandbox-exec`/`bwrap` availability and degrades to wrapped-no-op when absent (documented reason), so a host without the binary still boots.
- Real Worktree shells out to `git`; failures surface as the real `WorktreeManager` errors (no longer silent `Unsupported`).
- SecureStorage factory warns + falls back to plaintext when the OS keychain backend is unavailable; never fails `build()`.

## Testing / verification

- After each primitive swap: `cargo build -p engine-desktop -p cli` clean + the affected existing tests pass.
- The real primitives' behavior is covered by `platform-posix`'s own test suite (`cargo test -p platform-posix`) — the engine only *wires* them, so we rely on those tests for real Process/Sandbox/HTTP/Worktree/SecureStorage correctness rather than re-proving end-to-end.
- **Stub-gone check:** `lingxi-cli -p hello` (with `CLAUDE_CODE_MAX_RETRIES=0`) must no longer emit `posix-minimal: HTTP stub …`; it now attempts a real connection (which, in a no-network env, fails with a real transport/connection error — different from the stub message). Capture this as the concrete proof the HTTP swap took.
- Full affected-crate test run + `cargo build --workspace`.
- **Environment caveat (explicit):** this dev sandbox has no network and may lack `bwrap`, so a real API turn and the Linux sandbox path cannot be fully exercised here; macOS `sandbox-exec` is present. Real-network / Linux-sandbox behavior is asserted by `platform-posix`'s tests, not re-proven in this environment.

## Out of scope

- Changing `platform-posix-minimal` itself (it stays as the minimal-platform contract for other consumers).
- Mobile/android platform wiring (`platforms/android-*`, `engine-mobile`) — separate.
- WebSocket MCP transport completeness (the real MCP is Stdio/SSE/HTTP-complete, WebSocket partial) — out of scope; not regressed.
