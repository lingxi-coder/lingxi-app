# Android Git Tool (P4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A model-facing structured `Git` tool for Android backed by libgit2 (the `git2` crate) running **in-process** — read + local-write git (no push), HTTPS-token auth supplied in-memory by the Kotlin host, registered behind an independent gate, with all third-party libs vendored into `third_party/`.

**Architecture:** Spec `docs/superpowers/specs/2026-06-13-android-git-design.md` (G1–G8). libgit2 links into the engine `.so` as a *library* (no exec, no W^X, no minijail). The `Git` tool is a new `tools/git-mobile` crate implementing the `Tool` trait; operations map to deterministic `git2` calls. Gate + token thread from `android-aar` → `MobileConfig` → `BuiltinToolContext.android_git` → `tool_git_mobile::register_all`, mirroring the P3 `AndroidShellToolCtx` pattern exactly.

**Tech Stack:** Rust workspace at `lingxi-code/` (run cargo from there). `git2` + `libgit2-sys` (vendored into `third_party/git2-rs`), vendored `third_party/libgit2` (C), Android NDK 27.0.12077973 + `cargo-ndk` (`export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973`), `tool-api` Tool trait, `permission`, the engine `AdapterPermissionGate`.

**Predecessor:** P0a+P1+P2+P3 merged to main (`006a1b17`). This branch (`android-git-p4`) is cut from main. The Shell tool + `AndroidShellToolCtx` carrier + `android-libcap`/`android-minijail` vendoring precedents all exist and are the templates.

**Spec invariants P4 must not break:**
- `tools/git-mobile` stays `#![forbid(unsafe_code)]` (`git2` is a safe wrapper; the only C is libgit2-sys at build time).
- Git is **decoupled from minijail** — no sandbox capability in its gate, no exec.
- Token never reaches disk or a child-process env (in-process to the libgit2 credential callback only).
- Tool **absent, not erroring** when the gate fails; missing token disables only network ops.
- merge/pull **fast-forward only**; non-ff → named error, never leftover conflict markers.

---

## ⚠️ P4a entry decision — TLS backend (resolve FIRST, with evidence)

The spec (G6) chose **mbedtls + system cacerts**. But the proven cargo-ndk path for `git2`/`libgit2-sys` is `features = ["vendored-libgit2", "vendored-openssl"]` (libgit2-sys builds its bundled libgit2 + openssl via the `cmake`/`cc` crates, which cargo-ndk already wires a toolchain for). mbedtls means building libgit2 **ourselves** (`-DUSE_HTTPS=mbedTLS` + vendored mbedtls in a `platform-android-libgit2` build crate, then `LIBGIT2_NO_VENDOR=1`). Two paths:

- **Path V (vendored-openssl, lower build risk):** `git2` with `vendored-libgit2` + `vendored-openssl`, vendored into `third_party/git2-rs`; openssl provides its own CA bundle handling but we still point it at system cacerts via `git_libgit2_opts`. Size ~+3–4 MB/ABI.
- **Path M (mbedtls, spec G6, lower size):** self-built libgit2 with mbedtls; ~+2 MB/ABI; more build wiring + a `platform-android-libgit2` crate mirroring `android-libcap`.

**Task 1 resolves this**: attempt Path M (spec) ≤3 build-fix iterations; if mbedtls-under-NDK proves too costly, fall back to Path V and **amend spec G6** in the same commit (note the deviation + size delta). Either way TLS verification (real HTTPS clone) is the P4d device gate. The rest of the plan is TLS-backend-agnostic.

---

## File structure

