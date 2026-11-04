# is_terminal_polyfill 1.70.2 — exact full-source review

Reviewer: Codex AI-assisted, 2026-11-04. This full package review supports
`safe-to-deploy` (and therefore `safe-to-run`) for this precise registry
version. The independently performed source and behavioral cross-review is
in [root-cross-review.md](root-cross-review.md). The conclusion is limited to
this crate: it does not certify the standard library, compiler, operating
systems, consumers, complete dependency graph, or product.

## Source identity and dependency graph

The package ID is
`registry+https://github.com/rust-lang/crates.io-index#is_terminal_polyfill@1.70.2`.
The downloaded [crates.io archive](https://static.crates.io/crates/is_terminal_polyfill/is_terminal_polyfill-1.70.2.crate),
local Cargo archive, App lock, and captured
[crates.io index](https://index.crates.io/is/_t/is_terminal_polyfill) all
agree on SHA-256
`a6cb138bb79a146c1bd460005623e142ef0181e3d0219cb493e02f7d08a35695`.
The captured index lists no dependencies, an empty default feature, and
`yanked = false` at retrieval; this is not a claim about future index state.
All eight regular archive files, with no duplicate or traversal paths, match
the extracted Cargo cache byte for byte. The only extra cache file is Cargo's
`.cargo-ok` marker. [identity.json](identity.json) records per-file SHA-256,
Git blob hashes, modes and sizes.

The archive VCS metadata specifies upstream commit
[`9ed4c24ea4e9202cb38a6564368252ed35197a08`](https://github.com/polyfill-rs/is_terminal_polyfill/commit/9ed4c24ea4e9202cb38a6564368252ed35197a08).
The exact public Git object was fetched separately and its object SHA-1
recomputed; it identifies root tree `a051059a89eaee3df42072cd9e440f8c17ddc12c`.
The captured recursive Git tree is reconstructed and hashed, including all
four tree objects, and the six upstream-origin archive files match their
respective Git blobs. `Cargo.toml.orig` maps to the upstream `Cargo.toml`;
the normalized manifest and `.cargo_vcs_info.json` are Cargo-generated and
are checked against the archive and cache, rather than asserted to be Git
blobs. The commit is unsigned; these checks establish matching public object
identity, not author authentication. Run `python3 verify-identity.py` from
any working directory (Python 3.12 or newer); `--cache <directory>` also
checks the extracted Cargo cache.

The reviewed App is Git commit `ba4f9905485159c652786772ee964cb0fbab2169`,
its `Cargo.lock` SHA-256 is
`0fa2e3751d53c3c3e6d455aad3cb5ecaf826909f6314ecd8caf8d005f998f015`,
and its Harness Git pin is `ae5a65b1f4816cd26a06d462573ebc9841f14f96`.
`cargo metadata --format-version 1 --all-features --locked --frozen`
resolves 1,052 packages and this package's sole direct reverse parent as
`anstream 0.6.21`, a normal dependency edge. It resolves the empty `default`
feature and no child dependencies; [graph-summary.json](graph-summary.json)
records the exact IDs. Cached anstream source calls the polyfill for stdout,
stderr and their locks. This contextual observation is not a source audit of
anstream or proof every platform executes the edge.

## Complete source review

Read every packaged file: `src/lib.rs`, normalized and original manifests,
package lock, VCS metadata, README, and MIT and Apache licenses. The manifest
sets Rust 1.70 as minimum, declares a single edition-2021 library, explicitly
disables auto-discovered targets and build script, has an empty default
feature and no dependencies. Its release-tool metadata does not execute as a
Cargo dependency build. The package lock contains only the package itself;
it is not the consuming App lock.

`src/lib.rs` defines a public sealed `IsTerminal` trait and a macro generating
seven implementations: `File`, `Stdin`, `StdinLock`, `Stdout`,
`StdoutLock`, `Stderr`, and `StderrLock`. Each directly returns
`std::io::IsTerminal::is_terminal(self)` on the same borrowed value. The
private supertrait prevents external implementations. There is no local
parsing, `unsafe`, FFI, native code, procedural macro, binary, filesystem
operation, environment read, process execution, network access, mutation or
allocation in the package. The wrapper adds no independent detection logic;
its result and any platform heuristics/errors are those of Rust's standard
library. The docs explicitly say unsupported platforms and unexpected
errors produce false and mention Windows pseudo-terminal heuristics; exact
platform behavior can change with the standard library. Callers should not
interpret this boolean as authentication or a general security boundary.
No serious package-local vulnerability was found under documented use.

## Validation and limits

The exact package compiled with Rust/Cargo 1.94.0 on macOS arm64 using
`cargo test --locked --frozen --all-features` and an isolated Cargo home and
target. It contains **zero** packaged unit and documentation tests, so that
successful command is compilation evidence, not a claim of upstream test
coverage. [run-probes.py](run-probes.py) builds the exact archive source and
[probe.rs](probe.rs), checks all seven implementations against std for a
regular file and the six standard stream/lock handles with redirected pipes
and then a real pseudo-terminal. The regular file and pipes returned false;
the six PTY handles returned true and agreed with std. A temporary inverted
implementation was caught on the file, and a temporary always-false
implementation was caught on PTY stdin. [probe.log](probe.log) records the
run. These controls establish sensitivity to those regressions only.

An initial probe failed before compilation because macOS resolved the
temporary directory's symlink while the comparison path remained unresolved;
the saved `probe-path-failure.log` and corrected script preserve that history.
The first corrected run passed but did not capture PTY output after the last
slave was closed on macOS; `probe-uncaptured-pty.log` retains it, and the
final run reads the output before closing the slave and asserts the expected
terminal states. The independent reviewer separately checked real PTY and
`/dev/null` File behavior. Neither review executed Windows, Linux, Rust 1.70,
all operating-system edge cases, or an actual product deployment.

The actual App baseline `cargo vet check --locked --frozen
--no-minimize-exemptions --no-registry-suggestions` with official cargo-vet
0.10.3 reports **819** unmet package criteria, including this exact version's
`safe-to-deploy`; see [before-vet.json](before-vet.json) and
[before-context.json](before-context.json). The first direct binary
invocation failed because Cargo had not supplied `$CARGO`, and is retained as
[direct-invocation-error.json](direct-invocation-error.json). The subsequent
standard `cargo vet` invocation produced the baseline. No wildcard,
exemption, publisher-trust entry, or other criterion change is justified by
this review. Overall source-review closure remains a separate gate.
