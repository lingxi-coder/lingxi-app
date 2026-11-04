# Round 3 source audits — lane A

Review date: 2026-11-03. Reviewer: Codex (GPT-6.1 Terra; AI-assisted). This is an independent source review, not an assertion that the upstream author or any organization audited these versions. Final registration belongs to the delivery owner and requires independent cross-review.

## Scope and immutable identity

App `delivery-round1/lingxi-app` HEAD `f6ebd8dc194f49b3ceb142dc7c5950d8a1f1ed3e`, Cargo.lock SHA-256 `8107432e39037dbd710de6b78bdb706d8aebdb63ab0d4175a94d487ee3e153c6`. The snapshot's `integration/current-main-audit-20261103/after.stdout` reports both exact versions missing `safe-to-deploy`. No matching audit/import record was found in the delivery supply-chain store at review time. `identity-inventory.json` records the complete archive file lists, each member SHA-256, cached sparse-index records, and registry identities. Both cached `.crate` archives match the App lock and cached index checksums, and every archive member matches the corresponding extracted source byte for byte; the only extra extracted file is Cargo's `.cargo-ok`. The index is a cached observation, not a claim about present registry yanking status. No remote re-download was used.

Cargo-vet [built-in criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html): `safe-to-run` concerns effects of controlled local compilation, execution and testing; `safe-to-deploy` additionally concerns serious vulnerabilities when exposed to untrusted input and implies `safe-to-run`. Exact-package conclusions below do not cover its consumers, the compiler, other dependencies, the full application, platforms not tested, or live services.

## version_check 0.9.5 — propose full safe-to-deploy

Identity: `registry+https://github.com/rust-lang/crates.io-index#version_check@0.9.5`; archive SHA-256 `0b928f33d975fc6ad9f86c8f283853ad26bdd5b10b7f1542aa2fa15e2289105a`. Archive has 13 files. Reviewed all shipped Rust (`src/lib.rs`, `src/channel.rs`, `src/date.rs`, `src/version.rs`, including test modules), both manifests, README, CI workflows, VCS metadata and archive inventory. No build script, proc macro, native/platform-specific source, dependencies or `unsafe` blocks ship in the crate. Included static compiler-output fixtures are absent by design (`Cargo.toml.orig` excludes `static`).

`get_version_and_date` (`src/lib.rs:158-164`) obtains `RUSTC`, falling back to the PATH executable `rustc`, and starts it directly with fixed arguments `--verbose --version`. It inherits the build environment, captures stdout without a shell, and parses version/release/date. This deliberately executes whatever binary the trusted compiler environment names; a hostile `RUSTC` or PATH is therefore arbitrary code execution in the caller's build environment, not sandboxed by the crate. It neither performs network I/O nor autonomously reads/writes unrelated files. `supports_feature` (`324-351`) derives channel availability and reads `CARGO_ENCODED_RUSTFLAGS` / `RUSTFLAGS`; it does not compile a feature probe or guarantee that a feature actually works. `Version::parse` (`version.rs:57-74`) and `Date::parse` (`date.rs:51-66`) use checked numeric conversion and reject excess components; their values are packed into integers. Channel and version comparison use normal safe Rust.

Documented limitations requiring caller judgment: `Command::output` exit status is not checked before parsing stdout, a date like `2024-02-31` passes the day/month bounds without Gregorian validation, and a non-hyphenated arbitrary string can be parsed as the stable channel. `Version` intentionally ignores the release channel and drops suffix content. `probe/version_check_probe.rs`, `nonzero-rustc`, and `.out` reproduce these behaviors with rustc 1.94.0. The failed-compiler case requires a selected compiler/wrapper that emits parseable output while failing; ordinary builds must already trust that executable. These findings are correctness and trust-boundary limitations, not an established attacker-reachable severe production vulnerability. Consumer scripts identified from the current lock include ahash, figment, generic-array, proc-macro2-diagnostics and uncased; a selected source check found feature/cfg decisions from these APIs, but did not establish all target activation or audit their resulting code.

On rustc `1.94.0 (4a4ef493e 2026-03-02)`, direct archive-matched unit test compilation and execution reported 10 passed. The shipped test `test_stable_compatibility` returns early because `static` is excluded; this is not 51 historical-compiler fixture passes. The bounded custom probe checks excess components, calendar/channel limitations and a controlled failing fake compiler. This is not an all-platform or exhaustive parser test. Under the ordinary trusted-compiler environment contract, the review supports full `safe-to-deploy` and its implied `safe-to-run`, with the above limitations explicitly recorded.

