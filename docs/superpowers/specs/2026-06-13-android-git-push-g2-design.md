# Android Git Tool — Push (G2) Design

Date: 2026-06-13
Status: Approved design (brainstormed + decisions locked)
Part of: the Android Git tool effort (spec `2026-06-13-android-git-design.md`,
§G2 = "operation set"). P4 shipped v1 = **read + local-write, no push**; this
spec lifts the **no-push** deferral (G2) by adding a single `Push` operation to
the existing structured `git-mobile` tool.

## Summary

P4 built a structured, libgit2-in-process Git tool: the model selects an
operation enum (never argv), and networked ops (clone/fetch/pull) run through an
in-process HTTPS-token credential callback + Android CA wiring (`auth.rs`). The
**only** deferred capability that v1 explicitly named was **push** (G2). This
spec adds `Push` as a new network operation on that same tool — reusing the
existing token callback, CA wiring, SSH-URL guard, network-gating, and the
already-present `GitOpError::NonFastForward`. No new crate, no new credential
surface, no new registration gate.

Push is the first **remote-write** capability. It stays inside the codebase's
fast-forward-only safety philosophy (G8): a non-fast-forward push is **rejected
with a named error**, never force-applied.

## Decision log (this G2 brainstorm, 2026-06-13)

| # | Decision | Choice |
|---|----------|--------|
| GP1 | Force-push | **No force-push.** A non-ff push is rejected with `GitOpError::NonFastForward` ("remote has commits you don't have; pull/rebase first"). No `force` flag, no `+` refspec prefix. Consistent with G8 (ff-only merge/pull). Force can be added later if a real need appears. |
| GP2 | Target + upstream | **Current branch → `origin`, set upstream if unset.** Push HEAD's current branch (or an explicit `branch`) to the named `remote` (default `origin`), refspec `refs/heads/<b>:refs/heads/<b>`. On first push of a branch with **no** upstream, write `branch.<b>.remote`/`.merge` (the `git push -u` convenience) and report `set_upstream: true`. **Detached HEAD with no explicit `branch` → named error.** |
| GP3 | Tags | **Branches only.** No tag refs (`refs/tags/*`) in v1 — deferred (YAGNI). Keeps refspec validation and result reporting minimal. |
| GP4 | Delete remote refs | **Never.** No delete refspecs (`:refs/heads/x`). v1 only advances a branch ref. |
| GP5 | Auth / gating | **Reuse the P4 network gate.** Push requires the host HTTPS token (`has_token`); missing token → the existing "git credentials not configured" error. Token stays in-process only (never disk/env/argv/log), under the D11 Keystore gate. No new gate conjunct. |
| GP6 | SSH | Reuse `reject_ssh_url` — `git@…`/`ssh://…` remotes return the existing SSH-unsupported error (G7 unchanged). |

## Goals

1. Add a `Push` operation that pushes the current (or explicitly named) local
   branch to a remote (default `origin`) over HTTPS with the in-process token.
2. Stay fast-forward-only: reject non-ff pushes with a named error; never force,
   never delete remote refs.
3. Set the branch upstream on first push when none is configured (`-u`).
4. Reuse the existing auth/CA/SSH-guard/network-gating machinery unchanged — no
   new credential surface, no new registration gate, no new crate.

## Non-goals

1. No force-push / `--force-with-lease` (GP1). No tag pushing (GP3). No remote
   ref deletion (GP4).
2. No SSH (G7 unchanged). No new credential storage or credential-helper (P4
   §G3 unchanged — token is in-memory, in-process only).
3. No push of multiple branches / arbitrary refspecs in one call — one branch
   per `Push` invocation.
4. No interactive conflict handling — a diverged remote is a `NonFastForward`
   error the model resolves via `pull`/rebase, exactly as for merge/pull (G8).

## Architecture

```text
lingxi-code/tools/git-mobile/src/
├── lib.rs    MODIFY: add `Push` to the op enum + JSON schema; route it through
│             dispatch_network (so it inherits the has_token network gate);
│             prompt now declares push as supported (was "no push").
├── ops.rs    MODIFY: + GitPushResult; + pub fn push(net, repo, remote, branch).
│             Reuses existing GitOpError::NonFastForward + reject_ssh_url.
└── auth.rs   MODIFY: + make_push_options(token) -> git2::PushOptions, sharing
              the SAME credentials closure as make_fetch_options (factor the
              token→Cred convention into one place).
```

- **Reuses the P4 seam end-to-end.** Push is a network op, so it flows through
  the existing `dispatch_network` path in `lib.rs` (the one already gated on the
  host token via `GitNetConfig { token, ca_dir }`). No change to the gate, the
  `AndroidGitToolCtx`/`AndroidGitSecret` carriers, or registration.
- **libgit2 push.** `git2::Remote::push(&[refspec], Some(&mut push_opts))` with
  `PushOptions` carrying `RemoteCallbacks::credentials` (same token closure) and
  a `push_update_reference` callback to capture per-ref rejection status.
- **`deny(unsafe_code)` preserved.** Push adds no `unsafe`; the lone audited
  carve-out (`set_ssl_cert_dir` in `auth.rs`) is reused via `set_ca_location`.

### Operation → git2 mapping (the new op)

| Op | Inputs | git2 path | Result |
|----|--------|-----------|--------|
| `Push` | `remote?`(default `origin`), `branch?`(default current HEAD branch) | `set_ca_location` → `find_remote` → `reject_ssh_url(remote.url())` → `remote.push([refs/heads/<b>:refs/heads/<b>], PushOptions{credentials, push_update_reference})` → (if no upstream) `repo.branch(...).set_upstream` / write `branch.<b>.remote`+`.merge` | `GitPushResult { remote, branch, pushed_oid, set_upstream }` |

