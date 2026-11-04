# Independent cross-review: diff 0.1.13 and gethostname 1.1.0

Reviewer: Codex AI-assisted, 2026-11-03. This review covers the exact package
contents and criteria below, with the stated environment and dependency
boundaries. It recommends registrations; it does not edit canonical audits,
claim that the entire dependency graph has passed, or establish product
runtime, target, or security acceptance.

## Verdict and identity

| Exact package | SHA-256 of official `.crate` | Recommended criterion | Initial review SHA-256 |
| --- | --- | --- | --- |
| `diff 0.1.13` | `56254986775e3233ffa9c4d7d3faaf6d36a2c09d30b20687e9f88bc8bafc16c8` | **safe-to-run** only | `4266434dd83b15fe427c892214b3bd9a2ee7cf1a1145967552354eeb7a1ef4c9` |
| `gethostname 1.1.0` | `1bd49230192a3797a9a4d6abe9b3eed6f7fa4c8a8a4947977c6f80025f92cbd8` | **safe-to-deploy** (implies safe-to-run) | `74956558b70b797324841f51472a10c2e2e0a94890ce96d5b559fde91b4f70f1` |

The initial review files are, respectively,
`../next-lane-b/diff-0.1.13.md` and `../next-lane-b/gethostname-1.1.0.md`.
Their `proposed-audits.toml` has SHA-256
`c16da2634f6213a1385b31bc8e1081648431bc48305fd27c3f2101193c194d02`.
The proposal's exact package, versions, criteria, and review hashes match the
files reviewed. These recommendations are based on source behavior under the
[cargo-vet built-in criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html),
not on the archive hashes alone. The recording notes should retain the
test-side-effect and resource caveats for `diff`, and the API panic/privacy
and separate-dependency caveats for `gethostname`.

An independently fetched crates.io archive for each version matched the App
lock, separately fetched crates.io sparse index, and local `.crate` archive.
Every archive member was an ordinary file and matched the extracted Cargo
source byte for byte: 16/16 for `diff`, 11/11 for `gethostname`. The only extra
extracted-cache marker was `.cargo-ok`. `identity.json` records each member's
hash, mode and graph parents. At verification App HEAD was
`d4794f639be55d7cf7659be784d8b2f6608b9abf`, its `Cargo.lock` SHA-256 was
`221db470e0c34f2842b439ce0d4b2530f2093e5292871de6dde1d2f1af528b52`, and
the 1,009-package, all-features metadata snapshot SHA-256 was
`ad6283c5ee42e875a3c542bd268a319266c5162c647e74290a397ef1ee1054da`.
This graph identifies packages; it is not proof all targets execute them.

I read both normalized and original manifests and all packaged executable Rust
source, tests, examples, benchmarks, and relevant CI or manual command files.
The archive inventory and metadata files were checked for identity and target
declarations. This is an independent source cross-review of the two packages,
not a complete audit of their dependencies or consumers. `context-identity.json`
authenticates the specific dependency/consumer source files examined below.

## `diff 0.1.13`: safe-to-run, with recorded discretion

The manifest declares a library, with no build script, proc macro or normal
dependencies. `src/lib.rs:12-49` accepts caller-supplied values and exposes
slice, line and character differences; `51-135` trims equal edges, fills a
longest-common-subsequence table, and reconstructs the result; `137-167` uses
ordinary checked `Vec` indexing. The production package contains no unsafe
blocks, ambient filesystem/network/process activity, or hidden input source.
Caller-defined `PartialEq` can itself have effects and is outside this audit.

For unmatched dimensions `m` and `n`, the table needs approximately
`4*(m+1)*(n+1)` bytes and `O(m*n)` comparisons; other input/output allocations
also occur. Neither dimension is bounded by the crate. Callers accepting large
or untrusted data must impose appropriate input/resource limits. Extreme sizes
can overflow table/index or `u32` score arithmetic, panic, allocate excessively,
or yield an incorrect result; safe indexing does not make the result reliable.
`lines` uses `str::lines`, so CRLF and LF compare as the same line: it is not a
byte-exact equality check. The currently identified `pretty_assertions 1.4.1`
consumer compares the actual values before producing a diagnostic diff; the
graph selects it as a development dependency of `config-requirements`. That
bounded consumer observation is not an audit of every downstream caller.

Packaged `tests/tests.rs:235-303` reads `tests/data/gitignores.txt`; for each of
two expected output fixtures, it writes generated output if `read_to_string`
fails, including on invalid UTF-8. `std::fs::write` can truncate an existing
file and follows symlinks. Normal Cargo tests run with the package root as
their working directory, and all three archive fixtures are regular, valid
UTF-8 files (independently checked). A changed fixture, hostile symlink, or
nonstandard working directory can therefore make the fallback affect an
unexpected path. This is an explicit test-side-effect discretion, not a claim
of zero filesystem writes. The benchmark reads a fixture, and the example
prints output; the packaged CI workflow and test/build tools are separate
actions/dependencies. Ordinary use as a dependency does not run those tests.

