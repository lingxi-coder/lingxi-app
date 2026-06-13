# Android Git Push (G2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `Push` operation to the existing structured `git-mobile` tool — push the current (or named) local branch to a remote (default `origin`) over HTTPS with the in-process token, fast-forward-only, setting upstream on first push.

**Architecture:** Push is a new network operation on the P4 libgit2-in-process Git tool. It reuses the existing token credential callback, CA wiring, SSH-URL guard, the `has_token` network gate, and the already-present `GitOpError::NonFastForward`. libgit2 reports non-ff per-ref via a `push_update_reference` callback (NOT a top-level error), so `push` builds a `RemoteCallbacks` carrying BOTH the shared token credentials closure and a rejection-capturing callback. No new crate, credential surface, or registration gate.

**Tech Stack:** Rust workspace at `lingxi-code/` (run cargo from there). `git2` crate (vendored, pinned 0.21) — `Remote::push`, `PushOptions`, `RemoteCallbacks::push_update_reference`. Host tests run against `file://` bare remotes (libgit2 is in-process). Crate `tools/git-mobile` (`tool-git-mobile`) is `#![deny(unsafe_code)]` with one pre-existing audited carve-out in `auth.rs`.

**Spec:** `docs/superpowers/specs/2026-06-13-android-git-push-g2-design.md` (decisions GP1-GP6).

**Predecessor:** P4 (Git tool) is on `main` (`13788128`); this branch (`android-git-push-g2`) is cut from `main` (`a3c6576a`, post-P5). The Git tool lives at `lingxi-code/tools/git-mobile/src/{lib.rs,ops.rs,auth.rs}`.

**Invariants this plan must not break:**
- `tool-git-mobile` stays `#![deny(unsafe_code)]` — push adds NO unsafe (the lone `set_ssl_cert_dir` carve-out in `auth.rs` is reused via `set_ca_location`).
- Token stays in-process only (never disk/env/argv/log). No new gate — push is gated by the existing `has_token` network requirement.
- Fast-forward-only (GP1): non-ff push → `GitOpError::NonFastForward`. No force, no `+` refspec, no ref deletion (GP4), no tags (GP3).

---

## File structure

```text
lingxi-code/tools/git-mobile/src/
├── auth.rs   MODIFY: extract `install_token_credentials(&mut RemoteCallbacks, Option<&str>)`
│             (shared token→Cred closure); refactor make_fetch_options to call it.
├── ops.rs    MODIFY: + GitPushResult struct; + pub fn push(net, repo, remote, branch).
│             Reuses GitOpError::{NonFastForward,InvalidInput,Libgit2}, reject_ssh_url, GitNetConfig.
└── lib.rs    MODIFY: + "push" in OPERATIONS; network-detect "push"; dispatch_network "push" arm;
              prompt declares push supported; update prompt test.
```

---

# Phase GPa — ops + auth (host TDD)

### Task 1: `auth.rs` — factor the shared token credentials closure

**Files:** Modify `lingxi-code/tools/git-mobile/src/auth.rs`.

**Why:** `push` needs a `RemoteCallbacks` carrying the SAME token→`Cred` convention as fetch PLUS a `push_update_reference` callback. Extracting the credentials installation into one helper keeps the token convention (sentinel username + token-as-password) in a single place (spec risk-mitigation) and lets `push` build its own callbacks.

- [ ] **Step 1: Write the failing test.** Add to `auth.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn install_token_credentials_is_noop_without_token() {
        // No token => no credentials callback installed; building options/callbacks
        // must not panic. (We cannot invoke libgit2's private dispatch in isolation.)
        let mut cb = git2::RemoteCallbacks::new();
        install_token_credentials(&mut cb, None);
        let mut cb2 = git2::RemoteCallbacks::new();
        install_token_credentials(&mut cb2, Some("tok-xyz"));
        // The token convention is still the userpass_plaintext sentinel:
        git2::Cred::userpass_plaintext(TOKEN_USERNAME, "tok-xyz")
            .expect("userpass_plaintext should build a Cred");
    }
```

- [ ] **Step 2: Run it — expect FAIL** (unresolved `install_token_credentials`).

Run: `cargo test -p tool-git-mobile auth::tests::install_token_credentials -- --nocapture` (from `lingxi-code/`)
Expected: compile error `cannot find function install_token_credentials`.

