# sandbox-runtime P4-2b — generateFilesystemArgs + FS helpers (linux-sandbox-utils.js, part 1)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Port `generateFilesystemArgs` + its FS helpers + `linuxGetMandatoryDenyPaths` from `linux-sandbox-utils.js` into `sandbox-runtime/src/fs_args.rs`. The bwrap filesystem-restriction arg builder — the most intricate, SECURITY-CRITICAL function in the package (deny-write/read `/dev/null` masking, empty-dir-for-intermediate, file-ancestor skip, denyRead-tmpfs ordering, allowRead-within-deny re-bind, denyWrite-after-denyRead masking). Produces the `--ro-bind`/`--bind`/`--tmpfs` arg vector + the set of host mount-point files to clean up afterward.

**Reference of truth (READ each — `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/linux-sandbox-utils.js`):**
- `findSymlinkInPath` :21-44, `hasFileAncestor` :53-81, `findFirstNonExistentComponent` :83-100 (pure FS path walks).
- `linuxGetMandatoryDenyPaths` :102-215 (ripgrep `--files --hidden --max-depth N --iglob ...` over `getDangerousDirectories()`/`DANGEROUS_FILES` from path_utils + the `.git/hooks`/`.git/config` conditionals).
- `generateFilesystemArgs` :527-772 (the orchestrator).
- `DEFAULT_MANDATORY_DENY_SEARCH_DEPTH = 3` (:13).

**Config shapes (from sandbox-schemas.d.ts):** `ReadConfig { deny_only: Vec<String>, allow_within_deny: Vec<String> }` (EMPTY deny_only = allow ALL reads); `WriteConfig { allow_only: Vec<String>, deny_within_allow: Vec<String> }` (EMPTY allow_only = deny ALL writes). Define these in `fs_args.rs` (or a `config.rs` addition) faithfully. Uses `path_utils::{normalize_path_for_sandbox, is_symlink_outside_boundary, get_dangerous_directories, DANGEROUS_FILES}` (P4-2a, merged).

**Branch:** `parity-sandbox-runtime-p4-2b`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`**. `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. `tempfile` is a dev-dep already; `rg` is shelled (the host has ripgrep; tests that exercise the ripgrep path run against a tempdir cwd, gated to skip if `rg` is absent).

---

### Task 1: FS path-walk helpers + mandatory-deny

**Files:** Create `lingxi-code/sandbox-runtime/src/fs_args.rs`; Modify `src/lib.rs`.

- [ ] Port (TDD, tempdir fixtures) `find_symlink_in_path(target, &allowed_write_paths) -> Option<String>`, `has_file_ancestor(target) -> bool` (existing component is a file/symlink ⇒ true — the git-worktree `.git`-is-a-file case), `find_first_non_existent_component(target) -> String`. Tests: build a tempdir tree with a symlink + a file-where-a-dir-is-expected + a partially-existing path; assert each helper. These are pure FS walks — faithful to :21-100.

- [ ] Port `linux_get_mandatory_deny_paths(ripgrep_cmd, max_depth, allow_git_config, cwd) -> Vec<String>` (:102-215): seed = `DANGEROUS_FILES.map(resolve(cwd,_))` + `get_dangerous_directories().map(resolve(cwd,_))`; add `.git/hooks` (+ `.git/config` unless allow_git_config) ONLY when `<cwd>/.git` is a directory; then a SINGLE `rg --files --hidden --max-depth <n> --iglob <file>… --iglob **/<dir>/** --iglob **/.git/hooks/** [--iglob **/.git/config]` over cwd, resolve each match against cwd, union with the seed (dedup). Shell `rg` via `std::process::Command` (or tokio). Test against a tempdir cwd containing a nested `.env` + a `.git/` dir; gate-skip if `rg` not on PATH (report). Faithful to the exact iglob args + the `.git`-is-dir conditional.

### Task 2: generateFilesystemArgs