```text
third_party/
├── minijail/        MODIFY (Task 2): symlink → properly vendored + committed subset
├── libgit2/         CREATE (Task 1, Path M only): vendored libgit2 C sources
└── git2-rs/         CREATE (Task 1): vendored git2 + libgit2-sys crate sources

lingxi-code/
├── platforms/android-libgit2/   CREATE (Task 1, Path M only): build crate (cc/cmake mbedtls+libgit2)
├── tools/git-mobile/            CREATE: `tool-git-mobile`
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs    GitTool (Tool impl) + register_all (gated) + schema
│       ├── ops.rs    operation enum → git2 calls (host-testable against temp repos)
│       └── auth.rs   credential callback + CA wiring
├── tool-api/src/builtin_context.rs   MODIFY: + AndroidGitToolCtx + android_git field
├── apps/engine-mobile/{Cargo.toml,lib.rs,host.rs}  MODIFY: dep + register + MobileConfig.android_git
├── apps/android-aar/src/lib.rs       MODIFY: AndroidGitConfig + gate + token → MobileConfig
└── lingxi-code/Cargo.toml            MODIFY: workspace members
```

---

# Phase P4a — vendoring + NDK build proof + minijail re-vendor

> Iterative build work; structured as proof-gates (not rigid TDD). Cross-compile success is the gate — the host can't exercise libgit2's TLS, but it CAN link+test the local ops (P4b).

### Task 1: vendor git2-rs (+ libgit2) and prove the NDK cross-compile

**Files:** Create `third_party/git2-rs/`, (Path M) `third_party/libgit2/` + `lingxi-code/platforms/android-libgit2/`; Modify `lingxi-code/Cargo.toml`.

- [ ] **Step 1: Preflight.** `export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973`; confirm `cargo ndk --version` and the arm64/x86_64 rust targets are installed (P0a established these). Confirm `cmake` is on PATH (`cmake --version`) — libgit2-sys needs it.

- [ ] **Step 2: Vendor the Rust crates.** Fetch pinned `git2` + `libgit2-sys` sources (the versions cargo resolves on crates.io today; pin exact). Place under `third_party/git2-rs/{git2,libgit2-sys}` as REAL dirs (remove any `.git`). These will be path-deps. (libgit2-sys bundles libgit2 C source for the `vendored-libgit2` feature — Path V uses that; Path M ignores it.)

- [ ] **Step 3 (Path M only): vendor libgit2 + create the build crate.** Clone libgit2 (pinned release, e.g. v1.8.x) into `third_party/libgit2` (remove `.git`); vendor mbedtls similarly. Create `platforms/android-libgit2` mirroring `platforms/android-libcap` (Cargo.toml `links = "git2"`, `build.rs` cc/cmake builds libgit2 with `-DUSE_HTTPS=mbedTLS -DBUILD_SHARED_LIBS=OFF` under the NDK, emits `cargo:rustc-link-lib=static=git2` + mbedtls + zlib + search path; host build = no-op). Read `platforms/android-libcap/build.rs` for the exact ancestors()/assert/cc pattern.

- [ ] **Step 4: Wire the crate(s).** Add `"platforms/android-libgit2"` (Path M) to workspace `members`. Decide the `tool-git-mobile` git2 dep form (Task 3 uses it): path-dep into `third_party/git2-rs/git2` with the chosen features (`vendored-libgit2`+`vendored-openssl` for Path V; bare + `LIBGIT2_NO_VENDOR` env for Path M).

- [ ] **Step 5: THE build proof.** A tiny throwaway lib that depends on git2 and calls `git2::Version::get()`:
```bash
cargo ndk -t arm64-v8a build -p tool-git-mobile 2>&1 | tail -30   # (after Task 3 stub exists; for Task 1 use a scratch crate)
```
For Task 1 alone, prove libgit2-sys compiles: a scratch `cargo ndk -t arm64-v8a build` of a 3-line bin depending on the vendored git2. Expected: libgit2 (+ TLS backend) compiles under NDK clang, links. Then `cargo ndk -t x86_64 build` (2nd ABI — watch for arch-specific cmake flags). **Record which path (M/V) won + why in the commit; if Path V, amend spec G6 (mbedtls→openssl-vendored) in the same commit.**

- [ ] **Step 6: Host build stays clean.** `cargo build -p tool-git-mobile` (host) must compile — libgit2-sys vendored-libgit2 builds on macOS too (this is what makes P4b host-testable). Verify.