- [ ] **Step 3: Implement.** Add the helper and refactor `make_fetch_options` to use it. Replace the body of `make_fetch_options` and add the helper above it:

```rust
/// Install the HTTPS-token credentials callback on `callbacks`.
///
/// When `token` is `Some`, a `credentials` callback yields
/// `Cred::userpass_plaintext(TOKEN_USERNAME, token)` — the token is borrowed for
/// the callbacks' lifetime (`'a`), never copied into a longer-lived store. When
/// `None`, nothing is installed (public/anonymous HTTPS + `file://` still work).
/// The token is never logged or written anywhere; it only flows into libgit2's
/// in-process credential callback. Shared by `make_fetch_options` (clone/fetch/
/// pull) and `ops::push`.
pub fn install_token_credentials<'a>(
    callbacks: &mut git2::RemoteCallbacks<'a>,
    token: Option<&'a str>,
) {
    if let Some(token) = token {
        callbacks.credentials(move |_url, _username_from_url, _allowed| {
            git2::Cred::userpass_plaintext(TOKEN_USERNAME, token)
        });
    }
}

/// Build the [`git2::FetchOptions`] used by every network fetch op (clone /
/// fetch / pull), with the in-process token credentials callback installed.
#[must_use]
pub fn make_fetch_options(token: Option<&str>) -> git2::FetchOptions<'_> {
    let mut callbacks = git2::RemoteCallbacks::new();
    install_token_credentials(&mut callbacks, token);
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(callbacks);
    opts
}
```

- [ ] **Step 4: Run tests — expect PASS** (the new test + the existing `credentials_callback_yields_token` / `set_ca_location_*`).

Run: `cargo test -p tool-git-mobile auth::tests` (from `lingxi-code/`)
Expected: all auth tests pass.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/tools/git-mobile/src/auth.rs
git commit -m "refactor(tool-git-mobile): factor install_token_credentials shared by fetch+push (G2)"
```

### Task 2: `ops.rs` — `GitPushResult` + `push()`

**Files:** Modify `lingxi-code/tools/git-mobile/src/ops.rs`.

`GitNetConfig { token, ca_dir }`, `GitOpError::{NonFastForward, InvalidInput, Libgit2}`, `reject_ssh_url`, and the test helper `init_history(dir)` already exist — reuse them.

- [ ] **Step 1: Write the failing tests.** Add to `ops.rs`'s `#[cfg(test)] mod tests`. First add two helpers, then the tests:

