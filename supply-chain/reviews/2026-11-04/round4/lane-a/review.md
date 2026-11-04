# cfg-if 1.0.4 — full exact-version source review

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review), 2026-11-04. This is an independent full review of this exact package, not a publisher-wide trust decision, a hash-only certification, or an inherited upstream audit. No canonical supply-chain file was changed. **Recommendation: `safe-to-deploy` for `cfg-if` 1.0.4**, which also implies `safe-to-run`, subject to independent cross-review before registration. No serious package-local vulnerability was identified. The compiler, generated caller code, and optional dependency remain separate review subjects.

## Identity and actual graph

- Package ID: `registry+https://github.com/rust-lang/crates.io-index#cfg-if@1.0.4`. Archive SHA-256: `9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801`.
- The cached crate archive matches that digest in App's `Cargo.lock:1228-1231` and both locally cached crates.io index entries. An independent HTTPS download from `https://static.crates.io/crates/cfg-if/cfg-if-1.0.4.crate` was streamed through SHA-256 and produced the same digest; no new archive copy was retained. The index is a cached observation, not a claim of current yank status.
- `identity-inventory.json` records all **14** archive members, sizes, modes and hashes; each is a regular file, with no duplicate or traversal name, and matched its extracted source byte-for-byte. No unlisted non-Cargo-cache files were found in the extracted tree. The archive's `.cargo_vcs_info.json` declares commit `3510ca6abea34cbbc702509a4e50ea9709925eda`; this is package-supplied metadata, not an independently checked Git-tree identity.
- App delivery lock SHA-256 `8107432e39037dbd710de6b78bdb706d8aebdb63ab0d4175a94d487ee3e153c6` and supply-chain store match both the 821-failure frozen receipt `integration/round3-20261103/source-audit/after-run.json` and App main commit `d1560f56e6e30795775f110fa2c834217b5eb762`. Its `after.stdout` requires `safe-to-deploy` for this exact version. No `cfg-if` entry was found in the delivery store's audits, imports, or config. Existing concurrent App edits were not touched.
- Fresh Rust 1.94.0 `cargo metadata --locked --offline` on that actual delivery manifest found the exact package, **44 direct reverse-parent package identities**, and no resolved features or dependencies for it; see `resolved-identity.json`. Separate workspace `--all-features` metadata also resolved no features/dependencies on this package (`all-features-node.json`); this does **not** force this dependency's own optional feature. Its feature-declared optional `rustc-std-workspace-core` is a separate crate and was not audited here.
- There was an earlier reviewed `1.0.0 -> 1.0.4` delta proposal and a candidate store under `source-audits/lane-c-001` / `resume-20261103/next-lane-b`, but that proposal is not a registered exact full audit in the delivery store. This review covers every file in 1.0.4 itself and does not rely on the old delta or a baseline import.

## Files and behavior reviewed

Every shipped file was read. `Cargo.toml`, `Cargo.toml.orig`, package `Cargo.lock`, `src/lib.rs`, and `tests/xcrate.rs` contain the package's build/runtime/test surface. `README.md`, `CHANGELOG.md`, `.cargo_vcs_info.json`, `.gitignore`, `LICENSE-APACHE`, and `LICENSE-MIT` provide descriptions or metadata. `.github/dependabot.yml` and both `.github/workflows/{main,publish}.yaml` are upstream repository automation: the publish workflow references GitHub/Cargo release secrets and external actions when separately run by that repository, but Cargo compilation of a registry dependency does not execute these workflows. The upstream actions themselves were not audited.

The normalized manifest is edition 2018, declares Rust 1.32, sets `build = false`, disables autodiscovered targets, explicitly defines only the library and one integration test, and has no native code, procedural macro, build script, binary, or default runtime dependency. It declares optional `rustc-std-workspace-core` via feature `rustc-dep-of-std`; dependency review is separate. The shipped lockfile describes the package's own test/dependency resolution, not the consuming App's lock.