- [ ] **Step 7: Commit.**
```bash
git add third_party/git2-rs lingxi-code/Cargo.toml lingxi-code/Cargo.lock
# Path M also: git add third_party/libgit2 third_party/mbedtls lingxi-code/platforms/android-libgit2
git commit -m "feat(android-git): vendor git2-rs (+libgit2) and prove NDK cross-compile (P4a)"
```

### Task 2: re-vendor minijail (symlink → committed subset)

**Files:** Create `third_party/minijail/` (real, committed); the worktree symlink is replaced.

- [ ] **Step 1: Identify the build-required subset.** From `platforms/android-minijail/build.rs` + `android-libcap`, list exactly what the NDK build reads: the CORE `.c`/`.h` sources, `rust/` (if used), the pre-generated `*.gen.c`/tables, `libminijail.h`, license. EXCLUDE `.git/`, `tests/`, `graphify-out/`, examples.

- [ ] **Step 2: Materialize.** Copy the main-checkout `third_party/minijail` content (the symlink target) into a real `third_party/minijail/` in THIS worktree, pruned to the subset. Remove the symlink first.

- [ ] **Step 3: Prove the clean-checkout build still works.** With the real dir (no symlink): `export ANDROID_NDK_HOME=...; cargo ndk -t arm64-v8a build -p platform-android-minijail && cargo ndk -t x86_64 build -p platform-android-minijail`. Expected: the P0a–P2 minijail cross-compile passes against the vendored subset. If a pruned file turns out to be needed, add it back (note which).

- [ ] **Step 4: Commit.**
```bash
git add third_party/minijail
git commit -m "chore(android): vendor minijail into third_party (symlink → committed subset); repays CI build debt (P4a)"
```

---

# Phase P4b — local git operations (host-testable, no network)

> libgit2 is a library → these run end-to-end on the macOS host against real temp repos. No device needed. Pure TDD.

### Task 3: `tool-git-mobile` crate skeleton + `Git` schema + `GitTool` shell

**Files:** Create `tools/git-mobile/Cargo.toml`, `src/lib.rs`, `src/ops.rs`; Modify workspace `Cargo.toml` members.

- [ ] **Step 1: Cargo.toml.** Mirror `tools/shell-mobile/Cargo.toml`: deps `tool-api`, `traits`, `permission`, `git2` (path-dep `../../third_party/git2-rs/git2` with the Task-1 features), `serde`, `serde_json`, `async-trait`, `once_cell`; dev-deps `tool-api` (test-support feature), `tokio` (rt-multi-thread, macros), `tempfile`. `[lints] workspace = true`. `#![forbid(unsafe_code)]` in lib.rs. Add `"tools/git-mobile"` to workspace members.

- [ ] **Step 2: Write the failing schema/name test** in `lib.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn name_is_git_and_schema_has_operation() {
        let t = GitTool::new(test_ctx_git_enabled());
        assert_eq!(t.name(), "Git");
        let schema = t.input_schema();
        let props = &schema["properties"];
        assert!(props.get("operation").is_some(), "operation param required");
        assert!(schema["required"].as_array().unwrap().iter().any(|v| v == "operation"));
    }
}
```
(Add a `test_ctx_git_enabled()` helper building a `BuiltinToolContext` via `tool_api::test_support` with `android_git = Some(AndroidGitToolCtx{enabled:true, has_token:true, workspace_root:<tempdir>})` — `AndroidGitToolCtx` lands in Task 7; for now define a local minimal stand-in OR sequence Task 7 before this. SEQUENCING: do Task 7 (the carrier) before Task 3 so the ctx field exists. Reorder if executing strictly.)

- [ ] **Step 3: Run → FAIL** (`cargo test -p tool-git-mobile name_is_git`).