```rust
    /// Init a bare repo at `dir` to act as a `file://` "remote".
    fn init_bare_remote(dir: &Path) -> git2::Repository {
        git2::Repository::init_bare(dir).unwrap()
    }

    /// Short name of the repo's current HEAD branch (e.g. "master").
    fn current_branch(repo: &git2::Repository) -> String {
        repo.head().unwrap().shorthand().unwrap().to_owned()
    }

    #[test]
    fn push_advances_remote_ref_and_sets_upstream() {
        let work = tempdir().unwrap();
        let (repo, _f, second) = init_history(work.path());
        let bare = tempdir().unwrap();
        let remote_repo = init_bare_remote(bare.path());
        repo.remote("origin", &format!("file://{}", bare.path().display()))
            .unwrap();

        let branch = current_branch(&repo);
        let net = GitNetConfig::default(); // file:// needs no token
        let res = push(&net, &repo, "origin", "").expect("push ok");

        assert_eq!(res.remote, "origin");
        assert_eq!(res.branch, branch);
        assert_eq!(res.pushed_oid, second.to_string());
        assert!(res.set_upstream, "first push sets upstream");
        // The bare remote's branch ref now points at the local tip.
        let remote_ref = remote_repo
            .find_reference(&format!("refs/heads/{branch}"))
            .unwrap();
        assert_eq!(remote_ref.target().unwrap(), second);
        // Upstream config written.
        let cfg = repo.config().unwrap();
        assert_eq!(cfg.get_string(&format!("branch.{branch}.remote")).unwrap(), "origin");
        assert_eq!(
            cfg.get_string(&format!("branch.{branch}.merge")).unwrap(),
            format!("refs/heads/{branch}")
        );
    }

    #[test]
    fn push_is_idempotent_when_nothing_new() {
        let work = tempdir().unwrap();
        let (repo, _f, _second) = init_history(work.path());
        let bare = tempdir().unwrap();
        init_bare_remote(bare.path());
        repo.remote("origin", &format!("file://{}", bare.path().display()))
            .unwrap();
        let net = GitNetConfig::default();
        push(&net, &repo, "origin", "").expect("first push");
        // Second push: nothing new; succeeds, upstream already set.
        let res = push(&net, &repo, "origin", "").expect("re-push ok");
        assert!(!res.set_upstream, "upstream already configured");
    }

    #[test]
    fn push_detached_head_without_branch_errors() {
        let work = tempdir().unwrap();
        let (repo, _f, second) = init_history(work.path());
        let bare = tempdir().unwrap();
        init_bare_remote(bare.path());
        repo.remote("origin", &format!("file://{}", bare.path().display()))
            .unwrap();
        // Detach HEAD at `second`.
        repo.set_head_detached(second).unwrap();
        let net = GitNetConfig::default();
        let err = push(&net, &repo, "origin", "").unwrap_err();
        assert!(matches!(err, GitOpError::InvalidInput(ref m) if m.contains("detached")));
    }

    #[test]
    fn push_non_fast_forward_is_rejected() {
        // A: local working repo pushed to bare; B: a second clone that advances
        // the bare; then A commits divergently and pushes -> non-ff.
        let work = tempdir().unwrap();
        let (repo_a, _f, _second) = init_history(work.path());
        let bare = tempdir().unwrap();
        init_bare_remote(bare.path());
        repo_a
            .remote("origin", &format!("file://{}", bare.path().display()))
            .unwrap();
        let net = GitNetConfig::default();
        let branch = current_branch(&repo_a);
        push(&net, &repo_a, "origin", "").expect("A initial push");

        // B clones the bare, commits, and pushes (fast-forward) to advance origin.
        let clone_b = tempdir().unwrap();
        let repo_b = git2::Repository::clone(
            &format!("file://{}", bare.path().display()),
            clone_b.path(),
        )
        .unwrap();
        {
            let sig = git2::Signature::now("B", "b@example.com").unwrap();
            std::fs::write(clone_b.path().join("c.txt"), "gamma\n").unwrap();
            let mut idx = repo_b.index().unwrap();
            idx.add_path(Path::new("c.txt")).unwrap();
            idx.write().unwrap();
            let tree = repo_b.find_tree(idx.write_tree().unwrap()).unwrap();
            let head = repo_b.head().unwrap().peel_to_commit().unwrap();
            repo_b
                .commit(Some("HEAD"), &sig, &sig, "B commit", &tree, &[&head])
                .unwrap();
        }
        push(&net, &repo_b, "origin", &branch).expect("B ff push");

        // A (now stale) commits divergently and pushes -> non-fast-forward.
        {
            let sig = git2::Signature::now("A", "a@example.com").unwrap();
            std::fs::write(work.path().join("d.txt"), "delta\n").unwrap();
            let mut idx = repo_a.index().unwrap();
            idx.add_path(Path::new("d.txt")).unwrap();
            idx.write().unwrap();
            let tree = repo_a.find_tree(idx.write_tree().unwrap()).unwrap();
            let head = repo_a.head().unwrap().peel_to_commit().unwrap();
            repo_a
                .commit(Some("HEAD"), &sig, &sig, "A commit", &tree, &[&head])
                .unwrap();
        }
        let err = push(&net, &repo_a, "origin", &branch).unwrap_err();
        assert!(
            matches!(err, GitOpError::NonFastForward(_)),
            "stale push must be non-fast-forward, got {err:?}"
        );
    }

    #[test]
    fn push_rejects_ssh_remote() {
        let work = tempdir().unwrap();
        let (repo, _f, _s) = init_history(work.path());
        repo.remote("origin", "git@github.com:owner/repo.git").unwrap();
        let net = GitNetConfig::default();
        let err = push(&net, &repo, "origin", "").unwrap_err();
        assert!(matches!(err, GitOpError::InvalidInput(ref m) if m.contains("ssh")));
    }