`src/lib.rs:26-95` is `no_std` and exports only the declarative `cfg_if!` macro; there is no package-local `unsafe`, FFI, filesystem, process, environment, network, time, allocation, or runtime input handling. Caller-provided token trees are the code emitted: if a caller puts unsafe code or side effects in a selected branch, those are caller behavior and not certified by this package review.

The top-level rule accepts one `if #[cfg(...)]`, zero or more `else if` clauses, and an optional `else`. It transforms clauses into a list for the internal `@__items` rule. For each conditional branch, the emitted cfg is `all(current, not(any(previous)))`; previous conditions accumulate on recursion. An unconditional final else has only the negation of earlier conditions. Thus the first true branch is included, and no else is emitted when absent. The `@__temp_group` indirection places one cfg on the macro invocation that groups **all** tokens from a branch, so multiple items or statements cannot have only their first token gated. `$crate` recursion selects the originating macro even when the external crate is renamed. Invalid cfg syntax is left to the Rust compiler; the macro does not synthesize extra platform policy. Internal `@__...` arms are exported as implementation details and should not be assumed stable APIs. Very long caller-supplied chains can encounter the compiler's macro recursion limit; no untrusted runtime input enters this expansion.

The two embedded unit tests and `tests/xcrate.rs` exercise first match, fallbacks, function statements, trait method placement, and a multiple-item conditional. Doc examples contain only ordinary sample declarations. No package-local security finding resulted from the source review.

## Criteria and validation

[cargo-vet's built-in criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html) distinguish controlled local compile/run/test effects (`safe-to-run`) from serious security vulnerabilities in production exposed to untrusted input (`safe-to-deploy`, which implies the former). The manifest, code and tests support `safe-to-run`: building and testing this package itself has no surprising ambient capability. They also support `safe-to-deploy`: the package has no runtime parser or unsafe/privileged operations, and the generated conditional selection preserves the source-provided first-match semantics in the reviewed rules. The impact of a consumer choosing an unsafe implementation, misuse of a platform cfg, or untrusted contents in emitted caller code is not certified by this finding.

Validation used rustc/cargo **1.94.0** on the local macOS host, not a platform matrix or formal proof:

1. `cargo test --locked` on the byte-matched exact crate passed **2 unit + 1 integration + 1 doc test** (`upstream-cargo-tests.log`). The initial offline attempt lacked the optional dependency's registry index and failed before running tests (`upstream-cargo-offline-attempt.log`); the successful retry used a fresh temporary Cargo home and target with network access, then removed them. No product lock/store was changed.
2. `run-cfg-if-probes.py` separately built the exact source with rustc, reran the two unit and one external tests, then compiled and ran `cfg-if-probe.rs` for all eight combinations of three cfg predicates. These exercise overlapping true predicates, fallback, missing else, nested expressions and nested macros, rename hygiene, multiple items, function statements, trait methods, and elimination of a nonexistent type in a false branch. All eight passed (`probe-results.log`).
3. `negative-control.py` changes the previous-branch negation only in a temporary copy; with two matching branches the same probe rejects duplicate definitions (`negative-control.log`). This demonstrates sensitivity to that specific first-match regression, not all possible defects.

The Rust 1.32 MSRV, every compiler target, unusual macro-internal invocations, and all 44 consumers were not executed or exhaustively reviewed. The crate's only optional dependency and the build toolchain require their own evidence. This is a package-specific risk judgment, not evidence that the overall cargo-vet gate, the application, or every platform passes.

## Proposed exact full audit — not registered

```toml
[[audits.cfg-if]]
who = "Codex AI-assisted"
criteria = "safe-to-deploy"
version = "1.0.4"
notes = "Full exact-version source review; no build script, proc macro or package-local unsafe. First-match and multi-item cfg expansion reviewed and exercised; caller code and optional dependency assessed separately. See round4 lane-a evidence and independent cross-review before registration."
```

The final store notes should reference the delivered report/cross-review paths and hashes after the independent reviewer confirms them. No exemption, wildcard, trusted publisher entry, or criteria mapping is proposed.
