# sandbox-runtime P4-2a — Path/glob utilities (sandbox-utils.js, part 2)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Port the pure path/glob utilities from `sandbox-utils.js` that `generateFilesystemArgs` (P4-2b) depends on, into `sandbox-runtime/src/path_utils.rs`. Faithful 1:1; unit-testable.

**Functions (reference: `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/sandbox-utils.js` — READ each):**
- `DANGEROUS_FILES` (const, :10-30 region) + `getDangerousDirectories()` (:31-46) — the mandatory-deny seed sets.
- `normalizeCaseForComparison(pathStr)` (:47-52).
- `containsGlobChars(pathPattern)` (:53-62).
- `removeTrailingGlobSuffix(pathPattern)` (:63-79).
- `isSymlinkOutsideBoundary(originalPath, resolvedPath)` (:80-168) — the symlink-escape check (security-critical).
- `normalizePathForSandbox(pathPattern)` (:169-237) — `~` expansion, abs-resolve, trailing-slash, the core path normalizer.
- `getDefaultWritePaths()` (:238-260).
- `globToRegex(globPattern)` (:402-428) — gitignore-style glob→regex.
- `expandGlobPattern(globPath)` (:429-end) — filesystem glob expansion.

**Branch:** `parity-sandbox-runtime-p4-2a`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`** (untracked codex/liter-llm/opencode dirs). `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

### Task 1: `path_utils.rs`

**Files:** Create `lingxi-code/sandbox-runtime/src/path_utils.rs`; Modify `src/lib.rs` (+`pub mod path_utils;`). Deps: `dirs` (home dir for `~` expansion — likely in lock; else add) + `regex` (for globToRegex — check the lock; `regex` is almost certainly present) + std::fs/std::path.

- [ ] **Step 1: Read the 9 functions in the reference + write failing tests** covering the key faithful behaviors (table-driven). MINIMUM cases:
  - `normalize_path_for_sandbox`: `~/x` → `<home>/x`; relative → absolute (against cwd); trailing slash handling; `.`/`..` resolution per the TS. (Use an injected home + cwd so tests are deterministic — pass them as params rather than reading env/cwd inside; if the TS reads `os.homedir()`/`process.cwd()`, take them as args and have a thin wrapper read the real ones, keeping the core pure + testable.)
  - `is_symlink_outside_boundary`: a resolved path under the original's parent boundary → false (inside); a resolved path escaping to `/etc` → true. Cover the exact boundary logic in the TS.
  - `contains_glob_chars`: `*`,`?`,`[`,`]` → true; plain path → false.
  - `remove_trailing_glob_suffix`: `a/b/**` → `a/b`; `a/*.ts` → unchanged (only trailing `/**`-style per the TS).
  - `glob_to_regex`: `*.ts` matches `foo.ts` not `foo/bar.ts`; `src/**/*.ts` matches nested; `?`/`[abc]` per the TS doc.
  - `get_dangerous_directories` / `DANGEROUS_FILES`: assert the exact set the TS lists.
  - `normalize_case_for_comparison`: platform-case behavior per TS.

- [ ] **Step 2: Verify fail → implement** faithfully from the reference. Keep FS-touching fns (`expand_glob_pattern`, the `realpathSync` in symlink checks) thin; the pure logic (glob→regex, normalize, contains/remove-glob, dangerous sets) is the bulk + the unit-test target. For `expand_glob_pattern` (FS walk), a small integration test against a tempdir.
  - `~` expansion: use `dirs::home_dir()` in a thin wrapper; the core normalizer takes the home dir as a param.
  - `glob_to_regex`: the TS converts gitignore-style globs; port the exact escaping + `**`/`*`/`?`/`[...]` translation. Use the `regex` crate for the resulting `Regex`.

- [ ] **Step 3: Run `cargo test -p sandbox-runtime` → PASS. lib.rs decl. Gate + commit** (`feat(sandbox-runtime): path/glob utilities — normalize/glob/symlink-boundary/dangerous-sets (P4-2a)`).

### Task 2: gates
`cargo test -p sandbox-runtime` + `cargo clippy -p sandbox-runtime --all-targets --no-deps -- -D warnings` + `cargo test --workspace --no-run` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0) + frozen diff empty. Stage ONLY explicit sandbox-runtime + Cargo paths.

## Final verification
1. The 9 functions ported faithfully (cite file:line in doc comments); security-critical `is_symlink_outside_boundary` + `normalize_path_for_sandbox` covered by tests.
2. engine-mobile 0-dep; frozen empty. No new heavy deps beyond regex/dirs (both likely already in lock — confirm; if a NEW dep, note it).