### `push` data flow

1. Require `has_token` (handled by the network dispatch path) — else existing
   "git credentials not configured" error.
2. Resolve `remote` (explicit, else `origin`); `find_remote`; `reject_ssh_url`
   on its configured URL.
3. Resolve `branch`: explicit arg, else the short name of the current HEAD
   branch. **Detached HEAD and no `branch` → `GitOpError::InvalidInput`** ("no
   current branch to push; HEAD is detached").
4. `set_ca_location(net.ca_dir)`; build `make_push_options(net.token)`.
5. Refspec = `refs/heads/<branch>:refs/heads/<branch>` (no `+`, no delete).
6. `remote.push(&[refspec], opts)`. The `push_update_reference` callback records
   any per-ref status string; a non-empty status = the remote rejected the ref.
   Map that to `GitOpError::NonFastForward` (the typical cause — remote ahead),
   carrying the remote's message. (libgit2 may also surface this directly as a
   non-ff `git2::Error`; both routes map to `NonFastForward`.)
7. **Set upstream if unset:** if the local branch has no configured upstream,
   set `branch.<branch>.remote = <remote>` and `branch.<branch>.merge =
   refs/heads/<branch>`; report `set_upstream: true` (else `false`).
8. Return `GitPushResult { remote, branch, pushed_oid: <local branch tip OID>,
   set_upstream }`.

## Error handling (all named, fail-closed)

- Missing token → existing "git credentials not configured" (GP5).
- SSH remote URL → existing SSH-unsupported `InvalidInput` (GP6).
- Detached HEAD, no explicit branch → `InvalidInput` ("HEAD is detached").
- Unknown remote / unborn branch / network / TLS / auth failure → mapped
  `GitOpError::Libgit2` (via `from_git2`).
- Non-fast-forward rejection → `GitOpError::NonFastForward` (reused; "remote has
  commits you don't have — pull/rebase first"). No force, no ref deletion.

## Security

Unchanged from P4 §G3/§G6. The token is supplied in-memory by the Kotlin host,
installed into the libgit2 credentials callback only for the push call's
duration, and never written to disk/env/argv or logged. Push adds remote-write
capability but **no new credential surface** and **no new gate** — it is gated
by the same `has_token` network requirement and the D11 Keystore gate as
clone/fetch/pull. The crate stays `#![deny(unsafe_code)]` with the single
pre-existing audited `set_ssl_cert_dir` carve-out.

## Testing

Host tests run against `file://` bare remotes (libgit2 is in-process — the same
approach the P4 clone/fetch/pull host tests use). Each test creates a bare
"remote" repo + a working clone, then:

- **push advances the remote ref:** commit locally, `push`, assert the bare
  repo's `refs/heads/<b>` now points at the new local tip; `set_upstream==true`
  on first push.
- **default targeting:** `push` with no `branch`/`remote` pushes the current
  branch to `origin`.
- **idempotent re-push:** pushing with nothing new → success, no ref change,
  `set_upstream==false` (already set).
- **non-ff rejection:** advance the bare remote past the local branch, then
  `push` → `GitOpError::NonFastForward`; the remote ref is unchanged.
- **detached HEAD:** detach HEAD, `push` with no branch → `InvalidInput`.
- **guards:** SSH remote URL → SSH error; missing token (network op with
  `has_token==false` at the tool layer) → credentials error.
- **`lib.rs`:** the op enum/schema accepts `Push`; `dispatch_network` routes it;
  the prompt now declares push supported (update the existing
  `prompt_declares_structured_git_no_push` test to reflect push availability).

Device HTTPS-push acceptance (push to a real remote with a host token) is
**PENDING-DEVICE**, gated behind the same probe path as P4's pending
HTTPS-clone acceptance — host file:// tests + the build gate are the merge gate.

## Phasing (single plan)

- **GPa — ops + auth (host TDD):** `auth::make_push_options`; `ops::push` +
  `GitPushResult`; all `ops::` push tests against file:// bare remotes (advance,
  default, non-ff, detached, set-upstream, idempotent).
- **GPb — tool wiring (host TDD):** `lib.rs` op enum + JSON schema + dispatch
  through `dispatch_network`; prompt update; tool-layer dispatch test
  (`call_dispatches_push_*`) + the prompt test update.
- **GPc — gate + device runbook:** `cargo test --workspace`, `cargo clippy
  --workspace --all-targets -D warnings` + android-target clippy for
  `tool-git-mobile`, both-ABI `cargo ndk build`; document the PENDING-DEVICE
  HTTPS-push acceptance runbook (extend the P4 `android_git_probe` path).

## Risks

| Risk | Mitigation |
|------|------------|
| Non-ff detection: libgit2 may report rejection via the `push_update_reference` callback rather than as a top-level error | Install the callback and treat any non-empty status string as a rejection → `NonFastForward`; cover with the non-ff host test (advanced bare remote). |
| Upstream-set writes repo config | Only `branch.<b>.remote`/`.merge` in the repo-local config (never global/env — same discipline as P4's commit-identity rule); covered by the set-upstream test. |
| `PushOptions` vs `FetchOptions` lifetime/callback duplication | Factor the shared token→`Cred` closure so the convention lives in one place; `make_push_options` mirrors `make_fetch_options`. |
| Device-only HTTPS-push | Host file:// tests prove the push/ref/upstream/non-ff logic; real-token HTTPS push is PENDING-DEVICE (same posture as P4 clone). |