```

(The tests inline `format!("file://{}", bare.path().display())` for remote URLs; the only helpers needed are `init_bare_remote` and `current_branch`.)

- [ ] **Step 2: Run — expect FAIL** (no `push` / `GitPushResult`).

Run: `cargo test -p tool-git-mobile ops::tests::push -- --nocapture` (from `lingxi-code/`)
Expected: compile error `cannot find function push` / `GitPushResult`.

- [ ] **Step 3: Implement.** Add the result struct near the other `Git*Result` structs (after `GitPullResult`), and the `push` fn after `pull`:

```rust
/// Result of a [`push`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitPushResult {
    /// The remote pushed to.
    pub remote: String,
    /// The branch short name pushed.
    pub branch: String,
    /// OID the local branch tip points at (now also on the remote).
    pub pushed_oid: String,
    /// Whether this push set the branch's upstream (true only on first push of
    /// a branch that had none).
    pub set_upstream: bool,
}

/// Push the current (or explicit) local branch to `remote_name` (default
/// `origin`) over the in-process token, **fast-forward-only** (GP1). On first
/// push of a branch with no upstream, write `branch.<b>.remote`/`.merge`
/// (`git push -u`). No force, no ref deletion, no tags.
///
/// libgit2 reports a non-fast-forward as a per-ref status via the
/// `push_update_reference` callback (NOT a top-level error), so we install that
/// callback and map any non-empty status to [`GitOpError::NonFastForward`].
///
/// # Errors
///
/// - [`GitOpError::InvalidInput`] — SSH remote URL, or HEAD is detached and no
///   `branch` was supplied.
/// - [`GitOpError::NonFastForward`] — the remote rejected the ref (remote ahead).
/// - [`GitOpError::Libgit2`] — unknown remote / network / TLS / auth failure.
pub fn push(
    net: &GitNetConfig,
    repo: &git2::Repository,
    remote_name: &str,
    branch: &str,
) -> Result<GitPushResult, GitOpError> {
    let remote_name = if remote_name.is_empty() {
        "origin"
    } else {
        remote_name
    };

    // Resolve the branch short name: explicit param, else current HEAD branch.
    let branch_name = if branch.is_empty() {
        let head = repo.head().map_err(|e| GitOpError::from_git2(&e))?;
        if !head.is_branch() {
            return Err(GitOpError::InvalidInput(
                "push on a detached HEAD requires an explicit `branch`".into(),
            ));
        }
        head.shorthand()
            .ok_or_else(|| GitOpError::InvalidInput("HEAD has no branch name".into()))?
            .to_owned()
    } else {
        branch.to_owned()
    };

    let mut remote = repo
        .find_remote(remote_name)
        .map_err(|e| GitOpError::from_git2(&e))?;
    if let Ok(url) = remote.url() {
        reject_ssh_url(url)?;
    }

    crate::auth::set_ca_location(net.ca_dir.as_deref())?;

    // Capture a per-ref rejection reported via push_update_reference.
    let rejection: std::rc::Rc<std::cell::RefCell<Option<String>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let mut callbacks = git2::RemoteCallbacks::new();
    crate::auth::install_token_credentials(&mut callbacks, net.token.as_deref());
    {
        let rejection = std::rc::Rc::clone(&rejection);
        callbacks.push_update_reference(move |refname, status| {
            if let Some(msg) = status {
                *rejection.borrow_mut() = Some(format!("{refname}: {msg}"));
            }
            Ok(())
        });
    }
    let mut push_opts = git2::PushOptions::new();
    push_opts.remote_callbacks(callbacks);

    // No `+` prefix (no force, GP1); branch ref only (no tags GP3, no delete GP4).
    let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");
    remote
        .push(&[refspec.as_str()], Some(&mut push_opts))
        .map_err(|e| GitOpError::from_git2(&e))?;

    // A rejection reported via the callback is (for a plain branch push) a
    // non-fast-forward: the remote has commits we don't have.
    if let Some(msg) = rejection.borrow().clone() {
        return Err(GitOpError::NonFastForward(format!(
            "remote rejected {msg}; pull/rebase first"
        )));
    }

    // The local branch tip we just pushed.
    let pushed_oid = repo
        .refname_to_id(&format!("refs/heads/{branch_name}"))
        .map_err(|e| GitOpError::from_git2(&e))?
        .to_string();

    // Set upstream on first push (branch had no `branch.<b>.remote`). Write the
    // config directly (Branch::set_upstream needs a remote-tracking ref that
    // does not exist right after a push).
    let mut config = repo.config().map_err(|e| GitOpError::from_git2(&e))?;
    let remote_key = format!("branch.{branch_name}.remote");
    let set_upstream = if config.get_string(&remote_key).is_err() {
        config
            .set_str(&remote_key, remote_name)
            .map_err(|e| GitOpError::from_git2(&e))?;
        config
            .set_str(
                &format!("branch.{branch_name}.merge"),
                &format!("refs/heads/{branch_name}"),
            )
            .map_err(|e| GitOpError::from_git2(&e))?;
        true
    } else {
        false
    };

    Ok(GitPushResult {
        remote: remote_name.to_owned(),
        branch: branch_name,
        pushed_oid,
        set_upstream,
    })
}
```

- [ ] **Step 4: Run — expect PASS.**

Run: `cargo test -p tool-git-mobile ops::tests::push` (from `lingxi-code/`)
Expected: all 5 push tests pass. Then `cargo test -p tool-git-mobile` (whole crate) green.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/tools/git-mobile/src/ops.rs
git commit -m "feat(tool-git-mobile): ops::push — ff-only, set-upstream, non-ff rejection (G2)"
```

