# Independent cross-review of lane A

Reviewer: Codex AI-assisted, 2026-11-03. This cross-review concerns the exact
packages and criteria below; it neither changes canonical audits nor certifies
the complete graph. The primary review is `../../lane-a/review.md`, SHA-256
`d77a65649ed69b6e72a9d852398fb5c65eaeb3f2b04ca44a4ac3ea922aa9c309`.

The verified App snapshot is `f6ebd8dc194f49b3ceb142dc7c5950d8a1f1ed3e`, lock
SHA-256 `8107432e39037dbd710de6b78bdb706d8aebdb63ab0d4175a94d487ee3e153c6`.
I independently downloaded both official crates.io archives, checked their
hashes against the App lock and cached archives, and byte-compared all
packaged source files: 13 for `version_check`, 15 for `version-compare`.
`identity.json` records each file's hash. I independently read the Rust source,
tests, examples, manifests, and shipped CI configuration. Dependencies and
consumers were inspected only as bounded context where described below.

## version_check 0.9.5

Archive SHA-256:
`0b928f33d975fc6ad9f86c8f283853ad26bdd5b10b7f1542aa2fa15e2289105a`.
I support the primary recommendation: exact-version **safe-to-deploy**, which
implies `safe-to-run`, under the ordinary trusted compiler/build environment.

The package contains only its library and tests, no build script, proc macro,
dependencies, native code or unsafe block. `src/lib.rs:158-164` executes the
`RUSTC` value or PATH `rustc` directly with fixed version arguments, then
parses stdout. This deliberately trusts the configured executable and its
output; the process is not sandboxed and its stdout is collected into memory.
No shell, network or unrelated file operation is initiated by the library.
The CI workflow is only executed if separately enabled. The package's tests
read static fixtures only when available or forced; they were excluded from
the published archive.

The source and independently rerun primary probe confirm that the child exit
status is not checked, `Date::parse` accepts non-Gregorian combinations within
month/day bounds, `Channel::parse` treats arbitrary non-hyphenated text as
stable, and `Version` deliberately ignores the release suffix. Feature checks
use compiler channel and flags rather than compiling a feature probe. These
are real correctness/trust limits, not a demonstrated attacker-reachable severe
production vulnerability when the compiler and build environment are trusted.
Callers must not treat these APIs as independently authenticated compiler or
feature attestation.

Rust 1.94.0 rebuilt the archive-matched source and passed 10 packaged unit
tests; the static-fixture compatibility test returned early because the
archive excludes those fixtures. The nonzero fake-compiler/date/channel probe
also reproduced the primary observations. No claim is made for every compiler
wrapper, environment, platform, or deployed program.

## version-compare 0.2.1

Archive SHA-256:
`03c2856837ef78f57382f06b2b8563a2f512f7185d732608fd9176cb3b8edf0e`.
I support **safe-to-run** only; **safe-to-deploy remains HOLD**. The package
has no build script, proc macro, dependencies, unsafe blocks or ambient
network/filesystem/process actions; its two explicit examples only compare
fixed strings and print results. It allocates based on caller-provided text;
`src/version.rs:365-372` can repeatedly rescan long digit-prefixed tokens, so
callers must bound untrusted input resources.

The default comparison's `Number` versus `Text` fallback is a no-op
(`src/version.rs:441-453`); `PartialEq`/`PartialOrd` use that comparison and
the left-hand manifest (`241-249`, `300-311`). Independently rebuilt probes
confirm `1.0.0-1 == 1.0.0-alpha`, nontransitive equality among `1.2.1`,
`1.alpha.1`, and `1.3.1`, and asymmetric equality when mixing GNU and default
manifests. Build metadata is also ordered rather than ignored. These conflict
with the README's semver claim and ordinary equality expectations; a program
that treats a comparison as a security threshold can make a wrong decision.
No actual product exploitation, attacker control, or memory unsafety is shown.
The deploy criterion remains held because the impact for reasonable uses has
not been resolved, rather than treating every semantic defect as an exploit.

All 30 shipped unit tests passed in the independent Rust 1.94.0 run; they do not
cover the listed cases. The counterexamples and exact commands/output are in
`cross-probe-results.json`.

## Separate, bounded system-deps observation

The current lock contains `system-deps` 6.2.2, 7.0.8 and 9.0.0, each depending
on this version comparator. I verified each cached archive against the App
lock and byte-matched the inspected `src/lib.rs` in
`system-deps-context.json`. In their respective internal-build paths at lines
893, 930 and 1121, each tests `compare(&lib.version, max) == Ok(Cmp::Ge)`.
`compare` can return only `Lt`, `Eq`, or `Gt`; therefore that specific branch
cannot reject any result as `Ge`. The probe confirms that `compare("2", "1")`
is `Gt` while `compare_to("2", "1", Ge)` is true. This is a finding in the
consumer's separate internal-build branch, not a full consumer review, a
demonstrated active product path, or evidence about ordinary pkg-config checks.

## Evidence

* `identity.json`: SHA-256 `288eaefd1332c4d3d9314de088fa171016e95b6348c94485c7243bb80bd7b542`.
* `cross-probe-results.json`: SHA-256 `875bdc5fd93a6cff124be113698081ec3d9e44b9c06a851d3f027fcdb99aef09`.
* `system-deps-context.json`: SHA-256 `45ccf8c6d82f9e4e1babc013c1c5885ecee37def1ebbbaf1856c9a7eb7488909`.

`run_cross.py` records exact commands, outputs and input hashes. The checks are
bounded host-source tests, not device execution or all-platform acceptance.