Proposed record, not registered:

```toml
[[audits.version_check]]
who = "Codex (GPT-6.1 Terra; AI-assisted)"
criteria = "safe-to-deploy"
version = "0.9.5"
notes = "Full exact-source review: invokes RUSTC/PATH rustc directly; compiler environment must be trusted. Does not check child exit status, validate Gregorian dates, or probe whether a feature actually works. See lane A review."
```

## version-compare 0.2.1 — safe-to-run proposal; safe-to-deploy HOLD

Identity: `registry+https://github.com/rust-lang/crates.io-index#version-compare@0.2.1`; archive SHA-256 `03c2856837ef78f57382f06b2b8563a2f512f7185d732608fd9176cb3b8edf0e`. Archive has 15 files. Reviewed every Rust source including tests and both examples (`src/{lib,cmp,compare,manifest,part,test,version}.rs`), both manifests, package lock, README and VCS metadata. The crate has no dependencies, build script, proc macro, native/platform code, `unsafe`, process execution, filesystem or network access. It parses caller-provided strings into borrowed text/numeric `Part`s and compares them; it allocates vectors and lowercase strings. Long digit-plus-text tokens cause repeated character rescans (`version.rs:365-372`); the crate imposes no input-size bound, so callers exposed to untrusted inputs need their own resource policy. No large-input timing benchmark was performed.

The exact source contains a substantive semantic flaw. In `compare_iter` (`version.rs:407-465`), the default-manifest Number/Text combination reaches the catch-all arm at lines 452-453 and silently compares that position as equal. With rustc 1.94.0 the checked-in `probe/version_compare_probe.rs` / `.out` reproducibly show:

- `1.0.0-1` and `1.0.0-alpha` compare `Eq` despite distinct semver precedence.
- `1.2.1 == 1.alpha.1` and `1.alpha.1 == 1.3.1`, while `1.2.1 < 1.3.1`: equality is not transitive.
- `1.0.0+build.1` compares less than `1.0.0`, despite build metadata not affecting semver precedence.
- Mixing GNU and default manifests yields `a == b` but `b != a`: comparisons use only the left-hand manifest. `set_manifest` also changes the stored manifest without re-parsing existing parts (`version.rs:155-159`).

The 30 upstream unit tests passed under rustc 1.94.0, but do not cover these counterexamples. The README/lib explicitly claim semver comparison works without extra configuration. This is a real library correctness defect, not a demonstrated memory-safety flaw or confirmed vulnerability in the deployed product. The App lock names `system-deps` 6.2.2, 7.0.8, and 9.0.0 as direct consumers. Their source uses this comparator to choose feature overrides and, in an internal-build callback path, to check native library versions. We did not show attacker control of those inputs, actual product activation, or exploitation. Nevertheless, the exact package can be used for version/security thresholds and we have not excluded serious consequences under reasonable untrusted-input use. **Do not propose `safe-to-deploy` yet**; it remains held pending a reasoned risk resolution or corrected version.

There are no ambient machine effects in this crate's sources or examples beyond normal allocation/printing, and controlled local execution does not rely on the correctness of a version-based security gate. Independently of the held deploy criterion, the source supports the narrower `safe-to-run` criterion, with the comparison and resource limits disclosed. This does not close the required `safe-to-deploy` gap.

Proposed record, not registered:

```toml
[[audits.version-compare]]
who = "Codex (GPT-6.1 Terra; AI-assisted)"
criteria = "safe-to-run"
version = "0.2.1"
notes = "Full source reviewed for controlled local execution. Comparison is not semver-correct and can violate equality laws; long input can be expensive. safe-to-deploy remains held. See lane A review."
```

## Related, separate consumer finding: system-deps upper bound

In the reviewed cached sources for system-deps 6.2.2 (`src/lib.rs:892-894`), 7.0.8 (`:929-930`) and 9.0.0 (`:1120-1121`), `call_build_internal` compares `version_compare::compare(&lib.version, max) == Ok(Cmp::Ge)`. The compared `compare` API only returns `Lt`, `Eq` or `Gt`; `Ge` is never a possible successful result. The probe demonstrates `compare("2", "1") == Ok(Gt)` and `compare_to("2", "1", Ge) == Ok(true)`. Consequently that particular internal-build upper-bound branch cannot reject an equal or greater version. This is a source-derived consumer finding, **not** a full audit or execution of system-deps and not evidence that the normal pkg-config path is affected. Whether the product activates an internal-build callback and the security impact are not established.