- [ ] Port `generate_filesystem_args(read_config: Option<&ReadConfig>, write_config: Option<&WriteConfig>, ripgrep_cmd, max_depth, allow_git_config, cwd) -> (Vec<String> args, Vec<PathBuf> mount_points)` (:527-772). **Faithful refactor:** the TS uses a module-global `bwrapMountPoints` Set + `registerExitCleanupHandler`; instead RETURN the `mount_points` (the host `/dev/null`/empty-dir mount files created for non-existent denies) so the caller (P4-2c) owns cleanup — cleaner + testable; document this divergence. Port EVERY branch:
  - write_config present → `--ro-bind / /`; for each `allow_only` (skip `/dev/*`, skip non-existent, skip symlink-outside-boundary via realpath + `is_symlink_outside_boundary`) → `--bind p p` + record allowedWritePaths.
  - deny set = `deny_within_allow` ++ `linux_get_mandatory_deny_paths(...)`; dedup post-normalize; skip `/dev/*`; symlink-in-path (within an allowed write path) → buffer `--ro-bind /dev/null <symlink>`; non-existent → `has_file_ancestor` skip, else deepest-existing-ancestor-within-allowed check → `find_first_non_existent_component`: intermediate ⇒ `--ro-bind <empty tempdir> <firstNonExistent>` + record mount-point, leaf ⇒ `--ro-bind /dev/null <firstNonExistent>` + record; existent-within-allowed → buffer `--ro-bind p p`; else skip.
  - no write_config → `--bind / /`.
  - read deny: expand `/` deny into children (skip proc/dev/sys); always add `/etc/ssh/ssh_config.d` if it exists; normalize + sort shallow-first; dir → `--tmpfs p` + re-bind allowedWritePaths under it + re-bind allowWithinDeny under it (with the write-path-covers-allowPath skip); file → exact-allowRead-match skip, else `--ro-bind /dev/null p` + record masked; 
  - emit buffered denyWrite LAST, skipping any dest in maskedFiles.
- [ ] **Tests (tempdir matrix)** — each asserts the produced arg vector contains/excludes the right `--ro-bind`/`--bind`/`--tmpfs` triples: (a) write-restrict root + allow a dir → `--ro-bind / /` then `--bind <dir> <dir>`; (b) deny within allow (existent) → `--ro-bind <p> <p>` AFTER the allow bind; (c) non-existent leaf deny within allowed → `--ro-bind /dev/null <leaf>` + mount_point recorded; (d) non-existent intermediate → `--ro-bind <emptydir> <component>`; (e) file-ancestor (`.git` is a file) deny → SKIPPED; (f) denyRead dir → `--tmpfs <dir>` + write/allowRead re-binds; (g) denyRead file → `--ro-bind /dev/null <file>`; (h) denyWrite dest masked by a denyRead `/dev/null` → denyWrite NOT re-emitted; (i) no write_config → `--bind / /`. Use real tempdirs.

- [ ] **Run `cargo test -p sandbox-runtime` → PASS. Gate + commit** (`feat(sandbox-runtime): generateFilesystemArgs + FS helpers + mandatory-deny (P4-2b)`).

### Task 3: gates
`cargo test -p sandbox-runtime` + clippy `-D warnings` + `cargo test --workspace --no-run` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0) + frozen diff empty. Stage ONLY explicit sandbox-runtime + Cargo paths.

## Final verification
1. Every branch of generateFilesystemArgs ported (cite file:line); the security invariants hold: non-existent denies blocked via /dev/null-or-emptydir, denyRead-before-denyWrite ordering, allowRead exact-file-match (not dir) un-deny, symlink-replacement masking. mount_points returned for caller cleanup (documented divergence from the global Set).
2. ripgrep mandatory-deny: exact iglob args + `.git`-is-dir conditional; test gate-skips if `rg` absent.
3. engine-mobile 0-dep; frozen empty.
