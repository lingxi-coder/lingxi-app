# Linux bwrap Sandbox Hardening — Design

**Date:** 2026-06-10
**Status:** Approved (user: do the 5 security findings; defer the socat domain-filter; verify each in Docker)
**Reference of truth:** `claude-code/src/utils/sandbox/sandbox-adapter.ts` (deny/scrub semantics; the bwrap argv builder lives in the UNVENDORED `@anthropic-ai/sandbox-runtime` npm package — NOT in repo — so the bwrap FLAG SHAPE's reference is the container's real bwrap behavior, runtime-verified). Findings are the locked requirements from Batch-14's adversarial review.
**Branch:** new feature branch off `main`, merged locally after gates + Docker verification.
**Related memory:** `[[bwrap-docker-verification]]` (the Docker harness + verified runtime facts), `[[parity-1to1-effort]]` Batch-14.

## Goal

Close the 5 security findings that held Batch-14's bwrap sandbox, building the faithful hardening on top of the current SIMPLIFIED `wrap_linux_bwrap` baseline (the buggy reverted code + its patch are lost). Each fix is runtime-verified inside a `--privileged` Linux Docker container — the original deferral blocker (bwrap must be RUN on Linux) is resolved by that harness.

## Current baseline (verified)

`sandbox/src/wrap.rs::wrap_linux_bwrap` today emits: `--ro-bind / /`, `--tmpfs /tmp`, `--proc /proc`, `--dev /dev`, `--unshare-pid`, `--die-with-parent`, `--bind <p> <p>` per `allow_write`, and `--share-net` (if ANY network wanted) else `--unshare-net`. It has **NO user namespace** (so on a normal unprivileged host bwrap cannot create the pid/net ns at all), **ignores `deny_write` entirely** (denied paths never reach bwrap), and `runtime_config_from_policy` (platforms/posix/src/sandbox.rs:239) maps **both** `Allowed` and `LoopbackOnly` → `allowed_domains:["*"]` → `--share-net` full egress.

## The 5 findings → fixes

### Finding 2 (+ part of 1): conservative network mapping
`runtime_config_from_policy` must NOT map `LoopbackOnly` to full egress. Verified: `bwrap --unshare-net` creates a fresh network namespace with **loopback-only** (external blocked — `getent example.com` → NET_BLOCKED). So:
- `NetworkPolicy::Allowed` → `allowed_domains: ["*"]` (the ONLY full-egress case).
- `NetworkPolicy::LoopbackOnly` → `allowed_domains: []` (mapped to `--unshare-net`: loopback present, external blocked — exactly loopback-only).
- `NetworkPolicy::Disabled` → `allowed_domains: []` (also `--unshare-net`; bwrap always gives loopback in a fresh netns — a documented slight permissiveness vs a true no-loopback, acceptable/safe).
`wrap_linux_bwrap`'s net decision becomes: `--share-net` iff `!allowed_domains.is_empty()` (== `Allowed` only); else `--unshare-net`. Drop `allow_local_binding`/unix-socket fields from the share-net trigger (LoopbackOnly's loopback need is satisfied by the fresh netns).

### Finding 1: missing socat must not un-sandbox
With the conservative mapping there is NO socat domain-filter, so `socat` is irrelevant. Today `dependency_check` lists missing `socat` as a BLOCKING `error`, and posix `prepare` returns UN-SANDBOXED (`backend: None`) when `errors` is non-empty — so a missing socat silently disables the whole sandbox (the finding-1 hazard). Fix: socat → a `warning`, not an `error` (drop it from `into_errors`; keep the `socat: bool` probe + a warning). `bwrap` missing stays a blocking error (no bwrap ⇒ genuinely no sandbox).

### Finding 5: `--unshare-user-try`
`wrap_linux_bwrap` adds `--unshare-user-try` (creates a user namespace where possible; degrades gracefully where unprivileged userns is disabled — verified: with `user.max_user_namespaces=0` the `--unshare-user-try` invocation still ran). This is also a correctness fix: without a userns, an unprivileged bwrap cannot create the pid/net namespaces the baseline already requests.

### Finding 3: deny-write by existence (ro-bind-in-place, never /dev/null)
Verified: `--ro-bind <p> <p>` (in place) denies writes inside the sandbox ("Read-only file system") and leaves the host file unchanged; the buggy `--ro-bind-try /dev/null <p>` REPLACES the file with /dev/null (blanks it). So:
- The posix `prepare` layer (which has FS access; `wrap` is pure) splits denied paths by existence:
  - **existing** denied paths (the generic `deny_write` set that exist + existing bare-repo files) → `ro_bind_in_place` list.
  - **non-existing** bare-repo files → the `scrub` list (finding 4).
- `wrap_linux_bwrap` emits `--ro-bind <p> <p>` for each `ro_bind_in_place` path, placed AFTER the `--bind` (allow_write) binds so a deny overrides a writable parent (bind ordering: later wins — verified in Docker).
- Bare-repo file set (1:1 sandbox-adapter.ts:267): `["HEAD", "objects", "refs", "hooks", "config"]`, scoped over `cwd == original_cwd ? [original_cwd] : [original_cwd, cwd]`, resolved against each dir.

