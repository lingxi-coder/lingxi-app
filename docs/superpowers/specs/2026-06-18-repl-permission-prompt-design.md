# Wire interactive permission prompts into the stdio REPL (design)

**Date:** 2026-06-18
**Status:** approved for planning
**Origin:** follow-up to the TUI permission prompt (`docs/superpowers/specs/2026-06-17-tui-permission-prompt-design.md`, merged to main `2a51d90c`). That wired interactive prompts for the iocraft TUI but explicitly left the `--no-tui` stdio REPL routing through `build_runtime` with no injected gate — so an unresolved mutating `Ask` in an interactive REPL session still silently auto-allows via `NoOpPermissionGate`. This closes that documented limitation for the interactive (TTY) REPL.

## Goal

When the stdio REPL runs interactively (stdin is a TTY), surface claude-code's byte-locked `y/n` permission prompt over stdin/stderr for an otherwise-unresolved mutating-tool `Ask`, instead of auto-allowing. Reuse the already-built `InteractivePromptingGate`. Share a single stdin reader between the REPL loop and the gate so the two never race on fd 0 or lose type-ahead bytes.

## Background (what already exists)

- `permission::InteractivePromptingGate` (`permission/src/prompting_gate.rs`) is a complete `PermissionGate`: a byte-locked stdin/stderr prompt loop (`"Claude needs your permission to use {tool}\n[y/N] "`), `y`/`yes`/`n`/`no`/Empty parsing, `MAX_RETRIES = 3`, EOF→`Deny`. It is **inert today** — constructed only in its own unit tests; no production path wires it.
- The engine composition root already honors `cfg.injected_permission_gate` (`apps/engine-desktop/src/lib.rs:1836`, the `if let Some(injected)` arm), wrapping it in the default-on `PolicyPermissionGate`. The TUI feature added the shared `build_runtime_from_config(cfg, output)` seam (`apps/cli/src/init.rs`).
- The REPL (`apps/cli/src/repl.rs::run_repl`) builds via `build_runtime` (no gate), owns `BufReader::new(tokio::io::stdin())`, and drives a cancel-safe pinned `read_line` loop in `step` (`apps/cli/src/repl_loop.rs`). `Mode::StdioRepl` is selected for `--no-tui` OR a non-TTY (`apps/cli/src/mode.rs::decide_mode`), so `run_repl` serves both the interactive `--no-tui`-on-TTY case and the piped/CI non-TTY case.

## The core problem: stdin ownership

Two independent line-buffered readers over fd 0 lose bytes at their buffer boundary on type-ahead, and `tokio::io::stdin()` runs an internal blocking reader thread — so a second `tokio::io::stdin()` racing the REPL's is undefined. The REPL loop and the gate never read *concurrently* (the REPL reads the prompt line, then the turn runs and the gate reads `y/n` during it), so the correct design is a **single shared `BufReader`** used by both, with the gate reading from it directly (no second `BufReader`).

A subtlety drives a small gate refinement: `InteractivePromptingGate` currently wraps its injected reader in its own `BufReader` (`prompting_gate.rs:195`) to avoid losing pre-fetched bytes across retry iterations *within one prompt*. If the REPL passes an already-`BufReader`-wrapped shared reader, that inner wrap double-buffers and can drop type-ahead bytes belonging to the REPL's next read. Fix: the gate accepts and reads from a shared `AsyncBufRead` directly.

## Components / file structure

1. **`permission/src/prompting_gate.rs`** — refine the gate to be share-friendly:
   - Change the `stdin` field type from `Arc<Mutex<dyn AsyncRead + Send + Unpin>>` to `Arc<Mutex<dyn AsyncBufRead + Send + Unpin>>`.
   - In `prompt_user`, delete the inner `let mut reader = BufReader::new(&mut *in_guard);` and call `read_line` directly on `&mut *in_guard` (the guard is now `AsyncBufRead`). The retry loop reads successive lines from the SAME shared buffer — byte-correct.
   - `with_stdio()` wraps `BufReader::new(tokio::io::stdin())` into the `Arc<Mutex<dyn AsyncBufRead…>>`.
   - Update the gate's own unit tests' construction (they pipe `tokio::io::duplex`; wrap the read half in `BufReader`). Behavior/assertions unchanged.
   - Safe because the gate is inert in production today (no caller besides tests + `with_stdio`, which this updates).

2. **`apps/cli/src/repl.rs`** (`run_repl`) — own one shared reader + conditionally inject:
   - `let stdin_reader: Arc<Mutex<BufReader<Stdin>>> = Arc::new(Mutex::new(BufReader::new(stdin())));`
   - Add a pure helper `fn should_prompt_interactively(is_tty: bool, print: bool) -> bool { is_tty && !print }` (REPL is never `print`, but the helper keeps the rule explicit + testable).
   - When `std::io::stdin().is_terminal()`: resolve config once (`init::resolve_desktop_config(argv, permission_mode)`), build `InteractivePromptingGate::new(stdin_reader.clone() as Arc<Mutex<dyn AsyncBufRead + Send + Unpin>>, Arc::new(Mutex::new(tokio::io::stderr())))`, set `cfg.injected_permission_gate = Some(Arc::new(gate))`, and build via `init::build_runtime_from_config(cfg, adapter)`.
   - **Visibility:** `resolve_desktop_config` is currently a private `fn` in `init.rs`; `run_repl` lives in `repl.rs`, so promote it to `pub(crate)` (and `build_runtime_from_config` is already `pub`). No behavior change — only the module boundary.
   - When not a TTY: build via the existing `build_runtime(argv, adapter, permission_mode)` — byte-identical to today (no gate).
   - Pass `stdin_reader` (the shared handle) to `step` instead of a local `&mut BufReader`.