- [ ] **Step 4: Implement `GitTool { ctx }`** + `new` + `name()`→"Git" + `input_schema()` (operation enum string + optional repo_url/remote/branch/refspec/paths/message/rev/rev_range/new_branch; `operation` required) + `is_enabled` (`ctx.android_git.as_ref().is_some_and(|g| g.enabled)`) + `max_result_size_chars`/`is_concurrency_safe(false)`/`is_read_only(false)`/`check_permissions` (mirror shell-mobile stub) + `description` + `prompt` (declares structured git, lists supported operations, states no-push/ff-only/HTTPS-only, notes network ops need approval; if `!has_token` says network ops need credential config). `call()` dispatches on `operation` to `ops::` (Task 4/5/6); for now a `todo!`-free stub returning `ToolError::InvalidInput("unknown operation")` for all.

- [ ] **Step 5: Run → PASS; Commit.**
```bash
git add tools/git-mobile lingxi-code/Cargo.toml lingxi-code/Cargo.lock
git commit -m "feat(tool-git-mobile): Git tool skeleton + structured schema (P4b)"
```

### Task 4: `ops.rs` — workspace-anchored repo open + path validation

**Files:** `tools/git-mobile/src/ops.rs`.

- [ ] **Step 1: Failing tests** (use `tempfile` + `git2::Repository::init` to make a real repo under a temp workspace root):
```rust
#[test]
fn open_repo_under_workspace_ok() { /* init repo at <ws>/r; open_repo(<ws>, "r") -> Ok */ }
#[test]
fn open_repo_escaping_workspace_rejected() { /* path "../evil" or abs outside ws -> Err GitError naming escape */ }
#[test]
fn open_nonexistent_repo_named_error() { /* -> Err, not panic */ }
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** `fn open_repo(workspace_root: &Path, repo_rel: &str) -> Result<git2::Repository, GitOpError>`: canonicalize `workspace_root.join(repo_rel)`, require `starts_with(canonical workspace_root)` (reject escapes, mirror `AndroidMinijailSandbox::resolve_cwd`), then `git2::Repository::open`. Define a `GitOpError` enum (NotFound/Escape/Libgit2(String)/...) mapped from `git2::Error`.
- [ ] **Step 4: Run → PASS; Commit** `feat(tool-git-mobile): workspace-anchored repo open + path validation`.

### Task 5: `ops.rs` — read operations (status/diff/log/show/branch_list)

**Files:** `tools/git-mobile/src/ops.rs`.

- [ ] **Step 1: Failing tests** against a temp repo with a known commit history (helper: init → write file → add → commit twice on a branch). Test each: `status` reports a dirty file; `log` returns the 2 commits newest-first (with cap/paging); `diff` (workdir vs HEAD) shows the change; `show <rev>` returns the commit's diff; `branch_list` includes the default branch. Assert the returned serialized shape (define a `GitOutput`/JSON each op produces).
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** `status`(`Repository::statuses`), `log`(`Revwalk` + cap), `diff`(`diff_index_to_workdir`/`diff_tree_to_workdir`), `show`(`find_commit` + `diff_tree_to_tree`), `branch_list`(`branches`). Each returns a structured result (commit oid/summary/author/time for log; path+status for status; unified diff text for diff/show — truncate to a cap). No network, no writes.
- [ ] **Step 4: Run → PASS; Commit** `feat(tool-git-mobile): read ops (status/diff/log/show/branch_list)`.

### Task 6: `ops.rs` — local write operations (add/commit/branch_create/checkout/merge-ff)

**Files:** `tools/git-mobile/src/ops.rs`.

- [ ] **Step 1: Failing tests** against temp repos:
  - `add` then `commit` produces a new HEAD commit with the message; committer identity = repo `.git/config` user.* if set, else the fixed default `LingXi <noreply@lingxi>` (test both branches by setting/clearing config).
  - `branch_create` makes a new branch at HEAD.
  - `checkout` to another branch switches HEAD + worktree; **dirty worktree → rejected** (named error, no data loss).
  - `merge` fast-forward case advances HEAD; **non-ff case → named error** (no conflict markers written).
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** `add`(`Index::add_all`+`write`), `commit`(`Repository::commit` with `signature()` resolving config→default), `branch_create`(`Repository::branch`), `checkout`(`merge_analysis`-free: check `statuses` clean else reject; `set_head`+`checkout_tree` safe mode), `merge`(`merge_analysis` → only `ANALYSIS_FASTFORWARD` proceeds via `checkout_tree`+`set_head`; else `GitOpError::NonFastForward`). Wire all of Task 5+6 into `GitTool::call`'s dispatch (replace the Task-3 stub).
- [ ] **Step 4: Run → PASS** (`cargo test -p tool-git-mobile`), `cargo clippy -p tool-git-mobile --all-targets -- -D warnings`, `cargo fmt -p tool-git-mobile --check`; **Commit** `feat(tool-git-mobile): local write ops (add/commit/branch/checkout/merge-ff) + call dispatch`.

---

# Phase P4c — network operations + auth + wiring

### Task 7: `AndroidGitToolCtx` carrier on `BuiltinToolContext`

**Files:** Modify `tool-api/src/builtin_context.rs` (+ every ctx construction site, like P3 Task 1).

> SEQUENCING NOTE: execute this BEFORE Task 3 (Task 3's tests reference the field). Listed here to keep the network/wiring tasks together; the executor should do Task 7 first.

- [ ] **Step 1: Failing test** — `AndroidGitToolCtx { enabled, has_token, workspace_root }` constructs; `BuiltinToolContext` test builder defaults `android_git: None`.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** the carrier struct (mirror `AndroidShellToolCtx`, doc comments) + `pub android_git: Option<AndroidGitToolCtx>` field; re-export from crate root; add `android_git: None` to EVERY `BuiltinToolContext { .. }` site (`rg "BuiltinToolContext \{"` — engine-desktop, engine-mobile host, test_support builders cover the tool tests). `cargo check --workspace` finds them all.
- [ ] **Step 4: Run** `cargo test -p tool-api && cargo check --workspace`; **Commit** `feat(tool-api): AndroidGitToolCtx carrier on BuiltinToolContext (P4 seam)`.

### Task 8: `auth.rs` — credential callback + CA wiring + network ops

**Files:** `tools/git-mobile/src/auth.rs`, `src/ops.rs` (clone/fetch/pull).

- [ ] **Step 1: Failing tests** (host, against a `file://` bare remote so no real network/CA/token needed):
  - `clone` from a `file://` bare repo into a temp workspace produces a working repo with the remote's HEAD.
  - `fetch` then `pull` (ff) from the `file://` remote advances the local branch.
  - the credential callback is invoked with the configured token (unit-test `make_remote_callbacks(token)` installs a `credentials` cb that yields `Cred::userpass_plaintext`); assert it's wired (a test that the callback closure returns the expected Cred given a token).
  - non-ff `pull` → `GitOpError::NonFastForward`.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** `auth.rs`: `fn make_fetch_options(token: Option<&str>) -> git2::FetchOptions` installing `RemoteCallbacks::credentials` → `Cred::userpass_plaintext("x-access-token", token)` when a token is present (else default for public); a `fn set_ca_location()` calling `git2::opts::set_ssl_cert_locations` (or the Path-V openssl equivalent) pointed at the ctx-supplied CA dir/file. `ops.rs`: `clone`(`RepoBuilder::clone` + fetch options), `fetch`(`Remote::fetch`), `pull`(fetch + ff-merge reusing Task 6's ff logic). Network ops read the token + CA path from a small `GitNetConfig` passed by `GitTool::call` (sourced from ctx).
- [ ] **Step 4: Run → PASS** + clippy/fmt; **Commit** `feat(tool-git-mobile): network ops (clone/fetch/pull) + in-process token cred callback + CA (P4c)`.

### Task 9: register `Git` in engine-mobile + thread `MobileConfig.android_git`

**Files:** Modify `apps/engine-mobile/{Cargo.toml,lib.rs,host.rs}`, workspace `Cargo.toml`.

- [ ] **Step 1:** add `tool-git-mobile` dep to engine-mobile (mirror the `tool-shell-mobile` dep line).
- [ ] **Step 2:** `MobileConfig` gains `pub android_git: Option<tool_api::AndroidGitToolCtx>` (+ `Default` = None); `host.rs` BuiltinToolContext literal sets `android_git: cfg.android_git.clone()`.
- [ ] **Step 3:** `register_mobile_tools` adds `tool_git_mobile::register_all(reg, ctx.clone());` among the mobile-exclusive tools (self-gates on `ctx.android_git`).
- [ ] **Step 4:** Test (in `mobile_shell_gating.rs` sibling or a new `mobile_git_gating.rs`): default ctx (`android_git: None`) → registry lacks "Git"; `Some(enabled:true)` → contains "Git"; `Some(enabled:false)` → lacks it. Verify the existing `mobile_tool_list_snapshot` is unchanged (Git absent by default).
- [ ] **Step 5:** `cargo test -p engine-mobile && cargo clippy -p engine-mobile -p tool-git-mobile --all-targets -- -D warnings && cargo check --workspace`; **Commit** `feat(engine-mobile): register the mobile Git tool, gated on android_git ctx (P4c)`.

### Task 10: compute the Git gate + token in android-aar

**Files:** Modify `apps/android-aar/src/lib.rs`.

- [ ] **Step 1:** Host-testable gate fn (mirror `android_shell_gate`):
```rust
#[must_use]
fn android_git_gate(enable_git: bool, workspace_ready: bool, ca_store_reachable: bool) -> bool {
    enable_git && workspace_ready && ca_store_reachable
}
```
+ table-driven unit test (true iff all three; each single-false → false).
- [ ] **Step 2:** Add `AndroidGitConfig { enable_git, workspace_root, ca_cert_dir, https_token: Option<String> }` to the Android inputs (FFI record `AndroidGitConfigFfi` mirroring `AndroidShellConfigFfi`); in `build_android_engine` (android branch) compute `AndroidGitToolCtx { enabled: android_git_gate(...), has_token: cfg.https_token.is_some(), workspace_root }` and a `GitNetConfig`(token + ca_dir) threaded so `GitTool::call` can reach the token — store the token in `MobileConfig.android_git`-adjacent state (the token rides a separate field, NOT the public `AndroidGitToolCtx` if you want to keep the ctx token-free; simplest: a `MobileConfig.android_git_secret: Option<GitNetConfig>` consumed at BuiltinToolContext build time and stored where `GitTool` reads it). Assign into `MobileConfig`. Non-android branch → None.
- [ ] **Step 3:** `cargo test -p android-aar && cargo check --workspace`; `cargo ndk -t arm64-v8a build -p android-aar && cargo ndk -t arm64-v8a clippy -p android-aar -- -D warnings`.
- [ ] **Step 4: Commit** `feat(android-aar): compute Git-tool gate + in-process token into MobileConfig (P4c)`.

---

# Phase P4d — device acceptance + P4 gate

### Task 11: on-device Git acceptance (real HTTPS clone)

**Files:** `apps/android-aar/src/lib.rs` (test export), `clients/android/app/src/androidTest/java/com/lingxi/code/GitToolTest.kt`.

- [ ] **Step 1:** UniFFI export `android_git_probe(operation_json: String, workspace: String) -> String` that drives the real `GitTool` (build ctx with a probed `android_git` + a test CA dir, run the op, return JSON). Host build → `{"error":"host build"}`.
- [ ] **Step 2:** `GitToolTest.kt` (mirror `SandboxRunTest.kt`): on the API-34 arm64 emulator, `clone` a small public HTTPS repo into the app filesDir → assert success + a known file exists; then local `log`/`status` round-trip. This proves mbedtls/openssl + system cacerts + real TLS end-to-end.
- [ ] **Step 3:** `bash clients/android/scripts/build-jni.sh` (NDK env) → bindings contain `androidGitProbe`; `./gradlew :app:assembleDebugAndroidTest`; boot emulator `p0a` (P0a/P2 runbook) and `./gradlew :app:connectedDebugAndroidTest -Pandroid.testInstrumentationRunnerArguments.class=com.lingxi.code.GitToolTest`. If the emulator/network is unavailable, report **PENDING-DEVICE** with the runbook; the host file:// tests (P4b/c) + cross-build are the merge gate. Commit the artifacts regardless.
- [ ] **Step 4: Commit** `feat(android-aar): on-device Git clone acceptance test (P4d)`.

### Task 12: P4 gate

- [ ] **Step 1:** `cargo fmt --all`; revert drift outside P4 crates (tool-api, tool-git-mobile, engine-mobile, android-aar, android-libgit2) — `git diff --name-only main...HEAD | grep -v docs/` lists touched files.
- [ ] **Step 2:** `cargo clippy --workspace --all-targets -- -D warnings` (fix only blockers; note pre-existing vs P4); android-target clippy `cargo ndk -t arm64-v8a clippy -p tool-git-mobile -p engine-mobile -p android-aar -- -D warnings`.
- [ ] **Step 3:** `cargo test --workspace` (known host flakes: `tool-shell cwd_persistence`, `powershell` pwsh-missing, `platform-posix mcp_stdio` build-order — rerun/note as established).
- [ ] **Step 4:** `cargo ndk -t arm64-v8a build -p android-aar && cargo ndk -t x86_64 build -p android-aar` (both cdylib link, now pulling libgit2). Watch for a libgit2 link-anchor need (rustc dropping the build-only crate, the P2 libcap lesson) — if `dlopen`/link drops `git2_*` symbols, add `use ... as _;` anchor where `tool-git-mobile` is reachable.
- [ ] **Step 5: Commit** any fixups `chore(android-git): P4 gate — workspace + android-target clean`.

---

## Self-review / spec coverage

- G1 libgit2 in-process: Task 1 (vendor+build) + Task 3 (git2 dep, forbid-unsafe). ✓
- G2 op set (read+local-write, no push): Tasks 5+6 (local), Task 8 (clone/fetch/pull); no push anywhere. ✓
- G3 auth in-process token: Task 8 (cred callback) + Task 10 (host token threading, never disk/env). ✓
- G4 vendoring: Task 1 (git2-rs/libgit2) + Task 2 (minijail re-vendor). ✓
- G5 gate independent of sandbox, token not in gate: Task 10 (`android_git_gate` = enable+workspace+CA, no caps; token→has_token only) + Task 9 (register_all gate). ✓
- G6 TLS/CA: Task 1 entry decision (M/V) + Task 8 (`set_ca_location`) + Task 11 (device TLS proof). ✓
- G7 HTTPS-only: Task 8 (userpass cred only; `git@` → named error — add to Task 8 impl). ✓
- G8 ff-only + commit identity: Task 6 (merge/pull ff, identity fallback) + Task 8 (pull ff). ✓
- Absent-not-erroring: Task 9 gating tests. ✓ forbid-unsafe: Task 3. ✓

## Risks (P4-specific)

- **libgit2-sys NDK cross-compile is the gate** (Task 1) — front-loaded; M/V fallback documented; cmake-under-cargo-ndk is the usual sharp edge.
- **Token threading shape** (Task 10): keeping the public `AndroidGitToolCtx` token-free (token in a separate secret field) avoids leaking the token into a broadly-cloned ctx struct — chosen for that reason; verify `GitTool` reads it from the intended place.
- **libgit2 link-anchor** (Task 12): the P2 libcap lesson — a build-only/native dep can be dropped from the cdylib link; anchor if symbols go missing on-device.
- **third_party size**: git2-rs + libgit2 (+ openssl/mbedtls) vendored adds repo weight; prune to build-required (Task 1/2 prune like libcap).
- **Device-only TLS**: real HTTPS clone is unverifiable on host (file:// stands in); Task 11 is the acceptance proof, PENDING-DEVICE acceptable for merge.