### Finding 4: scrub planted bare-repo files post-command (host-side, no frozen change)
`scrubBareGitRepoFiles` (sandbox-adapter.ts:404) deletes the non-existent-at-config-time bare-repo files after the command, before unsandboxed git can see them. `SandboxedCommand` is in the FROZEN `traits/`, so instead of adding a field, `wrap_linux_bwrap` appends a host-side scrub to the shell string it already produces:
```
bwrap <args> -- /bin/sh -c '<inner>'
rc=$?; rm -rf -- '<scrub p1>' '<scrub p2>' … 2>/dev/null; exit $rc
```
The scrub runs ON THE HOST after bwrap exits (faithful — the planted files land in the host cwd), is ENOENT-tolerant (`rm -f`/`2>/dev/null`), preserves bwrap's exit code (`rc=$?` … `exit $rc` — verified in Docker), and deletes ONLY the scrub list (never the existing files, which went to `ro_bind_in_place`). NO `traits/` change. When the scrub list is empty, no suffix is appended (byte-identical to today).

## Architecture

```
sandbox/src/wrap.rs              # wrap_linux_bwrap: + --unshare-user-try, conservative net,
                                 #   --ro-bind-in-place (ordered after allow_write), host-side scrub append
sandbox/src/dependency_check.rs  # socat → warning (not blocking error)
platforms/posix/src/sandbox.rs   # runtime_config_from_policy conservative net;
                                 #   prepare: FS-existence split → ro_bind_in_place + scrub lists;
                                 #   thread both into the wrap call
scripts/verify-bwrap.sh          # the reusable Docker runtime-verification harness (NEW)
```

The split lists (`ro_bind_in_place`, `scrub_paths`) are computed in the posix `prepare` layer (FS access) and passed to a widened pure `wrap_linux_bwrap(command, policy, ro_bind_in_place, scrub_paths)` (or carried on `SandboxRuntimeConfig` as additive fields — decided at plan time; both keep `wrap` pure). `wrap` stays a deterministic string builder (unit-testable on macOS via exact-argv assertions).

## Scope cut (deferred, documented)

The **socat domain-filtering companion** (real per-domain egress for an `allowed_domains` policy) is NOT built. The conservative mapping routes any non-full-allow network policy to `--unshare-net` (no external net) — erring SAFE (a domain-allowlist policy gets no-net rather than full-net). This is the documented interim until the socat proxy is built + Linux-iterated. Also deferred (unchanged from Batch-14): per-policy process limits (bwrap can't enforce), WSL1/unknown-POSIX (already refused upstream).

## Safety invariants

1. No network policy ever yields MORE access than requested: `LoopbackOnly`/`Disabled` → `--unshare-net` (no external egress); only `Allowed` → `--share-net`. (Closes the finding-2 full-egress leak.)
2. A missing socat NEVER disables the sandbox (finding 1) — it's a warning; the conservative path doesn't use socat.
3. Denied/bare-repo paths that EXIST are read-only-in-place (host file never blanked); non-existent planted bare-repo files are scrubbed post-command before unsandboxed git runs (findings 3+4 — the git-escape defense).
4. The sandbox actually FUNCTIONS unprivileged (finding 5 userns) instead of silently failing to create namespaces.
5. Frozen `traits/` + `protocol/` untouched; engine-mobile pulls no sandbox change; the scrub adds no new tool→host seam (host-side shell append).

## Verification — the Docker gate (load-bearing)

`scripts/verify-bwrap.sh` runs the wrapped-command shapes the Rust builder produces inside `docker run --rm --privileged arm64v8/debian:stable-slim` (installs bubblewrap), asserting per finding:
- **net**: `LoopbackOnly`/`Disabled` shape → external host unreachable, loopback reachable; `Allowed` shape → external reachable.
- **userns**: the `--unshare-user-try` shape starts; under `user.max_user_namespaces=0` it still runs (degrade).
- **deny-write**: an existing file under `ro_bind_in_place` is read-only inside + unchanged on host; an allow_write parent does NOT make a ro-bound child writable (ordering).
- **scrub**: a planted bare-repo file (created by the inner command in the host cwd) is gone after the wrapped string returns; bwrap's non-zero exit code is preserved through the scrub suffix; a pre-existing file is NOT scrubbed.

Each implementation task runs the relevant assertion in the container as its runtime gate, IN ADDITION to the deterministic macOS arg-vector unit tests. The harness is also the artifact a future Linux-CI run re-executes.

## Testing strategy

- **macOS unit tests** (deterministic, no Docker): `wrap_linux_bwrap` exact-argv assertions for each net policy, userns flag presence, ro-bind-in-place ordering, scrub-suffix shape (incl. empty-list = no suffix, exit-preservation literal, single-quote escaping of scrub paths); `runtime_config_from_policy` net-mapping matrix; `dependency_check` socat→warning.
- **Docker runtime assertions** (`scripts/verify-bwrap.sh`): the per-finding behavioral proofs above.
- **Gates**: `cargo test -p sandbox -p platform-posix` + `clippy -D warnings` on touched crates + `cargo test --workspace --no-run` struct-trap + both engines build + engine-mobile pulls no sandbox change + `git diff main -- traits protocol` empty. Plus the Docker harness green.

## Out of scope (documented follow-ups)

- socat domain-filtering companion (real per-domain egress).
- True no-loopback for `Disabled` (bwrap's fresh netns always has loopback).
- per-policy process/resource limits (bwrap limitation).
- The setuid-bwrap + userns-disabled finding-5 edge (verified degrade direction; full setuid isolation is a harness refinement, not code).