---

# Phase GPb — tool wiring (host TDD)

### Task 3: `lib.rs` — wire `push` into the tool (enum, schema, dispatch, prompt)

**Files:** Modify `lingxi-code/tools/git-mobile/src/lib.rs`.

- [ ] **Step 1: Write the failing tests.** Add a dispatch test (mirror the existing `call_dispatches_clone_from_file_remote`) and a prompt test update. Add to `lib.rs`'s test module:

```rust
    #[tokio::test]
    async fn call_dispatches_push_to_file_remote() {
        // Build a working repo with a commit + an `origin` pointing at a bare repo,
        // then drive GitTool::call with operation=push and assert the bare advanced.
        let work = tempfile::tempdir().unwrap();
        let bare = tempfile::tempdir().unwrap();
        let remote_repo = git2::Repository::init_bare(bare.path()).unwrap();
        let repo = git2::Repository::init(work.path()).unwrap();
        let sig = git2::Signature::now("T", "t@example.com").unwrap();
        std::fs::write(work.path().join("a.txt"), "x\n").unwrap();
        let oid = {
            let mut idx = repo.index().unwrap();
            idx.add_path(std::path::Path::new("a.txt")).unwrap();
            idx.write().unwrap();
            let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "c1", &tree, &[]).unwrap()
        };
        let branch = repo.head().unwrap().shorthand().unwrap().to_owned();
        repo.remote("origin", &format!("file://{}", bare.path().display()))
            .unwrap();

        // ctx anchored at the working repo root, token present (file:// ignores it).
        let tool = GitTool::new(test_ctx_git_enabled(work.path().to_str().unwrap()));
        let input = json!({ "operation": "push", "repo": ".", "remote": "origin" });
        let res = tool
            .call(input, dummy_tool_use_ctx(), dummy_progress())
            .await
            .expect("push call ok");
        // The op JSON reports the pushed branch/oid; the bare remote advanced.
        let data = res.data;
        assert_eq!(data["branch"], branch);
        assert_eq!(data["pushed_oid"], oid.to_string());
        assert_eq!(
            remote_repo
                .find_reference(&format!("refs/heads/{branch}"))
                .unwrap()
                .target()
                .unwrap(),
            oid
        );
    }
```