Rust 1.94.0 independently rebuilt the archive-matched library and the proposed
bounded brute-force oracle test. Both tests passed: all 16,129 pairs of binary
sequences of lengths 0–6 reconstructed correctly and had maximal common
subsequences, and Unicode, final newline and CRLF cases passed. The oracle is
not the package's dynamic-programming implementation. This is finite host
coverage, not a resource-exhaustion or universal correctness proof. Exact
commands, fixture inspection and output are in `probe-results.json` and
`run_probes.py`. On the evidence above, I support only the exact-version
`safe-to-run` registration, with the noted test and resource boundaries.

## `gethostname 1.1.0`: safe-to-deploy for this package

The normalized manifest sets `build = false`, disables automatic targets and
declares one library; `src/lib.rs` is the only packaged Rust file. There is no
build script, proc macro, production network call, subprocess, or filesystem
write. Unix uses `rustix::system::uname().nodename().to_bytes()` and copies
bytes into `OsString` (`src/lib.rs:49-54`), preserving non-UTF-8 bytes. This
data is supplied by the OS and is not necessarily a resolvable hostname,
trusted principal, or suitable log field; the caller owns those decisions.

On Windows (`61-111`) the crate declares `GetComputerNameExW` through
`windows-link` with the `system` ABI, `i32` format and return, writable `u16`
buffer and `u32` size pointer; enum value 5 identifies the physical DNS
hostname. The first call queries with a null buffer and zero length; the code
asserts the returned capacity is positive, allocates that many initialized
UTF-16 units and supplies the matching length to the second call. It checks
failure, requires the returned character count to equal allocated length minus
the terminator, then scans within the allocated slice before conversion. Under
the [Windows API buffer contract](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-getcomputernameexw)
and [format enumeration](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/ne-sysinfoapi-computer_name_format),
the pointers and bounded read are justified; the comments' “zero byte” means
a zero `u16` code unit here. An API error, changed name, or unexpected length
can panic. No claim is made for targets satisfying neither `unix` nor `windows`.

The packaged unit test deliberately starts `hostname` or `uname -n` found via
`PATH`; it assumes a trusted test environment. The `justfile` contains explicit
manual commands including release, and is not an automatic Cargo build script.
The observed Linux/X11 path is `arboard -> x11rb -> gethostname`; x11rb uses
the hostname for X resource and local Xauthority context, with its own
invalid-UTF-8 behavior. This is contextual source inspection, not a separate
consumer audit or proof that every target reaches the path.

Using Rust 1.94.0, I cross-compiled the exact `windows-link` and `gethostname`
source for `x86_64-pc-windows-gnu`, linked a minimal caller, and inspected the
PE imports: `kernel32.dll` imports `GetComputerNameExW`. Commands and output
are in `probe-results.json` and `windows_objdump.log`. The executable was not
run; these checks do not validate Windows behavior, API failures, races, or
other Windows targets. Subject to the stated OS and independently audited
dependency contracts, I support exact-version `safe-to-deploy` for
`gethostname 1.1.0`.

## Separate dependency concern: `rustix 1.1.5`

This is **not** a finding in `gethostname`'s own source and the above package
recommendation does not certify `rustix`. Its authenticated Linux-raw uname
implementation initializes `MaybeUninit<RawUname>`, invokes the syscall,
passes its return to `ret_infallible`, then calls `assume_init`
(`src/backend/linux_raw/system/syscalls.rs:18-23`). In builds without debug
assertions, `ret_infallible` drops the return without checking for errors
(`src/backend/linux_raw/conv.rs:909-915`); the libc backend has a comparable
pattern. Linux [seccomp filter semantics](https://docs.kernel.org/4.20/userspace-api/seccomp_filter.html)
allow a filter to return an error without executing the syscall, which can
leave the output unwritten. Under that condition and without debug assertions,
the source appears to reach `assume_init` on uninitialized data, violating
the [Rust initialization contract](https://doc.rust-lang.org/core/mem/union.MaybeUninit.html).
This is a conditional, source-derived dependency concern requiring its own
review; I did not install seccomp, reproduce it at runtime, establish attacker
control, or establish a product exploit. The inspected Harness seccomp filter
at HEAD `a2060969a2e027304f6bb8baec7abdfa91639177` defaults to allow and only
denies matching AF_UNIX socket/socketpair calls, so **that filter does not
deny uname**. External environment filters may differ. `rustix`, `windows-link`,
and their transitive dependencies need their own criteria evidence before the
whole graph can pass; an audit of `gethostname` does not close those gaps.

Evidence hashes: `identity.json`
`e631a3a290609e5598e2be060863d90d3621e8fbb6fa21eb734dab5170c89640`;
`context-identity.json`
`6c319ba428e9ee922700bfbf2d236b20ae10df6e984b74f00e808455613e6050`;
`probe-results.json`
`c37ff991bfb58995428ffd052cf3e023df6f9d4f41b40e2b9d81ce048d6b5e2b`;
`windows_objdump.log`
`5f02045f9c904a512256852792631a4e751925043dcf1bdecc486fc03d9513ed`.