3. **`apps/cli/src/repl_loop.rs`** (`step`) — read under a short-lived lock:
   - Change the signature to take `stdin: Arc<Mutex<dyn AsyncBufRead + Send + Unpin>>` (drop the `R` generic; keep `W` for stderr).
   - Acquire the lock, run the existing pinned-`read_line` + SIGINT/idle `select!` **inside** the locked scope, then **drop the guard before** invoking `run_turn_fn`. Between turns the gate never reads, so holding the lock across the prompt-wait `select!` is contention-free; releasing before the turn lets the gate lock it for `y/n`.
   - Cancel-safety unchanged: the `read_line` future is still pinned once and re-awaited across `select!` iterations within the locked scope.

### Data flow
```
run_repl: ONE Arc<Mutex<BufReader<Stdin>>>
   ├─ stdin is a TTY? → InteractivePromptingGate(stdin_reader.clone(), stderr)
   │                      → cfg.injected_permission_gate → build_runtime_from_config
   │                          (base gate wrapped by default-on PolicyPermissionGate)
   └─ loop: step(stdin_reader, …)
        lock → write "> " → pinned read_line (select sigint/idle) → got line → UNLOCK
             → run_turn_fn(prompt)
                  └─ unresolved mutating Ask → gate.check():
                        lock stdin_reader → "Claude needs your permission to use Write\n[y/N] "
                        → read y/n → UNLOCK → Allow / Deny
```

## Why no deadlock / no byte loss

- `step` holds the lock ONLY while reading the prompt line (between turns). The gate locks ONLY during a turn. These phases are strictly sequential within one loop iteration, so the `tokio::sync::Mutex` is never contended and never deadlocks.
- There is exactly ONE `BufReader` over fd 0, shared via the `Arc<Mutex>`. Neither side creates a second buffer, so type-ahead bytes are never stranded behind a dropped buffer.
- Only ONE `tokio::io::stdin()` exists → no double blocking-reader-thread race.
- stderr is NOT shared: `step` writes `"> "` between turns; the gate writes its prompt during a turn (its own `tokio::io::stderr()`). Writes to fd 2 are sequential and interleave-safe — no read-thread hazard like stdin.

## Behavior & scope

- **Enforcement reuse:** the injected gate is the base, wrapped by `PolicyPermissionGate` (default-on). Deny/allow rules, read-only auto-allow, and sandbox-auto-allow resolve BEFORE the gate sees an `Ask` — only an unresolved mutating `Ask` reaches the `y/n` prompt. Identical to the TUI path.
- **`y/n` only:** no "always"/`AllowAlways` and no `settings.local.json` persistence — byte-locked to claude-code's stdio prompt; AllowAlways stays a TUI feature.
- **Non-interactive untouched:** piped/CI `StdioRepl` (non-TTY) injects no gate → today's `NoOpPermissionGate` auto-allow. `Mode::Print` (`-p`) is a different path entirely → unchanged (`deny_unresolved_ask` already denies).

## Error handling

- stdin EOF / closed mid-prompt → gate returns `Deny { reason: "prompt cancelled: stdin closed" }` (already implemented).
- ≥3 consecutive invalid inputs → `Deny` (gate `MAX_RETRIES`).
- SIGINT during a turn → `run_turn_with_cancel`'s token cancels the turn; the gate's in-flight `read_line` future is dropped (not cancel-safe, but the turn is being torn down anyway) → the check resolves `Deny`. Consistent with the existing accepted SIGINT-drop note in `repl_loop.rs`.
- Gate build/inject never fails `run_repl` (infallible construction).

## Testing (TDD)

- **Gate (`prompting_gate.rs`):** the `AsyncBufRead` refactor keeps the existing duplex-scripted unit tests green (wrap the duplex read half in `BufReader`); line-reading semantics and the retry/EOF assertions are unchanged. Add a focused test that a single shared `BufReader` carrying `"y\n"` then a follow-up line is read without losing the follow-up (proves no double-buffer byte loss).
- **`should_prompt_interactively`:** pure unit tests — `(true, false) → true`; `(false, false) → false`; `(true, true) → false`.
- **`step` refactor:** existing `step` tests adapted to pass `Arc<Mutex<BufReader<_>>>` (over a `tokio::io::duplex`/cursor). Add a **lock-release test**: a fake `run_turn_fn` that locks the SAME shared reader and reads a pre-queued `"y\n"` succeeds — proving `step` dropped the guard before the turn (no deadlock, byte available to the gate).
- **Integration (the real seam):** `PolicyPermissionGate::new(PermissionPolicy::new(Default), Arc::new(InteractivePromptingGate::new(shared, stderr)))` over a scripted shared `BufReader` — `check("Write", …)` with `"y\n"` queued → `Allow`; with `"n\n"` → `Deny`; `check("Read", …)` → `Allow` with NO byte consumed (read-only resolves in the policy, gate never prompts). Mirrors the TUI `policy_gate_*` tests.
- **Regression:** non-TTY `run_repl` still builds via `build_runtime` (no gate); existing REPL `step`/pty smoke tests stay green.

## Out of scope

- Trust dialog / `project_trust` store (the next, separate cycle).
- "Always"/`AllowAlways` in the REPL.
- Changing piped/non-TTY REPL behavior (still auto-allow; a deny-on-ask variant is a separate decision).
- The `OrchestratorConfig::interactive_permissions` flag — left as-is; injection is via `injected_permission_gate` (the seam the engine actually consumes), consistent with the TUI.