(Use the SAME test scaffolding the existing `call_dispatches_clone_from_file_remote` test uses — `test_ctx_git_enabled(...)`, and whatever helpers it uses to build the `ToolUseContext` + progress sender. Copy those exact helper calls; do not invent new ones. If `test_ctx_git_enabled` does not take a workspace-root arg, set the ctx's `workspace_root` the same way that test does.)

Also UPDATE the existing prompt test `prompt_declares_structured_git_no_push` to assert push is now supported. Rename it and change its assertions:

```rust
    #[tokio::test]
    async fn prompt_declares_structured_git_with_push() {
        let tool = GitTool::new(test_ctx_git_enabled_with_token());
        let prompt = tool.prompt(&PromptOptions::default()).await;
        assert!(prompt.contains("push"), "push now listed: {prompt}");
        assert!(!prompt.to_lowercase().contains("no push"), "must not say 'no push': {prompt}");
        assert!(prompt.contains("FAST-FORWARD-ONLY"), "ff-only still stated: {prompt}");
    }
```

(Match the helper the original test used to build a token-bearing ctx — reuse its exact name.)

- [ ] **Step 2: Run — expect FAIL** (push not in OPERATIONS / not dispatched; old "no push" prompt).

Run: `cargo test -p tool-git-mobile call_dispatches_push_to_file_remote prompt_declares_structured_git_with_push` (from `lingxi-code/`)
Expected: dispatch returns an "unknown" error (push not wired) and the prompt test fails on "no push".

- [ ] **Step 3: Implement.** Four edits in `lib.rs`:

1. Add `"push"` to the `OPERATIONS` const array (after `"merge"`):
```rust
    "merge",
    "push",
];
```

2. In `INPUT_SCHEMA`, broaden the `branch` description to mention push (no new field needed — `remote`/`branch` already exist):
```rust
            "branch":      { "type": "string", "description": "Branch name (checkout/branch_create/merge target; push source — defaults to the current branch)." },
```

3. In `call`, add `"push"` to the network-op match so it routes through `dispatch_network` (token gate):
```rust
        let dispatch_result = if matches!(operation, "clone" | "fetch" | "pull" | "push") {
```

4. In `dispatch_network`, add the `"push"` arm (open the existing repo like fetch/pull):
```rust
        "push" => {
            let repo = ops::open_repo(workspace_root, repo_rel)?;
            let remote = str_param("remote").unwrap_or("origin");
            let branch = str_param("branch").unwrap_or("");
            Ok(serde_json::to_value(ops::push(net, &repo, remote, branch)?).unwrap_or(Value::Null))
        }
```

5. In `prompt`, change the v1-boundary sentence and the network-op lists to include push. Replace the "v1 is READ + LOCAL-WRITE only: there is NO push." paragraph:
```rust
        prompt.push_str(
            "v1 is READ + LOCAL-WRITE + PUSH. merge and pull are FAST-FORWARD-ONLY \
             and push is fast-forward-only too (a non-fast-forward is reported as \
             a named error — pull/rebase first — never forced). Remotes are \
             HTTPS-ONLY (git@/ssh URLs are rejected).\n\n",
        );
```
and update the two network-op lists from `(clone/fetch/pull)` to `(clone/fetch/pull/push)` in BOTH the `has_token` and the no-token branches.

- [ ] **Step 4: Run — expect PASS.**

Run: `cargo test -p tool-git-mobile` (from `lingxi-code/`)
Expected: the new dispatch + prompt tests pass; all pre-existing git-mobile tests still pass.

- [ ] **Step 5: Commit.**

```bash
git add lingxi-code/tools/git-mobile/src/lib.rs
git commit -m "feat(tool-git-mobile): wire push op — enum/schema/dispatch + prompt declares push (G2)"
```

---

# Phase GPc — gate + device runbook

### Task 4: G2 gate + PENDING-DEVICE runbook

**Files:** Modify `docs/superpowers/plans/2026-06-13-android-git-push-g2.md` (append the runbook note to this file's Device section — Step 4 below). No production-code changes here beyond what Tasks 1-3 landed.

- [ ] **Step 1: Workspace test.**

Run: `cargo test --workspace 2>&1 | tail -40` (from `lingxi-code/`)
Expected: green. KNOWN non-regressions (do NOT treat as failures): `tool-shell` `cwd_persistence` / `powershell` parallel-isolation flake; `platform-posix` `mcp_stdio` needs `cargo build -p mock_stdio_mcp` then a re-run. Triage any failure: a real failure in `tool-git-mobile` is BLOCKED; a known flake is not.

- [ ] **Step 2: Clippy (workspace + the G2 crate on the android target).**

Run:
```bash
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30
export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
cargo ndk -t arm64-v8a clippy -p tool-git-mobile -- -D warnings 2>&1 | tail -30
```
Expected: both clean (the only known pre-existing warning is `third_party/git2-rs` `unused manifest key: package.autolib` — unrelated). Fix any NEW warning in `tool-git-mobile`.

- [ ] **Step 3: Both-ABI cross-build of the consuming AAR.**

Run:
```bash
cargo ndk -t arm64-v8a build -p android-aar 2>&1 | tail -8
cargo ndk -t x86_64 build -p android-aar 2>&1 | tail -8
```
Expected: both link cleanly (push adds no new native deps — git2/libgit2 already linked by P4).

- [ ] **Step 4: Record the PENDING-DEVICE HTTPS-push runbook.** Push to a real remote needs a device + a host HTTPS token, so on-device acceptance is PENDING-DEVICE (same posture as P4's HTTPS-clone). Document the runbook so a future device run is turn-key. Add this block to the spec's Testing section is NOT needed; instead note it here in the plan by leaving this checkbox's text as the durable record:

  Runbook (when a device + test token are available): extend the existing P4 `android_git_probe` UniFFI path (in `apps/android-aar/src/lib.rs`) with a push case — clone a scratch repo to the device, make a commit, call the `push` op through the real `GitTool`, and assert the remote ref advanced; gate it behind a test-only token env the same way P4's clone probe is. This is additive and NOT required for the G2 host-merge gate.

- [ ] **Step 5: Commit (gate marker — only if Steps 1-3 produced fixes; else nothing to commit).**

```bash
git add -A
git commit -m "chore(tool-git-mobile): G2 push gate — workspace + android-target clean"
```

---

## Self-review / spec coverage

- GP1 no force / non-ff named error: Task 2 (`push` installs `push_update_reference`, maps rejection → `NonFastForward`; no `+` refspec) + test `push_non_fast_forward_is_rejected`. ✓
- GP2 current-branch→origin + set-upstream-if-unset + detached error: Task 2 (`push` branch resolution, config writes, detached `InvalidInput`) + tests `push_advances_remote_ref_and_sets_upstream`, `push_is_idempotent_when_nothing_new`, `push_detached_head_without_branch_errors`. ✓
- GP3 branches only: refspec is `refs/heads/...` only; no tag handling. ✓
- GP4 no ref deletion: refspec never empty-LHS. ✓
- GP5 token gate / in-process: Task 3 routes push through `dispatch_network` (the `has_token` gated path) reusing `git_net_config`; no new gate. ✓
- GP6 SSH rejected: Task 2 reuses `reject_ssh_url` + test `push_rejects_ssh_remote`. ✓
- Auth DRY (spec risk): Task 1 `install_token_credentials`. ✓
- `deny(unsafe_code)` preserved: no new unsafe in any task. ✓
- Testing via file:// bare remotes: Tasks 2-3. ✓ Device acceptance PENDING-DEVICE: Task 4 Step 4. ✓

---

## Device acceptance (PENDING-DEVICE)

The G2 host-merge gate (workspace tests + workspace clippy + android-target clippy on `tool-git-mobile` + both-ABI AAR cross-build) is GREEN. The items below require a physical Android device plus a test-only HTTPS token and are **NOT** required for the host-merge gate — they are recorded here so a future device run is turn-key.

### (a) HTTPS-push device runbook

When a device + a test HTTPS token are available, extend the existing P4 `android_git_probe` UniFFI path in `apps/android-aar/src/lib.rs` with a **push** case (additive to the existing clone probe):

1. Clone a scratch repo to the device (reuse the P4 clone probe path).
2. Make a local commit on the checked-out branch.
3. Call the `push` op through the real `GitTool` (same dispatch the production tool uses — `dispatch_network` / `has_token` gated path).
4. Assert the **remote ref advanced** to the new commit (re-fetch / ls-remote the ref and compare OIDs).
5. Gate the whole probe behind a **test-only token env var**, exactly as P4's HTTPS-clone probe is gated (no token → probe skips, never hard-fails CI).

This mirrors P4's clone-probe posture: additive, env-gated, and excluded from the host gate.

### (b) RESIDUAL RISK — HTTP smart-protocol non-fast-forward route is UNTESTED on host (flagged by code review)

The host tests cover non-fast-forward **rejection** only via the `file://` local transport, where libgit2 surfaces the rejection as a **top-level error** (test `push_non_fast_forward_is_rejected`). That route is verified.

The **HTTP smart-protocol** non-ff route is different: over a real HTTPS remote, libgit2 reports a rejected ref update via the **`push_update_reference` callback** (per-ref status string) rather than as a top-level error. The push code **is wired** for this — the `push_update_reference` callback is installed and maps a non-empty rejection status to `GitError::NonFastForward` — but there is **NO host test** exercising it, because the `file://` transport doesn't drive that callback. It is therefore exercised only against a real HTTPS remote and remains **UNTESTED until the device run**.

**Action for the device run:** the (a) runbook above MUST include an explicit **non-ff HTTPS push** case — push a branch, advance the remote out-of-band so the local push is no longer fast-forward, attempt the push, and assert the operation fails with the `NonFastForward` error mapped from the `push_update_reference` callback. This is the only path that validates the callback-based rejection route end-to-end.
